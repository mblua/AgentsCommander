#[cfg(test)]
use super::super::SETTINGS_PRESERVE_LOCK_HOOK;
use super::super::{
    agents_instance_path, read_agents_instance_file, replace_project_projection,
    restore_codex_home_carrier, retain_configuration_commands,
    retain_configuration_commands_with_startup, source_general_field_names, write_identity_rows,
    AppSettings, MainWindowPlacementWriteMode, ProjectWriteMode, StartupAgentRows,
    TerminalSnapshotGateWriteMode, AGENTS_INSTANCE_KEYS, FIELD_MAIN_DISPLAY_STATE,
    FIELD_MAIN_GEOMETRY, FIELD_TERMINAL_SNAPSHOTS_ENABLED,
};
use super::{
    save_settings_value_locked_with_startup, SettingsFileLock, SettingsSaveError,
    SettingsSaveLegacyOutward, SettingsSaveReason, SettingsSaveStage,
};
use crate::config::local_overlay::LocalSettingsOverlay;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::Path;

/// #2348 - serialize one placement component with the same error shape as the
/// whole-object serializer, so a (theoretically impossible) serialization failure
/// aborts the save instead of writing a partial pair.
pub(crate) fn serialize_settings_component<T: serde::Serialize>(
    value: &T,
    path: &Path,
) -> Result<Value, SettingsSaveError> {
    serde_json::to_value(value).map_err(|source| {
        let outward = format!("Failed to serialize settings: {source}");
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::Serialize(outward),
        )
    })
}

/// #2348 - central `Preserve` handling for the placement pair. A present disk
/// VALUE is authoritative; a missing key and an explicit `null` both mean "no
/// disk truth" (the same reading `read_project_paths_from_disk` uses for the
/// `Option` fields), so neither erases a caller value synthesized by the
/// load-time `terminalGeometry` -> `mainGeometry` migration. Split out so each
/// key's disk-present/absent x caller-present/absent combinations are testable
/// without building an `AppSettings`.
pub(crate) fn preserve_main_window_placement_keys(
    out: &mut Map<String, Value>,
    disk: &Map<String, Value>,
) {
    for key in [FIELD_MAIN_GEOMETRY, FIELD_MAIN_DISPLAY_STATE] {
        match disk.get(key) {
            None | Some(Value::Null) => {}
            Some(value) => {
                out.insert(key.to_string(), value.clone());
            }
        }
    }
}

