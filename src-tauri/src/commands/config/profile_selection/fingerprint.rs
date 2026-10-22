use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::config::settings::SettingsState;
use crate::session::session::SessionInfo;

use super::read_agent_file_source_inner;

use super::wire::{
    AssignmentMode, ConflictDecision, ProfileAssignmentScope, SelectionDecisionProjection,
    SelectionDecisions, SelectionState,
};

use super::target::{canonical_compare_key, target_pair, ProfileTargetEnumeration};

// ── #1941 selection fingerprints ───────────────────────────────────────────
//
// Fingerprints are lowercase SHA-256 of a typed serde tuple with a schema tag.
// The complete candidate list (pair, flag, identity state and the live-session
// provider/profile identity) plus the eligible membership are part of the
// tuple, so a flag, pair, membership, anchor-identity or relevant session change
// invalidates an old approval. A conflicting bulk assign-and-lock offers one
// hash per decision; a no-conflict operation binds decision `null`.

pub(crate) const SELECTION_LOCK_FINGERPRINT_SCHEMA: &str = "selection-lock-v1";
pub(crate) const SELECTION_DEFAULT_FINGERPRINT_SCHEMA: &str = "replica-selection-default-v1";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionFingerprintPair {
    coding_agent_id: String,
    requested_profile: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionFingerprintSession {
    id: String,
    coding_agent_id: Option<String>,
    requested_profile: Option<String>,
    effective_profile: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionFingerprintCandidate {
    path: String,
    matrix: String,
    pair: Option<SelectionFingerprintPair>,
    locked: bool,
    invalid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    configuration_ref: Option<crate::config::settings::ConfigurationRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_override_source: Option<String>,
    live_sessions: Vec<SelectionFingerprintSession>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionAssignmentFingerprint {
    schema: &'static str,
    operation: &'static str,
    anchor: String,
    matrix: String,
    scope: &'static str,
    coding_agent_id: String,
    requested_profile: String,
    restart_sessions: bool,
    decision: Option<&'static str>,
    candidates: Vec<SelectionFingerprintCandidate>,
    eligible_paths: Vec<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionRemovalFingerprint {
    schema: &'static str,
    operation: &'static str,
    anchor: String,
    matrix: String,
    scope: &'static str,
    candidates: Vec<SelectionFingerprintCandidate>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionFingerprintDefault {
    coding_agent_id: String,
    requested_profile: String,
    selection_locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    configuration_ref: Option<crate::config::settings::ConfigurationRef>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionDefaultFingerprint {
    schema: &'static str,
    matrix: String,
    default: Option<SelectionFingerprintDefault>,
}

pub(crate) fn sha256_hex<T: serde::Serialize>(value: &T) -> Result<String, String> {
    let serialized = serde_json::to_vec(value)
        .map_err(|e| format!("Failed to serialize selection fingerprint: {e}"))?;
    Ok(Sha256::digest(&serialized)
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect())
}

pub(crate) fn selection_scope_wire(scope: &ProfileAssignmentScope) -> &'static str {
    match scope {
        ProfileAssignmentScope::Replica => "replica",
        ProfileAssignmentScope::Kind => "kind",
        ProfileAssignmentScope::Workgroup => "workgroup",
    }
}

pub(crate) fn conflict_decision_wire(decision: ConflictDecision) -> &'static str {
    match decision {
        ConflictDecision::UnlockedOnly => "unlockedOnly",
        ConflictDecision::ForceReviewed => "forceReviewed",
    }
}

pub(crate) fn live_session_identities(
    sessions: &[SessionInfo],
) -> BTreeMap<String, SelectionFingerprintSession> {
    sessions
        .iter()
        .map(|session| {
            (
                session.id.clone(),
                SelectionFingerprintSession {
                    id: session.id.clone(),
                    coding_agent_id: session.agent_id.clone(),
                    requested_profile: session.requested_profile.clone(),
                    effective_profile: session.effective_profile.clone(),
                },
            )
        })
        .collect()
}

pub(crate) fn fingerprint_candidates(
    enumeration: &ProfileTargetEnumeration,
    sessions: &[SessionInfo],
) -> Vec<SelectionFingerprintCandidate> {
    let live = live_session_identities(sessions);
    let mut candidates: Vec<SelectionFingerprintCandidate> = enumeration
        .candidates
        .iter()
        .map(|candidate| {
            let target = &candidate.target;
            let mut live_sessions: Vec<SelectionFingerprintSession> = target
                .live_session_ids
                .iter()
                .filter_map(|id| live.get(id).cloned())
                .collect();
            live_sessions.sort_by(|a, b| a.id.cmp(&b.id));
            SelectionFingerprintCandidate {
                path: canonical_compare_key(Path::new(&target.replica_path)),
                matrix: canonical_compare_key(&candidate.matrix_dir),
                pair: target_pair(target).map(|pair| SelectionFingerprintPair {
                    coding_agent_id: pair.coding_agent_id,
                    requested_profile: pair.requested_profile,
                }),
                locked: target.selection_state == SelectionState::Locked,
                invalid: target.selection_state == SelectionState::Invalid,
                configuration_ref: candidate
                    .expected
                    .as_ref()
                    .and_then(|expected| expected.configuration_ref.clone()),
                profile_override_source: candidate
                    .expected
                    .as_ref()
                    .and_then(|expected| expected.profile_override_source.clone()),
                live_sessions,
            }
        })
        .collect();
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    candidates
}

pub(crate) struct EligibilitySets {
    unlocked_paths: Vec<String>,
    valid_paths: Vec<String>,
}

pub(crate) fn eligibility_sets(enumeration: &ProfileTargetEnumeration) -> EligibilitySets {
    let mut unlocked_paths = Vec::new();
    let mut valid_paths = Vec::new();
    for candidate in &enumeration.candidates {
        let path = canonical_compare_key(Path::new(&candidate.target.replica_path));
        match candidate.target.selection_state {
            SelectionState::Invalid => {}
            SelectionState::Unlocked => {
                unlocked_paths.push(path.clone());
                valid_paths.push(path);
            }
            SelectionState::Locked => valid_paths.push(path),
        }
    }
    unlocked_paths.sort();
    valid_paths.sort();
    EligibilitySets {
        unlocked_paths,
        valid_paths,
    }
}

pub(crate) fn live_session_count_for(
    enumeration: &ProfileTargetEnumeration,
    eligible_paths: &[String],
) -> usize {
    enumeration
        .candidates
        .iter()
        .filter(|candidate| {
            eligible_paths.contains(&canonical_compare_key(Path::new(
                &candidate.target.replica_path,
            )))
        })
        .map(|candidate| candidate.target.live_session_ids.len())
        .sum()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn assignment_fingerprint(
    enumeration: &ProfileTargetEnumeration,
    sessions: &[SessionInfo],
    scope: &ProfileAssignmentScope,
    operation: &'static str,
    coding_agent_id: &str,
    requested_profile: &str,
    restart_sessions: bool,
    decision: Option<ConflictDecision>,
    eligible_paths: &[String],
) -> Result<String, String> {
    let tuple = SelectionAssignmentFingerprint {
        schema: SELECTION_LOCK_FINGERPRINT_SCHEMA,
        operation,
        anchor: canonical_compare_key(&enumeration.anchor_dir),
        matrix: canonical_compare_key(&enumeration.anchor_matrix_dir),
        scope: selection_scope_wire(scope),
        coding_agent_id: coding_agent_id.to_string(),
        requested_profile: requested_profile.to_string(),
        restart_sessions,
        decision: decision.map(conflict_decision_wire),
        candidates: fingerprint_candidates(enumeration, sessions),
        eligible_paths: eligible_paths.to_vec(),
    };
    sha256_hex(&tuple)
}

/// #1941 - a removal fingerprint binds only the operation, scope, anchor and
/// each candidate's pair/flag/identity state; removal has no session operation.
pub(crate) fn removal_fingerprint(
    enumeration: &ProfileTargetEnumeration,
    scope: &ProfileAssignmentScope,
) -> Result<String, String> {
    let tuple = SelectionRemovalFingerprint {
        schema: SELECTION_LOCK_FINGERPRINT_SCHEMA,
        operation: "unlock",
        anchor: canonical_compare_key(&enumeration.anchor_dir),
        matrix: canonical_compare_key(&enumeration.anchor_matrix_dir),
        scope: selection_scope_wire(scope),
        candidates: fingerprint_candidates(enumeration, &[]),
    };
    sha256_hex(&tuple)
}

pub(crate) fn selection_default_fingerprint(
    matrix_dir: &Path,
    default: Option<&crate::config::coding_agent_profiles::ReplicaSelectionDefault>,
) -> Result<String, String> {
    let tuple = SelectionDefaultFingerprint {
        schema: SELECTION_DEFAULT_FINGERPRINT_SCHEMA,
        matrix: canonical_compare_key(matrix_dir),
        default: default.map(|default| SelectionFingerprintDefault {
            coding_agent_id: default.coding_agent_id.clone(),
            requested_profile: default.requested_profile.clone(),
            selection_locked: default.selection_locked,
            configuration_ref: default.configuration_ref.clone(),
        }),
    };
    sha256_hex(&tuple)
}

/// Prepared source approvals bind the normalized ticket and marker as well as
/// the legacy target fingerprint. This companion has no IPC caller in P03.
#[allow(dead_code)]
pub(crate) fn source_selection_fingerprint(
    target_fingerprint: &str,
    ticket: &crate::config::settings::SourceSelectionTicket,
    marker: Option<&str>,
) -> Result<String, String> {
    sha256_hex(&("source-selection-v1", target_fingerprint, ticket, marker))
}

/// Prepare a manual setter from a freshly authorized physical source. Publication
/// remains a separate guarded step so no source IO enters the config layer.
#[allow(dead_code)]
pub(crate) async fn prepare_source_profile_override(
    settings: &SettingsState,
    replica_path: &Path,
    profile: &str,
) -> Result<
    (
        crate::config::coding_agent_profiles::ReplicaSelectionExpectation,
        crate::config::coding_agent_profiles::PreparedSourceSelection,
    ),
    String,
> {
    use crate::config::coding_agent_profiles::{
        read_replica_selection_state, resolve_profile, PreparedSourceSelection,
        ProfileResolutionRequest, ReplicaSelectionPair,
    };
    use crate::config::settings::{agent_file_projection, configuration_identity, pair_identity};
    let profile = crate::config::settings::normalize_profile_letter(profile)
        .ok_or("Profile must be a single letter A through Z")?;
    let host = settings
        .current_snapshot()
        .await
        .map_err(|e| e.to_string())?;
    let validated = crate::config::coding_agent_profiles::validate_profile_selection_agent_path(
        &host,
        replica_path,
    )?;
    let expected = read_replica_selection_state(&validated.launch_path).expectation()?;
    let mut reference = expected
        .configuration_ref
        .clone()
        .ok_or("source reference required")?;
    let snapshot = read_agent_file_source_inner(settings, reference.source.clone()).await?;
    let hint = reference
        .legacy_entry_hint
        .as_deref()
        .ok_or("source hint required")?;
    let matches: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|row| row.entry_hint == hint)
        .collect();
    if matches.len() != 1
        || !matches[0].launchable
        || matches[0].base_hash.as_deref() != Some(reference.calculated_id.as_str())
    {
        return Err("stored source selection is unavailable or changed".into());
    }
    let scoped = agent_file_projection(&snapshot)?;
    let agents: Vec<_> = scoped
        .agents
        .iter()
        .filter(|agent| agent.id == hint)
        .collect();
    if agents.len() != 1
        || configuration_identity(&agents[0].command, &agents[0].envs) != reference.calculated_id
    {
        return Err("stored source identity is missing or ambiguous".into());
    }
    let resolution = resolve_profile(
        &scoped,
        ProfileResolutionRequest {
            coding_agent_id: hint,
            launch_path: None,
            agent_matrix_name: None,
            requested_profile: Some(&profile),
            requested_profile_authoritative: true,
        },
    );
    reference.profile_calculated_id = Some(pair_identity(
        &agents[0].command,
        &agents[0].envs,
        &resolution.cell,
    ));
    if reference.requested_profile.is_some() {
        reference.requested_profile = Some(profile.clone());
    }
    let prepared = PreparedSourceSelection {
        pair: ReplicaSelectionPair {
            coding_agent_id: hint.to_owned(),
            requested_profile: profile,
        },
        configuration_ref: reference,
        profile_override_source: Some("manual".into()),
    };
    Ok((expected, prepared))
}

pub(crate) struct AssignmentSnapshot {
    direct_fingerprint: String,
    pub(crate) decisions: Option<SelectionDecisions>,
    pub(crate) conflict_count: usize,
    pub(crate) protected_count: usize,
    pub(crate) invalid_count: usize,
    pub(crate) top_fingerprint: String,
    pub(crate) top_eligible_count: usize,
    pub(crate) top_live_session_count: usize,
    eligibility: EligibilitySets,
}

pub(crate) fn assignment_snapshot(
    enumeration: &ProfileTargetEnumeration,
    sessions: &[SessionInfo],
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    coding_agent_id: &str,
    requested_profile: &str,
    restart_sessions: bool,
) -> Result<AssignmentSnapshot, String> {
    let eligibility = eligibility_sets(enumeration);
    let protected_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Locked)
        .count();
    let invalid_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Invalid)
        .count();
    let bulk = *scope != ProfileAssignmentScope::Replica;
    let conflict_count = if bulk && mode == AssignmentMode::AssignAndLock {
        protected_count
    } else {
        0
    };
    let operation = match mode {
        AssignmentMode::Ordinary => "assign",
        AssignmentMode::AssignAndLock => "assignAndLock",
    };
    // A deliberate replica target counts whenever it is valid; a bulk ordinary
    // assignment is actionable only for unlocked replicas.
    let direct_eligible = if *scope == ProfileAssignmentScope::Replica {
        eligibility.valid_paths.clone()
    } else {
        eligibility.unlocked_paths.clone()
    };
    let direct_fingerprint = assignment_fingerprint(
        enumeration,
        sessions,
        scope,
        operation,
        coding_agent_id,
        requested_profile,
        restart_sessions,
        None,
        &direct_eligible,
    )?;
    let decisions = if conflict_count > 0 {
        let unlocked_only = SelectionDecisionProjection {
            fingerprint: assignment_fingerprint(
                enumeration,
                sessions,
                scope,
                operation,
                coding_agent_id,
                requested_profile,
                restart_sessions,
                Some(ConflictDecision::UnlockedOnly),
                &eligibility.unlocked_paths,
            )?,
            eligible_paths: eligibility.unlocked_paths.clone(),
            eligible_count: eligibility.unlocked_paths.len(),
            skipped_locked_count: protected_count,
            live_session_count: live_session_count_for(enumeration, &eligibility.unlocked_paths),
        };
        let force_reviewed = SelectionDecisionProjection {
            fingerprint: assignment_fingerprint(
                enumeration,
                sessions,
                scope,
                operation,
                coding_agent_id,
                requested_profile,
                restart_sessions,
                Some(ConflictDecision::ForceReviewed),
                &eligibility.valid_paths,
            )?,
            eligible_paths: eligibility.valid_paths.clone(),
            eligible_count: eligibility.valid_paths.len(),
            skipped_locked_count: 0,
            live_session_count: live_session_count_for(enumeration, &eligibility.valid_paths),
        };
        Some(SelectionDecisions {
            unlocked_only,
            force_reviewed,
        })
    } else {
        None
    };
    let (top_fingerprint, top_eligible_paths) = match &decisions {
        Some(decisions) => (
            decisions.unlocked_only.fingerprint.clone(),
            decisions.unlocked_only.eligible_paths.clone(),
        ),
        None => (direct_fingerprint.clone(), direct_eligible),
    };
    let top_live_session_count = live_session_count_for(enumeration, &top_eligible_paths);
    Ok(AssignmentSnapshot {
        direct_fingerprint,
        decisions,
        conflict_count,
        protected_count,
        invalid_count,
        top_fingerprint,
        top_eligible_count: top_eligible_paths.len(),
        top_live_session_count,
        eligibility,
    })
}

pub(crate) fn resolve_assignment_conflict_decision(
    snapshot: &AssignmentSnapshot,
    requested: Option<ConflictDecision>,
) -> Result<Option<ConflictDecision>, String> {
    if snapshot.conflict_count == 0 {
        if requested.is_some() {
            return Err(
                "conflictDecision is not required because no protected replica is in scope"
                    .to_string(),
            );
        }
        return Ok(None);
    }
    Ok(Some(requested.ok_or_else(|| {
        "conflictDecisionRequired: protected replicas are in scope; choose unlockedOnly or \
            forceReviewed"
            .to_string()
    })?))
}

pub(crate) fn assignment_expected_fingerprint(
    snapshot: &AssignmentSnapshot,
    decision: Option<ConflictDecision>,
) -> Result<String, String> {
    match decision {
        None => Ok(snapshot.direct_fingerprint.clone()),
        Some(ConflictDecision::UnlockedOnly) => snapshot
            .decisions
            .as_ref()
            .map(|decisions| decisions.unlocked_only.fingerprint.clone())
            .ok_or_else(|| "selection decision projection is missing".to_string()),
        Some(ConflictDecision::ForceReviewed) => snapshot
            .decisions
            .as_ref()
            .map(|decisions| decisions.force_reviewed.fingerprint.clone())
            .ok_or_else(|| "selection decision projection is missing".to_string()),
    }
}

pub(crate) fn assignment_write_paths(
    snapshot: &AssignmentSnapshot,
    scope: &ProfileAssignmentScope,
    decision: Option<ConflictDecision>,
) -> Vec<String> {
    if *scope == ProfileAssignmentScope::Replica
        || decision == Some(ConflictDecision::ForceReviewed)
    {
        snapshot.eligibility.valid_paths.clone()
    } else {
        snapshot.eligibility.unlocked_paths.clone()
    }
}

pub(crate) fn assignment_write_intent(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    decision: Option<ConflictDecision>,
) -> crate::config::coding_agent_profiles::SelectionWriteIntent {
    use crate::config::coding_agent_profiles::SelectionWriteIntent;
    match (scope, mode) {
        (ProfileAssignmentScope::Replica, AssignmentMode::Ordinary) => {
            SelectionWriteIntent::Individual
        }
        (ProfileAssignmentScope::Replica, AssignmentMode::AssignAndLock) => {
            SelectionWriteIntent::IndividualAssignLock
        }
        (_, AssignmentMode::Ordinary) => SelectionWriteIntent::BulkOrdinary,
        (_, AssignmentMode::AssignAndLock) => match decision {
            Some(ConflictDecision::ForceReviewed) => SelectionWriteIntent::BulkForceReviewed,
            _ => SelectionWriteIntent::BulkAssignLockUnlockedOnly,
        },
    }
}

/// #1941 - classify a guarded-write error into the wire error vocabulary while
/// preserving the original message and paths.
pub(crate) fn selection_error_code(message: &str) -> &'static str {
    if message.starts_with("stale replica selection") {
        "stalePreview"
    } else if message.starts_with("invalid replica selection") {
        "invalidSelectionState"
    } else if message.contains("configLockTimeout") {
        "configLockTimeout"
    } else if message.contains("'tooling' must be a JSON object in the tracked config file") {
        // #2816 (C3) - the pair's cleanup rejects a malformed TRACKED `tooling`
        // before the closure runs, so the pre-C3 `invalid replica selection ...`
        // text no longer reaches here. Same classification, new spelling. The
        // state-side text is deliberately NOT matched: see consequence 4.
        "invalidSelectionState"
    } else {
        "configWriteFailed"
    }
}

/// #1941 - count the protected replicas left after an operation. `None` means
/// the total cannot be established (incomplete membership or an invalid state),
/// never a false zero.
pub(crate) fn refresh_locked_count(enumeration: &ProfileTargetEnumeration) -> Option<usize> {
    if !enumeration.counts_complete {
        return None;
    }
    let mut count = 0;
    for candidate in &enumeration.candidates {
        match crate::config::coding_agent_profiles::read_replica_selection_state(Path::new(
            &candidate.target.replica_path,
        )) {
            crate::config::coding_agent_profiles::ReplicaSelectionState::Unlocked { .. } => {}
            crate::config::coding_agent_profiles::ReplicaSelectionState::Locked { .. } => {
                count += 1
            }
            crate::config::coding_agent_profiles::ReplicaSelectionState::Invalid { .. } => {
                return None
            }
        }
    }
    Some(count)
}
