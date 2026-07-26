use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::resource_monitor::types::{
    ResourceGroupState, ResourceKillReason, ResourceKillRequest, ResourceKillResult,
    ResourceLimits, ResourceSnapshot,
};
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::{CommitDecision, LifecycleMutations};
use crate::session::selection::{
    SelectionCause, SelectionCoordinator, SelectionSource, SelectionTransaction,
    TrustedResourceIntent,
};

#[tauri::command]
pub async fn get_resource_snapshot(
    monitor: State<'_, Arc<ResourceMonitorState>>,
    settings: State<'_, SettingsState>,
) -> Result<ResourceSnapshot, String> {
    let cfg = settings.read().await.clone();
    let limits = ResourceLimits::from(&cfg);
    let monitor = Arc::clone(&monitor);
    tokio::task::spawn_blocking(move || monitor.snapshot(limits))
        .await
        .map_err(|e| e.to_string())
}

/// #647 A: route the manual RM Kill through the per-agent Job Object (the only
/// mechanism that bypasses per-handle `PROCESS_TERMINATE` stripping by an AV/EDR),
/// then VERIFY the kill via the reaper before tearing the session down.
///
/// Fire-then-verify-then-settle (grinch HIGH): the job is fired by the pure
/// `terminate_job_for_session` (which keeps the instance/job). Verification re-runs
/// `kill_group` until the group leaves the transient `Terminating` a concurrent kill
/// (e.g. a watchdog retry) may briefly own, so an in-flight kill is not mistaken for a
/// verified one. Only a settled, verified `Terminated` runs the full `kill` cleanup,
/// flips the tile to `Exited`, and sets `finalized = true`. A `Quarantined` (or a
/// budget-exhausted `Terminating`) result leaves the instance/job intact for a
/// Force/Retry and never marks a possibly-alive agent `Exited`.
#[tauri::command]
pub async fn kill_resource_group<R: tauri::Runtime>(
    app: AppHandle<R>,
    request: ResourceKillRequest,
    monitor: State<'_, Arc<ResourceMonitorState>>,
    _pty_mgr: State<'_, Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>,
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>,
) -> Result<ResourceKillResult, String> {
    let session_id = Uuid::parse_str(&request.session_id).map_err(|e| e.to_string())?;
    if request.reason != ResourceKillReason::User {
        return Err("resourceMonitor user command requires reason=user".to_string());
    }
    if !monitor.supports_process_tree_enforcement() {
        return Ok(unsupported_kill_result(session_id));
    }
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator
        .resource_kill(session_id, TrustedResourceIntent::User)
        .await
}

pub(crate) async fn execute_resource_kill_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    intent: TrustedResourceIntent,
) -> Result<ResourceKillResult, String> {
    let reason = match intent {
        TrustedResourceIntent::User => ResourceKillReason::User,
        TrustedResourceIntent::Watchdog => ResourceKillReason::Watchdog,
    };
    let monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if !monitor.supports_process_tree_enforcement() {
        return Ok(unsupported_kill_result(session_id));
    }
    let pty_mgr = transaction
        .app()
        .state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>();

    // Fire the Job Object FIRST (pure: keeps the instance/job for a Retry). The
    // std-Mutex guard is dropped before any await.
    let job_fired = {
        pty_mgr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .terminate_job_for_session(session_id)
    };
    log::info!(
        "[resource-monitor] job-fire session={} job_present={}",
        session_id,
        job_fired
    );

    // Verify + account, settling past a concurrent kill's transient `Terminating`.
    let mut result = verify_kill_settled(
        Arc::clone(&monitor),
        session_id,
        reason,
        SETTLE_BUDGET,
        SETTLE_POLL,
    )
    .await?;

    if should_finalize_kill(result.state) {
        // Verified dead: full PTY cleanup (drops the job handle = KILL_ON_JOB_CLOSE
        // backstop, reaps the child, tears down idle/git/watchers/parser), then flip
        // the tile to Exited. This is the only path that consumes the job.
        {
            if let Err(error) = pty_mgr
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .kill(session_id)
            {
                log::warn!(
                    "[resource-monitor] PTY teardown failed after verified kill session={}: {}",
                    session_id,
                    error
                );
            }
        }
        transaction
            .app()
            .state::<crate::DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        let window_label = format!("terminal-{}", session_id.to_string().replace('-', ""));
        if let Some(window) = transaction.app().get_webview_window(&window_label) {
            if let Err(error) = window.destroy() {
                log::warn!(
                    "[resource-monitor] detached window close failed session={}: {}",
                    session_id,
                    error
                );
            }
        }
        let snapshot = transaction.aggregate_snapshot().await;
        let decision = if snapshot.selection.id() == Some(session_id) {
            CommitDecision::Clear
        } else {
            CommitDecision::Keep
        };
        let mut mutations = LifecycleMutations::default();
        mutations.mark_exited(session_id, 0);
        let committed = transaction
            .commit(decision, SelectionCause::ResourceMonitor(intent), mutations)
            .await?;
        if !committed.changed_rows.is_empty() {
            transaction
                .persist(SelectionSource::ResourceMonitor, Some(session_id))
                .await;
            transaction.publish_destroyed(session_id);
            for row in &committed.changed_rows {
                if row.id == session_id.to_string() {
                    transaction.publish_created(row);
                }
            }
            for cleared in &committed.cleared_raise_hand_ids {
                transaction.publish_communication_cleared(*cleared);
            }
            if let Some(selection) = committed.selection.as_ref() {
                transaction.publish_selection(selection);
            }
        }
        result.finalized = true;
    } else {
        // NOT verified dead (kernel-AV residual, no job, or a concurrent kill that did
        // not settle within the budget). Keep the instance/job so Force/Retry re-fires
        // the durable kill; do NOT mark Exited. result.finalized stays false so the FE
        // keeps the modal open with the per-PID errors + blocked_by_security.
        log::warn!(
            "[resource-monitor] kill session={} not finalized (state={:?} job_fired={}): {}",
            session_id,
            result.state,
            job_fired,
            result.message
        );
    }

    Ok(result)
}

