mod carrier;
#[cfg(test)]
pub(crate) use carrier::restore_test_carrier_scopes_are_empty;
mod live_update;
mod operation;
mod orphan;
mod persist;
mod project_scope;
mod record;
mod replacement;
mod retained_prompt;
mod store;
mod write;

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
#[expect(
    unused_imports,
    reason = "P01 additive observation API activates in later phases"
)]
pub(crate) use operation::{
    run_observed_owned_core, OperationScope, OwnedTaskSet, OwnedTaskTicket, TaskExit,
};
#[cfg(test)]
#[expect(
    unused_imports,
    reason = "lower producer probes are consumed in later phases"
)]
pub(crate) use operation::{OperationProbe, ProbeContext, ProbeEvent};
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

#[cfg(test)]
mod tests;
