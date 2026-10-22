use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

use sha2::{Digest, Sha256};

use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;

use super::read_agent_file_source_inner;

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

#[tauri::command]
pub async fn preview_coding_agent_profile_selection(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    request: PreviewCodingAgentProfileSelectionRequest,
) -> Result<PreviewCodingAgentProfileSelectionResult, String> {
    // #1941 - a preview takes the shared operation turn for a consistent
    // operation boundary; its owned task survives a dropped or timed-out caller
    // (the transport discards the late response) without cancelling the read.
    let session_mgr = Arc::clone(session_mgr.inner());
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_read_timed(
        "preview_coding_agent_profile_selection",
        move || async move {
            preview_coding_agent_profile_selection_inner(&session_mgr, &settings, request).await
        },
    )
    .await
}

/// Enumerate the replicas a broad-scope profile assignment would touch and
/// return the fingerprints the frontend echoes back on apply. Shared by the
/// desktop command and the web dispatcher; the caller owns the #1940 turn and
/// this function only reads settings, sessions and strict replica state.
pub(crate) async fn preview_coding_agent_profile_selection_inner(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
    request: PreviewCodingAgentProfileSelectionRequest,
) -> Result<PreviewCodingAgentProfileSelectionResult, String> {
    let settings_snapshot = settings.read().await.clone();
    validate_profile_assignment_request(
        &settings_snapshot,
        &request.coding_agent_id,
        &request.profile,
    )?;
    let normalized_profile = normalize_profile_letter_for_assignment(&request.profile)?;
    let sessions = { session_mgr.read().await.list_sessions().await };
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        &sessions,
    )?;
    let snapshot = assignment_snapshot(
        &enumeration,
        &sessions,
        &request.scope,
        request.assignment_mode,
        &request.coding_agent_id,
        &normalized_profile,
        request.restart_sessions,
    )?;
    Ok(PreviewCodingAgentProfileSelectionResult {
        scope: request.scope.clone(),
        target_count: snapshot.top_eligible_count,
        live_session_count: snapshot.top_live_session_count,
        target_fingerprint: snapshot.top_fingerprint,
        requires_explicit_confirmation: profile_assignment_requires_explicit_confirmation(
            &request.scope,
            request.assignment_mode,
        ),
        targets: enumeration.targets(),
        warnings: enumeration.warnings,
        counts_complete: enumeration.counts_complete,
        candidate_count: enumeration.candidates.len(),
        protected_count: snapshot.protected_count,
        invalid_count: snapshot.invalid_count,
        conflict_count: snapshot.conflict_count,
        decisions: snapshot.decisions,
    })
}

#[tauri::command]
pub async fn apply_coding_agent_profile_selection(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    settings: State<'_, SettingsState>,
    request: ApplyCodingAgentProfileSelectionRequest,
) -> Result<ApplyCodingAgentProfileSelectionResult, String> {
    // #1940 - the entire apply (enumeration, per-replica publication, every
    // restart submission, its settlement and classification) is one owned
    // selection operation. Dropping this caller detaches the owned task instead
    // of releasing the turn while an accepted restart still settles.
    let app_handle = app.clone();
    let session_mgr = Arc::clone(session_mgr.inner());
    let pty_mgr = Arc::clone(pty_mgr.inner());
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || {
        let app = app_handle.clone();
        async move {
            let (result, payload) = apply_coding_agent_profile_selection_inner(
                &app,
                &session_mgr,
                &pty_mgr,
                &settings,
                request,
            )
            .await?;
            let _ = app.emit("coding_agent_profile_selection_updated", payload);
            Ok(result)
        }
    })
    .await
}