fn unsupported_kill_result(session_id: Uuid) -> ResourceKillResult {
    ResourceKillResult {
        session_id: session_id.to_string(),
        state: ResourceGroupState::Running,
        killed_processes: Vec::new(),
        quarantined: false,
        message:
            "resource monitor enforcement is unsupported on this platform; no process was killed"
                .to_string(),
        blocked_by_security: false,
        finalized: false,
    }
}

/// Upper bound on how long `verify_kill_settled` waits for a concurrent kill to leave
/// `Terminating`. After the job fire the concurrent reaper sees an AlreadyGone tree, so
/// the common case settles in one or two ~75ms polls. The budget sits ABOVE the
/// worst-case single `kill_group` latency (a ~2s `WaitForSingleObject` in
/// `terminate_verified`), so a genuinely in-flight kill is awaited to completion rather
/// than misreported as not-finalized; the cap only trips on a pathologically stuck
/// verify, after which we return a not-finalized result the FE surfaces for a retry.
const SETTLE_BUDGET: Duration = Duration::from_millis(2500);
/// Re-poll interval while a concurrent kill owns the `Terminating` state.
const SETTLE_POLL: Duration = Duration::from_millis(75);

/// #647 (Step 7): re-run `kill_group` until the group leaves `Terminating` or the
/// budget elapses. `kill_group` is idempotent under concurrency: while another call
/// owns `Terminating` it early-returns that state with no side effects; once the owner
/// settles to `Terminated` it early-returns `Terminated`; a settled `Quarantined`
/// re-runs the reaper (now seeing the job-killed tree as AlreadyGone -> `Terminated`).
/// So a single Force-kill click reliably reaches a verified outcome instead of bailing
/// on the transient `Terminating` and silently leaving a dead tree with a Running tile.
async fn verify_kill_settled(
    monitor: Arc<ResourceMonitorState>,
    session_id: Uuid,
    reason: ResourceKillReason,
    budget: Duration,
    poll: Duration,
) -> Result<ResourceKillResult, String> {
    let deadline = Instant::now() + budget;
    loop {
        let monitor_c = Arc::clone(&monitor);
        let result = tokio::task::spawn_blocking(move || monitor_c.kill_group(session_id, reason))
            .await
            .map_err(|e| e.to_string())??;
        if result.state != ResourceGroupState::Terminating || Instant::now() >= deadline {
            return Ok(result);
        }
        tokio::time::sleep(poll).await;
    }
}

/// #647 A (grinch HIGH-1 invariant): finalize the kill (full PTY teardown + flip the
/// tile to `Exited`) ONLY on a result whose state is the verified `Terminated`. This is
/// STRICTER than `!quarantined`: `kill_group`'s early return for an already-`Terminating`
/// group (a concurrent kill, e.g. a watchdog tick still mid observe/verify) also reports
/// `quarantined == false`, but that kill is NOT yet verified, so finalizing on it could
/// tear down the job and mark a possibly-alive agent `Exited`. Gating on `Terminated`
/// lets the in-flight kill own the outcome; `Quarantined`/`Terminating`/`Running` never
/// finalize.
fn should_finalize_kill(state: ResourceGroupState) -> bool {
    matches!(state, ResourceGroupState::Terminated)
}

