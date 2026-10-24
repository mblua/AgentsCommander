use super::super::{
    agents_instance_groups, agents_instance_path, apply_project_decode_to_value,
    apply_selected_project_state, coding_agent_identity_with_physical_overlay,
    empty_project_transaction, ensure_project_paths_migrated_at_locked_path, finalize_agent_order,
    merge_agents_instance_file, normalize_agent_order, production_instance_base,
    read_agents_instance_file, refresh_agent_id_hashes, refresh_pair_id_hashes,
    registered_profiles_for_read, repair_loaded_response_close_settings,
    repair_loaded_room_idle_confirmation_settings, replace_project_projection,
    restore_codex_home_carrier, select_project_source_at_locked_path,
    select_project_source_at_locked_path_with_limit, select_project_source_locked,
    selected_registered_profile_owner, set_agents_layer_state, settings_path, AppSettings,
    MainWindowDisplayState, MainWindowPlacementWriteMode, ProjectWriteMode,
    TerminalSnapshotGateWriteMode, WindowGeometry, FIELD_PROJECT_PATH, FIELD_PROJECT_PATHS,
    FIELD_PROJECT_PATHS_REL, FIELD_PROJECT_PATH_REL, FIELD_TERMINAL_SNAPSHOTS_ENABLED,
    OVERLAY_KEY_MAIN_DISPLAY_STATE, OVERLAY_KEY_MAIN_GEOMETRY,
};
use super::{
    read_disk_object_for_write_typed, report_settings_save_error, save_settings_value_locked,
    SettingsFileLock, SettingsSaveLegacyOutward, SettingsSaveReportSurface, SettingsSaveStage,
};
use crate::config::local_overlay::LocalSettingsOverlay;
use crate::config::settings::project_path as projects;
use serde_json::{Map, Value};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskProjectLists {
    pub project_paths: Vec<String>,
    pub project_path: Option<String>,
    pub archived_project_paths: Option<Vec<String>>,
}

/// Read-only privileged source capture. The shared settings lock covers both
/// objects and their conflict evaluation; this reader never migrates or repairs.
pub(crate) fn read_pty_input_project_paths_strict() -> Result<Option<Vec<String>>, String> {
    let Some(path) = settings_path() else {
        return Err("settings_unavailable".to_string());
    };
    read_pty_input_project_paths_strict_from_path(&path)
}

pub(crate) fn read_pty_input_project_paths_strict_from_path(
    path: &Path,
) -> Result<Option<Vec<String>>, String> {
    let _lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "read_pty_input_project_paths_strict_from_path",
    )
    .map_err(|_| "unsafe_path".to_string())?;
    let selected =
        select_project_source_at_locked_path_with_limit(path, 1024 * 1024).map_err(|error| {
            match &error.legacy_outward {
                SettingsSaveLegacyOutward::DiskRead(reason) if reason.ends_with("unsafe_path") => {
                    "unsafe_path".to_string()
                }
                _ => "settings_invalid".to_string(),
            }
        })?;
    if selected.legacy.is_none() && selected.destination.is_none() {
        return Ok(None);
    }
    let object = selected.projection;
    let Some(project_paths) = object.get("projectPaths") else {
        return Ok(Some(Vec::new()));
    };
    if project_paths.is_null() {
        return Ok(Some(Vec::new()));
    }
    let project_paths = project_paths
        .as_array()
        .filter(|paths| paths.len() <= 1_024)
        .ok_or_else(|| "settings_invalid".to_string())?;
    let mut strict = Vec::with_capacity(project_paths.len());
    for path in project_paths {
        let path = path
            .as_str()
            .filter(|path| !path.is_empty() && path.len() <= 32 * 1024 && !path.contains('\0'))
            .ok_or_else(|| "settings_invalid".to_string())?;
        strict.push(path.to_string());
    }
    Ok(Some(strict))
}

pub(crate) async fn read_pty_input_project_paths_strict_offloaded(
) -> Result<Option<Vec<String>>, String> {
    tokio::task::spawn_blocking(read_pty_input_project_paths_strict)
        .await
        .map_err(|_| "settings_unavailable".to_string())?
}

