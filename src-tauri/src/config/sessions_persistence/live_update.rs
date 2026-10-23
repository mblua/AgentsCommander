use super::persist::snapshot_and_save_locked;
use super::project_scope::session_retention_project_paths;
use super::write::run_owned_persist_core;
use crate::session::manager::{
    acquire_carrier_mutation_permit, RaiseCarrierOutcome, SessionManager,
};
use crate::session::session::SessionCommunication;
use std::path::Path;
use uuid::Uuid;

/// #698 — outcome of an atomic raise-hand-and-persist attempt.
#[derive(Debug)]
pub enum RaiseHandPersistOutcome {
    /// The hand was newly raised and the snapshot was saved durably. The caller
    /// should emit `session_communication_changed` with this communication.
    Raised(SessionCommunication),
    /// A visible raise-hand was already present; nothing changed and nothing was
    /// persisted. The raise is still active (`raised: true`, status `already_visible`).
    AlreadyVisible,
    /// The session cannot raise its hand (missing, non-coordinator, or exited);
    /// `raised: false`, status `not_visible`.
    NotRaisable,
}

/// #698 — apply the raise-hand transition and persist the resulting snapshot
/// atomically with respect to ALL session persistence.
///
/// Fix for the HIGH grinch finding: the live mutation and its durable snapshot
/// must not be observable independently. We take `sessions_save_lock()` FIRST,
/// so no concurrent `persist_current_state` caller can snapshot or write the
/// raised state between our mutation and our save. We then mutate, snapshot, and
/// save under that single lock. On save failure we roll back the live
/// communication BEFORE releasing the lock, so neither memory nor disk retains a
/// raise that did not survive: any later persist can only ever snapshot the
/// cleared state.
///
/// We deliberately do NOT call `persist_current_state_result` here; it would
/// re-acquire the same (non-reentrant) tokio mutex and deadlock. We call the
/// lock-free `snapshot_and_save_locked` instead.
pub async fn raise_hand_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<RaiseHandPersistOutcome, String> {
    let dir = super::super::config_dir().ok_or("Could not determine home directory")?;
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    raise_hand_and_persist_to_dir_result(mgr, session_id, now, &dir, Some(&project_paths)).await
}

pub(super) async fn raise_hand_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<RaiseHandPersistOutcome, String> {
    let manager = mgr.clone();
    let dir = dir.to_path_buf();
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        raise_hand_and_persist_to_dir_result_core(&manager, session_id, now, &dir, roots.as_deref())
            .await
    })
    .await
}

pub(super) async fn raise_hand_and_persist_to_dir_result_core(
    mgr: &SessionManager,
    session_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<RaiseHandPersistOutcome, String> {
    let carrier_permit = acquire_carrier_mutation_permit().await;

    let (communication, receipt) = match mgr
        .raise_hand_with_carrier_permit(&carrier_permit, session_id, now)
        .await
    {
        RaiseCarrierOutcome::Changed(communication, receipt) => (communication, receipt),
        RaiseCarrierOutcome::AlreadyVisible(_) => {
            return Ok(RaiseHandPersistOutcome::AlreadyVisible)
        }
        RaiseCarrierOutcome::NotRaisable => return Ok(RaiseHandPersistOutcome::NotRaisable),
    };

    if let Err(e) = snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit).await {
        // Reacquire U and roll back only this nonce/version receipt. A successor
        // transition after a waiting recapture must remain visible.
        let carrier_permit = acquire_carrier_mutation_permit().await;
        let _ = mgr
            .rollback_raise_with_carrier_permit(&carrier_permit, receipt)
            .await;
        return Err(e);
    }

    Ok(RaiseHandPersistOutcome::Raised(communication))
}

/// #1669 - outcome of a set-blocked-menu-and-persist attempt. There is
/// deliberately no failure variant: a save failure cannot change the outcome,
/// because the in-memory transition stands and the caller must still publish it.
#[derive(Debug)]
pub enum BlockedMenuPersistOutcome {
    /// A real transition applied. The caller MUST emit
    /// `session_communication_changed` with this communication, whether or not
    /// the snapshot reached disk.
    Changed(SessionCommunication),
    /// The identical blocked-menu message was already visible. Nothing mutated,
    /// nothing written. This is the 250 ms scan-loop steady state and is why a
    /// blocked session does not write once per tick.
    Unchanged,
    /// The session is missing, pending-create, or exited. Nothing mutated,
    /// nothing written.
    NotApplicable,
}