/// Apply a coding-agent profile assignment across the enumerated replicas
/// (optionally restarting live sessions) and return both the result and the
/// `coding_agent_profile_selection_updated` event payload. The caller emits the
/// payload (desktop) or broadcasts it to web clients. Shared by the desktop
/// command and the web dispatcher.
pub(crate) async fn apply_coding_agent_profile_selection_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<std::sync::Mutex<PtyManager>>,
    settings: &SettingsState,
    request: ApplyCodingAgentProfileSelectionRequest,
) -> Result<(ApplyCodingAgentProfileSelectionResult, serde_json::Value), String> {
    // #1940 - the caller owns the selection operation turn; this inner function
    // neither reacquires it nor spawns another turn, and it does not release it
    // before the last restart reply, classification and publication.
    //
    // #1941 - a conflict decision is only meaningful for a bulk assign-and-lock;
    // every other scope/mode rejects it before any enumeration or write.
    validate_assignment_decision_scope(
        &request.scope,
        request.assignment_mode,
        request.conflict_decision,
    )?;
    let settings_snapshot = settings.read().await.clone();
    validate_profile_assignment_request(
        &settings_snapshot,
        &request.coding_agent_id,
        &request.profile,
    )?;
    let normalized_profile = normalize_profile_letter_for_assignment(&request.profile)?;
    let sessions = { session_mgr.read().await.list_sessions().await };
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        &sessions,
    )?;
    // #1941 - re-enumerate the complete candidate set under the owned turn and
    // build fresh fingerprints from the strict per-candidate state; the client's
    // approval is compared against THIS snapshot before any write.
    let snapshot = assignment_snapshot(
        &enumeration,
        &sessions,
        &request.scope,
        request.assignment_mode,
        &request.coding_agent_id,
        &normalized_profile,
        request.restart_sessions,
    )?;
    let resolved_decision =
        resolve_assignment_conflict_decision(&snapshot, request.conflict_decision)?;
    let target_fingerprint = assignment_expected_fingerprint(&snapshot, resolved_decision)?;
    validate_profile_assignment_confirmation(
        &request.scope,
        request.assignment_mode,
        request.confirmed_target_fingerprint.as_deref(),
        &target_fingerprint,
    )?;

    let write_eligible_paths = assignment_write_paths(&snapshot, &request.scope, resolved_decision);
    if request.restart_sessions {
        let restart_targets: Vec<ProfileAssignmentTarget> = enumeration
            .candidates
            .iter()
            .filter(|candidate| {
                write_eligible_paths.contains(&canonical_compare_key(Path::new(
                    &candidate.target.replica_path,
                )))
            })
            .map(|candidate| candidate.target.clone())
            .collect();
        prevalidate_profile_assignment_restarts(
            &settings_snapshot,
            &request.coding_agent_id,
            &normalized_profile,
            &restart_targets,
        )?;
    }

    let intent =
        assignment_write_intent(&request.scope, request.assignment_mode, resolved_decision);
    let mut updated_replica_paths = Vec::new();
    let mut newly_protected_paths = Vec::new();
    let mut skipped_locked_paths = Vec::new();
    let mut invalid_paths = Vec::new();
    let mut write_succeeded_keys = BTreeSet::new();
    let mut errors = Vec::new();
    let mut warnings = enumeration.warnings.clone();
    let mut force_applied = false;
    for candidate in &enumeration.candidates {
        let target = &candidate.target;
        let key = canonical_compare_key(Path::new(&target.replica_path));
        if target.selection_state == SelectionState::Invalid {
            // #1941 - an invalid protection state is an explicit error with no
            // write and no restart; it is never read as an unlocked replica.
            invalid_paths.push(target.replica_path.clone());
            errors.push(ProfileAssignmentError {
                code: "invalidSelectionState".to_string(),
                message: target.selection_error.clone().unwrap_or_else(|| {
                    format!(
                        "Replica '{}' has an invalid selection state",
                        target.replica_path
                    )
                }),
                session_ids: target.live_session_ids.clone(),
                replica_paths: vec![target.replica_path.clone()],
            });
            continue;
        }
        if !write_eligible_paths.contains(&key) {
            if target.selection_state == SelectionState::Locked {
                // #1939/#1941 - ordinary and unlocked-only policies never force
                // a valid lock: skip it (no write, no restart) and report it.
                skipped_locked_paths.push(target.replica_path.clone());
                warnings.push(format!("Skipping locked replica '{}'", target.replica_path));
            }
            continue;
        }
        let expected = candidate
            .expected
            .clone()
            .ok_or("invalid selection expectation")?;
        let pair = crate::config::coding_agent_profiles::ReplicaSelectionPair {
            coding_agent_id: request.coding_agent_id.clone(),
            requested_profile: normalized_profile.clone(),
        };
        // #1940/#1939 - the guarded callback re-reads the strict state and
        // compares this expectation inside the OS write lock, so a target that
        // changed after enumeration fails its own CAS instead of being
        // overwritten; earlier writes stay committed and accurately reported.
        match crate::config::coding_agent_profiles::write_replica_selection(
            &settings_snapshot,
            Path::new(&target.replica_path),
            &pair,
            intent,
            &expected,
        ) {
            Ok(outcome) if outcome.changed => {
                use crate::config::coding_agent_profiles::SelectionLockTransition;
                if outcome.lock_transition == Some(SelectionLockTransition::UnlockedToLocked) {
                    newly_protected_paths.push(target.replica_path.clone());
                }
                if intent
                    == crate::config::coding_agent_profiles::SelectionWriteIntent::BulkForceReviewed
                    && expected.locked
                {
                    // #1941 - forceApplied means a protected target was actually
                    // overwritten, never merely that a decision was selected.
                    force_applied = true;
                }
                updated_replica_paths.push(target.replica_path.clone());
                write_succeeded_keys.insert(key);
            }
            Ok(_unchanged) => warnings.push(format!(
                "Replica '{}' was already in the requested state",
                target.replica_path
            )),
            Err(e) => errors.push(ProfileAssignmentError {
                code: selection_error_code(&e).to_string(),
                message: e,
                session_ids: target.live_session_ids.clone(),
                replica_paths: vec![target.replica_path.clone()],
            }),
        }
    }

    let mut restarted_session_ids = Vec::new();
    let mut destroyed_but_not_recreated_session_ids = Vec::new();
    if request.restart_sessions && !write_succeeded_keys.is_empty() {
        for target in &enumeration.targets() {
            if !write_succeeded_keys
                .contains(&canonical_compare_key(Path::new(&target.replica_path)))
            {
                continue;
            }
            for session_id in &target.live_session_ids {
                let Ok(uuid) = uuid::Uuid::parse_str(session_id) else {
                    continue;
                };
                match crate::commands::session::restart_session_inner_with_intent(
                    app,
                    session_mgr,
                    pty_mgr,
                    settings,
                    uuid,
                    Some(request.coding_agent_id.clone()),
                    Some(normalized_profile.clone()),
                    Some(true),
                    false,
                    crate::session::selection::TrustedRestartIntent::Background,
                    None,
                    crate::config::sessions_persistence::default_creation_gate_enforcement(),
                )
                .await
                {
                    Ok(_) => restarted_session_ids.push(session_id.clone()),
                    Err(e) => {
                        let (code, destroyed_but_not_recreated) =
                            classify_restart_failure(session_mgr, uuid).await;
                        errors.push(ProfileAssignmentError {
                            code: code.to_string(),
                            message: e,
                            session_ids: vec![session_id.clone()],
                            replica_paths: vec![target.replica_path.clone()],
                        });
                        if destroyed_but_not_recreated {
                            destroyed_but_not_recreated_session_ids.push(session_id.clone());
                        }
                    }
                }
            }
        }
    }

    // #1941 - refresh under the same turn so the reported remaining protection
    // never comes from the pre-write snapshot; an unknowable total is `null`.
    let locked_after_apply_count = refresh_locked_count(&enumeration);
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
    Ok((result, payload))
}

