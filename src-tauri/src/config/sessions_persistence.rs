mod carrier;
#[cfg(test)]
pub(crate) use carrier::poison_discard_slot_for_test;
#[cfg(test)]
pub(crate) use carrier::restore_test_carrier_scopes_are_empty;
pub(crate) use carrier::{observe_discard_veto, DiscardVetoObservation, DiscardVetoReason};
mod live_update;
mod orphan;
mod persist;
mod project_scope;
mod record;
mod replacement;
mod retained_prompt;
mod store;
mod write;

use std::collections::HashMap;

use crate::session::session::{SessionCommunicationKind, SessionStatus, TEMP_SESSION_PREFIX};

#[cfg(test)]
use crate::session::manager::acquire_carrier_mutation_permit;
pub(crate) use carrier::{
    abort_owned_carrier_replacement, begin_carrier_replacement, classify_create_carrier_scope,
    classify_lifecycle_carrier_scope, current_protected_carrier, finish_carrier_replacement,
    manual_discard_carrier, register_published_carrier, renew_carrier_replacement,
    restore_carrier_scopes_are_empty, retained_carrier_for_replacement, retire_carrier_for_discard,
    retire_manual_discard_carrier, unconfirmed_replacement_work_waits, with_confirmed_carrier,
    CarrierCheckpointTransition, CreateCarrierScope, InitialCarrierIntent, PublishedCarrier,
};
pub use live_update::{
    clear_blocked_menu_and_persist, clear_start_fresh_and_persist_result,
    clear_user_input_transitions_and_persist_result, raise_hand_and_persist_result,
    set_blocked_menu_and_persist, set_start_fresh_and_persist_result, BlockedMenuPersistOutcome,
    ClearedUserInputTransitions, RaiseHandPersistOutcome,
};
pub(crate) use orphan::append_orphan_archive_record;
#[cfg(test)]
use orphan::persist_prepared_locked;
#[cfg(test)]
pub(crate) use orphan::{
    orphan_counters, orphan_warned_len, reset_orphan_archived, reset_orphan_counters,
    reset_orphan_warned, set_test_orphan_archive_cap,
};
#[cfg(test)]
pub(crate) use persist::persist_current_state_to_dir_result;
pub use persist::{
    load_sessions_purging_outside_project_paths, persist_current_state,
    persist_current_state_prune_dormant, persist_current_state_result, persist_merging_failed,
    persist_merging_failed_result, snapshot_sessions,
};
pub(crate) use persist::{
    persist_initial_carrier, purge_sessions_outside_project_paths_in_dir, strip_auto_injected_args,
};
pub(crate) use project_scope::{
    default_creation_gate_enforcement, enforce_creation_gate, is_under_normalized_archived_roots,
    normalize_project_roots, raw_project_path_containing, session_retention_project_paths,
    working_directory_under_any_project_path, CreationGateEnforcement,
};
#[cfg(test)]
pub(crate) use project_scope::{normalize_call_count, reset_normalize_call_count};
#[cfg(test)]
use project_scope::{FILTER_PROJECT_PATHS_THREAD_IDS, NORMALIZE_CALLS};
pub use record::{CodingAgentBindingSeed, PersistedSession};
pub(crate) use replacement::{
    prepare_owned_replacement_publication, publish_owned_replacement,
    reconcile_owned_replacement_readback, OwnedReplacementPublicationOutcome,
};
pub(crate) use retained_prompt::{
    block_duplicate_restore_prompt_domains, initialize_prompt_retention, prompt_key,
};
#[cfg(test)]
pub(crate) use store::load_sessions_from_dir;
#[cfg(any(test, debug_assertions))]
pub use store::load_sessions_raw_from_dir_for_test;
pub use store::{load_sessions, load_sessions_raw};
#[cfg(test)]
pub use write::save_sessions;
#[cfg(test)]
use write::save_sessions_to_dir;
pub(crate) use write::{rename_with_retry, run_owned_persist_core};
#[cfg(test)]
pub(crate) use write::{save_sessions_to_dir_for_test, FixturePublicationActionKind};

