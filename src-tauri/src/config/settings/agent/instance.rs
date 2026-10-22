mod layer;
mod overlay;

#[cfg(test)]
use super::super::project_atomic_checkpoint;
#[cfg(windows)]
use super::super::replace_settings_file_atomic;
use super::super::{
    read_disk_object_for_write_typed, refresh_physical_pair_id_hashes,
    serialize_settings_component, validate_physical_identity_delta, AgentsInstanceRead,
    AppSettings, CodingAgentProfilesConfig, ProjectWriteMode, SettingsFileLock, SettingsSaveError,
    SettingsSaveLegacyOutward, SettingsSaveReason, SettingsSaveStage, OVERLAY_KEY_AGENTS,
    SAVE_OP_ID,
};
use super::{
    duplicate_profile_document, registered_profile_raw_gate, registered_profiles_for_read,
    retain_configuration_commands, AgentConfig, StartupAgentRows,
};
use crate::config::instance_artifacts::{
    AGENTS_INSTANCE_FILE_NAME, SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
};
pub(crate) use layer::{
    agents_layer_state_at, agents_layer_state_at_labeled, agents_layer_unreadable_for_dir,
    agents_unreadable_cli_error, agents_unreadable_session_error, agents_unreadable_startup_notice,
    set_agents_layer_state, try_with_agents_layer_publication_state, AgentsLayerState,
};
#[cfg(test)]
pub(crate) use layer::{
    agents_unreadable_dirs, try_with_agents_layer_publication_state_in, AgentsPublicationLockError,
};
#[cfg(test)]
pub(crate) use overlay::absent_main_overlay_comparison;
pub(crate) use overlay::{
    prepare_registered_profile_owner, refresh_registered_agent_row_metadata,
    restore_agents_groups_for_redecode, restore_codex_home_carrier,
    restrict_split_profile_mutation_rows, retain_selected_legacy_profile_aliases,
    retain_unknown_registered_agent_fields, validate_registered_overlay_profile_edits,
    validate_split_canonical_profile_edits,
};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

/// #2716 (B3) - the two top-level groups that live in `agents.30.instance.no-git.json`
/// instead of the settings file. Serde names, so the file reuses the settings shape.
pub(crate) const AGENTS_INSTANCE_KEYS: [&str; 2] = ["agents", "codingAgentProfiles"];

/// #2716 (B3) - the agents file sits beside the settings file.
pub(crate) fn agents_instance_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENTS_INSTANCE_FILE_NAME)
}

/// #2716 (B3) - `Ok(None)` when the file is absent. A file that is unreadable, is not
/// a JSON object, or holds a group that does not decode as its `AppSettings` field is
/// an `Err`: callers keep what they have and never replace it.
pub(crate) fn read_agents_instance_file(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    read_agents_instance_file_labeled(path, path)
}

pub(crate) fn read_agents_instance_file_labeled(
    path: &Path,
    label: &Path,
) -> Result<Option<Map<String, Value>>, String> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not read {}: {e}", label.display())),
    };
    parse_agents_instance_contents(&contents, label).map(Some)
}

/// The decode half of `read_agents_instance_file_labeled`, shared with the owner's
/// single capture so both judge the same bytes the same way.
pub(crate) fn parse_agents_instance_contents(
    contents: &str,
    label: &Path,
) -> Result<Map<String, Value>, String> {
    if let Some(reason) = duplicate_profile_document(contents.as_bytes(), label) {
        log::warn!("{reason}");
    }
    let map = match serde_json::from_str::<Value>(contents) {
        Ok(Value::Object(map)) => map,
        Ok(_) => return Err(format!("{} is not a JSON object", label.display())),
        Err(e) => return Err(format!("{} is not valid JSON: {e}", label.display())),
    };
    if let Some(agents) = map.get(AGENTS_INSTANCE_KEYS[0]) {
        serde_json::from_value::<Vec<AgentConfig>>(agents.clone())
            .map_err(|e| format!("{} holds an undecodable agents array: {e}", label.display()))?;
    }
    let source = Value::Object(map.clone());
    let projected = registered_profiles_for_read(&source);
    if let Some(profiles) = projected.get(AGENTS_INSTANCE_KEYS[1]) {
        serde_json::from_value::<CodingAgentProfilesConfig>(profiles.clone()).map_err(|e| {
            format!(
                "{} holds an undecodable codingAgentProfiles object: {e}",
                label.display()
            )
        })?;
    }
    Ok(map)
}

