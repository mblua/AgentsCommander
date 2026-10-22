use super::super::projects;
use super::default_true;
#[cfg(test)]
use super::project_atomic_checkpoint;
#[cfg(windows)]
use super::replace_settings_file_atomic;
use super::{
    apply_project_decode_to_value, capture_source_file, config_seed_dest_os_token_stem,
    configuration_identity, configuration_identity_debug, decode_project_state,
    decode_registered_raw, default_profiles_schema_version, io_error_kind_as_str,
    list_active_sources, production_instance_base, project_paths_path, raw_entries,
    raw_project_source_from_bytes, read_disk_object_for_write_typed, refresh_agent_id_hashes,
    refresh_pair_id_hashes, refresh_physical_pair_id_hashes, select_project_source_from_raw,
    serialize_settings_component, settings_path, source_bytes_revision, source_diagnostic,
    validate_config_seed_dest, validate_physical_identity_delta, write_value_atomic_labeled,
    AgentsInstanceRead, AppSettings, CapturedSource, CodingAgentProfilesConfig, ConfigSeedConfig,
    OperationEffect, OperationOutcome, ProjectWriteMode, SettingsFileLock, SettingsOperationGuard,
    SettingsOwner, SettingsOwnerError, SettingsPublisher, SettingsSaveError,
    SettingsSaveLegacyOutward, SettingsSaveReason, SettingsSaveStage, SettingsState,
    SourceAvailability, SourceCapabilities, SourceDiagnostic, SourceKind, SourceReadContext,
    SourceRef, SourceRevision, SourceScope, SourceSelectionTicket, OVERLAY_DERIVED_ID_CLOSURES,
    OVERLAY_INELIGIBLE_DISK_KEYS, OVERLAY_INELIGIBLE_LEGACY_KEYS, OVERLAY_KEY_AGENTS, SAVE_OP_ID,
    SETTINGS_CAPTURE_LIMIT,
};
use crate::config::instance_artifacts::{
    AGENTS_INSTANCE_FILE_NAME, AGENT_HELP_LOCAL_FILE_NAME, AGENT_HELP_REMOTE_CHECK_FILE_NAME,
    AGENT_HELP_REMOTE_FILE_NAME, AGENT_HELP_SHIPPED_FILE_NAME, BLOCKING_MENUS_LOCAL_FILE_NAME,
    BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME, BLOCKING_MENUS_REMOTE_FILE_NAME,
    BLOCKING_MENUS_SHIPPED_FILE_NAME, SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
};
use crate::config::local_overlay::{LocalSettingsOverlay, PairedLeaf};
use crate::config::placeholders::AC_PLACEHOLDER_TOKENS;
use crate::pty::backend::SessionBackendKind;
use crate::session::profile::CodingAgentKind;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

/// Configuration selection uses the frozen cross-runtime set, never Rust trim.
pub(crate) fn configuration_command_has_content(value: &str) -> bool {
    value.chars().any(|c| {
        !matches!(c,
            '\u{0009}'..='\u{000D}' | '\u{0020}' | '\u{00A0}' | '\u{1680}' |
            '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' |
            '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    })
}

pub(crate) fn select_configuration_command<'a>(
    new: Option<&'a str>,
    old: Option<&'a str>,
) -> Option<&'a str> {
    new.filter(|value| configuration_command_has_content(value))
        .or_else(|| old.filter(|value| configuration_command_has_content(value)))
        .or(new)
        .or(old)
}

/// Absent is optional; a present null or any other non-string is an error.
pub(crate) fn deserialize_present_command<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

pub(crate) fn raw_configuration_command<'a>(
    row: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    for name in [key, "command"] {
        if row.get(name).is_some_and(|value| !value.is_string()) {
            return Err(format!("{name} must be a string"));
        }
    }
    Ok(select_configuration_command(
        row.get(key).and_then(Value::as_str),
        row.get("command").and_then(Value::as_str),
    ))
}

pub(crate) fn overlay_configuration_command(
    row: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    raw_configuration_command(row, key).map(|value| value.map(str::to_owned))
}

pub(crate) const CONFIGURATION_COMMAND_PAIRS: &[PairedLeaf] = &[
    PairedLeaf {
        path: &["agents", "*"],
        canonical: "baseCommand",
        legacy: "command",
        select: overlay_configuration_command,
    },
    PairedLeaf {
        path: &["codingAgentProfiles", "profilesByAgent", "*", "*"],
        canonical: "additionalArguments",
        legacy: "command",
        select: overlay_configuration_command,
    },
];

/// An edit owns the canonical leaf only. The locked physical source owns the
/// legacy leaf, including its absence; a caller never supplies that authority.
pub(crate) fn retain_command_leaf(
    row: &mut Map<String, Value>,
    before: Option<&Map<String, Value>>,
    key: &str,
) -> Result<(), String> {
    if before.is_some_and(|before| {
        row.get(key) == before.get(key) && row.get("command") == before.get("command")
    }) {
        return Ok(());
    }
    raw_configuration_command(row, key)?;
    let edit = row
        .get(key)
        .or_else(|| row.get("command"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(before) = before {
        raw_configuration_command(before, key)?;
    }
    if let Some(edit) = edit {
        row.insert(key.to_owned(), Value::String(edit));
    }
    match before.and_then(|row| row.get("command")) {
        Some(value) => {
            row.insert("command".to_owned(), value.clone());
        }
        None => {
            row.remove("command");
        }
    }
    Ok(())
}

pub(crate) fn retain_configuration_commands(
    after: &mut Value,
    before: &Value,
    identity: &str,
    catalog: bool,
    agents: bool,
    profiles: bool,
) -> Result<(), String> {
    retain_configuration_commands_with_startup(
        after, before, identity, catalog, agents, profiles, None,
    )
}

pub(crate) fn retain_configuration_commands_with_startup(
    after: &mut Value,
    before: &Value,
    identity: &str,
    catalog: bool,
    agents: bool,
    profiles: bool,
    startup: Option<&StartupAgentRows>,
) -> Result<(), String> {
    if agents {
        if !catalog {
            retain_duplicate_agent_rows(after, before, startup)?;
        }
        if let Some(rows) = after.get_mut("agents").and_then(Value::as_array_mut) {
            let mut counts = BTreeMap::new();
            for row in before
                .get("agents")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = row.get(identity).and_then(Value::as_str) {
                    *counts.entry(id).or_insert(0usize) += 1;
                }
            }
            for row in rows {
                if !catalog
                    && row
                        .get(identity)
                        .and_then(Value::as_str)
                        .is_some_and(|id| counts.get(id).copied().unwrap_or(0) > 1)
                {
                    continue;
                }
                let previous = row.get(identity).and_then(Value::as_str).and_then(|id| {
                    before
                        .get("agents")
                        .and_then(Value::as_array)
                        .and_then(|rows| {
                            rows.iter()
                                .find(|row| row.get(identity).and_then(Value::as_str) == Some(id))
                        })
                });
                if let Some(row) = row.as_object_mut() {
                    retain_command_leaf(row, previous.and_then(Value::as_object), "baseCommand")?;
                }
            }
        }
    }
    if profiles {
        let pointer = if catalog {
            "/codingAgentProfiles"
        } else {
            "/codingAgentProfiles/profilesByAgent"
        };
        if let Some(groups) = after.pointer_mut(pointer).and_then(Value::as_object_mut) {
            for (id, cells) in groups {
                if let Some(cells) = cells.as_object_mut() {
                    for (letter, cell) in cells {
                        let previous = before
                            .pointer(pointer)
                            .and_then(|groups| groups.get(id))
                            .and_then(|cells| cells.get(letter))
                            .and_then(Value::as_object);
                        if let Some(cell) = cell.as_object_mut() {
                            retain_command_leaf(cell, previous, "additionalArguments")?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Private to one startup operation; never accepted from a settings caller.
pub(crate) struct StartupAgentRows {
    pub(crate) raw: Value,
    pub(crate) sidecar_owner: bool,
    pub(crate) sidecar_absent: bool,
    pub(crate) permutation: Vec<usize>,
    pub(crate) expected: Vec<AgentConfig>,
}

impl StartupAgentRows {
    pub(crate) fn capture(raw: Value, sidecar_owner: bool, sidecar_absent: bool) -> Option<Self> {
        let mut expected: Vec<AgentConfig> = serde_json::from_value(raw.clone()).ok()?;
        let mut ids = HashSet::new();
        if !expected.iter().any(|agent| !ids.insert(agent.id.clone())) {
            return None;
        }
        let permutation = agent_order_permutation(&expected);
        finalize_agent_order(&mut expected);
        Some(Self {
            raw,
            sidecar_owner,
            sidecar_absent,
            permutation,
            expected,
        })
    }

    pub(crate) fn validate_source(&self, raw: &Value, sidecar_owner: bool) -> Result<(), String> {
        if self.raw == *raw
            && (self.sidecar_owner == sidecar_owner
                || (!self.sidecar_owner && self.sidecar_absent && sidecar_owner))
        {
            Ok(())
        } else {
            let mut ids = HashSet::new();
            let id = self
                .expected
                .iter()
                .find(|agent| !ids.insert(agent.id.as_str()))
                .map(|agent| agent.id.as_str())
                .unwrap_or("unknown");
            Err(format!(
                "duplicate agent ID '{id}': unprovable row correspondence (stale startup source)"
            ))
        }
    }
}

// Includes all AgentConfig wire fields and its sole alias, even omitted defaults.
pub(crate) fn known_agent_field(key: &str) -> bool {
    matches!(
        key,
        "id" | "id_hash"
            | "id_hash_debug"
            | "label"
            | "command"
            | "baseCommand"
            | "preflight"
            | "preflightTimeoutSeconds"
            | "color"
            | "envs"
            | "isolatedHome"
            | "isolateCodexHome"
            | "instructionsFilename"
            | "configSeed"
            | "contextRegex"
            | "blockingMenus"
            | "order"
            | "backend"
    )
}

pub(crate) fn comparable_agent(agent: &AgentConfig) -> Result<Value, String> {
    let mut value = serde_json::to_value(agent).map_err(|error| error.to_string())?;
    if let Some(object) = value.as_object_mut() {
        object.remove("order");
        object.remove("id_hash");
        object.remove("id_hash_debug");
    }
    Ok(value)
}

/// Prove the ordered duplicate subsequence before transferring any raw payload.
pub(crate) fn retain_duplicate_agent_rows(
    after: &mut Value,
    before: &Value,
    startup: Option<&StartupAgentRows>,
) -> Result<(), String> {
    let Some(raw) = before.get("agents").and_then(Value::as_array) else {
        return Ok(());
    };
    let Some(rows) = after.get_mut("agents").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, row) in raw.iter().enumerate() {
        if let Some(id) = row.get("id").and_then(Value::as_str) {
            groups.entry(id).or_default().push(index);
        }
    }
    // Existing unique/new-row saves need no typed duplicate preparation.
    let mut candidate_counts = BTreeMap::new();
    for row in rows.iter() {
        if let Some(id) = row.get("id").and_then(Value::as_str) {
            *candidate_counts.entry(id.to_owned()).or_insert(0usize) += 1;
        }
    }
    for (id, indices) in &groups {
        if indices.len() == 1 && candidate_counts.get(*id).copied().unwrap_or(0) > 1 {
            return Err(format!(
                "duplicate agent ID '{id}': unprovable row correspondence"
            ));
        }
    }
    if !groups.values().any(|indices| indices.len() > 1) {
        return Ok(());
    }
    let decoded: Vec<AgentConfig> =
        serde_json::from_value(Value::Array(raw.clone())).map_err(|error| error.to_string())?;
    let permutation = startup
        .map(|context| context.permutation.clone())
        .unwrap_or_else(|| agent_order_permutation(&decoded));
    let expected = startup
        .map(|context| context.expected.clone())
        .unwrap_or_else(|| {
            let mut expected = decoded;
            finalize_agent_order(&mut expected);
            expected
        });
    for (id, indices) in groups.iter().filter(|(_, indices)| indices.len() > 1) {
        let rejected = || format!("duplicate agent ID '{id}': unprovable row correspondence");
        let candidates: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.get("id").and_then(Value::as_str) == Some(*id))
            .map(|(index, _)| index)
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let baseline: Vec<(usize, &AgentConfig)> = permutation
            .iter()
            .copied()
            .zip(expected.iter())
            .filter(|(_, agent)| agent.id == *id)
            .collect();
        if candidates.len() != indices.len() || baseline.len() != indices.len() {
            return Err(rejected());
        }
        for (candidate_index, (raw_index, expected)) in candidates.into_iter().zip(baseline) {
            let candidate: AgentConfig =
                serde_json::from_value(rows[candidate_index].clone()).map_err(|_| rejected())?;
            if comparable_agent(&candidate)? != comparable_agent(expected)? {
                return Err(rejected());
            }
            let original = raw[raw_index].as_object().ok_or_else(rejected)?;
            let provided = rows[candidate_index].as_object().ok_or_else(rejected)?;
            let mut retained = original.clone();
            for (key, value) in provided {
                if known_agent_field(key) {
                    continue;
                }
                if original.get(key) != Some(value) {
                    return Err(rejected());
                }
            }
            // Rebuild recognized fields through the existing serializer, preserving
            // unknown metadata from this exact whole raw row only.
            retained.retain(|key, _| !known_agent_field(key));
            retained.extend(
                serde_json::to_value(&candidate)
                    .map_err(|error| error.to_string())?
                    .as_object()
                    .ok_or_else(rejected)?
                    .clone(),
            );
            retained.insert("baseCommand".to_owned(), Value::String(candidate.command));
            match original.get("command") {
                Some(value) => {
                    retained.insert("command".to_owned(), value.clone());
                }
                None => {
                    retained.remove("command");
                }
            }
            rows[candidate_index] = Value::Object(retained);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendConfig {
    #[serde(default)]
    pub kind: SessionBackendKind,
    /// #868 - optional per-agent Docker image override for container runtime.
    /// None falls back to AGENTSCOMMANDER_CONTAINER_IMAGE at launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

impl Default for AgentBackendConfig {
    fn default() -> Self {
        Self {
            kind: SessionBackendKind::LocalProcess,
            image: None,
        }
    }
}

impl AgentBackendConfig {
    pub(crate) fn is_default(&self) -> bool {
        self.kind == SessionBackendKind::LocalProcess
    }
}

impl From<&AgentBackendConfig> for SessionBackendKind {
    fn from(config: &AgentBackendConfig) -> Self {
        config.kind
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub id: String,
    #[serde(
        default,
        rename = "id_hash",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_agent_id_hash"
    )]
    // Base-command legacy metadata; profile-cell id_hash identifies a command/profile pair.
    pub id_hash: Option<String>,
    #[serde(
        default,
        rename = "id_hash_debug",
        skip_serializing_if = "Option::is_none"
    )]
    pub id_hash_debug: Option<Vec<String>>,
    pub label: String,
    #[serde(rename = "baseCommand")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_timeout_seconds: Option<u32>,
    pub color: String,
    /// Base environment rows applied to every launch of this coding agent.
    #[serde(default)]
    pub envs: Vec<CodingAgentEnv>,
    /// When true for Codex, AC provides an isolated CODEX_HOME at spawn time.
    #[serde(default, alias = "isolateCodexHome")]
    pub isolated_home: bool,
    /// #529 - filename AC writes into the agent root at launch (content = AC
    /// context + Role.md). `None`/empty falls back to the command-derived default
    /// (Claude -> CLAUDE.md, Codex/Pi/Antigravity/else -> AGENTS.md). Serialized as
    /// `instructionsFilename`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    /// #598 - optional config-folder seed (e.g. `.claude`) copied from a
    /// convention-chosen template into the replica at spawn. `None`/inactive
    /// means no seeding. Serialized as `configSeed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed: Option<ConfigSeedConfig>,
    /// #1032 - per-agent regex run over this agent's screen rows to read its
    /// context-window usage. Capture group 1 is the percentage. `None`/absent means
    /// the feature is off for this agent: no event, no reading, no PTY lock, no
    /// #1032 - per-agent regex run over this agent's screen rows to read its
    /// context-window usage. Capture group 1 is the percentage. `None`/absent means
    /// the feature is off for this agent: no event, no reading, no PTY lock, no
    /// compile. Serialized as `contextRegex`.
    ///
    /// The engine ships no anchoring rules of its own; every rule that makes a
    /// pattern trustworthy lives in the pattern, because only the user knows what
    /// their agent renders. A reading may enqueue an informational notice to the exact
    /// workgroup coordinator, but it never drives remedial or destructive session action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_regex: Option<String>,
    /// #1905 - legacy. Read by `export_blocking_menus_to_local_file`, which moves it to
    /// `settings-blocking-menus.local.json` and sets it to `None`, and by the menu guard as
    /// layer 0 while it is still present. No production writer sets `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_menus: Option<Vec<BlockingMenuEntry>>,
    /// #2306 P1 - explicit zero-based position of this tool inside
    /// `settings.agents`. `None` means no usable stored position: the record
    /// predates the field, or its JSON value was not a nonnegative 32-bit
    /// integer. Leaving `order` absent never fails the settings decode.
    ///
    /// Loading finalizes every record to a unique contiguous `Some(0..n-1)`
    /// position and loading alone never writes; the next authorized save
    /// persists what the loader derived.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_agent_order"
    )]
    pub order: Option<u32>,
    /// Backend used for future non-local session transports. Omitted/default
    /// keeps today's local-process behavior.
    #[serde(default, skip_serializing_if = "AgentBackendConfig::is_default")]
    pub backend: AgentBackendConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentConfigWire {
    pub id: String,
    #[serde(
        default,
        rename = "id_hash",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_agent_id_hash"
    )]
    pub id_hash: Option<String>,
    #[serde(
        default,
        rename = "id_hash_debug",
        deserialize_with = "deserialize_agent_id_hash_debug"
    )]
    pub id_hash_debug: Option<Vec<String>>,
    pub label: String,
    #[serde(
        default,
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub command: Option<String>,
    #[serde(
        default,
        rename = "baseCommand",
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub canonical_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_timeout_seconds: Option<u32>,
    pub color: String,
    #[serde(default)]
    pub envs: Vec<CodingAgentEnv>,
    #[serde(default, alias = "isolateCodexHome")]
    pub isolated_home: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed: Option<ConfigSeedConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_menus: Option<Vec<BlockingMenuEntry>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_agent_order"
    )]
    pub order: Option<u32>,
    #[serde(default, skip_serializing_if = "AgentBackendConfig::is_default")]
    pub backend: AgentBackendConfig,
}

impl<'de> Deserialize<'de> for AgentConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AgentConfigWire::deserialize(deserializer)?;
        let command = crate::config::settings::select_configuration_command(
            wire.canonical_command.as_deref(),
            wire.command.as_deref(),
        )
        .ok_or_else(|| <D::Error as serde::de::Error>::missing_field("baseCommand"))?
        .to_owned();
        Ok(Self {
            id: wire.id,
            id_hash: wire.id_hash,
            id_hash_debug: wire.id_hash_debug,
            label: wire.label,
            command,
            preflight: wire.preflight,
            preflight_timeout_seconds: wire.preflight_timeout_seconds,
            color: wire.color,
            envs: wire.envs,
            isolated_home: wire.isolated_home,
            instructions_filename: wire.instructions_filename,
            config_seed: wire.config_seed,
            context_regex: wire.context_regex,
            blocking_menus: wire.blocking_menus,
            order: wire.order,
            backend: wire.backend,
        })
    }
}

/// #2306 P1 - tolerant decode for [`AgentConfig::order`].
///
/// Only a nonnegative integer that fits `u32` decodes as `Some`. `null`,
/// negative, fractional, string and overflowing values become `None` so one
/// hand-edited ordinal cannot make the whole settings file unreadable; absent
/// and invalid are indistinguishable after decode by design.
pub(crate) fn deserialize_agent_order<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value
        .as_ref()
        .and_then(Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok()))
}

/// Optional, non-authoritative metadata: preserve strings and ignore other JSON types.
pub(crate) fn deserialize_agent_id_hash<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        Value::String(hash) => Some(hash),
        _ => None,
    }))
}

/// Optional debug leaves are accepted only as a complete array of strings.
pub(crate) fn deserialize_agent_id_hash_debug<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        Value::Array(values) => values
            .into_iter()
            .map(|value| match value {
                Value::String(value) => Some(value),
                _ => None,
            })
            .collect(),
        _ => None,
    }))
}

