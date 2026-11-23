//! Snapshot model of one physical agent file (registered instance or catalog) for the UI.
//! Owns the wire types such as `SelectedAgentFileSnapshot`, `AgentFileEdits` and
//! `AgentFileSaveReply`, the decode of a captured file into rows and profile sets, and the
//! per-menu source choice in `agent_file_sources_reply`.
//! Owner-locked reads and catalog saves live in `authority`; edit preparation and the
//! registered save live in `edit`.

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
use super::profile::PhysicalProfileSets;
use super::{
    collect_registered_profile_gate, decode_physical_catalog, duplicate_profile_document,
    is_valid_profile_letter, physical_profile_sets, profile_container, validate_agent_commands,
    AgentBackendConfig, AgentConfig, ProfileSlotConfig, RegisteredProfileCapture,
};
use crate::config::coding_agent_files::inventory::list_entries;
use crate::config::coding_agent_files::locator::resolve_locator;
use crate::config::coding_agent_files::{
    active_locator, CatalogLayer, ConfigContext, ConfigDirs, ConfigFileError, ConfigFileLocator,
    ConfigScope, InventoryEntry,
};
use crate::config::instance_artifacts::CODING_AGENTS_CATALOG_DIR_NAME;
use crate::filesystem::settings_sources;
#[cfg(test)]
pub(crate) use authority::AGENT_FILE_CHECKPOINT;
pub(crate) use authority::{
    agent_file_checkpoint, observe_agent_file, AgentFileReadReply, AgentFileReadRequest,
    BoundedSourceObservation, CatalogSourceOperationGuard,
};
#[cfg(test)]
pub(crate) use edit::save_agent_file_locked;
#[allow(unused_imports, reason = "I4845 staged; removed in P18")]
pub(crate) use edit::{
    create_config_file, register_loaded, save_config_file, set_active_config_file,
};
pub(crate) use edit::{
    persist_agent_file_edits, prepare_agent_file_edits, publish_config_selection_locked,
    remember_agent_file_locked,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    let decoded = decode_agent_file_snapshot(
        &mut snapshot,
        source,
        observation,
        registered_validation,
        &path,
        registered,
    );
    if let Err(reason) = decoded {
        invalidate_agent_file_snapshot(&mut snapshot, reason);
    }
    Ok(snapshot)
}

fn decode_agent_file_snapshot(
    snapshot: &mut SelectedAgentFileSnapshot,
    source: &SourceRef,
    observation: &BoundedSourceObservation,
    registered_validation: Option<&RegisteredProfileCapture>,
    path: &Path,
    registered: bool,
) -> Result<(), String> {
    let bytes = observation.bytes().ok_or("sourceReadFailed")?;
    snapshot.revision = source_bytes_revision(bytes);
    if let Some(reason) = duplicate_profile_document(bytes, path) {
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
    if registered
        && let Some(reason) =
            registered_validation.and_then(RegisteredProfileCapture::ordinary_error)
    {
        return Err(reason);
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
        || (source.scope == SourceScope::Project && source.kind == SourceKind::CatalogPersonal))
        && value.get("managed").is_none();
    snapshot.capabilities = SourceCapabilities {
        available_read: true,
        editable_save: editable,
        ..SourceCapabilities::default()
    };
    push_agent_file_rows(snapshot, &value, &sets, registered, editable)?;
    snapshot.profiles_by_base_hash = sets.cells.into_iter().collect();
    snapshot.profile_labels_by_base_hash = sets.labels.into_iter().collect();
    snapshot.availability = SourceAvailability::Available;
    Ok(())
}

fn push_agent_file_rows(
    snapshot: &mut SelectedAgentFileSnapshot,
    value: &Value,
    sets: &PhysicalProfileSets,
    registered: bool,
    editable: bool,
) -> Result<(), String> {
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
    Ok(())
}

fn invalidate_agent_file_snapshot(snapshot: &mut SelectedAgentFileSnapshot, reason: String) {
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

// #4845 P17: the settings-side glue of the coding-agent config files.

/// One config file read for a locator. `registeredInstance` reads live settings.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
#[derive(Debug, Clone)]
pub(crate) struct ConfigFileSnapshot {
    pub(crate) locator: ConfigFileLocator,
    pub(crate) path: PathBuf,
    pub(crate) revision: SourceRevision,
    pub(crate) availability: SourceAvailability,
    pub(crate) agents: Vec<AgentConfig>,
    pub(crate) coding_agent_profiles: CodingAgentProfilesConfig,
    pub(crate) read_only: bool,
    pub(crate) diagnostic: Option<String>,
}

/// The inventory of a context with availability, read-only flag and agent count.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
#[derive(Debug, Clone)]
pub(crate) struct ConfigFileListing {
    pub(crate) context: ConfigContext,
    pub(crate) entries: Vec<(InventoryEntry, SourceAvailability, bool, Option<usize>)>,
    pub(crate) active: ConfigFileLocator,
    pub(crate) active_notice: Option<String>,
}

const CONFIG_LAYER_KINDS: [(SourceKind, CatalogLayer); 3] = [
    (SourceKind::CatalogBase, CatalogLayer::CatalogBase),
    (SourceKind::CatalogProject, CatalogLayer::CatalogProject),
    (SourceKind::CatalogPersonal, CatalogLayer::CatalogPersonal),
];

fn layer_kind(layer: CatalogLayer) -> SourceKind {
    CONFIG_LAYER_KINDS
        .iter()
        .find(|(_, l)| *l == layer)
        .map(|(kind, _)| *kind)
        .unwrap_or(SourceKind::CatalogBase)
}

fn config_scope_layers(
    scope: ConfigScope,
    ac_dir: &Path,
) -> impl Iterator<Item = (ConfigScope, CatalogLayer, PathBuf)> + '_ {
    CONFIG_LAYER_KINDS
        .iter()
        .filter(move |(kind, _)| {
            scope == ConfigScope::Project || *kind != SourceKind::CatalogProject
        })
        .filter_map(move |(kind, layer)| {
            crate::config::coding_agents_catalog::independent_catalog_path(ac_dir, *kind)
                .map(|path| (scope, *layer, path))
        })
}

/// Decision 1: the dirs and layer files a locator of `ctx` can point at.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn config_dirs(
    _settings: &AppSettings,
    settings_path: &Path,
    ctx: &ConfigContext,
) -> ConfigDirs {
    let settings_dir = settings_path
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let mut layers: Vec<_> = config_scope_layers(ConfigScope::Instance, &settings_dir).collect();
    let project = match ctx {
        ConfigContext::Instance => None,
        ConfigContext::Project(key) => {
            let ac_dir = Path::new(key).join(".ac");
            layers.extend(config_scope_layers(ConfigScope::Project, &ac_dir));
            Some((key.clone(), ac_dir.join(CODING_AGENTS_CATALOG_DIR_NAME)))
        }
    };
    ConfigDirs {
        registered_path: super::agents_instance_path(settings_path),
        instance_dir: settings_dir.join(CODING_AGENTS_CATALOG_DIR_NAME),
        project,
        layers,
    }
}