pub(crate) fn read_terminal_snapshot_project_paths_strict() -> Result<Vec<String>, String> {
    let path = settings_path().ok_or_else(|| "snapshot_settings_invalid".to_string())?;
    let _lock = SettingsFileLock::acquire_contextual(
        &path,
        std::time::Duration::from_secs(2),
        "read_terminal_snapshot_project_paths_strict",
    )
    .map_err(|_| "snapshot_settings_invalid".to_string())?;
    let selected = select_project_source_at_locked_path_with_limit(&path, 1024 * 1024)
        .map_err(|_| "snapshot_settings_invalid".to_string())?;
    if selected.legacy.is_none() && selected.destination.is_none() {
        return Err("snapshot_settings_invalid".to_string());
    }
    let object = selected.projection;
    let Some(paths) = object.get(FIELD_PROJECT_PATHS) else {
        return Ok(Vec::new());
    };
    let paths = paths
        .as_array()
        .filter(|paths| paths.len() <= 4_096)
        .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
    let mut project_paths = Vec::with_capacity(paths.len());
    let mut aggregate_bytes = 0usize;
    for value in paths {
        let path = value
            .as_str()
            .filter(|path| !path.is_empty() && path.len() <= 32 * 1024 && !path.contains('\0'))
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
        aggregate_bytes = aggregate_bytes
            .checked_add(path.len())
            .filter(|bytes| *bytes <= 4 * 1024 * 1024)
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
        project_paths.push(path.to_string());
    }
    Ok(project_paths)
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TerminalSnapshotSecuritySettings {
    pub terminal_snapshots_enabled: bool,
    pub project_paths: Vec<String>,
}

impl std::fmt::Debug for TerminalSnapshotSecuritySettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let project_path_bytes = self
            .project_paths
            .iter()
            .fold(0usize, |total, path| total.saturating_add(path.len()));
        formatter
            .debug_struct("TerminalSnapshotSecuritySettings")
            .field(
                "terminal_snapshots_enabled",
                &self.terminal_snapshots_enabled,
            )
            .field("project_paths", &self.project_paths.len())
            .field("project_path_bytes", &project_path_bytes)
            .finish()
    }
}

/// Read only the security-bearing snapshot gate and active project list. This
/// path never repairs, migrates, or writes settings. Every ambiguity fails
/// closed and callers map it to the fixed disabled response.
pub(crate) fn read_terminal_snapshot_security_settings_strict(
) -> Result<TerminalSnapshotSecuritySettings, String> {
    let path = settings_path().ok_or_else(|| "snapshot_settings_invalid".to_string())?;
    read_terminal_snapshot_security_settings_strict_from_path(&path)
}

pub(crate) fn read_terminal_snapshot_security_settings_strict_from_path(
    path: &Path,
) -> Result<TerminalSnapshotSecuritySettings, String> {
    let _lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "read_terminal_snapshot_security_settings_strict_from_path",
    )
    .map_err(|_| "snapshot_settings_invalid".to_string())?;
    let selected = select_project_source_at_locked_path_with_limit(path, 1024 * 1024)
        .map_err(|_| "snapshot_settings_invalid".to_string())?;
    let object = selected
        .legacy
        .as_ref()
        .map(|source| &source.object)
        .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
    let enabled = match object.get(FIELD_TERMINAL_SNAPSHOTS_ENABLED) {
        Some(Value::Bool(enabled)) => *enabled,
        Some(_) => return Err("snapshot_settings_invalid".to_string()),
        None => true,
    };
    let paths = match selected.projection.get(FIELD_PROJECT_PATHS) {
        Some(value) => value
            .as_array()
            .filter(|paths| paths.len() <= 4_096)
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?,
        None if [
            FIELD_PROJECT_PATH,
            FIELD_PROJECT_PATH_REL,
            FIELD_PROJECT_PATHS_REL,
        ]
        .iter()
        .all(|key| !selected.projection.contains_key(*key)) =>
        {
            return Ok(TerminalSnapshotSecuritySettings {
                terminal_snapshots_enabled: enabled,
                project_paths: Vec::new(),
            });
        }
        None => return Err("snapshot_settings_invalid".to_string()),
    };
    let mut project_paths = Vec::with_capacity(paths.len());
    let mut aggregate_bytes = 0usize;
    for value in paths {
        let path = value
            .as_str()
            .filter(|path| !path.is_empty() && path.len() <= 32 * 1024 && !path.contains('\0'))
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
        aggregate_bytes = aggregate_bytes
            .checked_add(path.len())
            .filter(|bytes| *bytes <= 4 * 1024 * 1024)
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
        project_paths.push(path.to_string());
    }
    Ok(TerminalSnapshotSecuritySettings {
        terminal_snapshots_enabled: enabled,
        project_paths,
    })
}