/// #1941 - one identified candidate plus its canonical Matrix dir, kept so a
/// fingerprint never has to re-resolve identity from a mutable path.
pub(crate) struct ProfileCandidate {
    pub(crate) target: ProfileAssignmentTarget,
    pub(crate) matrix_dir: PathBuf,
    pub(crate) expected: Option<crate::config::coding_agent_profiles::ReplicaSelectionExpectation>,
}

/// #2557 - one replica or folder the scope walk could not count, and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeFault {
    pub code: ScopeFaultCode,
    pub replica_name: String,
    pub replica_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScopeFaultCode {
    ConfigUnreadable,
    ConfigNotJson,
    ConfigNotObject,
    IdentityMissing,
    IdentityMismatch,
    LocationInvalid,
    PathUnreadable,
    FolderUnreadable,
}

impl ScopeFault {
    /// Unlike `build_profile_assignment_target`, a missing `__agent_` prefix
    /// keeps the whole folder name: a fault is never nameless.
    pub(crate) fn new(code: ScopeFaultCode, path: &Path) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            code,
            replica_name: name
                .strip_prefix("__agent_")
                .map(str::to_string)
                .unwrap_or(name),
            replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(path),
        }
    }
}

// Exhaustive on purpose: a new replica_identity fault must break the build here.
pub(crate) fn scope_fault_code(
    fault: crate::config::replica_identity::ReplicaConfigFault,
) -> ScopeFaultCode {
    use crate::config::replica_identity::ReplicaConfigFault;
    match fault {
        ReplicaConfigFault::ConfigUnreadable => ScopeFaultCode::ConfigUnreadable,
        ReplicaConfigFault::ConfigNotJson => ScopeFaultCode::ConfigNotJson,
        ReplicaConfigFault::ConfigNotObject => ScopeFaultCode::ConfigNotObject,
        ReplicaConfigFault::IdentityMissing => ScopeFaultCode::IdentityMissing,
        ReplicaConfigFault::IdentityMismatch => ScopeFaultCode::IdentityMismatch,
        ReplicaConfigFault::LocationInvalid => ScopeFaultCode::LocationInvalid,
    }
}

/// #1941 - the complete identified candidate set for one scope, together with
/// whether the directory walk could establish that set.
pub(crate) struct ProfileTargetEnumeration {
    pub(crate) anchor_dir: PathBuf,
    pub(crate) anchor_matrix_dir: PathBuf,
    pub(crate) candidates: Vec<ProfileCandidate>,
    pub(crate) warnings: Vec<String>,
    pub(crate) scope_faults: Vec<ScopeFault>,
    pub(crate) counts_complete: bool,
}

impl ProfileTargetEnumeration {
    fn targets(&self) -> Vec<ProfileAssignmentTarget> {
        self.candidates
            .iter()
            .map(|candidate| candidate.target.clone())
            .collect()
    }
}

/// #1941 - directory-walk accumulator. A read failure is a visible warning that
/// makes the scope counts incomplete, never a silent skip: an unreadable member
/// could be protected, so the scope can no longer certify a total.
#[derive(Default)]
pub(crate) struct CandidateDirs {
    pub(crate) dirs: Vec<PathBuf>,
    warnings: Vec<String>,
    pub(crate) faults: Vec<ScopeFault>,
    pub(crate) complete: bool,
}

impl CandidateDirs {
    pub(crate) fn new() -> Self {
        Self {
            dirs: Vec::new(),
            warnings: Vec::new(),
            faults: Vec::new(),
            complete: true,
        }
    }

    fn mark_incomplete(&mut self, warning: String, fault: ScopeFault) {
        self.warnings.push(warning);
        self.faults.push(fault);
        self.complete = false;
    }
}

