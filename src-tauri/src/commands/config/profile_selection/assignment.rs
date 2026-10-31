mod publication;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

use super::wire::{
    ApplyCodingAgentProfileSelectionRequest, ApplyCodingAgentProfileSelectionResult,
    PreviewCodingAgentProfileSelectionRequest, PreviewCodingAgentProfileSelectionResult,
    ProfileAssignmentError, ProfileAssignmentTarget, SelectionState,
};
use super::{
    assignment_expected_fingerprint, assignment_snapshot, assignment_write_intent,
    assignment_write_paths, canonical_compare_key, classify_restart_failure,
    enumerate_profile_assignment_targets, normalize_profile_letter_for_assignment,
    prevalidate_profile_assignment_restarts, profile_assignment_requires_explicit_confirmation,
    refresh_locked_count, resolve_assignment_conflict_decision, selection_error_code,
    validate_assignment_decision_scope, validate_profile_assignment_confirmation,
    validate_profile_assignment_request,
};

pub(crate) struct ProfileAssignmentWrites {
    updated_replica_paths: Vec<String>,
    newly_protected_paths: Vec<String>,
    skipped_locked_paths: Vec<String>,
    invalid_paths: Vec<String>,
    write_succeeded_keys: BTreeSet<String>,
    errors: Vec<ProfileAssignmentError>,
    warnings: Vec<String>,
    force_applied: bool,
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
    let (target_fingerprint, mut writes) = prepare_profile_assignment_writes(
        &settings_snapshot,
        &enumeration,
        &snapshot,
        &request,
        &normalized_profile,
    )?;

    let mut restarted_session_ids = Vec::new();
    let mut destroyed_but_not_recreated_session_ids = Vec::new();
    if request.restart_sessions && !writes.write_succeeded_keys.is_empty() {
        for target in &enumeration.targets() {
            if !writes
                .write_succeeded_keys
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
                        writes.errors.push(ProfileAssignmentError {
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
    Ok(publication::assignment_publication(
        request,
        normalized_profile,
        target_fingerprint,
        writes,
        restarted_session_ids,
        destroyed_but_not_recreated_session_ids,
        locked_after_apply_count,
    ))
}

fn prepare_profile_assignment_writes(
    settings_snapshot: &crate::config::settings::AppSettings,
    enumeration: &super::target::ProfileTargetEnumeration,
    snapshot: &super::fingerprint::AssignmentSnapshot,
    request: &ApplyCodingAgentProfileSelectionRequest,
    normalized_profile: &str,
) -> Result<(String, ProfileAssignmentWrites), String> {
    let resolved_decision =
        resolve_assignment_conflict_decision(snapshot, request.conflict_decision)?;
    let target_fingerprint = assignment_expected_fingerprint(snapshot, resolved_decision)?;
    validate_profile_assignment_confirmation(
        &request.scope,
        request.assignment_mode,
        request.confirmed_target_fingerprint.as_deref(),
        &target_fingerprint,
    )?;

    let write_eligible_paths = assignment_write_paths(snapshot, &request.scope, resolved_decision);
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
            settings_snapshot,
            &request.coding_agent_id,
            normalized_profile,
            &restart_targets,
        )?;
    }

    let intent =
        assignment_write_intent(&request.scope, request.assignment_mode, resolved_decision);
    let writes = write_profile_assignment(
        settings_snapshot,
        enumeration,
        &write_eligible_paths,
        request,
        normalized_profile,
        intent,
    )?;

    Ok((target_fingerprint, writes))
}

fn write_profile_assignment(
    settings_snapshot: &crate::config::settings::AppSettings,
    enumeration: &super::target::ProfileTargetEnumeration,
    write_eligible_paths: &[String],
    request: &ApplyCodingAgentProfileSelectionRequest,
    normalized_profile: &str,
    intent: crate::config::coding_agent_profiles::SelectionWriteIntent,
) -> Result<ProfileAssignmentWrites, String> {
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
            requested_profile: normalized_profile.to_string(),
        };
        // #1940/#1939 - the guarded callback re-reads the strict state and
        // compares this expectation inside the OS write lock, so a target that
        // changed after enumeration fails its own CAS instead of being
        // overwritten; earlier writes stay committed and accurately reported.
        match crate::config::coding_agent_profiles::write_replica_selection(
            settings_snapshot,
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

    Ok(ProfileAssignmentWrites {
        updated_replica_paths,
        newly_protected_paths,
        skipped_locked_paths,
        invalid_paths,
        write_succeeded_keys,
        errors,
        warnings,
        force_applied,
    })
}