#[cfg(test)]
mod tests {
    use super::{
        execute_resource_kill_transaction, kill_resource_group, should_finalize_kill,
        verify_kill_settled,
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

        fn observe_tree(
            &self,
            _root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
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
    }

    impl PtyBackend for ResourcePtyBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: BackendSpawnSpec,
        ) -> BoxFuture<'_, Result<(), crate::errors::AppError>> {
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

    #[tokio::test]
    async fn unsupported_backend_manual_kill_is_side_effect_free() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "/tmp/unsupported-resource-kill".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let pty_backend = Arc::new(ResourcePtyBackend::default());
        pty_backend.set_live(session.id);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let process_backend = Arc::new(UnsupportedKillBackend::default());
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            process_backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        let before_selection = manager.read().await.selection_payload().await;

        let app = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![kill_resource_group])
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(Arc::clone(&monitor))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build unsupported resource-kill app");
        assert!(app.try_state::<SelectionCoordinator>().is_none());
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let public_response = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "kill_resource_group".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                    "request": {
                        "sessionId": session.id,
                        "reason": "user",
                    }
                })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .expect("unsupported public kill returns success")
        .deserialize::<ResourceKillResult>()
        .unwrap();
        assert_unsupported_kill_result(&public_response, session.id);

        let transaction = SelectionTransaction::for_test(app.handle().clone());
        let inner_response = execute_resource_kill_transaction(
            &transaction,
            session.id,
            TrustedResourceIntent::Watchdog,
        )
        .await
        .unwrap();
        assert_unsupported_kill_result(&inner_response, session.id);

        assert_eq!(process_backend.observe_tree_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            process_backend
                .observe_identity_calls
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(
            process_backend
                .terminate_verified_calls
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(
            process_backend
                .current_process_memory_calls
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 0);
        assert_eq!(pty_backend.kill_count.load(Ordering::SeqCst), 0);
        assert!(pty.lock().unwrap().has_session(session.id));
        assert!(!monitor.has_registered_group(session.id));
        assert_eq!(monitor.active_agent_groups(), 0);
        let after_selection = manager.read().await.selection_payload().await;
        assert_eq!(after_selection.revision(), before_selection.revision());
        assert_eq!(after_selection.id(), before_selection.id());
        assert_eq!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Active
        );
    }

    #[test]
    fn finalizes_only_on_verified_terminated() {
        // Verified dead -> tear down + mark Exited.
        assert!(should_finalize_kill(ResourceGroupState::Terminated));
        // Quarantined (possibly alive) -> keep instance/job, never mark Exited.
        assert!(!should_finalize_kill(ResourceGroupState::Quarantined));
        // A concurrent kill still in flight (early return) -> let it own the outcome,
        // do NOT finalize here (the bug grinch HIGH-1 would otherwise reintroduce).
        assert!(!should_finalize_kill(ResourceGroupState::Terminating));
        assert!(!should_finalize_kill(ResourceGroupState::Running));
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
        fn observe_tree(
            &self,
            _root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
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

    // Grinch HIGH (Step 7): a manual kill that races a concurrent watchdog kill must not
    // mistake the transient `Terminating` for a verified success, and must settle to the
    // real outcome. Proves both halves: (1) while a watchdog kill owns `Terminating`, a
    // direct kill_group returns `Terminating` (NOT finalizable); (2) verify_kill_settled
    // waits it out and returns the settled `Terminated` (finalizable).
    #[tokio::test]
    async fn force_kill_settles_past_concurrent_terminating() {
        let root = ProcessIdentity {
            pid: 4321,
            creation_time_100ns: 99,
        };
        let backend = Arc::new(GatedBackend::new(root));
        let state =
            ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);
        let id = Uuid::new_v4();
        register_running(&state, id, root);

        // Safety net: release the gate on scope exit so a panicking assertion below can
        // never leave the blocked watchdog thread parked on the Condvar. `release` is
        // idempotent, so the timed releaser on the happy path is unaffected.
        struct ReleaseOnDrop(Arc<GatedBackend>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.release();
            }
        }
        let _release_guard = ReleaseOnDrop(backend.clone());

        // Arm so the next observe_tree (the watchdog kill) blocks in `Terminating`.
        backend.armed.store(true, Ordering::SeqCst);
        let watchdog = {
            let s = state.clone();
            std::thread::spawn(move || s.kill_group(id, ResourceKillReason::Watchdog))
        };
        // Wait until the watchdog kill has parked the group in `Terminating`.
        let mut waited_ms = 0;
        while !backend.entered.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
            waited_ms += 5;
            assert!(waited_ms < 2000, "watchdog kill never entered Terminating");
        }

        // (1) A direct kill now hits the `Terminating` early-return: not finalizable.
        let racing = {
            let s = state.clone();
            tokio::task::spawn_blocking(move || s.kill_group(id, ResourceKillReason::User))
                .await
                .unwrap()
                .unwrap()
        };
        assert_eq!(racing.state, ResourceGroupState::Terminating);
        assert!(!racing.finalized);
        assert!(!should_finalize_kill(racing.state));

        // (2) Release the watchdog kill shortly, then verify_kill_settled must wait out
        // `Terminating` and return the settled `Terminated`.
        let releaser = {
            let b = backend.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                b.release();
            })
        };
        let settled = verify_kill_settled(
            Arc::new(state.clone()),
            id,
            ResourceKillReason::User,
            Duration::from_millis(2000),
            Duration::from_millis(25),
        )
        .await
        .unwrap();
        assert_eq!(settled.state, ResourceGroupState::Terminated);
        assert!(should_finalize_kill(settled.state));

        watchdog.join().unwrap().unwrap();
        releaser.join().unwrap();
    }

    async fn assert_serialized_resource_kills(watchdog_first: bool) {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/resource-serialization".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let pty_backend = Arc::new(ResourcePtyBackend::default());
        pty_backend.set_live(session.id);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);

        let order_offset = if watchdog_first { 1 } else { 0 };
        let root = ProcessIdentity {
            pid: 5100 + order_offset,
            creation_time_100ns: 700 + u64::from(order_offset),
        };
        let process_backend = Arc::new(GatedBackend::new(root));
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            process_backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        register_running(monitor.as_ref(), session.id, root);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(Arc::clone(&monitor))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build serialized resource-kill app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();
        let manager_handle = manager.read().await.clone();
        let before_revision = manager_handle.selection_payload().await.revision();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_destroyed", "session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |_| {
                let _ = events_tx.send(event_name);
            });
        }

        struct ReleaseOnDrop(Arc<GatedBackend>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.release();
            }
        }
        let _release_guard = ReleaseOnDrop(process_backend.clone());
        process_backend.armed.store(true, Ordering::SeqCst);

        if watchdog_first {
            let watchdog = {
                let coordinator = coordinator.clone();
                tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
            };
            tokio::time::timeout(Duration::from_secs(2), async {
                while !process_backend.entered.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("watchdog resource kill enters the backend barrier");
            let mut user = {
                let coordinator = coordinator.clone();
                tokio::spawn(async move {
                    coordinator
                        .resource_kill(session.id, TrustedResourceIntent::User)
                        .await
                })
            };
            assert!(
                tokio::time::timeout(Duration::from_millis(50), &mut user)
                    .await
                    .is_err(),
                "user finalizer must queue behind the held watchdog transaction"
            );
            process_backend.release();
            let watchdog_result = watchdog.await.unwrap().unwrap();
            assert!(matches!(
                watchdog_result,
                WatchdogKillOutcome::Completed(ref result) if result.finalized
            ));
            assert!(user.await.unwrap().unwrap().finalized);
        } else {
            let user = {
                let coordinator = coordinator.clone();
                tokio::spawn(async move {
                    coordinator
                        .resource_kill(session.id, TrustedResourceIntent::User)
                        .await
                })
            };
            tokio::time::timeout(Duration::from_secs(2), async {
                while !process_backend.entered.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("user resource kill enters the backend barrier");
            let mut watchdog = {
                let coordinator = coordinator.clone();
                tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
            };
            assert!(
                tokio::time::timeout(Duration::from_millis(50), &mut watchdog)
                    .await
                    .is_err(),
                "watchdog finalizer must queue behind the held user transaction"
            );
            process_backend.release();
            assert!(user.await.unwrap().unwrap().finalized);
            let watchdog_result = watchdog.await.unwrap().unwrap();
            assert!(matches!(
                watchdog_result,
                WatchdogKillOutcome::Completed(ref result) if result.finalized
            ));
        }

        let row = manager_handle.get_session(session.id).await.unwrap();
        assert_eq!(row.status, SessionStatus::Exited(0));
        let selection = manager_handle.selection_payload().await;
        assert_eq!(selection.mode(), SelectionMode::None);
        assert_eq!(selection.id(), None);
        assert_eq!(selection.source(), SelectionSource::ResourceMonitor);
        assert_eq!(selection.user_initiated(), !watchdog_first);
        assert_eq!(selection.revision(), before_revision + 1);
        assert_eq!(
            (0..3)
                .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
                .collect::<Vec<_>>(),
            vec!["session_destroyed", "session_created", "session_switched"]
        );
        assert!(events_rx.try_recv().is_err());
        assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 1);
        assert!(!pty.lock().unwrap().has_session(session.id));
        coordinator.close_and_join().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn user_and_watchdog_resource_finalizers_serialize_in_both_orders() {
        assert_serialized_resource_kills(false).await;
        assert_serialized_resource_kills(true).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_queue_watchdog_waiter_deduplicates_then_runs_one_whole_finalizer() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/watchdog-capacity".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let pty_backend = Arc::new(ResourcePtyBackend::default());
        pty_backend.set_live(session.id);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let root = ProcessIdentity {
            pid: 5300,
            creation_time_100ns: 900,
        };
        let process_backend = Arc::new(GatedBackend::new(root));
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            process_backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        register_running(monitor.as_ref(), session.id, root);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(monitor)
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build full-queue watchdog app");
        coordinator.start(app.handle().clone()).unwrap();
        let restore = coordinator.submit_restore_first().await.unwrap();
        let mut reservations = (0..64)
            .map(|_| coordinator.reserve_auto_close().unwrap())
            .collect::<Vec<_>>();
        let before_revision = manager.read().await.selection_payload().await.revision();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_destroyed", "session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |_| {
                let _ = events_tx.send(event_name);
            });
        }
        struct ReleaseOnDrop(Arc<GatedBackend>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.release();
            }
        }
        let _release_guard = ReleaseOnDrop(process_backend.clone());
        process_backend.armed.store(true, Ordering::SeqCst);

        let waiter = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator
                .critical_key_registered_for_test(session.id, CriticalAdmissionKind::WatchdogKill)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("watchdog waiter registers while logical capacity is full");
        assert!(matches!(
            coordinator
                .watchdog_resource_kill(session.id)
                .await
                .unwrap(),
            WatchdogKillOutcome::AlreadyInFlight
        ));

        drop(reservations.pop());
        restore.finish();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !process_backend.entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("capacity release fairly admits one whole watchdog finalizer");
        assert_eq!(
            manager.read().await.selection_payload().await.revision(),
            before_revision
        );
        assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 1);
        process_backend.release();
        let result = waiter.await.unwrap().unwrap();
        assert!(matches!(
            result,
            WatchdogKillOutcome::Completed(ref result) if result.finalized
        ));
        drop(reservations);

        let selection = manager.read().await.selection_payload().await;
        assert_eq!(selection.revision(), before_revision + 1);
        assert_eq!(selection.mode(), SelectionMode::None);
        assert!(!selection.user_initiated());
        assert_eq!(
            (0..3)
                .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
                .collect::<Vec<_>>(),
            vec!["session_destroyed", "session_created", "session_switched"]
        );
        assert!(events_rx.try_recv().is_err());
        assert!(matches!(
            coordinator
                .watchdog_resource_kill(session.id)
                .await
                .unwrap(),
            WatchdogKillOutcome::Completed(_)
        ));
        assert!(events_rx.try_recv().is_err());
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn full_coordinator_rejects_user_resource_kill_before_any_side_effect() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/resource-busy".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let pty_backend = Arc::new(ResourcePtyBackend::default());
        pty_backend.set_live(session.id);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let root = ProcessIdentity {
            pid: 5200,
            creation_time_100ns: 800,
        };
        let process_backend = Arc::new(GatedBackend::new(root));
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            process_backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        register_running(monitor.as_ref(), session.id, root);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(monitor)
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build busy resource-kill app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let reservations = (0..64)
            .map(|_| coordinator.reserve_auto_close().unwrap())
            .collect::<Vec<_>>();
        let before = manager.read().await.selection_payload().await;

        assert_eq!(
            coordinator
                .resource_kill(session.id, TrustedResourceIntent::User)
                .await
                .unwrap_err(),
            "selectionCoordinatorBusy"
        );
        assert!(!process_backend.entered.load(Ordering::SeqCst));
        assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 0);
        assert_eq!(pty_backend.kill_count.load(Ordering::SeqCst), 0);
        let after = manager.read().await.selection_payload().await;
        assert_eq!(after.revision(), before.revision());
        assert_eq!(after.id(), before.id());
        assert_eq!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Active
        );

        drop(reservations);
        guard.finish();
        coordinator.close_and_join().await;
    }
}