pub(crate) fn validate_profile_assignment_request(
    settings: &AppSettings,
    coding_agent_id: &str,
    profile: &str,
) -> Result<(), String> {
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == coding_agent_id)
    {
        return Err(format!("Agent '{}' is not configured", coding_agent_id));
    }
    normalize_profile_letter_for_assignment(profile)?;
    Ok(())
}

pub(crate) fn normalize_profile_letter_for_assignment(profile: &str) -> Result<String, String> {
    crate::config::settings::normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())
}

pub(crate) fn profile_assignment_requires_explicit_confirmation(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
) -> bool {
    *scope != ProfileAssignmentScope::Replica || mode == AssignmentMode::AssignAndLock
}

/// #1941 - a conflict decision belongs only to a bulk assign-and-lock; anything
/// else is rejected before enumeration so no client boolean can widen force.
pub(crate) fn validate_assignment_decision_scope(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    decision: Option<ConflictDecision>,
) -> Result<(), String> {
    if decision.is_none() {
        return Ok(());
    }
    if *scope == ProfileAssignmentScope::Replica {
        return Err("conflictDecision is not valid for replica scope".to_string());
    }
    if mode == AssignmentMode::Ordinary {
        return Err("conflictDecision is only valid with assignmentMode assignAndLock".to_string());
    }
    Ok(())
}

pub(crate) fn validate_profile_assignment_confirmation(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    provided: Option<&str>,
    fingerprint: &str,
) -> Result<(), String> {
    match provided {
        Some(value) if value == fingerprint => Ok(()),
        // #1941 - the legacy deliberate replica assignment keeps working without
        // a fingerprint; every bulk, assignAndLock and removal path needs one.
        None if *scope == ProfileAssignmentScope::Replica && mode == AssignmentMode::Ordinary => {
            Ok(())
        }
        // #2051 - no fingerprint at all is a client contract failure, not a
        // target change: never report it as a stale preview.
        None => Err(
            "confirmationMissing: This operation requires the preview confirmation, and none was \
                sent. Nothing was written."
                .to_string(),
        ),
        Some(_) => Err(
            "stalePreview: Target selection changed. Rerun preview before applying profile \
                selection."
                .to_string(),
        ),
    }
}

pub(crate) fn prevalidate_profile_assignment_restarts(
    settings: &AppSettings,
    coding_agent_id: &str,
    profile: &str,
    targets: &[ProfileAssignmentTarget],
) -> Result<(), String> {
    for target in targets {
        if target.live_session_ids.is_empty() {
            continue;
        }
        crate::config::agent_command::build_agent_spawn_command(
            settings,
            coding_agent_id,
            Some(Path::new(&target.replica_path)),
            Some(profile),
        )?;
    }
    Ok(())
}

pub(crate) async fn classify_restart_failure(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    uuid: uuid::Uuid,
) -> (&'static str, bool) {
    let session_survived = {
        let mgr = session_mgr.read().await;
        mgr.get_session(uuid).await.is_some()
    };
    if session_survived {
        ("restartFailed", false)
    } else {
        ("destroyedButNotRecreated", true)
    }
}

/// #1941 - the scope anchor must be a strictly validated `__agent_*` replica
/// inside a `room-*`/`wg-*` directory whose canonical origin Matrix lives under
/// a configured project root. An origin Matrix or the Root Agent is never a
/// lockable replica target, and no client-supplied target list is accepted.
pub(crate) fn validate_selection_anchor(
    settings: &AppSettings,
    target_replica_path: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    let target_replica = canonical_real_dir(target_replica_path, "target replica")?;
    validate_wg_replica_path(&target_replica)?;
    let validated = crate::config::coding_agent_profiles::validate_profile_selection_agent_path(
        settings,
        &target_replica,
    )?;
    let is_replica = validated
        .launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("__agent_"));
    if !is_replica {
        return Err(format!(
            "Selection lock target '{}' is not a Room replica",
            validated.launch_path.display()
        ));
    }
    Ok((validated.launch_path, validated.origin_matrix_dir))
}

