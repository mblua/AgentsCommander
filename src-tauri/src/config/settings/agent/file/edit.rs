//! Applies UI edits to a physical agent file and persists them.
//! Owns `prepare_agent_file_edits` (row patches, profile sets and slots, then command
//! validation), the registered-instance save, the remembered source per menu, and the async
//! `persist_agent_file_edits` entry point.
//! Project personal catalogs are routed to the scoped catalog operation in `authority`; this
//! module writes only the registered file and `settings.json`.

use super::super::super::{
    list_active_sources, source_bytes_revision, source_diagnostic, write_value_atomic_labeled,
    AppSettings, OperationOutcome, SettingsOperationGuard, SettingsPublisher, SettingsState,
    SourceAvailability, SourceKind, SourceReadContext, SourceRef, SourceRevision, SourceScope,
    SETTINGS_CAPTURE_LIMIT,
};
use super::super::instance::write_agents_instance_file;
use super::super::{
    canonical_profile_source, capture_registered_profile_gate, captured_registered_profile_object,
    is_valid_profile_letter, normalized_profile_cells, physical_profile_rows,
    project_physical_profiles, recheck_registered_profile_gate, validate_agent_commands,
    ProfileCellConfig, ProfileSlotConfig,
};
use super::super::{AgentConfig, AGENTS_INSTANCE_KEYS};
use super::{
    agent_file_checkpoint, agent_file_decode, agent_file_read, read_agent_file_snapshot,
    AgentFileEdits, AgentFileProfileEdit, AgentFileRowAction, AgentFileRowEdit,
    AgentFileSaveOutcome, AgentFileSaveReply, AgentFileSourceMenu, AgentFileSourcePreferences,
    SelectedAgentFileSnapshot,
};
use super::{
    config_dirs, config_file_unavailable, config_selection, config_snapshot, read_config_file,
    read_registered_shape, read_resolved, ConfigFileSnapshot,
};
use crate::config::coding_agent_files::locator::{normalize_config_name, resolve_locator};
use crate::config::coding_agent_files::{
    with_active, with_loaded, CodingAgentConfigFiles, ConfigContext, ConfigFileError,
    ConfigFileLocator, ConfigScope,
};
use crate::config::settings::{refresh_agent_id_hashes, CodingAgentProfilesConfig};
use crate::filesystem::settings_sources;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) fn prepare_agent_file_edits(
    before: &Value,
    registered: bool,
    edits: &AgentFileEdits,
) -> Result<Value, String> {
    let mut incoming = project_physical_profiles(before, registered)?;
    let key = if registered { "id" } else { "key" };
    let rows = incoming
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .ok_or("invalidAgentsArray")?;
    apply_agent_row_edits(rows, key, registered, &edits.rows)?;
    let mut canonical = canonical_profile_source(before, &incoming, registered)?;
    let hashes = physical_profile_rows(&canonical, registered)?;
    let owner = if registered {
        canonical
            .get_mut("codingAgentProfiles")
            .and_then(Value::as_object_mut)
            .ok_or("invalidProfileContainer")?
    } else {
        canonical.as_object_mut().ok_or("invalidSource")?
    };
    apply_profile_set_edits(owner, &hashes, &edits.profile_sets)?;
    if let Some(slots) = &edits.profile_slots {
        apply_profile_slot_edits(owner, slots)?;
    }
    let (agents, profiles) = agent_file_decode(&canonical, registered)?;
    validate_agent_commands(&AppSettings {
        agents,
        coding_agent_profiles: profiles,
        ..AppSettings::default()
    })?;
    Ok(canonical)
}

