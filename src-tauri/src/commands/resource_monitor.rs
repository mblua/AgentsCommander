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
            // Drop the std-Mutex guard before the S1 re-check below: the defer test
            // proved a re-entrant same-thread relock (`runtime_snapshot` re-locks the
            // pty) deadlocks. `kill` itself is quick and side-effect-free on the
            // failure arm, so no lock is held across the re-check.
            let kill_result = {
                let guard = pty_mgr
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let r = guard.kill(session_id);
                drop(guard);
                r
            };
            if let Err(error) = kill_result {
                // §1295 S1: the PTY `kill` returned Err. Re-check whether the PTY is
                // still live (`.has_pty` == `pty_mgr.has_session`). If it is, schedule
                // the GUARANTEED bounded deferred teardown (round-3 finding-1(b)) so the
                // route is torn down and the tile released within bounded wall time.
                if transaction.runtime_snapshot(session_id).has_pty {
                    log::warn!(
                        "[resource-monitor] PTY teardown failed after verified kill and PTY still live session={}: {}; retrying bounded deferred teardown",
                        session_id,
                        error
                    );
                    finalize_dead_pty_route_bounded(transaction, session_id, intent, &pty_mgr)
                        .await?;
                    result.finalized = true;
                    return Ok(result);
                }
                log::warn!(
                    "[resource-monitor] PTY teardown failed after verified kill session={}: {}",
                    session_id,
                    error
                );
            }
        }
        finalize_verified_dead_session(transaction, session_id, intent).await?;
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

/// Round-3 finding-1(b): how many times the bounded deferred PTY-route teardown retries
/// a still-live route before escalating to a best-effort force-close. Finite and small;
/// wall time is bounded by `ATTEMPTS * DELAY`.
const PTY_ROUTE_RETRY_ATTEMPTS: u32 = 3;
/// Real delay between deferred teardown retries.
const PTY_ROUTE_RETRY_DELAY: Duration = Duration::from_millis(200);

/// The shared verified-dead finalize used by BOTH the happy finalize path and the
/// bounded deferred teardown. Removes the detached-state record, destroys the terminal
/// window, commits the Exited flip, then persists and publishes, so the tile is released
/// and the FE modal resolves. Idempotent for a missing/Exited row: `commit` / `mark_exited`
/// on a gone row is a no-op and must not error.
async fn finalize_verified_dead_session<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    intent: TrustedResourceIntent,
) -> Result<(), String> {
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
    Ok(())
}

/// Round-3 finding-1(b): a guaranteed bounded teardown for a still-live PTY route after a
/// verified-dead kill whose `PtyManager::kill` returned Err. Runs on the local async task
/// with a real delay between attempts; once the route clears it runs the shared
/// verified-dead finalize. If the route never clears within `PTY_ROUTE_RETRY_ATTEMPTS`, it
/// escalates to a best-effort force-close of the route and then finalizes anyway, accepting
/// a residual leaked route over a permanent Running strand. The std-Mutex guard is dropped
/// before any await or re-lock (same re-entrant-discipline as the S1 fix).
async fn finalize_dead_pty_route_bounded<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    intent: TrustedResourceIntent,
    pty_mgr: &Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>,
) -> Result<(), String> {
    for attempt in 1..=PTY_ROUTE_RETRY_ATTEMPTS {
        tokio::time::sleep(PTY_ROUTE_RETRY_DELAY).await;
        let route_gone = {
            let guard = pty_mgr.lock().unwrap_or_else(|error| error.into_inner());
            let kill_ok = guard.kill(session_id).is_ok();
            let live = guard.has_session(session_id);
            drop(guard);
            kill_ok || !live
        };
        if route_gone {
            log::info!(
                "[resource-monitor] bounded deferred PTY teardown succeeded session={} attempt={}",
                session_id,
                attempt
            );
            return finalize_verified_dead_session(transaction, session_id, intent).await;
        }
        log::warn!(
            "[resource-monitor] bounded deferred PTY teardown attempt {}/{} still live session={}",
            attempt,
            PTY_ROUTE_RETRY_ATTEMPTS,
            session_id
        );
        #[cfg(test)]
        DEFER_RETRY_FAILURES.with(|c| c.set(c.get() + 1));
    }
    log::error!(
        "[resource-monitor] bounded deferred PTY teardown exhausted {} attempts session={}; escalating to best-effort force-close",
        PTY_ROUTE_RETRY_ATTEMPTS,
        session_id
    );
    {
        let guard = pty_mgr.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(kind) = guard.backend_kind(session_id) {
            let _ = guard.try_remove_route_if_kind(session_id, kind);
        }
        drop(guard);
    }
    finalize_verified_dead_session(transaction, session_id, intent).await
}

