use std::hash::Hash;

use super::ScopeFault;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ProfileAssignmentScope {
    Replica,
    Kind,
    Workgroup,
}

/// #1941 - requested publication policy. `Ordinary` preserves the stored lock
/// flag exactly; `AssignAndLock` publishes the pair and sets the flag, and is
/// the only mode that offers a reviewed force for protected replicas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AssignmentMode {
    #[default]
    Ordinary,
    AssignAndLock,
}

/// #1941 - explicit review decision for a bulk assign-and-lock that found
/// protected replicas. There is deliberately no default: a conflict needs a
/// choice, so the backend can never force a lock silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictDecision {
    UnlockedOnly,
    ForceReviewed,
}

/// #1941 - strict protection state of a replica, as read by #1939. `Invalid`
/// is a diagnostic, never a permissive default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionState {
    Unlocked,
    Locked,
    Invalid,
}

/// #1941 - the protected pair as exposed on the wire. Both fields are `null`
/// when no complete pair is stored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPair {
    pub coding_agent_id: Option<String>,
    pub requested_profile: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAssignmentTarget {
    pub workgroup_name: String,
    pub workgroup_path: String,
    pub replica_name: String,
    pub replica_path: String,
    pub identity_path: String,
    pub origin_project: Option<String>,
    pub live_session_ids: Vec<String>,
    /// #1941 - the complete stored pair, or `null` when absent/invalid.
    pub saved_pair: Option<SavedPair>,
    /// #1941 - strict protection state; never a synthesized `false`.
    pub selection_state: SelectionState,
    /// #1941 - the strict-read diagnostic when `selection_state` is invalid.
    pub selection_error: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCodingAgentProfileSelectionRequest {
    pub target_replica_path: String,
    pub coding_agent_id: String,
    pub profile: String,
    pub scope: ProfileAssignmentScope,
    #[serde(default)]
    pub restart_sessions: bool,
    /// #1941 - absent keeps the legacy ordinary semantics.
    #[serde(default)]
    pub assignment_mode: AssignmentMode,
}

/// #1941 - one explicit conflict outcome as offered by a preview.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionDecisionProjection {
    pub fingerprint: String,
    pub eligible_paths: Vec<String>,
    pub eligible_count: usize,
    pub skipped_locked_count: usize,
    pub live_session_count: usize,
}

/// #1941 - both reviewable outcomes; present only when a bulk
/// assign-and-lock preview actually found protected candidates. Each carries
/// its own fingerprint, so the client echoes exactly the reviewed decision.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionDecisions {
    pub unlocked_only: SelectionDecisionProjection,
    pub force_reviewed: SelectionDecisionProjection,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCodingAgentProfileSelectionResult {
    pub scope: ProfileAssignmentScope,
    pub target_count: usize,
    pub live_session_count: usize,
    pub target_fingerprint: String,
    pub requires_explicit_confirmation: bool,
    pub targets: Vec<ProfileAssignmentTarget>,
    pub warnings: Vec<String>,
    /// #1941 - false when scoped membership could not be established from the
    /// directory walk (unreadable entries / unprovable identities).
    pub counts_complete: bool,
    pub candidate_count: usize,
    pub protected_count: usize,
    pub invalid_count: usize,
    /// #1941 - valid protected candidates for a bulk assign-and-lock; zero for
    /// every other mode.
    pub conflict_count: usize,
    pub decisions: Option<SelectionDecisions>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCodingAgentProfileSelectionRequest {
    pub target_replica_path: String,
    pub coding_agent_id: String,
    pub profile: String,
    pub scope: ProfileAssignmentScope,
    pub restart_sessions: bool,
    pub confirmed_target_fingerprint: Option<String>,
    pub typed_confirmation: Option<String>,
    /// #1941 - absent keeps the legacy ordinary semantics.
    #[serde(default)]
    pub assignment_mode: AssignmentMode,
    /// #1941 - required only for a bulk assign-and-lock that found protected
    /// candidates. Rejected in every other mode/scope combination.
    #[serde(default)]
    pub conflict_decision: Option<ConflictDecision>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAssignmentError {
    pub code: String,
    pub message: String,
    pub session_ids: Vec<String>,
    pub replica_paths: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCodingAgentProfileSelectionResult {
    pub scope: ProfileAssignmentScope,
    pub updated_count: usize,
    pub restarted_count: usize,
    pub updated_replica_paths: Vec<String>,
    pub restarted_session_ids: Vec<String>,
    pub destroyed_but_not_recreated_session_ids: Vec<String>,
    pub target_fingerprint: String,
    pub warnings: Vec<String>,
    pub errors: Vec<ProfileAssignmentError>,
    /// #1941 - false-to-true lock flips only.
    pub newly_protected_paths: Vec<String>,
    /// #1941 - valid protected replicas deliberately skipped by the policy.
    pub skipped_locked_paths: Vec<String>,
    /// #1941 - candidates whose strict protection state is invalid.
    pub invalid_paths: Vec<String>,
    /// #1941 - protected replicas left after the operation; `null` when the
    /// refresh cannot establish the total.
    pub locked_after_apply_count: Option<usize>,
    /// #1941 - true only when a reviewed force actually overwrote a protected
    /// target, never merely because a decision was selected.
    pub force_applied: bool,
}

// ── #1941 selection-lock removal and Matrix default wire contract ──────────

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewSelectionLockRemovalRequest {
    pub target_replica_path: String,
    pub scope: ProfileAssignmentScope,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplySelectionLockRemovalRequest {
    pub target_replica_path: String,
    pub scope: ProfileAssignmentScope,
    pub confirmed_target_fingerprint: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetReplicaSelectionDefaultRequest {
    pub target_replica_path: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetReplicaSelectionDefaultRequest {
    pub target_replica_path: String,
    pub coding_agent_id: String,
    pub requested_profile: String,
    pub selection_locked: bool,
    pub confirmed_default_fingerprint: String,
}

/// #1941 - Matrix creation default (`tooling.replicaSelectionDefault`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionDefault {
    pub coding_agent_id: String,
    pub requested_profile: String,
    pub selection_locked: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSelectionLockRemovalResult {
    pub scope: ProfileAssignmentScope,
    pub target_fingerprint: String,
    pub candidate_count: usize,
    pub counts_complete: bool,
    pub protected_count: usize,
    pub already_unlocked_count: usize,
    pub invalid_count: usize,
    pub targets: Vec<ProfileAssignmentTarget>,
    pub warnings: Vec<String>,
    pub scope_faults: Vec<ScopeFault>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplySelectionLockRemovalResult {
    pub scope: ProfileAssignmentScope,
    pub target_fingerprint: String,
    pub removed_count: usize,
    pub removed_replica_paths: Vec<String>,
    pub already_unlocked_paths: Vec<String>,
    pub failed_replica_paths: Vec<String>,
    pub remaining_protected_count: Option<usize>,
    pub candidate_count: usize,
    pub counts_complete: bool,
    pub invalid_count: usize,
    pub errors: Vec<ProfileAssignmentError>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplicaSelectionDefaultResult {
    pub target_replica_path: String,
    pub matrix_path: String,
    /// `null` only when the Matrix has no stored default (old creation
    /// behavior); a malformed default is an error, never a silent `null`.
    pub default: Option<SelectionDefault>,
    pub default_fingerprint: String,
    pub warnings: Vec<String>,
}