pub(crate) fn enumerate_profile_assignment_targets(
    settings: &AppSettings,
    target_replica_path: &Path,
    scope: &ProfileAssignmentScope,
    sessions: &[SessionInfo],
) -> Result<ProfileTargetEnumeration, String> {
    let (anchor_dir, anchor_matrix_dir) = validate_selection_anchor(settings, target_replica_path)?;
    let mut candidate_dirs = CandidateDirs::new();
    match scope {
        ProfileAssignmentScope::Replica => candidate_dirs.dirs.push(anchor_dir.clone()),
        ProfileAssignmentScope::Workgroup => {
            let room_dir = anchor_dir
                .parent()
                .ok_or_else(|| "Target replica has no room parent".to_string())?;
            collect_replica_dirs_in_workgroup(room_dir, &mut candidate_dirs);
        }
        ProfileAssignmentScope::Kind => {
            for ac_root in crate::config::coding_agent_profiles::configured_ac_roots(settings) {
                collect_kind_replica_dirs(&ac_root, &mut candidate_dirs);
            }
        }
    }
    let mut warnings = candidate_dirs.warnings;
    let mut scope_faults = candidate_dirs.faults;
    let mut counts_complete = candidate_dirs.complete;

    let anchor_matrix_key = canonical_compare_key(&anchor_matrix_dir);
    let live_by_cwd = live_sessions_by_cwd(sessions);
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();

    // The anchor always belongs to its own scope; the walk dedupes against it.
    let (anchor_config, anchor_identity) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&anchor_dir)
            .map_err(|e| format!("Target replica '{}': {}", anchor_dir.display(), e))?;
    let anchor_state = crate::config::coding_agent_profiles::selection_state_from_value(
        &anchor_config,
        &anchor_identity.identity,
    );
    seen.insert(canonical_compare_key(&anchor_dir));
    candidates.push(ProfileCandidate {
        expected: anchor_state.expectation().ok(),
        target: build_profile_assignment_target(
            &anchor_dir,
            &anchor_identity,
            &anchor_state,
            &live_by_cwd,
        ),
        matrix_dir: anchor_identity.matrix_dir,
    });

    for candidate in candidate_dirs.dirs {
        let Ok(replica_dir) = canonical_real_dir(&candidate, "candidate replica") else {
            warnings.push(format!(
                "Skipping unreadable replica '{}'",
                candidate.display()
            ));
            scope_faults.push(ScopeFault::new(ScopeFaultCode::PathUnreadable, &candidate));
            counts_complete = false;
            continue;
        };
        let key = canonical_compare_key(&replica_dir);
        if !seen.insert(key) {
            continue;
        }
        let (config, identity) =
            match crate::config::replica_identity::read_wg_replica_config_read_only_classified(
                &replica_dir,
            ) {
                Ok(read) => read,
                Err((fault, _message)) => {
                    // #1941 - an unprovable identity inside the scope means membership
                    // cannot be established: keep the diagnostic visible and refuse to
                    // certify a scope total for it.
                    warnings.push(format!(
                        "Skipping invalid replica '{}'",
                        replica_dir.display()
                    ));
                    scope_faults.push(ScopeFault::new(scope_fault_code(fault), &replica_dir));
                    counts_complete = false;
                    continue;
                }
            };
        if *scope == ProfileAssignmentScope::Kind
            && canonical_compare_key(&identity.matrix_dir) != anchor_matrix_key
        {
            // Homonymous matrices in other projects stay excluded: Kind means
            // the canonical origin Matrix, not the directory name.
            continue;
        }
        let state = crate::config::coding_agent_profiles::selection_state_from_value(
            &config,
            &identity.identity,
        );
        let target = build_profile_assignment_target(&replica_dir, &identity, &state, &live_by_cwd);
        candidates.push(ProfileCandidate {
            expected: state.expectation().ok(),
            target,
            matrix_dir: identity.matrix_dir,
        });
    }
    candidates.sort_by(|a, b| {
        a.target
            .workgroup_name
            .cmp(&b.target.workgroup_name)
            .then_with(|| a.target.replica_name.cmp(&b.target.replica_name))
            .then_with(|| a.target.replica_path.cmp(&b.target.replica_path))
    });
    // read_dir order is platform-defined; the payload order must not be.
    scope_faults.sort_by(|a, b| (&a.replica_path, a.code).cmp(&(&b.replica_path, b.code)));
    Ok(ProfileTargetEnumeration {
        anchor_dir,
        anchor_matrix_dir,
        candidates,
        warnings,
        scope_faults,
        counts_complete,
    })
}

pub(crate) fn canonical_compare_key(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&path).replace('\\', "/");
    if cfg!(windows) {
        text = text.to_ascii_lowercase();
    }
    text
}

pub(crate) fn canonical_real_dir(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("{} '{}' is not readable: {}", label, path.display(), e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} '{}' is not a real directory",
            label,
            path.display()
        ));
    }
    std::fs::canonicalize(path)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
        .map_err(|e| {
            format!(
                "Failed to canonicalize {} '{}': {}",
                label,
                path.display(),
                e
            )
        })
}

pub(crate) fn validate_wg_replica_path(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !name.starts_with("__agent_") {
        return Err(format!("Target '{}' is not a Room replica", path.display()));
    }
    let wg_name = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !crate::config::entity_prefix::has_entity_prefix(wg_name) {
        return Err(format!(
            "Target '{}' is not inside a `room-*` or legacy `wg-*` Room directory",
            path.display()
        ));
    }
    Ok(())
}