#[cfg(test)]
use carrier::{publication_slot, ProtectionPhase, PublicationCheckpoint};
#[cfg(test)]
use live_update::{
    clear_blocked_menu_and_persist_to_dir, clear_user_input_transitions_and_persist_to_dir_result,
    raise_hand_and_persist_to_dir_result, set_blocked_menu_and_persist_to_dir,
    write_start_fresh_and_persist_to_dir_result,
};
#[cfg(test)]
use orphan::{prepare_persist_snapshot, OrphanSweepCounts, ORPHAN_ARCHIVE_FILENAME};
#[cfg(test)]
use persist::{
    load_sessions_purging_outside_project_paths_in_dir,
    persist_current_state_to_dir_for_project_paths_result,
    purge_sessions_outside_project_paths_in_dir_locked, sanitize_failed_recoverable,
};
#[cfg(test)]
use project_scope::{
    filter_sessions_for_normalized_roots, filter_sessions_for_project_paths,
    filter_sessions_for_project_paths_blocking, normalize_for_project_compare,
    validate_session_creation_cwd,
};
#[cfg(all(test, windows))]
use record::deduplicate;
#[cfg(test)]
use retained_prompt::{
    flush_retained_prompts_locked, flush_retained_prompts_locked_with_probe, load_retained_prompts,
    RETAINED_PROMPTS,
};
#[cfg(test)]
use store::RENAME_ATTEMPTS;
#[cfg(test)]
use store::{
    load_sessions_from_path, load_sessions_from_path_with_settings, load_sessions_raw_from_path,
};
#[cfg(test)]
use write::{
    confirm_publication_readback, rename_and_publish, sessions_save_lock, settle_rename_attempt,
    PersistMode, RenamePublicationAttempt, SettledRenameAttempt,
};

#[cfg(test)]
pub(crate) use retained_prompt::initialize_prompt_retention_in_dir;

// ── Issue #206: working-state derivation from sessions.json ──────────

pub(crate) struct CandidateSession {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: SessionStatus,
    pub(crate) waiting_for_input: bool,
    pub(crate) context_percent: Option<u8>,
    pub(crate) agent_id: Option<String>,
    pub(crate) effective_profile: Option<String>,
    /// #1669 - the row is parked on a blocking terminal menu
    /// (`communication.kind == BlockedMenu && visible`).
    pub(crate) blocked_menu: bool,
    /// #1669 - the agent-specific notice for that menu, when the row carries one.
    pub(crate) blocked_menu_message: Option<String>,
}

pub(crate) struct PeerStatus {
    pub(crate) working: bool,
    pub(crate) session_status: &'static str,
    pub(crate) status_legacy: &'static str,
    pub(crate) session_id: Option<String>,
    pub(crate) waiting_for_input: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) context_percent: Option<u8>,
    pub(crate) agent_id: Option<String>,
    pub(crate) effective_profile: Option<String>,
    /// #1669 - the chosen session is parked on a blocking terminal menu.
    pub(crate) blocked_menu: bool,
    /// #1669 - the agent-specific notice for that menu, when there is one.
    pub(crate) blocked_menu_message: Option<String>,
}

impl PeerStatus {
    fn none() -> Self {
        PeerStatus {
            working: false,
            session_status: "none",
            status_legacy: "unknown",
            session_id: None,
            waiting_for_input: false,
            exit_code: None,
            context_percent: None,
            agent_id: None,
            effective_profile: None,
            blocked_menu: false,
            blocked_menu_message: None,
        }
    }
}