fn apply_agent_row_edits(
    rows: &mut Vec<Value>,
    key: &str,
    registered: bool,
    row_edits: &[AgentFileRowEdit],
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for edit in row_edits {
        if edit.entry_hint.is_empty() || !seen.insert(&edit.entry_hint) {
            return Err("duplicateOrInvalidRowEdit".into());
        }
        if !registered {
            crate::config::coding_agents_catalog::validate_catalog_key(&edit.entry_hint)?;
        }
        if edit.patch.contains_key("command") && edit.patch.contains_key("baseCommand") {
            return Err("ambiguousCommandEdit".into());
        }
        let index = rows
            .iter()
            .position(|row| row.get(key).and_then(Value::as_str) == Some(&edit.entry_hint));
        if matches!(edit.action, AgentFileRowAction::Delete) {
            if !edit.patch.is_empty() {
                return Err("deletePatchNotEmpty".into());
            }
            rows.remove(index.ok_or("unknownEntryHint")?);
            continue;
        }
        let mut row = index.map_or_else(
            || {
                let mut row = Map::new();
                row.insert(key.into(), Value::String(edit.entry_hint.clone()));
                Value::Object(row)
            },
            |i| rows[i].clone(),
        );
        let object = row.as_object_mut().ok_or("invalidAgentRow")?;
        for (field, value) in &edit.patch {
            apply_agent_row_field(object, field, value, registered)?;
        }
        if index.is_none() && !registered {
            let complete = serde_json::json!({"schemaVersion":2,"agents":[row.clone()]});
            crate::config::coding_agents_catalog::parse_catalog_schema2(
                &serde_json::to_vec(&complete).map_err(|_| "invalidAgentRow")?,
            )
            .map_err(|_| "incompleteNewCatalogRow")?;
        }
        if let Some(index) = index {
            rows[index] = row;
        } else {
            rows.push(row);
        }
    }
    Ok(())
}

fn apply_agent_row_field(
    object: &mut Map<String, Value>,
    field: &String,
    value: &Value,
    registered: bool,
) -> Result<(), String> {
    let common = [
        "label",
        "color",
        "command",
        "baseCommand",
        "envs",
        "instructionsFilename",
        "configSeed",
        "isolatedHome",
    ];
    let owned = common.contains(&field.as_str())
        || (registered
            && [
                "preflight",
                "preflightTimeoutSeconds",
                "contextRegex",
                "backend",
                "order",
                "blockingMenus",
            ]
            .contains(&field.as_str()))
        || (!registered
            && [
                "description",
                "remove",
                "removable",
                "updateCommands",
                "autoUpdate",
                "idleBurst",
                "installCommands",
            ]
            .contains(&field.as_str()));
    if !owned {
        return Err("uneditablePatchKey".into());
    }
    if value.is_null() {
        if ![
            "preflight",
            "preflightTimeoutSeconds",
            "instructionsFilename",
            "configSeed",
            "contextRegex",
            "order",
            "blockingMenus",
        ]
        .contains(&field.as_str())
        {
            return Err("requiredFieldRemoval".into());
        }
        object.remove(field);
    } else {
        if field == "order"
            && value
                .as_u64()
                .is_none_or(|order| order > u64::from(u32::MAX))
        {
            return Err("invalidAgentOrder".into());
        }
        if field == "command" || field == "baseCommand" {
            object.remove("command");
            object.remove("baseCommand");
        }
        object.insert(field.clone(), value.clone());
    }
    Ok(())
}

fn apply_profile_set_edits(
    owner: &mut Map<String, Value>,
    hashes: &BTreeMap<String, String>,
    profile_sets: &[AgentFileProfileEdit],
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for edit in profile_sets {
        if !seen.insert(&edit.base_hash) {
            return Err("duplicateProfileEdit".into());
        }
        if !hashes.values().any(|hash| hash == &edit.base_hash) {
            return Err("unknownBaseHash".into());
        }
        if edit
            .labels
            .keys()
            .any(|letter| !is_valid_profile_letter(letter))
        {
            return Err("invalidProfileLetter".into());
        }
        let previous = owner
            .get("profilesByBaseHash")
            .and_then(|sets| sets.get(&edit.base_hash));
        let mut submitted = Map::new();
        for (letter, raw) in &edit.cells {
            if !is_valid_profile_letter(letter) {
                return Err("invalidProfileLetter".into());
            }
            let old = previous
                .and_then(|set| set.get(letter))
                .and_then(Value::as_object);
            submitted.insert(letter.clone(), submitted_profile_cell(raw, old)?);
        }
        let cells = normalized_profile_cells(Some(&Value::Object(submitted)))?;
        owner
            .get_mut("profilesByBaseHash")
            .and_then(Value::as_object_mut)
            .ok_or("invalidProfileContainer")?
            .insert(edit.base_hash.clone(), cells);
        owner
            .get_mut("profileLabelsByBaseHash")
            .and_then(Value::as_object_mut)
            .ok_or("invalidProfileContainer")?
            .insert(
                edit.base_hash.clone(),
                serde_json::to_value(&edit.labels).map_err(|_| "invalidProfileLabels")?,
            );
    }
    Ok(())
}

