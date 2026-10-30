use super::super::wire::{
    ApplyCodingAgentProfileSelectionRequest, ApplyCodingAgentProfileSelectionResult, AssignmentMode,
};
use super::ProfileAssignmentWrites;

pub(crate) fn assignment_publication(
    request: ApplyCodingAgentProfileSelectionRequest,
    normalized_profile: String,
    target_fingerprint: String,
    writes: ProfileAssignmentWrites,
    restarted_session_ids: Vec<String>,
    destroyed_but_not_recreated_session_ids: Vec<String>,
    locked_after_apply_count: Option<usize>,
) -> (ApplyCodingAgentProfileSelectionResult, serde_json::Value) {
    let ProfileAssignmentWrites {
        updated_replica_paths,
        newly_protected_paths,
        skipped_locked_paths,
        invalid_paths,
        write_succeeded_keys: _,
        errors,
        warnings,
        force_applied,
    } = writes;
    let result = ApplyCodingAgentProfileSelectionResult {
        scope: request.scope.clone(),
        updated_count: updated_replica_paths.len(),
        restarted_count: restarted_session_ids.len(),
        updated_replica_paths,
        restarted_session_ids,
        destroyed_but_not_recreated_session_ids,
        target_fingerprint,
        warnings,
        errors,
        newly_protected_paths,
        skipped_locked_paths,
        invalid_paths,
        locked_after_apply_count,
        force_applied,
    };
    let operation = match request.assignment_mode {
        AssignmentMode::Ordinary => "assign",
        AssignmentMode::AssignAndLock => "assignAndLock",
    };
    let payload = serde_json::json!({
        "scope": request.scope,
        "operation": operation,
        "codingAgentId": request.coding_agent_id,
        "profile": normalized_profile,
        "updatedCount": result.updated_count,
        "restartedCount": result.restarted_count,
        "targetFingerprint": &result.target_fingerprint,
        "affectedPaths": &result.updated_replica_paths,
        "newlyProtectedCount": result.newly_protected_paths.len(),
        "skippedLockedCount": result.skipped_locked_paths.len(),
        "errors": &result.errors,
    });
    (result, payload)
}