/// #2306 P1 - sort `agents` into their effective order and rewrite every
/// `order` as a unique contiguous `0..n-1` value.
///
/// The sort key is the explicit [`AgentConfig::order`] when present, otherwise
/// the record's original array index; ties break by original array index, which
/// keeps a legacy file without ordinals in its exact vector order and makes the
/// result deterministic for mixed, duplicate and out-of-range ordinals. Records
/// move whole, so ids, labels, profile cells and every other field are
/// untouched. This is the load-side finalize and the only place allowed to
/// re-sort; use [`normalize_agent_order`] on the write side.
pub fn finalize_agent_order(agents: &mut Vec<AgentConfig>) {
    let permutation = agent_order_permutation(agents);
    if permutation
        .iter()
        .enumerate()
        .all(|(position, &index)| position == index)
    {
        normalize_agent_order(agents);
        return;
    }
    let mut slots: Vec<Option<AgentConfig>> =
        std::mem::take(agents).into_iter().map(Some).collect();
    let mut ordered: Vec<AgentConfig> = Vec::with_capacity(slots.len());
    for index in permutation {
        if let Some(agent) = slots[index].take() {
            ordered.push(agent);
        }
    }
    *agents = ordered;
    normalize_agent_order(agents);
}

pub(crate) fn agent_order_permutation(agents: &[AgentConfig]) -> Vec<usize> {
    let mut keyed: Vec<(u64, usize)> = agents
        .iter()
        .enumerate()
        .map(|(index, agent)| (agent.order.map_or(index as u64, u64::from), index))
        .collect();
    // The original index is part of the key, so the order is total and the
    // sort outcome does not depend on the sort implementation being stable.
    keyed.sort_unstable();
    keyed.into_iter().map(|(_, index)| index).collect()
}

/// #2306 P1 - write-side normalizer: renumber `agents` in current vector order
/// as contiguous `0..n-1`.
///
/// It never sorts, so stale numeric values cannot resurrect an old order;
/// idempotent. Call it immediately before a write when the in-memory vector is
/// already the effective order.
pub fn normalize_agent_order(agents: &mut [AgentConfig]) {
    for (position, agent) in agents.iter_mut().enumerate() {
        agent.order = Some(position as u32);
    }
}

/// #1646 / #1647 - one entry of an agent's `blockingMenus` array, or whatever the user wrote there.
/// Untagged so `Invalid` catches malformed entries without failing settings deserialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum BlockingMenuEntry {
    Valid(BlockingMenuConfig),
    /// Anything that did not deserialize as a `BlockingMenuConfig`. Skipped by evaluation.
    Invalid(serde_json::Value),
}

impl BlockingMenuEntry {
    pub fn valid(&self) -> Option<&BlockingMenuConfig> {
        match self {
            BlockingMenuEntry::Valid(config) => Some(config),
            BlockingMenuEntry::Invalid(_) => None,
        }
    }
}

/// #1646 / #1647 - one configured blocking menu pattern and notification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockingMenuConfig {
    pub pattern: String,
    pub notification: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_against: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentEnv {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub source: CodingAgentEnvSource,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CodingAgentEnvSource {
    #[default]
    User,
    #[serde(alias = "agentsCommander")]
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSlotConfig {
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCellConfig {
    #[serde(default, rename = "id_hash", skip_serializing_if = "Option::is_none")]
    pub id_hash: Option<String>,
    #[serde(
        default,
        rename = "id_hash_debug",
        skip_serializing_if = "Option::is_none"
    )]
    pub id_hash_debug: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    #[serde(rename = "additionalArguments")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_timeout_seconds: Option<u32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileCellConfigWire {
    #[serde(
        default,
        rename = "id_hash",
        deserialize_with = "deserialize_agent_id_hash"
    )]
    pub id_hash: Option<String>,
    #[serde(
        default,
        rename = "id_hash_debug",
        deserialize_with = "deserialize_agent_id_hash_debug"
    )]
    pub id_hash_debug: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(
        default,
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub command: Option<String>,
    #[serde(
        default,
        rename = "additionalArguments",
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub canonical_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_timeout_seconds: Option<u32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: String,
}

impl<'de> Deserialize<'de> for ProfileCellConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProfileCellConfigWire::deserialize(deserializer)?;
        let command = crate::config::settings::select_configuration_command(
            wire.canonical_command.as_deref(),
            wire.command.as_deref(),
        )
        .unwrap_or_default()
        .to_owned();
        Ok(Self {
            id_hash: wire.id_hash,
            id_hash_debug: wire.id_hash_debug,
            enabled: wire.enabled,
            command,
            preflight: wire.preflight,
            preflight_timeout_seconds: wire.preflight_timeout_seconds,
            env: wire.env,
            notes: wire.notes,
        })
    }
}

// E2432: raw physical storage is authoritative. ID maps below are transient
// compatibility projections only; quarantine never participates in projection.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhysicalProfileSets {
    pub(crate) rows: BTreeMap<String, String>,
    pub(crate) cells: Map<String, Value>,
    pub(crate) labels: Map<String, Value>,
    pub(crate) orphans: Option<Map<String, Value>>,
}

pub(crate) fn profile_object(
    value: Option<&Value>,
    path: &str,
) -> Result<Map<String, Value>, String> {
    match value {
        None => Ok(Map::new()),
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(_) => Err(format!("invalidProfileMap: {path}")),
    }
}

pub(crate) fn profile_container(source: &Value, registered: bool) -> Option<&Value> {
    if registered {
        source.get("codingAgentProfiles")
    } else {
        Some(source)
    }
}

pub(crate) fn validate_profile_orphans(
    value: Option<&Value>,
) -> Result<Option<Map<String, Value>>, String> {
    let Some(value) = value else { return Ok(None) };
    let invalid = |path: &str| format!("invalidLegacyProfileOrphans: {path}");
    let object = value
        .as_object()
        .ok_or_else(|| invalid("legacyProfileOrphans"))?;
    for (id, records) in object {
        let path = format!("legacyProfileOrphans[{id:?}]");
        let records = records
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid(&path))?;
        for (index, record) in records.iter().enumerate() {
            let record = record
                .as_object()
                .ok_or_else(|| invalid(&format!("{path}[{index}]")))?;
            if !["cells", "labels", "identity"]
                .iter()
                .any(|key| record.contains_key(*key))
            {
                return Err(invalid(&format!("{path}[{index}]")));
            }
        }
    }
    Ok(Some(object.clone()))
}

pub(crate) fn normalized_profile_cells(value: Option<&Value>) -> Result<Value, String> {
    let mut cells = profile_object(value, "cells")?;
    for (letter, raw) in &mut cells {
        if !is_valid_profile_letter(letter) {
            return Err(format!("invalidProfileLetter: {letter}"));
        }
        let cell: ProfileCellConfig =
            serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
        let mut normalized = raw.as_object().cloned().ok_or("invalidProfileCell")?;
        normalized.remove("id_hash");
        normalized.remove("id_hash_debug");
        let mut typed = serde_json::to_value(cell).map_err(|e| e.to_string())?;
        let typed = typed.as_object_mut().expect("cell object");
        typed.remove("id_hash");
        typed.remove("id_hash_debug");
        normalized.extend(typed.clone());
        // Optional serde-default fields compare equally whether omitted or null.
        for key in ["preflight", "preflightTimeoutSeconds"] {
            if normalized.get(key).is_some_and(Value::is_null) {
                normalized.remove(key);
            }
        }
        *raw = Value::Object(normalized);
    }
    if !cells.contains_key("A") {
        let mut empty = serde_json::to_value(empty_profile_cell()).map_err(|e| e.to_string())?;
        empty.as_object_mut().unwrap().remove("id_hash");
        empty.as_object_mut().unwrap().remove("id_hash_debug");
        cells.insert("A".into(), empty);
    }
    Ok(Value::Object(cells))
}

pub(crate) fn physical_profile_rows(
    source: &Value,
    registered: bool,
) -> Result<BTreeMap<String, String>, String> {
    let mut rows = BTreeMap::new();
    let mut seen = HashSet::new();
    let empty = Vec::new();
    let agents = match source.get("agents") {
        None => &empty,
        Some(Value::Array(rows)) => rows,
        Some(_) => return Err("invalidAgentsArray".into()),
    };
    for row in agents {
        let object = row.as_object().ok_or("invalidAgentRow")?;
        let id = object
            .get(if registered { "id" } else { "key" })
            .and_then(Value::as_str)
            .ok_or("invalidAgentKey")?;
        if id.is_empty() {
            return Err("invalidAgentKey".into());
        }
        if !registered {
            crate::config::coding_agents_catalog::validate_catalog_key(id)?;
        }
        if !seen.insert(id.to_owned()) {
            return Err(format!("duplicateAgentKey: {id}"));
        }
        if object.get("remove") == Some(&Value::Bool(true)) {
            continue;
        }
        let command = raw_configuration_command(object, "baseCommand")?;
        let Some(command) = command.filter(|command| !command.trim().is_empty()) else {
            if registered {
                return Err("invalidRegisteredCommand".into());
            }
            continue;
        };
        if let Err(reason) = validate_agent_command_text("physical profile row", command) {
            if registered {
                return Err(reason);
            }
            continue;
        }
        let envs: Vec<CodingAgentEnv> = object
            .get("envs")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        rows.insert(id.to_owned(), configuration_identity(command, &envs));
    }
    Ok(rows)
}

pub(crate) fn physical_profile_sets(
    source: &Value,
    registered: bool,
) -> Result<PhysicalProfileSets, String> {
    let rows = physical_profile_rows(source, registered)?;
    let empty = Value::Object(Map::new());
    let profiles = profile_container(source, registered).unwrap_or(&empty);
    if !profiles.is_object() {
        return Err("invalidProfileContainer".into());
    }
    let version = profiles.get("profileBindingVersion");
    if version.is_some() && version != Some(&Value::from(1)) {
        return Err("unsupportedProfileBindingVersion".into());
    }
    let canonical = version.is_some();
    let legacy_cells_key = if registered {
        "profilesByAgent"
    } else {
        "codingAgentProfiles"
    };
    let legacy_labels_key = if registered {
        "profileLabelsByAgent"
    } else {
        "profileLabels"
    };
    let legacy_cells = profile_object(profiles.get(legacy_cells_key), legacy_cells_key)?;
    let legacy_labels = profile_object(profiles.get(legacy_labels_key), legacy_labels_key)?;
    let identities = if registered {
        profile_object(profiles.get("identityByAgent"), "identityByAgent")?
    } else {
        Map::new()
    };
    let mut result = PhysicalProfileSets {
        rows,
        cells: profile_object(profiles.get("profilesByBaseHash"), "profilesByBaseHash")?,
        labels: profile_object(
            profiles.get("profileLabelsByBaseHash"),
            "profileLabelsByBaseHash",
        )?,
        orphans: validate_profile_orphans(profiles.get("legacyProfileOrphans"))?,
    };
    for labels in legacy_labels
        .iter()
        .filter(|(id, _)| result.rows.contains_key(*id))
        .filter_map(|(_, value)| value.as_object())
        .filter(|_| !canonical)
    {
        if labels.keys().any(|letter| !is_valid_profile_letter(letter)) {
            return Err("invalidProfileLetter".into());
        }
    }
    for cells in result.cells.values_mut() {
        *cells = normalized_profile_cells(Some(cells))?;
    }
    for labels in result.labels.values() {
        if labels
            .as_object()
            .is_some_and(|labels| labels.keys().any(|letter| !is_valid_profile_letter(letter)))
        {
            return Err("invalidProfileLetter".into());
        }
        serde_json::from_value::<BTreeMap<String, String>>(labels.clone())
            .map_err(|e| e.to_string())?;
    }
    if !canonical {
        for (id, hash) in &result.rows {
            let cells = normalized_profile_cells(legacy_cells.get(id))?;
            let labels = Value::Object(profile_object(legacy_labels.get(id), "labels")?);
            serde_json::from_value::<BTreeMap<String, String>>(labels.clone())
                .map_err(|e| e.to_string())?;
            if result
                .cells
                .get(hash)
                .is_some_and(|previous| previous != &cells)
                || result
                    .labels
                    .get(hash)
                    .is_some_and(|previous| previous != &labels)
            {
                return Err(format!("sourceProfileConflict: {hash}; row {id}"));
            }
            result.cells.insert(hash.clone(), cells);
            result.labels.insert(hash.clone(), labels);
        }
    }
    // Capture raw values before typed decoding. Existing arrays (including equal
    // records) keep their order. Only newly appended equal records are suppressed.
    let ids: std::collections::BTreeSet<_> = legacy_cells
        .keys()
        .chain(legacy_labels.keys())
        .chain(identities.keys())
        .collect();
    for id in ids {
        if result.rows.contains_key(id) {
            continue;
        }
        let mut record = Map::new();
        for (key, map) in [
            ("cells", &legacy_cells),
            ("labels", &legacy_labels),
            ("identity", &identities),
        ] {
            if let Some(value) = map.get(id) {
                record.insert(key.into(), value.clone());
            }
        }
        let record = Value::Object(record);
        let records = result
            .orphans
            .get_or_insert_with(Map::new)
            .entry(id.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let records = records.as_array_mut().expect("validated quarantine array");
        if !records.contains(&record) {
            records.push(record);
        }
    }
    Ok(result)
}

/// A read-only compatibility view. Never serialize this projection as storage.
pub(crate) fn project_physical_profiles(source: &Value, registered: bool) -> Result<Value, String> {
    let sets = physical_profile_sets(source, registered)?;
    let mut projected = source.clone();
    if registered && projected.get("codingAgentProfiles").is_none() {
        projected["codingAgentProfiles"] = serde_json::json!({});
    }
    let profiles = if registered {
        projected.get_mut("codingAgentProfiles").unwrap()
    } else {
        &mut projected
    };
    let profiles = profiles.as_object_mut().ok_or("invalidProfileContainer")?;
    let mut cells = Map::new();
    let mut labels = Map::new();
    for (id, hash) in &sets.rows {
        cells.insert(
            id.clone(),
            sets.cells
                .get(hash)
                .cloned()
                .unwrap_or(normalized_profile_cells(None)?),
        );
        labels.insert(
            id.clone(),
            sets.labels
                .get(hash)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        );
    }
    profiles.insert(
        if registered {
            "profilesByAgent"
        } else {
            "codingAgentProfiles"
        }
        .into(),
        Value::Object(cells),
    );
    profiles.insert(
        if registered {
            "profileLabelsByAgent"
        } else {
            "profileLabels"
        }
        .into(),
        Value::Object(labels),
    );
    if registered {
        profiles.remove("identityByAgent");
    }
    // A compatibility draft is an ID projection, never a second canonical
    // authority. This also distinguishes an explicit alias edit from stale
    // legacy maps still present in a raw version1 physical document.
    for key in [
        "profileBindingVersion",
        "profilesByBaseHash",
        "profileLabelsByBaseHash",
    ] {
        profiles.remove(key);
    }
    Ok(projected)
}

pub(crate) fn registered_profiles_for_read(source: &Value) -> Value {
    match project_physical_profiles(source, true) {
        Ok(projected) => projected,
        Err(reason) => {
            log::warn!("[profiles] read-only diagnostic: {reason}");
            let mut diagnostic = source.clone();
            if let Some(profiles) = diagnostic
                .get_mut("codingAgentProfiles")
                .and_then(Value::as_object_mut)
            {
                if profiles.get("profileBindingVersion") == Some(&Value::from(1)) {
                    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
                        profiles.remove(key);
                    }
                }
            }
            diagnostic
        }
    }
}

// Key presence selects each registered group independently, including []/{}.
// Legacy preparation and the directional ordinary view use only this owner's
// selected physical rows. Borrowing a row never transfers its ownership.
pub(crate) fn selected_registered_profile_owner(
    main: &Map<String, Value>,
    sidecar: Option<&Map<String, Value>>,
    consolidate_legacy: bool,
) -> Value {
    let profiles_in_sidecar = sidecar.is_some_and(|map| {
        map.contains_key("codingAgentProfiles")
            || (map.contains_key("agents") && !main.contains_key("codingAgentProfiles"))
    });
    let mut owner = if profiles_in_sidecar {
        sidecar.cloned().unwrap_or_default()
    } else {
        main.clone()
    };
    // Shadowed main rows never become a donor, even for a canonical main group.
    if !profiles_in_sidecar && sidecar.is_some_and(|map| map.contains_key("agents")) {
        owner.remove("agents");
    }
    let directional = profiles_in_sidecar
        && sidecar.is_some_and(|map| !map.contains_key("agents"))
        && owner
            .get("codingAgentProfiles")
            .is_some_and(|profiles| profiles.get("profileBindingVersion") == Some(&Value::from(1)));
    if directional
        || (consolidate_legacy
            && owner
                .get("codingAgentProfiles")
                .is_none_or(|profiles| profiles.get("profileBindingVersion").is_none()))
    {
        match sidecar
            .and_then(|map| map.get("agents"))
            .or_else(|| main.get("agents"))
        {
            Some(rows) => {
                owner.insert("agents".into(), rows.clone());
            }
            None => {
                owner.remove("agents");
            }
        }
    }
    Value::Object(owner)
}

/// Regroup an explicit ID-view edit against the previous physical projection.
/// Unchanged aliases never compete with a changed alias. Raw extensions survive.
pub(crate) fn canonical_profile_source(
    before: &Value,
    incoming: &Value,
    registered: bool,
) -> Result<Value, String> {
    if !registered && before.get("managed").is_some() {
        return Err("sourceReadOnly".into());
    }
    let mut sets = physical_profile_sets(before, registered)?;
    let next_rows = physical_profile_rows(incoming, registered)?;
    let empty = serde_json::json!({});
    let incoming_profiles = profile_container(incoming, registered).unwrap_or(&empty);
    let compatible_draft =
        if incoming_profiles.get("profileBindingVersion") == Some(&Value::from(1)) {
            project_physical_profiles(before, registered)?
        } else {
            incoming.clone()
        };
    let submitted = profile_container(&compatible_draft, registered).unwrap_or(&empty);
    let old_projection = project_physical_profiles(before, registered)?;
    let old = profile_container(&old_projection, registered).unwrap_or(&empty);
    let cells_key = if registered {
        "profilesByAgent"
    } else {
        "codingAgentProfiles"
    };
    let labels_key = if registered {
        "profileLabelsByAgent"
    } else {
        "profileLabels"
    };
    let submitted_cells = profile_object(submitted.get(cells_key), cells_key)?;
    let submitted_labels = profile_object(submitted.get(labels_key), labels_key)?;
    let mut changes = BTreeMap::<String, (Value, Value)>::new();
    let mut forks = BTreeMap::<String, (Value, Value)>::new();
    for (id, hash) in &next_rows {
        let old_hash = sets.rows.get(id);
        let previous_cells = old.get(cells_key).and_then(|map| map.get(id));
        let previous_labels = old
            .get(labels_key)
            .and_then(|map| map.get(id))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let mut cells = normalized_profile_cells(submitted_cells.get(id))?;
        let labels = Value::Object(profile_object(submitted_labels.get(id), labels_key)?);
        let old_cells = normalized_profile_cells(previous_cells)?;
        if let (Some(new_cells), Some(previous)) = (cells.as_object_mut(), old_cells.as_object()) {
            for (letter, cell) in new_cells {
                if let (Some(cell), Some(previous)) = (
                    cell.as_object_mut(),
                    previous.get(letter).and_then(Value::as_object),
                ) {
                    let typed: ProfileCellConfig =
                        serde_json::from_value(Value::Object(previous.clone()))
                            .map_err(|e| e.to_string())?;
                    let known = serde_json::to_value(typed).map_err(|e| e.to_string())?;
                    for (key, value) in previous {
                        if !known.as_object().unwrap().contains_key(key)
                            && !["id_hash", "id_hash_debug"].contains(&key.as_str())
                        {
                            cell.entry(key.clone()).or_insert_with(|| value.clone());
                        }
                    }
                }
            }
        }
        let changed = cells != old_cells || labels != previous_labels;
        if changed {
            let value = (cells, labels);
            if changes.get(hash).is_some_and(|existing| existing != &value) {
                return Err(format!("sourceProfileConflict: {hash}; row {id}"));
            }
            changes.insert(hash.clone(), value);
        } else if old_hash.is_some_and(|old_hash| old_hash != hash) {
            let value = (old_cells, previous_labels);
            if forks.get(hash).is_some_and(|existing| existing != &value) {
                return Err(format!("sourceProfileConflict: {hash}; row {id}"));
            }
            forks.insert(hash.clone(), value);
        } else if !sets.cells.contains_key(hash) {
            forks.insert(hash.clone(), (cells, labels));
        }
    }
    for (hash, (cells, labels)) in forks {
        if sets
            .cells
            .get(&hash)
            .is_some_and(|existing| existing != &cells)
            || sets
                .labels
                .get(&hash)
                .is_some_and(|existing| existing != &labels)
        {
            return Err(format!("sourceProfileConflict: {hash}"));
        }
        sets.cells.entry(hash.clone()).or_insert(cells);
        sets.labels.entry(hash).or_insert(labels);
    }
    for (hash, (cells, labels)) in changes {
        if next_rows
            .iter()
            .any(|(id, h)| h == &hash && sets.rows.get(id).is_none_or(|old| old != h))
            && (sets.cells.get(&hash).is_some_and(|v| v != &cells)
                || sets.labels.get(&hash).is_some_and(|v| v != &labels))
        {
            return Err(format!("sourceProfileConflict: {hash}"));
        }
        sets.cells.insert(hash.clone(), cells);
        sets.labels.insert(hash, labels);
    }
    let mut result = before.clone();
    let result_root = result.as_object_mut().ok_or("invalidSource")?;
    let incoming_root = incoming.as_object().ok_or("invalidSource")?;
    if let Some(rows) = incoming_root.get("agents") {
        result_root.insert("agents".into(), rows.clone());
    }
    if registered && !result_root.contains_key("codingAgentProfiles") {
        result_root.insert("codingAgentProfiles".into(), empty.clone());
    }
    let target = if registered {
        result_root
            .get_mut("codingAgentProfiles")
            .unwrap()
            .as_object_mut()
            .ok_or("invalidProfileContainer")?
    } else {
        result_root
    };
    if let Some(fields) = incoming_profiles.as_object() {
        for key in [
            "schemaVersion",
            "profileSlots",
            "defaultProfileByAgent",
            "codexHomeKeyByAgent",
        ] {
            if let Some(value) = fields.get(key) {
                target.insert(key.into(), value.clone());
            }
        }
    }
    if !registered {
        target
            .entry("profileSlots".to_owned())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(order) = incoming_root.get("order") {
            target.insert("order".into(), order.clone());
        } else {
            target.remove("order");
        }
    }
    target.insert("profileBindingVersion".into(), Value::from(1));
    target.insert("profilesByBaseHash".into(), Value::Object(sets.cells));
    target.insert("profileLabelsByBaseHash".into(), Value::Object(sets.labels));
    if let Some(orphans) = sets.orphans {
        target.insert("legacyProfileOrphans".into(), Value::Object(orphans));
    }
    target.remove(cells_key);
    target.remove(labels_key);
    if registered {
        target.remove("identityByAgent");
    }
    refresh_physical_source_rows(&mut result, registered)?;
    Ok(result)
}

