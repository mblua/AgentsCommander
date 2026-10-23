use std::sync::Arc;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

/// (#756) Record an AC-driven fresh-conversation boundary for `session_id`:
/// write the coordinator-clocks mirror (`start_fresh_at`, persisted
/// immediately) and THEN stamp the durable record intent
/// (`start_fresh_on_restore = true`, persisted under the sessions save lock).
/// Mirror-first (section 19.3): the death-between-halves residue must fail
/// FRESH, never resurrect. The intent survives record destruction (idle
/// auto-close, manual close). Call only after a successful logical clear
/// injection (/clear or Pi /new; C1 remote action, C2 self-clear phase 1). The restart
/// path (C3) stamps the record itself and calls only the mirror half.
/// Root-agent sessions skip the record half (the root restore path ignores the
/// marker, #630 scope; mirrors the restart site's exclusion in
/// commands/session.rs); the mirror half self-gates on coordinators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundaryMetadataOutcome {
    Applied,
    Unchanged,
    Failed,
}

pub(super) fn combine_boundary_metadata(
    first: BoundaryMetadataOutcome,
    second: BoundaryMetadataOutcome,
) -> BoundaryMetadataOutcome {
    use BoundaryMetadataOutcome as O;
    match (first, second) {
        (O::Failed, _) | (_, O::Failed) => O::Failed,
        (O::Applied, _) | (_, O::Applied) => O::Applied,
        (O::Unchanged, O::Unchanged) => O::Unchanged,
    }
}

pub(crate) async fn stamp_fresh_boundary_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> BoundaryMetadataOutcome {
    // (#756, section 19.3) MIRROR-FIRST: if the app dies between the halves,
    // the residue (mirror=Some, record=false) forces fresh on every reopen
    // path and self-heals (E3 re-propagates; typed input or injected content
    // clears both). Record-first residue (record=true, mirror=None) would let
    // a later record destroy resurrect the pre-boundary conversation: the
    // exact #756 bug.
    let mirror = write_start_fresh_mirror_outcome(app, session_id, true).await;
    if let Some(state) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
        state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset(session_id);
    }
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    // Single-clone lookup; DUAL root predicate mirrors the restart path so the
    // two exclusions never disagree.
    let is_root = manager
        .get_session(session_id)
        .await
        .map(|s| {
            s.is_root_agent || crate::config::root_agent::is_root_agent_path(&s.working_directory)
        })
        .unwrap_or(false);
    let record = if is_root {
        BoundaryMetadataOutcome::Unchanged
    } else {
        match crate::config::sessions_persistence::set_start_fresh_and_persist_result(
            &manager, session_id,
        )
        .await
        {
            Ok(true) => {
                log::info!(
                    "[session-state] {} fresh-boundary stamped (record, #756)",
                    &session_id.to_string()[..8]
                );
                BoundaryMetadataOutcome::Applied
            }
            Ok(false) => BoundaryMetadataOutcome::Unchanged,
            Err(_) => {
                log::error!(
                    "[session-state] fresh-boundary stamp persist \
                        failed session={} code=boundary_metadata_failed",
                    session_id
                );
                BoundaryMetadataOutcome::Failed
            }
        }
    };
    combine_boundary_metadata(mirror, record)
}

/// (#756) Drop the durable fresh intent after AC successfully injected message
/// CONTENT into `session_id` (standard mailbox body, follow-up after a remote
/// command, phase-2 handoff prompts, loop prompts). The injected body creates a
/// post-boundary transcript, so provider resume becomes safe and desirable
/// again; a lingering stamp would wipe a live autonomous conversation on the
/// next reopen. DELIBERATELY NARROW: must NOT reuse
/// `note_user_message_to_session`, whose injection-exclusion protects
/// silence/badge/auto-close semantics (see its doc comment above); this helper
/// touches ONLY the fresh intent (mirror first, then record; section 19.3).
/// Never call it for bare logical-action text (/clear, Pi /new, or /compact).
pub(crate) async fn note_post_boundary_content_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> BoundaryMetadataOutcome {
    // (#756, section 19.3) MIRROR-FIRST: the drop residue (mirror=None,
    // record=true) only mis-freshes the record-alive restore until the next
    // heal; record-first residue (record=false, mirror=Some) would wrongly
    // force-fresh BOTH reopen paths.
    let mirror = write_start_fresh_mirror_outcome(app, session_id, false).await;
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    let record = match crate::config::sessions_persistence::clear_start_fresh_and_persist_result(
        &manager, session_id,
    )
    .await
    {
        Ok(true) => {
            log::info!(
                "[session-state] {} fresh intent dropped (post-boundary content, #756)",
                &session_id.to_string()[..8]
            );
            BoundaryMetadataOutcome::Applied
        }
        Ok(false) => BoundaryMetadataOutcome::Unchanged,
        Err(_) => {
            log::error!(
                "[session-state] post-boundary-content drop persist \
                    failed session={} code=boundary_metadata_failed",
                session_id
            );
            BoundaryMetadataOutcome::Failed
        }
    };
    combine_boundary_metadata(mirror, record)
}

/// (#756) Mirror half: write the coordinator-clocks `start_fresh_at` for the
/// session's cwd. Returns false without touching anything for non-coordinators
/// (`coordinator_cwd` -> None; root agents land here too) or when the value is
/// already in the target state. Persists the clocks file immediately on a real
/// transition: these boundaries are rare and must survive an app close inside
/// the 60s flush tick (same discipline as close_coordinator's explicit save).
pub(crate) async fn write_start_fresh_mirror_for_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    on: bool,
) -> bool {
    write_start_fresh_mirror_outcome(app, session_id, on).await == BoundaryMetadataOutcome::Applied
}

async fn write_start_fresh_mirror_outcome<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    on: bool,
) -> BoundaryMetadataOutcome {
    let cwd = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cwd = mgr.read().await.coordinator_cwd(session_id).await;
        cwd
    };
    let Some(cwd) = cwd else {
        return BoundaryMetadataOutcome::Unchanged;
    };
    let Some(clocks) = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
    else {
        return BoundaryMetadataOutcome::Unchanged;
    };
    // agent_fqn_from_path returns String (teams.rs:80), not Option.
    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let changed = {
        let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            guard.mark_start_fresh(&fqn, chrono::Utc::now())
        } else {
            guard.clear_start_fresh(&fqn)
        }
    };
    if changed {
        log::info!(
            "[coordinator-clocks] start_fresh_at {} for '{}' (#756)",
            if on { "set" } else { "cleared" },
            fqn
        );
        let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if crate::config::coordinator_clocks::save_map(&snapshot).is_err() {
            log::warn!(
                "[coordinator-clocks] start_fresh_at save failed code=boundary_metadata_failed"
            );
            return BoundaryMetadataOutcome::Failed;
        }
        BoundaryMetadataOutcome::Applied
    } else {
        BoundaryMetadataOutcome::Unchanged
    }
}