fn submitted_profile_cell(raw: &Value, old: Option<&Map<String, Value>>) -> Result<Value, String> {
    let cell: ProfileCellConfig =
        serde_json::from_value(raw.clone()).map_err(|_| "invalidProfileCells")?;
    let mut cell = serde_json::to_value(cell).map_err(|_| "invalidProfileCells")?;
    let known = [
        "enabled",
        "command",
        "additionalArguments",
        "env",
        "notes",
        "preflight",
        "preflightTimeoutSeconds",
        "id_hash",
        "id_hash_debug",
    ];
    for (field, value) in raw.as_object().ok_or("invalidProfileCells")? {
        if !known.contains(&field.as_str()) && old.and_then(|old| old.get(field)) != Some(value) {
            return Err("uneditableProfileKey".into());
        }
    }
    if let Some(old) = old {
        for (field, value) in old {
            if !known.contains(&field.as_str()) {
                cell.as_object_mut()
                    .unwrap()
                    .insert(field.clone(), value.clone());
            }
        }
    }
    Ok(cell)
}

fn apply_profile_slot_edits(
    owner: &mut Map<String, Value>,
    slots: &BTreeMap<String, ProfileSlotConfig>,
) -> Result<(), String> {
    if slots.keys().any(|letter| !is_valid_profile_letter(letter)) {
        return Err("invalidProfileLetter".into());
    }
    let mut replacement = Map::new();
    for (letter, slot) in slots {
        let mut raw = owner
            .get("profileSlots")
            .and_then(|slots| slots.get(letter))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        raw.insert("label".into(), Value::String(slot.label.clone()));
        replacement.insert(letter.clone(), Value::Object(raw));
    }
    owner.insert("profileSlots".into(), Value::Object(replacement));
    Ok(())
}

pub(crate) fn save_agent_file_locked(
    current: &AppSettings,
    guard: &mut SettingsOperationGuard<'_>,
    source: &SourceRef,
    expected: &SourceRevision,
    context_revision: &str,
    edits: &AgentFileEdits,
) -> Result<SelectedAgentFileSnapshot, String> {
    let context = SourceReadContext::from_settings(current, guard.settings_path())
        .map_err(|_| "sourceContextUnavailable")?;
    if context.revision != context_revision {
        return Err("staleSourceContext".into());
    }
    let snapshot = read_agent_file_snapshot(&context, source)?;
    if &snapshot.revision != expected {
        return Err("staleSourceRevision".into());
    }
    if !snapshot.capabilities.editable_save {
        return Err("sourceReadOnlyOrUnavailable".into());
    }
    let registered = source.kind == SourceKind::RegisteredInstance;
    if registered
        && (current.local_overlay_state.owns_top_level("agents")
            || current
                .local_overlay_state
                .owns_top_level("codingAgentProfiles"))
    {
        return Err("overlayOwnedGroups".into());
    }
    let path = context
        .locator(source)
        .map_err(|_| "sourceContextUnavailable")?;
    let bytes = agent_file_read(&path)?;
    if &source_bytes_revision(&bytes) != expected {
        return Err("staleSourceRevision".into());
    }
    let before = crate::config::coding_agents_catalog::parse_strict_json(&bytes)?;
    let candidate = prepare_agent_file_edits(&before, registered, edits)?;
    if serde_json::to_vec_pretty(&candidate)
        .map_err(|_| "invalidSource")?
        .len()
        > SETTINGS_CAPTURE_LIMIT
    {
        return Err("sourceTooLarge".into());
    }
    if registered {
        guard.verify_agent_file_context(context_revision)?;
        if &source_bytes_revision(&agent_file_read(&path)?) != expected {
            return Err("staleSourceRevision".into());
        }
        guard.publish(SettingsPublisher::AgentsSave, |_| {
            write_value_atomic_labeled(&candidate, &path, &path)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })?;
    } else {
        return Err("catalogRequiresScopedOperation".into());
    }
    let snapshot = read_agent_file_snapshot(&context, source)?;
    if snapshot.availability != SourceAvailability::Available {
        return Err("sourcePostPublicationUnavailable".into());
    }
    Ok(snapshot)
}