pub(super) fn config_snapshot(loc: &ConfigFileLocator, path: PathBuf) -> ConfigFileSnapshot {
    ConfigFileSnapshot {
        read_only: match loc {
            ConfigFileLocator::Other { .. } => true,
            ConfigFileLocator::Layer { .. } => path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("agents.10.") || name.starts_with("agents.40.")
                }),
            _ => false,
        },
        locator: loc.clone(),
        path,
        revision: SourceRevision::Absent {},
        availability: SourceAvailability::Absent,
        agents: Vec::new(),
        coding_agent_profiles: CodingAgentProfilesConfig::default(),
        diagnostic: None,
    }
}

fn invalid_config(snapshot: &mut ConfigFileSnapshot, code: impl Into<String>) {
    snapshot.availability = SourceAvailability::Invalid;
    snapshot.diagnostic = Some(code.into());
}

/// Decision 3, registered shape. Returns the file object when it is `Available`.
pub(super) fn read_registered_shape(snapshot: &mut ConfigFileSnapshot) -> Option<Value> {
    let bytes = match settings_sources::bytes(&snapshot.path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            invalid_config(snapshot, "sourceReadFailed");
            return None;
        }
    };
    snapshot.revision = source_bytes_revision(&bytes);
    let value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.get("agents").is_some_and(Value::is_array) => value,
        _ => {
            invalid_config(snapshot, "invalidAgentsArray");
            return None;
        }
    };
    match agent_file_decode(&value, true) {
        Ok((agents, profiles)) => {
            snapshot.agents = agents;
            snapshot.coding_agent_profiles = profiles;
            snapshot.availability = SourceAvailability::Available;
            Some(value)
        }
        Err(code) => {
            invalid_config(snapshot, code);
            None
        }
    }
}

fn read_layer(
    snapshot: &mut ConfigFileSnapshot,
    settings_path: &Path,
    scope: ConfigScope,
    layer: CatalogLayer,
    ctx: &ConfigContext,
) {
    let primary = match (scope, ctx) {
        (ConfigScope::Project, ConfigContext::Project(key)) => Some(PathBuf::from(key)),
        _ => None,
    };
    let read = SourceReadContext::from_primary(settings_path, primary)
        .map_err(|_| "sourceContextUnavailable".to_owned())
        .and_then(|context| {
            let source = context.source(layer_kind(layer));
            read_agent_file_snapshot(&context, &source)
        });
    let selected = match read {
        Ok(selected) => selected,
        Err(code) => return invalid_config(snapshot, code),
    };
    if selected.availability == SourceAvailability::Absent {
        return;
    }
    snapshot.revision = selected.revision.clone();
    match agent_file_projection(&selected) {
        Ok(projection) => {
            snapshot.agents = projection.agents;
            snapshot.coding_agent_profiles = projection.coding_agent_profiles;
            snapshot.availability = SourceAvailability::Available;
        }
        Err(_) => {
            let code = selected.diagnostics.first().map(|d| d.code.clone());
            invalid_config(snapshot, code.unwrap_or_else(|| "invalidSource".into()));
        }
    }
}