pub(crate) fn norm_path(path: &str) -> String {
    let stripped = path.strip_prefix(r"\\?\").unwrap_or(path);
    stripped
        .replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

pub(crate) fn canon_or_norm(path: &str) -> String {
    match crate::filesystem::directories::canonical_peer_cwd(std::path::Path::new(path)) {
        Ok(canon) => norm_path(&canon.to_string_lossy()),
        Err(_) => norm_path(path),
    }
}

/// Pure inner: build the cwd → candidate index from a slice of persisted rows.
/// Exposed (private) so unit tests can drive it without touching the filesystem.
pub(crate) fn build_session_index_from(
    rows: &[PersistedSession],
) -> HashMap<String, Vec<CandidateSession>> {
    let mut index: HashMap<String, Vec<CandidateSession>> = HashMap::new();
    for ps in rows {
        if ps.name.starts_with(TEMP_SESSION_PREFIX) {
            continue;
        }
        let (Some(id), Some(status)) = (ps.id.clone(), ps.status.clone()) else {
            continue;
        };
        let key = canon_or_norm(&ps.working_directory);
        // #1669: a blocked terminal is a visible BlockedMenu communication. RaiseHand
        // and cleared/invisible communications are not blocked terminals.
        let blocked_menu = ps
            .communication
            .as_ref()
            .is_some_and(|c| c.kind == SessionCommunicationKind::BlockedMenu && c.visible);
        let blocked_menu_message = if blocked_menu {
            ps.communication.as_ref().and_then(|c| c.message.clone())
        } else {
            None
        };
        index.entry(key).or_default().push(CandidateSession {
            id,
            name: ps.name.clone(),
            status,
            waiting_for_input: ps.waiting_for_input.unwrap_or(false),
            context_percent: ps.context_percent,
            agent_id: ps.agent_id.clone(),
            effective_profile: ps.effective_profile.clone(),
            blocked_menu,
            blocked_menu_message,
        });
    }
    index
}

/// Production entry point: read sessions.json and build the index.
pub(crate) fn build_session_index() -> HashMap<String, Vec<CandidateSession>> {
    build_session_index_from(&load_sessions_raw())
}

/// Priority: waiting(4) > active(3) > running(2) > idle(1) > exited(0).
/// Uses `match &c.status` to avoid moving the non-Copy `SessionStatus`
/// (matches the proven pattern in `list_sessions.rs:status_tag`).
fn priority(c: &CandidateSession) -> u8 {
    if c.waiting_for_input {
        return 4;
    }
    match &c.status {
        SessionStatus::Active => 3,
        SessionStatus::Running => 2,
        SessionStatus::Idle => 1,
        SessionStatus::Exited(_) => 0,
    }
}

/// Compute a peer's working state.
///
/// `expected_name`:
///   - `Some("wg/agent")` for WG peers → filters candidates by exact session
///     name to mirror the sidebar's `findSessionByName` predicate.
///   - `None` for non-WG peers → cwd-only match (no sidebar predicate to
///     mirror; see §6.1 and §10.8 of the plan).
pub(crate) fn compute_peer_status(
    peer_path: &str,
    expected_name: Option<&str>,
    index: &HashMap<String, Vec<CandidateSession>>,
) -> PeerStatus {
    let key = canon_or_norm(peer_path);
    let Some(candidates) = index.get(&key) else {
        return PeerStatus::none();
    };

    let filtered: Vec<&CandidateSession> = match expected_name {
        Some(name) => candidates.iter().filter(|c| c.name == name).collect(),
        None => candidates.iter().collect(),
    };

    let Some(chosen) = filtered.iter().copied().max_by_key(|c| priority(c)) else {
        return PeerStatus::none();
    };

    let (session_status, status_legacy, working, exit_code): (&str, &str, bool, Option<i32>) =
        if chosen.waiting_for_input {
            ("waiting", "unknown", false, None)
        } else {
            match &chosen.status {
                SessionStatus::Active => ("active", "active", true, None),
                SessionStatus::Running => ("running", "active", true, None),
                SessionStatus::Idle => ("idle", "unknown", false, None),
                SessionStatus::Exited(n) => ("exited", "unknown", false, Some(*n)),
            }
        };

    // #1669: `snapshot_sessions` deliberately preserves `communication` for exited
    // rows (#747), so a crash between the block and the exit can leave a stale
    // BlockedMenu on a session whose terminal is gone. A terminal that exited is
    // not waiting on a menu, so suppress the pair rather than report a false
    // positive alongside sessionStatus == "exited".
    let blocked_menu = chosen.blocked_menu && !matches!(chosen.status, SessionStatus::Exited(_));
    let blocked_menu_message = if blocked_menu {
        chosen.blocked_menu_message.clone()
    } else {
        None
    };

    PeerStatus {
        working,
        session_status,
        status_legacy,
        session_id: Some(chosen.id.clone()),
        waiting_for_input: chosen.waiting_for_input,
        exit_code,
        context_percent: chosen.context_percent,
        agent_id: chosen.agent_id.clone(),
        effective_profile: chosen.effective_profile.clone(),
        blocked_menu,
        blocked_menu_message,
    }
}

#[cfg(test)]
mod tests;