/// #1669 - apply the blocked-menu set transition and persist the resulting
/// snapshot atomically with respect to ALL session persistence: take
/// `sessions_save_lock()` FIRST, then mutate, snapshot and save under that single
/// acquisition. Do NOT call `persist_current_state_result` here; it re-acquires
/// the same non-reentrant tokio mutex and would deadlock.
///
/// FAILURE POLICY DIFFERS FROM RAISE-HAND, deliberately: NO rollback, matching the
/// #756 contract in `write_start_fresh_and_persist_to_dir_result`; but the transition
/// is reported even when the save fails, which DIVERGES from #756 (it returns `Err`
/// and reports nothing). Raise-hand can roll back because a user can raise a hand
/// again; this cannot be retried. `MenuGuard::scan_tick`
/// evaluates only on a CHANGED screen (`ScreenRowsSince::Unchanged` is an empty
/// arm) and a menu-parked terminal emits no further frames, and
/// `evaluate_logical_rows` has already set `MenuGuardSessionState.is_blocked`,
/// which this function cannot reach. A rollback would therefore freeze PTY
/// injection permanently with no toast, no Sidebar indicator and no disk row.
///
/// `settings` is the caller's already-resolved `AppSettings`, not a
/// `load_settings_for_cli()` call: this runs on the 250 ms scan loop, that loader
/// is uncached, and `scan_tick` already holds a fresh clone.
pub async fn set_blocked_menu_and_persist(
    mgr: &SessionManager,
    session_id: Uuid,
    message: String,
    now: chrono::DateTime<chrono::Utc>,
    settings: &crate::config::settings::AppSettings,
) -> BlockedMenuPersistOutcome {
    let dir = super::super::config_dir();
    let project_paths = match session_retention_project_paths(settings) {
        Ok(paths) => paths,
        Err(error) => {
            log::error!("Blocked-menu persistence blocked: {}", error);
            return BlockedMenuPersistOutcome::NotApplicable;
        }
    };
    let (outcome, error) = set_blocked_menu_and_persist_to_dir(
        mgr,
        session_id,
        message,
        now,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await;
    if let Some(e) = error {
        log::error!("Failed to persist blocked-menu set: {}", e);
    }
    outcome
}

/// Returns `(outcome, save_error)`. `outcome` is the caller's publish gate;
/// `save_error` is reported separately because it must not suppress the publish.
pub(super) async fn set_blocked_menu_and_persist_to_dir(
    mgr: &SessionManager,
    session_id: Uuid,
    message: String,
    now: chrono::DateTime<chrono::Utc>,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> (BlockedMenuPersistOutcome, Option<String>) {
    let manager = mgr.clone();
    let dir = dir.map(Path::to_path_buf);
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        set_blocked_menu_and_persist_to_dir_core(
            &manager,
            session_id,
            message,
            now,
            dir.as_deref(),
            roots.as_deref(),
        )
        .await
    })
    .await
}

pub(super) async fn set_blocked_menu_and_persist_to_dir_core(
    mgr: &SessionManager,
    session_id: Uuid,
    message: String,
    now: chrono::DateTime<chrono::Utc>,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> (BlockedMenuPersistOutcome, Option<String>) {
    let carrier_permit = acquire_carrier_mutation_permit().await;

    let communication = match mgr
        .set_blocked_menu_with_carrier_permit(&carrier_permit, session_id, message, now)
        .await
    {
        Some((true, communication)) => communication,
        Some((false, _)) => return (BlockedMenuPersistOutcome::Unchanged, None),
        None => return (BlockedMenuPersistOutcome::NotApplicable, None),
    };

    let Some(dir) = dir else {
        return (
            BlockedMenuPersistOutcome::Changed(communication),
            Some("Could not determine home directory".to_string()),
        );
    };
    let error = snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit)
        .await
        .err();
    (BlockedMenuPersistOutcome::Changed(communication), error)
}

