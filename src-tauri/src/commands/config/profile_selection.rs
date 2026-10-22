use super::read_agent_file_source_inner;

mod wire;
pub use wire::{
    ApplyCodingAgentProfileSelectionRequest, ApplyCodingAgentProfileSelectionResult,
    ApplySelectionLockRemovalRequest, ApplySelectionLockRemovalResult, AssignmentMode,
    ConflictDecision, GetReplicaSelectionDefaultRequest, PreviewCodingAgentProfileSelectionRequest,
    PreviewCodingAgentProfileSelectionResult, PreviewSelectionLockRemovalRequest,
    PreviewSelectionLockRemovalResult, ProfileAssignmentError, ProfileAssignmentScope,
    ProfileAssignmentTarget, ReplicaSelectionDefaultResult, SavedPair, SelectionDecisionProjection,
    SelectionDecisions, SelectionDefault, SelectionState, SetReplicaSelectionDefaultRequest,
};

mod assignment;
pub use assignment::{
    __cmd__apply_coding_agent_profile_selection, __cmd__preview_coding_agent_profile_selection,
    __tauri_command_name_apply_coding_agent_profile_selection,
    __tauri_command_name_preview_coding_agent_profile_selection,
    apply_coding_agent_profile_selection, preview_coding_agent_profile_selection,
};
pub(crate) use assignment::{
    apply_coding_agent_profile_selection_inner, preview_coding_agent_profile_selection_inner,
};

mod target;
pub(crate) use target::{
    canonical_compare_key, classify_restart_failure, enumerate_profile_assignment_targets,
    normalize_profile_letter_for_assignment, prevalidate_profile_assignment_restarts,
    profile_assignment_requires_explicit_confirmation, validate_assignment_decision_scope,
    validate_profile_assignment_confirmation, validate_profile_assignment_request,
};
pub use target::{ScopeFault, ScopeFaultCode};

mod fingerprint;
pub(crate) use fingerprint::{
    assignment_expected_fingerprint, assignment_snapshot, assignment_write_intent,
    assignment_write_paths, refresh_locked_count, resolve_assignment_conflict_decision,
    selection_error_code,
};

mod lock_removal;
pub use lock_removal::{
    __cmd__apply_selection_lock_removal, __cmd__preview_selection_lock_removal,
    __tauri_command_name_apply_selection_lock_removal,
    __tauri_command_name_preview_selection_lock_removal, apply_selection_lock_removal,
    preview_selection_lock_removal,
};
pub(crate) use lock_removal::{
    apply_selection_lock_removal_inner, preview_selection_lock_removal_inner,
};

mod selection_default;
#[cfg(test)]
pub(crate) use fingerprint::{
    assignment_fingerprint, selection_default_fingerprint, source_selection_fingerprint,
};
pub use selection_default::{
    __cmd__get_replica_selection_default, __cmd__set_replica_selection_default,
    __tauri_command_name_get_replica_selection_default,
    __tauri_command_name_set_replica_selection_default, get_replica_selection_default,
    set_replica_selection_default,
};
pub(crate) use selection_default::{
    get_replica_selection_default_inner, set_replica_selection_default_inner,
};
#[cfg(test)]
pub(crate) use target::{
    build_profile_assignment_target, canonical_real_dir, collect_kind_replica_dirs,
    collect_replica_dirs_in_workgroup, target_pair, CandidateDirs, ProfileCandidate,
    ProfileTargetEnumeration,
};
