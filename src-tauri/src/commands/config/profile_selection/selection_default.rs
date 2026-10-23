use std::path::Path;
use tauri::{AppHandle, Emitter, State};

use crate::config::settings::SettingsState;

use super::wire::{
    GetReplicaSelectionDefaultRequest, ReplicaSelectionDefaultResult, SelectionDefault,
    SetReplicaSelectionDefaultRequest,
};

use super::target::{
    normalize_profile_letter_for_assignment, validate_profile_assignment_request,
    validate_selection_anchor,
};

use super::fingerprint::selection_default_fingerprint;

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