pub(crate) fn refresh_physical_source_rows(
    source: &mut Value,
    registered: bool,
) -> Result<(), String> {
    let hashes = physical_profile_rows(source, registered)?;
    for row in source
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let object = row.as_object_mut().ok_or("invalidAgentRow")?;
        let key = object
            .get(if registered { "id" } else { "key" })
            .and_then(Value::as_str)
            .ok_or("invalidAgentKey")?;
        if let Some(hash) = hashes.get(key).filter(|_| registered) {
            let command = raw_configuration_command(object, "baseCommand")?.unwrap_or_default();
            let envs: Vec<CodingAgentEnv> = object
                .get("envs")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let debug = serde_json::json!(configuration_identity_debug(command, &envs));
            object.insert("id_hash".into(), Value::String(hash.clone()));
            object.insert("id_hash_debug".into(), debug);
        } else {
            object.remove("id_hash");
            object.remove("id_hash_debug");
        }
    }
    Ok(())
}

/// Catalog keys become transient IDs; physical definitions remain untouched.
pub(crate) fn decode_physical_catalog(
    value: &Value,
) -> Result<(Vec<AgentConfig>, CodingAgentProfilesConfig), String> {
    if !matches!(
        value.get("schemaVersion").and_then(Value::as_u64),
        Some(1 | 2)
    ) {
        return Err("unsupportedCatalogSchema".into());
    }
    if value.get("schemaVersion").and_then(Value::as_u64) == Some(2) {
        crate::config::coding_agents_catalog::parse_catalog_schema2(
            &serde_json::to_vec(value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    let sets = physical_profile_sets(value, false)?;
    let projected = project_physical_profiles(value, false)?;
    let mut agents = Vec::new();
    for raw in value
        .get("agents")
        .and_then(Value::as_array)
        .ok_or("invalidAgentsArray")?
    {
        let key = raw
            .get("key")
            .and_then(Value::as_str)
            .ok_or("invalidAgentKey")?;
        if !sets.rows.contains_key(key) {
            continue;
        }
        crate::config::coding_agents_catalog::validate_catalog_key(key)?;
        let mut row = raw.as_object().cloned().ok_or("invalidAgentRow")?;
        row.insert("id".into(), Value::String(key.to_owned()));
        row.entry("label".to_owned())
            .or_insert_with(|| Value::String(String::new()));
        row.entry("color".to_owned())
            .or_insert_with(|| Value::String(String::new()));
        agents.push(serde_json::from_value(Value::Object(row)).map_err(|e| e.to_string())?);
    }
    let profiles = serde_json::json!({
        "profilesByAgent":projected.get("codingAgentProfiles"),
        "profileLabelsByAgent":projected.get("profileLabels"),
        "profileSlots":value.get("profileSlots").cloned().unwrap_or_else(|| serde_json::json!({}))
    });
    let mut profiles = serde_json::from_value(profiles).map_err(|e| e.to_string())?;
    refresh_agent_id_hashes(&mut agents);
    refresh_pair_id_hashes(&agents, &mut profiles);
    Ok((agents, profiles))
}

pub(crate) fn duplicate_profile_document(bytes: &[u8], path: &Path) -> Option<String> {
    serde_json::from_slice::<Value>(bytes).ok()?;
    crate::config::coding_agents_catalog::parse_strict_json(bytes)
        .err()
        .map(|reason| {
            format!(
                "dupl\
        icateJsonMember: {}: {reason}; no files changed; correct the ambiguous file \
        externally",
                path.display()
            )
        })
}

pub(crate) type RegisteredProfileDocument = (PathBuf, Option<Vec<u8>>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegisteredProfileProblemKind {
    Io,
    JsonSyntax,
    DuplicateMember,
    ProfileSemantic,
}

pub(crate) struct RegisteredProfileProblem {
    pub(crate) path: PathBuf,
    pub(crate) kind: RegisteredProfileProblemKind,
    pub(crate) reason: String,
    pub(crate) ordinary_participant: bool,
}

#[derive(Default)]
pub(crate) struct RegisteredProfileCapture {
    pub(crate) documents: Vec<RegisteredProfileDocument>,
    pub(crate) problems: Vec<RegisteredProfileProblem>,
}

impl RegisteredProfileCapture {
    pub(crate) fn ordinary_error(&self) -> Option<String> {
        self.problems
            .iter()
            .find(|problem| problem.ordinary_participant)
            .map(|problem| problem.reason.clone())
    }

    pub(crate) fn admits_project_migration(&self, path: &Path) -> bool {
        let sidecar = agents_instance_path(path);
        self.problems.iter().all(|problem| {
            !problem.ordinary_participant
                || (problem.path == sidecar
                    && problem.kind == RegisteredProfileProblemKind::JsonSyntax)
        })
    }

    pub(crate) fn participant_documents(&self) -> Vec<RegisteredProfileDocument> {
        self.documents
            .iter()
            .filter(|(path, _)| {
                !self
                    .problems
                    .iter()
                    .any(|problem| !problem.ordinary_participant && &problem.path == path)
            })
            .cloned()
            .collect()
    }
}

// Collect every participant even after a failure. In particular, malformed
// sidecar bytes must not hide a later participating overlay's rejection.
pub(crate) fn collect_registered_profile_gate(path: &Path) -> RegisteredProfileCapture {
    let inputs = [
        path.to_path_buf(),
        agents_instance_path(path),
        path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
    ]
    .into_iter()
    .map(|participant| {
        let input = match std::fs::read(&participant) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        };
        (participant, input)
    })
    .collect();
    classify_registered_profile_inputs(path, inputs)
}

pub(crate) type RegisteredProfileInput = (PathBuf, Result<Option<Vec<u8>>, String>);

pub(crate) fn classify_registered_profile_inputs(
    path: &Path,
    inputs: Vec<RegisteredProfileInput>,
) -> RegisteredProfileCapture {
    let mut captured = RegisteredProfileCapture::default();
    let mut sources = Vec::new();
    for (participant, input) in inputs {
        let overlay = participant.file_name()
            == Some(std::ffi::OsStr::new(SETTINGS_LOCAL_OVERRIDE_FILE_NAME));
        let bytes = match input {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                captured.documents.push((participant, None));
                continue;
            }
            Err(e) => {
                captured.problems.push(RegisteredProfileProblem {
                    reason: format!("{}: {e}", participant.display()),
                    path: participant,
                    kind: RegisteredProfileProblemKind::Io,
                    ordinary_participant: !overlay,
                });
                continue;
            }
        };
        let value = match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => value,
            Err(e) => {
                captured.problems.push(RegisteredProfileProblem {
                    reason: format!("{}: {e}", participant.display()),
                    path: participant.clone(),
                    kind: RegisteredProfileProblemKind::JsonSyntax,
                    ordinary_participant: !overlay,
                });
                captured.documents.push((participant, Some(bytes)));
                continue;
            }
        };
        if participant.file_name() == Some(std::ffi::OsStr::new(SETTINGS_LOCAL_OVERRIDE_FILE_NAME))
            && !AGENTS_INSTANCE_KEYS
                .iter()
                .any(|key| value.get(*key).is_some())
        {
            continue;
        }
        if let Some(reason) = duplicate_profile_document(&bytes, &participant) {
            captured.problems.push(RegisteredProfileProblem {
                path: participant.clone(),
                kind: RegisteredProfileProblemKind::DuplicateMember,
                reason,
                ordinary_participant: true,
            });
        }
        sources.push((participant.clone(), value));
        captured.documents.push((participant, Some(bytes)));
    }
    validate_registered_profile_capture(path, &sources, &mut captured);
    captured
}

pub(crate) fn validate_registered_profile_capture(
    path: &Path,
    sources: &[(PathBuf, Value)],
    captured: &mut RegisteredProfileCapture,
) {
    let sidecar_path = agents_instance_path(path);
    let sidecar = sources
        .iter()
        .find(|(source, _)| source == &sidecar_path)
        .and_then(|(_, value)| value.as_object());
    let main = sources
        .iter()
        .find(|(source, _)| source == path)
        .and_then(|(_, value)| value.as_object())
        .cloned()
        .unwrap_or_default();
    let protected = sources.iter().any(|(source, value)| {
        source.file_name() == Some(std::ffi::OsStr::new(SETTINGS_LOCAL_OVERRIDE_FILE_NAME))
            && AGENTS_INSTANCE_KEYS
                .iter()
                .any(|key| value.get(*key).is_some())
    });
    for (participant, value) in sources {
        let mut profile_source = value.clone();
        // Shadowed rows/groups cannot contribute active collisions. Structural
        // map/quarantine validation still covers each original participant.
        if participant == path
            && sidecar.is_some_and(|map| {
                map.contains_key("agents") || map.contains_key("codingAgentProfiles")
            })
            || physical_profile_rows(&profile_source, true).is_err()
        {
            if let Some(object) = profile_source.as_object_mut() {
                object.remove("agents");
            }
        }
        if let Err(reason) = physical_profile_sets(&profile_source, true) {
            captured.problems.push(RegisteredProfileProblem {
                path: participant.clone(),
                kind: RegisteredProfileProblemKind::ProfileSemantic,
                reason: format!("{}: {reason}", participant.display()),
                ordinary_participant: true,
            });
        }
    }
    if !protected {
        let selected = selected_registered_profile_owner(&main, sidecar, true);
        if physical_profile_rows(&selected, true).is_ok() {
            if let Err(reason) = physical_profile_sets(&selected, true) {
                captured.problems.push(RegisteredProfileProblem {
                    path: path.to_path_buf(),
                    kind: RegisteredProfileProblemKind::ProfileSemantic,
                    reason: format!("{}: {reason}", path.display()),
                    ordinary_participant: true,
                });
            }
        }
    }
}

pub(crate) fn capture_registered_profile_gate(
    path: &Path,
) -> Result<Vec<RegisteredProfileDocument>, String> {
    let captured = collect_registered_profile_gate(path);
    match captured.ordinary_error() {
        Some(reason) => Err(reason),
        None => Ok(captured.participant_documents()),
    }
}

pub(crate) fn captured_registered_profile_object(
    captured: &[RegisteredProfileDocument],
    path: &Path,
) -> Result<Option<Map<String, Value>>, String> {
    let (_, bytes) = captured
        .iter()
        .find(|(source, _)| source == path)
        .ok_or_else(|| format!("staleProfileSource: {}", path.display()))?;
    bytes
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn recheck_registered_profile_gate(
    captured: &[RegisteredProfileDocument],
) -> Result<(), String> {
    for (path, before) in captured {
        let current = match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if &current != before {
            return Err(format!("staleProfileSource: {}", path.display()));
        }
    }
    Ok(())
}

pub(crate) fn registered_profile_raw_gate(path: &Path) -> Result<(), String> {
    let captured = capture_registered_profile_gate(path)?;
    recheck_registered_profile_gate(&captured)
}

/// #1757 - the Codex "Hooks need review" startup dialog. One literal, two consumers:
/// `apply_issue_1757_migration` (installs whose `blockingMenus` array was already materialized
/// against the older default) and test T1 `embedded_content_matches_the_rust_literals`, which
/// pins the embedded `settings-blocking-menus.json` to this constant so the two cannot drift.
pub(crate) const CODEX_HOOKS_REVIEW_PATTERN: &str = r"^[^A-Za-z0-9]*Hooks need review\b";

pub(crate) fn codex_hooks_review_menu() -> BlockingMenuConfig {
    BlockingMenuConfig {
        pattern: CODEX_HOOKS_REVIEW_PATTERN.to_string(),
        notification: "codex is waiting for you to answer the hooks-review menu in this terminal"
            .to_string(),
        enabled: true,
        captured_against: Some("codex 0.153.2 / Windows".to_string()),
    }
}

/// #1905 - the patterns AC ships, embedded at build time. The config-dir copy is a
/// materialization of this content; the runtime evaluates this constant, never the disk file.
pub(crate) const EMBEDDED_BLOCKING_MENUS_JSON: &str =
    include_str!("../../../resources/blocking-menus/settings-blocking-menus.json");

pub const BLOCKING_MENUS_SCHEMA_VERSION: u32 = 1;

/// #1925 (D2) - the published file: one pinned literal with the source ref `main` and a
/// `v1` segment that must equal `BLOCKING_MENUS_SCHEMA_VERSION`; a test pins both.
pub(crate) const REMOTE_BLOCKING_MENUS_URL: &str =
    "https://raw.githubusercontent.com/mblua/AgentsCommander/main/remote-resources/blocking-menus/v\
        1/settings-blocking-menus.json";
/// #1925 (D2) - the ref the URL pins; the cached note names it.
pub(crate) const REMOTE_BLOCKING_MENUS_SOURCE_REF: &str = "main";
/// #1925 (D5) - hard cap on the downloaded body, checked while reading and on acceptance.
pub(crate) const REMOTE_BLOCKING_MENUS_MAX_BYTES: usize = 64 * 1024;

/// #1925 (D6) - hard ceilings for the downloaded layer: 200 entries in total across
/// every `byCommand` array, a pattern of 512 bytes and a notification of 200 bytes.
pub(crate) const REMOTE_BLOCKING_MENUS_MAX_ENTRIES: usize = 200;
pub(crate) const REMOTE_PATTERN_MAX_BYTES: usize = 512;
pub(crate) const REMOTE_NOTIFICATION_MAX_BYTES: usize = 200;

/// #1925 (D6) - compile ceiling for one downloaded pattern. The menu guard keeps one
/// compiled copy per accepted pattern, so an accepted file adds at most
/// 200 x 256 KiB = 50 MiB of compiled programs, plus each regex's lazy-DFA cache at
/// the `regex` crate default. Reaching that ceiling needs repo-write access; the real
/// published file compiles to a few KiB.
pub(crate) const REMOTE_PATTERN_REGEX_SIZE_LIMIT: usize = 256 * 1024;

/// #1925 (D7) - every accepted pattern must leave these rows alone. Matching is
/// `Regex::is_match`, unanchored, the way the guard matches, so a catch-all pattern
/// that would silently defer every injection is rejected before it reaches the guard.
/// Menu-like rows (`1. Yes`) are deliberately absent: a legitimate trust-menu pattern
/// may match them.
pub(crate) const REMOTE_BLOCKING_MENUS_BENIGN_ROWS: &[&str] = &[
    "",
    "   ",
    "$ ",
    "> ",
    r"PS C:\Users\dev\project> ",
    "dev@host:~/project$ ",
    "The quick brown fox jumps over the lazy dog.",
    "    at main (src/index.ts:10:5)",
    "thread 'main' panicked at src/main.rs:2:5:",
    "Traceback (most recent call last):",
    "error: could not compile `app` (bin \"app\") due to 1 previous error",
    "\u{256d}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256e}",
    "\u{2502}                \u{2502}",
];

pub(crate) fn default_blocking_menus_schema_version() -> u32 {
    BLOCKING_MENUS_SCHEMA_VERSION
}

/// #1905 - one blocking-menus file; the shipped file and the `.local` file share this shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockingMenusFile {
    #[serde(default = "default_blocking_menus_schema_version")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub by_command: BTreeMap<String, Vec<BlockingMenuEntry>>,
    #[serde(default)]
    pub by_agent: BTreeMap<String, Vec<BlockingMenuEntry>>,
}

impl Default for BlockingMenusFile {
    fn default() -> Self {
        Self {
            schema_version: BLOCKING_MENUS_SCHEMA_VERSION,
            note: None,
            by_command: BTreeMap::new(),
            by_agent: BTreeMap::new(),
        }
    }
}

/// #1905 (D6) - the one parser both files go through; the migration validates against it too.
/// The `Value` step rejects a JSON array, which serde would otherwise decode as an empty file.
pub(crate) fn parse_blocking_menus_file(contents: &str) -> Result<BlockingMenusFile, String> {
    let value =
        serde_json::from_str::<Value>(contents).map_err(|e| format!("does not parse: {e}"))?;
    if !value.is_object() {
        return Err("is not a JSON object".to_string());
    }
    let file = serde_json::from_value::<BlockingMenusFile>(value)
        .map_err(|e| format!("does not parse: {e}"))?;
    if file.schema_version != BLOCKING_MENUS_SCHEMA_VERSION {
        return Err(format!(
            "has schemaVersion {}; only {} is supported",
            file.schema_version, BLOCKING_MENUS_SCHEMA_VERSION
        ));
    }
    Ok(file)
}

/// #1925 (D4 to D7) - the whole-file gate for the downloaded cache. The first failed
/// check rejects the whole file, never one entry; `byAgent` is rejected because the
/// remote layer is `byCommand`-only. Checked on arrival and again at every read.
pub(crate) fn validate_remote_blocking_menus_file(
    contents: &str,
) -> Result<BlockingMenusFile, String> {
    let file = parse_blocking_menus_file(contents)?;
    if !file.by_agent.is_empty() {
        return Err("byAgent must be empty".to_string());
    }
    let total: usize = file.by_command.values().map(Vec::len).sum();
    if total > REMOTE_BLOCKING_MENUS_MAX_ENTRIES {
        return Err(format!(
            "holds {total} entries; more than {REMOTE_BLOCKING_MENUS_MAX_ENTRIES} entries are not \
                allowed"
        ));
    }
    for (stem, entries) in &file.by_command {
        for (index, entry) in entries.iter().enumerate() {
            let Some(config) = entry.valid() else {
                return Err(format!(
                    "entry {stem}[{index}]: is not a valid blocking-menu entry"
                ));
            };
            if config.pattern.len() > REMOTE_PATTERN_MAX_BYTES {
                return Err(format!(
                    "entry {stem}[{index}]: pattern longer than {REMOTE_PATTERN_MAX_BYTES} bytes"
                ));
            }
            if config.notification.len() > REMOTE_NOTIFICATION_MAX_BYTES {
                return Err(format!(
                    "entry {stem}[{index}]: notification longer than \
                        {REMOTE_NOTIFICATION_MAX_BYTES} bytes"
                ));
            }
            if config.notification.chars().any(char::is_control) {
                return Err(format!(
                    "entry {stem}[{index}]: notification contains a control character"
                ));
            }
            let regex = match regex::RegexBuilder::new(&config.pattern)
                .size_limit(REMOTE_PATTERN_REGEX_SIZE_LIMIT)
                .build()
            {
                Ok(regex) => regex,
                Err(e) => {
                    return Err(format!(
                        "entry {stem}[{index}]: pattern does not compile: {e}"
                    ));
                }
            };
            if regex.is_match("") {
                return Err(format!(
                    "entry {stem}[{index}]: pattern matches the empty string"
                ));
            }
            if REMOTE_BLOCKING_MENUS_BENIGN_ROWS
                .iter()
                .any(|row| regex.is_match(row))
            {
                return Err(format!(
                    "entry {stem}[{index}]: pattern matches a benign row"
                ));
            }
        }
    }
    Ok(file)
}

/// #1925 (D5) - accept a response only when the status, the byte size and the encoding
/// are all right; the body then goes through the same whole-file validator as the cache.
pub(crate) fn accept_remote_blocking_menus_response(
    status: u16,
    body: &[u8],
) -> Result<BlockingMenusFile, String> {
    if status != 200 {
        return Err(format!("HTTP status {status}"));
    }
    if body.len() > REMOTE_BLOCKING_MENUS_MAX_BYTES {
        return Err(format!(
            "body larger than {REMOTE_BLOCKING_MENUS_MAX_BYTES} bytes"
        ));
    }
    let text = std::str::from_utf8(body).map_err(|_| "body is not UTF-8".to_string())?;
    validate_remote_blocking_menus_file(text)
}

/// A parse failure is a build defect T1 catches; production logs once and serves an empty file.
pub fn shipped_blocking_menus() -> &'static BlockingMenusFile {
    static SHIPPED: OnceLock<BlockingMenusFile> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        parse_blocking_menus_file(EMBEDDED_BLOCKING_MENUS_JSON).unwrap_or_else(|e| {
            log::error!("[blocking-menus] embedded settings-blocking-menus.json {e}");
            BlockingMenusFile::default()
        })
    })
}

