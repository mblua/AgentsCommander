use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;

use super::wire::{
    ApplySelectionLockRemovalRequest, ApplySelectionLockRemovalResult,
    PreviewSelectionLockRemovalRequest, PreviewSelectionLockRemovalResult, ProfileAssignmentError,
    SelectionState,
};

use super::target::enumerate_profile_assignment_targets;

use super::fingerprint::{refresh_locked_count, removal_fingerprint, selection_error_code};

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
