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
pub async fn kill_resource_group(
    app: AppHandle,
    request: ResourceKillRequest,
    _monitor: State<'_, Arc<ResourceMonitorState>>,
    _pty_mgr: State<'_, Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>,
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>,
) -> Result<ResourceKillResult, String> {
    let session_id = Uuid::parse_str(&request.session_id).map_err(|e| e.to_string())?;
    if request.reason != ResourceKillReason::User {
        return Err("resourceMonitor user command requires reason=user".to_string());
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
    let pty_mgr = transaction
        .app()
        .state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>();
    let monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();

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
    use super::{should_finalize_kill, verify_kill_settled};
    use crate::resource_monitor::registry::{
        ProcessTreeBackend, ResourceError, ResourceLaunchRegistration,
    };
    use crate::resource_monitor::types::{
        ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, ResourceGroupState,
        ResourceKillReason, ResourceLaunchMetadata, ResourceLimits, TerminateOutcome,
    };
    use crate::resource_monitor::ResourceMonitorState;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;
    use uuid::Uuid;

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
}