pub(super) fn read_resolved(
    settings: &AppSettings,
    settings_path: &Path,
    ctx: &ConfigContext,
    loc: &ConfigFileLocator,
    path: PathBuf,
) -> ConfigFileSnapshot {
    let mut snapshot = config_snapshot(loc, path);
    match loc {
        ConfigFileLocator::RegisteredInstance {} => {
            snapshot.revision = match settings_sources::bytes(&snapshot.path) {
                Ok(bytes) => source_bytes_revision(&bytes),
                Err(_) => SourceRevision::Absent {},
            };
            snapshot.agents = settings.agents.clone();
            snapshot.coding_agent_profiles = settings.coding_agent_profiles.clone();
            snapshot.availability = SourceAvailability::Available;
        }
        ConfigFileLocator::Layer { scope, layer } => {
            read_layer(&mut snapshot, settings_path, *scope, *layer, ctx);
        }
        _ => {
            read_registered_shape(&mut snapshot);
        }
    }
    snapshot
}

pub(super) fn config_selection(
    settings: &AppSettings,
) -> crate::config::coding_agent_files::CodingAgentConfigFiles {
    settings
        .coding_agent_config_files
        .clone()
        .unwrap_or_default()
}

/// Decision 3: read one file of `ctx`. Never creates files.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn read_config_file(
    settings: &AppSettings,
    settings_path: &Path,
    ctx: &ConfigContext,
    loc: &ConfigFileLocator,
) -> Result<ConfigFileSnapshot, ConfigFileError> {
    let dirs = config_dirs(settings, settings_path, ctx);
    let path = resolve_locator(&config_selection(settings), &dirs, ctx, loc)?;
    Ok(read_resolved(settings, settings_path, ctx, loc, path))
}

/// Decision 9: every file of `ctx`, with the active one and its notice.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn list_config_files(
    settings: &AppSettings,
    settings_path: &Path,
    ctx: &ConfigContext,
) -> ConfigFileListing {
    let sel = config_selection(settings);
    let dirs = config_dirs(settings, settings_path, ctx);
    let active = active_locator(settings.coding_agent_config_files.as_ref(), ctx);
    let mut active_available = false;
    let entries: Vec<_> = list_entries(&sel, &dirs, ctx)
        .into_iter()
        .map(|entry| {
            let snapshot = read_resolved(
                settings,
                settings_path,
                ctx,
                &entry.locator,
                entry.path.clone(),
            );
            let available = snapshot.availability == SourceAvailability::Available;
            active_available |= available && entry.locator == active;
            let count = available.then_some(snapshot.agents.len());
            (entry, snapshot.availability, snapshot.read_only, count)
        })
        .collect();
    ConfigFileListing {
        context: ctx.clone(),
        entries,
        active_notice: (!active_available).then(|| "activeConfigFileUnavailable".to_owned()),
        active,
    }
}

pub(super) fn config_file_unavailable() -> ConfigFileError {
    ConfigFileError::new(
        "configFileUnavailable",
        "This file is missing or not valid.",
    )
}

/// Decision 8: the agents and profiles of an `Available` file.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn agents_of(
    settings: &AppSettings,
    settings_path: &Path,
    ctx: &ConfigContext,
    loc: &ConfigFileLocator,
) -> Result<(Vec<AgentConfig>, CodingAgentProfilesConfig), ConfigFileError> {
    let snapshot = read_config_file(settings, settings_path, ctx, loc)?;
    if snapshot.availability != SourceAvailability::Available {
        return Err(config_file_unavailable());
    }
    Ok((snapshot.agents, snapshot.coding_agent_profiles))
}

/// Decision 8: a clone of `settings` whose agents come from `loc`.
#[allow(dead_code, reason = "I4845 staged; removed in P18")]
pub(crate) fn scoped_agent_settings(
    settings: &AppSettings,
    settings_path: &Path,
    ctx: &ConfigContext,
    loc: &ConfigFileLocator,
) -> Result<AppSettings, ConfigFileError> {
    let (agents, coding_agent_profiles) = agents_of(settings, settings_path, ctx, loc)?;
    if matches!(loc, ConfigFileLocator::RegisteredInstance {}) {
        return Ok(settings.clone());
    }
    Ok(AppSettings {
        agents,
        coding_agent_profiles,
        ..settings.clone()
    })
}