/// #1669 - clear a visible blocked-menu communication and persist the result
/// atomically. Returns `true` when a visible blocked-menu was actually cleared,
/// which is exactly when the caller must publish `communication: null`. When
/// nothing was visible to clear it returns BEFORE any save, so a scan tick over
/// an unblocked session writes nothing.
///
/// Same failure policy as the set path: no rollback, and the clear is reported
/// even when the save fails. This DIVERGES from
/// `clear_user_input_transitions_and_persist_result`, which suppresses its clear
/// event on save failure so `list-sessions` and the UI agree. That trade is right
/// for a raise-hand lower, whose absence is invisible; it is wrong here, because
/// this event dismisses a STICKY toast for a menu already gone from the screen and
/// the clear has no retry path. Do not "fix" it back. Failures are logged.
///
/// Unlike the set path this resolves its own settings: it is not on the scan loop
/// (at most one clear per episode, per guard-disable, and per user click).
pub async fn clear_blocked_menu_and_persist(mgr: &SessionManager, session_id: Uuid) -> bool {
    let dir = super::super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = match session_retention_project_paths(&settings) {
        Ok(paths) => paths,
        Err(error) => {
            log::error!("Session persistence blocked: {}", error);
            return false;
        }
    };
    let (cleared, error) = clear_blocked_menu_and_persist_to_dir(
        mgr,
        session_id,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await;
    if let Some(e) = error {
        log::error!("Failed to persist blocked-menu clear: {}", e);
    }
    cleared
}

/// Returns `(cleared, save_error)`. `cleared` is the caller's publish gate;
/// `save_error` is reported separately because it must not suppress the clear.
pub(super) async fn clear_blocked_menu_and_persist_to_dir(
    mgr: &SessionManager,
    session_id: Uuid,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> (bool, Option<String>) {
    let manager = mgr.clone();
    let dir = dir.map(Path::to_path_buf);
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        clear_blocked_menu_and_persist_to_dir_core(
            &manager,
            session_id,
            dir.as_deref(),
            roots.as_deref(),
        )
        .await
    })
    .await
}

pub(super) async fn clear_blocked_menu_and_persist_to_dir_core(
    mgr: &SessionManager,
    session_id: Uuid,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> (bool, Option<String>) {
    let carrier_permit = acquire_carrier_mutation_permit().await;

    if !mgr
        .clear_blocked_menu_with_carrier_permit(&carrier_permit, session_id)
        .await
    {
        return (false, None);
    }
    let Some(dir) = dir else {
        return (true, Some("Could not determine home directory".to_string()));
    };
    match snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit).await {
        Ok(()) => (true, None),
        Err(e) => (true, Some(e)),
    }
}

/// #698: the user-input session-state transitions cleared by
/// `clear_user_input_transitions_and_persist_result`, reported so the caller can
/// decide whether to emit the raise-hand clear event.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClearedUserInputTransitions {
    /// `start_fresh_on_restore` flipped `true -> false` (#630/#631 re-arm).
    pub cleared_start_fresh: bool,
    /// A visible raise-hand was lowered (#698).
    pub cleared_raise_hand: bool,
}

/// #698: clear the user-input session-state transitions and persist the result
/// atomically with respect to all session persistence. `clear_fresh` gates the
/// `start_fresh_on_restore` re-arm to substantive post-boundary submissions
/// (#871); lowering any visible raise-hand remains unconditional.
///
/// Fix for the MEDIUM grinch finding: the two field clears previously ran in two
/// separate critical sections with an await between them, so a concurrent persist
/// could snapshot a half-applied state (`startFreshOnRestore: false` with a still
/// visible `raiseHand`). Here both fields flip in ONE `SessionManager` critical
/// section (`clear_user_input_transitions`), and the mutation plus its snapshot
/// save run under a single `sessions_save_lock()` acquisition, so no other
/// persistence caller can write an intermediate state.
///
/// Unlike the raise-hand path there is no rollback: a real user message means the
/// hand is lowered and the fresh intent re-armed regardless of whether this
/// snapshot reaches disk. The in-memory clear is therefore applied unconditionally
/// (even when the home directory cannot be resolved); on save failure it stands
/// and the next persist reconciles the file. The caller learns of any failure
/// through `Err` and, per existing behavior, suppresses the clear event.
pub async fn clear_user_input_transitions_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
    clear_fresh: bool,
) -> Result<ClearedUserInputTransitions, String> {
    let dir = super::super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    clear_user_input_transitions_and_persist_to_dir_result(
        mgr,
        session_id,
        clear_fresh,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

pub(super) async fn clear_user_input_transitions_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    clear_fresh: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<ClearedUserInputTransitions, String> {
    let manager = mgr.clone();
    let dir = dir.map(Path::to_path_buf);
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        clear_user_input_transitions_and_persist_to_dir_result_core(
            &manager,
            session_id,
            clear_fresh,
            dir.as_deref(),
            roots.as_deref(),
        )
        .await
    })
    .await
}