pub(crate) async fn read_terminal_snapshot_security_settings_strict_offloaded(
) -> Result<TerminalSnapshotSecuritySettings, String> {
    let joined = tokio::task::spawn_blocking(|| {
        crate::logging::catch_payload_unwind(read_terminal_snapshot_security_settings_strict)
    })
    .await;
    match crate::logging::collapse_payload_task(joined) {
        Ok(result) => result,
        Err(_) => {
            log::error!("[terminal-snapshot] stage=gate_read_task code=internal");
            Err("snapshot_settings_invalid".to_string())
        }
    }
}

/// The sole persistence owner for the #1173 disclosure gate. The caller holds
/// the managed settings write guard while this function serializes every AC
/// process with the settings file lock and compares against current disk truth.
pub(crate) fn compare_and_set_terminal_snapshots_enabled(
    current: &AppSettings,
    expected: bool,
    enabled: bool,
) -> Result<AppSettings, String> {
    let path =
        settings_path().ok_or_else(|| "terminal_snapshot_setting_save_failed".to_string())?;
    compare_and_set_terminal_snapshots_enabled_at_path(current, &path, expected, enabled)
}

pub(crate) fn compare_and_set_terminal_snapshots_enabled_at_path(
    current: &AppSettings,
    path: &Path,
    expected: bool,
    enabled: bool,
) -> Result<AppSettings, String> {
    let report_typed_persistence_failure = |error| {
        report_settings_save_error(
            error,
            SettingsSaveReportSurface::TerminalSnapshotCompareAndSet,
        )
    };
    let lock_guard = match SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "compare_and_set_terminal_snapshots_enabled_at_path",
    ) {
        Ok(lock_guard) => lock_guard,
        Err(error) => return Err(report_typed_persistence_failure(error)),
    };
    let disk = match read_disk_object_for_write_typed(path, SettingsSaveStage::PreserveDiskGate) {
        Ok(disk) => disk,
        Err(error) => {
            drop(lock_guard);
            return Err(report_typed_persistence_failure(error));
        }
    };
    if let Err(error) = ensure_project_paths_migrated_at_locked_path(path, &mut |_, _| Ok(())) {
        drop(lock_guard);
        return Err(report_typed_persistence_failure(error));
    }
    let disk_file_absent = disk.is_none();
    // An absent file compares as the default-on value, but its absence is
    // tracked separately: the idempotent early return below may not report
    // success before a valid settings file exists, because the strict reader
    // cannot authorize anything without one.
    let disk_gate = match disk
        .as_ref()
        .and_then(|object| object.get(FIELD_TERMINAL_SNAPSHOTS_ENABLED))
    {
        Some(Value::Bool(value)) => *value,
        None => true,
        Some(_) => return Err("terminal_snapshot_setting_save_failed".to_string()),
    };
    if disk_gate != expected && disk_gate != enabled {
        return Err("terminal_snapshot_setting_conflict".to_string());
    }

    // #1737 (D17): the disk-decoded candidate must carry the overlay VALUES, not just
    // the overlay state, because the `disk_gate == enabled` early return below adopts
    // it with no save and no re-decode. The `None` arm clones the live settings and
    // already carries both.
    let effective: Map<String, Value> = if current.local_overlay_state.is_empty() {
        Map::new()
    } else {
        match serde_json::to_value(current) {
            Ok(Value::Object(object)) => object,
            _ => Map::new(),
        }
    };
    let mut candidate = match disk {
        Some(object) => decode_disk_settings_for_terminal_snapshot_cas(
            object,
            path,
            &current.local_overlay_state,
            &effective,
        )
        .map_err(|_| "terminal_snapshot_setting_save_failed".to_string())?,
        None => {
            let mut fresh = current.clone();
            let projection = select_project_source_at_locked_path(path)
                .map_err(|error| {
                    report_settings_save_error(
                        error,
                        SettingsSaveReportSurface::TerminalSnapshotCompareAndSet,
                    )
                })?
                .projection;
            apply_selected_project_state(&mut fresh, &projection);
            fresh
        }
    };
    candidate.local_overlay_state = current.local_overlay_state.clone();
    candidate.terminal_snapshots_enabled = enabled;
    if !disk_file_absent && disk_gate == enabled {
        return Ok(candidate);
    }

    // #2306 P1 - renumber in the candidate's current (effective) vector order
    // right before the direct write; never re-sort from stored ordinals.
    normalize_agent_order(&mut candidate.agents);
    let written = match save_settings_value_locked(
        &candidate,
        path,
        ProjectWriteMode::Preserve,
        TerminalSnapshotGateWriteMode::Explicit(enabled),
        MainWindowPlacementWriteMode::Preserve,
    ) {
        Ok(written) => written,
        Err(error) => {
            drop(lock_guard);
            return Err(report_typed_persistence_failure(error));
        }
    };
    let (bytes, _) = crate::path_identity::read_bounded_regular(path, 1024 * 1024)
        .map_err(|_| "terminal_snapshot_setting_save_failed".to_string())?;
    let value = crate::path_identity::parse_json_no_duplicates(&bytes)
        .map_err(|_| "terminal_snapshot_setting_save_failed".to_string())?;
    if value
        .as_object()
        .and_then(|object| object.get(FIELD_TERMINAL_SNAPSHOTS_ENABLED))
        .and_then(Value::as_bool)
        != Some(enabled)
    {
        return Err("terminal_snapshot_setting_save_failed".to_string());
    }
    Ok(written)
}