pub(crate) fn collect_replica_dirs_in_workgroup(wg_dir: &Path, out: &mut CandidateDirs) {
    let entries = match std::fs::read_dir(wg_dir) {
        Ok(entries) => entries,
        Err(e) => {
            out.mark_incomplete(
                format!("Failed to read room '{}': {}", wg_dir.display(), e),
                ScopeFault::new(ScopeFaultCode::FolderUnreadable, wg_dir),
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                out.mark_incomplete(
                    format!(
                        "Failed to read an entry in room '{}': {}",
                        wg_dir.display(),
                        e
                    ),
                    ScopeFault::new(ScopeFaultCode::FolderUnreadable, wg_dir),
                );
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                out.mark_incomplete(
                    format!(
                        "Failed to inspect an entry in room '{}': {}",
                        wg_dir.display(),
                        e
                    ),
                    ScopeFault::new(ScopeFaultCode::FolderUnreadable, wg_dir),
                );
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("__agent_") {
            out.dirs.push(entry.path());
        }
    }
}

pub(crate) fn collect_kind_replica_dirs(ac_root: &Path, out: &mut CandidateDirs) {
    let entries = match std::fs::read_dir(ac_root) {
        Ok(entries) => entries,
        Err(e) => {
            out.mark_incomplete(
                format!("Failed to read workspace '{}': {}", ac_root.display(), e),
                ScopeFault::new(ScopeFaultCode::FolderUnreadable, ac_root),
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                out.mark_incomplete(
                    format!(
                        "Failed to read an entry in workspace '{}': {}",
                        ac_root.display(),
                        e
                    ),
                    ScopeFault::new(ScopeFaultCode::FolderUnreadable, ac_root),
                );
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                out.mark_incomplete(
                    format!(
                        "Failed to inspect an entry in workspace '{}': {}",
                        ac_root.display(),
                        e
                    ),
                    ScopeFault::new(ScopeFaultCode::FolderUnreadable, ac_root),
                );
                continue;
            }
        };
        if !file_type.is_dir()
            || !crate::config::entity_prefix::has_entity_prefix(
                &entry.file_name().to_string_lossy(),
            )
        {
            continue;
        }
        collect_replica_dirs_in_workgroup(&entry.path(), out);
    }
}

pub(crate) fn live_sessions_by_cwd(sessions: &[SessionInfo]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for session in sessions {
        if matches!(
            session.status,
            crate::session::session::SessionStatus::Exited(_)
        ) {
            continue;
        }
        out.entry(canonical_compare_key(Path::new(&session.working_directory)))
            .or_default()
            .push(session.id.clone());
    }
    out
}

pub(crate) fn build_profile_assignment_target(
    replica_dir: &Path,
    identity: &crate::config::replica_identity::WgReplicaIdentity,
    state: &crate::config::coding_agent_profiles::ReplicaSelectionState,
    live_by_cwd: &BTreeMap<String, Vec<String>>,
) -> ProfileAssignmentTarget {
    let wg_dir = replica_dir.parent().unwrap_or_else(|| Path::new(""));
    let workgroup_name = wg_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let replica_name = replica_dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("__agent_"))
        .unwrap_or("")
        .to_string();
    let origin_project = identity
        .ac_root
        .parent()
        .and_then(|project| project.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_string);
    let (saved_pair, selection_state, selection_error) = target_selection_wire(state);
    ProfileAssignmentTarget {
        workgroup_name,
        workgroup_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(wg_dir),
        replica_name,
        replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            replica_dir,
        ),
        identity_path: identity.identity.clone(),
        origin_project,
        live_session_ids: live_by_cwd
            .get(&canonical_compare_key(replica_dir))
            .cloned()
            .unwrap_or_default(),
        saved_pair,
        selection_state,
        selection_error,
    }
}

/// #1941 - strict state to wire fields. An invalid state carries its diagnostic
/// and never a synthesized `selectionLocked: false`.
pub(crate) fn target_selection_wire(
    state: &crate::config::coding_agent_profiles::ReplicaSelectionState,
) -> (Option<SavedPair>, SelectionState, Option<String>) {
    use crate::config::coding_agent_profiles::ReplicaSelectionState;
    match state {
        ReplicaSelectionState::Unlocked { pair, .. } => (
            pair.as_ref().map(saved_pair_wire),
            SelectionState::Unlocked,
            None,
        ),
        ReplicaSelectionState::Locked { pair, .. } => {
            (Some(saved_pair_wire(pair)), SelectionState::Locked, None)
        }
        ReplicaSelectionState::Invalid { diagnostic } => {
            (None, SelectionState::Invalid, Some(diagnostic.clone()))
        }
    }
}

pub(crate) fn saved_pair_wire(
    pair: &crate::config::coding_agent_profiles::ReplicaSelectionPair,
) -> SavedPair {
    SavedPair {
        coding_agent_id: Some(pair.coding_agent_id.clone()),
        requested_profile: Some(pair.requested_profile.clone()),
    }
}

pub(crate) fn target_pair(
    target: &ProfileAssignmentTarget,
) -> Option<crate::config::coding_agent_profiles::ReplicaSelectionPair> {
    let pair = target.saved_pair.as_ref()?;
    match (&pair.coding_agent_id, &pair.requested_profile) {
        (Some(coding_agent_id), Some(requested_profile)) => {
            Some(crate::config::coding_agent_profiles::ReplicaSelectionPair {
                coding_agent_id: coding_agent_id.clone(),
                requested_profile: requested_profile.clone(),
            })
        }
        _ => None,
    }
}

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
    decisions: Option<SelectionDecisions>,
    conflict_count: usize,
    protected_count: usize,
    invalid_count: usize,
    top_fingerprint: String,
    top_eligible_count: usize,
    top_live_session_count: usize,
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

// ── #1941 selection-lock removal commands ────────────────────────────────