pub(super) async fn clear_user_input_transitions_and_persist_to_dir_result_core(
    mgr: &SessionManager,
    session_id: Uuid,
    clear_fresh: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<ClearedUserInputTransitions, String> {
    let carrier_permit = acquire_carrier_mutation_permit().await;

    // Mutate FIRST and unconditionally: the user typed, so the hand is lowered
    // and, when gated, the fresh intent is re-armed even if we cannot persist.
    // Both fields flip in one critical section, so no snapshot can capture a
    // half-applied state.
    let (cleared_start_fresh, cleared_raise_hand) = mgr
        .clear_user_input_transitions_with_carrier_permit(&carrier_permit, session_id, clear_fresh)
        .await;
    let cleared = ClearedUserInputTransitions {
        cleared_start_fresh,
        cleared_raise_hand,
    };

    if cleared_start_fresh || cleared_raise_hand {
        let dir = dir.ok_or("Could not determine home directory")?;
        snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit).await?;
    }

    Ok(cleared)
}

/// (#756) Stamp the durable fresh intent (AC-driven clear boundary) and persist
/// the result atomically with respect to all session persistence. Returns
/// Ok(true) iff the field transitioned false -> true (a snapshot was saved).
/// Same lock discipline as `clear_user_input_transitions_and_persist_result`:
/// mutation + snapshot + save under a single `sessions_save_lock()` acquisition,
/// so no concurrent persist can write an intermediate state.
pub async fn set_start_fresh_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
) -> Result<bool, String> {
    let dir = super::super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    write_start_fresh_and_persist_to_dir_result(
        mgr,
        session_id,
        true,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

/// (#756) Drop the durable fresh intent (AC injected post-boundary CONTENT) and
/// persist. Returns Ok(true) iff the field transitioned true -> false.
/// DELIBERATELY NARROW: touches only `start_fresh_on_restore`; never raise-hand,
/// silence, or badge state (this is NOT the user-input choke point).
pub async fn clear_start_fresh_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
) -> Result<bool, String> {
    let dir = super::super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    write_start_fresh_and_persist_to_dir_result(
        mgr,
        session_id,
        false,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

/// (#756) Shared core for the fresh-intent stamp/drop persist wrappers. Like the
/// #698 helper, the in-memory mutation stands even if the save fails (Err
/// reports the save failure; callers log a warn, no rollback).
pub(super) async fn write_start_fresh_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    value: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<bool, String> {
    let manager = mgr.clone();
    let dir = dir.map(Path::to_path_buf);
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        write_start_fresh_and_persist_to_dir_result_core(
            &manager,
            session_id,
            value,
            dir.as_deref(),
            roots.as_deref(),
        )
        .await
    })
    .await
}

pub(super) async fn write_start_fresh_and_persist_to_dir_result_core(
    mgr: &SessionManager,
    session_id: Uuid,
    value: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<bool, String> {
    let carrier_permit = acquire_carrier_mutation_permit().await;
    let changed = if value {
        mgr.set_start_fresh_on_restore_if_unset_with_carrier_permit(&carrier_permit, session_id)
            .await
    } else {
        mgr.clear_start_fresh_on_restore_if_set_with_carrier_permit(&carrier_permit, session_id)
            .await
    };
    if changed {
        let dir = dir.ok_or("Could not determine home directory")?;
        snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit).await?;
    }
    Ok(changed)
}