pub(crate) fn decode_disk_settings_for_terminal_snapshot_cas(
    object: Map<String, Value>,
    settings_path: &Path,
    overlay: &LocalSettingsOverlay,
    effective: &Map<String, Value>,
) -> Result<AppSettings, String> {
    let base = production_instance_base();
    let mut value = Value::Object(object);
    let selected = select_project_source_at_locked_path(settings_path).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    replace_project_projection(value.as_object_mut().unwrap(), &selected.projection);
    // #2716 (B3) 4.2b: the same merge as `parse_settings_json`, in the same layer
    // order, under the overlay reapplied below. The decoded value is adopted and
    // written, so without it an empty `agents` would reach the agents file.
    let main_profile_owner = value.as_object().expect("settings object").clone();
    let agents_layer = merge_agents_instance_file(settings_path, &mut value);
    let physical_sidecar = read_agents_instance_file(&agents_instance_path(settings_path))
        .ok()
        .flatten();
    let profile_owner = selected_registered_profile_owner(
        &main_profile_owner,
        physical_sidecar.as_ref(),
        !overlay.owns_top_level("codingAgentProfiles"),
    );
    let physical_groups = physical_sidecar
        .map(|groups| {
            agents_instance_groups(&groups)
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let identity_physical =
        coding_agent_identity_with_physical_overlay(value.clone(), settings_path, &physical_groups);
    let base_profiles = value.get("codingAgentProfiles").cloned();
    if let Some(profiles) = registered_profiles_for_read(&profile_owner).get("codingAgentProfiles")
    {
        value["codingAgentProfiles"] = profiles.clone();
    }
    // #1737 (D17): re-apply the overlay to the disk-decoded object, so the
    // `disk_gate == enabled` early return does not hand the caller base values.
    // Pinned before the project decode, and therefore strictly before
    // `candidate.terminal_snapshots_enabled = enabled` runs on the typed value.
    if let Value::Object(map) = &mut value {
        overlay.reapply_from(effective, map);
    }
    restore_codex_home_carrier(value.as_object_mut().unwrap(), base_profiles.as_ref());
    let state =
        apply_project_decode_to_value(&mut value, base.as_deref(), &projects::FsCandidateResolver);
    let mut settings: AppSettings =
        serde_json::from_value(value).map_err(|_| "settings_invalid".to_string())?;
    settings.project_path_state = Arc::new(state);
    set_agents_layer_state(&mut settings, settings_path, agents_layer);
    settings.coding_agent_identity_physical = Some(Arc::new(identity_physical));
    // #2306 P1 - finalize before the caller's no-op return or direct write, so
    // both the returned and the written snapshot carry contiguous ordinals.
    // Overlay values stay in memory: `save_settings_value_locked` restores the
    // base `agents` array before writing the file.
    finalize_agent_order(&mut settings.agents);
    refresh_agent_id_hashes(&mut settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    repair_loaded_response_close_settings(&mut settings, &settings_path.display().to_string());
    repair_loaded_room_idle_confirmation_settings(
        &mut settings,
        &settings_path.display().to_string(),
    );
    Ok(settings)
}

/// #2348 - the sole persistence owner for the main-window placement pair. The
/// caller holds the managed settings write guard while this function serializes
/// every AC process with the settings file lock. Returns the re-decoded settings
/// the caller must adopt.
pub(crate) fn set_main_window_placement(
    current: &AppSettings,
    geometry: &WindowGeometry,
    display_state: MainWindowDisplayState,
) -> Result<AppSettings, String> {
    let path = settings_path().ok_or_else(|| "main_window_placement_save_failed".to_string())?;
    set_main_window_placement_at_path(current, &path, geometry, display_state)
}

pub(crate) fn set_main_window_placement_at_path(
    current: &AppSettings,
    path: &Path,
    geometry: &WindowGeometry,
    display_state: MainWindowDisplayState,
) -> Result<AppSettings, String> {
    // #1737 (D7c): `restore_base` is the last writer over the base file, so a
    // placement write beneath an overlay that owns either placement key would be
    // silently reverted. Refuse with the stable signal instead of pretending.
    if current
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        || current
            .local_overlay_state
            .owns_top_level(OVERLAY_KEY_MAIN_DISPLAY_STATE)
    {
        return Err("main_window_placement_overlay_pinned".to_string());
    }
    let _lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "set_main_window_placement_at_path",
    )
    .map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    save_settings_value_locked(
        current,
        path,
        ProjectWriteMode::Preserve,
        TerminalSnapshotGateWriteMode::Preserve,
        MainWindowPlacementWriteMode::Explicit {
            geometry: geometry.clone(),
            display_state,
        },
    )
    .map_err(|error| report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings))
}

pub(crate) fn read_project_paths_from_disk(
    path: &Path,
) -> Result<Option<DiskProjectLists>, String> {
    let transaction =
        empty_project_transaction(path, "read_project_paths_from_disk").map_err(|error| {
            report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
        })?;
    let selected = select_project_source_locked(&transaction).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    if selected.legacy.is_none() && selected.destination.is_none() {
        return Ok(None);
    }
    project_lists_from_projection(&selected.projection)
}

pub(crate) fn project_lists_from_projection(
    projection: &Map<String, Value>,
) -> Result<Option<DiskProjectLists>, String> {
    let value = Value::Object(projection.clone());
    let path = Path::new("project-paths.json");
    let project_paths = match value.get("projectPaths") {
        None | Some(serde_json::Value::Null) => value
            .get(FIELD_PROJECT_PATH)
            .and_then(Value::as_str)
            .map(str::to_string)
            .into_iter()
            .collect(),
        // #881: non-string elements are dropped here, but rejected in the
        // archivedProjectPaths arm below. Deliberate. `projectPaths` only feeds
        // discovery; `archivedProjectPaths` feeds session retention, where a
        // silently-emptied list deletes sessions. Tightening this one is
        // #888's call, not #881's.
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect::<Vec<_>>(),
        Some(other) => {
            return Err(format!(
                "{}: projectPaths is {}, not an array (aborting save)",
                path.display(),
                other
            ));
        }
    };
    let project_path = value
        .get("projectPath")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let archived_project_paths = match value.get("archivedProjectPaths") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(arr)) => {
            // #881 R2-G5: a non-string element is corruption, not an entry to
            // skip. The `projectPaths` arm above silently drops non-strings;
            // do not copy that here. `[123]` would read as `Some(vec![])`,
            // meaning disk authoritatively says nothing is archived.
            let mut archived = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(archived_path) = item.as_str() else {
                    return Err(format!(
                        "{}: archivedProjectPaths contains {}, not a string (aborting save)",
                        path.display(),
                        item
                    ));
                };
                archived.push(archived_path.to_string());
            }
            Some(archived)
        }
        Some(other) => {
            return Err(format!(
                "{}: archivedProjectPaths is {}, not an array (aborting save)",
                path.display(),
                other
            ));
        }
    };
    Ok(Some(DiskProjectLists {
        project_paths,
        project_path,
        archived_project_paths,
    }))
}

