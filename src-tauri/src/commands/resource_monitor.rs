use std::sync::Arc;

use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::resource_monitor::types::{
    ResourceGroupState, ResourceKillRequest, ResourceKillResult, ResourceLimits, ResourceSnapshot,
};
use crate::resource_monitor::ResourceMonitorState;

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
/// Fire-then-verify (grinch HIGH-1): the job is fired by the pure
/// `terminate_job_for_session` (which keeps the instance/job), and only a result whose
/// state is the verified `Terminated` runs the full `kill` cleanup + flips the tile to
/// `Exited`. A `Quarantined` result (or an unverified `Terminating` early-return from a
/// concurrent kill) leaves the instance/job intact so a Force/Retry can re-fire the
/// durable kill, and never marks a possibly-alive agent `Exited`.
#[tauri::command]
pub async fn kill_resource_group(
    app: AppHandle,
    request: ResourceKillRequest,
    monitor: State<'_, Arc<ResourceMonitorState>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>,
) -> Result<ResourceKillResult, String> {
    let session_id = Uuid::parse_str(&request.session_id).map_err(|e| e.to_string())?;
    let reason = request.reason;

    // Fire the Job Object FIRST (pure: keeps the instance/job for a Retry). The
    // std-Mutex guard is dropped before any await.
    let job_fired = { pty_mgr.lock().unwrap().terminate_job_for_session(session_id) };
    log::info!(
        "[resource-monitor] job-fire session={} job_present={}",
        session_id,
        job_fired
    );

    // Verify + account on the blocking pool. After the job kill the reaper observes
    // the dead tree as AlreadyGone -> quarantined=false -> Terminated (slot released).
    let monitor_c = Arc::clone(&monitor);
    let result = tokio::task::spawn_blocking(move || monitor_c.kill_group(session_id, reason))
        .await
        .map_err(|e| e.to_string())??;

    if should_finalize_kill(result.state) {
        // Verified dead: full PTY cleanup (drops the job handle = KILL_ON_JOB_CLOSE
        // backstop, reaps the child, tears down idle/git/watchers/parser), then flip
        // the tile to Exited. This is the only path that consumes the job.
        {
            let _ = pty_mgr.lock().unwrap().kill(session_id);
        }
        let mgr = session_mgr.read().await;
        mgr.mark_exited(session_id, 0).await;
        mgr.clear_active_if(session_id).await;
        if let Some(updated) = mgr.get_session(session_id).await {
            let info = crate::session::session::SessionInfo::from(&updated);
            let _ = tauri::Emitter::emit(&app, "session_created", info);
        }
        crate::config::sessions_persistence::persist_current_state(&mgr).await;
    } else {
        // NOT verified dead (kernel-AV residual, no job, or a sub-ms async-reap race).
        // Keep the instance/job so Force/Retry re-fires the durable kill; do NOT mark
        // Exited. The result carries the per-PID errors + blocked_by_security.
        log::warn!(
            "[resource-monitor] kill session={} quarantined (job_fired={}): {}",
            session_id,
            job_fired,
            result.message
        );
    }

    Ok(result)
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
    use super::should_finalize_kill;
    use crate::resource_monitor::types::ResourceGroupState;

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
}