/// #2716 (B3) - the groups `object` actually holds, key by key.
pub(crate) fn agents_instance_groups(object: &Map<String, Value>) -> Map<String, Value> {
    AGENTS_INSTANCE_KEYS
        .iter()
        .filter_map(|key| object.get(*key).map(|v| ((*key).to_string(), v.clone())))
        .collect()
}

/// #2716 (B3) - load merge, on the raw base object before the overlay merge: each
/// group the agents file holds replaces the base object's key. When no source
/// supplies `agents`, `[]` is inserted, so a deleted or corrupt agents file costs
/// the agent list and never every other setting. A corrupt file is logged, keeps
/// the values already in `base`, and returns the unreadable state (option C).
pub(crate) fn merge_agents_instance_file(
    settings_path: &Path,
    base: &mut Value,
) -> AgentsLayerState {
    merge_agents_instance_file_labeled(settings_path, settings_path, base)
}

pub(crate) fn merge_agents_instance_file_labeled(
    io_path: &Path,
    settings_path: &Path,
    base: &mut Value,
) -> AgentsLayerState {
    merge_agents_instance_file_with_capture(io_path, settings_path, base, None)
}

pub(crate) fn merge_agents_instance_file_with_capture(
    io_path: &Path,
    settings_path: &Path,
    base: &mut Value,
    capture: Option<&mut Option<StartupAgentRows>>,
) -> AgentsLayerState {
    let path = agents_instance_path(settings_path);
    let read = read_agents_instance_file_labeled(&agents_instance_path(io_path), &path);
    merge_agents_instance_groups(path, read, base, capture)
}

/// The merge half of `merge_agents_instance_file_with_capture`, over a read the
/// caller already made, so one capture never reads the agents file twice.
pub(crate) fn merge_agents_instance_groups(
    path: PathBuf,
    read: AgentsInstanceRead,
    base: &mut Value,
    capture: Option<&mut Option<StartupAgentRows>>,
) -> AgentsLayerState {
    let mut sidecar_owner = false;
    let mut sidecar_absent = false;
    let state = match read {
        Ok(None) => {
            sidecar_absent = true;
            AgentsLayerState::Readable
        }
        Ok(Some(sidecar)) => {
            sidecar_owner = sidecar.contains_key("agents");
            if let Some(object) = base.as_object_mut() {
                object.extend(agents_instance_groups(&sidecar));
            }
            AgentsLayerState::Readable
        }
        Err(reason) => {
            log::error!("{} ({reason})", agents_unreadable_startup_notice(&path));
            AgentsLayerState::Unreadable { path, reason }
        }
    };
    if let Some(object) = base.as_object_mut() {
        object
            .entry(AGENTS_INSTANCE_KEYS[0])
            .or_insert_with(|| Value::Array(Vec::new()));
    }
    if let Some(capture) = capture {
        if matches!(state, AgentsLayerState::Readable) {
            *capture = base
                .get("agents")
                .cloned()
                .and_then(|raw| StartupAgentRows::capture(raw, sidecar_owner, sidecar_absent));
        }
    }
    state
}