#[tauri::command]
pub async fn preview_selection_lock_removal(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    request: PreviewSelectionLockRemovalRequest,
) -> Result<PreviewSelectionLockRemovalResult, String> {
    let session_mgr = Arc::clone(session_mgr.inner());
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_read_timed(
        "preview_selection_lock_removal",
        move || async move {
            preview_selection_lock_removal_inner(&session_mgr, &settings, request).await
        },
    )
    .await
}

/// Enumerate the removal candidates and compute the fingerprint the client
/// echoes back on apply. Reads only; the caller owns the #1940 turn.
pub(crate) async fn preview_selection_lock_removal_inner(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
    request: PreviewSelectionLockRemovalRequest,
) -> Result<PreviewSelectionLockRemovalResult, String> {
    let settings_snapshot = settings.read().await.clone();
    let sessions = { session_mgr.read().await.list_sessions().await };
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        &sessions,
    )?;
    let target_fingerprint = removal_fingerprint(&enumeration, &request.scope)?;
    let protected_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Locked)
        .count();
    let already_unlocked_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Unlocked)
        .count();
    let invalid_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Invalid)
        .count();
    Ok(PreviewSelectionLockRemovalResult {
        scope: request.scope,
        target_fingerprint,
        candidate_count: enumeration.candidates.len(),
        counts_complete: enumeration.counts_complete,
        protected_count,
        already_unlocked_count,
        invalid_count,
        targets: enumeration.targets(),
        warnings: enumeration.warnings,
        scope_faults: enumeration.scope_faults,
    })
}

#[tauri::command]
pub async fn apply_selection_lock_removal<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
    request: ApplySelectionLockRemovalRequest,
) -> Result<ApplySelectionLockRemovalResult, String> {
    // #1940/#1941 - the whole removal (prevalidation re-read, every guarded
    // unlock and the final refresh) is one owned operation; the event is emitted
    // before the turn is released, so no later bulk can restart behind it.
    let app_handle = app.clone();
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || {
        let app = app_handle.clone();
        async move {
            let (result, payload) = apply_selection_lock_removal_inner(&settings, request).await?;
            let _ = app.emit("coding_agent_profile_selection_updated", payload);
            Ok(result)
        }
    })
    .await
}

/// Remove selection locks for the enumerated candidates. Already-unlocked
/// targets are classified from the validated read and never rewritten; only
/// protected candidates reach `clear_replica_selection_lock`, each behind its
/// own per-file CAS. Removal never assigns and never requests a restart.
pub(crate) async fn apply_selection_lock_removal_inner(
    settings: &SettingsState,
    request: ApplySelectionLockRemovalRequest,
) -> Result<(ApplySelectionLockRemovalResult, serde_json::Value), String> {
    let settings_snapshot = settings.read().await.clone();
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        // Removal has no session operation, so live-session identity is not part
        // of its fingerprint and no session state is read.
        &[],
    )?;
    let target_fingerprint = removal_fingerprint(&enumeration, &request.scope)?;
    if request.confirmed_target_fingerprint != target_fingerprint {
        return Err(
            "stalePreview: Target selection changed. Rerun preview before removing selection locks."
                .to_string(),
        );
    }
    let mut removed_replica_paths = Vec::new();
    let mut already_unlocked_paths = Vec::new();
    let mut failed_replica_paths = Vec::new();
    let mut errors = Vec::new();
    for candidate in &enumeration.candidates {
        let target = &candidate.target;
        match target.selection_state {
            SelectionState::Invalid => {
                errors.push(ProfileAssignmentError {
                    code: "invalidSelectionState".to_string(),
                    message: target.selection_error.clone().unwrap_or_else(|| {
                        format!(
                            "Replica '{}' has an invalid selection state",
                            target.replica_path
                        )
                    }),
                    session_ids: target.live_session_ids.clone(),
                    replica_paths: vec![target.replica_path.clone()],
                });
            }
            SelectionState::Unlocked => {
                // #1941 - classified from the validated read; the mutation
                // funnel is deliberately NOT called for this target, so no
                // config publish, sidecar or restart can happen for it.
                already_unlocked_paths.push(target.replica_path.clone());
            }
            SelectionState::Locked => {
                let expected = candidate
                    .expected
                    .clone()
                    .ok_or("invalid selection expectation")?;
                match crate::config::coding_agent_profiles::clear_replica_selection_lock(
                    &settings_snapshot,
                    Path::new(&target.replica_path),
                    &expected,
                ) {
                    Ok(outcome) if outcome.changed => {
                        removed_replica_paths.push(target.replica_path.clone())
                    }
                    Ok(_unchanged) => already_unlocked_paths.push(target.replica_path.clone()),
                    Err(e) => {
                        failed_replica_paths.push(target.replica_path.clone());
                        errors.push(ProfileAssignmentError {
                            code: selection_error_code(&e).to_string(),
                            message: e,
                            session_ids: target.live_session_ids.clone(),
                            replica_paths: vec![target.replica_path.clone()],
                        });
                    }
                }
            }
        }
    }
    let remaining_protected_count = refresh_locked_count(&enumeration);
    let invalid_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Invalid)
        .count();
    let removed_count = removed_replica_paths.len();
    let result = ApplySelectionLockRemovalResult {
        scope: request.scope.clone(),
        target_fingerprint,
        removed_count,
        removed_replica_paths,
        already_unlocked_paths,
        failed_replica_paths,
        remaining_protected_count,
        candidate_count: enumeration.candidates.len(),
        counts_complete: enumeration.counts_complete,
        invalid_count,
        errors,
        warnings: enumeration.warnings.clone(),
    };
    let payload = serde_json::json!({
        "scope": request.scope,
        "operation": "unlock",
        "affectedPaths": &result.removed_replica_paths,
        "removedCount": result.removed_count,
    });
    Ok((result, payload))
}