pub(crate) fn remember_agent_file_locked(
    current: &AppSettings,
    guard: &mut SettingsOperationGuard<'_>,
    menu: AgentFileSourceMenu,
    source: &SourceRef,
) -> Result<AgentFileSourcePreferences, String> {
    let context = SourceReadContext::from_settings(current, guard.settings_path())
        .map_err(|_| "sourceContextUnavailable")?;
    if !list_active_sources(&context).contains(source) {
        return Err("sourceAbsentOrForged".into());
    }
    let receipt = capture_registered_profile_gate(guard.settings_path())?;
    let mut raw = captured_registered_profile_object(&receipt, guard.settings_path())?
        .ok_or("sourceContextUnavailable")?;
    let bytes = receipt
        .iter()
        .find(|(path, _)| path == guard.settings_path())
        .and_then(|(_, bytes)| bytes.as_ref())
        .ok_or("sourceContextUnavailable")?;
    if agent_file_read(guard.settings_path())? != *bytes {
        return Err("staleSettingsRevision".into());
    }
    let mut preferences: AgentFileSourcePreferences = raw
        .get("agentFileSourceByMenu")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| "invalidSourcePreferences")?
        .unwrap_or_default();
    match menu {
        AgentFileSourceMenu::Settings => preferences.settings = Some(source.clone()),
        AgentFileSourceMenu::Picker => preferences.picker = Some(source.clone()),
    }
    raw.insert(
        "agentFileSourceByMenu".into(),
        serde_json::to_value(&preferences).map_err(|_| "invalidSourcePreferences")?,
    );
    let raw = Value::Object(raw);
    if serde_json::to_vec_pretty(&raw)
        .map_err(|_| "invalidSourcePreferences")?
        .len()
        > SETTINGS_CAPTURE_LIMIT
    {
        return Err("sourceTooLarge".into());
    }
    guard.verify_agent_file_context(&context.revision)?;
    if !list_active_sources(&context).contains(source) {
        return Err("sourceAbsentOrForged".into());
    }
    recheck_registered_profile_gate(&receipt)?;
    if agent_file_read(guard.settings_path())? != *bytes {
        return Err("staleSettingsRevision".into());
    }
    guard.publish(SettingsPublisher::SettingsSave, |path| {
        write_value_atomic_labeled(&raw, path, path)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })?;
    agent_file_checkpoint("preference_published", guard.settings_path());
    Ok(preferences)
}

