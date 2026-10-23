use super::{
    defer_retry_failures, execute_resource_kill_transaction, kill_resource_group,
    reset_defer_retry_failures, should_finalize_kill, verify_kill_settled,
    PTY_ROUTE_RETRY_ATTEMPTS,
};
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use crate::pty::manager::PtyManager;
use crate::resource_monitor::registry::{
    ProcessTreeBackend, ResourceError, ResourceLaunchRegistration,
};
use crate::resource_monitor::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, ResourceGroupState,
    ResourceKillReason, ResourceKillResult, ResourceLaunchMetadata, ResourceLimits,
    TerminateOutcome,
};
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::SessionManager;
use crate::session::selection::{
    CriticalAdmissionKind, SelectionCoordinator, SelectionMode, SelectionSource,
    SelectionTransaction, TrustedResourceIntent, WatchdogKillOutcome,
};
use crate::session::session::SessionStatus;
use crate::web::broadcast::WsBroadcaster;
use crate::DetachedSessionsState;
use futures::future::BoxFuture;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tauri::{Listener, Manager};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default)]
struct ResourcePtyBackend {
    live: Mutex<HashSet<Uuid>>,
    terminate_count: AtomicUsize,
    kill_count: AtomicUsize,
    fail_kill: AtomicBool,
    fail_kill_remaining: AtomicUsize,
    kill_fail_count: AtomicUsize,
}

#[derive(Default)]
struct UnsupportedKillBackend {
    observe_tree_calls: AtomicUsize,
    observe_identity_calls: AtomicUsize,
    terminate_verified_calls: AtomicUsize,
    current_process_memory_calls: AtomicUsize,
}

impl ProcessTreeBackend for UnsupportedKillBackend {
    fn supports_process_tree_enforcement(&self) -> bool {
        false
    }

    fn observe_tree(&self, _root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ObservedProcessTree::default())
    }

    fn observe_identity(&self, _pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }

    fn terminate_verified(
        &self,
        _process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError> {
        self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
        Ok(TerminateOutcome::AlreadyGone)
    }

    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        self.current_process_memory_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(ProcessMemory::default())
    }
}

impl ResourcePtyBackend {
    fn set_live(&self, id: Uuid) {
        self.live.lock().unwrap().insert(id);
    }

    /// Fixture: when set, `kill` returns Err while `has_session` stays true,
    /// simulating a PTY teardown failure with a still-live route (S1 defer
    /// branch).
    fn set_fail_kill(&self, fail: bool) {
        self.fail_kill.store(fail, Ordering::SeqCst);
    }

    /// Fixture: fail the next `n` `kill` calls then succeed, so the deferred
    /// teardown retry loop observes a bounded number of failures before the route
    /// clears (round-3 test 19).
    fn set_fail_kill_remaining(&self, n: usize) {
        self.fail_kill_remaining.store(n, Ordering::SeqCst);
    }
}

impl PtyBackend for ResourcePtyBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(&self, spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.set_live(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.has_session(id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        self.write(
            &crate::pty::manager::BackendWriteAuthority::for_backend_test(),
            id,
            &[],
        )
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.kill_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_kill_remaining.load(Ordering::SeqCst) > 0 {
            self.fail_kill_remaining.fetch_sub(1, Ordering::SeqCst);
            self.kill_fail_count.fetch_add(1, Ordering::SeqCst);
            return Err(crate::errors::AppError::PtyError(
                "synthetic kill failure".into(),
            ));
        }
        if self.fail_kill.load(Ordering::SeqCst) {
            self.kill_fail_count.fetch_add(1, Ordering::SeqCst);
            return Err(crate::errors::AppError::PtyError(
                "synthetic kill failure".into(),
            ));
        }
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.has_session(id).then_some((120, 30))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        if self.has_session(id) {
            self.terminate_count.fetch_add(1, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

fn assert_unsupported_kill_result(result: &ResourceKillResult, session_id: Uuid) {
    assert_eq!(result.session_id, session_id.to_string());
    assert_eq!(result.state, ResourceGroupState::Running);
    assert!(result.killed_processes.is_empty());
    assert!(!result.quarantined);
    assert_eq!(
        result.message,
        "resource monitor enforcement is unsupported on this platform; no process was killed"
    );
    assert!(!result.blocked_by_security);
    assert!(!result.finalized);
}

/// A backend whose `observe_tree` BLOCKS (once armed) until the test releases it, so
/// a `kill_group` call parks the group in `Terminating`. Registration runs before
/// arming, so it observes the live root; after release the root reads as gone, so the
/// blocked kill settles cleanly to `Terminated`.
struct GatedBackend {
    root: ProcessIdentity,
    armed: AtomicBool,
    entered: AtomicBool,
    gate: (Mutex<bool>, Condvar),
}

impl GatedBackend {
    fn new(root: ProcessIdentity) -> Self {
        Self {
            root,
            armed: AtomicBool::new(false),
            entered: AtomicBool::new(false),
            gate: (Mutex::new(false), Condvar::new()),
        }
    }
    fn released(&self) -> bool {
        *self.gate.0.lock().unwrap()
    }
    fn release(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}

impl ProcessTreeBackend for GatedBackend {
    fn observe_tree(&self, _root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        if self.armed.load(Ordering::SeqCst) {
            self.entered.store(true, Ordering::SeqCst);
            let mut released = self.gate.0.lock().unwrap();
            while !*released {
                released = self.gate.1.wait(released).unwrap();
            }
            // Released: tree is gone -> the reaper sees AlreadyGone -> Terminated.
            Ok(ObservedProcessTree {
                processes: Vec::new(),
                errors: vec![format!(
                    "root pid {} was not in process snapshot",
                    self.root.pid
                )],
            })
        } else {
            Ok(ObservedProcessTree {
                processes: vec![ObservedProcess {
                    identity: self.root,
                    parent_pid: None,
                    parent_identity: None,
                    exe_name: "root".to_string(),
                    depth: 0,
                    private_bytes: None,
                    working_set_bytes: None,
                    cpu_percent: None,
                    kill_allowed: true,
                }],
                errors: Vec::new(),
            })
        }
    }
    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        if pid == self.root.pid && !self.released() {
            Ok(Some(self.root))
        } else {
            Ok(None)
        }
    }
    fn terminate_verified(
        &self,
        _process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError> {
        Ok(TerminateOutcome::AlreadyGone)
    }
    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        Ok(ProcessMemory::default())
    }
}

fn test_limits() -> ResourceLimits {
    ResourceLimits {
        monitor_enabled: true,
        max_concurrent_agent_processes: 4,
        group_warn_private_bytes: 100,
        group_kill_private_bytes: 200,
        process_kill_private_bytes: 200,
    }
}

fn register_running(state: &ResourceMonitorState, id: Uuid, root: ProcessIdentity) {
    let permit = state
        .try_reserve_agent_slot(test_limits())
        .unwrap()
        .unwrap();
    let mut reg = ResourceLaunchRegistration::new(
        state.clone(),
        permit,
        ResourceLaunchMetadata {
            session_id: id,
            name: "agent".to_string(),
            agent_id: None,
            agent_label: None,
            workgroup: None,
            agent: None,
            project: None,
        },
    );
    reg.register_root_pid(root.pid).unwrap();
}

mod kill_errors;
mod quarantine;
mod serialization;
