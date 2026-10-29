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
    let cfg = settings
        .current_snapshot()
        .await
        .map_err(|error| error.to_string())?;
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
    #[cfg(test)]
    let retry_failures = std::sync::atomic::AtomicUsize::new(0);
    execute_resource_kill_transaction_inner(
        transaction,
        session_id,
        intent,
        #[cfg(test)]
        &retry_failures,
    )
    .await
}

async fn execute_resource_kill_transaction_inner<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    intent: TrustedResourceIntent,
    #[cfg(test)] retry_failures: &std::sync::atomic::AtomicUsize,
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
                        "[resource-monitor] PTY teardown failed after verified kill and PTY still \
                        live session={}: {}; retrying bounded deferred teardown",
                        session_id,
                        error
                    );
                    finalize_dead_pty_route_bounded(
                        transaction,
                        session_id,
                        intent,
                        &pty_mgr,
                        #[cfg(test)]
                        retry_failures,
                    )
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
    #[cfg(test)] retry_failures: &std::sync::atomic::AtomicUsize,
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
        retry_failures.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    log::error!(
        "[resource-monitor] bounded deferred PTY teardown exhausted {} attempts session={}; \
        escalating to best-effort force-close",
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
mod tests;