/// #4845 P02 decision 1: publish the config-file selection under the held settings lock.
/// The raw disk object is the base, as in `remember_agent_file_locked`. Bare error codes.
pub(crate) fn publish_config_selection_locked(
    guard: &mut SettingsOperationGuard<'_>,
    selection: &CodingAgentConfigFiles,
) -> Result<(), String> {
    let receipt = capture_registered_profile_gate(guard.settings_path())?;
    let mut raw = captured_registered_profile_object(&receipt, guard.settings_path())?
        .ok_or("sourceContextUnavailable")?;
    let bytes = receipt
        .iter()
        .find(|(path, _)| path == guard.settings_path())
        .and_then(|(_, bytes)| bytes.as_ref())
        .ok_or("sourceContextUnavailable")?;
    if agent_file_read(guard.settings_path())? != *bytes {
        return Err("staleSettingsRevision".into());
    }
    raw.insert(
        "codingAgentConfigFiles".into(),
        serde_json::to_value(selection).map_err(|_| "configSelectionInvalid")?,
    );
    let raw = Value::Object(raw);
    if serde_json::to_vec_pretty(&raw)
        .map_err(|_| "configSelectionInvalid")?
        .len()
        > SETTINGS_CAPTURE_LIMIT
    {
        return Err("sourceTooLarge".into());
    }
    recheck_registered_profile_gate(&receipt)?;
    if agent_file_read(guard.settings_path())? != *bytes {
        return Err("staleSettingsRevision".into());
    }
    guard.publish(SettingsPublisher::SettingsSave, |path| {
        write_value_atomic_labeled(&raw, path, path)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

pub(crate) async fn persist_agent_file_edits(
    owner: &SettingsState,
    source: SourceRef,
    expected: SourceRevision,
    context_revision: String,
    edits: AgentFileEdits,
) -> Result<AgentFileSaveReply, String> {
    let owner = Arc::clone(owner);
    tokio::task::spawn_blocking(move || {
        if source.kind == SourceKind::CatalogPersonal && source.scope == SourceScope::Project {
            return owner.save_catalog_source_blocking(source, expected, context_revision, edits);
        }
        let mut selected_context = None;
        let report = owner
            .with_operation_blocking("save_agent_file_source", |current, guard| {
                selected_context =
                    SourceReadContext::from_settings(current, guard.settings_path()).ok();
                save_agent_file_locked(
                    current,
                    guard,
                    &source,
                    &expected,
                    &context_revision,
                    &edits,
                )
            })
            .map_err(|_| "settingsOwnerUnavailable")?;
        let snapshot = match report.value {
            Some(snapshot) => Some(snapshot),
            None => selected_context
                .as_ref()
                .and_then(|context| read_agent_file_snapshot(context, &source).ok()),
        };
        let outcome = match report.outcome {
            OperationOutcome::Committed => AgentFileSaveOutcome::Committed,
            OperationOutcome::Rejected => AgentFileSaveOutcome::Rejected,
            OperationOutcome::PartialOrUnknown => AgentFileSaveOutcome::PartialOrUnknown,
        };
        let diagnostics = report
            .error
            .map(|reason| {
                let code = [
                    "staleSourceRevision",
                    "staleSourceContext",
                    "sourceProfileConflict",
                    "overlayOwnedGroups",
                    "catalogLockTimeout",
                    "sourceReadOnlyOrUnavailable",
                    "uneditablePatchKey",
                    "unknownBaseHash",
                ]
                .into_iter()
                .find(|code| reason.contains(code))
                .unwrap_or("sourceSaveFailed");
                let mut diagnostic = source_diagnostic(code);
                if code == "sourceProfileConflict" {
                    diagnostic.entry_hint = Some(reason);
                }
                vec![diagnostic]
            })
            .unwrap_or_default();
        Ok(AgentFileSaveReply {
            outcome,
            snapshot,
            effective_settings: owner.blocking_current_snapshot().ok(),
            diagnostics,
        })
    })
    .await
    .map_err(|_| "sourceWorkerFailed")?
}

// #4845 P17: save, create, register and activate coding-agent config files.

fn config_save_failed(error: impl std::fmt::Display) -> ConfigFileError {
    ConfigFileError::new("configFileSaveFailed", error.to_string())
}

/// Decision 4 encode + write: `before` keeps every other top-level key.
fn write_config_groups(
    settings: &AppSettings,
    before: &Value,
    path: &std::path::Path,
    mut agents: Vec<AgentConfig>,
    profiles: CodingAgentProfilesConfig,
) -> Result<(), ConfigFileError> {
    refresh_agent_id_hashes(&mut agents);
    validate_agent_commands(&AppSettings {
        agents: agents.clone(),
        ..settings.clone()
    })
    .map_err(|reason| ConfigFileError::new("configFileInvalidAgents", reason))?;
    let incoming = serde_json::json!({"agents": agents, "codingAgentProfiles": profiles});
    let mut groups =
        canonical_profile_source(before, &incoming, true).map_err(config_save_failed)?;
    if let (Some(slots), Some(raw_slots)) = (
        groups
            .pointer_mut("/codingAgentProfiles/profileSlots")
            .and_then(Value::as_object_mut),
        before
            .pointer("/codingAgentProfiles/profileSlots")
            .and_then(Value::as_object),
    ) {
        for (letter, slot) in slots {
            let raw = raw_slots.get(letter).and_then(Value::as_object);
            if let (Some(slot), Some(raw)) = (slot.as_object_mut(), raw) {
                for (key, value) in raw.iter().filter(|(key, _)| *key != "label") {
                    slot.insert(key.clone(), value.clone());
                }
            }
        }
    }
    let mut output = before.as_object().cloned().unwrap_or_default();
    for key in AGENTS_INSTANCE_KEYS {
        if let Some(value) = groups.get_mut(key).map(Value::take) {
            output.insert(key.to_owned(), value);
        }
    }
    write_agents_instance_file(&output, path).map_err(config_save_failed)
}

/// Decision 4: save `named`/`loaded` files only, under the read revision.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn save_config_file(
    settings: &AppSettings,
    settings_path: &std::path::Path,
    ctx: &ConfigContext,
    loc: &ConfigFileLocator,
    expected: &SourceRevision,
    agents: Vec<AgentConfig>,
    profiles: CodingAgentProfilesConfig,
) -> Result<ConfigFileSnapshot, ConfigFileError> {
    let dirs = config_dirs(settings, settings_path, ctx);
    let path = resolve_locator(&config_selection(settings), &dirs, ctx, loc)?;
    match loc {
        ConfigFileLocator::RegisteredInstance {} => {
            return Err(ConfigFileError::new(
                "configFileUseSettings",
                "Save this file from the settings.",
            ))
        }
        ConfigFileLocator::Layer { .. } | ConfigFileLocator::Other { .. } => {
            return Err(ConfigFileError::new(
                "configFileReadOnly",
                "This file is read-only.",
            ))
        }
        ConfigFileLocator::Named { .. } | ConfigFileLocator::Loaded { .. } => {}
    }
    let mut current = config_snapshot(loc, path.clone());
    let before = read_registered_shape(&mut current).ok_or_else(config_file_unavailable)?;
    if &current.revision != expected {
        return Err(ConfigFileError::new(
            "staleConfigFile",
            "This file changed on disk. Reload it first.",
        ));
    }
    write_config_groups(settings, &before, &path, agents, profiles)?;
    Ok(read_resolved(settings, settings_path, ctx, loc, path))
}

/// Decision 5: create a named file in the instance or project dir.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn create_config_file(
    settings: &AppSettings,
    settings_path: &std::path::Path,
    ctx: &ConfigContext,
    scope: ConfigScope,
    name: &str,
    agents: Vec<AgentConfig>,
    profiles: CodingAgentProfilesConfig,
) -> Result<ConfigFileSnapshot, ConfigFileError> {
    let name = normalize_config_name(name)?;
    if scope == ConfigScope::Project && !matches!(ctx, ConfigContext::Project(_)) {
        return Err(ConfigFileError::new(
            "configFileWrongProject",
            "This file belongs to another project.",
        ));
    }
    let loc = ConfigFileLocator::Named { scope, name };
    let dirs = config_dirs(settings, settings_path, ctx);
    let path = resolve_locator(&config_selection(settings), &dirs, ctx, &loc)?;
    let dir = path.parent().unwrap_or(std::path::Path::new(""));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let exists = settings_sources::list_file_names(dir)
        .map_err(config_save_failed)?
        .iter()
        .any(|known| known.to_lowercase() == file_name);
    if exists {
        return Err(ConfigFileError::new(
            "configNameExists",
            "A file with this name already exists.",
        ));
    }
    settings_sources::create_dir_all(dir).map_err(config_save_failed)?;
    write_config_groups(settings, &serde_json::json!({}), &path, agents, profiles)?;
    Ok(read_resolved(settings, settings_path, ctx, &loc, path))
}

/// Decision 6: the selection with a valid registered-shape file appended once.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn register_loaded(
    settings: &AppSettings,
    path: &str,
) -> Result<CodingAgentConfigFiles, ConfigFileError> {
    let invalid = || ConfigFileError::new("configFileInvalid", "This is not a valid agents file.");
    let canonical =
        settings_sources::canonical(std::path::Path::new(path)).map_err(|_| invalid())?;
    if !settings_sources::is_regular_file(&canonical) {
        return Err(invalid());
    }
    let canonical = canonical.to_string_lossy().into_owned();
    let loc = ConfigFileLocator::Loaded {
        path: canonical.clone(),
    };
    let mut snapshot = config_snapshot(&loc, PathBuf::from(&canonical));
    read_registered_shape(&mut snapshot).ok_or_else(invalid)?;
    Ok(with_loaded(
        settings.coding_agent_config_files.as_ref(),
        &canonical,
    ))
}

/// Decision 7: the selection with `loc` active for `ctx`, if `loc` is `Available`.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn set_active_config_file(
    settings: &AppSettings,
    settings_path: &std::path::Path,
    ctx: &ConfigContext,
    loc: &ConfigFileLocator,
) -> Result<CodingAgentConfigFiles, ConfigFileError> {
    let snapshot = read_config_file(settings, settings_path, ctx, loc)?;
    if snapshot.availability != SourceAvailability::Available {
        return Err(config_file_unavailable());
    }
    Ok(with_active(
        settings.coding_agent_config_files.as_ref(),
        ctx,
        loc.clone(),
    ))
}