/// #1646 / #1647 / #1905 - the shipped patterns for a command, by executable stem.
pub fn default_blocking_menus_for_command(command: &str) -> Vec<BlockingMenuEntry> {
    let Some(stem) = crate::config::coding_agents_catalog::command_executable_basename(command)
    else {
        return Vec::new();
    };
    shipped_blocking_menus()
        .by_command
        .get(&stem)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn blocking_menus_shipped_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_SHIPPED_FILE_NAME)
}

pub(crate) fn blocking_menus_local_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_LOCAL_FILE_NAME)
}

pub(crate) fn blocking_menus_remote_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_REMOTE_FILE_NAME)
}

/// Pretty JSON plus one trailing newline: the only byte shape AC writes for both files.
pub(crate) fn pretty_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// #1905 (D5) - make the on-disk shipped file equal to the embedded content. True when written.
pub(crate) fn refresh_shipped_blocking_menus_file(settings_path: &Path) -> bool {
    let path = blocking_menus_shipped_path(settings_path);
    let Ok(canonical) = pretty_json_bytes(shipped_blocking_menus()) else {
        return false;
    };
    if matches!(std::fs::read(&path), Ok(existing) if existing == canonical) {
        return false;
    }
    match crate::config::local_config_io::write_file_atomic(&path, &canonical) {
        Ok(()) => {
            log::info!(
                "[blocking-menus] wrote the shipped patterns to {}",
                path.display()
            );
            true
        }
        Err(e) => {
            log::error!("[blocking-menus] could not write {}: {e}", path.display());
            false
        }
    }
}

/// #1905 (D6) - the user layer. Every rejection logs once and yields an empty layer.
pub(crate) fn load_local_blocking_menus_file(settings_path: &Path) -> BlockingMenusFile {
    let path = blocking_menus_local_path(settings_path);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BlockingMenusFile::default(),
        Err(e) => {
            log::error!("[blocking-menus] could not read {}: {e}", path.display());
            return BlockingMenusFile::default();
        }
    };
    match parse_blocking_menus_file(&contents) {
        Ok(file) => file,
        Err(e) => {
            log::error!("[blocking-menus] {} {e}; ignoring the file", path.display());
            BlockingMenusFile::default()
        }
    }
}

/// #1925 (D4, D5) - the downloaded layer, re-validated at every read because the cache
/// sits in a directory the local user can edit. A rejected or unreadable cache yields an
/// empty layer and one warning; a missing cache is silent.
pub(crate) fn load_remote_blocking_menus_file(settings_path: &Path) -> BlockingMenusFile {
    let path = blocking_menus_remote_path(settings_path);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BlockingMenusFile::default(),
        Err(e) => {
            log::warn!("[blocking-menus] could not read {}: {e}", path.display());
            return BlockingMenusFile::default();
        }
    };
    match validate_remote_blocking_menus_file(&contents) {
        Ok(file) => file,
        Err(e) => {
            log::warn!(
                "[blocking-menus] {} {e}; ignoring the downloaded patterns",
                path.display()
            );
            BlockingMenusFile::default()
        }
    }
}

/// #1925 (D7, D8) - replace the downloaded cache only after validation passed. The note
/// records where and when the bytes came from; the next accepted download replaces it and a
/// file that fails validation at the next start is ignored.
pub(crate) fn write_remote_blocking_menus_cache(
    settings_path: &Path,
    mut file: BlockingMenusFile,
    source_url: &str,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    file.note = Some(format!(
        "Downloaded by AgentsCommander from {source_url} (source ref \
            {REMOTE_BLOCKING_MENUS_SOURCE_REF}) at {}. Replaced by the next accepted download and \
            ignored at start if it fails validation; put your own patterns in {}.",
        fetched_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        crate::config::instance_artifacts::BLOCKING_MENUS_LOCAL_FILE_NAME
    ));
    let bytes = pretty_json_bytes(&file).map_err(|e| e.to_string())?;
    crate::config::local_config_io::write_file_atomic(
        &blocking_menus_remote_path(settings_path),
        &bytes,
    )
}

/// #1925 (D4) - the throttle stamp lives next to `settings.json`.
pub(crate) fn blocking_menus_remote_check_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteBlockingMenusCheckStamp {
    pub(crate) last_checked_at: chrono::DateTime<chrono::Utc>,
}

/// #1925 (D4) - missing, unreadable and malformed stamps all mean "no throttle": the
/// caller treats `None` as due.
pub(crate) fn read_remote_blocking_menus_check_stamp(
    settings_path: &Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let bytes = std::fs::read(blocking_menus_remote_check_path(settings_path)).ok()?;
    let stamp: RemoteBlockingMenusCheckStamp = serde_json::from_slice(&bytes).ok()?;
    Some(stamp.last_checked_at)
}

/// #1925 (D4) - written after every attempt that passed the due check, accepted or not.
pub(crate) fn write_remote_blocking_menus_check_stamp(
    settings_path: &Path,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let stamp = RemoteBlockingMenusCheckStamp {
        last_checked_at: at,
    };
    let bytes = pretty_json_bytes(&stamp).map_err(|e| e.to_string())?;
    crate::config::local_config_io::write_file_atomic(
        &blocking_menus_remote_check_path(settings_path),
        &bytes,
    )
}

/// #1905 (D3, D10) - what the menu guard evaluates. Built once per process; no file watcher.
#[derive(Debug, Clone)]
pub struct BlockingMenusStore {
    pub(crate) shipped: &'static BlockingMenusFile,
    pub(crate) remote: BlockingMenusFile,
    pub(crate) local: BlockingMenusFile,
}

impl BlockingMenusStore {
    pub fn shipped_only() -> Self {
        Self::with_local(BlockingMenusFile::default())
    }

    pub fn with_local(local: BlockingMenusFile) -> Self {
        Self::with_layers(BlockingMenusFile::default(), local)
    }

    /// #1925 - the downloaded cache plus the user layer; the argument order matches `resolve`.
    pub(crate) fn with_layers(remote: BlockingMenusFile, local: BlockingMenusFile) -> Self {
        Self {
            shipped: shipped_blocking_menus(),
            remote,
            local,
        }
    }

    /// GUI startup entry point: refresh the shipped file, then read the user layer.
    pub fn load_from_config_dir() -> Self {
        match settings_path() {
            Some(path) => Self::load_from_settings_path(&path),
            None => Self::shipped_only(),
        }
    }

    pub(crate) fn load_from_settings_path(settings_path: &Path) -> Self {
        refresh_shipped_blocking_menus_file(settings_path);
        Self::with_layers(
            load_remote_blocking_menus_file(settings_path),
            load_local_blocking_menus_file(settings_path),
        )
    }

    /// D3 layers 1 to 5: local.byAgent, local.byCommand, remote.byCommand, then the shipped
    /// patterns. Layer 0 is the legacy agent array, handled by `resolve_for`.
    pub fn resolve(&self, agent_id: &str, command: &str) -> Vec<BlockingMenuEntry> {
        if let Some(entries) = self.local.by_agent.get(agent_id) {
            return entries.clone();
        }
        let Some(stem) = crate::config::coding_agents_catalog::command_executable_basename(command)
        else {
            return Vec::new();
        };
        if let Some(entries) = self.local.by_command.get(&stem) {
            return entries.clone();
        }
        if let Some(entries) = self.remote.by_command.get(&stem) {
            return entries.clone();
        }
        self.shipped
            .by_command
            .get(&stem)
            .cloned()
            .unwrap_or_default()
    }

    /// D3 layer 0 first: an array still on the agent (settings.json not yet migrated, or an
    /// overlay-owned agents array) is what applied before #1905 and keeps applying unchanged.
    pub fn resolve_for(&self, agent: &AgentConfig) -> Vec<BlockingMenuEntry> {
        match &agent.blocking_menus {
            Some(entries) => entries.clone(),
            None => self.resolve(&agent.id, &agent.command),
        }
    }
}

/// #2133 - the embedded per-agent help, the shipped layer. P1 owns the file.
pub(crate) const EMBEDDED_AGENT_HELP_JSON: &str =
    include_str!("../../../resources/agent-help/agent-help.json");

pub const AGENT_HELP_SCHEMA_VERSION: u32 = 1;

/// #2133 - a link at the end of a tip.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpLink {
    pub label: String,
    pub url: String,
}

/// #2133 - one tip: a title, a body and an optional link.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpTip {
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<AgentHelpLink>,
}

/// #2133 - the help for one command stem, one agent, or the general block.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_example: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub tips: Vec<AgentHelpTip>,
}

/// #2133 - one agent-help file; the embedded, local and remote layers share this shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpFile {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general: Option<AgentHelpEntry>,
    #[serde(default)]
    pub by_command: BTreeMap<String, AgentHelpEntry>,
    #[serde(default)]
    pub by_agent: BTreeMap<String, AgentHelpEntry>,
}

impl Default for AgentHelpFile {
    fn default() -> Self {
        Self {
            schema_version: AGENT_HELP_SCHEMA_VERSION,
            note: None,
            general: None,
            by_command: BTreeMap::new(),
            by_agent: BTreeMap::new(),
        }
    }
}

/// #2133 - an absolute `https` URL with a non-empty host. No URL crate: the prefix and the
/// host segment are all the help links need.
pub(crate) fn is_https_url(value: &str) -> bool {
    value
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .is_some_and(|host| !host.is_empty())
}

pub(crate) fn check_agent_help_entry_urls(key: &str, entry: &AgentHelpEntry) -> Result<(), String> {
    if let Some(docs_url) = &entry.docs_url {
        if !is_https_url(docs_url) {
            return Err(format!("entry {key}: docsUrl is not an https URL"));
        }
    }
    for (index, tip) in entry.tips.iter().enumerate() {
        if let Some(link) = &tip.link {
            if !is_https_url(&link.url) {
                return Err(format!(
                    "entry {key} tip {index}: link url is not an https URL"
                ));
            }
        }
    }
    Ok(())
}

/// #2133 (D10) - the one parser all three layers go through, shaped like
/// `parse_blocking_menus_file`. The `Value` step rejects a JSON array, which serde would
/// otherwise decode as an empty file.
pub(crate) fn parse_agent_help_file(contents: &str) -> Result<AgentHelpFile, String> {
    let value =
        serde_json::from_str::<Value>(contents).map_err(|e| format!("does not parse: {e}"))?;
    if !value.is_object() {
        return Err("is not a JSON object".to_string());
    }
    let file = serde_json::from_value::<AgentHelpFile>(value)
        .map_err(|e| format!("does not parse: {e}"))?;
    if file.schema_version != AGENT_HELP_SCHEMA_VERSION {
        return Err(format!(
            "has schemaVersion {}; only {} is supported",
            file.schema_version, AGENT_HELP_SCHEMA_VERSION
        ));
    }
    if let Some(general) = &file.general {
        check_agent_help_entry_urls("general", general)?;
    }
    for (key, entry) in file.by_command.iter().chain(file.by_agent.iter()) {
        check_agent_help_entry_urls(key, entry)?;
    }
    Ok(file)
}

/// #2133 - the operator-owned overlay lives next to `settings.json`.
pub(crate) fn agent_help_local_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENT_HELP_LOCAL_FILE_NAME)
}

/// #2133 (D7) - the user layer, no caps. A missing file is silent; any other rejection logs
/// once and yields an empty layer plus the reason, which P4 hands to the UI.
pub(crate) fn load_local_agent_help_file(settings_path: &Path) -> (AgentHelpFile, Option<String>) {
    let path = agent_help_local_path(settings_path);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (AgentHelpFile::default(), None)
        }
        Err(e) => {
            log::warn!("[agent-help] could not read {}: {e}", path.display());
            return (
                AgentHelpFile::default(),
                Some(format!("could not read {}: {e}", path.display())),
            );
        }
    };
    match parse_agent_help_file(&contents) {
        Ok(file) => (file, None),
        Err(e) => {
            log::warn!("[agent-help] {} {e}; ignoring the file", path.display());
            (
                AgentHelpFile::default(),
                Some(format!("{} {e}", path.display())),
            )
        }
    }
}

/// #2133 - the AC-owned shipped copy lives next to `settings.json`. Only the refresh writes it;
/// nothing reads it back (the #1905 rule): the runtime uses `shipped_agent_help()`.
pub(crate) fn agent_help_shipped_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENT_HELP_SHIPPED_FILE_NAME)
}

/// #2133 - make the on-disk shipped file equal to the canonical form of the embedded content.
/// True when written. A hand edit is overwritten at the next start: the file is AC-owned.
/// Never fails: a write error is logged and startup continues on the embedded constant.
pub(crate) fn refresh_shipped_agent_help_file(settings_path: &Path) -> bool {
    refresh_shipped_agent_help_file_with_publish(
        settings_path,
        &crate::config::local_config_io::publish_temp_config,
    )
}

pub(crate) fn refresh_shipped_agent_help_file_with_publish(
    settings_path: &Path,
    publish: crate::config::local_config_io::ConfigPublisher<'_>,
) -> bool {
    let path = agent_help_shipped_path(settings_path);
    let Ok(canonical) = pretty_json_bytes(shipped_agent_help()) else {
        return false;
    };
    if matches!(std::fs::read(&path), Ok(existing) if existing == canonical) {
        return false;
    }
    match crate::config::local_config_io::write_file_atomic_with_publish(&path, &canonical, publish)
    {
        Ok(()) => {
            log::info!("[agent-help] wrote the shipped help to {}", path.display());
            true
        }
        Err(e) => {
            log::error!("[agent-help] could not write {}: {e}", path.display());
            false
        }
    }
}

/// #2133 - the startup entry point; the only agent-help function that resolves the config dir.
pub fn refresh_shipped_agent_help_from_config_dir() {
    match settings_path() {
        Some(path) => {
            refresh_shipped_agent_help_file(&path);
        }
        None => log::debug!("[agent-help] no config dir; the shipped help is not written"),
    }
}

/// #2133 - the git ref the published per-agent help is served from; named in every
/// cache note. The URL literal that embeds it is P7's, with the SERVED-PATHS row.
pub(crate) const REMOTE_AGENT_HELP_SOURCE_REF: &str = "main";

/// #2133 - the published file: one pinned literal whose ref segment must equal
/// `REMOTE_AGENT_HELP_SOURCE_REF` (P6's) and whose `v1` segment must equal
/// `AGENT_HELP_SCHEMA_VERSION`; T1 pins both.
pub(crate) const REMOTE_AGENT_HELP_URL: &str =
    "https://raw.githubusercontent.com/mblua/AgentsCommander/main/remote-resources/agent-help/v1/ag\
        ent-help.json";

/// #2133 (D4) - hard caps on the downloaded layer. Every cap counts UTF-8 bytes
/// (`str::len()`), not characters; TypeScript's `.length` counts UTF-16 code units instead.
pub(crate) const REMOTE_AGENT_HELP_MAX_BYTES: usize = 64 * 1024;
pub(crate) const REMOTE_AGENT_HELP_MAX_ENTRIES: usize = 40;
pub(crate) const REMOTE_AGENT_HELP_LABEL_MAX_BYTES: usize = 80;
pub(crate) const REMOTE_AGENT_HELP_PARAMS_MAX_BYTES: usize = 256;
pub(crate) const REMOTE_AGENT_HELP_URL_MAX_BYTES: usize = 512;
pub(crate) const REMOTE_AGENT_HELP_MAX_TIPS: usize = 12;
pub(crate) const REMOTE_AGENT_HELP_TITLE_MAX_BYTES: usize = 120;
pub(crate) const REMOTE_AGENT_HELP_BODY_MAX_BYTES: usize = 1200;

/// UTF-8 bytes, as every remote cap.
pub(crate) fn check_remote_agent_help_cap(
    at: &str,
    field: &str,
    value: &str,
    cap: usize,
) -> Result<(), String> {
    if value.len() > cap {
        return Err(format!(
            "{at}: {field} is {} bytes; more than {cap} bytes are not allowed",
            value.len()
        ));
    }
    Ok(())
}

pub(crate) fn check_remote_agent_help_entry(
    key: &str,
    entry: &AgentHelpEntry,
) -> Result<(), String> {
    let at = format!("entry {key}");
    let optional = [
        ("label", &entry.label, REMOTE_AGENT_HELP_LABEL_MAX_BYTES),
        (
            "paramsExample",
            &entry.params_example,
            REMOTE_AGENT_HELP_PARAMS_MAX_BYTES,
        ),
        ("docsUrl", &entry.docs_url, REMOTE_AGENT_HELP_URL_MAX_BYTES),
    ];
    for (field, value, cap) in optional {
        if let Some(value) = value {
            check_remote_agent_help_cap(&at, field, value, cap)?;
        }
    }
    if entry.tips.len() > REMOTE_AGENT_HELP_MAX_TIPS {
        return Err(format!(
            "{at}: holds {} tips; more than {REMOTE_AGENT_HELP_MAX_TIPS} are not allowed",
            entry.tips.len()
        ));
    }
    for (index, tip) in entry.tips.iter().enumerate() {
        let at = format!("entry {key} tip {index}");
        check_remote_agent_help_cap(&at, "title", &tip.title, REMOTE_AGENT_HELP_TITLE_MAX_BYTES)?;
        check_remote_agent_help_cap(&at, "body", &tip.body, REMOTE_AGENT_HELP_BODY_MAX_BYTES)?;
        if let Some(link) = &tip.link {
            check_remote_agent_help_cap(
                &at,
                "link url",
                &link.url,
                REMOTE_AGENT_HELP_URL_MAX_BYTES,
            )?;
        }
    }
    Ok(())
}

/// #2133 (D4, D10) - the whole-file gate for the downloaded layer, shaped like
/// `validate_remote_blocking_menus_file`. The first failed check rejects the whole file,
/// never one entry; `byAgent` is rejected because the remote layer is `byCommand`-only.
pub(crate) fn validate_remote_agent_help_file(contents: &str) -> Result<AgentHelpFile, String> {
    let file = parse_agent_help_file(contents)?;
    if !file.by_agent.is_empty() {
        return Err("byAgent must be empty".to_string());
    }
    if file.by_command.len() > REMOTE_AGENT_HELP_MAX_ENTRIES {
        return Err(format!(
            "holds {} entries; more than {REMOTE_AGENT_HELP_MAX_ENTRIES} entries are not allowed",
            file.by_command.len()
        ));
    }
    if let Some(general) = &file.general {
        check_remote_agent_help_entry("general", general)?;
    }
    for (key, entry) in &file.by_command {
        check_remote_agent_help_entry(key, entry)?;
    }
    Ok(file)
}

/// #2133 - the response gate: status, body size and UTF-8 before the whole-file validator.
pub(crate) fn accept_remote_agent_help_response(
    status: u16,
    body: &[u8],
) -> Result<AgentHelpFile, String> {
    if status != 200 {
        return Err(format!("HTTP status {status}"));
    }
    if body.len() > REMOTE_AGENT_HELP_MAX_BYTES {
        return Err(format!(
            "body larger than {REMOTE_AGENT_HELP_MAX_BYTES} bytes"
        ));
    }
    let text = std::str::from_utf8(body).map_err(|_| "body is not UTF-8".to_string())?;
    validate_remote_agent_help_file(text)
}

pub(crate) fn agent_help_remote_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENT_HELP_REMOTE_FILE_NAME)
}