// ── #1941 Matrix selection default commands ──────────────────────────────

#[tauri::command]
pub async fn get_replica_selection_default(
    settings: State<'_, SettingsState>,
    request: GetReplicaSelectionDefaultRequest,
) -> Result<ReplicaSelectionDefaultResult, String> {
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_read_timed(
        "get_replica_selection_default",
        move || async move { get_replica_selection_default_inner(&settings, request).await },
    )
    .await
}

pub(crate) async fn get_replica_selection_default_inner(
    settings: &SettingsState,
    request: GetReplicaSelectionDefaultRequest,
) -> Result<ReplicaSelectionDefaultResult, String> {
    let settings_snapshot = settings.read().await.clone();
    let (replica_dir, matrix_dir) =
        validate_selection_anchor(&settings_snapshot, Path::new(&request.target_replica_path))?;
    let default =
        crate::config::coding_agent_profiles::read_replica_selection_default(&matrix_dir)?;
    let default_wire = default.as_ref().map(selection_default_to_wire);
    let default_fingerprint = selection_default_fingerprint(&matrix_dir, default.as_ref())?;
    Ok(ReplicaSelectionDefaultResult {
        target_replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            &replica_dir,
        ),
        matrix_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(&matrix_dir),
        default: default_wire,
        default_fingerprint,
        warnings: Vec::new(),
    })
}

#[tauri::command]
pub async fn set_replica_selection_default<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
    request: SetReplicaSelectionDefaultRequest,
) -> Result<ReplicaSelectionDefaultResult, String> {
    let app_handle = app.clone();
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || {
        let app = app_handle.clone();
        async move {
            let (result, payload) = set_replica_selection_default_inner(&settings, request).await?;
            let _ = app.emit("coding_agent_profile_selection_updated", payload);
            Ok(result)
        }
    })
    .await
}

pub(crate) async fn set_replica_selection_default_inner(
    settings: &SettingsState,
    request: SetReplicaSelectionDefaultRequest,
) -> Result<(ReplicaSelectionDefaultResult, serde_json::Value), String> {
    let settings_snapshot = settings.read().await.clone();
    validate_profile_assignment_request(
        &settings_snapshot,
        &request.coding_agent_id,
        &request.requested_profile,
    )?;
    let normalized_profile = normalize_profile_letter_for_assignment(&request.requested_profile)?;
    let (replica_dir, matrix_dir) =
        validate_selection_anchor(&settings_snapshot, Path::new(&request.target_replica_path))?;
    // #1941 - the fingerprint binds the canonical Matrix plus the PRIOR default
    // (including absence), so a stale window cannot silently overwrite a default
    // published by another window; the write repeats the CAS inside its lock.
    let prior = crate::config::coding_agent_profiles::read_replica_selection_default(&matrix_dir)?;
    let prior_fingerprint = selection_default_fingerprint(&matrix_dir, prior.as_ref())?;
    if request.confirmed_default_fingerprint != prior_fingerprint {
        return Err(
            "stalePreview: The Matrix selection default changed. Rerun the read before saving."
                .to_string(),
        );
    }
    let default = crate::config::coding_agent_profiles::ReplicaSelectionDefault {
        configuration_ref: None,
        coding_agent_id: request.coding_agent_id.clone(),
        requested_profile: normalized_profile,
        selection_locked: request.selection_locked,
    };
    crate::config::coding_agent_profiles::write_replica_selection_default(
        &matrix_dir,
        &default,
        prior.as_ref(),
    )?;
    let default_wire = selection_default_to_wire(&default);
    let default_fingerprint = selection_default_fingerprint(&matrix_dir, Some(&default))?;
    let result = ReplicaSelectionDefaultResult {
        target_replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            &replica_dir,
        ),
        matrix_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(&matrix_dir),
        default: Some(default_wire.clone()),
        default_fingerprint,
        warnings: Vec::new(),
    };
    let payload = serde_json::json!({
        "scope": "default",
        "operation": "default",
        "agentPath": &result.matrix_path,
        "affectedPaths": [],
        "default": default_wire,
    });
    Ok((result, payload))
}

pub(crate) fn selection_default_to_wire(
    default: &crate::config::coding_agent_profiles::ReplicaSelectionDefault,
) -> SelectionDefault {
    SelectionDefault {
        coding_agent_id: default.coding_agent_id.clone(),
        requested_profile: default.requested_profile.clone(),
        selection_locked: default.selection_locked,
    }
}
