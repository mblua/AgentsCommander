mod authority;
mod edit;

#[cfg(test)]
use super::super::list_active_sources;
use super::super::{
    decode_registered_raw, source_bytes_revision, source_diagnostic, AppSettings,
    CodingAgentProfilesConfig, SourceAvailability, SourceCapabilities, SourceDiagnostic,
    SourceKind, SourceReadContext, SourceRef, SourceRevision, SourceScope, SourceSelectionTicket,
    SETTINGS_CAPTURE_LIMIT,
};
use super::{
    collect_registered_profile_gate, decode_physical_catalog, duplicate_profile_document,
    is_valid_profile_letter, physical_profile_sets, profile_container, validate_agent_commands,
    AgentBackendConfig, AgentConfig, ProfileSlotConfig, RegisteredProfileCapture,
};
#[cfg(test)]
pub(crate) use authority::AGENT_FILE_CHECKPOINT;
pub(crate) use authority::{
    agent_file_checkpoint, observe_agent_file, AgentFileReadReply, AgentFileReadRequest,
    BoundedSourceObservation, CatalogSourceOperationGuard,
};
#[cfg(test)]
pub(crate) use edit::save_agent_file_locked;
pub(crate) use edit::{
    persist_agent_file_edits, prepare_agent_file_edits, remember_agent_file_locked,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

// E2432 P02: finite physical-file APIs. These do not activate the broad source protocol.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileSourcePreferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<SourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picker: Option<SourceRef>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentFileSourceMenu {
    Settings,
    Picker,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentFileSourceFormat {
    Registered,
    CatalogSchema1,
    CatalogSchema2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentFileRowKind {
    Registered,
    CatalogComplete,
    CatalogPatch,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileRow {
    pub entry_hint: String,
    pub kind: AgentFileRowKind,
    pub raw: Value,
    pub base_hash: Option<String>,
    pub launchable: bool,
    pub editable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedAgentFileSnapshot {
    pub source: SourceRef,
    pub revision: SourceRevision,
    pub context_revision: String,
    pub format: AgentFileSourceFormat,
    pub availability: SourceAvailability,
    pub capabilities: SourceCapabilities,
    pub rows: Vec<AgentFileRow>,
    // Raw cells retain extensions that typed runtime consumers do not understand.
    pub profiles_by_base_hash: BTreeMap<String, Value>,
    pub profile_labels_by_base_hash: BTreeMap<String, Value>,
    pub profile_slots: BTreeMap<String, ProfileSlotConfig>,
    pub diagnostics: Vec<SourceDiagnostic>,
    pub tickets: BTreeMap<String, BTreeMap<String, SourceSelectionTicket>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileSourceOption {
    pub source: SourceRef,
    pub revision: SourceRevision,
    pub context_revision: String,
    pub format: AgentFileSourceFormat,
    pub availability: SourceAvailability,
    pub capabilities: SourceCapabilities,
    pub diagnostics: Vec<SourceDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileSources {
    pub sources: Vec<AgentFileSourceOption>,
    pub last_selected: AgentFileSourcePreferences,
    pub active_source_by_menu: AgentFileSourcePreferences,
    pub menu_diagnostics: BTreeMap<String, Vec<SourceDiagnostic>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileRowEdit {
    pub entry_hint: String,
    pub action: AgentFileRowAction,
    pub patch: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentFileRowAction {
    Upsert,
    Delete,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileProfileEdit {
    pub base_hash: String,
    pub cells: BTreeMap<String, Value>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileEdits {
    pub rows: Vec<AgentFileRowEdit>,
    pub profile_sets: Vec<AgentFileProfileEdit>,
    pub profile_slots: Option<BTreeMap<String, ProfileSlotConfig>>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentFileSaveOutcome {
    Committed,
    Rejected,
    PartialOrUnknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileSaveReply {
    pub outcome: AgentFileSaveOutcome,
    pub snapshot: Option<SelectedAgentFileSnapshot>,
    pub effective_settings: Option<AppSettings>,
    pub diagnostics: Vec<SourceDiagnostic>,
}

pub(crate) fn agent_file_read(path: &Path) -> Result<Vec<u8>, String> {
    crate::path_identity::read_bounded_regular(path, SETTINGS_CAPTURE_LIMIT).map(|(bytes, _)| bytes)
}

pub(crate) fn agent_file_decode(
    value: &Value,
    registered: bool,
) -> Result<(Vec<AgentConfig>, CodingAgentProfilesConfig), String> {
    if registered {
        decode_registered_raw(value).map_err(|_| "invalidRegisteredSource".to_owned())
    } else {
        let (agents, profiles) = decode_physical_catalog(value)?;
        let agents = agents
            .into_iter()
            .map(|agent| AgentConfig {
                id: agent.id,
                id_hash: agent.id_hash,
                id_hash_debug: agent.id_hash_debug,
                label: agent.label,
                command: agent.command,
                color: agent.color,
                envs: agent.envs,
                isolated_home: agent.isolated_home,
                instructions_filename: agent.instructions_filename,
                config_seed: agent.config_seed,
                preflight: None,
                preflight_timeout_seconds: None,
                context_regex: None,
                blocking_menus: None,
                order: None,
                backend: AgentBackendConfig::default(),
            })
            .collect();
        Ok((agents, profiles))
    }
}

pub(crate) fn agent_file_projection(
    snapshot: &SelectedAgentFileSnapshot,
) -> Result<AppSettings, String> {
    if snapshot.availability != SourceAvailability::Available {
        return Err("sourceUnavailable".into());
    }
    let registered = snapshot.source.kind == SourceKind::RegisteredInstance;
    let mut value = serde_json::json!({
        "agents": snapshot.rows.iter().map(|row| row.raw.clone()).collect::<Vec<_>>()
    });
    let profiles = serde_json::json!({
        "profileBindingVersion": 1,
        "profilesByBaseHash": snapshot.profiles_by_base_hash,
        "profileLabelsByBaseHash": snapshot.profile_labels_by_base_hash,
        "profileSlots": snapshot.profile_slots
    });
    if registered {
        value["codingAgentProfiles"] = profiles;
    } else {
        value["schemaVersion"] = serde_json::json!(match snapshot.format {
            AgentFileSourceFormat::CatalogSchema2 => 2,
            _ => 1,
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(profiles.as_object().unwrap().clone());
    }
    let (agents, profiles) = agent_file_decode(&value, registered)?;
    Ok(AppSettings {
        agents,
        coding_agent_profiles: profiles,
        ..AppSettings::default()
    })
}

pub(crate) fn read_agent_file_snapshot(
    context: &SourceReadContext,
    source: &SourceRef,
) -> Result<SelectedAgentFileSnapshot, String> {
    let path = context
        .locator(source)
        .map_err(|_| "sourceContextUnavailable")?;
    let observation = observe_agent_file(&path);
    let validation = (source.kind == SourceKind::RegisteredInstance)
        .then(|| collect_registered_profile_gate(&context.settings_path));
    snapshot_from_captured_source(context, source, &observation, validation.as_ref())
}

pub(crate) fn snapshot_from_captured_source(
    context: &SourceReadContext,
    source: &SourceRef,
    observation: &BoundedSourceObservation,
    registered_validation: Option<&RegisteredProfileCapture>,
) -> Result<SelectedAgentFileSnapshot, String> {
    let path = context
        .locator(source)
        .map_err(|_| "sourceContextUnavailable")?;
    let registered = source.kind == SourceKind::RegisteredInstance;
    let mut snapshot = SelectedAgentFileSnapshot {
        source: source.clone(),
        revision: SourceRevision::Absent {},
        context_revision: context.revision.clone(),
        format: if registered {
            AgentFileSourceFormat::Registered
        } else {
            AgentFileSourceFormat::CatalogSchema1
        },
        availability: SourceAvailability::Absent,
        capabilities: SourceCapabilities::default(),
        rows: Vec::new(),
        profiles_by_base_hash: BTreeMap::new(),
        profile_labels_by_base_hash: BTreeMap::new(),
        profile_slots: BTreeMap::new(),
        diagnostics: Vec::new(),
        tickets: BTreeMap::new(),
    };
    if matches!(observation, BoundedSourceObservation::Absent) {
        snapshot.diagnostics.push(source_diagnostic("sourceAbsent"));
        return Ok(snapshot);
    }
    let decoded = (|| -> Result<(), String> {
        let bytes = observation.bytes().ok_or("sourceReadFailed")?;
        snapshot.revision = source_bytes_revision(bytes);
        if let Some(reason) = duplicate_profile_document(bytes, &path) {
            return Err(reason);
        }
        let value = crate::config::coding_agents_catalog::parse_strict_json(bytes)?;
        let sets = physical_profile_sets(&value, registered)?;
        let (agents, profiles) = agent_file_decode(&value, registered)?;
        let projection = AppSettings {
            agents,
            coding_agent_profiles: profiles,
            ..AppSettings::default()
        };
        validate_agent_commands(&projection)?;
        if registered {
            if let Some(reason) =
                registered_validation.and_then(RegisteredProfileCapture::ordinary_error)
            {
                return Err(reason);
            }
        }
        let owner = profile_container(&value, registered)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        snapshot.profile_slots = owner
            .get("profileSlots")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "invalidProfileSlots")?
            .unwrap_or_default();
        if snapshot
            .profile_slots
            .keys()
            .any(|letter| !is_valid_profile_letter(letter))
        {
            return Err("invalidProfileLetter".into());
        }
        if !registered && value.get("schemaVersion").and_then(Value::as_u64) == Some(2) {
            snapshot.format = AgentFileSourceFormat::CatalogSchema2;
        }
        let editable = (registered
            || (source.scope == SourceScope::Project
                && source.kind == SourceKind::CatalogPersonal))
            && value.get("managed").is_none();
        snapshot.capabilities = SourceCapabilities {
            available_read: true,
            editable_save: editable,
            ..SourceCapabilities::default()
        };
        for raw in value
            .get("agents")
            .and_then(Value::as_array)
            .ok_or("invalidAgentsArray")?
        {
            let hint = raw
                .get(if registered { "id" } else { "key" })
                .and_then(Value::as_str)
                .ok_or("invalidAgentKey")?
                .to_owned();
            let hash = sets.rows.get(&hint).cloned();
            let remove = raw.get("remove") == Some(&Value::Bool(true));
            let kind = if registered {
                AgentFileRowKind::Registered
            } else if remove {
                AgentFileRowKind::Remove
            } else if matches!(snapshot.format, AgentFileSourceFormat::CatalogSchema2) {
                AgentFileRowKind::CatalogComplete
            } else {
                AgentFileRowKind::CatalogPatch
            };
            if hash.is_none() {
                let mut diagnostic = source_diagnostic(if remove {
                    "removedRow"
                } else {
                    "incompleteRow"
                });
                diagnostic.entry_hint = Some(hint.clone());
                snapshot.diagnostics.push(diagnostic);
            }
            snapshot.rows.push(AgentFileRow {
                entry_hint: hint,
                kind,
                raw: raw.clone(),
                launchable: hash.is_some(),
                base_hash: hash,
                editable,
            });
        }
        snapshot.profiles_by_base_hash = sets.cells.into_iter().collect();
        snapshot.profile_labels_by_base_hash = sets.labels.into_iter().collect();
        snapshot.availability = SourceAvailability::Available;
        Ok(())
    })();
    if let Err(reason) = decoded {
        snapshot.availability = SourceAvailability::Invalid;
        snapshot.capabilities = SourceCapabilities::default();
        snapshot.rows.clear();
        snapshot.profiles_by_base_hash.clear();
        snapshot.profile_labels_by_base_hash.clear();
        snapshot.profile_slots.clear();
        let code = [
            "sourceProfileConflict",
            "invalidLegacyProfileOrphans",
            "duplicateJsonMember",
        ]
        .into_iter()
        .find(|code| reason.contains(code))
        .unwrap_or("invalidSource");
        let mut diagnostic = source_diagnostic(code);
        if code == "sourceProfileConflict" {
            diagnostic.entry_hint = Some(reason);
        }
        snapshot.diagnostics.push(diagnostic);
    }
    Ok(snapshot)
}

#[cfg(test)]
pub(crate) fn list_agent_files(
    current: &AppSettings,
    path: &Path,
) -> Result<AgentFileSources, String> {
    let context =
        SourceReadContext::from_settings(current, path).map_err(|_| "sourceContextUnavailable")?;
    let sources = list_active_sources(&context)
        .iter()
        .map(|source| {
            let snapshot = read_agent_file_snapshot(&context, source)?;
            Ok(AgentFileSourceOption {
                source: snapshot.source,
                revision: snapshot.revision,
                context_revision: snapshot.context_revision,
                format: snapshot.format,
                availability: snapshot.availability,
                capabilities: snapshot.capabilities,
                diagnostics: snapshot.diagnostics,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    agent_file_sources_reply(
        sources,
        current
            .agent_file_source_by_menu
            .clone()
            .unwrap_or_default(),
    )
}

pub(crate) fn agent_file_sources_reply(
    mut sources: Vec<AgentFileSourceOption>,
    last_selected: AgentFileSourcePreferences,
) -> Result<AgentFileSources, String> {
    sources.sort_by_key(|option| {
        (
            match option.source.kind {
                SourceKind::RegisteredInstance => 0,
                SourceKind::CatalogBase => 1,
                SourceKind::CatalogProject => 2,
                SourceKind::CatalogPersonal => 3,
            },
            option.source.context_root.clone(),
        )
    });
    let mut active = AgentFileSourcePreferences::default();
    let mut diagnostics = BTreeMap::new();
    for (menu, remembered, selected) in [
        ("settings", &last_selected.settings, &mut active.settings),
        ("picker", &last_selected.picker, &mut active.picker),
    ] {
        if let Some(remembered) = remembered {
            if let Some(snapshot) = sources
                .iter()
                .find(|snapshot| &snapshot.source == remembered)
            {
                *selected = Some(remembered.clone());
                if snapshot.availability != SourceAvailability::Available {
                    diagnostics.insert(menu.to_owned(), snapshot.diagnostics.clone());
                }
            } else {
                diagnostics.insert(
                    menu.to_owned(),
                    vec![source_diagnostic("rememberedSourceUnavailable")],
                );
            }
        } else {
            *selected = sources
                .iter()
                .find(|s| {
                    s.source.kind == SourceKind::RegisteredInstance
                        && s.availability == SourceAvailability::Available
                })
                .or_else(|| {
                    sources
                        .iter()
                        .find(|s| s.availability == SourceAvailability::Available)
                })
                .map(|s| s.source.clone());
        }
    }
    Ok(AgentFileSources {
        sources,
        last_selected,
        active_source_by_menu: active,
        menu_diagnostics: diagnostics,
    })
}

// Only the ordinary optional field is lenient; captured raw authority stays strict.
pub(crate) fn deserialize_agent_file_source_preferences<'de, D>(
    deserializer: D,
) -> Result<Option<AgentFileSourcePreferences>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    Ok(raw.and_then(|value| serde_json::from_value(value).ok()))
}