/// #778/#881: overwrite `settings`' in-memory project lists with the
/// disk-authoritative ones. The deliberate list mutators
/// (`open_project`/`new_project`/`remove_project`)
/// call this under the `SettingsState` write lock BEFORE they add/remove, so a
/// concurrent CLI append is reconciled into the list rather than clobbered by the
/// following verbatim write. Aborts (propagates `Err`) on a non-`NotFound` read
/// error per G2; a missing home dir is a no-op (the following save surfaces it).
pub fn refresh_project_paths_from_disk(settings: &mut AppSettings) -> Result<(), String> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    refresh_project_paths_from_path(settings, &path)
}

pub(crate) fn refresh_project_paths_from_path(
    settings: &mut AppSettings,
    path: &Path,
) -> Result<(), String> {
    let lists = read_project_paths_from_disk(path).inspect_err(|error| {
        settings.project_paths_persistence_error = Some(error.clone());
    })?;
    if let Some(lists) = lists {
        settings.project_paths = lists.project_paths;
        settings.project_path = lists.project_path;
        settings.archived_project_paths = lists.archived_project_paths.unwrap_or_default();
    }
    settings.project_paths_persistence_error = None;
    Ok(())
}

/// #1077 §4.3 step 2: fresh-read and fully re-decode the six disk fields (so a
/// post-startup CLI write is authoritative), replacing the runtime lists with the
/// SELECTED canonical paths and installing the fresh hidden pair state. Aborts
/// (`Err`) on a present-but-invalid whole object; keeps the live runtime/hidden
/// state when the file is absent.
#[cfg(test)]
pub(crate) fn refresh_and_decode_project_paths_from_path(
    settings: &mut AppSettings,
    path: &Path,
) -> Result<(), String> {
    let lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "refresh_and_decode_project_paths_from_path",
    )
    .map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    let projection = (|| {
        read_disk_object_for_write_typed(path, SettingsSaveStage::ProjectPathsRefreshDiskGate)?;
        select_project_source_at_locked_path(path).map(|selected| selected.projection)
    })();
    drop(lock);
    let projection = projection.map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    apply_selected_project_state(settings, &projection);
    Ok(())
}

/// Whether the current hidden project state carries structural corruption, which
/// must block any explicit project-list mutation (§4.2).
pub(crate) fn project_state_has_structural(settings: &AppSettings) -> bool {
    settings.project_paths_persistence_error.is_some()
        || settings.project_path_state.has_structural()
}