/// #2133 (D4) - the downloaded layer, re-validated at every read because the cache sits in a
/// directory the local user can edit. A rejected or unreadable cache yields an empty layer and
/// one warning; a missing cache is silent.
pub(crate) fn load_remote_agent_help_file(settings_path: &Path) -> AgentHelpFile {
    let path = agent_help_remote_path(settings_path);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AgentHelpFile::default(),
        Err(e) => {
            log::warn!("[agent-help] could not read {}: {e}", path.display());
            return AgentHelpFile::default();
        }
    };
    match validate_remote_agent_help_file(&contents) {
        Ok(file) => file,
        Err(e) => {
            log::warn!(
                "[agent-help] {} {e}; ignoring the downloaded help",
                path.display()
            );
            AgentHelpFile::default()
        }
    }
}

/// #2133 - replace the downloaded cache only after validation passed. The note records where
/// and when the bytes came from; `source_url` is the caller's, so this phase holds no URL.
pub(crate) fn write_remote_agent_help_cache(
    settings_path: &Path,
    mut file: AgentHelpFile,
    source_url: &str,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    file.note = Some(format!(
        "Downloaded by AgentsCommander from {source_url} (source ref \
            {REMOTE_AGENT_HELP_SOURCE_REF}) at {}. Replaced by the next accepted download and \
            ignored at start if it fails validation; put your own help in agent-help.local.json.",
        fetched_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
    let bytes = pretty_json_bytes(&file).map_err(|e| e.to_string())?;
    crate::config::local_config_io::write_file_atomic(
        &agent_help_remote_path(settings_path),
        &bytes,
    )
}

/// #2133 (D4) - the throttle stamp lives next to `settings.json`.
pub(crate) fn agent_help_remote_check_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENT_HELP_REMOTE_CHECK_FILE_NAME)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteAgentHelpCheckStamp {
    pub(crate) last_checked_at: chrono::DateTime<chrono::Utc>,
}

/// #2133 (D4) - missing, unreadable and malformed stamps all mean "no throttle": the caller
/// treats `None` as due.
pub(crate) fn read_remote_agent_help_check_stamp(
    settings_path: &Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let bytes = std::fs::read(agent_help_remote_check_path(settings_path)).ok()?;
    let stamp: RemoteAgentHelpCheckStamp = serde_json::from_slice(&bytes).ok()?;
    Some(stamp.last_checked_at)
}

/// #2133 (D4) - written after every attempt that passed the due check, accepted or not.
pub(crate) fn write_remote_agent_help_check_stamp(
    settings_path: &Path,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let stamp = RemoteAgentHelpCheckStamp {
        last_checked_at: at,
    };
    let bytes = pretty_json_bytes(&stamp).map_err(|e| e.to_string())?;
    crate::config::local_config_io::write_file_atomic(
        &agent_help_remote_check_path(settings_path),
        &bytes,
    )
}

/// #2133 - a parse failure is a build defect T1 catches; production logs once and serves an
/// empty file. The runtime reads this, never a disk copy.
pub fn shipped_agent_help() -> &'static AgentHelpFile {
    static SHIPPED: OnceLock<AgentHelpFile> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        parse_agent_help_file(EMBEDDED_AGENT_HELP_JSON).unwrap_or_else(|e| {
            log::error!("[agent-help] embedded agent-help.json {e}");
            AgentHelpFile::default()
        })
    })
}

/// #1757 - one-shot migration for installs whose Codex `blockingMenus` array was already
/// materialized against the pre-#1757 default, which is every install created before this
/// change. The legacy materialize step only filled `None` arrays, so those users would
/// otherwise never receive the hooks-review pattern.
///
/// Keyed by content, not by a marker field: the entry is appended only when no valid entry
/// already carries `CODEX_HOOKS_REVIEW_PATTERN`, so the function is idempotent across
/// restarts and adds no key to the settings schema. The check ignores `enabled`, so setting
/// `enabled: false` on the entry is a durable off switch; deleting it is not (#1757 D2).
///
/// Extends an ACTIVE set, never activates an inactive one: it only fills arrays that
/// already exist (never `None`), and `Some(vec![])` is the field's documented
/// "explicitly disabled" state.
///
/// #1737 (D7c): an overlay that owns `agents` supplies the whole array and `restore_base`
/// writes the base array back on save, so appending here would be discarded on write and
/// would overwrite the operator's in-memory array meanwhile. Owning `agents` therefore
/// suppresses the migration, which then runs correctly the first time the overlay is removed.
///
/// Since #1905 this runs only as the second step of `export_blocking_menus_to_local_file`.
///
/// Returns true when any agent's array changed.
pub fn apply_issue_1757_migration(settings: &mut AppSettings) -> bool {
    if settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_AGENTS)
    {
        return false;
    }
    let mut changed = false;
    for agent in &mut settings.agents {
        if crate::config::coding_agents_catalog::command_executable_basename(&agent.command)
            .as_deref()
            != Some("codex")
        {
            continue;
        }
        let Some(entries) = agent.blocking_menus.as_mut() else {
            continue;
        };
        if entries.is_empty() {
            continue;
        }
        if entries.iter().any(|entry| {
            entry
                .valid()
                .is_some_and(|c| c.pattern == CODEX_HOOKS_REVIEW_PATTERN)
        }) {
            continue;
        }
        entries.push(BlockingMenuEntry::Valid(codex_hooks_review_menu()));
        changed = true;
    }
    changed
}

/// Every failure path of the export: log, strip nothing. The loader may still save
/// settings.json for another migration's reason; the arrays survive that save unchanged.
pub(crate) fn abort_blocking_menus_export(local_path: &Path, why: &str) -> bool {
    log::error!(
        "[settings-migration] #1905 - {why} ({}); no blockingMenus array stripped",
        local_path.display()
    );
    false
}

/// #1905 (D7) - one-shot, state-keyed export of every `blockingMenus` array still in
/// `settings.json`. Writes the `.local` file before it strips anything; true means "save".
#[cfg(test)]
pub(crate) fn export_blocking_menus_to_local_file(
    settings: &mut AppSettings,
    settings_path: &Path,
) -> bool {
    export_blocking_menus_to_local_file_with_publish(
        settings,
        settings_path,
        &crate::config::local_config_io::publish_temp_config,
    )
}

#[cfg(test)]
pub(crate) fn export_blocking_menus_to_local_file_with_publish(
    settings: &mut AppSettings,
    settings_path: &Path,
    publish: crate::config::local_config_io::ConfigPublisher<'_>,
) -> bool {
    export_blocking_menus_with_startup(settings, settings_path, publish, None)
}

pub(crate) fn export_blocking_menus_with_startup(
    settings: &mut AppSettings,
    settings_path: &Path,
    publish: crate::config::local_config_io::ConfigPublisher<'_>,
    mut startup: Option<&mut StartupAgentRows>,
) -> bool {
    if let Err(reason) = registered_profile_raw_gate(settings_path) {
        log::warn!("{reason}");
        return false;
    }
    if settings.agents.iter().all(|a| a.blocking_menus.is_none()) {
        return false;
    }
    let local_path = blocking_menus_local_path(settings_path);
    if settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_AGENTS)
    {
        for agent in settings
            .agents
            .iter()
            .filter(|a| a.blocking_menus.is_some())
        {
            log::info!(
                "[settings-migration] #1905 - agent '{}' carries blockingMenus inside an \
                overlay-owned agents array; it keeps applying from there and is not exported; move \
                it to {} to use the new file",
                agent.id,
                BLOCKING_MENUS_LOCAL_FILE_NAME
            );
        }
        return false;
    }
    apply_issue_1757_migration(settings);
    if let Some(context) = startup.as_deref_mut() {
        context.expected = settings.agents.clone();
    }
    // One id, one array, one command: two agents sharing an id with different arrays cannot
    // both live under byAgent[id], and equal arrays under different commands would make the
    // pristine test below order-dependent, so the export aborts and layer 0 keeps serving both.
    let mut by_id: BTreeMap<&str, (&str, &Vec<BlockingMenuEntry>)> = BTreeMap::new();
    for agent in &settings.agents {
        let Some(entries) = agent.blocking_menus.as_ref() else {
            continue;
        };
        let current = (agent.command.as_str(), entries);
        if let Some(previous) = by_id.insert(agent.id.as_str(), current) {
            if previous != current {
                return abort_blocking_menus_export(
                    &local_path,
                    &format!(
                        "two agents share the id \
                    '{}' with different blockingMenus arrays or commands",
                        agent.id
                    ),
                );
            }
        }
    }
    let candidates: Vec<(String, Vec<BlockingMenuEntry>)> = by_id
        .iter()
        .filter(|(_, (command, entries))| **entries != default_blocking_menus_for_command(command))
        .map(|(id, (_, entries))| ((*id).to_string(), (*entries).clone()))
        .collect();
    let raw = match std::fs::read_to_string(&local_path) {
        Ok(contents) => Some(contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return abort_blocking_menus_export(
                &local_path,
                &format!("could not read the file: {e}"),
            )
        }
    };
    let mut root = match raw.as_deref() {
        None => Map::new(),
        Some(contents) => {
            // Schema parity: the runtime's own parser, on the complete file, before any mutation.
            if let Err(e) = parse_blocking_menus_file(contents) {
                return abort_blocking_menus_export(&local_path, &format!("the file {e}"));
            }
            match serde_json::from_str::<Value>(contents) {
                Ok(Value::Object(map)) => map,
                // Unreachable once the parser above rejected non-objects; kept as a guard.
                _ => {
                    return abort_blocking_menus_export(
                        &local_path,
                        "the file is not a JSON object",
                    )
                }
            }
        }
    };
    root.entry("schemaVersion")
        .or_insert_with(|| Value::from(BLOCKING_MENUS_SCHEMA_VERSION));
    let Some(by_agent) = root
        .entry("byAgent")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    else {
        return abort_blocking_menus_export(&local_path, "byAgent is not an object");
    };
    let mut inserted = 0usize;
    for (id, entries) in candidates {
        if by_agent.contains_key(&id) {
            log::info!(
                "[settings-migration] #1905 - {} already has byAgent['{id}']; keeping it",
                local_path.display()
            );
            continue;
        }
        match serde_json::to_value(&entries) {
            Ok(value) => {
                by_agent.insert(id, value);
                inserted += 1;
            }
            Err(e) => {
                return abort_blocking_menus_export(
                    &local_path,
                    &format!("could not serialize blockingMenus of '{id}': {e}"),
                )
            }
        }
    }
    if inserted > 0 {
        let bytes = match pretty_json_bytes(&Value::Object(root)) {
            Ok(bytes) => bytes,
            Err(e) => {
                return abort_blocking_menus_export(
                    &local_path,
                    &format!("could not serialize the file: {e}"),
                )
            }
        };
        // Schema parity again, on the exact bytes about to be published.
        if let Err(e) = std::str::from_utf8(&bytes)
            .map_err(|e| e.to_string())
            .and_then(parse_blocking_menus_file)
        {
            return abort_blocking_menus_export(&local_path, &format!("the merged file {e}"));
        }
        if let Err(e) = crate::config::local_config_io::write_file_atomic_with_publish(
            &local_path,
            &bytes,
            publish,
        ) {
            return abort_blocking_menus_export(
                &local_path,
                &format!("could not write the file: {e}"),
            );
        }
        log::info!(
            "[settings-migration] #1905 - exported {inserted} blockingMenus array(s) to {}",
            local_path.display()
        );
    }
    for agent in &mut settings.agents {
        agent.blocking_menus = None;
    }
    if let Some(context) = startup {
        context.expected = settings.agents.clone();
    }
    true
}

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

/// #2716 (B3, option C) - whether the agents file could be read on this load.
/// `Default` is "readable". Carried on `AppSettings` as a never-serialized field,
/// like `local_overlay_state`, so no save can write it and no client can forge it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum AgentsLayerState {
    #[default]
    Readable,
    Unreadable {
        path: PathBuf,
        reason: String,
    },
}

impl AgentsLayerState {
    /// The agents file's path while the layer is unreadable.
    pub(crate) fn unreadable_path(&self) -> Option<&Path> {
        match self {
            Self::Readable => None,
            Self::Unreadable { path, .. } => Some(path),
        }
    }
}

/// #2716 (B3, option C) - the CLI's answer: the list is unknown, not empty.
pub(crate) fn agents_unreadable_cli_error(path: &Path) -> String {
    format!(
        "Coding agents are unavailable: AgentsCommander could not read {}. Fix or delete that file \
            and restart.",
        path.display()
    )
}

/// #2716 (B3, option C) - notice 1: the startup dialog, raised by the error log.
pub(crate) fn agents_unreadable_startup_notice(path: &Path) -> String {
    format!(
        "Coding agents are unavailable: AgentsCommander could not read {}. Agent sessions will not \
            start until this file is fixed or deleted. Every other setting is unchanged. While \
            this file is unreadable, changes to your sessions are not saved.",
        path.display()
    )
}

/// #2716 (B3, option C) - notice 3: the error every spawn entry point returns.
pub(crate) fn agents_unreadable_session_error(path: &Path) -> String {
    format!(
        "Cannot start this coding agent: AgentsCommander could not read {}. Fix or delete that \
            file and restart.",
        path.display()
    )
}

/// #2716 (B3, option C) - the directories whose agents file could not be read,
/// keyed by the settings file's parent. `save_sessions_to_dir` reads it with the
/// directory it already receives, so a test in its own tempdir is isolated.
pub(crate) fn agents_unreadable_dirs() -> &'static std::sync::Mutex<HashSet<PathBuf>> {
    static DIRS: OnceLock<std::sync::Mutex<HashSet<PathBuf>>> = OnceLock::new();
    DIRS.get_or_init(Default::default)
}

// Dormant until the P05 carrier publication consumer is wired.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentsPublicationLockError {
    WouldBlock,
    Poisoned,
}

/// Runs a synchronous publication step while the suppression registry is locked.
/// The callback receives the unreadable flag and must not await or reacquire
/// this registry. Contention and poison never authorize publication.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn try_with_agents_layer_publication_state<R>(
    dir: &Path,
    f: impl FnOnce(bool) -> R,
) -> Result<R, AgentsPublicationLockError> {
    try_with_agents_layer_publication_state_in(agents_unreadable_dirs(), dir, f)
}

// A local registry lets poison tests exercise this same path without poisoning
// the process-wide registry used by other settings tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn try_with_agents_layer_publication_state_in<R>(
    registry: &std::sync::Mutex<HashSet<PathBuf>>,
    dir: &Path,
    f: impl FnOnce(bool) -> R,
) -> Result<R, AgentsPublicationLockError> {
    let dirs = registry.try_lock().map_err(|error| match error {
        std::sync::TryLockError::WouldBlock => AgentsPublicationLockError::WouldBlock,
        std::sync::TryLockError::Poisoned(_) => AgentsPublicationLockError::Poisoned,
    })?;
    Ok(f(dirs.contains(dir)))
}

/// #2716 (B3, option C) - true while the agents file beside `dir`'s settings file
/// could not be read by the last full load of it.
pub(crate) fn agents_layer_unreadable_for_dir(dir: &Path) -> bool {
    agents_unreadable_dirs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(dir)
}

/// #2716 (B3, option C) - the one setter: the field and the registry change in
/// the same statement, so they cannot diverge.
pub(crate) fn set_agents_layer_state(
    settings: &mut AppSettings,
    settings_path: &Path,
    state: AgentsLayerState,
) {
    if let Some(dir) = settings_path.parent() {
        let mut dirs = agents_unreadable_dirs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state {
            AgentsLayerState::Readable => dirs.remove(dir),
            AgentsLayerState::Unreadable { .. } => dirs.insert(dir.to_path_buf()),
        };
    }
    settings.agents_layer = state;
}

/// #2716 (B3, option C) - the agents layer's state from the file alone, for the
/// defaults fallback, where no merge result survives.
pub(crate) fn agents_layer_state_at(settings_path: &Path) -> AgentsLayerState {
    agents_layer_state_at_labeled(settings_path, settings_path)
}