#[cfg(test)]
use std::cell::Cell;

// Per-test count of bounded-deferred-teardown retry failures. `#[tokio::test]` (current
// thread runtime) runs each test body on one thread, so a thread-local isolates tests
// that run in parallel and is reset at the start of each retry test.
#[cfg(test)]
thread_local! {
    static DEFER_RETRY_FAILURES: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn defer_retry_failures() -> usize {
    DEFER_RETRY_FAILURES.with(|c| c.get())
}

#[cfg(test)]
fn reset_defer_retry_failures() {
    DEFER_RETRY_FAILURES.with(|c| c.set(0));
}

#[cfg(test)]
mod tests {
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

    /// Build the 4-state mock app (manager, PtyManager, ResourceMonitorState,
    /// DetachedSessionsState) with a Group-neutral GatedBackend (supports process
    /// tree enforcement = true) but NO group registration, so `kill_group` reports
    /// Terminated immediately with no coordinator/reaper barrier. Used by the S1
    /// defer-arm test without any coordinator.
    async fn run_kill_error_branch(
        fail_kill: bool,
        keep_pty_live: bool,
    ) -> (
        ResourceKillResult,
        Arc<tokio::sync::RwLock<SessionManager>>,
        Uuid,
        Arc<ResourcePtyBackend>,
        Arc<Mutex<PtyManager>>,
    ) {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/kill-error-defer".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create session");
        let pty_backend = Arc::new(ResourcePtyBackend::default());
        pty_backend.set_live(session.id);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let root = ProcessIdentity {
            pid: 5400,
            creation_time_100ns: 910,
        };
        let process_backend = Arc::new(GatedBackend::new(root));
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            process_backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(Arc::clone(&monitor))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build kill-error app");
        pty_backend.set_fail_kill(fail_kill);
        if !keep_pty_live {
            // Simulate the PTY route already gone while `kill` still errors.
            pty_backend.live.lock().unwrap().remove(&session.id);
        }
        let transaction = SelectionTransaction::for_test(app.handle().clone());
        let result = execute_resource_kill_transaction(
            &transaction,
            session.id,
            TrustedResourceIntent::Watchdog,
        )
        .await
        .expect("kill transaction");
        (result, manager, session.id, pty_backend, pty)
    }

    /// Test 18 (replaces test 17): the S1 kill-error path. Round-3 finding-1(b) replaces
    /// indefinite deferral with a GUARANTEED bounded teardown, so with a still-live PTY
    /// whose `kill` keeps erroring the transaction escalates to a best-effort force-close
    /// and finalizes within bounded time (finalized true, row Exited, route dropped). When
    /// the PTY is already gone, the same `kill` error falls through and finalizes as today.
    /// Both branches call `execute_resource_kill_transaction` DIRECTLY (no coordinator)
    /// under a hard timeout.
    #[tokio::test]
    async fn resource_monitor_kill_error_defers_exited_while_pty_live() {
        let (result, manager, session_id, _pty_backend, pty) =
            tokio::time::timeout(Duration::from_secs(10), run_kill_error_branch(true, true))
                .await
                .expect("branch 1 (escalate) did not complete in time");
        assert!(
            result.finalized,
            "bounded teardown finalizes even while the PTY route stays live (escalation)"
        );
        let row = manager
            .read()
            .await
            .get_session(session_id)
            .await
            .expect("row present after finalize");
        assert!(
            matches!(row.status, SessionStatus::Exited(_)),
            "row Exited after escalation finalize"
        );
        assert!(
            !pty.lock().unwrap().has_session(session_id),
            "escalation force-close dropped the PTY route"
        );

        let (result, manager, session_id, _pty_backend, _pty) =
            tokio::time::timeout(Duration::from_secs(10), run_kill_error_branch(true, false))
                .await
                .expect("branch 2 (finalize) did not complete in time");
        assert!(
            result.finalized,
            "finalize proceeds when the PTY route is already gone"
        );
        let row = manager
            .read()
            .await
            .get_session(session_id)
            .await
            .expect("row present after finalize");
        assert!(
            matches!(row.status, SessionStatus::Exited(_)),
            "row Exited after the finalize branch"
        );
    }

    /// Builds the 4-state mock app for the round-3 bounded-deferred-teardown tests
    /// (tests 19-20): a GatedBackend with no group registration so `kill_group` reports
    /// Terminated immediately, and a backend-setup closure so each test configures how the
    /// PTY `kill` fails. Returns the result, manager, session id, backend, and pty manager.
    async fn run_bounded_teardown_branch(
        backend_setup: impl FnOnce(Arc<ResourcePtyBackend>),
    ) -> (
        ResourceKillResult,
        Arc<tokio::sync::RwLock<SessionManager>>,
        Uuid,
        Arc<ResourcePtyBackend>,
        Arc<Mutex<PtyManager>>,
    ) {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/bounded-teardown".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create session");
        let pty_backend = Arc::new(ResourcePtyBackend::default());
        pty_backend.set_live(session.id);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let process_backend = Arc::new(GatedBackend::new(ProcessIdentity {
            pid: 5600,
            creation_time_100ns: 1200,
        }));
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            process_backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        backend_setup(Arc::clone(&pty_backend));
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(Arc::clone(&monitor))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build bounded teardown app");
        let transaction = SelectionTransaction::for_test(app.handle().clone());
        let result = execute_resource_kill_transaction(
            &transaction,
            session.id,
            TrustedResourceIntent::Watchdog,
        )
        .await
        .expect("kill transaction");
        (result, manager, session.id, pty_backend, pty)
    }

    /// Test 19 (round-3): the bounded deferred teardown retries the still-live PTY route
    /// and finalizes once the route clears on the last attempt. The mock `kill` fails the
    /// initial verified-kill call and the first N-1 retry attempts (so exactly N-1 teardown
    /// WARNs), then succeeds on the Nth attempt: the route clears, the row flips to Exited,
    /// and `result.finalized` is true. Deterministic and bounded.
    #[tokio::test]
    async fn s1_defer_retries_pty_teardown_then_finalizes_when_route_clears() {
        reset_defer_retry_failures();
        let (result, manager, session_id, pty_backend, pty) = tokio::time::timeout(
            Duration::from_secs(10),
            run_bounded_teardown_branch(|backend| {
                backend.set_fail_kill(false);
                backend.set_fail_kill_remaining(PTY_ROUTE_RETRY_ATTEMPTS as usize);
            }),
        )
        .await
        .expect("bounded teardown (route clears) did not complete in time");
        assert!(
            result.finalized,
            "finalized after the route clears on the last retry attempt"
        );
        let row = manager
            .read()
            .await
            .get_session(session_id)
            .await
            .expect("row present after finalize");
        assert!(
            matches!(row.status, SessionStatus::Exited(_)),
            "row Exited after the bounded teardown finalize"
        );
        assert!(
            !pty.lock().unwrap().has_session(session_id),
            "PTY route cleared on the final retry attempt"
        );
        assert_eq!(
            defer_retry_failures(),
            (PTY_ROUTE_RETRY_ATTEMPTS - 1) as usize,
            "exactly N-1 teardown WARNs emitted before the route cleared"
        );
        assert_eq!(
            pty_backend.kill_fail_count.load(Ordering::SeqCst),
            PTY_ROUTE_RETRY_ATTEMPTS as usize,
            "the initial kill plus N-1 retries failed before the route cleared"
        );
    }

    /// Test 20 (round-3): when the PTY route never clears, the bounded deferred teardown
    /// escalates after exactly N failed attempts to a best-effort force-close of the route
    /// and still runs the finalize, so the tile is released (row Exited, finalized true)
    /// instead of stranding in Running forever. No infinite retry.
    #[tokio::test]
    async fn s1_defer_escalates_force_close_after_n_failures() {
        reset_defer_retry_failures();
        let (result, manager, session_id, _pty_backend, pty) = tokio::time::timeout(
            Duration::from_secs(10),
            run_bounded_teardown_branch(|backend| {
                backend.set_fail_kill(true);
            }),
        )
        .await
        .expect("bounded teardown (escalate) did not complete in time");
        assert!(
            result.finalized,
            "escalating force-close releases the tile via the Exited flip"
        );
        let row = manager
            .read()
            .await
            .get_session(session_id)
            .await
            .expect("row present after escalation");
        assert!(
            matches!(row.status, SessionStatus::Exited(_)),
            "row Exited after escalation force-close"
        );
        assert_eq!(
            defer_retry_failures(),
            PTY_ROUTE_RETRY_ATTEMPTS as usize,
            "exactly N failed retry attempts before escalation"
        );
        assert!(
            !pty.lock().unwrap().has_session(session_id),
            "best-effort force-close dropped the PTY route on escalation"
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
    async fn full_orchestrator_rejects_user_resource_kill_before_any_side_effect() {
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

    /// #1151 (W6) - a backend that quarantines the first cleanup on a stubborn child and,
    /// once armed, BLOCKS inside `observe_tree`, which is `kill_group_inner`'s first
    /// backend call and therefore a deterministic barrier. Releasing the gate also makes
    /// the child verifiably gone, so the parked retry settles to Terminated.
    ///
    /// The barrier is announced through a Condvar rather than an AtomicBool so the test
    /// thread can wait for it with a plain `std` wait. W6 must never need the async
    /// runtime to make progress in order to report, because the defect it exists to catch
    /// is exactly the one that stops the runtime from making progress.
    struct QuarantineGatedBackend {
        root: ProcessIdentity,
        armed: AtomicBool,
        entered: (Mutex<bool>, Condvar),
        /// The thread that ran the armed `observe_tree`, recorded before it blocks. This
        /// is the subject of W6's assertion 1.
        observer: Mutex<Option<std::thread::ThreadId>>,
        child_gone: AtomicBool,
        gate: (Mutex<bool>, Condvar),
    }

    impl QuarantineGatedBackend {
        fn new(root: ProcessIdentity) -> Self {
            Self {
                root,
                armed: AtomicBool::new(false),
                entered: (Mutex::new(false), Condvar::new()),
                observer: Mutex::new(None),
                child_gone: AtomicBool::new(false),
                gate: (Mutex::new(false), Condvar::new()),
            }
        }

        /// Blocks the calling thread, using `std` only, until the armed `observe_tree` has
        /// recorded its thread and is about to park on the release gate. Returns false on
        /// timeout so the caller reports an assertion failure instead of hanging.
        fn wait_entered(&self, timeout: Duration) -> bool {
            let (lock, condvar) = &self.entered;
            let (entered, wait) = condvar
                .wait_timeout_while(lock.lock().unwrap(), timeout, |entered| !*entered)
                .unwrap();
            !wait.timed_out() && *entered
        }

        fn observer(&self) -> Option<std::thread::ThreadId> {
            *self.observer.lock().unwrap()
        }

        fn child(&self) -> ProcessIdentity {
            ProcessIdentity {
                pid: self.root.pid + 1,
                creation_time_100ns: self.root.creation_time_100ns + 1,
            }
        }

        fn released(&self) -> bool {
            *self.gate.0.lock().unwrap()
        }

        fn release(&self) {
            self.child_gone.store(true, Ordering::SeqCst);
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }

        fn observed(
            identity: ProcessIdentity,
            depth: u32,
            parent_pid: Option<u32>,
        ) -> ObservedProcess {
            ObservedProcess {
                identity,
                parent_pid,
                parent_identity: None,
                exe_name: format!("p{}", identity.pid),
                depth,
                private_bytes: None,
                working_set_bytes: None,
                cpu_percent: None,
                kill_allowed: true,
            }
        }
    }

    impl ProcessTreeBackend for QuarantineGatedBackend {
        fn observe_tree(
            &self,
            _root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            if self.armed.load(Ordering::SeqCst) {
                // Record the observer BEFORE announcing the barrier, so a waiter that
                // sees `entered` always sees the thread id too.
                *self.observer.lock().unwrap() = Some(std::thread::current().id());
                {
                    let (lock, condvar) = &self.entered;
                    *lock.lock().unwrap() = true;
                    condvar.notify_all();
                }
                let mut released = self.gate.0.lock().unwrap();
                while !*released {
                    released = self.gate.1.wait(released).unwrap();
                }
            }
            let mut processes = vec![Self::observed(self.root, 0, None)];
            if !self.child_gone.load(Ordering::SeqCst) {
                processes.push(Self::observed(self.child(), 1, Some(self.root.pid)));
            }
            Ok(ObservedProcessTree {
                processes,
                errors: Vec::new(),
            })
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            if pid == self.root.pid {
                return Ok(Some(self.root));
            }
            if pid == self.child().pid && !self.child_gone.load(Ordering::SeqCst) {
                return Ok(Some(self.child()));
            }
            Ok(None)
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            if process.identity == self.child() && !self.child_gone.load(Ordering::SeqCst) {
                return Err(ResourceError::Message(format!(
                    "pid {}: process still alive after terminate",
                    process.identity.pid
                )));
            }
            Ok(TerminateOutcome::AlreadyGone)
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            Ok(ProcessMemory::default())
        }
    }

    // #1151 (W2) - a LIVE public session keeps the coordinator plus Job Object path
    // exactly as before: the transaction runs, the session finalizes once, and the
    // registry-only orphan path is never entered.
    #[tokio::test]
    async fn live_public_session_still_uses_orchestrator() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/orphan-live-session".to_string(),
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
        assert_eq!(monitor.active_agent_groups(), 1);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(Arc::clone(&monitor))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build live-session orphan-retry app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();

        let report = crate::resource_monitor::watchdog::retry_quarantined_group(
            monitor.as_ref(),
            &coordinator,
            session.id,
            root,
        )
        .await;

        assert_eq!(
            report.path,
            crate::resource_monitor::watchdog::QuarantineRetryPath::Coordinator
        );
        assert_eq!(report.root_pid, root.pid);
        assert_eq!(report.state, Some(ResourceGroupState::Terminated));
        assert!(!report.still_counts_toward_admission);
        assert_eq!(report.active_agent_groups, 0);
        assert_eq!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Exited(0)
        );
        // Finalized exactly once, through the coordinator: one PTY teardown and the route
        // gone. The registry-only path never touches the PTY at all.
        assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 1);
        assert!(!pty.lock().unwrap().has_session(session.id));
        coordinator.close_and_join().await;
    }

    // #1151 (W5) - NON-REGRESSION. A destroy-retained root agent keeps its row as Exited,
    // so contains_public_or_pending is still true and the coordinator path stays. An
    // implementation that keyed the orphan decision on "the PTY is gone" rather than "the
    // session row is gone" would silently divert this working path onto the new one.
    #[tokio::test]
    async fn exited_row_still_uses_orchestrator() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/orphan-exited-row".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        // mark_exited's bool reports whether a raise-hand flag was cleared, not success.
        manager.read().await.mark_exited(session.id, 0).await;
        assert_eq!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Exited(0)
        );
        let pty_backend = Arc::new(ResourcePtyBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
        let root = ProcessIdentity {
            pid: 5400,
            creation_time_100ns: 1000,
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
            .expect("build exited-row orphan-retry app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();

        let report = crate::resource_monitor::watchdog::retry_quarantined_group(
            monitor.as_ref(),
            &coordinator,
            session.id,
            root,
        )
        .await;

        assert_eq!(
            report.path,
            crate::resource_monitor::watchdog::QuarantineRetryPath::Coordinator
        );
        assert_eq!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Exited(0)
        );
        coordinator.close_and_join().await;
    }

    // #1151 (W6) - the orphan retry runs OFF the async runtime. kill_group can cost about
    // two seconds per stubborn PID, so inlining it would stall a Tokio worker.
    //
    // Exactly ONE worker thread is load-bearing, not a style choice: it makes "the async
    // runtime" an enumerable set of one OS thread, so the thread-identity assertion below
    // is a complete statement rather than a sample. At two workers it degrades to sampling
    // one worker out of two, and whether the mutation is caught becomes scheduler
    // dependent. That is not acceptable for a regression pin, however it happens to land
    // in any one environment.
    //
    // Every wait here is a `std` wait on the test thread, capped so a failure is an
    // assertion and not a hang. `tokio::time` must not appear in this test: a shape that
    // needs the runtime to make progress in order to REPORT cannot report the very defect
    // that stops the runtime from making progress.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn orphan_retry_runs_off_the_async_runtime() {
        // Harness self-check, taken while the runtime is still idle: spawned tasks really
        // do run on a worker thread rather than on the block_on thread. If a future Tokio
        // ever changed that, every assertion below would be silently vacuous.
        let test_thread = std::thread::current().id();
        let worker_thread = tokio::spawn(async { std::thread::current().id() })
            .await
            .expect("the worker identity probe joins");
        assert_ne!(
            worker_thread, test_thread,
            "spawned tasks must run on the runtime's worker thread, not on the test thread"
        );

        let root = ProcessIdentity {
            pid: 5500,
            creation_time_100ns: 1100,
        };
        let process_backend = Arc::new(QuarantineGatedBackend::new(root));
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            process_backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        // No session row is ever created: that is what makes this group an orphan.
        let session_id = Uuid::new_v4();
        register_running(monitor.as_ref(), session_id, root);
        let quarantine = monitor
            .kill_group(session_id, ResourceKillReason::SessionDestroy)
            .expect("first cleanup runs");
        assert!(quarantine.quarantined);

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            ResourcePtyBackend::default(),
        ))));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(pty)
            .manage(Arc::clone(&monitor))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build off-runtime orphan-retry app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();

        struct ReleaseOnDrop(Arc<QuarantineGatedBackend>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.release();
            }
        }
        let _release_guard = ReleaseOnDrop(process_backend.clone());
        process_backend.armed.store(true, Ordering::SeqCst);

        // tokio::spawn is load-bearing too: it is what puts an inlined blocking call on
        // the worker thread, instead of on the test thread where it would satisfy
        // assertion 1 for the wrong reason.
        let retry = {
            let monitor = Arc::clone(&monitor);
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                crate::resource_monitor::watchdog::retry_quarantined_group(
                    monitor.as_ref(),
                    &coordinator,
                    session_id,
                    root,
                )
                .await
            })
        };
        assert!(
            process_backend.wait_entered(Duration::from_secs(5)),
            "the orphan retry enters the backend barrier"
        );
        let observer = process_backend
            .observer()
            .expect("the barrier records its thread before parking");

        // Assertion 2's probe, taken while the gate is still held: a task that the runtime
        // polls right now proves the runtime is still polling tasks. The channel is `std`,
        // so its result reaches the test thread whether or not the runtime is alive.
        let (tx, rx) = std::sync::mpsc::channel();
        tokio::spawn(async move {
            let _ = tx.send(());
        });
        let runtime_polled_a_task = rx.recv_timeout(Duration::from_secs(5)).is_ok();

        assert!(
            !process_backend.released(),
            "the barrier must still be held while both facts are captured"
        );
        process_backend.release();

        // Assertion 1: the blocking work ran on neither the runtime's single worker nor
        // the test thread, so it ran on the blocking pool. With one worker those two are
        // the only non-blocking-pool threads, so this is exhaustive. It rejects an inline
        // call and tokio::task::block_in_place alike.
        assert_ne!(
            observer, worker_thread,
            "the orphan retry must not run on the async runtime's worker thread"
        );
        assert_ne!(
            observer, test_thread,
            "the orphan retry must not run on the test thread"
        );
        // Assertion 2: the runtime kept making progress while the blocking call was in
        // flight. This is what rejects std::thread::spawn(..).join(), which is off the
        // runtime by identity yet parks the worker for the whole call. Never assert WHICH
        // thread sent it: block_in_place legitimately migrates the scheduler to a
        // replacement thread, so pinning the sender would add a false-failure mode.
        assert!(
            runtime_polled_a_task,
            "the async runtime must keep polling tasks while the orphan retry blocks"
        );

        let report = retry.await.unwrap();
        assert_eq!(
            report.path,
            crate::resource_monitor::watchdog::QuarantineRetryPath::Orphan
        );
        assert_eq!(report.state, Some(ResourceGroupState::Terminated));
        assert!(!report.still_counts_toward_admission);
        coordinator.close_and_join().await;
    }
}