/// #2716 (B3) - writes the agents file through `<name>.<pid>.<op>.tmp`, synced, then
/// replaced onto the destination. The temporary is removed on any failure.
pub(crate) fn write_agents_instance_file(
    groups: &Map<String, Value>,
    path: &Path,
) -> Result<(), SettingsSaveError> {
    use std::io::Write as _;

    let bytes = serde_json::to_vec_pretty(&Value::Object(groups.clone())).map_err(|source| {
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    let op_id = SAVE_OP_ID.fetch_add(1, Ordering::Relaxed);
    let tmp_path = path.with_file_name(format!(
        "{AGENTS_INSTANCE_FILE_NAME}.{}.{op_id}.tmp",
        std::process::id()
    ));
    let io_error = |stage: SettingsSaveStage, source: std::io::Error| {
        SettingsSaveError::io(
            stage,
            path,
            Some(&tmp_path),
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    };
    let result = (|| {
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempCreate)?;
        let mut temporary = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|e| io_error(SettingsSaveStage::TempCreate, e))?;
        temporary
            .write_all(&bytes)
            .map_err(|e| io_error(SettingsSaveStage::TempWrite, e))?;
        temporary
            .sync_all()
            .map_err(|e| io_error(SettingsSaveStage::TempSync, e))?;
        drop(temporary);
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::AtomicReplace)?;
        replace_settings_file_atomic(&tmp_path, path)
            .map_err(|e| io_error(SettingsSaveStage::AtomicReplace, e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

/// #2716 (B3) - one-shot export of the legacy `agents` and `codingAgentProfiles`
/// keys still in the settings file. Writes the agents file before anything is
/// stripped and moves the disk values verbatim; true means "save", and that save
/// strips the keys. Never overwrites or merges an existing agents file.
pub(crate) fn export_agents_to_instance_file(
    settings: &mut AppSettings,
    settings_path: &Path,
) -> bool {
    // #2716 (B3) r12n: the read, the existence check and the write are one
    // decision, so they run under the settings lock. It is released on return,
    // before the loader's own save takes it again.
    let _lock = match SettingsFileLock::acquire_contextual(
        settings_path,
        std::time::Duration::from_secs(2),
        "export_agents_to_instance_file",
    ) {
        Ok(lock) => lock,
        Err(e) => {
            log::error!("[settings-migration] #2716 - {e}; nothing exported, nothing stripped");
            return false;
        }
    };
    if let Err(reason) = registered_profile_raw_gate(settings_path) {
        log::warn!("{reason}");
        return false;
    }
    let legacy = match std::fs::read_to_string(settings_path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
    {
        Some(Value::Object(disk)) => agents_instance_groups(&disk),
        _ => return false,
    };
    if legacy.is_empty() {
        return false;
    }
    if legacy
        .get("codingAgentProfiles")
        .and_then(|profiles| profiles.get("profileBindingVersion"))
        == Some(&Value::from(1))
    {
        // Canonical groups keep their physical owner. Only the admitted legacy
        // preparation may transfer a registered profile packet to the sidecar.
        return false;
    }
    let path = agents_instance_path(settings_path);
    if settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_AGENTS)
    {
        log::info!(
            "[settings-migration] #2716 - the overlay {} owns agents; agents and \
                codingAgentProfiles stay in {} and are not exported",
            settings_path
                .with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME)
                .display(),
            settings_path.display()
        );
        return false;
    }
    match read_agents_instance_file(&path) {
        Err(e) => {
            log::error!("[settings-migration] #2716 - {e}; nothing exported, nothing stripped");
            false
        }
        Ok(Some(_)) => {
            log::warn!(
                "[settings-migration] #2716 - {} still carries agents or codingAgentProfiles while \
                    {} exists; the agents file wins and is not overwritten",
                settings_path.display(),
                path.display()
            );
            false
        }
        Ok(None) => match write_agents_instance_file(&legacy, &path) {
            Ok(()) => {
                log::info!(
                    "[settings-migration] #2716 - exported agents and codingAgentProfiles to {}",
                    path.display()
                );
                true
            }
            Err(e) => {
                log::error!("[settings-migration] #2716 - {e}; nothing stripped");
                false
            }
        },
    }
}

/// #2716 (B3) - the agents-file half of a save, run under the settings lock before
/// the settings file is written. Returns the groups the agents file now holds, which
/// are exactly the keys the caller may strip from `out`. `Reconcile` never writes the
/// caller's groups; `Preserve` writes them except a key the overlay owns.
pub(crate) fn save_agents_instance_file(
    settings: &AppSettings,
    path: &Path,
    mode: &ProjectWriteMode,
    #[cfg(test)] disk: Option<&Map<String, Value>>,
    out: &Map<String, Value>,
    disk_gate_stage: SettingsSaveStage,
    prepared_groups: (Option<&Value>, Option<&Value>, Option<bool>),
) -> Result<Map<String, Value>, SettingsSaveError> {
    let (prepared_duplicate_agents, prepared_profiles, profiles_in_sidecar) = prepared_groups;
    let sidecar_path = agents_instance_path(path);
    let existing = read_agents_instance_file(&sidecar_path).map_err(|e| {
        SettingsSaveError::io(
            disk_gate_stage,
            &sidecar_path,
            None,
            std::io::Error::other(e.clone()),
            SettingsSaveLegacyOutward::DiskRead(format!(
                "Refusing to overwrite {}: {e}",
                sidecar_path.display()
            )),
        )
    })?;
    let existing_groups = existing.as_ref().map(agents_instance_groups);
    // Read the physical base, never the request or its overlay. The legacy
    // settings group remains authoritative only when the sidecar lacks it.
    let legacy = if existing_groups.as_ref().is_some_and(|groups| {
        AGENTS_INSTANCE_KEYS
            .iter()
            .all(|key| groups.contains_key(*key))
    }) {
        None
    } else {
        read_disk_object_for_write_typed(path, disk_gate_stage)?
    };
    let base_profiles = existing_groups
        .as_ref()
        .and_then(|groups| groups.get("codingAgentProfiles"))
        .or_else(|| {
            legacy
                .as_ref()
                .and_then(|disk| disk.get("codingAgentProfiles"))
        });
    let target = match mode {
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => {
            let target = existing
                .clone()
                .unwrap_or_else(|| disk.map(agents_instance_groups).unwrap_or_default());
            if existing.is_none() && !target.is_empty() {
                write_agents_instance_file(&target, &sidecar_path)?;
            }
            return Ok(agents_instance_groups(&target));
        }
        ProjectWriteMode::Preserve => {
            // With neither physical file present, an overlay-backed first save
            // establishes the main owner only. Defaults/effective overlay data
            // do not grant authority to create a registered sidecar.
            if existing.is_none() && legacy.is_none() && !settings.local_overlay_state.is_empty() {
                return Ok(Map::new());
            }
            collect_preserved_agents_groups(
                settings,
                &sidecar_path,
                out,
                (existing_groups.as_ref(), base_profiles),
                profiles_in_sidecar,
            )?
        }
    };
    // A carrier belongs to a group this file already owns. An empty target
    // must not create an absent sidecar from a legacy main-file carrier.
    if existing.is_none() && target.is_empty() {
        return Ok(Map::new());
    }
    let mut before = legacy
        .as_ref()
        .map(agents_instance_groups)
        .unwrap_or_default();
    if let Some(groups) = &existing_groups {
        before.extend(groups.clone());
    }
    let editable_agents = matches!(mode, ProjectWriteMode::Preserve)
        && !settings.local_overlay_state.owns_top_level("agents");
    let editable_profiles = matches!(mode, ProjectWriteMode::Preserve)
        && !settings
            .local_overlay_state
            .owns_top_level("codingAgentProfiles");
    let rejected = |reason: String| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            &sidecar_path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    let mut retained = Value::Object(target);
    if editable_agents {
        if let Some(prepared) = prepared_duplicate_agents {
            retained["agents"] = prepared.clone();
        }
    }
    retain_configuration_commands(
        &mut retained,
        &Value::Object(before.clone()),
        "id",
        false,
        editable_agents && prepared_duplicate_agents.is_none(),
        editable_profiles,
    )
    .map_err(&rejected)?;
    let mut target = retained
        .as_object()
        .expect("retained settings object")
        .clone();
    if editable_profiles {
        // A present empty physical agents array is authoritative. Never substitute
        // the effective (possibly overlay-owned) base for physical metadata.
        let agents = target
            .get("agents")
            .or_else(|| before.get("agents"))
            .cloned();
        if !editable_agents {
            if let Some(rows) = &agents {
                let _: Vec<AgentConfig> = serde_json::from_value(rows.clone())
                    .map_err(|error| rejected(error.to_string()))?;
            }
        }
        if let Some(profiles) = target.get_mut("codingAgentProfiles") {
            refresh_physical_pair_id_hashes(agents.as_ref(), profiles);
        }
    }
    if let Some(prepared) = prepared_profiles {
        if editable_profiles && target.contains_key("codingAgentProfiles") {
            match prepared.get("codingAgentProfiles") {
                Some(profiles) => {
                    target.insert("codingAgentProfiles".into(), profiles.clone());
                }
                None => {
                    target.remove("codingAgentProfiles");
                }
            }
        }
        if editable_agents && target.contains_key("agents") {
            if let Some(rows) = prepared.get("agents") {
                target.insert("agents".into(), rows.clone());
            }
        }
    }
    // Prepared packets may retain original raw rows. Derive metadata from the
    // final physical rows, after insertion, without changing protected rows.
    if editable_agents {
        refresh_registered_agent_row_metadata(&mut target).map_err(&rejected)?;
    }
    // Prepared canonical profiles cannot replace the physical owner's HOME carrier.
    restore_codex_home_carrier(&mut target, base_profiles);
    if editable_agents || editable_profiles {
        let physical_before = if prepared_profiles.is_some()
            && before
                .get("codingAgentProfiles")
                .is_some_and(|profiles| profiles.get("profileBindingVersion").is_none())
        {
            // A legacy same-owner transfer already owns these incumbents in
            // main. Moving their packet must not turn them into new duplicates.
            before.clone()
        } else {
            existing.clone().unwrap_or_default()
        };
        validate_physical_identity_delta(
            &physical_before,
            &target,
            editable_agents,
            editable_profiles,
        )
        .map_err(rejected)?;
    }
    // Preserve unrelated physical fields as well as recognized groups.
    if let Some(existing) = &existing {
        for (key, value) in existing {
            if !AGENTS_INSTANCE_KEYS.contains(&key.as_str()) {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    if existing.as_ref() == Some(&target) {
        return Ok(agents_instance_groups(&target));
    }
    if !target.is_empty() {
        write_agents_instance_file(&target, &sidecar_path)?;
    }
    Ok(agents_instance_groups(&target))
}

pub(crate) fn collect_preserved_agents_groups(
    settings: &AppSettings,
    sidecar_path: &Path,
    out: &Map<String, Value>,
    physical: (Option<&Map<String, Value>>, Option<&Value>),
    profiles_in_sidecar: Option<bool>,
) -> Result<Map<String, Value>, SettingsSaveError> {
    let (existing_groups, base_profiles) = physical;
    let mut groups = existing_groups.cloned().unwrap_or_default();
    // `out` is read after `write_identity_rows` and `restore_base`, so
    // the #2450 identity rows and the base values reach the agents file.
    for key in AGENTS_INSTANCE_KEYS {
        // Publication eligibility comes from the original physical owner,
        // independently of whether profile conversion was possible. A
        // canonical main owner stays put; legacy and new ordinary groups
        // retain the established materialization path.
        let owns = existing_groups
            .as_ref()
            .is_some_and(|groups| groups.contains_key(key))
            || !base_profiles.is_some_and(|profiles| {
                profiles.get("profileBindingVersion") == Some(&Value::from(1))
            });
        let canonical_split_rows = key == "agents"
            && existing_groups.as_ref().is_some_and(|groups| {
                !groups.contains_key("agents")
                    && groups.get("codingAgentProfiles").is_some_and(|profiles| {
                        profiles.get("profileBindingVersion") == Some(&Value::from(1))
                    })
            });
        if !owns
            || canonical_split_rows
            || (key == "codingAgentProfiles" && profiles_in_sidecar == Some(false))
        {
            continue;
        }
        if settings.local_overlay_state.owns_top_level(key) {
            continue;
        }
        let value = match out.get(key) {
            Some(value) => value.clone(),
            None if key == AGENTS_INSTANCE_KEYS[0] => {
                serialize_settings_component(&settings.agents, sidecar_path)?
            }
            None => serialize_settings_component(&settings.coding_agent_profiles, sidecar_path)?,
        };
        groups.insert(key.to_string(), value);
    }
    Ok(groups)
}