pub(crate) fn agents_layer_state_at_labeled(
    io_path: &Path,
    settings_path: &Path,
) -> AgentsLayerState {
    let path = agents_instance_path(settings_path);
    match read_agents_instance_file_labeled(&agents_instance_path(io_path), &path) {
        Ok(_) => AgentsLayerState::Readable,
        Err(reason) => AgentsLayerState::Unreadable { path, reason },
    }
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

/// #2716 (B3) - puts the agents file's groups back into the just-written object
/// for the save's re-decode. Like the load merge, `[]` stands in when neither file
/// holds `agents`.
pub(crate) fn restore_agents_groups_for_redecode(written: &mut Value, groups: Map<String, Value>) {
    if let Value::Object(object) = written {
        object.extend(groups);
        object
            .entry(AGENTS_INSTANCE_KEYS[0])
            .or_insert_with(|| Value::Array(Vec::new()));
    }
}

/// Restore only the backend-owned carrier; ordinary profile edits still apply.
/// Absence in the physical base removes a forged client/overlay carrier.
pub(crate) fn restore_codex_home_carrier(
    out: &mut Map<String, Value>,
    base_profiles: Option<&Value>,
) {
    const KEY: &str = "codexHomeKeyByAgent";
    let carrier = base_profiles
        .and_then(Value::as_object)
        .and_then(|profiles| profiles.get(KEY));
    if carrier.is_some() {
        out.entry("codingAgentProfiles")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if let Some(profiles) = out
        .get_mut("codingAgentProfiles")
        .and_then(Value::as_object_mut)
    {
        match carrier {
            Some(value) => {
                profiles.insert(KEY.to_string(), value.clone());
            }
            None => {
                profiles.remove(KEY);
            }
        }
    }
}

// A missing physical identity image is not an absence proof. This local
// reference is admitted only by the locked operation's original raw capture.
// It is never installed as identity state or used as a publication candidate.
pub(crate) fn absent_main_overlay_comparison(
    settings: &AppSettings,
    path: &Path,
    physical_sidecar: &Option<Map<String, Value>>,
    initial_disk: &Option<Map<String, Value>>,
    captured: &[RegisteredProfileDocument],
) -> Result<Value, String> {
    let unavailable = || "overlayProfileComparisonUnavailable".to_owned();
    if initial_disk.is_some() || settings.local_overlay_state.rejection().is_some() {
        return Err(unavailable());
    }
    let main = captured
        .iter()
        .find(|(document, _)| document == path)
        .ok_or_else(unavailable)?;
    if main.1.is_some() {
        return Err(unavailable());
    }
    let sidecar_path = agents_instance_path(path);
    let sidecar = captured
        .iter()
        .find(|(document, _)| document == &sidecar_path)
        .ok_or_else(unavailable)?;
    let captured_sidecar: Option<Map<String, Value>> = sidecar
        .1
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| unavailable())?;
    if &captured_sidecar != physical_sidecar {
        return Err(unavailable());
    }
    let overlay_path = path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
    let bytes = captured
        .iter()
        .find(|(document, _)| document == &overlay_path)
        .and_then(|(_, bytes)| bytes.as_deref())
        .ok_or_else(unavailable)?;
    let mut reference = serde_json::to_value(AppSettings::default()).map_err(|_| unavailable())?;
    let overlay = LocalSettingsOverlay::from_overlay_bytes_paired(
        &mut reference,
        bytes,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    if overlay.rejection().is_some()
        || overlay.is_empty()
        || overlay.owned_paths() != settings.local_overlay_state.owned_paths()
        || overlay.dropped_keys() != settings.local_overlay_state.dropped_keys()
    {
        return Err(unavailable());
    }
    restore_codex_home_carrier(
        reference.as_object_mut().ok_or_else(unavailable)?,
        captured_sidecar
            .as_ref()
            .and_then(|sidecar| sidecar.get("codingAgentProfiles")),
    );
    serde_json::from_value::<AppSettings>(reference.clone()).map_err(|_| unavailable())?;
    Ok(reference)
}

pub(crate) fn validate_registered_overlay_profile_edits(
    settings: &AppSettings,
    profile_before: &Value,
    physical_sidecar: &Option<Map<String, Value>>,
    initial_disk: &Option<Map<String, Value>>,
    path: &Path,
    captured: &[RegisteredProfileDocument],
) -> Result<(), String> {
    let paths = settings.local_overlay_state.owned_paths();
    if !paths.iter().any(|path| {
        path.first()
            .is_some_and(|key| AGENTS_INSTANCE_KEYS.contains(&key.as_str()))
    }) {
        return Ok(());
    }
    let absence_reference = if settings.coding_agent_identity_physical.is_none() {
        Some(absent_main_overlay_comparison(
            settings,
            path,
            physical_sidecar,
            initial_disk,
            captured,
        )?)
    } else {
        None
    };
    let raw = settings
        .coding_agent_identity_physical
        .as_ref()
        .and_then(|raw| raw.as_object())
        .or_else(|| absence_reference.as_ref().and_then(Value::as_object))
        .ok_or("overlayProfileComparisonUnavailable")?;
    let mut previous = registered_profiles_for_read(profile_before);
    if let Some(rows) = physical_sidecar
        .as_ref()
        .and_then(|map| map.get("agents"))
        .or_else(|| initial_disk.as_ref().and_then(|map| map.get("agents")))
    {
        previous["agents"] = rows.clone();
    }
    settings
        .local_overlay_state
        .reapply_from(raw, previous.as_object_mut().ok_or("invalidSource")?);
    // These copies are comparison data only, never physical publication inputs.
    let normalize = |mut value: Value| -> Result<Value, String> {
        restore_codex_home_carrier(
            value.as_object_mut().ok_or("invalidSource")?,
            profile_before.get("codingAgentProfiles"),
        );
        let mut agents: Vec<AgentConfig> = value
            .get("agents")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        let mut profiles: CodingAgentProfilesConfig = value
            .get("codingAgentProfiles")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        finalize_agent_order(&mut agents);
        // Match only implicit empty-A repair; do not prune submitted values.
        profiles
            .profile_slots
            .entry("A".into())
            .or_insert_with(|| ProfileSlotConfig {
                label: String::new(),
            });
        for agent in &agents {
            profiles
                .profiles_by_agent
                .entry(agent.id.clone())
                .or_default()
                .entry("A".into())
                .or_insert_with(empty_profile_cell);
        }
        refresh_agent_id_hashes(&mut agents);
        refresh_pair_id_hashes(&agents, &mut profiles);
        for cell in profiles
            .profiles_by_agent
            .values_mut()
            .flat_map(|cells| cells.values_mut())
        {
            cell.id_hash = None;
            cell.id_hash_debug = None;
        }
        profiles.identity_by_agent.clear();
        let agents = agents
            .iter()
            .map(comparable_agent)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(serde_json::json!({"agents":agents,"codingAgentProfiles":profiles}))
    };
    let previous = normalize(previous)?;
    let incoming = normalize(serde_json::json!({
        "agents":&settings.agents,
        "codingAgentProfiles":&settings.coding_agent_profiles
    }))?;
    for path in paths {
        let Some(key) = path
            .first()
            .filter(|key| AGENTS_INSTANCE_KEYS.contains(&key.as_str()))
        else {
            continue;
        };
        let previous_value = path
            .iter()
            .try_fold(&previous, |value, segment| value.get(segment));
        let incoming_value = path
            .iter()
            .try_fold(&incoming, |value, segment| value.get(segment));
        if previous_value != incoming_value {
            return Err(format!("overlayOwnedProfileGroup: {key}"));
        }
    }
    Ok(())
}

pub(crate) fn retain_unknown_registered_agent_fields(
    profile_before: &Value,
    profile_incoming: &mut Value,
) -> Result<(), String> {
    // The raw physical row retains fields unknown to AgentConfig.
    let prior_rows = raw_entries(profile_before, "id");
    if let Some(rows) = profile_incoming
        .get_mut("agents")
        .and_then(Value::as_array_mut)
    {
        for row in rows {
            let Some(id) = row.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(prior) = prior_rows.get(id).and_then(Value::as_object) else {
                continue;
            };
            let typed: AgentConfig =
                serde_json::from_value(Value::Object(prior.clone())).map_err(|e| e.to_string())?;
            let known = serde_json::to_value(typed).map_err(|e| e.to_string())?;
            if let Some(object) = row.as_object_mut() {
                for (key, value) in prior {
                    if !known.as_object().unwrap().contains_key(key) && key != "command" {
                        object.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        }
    }
    Ok(())
}

// A legacy split may have projected no cells (or shadowed rows) on read. Keep
// the selected raw payload for untouched aliases; only actual draft edits win.
pub(crate) fn retain_selected_legacy_profile_aliases(
    selected: &Value,
    previous_owner: &Value,
    incoming: &mut Value,
) -> Result<(), String> {
    let previous = registered_profiles_for_read(previous_owner);
    let previous: CodingAgentProfilesConfig = previous
        .get("codingAgentProfiles")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let previous = serde_json::to_value(previous).map_err(|error| error.to_string())?;
    let projected = project_physical_profiles(selected, true)?;
    for id in physical_profile_rows(selected, true)?.keys() {
        let old_cells = previous.get("profilesByAgent").and_then(|map| map.get(id));
        let cells = incoming
            .pointer("/codingAgentProfiles/profilesByAgent")
            .and_then(|map| map.get(id));
        if normalized_profile_cells(old_cells)? == normalized_profile_cells(cells)? {
            incoming["codingAgentProfiles"]["profilesByAgent"][id] =
                projected["codingAgentProfiles"]["profilesByAgent"][id].clone();
        }
        let empty = serde_json::json!({});
        let old_labels = previous
            .get("profileLabelsByAgent")
            .and_then(|map| map.get(id))
            .unwrap_or(&empty);
        let labels = incoming
            .pointer("/codingAgentProfiles/profileLabelsByAgent")
            .and_then(|map| map.get(id))
            .unwrap_or(&empty);
        if old_labels == labels {
            incoming["codingAgentProfiles"]["profileLabelsByAgent"][id] =
                projected["codingAgentProfiles"]["profileLabelsByAgent"][id].clone();
        }
    }
    Ok(())
}

pub(crate) fn comparable_registered_profile_draft(
    mut profiles: CodingAgentProfilesConfig,
    agents: &[AgentConfig],
) -> CodingAgentProfilesConfig {
    repair_coding_agent_profiles_config(&mut profiles, agents);
    profiles.identity_by_agent.clear();
    for cell in profiles
        .profiles_by_agent
        .values_mut()
        .flat_map(|cells| cells.values_mut())
    {
        cell.id_hash = None;
        cell.id_hash_debug = None;
    }
    profiles
}

pub(crate) fn validate_split_canonical_profile_edits(
    settings: &AppSettings,
    original_owner: &Value,
    selected_rows: Option<&Value>,
) -> Result<(), String> {
    let previous = registered_profiles_for_read(original_owner);
    let previous = previous
        .get("codingAgentProfiles")
        .cloned()
        .map(serde_json::from_value::<CodingAgentProfilesConfig>)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    if comparable_registered_profile_draft(previous, &settings.agents)
        != comparable_registered_profile_draft(
            settings.coding_agent_profiles.clone(),
            &settings.agents,
        )
    {
        return Err("crossOwnerCanonicalProfileEdit".into());
    }
    let old_rows = physical_profile_rows(
        &serde_json::json!({
            "agents": selected_rows.cloned().unwrap_or_else(|| serde_json::json!([]))
        }),
        true,
    )?;
    if settings.agents.iter().any(|agent| {
        old_rows
            .get(&agent.id)
            .is_some_and(|hash| hash != &configuration_identity(&agent.command, &agent.envs))
    }) {
        return Err("crossOwnerCanonicalProfileEdit".into());
    }
    Ok(())
}

// Compare the full ordinary baseline before restricting mutation rows. A save
// of unrelated preferences must not bind an externally changed physical BASE.
pub(crate) fn restrict_split_profile_mutation_rows(
    settings: &AppSettings,
    before: &Value,
    incoming: &mut Value,
) -> Result<(), String> {
    let sets = physical_profile_sets(before, true)?;
    let previous = project_physical_profiles(before, true)?;
    let rows = physical_profile_rows(incoming, true)?;
    let empty = serde_json::json!({});
    let mut admitted = HashSet::new();
    for (id, hash) in rows {
        let old_hash = sets.rows.get(&id);
        // Both sides of intent comparison use the existing typed draft shape;
        // canonical_profile_source retains the complete raw unknown fields.
        let typed_cells = |source: &Value| -> Result<Value, String> {
            let cells = normalized_profile_cells(
                source
                    .pointer("/codingAgentProfiles/profilesByAgent")
                    .and_then(|map| map.get(&id)),
            )?;
            let cells: BTreeMap<String, ProfileCellConfig> =
                serde_json::from_value(cells).map_err(|error| error.to_string())?;
            normalized_profile_cells(Some(
                &serde_json::to_value(cells).map_err(|error| error.to_string())?,
            ))
        };
        let labels = incoming
            .pointer("/codingAgentProfiles/profileLabelsByAgent")
            .and_then(|map| map.get(&id))
            .unwrap_or(&empty);
        let old_labels = previous
            .pointer("/codingAgentProfiles/profileLabelsByAgent")
            .and_then(|map| map.get(&id))
            .unwrap_or(&empty);
        let profile_edit =
            typed_cells(incoming)? != typed_cells(&previous)? || labels != old_labels;
        let base_edit = old_hash != Some(&hash);
        let stored = old_hash.is_some_and(|hash| sets.cells.contains_key(hash));
        if settings.local_overlay_state.owns_top_level("agents") && !stored && profile_edit {
            return Err("crossOwnerCanonicalProfileEdit: protected missing binding".into());
        }
        if stored || base_edit || profile_edit {
            admitted.insert(id.clone());
            if old_hash.is_none() && !profile_edit {
                if let Some(cells) = sets.cells.get(&hash) {
                    incoming["codingAgentProfiles"]["profilesByAgent"][&id] = cells.clone();
                    incoming["codingAgentProfiles"]["profileLabelsByAgent"][&id] = sets
                        .labels
                        .get(&hash)
                        .cloned()
                        .unwrap_or_else(|| empty.clone());
                }
            }
        }
    }
    if let Some(rows) = incoming.get_mut("agents").and_then(Value::as_array_mut) {
        rows.retain(|row| {
            row.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| admitted.contains(id))
        });
    }
    Ok(())
}

pub(crate) fn prepare_registered_profile_owner(
    settings: &AppSettings,
    path: &Path,
    profile_before: &Value,
    profile_incoming: &Value,
    profiles_in_sidecar: bool,
    startup: Option<&StartupAgentRows>,
    duplicate_rows: bool,
) -> Result<Option<Value>, String> {
    let prepared_profiles = if settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles")
    {
        None
    } else {
        let owner_path = if profiles_in_sidecar {
            agents_instance_path(path)
        } else {
            path.to_path_buf()
        };
        let prepared = if duplicate_rows || physical_profile_rows(profile_before, true).is_err() {
            // Preserve the pre-existing lenient row view without converting an
            // ambiguous ID map. Profile edits still need an unambiguous BASE.
            let previous_view = registered_profiles_for_read(profile_before);
            let previous: CodingAgentProfilesConfig = previous_view
                .get("codingAgentProfiles")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            if comparable_registered_profile_draft(previous, &settings.agents)
                != comparable_registered_profile_draft(
                    settings.coding_agent_profiles.clone(),
                    &settings.agents,
                )
            {
                return Err("ambiguousProfileConversion".into());
            }
            let mut retained = profile_incoming.clone();
            retain_configuration_commands_with_startup(
                &mut retained,
                profile_before,
                "id",
                false,
                true,
                false,
                startup,
            )?;
            match profile_before.get("codingAgentProfiles") {
                Some(profiles) => retained["codingAgentProfiles"] = profiles.clone(),
                None => {
                    retained
                        .as_object_mut()
                        .unwrap()
                        .remove("codingAgentProfiles");
                }
            }
            retained
        } else if profile_before.get("agents").is_none()
            && profile_incoming.get("agents").is_none()
            && profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
                == Some(&Value::from(1))
            && !["profilesByAgent", "profileLabelsByAgent", "identityByAgent"]
                .iter()
                .any(|key| {
                    profile_before
                        .get("codingAgentProfiles")
                        .is_some_and(|profiles| profiles.get(*key).is_some())
                })
        {
            // Canonical split owners do not acquire a new physical row donor
            // or rewrite their packet for an unrelated preference save.
            profile_before.clone()
        } else {
            let mut prepared = canonical_profile_source(profile_before, profile_incoming, true)
                .map_err(|reason| format!("{}: {reason}", owner_path.display()))?;
            // Typed slot edits own `label`; opaque siblings belong to the raw
            // owner, including obsolete `name` and extension values.
            if let Some(slots) = prepared
                .pointer_mut("/codingAgentProfiles/profileSlots")
                .and_then(Value::as_object_mut)
            {
                for (letter, slot) in slots {
                    if let (Some(slot), Some(raw)) = (
                        slot.as_object_mut(),
                        profile_before
                            .pointer("/codingAgentProfiles/profileSlots")
                            .and_then(|slots| slots.get(letter))
                            .and_then(Value::as_object),
                    ) {
                        for (key, value) in raw {
                            if key != "label" {
                                slot.insert(key.clone(), value.clone());
                            }
                        }
                    }
                }
            }
            retain_configuration_commands_with_startup(
                &mut prepared,
                profile_before,
                "id",
                false,
                true,
                false,
                startup,
            )?;
            prepared
        };
        Some(prepared)
    };
    Ok(prepared_profiles)
}

pub(crate) fn refresh_registered_agent_row_metadata(
    target: &mut Map<String, Value>,
) -> Result<(), String> {
    if let Some(rows) = target.get_mut("agents") {
        let agents: Vec<AgentConfig> =
            serde_json::from_value(rows.clone()).map_err(|error| error.to_string())?;
        for (raw, agent) in rows.as_array_mut().into_iter().flatten().zip(agents) {
            if let Some(object) = raw.as_object_mut() {
                object.insert(
                    "id_hash".to_string(),
                    Value::String(configuration_identity(&agent.command, &agent.envs)),
                );
                object.insert(
                    "id_hash_debug".into(),
                    serde_json::json!(configuration_identity_debug(&agent.command, &agent.envs)),
                );
            }
        }
    }
    Ok(())
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

/// The tree's only executable-stem rule, and the byte-for-byte twin of
/// `executableTokenBasename` in `src/shared/profile-utils.ts`. It deliberately does
/// not use `std::path::Path`, which ignores `\` off Windows (#2430).
pub(crate) fn command_token_basename(token: &str) -> String {
    let normalized = token.replace('\\', "/");
    let leaf = match normalized.rsplit('/').next() {
        Some(segment) if !segment.is_empty() => segment,
        _ => normalized.as_str(),
    };
    let stem = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty() => stem,
        _ => leaf,
    };
    stem.to_lowercase()
}

pub(crate) fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
    token.chars().filter(|c| *c == quote).count() % 2 == 1
}

pub(crate) fn advance_past_config_value(tokens: &[&str], start: usize) -> usize {
    if start >= tokens.len() {
        return start;
    }

    let mut idx = start;
    let mut in_single = false;
    let mut in_double = false;

    while idx < tokens.len() {
        let token = tokens[idx];
        if token_has_unclosed_quote(token, '\'') {
            in_single = !in_single;
        }
        if token_has_unclosed_quote(token, '"') {
            in_double = !in_double;
        }
        idx += 1;
        if !in_single && !in_double {
            break;
        }
    }

    idx
}

pub(crate) fn find_provider_token(tokens: &[&str], provider: &str) -> Option<usize> {
    tokens
        .iter()
        .position(|token| command_token_basename(token) == provider)
}

pub(crate) fn antigravity_has_manual_resume(tokens: &[&str], antigravity_idx: usize) -> bool {
    tokens[antigravity_idx + 1..].iter().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower == "-c"
    })
}

pub(crate) fn codex_has_manual_resume(tokens: &[&str], codex_idx: usize) -> bool {
    let mut idx = codex_idx + 1;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if token.eq_ignore_ascii_case("resume") || token.eq_ignore_ascii_case("--last") {
            return true;
        }
        idx += 1;
    }
    false
}

pub fn is_valid_profile_letter(letter: &str) -> bool {
    letter.len() == 1 && letter.as_bytes()[0].is_ascii_uppercase()
}

pub fn normalize_profile_letter(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() == 1 {
        let upper = trimmed.to_ascii_uppercase();
        if is_valid_profile_letter(&upper) {
            return Some(upper);
        }
    }
    None
}

pub fn empty_profile_cell() -> ProfileCellConfig {
    ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        enabled: true,
        command: String::new(),
        env: BTreeMap::new(),
        notes: String::new(),
    }
}

pub fn repair_coding_agent_profiles_config(
    profiles: &mut CodingAgentProfilesConfig,
    agents: &[AgentConfig],
) -> bool {
    let mut changed = false;

    if profiles.schema_version < 2 {
        profiles.schema_version = default_profiles_schema_version();
        changed = true;
    }

    let original_letters_len = profiles.profile_slots.len();
    profiles
        .profile_slots
        .retain(|letter, _| is_valid_profile_letter(letter));
    changed |= profiles.profile_slots.len() != original_letters_len;

    if !profiles.profile_slots.contains_key("A") {
        profiles.profile_slots.insert(
            "A".to_string(),
            ProfileSlotConfig {
                label: String::new(),
            },
        );
        changed = true;
    }

    let original_defaults_len = profiles.default_profile_by_agent.len();
    profiles
        .default_profile_by_agent
        .retain(|_, letter| is_valid_profile_letter(letter));
    changed |= profiles.default_profile_by_agent.len() != original_defaults_len;

    for cells in profiles.profiles_by_agent.values_mut() {
        let original_cells_len = cells.len();
        cells.retain(|letter, _| is_valid_profile_letter(letter));
        changed |= cells.len() != original_cells_len;
    }

    // #548: prune invalid (non A..Z) letters from each agent's label-override map,
    // mirroring the cell-letter prune above. Never creates override entries and never
    // prunes by agent id (an override for a not-yet-loaded agent is harmless and may
    // precede its agent in load order, per plan 3.7). `retain` keeps every valid key,
    // so a clean config leaves `changed` false and is not rewritten on load.
    for labels in profiles.profile_labels_by_agent.values_mut() {
        let original_len = labels.len();
        labels.retain(|letter, _| is_valid_profile_letter(letter));
        changed |= labels.len() != original_len;
    }

    for agent in agents {
        let cells = profiles
            .profiles_by_agent
            .entry(agent.id.clone())
            .or_default();
        if !cells.contains_key("A") {
            cells.insert("A".to_string(), empty_profile_cell());
            changed = true;
        }
    }

    changed
}

/// #2450: stable, local-only identity of an agent, used to re-bind orphaned
/// profiles. NOT the per-cell digest: a cell's own hash changes whenever that cell
/// is edited, so keying a cell's home by it would move the home on every edit.
/// The case-folded label is mixed in so two agents sharing a command and env stay
/// distinct. Never leaves the machine.
pub fn agent_profile_identity(agent: &AgentConfig) -> String {
    use sha2::{Digest, Sha256};
    let env: BTreeMap<String, String> = agent
        .envs
        .iter()
        .filter(|row| row.enabled)
        .map(|row| (row.key.clone(), row.value.clone()))
        .collect();
    let mut buf = crate::config::agent_command::profile_hash_preimage(&agent.command, &env);
    buf.push_str("label\u{0}");
    buf.push_str(&agent.label.to_ascii_lowercase());
    buf.push('\u{0}');
    let digest = format!("{:x}", Sha256::digest(buf.as_bytes()));
    digest[..16].to_string()
}

pub(crate) fn is_identity_shape(identity: &str) -> bool {
    identity.len() == 16 && identity.bytes().all(|b| b.is_ascii_hexdigit())
}

/// #2450: an agent's profile entry holds nothing real: absent, empty, or exactly
/// the empty `A` that repair creates, with no label overrides.
pub(crate) fn is_profile_gap(profiles: &CodingAgentProfilesConfig, id: &str) -> bool {
    let no_labels = profiles
        .profile_labels_by_agent
        .get(id)
        .is_none_or(|labels| labels.is_empty());
    let no_cells = match profiles.profiles_by_agent.get(id) {
        None => true,
        Some(cells) => {
            cells.is_empty()
                || (cells.len() == 1
                    && cells.get("A").is_some_and(|cell| {
                        let mut content = cell.clone();
                        content.id_hash = None;
                        content.id_hash_debug = None;
                        content == empty_profile_cell()
                    }))
        }
    };
    no_labels && no_cells
}

