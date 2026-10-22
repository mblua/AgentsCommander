use super::super::super::{
    list_active_sources, source_bytes_revision, source_diagnostic, write_value_atomic_labeled,
    AppSettings, OperationOutcome, SettingsOperationGuard, SettingsPublisher, SettingsState,
    SourceAvailability, SourceKind, SourceReadContext, SourceRef, SourceRevision, SourceScope,
    SETTINGS_CAPTURE_LIMIT,
};
use super::super::{
    canonical_profile_source, capture_registered_profile_gate, captured_registered_profile_object,
    is_valid_profile_letter, normalized_profile_cells, physical_profile_rows,
    project_physical_profiles, recheck_registered_profile_gate, validate_agent_commands,
    ProfileCellConfig,
};
use super::{
    agent_file_checkpoint, agent_file_decode, agent_file_read, read_agent_file_snapshot,
    AgentFileEdits, AgentFileRowAction, AgentFileSaveOutcome, AgentFileSaveReply,
    AgentFileSourceMenu, AgentFileSourcePreferences, SelectedAgentFileSnapshot,
};
use serde_json::{Map, Value};
use std::collections::HashSet;
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
    let mut seen = HashSet::new();
    for edit in &edits.rows {
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
        let mut row = index.map(|i| rows[i].clone()).unwrap_or_else(|| {
            let mut row = Map::new();
            row.insert(key.into(), Value::String(edit.entry_hint.clone()));
            Value::Object(row)
        });
        let object = row.as_object_mut().ok_or("invalidAgentRow")?;
        for (field, value) in &edit.patch {
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
    let mut seen = HashSet::new();
    for edit in &edits.profile_sets {
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
            let old = previous
                .and_then(|set| set.get(letter))
                .and_then(Value::as_object);
            for (field, value) in raw.as_object().ok_or("invalidProfileCells")? {
                if !known.contains(&field.as_str())
                    && old.and_then(|old| old.get(field)) != Some(value)
                {
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
            submitted.insert(letter.clone(), cell);
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
    if let Some(slots) = &edits.profile_slots {
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
    }
    let (agents, profiles) = agent_file_decode(&canonical, registered)?;
    validate_agent_commands(&AppSettings {
        agents,
        coding_agent_profiles: profiles,
        ..AppSettings::default()
    })?;
    Ok(canonical)
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