pub(crate) fn preflight_duplicate_agents(
    settings: &AppSettings,
    path: &Path,
    disk: Option<&Map<String, Value>>,
    startup: Option<&StartupAgentRows>,
) -> Result<Option<Value>, SettingsSaveError> {
    let rejected = |reason: String| {
        SettingsSaveError::semantic(
            SettingsSaveStage::PreserveDiskGate,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    let sidecar = read_agents_instance_file(&agents_instance_path(path)).map_err(&rejected)?;
    let sidecar_owner = sidecar
        .as_ref()
        .is_some_and(|groups| groups.contains_key("agents"));
    let raw = sidecar
        .as_ref()
        .and_then(|groups| groups.get("agents"))
        .or_else(|| disk.and_then(|disk| disk.get("agents")))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    if let Some(context) = startup {
        context
            .validate_source(&raw, sidecar_owner)
            .map_err(&rejected)?;
    }
    let mut ids = HashSet::new();
    let has_duplicates = raw
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .any(|id| !ids.insert(id));
    let mut candidate_ids = HashSet::new();
    let newly_ambiguous = settings
        .agents
        .iter()
        .any(|agent| !candidate_ids.insert(agent.id.as_str()) && ids.contains(agent.id.as_str()));
    // Complete the set when the duplicate scan short-circuited.
    if !has_duplicates && !newly_ambiguous {
        return Ok(None);
    }
    let mut candidate = serialize_settings_component(settings, path)?;
    let object = candidate
        .as_object_mut()
        .expect("serialized settings object");
    write_identity_rows(object, &settings.local_overlay_state);
    settings.local_overlay_state.restore_base(object);
    let before = serde_json::json!({"agents":raw});
    retain_configuration_commands_with_startup(
        &mut candidate,
        &before,
        "id",
        false,
        true,
        false,
        startup,
    )
    .map_err(rejected)?;
    Ok(candidate.get("agents").cloned())
}

/// The #1077 project-aware atomic writer. Builds the output object per `mode`,
/// writes it atomically, then re-decodes the exact written value so the returned
/// `AppSettings` carries fresh runtime projections + hidden state (never the
/// possibly-stale caller state). See §4.2/§4.3.
pub(crate) fn save_settings_value(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
) -> Result<AppSettings, SettingsSaveError> {
    save_settings_value_with_startup(settings, path, mode, None)
}

pub(crate) fn save_settings_value_with_startup(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
    startup: Option<&StartupAgentRows>,
) -> Result<AppSettings, SettingsSaveError> {
    let _lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "save_settings_value",
    )?;
    #[cfg(test)]
    SETTINGS_PRESERVE_LOCK_HOOK.with(|hook| {
        if let Some(callback) = hook.borrow().as_ref() {
            callback(path);
        }
    });
    save_settings_value_locked_with_startup(
        settings,
        path,
        mode,
        TerminalSnapshotGateWriteMode::Preserve,
        MainWindowPlacementWriteMode::Preserve,
        startup,
    )
}

/// Locked half of the project-aware writer. Assumes `settings.json.lock` is
/// already held by the caller (both production entry points acquire it before
/// reaching here), and performs #2058 backup rotation after a successful write.
pub(crate) fn save_settings_value_locked(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
    terminal_snapshot_gate_mode: TerminalSnapshotGateWriteMode,
    main_window_placement_mode: MainWindowPlacementWriteMode,
) -> Result<AppSettings, SettingsSaveError> {
    save_settings_value_locked_with_startup(
        settings,
        path,
        mode,
        terminal_snapshot_gate_mode,
        main_window_placement_mode,
        None,
    )
}

pub(crate) fn restore_protected_agents_instance_groups(
    out: &mut Map<String, Value>,
    disk: Option<&Map<String, Value>>,
    overlay: &LocalSettingsOverlay,
    agents_groups: &Map<String, Value>,
) {
    let identity_rows = out
        .get("codingAgentProfiles")
        .and_then(|profiles| profiles.get("identityByAgent"))
        .cloned();
    for key in AGENTS_INSTANCE_KEYS {
        if !overlay.owns_top_level(key) || agents_groups.contains_key(key) {
            continue;
        }
        match disk.and_then(|disk| disk.get(key)) {
            Some(raw) => {
                out.insert(key.to_owned(), raw.clone());
            }
            None => {
                out.remove(key);
            }
        }
        // Only legacy MAIN identities retain the existing #2450 writer output.
        // All other fields of a protected profiles group remain raw disk values.
        if key == "codingAgentProfiles" {
            if out
                .get(key)
                .and_then(|profiles| profiles.get("profileBindingVersion"))
                == Some(&Value::from(1))
            {
                continue;
            }
            // No projected row grants deletion of raw identities.
            if let Some(rows) = &identity_rows {
                let profiles = out
                    .entry(key.to_owned())
                    .or_insert_with(|| Value::Object(Map::new()));
                let physical_ids: HashSet<_> = disk
                    .and_then(|disk| disk.get("agents"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|row| row.get("id").and_then(Value::as_str))
                    .collect();
                let identities = profiles
                    .as_object_mut()
                    .expect("validated profiles object")
                    .entry("identityByAgent")
                    .or_insert_with(|| serde_json::json!({}));
                if let (Some(identities), Some(rows)) =
                    (identities.as_object_mut(), rows.as_object())
                {
                    for (id, value) in rows {
                        if physical_ids.contains(id.as_str()) {
                            identities.insert(id.clone(), value.clone());
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn retain_untransferred_registered_main_groups(
    out: &mut Map<String, Value>,
    disk: Option<&Map<String, Value>>,
    published: &Map<String, Value>,
    sidecar_before: Option<&Map<String, Value>>,
    overlay: &LocalSettingsOverlay,
    path: &Path,
) {
    for key in AGENTS_INSTANCE_KEYS {
        let Some(destination) = published.get(key) else {
            continue;
        };
        let raw = disk.and_then(|map| map.get(key));
        let selected_main = !sidecar_before.is_some_and(|map| map.contains_key(key));
        if !overlay.owns_top_level(key) && (selected_main || raw == Some(destination)) {
            // Called only after the sidecar publisher confirmed this packet.
            out.remove(key);
        } else if let Some(raw) = raw {
            if !overlay.owns_top_level(key) && raw != destination {
                log::warn!(
                    "[profiles] {}: retaining unselected shadowed {key}; raw value differs \
                    from registered sidecar",
                    path.display()
                );
            }
            out.insert(key.into(), raw.clone());
        } else {
            out.remove(key);
        }
    }
}

pub(crate) type RegisteredMainPreparedGroups<'a> = (
    Option<&'a Value>,
    Option<&'a Value>,
    Option<&'a Map<String, Value>>,
    &'a Path,
);

pub(crate) fn prepare_registered_main_agent_groups(
    mut out: Map<String, Value>,
    settings: &AppSettings,
    disk: Option<&Map<String, Value>>,
    agents_groups: &Map<String, Value>,
    profiles_in_sidecar: bool,
    prepared_groups: RegisteredMainPreparedGroups<'_>,
    mode: &ProjectWriteMode,
) -> Result<Map<String, Value>, String> {
    let (prepared_profiles, prepared_duplicate_agents, sidecar_before, path) = prepared_groups;
    #[cfg(test)]
    if matches!(mode, ProjectWriteMode::Reconcile { .. }) {
        for key in AGENTS_INSTANCE_KEYS {
            match disk.and_then(|disk| disk.get(key)) {
                Some(raw) if !agents_groups.contains_key(key) => {
                    out.insert(key.into(), raw.clone());
                }
                _ => {
                    out.remove(key);
                }
            }
        }
        return Ok(out);
    }
    #[cfg(not(test))]
    let _ = mode;
    retain_untransferred_registered_main_groups(
        &mut out,
        disk,
        agents_groups,
        sidecar_before,
        &settings.local_overlay_state,
        path,
    );
    if profiles_in_sidecar
        && !settings
            .local_overlay_state
            .owns_top_level("codingAgentProfiles")
        && disk.is_none_or(|main| !main.contains_key("codingAgentProfiles"))
        && prepared_profiles.is_some_and(|prepared| prepared.get("codingAgentProfiles").is_none())
    {
        // Ambiguous agents without a physical profile group do not grant
        // authority to materialize the caller's repaired/default profile view.
        out.remove("codingAgentProfiles");
    }
    if !profiles_in_sidecar {
        if let Some(prepared) = prepared_profiles {
            if let Some(profiles) = prepared.get("codingAgentProfiles") {
                out.insert("codingAgentProfiles".into(), profiles.clone());
            }
            if !agents_groups.contains_key("agents") {
                if let Some(rows) = prepared.get("agents") {
                    out.insert("agents".into(), rows.clone());
                }
            }
        }
    }

    // Partial profile ownership can keep these groups in settings.json instead
    // of the sidecar. Retain its locked legacy leaves after raw overlay restore;
    // complete overlay-owned groups stay outside this writer's authority.
    if let Some(disk) = disk {
        let profiles_owned = settings
            .local_overlay_state
            .owned_paths()
            .iter()
            .any(|path| path.len() == 1 && path[0] == "codingAgentProfiles");
        let mut retained = Value::Object(out);
        if retained.get("agents").is_some() {
            if let Some(prepared) = prepared_duplicate_agents {
                retained["agents"] = prepared.clone();
            }
        }
        retain_configuration_commands(
            &mut retained,
            &Value::Object(disk.clone()),
            "id",
            false,
            !settings.local_overlay_state.owns_top_level("agents")
                && !agents_groups.contains_key("agents")
                && prepared_duplicate_agents.is_none(),
            !profiles_owned && !agents_groups.contains_key("codingAgentProfiles"),
        )?;
        out = retained
            .as_object()
            .expect("retained settings object")
            .clone();
    }

    // A synthesized legacy state (direct-constructed AppSettings) has no dirty
    // repair, so the eligible branches above never fire for it; its groups are
    // written verbatim from the live settings/disk, matching pre-#1077 behavior.
    // Restore complete protected groups after every retention/repair stage.
    // Sidecar key presence, including empty values, takes precedence over main.
    restore_protected_agents_instance_groups(
        &mut out,
        disk,
        &settings.local_overlay_state,
        agents_groups,
    );
    if !profiles_in_sidecar {
        restore_codex_home_carrier(
            &mut out,
            disk.and_then(|disk| disk.get("codingAgentProfiles")),
        );
    }
    retain_untransferred_registered_main_groups(
        &mut out,
        disk,
        agents_groups,
        sidecar_before,
        &settings.local_overlay_state,
        path,
    );
    Ok(out)
}

pub(crate) fn apply_registered_main_disk_fields(
    out: &mut Map<String, Value>,
    disk: Option<&Map<String, Value>>,
    path: &Path,
    disk_gate_stage: SettingsSaveStage,
    terminal_snapshot_gate_mode: TerminalSnapshotGateWriteMode,
    main_window_placement_mode: MainWindowPlacementWriteMode,
) -> Result<(), SettingsSaveError> {
    if let Some(raw) = disk {
        // Use Serde's existing field table without requiring the physical main
        // object to contain groups legitimately owned by the sidecar.
        let known = source_general_field_names();
        for (key, value) in raw {
            if !known.contains(&key.as_str()) {
                out.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    replace_project_projection(out, &Map::new());

    // #1173: a whole-settings writer cannot opt in or re-enable an explicit
    // value, and a present malformed gate is an error. An absent file or a
    // valid object with an absent legacy key materializes true: the default-on
    // phase deliberately reverses the shipped absent-key rule. Only the
    // dedicated CAS uses Explicit.
    let terminal_snapshots_enabled = match terminal_snapshot_gate_mode {
        TerminalSnapshotGateWriteMode::Explicit(enabled) => enabled,
        TerminalSnapshotGateWriteMode::Preserve => match disk {
            Some(disk) => match disk.get(FIELD_TERMINAL_SNAPSHOTS_ENABLED) {
                Some(Value::Bool(enabled)) => *enabled,
                None => true,
                Some(_) => {
                    return Err(SettingsSaveError::semantic(
                        disk_gate_stage,
                        path,
                        None,
                        SettingsSaveReason::TerminalSnapshotSettingInvalid,
                        SettingsSaveLegacyOutward::TerminalSnapshotSettingInvalid(
                            "terminal_snapshot_setting_invalid".to_string(),
                        ),
                    ));
                }
            },
            None => true,
        },
    };
    out.insert(
        FIELD_TERMINAL_SNAPSHOTS_ENABLED.to_string(),
        Value::Bool(terminal_snapshots_enabled),
    );

    // #2348: main-window placement is owned by the narrow placement command.
    // `Preserve` never regresses it: for each key independently, a PRESENT disk
    // value is authoritative, and an absent disk key leaves the caller value
    // alone. An absent legacy `mainGeometry`
    // must not erase the value the load-time migration synthesized in memory.
    // `Explicit` writes the pair exactly as the placement command received it.
    // Runs before `restore_base`, which stays the last writer over the base file.
    match main_window_placement_mode {
        MainWindowPlacementWriteMode::Preserve => {
            if let Some(disk) = disk {
                preserve_main_window_placement_keys(out, disk);
            }
        }
        MainWindowPlacementWriteMode::Explicit {
            geometry,
            display_state,
        } => {
            out.insert(
                FIELD_MAIN_GEOMETRY.to_string(),
                serialize_settings_component(&geometry, path)?,
            );
            out.insert(
                FIELD_MAIN_DISPLAY_STATE.to_string(),
                serialize_settings_component(&display_state, path)?,
            );
        }
    }

    Ok(())
}