/// #2450: re-bind an orphaned profile set to the agent that now carries its
/// identity. Returns true when anything moved. NO-OP while any overlay is in
/// force. Runs before every `repair_coding_agent_profiles_config` call, because
/// repair fills a gap with an empty `A` that would otherwise hide it.
pub(crate) fn adopt_orphaned_profiles(settings: &mut AppSettings) -> bool {
    if settings
        .coding_agent_identity_physical
        .as_ref()
        .is_some_and(|raw| raw.get("codingAgentProfiles").is_some())
    {
        return false;
    }
    if settings.coding_agent_profiles.identity_by_agent.is_empty() {
        return false;
    }
    if !settings.local_overlay_state.is_empty() {
        log::info!(
            "[profiles] #2450 - orphan adoption skipped: a settings.local.json overlay is in force"
        );
        return false;
    }
    let current: HashSet<&str> = settings.agents.iter().map(|a| a.id.as_str()).collect();
    let mut orphans: Vec<(String, String)> = settings
        .coding_agent_profiles
        .identity_by_agent
        .iter()
        .filter(|(id, identity)| !current.contains(id.as_str()) && is_identity_shape(identity))
        .map(|(id, identity)| (id.clone(), identity.clone()))
        .collect();
    // Earliest agent in `settings.agents` order owns an identity; a later
    // colliding agent keeps its own state and never adopts.
    let mut claimed: BTreeMap<String, String> = BTreeMap::new();
    let mut moved = false;
    for agent in &settings.agents {
        let identity = agent_profile_identity(agent);
        if let Some(winner) = claimed.get(&identity) {
            log::warn!(
                "[profiles] #2450 - agents '{}' and '{}' share identity {}; only '{}' may adopt",
                winner,
                agent.id,
                identity,
                winner
            );
            continue;
        }
        claimed.insert(identity.clone(), agent.id.clone());
        if !is_profile_gap(&settings.coding_agent_profiles, &agent.id) {
            continue;
        }
        let Some(pos) = orphans.iter().position(|(_, o)| *o == identity) else {
            continue;
        };
        let (orphan_id, _) = orphans.remove(pos);
        let profiles = &mut settings.coding_agent_profiles;
        match profiles.profiles_by_agent.remove(&orphan_id) {
            Some(cells) => {
                profiles.profiles_by_agent.insert(agent.id.clone(), cells);
            }
            None => {
                profiles.profiles_by_agent.remove(&agent.id);
            }
        }
        if let Some(labels) = profiles.profile_labels_by_agent.remove(&orphan_id) {
            profiles
                .profile_labels_by_agent
                .insert(agent.id.clone(), labels);
        }
        profiles.identity_by_agent.remove(&orphan_id);
        log::info!(
            "[profiles] #2450 - agent '{}' adopted the orphaned profiles of '{}'",
            agent.id,
            orphan_id
        );
        moved = true;
    }
    moved
}

/// Pure carrier construction from raw merged inputs, before typed repair.
pub(crate) fn coding_agent_identity_physical_from_raw(
    value: &Value,
    physical_groups: &[String],
    overlay: &LocalSettingsOverlay,
) -> Value {
    serde_json::json!({
        "agents": value.get("agents"),
        "codingAgentProfiles": value.get("codingAgentProfiles"),
        "quotaSources": value.get("quotaSources"),
        "groupOwnership": { "instanceGroups": physical_groups, "overlayPath\
            s": overlay.owned_paths().iter().filter(|path| path.first().is_some_and(|key| ["agent\
            s", "codingAgentProfiles", "quotaSource\
            s"].contains(&key.as_str()))).collect::<Vec<_>>() },
    })
}

/// Borrow the caller's settings file lock; never reenter a locking loader.
/// Physical overlay bytes retain identity fields unknown to the typed view.
pub(crate) fn coding_agent_identity_with_physical_overlay(
    mut value: Value,
    path: &Path,
    physical_groups: &[String],
) -> Value {
    let unmerged = value.clone();
    let base_profiles = value.get("codingAgentProfiles").cloned();
    let overlay = LocalSettingsOverlay::load_and_merge_paired(
        path,
        &mut value,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    restore_codex_home_carrier(value.as_object_mut().unwrap(), base_profiles.as_ref());
    // Match the loader's undecodable-overlay rule without using its repaired
    // typed value as identity authority. The fallback is the raw physical base.
    if !overlay.is_empty() {
        let mut decode_value = value.clone();
        apply_project_decode_to_value(
            &mut decode_value,
            production_instance_base().as_deref(),
            &projects::FsCandidateResolver,
        );
        if serde_json::from_value::<AppSettings>(decode_value).is_err() {
            return coding_agent_identity_physical_from_raw(
                &unmerged,
                physical_groups,
                &LocalSettingsOverlay::default(),
            );
        }
    }
    coding_agent_identity_physical_from_raw(&value, physical_groups, &overlay)
}

/// Refresh clone-based state replacements while borrowing their file lock.
/// Unavailable raw inputs clear transition evidence rather than retaining a stale seed source.
pub(crate) fn refresh_coding_agent_identity_at_locked_path(
    settings: &mut AppSettings,
    path: &Path,
) {
    let captured = (|| {
        let Some(object) =
            read_disk_object_for_write_typed(path, SettingsSaveStage::PreserveDiskGate)?
        else {
            return Ok(None);
        };
        let groups = read_agents_instance_file(&agents_instance_path(path));
        let layer = match &groups {
            Ok(_) => AgentsLayerState::Readable,
            Err(reason) => AgentsLayerState::Unreadable {
                path: agents_instance_path(path),
                reason: reason.clone(),
            },
        };
        let mut value = Value::Object(object);
        let groups = groups
            .ok()
            .flatten()
            .map(|groups| agents_instance_groups(&groups))
            .unwrap_or_default();
        let physical_groups = groups.keys().cloned().collect::<Vec<_>>();
        value.as_object_mut().unwrap().extend(groups);
        let raw = coding_agent_identity_with_physical_overlay(value, path, &physical_groups);
        Ok::<_, SettingsSaveError>(Some((raw, layer)))
    })();
    match captured {
        Ok(Some((raw, layer))) => {
            settings.coding_agent_identity_physical = Some(Arc::new(raw));
            set_agents_layer_state(settings, path, layer);
        }
        Ok(None) => settings.coding_agent_identity_physical = None,
        Err(error) => {
            log::warn!("[settings] binding physical refresh unavailable: {error:?}");
            settings.coding_agent_identity_physical = None;
        }
    }
}

/// Canonical identity-only projection of backend physical inputs. Never infer
/// authority from a client draft or from repaired/defaulted runtime rows.
pub(crate) fn coding_agent_identity_projection_hash(
    settings: &AppSettings,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    if settings.agents_layer.unreadable_path().is_some() {
        return Err("coding_agent_identity_physical_unreadable".to_string());
    }
    let raw = settings
        .coding_agent_identity_physical
        .as_ref()
        .ok_or_else(|| "coding_agent_identity_physical_missing".to_string())?;
    let mut projection = raw.as_ref().clone();
    let agents = projection
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "coding_agent_identity_agents_missing".to_string())?;
    let mut ids = HashSet::new();
    for agent in agents.iter_mut() {
        let object = agent
            .as_object_mut()
            .ok_or_else(|| "coding_agent_identity_invalid".to_string())?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "coding_agent_identity_invalid".to_string())?;
        if !ids.insert(id.to_string()) {
            return Err("coding_agent_identity_duplicate".to_string());
        }
        for key in ["order", "name", "label", "color", "id_hash_debug"] {
            object.remove(key);
        }
    }
    agents.sort_by(|a, b| {
        a.get("id")
            .and_then(Value::as_str)
            .cmp(&b.get("id").and_then(Value::as_str))
    });
    if let Some(slots) = projection
        .get_mut("codingAgentProfiles")
        .and_then(|profiles| profiles.get_mut("profileSlots"))
        .and_then(Value::as_object_mut)
    {
        for slot in slots.values_mut() {
            if let Some(slot) = slot.as_object_mut() {
                slot.remove("label");
            }
        }
    }
    if let Some(profiles) = projection
        .get_mut("codingAgentProfiles")
        .and_then(|profiles| profiles.get_mut("profilesByAgent"))
        .and_then(Value::as_object_mut)
    {
        for cells in profiles.values_mut().filter_map(Value::as_object_mut) {
            for cell in cells.values_mut().filter_map(Value::as_object_mut) {
                cell.remove("id_hash_debug");
            }
        }
    }
    // serde_json's Map is sorted (no preserve_order feature); recurse explicitly
    // so the revision contract remains canonical if that feature changes later.
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let sorted: BTreeMap<_, _> = object
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            _ => value.clone(),
        }
    }
    let bytes = serde_json::to_vec(&canonical(&projection)).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn bootstrap_identity_revision(settings: &AppSettings) -> Result<String, String> {
    coding_agent_identity_projection_hash(settings).map(|hash| format!("r1:0:{hash}"))
}

pub(crate) fn bootstrap_binding_revision(
    settings: &AppSettings,
    original_agent_id: &str,
) -> Result<String, String> {
    let revision = bootstrap_identity_revision(settings)?;
    let rows = settings
        .coding_agent_identity_physical
        .as_ref()
        .and_then(|raw| raw.get("agents"))
        .and_then(Value::as_array)
        .ok_or_else(|| "coding_agent_identity_agents_missing".to_string())?;
    if rows
        .iter()
        .filter(|row| row.get("id").and_then(Value::as_str) == Some(original_agent_id))
        .count()
        != 1
    {
        return Err("coding_agent_binding_entry_missing_or_ambiguous".to_string());
    }
    Ok(revision)
}

/// #2450: record `identityByAgent[id]` for every agent actually written to the
/// base file. Under an overlay those are the agents `restore_base` leaves, so a
/// clone is restored first and an overlay's effective definition is never
/// recorded; `restore_base` itself still runs last over `out`. Rows for other
/// ids are kept verbatim (orphans).
pub(crate) fn write_identity_rows(out: &mut Map<String, Value>, overlay: &LocalSettingsOverlay) {
    let base_agents = if overlay.is_empty() {
        out.get("agents").cloned()
    } else {
        let mut restored = out.clone();
        overlay.restore_base(&mut restored);
        restored.remove("agents")
    };
    let Some(Value::Array(agents)) = base_agents else {
        return;
    };
    let Some(Value::Object(profiles)) = out.get_mut("codingAgentProfiles") else {
        return;
    };
    let Value::Object(rows) = profiles
        .entry("identityByAgent")
        .or_insert_with(|| Value::Object(Map::new()))
    else {
        return;
    };
    for agent in agents {
        match serde_json::from_value::<AgentConfig>(agent) {
            Ok(agent) => {
                rows.insert(
                    agent.id.clone(),
                    Value::String(agent_profile_identity(&agent)),
                );
            }
            Err(e) => log::warn!("[profiles] #2450 - identity row skipped: {e}"),
        }
    }
}

pub fn normalize_env_key_for_platform(key: &str) -> String {
    if cfg!(windows) {
        key.to_ascii_uppercase()
    } else {
        key.to_string()
    }
}

pub fn is_codex_home_key(key: &str) -> bool {
    normalize_env_key_for_platform(key) == normalize_env_key_for_platform("CODEX_HOME")
}

/// True for opencode's config-dir env key. Analogous to [`is_codex_home_key`];
/// the launch path uses it to auto-create the resolved `OPENCODE_CONFIG_DIR`
/// before spawn (opencode does not create that dir itself and exits 1 if it is
/// missing). Case-insensitive on Windows via `normalize_env_key_for_platform`.
pub fn is_opencode_config_dir_key(key: &str) -> bool {
    normalize_env_key_for_platform(key) == normalize_env_key_for_platform("OPENCODE_CONFIG_DIR")
}

pub fn is_reserved_env_key(key: &str) -> bool {
    let normalized = normalize_env_key_for_platform(key);
    let ac_prefix = normalize_env_key_for_platform("AGENTSCOMMANDER_");
    normalized.starts_with(&ac_prefix)
        || normalized == normalize_env_key_for_platform("AC_REAL_GIT")
        || normalized == normalize_env_key_for_platform("TERM")
        || normalized == normalize_env_key_for_platform("GIT_CEILING_DIRECTORIES")
        || normalized == normalize_env_key_for_platform("PATH")
        || normalized == normalize_env_key_for_platform("PATHEXT")
}

pub fn validate_user_env_key(key: &str, context: &str) -> Result<(), String> {
    if key.trim() != key {
        return Err(format!(
            "{context}: env key must not have leading or trailing whitespace"
        ));
    }
    if key.is_empty() {
        return Err(format!("{context}: env key must not be empty"));
    }
    if key.contains('=') {
        return Err(format!("{context}: env key '{key}' must not contain '='"));
    }
    if key.contains('\0') || key.contains('\n') || key.contains('\r') {
        return Err(format!(
            "{context}: env key '{key}' must not contain NUL or newline characters"
        ));
    }
    if is_reserved_env_key(key) {
        return Err(format!(
            "{context}: env key '{key}' is reserved by AgentsCommander"
        ));
    }
    Ok(())
}

pub(crate) fn validate_codex_home_basic<'a>(
    value: &'a str,
    context: &str,
) -> Result<&'a str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{context}: CODEX_HOME must not be empty"));
    }
    if trimmed.contains('\0') || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(format!(
            "{context}: CODEX_HOME must not contain NUL or newline characters"
        ));
    }
    Ok(trimmed)
}

pub fn validate_codex_home_template_value(value: &str, context: &str) -> Result<(), String> {
    let trimmed = validate_codex_home_basic(value, context)?;

    // Accept when the value's COMPLETE LEADING path segment is a known token.
    // Do NOT pick "first token contained anywhere" (it would name the wrong token
    // for a value that starts with one token but mentions another later; #576 R1 F3).
    let starts_with_token = AC_PLACEHOLDER_TOKENS.iter().any(|&token| {
        trimmed == token
            || trimmed.starts_with(&format!("{token}/"))
            || trimmed.starts_with(&format!("{token}\\"))
    });
    if starts_with_token {
        // Leading segment OK; the remainder scanner rejects any unknown %marker%
        // (and skips known tokens that appear later in the path).
        reject_unknown_codex_home_template_markers(trimmed, context)?;
        return Ok(());
    }

    // No valid leading token. If a known token still appears anywhere, the leading
    // segment is wrong: report the "complete path segment" error naming that token.
    if let Some(&token) = AC_PLACEHOLDER_TOKENS
        .iter()
        .find(|&&token| trimmed.contains(token))
    {
        return Err(format!(
            "{context}: CODEX_HOME template must start with {token} as a complete path segment"
        ));
    }

    // No token at all: must already be a literal absolute path.
    reject_unknown_codex_home_template_markers(trimmed, context)?;
    validate_expanded_codex_home_value(trimmed, context).map(|_| ())
}

pub fn validate_expanded_codex_home_value(value: &str, context: &str) -> Result<PathBuf, String> {
    let trimmed = validate_codex_home_basic(value, context)?;
    crate::config::placeholders::reject_unexpanded_markers(trimmed, context, true)?;
    if trimmed.contains('$') || trimmed.contains('%') {
        return Err(format!(
            "{context}: CODEX_HOME must be an absolute literal path without variable markers"
        ));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(format!("{context}: CODEX_HOME must be an absolute path"));
    }
    Ok(path)
}

pub(crate) fn reject_unknown_codex_home_template_markers(
    value: &str,
    context: &str,
) -> Result<(), String> {
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        rest = &rest[start..];
        if let Some(&token) = AC_PLACEHOLDER_TOKENS
            .iter()
            .find(|&&token| rest.starts_with(token))
        {
            rest = &rest[token.len()..];
            continue;
        }
        if let Some(end) = rest[1..].find('%') {
            let marker = &rest[..end + 2];
            return Err(format!(
                "{context}: CODEX_HOME contains unknown placeholder {marker}"
            ));
        }
        break;
    }
    Ok(())
}

pub(crate) fn validate_env_map(
    env: &BTreeMap<String, String>,
    context: &str,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for (key, value) in env {
        validate_user_env_key(key, context)?;
        let normalized = normalize_env_key_for_platform(key);
        if !seen.insert(normalized) {
            return Err(format!("{context}: duplicate env key '{key}'"));
        }
        if is_codex_home_key(key) {
            validate_codex_home_template_value(value, context)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_env_rows(rows: &[CodingAgentEnv], context: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    for row in rows.iter().filter(|row| row.enabled) {
        validate_user_env_key(&row.key, context)?;
        let normalized = normalize_env_key_for_platform(&row.key);
        if !seen.insert(normalized) {
            return Err(format!("{context}: duplicate env key '{}'", row.key));
        }
        if is_codex_home_key(&row.key) {
            validate_codex_home_template_value(&row.value, context)?;
        }
    }
    Ok(())
}

pub fn normalize_container_image_input(value: &str, context: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{context} must not be empty"));
    }
    if trimmed.starts_with('-') {
        return Err(format!("{context} must not start with '-'"));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn normalize_agent_backend_config(
    backend: &mut AgentBackendConfig,
    context: &str,
) -> Result<(), String> {
    if backend.kind == SessionBackendKind::LocalProcess {
        backend.image = None;
        return Ok(());
    }

    let Some(raw_image) = backend.image.take() else {
        return Ok(());
    };
    let trimmed = raw_image.trim();
    if trimmed.is_empty() {
        backend.image = None;
        return Ok(());
    }
    backend.image = Some(normalize_container_image_input(trimmed, context)?);
    Ok(())
}

pub(crate) fn normalize_agent_backend_configs(settings: &mut AppSettings) -> Result<(), String> {
    for agent in &mut settings.agents {
        normalize_agent_backend_config(
            &mut agent.backend,
            &format!("Agent \"{}\" container image", agent.label),
        )?;
    }
    Ok(())
}

/// Validate stored seconds without resolving the later execution default.
pub fn validate_preflight_timeout_seconds(value: Option<u32>) -> Result<(), String> {
    if value == Some(0) {
        Err("preflightTimeoutSeconds must be at least 1 second".to_string())
    } else {
        Ok(())
    }
}

pub fn validate_preflight_timeouts(settings: &AppSettings) -> Result<(), String> {
    for agent in &settings.agents {
        validate_preflight_timeout_seconds(agent.preflight_timeout_seconds)
            .map_err(|error| format!("Agent '{}' {error}", agent.id))?;
    }
    for (agent_id, cells) in &settings.coding_agent_profiles.profiles_by_agent {
        for (letter, cell) in cells {
            validate_preflight_timeout_seconds(cell.preflight_timeout_seconds)
                .map_err(|error| format!("Agent '{agent_id}' profile '{letter}' {error}"))?;
        }
    }
    Ok(())
}

pub fn validate_agent_commands(settings: &AppSettings) -> Result<(), String> {
    for agent in &settings.agents {
        validate_agent_command_text(&format!("Agent \"{}\"", agent.label), &agent.command)?;

        validate_env_rows(
            &agent.envs,
            &format!("Agent \"{}\" env settings", agent.label),
        )?;

        // #529 (G8): reject a bad instructions filename here so the user gets a
        // clear IPC error at save, not a silent fallback at launch. Empty/absent
        // is allowed (means "use the command-derived default").
        if let Some(f) = agent.instructions_filename.as_deref() {
            let f = f.trim();
            if !f.is_empty() && !crate::config::agent_command::is_safe_instructions_filename(f) {
                return Err(format!(
                    "Agent \"{}\" instructions filename is invalid: must be a bare .md filename \
                        with no path separators",
                    agent.label
                ));
            }
        }

        // #598: reject a bad config-seed dest at save so the user gets a clear
        // IPC error here, not a silent fail-soft skip at launch. Inactive
        // (disabled or empty dest) is allowed (means "no seeding").
        if let Some(seed) = agent.config_seed.as_ref() {
            if seed.is_active() {
                validate_config_seed_dest(&seed.dest).map_err(|e| {
                    format!("Agent \"{}\" config seed is invalid: {}", agent.label, e)
                })?;
                // #2162: a dest ending in an OS token names the same folder as
                // the OS variant of the shorter dest. Warn, never reject.
                if let Some(stem) = config_seed_dest_os_token_stem(&seed.dest) {
                    if stem.is_empty() {
                        log::warn!(
                            "[config-seed] agent \"{}\" dest '{}' is only an OS token suffix",
                            agent.label,
                            seed.dest.trim()
                        );
                    } else {
                        log::warn!(
                            "[config-seed] agent \"{}\" dest '{}' ends with an OS token; it names \
                                the same template folder as the OS variant of dest '{}'",
                            agent.label,
                            seed.dest.trim(),
                            stem
                        );
                    }
                }
            }
        }
    }

    for (agent_id, cells) in &settings.coding_agent_profiles.profiles_by_agent {
        for (letter, cell) in cells {
            if !is_valid_profile_letter(letter) {
                return Err(format!(
                    "Coding agent profile cell '{}:{}' uses an invalid profile letter",
                    agent_id, letter
                ));
            }
            validate_env_map(
                &cell.env,
                &format!(
                    "Coding agent profile '{}:{}' env settings",
                    agent_id, letter
                ),
            )?;
            if cell.enabled && !cell.command.trim().is_empty() {
                // #597 - the cell holds params appended to the agent base command,
                // so validate the COMPOSED effective command. Otherwise a banned
                // provider flag (Claude --continue/-c or Codex manual resume /
                // Antigravity --continue/-c) placed in the cell params escapes the
                // check. Pi session selectors are intentionally allowed and remain
                // user-authoritative. The provider token lives in the base, not the
                // cell. Falls back to the cell text when the cell references an
                // agent id that has no configured agent.
                let base = settings
                    .agents
                    .iter()
                    .find(|a| a.id == *agent_id)
                    .map(|a| a.command.as_str())
                    .unwrap_or("");
                let effective =
                    crate::config::agent_command::compose_effective_command(base, &cell.command);
                validate_agent_command_text(
                    &format!("Coding agent profile '{}:{}'", agent_id, letter),
                    &effective,
                )?;
            }
        }
    }

    for (agent_name, letter) in &settings.coding_agent_profiles.default_profile_by_agent {
        if !is_valid_profile_letter(letter) {
            return Err(format!(
                "Coding agent default profile for '{}' must be A through Z",
                agent_name
            ));
        }
    }

    Ok(())
}

pub(crate) fn validate_agent_command_text(context: &str, command: &str) -> Result<(), String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command)
        .map_err(|e| format!("{context}: invalid command: {e}. command={command:?}"))?;

    // Pi selectors are user-authored configuration and intentionally outrank AC
    // automation. Canonical Pi identity must win before the independent legacy
    // provider-token scans below inspect model or provider option values.
    // #1873 - the same holds for a directly configured Muse: any configured
    // argument (resume, --last, UUID, prompt, --workspace ...) is user-authored,
    // suppresses AC injection, and is neither validated nor rewritten here. A
    // wrapper (`env muse ...`) is not Muse and keeps the legacy scans below.
    if matches!(
        CodingAgentKind::detect(&normalized.shell, &normalized.shell_args),
        Some(CodingAgentKind::Pi | CodingAgentKind::Muse)
    ) {
        return Ok(());
    }

    let mut token_strings = Vec::with_capacity(normalized.shell_args.len() + 1);
    token_strings.push(normalized.shell);
    token_strings.extend(normalized.shell_args);
    let tokens: Vec<&str> = token_strings.iter().map(String::as_str).collect();

    if let Some(claude_idx) = find_provider_token(&tokens, "claude") {
        if tokens[claude_idx + 1..].iter().any(|token| {
            token.eq_ignore_ascii_case("--continue") || token.eq_ignore_ascii_case("-c")
        }) {
            return Err(format!(
                "{context}: Claude commands must not include --continue or -c"
            ));
        }
    }

    if let Some(codex_idx) = find_provider_token(&tokens, "codex") {
        if codex_has_manual_resume(&tokens, codex_idx) {
            return Err(format!(
                "{context}: Codex commands must not include resume or --last; AgentsCommander \
                    injects codex resume --last automatically"
            ));
        }
    }

    if let Some(antigravity_idx) =
        find_provider_token(&tokens, "agy").or_else(|| find_provider_token(&tokens, "antigravity"))
    {
        if antigravity_has_manual_resume(&tokens, antigravity_idx) {
            return Err(format!(
                "{context}: Antigravity commands must not include --continue or -c; \
                    AgentsCommander injects agy --continue automatically"
            ));
        }
    }

    Ok(())
}

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

// Source diagnostics retain bounded observations; Failure never means absence.
#[derive(Debug, Clone)]
pub(crate) enum BoundedSourceObservation {
    Absent,
    Bytes {
        bytes: Vec<u8>,
        witness: crate::path_identity::VerifiedPathIdentity,
    },
    Failure(String),
}

impl BoundedSourceObservation {
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes { bytes, witness } => {
                let _captured_identity = witness;
                Some(bytes)
            }
            _ => None,
        }
    }

    pub(crate) fn profile_input(&self) -> Result<Option<Vec<u8>>, String> {
        match self {
            Self::Absent => Ok(None),
            Self::Bytes { bytes, .. } => Ok(Some(bytes.clone())),
            Self::Failure(kind) => Err(kind.clone()),
        }
    }
}

pub(crate) fn observe_agent_file(path: &Path) -> BoundedSourceObservation {
    agent_file_checkpoint("observe", path);
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BoundedSourceObservation::Absent
        }
        Err(error) => {
            return BoundedSourceObservation::Failure(io_error_kind_as_str(error.kind()).into())
        }
        Ok(_) => {}
    }
    match crate::path_identity::read_bounded_regular(path, SETTINGS_CAPTURE_LIMIT) {
        Ok((bytes, witness)) => BoundedSourceObservation::Bytes { bytes, witness },
        Err(_) => BoundedSourceObservation::Failure("sourceReadFailed".into()),
    }
}

pub(crate) fn same_captured_source(left: &CapturedSource, right: &CapturedSource) -> bool {
    match (left, right) {
        (CapturedSource::Absent, CapturedSource::Absent) => true,
        (
            CapturedSource::Present {
                bytes: a,
                witness: aw,
            },
            CapturedSource::Present {
                bytes: b,
                witness: bw,
            },
        ) => a == b && crate::path_identity::same_object(aw, bw),
        _ => false,
    }
}

// Authorization only: never a composed or adoptable AppSettings projection.
pub(crate) struct AgentFileAuthority {
    pub(crate) base: CapturedSource,
    pub(crate) project_paths: CapturedSource,
    pub(crate) context: SourceReadContext,
    pub(crate) root: crate::path_identity::RetainedDirectory,
    pub(crate) raw_main: Map<String, Value>,
}

impl AgentFileAuthority {
    pub(crate) fn capture(guard: &SettingsOperationGuard<'_>) -> Result<Self, String> {
        guard.lease.verify().map_err(|_| "ownerIntegrityLost")?;
        let path = guard.settings_path();
        let destination = project_paths_path(path);
        let base =
            capture_source_file(path, guard.counters).map_err(|_| "sourceContextUnavailable")?;
        let project_paths = capture_source_file(&destination, guard.counters)
            .map_err(|_| "sourceContextUnavailable")?;
        let legacy = base
            .bytes()
            .map(|bytes| raw_project_source_from_bytes(bytes.to_vec(), path, path, true))
            .transpose()
            .map_err(|_| "sourceContextUnavailable")?;
        let project = project_paths
            .bytes()
            .map(|bytes| {
                raw_project_source_from_bytes(bytes.to_vec(), &destination, &destination, false)
            })
            .transpose()
            .map_err(|_| "sourceContextUnavailable")?;
        let selected =
            select_project_source_from_raw(legacy, project, path, &destination, &destination)
                .map_err(|_| "sourceContextUnavailable")?;
        let raw_main = selected
            .legacy
            .as_ref()
            .map(|source| source.object.clone())
            .unwrap_or_default();
        let decoded = decode_project_state(
            &selected.projection,
            production_instance_base().as_deref(),
            &projects::FsCandidateResolver,
        );
        if decoded.has_structural() {
            return Err("sourceContextUnavailable".into());
        }
        let primary = decoded
            .active_selected()
            .into_iter()
            .next()
            .or(decoded.selected_head)
            .map(PathBuf::from);
        let context = SourceReadContext::from_primary(path, primary)
            .map_err(|_| "sourceContextUnavailable")?;
        let root = crate::path_identity::retain_directory(&context.catalog_root)
            .map_err(|_| "sourceContextUnavailable")?;
        Ok(Self {
            base,
            project_paths,
            context,
            root,
            raw_main,
        })
    }

    pub(crate) fn recheck(&self, guard: &SettingsOperationGuard<'_>) -> Result<(), String> {
        guard.lease.verify().map_err(|_| "ownerIntegrityLost")?;
        self.root
            .verify_current()
            .map_err(|_| "staleSourceContext")?;
        let current = Self::capture(guard)?;
        if self.context != current.context
            || !same_captured_source(&self.base, &current.base)
            || !same_captured_source(&self.project_paths, &current.project_paths)
        {
            return Err("staleSourceContext".into());
        }
        Ok(())
    }

    pub(crate) fn preferences(&self) -> Result<AgentFileSourcePreferences, String> {
        self.raw_main
            .get("agentFileSourceByMenu")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "invalidSourcePreferences".into())
            .map(|value| value.unwrap_or_default())
    }

    pub(crate) fn registered_validation(
        &self,
    ) -> (BoundedSourceObservation, RegisteredProfileCapture) {
        let path = &self.context.settings_path;
        let registered = observe_agent_file(&agents_instance_path(path));
        let overlay_path = path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
        let overlay = observe_agent_file(&overlay_path);
        let inputs = vec![
            (path.clone(), Ok(self.base.bytes().map(<[u8]>::to_vec))),
            (agents_instance_path(path), registered.profile_input()),
            (overlay_path, overlay.profile_input()),
        ];
        (registered, classify_registered_profile_inputs(path, inputs))
    }
}

pub(crate) enum AgentFileReadRequest {
    List,
    Read(SourceRef),
}
pub(crate) enum AgentFileReadReply {
    List(AgentFileSources),
    Read(SelectedAgentFileSnapshot),
}

impl SettingsOwner {
    pub(crate) async fn read_agent_files(
        self: &Arc<Self>,
        request: AgentFileReadRequest,
    ) -> Result<AgentFileReadReply, String> {
        let owner = Arc::clone(self);
        tokio::task::spawn_blocking(move || owner.read_agent_files_blocking(request))
            .await
            .map_err(|_| "sourceWorkerFailed")?
    }

    pub(crate) fn read_agent_files_blocking(
        &self,
        request: AgentFileReadRequest,
    ) -> Result<AgentFileReadReply, String> {
        let _admission = self
            .admit_operation()
            .map_err(|_| "settingsOwnerUnavailable")?;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = self.settings.blocking_write();
            let lease = self.lease_guard();
            let lease = lease.as_ref().ok_or("settingsOwnerUnavailable")?;
            let guard = self
                .operation_guard(lease)
                .map_err(|_| "ownerIntegrityLost")?;
            self.revalidate().map_err(|_| "settingsOwnerUnavailable")?;
            let authority = AgentFileAuthority::capture(&guard)?;
            let preferences = authority.preferences()?;
            let (registered, validation) = authority.registered_validation();
            agent_file_checkpoint("diagnostic_captured", guard.settings_path());
            let snapshot = |source: &SourceRef| -> Result<SelectedAgentFileSnapshot, String> {
                let registered_source = source.kind == SourceKind::RegisteredInstance;
                let path = authority
                    .context
                    .locator(source)
                    .map_err(|_| "sourceContextUnavailable")?;
                let observed = if registered_source {
                    registered.clone()
                } else {
                    observe_agent_file(&path)
                };
                let mut result = snapshot_from_captured_source(
                    &authority.context,
                    source,
                    &observed,
                    registered_source.then_some(&validation),
                )?;
                if validation
                    .problems
                    .iter()
                    .any(|problem| !problem.ordinary_participant)
                {
                    result
                        .diagnostics
                        .push(source_diagnostic("overlayUnavailable"));
                }
                Ok(result)
            };
            let reply = match request {
                AgentFileReadRequest::Read(source) => AgentFileReadReply::Read(snapshot(&source)?),
                AgentFileReadRequest::List => {
                    let sources = list_active_sources(&authority.context)
                        .iter()
                        .map(|source| {
                            let s = snapshot(source)?;
                            Ok(AgentFileSourceOption {
                                source: s.source,
                                revision: s.revision,
                                context_revision: s.context_revision,
                                format: s.format,
                                availability: s.availability,
                                capabilities: s.capabilities,
                                diagnostics: s.diagnostics,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    AgentFileReadReply::List(agent_file_sources_reply(sources, preferences)?)
                }
            };
            authority.recheck(&guard)?;
            self.revalidate().map_err(|_| "settingsOwnerUnavailable")?;
            Ok(reply)
        }))
        .map_err(|_| "sourceWorkerFailed".to_owned())?
    }
}

// Constructed only while the closed catalog operation holds owner authority.
pub(crate) struct CatalogSourceOperationGuard<'op, 'lease> {
    pub(crate) owner: &'op SettingsOwner,
    pub(crate) guard: &'op mut SettingsOperationGuard<'lease>,
    pub(crate) authority: AgentFileAuthority,
    pub(crate) source: SourceRef,
    pub(crate) path: PathBuf,
    pub(crate) expected: SourceRevision,
    pub(crate) adopted: AppSettings,
}

impl CatalogSourceOperationGuard<'_, '_> {
    pub(crate) fn target(&self) -> &Path {
        &self.path
    }

    pub(crate) fn verify_target_parent_chain(&self) -> Result<(), String> {
        crate::path_identity::verify_component_chain(self.path.parent().expect("parent"))
            .map_err(|_| "sourceContextUnavailable".to_owned())
    }

    pub(crate) fn effect(&self) -> OperationEffect {
        self.guard.effect()
    }

    pub(crate) fn prepare(&self, edits: &AgentFileEdits) -> Result<Vec<u8>, String> {
        let observation = observe_agent_file(&self.path);
        let snapshot = snapshot_from_captured_source(
            &self.authority.context,
            &self.source,
            &observation,
            None,
        )?;
        if snapshot.revision != self.expected {
            return Err("staleSourceRevision".into());
        }
        if !snapshot.capabilities.editable_save {
            return Err("sourceReadOnlyOrUnavailable".into());
        }
        let before = crate::config::coding_agents_catalog::parse_strict_json(
            observation.bytes().ok_or("sourceReadFailed")?,
        )?;
        let candidate = prepare_agent_file_edits(&before, false, edits)?;
        let encoded = serde_json::to_vec_pretty(&candidate).map_err(|_| "invalidSource")?;
        if encoded.len() > SETTINGS_CAPTURE_LIMIT {
            return Err("sourceTooLarge".into());
        }
        Ok(encoded)
    }

    pub(crate) fn recheck(&self) -> Result<(), String> {
        agent_file_checkpoint("catalog_pre_recheck", &self.path);
        self.authority.recheck(self.guard)?;
        self.owner
            .revalidate()
            .map_err(|_| "settingsOwnerUnavailable")?;
        let path = self
            .authority
            .context
            .locator(&self.source)
            .map_err(|_| "sourceContextUnavailable")?;
        if path != self.path {
            return Err("staleSourceContext".into());
        }
        let observation = observe_agent_file(&path);
        let bytes = observation.bytes().ok_or("sourceReadFailed")?;
        if source_bytes_revision(bytes) != self.expected {
            return Err("staleSourceRevision".into());
        }
        Ok(())
    }

    pub(crate) fn publish(&mut self, bytes: &[u8]) -> Result<(), String> {
        let path = &self.path;
        self.guard.publish(SettingsPublisher::AgentsSave, |_| {
            agent_file_checkpoint("catalog_publish", path);
            crate::config::coding_agents_catalog::replace_selected_personal_bytes(path, bytes)
        })
    }

    pub(crate) fn finish(
        &self,
        mut outcome: AgentFileSaveOutcome,
        reason: Option<&str>,
    ) -> AgentFileSaveReply {
        agent_file_checkpoint("catalog_before_reread", &self.path);
        let observed = observe_agent_file(&self.path);
        let mut snapshot =
            snapshot_from_captured_source(&self.authority.context, &self.source, &observed, None)
                .ok();
        let mut diagnostics = vec![source_diagnostic("effectiveSettingsUnchanged")];
        if let Some(reason) = reason {
            let code = [
                "staleSourceRevision",
                "staleSourceContext",
                "sourceProfileConflict",
                "invalidLegacyProfileOrphans",
                "duplicateJsonMember",
                "sourceReadOnlyOrUnavailable",
                "catalogLockTimeout",
                "uneditablePatchKey",
                "unknownBaseHash",
            ]
            .into_iter()
            .find(|code| reason.contains(code))
            .unwrap_or("catalogSaveFailed");
            let mut diagnostic = source_diagnostic(code);
            if code == "sourceProfileConflict" {
                diagnostic.entry_hint = Some(reason.to_owned());
            }
            diagnostics.push(diagnostic);
        }
        let authority = self.authority.recheck(self.guard);
        let integrity_lost = matches!(&authority, Err(reason) if reason == "ownerIntegrityLost");
        if integrity_lost {
            self.owner.mark_invalid("catalog-owner-integrity");
        }
        let ready = self.owner.revalidate().is_ok();
        if authority.is_err() || !ready {
            diagnostics.push(source_diagnostic(if integrity_lost {
                "ownerIntegrityLost"
            } else {
                "sourceContextUnavailable"
            }));
            if let Some(snapshot) = snapshot.as_mut() {
                snapshot.availability = SourceAvailability::ContextUnavailable;
                snapshot.capabilities = SourceCapabilities::default();
                snapshot.rows.clear();
                snapshot.profiles_by_base_hash.clear();
                snapshot.profile_labels_by_base_hash.clear();
                snapshot.profile_slots.clear();
                snapshot.tickets.clear();
            }
            outcome = if self.effect() == OperationEffect::Possible {
                AgentFileSaveOutcome::PartialOrUnknown
            } else {
                AgentFileSaveOutcome::Rejected
            };
        }
        if outcome == AgentFileSaveOutcome::Committed
            && snapshot
                .as_ref()
                .is_none_or(|s| s.availability != SourceAvailability::Available)
        {
            outcome = AgentFileSaveOutcome::PartialOrUnknown;
            diagnostics.push(source_diagnostic("catalogRereadUnavailable"));
        }
        AgentFileSaveReply {
            outcome,
            snapshot,
            effective_settings: (ready && !integrity_lost).then(|| self.adopted.clone()),
            diagnostics,
        }
    }

    pub(crate) fn interrupted(&self) -> AgentFileSaveReply {
        AgentFileSaveReply {
            outcome: if self.effect() == OperationEffect::Possible {
                AgentFileSaveOutcome::PartialOrUnknown
            } else {
                AgentFileSaveOutcome::Rejected
            },
            snapshot: None,
            effective_settings: None,
            diagnostics: vec![source_diagnostic("catalogRereadUnavailable")],
        }
    }
}

impl SettingsOwner {
    pub(crate) fn save_catalog_source_blocking(
        &self,
        source: SourceRef,
        expected: SourceRevision,
        context_revision: String,
        edits: AgentFileEdits,
    ) -> Result<AgentFileSaveReply, String> {
        let _admission = self
            .admit_operation()
            .map_err(|_| "settingsOwnerUnavailable")?;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let state = self.settings.blocking_write();
            let lease = self.lease_guard();
            let lease = lease.as_ref().ok_or("settingsOwnerUnavailable")?;
            let mut guard = self.operation_guard(lease).map_err(|error| {
                if matches!(error, SettingsOwnerError::Lifecycle(_)) {
                    self.mark_invalid("catalog-owner-integrity");
                    "ownerIntegrityLost"
                } else {
                    "settingsOwnerUnavailable"
                }
            })?;
            self.revalidate().map_err(|_| "settingsOwnerUnavailable")?;
            let authority = match AgentFileAuthority::capture(&guard) {
                Ok(authority) => authority,
                Err(reason) => {
                    if reason == "ownerIntegrityLost" {
                        self.mark_invalid("catalog-owner-integrity");
                    }
                    return Ok(AgentFileSaveReply {
                        outcome: AgentFileSaveOutcome::Rejected,
                        snapshot: None,
                        effective_settings: (self.revalidate().is_ok()).then(|| (*state).clone()),
                        diagnostics: vec![source_diagnostic(&reason)],
                    });
                }
            };
            if source.scope != SourceScope::Project
                || source.kind != SourceKind::CatalogPersonal
                || context_revision != authority.context.revision
            {
                return Ok(AgentFileSaveReply {
                    outcome: AgentFileSaveOutcome::Rejected,
                    snapshot: None,
                    effective_settings: Some((*state).clone()),
                    diagnostics: vec![source_diagnostic("sourceContextUnavailable")],
                });
            }
            let path = match authority.context.locator(&source) {
                Ok(path) => path,
                Err(_) => {
                    return Ok(AgentFileSaveReply {
                        outcome: AgentFileSaveOutcome::Rejected,
                        snapshot: None,
                        effective_settings: Some((*state).clone()),
                        diagnostics: vec![source_diagnostic("sourceContextUnavailable")],
                    })
                }
            };
            let mut proof = CatalogSourceOperationGuard {
                owner: self,
                guard: &mut guard,
                authority,
                source,
                path,
                expected,
                adopted: (*state).clone(),
            };
            agent_file_checkpoint("catalog_authorized", proof.target());
            Ok(crate::config::coding_agents_catalog::publish_selected_personal(&mut proof, &edits))
        }))
        .unwrap_or_else(|_| {
            Ok(AgentFileSaveReply {
                outcome: AgentFileSaveOutcome::Rejected,
                snapshot: None,
                effective_settings: None,
                diagnostics: vec![source_diagnostic("catalogSetupPanicked")],
            })
        })
    }
}

#[cfg(test)]
pub(crate) type AgentFileTestHook = Box<dyn FnMut(&str, &Path)>;

#[cfg(test)]
thread_local! {
    pub(crate) static AGENT_FILE_CHECKPOINT: std::cell::RefCell<Option<AgentFileTestHook>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn agent_file_checkpoint(stage: &str, path: &Path) {
    #[cfg(test)]
    AGENT_FILE_CHECKPOINT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(stage, path);
        }
    });
    #[cfg(not(test))]
    let _ = (stage, path);
}
