use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

use crate::config::instance_artifacts::{
    AGENTS_INSTANCE_FILE_NAME, AGENT_HELP_LOCAL_FILE_NAME, AGENT_HELP_REMOTE_CHECK_FILE_NAME,
    AGENT_HELP_REMOTE_FILE_NAME, AGENT_HELP_SHIPPED_FILE_NAME, BLOCKING_MENUS_LOCAL_FILE_NAME,
    BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME, BLOCKING_MENUS_REMOTE_FILE_NAME,
    BLOCKING_MENUS_SHIPPED_FILE_NAME, SETTINGS_BACKUP_PREFIX, SETTINGS_BACKUP_SUFFIX,
    SETTINGS_FILE_NAME, SETTINGS_LOCAL_OVERRIDE_FILE_NAME, SETTINGS_LOCK_FILE_NAME,
    SETTINGS_OWNER_LOCK_FILE_NAME,
};
use crate::config::local_overlay::{DerivedIdClosure, LocalSettingsOverlay, PairedLeaf};
use crate::config::placeholders::AC_PLACEHOLDER_TOKENS;
use crate::pty::backend::SessionBackendKind;
use crate::session::profile::CodingAgentKind;
use crate::telegram::types::TelegramBotConfig;

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

fn raw_configuration_command<'a>(
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

fn overlay_configuration_command(
    row: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    raw_configuration_command(row, key).map(|value| value.map(str::to_owned))
}

const CONFIGURATION_COMMAND_PAIRS: &[PairedLeaf] = &[
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
fn retain_command_leaf(
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

fn retain_configuration_commands_with_startup(
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
struct StartupAgentRows {
    raw: Value,
    sidecar_owner: bool,
    sidecar_absent: bool,
    permutation: Vec<usize>,
    expected: Vec<AgentConfig>,
}

impl StartupAgentRows {
    fn capture(raw: Value, sidecar_owner: bool, sidecar_absent: bool) -> Option<Self> {
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

    fn validate_source(&self, raw: &Value, sidecar_owner: bool) -> Result<(), String> {
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
fn known_agent_field(key: &str) -> bool {
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

fn comparable_agent(agent: &AgentConfig) -> Result<Value, String> {
    let mut value = serde_json::to_value(agent).map_err(|error| error.to_string())?;
    if let Some(object) = value.as_object_mut() {
        object.remove("order");
        object.remove("id_hash");
        object.remove("id_hash_debug");
    }
    Ok(value)
}

/// Prove the ordered duplicate subsequence before transferring any raw payload.
fn retain_duplicate_agent_rows(
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
    fn is_default(&self) -> bool {
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
struct AgentConfigWire {
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
fn deserialize_agent_order<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
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
fn deserialize_agent_id_hash<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
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
fn deserialize_agent_id_hash_debug<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
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

fn agent_order_permutation(agents: &[AgentConfig]) -> Vec<usize> {
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
pub struct CodingAgentProfilesConfig {
    #[serde(default = "default_profiles_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_profile_slots")]
    pub profile_slots: BTreeMap<String, ProfileSlotConfig>,
    #[serde(default)]
    pub default_profile_by_agent: BTreeMap<String, String>,
    #[serde(default)]
    pub profiles_by_agent: BTreeMap<String, BTreeMap<String, ProfileCellConfig>>,
    /// #548: per-(agent, letter) label override. Resolution is own label ->
    /// legacy profile_slots[letter].label -> bare letter; there is no first-agent
    /// inheritance. Always serializes (no skip_serializing_if) like
    /// profiles_by_agent, so the key is always present on disk.
    #[serde(default)]
    pub profile_labels_by_agent: BTreeMap<String, BTreeMap<String, String>>,
    /// #2450: agent id -> `agent_profile_identity` as of the last save. Rows for
    /// ids that are no longer agents are kept: they are the orphans a recreated
    /// agent adopts (`adopt_orphaned_profiles`). Absent in an older file.
    #[serde(default)]
    pub identity_by_agent: BTreeMap<String, String>,
    /// Backend-owned isolated Codex HOME leaves, retained independently of IDs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub codex_home_key_by_agent: BTreeMap<String, String>,
}

impl Default for CodingAgentProfilesConfig {
    fn default() -> Self {
        Self {
            schema_version: default_profiles_schema_version(),
            profile_slots: default_profile_slots(),
            default_profile_by_agent: BTreeMap::new(),
            profiles_by_agent: BTreeMap::new(),
            profile_labels_by_agent: BTreeMap::new(),
            identity_by_agent: BTreeMap::new(),
            codex_home_key_by_agent: BTreeMap::new(),
        }
    }
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
struct ProfileCellConfigWire {
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
    rows: BTreeMap<String, String>,
    cells: Map<String, Value>,
    labels: Map<String, Value>,
    orphans: Option<Map<String, Value>>,
}

fn profile_object(value: Option<&Value>, path: &str) -> Result<Map<String, Value>, String> {
    match value {
        None => Ok(Map::new()),
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(_) => Err(format!("invalidProfileMap: {path}")),
    }
}

fn profile_container(source: &Value, registered: bool) -> Option<&Value> {
    if registered {
        source.get("codingAgentProfiles")
    } else {
        Some(source)
    }
}

fn validate_profile_orphans(value: Option<&Value>) -> Result<Option<Map<String, Value>>, String> {
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

fn normalized_profile_cells(value: Option<&Value>) -> Result<Value, String> {
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

fn physical_profile_rows(
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

fn registered_profiles_for_read(source: &Value) -> Value {
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
fn selected_registered_profile_owner(
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

fn refresh_physical_source_rows(source: &mut Value, registered: bool) -> Result<(), String> {
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
fn decode_physical_catalog(
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

fn duplicate_profile_document(bytes: &[u8], path: &Path) -> Option<String> {
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

type RegisteredProfileDocument = (PathBuf, Option<Vec<u8>>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegisteredProfileProblemKind {
    Io,
    JsonSyntax,
    DuplicateMember,
    ProfileSemantic,
}

struct RegisteredProfileProblem {
    path: PathBuf,
    kind: RegisteredProfileProblemKind,
    reason: String,
    ordinary_participant: bool,
}

#[derive(Default)]
struct RegisteredProfileCapture {
    documents: Vec<RegisteredProfileDocument>,
    problems: Vec<RegisteredProfileProblem>,
}

impl RegisteredProfileCapture {
    fn ordinary_error(&self) -> Option<String> {
        self.problems
            .iter()
            .find(|problem| problem.ordinary_participant)
            .map(|problem| problem.reason.clone())
    }

    fn admits_project_migration(&self, path: &Path) -> bool {
        let sidecar = agents_instance_path(path);
        self.problems.iter().all(|problem| {
            !problem.ordinary_participant
                || (problem.path == sidecar
                    && problem.kind == RegisteredProfileProblemKind::JsonSyntax)
        })
    }

    fn participant_documents(&self) -> Vec<RegisteredProfileDocument> {
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
fn collect_registered_profile_gate(path: &Path) -> RegisteredProfileCapture {
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

type RegisteredProfileInput = (PathBuf, Result<Option<Vec<u8>>, String>);

fn classify_registered_profile_inputs(
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

fn validate_registered_profile_capture(
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

fn capture_registered_profile_gate(path: &Path) -> Result<Vec<RegisteredProfileDocument>, String> {
    let captured = collect_registered_profile_gate(path);
    match captured.ordinary_error() {
        Some(reason) => Err(reason),
        None => Ok(captured.participant_documents()),
    }
}

fn captured_registered_profile_object(
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

fn recheck_registered_profile_gate(captured: &[RegisteredProfileDocument]) -> Result<(), String> {
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

fn registered_profile_raw_gate(path: &Path) -> Result<(), String> {
    let captured = capture_registered_profile_gate(path)?;
    recheck_registered_profile_gate(&captured)
}

// #2892 P01: additive source-aware contracts. These are data and pure helpers;
// none of them changes the legacy selection, settings or persistence paths.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceFamily {
    Agents,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceRole {
    Catalog,
    Registered,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceScope {
    Project,
    Instance,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    CatalogBase,
    CatalogProject,
    CatalogPersonal,
    RegisteredInstance,
}

/// Logical source only. A backend resolver must derive and authorize its locator
/// and capabilities; a deserialized reference grants no filesystem authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRef {
    pub family: SourceFamily,
    pub role: SourceRole,
    pub scope: SourceScope,
    pub kind: SourceKind,
    pub context_root: String,
}

/// Physical absence is distinct from every byte revision, including empty bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SourceRevision {
    Bytes {
        #[serde(deserialize_with = "deserialize_source_sha256")]
        sha256: String,
    },
    Absent {},
}

fn deserialize_source_sha256<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let sha256 = String::deserialize(deserializer)?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(serde::de::Error::custom(
            "sha256 must be a full lowercase SHA256 digest",
        ));
    }
    Ok(sha256)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationRef {
    pub version: u32,
    pub source: SourceRef,
    pub calculated_id: String,
    #[serde(default)]
    pub requested_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_calculated_id: Option<String>,
    /// Existing local key/id, never an alternate calculated identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_entry_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelectionTicket {
    #[serde(rename = "ref")]
    pub configuration_ref: ConfigurationRef,
    pub expected_source_revision: SourceRevision,
    pub catalog_context_revision: String,
}

/// Generic companions keep the legacy pair and complete physical CAS contract
/// owned by their existing modules, without importing profiles into settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelectionState<LegacyPair, PhysicalRevision> {
    pub legacy_pair: LegacyPair,
    #[serde(rename = "ref")]
    pub configuration_ref: Option<ConfigurationRef>,
    pub physical_revision: PhysicalRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelectionExpectation<LegacyPair, PhysicalRevision> {
    pub legacy_pair: LegacyPair,
    #[serde(rename = "ref")]
    pub configuration_ref: Option<ConfigurationRef>,
    pub physical_revision: PhysicalRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelectionDefault<LegacyPair, PhysicalRevision> {
    pub legacy_pair: LegacyPair,
    #[serde(rename = "ref")]
    pub configuration_ref: ConfigurationRef,
    pub physical_revision: PhysicalRevision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceAvailability {
    Available,
    Absent,
    Invalid,
    IndependentFormatRequired,
    ContextUnavailable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceCapabilities {
    pub available_read: bool,
    pub editable_create: bool,
    pub editable_save: bool,
    pub repair_by_owner: bool,
    pub publish_managed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDiagnostic {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
}

/// The agent model is source-owned: registered entries and catalog definitions
/// need not be converted into one another to carry an independent snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSnapshot<Agent> {
    pub source: SourceRef,
    pub revision: SourceRevision,
    pub availability: SourceAvailability,
    pub capabilities: SourceCapabilities,
    pub agents: Vec<Agent>,
    pub profiles: BTreeMap<String, BTreeMap<String, ProfileCellConfig>>,
    pub diagnostics: Vec<SourceDiagnostic>,
    pub context_revision: String,
}

// #2897 P03: compiled gate; no caller-controlled activation bypass.
#[allow(dead_code)]
pub(crate) const fn source_protocol_active() -> bool {
    false
}

// Keep lower IO carriers behind the existing settings facade.
#[allow(unused_imports)]
pub(crate) use crate::config::local_config_io::{
    PairCompleteProof as IdentityReferenceCompleteProof,
    PairReservation as IdentityReferencePacket, PairReservationRequest as IdentityReferenceRequest,
    PhysicalState as IdentityReferencePhysicalState, PreparedPairError as IdentityReferenceError,
    ProtectedRevision as IdentityReferenceProtectedRevision,
};

/// PAIR layout belongs beside the physical identity-reference facade.
#[allow(dead_code)]
pub(crate) fn identity_reference_pair_paths(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    (
        root.join("config.json"),
        root.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
        root.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME),
    )
}

#[allow(dead_code)]
pub(crate) fn read_identity_reference_physical(
    path: &Path,
) -> Result<IdentityReferencePhysicalState, IdentityReferenceError> {
    crate::config::local_config_io::read_config_pair_physical(path)
}

#[allow(dead_code)]
pub(crate) fn identity_reference_protected_revision(
    tuple: &[IdentityReferencePhysicalState; 2],
) -> Result<[IdentityReferenceProtectedRevision; 2], IdentityReferenceError> {
    crate::config::local_config_io::protected_config_pair_revision(tuple)
}

#[allow(dead_code)]
pub(crate) fn execute_identity_reference_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<(), IdentityReferenceError> {
    crate::config::local_config_io::execute_reserved_config_pair_guarded(
        decisions,
        state,
        reservation,
        operation_id,
        validate,
    )
}

#[allow(dead_code)]
pub(crate) fn verify_identity_reference_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<[IdentityReferencePhysicalState; 2], IdentityReferenceError> {
    crate::config::local_config_io::verify_reserved_config_pair_guarded(
        decisions,
        state,
        reservation,
        operation_id,
        plan_digest,
        validate,
    )
}

#[allow(dead_code)]
pub(crate) fn release_identity_reference_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    proof: &IdentityReferenceCompleteProof,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<[IdentityReferencePhysicalState; 2], IdentityReferenceError> {
    crate::config::local_config_io::release_reserved_config_pair_guarded(
        decisions,
        state,
        reservation,
        proof,
        validate,
    )
}

// Dormant single-editor source primitives. C serializes internal writes;
// settings owns authorization, original inventory and the local journal.
// No legacy loader, Save endpoint or transport invokes these primitives yet.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum SourceCoordinationError {
    #[error("sourceIdentityUncertain")]
    IdentityUncertain,
    #[error("sourceBusy")]
    Busy,
    #[error("sourceTransitionPending")]
    Pending,
    #[error("staleRevision")]
    Stale,
    #[error("journalInvalid")]
    Invalid,
    #[error("notFound")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("sourceCoordinationIo: {0}")]
    Io(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) enum SourceTransitionPhase {
    IntentStored,
    Prepared,
    SourcePublished,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourcePacketRecord {
    pub(crate) target: PathBuf,
    pub(crate) target_id: String,
    pub(crate) before_protected_revision: SourceRevision,
    pub(crate) after_protected_revision: SourceRevision,
    pub(crate) plan_digest: String,
}

// This is metadata only; original candidate bytes stay in the private Intent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourceIdentityMapping {
    pub(crate) entry_hint: String,
    pub(crate) old_calculated_id: Option<String>,
    pub(crate) new_calculated_id: Option<String>,
    pub(crate) profiles: BTreeMap<String, (Option<String>, Option<String>)>,
    pub(crate) unavailable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourceReceipt {
    pub(crate) operation_id: String,
    pub(crate) source_revision: SourceRevision,
}

fn coordination_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn coordination_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn valid_coordination_revision(revision: &SourceRevision) -> bool {
    match revision {
        SourceRevision::Absent {} => true,
        SourceRevision::Bytes { sha256 } => coordination_digest(sha256),
    }
}

#[allow(dead_code)]
pub(crate) struct IdentityWriteLease {
    _lock: crate::config::local_config_io::SidecarWriteLock,
    parent: crate::path_identity::RetainedDirectory,
    instance_id: String,
}
#[allow(dead_code)]
impl IdentityWriteLease {
    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub(crate) fn acquire(
        context: &SourceReadContext,
        timeout: std::time::Duration,
    ) -> Result<Self, SourceCoordinationError> {
        #[cfg(test)]
        SOURCE_COORDINATION_ACQUISITIONS.with(|count| {
            let mut value = count.get();
            value[0] += 1;
            count.set(value);
        });
        let parent = coordination_parent(&context.settings_path)?;
        let canonical = &parent.identity().canonical_path;
        let spelling = canonical
            .to_str()
            .ok_or(SourceCoordinationError::IdentityUncertain)?;
        let spelling = if cfg!(windows) {
            spelling.to_lowercase()
        } else {
            spelling.into()
        };
        let SourceRevision::Bytes {
            sha256: instance_id,
        } = source_bytes_revision(spelling.as_bytes())
        else {
            unreachable!()
        };
        let lock = crate::config::local_config_io::acquire_sidecar_write_lock(
            &canonical.join(crate::config::instance_artifacts::IDENTITY_TRANSITION_LOCK_NAME),
            timeout,
            "identityWriteTimeout",
            "identity write lock",
        )
        .map_err(SourceCoordinationError::Io)?;
        parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        Ok(Self {
            _lock: lock,
            parent,
            instance_id,
        })
    }
}

#[allow(dead_code)]
pub(crate) struct SourceWriteContext<'a> {
    identity: &'a IdentityWriteLease,
    parent: crate::path_identity::RetainedDirectory,
    source_path: PathBuf,
    source: SourceRef,
    context_revision: String,
    physical_source_key: String,
}

#[allow(dead_code)]
impl<'a> SourceWriteContext<'a> {
    pub(crate) fn source(&self) -> &SourceRef {
        &self.source
    }

    pub(crate) fn physical_source_key(&self) -> &str {
        &self.physical_source_key
    }

    pub(crate) fn context_revision(&self) -> &str {
        &self.context_revision
    }

    /// Internal only: caller acquires C first and supplies backend context.
    /// Parent object + normalized basename stays stable over atomic replace.
    pub(crate) fn acquire(
        context: &SourceReadContext,
        source: &SourceRef,
        identity: &'a IdentityWriteLease,
    ) -> Result<Self, SourceCoordinationError> {
        let own = coordination_parent(&context.settings_path)?;
        if !crate::path_identity::same_object(own.identity(), identity.parent.identity()) {
            return Err(SourceCoordinationError::Unauthorized);
        }
        identity
            .parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        let locator = context
            .locator(source)
            .map_err(|_| SourceCoordinationError::Unauthorized)?;
        let parent = coordination_parent(&locator)?;
        let canonical = parent.identity().canonical_path.clone();
        let name = locator
            .file_name()
            .ok_or(SourceCoordinationError::IdentityUncertain)?;
        let key = coordination_key(&parent.identity().object_id, name)?;
        // Legacy artifacts are diagnosed and preserved, never used as authority.
        reject_legacy_source_ledger(&canonical.join(source_ledger_name(source.kind)))?;
        let lease = Self {
            identity,
            parent,
            source_path: canonical.join(name),
            source: source.clone(),
            context_revision: context.revision.clone(),
            physical_source_key: key,
        };
        lease.revalidate(context)?;
        Ok(lease)
    }

    pub(crate) fn revalidate(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourceCoordinationError> {
        if context.revision != self.context_revision {
            return Err(SourceCoordinationError::Stale);
        }
        let locator = context
            .locator(&self.source)
            .map_err(|_| SourceCoordinationError::Unauthorized)?;
        let current = coordination_parent(&locator)?;
        if !crate::path_identity::same_object(current.identity(), self.parent.identity()) {
            return Err(SourceCoordinationError::IdentityUncertain);
        }
        self.parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        reject_legacy_source_ledger(
            &self
                .parent
                .identity()
                .canonical_path
                .join(source_ledger_name(self.source.kind)),
        )?;
        coordination_read(&self.source_path)?;
        Ok(())
    }

    pub(crate) fn source_revision(&self) -> Result<SourceRevision, SourceCoordinationError> {
        self.identity
            .parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        self.parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        Ok(coordination_read(&self.source_path)?.0)
    }
}

fn coordination_publication_error(error: IdentityReferenceError) -> SourceCoordinationError {
    match error {
        IdentityReferenceError::Conflict => SourceCoordinationError::Stale,
        IdentityReferenceError::InvalidPlan(_) => SourceCoordinationError::Invalid,
        other => SourceCoordinationError::Io(other.to_string()),
    }
}

fn coordination_parent(
    path: &Path,
) -> Result<crate::path_identity::RetainedDirectory, SourceCoordinationError> {
    let parent = std::fs::canonicalize(
        path.parent()
            .ok_or(SourceCoordinationError::IdentityUncertain)?,
    )
    .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
    crate::path_identity::retain_directory(&parent)
        .map_err(|_| SourceCoordinationError::IdentityUncertain)
}
fn coordination_key(
    object: &crate::path_identity::FileObjectId,
    name: &std::ffi::OsStr,
) -> Result<String, SourceCoordinationError> {
    let name = name
        .to_str()
        .ok_or(SourceCoordinationError::IdentityUncertain)?;
    let name = if cfg!(windows) {
        name.to_lowercase()
    } else {
        name.into()
    };
    let bytes = serde_json::to_vec(&(object.volume, object.file, name))
        .map_err(|_| SourceCoordinationError::Invalid)?;
    let SourceRevision::Bytes { sha256 } = source_bytes_revision(&bytes) else {
        unreachable!()
    };
    Ok(sha256)
}
fn source_ledger_name(kind: SourceKind) -> &'static str {
    use crate::config::instance_artifacts::*;
    match kind {
        SourceKind::CatalogBase => SOURCE_CATALOG_BASE_LEDGER_NAME,
        SourceKind::CatalogProject => SOURCE_CATALOG_PROJECT_LEDGER_NAME,
        SourceKind::CatalogPersonal => SOURCE_CATALOG_PERSONAL_LEDGER_NAME,
        SourceKind::RegisteredInstance => SOURCE_REGISTERED_INSTANCE_LEDGER_NAME,
    }
}
fn coordination_read(
    path: &Path,
) -> Result<(SourceRevision, Option<Vec<u8>>), SourceCoordinationError> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((SourceRevision::Absent {}, None)),
        Err(e) => Err(SourceCoordinationError::Io(e.to_string())),
        Ok(_) => {
            let (bytes, _) = crate::path_identity::read_bounded_regular(path, 16 * 1024 * 1024)
                .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
            Ok((source_bytes_revision(&bytes), Some(bytes)))
        }
    }
}

fn reject_legacy_source_ledger(path: &Path) -> Result<(), SourceCoordinationError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SourceCoordinationError::Io(error.to_string())),
        Ok(_) => Err(SourceCoordinationError::Invalid),
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct IdentityTransitionSummary {
    pub(crate) operation_id: String,
    pub(crate) phase: SourceTransitionPhase,
    pub(crate) source: SourceRef,
    pub(crate) source_revision_observed: SourceRevision,
    pub(crate) expected_source_revision: SourceRevision,
    pub(crate) after_source_revision: SourceRevision,
    pub(crate) journal_revision: SourceRevision,
    pub(crate) mappings: Vec<SourceIdentityMapping>,
    pub(crate) receipt: Option<SourceReceipt>,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct SourceTransitionStatus {
    pub(crate) journal_revision: SourceRevision,
    pub(crate) source_revision_observed: SourceRevision,
    pub(crate) summary: Option<IdentityTransitionSummary>,
}

/// Stable journal -> source -> journal observation; no write proofs or effects.
#[allow(dead_code)]
pub(crate) fn read_source_transition(
    context: &SourceReadContext,
    source: &SourceRef,
    operation_id: &str,
) -> Result<SourceTransitionStatus, SourceCoordinationError> {
    let locator = context
        .locator(source)
        .map_err(|_| SourceCoordinationError::Unauthorized)?;
    let parent = coordination_parent(&locator)?;
    reject_legacy_source_ledger(
        &parent
            .identity()
            .canonical_path
            .join(source_ledger_name(source.kind)),
    )?;
    for _ in 0..=2 {
        let (revision, journal) = read_own_source_journal(context)?;
        let observed = coordination_read(&locator)?.0;
        if read_own_source_journal(context)?.0 != revision {
            continue;
        }
        parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        let summary = match journal.intents.get(operation_id) {
            None => None,
            Some(intent) => {
                if &intent.source != source {
                    return Err(SourceCoordinationError::Unauthorized);
                }
                let progress = journal
                    .progress
                    .get(operation_id)
                    .ok_or(SourceCoordinationError::Invalid)?;
                Some(IdentityTransitionSummary {
                    operation_id: operation_id.into(),
                    phase: progress.phase(),
                    source: source.clone(),
                    source_revision_observed: observed.clone(),
                    expected_source_revision: intent.expected_revision.clone(),
                    after_source_revision: source_bytes_revision(&intent.serialized_after),
                    journal_revision: revision.clone(),
                    mappings: intent.mappings.clone(),
                    receipt: progress.receipt.clone(),
                })
            }
        };
        return Ok(SourceTransitionStatus {
            journal_revision: revision,
            source_revision_observed: observed,
            summary,
        });
    }
    Err(SourceCoordinationError::Busy)
}

/// Original private Intent. Persisted with inventory before any packet/source effect.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourceStoredIntent {
    pub(crate) schema_version: u32,
    pub(crate) operation_id: String,
    pub(crate) request_id: String,
    pub(crate) payload_digest: String,
    pub(crate) source: SourceRef,
    pub(crate) expected_revision: SourceRevision,
    pub(crate) before_bytes: Option<Vec<u8>>,
    pub(crate) serialized_after: Vec<u8>,
    pub(crate) mappings: Vec<SourceIdentityMapping>,
    pub(crate) editor_id: String,
    pub(crate) context_revision: String,
    pub(crate) authorization_revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourcePrivateJournal {
    pub(crate) schema_version: u32,
    pub(crate) intents: BTreeMap<String, SourceStoredIntent>,
    #[serde(default)]
    pub(crate) inventories: BTreeMap<String, SourceLocalInventory>,
    #[serde(default)]
    pub(crate) progress: BTreeMap<String, SourceLocalProgress>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceDiscoveryWitness {
    pub(crate) registry: Vec<String>,
    pub(crate) parents: Vec<PathBuf>,
    pub(crate) slots: Vec<(PathBuf, bool)>,
    pub(crate) membership: Vec<(PathBuf, Vec<PathBuf>)>,
}

#[cfg(test)]
thread_local! {
    static SOURCE_TRANSITION_PUBLICATIONS: std::cell::Cell<[usize; 3]> =
        const { std::cell::Cell::new([0; 3]) };
    static SOURCE_COORDINATION_ACQUISITIONS: std::cell::Cell<[usize; 2]> =
        const { std::cell::Cell::new([0, 0]) };
}

/// Calling-thread oracle only: driver fixtures and local reload execute synchronously
/// on this thread. A worker must return its own counts; these are not global totals.
#[cfg(test)]
pub(crate) fn source_transition_effect_counts() -> ([usize; 2], usize) {
    (
        SOURCE_COORDINATION_ACQUISITIONS.with(std::cell::Cell::get),
        crate::config::local_config_io::source_guard_publication_calls(),
    )
}

#[cfg(test)]
pub(crate) fn source_transition_publication_counts() -> [usize; 3] {
    SOURCE_TRANSITION_PUBLICATIONS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn count_source_transition_publication(kind: usize) {
    SOURCE_TRANSITION_PUBLICATIONS.with(|count| {
        let mut next = count.get();
        next[kind] += 1;
        count.set(next);
    });
}

/// Private locators are frozen by the backend's complete authorized inventory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceInventoryTarget {
    pub(crate) root: PathBuf,
    pub(crate) root_object: (u64, u64),
    pub(crate) decisions: PathBuf,
    pub(crate) state: PathBuf,
    pub(crate) reservation: PathBuf,
    pub(crate) before_protected: [IdentityReferenceProtectedRevision; 2],
    pub(crate) original_selections: Vec<ConfigurationRef>,
    #[serde(default)]
    pub(crate) original_physical: Option<[IdentityReferencePhysicalState; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceLocalInventory {
    pub(crate) editor_id: String,
    pub(crate) source: SourceRef,
    pub(crate) physical_source_key: String,
    pub(crate) context_revision: String,
    pub(crate) authorization_revision: String,
    pub(crate) authorized_roots: Vec<(PathBuf, (u64, u64))>,
    pub(crate) targets: Vec<SourceInventoryTarget>,
    pub(crate) mappings: Vec<SourceIdentityMapping>,
    origin_binding: Option<OriginBinding>,
    #[serde(default)]
    original_evidence: Option<SourceOriginalInventoryEvidence>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceLocalProgress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_publication:
        Option<crate::config::local_config_io::PreparedSourcePublication>,
    pub(crate) packets: Vec<SourcePacketRecord>,
    pub(crate) packets_prepared: bool,
    pub(crate) source_published: bool,
    pub(crate) refs_verified: bool,
    pub(crate) reloaded: bool,
    pub(crate) receipt: Option<SourceReceipt>,
}

impl SourceLocalProgress {
    pub(crate) fn phase(&self) -> SourceTransitionPhase {
        if self.receipt.is_some() {
            SourceTransitionPhase::Complete
        } else if self.source_published {
            SourceTransitionPhase::SourcePublished
        } else if self.packets_prepared {
            SourceTransitionPhase::Prepared
        } else {
            SourceTransitionPhase::IntentStored
        }
    }
}

impl SourcePrivateJournal {
    fn empty() -> Self {
        Self {
            schema_version: 2,
            intents: BTreeMap::new(),
            inventories: BTreeMap::new(),
            progress: BTreeMap::new(),
        }
    }

    fn validate(&self) -> Result<(), SourceCoordinationError> {
        if self.schema_version != 2 {
            return Err(SourceCoordinationError::Invalid);
        }
        if self.intents.keys().ne(self.inventories.keys())
            || self.intents.keys().ne(self.progress.keys())
        {
            return Err(SourceCoordinationError::Invalid);
        }
        let mut pending_sources = HashSet::new();
        for (id, inventory) in &self.inventories {
            if self.progress.get(id).is_none_or(|p| p.receipt.is_none())
                && !pending_sources.insert(&inventory.physical_source_key)
            {
                return Err(SourceCoordinationError::Invalid);
            }
        }
        let mut requests = std::collections::BTreeSet::new();
        for (id, intent) in &self.intents {
            if id != &intent.operation_id
                || !coordination_id(id)
                || intent.schema_version != 2
                || !coordination_id(&intent.request_id)
                || !requests.insert(&intent.request_id)
                || !coordination_digest(&intent.payload_digest)
                || !coordination_digest(&intent.editor_id)
                || !coordination_digest(&intent.context_revision)
                || !coordination_digest(&intent.authorization_revision)
                || intent.source.context_root.is_empty()
                || !valid_coordination_revision(&intent.expected_revision)
                || intent.serialized_after.is_empty()
                || intent
                    .before_bytes
                    .as_ref()
                    .map(|bytes| source_bytes_revision(bytes))
                    .unwrap_or(SourceRevision::Absent {})
                    != intent.expected_revision
            {
                return Err(SourceCoordinationError::Invalid);
            }
        }
        for (id, inventory) in &self.inventories {
            inventory.validate_original_evidence()?;
            if !coordination_id(id)
                || !coordination_digest(&inventory.editor_id)
                || !coordination_digest(&inventory.physical_source_key)
                || !coordination_digest(&inventory.context_revision)
                || !coordination_digest(&inventory.authorization_revision)
                || inventory.source.context_root.is_empty()
                || inventory
                    .authorized_roots
                    .windows(2)
                    .any(|pair| pair[0].0 >= pair[1].0)
                || inventory
                    .targets
                    .windows(2)
                    .any(|pair| pair[0].decisions >= pair[1].decisions)
            {
                return Err(SourceCoordinationError::Invalid);
            }
            for target in &inventory.targets {
                if !target.root.is_absolute()
                    || !target.decisions.is_absolute()
                    || !target.state.is_absolute()
                    || !target.reservation.is_absolute()
                    || target.decisions.parent() != Some(target.root.as_path())
                    || target.state.parent() != Some(target.root.as_path())
                    || target.reservation.parent() != Some(target.root.as_path())
                    || target.decisions == target.state
                    || target.decisions == target.reservation
                    || target.state == target.reservation
                    || !inventory
                        .authorized_roots
                        .iter()
                        .any(|(root, _)| target.root.starts_with(root))
                {
                    return Err(SourceCoordinationError::Invalid);
                }
            }
            if let Some(intent) = self.intents.get(id) {
                if intent.source != inventory.source
                    || intent.mappings != inventory.mappings
                    || intent.editor_id != inventory.editor_id
                    || intent.context_revision != inventory.context_revision
                    || intent.authorization_revision != inventory.authorization_revision
                {
                    return Err(SourceCoordinationError::Invalid);
                }
            }
        }
        for (id, progress) in &self.progress {
            if let Some(proof) = &progress.source_publication {
                let intent = self
                    .intents
                    .get(id)
                    .ok_or(SourceCoordinationError::Invalid)?;
                let digest = source_transition_digest(intent)?;
                if intent.before_bytes.is_some() || !proof.validate_record(&digest) {
                    return Err(SourceCoordinationError::Invalid);
                }
            }
            let inventory = self
                .inventories
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            let mut targets = std::collections::BTreeSet::new();
            for packet in &progress.packets {
                if !targets.insert(&packet.target)
                    || !inventory
                        .targets
                        .iter()
                        .any(|target| target.decisions == packet.target)
                    || !coordination_digest(&packet.target_id)
                    || !coordination_digest(&packet.plan_digest)
                    || !valid_coordination_revision(&packet.before_protected_revision)
                    || !valid_coordination_revision(&packet.after_protected_revision)
                    || inventory
                        .targets
                        .iter()
                        .find(|target| target.decisions == packet.target)
                        .is_none_or(|target| {
                            !source_transition_digest(&target.before_protected).is_ok_and(
                                |digest| {
                                    packet.before_protected_revision
                                        == (SourceRevision::Bytes { sha256: digest })
                                },
                            )
                        })
                {
                    return Err(SourceCoordinationError::Invalid);
                }
            }
            let intent = self
                .intents
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if progress.packets_prepared && progress.packets.len() != inventory.targets.len()
                || progress.source_published
                    && (!progress.packets_prepared
                        || intent.before_bytes.is_none() && progress.source_publication.is_none())
            {
                return Err(SourceCoordinationError::Invalid);
            }
            if progress.refs_verified && !progress.source_published
                || progress.reloaded && !progress.refs_verified
                || progress.receipt.as_ref().is_some_and(|receipt| {
                    receipt.operation_id != *id
                        || !progress.reloaded
                        || receipt.source_revision
                            != source_bytes_revision(&intent.serialized_after)
                })
            {
                return Err(SourceCoordinationError::Invalid);
            }
        }
        Ok(())
    }
}

fn own_source_journal_path(
    context: &SourceReadContext,
) -> Result<PathBuf, SourceCoordinationError> {
    Ok(context
        .settings_path
        .parent()
        .ok_or(SourceCoordinationError::Unauthorized)?
        .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME))
}

pub(crate) fn read_own_source_journal(
    context: &SourceReadContext,
) -> Result<(SourceRevision, SourcePrivateJournal), SourceCoordinationError> {
    let (revision, bytes) = coordination_read(&own_source_journal_path(context)?)?;
    let journal = match bytes {
        None => SourcePrivateJournal::empty(),
        Some(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| SourceCoordinationError::Invalid)?
        }
    };
    journal.validate()?;
    let parent = coordination_parent(&context.settings_path)?;
    let canonical = parent
        .identity()
        .canonical_path
        .to_str()
        .ok_or(SourceCoordinationError::IdentityUncertain)?;
    let spelling = if cfg!(windows) {
        canonical.to_lowercase()
    } else {
        canonical.to_owned()
    };
    let own_revision = source_bytes_revision(spelling.as_bytes());
    let SourceRevision::Bytes { sha256: own_id } = own_revision else {
        unreachable!()
    };
    if journal
        .intents
        .values()
        .any(|intent| intent.editor_id != own_id)
        || journal
            .inventories
            .values()
            .any(|inventory| inventory.editor_id != own_id)
    {
        return Err(SourceCoordinationError::Unauthorized);
    }
    parent
        .verify_current()
        .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
    Ok((revision, journal))
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct SourcePublicationObservation {
    pub(crate) revision: SourceRevision,
}

/// The caller retains its original authority; exact after is recognized outside
/// the publisher and is never interpreted as a rejected, effect-free attempt.
#[allow(dead_code)]
pub(crate) fn publish_source_intent_with_validation(
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    intent: &SourceStoredIntent,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<SourcePublicationObservation, SourceCoordinationError> {
    if intent.source != lease.source
        || intent.editor_id != lease.identity.instance_id
        || intent.context_revision != lease.context_revision
    {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let after = source_bytes_revision(&intent.serialized_after);
    let guard = || {
        lease
            .identity
            .parent
            .verify_current()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        lease
            .revalidate(context)
            .map_err(|_| IdentityReferenceError::Conflict)?;
        let revision = lease
            .source_revision()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        if revision != intent.expected_revision && revision != after {
            return Err(IdentityReferenceError::Conflict);
        }
        validate()
    };
    guard().map_err(coordination_publication_error)?;
    let observed = lease.source_revision()?;
    let before = match &intent.before_bytes {
        None => IdentityReferencePhysicalState::Absent,
        Some(bytes) => IdentityReferencePhysicalState::from_bytes(bytes.clone()),
    };
    let (mut private_revision, mut journal) = read_own_source_journal(context)?;
    if journal.intents.get(&intent.operation_id) != Some(intent) {
        return Err(SourceCoordinationError::Invalid);
    }
    let inventory = journal
        .inventories
        .get(&intent.operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if inventory.source != intent.source
        || inventory.editor_id != intent.editor_id
        || inventory.context_revision != intent.context_revision
        || inventory.authorization_revision != intent.authorization_revision
        || inventory.physical_source_key != lease.physical_source_key
        || inventory.mappings != intent.mappings
    {
        return Err(SourceCoordinationError::Invalid);
    }
    let progress = journal
        .progress
        .get(&intent.operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if !progress.packets_prepared {
        return Err(SourceCoordinationError::Pending);
    }
    let already_published = progress.source_published;
    let initial_proof = progress.source_publication.clone();
    let intent_digest = source_transition_digest(intent)?;
    let ownership_required = intent.before_bytes.is_none();
    if ownership_required && observed == after {
        let proof = initial_proof
            .as_ref()
            .ok_or(SourceCoordinationError::Stale)?;
        if !already_published {
            crate::config::local_config_io::verify_prepared_source_target(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                proof,
                &guard,
            )
            .map_err(coordination_publication_error)?;
        }
    }
    if observed == after {
        if coordination_read(&lease.source_path)?.1.as_deref()
            != Some(intent.serialized_after.as_slice())
        {
            return Err(SourceCoordinationError::Stale);
        }
        guard().map_err(coordination_publication_error)?;
        return Ok(SourcePublicationObservation { revision: after });
    }
    if already_published {
        return Err(SourceCoordinationError::Stale);
    }
    if ownership_required {
        let mut capability = match initial_proof.as_ref() {
            Some(proof) => crate::config::local_config_io::reopen_source_publication(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                proof,
                &guard,
            ),
            None => crate::config::local_config_io::prepare_source_publication(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                &intent_digest,
                &guard,
            ),
        }
        .map_err(coordination_publication_error)?;
        if initial_proof.is_none() {
            let proof = capability.proof().clone();
            let mut planned = journal.clone();
            planned
                .progress
                .get_mut(&intent.operation_id)
                .ok_or(SourceCoordinationError::Invalid)?
                .source_publication = Some(proof.clone());
            let checkpoint_guard = || capability.check();
            let checkpoint = update_own_source_journal_for_publication(
                lease.identity,
                lease,
                context,
                &private_revision,
                |journal| {
                    journal
                        .progress
                        .get_mut(&intent.operation_id)
                        .ok_or(SourceCoordinationError::Invalid)?
                        .source_publication = Some(proof);
                    Ok(())
                },
                &checkpoint_guard,
                Some((&intent.operation_id, &capability)),
            );
            match checkpoint {
                Ok(revision) => {
                    capability.check().map_err(coordination_publication_error)?;
                    let (confirmed_revision, confirmed) = read_own_source_journal(context)?;
                    if confirmed_revision != revision || confirmed != planned {
                        return Err(SourceCoordinationError::Stale);
                    }
                    private_revision = confirmed_revision;
                    journal = confirmed;
                }
                Err(error) => {
                    // Pure Err-before/Err-landed/other classification. Every
                    // outcome retains the stage and returns the original error;
                    // no native, unlink or second CAS follows an Err.
                    if capability.check().is_ok() {
                        if let Ok((revision, current)) = read_own_source_journal(context) {
                            let _before_exact = revision == private_revision && current == journal;
                            let _after_exact = current == planned;
                        }
                    }
                    return Err(error);
                }
            }
        }
        let checkpoint_guard = || {
            guard()?;
            let (revision, current) =
                read_own_source_journal(context).map_err(|_| IdentityReferenceError::Conflict)?;
            if revision != private_revision || current != journal {
                return Err(IdentityReferenceError::Conflict);
            }
            Ok(())
        };
        #[cfg(test)]
        count_source_transition_publication(2);
        capability
            .commit(&checkpoint_guard)
            .map_err(coordination_publication_error)?;
        return Ok(SourcePublicationObservation { revision: after });
    }
    #[cfg(test)]
    count_source_transition_publication(2);
    let publication =
        crate::config::local_config_io::publish_source_coordination_bytes_with_validation(
            &lease.source_path,
            &before,
            &intent.serialized_after,
            &lease.parent,
            &guard,
        );
    publication.result.map_err(coordination_publication_error)?;
    // Validate authority again, then classify durable bytes even if the final
    // publication check failed. No callback advances local progress or events.
    guard().map_err(coordination_publication_error)?;
    match lease.source_revision()? {
        revision if revision == after => Ok(SourcePublicationObservation { revision }),
        revision if revision == intent.expected_revision => Err(SourceCoordinationError::Stale),
        _ => Err(SourceCoordinationError::Stale),
    }
}

/// Pure record changes are redecoded before guarded CAS publication; callbacks
/// borrow original proofs and never advance coordinator progress themselves.
#[allow(dead_code)]
pub(crate) fn update_own_source_journal_with_validation<F>(
    identity: &IdentityWriteLease,
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    expected: &SourceRevision,
    change: F,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<SourceRevision, SourceCoordinationError>
where
    F: FnOnce(&mut SourcePrivateJournal) -> Result<(), SourceCoordinationError>,
{
    update_own_source_journal_for_publication(
        identity, lease, context, expected, change, validate, None,
    )
}

fn update_own_source_journal_for_publication<F>(
    identity: &IdentityWriteLease,
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    expected: &SourceRevision,
    change: F,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
    prepared: Option<(
        &str,
        &crate::config::local_config_io::PreparedSourceCapability<'_>,
    )>,
) -> Result<SourceRevision, SourceCoordinationError>
where
    F: FnOnce(&mut SourcePrivateJournal) -> Result<(), SourceCoordinationError>,
{
    if !std::ptr::eq(identity, lease.identity) {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let parent = coordination_parent(&context.settings_path)?;
    if !crate::path_identity::same_object(parent.identity(), identity.parent.identity()) {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let guard = || {
        identity
            .parent
            .verify_current()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        lease
            .revalidate(context)
            .map_err(|_| IdentityReferenceError::Conflict)?;
        validate()
    };
    guard().map_err(coordination_publication_error)?;
    let path = own_source_journal_path(context)?;
    let before = read_identity_reference_physical(&path).map_err(coordination_publication_error)?;
    let (revision, mut journal) = read_own_source_journal(context)?;
    if &revision != expected
        || before
            != read_identity_reference_physical(&path).map_err(coordination_publication_error)?
    {
        return Err(SourceCoordinationError::Stale);
    }
    let original = journal.clone();
    change(&mut journal)?;
    journal.validate()?;
    if original
        .intents
        .iter()
        .any(|(id, intent)| journal.intents.get(id) != Some(intent))
        || original
            .inventories
            .iter()
            .any(|(id, inventory)| journal.inventories.get(id) != Some(inventory))
        || original
            .progress
            .keys()
            .any(|id| !journal.progress.contains_key(id))
    {
        return Err(SourceCoordinationError::Invalid);
    }
    for (id, previous) in &original.progress {
        let next = journal
            .progress
            .get(id)
            .ok_or(SourceCoordinationError::Invalid)?;
        if !next.packets.starts_with(&previous.packets)
            || previous
                .source_publication
                .as_ref()
                .is_some_and(|proof| next.source_publication.as_ref() != Some(proof))
            || previous.packets_prepared && !next.packets_prepared
            || previous.source_published && !next.source_published
            || previous.refs_verified && !next.refs_verified
            || previous.reloaded && !next.reloaded
            || previous
                .receipt
                .as_ref()
                .is_some_and(|receipt| next.receipt.as_ref() != Some(receipt))
        {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for (id, next) in &journal.progress {
        if original
            .progress
            .get(id)
            .and_then(|p| p.source_publication.as_ref())
            .is_none()
            && next.source_publication.is_some()
        {
            let (prepared_id, capability) = prepared.ok_or(SourceCoordinationError::Invalid)?;
            let intent = journal
                .intents
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if prepared_id != id
                || intent.source != lease.source
                || next.source_publication.as_ref() != Some(capability.proof())
                || !next.packets_prepared
                || intent.editor_id != identity.instance_id
                || intent.expected_revision != (SourceRevision::Absent {})
                || lease.source_revision()? != (SourceRevision::Absent {})
            {
                return Err(SourceCoordinationError::Invalid);
            }
            capability.check().map_err(coordination_publication_error)?;
        }
    }
    for (id, next) in &journal.progress {
        let previous = original.progress.get(id);
        let publishing = next.source_published && previous.is_none_or(|p| !p.source_published);
        let completing = next.receipt.is_some() && previous.is_none_or(|p| p.receipt.is_none());
        if publishing || completing {
            let intent = journal
                .intents
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            let inventory = journal
                .inventories
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if intent.source != lease.source
                || intent.editor_id != identity.instance_id
                || inventory.physical_source_key != lease.physical_source_key
                || coordination_read(&lease.source_path)?.1.as_deref()
                    != Some(intent.serialized_after.as_slice())
            {
                return Err(SourceCoordinationError::Stale);
            }
            if publishing && intent.before_bytes.is_none() {
                let proof = next
                    .source_publication
                    .as_ref()
                    .ok_or(SourceCoordinationError::Invalid)?;
                crate::config::local_config_io::verify_prepared_source_target(
                    &lease.parent,
                    &lease.source_path,
                    &intent.serialized_after,
                    proof,
                    &guard,
                )
                .map_err(coordination_publication_error)?;
            }
        }
    }
    for (id, intent) in &journal.intents {
        if original.intents.contains_key(id) {
            continue;
        }
        if intent.editor_id != identity.instance_id
            || intent.source != lease.source
            || intent.context_revision != lease.context_revision
            || lease.source_revision()? != intent.expected_revision
        {
            return Err(SourceCoordinationError::Stale);
        }
    }
    for (id, inventory) in &journal.inventories {
        if original.inventories.get(id) == Some(inventory) {
            continue;
        }
        if inventory.editor_id != identity.instance_id
            || inventory.source != lease.source
            || inventory.physical_source_key != lease.physical_source_key
            || inventory.context_revision != lease.context_revision
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
        let intent = journal
            .intents
            .get(id)
            .ok_or(SourceCoordinationError::Invalid)?;
        let evidence = inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if intent.editor_id != identity.instance_id
            || intent.source != inventory.source
            || intent.mappings != inventory.mappings
            || intent.authorization_revision != inventory.authorization_revision
            || evidence.expected_source != intent.expected_revision
            || evidence.after_source != source_bytes_revision(&intent.serialized_after)
            || evidence.inventory_digest
                != source_inventory_digest(
                    &inventory.editor_id,
                    &inventory.source,
                    &inventory.physical_source_key,
                    &inventory.context_revision,
                    &evidence.topology,
                )?
            || lease.source_revision()? != evidence.expected_source
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
    }
    if journal == original {
        guard().map_err(coordination_publication_error)?;
        return Ok(revision);
    }
    let mut bytes =
        serde_json::to_vec_pretty(&journal).map_err(|_| SourceCoordinationError::Invalid)?;
    bytes.push(b'\n');
    let decoded: SourcePrivateJournal =
        serde_json::from_slice(&bytes).map_err(|_| SourceCoordinationError::Invalid)?;
    decoded.validate()?;
    if decoded != journal {
        return Err(SourceCoordinationError::Invalid);
    }
    #[cfg(test)]
    count_source_transition_publication(1);
    crate::config::local_config_io::publish_source_coordination_bytes_with_validation(
        &path,
        &before,
        &bytes,
        &identity.parent,
        &guard,
    )
    .result
    .map_err(coordination_publication_error)?;
    Ok(source_bytes_revision(&bytes))
}

/// Lookup is restricted to the own C path derived from backend context. Status
/// can expose the original ID and CAS before a shared fence exists.
#[allow(dead_code)]
pub(crate) fn read_own_source_intent(
    context: &SourceReadContext,
    operation_id: &str,
) -> Result<(SourceRevision, Option<SourceStoredIntent>), SourceCoordinationError> {
    let (revision, journal) = read_own_source_journal(context)?;
    Ok((revision, journal.intents.get(operation_id).cloned()))
}

#[allow(dead_code)]
pub(crate) fn source_complete_proof(
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    operation_id: &str,
    target_id: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<IdentityReferenceCompleteProof, SourceCoordinationError> {
    validate().map_err(coordination_publication_error)?;
    lease.revalidate(context)?;
    let (_, journal) = read_own_source_journal(context)?;
    let intent = journal
        .intents
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    let inventory = journal
        .inventories
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    let progress = journal
        .progress
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if intent.editor_id != lease.identity.instance_id
        || intent.source != lease.source
        || inventory.physical_source_key != lease.physical_source_key
        || coordination_read(&lease.source_path)?.1.as_deref()
            != Some(intent.serialized_after.as_slice())
    {
        return Err(SourceCoordinationError::Stale);
    }
    let projection = serde_json::json!({
        "schemaVersion":2, "operationId":operation_id, "phase":progress.phase(),
        "physicalSourceKey":inventory.physical_source_key,
        "afterSourceRevision":source_bytes_revision(&intent.serialized_after),
        "packetsPrepared":progress.packets_prepared, "sourcePublished":progress.source_published,
        "refsVerified":progress.refs_verified, "reloaded":progress.reloaded,
        "packets":progress.packets, "receipt":progress.receipt
    });
    let source = serde_json::to_value(lease.source_revision()?)
        .map_err(|_| SourceCoordinationError::Invalid)?;
    IdentityReferenceCompleteProof::from_single_editor_complete(
        &projection,
        operation_id,
        target_id,
        &source,
    )
    .map_err(coordination_publication_error)
}

// #2893 P02: internal readers and pure preparations. No caller publishes these
// plans yet; the existing whole-settings save remains the production boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) enum SourcePreparationError {
    #[error("contextUnavailable")]
    ContextUnavailable,
    #[error("contextChanged")]
    ContextChanged,
    #[error("originUnavailable")]
    OriginUnavailable,
    #[error("legacyOriginUnresolved")]
    LegacyOriginUnresolved,
    #[error("staleRevision")]
    StaleRevision,
    #[error("sourceReadOnly")]
    ReadOnly,
    #[error("independentFormatRequired")]
    IndependentFormatRequired,
    #[error("invalidSourceCandidate")]
    InvalidCandidate,
    #[error("protectedSettingsField")]
    ProtectedSettingsField,
    #[error("{0}")]
    Identity(IdentityValidationError),
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn source_transition_digest_bytes(bytes: &[u8]) -> SourceRevision {
    source_bytes_revision(bytes)
}

fn source_bytes_revision(bytes: &[u8]) -> SourceRevision {
    use sha2::{Digest, Sha256};
    SourceRevision::Bytes {
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

/// Unknown evidence is not physical absence and is never a CAS precondition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) enum OriginPhysicalInput {
    Absent,
    Bytes(Vec<u8>),
    Unknown,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl OriginPhysicalInput {
    fn read(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => Self::Bytes(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::Absent,
            Err(_) => Self::Unknown,
        }
    }

    fn revision(&self) -> Option<SourceRevision> {
        match self {
            Self::Absent => Some(SourceRevision::Absent {}),
            Self::Bytes(bytes) => Some(source_bytes_revision(bytes)),
            Self::Unknown => None,
        }
    }

    fn value(&self) -> Option<Value> {
        match self {
            Self::Bytes(bytes) => serde_json::from_slice(bytes).ok(),
            Self::Absent | Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct SourceReadContext {
    settings_path: PathBuf,
    catalog_root: PathBuf,
    project: bool,
    revision: String,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl SourceReadContext {
    /// Backend-only context: P comes from the current registry, never the
    /// assignment target Q or a deserialized SourceRef.
    pub(crate) fn from_settings(
        settings: &AppSettings,
        settings_path: &Path,
    ) -> Result<Self, SourcePreparationError> {
        let primary = crate::config::coding_agents_catalog::primary_project_root(settings);
        Self::from_primary(settings_path, primary)
    }

    fn from_primary(
        settings_path: &Path,
        primary: Option<PathBuf>,
    ) -> Result<Self, SourcePreparationError> {
        let config = settings_path
            .parent()
            .ok_or(SourcePreparationError::ContextUnavailable)?;
        let config = std::fs::canonicalize(config)
            .map_err(|_| SourcePreparationError::ContextUnavailable)?;
        let catalog_root = match &primary {
            Some(root) => std::fs::canonicalize(root)
                .map_err(|_| SourcePreparationError::ContextUnavailable)?,
            None => config.clone(),
        };
        let bytes = serde_json::to_vec(&(
            config.to_string_lossy(),
            catalog_root.to_string_lossy(),
            primary.is_some(),
        ))
        .map_err(|_| SourcePreparationError::ContextUnavailable)?;
        let SourceRevision::Bytes { sha256 } = source_bytes_revision(&bytes) else {
            unreachable!()
        };
        Ok(Self {
            settings_path: config.join(SETTINGS_FILE_NAME),
            catalog_root,
            project: primary.is_some(),
            revision: sha256,
        })
    }

    fn source(&self, kind: SourceKind) -> SourceRef {
        let registered = kind == SourceKind::RegisteredInstance;
        SourceRef {
            family: SourceFamily::Agents,
            role: if registered {
                SourceRole::Registered
            } else {
                SourceRole::Catalog
            },
            scope: if self.project && !registered {
                SourceScope::Project
            } else {
                SourceScope::Instance
            },
            kind,
            context_root: if registered {
                self.settings_path
                    .parent()
                    .expect("constructed with parent")
                    .to_string_lossy()
                    .into_owned()
            } else {
                self.catalog_root.to_string_lossy().into_owned()
            },
        }
    }

    fn locator(&self, source: &SourceRef) -> Result<PathBuf, SourcePreparationError> {
        if source.scope == SourceScope::Instance
            && source.kind != SourceKind::RegisteredInstance
            && self.project
        {
            let mut instance = self.clone();
            instance.project = false;
            instance.catalog_root = self
                .settings_path
                .parent()
                .ok_or(SourcePreparationError::ContextUnavailable)?
                .to_path_buf();
            return instance.locator(source);
        }
        if source != &self.source(source.kind)
            || (!self.project && source.kind == SourceKind::CatalogProject)
        {
            return Err(SourcePreparationError::ContextUnavailable);
        }
        if source.kind == SourceKind::RegisteredInstance {
            return Ok(agents_instance_path(&self.settings_path));
        }
        let ac = if self.project {
            self.catalog_root.join(".ac")
        } else {
            self.catalog_root.clone()
        };
        crate::config::coding_agents_catalog::independent_catalog_path(&ac, source.kind)
            .ok_or(SourcePreparationError::ContextUnavailable)
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn list_active_sources(context: &SourceReadContext) -> Vec<SourceRef> {
    let mut contexts = vec![context.clone()];
    if context.project {
        let mut instance = context.clone();
        instance.project = false;
        instance.catalog_root = context
            .settings_path
            .parent()
            .expect("settings parent")
            .to_path_buf();
        contexts.push(instance);
    }
    let mut paths = HashSet::new();
    let mut sources = Vec::new();
    for owner in contexts {
        for kind in [
            SourceKind::RegisteredInstance,
            SourceKind::CatalogBase,
            SourceKind::CatalogProject,
            SourceKind::CatalogPersonal,
        ] {
            if !owner.project && kind == SourceKind::CatalogProject {
                continue;
            }
            let source = owner.source(kind);
            let Ok(path) = owner.locator(&source) else {
                continue;
            };
            // Missing files are omitted; nonregular/unreadable existing sources
            // remain visible for a diagnostic snapshot. Never create on read.
            if std::fs::symlink_metadata(&path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            if paths.insert(path) {
                sources.push(source);
            }
        }
    }
    sources
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct RawSourceSnapshot {
    pub(crate) snapshot: SourceSnapshot<Value>,
    pub(crate) candidate_template: Option<Value>,
    raw: OriginPhysicalInput,
    context: SourceReadContext,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
fn source_diagnostic(code: &str) -> SourceDiagnostic {
    SourceDiagnostic {
        code: code.into(),
        entry_hint: None,
        existing_name: None,
        requested_profile: None,
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
fn decode_registered_raw(
    value: &Value,
) -> Result<(Vec<AgentConfig>, CodingAgentProfilesConfig), SourcePreparationError> {
    let map = value
        .as_object()
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let agents: Vec<AgentConfig> = map
        .get("agents")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| SourcePreparationError::InvalidCandidate)?
        .unwrap_or_default();
    let projected = project_physical_profiles(value, true)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let profiles: CodingAgentProfilesConfig = projected
        .get("codingAgentProfiles")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| SourcePreparationError::InvalidCandidate)?
        .unwrap_or_default();
    let mut keys = HashSet::new();
    if agents.iter().any(|agent| {
        agent.id.is_empty() || agent.command.trim().is_empty() || !keys.insert(&agent.id)
    }) {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    if profiles
        .profiles_by_agent
        .values()
        .any(|cells| cells.keys().any(|letter| !is_valid_profile_letter(letter)))
    {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    let mut agents = agents;
    let mut profiles = profiles;
    refresh_agent_id_hashes(&mut agents);
    refresh_pair_id_hashes(&agents, &mut profiles);
    Ok((agents, profiles))
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn read_source_snapshot(
    context: &SourceReadContext,
    source: &SourceRef,
) -> Result<RawSourceSnapshot, SourcePreparationError> {
    let path = context.locator(source)?;
    let raw = OriginPhysicalInput::read(&path);
    source_snapshot_from_original(context, source, raw)
}

fn source_snapshot_from_original(
    context: &SourceReadContext,
    source: &SourceRef,
    raw: OriginPhysicalInput,
) -> Result<RawSourceSnapshot, SourcePreparationError> {
    context.locator(source)?;
    let revision = raw.revision().unwrap_or(SourceRevision::Absent {});
    let mut snapshot = SourceSnapshot {
        source: source.clone(),
        revision,
        availability: SourceAvailability::Absent,
        capabilities: SourceCapabilities::default(),
        agents: Vec::new(),
        profiles: BTreeMap::new(),
        diagnostics: Vec::new(),
        context_revision: context.revision.clone(),
    };
    let candidate_template = None;
    match &raw {
        OriginPhysicalInput::Absent => {}
        OriginPhysicalInput::Bytes(bytes) => {
            let path = context.locator(source)?;
            let ambiguity = duplicate_profile_document(bytes, &path);
            let registered_gate = if source.kind == SourceKind::RegisteredInstance {
                registered_profile_raw_gate(&context.settings_path).err()
            } else {
                None
            };
            let catalog_gate = if source.kind != SourceKind::RegisteredInstance {
                crate::config::coding_agents_catalog::parse_strict_json(bytes)
                    .and_then(|value| decode_physical_catalog(&value))
                    .err()
                    .map(|reason| format!("{}: {reason}", path.display()))
            } else {
                None
            };
            let invalid = ambiguity.or(registered_gate).or(catalog_gate);
            if let Some(reason) = invalid {
                snapshot.availability = SourceAvailability::Invalid;
                let code = [
                    "duplicateJsonMember",
                    "invalidLegacyProfileOrphans",
                    "sourceProfileConflict",
                ]
                .into_iter()
                .find(|code| reason.contains(code))
                .unwrap_or("invalidSource");
                let mut diagnostic = source_diagnostic(code);
                diagnostic.entry_hint = Some(reason);
                snapshot.diagnostics.push(diagnostic);
                return Ok(RawSourceSnapshot {
                    snapshot,
                    candidate_template: None,
                    raw,
                    context: context.clone(),
                });
            }
            let decoded = if source.kind == SourceKind::RegisteredInstance {
                raw.value()
                    .ok_or(SourcePreparationError::InvalidCandidate)
                    .and_then(|value| {
                        let (agents, profiles) = decode_registered_raw(&value)?;
                        snapshot.profiles = profiles.profiles_by_agent;
                        let mut view = value.clone();
                        refresh_physical_source_rows(&mut view, true)
                            .map_err(|_| SourcePreparationError::InvalidCandidate)?;
                        snapshot.agents = view
                            .get("agents")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let entries: Vec<_> = agents
                            .iter()
                            .map(|agent| IdentitySnapshotEntry {
                                key: &agent.id,
                                name: &agent.label,
                                command: &agent.command,
                                envs: &agent.envs,
                                profiles: snapshot.profiles.get(&agent.id),
                            })
                            .collect();
                        if let Err(error) = validate_identity_snapshot(&entries) {
                            snapshot.diagnostics.push(SourceDiagnostic {
                                code: format!("{:?}", error.code),
                                entry_hint: Some(error.entry_hint),
                                existing_name: Some(error.existing_name),
                                requested_profile: error.requested_profile,
                            });
                        }
                        Ok(())
                    })
            } else {
                crate::config::coding_agents_catalog::parse_strict_json(bytes)
                    .and_then(|value| {
                        let (agents, profiles) = decode_physical_catalog(&value)?;
                        snapshot.profiles = profiles.profiles_by_agent;
                        let mut view = value.clone();
                        refresh_physical_source_rows(&mut view, false)?;
                        snapshot.agents = view
                            .get("agents")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter(|row| row.get("remove") != Some(&Value::Bool(true)))
                            .cloned()
                            .collect();
                        let active: HashSet<_> =
                            agents.iter().map(|agent| agent.id.as_str()).collect();
                        for row in value
                            .get("agents")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            if let Some(key) = row
                                .get("key")
                                .and_then(Value::as_str)
                                .filter(|key| !active.contains(key))
                            {
                                let mut diagnostic = source_diagnostic("incompletePhysicalRow");
                                diagnostic.entry_hint = Some(key.to_owned());
                                snapshot.diagnostics.push(diagnostic);
                            }
                        }
                        Ok(())
                    })
                    .map_err(|_| SourcePreparationError::InvalidCandidate)
            };
            match decoded {
                Ok(()) => {
                    snapshot.availability = SourceAvailability::Available;
                    snapshot.capabilities.available_read = true;
                    snapshot.capabilities.editable_save = matches!(
                        source.kind,
                        SourceKind::RegisteredInstance | SourceKind::CatalogPersonal
                    );
                    snapshot.capabilities.editable_save &= source.scope == SourceScope::Project
                        || source.kind == SourceKind::RegisteredInstance;
                    snapshot.capabilities.editable_save &= source.kind
                        == SourceKind::RegisteredInstance
                        || raw
                            .value()
                            .is_some_and(|value| value.get("managed").is_none());
                    snapshot.capabilities.repair_by_owner = false;
                }
                Err(SourcePreparationError::IndependentFormatRequired) => {
                    snapshot.availability = SourceAvailability::IndependentFormatRequired;
                    snapshot
                        .diagnostics
                        .push(source_diagnostic("independentFormatRequired"));
                }
                Err(_) => {
                    snapshot.availability = SourceAvailability::Invalid;
                    snapshot.agents.clear();
                    snapshot.profiles.clear();
                    snapshot
                        .diagnostics
                        .push(source_diagnostic("invalidSource"));
                }
            }
        }
        OriginPhysicalInput::Unknown => {
            snapshot.availability = SourceAvailability::Invalid;
            snapshot
                .diagnostics
                .push(source_diagnostic("unreadableSource"));
        }
    }
    Ok(RawSourceSnapshot {
        snapshot,
        candidate_template,
        raw,
        context: context.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) enum LegacyOriginUnit {
    Entry(String),
    Profile(String, String),
    Default(String),
    Presentation(String, String),
    Slot(String),
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl LegacyOriginUnit {
    fn paths(&self) -> Vec<Vec<String>> {
        let p = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect();
        match self {
            Self::Entry(_) => vec![p(&["agents"])],
            Self::Profile(key, letter) => vec![
                p(&["agents"]),
                p(&["codingAgentProfiles", "profilesByAgent", key, letter]),
                p(&["codingAgentProfiles", "identityByAgent", key]),
            ],
            Self::Default(matrix) => {
                vec![p(&["codingAgentProfiles", "defaultProfileByAgent", matrix])]
            }
            Self::Presentation(key, letter) => vec![p(&[
                "codingAgentProfiles",
                "profileLabelsByAgent",
                key,
                letter,
            ])],
            Self::Slot(letter) => vec![p(&["codingAgentProfiles", "profileSlots", letter])],
        }
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
fn origin_paths_overlap(a: &[String], b: &[String]) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
struct OriginBinding {
    registered: Option<SourceRevision>,
    settings: Option<SourceRevision>,
    overlay: Option<SourceRevision>,
    context: SourceReadContext,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct OriginInputs {
    registered: OriginPhysicalInput,
    settings: OriginPhysicalInput,
    overlay: OriginPhysicalInput,
    binding: OriginBinding,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl OriginInputs {
    fn read(context: &SourceReadContext) -> Self {
        let registered = OriginPhysicalInput::read(&agents_instance_path(&context.settings_path));
        let settings = OriginPhysicalInput::read(&context.settings_path);
        let overlay = OriginPhysicalInput::read(
            &context
                .settings_path
                .with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        );
        Self {
            binding: OriginBinding {
                registered: registered.revision(),
                settings: settings.revision(),
                overlay: overlay.revision(),
                context: context.clone(),
            },
            registered,
            settings,
            overlay,
        }
    }

    /// At most two retries of read-only preparation; callers must supply a fresh
    /// backend context on revalidation, not rely on the registered30 revision.
    pub(crate) fn capture(context: &SourceReadContext) -> Result<Self, SourcePreparationError> {
        for _ in 0..=2 {
            let first = Self::read(context);
            let second = Self::read(context);
            if first.binding == second.binding {
                return Ok(first);
            }
        }
        Err(SourcePreparationError::StaleRevision)
    }

    pub(crate) fn revalidate(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourcePreparationError> {
        if context != &self.binding.context {
            return Err(SourcePreparationError::ContextChanged);
        }
        let fresh = Self::read(context);
        if fresh.binding != self.binding {
            return Err(SourcePreparationError::StaleRevision);
        }
        if [
            self.binding.registered.as_ref(),
            self.binding.settings.as_ref(),
            self.binding.overlay.as_ref(),
        ]
        .iter()
        .any(|revision| revision.is_none())
        {
            return Err(SourcePreparationError::OriginUnavailable);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct LegacyCompatibilityOrigin {
    applied_paths: Vec<Vec<String>>,
    unknown_paths: Vec<Vec<String>>,
    represented: std::collections::BTreeSet<LegacyOriginUnit>,
    binding: OriginBinding,
    pub(crate) base: Value,
    pub(crate) effective: Value,
    pub(crate) overlay_diagnostics: Vec<crate::config::local_overlay::OverlayDiagnostic>,
}

/// Evidence issued by the backend that owns the original selection. No serde
/// implementation: source save payloads cannot claim an authorization or link.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct LegacyOriginLinkProof {
    binding: OriginBinding,
    units: std::collections::BTreeSet<LegacyOriginUnit>,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl LegacyCompatibilityOrigin {
    pub(crate) fn classify(inputs: &OriginInputs) -> Self {
        let mut unknown_paths = Vec::new();
        let mut base = match inputs.settings.value() {
            Some(value @ Value::Object(_)) => value,
            _ if inputs.settings == OriginPhysicalInput::Absent => {
                serde_json::to_value(AppSettings::default()).unwrap_or(Value::Null)
            }
            _ => {
                unknown_paths.push(Vec::new());
                Value::Null
            }
        };
        let settings_base = base.clone();
        let raw30 = inputs.registered.value();
        if let Some(raw) = &raw30 {
            if decode_registered_raw(raw).is_err() {
                unknown_paths.push(vec!["agents".into()]);
                unknown_paths.push(vec!["codingAgentProfiles".into()]);
            } else if let (Some(dst), Some(src)) = (base.as_object_mut(), raw.as_object()) {
                dst.extend(agents_instance_groups(src));
            }
        } else if inputs.registered != OriginPhysicalInput::Absent {
            unknown_paths.push(vec!["agents".into()]);
            unknown_paths.push(vec!["codingAgentProfiles".into()]);
        }
        let before = base.clone();
        let mut effective = base.clone();
        let mut overlay = match &inputs.overlay {
            OriginPhysicalInput::Absent => LocalSettingsOverlay::default(),
            OriginPhysicalInput::Bytes(bytes) => LocalSettingsOverlay::from_overlay_bytes_paired(
                &mut effective,
                bytes,
                OVERLAY_INELIGIBLE_DISK_KEYS,
                OVERLAY_INELIGIBLE_LEGACY_KEYS,
                OVERLAY_DERIVED_ID_CLOSURES,
                CONFIGURATION_COMMAND_PAIRS,
            ),
            OriginPhysicalInput::Unknown => {
                unknown_paths.push(Vec::new());
                LocalSettingsOverlay::default()
            }
        };
        // Same project decoder and AppSettings decoder as the ordinary loader,
        // before its order finalization/repair/adoption and without publication.
        let decode = |raw: &Value| {
            let mut probe = raw.clone();
            if let Some(root) = probe.as_object_mut() {
                root.entry("agents")
                    .or_insert_with(|| Value::Array(Vec::new()));
            }
            let instance_base = production_instance_base();
            apply_project_decode_to_value(
                &mut probe,
                instance_base.as_deref(),
                &projects::FsCandidateResolver,
            );
            serde_json::from_value::<AppSettings>(probe)
        };
        if let Err(error) = decode(&effective) {
            if !overlay.is_empty() && decode(&before).is_ok() {
                overlay = overlay.into_undecodable(error.to_string());
                effective = before;
            } else {
                unknown_paths.push(Vec::new());
            }
        }
        let mut represented = std::collections::BTreeSet::new();
        for value in inputs
            .settings
            .value()
            .iter()
            .chain(raw30.iter())
            .chain(std::iter::once(&effective))
        {
            collect_origin_units(value, &mut represented, &mut unknown_paths);
        }
        Self {
            applied_paths: overlay.owned_paths().to_vec(),
            unknown_paths,
            represented,
            binding: inputs.binding.clone(),
            base: settings_base,
            effective,
            overlay_diagnostics: overlay.diagnostics("settings overlay"),
        }
    }

    pub(crate) fn require_disjoint(
        &self,
        units: impl IntoIterator<Item = LegacyOriginUnit>,
    ) -> Result<(), SourcePreparationError> {
        for unit in units {
            for path in unit.paths() {
                if self
                    .unknown_paths
                    .iter()
                    .any(|unknown| origin_paths_overlap(unknown, &path))
                {
                    return Err(SourcePreparationError::OriginUnavailable);
                }
                if self
                    .applied_paths
                    .iter()
                    .any(|owned| origin_paths_overlap(owned, &path))
                {
                    return Err(SourcePreparationError::LegacyOriginUnresolved);
                }
            }
        }
        Ok(())
    }

    /// Only original source-aware selection evidence binds a default N to K/L.
    /// The future inventory owner supplies authorization and full target CAS;
    /// this pure seam verifies that evidence against the captured source and
    /// complete origin binding, without scanning or mutating any matrix.
    #[allow(clippy::too_many_arguments)] // Complete original source and target CAS evidence.
    pub(crate) fn bind_original_selection(
        &self,
        snapshot: &RawSourceSnapshot,
        selection: &ConfigurationRef,
        original_source_revision: &SourceRevision,
        original_context_revision: &str,
        original_target: &[u8],
        current_target: &[u8],
        matrix: Option<&str>,
    ) -> Result<LegacyOriginLinkProof, SourcePreparationError> {
        let slot = if matrix.is_some() {
            SourceOriginalSlot::MatrixDefault
        } else {
            SourceOriginalSlot::Decision
        };
        self.bind_original_selection_at(
            snapshot,
            selection,
            original_source_revision,
            original_context_revision,
            original_target,
            current_target,
            matrix,
            &slot,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bind_original_selection_at(
        &self,
        snapshot: &RawSourceSnapshot,
        selection: &ConfigurationRef,
        original_source_revision: &SourceRevision,
        original_context_revision: &str,
        original_target: &[u8],
        current_target: &[u8],
        matrix: Option<&str>,
        slot: &SourceOriginalSlot,
    ) -> Result<LegacyOriginLinkProof, SourcePreparationError> {
        if matrix.is_some() != matches!(slot, SourceOriginalSlot::MatrixDefault) {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        if original_target != current_target
            || selection.version != 1
            || selection.source != snapshot.snapshot.source
            || original_source_revision != &snapshot.snapshot.revision
            || original_context_revision != snapshot.snapshot.context_revision
            || self.binding.context != snapshot.context
            || self.binding.registered.as_ref() != Some(original_source_revision)
        {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let target: Value = serde_json::from_slice(current_target)
            .map_err(|_| SourcePreparationError::OriginUnavailable)?;
        let stored = slot.stored(&target);
        let stored: ConfigurationRef = serde_json::from_value(
            stored
                .cloned()
                .ok_or(SourcePreparationError::LegacyOriginUnresolved)?,
        )
        .map_err(|_| SourcePreparationError::LegacyOriginUnresolved)?;
        if &stored != selection {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let key = selection
            .legacy_entry_hint
            .as_deref()
            .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
        let raw = snapshot
            .raw
            .value()
            .ok_or(SourcePreparationError::OriginUnavailable)?;
        let (agents, profiles) = decode_registered_raw(&raw)?;
        let agent = agents
            .iter()
            .find(|agent| agent.id == key)
            .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
        if configuration_identity(&agent.command, &agent.envs) != selection.calculated_id {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let mut units = std::collections::BTreeSet::from([LegacyOriginUnit::Entry(key.into())]);
        if let Some(letter) = &selection.requested_profile {
            let cell = profiles
                .profiles_by_agent
                .get(key)
                .and_then(|cells| cells.get(letter))
                .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
            if selection.profile_calculated_id.as_deref() != Some(profile_identity(cell).as_str()) {
                return Err(SourcePreparationError::LegacyOriginUnresolved);
            }
            units.insert(LegacyOriginUnit::Profile(key.into(), letter.clone()));
        }
        if let Some(matrix) = matrix {
            units.insert(LegacyOriginUnit::Default(matrix.into()));
        }
        Ok(LegacyOriginLinkProof {
            binding: self.binding.clone(),
            units,
        })
    }

    pub(crate) fn require_legacy_units(
        &self,
        units: impl IntoIterator<Item = LegacyOriginUnit>,
        proof: Option<&LegacyOriginLinkProof>,
    ) -> Result<(), SourcePreparationError> {
        for unit in units {
            match self.require_disjoint([unit.clone()]) {
                Ok(()) => {}
                Err(SourcePreparationError::LegacyOriginUnresolved)
                    if proof.is_some_and(|proof| {
                        proof.binding == self.binding && proof.units.contains(&unit)
                    }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
fn collect_origin_units(
    value: &Value,
    out: &mut std::collections::BTreeSet<LegacyOriginUnit>,
    unknown: &mut Vec<Vec<String>>,
) {
    if let Some(agents) = value.get("agents") {
        let mut keys = HashSet::new();
        match agents.as_array() {
            Some(agents) => {
                for agent in agents {
                    match agent.get("id").and_then(Value::as_str) {
                        Some(key) if !key.is_empty() && keys.insert(key) => {
                            out.insert(LegacyOriginUnit::Entry(key.into()));
                        }
                        _ => unknown.push(vec!["agents".into()]),
                    }
                }
            }
            None => unknown.push(vec!["agents".into()]),
        }
    }
    let Some(profiles) = value.get("codingAgentProfiles") else {
        return;
    };
    if !profiles.is_object() {
        unknown.push(vec!["codingAgentProfiles".into()]);
        return;
    }
    for name in [
        "profilesByAgent",
        "profileLabelsByAgent",
        "identityByAgent",
        "defaultProfileByAgent",
        "profileSlots",
    ] {
        let Some(map) = profiles.get(name) else {
            continue;
        };
        let Some(map) = map.as_object() else {
            unknown.push(vec!["codingAgentProfiles".into(), name.into()]);
            continue;
        };
        for (key, value) in map {
            match name {
                "defaultProfileByAgent" => {
                    out.insert(LegacyOriginUnit::Default(key.clone()));
                }
                "profileSlots" => {
                    out.insert(LegacyOriginUnit::Slot(key.clone()));
                }
                "identityByAgent" => {
                    out.insert(LegacyOriginUnit::Entry(key.clone()));
                }
                _ => {
                    if let Some(cells) = value.as_object() {
                        for letter in cells.keys() {
                            out.insert(if name == "profilesByAgent" {
                                LegacyOriginUnit::Profile(key.clone(), letter.clone())
                            } else {
                                LegacyOriginUnit::Presentation(key.clone(), letter.clone())
                            });
                        }
                    } else {
                        unknown.push(vec!["codingAgentProfiles".into(), name.into(), key.clone()]);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct PreparedSourceCandidate {
    pub(crate) source: SourceRef,
    pub(crate) expected_revision: SourceRevision,
    pub(crate) context_revision: String,
    pub(crate) before_bytes: Option<Vec<u8>>,
    pub(crate) serialized_after: Vec<u8>,
    pub(crate) persisted_names: Vec<(String, String)>,
    origin_binding: Option<OriginBinding>,
}

#[allow(dead_code)] // The later writer calls this while holding its proofs.
impl PreparedSourceCandidate {
    pub(crate) fn revalidate_before_write(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourcePreparationError> {
        if self.context_revision != context.revision {
            return Err(SourcePreparationError::ContextChanged);
        }
        let observed = OriginPhysicalInput::read(&context.locator(&self.source)?)
            .revision()
            .ok_or(SourcePreparationError::OriginUnavailable)?;
        if observed != self.expected_revision {
            return Err(SourcePreparationError::StaleRevision);
        }
        if let Some(binding) = &self.origin_binding {
            if binding.registered.is_none()
                || binding.settings.is_none()
                || binding.overlay.is_none()
            {
                return Err(SourcePreparationError::OriginUnavailable);
            }
            let observed = OriginInputs::read(context);
            if &observed.binding != binding {
                return Err(SourcePreparationError::StaleRevision);
            }
        }
        Ok(())
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
fn raw_entries(value: &Value, key_name: &str) -> BTreeMap<String, Value> {
    value
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get(key_name)
                .and_then(Value::as_str)
                .map(|key| (key.into(), entry.clone()))
        })
        .collect()
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
fn changed_origin_units(
    before: &Value,
    after: &Value,
    origin: &LegacyCompatibilityOrigin,
) -> std::collections::BTreeSet<LegacyOriginUnit> {
    let mut affected = std::collections::BTreeSet::new();
    let old = raw_entries(before, "id");
    let new = raw_entries(after, "id");
    for key in old.keys().chain(new.keys()) {
        if old.get(key) != new.get(key) {
            affected.insert(LegacyOriginUnit::Entry(key.clone()));
            affected.extend(
                origin
                    .represented
                    .iter()
                    .filter(|unit| matches!(unit, LegacyOriginUnit::Profile(k, _) if k == key))
                    .cloned(),
            );
        }
    }
    // Include complete raw units, including removals, unknown fields within a
    // cell and absent/null changes. Labels stay a separate presentation domain.
    for map_name in [
        "profilesByAgent",
        "profileLabelsByAgent",
        "identityByAgent",
        "defaultProfileByAgent",
        "profileSlots",
    ] {
        let root = |v: &Value| {
            v.get("codingAgentProfiles")
                .and_then(|v| v.get(map_name))
                .cloned()
        };
        let old = root(before);
        let new = root(after);
        if old == new {
            continue;
        }
        let keys: std::collections::BTreeSet<_> = old
            .as_ref()
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.keys())
            .chain(
                new.as_ref()
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|m| m.keys()),
            )
            .cloned()
            .collect();
        for key in keys {
            let a = old.as_ref().and_then(|v| v.get(&key));
            let b = new.as_ref().and_then(|v| v.get(&key));
            if a == b {
                continue;
            }
            match map_name {
                "defaultProfileByAgent" => {
                    affected.insert(LegacyOriginUnit::Default(key));
                }
                "profileSlots" => {
                    affected.insert(LegacyOriginUnit::Slot(key));
                }
                "identityByAgent" => {
                    affected.extend(
                        origin
                            .represented
                            .iter()
                            .filter(|u| matches!(u, LegacyOriginUnit::Profile(k, _) if k == &key))
                            .cloned(),
                    );
                    // A missing/orphan profile container still needs its own
                    // identity ownership checked, not equality of the stamp.
                    if origin.applied_paths.iter().any(|p| {
                        origin_paths_overlap(
                            p,
                            &[
                                "codingAgentProfiles".into(),
                                "identityByAgent".into(),
                                key.clone(),
                            ],
                        )
                    }) {
                        affected.insert(LegacyOriginUnit::Profile(key, String::new()));
                    }
                }
                _ => {
                    let letters: std::collections::BTreeSet<_> = a
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|m| m.keys())
                        .chain(
                            b.and_then(Value::as_object)
                                .into_iter()
                                .flat_map(|m| m.keys()),
                        )
                        .cloned()
                        .collect();
                    for letter in letters {
                        if a.and_then(|v| v.get(&letter)) != b.and_then(|v| v.get(&letter)) {
                            affected.insert(if map_name == "profilesByAgent" {
                                LegacyOriginUnit::Profile(key.clone(), letter)
                            } else {
                                LegacyOriginUnit::Presentation(key.clone(), letter)
                            });
                        }
                    }
                }
            }
        }
    }
    affected
}

#[allow(dead_code)] // Dependency inventory is owned by the later coordinator.
fn unresolved_default_dependency(
    origin: &LegacyCompatibilityOrigin,
    units: &std::collections::BTreeSet<LegacyOriginUnit>,
) -> bool {
    units.iter().any(|unit| {
        matches!(
            unit,
            LegacyOriginUnit::Entry(_) | LegacyOriginUnit::Profile(_, _)
        )
    }) && origin.represented.iter().any(|unit| {
        matches!(unit, LegacyOriginUnit::Default(_))
            && origin.require_disjoint([unit.clone()]).is_err()
    })
}

/// Unknown persisted fields are not discarded by the typed validation view.
/// Schema keys derive from the concrete model; raw absence/null remains raw.
#[allow(dead_code)] // Dormant until the later source protocol cuts.
fn preserve_unknown_fields(before: &Value, after: &Value, model: &Value) -> bool {
    match (before, after, model) {
        (Value::Object(old), Value::Object(new), Value::Object(known)) => {
            old.iter().all(|(key, value)| match known.get(key) {
                Some(shape) => new
                    .get(key)
                    .is_none_or(|next| preserve_unknown_fields(value, next, shape)),
                None => new.get(key) == Some(value),
            })
        }
        (Value::Array(old), Value::Array(new), Value::Array(known)) => {
            let key = if old.first().is_some_and(|v| v.get("id").is_some()) {
                "id"
            } else {
                "key"
            };
            old.iter().all(|entry| {
                let id = entry.get(key);
                match id {
                    Some(id) => match new.iter().find(|entry| entry.get(key) == Some(id)) {
                        Some(next) => known
                            .iter()
                            .find(|entry| entry.get(key) == Some(id))
                            .is_some_and(|shape| preserve_unknown_fields(entry, next, shape)),
                        None => true, // Explicit removal is checked by OperationAffected.
                    },
                    None => true,
                }
            })
        }
        _ => true,
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
fn validate_source_identity_groups(
    before: &Value,
    after: &Value,
    registered: bool,
) -> Result<(), SourcePreparationError> {
    let groups = |value: &Value| -> Result<
        BTreeMap<String, std::collections::BTreeSet<String>>,
        SourcePreparationError,
    > {
        let mut groups = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
        let (agents, profiles) = if registered {
            let (agents, profiles) = decode_registered_raw(value)?;
            (
                agents
                    .into_iter()
                    .map(|a| (a.id, a.command, a.envs))
                    .collect::<Vec<_>>(),
                profiles.profiles_by_agent,
            )
        } else {
            let (agents, profiles) = decode_physical_catalog(value)
                .map_err(|_| SourcePreparationError::InvalidCandidate)?;
            (
                agents
                    .into_iter()
                    .map(|a| (a.id, a.command, a.envs))
                    .collect::<Vec<_>>(),
                profiles.profiles_by_agent,
            )
        };
        for (key, command, envs) in agents {
            groups
                .entry(configuration_identity(&command, &envs))
                .or_default()
                .insert(key.clone());
            for (letter, profile) in profiles
                .get(&key)
                .into_iter()
                .flat_map(|cells| cells.iter())
            {
                groups
                    .entry(format!(
                        "{}/{}",
                        configuration_identity(&command, &envs),
                        profile_identity(profile)
                    ))
                    .or_default()
                    .insert(letter.clone());
            }
        }
        Ok(groups)
    };
    let old = groups(before)?;
    let new = groups(after)?;
    if let Some((id, members)) = new.iter().find(|(id, members)| {
        id.contains("/pid1:")
            && members.len() > 1
            && old.get(*id).is_none_or(|prior| !members.is_subset(prior))
    }) {
        let entries = raw_entries(after, if registered { "id" } else { "key" });
        let profile_key = id.rsplit_once("/pid1:").map(|(key, _)| key);
        let incumbent = profile_key.unwrap_or_else(|| {
            old.get(id)
                .and_then(|prior| prior.iter().find(|key| members.contains(*key)))
                .or_else(|| members.first())
                .expect("duplicate group")
                .as_str()
        });
        return Err(SourcePreparationError::Identity(IdentityValidationError {
            code: if profile_key.is_some() {
                IdentityValidationCode::DuplicateProfileIdentity
            } else {
                IdentityValidationCode::DuplicateConfigurationIdentity
            },
            existing_name: entries
                .get(incumbent)
                .and_then(|entry| entry.get("label"))
                .and_then(Value::as_str)
                .unwrap_or(incumbent)
                .into(),
            entry_hint: incumbent.into(),
            requested_profile: profile_key.and_then(|_| members.last().cloned()),
        }));
    }
    Ok(())
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
/// Grouping preserves each original selection's Entry/Profile/Default relation.
/// Only the complete backend inventory factory may construct this aggregate.
#[derive(Debug)]
pub(crate) struct SourceOriginalProofAggregate {
    originals: Vec<LegacyOriginLinkProof>,
}

fn require_source_original_units(
    origin: &LegacyCompatibilityOrigin,
    units: &std::collections::BTreeSet<LegacyOriginUnit>,
    aggregate: Option<&SourceOriginalProofAggregate>,
) -> Result<(), SourcePreparationError> {
    let Some(aggregate) = aggregate else {
        if unresolved_default_dependency(origin, units) {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        return origin.require_disjoint(units.iter().cloned());
    };
    if aggregate
        .originals
        .iter()
        .any(|proof| proof.binding != origin.binding)
    {
        return Err(SourcePreparationError::StaleRevision);
    }
    for unit in units {
        if origin.require_disjoint([unit.clone()]).is_ok() {
            continue;
        }
        let mut covered = false;
        for proof in aggregate
            .originals
            .iter()
            .filter(|proof| proof.units.contains(unit))
        {
            // This model records binding and unit membership only; the per-unit
            // loop rechecks those facts. Same-Default relation coherence is
            // enforced below; distinct default/profile groups remain separate.
            origin.require_legacy_units([unit.clone()], Some(proof))?;
            covered = true;
        }
        if !covered {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
    }
    let entry_or_profile = units.iter().any(|unit| {
        matches!(
            unit,
            LegacyOriginUnit::Entry(_) | LegacyOriginUnit::Profile(_, _)
        )
    });
    if !entry_or_profile {
        return Ok(());
    }
    for default in origin
        .represented
        .iter()
        .filter(|unit| matches!(unit, LegacyOriginUnit::Default(_)))
    {
        if origin.require_disjoint([default.clone()]).is_ok() {
            continue;
        }
        let mut groups = aggregate
            .originals
            .iter()
            .filter(|proof| proof.units.contains(default));
        let group = groups
            .next()
            .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
        if groups.any(|other| other.units != group.units) {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let keys: Vec<_> = group
            .units
            .iter()
            .filter_map(|unit| match unit {
                LegacyOriginUnit::Entry(key) => Some(key),
                _ => None,
            })
            .collect();
        if keys.len() != 1 {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let key = keys[0];
        let profiles: Vec<_> = group
            .units
            .iter()
            .filter_map(|unit| match unit {
                LegacyOriginUnit::Profile(profile_key, letter) if profile_key == key => {
                    Some(letter)
                }
                _ => None,
            })
            .collect();
        if profiles.len() > 1 {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let changes_profile = units.iter().any(|unit| {
            matches!(unit,
            LegacyOriginUnit::Profile(profile_key, _) if profile_key == key)
        });
        // Missing original selected profile cannot prove disjunction for K/L.
        if changes_profile && profiles.is_empty() {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let linked_change = units.contains(&LegacyOriginUnit::Entry(key.clone()))
            || profiles.iter().any(|letter| {
                units.contains(&LegacyOriginUnit::Profile(key.clone(), (*letter).clone()))
            })
            || units.contains(default);
        if linked_change {
            origin.require_legacy_units([default.clone()], Some(group))?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn prepare_source_candidate(
    snapshot: &RawSourceSnapshot,
    expected: &SourceRevision,
    candidate: &Value,
    origin: Option<&LegacyCompatibilityOrigin>,
) -> Result<PreparedSourceCandidate, SourcePreparationError> {
    prepare_source_candidate_with_original_links(snapshot, expected, candidate, origin, None)
}

pub(crate) fn prepare_source_candidate_with_original_links(
    snapshot: &RawSourceSnapshot,
    expected: &SourceRevision,
    candidate: &Value,
    origin: Option<&LegacyCompatibilityOrigin>,
    proofs: Option<&SourceOriginalProofAggregate>,
) -> Result<PreparedSourceCandidate, SourcePreparationError> {
    if expected != &snapshot.snapshot.revision {
        return Err(SourcePreparationError::StaleRevision);
    }
    if !snapshot.snapshot.capabilities.editable_save
        && !snapshot.snapshot.capabilities.editable_create
    {
        return Err(SourcePreparationError::ReadOnly);
    }
    let registered = snapshot.snapshot.source.kind == SourceKind::RegisteredInstance;
    if registered {
        decode_registered_raw(candidate)?;
    }
    let before = snapshot
        .raw
        .value()
        .or_else(|| snapshot.candidate_template.clone())
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let mut retained_candidate = candidate.clone();
    retain_configuration_commands(
        &mut retained_candidate,
        &before,
        if registered { "id" } else { "key" },
        !registered,
        true,
        true,
    )
    .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let candidate = &retained_candidate;
    let model = if registered {
        let (agents, profiles) = decode_registered_raw(candidate)?;
        serde_json::json!({"agents":agents, "codingAgentProfiles":profiles})
    } else {
        decode_physical_catalog(candidate).map_err(|_| SourcePreparationError::InvalidCandidate)?;
        candidate.clone()
    };
    // Binding fields are validated and preserved by canonical_profile_source,
    // including unresolved raw legacy entries omitted from the editor's ID view.
    let mut unknown_before = before.clone();
    let binding = if registered {
        unknown_before.get_mut("codingAgentProfiles")
    } else {
        Some(&mut unknown_before)
    };
    if let Some(binding) = binding.and_then(Value::as_object_mut) {
        for key in [
            "profileBindingVersion",
            "profilesByBaseHash",
            "profileLabelsByBaseHash",
            if registered {
                "profilesByAgent"
            } else {
                "codingAgentProfiles"
            },
            if registered {
                "profileLabelsByAgent"
            } else {
                "profileLabels"
            },
        ] {
            binding.remove(key);
        }
        if registered {
            binding.remove("identityByAgent");
        }
    }
    // Catalog slots default to an empty object. Compare an omitted default
    // without discarding any actual before/candidate field or opaque data.
    if !registered
        && before
            .get("profileSlots")
            .and_then(Value::as_object)
            .is_some_and(Map::is_empty)
        && candidate.get("profileSlots").is_none()
    {
        unknown_before
            .as_object_mut()
            .expect("catalog object")
            .remove("profileSlots");
    }
    if !preserve_unknown_fields(&unknown_before, candidate, &model) {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    validate_source_identity_groups(&before, candidate, registered)?;
    if registered {
        let origin = origin.ok_or(SourcePreparationError::OriginUnavailable)?;
        if origin.unknown_paths.iter().any(Vec::is_empty) {
            return Err(SourcePreparationError::OriginUnavailable);
        }
        if origin.binding.registered.is_none()
            || origin.binding.settings.is_none()
            || origin.binding.overlay.is_none()
        {
            return Err(SourcePreparationError::OriginUnavailable);
        }
        if origin.binding.context != snapshot.context
            || origin.binding.registered.as_ref() != Some(expected)
        {
            return Err(SourcePreparationError::StaleRevision);
        }
        let units = changed_origin_units(&before, candidate, origin);
        require_source_original_units(origin, &units, proofs)?;
    }
    let mut candidate = candidate.clone();
    let key_name = if registered { "id" } else { "key" };
    let label_name = "label";
    let incumbents = raw_entries(&before, key_name);
    let mut used = std::collections::BTreeSet::new();
    let mut names = Vec::new();
    if let Some(agents) = candidate.get_mut("agents").and_then(Value::as_array_mut) {
        // Retained unchanged names are the incumbents even when a newly added
        // row appears earlier in the candidate's order.
        for agent in agents.iter() {
            let key = agent
                .get(key_name)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if incumbents.get(key).and_then(|a| a.get(label_name)) == agent.get(label_name) {
                if let Some(label) = agent.get(label_name).and_then(Value::as_str) {
                    used.insert(label.trim().to_lowercase());
                }
            }
        }
        for agent in agents {
            let key = agent
                .get(key_name)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let label = agent
                .get(label_name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if incumbents.get(key).and_then(|a| a.get(label_name)) == agent.get(label_name) {
                continue;
            }
            let mut persisted = label.clone();
            let mut suffix = 1;
            while !used.insert(persisted.trim().to_lowercase()) {
                persisted = format!("{label} ({suffix})");
                suffix += 1;
            }
            if persisted != label {
                names.push((label, persisted.clone()));
                agent
                    .as_object_mut()
                    .ok_or(SourcePreparationError::InvalidCandidate)?
                    .insert(label_name.into(), Value::String(persisted));
            }
        }
    }
    // Naming is also part of the full diff. Recheck it before serialization.
    if registered {
        let origin = origin.ok_or(SourcePreparationError::OriginUnavailable)?;
        let units = changed_origin_units(&before, &candidate, origin);
        require_source_original_units(origin, &units, proofs)?;
    }
    let candidate = canonical_profile_source(&before, &candidate, registered)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let mut serialized_after = serde_json::to_vec_pretty(&candidate)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    serialized_after.push(b'\n');
    Ok(PreparedSourceCandidate {
        source: snapshot.snapshot.source.clone(),
        expected_revision: expected.clone(),
        context_revision: snapshot.context.revision.clone(),
        before_bytes: match &snapshot.raw {
            OriginPhysicalInput::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        },
        serialized_after,
        persisted_names: names,
        origin_binding: origin.map(|origin| origin.binding.clone()),
    })
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
const SOURCE_GENERAL_PROTECTED: &[&str] = &[
    "agents",
    "codingAgentProfiles",
    "profileLabels",
    "rootToken",
    "projectPaths",
    "projectPath",
    "archivedProjectPaths",
    "railCollapsedProjects",
    "railFavoritesCollapsed",
    "terminalSnapshotsEnabled",
    "mainGeometry",
    "mainWindowDisplayState",
    "projectPathState",
    "localOverlayState",
];

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct PreparedSettingsWithoutSourceWrite {
    pub(crate) serialized_after: Vec<u8>,
    pub(crate) redecoded_settings: AppSettings,
}

#[allow(dead_code)] // Serde's field table also includes omitted optional fields.
fn source_general_field_names() -> Vec<&'static str> {
    struct Fields<'a>(&'a mut Vec<&'static str>);
    impl<'de> serde::Deserializer<'de> for Fields<'_> {
        type Error = serde::de::value::Error;
        fn deserialize_any<V: serde::de::Visitor<'de>>(
            self,
            _: V,
        ) -> Result<V::Value, Self::Error> {
            Err(serde::de::Error::custom("field table only"))
        }
        fn deserialize_struct<V: serde::de::Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Self::Error> {
            self.0.extend_from_slice(fields);
            Err(serde::de::Error::custom("field table captured"))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
            map enum identifier ignored_any
        }
    }
    let mut names = Vec::new();
    let _ = AppSettings::deserialize(Fields(&mut names));
    names
}

/// General-only pure preparation over the captured settings base. Never calls
/// a saver, migration, export, order/adoption/repair helper, or source writer.
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn prepare_settings_without_source_write(
    inputs: &OriginInputs,
    candidate_general: &Value,
) -> Result<PreparedSettingsWithoutSourceWrite, SourcePreparationError> {
    let origin = LegacyCompatibilityOrigin::classify(inputs);
    let mut out = match &inputs.settings {
        OriginPhysicalInput::Absent => Value::Object(Map::new()),
        OriginPhysicalInput::Bytes(_) => inputs
            .settings
            .value()
            .ok_or(SourcePreparationError::OriginUnavailable)?,
        OriginPhysicalInput::Unknown => return Err(SourcePreparationError::OriginUnavailable),
    };
    // Capture disk restoration before applying the draft. In particular, an
    // overlay-owned general key must retain its raw absent/null/value state,
    // rather than capturing the effective value echoed by the caller.
    let mut restoration_base = out.clone();
    let restoration = match &inputs.overlay {
        OriginPhysicalInput::Bytes(bytes) => LocalSettingsOverlay::from_overlay_bytes_paired(
            &mut restoration_base,
            bytes,
            OVERLAY_INELIGIBLE_DISK_KEYS,
            OVERLAY_INELIGIBLE_LEGACY_KEYS,
            OVERLAY_DERIVED_ID_CLOSURES,
            CONFIGURATION_COMMAND_PAIRS,
        ),
        _ => LocalSettingsOverlay::default(),
    };
    let map = out
        .as_object_mut()
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let general = candidate_general
        .as_object()
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let public = serde_json::to_value(AppSettings::default())
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let fields = source_general_field_names();
    if general.keys().any(|key| {
        SOURCE_GENERAL_PROTECTED.contains(&key.as_str()) || !fields.contains(&key.as_str())
    }) {
        return Err(SourcePreparationError::ProtectedSettingsField);
    }
    if public
        .as_object()
        .ok_or(SourcePreparationError::InvalidCandidate)?
        .keys()
        .any(|key| !SOURCE_GENERAL_PROTECTED.contains(&key.as_str()) && !general.contains_key(key))
    {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    // Only caller-owned general keys are updated. Unknown/source/placement keys
    // retain exact raw absence, null or value, even when the typed view differs.
    for key in fields {
        if !SOURCE_GENERAL_PROTECTED.contains(&key) && !general.contains_key(key) {
            map.remove(key);
        }
    }
    for (key, value) in general {
        map.insert(key.clone(), value.clone());
    }
    let mut effective = out.clone();
    if let (Some(dst), Some(raw)) = (
        effective.as_object_mut(),
        inputs
            .registered
            .value()
            .filter(|raw| decode_registered_raw(raw).is_ok())
            .and_then(|v| v.as_object().cloned()),
    ) {
        dst.extend(agents_instance_groups(&raw));
    }
    let unmerged = effective.clone();
    if let OriginPhysicalInput::Bytes(bytes) = &inputs.overlay {
        let overlay = LocalSettingsOverlay::from_overlay_bytes_paired(
            &mut effective,
            bytes,
            OVERLAY_INELIGIBLE_DISK_KEYS,
            OVERLAY_INELIGIBLE_LEGACY_KEYS,
            OVERLAY_DERIVED_ID_CLOSURES,
            CONFIGURATION_COMMAND_PAIRS,
        );
        let mut probe = effective.clone();
        if let Some(root) = probe.as_object_mut() {
            root.entry("agents")
                .or_insert_with(|| Value::Array(Vec::new()));
        }
        if !overlay.is_empty() && serde_json::from_value::<AppSettings>(probe).is_err() {
            effective = unmerged;
        } else {
            restoration.restore_base(out.as_object_mut().expect("object checked"));
        }
    }
    if origin.unknown_paths.iter().any(Vec::is_empty) {
        return Err(SourcePreparationError::OriginUnavailable);
    }
    if let Some(root) = effective.as_object_mut() {
        root.entry("agents")
            .or_insert_with(|| Value::Array(Vec::new()));
    }
    let base = production_instance_base();
    apply_project_decode_to_value(
        &mut effective,
        base.as_deref(),
        &projects::FsCandidateResolver,
    );
    let redecoded_settings =
        serde_json::from_value(effective).map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let mut serialized_after =
        serde_json::to_vec_pretty(&out).map_err(|_| SourcePreparationError::InvalidCandidate)?;
    serialized_after.push(b'\n');
    Ok(PreparedSettingsWithoutSourceWrite {
        serialized_after,
        redecoded_settings,
    })
}

fn identity_preimage(parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut scalars = Vec::new();
    for part in parts {
        scalars.extend(
            part.as_ref()
                .chars()
                .flat_map(char::to_lowercase)
                .filter(|scalar| !scalar.is_whitespace()),
        );
    }
    scalars.sort_unstable();
    scalars.into_iter().collect()
}

fn configuration_identity_preimage(command: &str, envs: &[CodingAgentEnv]) -> String {
    identity_preimage(
        std::iter::once(command).chain(
            envs.iter()
                .flat_map(|row| [row.key.as_str(), row.value.as_str()]),
        ),
    )
}

fn profile_identity_preimage(profile: &ProfileCellConfig) -> String {
    identity_preimage(
        std::iter::once(profile.command.as_str()).chain(
            profile
                .env
                .iter()
                .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
        ),
    )
}

fn identity_digest(prefix: &str, preimage: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{prefix}{:x}", Sha256::digest(preimage.as_bytes()))
}

/// Includes every configured env row, even disabled rows. Labels, backend and
/// other metadata do not participate. Anagrams and repeated scalars are retained.
pub fn configuration_identity(command: &str, envs: &[CodingAgentEnv]) -> String {
    identity_digest("cid1:", &configuration_identity_preimage(command, envs))
}

fn configuration_identity_debug(command: &str, envs: &[CodingAgentEnv]) -> Vec<String> {
    std::iter::once(command.to_owned())
        .chain(
            envs.iter()
                .flat_map(|row| [row.key.clone(), row.value.clone()]),
        )
        .collect()
}

pub(crate) fn pair_identity_debug(
    command: &str,
    envs: &[CodingAgentEnv],
    profile: &ProfileCellConfig,
) -> Vec<String> {
    let mut leaves = configuration_identity_debug(command, envs);
    leaves.push(profile.command.clone());
    leaves.extend(
        profile
            .env
            .iter()
            .flat_map(|(key, value)| [key.clone(), value.clone()]),
    );
    leaves
}

/// Refresh informational metadata only; stable IDs and row order stay untouched.
pub fn refresh_agent_id_hashes(agents: &mut [AgentConfig]) {
    for agent in agents {
        agent.id_hash = Some(configuration_identity(&agent.command, &agent.envs));
        agent.id_hash_debug = Some(configuration_identity_debug(&agent.command, &agent.envs));
    }
}

/// Informational identity of one base/profile pair, using the existing scalar rules.
pub(crate) fn pair_identity(
    command: &str,
    envs: &[CodingAgentEnv],
    profile: &ProfileCellConfig,
) -> String {
    identity_digest(
        "cid1:",
        &identity_preimage([
            configuration_identity_preimage(command, envs),
            profile_identity_preimage(profile),
        ]),
    )
}

fn refresh_pair_id_hashes(agents: &[AgentConfig], profiles: &mut CodingAgentProfilesConfig) {
    for (id, cells) in &mut profiles.profiles_by_agent {
        // Historical duplicate IDs use the same first-match rule as spawn.
        let agent = agents.iter().find(|agent| agent.id == *id);
        for cell in cells.values_mut() {
            cell.id_hash = agent.map(|agent| pair_identity(&agent.command, &agent.envs, cell));
            cell.id_hash_debug =
                agent.map(|agent| pair_identity_debug(&agent.command, &agent.envs, cell));
        }
    }
}

/// Project only identity inputs from the first physical row. Protected groups
/// are best effort here; their existing full decoders still own save errors.
fn physical_pair_base(agents: Option<&Value>, id: &str) -> Option<(String, Vec<CodingAgentEnv>)> {
    let row = agents?
        .as_array()?
        .iter()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))?
        .as_object()?;
    let command = raw_configuration_command(row, "baseCommand")
        .ok()?
        .unwrap_or_default();
    let envs = match row.get("envs") {
        Some(value) => serde_json::from_value(value.clone()).ok()?,
        None => Vec::new(),
    };
    Some((command.to_owned(), envs))
}

fn refresh_physical_pair_id_hashes(agents: Option<&Value>, profiles: &mut Value) {
    let Some(rows) = profiles
        .get_mut("profilesByAgent")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for (id, cells) in rows {
        let base = physical_pair_base(agents, id);
        for raw in cells
            .as_object_mut()
            .into_iter()
            .flat_map(|cells| cells.values_mut())
        {
            let metadata = base.as_ref().and_then(|(command, envs)| {
                // Decode the cell's existing aliases/env contract without rewriting it.
                let cell: ProfileCellConfig = serde_json::from_value(raw.clone()).ok()?;
                Some((
                    pair_identity(command, envs, &cell),
                    pair_identity_debug(command, envs, &cell),
                ))
            });
            if let Some(object) = raw.as_object_mut() {
                match metadata {
                    Some((hash, debug)) => {
                        object.insert("id_hash".into(), Value::String(hash));
                        object.insert("id_hash_debug".into(), serde_json::json!(debug));
                    }
                    None => {
                        object.remove("id_hash");
                        object.remove("id_hash_debug");
                    }
                }
            }
        }
    }
}

/// A configured empty cell has an identity; an absent cell has no identity.
/// The base configuration ID and decorative profile letter are not inputs.
pub fn profile_identity(profile: &ProfileCellConfig) -> String {
    identity_digest("pid1:", &profile_identity_preimage(profile))
}

/// Borrowed projection lets each source retain its own complete agent model.
pub struct IdentitySnapshotEntry<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub command: &'a str,
    pub envs: &'a [CodingAgentEnv],
    pub profiles: Option<&'a BTreeMap<String, ProfileCellConfig>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IdentityValidationCode {
    DuplicateConfigurationIdentity,
    DuplicateProfileIdentity,
    IdentityHashCollision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: existing Coding Agent '{existing_name}'")]
#[serde(rename_all = "camelCase")]
pub struct IdentityValidationError {
    pub code: IdentityValidationCode,
    pub existing_name: String,
    pub entry_hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
}

/// Validate one physical source at a time; this never compares other sources.
/// Historical duplicate policy and update exclusion are caller-owned: pass the
/// complete proposed snapshot, replacing the updated entry rather than appending.
pub fn validate_identity_snapshot(
    entries: &[IdentitySnapshotEntry<'_>],
) -> Result<(), IdentityValidationError> {
    validate_identity_snapshot_with_digest(entries, identity_digest)
}

fn validate_identity_snapshot_with_digest(
    entries: &[IdentitySnapshotEntry<'_>],
    digest: impl Fn(&str, &str) -> String,
) -> Result<(), IdentityValidationError> {
    let mut configurations = BTreeMap::<String, (String, &str)>::new();
    // Profile identities are local to an agent. Collision evidence, however,
    // must not be discarded simply because a second preimage is in another one.
    let mut profile_preimages = BTreeMap::<String, (String, &str)>::new();
    for entry in entries {
        let preimage = configuration_identity_preimage(entry.command, entry.envs);
        let id = digest("cid1:", &preimage);
        if let Some((existing_preimage, existing_name)) = configurations.get(&id) {
            return Err(IdentityValidationError {
                code: if existing_preimage == &preimage {
                    IdentityValidationCode::DuplicateConfigurationIdentity
                } else {
                    IdentityValidationCode::IdentityHashCollision
                },
                existing_name: (*existing_name).to_owned(),
                entry_hint: entry.key.to_owned(),
                requested_profile: None,
            });
        }
        configurations.insert(id, (preimage, entry.name));
        let mut local_profiles = BTreeMap::<String, &str>::new();
        for (letter, profile) in entry.profiles.into_iter().flat_map(|cells| cells.iter()) {
            let preimage = profile_identity_preimage(profile);
            let id = digest("pid1:", &preimage);
            if let Some((existing_preimage, existing_name)) = profile_preimages.get(&id) {
                if existing_preimage != &preimage {
                    return Err(IdentityValidationError {
                        code: IdentityValidationCode::IdentityHashCollision,
                        existing_name: (*existing_name).to_owned(),
                        entry_hint: entry.key.to_owned(),
                        requested_profile: Some(letter.clone()),
                    });
                }
            }
            profile_preimages
                .entry(id.clone())
                .or_insert((preimage, entry.name));
            if local_profiles.insert(id, letter).is_some() {
                return Err(IdentityValidationError {
                    code: IdentityValidationCode::DuplicateProfileIdentity,
                    existing_name: entry.name.to_owned(),
                    entry_hint: entry.key.to_owned(),
                    requested_profile: Some(letter.clone()),
                });
            }
        }
    }
    Ok(())
}

// Canonical pair signatures deliberately omit labels, order and supplied hashes.
// Counts retain repeated legacy rows, including repeated stable IDs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PhysicalIdentityViolation {
    Configuration([(String, String); 2]),
    Profile([(String, String, String); 2]),
}

fn physical_identity_violations(
    groups: &Map<String, Value>,
    configurations_editable: bool,
    profiles_editable: bool,
) -> Result<BTreeMap<PhysicalIdentityViolation, (usize, IdentityValidationError)>, String> {
    let agents: Vec<AgentConfig> = groups
        .get("agents")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let profiles: CodingAgentProfilesConfig = groups
        .get("codingAgentProfiles")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let mut violations = BTreeMap::new();
    let mut record = |key, error| {
        let entry = violations.entry(key).or_insert((0usize, error));
        entry.0 += 1;
    };
    if configurations_editable {
        collect_configuration_identity_violations(&agents, &mut record);
    }
    if profiles_editable {
        collect_profile_identity_violations(&agents, &profiles, &mut record);
    }
    Ok(violations)
}

fn collect_configuration_identity_violations(
    agents: &[AgentConfig],
    record: &mut impl FnMut(PhysicalIdentityViolation, IdentityValidationError),
) {
    let preimages: Vec<_> = agents
        .iter()
        .map(|agent| configuration_identity_preimage(&agent.command, &agent.envs))
        .collect();
    let hashes: Vec<_> = preimages
        .iter()
        .map(|p| identity_digest("cid1:", p))
        .collect();
    for (index, agent) in agents.iter().enumerate() {
        for other in 0..index {
            if hashes[index] != hashes[other]
                || (preimages[index] == preimages[other] && agent.id != agents[other].id)
            {
                continue;
            }
            let mut pair = [
                (agents[other].id.clone(), preimages[other].clone()),
                (agent.id.clone(), preimages[index].clone()),
            ];
            pair.sort();
            record(
                PhysicalIdentityViolation::Configuration(pair),
                IdentityValidationError {
                    code: if preimages[index] == preimages[other] {
                        IdentityValidationCode::DuplicateConfigurationIdentity
                    } else {
                        IdentityValidationCode::IdentityHashCollision
                    },
                    existing_name: agents[other].label.clone(),
                    entry_hint: agent.id.clone(),
                    requested_profile: None,
                },
            );
        }
    }
}

fn collect_profile_identity_violations(
    agents: &[AgentConfig],
    profiles: &CodingAgentProfilesConfig,
    record: &mut impl FnMut(PhysicalIdentityViolation, IdentityValidationError),
) {
    let group_for = |id: &str| {
        agents
            .iter()
            .find(|agent| agent.id == id)
            .map(|agent| configuration_identity(&agent.command, &agent.envs))
            .unwrap_or_else(|| format!("orphan:{id}"))
    };
    let mut groups = HashSet::new();
    let cells: Vec<_> = profiles
        .profiles_by_agent
        .iter()
        .filter(|(id, _)| groups.insert(group_for(id)))
        .flat_map(|(id, cells)| {
            cells
                .iter()
                .filter(|(letter, _)| is_valid_profile_letter(letter))
                .map(move |(letter, cell)| {
                    let preimage = profile_identity_preimage(cell);
                    let hash = identity_digest("pid1:", &preimage);
                    (id, letter, preimage, hash)
                })
        })
        .collect();
    for (index, (id, letter, preimage, hash)) in cells.iter().enumerate() {
        for (old_id, old_letter, old_preimage, old_hash) in &cells[..index] {
            if hash != old_hash || (id != old_id && preimage == old_preimage) {
                continue;
            }
            let mut pair = [
                (
                    group_for(old_id),
                    (*old_letter).clone(),
                    old_preimage.clone(),
                ),
                (group_for(id), (*letter).clone(), preimage.clone()),
            ];
            pair.sort();
            record(
                PhysicalIdentityViolation::Profile(pair),
                IdentityValidationError {
                    code: if preimage == old_preimage {
                        IdentityValidationCode::DuplicateProfileIdentity
                    } else {
                        IdentityValidationCode::IdentityHashCollision
                    },
                    existing_name: agents
                        .iter()
                        .find(|agent| &agent.id == *old_id)
                        .map(|agent| agent.label.clone())
                        .unwrap_or_else(|| (*old_id).clone()),
                    entry_hint: (*id).clone(),
                    requested_profile: Some((*letter).clone()),
                },
            );
        }
    }
}

fn validate_physical_identity_delta(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    configurations_editable: bool,
    profiles_editable: bool,
) -> Result<(), String> {
    let project = |groups: &Map<String, Value>| {
        let source = Value::Object(groups.clone());
        if physical_profile_rows(&source, true).is_err() {
            Ok(registered_profiles_for_read(&source))
        } else {
            project_physical_profiles(&source, true)
        }
    };
    let before_view = project(before)?;
    let after_view = project(after)?;
    let old = physical_identity_violations(
        before_view.as_object().unwrap(),
        configurations_editable,
        profiles_editable,
    )?;
    let new = physical_identity_violations(
        after_view.as_object().unwrap(),
        configurations_editable,
        profiles_editable,
    )?;
    for (signature, (count, mut error)) in new {
        if count > old.get(&signature).map_or(0, |(count, _)| *count) {
            // A prepended new row must still name an incumbent from BEFORE.
            if let PhysicalIdentityViolation::Configuration(pair) = &signature {
                let incumbent = before
                    .get("agents")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|raw| serde_json::from_value::<AgentConfig>(raw.clone()).ok())
                    .find(|agent| {
                        pair.contains(&(
                            agent.id.clone(),
                            configuration_identity_preimage(&agent.command, &agent.envs),
                        ))
                    });
                if let Some(incumbent) = incumbent {
                    error.existing_name = incumbent.label;
                }
            }
            return Err(error.to_string());
        }
    }
    Ok(())
}

/// Caller excludes the edited entry and supplies incumbent persisted names.
/// This computes a proposed name only; notices belong after the later commit.
pub fn identity_persisted_name(name: &str, occupied: &[String]) -> String {
    let names: HashSet<String> = occupied
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let available = |candidate: &str| !names.contains(&candidate.trim().to_lowercase());
    if available(name) {
        return name.to_owned();
    }
    let mut suffix = 1usize;
    loop {
        let candidate = format!("{} ({suffix})", name.trim());
        if available(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

/// Optional config-folder seed for a coding agent. When active, AC copies a
/// template config folder (chosen by convention across profile > matrix >
/// coding-agent-base; see `config_seed.rs`) into the replica at spawn. `dest` is
/// the destination folder NAME under `%AC_REPLICA_ROOT%/` (e.g. ".claude"). No
/// `source` field: the template locations are derived by naming convention.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSeedConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub dest: String,
}

impl ConfigSeedConfig {
    pub fn is_active(&self) -> bool {
        self.enabled && !self.dest.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MainSidebarSide {
    Left,
    #[default]
    Right,
}

/// #2348 - saved display state of the unified main window. Deliberately only two
/// states: minimized and fullscreen are unrepresentable, so an odd window state
/// can never be persisted as a placement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MainWindowDisplayState {
    #[default]
    Normal,
    Maximized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelegramPollFailureLogLevel {
    Debug,
    Warn,
    Error,
}

impl TelegramPollFailureLogLevel {
    pub fn as_log_level(self) -> log::Level {
        match self {
            Self::Debug => log::Level::Debug,
            Self::Warn => log::Level::Warn,
            Self::Error => log::Level::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelegramPollRecoveryLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl TelegramPollRecoveryLogLevel {
    pub fn as_log_level(self) -> log::Level {
        match self {
            Self::Debug => log::Level::Debug,
            Self::Info => log::Level::Info,
            Self::Warn => log::Level::Warn,
            Self::Error => log::Level::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TelegramNetworkPollErrorLogging {
    #[serde(default = "default_telegram_network_first_failure_level")]
    pub first_failure_level: TelegramPollFailureLogLevel,
    #[serde(default = "default_telegram_network_transient_repeat_level")]
    pub transient_repeat_level: TelegramPollFailureLogLevel,
    #[serde(default = "default_telegram_network_sustained_level")]
    pub sustained_level: TelegramPollFailureLogLevel,
    #[serde(default = "default_telegram_network_sustained_after_seconds")]
    pub sustained_after_seconds: u64,
    #[serde(default = "default_telegram_network_sustained_repeat_seconds")]
    pub sustained_repeat_seconds: u64,
    #[serde(default = "default_telegram_network_recovery_level")]
    pub recovery_level: TelegramPollRecoveryLogLevel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResourceWatchdogAction {
    Warn,
    KillGroup,
}

impl Default for TelegramNetworkPollErrorLogging {
    fn default() -> Self {
        Self {
            first_failure_level: default_telegram_network_first_failure_level(),
            transient_repeat_level: default_telegram_network_transient_repeat_level(),
            sustained_level: default_telegram_network_sustained_level(),
            sustained_after_seconds: default_telegram_network_sustained_after_seconds(),
            sustained_repeat_seconds: default_telegram_network_sustained_repeat_seconds(),
            recovery_level: default_telegram_network_recovery_level(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub default_shell: String,
    pub default_shell_args: Vec<String>,
    /// Available coding agents
    pub agents: Vec<AgentConfig>,
    /// Reusable coding-agent profile letters, defaults, and per-agent cells.
    #[serde(default)]
    pub coding_agent_profiles: CodingAgentProfilesConfig,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_agent_file_source_preferences"
    )]
    pub agent_file_source_by_menu: Option<AgentFileSourcePreferences>,
    /// Configured Telegram bots for bridge
    #[serde(default)]
    pub telegram_bots: Vec<TelegramBotConfig>,
    /// Controls log severity for transient and sustained Telegram getUpdates
    /// network failures. Non-network poll failures still log at ERROR.
    #[serde(default)]
    pub telegram_network_poll_error_logging: TelegramNetworkPollErrorLogging,
    /// When true, on app start, wake coordinators whose PTY was awake at shutdown.
    /// Coordinators that were asleep at shutdown remain asleep. Non-coordinator
    /// team members are never auto-woken on startup (the user must click their
    /// replica in the sidebar to wake them). Issue #248.
    #[serde(default)]
    pub restore_coordinator_wake_state: bool,
    /// Migration carrier: legacy field name from before issue #248. Read on
    /// deserialization, then translated into `restore_coordinator_wake_state` by
    /// `apply_issue_248_migration`. `skip_serializing_if = "Option::is_none"`
    /// elides it on the next save once the migration has run.
    ///
    /// One-shot migration semantics:
    ///   - legacy `startOnlyCoordinators: true`  → `restoreCoordinatorWakeState: true`
    ///   - legacy `startOnlyCoordinators: false` → `restoreCoordinatorWakeState: false`
    ///
    /// In both cases the legacy value is dropped from disk on next save.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "startOnlyCoordinators"
    )]
    pub legacy_start_only_coordinators: Option<bool>,
    /// (#1793) Typed into an ORCHESTRATOR whose persisted row was working when
    /// the app went down. Empty means inject nothing. No effect unless
    /// `restore_coordinator_wake_state` is on: a deferred orchestrator has no PTY.
    #[serde(default = "default_restart_resume_orchestrator_prompt")]
    pub restart_resume_orchestrator_prompt: String,
    /// (#1793) Typed into a NON-COORDINATOR replica whose persisted row was
    /// working. Empty means inject nothing. No effect unless
    /// `restart_resume_wake_working_agents` is on: a deferred replica has no PTY.
    #[serde(default = "default_restart_resume_agent_prompt")]
    pub restart_resume_agent_prompt: String,
    /// (#1793) Wake a non-coordinator replica whose persisted row was working.
    /// Off by default so the #248 policy (non-coordinators are never auto-woken)
    /// is unchanged unless the user opts in. This wakes; the prompt above types.
    #[serde(default)]
    pub restart_resume_wake_working_agents: bool,
    /// Keep sidebar window always on top
    #[serde(default)]
    pub sidebar_always_on_top: bool,
    /// When true, play a short beep when an entire team transitions from
    /// busy→all-idle. The transition is computed in the FE from `waitingForInput`.
    #[serde(default = "default_true")]
    pub team_idle_beep_enabled: bool,
    /// Master switch for all app-emitted sounds. When false, every current and
    /// future app-generated playback path must stay silent. Per-feature toggles
    /// (e.g. `team_idle_beep_enabled`) act as additional gates underneath this one.
    #[serde(default = "default_true")]
    pub sounds_enabled: bool,
    /// Raise terminal window when sidebar is clicked
    #[serde(default = "default_true")]
    pub raise_terminal_on_click: bool,
    /// #714 Native global hotkey for screenshot capture, e.g. "Ctrl+Q".
    #[serde(default = "default_screenshot_capture_hotkey")]
    pub screenshot_capture_hotkey: String,
    /// #2281 Sidebar compact toggle hotkey, e.g. "Ctrl+Shift+E". Stored
    /// byte-identical to what the user typed; an invalid value blocks the save.
    #[serde(default = "default_sidebar_compact_hotkey")]
    pub sidebar_compact_hotkey: String,
    /// #2507 One `#` per zero-padded digit of a NEWLY created room's number. Default
    /// `"#"` reproduces today's names (`room-1-<team>`); `"##"` gives `room-01-<team>`.
    /// A wider number is never truncated (`"##"` + 100 -> `room-100-<team>`). Existing
    /// rooms are never renamed. An invalid value blocks the save; it is not repaired.
    #[serde(default = "default_room_number_mask")]
    pub room_number_mask: String,
    /// Enable voice-to-text microphone button on session items
    #[serde(default)]
    pub voice_to_text_enabled: bool,
    /// Gemini API key for voice transcription
    #[serde(default)]
    pub gemini_api_key: String,
    /// Gemini model for voice transcription
    #[serde(default = "default_gemini_model")]
    pub gemini_model: String,
    /// #2265 Co-managed: Jev (TypeSafe System One) API key. `""` means not configured.
    #[serde(default)]
    pub jev_api_key: String,
    /// #2265 Co-managed: Jev model. Pinned to the version the 0.70/0.15
    /// thresholds were measured on; a newer model is a deliberate, visible
    /// change that invalidates those numbers.
    #[serde(default = "default_jev_model")]
    pub jev_model: String,
    /// #2265 Co-managed: Jev endpoint.
    #[serde(default = "default_jev_endpoint")]
    pub jev_endpoint: String,
    /// #2265 Co-managed: Jev request timeout, in seconds.
    #[serde(default = "default_jev_timeout_secs")]
    pub jev_timeout_secs: u64,
    /// #2265 Co-managed: judgment threshold.
    #[serde(default = "default_jev_threshold")]
    pub jev_threshold: f32,
    /// #2265 Co-managed: minimum margin between the top two judgments.
    #[serde(default = "default_jev_margin")]
    pub jev_margin: f32,
    /// Global Co-managed switch. **`false` by default: the feature is in
    /// development.** While `false` the feature is off app-wide: it masks every
    /// room `config.json` and hides the menu item. Only turned on by editing
    /// `settings.json` by hand; there is no UI control. Takes effect on restart.
    #[serde(default)]
    pub co_managed_enabled: bool,
    /// Show coding-agent installation actions only after a manual opt-in.
    /// Fresh settings and older files without this key keep the actions hidden.
    #[serde(default)]
    pub coding_agent_install_enabled: bool,
    /// Show container UI options only after a manual settings.json opt-in.
    /// Takes effect after reloading settings or restarting the app.
    #[serde(default)]
    pub container_options_enabled: bool,
    /// Auto-execute (send Enter) after voice transcription
    #[serde(default = "default_true")]
    pub voice_auto_execute: bool,
    /// Delay in seconds before auto-executing after transcription
    #[serde(default = "default_voice_delay")]
    pub voice_auto_execute_delay: u32,
    /// Zoom level for the sidebar window (1.0 = 100%). DEPRECATED in 0.8.0 — see `main_zoom`.
    /// Retained for one version for downgrade safety; seeded into `main_zoom` on first load.
    #[serde(default = "default_zoom")]
    pub sidebar_zoom: f64,
    /// Zoom level for the terminal window (1.0 = 100%). Still used by detached windows in 0.8.0.
    #[serde(default = "default_zoom")]
    pub terminal_zoom: f64,
    /// Zoom level for the unified main window (1.0 = 100%). Introduced in 0.8.0.
    #[serde(default = "default_zoom")]
    pub main_zoom: f64,
    /// Legacy: zoom level for the removed guide window. Kept for backwards-compat reads.
    #[serde(default = "default_zoom")]
    pub guide_zoom: f64,
    /// Legacy: zoom level for the removed dark factory window. Kept for backwards-compat reads.
    #[serde(default = "default_zoom")]
    pub darkfactory_zoom: f64,
    /// DEPRECATED in 0.8.0 — previously held the sidebar window geometry under the
    /// two-window model. `skip_serializing_if` drops it on next save. See `main_geometry`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_geometry: Option<WindowGeometry>,
    /// DEPRECATED in 0.8.0 — previously held the terminal window geometry. Seeded into
    /// `main_geometry` by the first-boot migration. `skip_serializing_if` drops it on next save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_geometry: Option<WindowGeometry>,
    /// Saved geometry for the unified main window. Introduced in 0.8.0.
    #[serde(default)]
    pub main_geometry: Option<WindowGeometry>,
    /// #2348 - saved display state for the unified main window. Defaulted to
    /// `Normal` so a pre-#2348 file loads unchanged. Whole-settings writers
    /// preserve a present on-disk value; only the narrow placement command
    /// (`commands::window::set_main_window_placement`) may change it.
    #[serde(default)]
    pub main_window_display_state: MainWindowDisplayState,
    /// Width of the sidebar pane inside the main window, in logical pixels.
    /// Clamped to [200, 600] at drag time and on load.
    #[serde(default = "default_main_sidebar_width")]
    pub main_sidebar_width: f64,
    /// Side of the unified main window where the sidebar is placed.
    #[serde(default)]
    pub main_sidebar_side: MainSidebarSide,
    /// Keep the unified main window always on top.
    #[serde(default)]
    pub main_always_on_top: bool,
    /// #587 - whether the Resource Monitor occupies the main central pane (vs the
    /// terminal). Restored on startup; default false (terminal).
    #[serde(default)]
    pub main_resource_monitor_attached: bool,
    /// Enable the embedded web server for remote browser access
    #[serde(default)]
    pub web_server_enabled: bool,
    /// Port for the web server
    #[serde(default = "default_web_port")]
    pub web_server_port: u16,
    /// Bind address: "127.0.0.1" (local only) or "0.0.0.0" (all interfaces)
    #[serde(default = "default_web_bind")]
    pub web_server_bind: String,
    /// #791 - enable the in-daemon control-plane API server (Docker/distributed
    /// agents). Default false: no new listening socket unless the operator opts in.
    #[serde(default)]
    pub api_server_enabled: bool,
    /// #791 - control-plane API port. Profile-aware default so dev/prod builds
    /// do not collide on one host.
    #[serde(default = "default_api_port")]
    pub api_server_port: u16,
    /// #791 - control-plane API bind address. Default "127.0.0.1" (safe, loopback).
    /// Reaching a Linux container requires a deliberate operator widening; any
    /// non-loopback bind logs a loud startup warning.
    #[serde(default = "default_api_bind")]
    pub api_server_bind: String,
    /// #1173 - disclosure gate for authorized backend terminal snapshots.
    /// Whole-settings writers preserve an explicit on-disk value; a legacy
    /// settings object whose key is absent is materialized as true (the
    /// default-on phase deliberately reversed the absent-key rule), and a
    /// malformed value fails closed. Only the dedicated compare-and-set
    /// command may change an explicit value.
    #[serde(default = "default_terminal_snapshots_enabled")]
    pub terminal_snapshots_enabled: bool,
    /// Currently loaded project path (legacy single-project, kept for backward compat)
    #[serde(default)]
    pub project_path: Option<String>,
    /// Currently loaded project paths (multi-project support)
    #[serde(default)]
    pub project_paths: Vec<String>,
    /// #881 - projects hidden from the sidebar but still registered. Disk-authoritative
    /// under the same Design S rules as `project_paths`: the default `save_settings`
    /// preserves the on-disk copy and only dedicated list commands mutate it.
    #[serde(default)]
    pub archived_project_paths: Vec<String>,
    /// #1077 - hidden persistence state for the portable dual project paths. Holds
    /// the resolved raw/selected pairs, their instance-relative companions,
    /// outcomes, and reconcile-eligibility bits so the codec can rebuild the six
    /// disk fields without re-reading disk. Never serialized directly (the codec
    /// owns the six on-disk fields); behind an `Arc` so the ubiquitous
    /// `AppSettings::clone()` stays cheap and copy-on-write mutation is explicit.
    #[serde(skip, default)]
    pub(crate) project_path_state: Arc<crate::config::projects::ProjectPathPersistenceState>,
    /// An unavailable source must never become an empty retention list.
    #[serde(skip, default)]
    pub(crate) project_paths_persistence_error: Option<String>,
    /// #1737 - the `settings.local.json` overlay in force for this load, and the base
    /// values it displaces. Never serialized; behind an `Arc` so `AppSettings::clone()`
    /// stays cheap. Like `project_path_state`, a value that arrives from the renderer
    /// or the WebSocket transport carries `Arc::default()` here, so every whole-object
    /// writer must restore it from live memory first (plan D14).
    #[serde(skip, default)]
    pub(crate) local_overlay_state: Arc<LocalSettingsOverlay>,
    /// #2716 (B3, option C) - whether `agents.30.instance.no-git.json` could be read.
    /// Never serialized, for the same D14 reason as `local_overlay_state`: a client
    /// draft can never carry or forge it. Set only through `set_agents_layer_state`.
    #[serde(skip, default)]
    pub(crate) agents_layer: AgentsLayerState,
    /// Backend physical identity inputs captured before typed repair/defaulting.
    #[serde(skip, default)]
    pub(crate) coding_agent_identity_physical: Option<Arc<Value>>,
    /// Sidebar visual style: "noir-minimal", "card-sections", "command-center", "deep-space",
    /// "arctic-ops", "obsidian-mesh", "neon-circuit"
    #[serde(default = "default_sidebar_style")]
    pub sidebar_style: String,
    /// #1796 - selected-row rail width. A whole number from 1 to 14, optionally
    /// suffixed `px`: `9px` and `12` are both accepted; `0`, `15`, `2em` and
    /// `40px` are not. The bound is `RAIL_WIDTH_MAX` in
    /// `src/sidebar/selected-row-rail.ts`. Rust does not validate this field. That
    /// one predicate runs at startup and on every keystroke in the settings modal,
    /// which is the only place it shows a warning; an unusable value is never
    /// published to the DOM and the stylesheet falls back to its `:root` default.
    #[serde(default = "default_selected_row_rail_width")]
    pub selected_row_rail_width: String,
    /// #1796 - selected-row rail colour, `#rrggbb` exactly; the 3- and 8-digit hex
    /// forms are not accepted. Same soft-warning contract as
    /// `selected_row_rail_width`.
    #[serde(default = "default_selected_row_rail_color")]
    pub selected_row_rail_color: String,
    /// Root token that bypasses all routing checks in the send command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_token: Option<String>,
    /// Whether the user has dismissed the first-run onboarding wizard
    #[serde(default)]
    pub onboarding_dismissed: bool,
    /// When true, sort the Coordinator Quick-Access list by most-recent-activity descending.
    /// Activity = busy→idle transition (IdleDetector emits session_idle).
    /// Per-session timestamps live in the frontend store and are NOT persisted.
    #[serde(default)]
    pub coord_sort_by_activity: bool,
    #[serde(default = "default_true")]
    pub always_show_selected_workgroup: bool,
    /// Optional logger filter expression. Applied at startup if `RUST_LOG` is unset.
    /// Uses standard `env_logger` filter syntax
    /// (e.g. `info,agentscommander_lib::config::teams=trace`).
    /// Phase 1 of #93 — settings-level control with `RUST_LOG` env override (backwards-compat).
    /// Phase 2 (UI dropdown) and Phase 3 (live reload) are deferred per the issue.
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub activity_log_enabled: bool,
    /// When true, on Coordinator session spawn AC injects a prompt asking the
    /// agent to add a YAML frontmatter `title:` line to its workgroup
    /// `TASK.md` (only if the brief is non-empty and has no `title:` yet).
    /// See plan `_plans/107-auto-brief-title.md`.
    #[serde(default = "default_true")]
    pub auto_generate_task_title: bool,
    /// Optional override for the local agent-templates directory used by the
    /// New Agent role-template picker (#271). Empty/missing ⇒ default
    /// `<config_dir>/agent-templates/`. Relative ⇒ resolved against
    /// `<config_dir>/`. Absolute ⇒ used as-is.
    #[serde(default)]
    pub agent_templates_path: Option<String>,
    /// When true, the app uses the light theme; when false, the dark theme.
    /// Defaults to `false` so fresh and missing values use dark mode.
    #[serde(default)]
    pub theme_light: bool,
    /// #965 - rail project sections the user has explicitly collapsed by clicking
    /// their header. This is the RAIL's own state; it is deliberately NOT the
    /// ProjectPanel's collapse (which stays session-only and auto-focus driven).
    /// Entries are FRONTEND-NORMALIZED project paths (lowercase, forward slashes,
    /// no trailing slash: `normalizeProjectPathForCompare` in
    /// `src/sidebar/stores/project-refresh.ts`).
    /// WRITTEN ONLY BY `set_rail_collapse`. A whole-object settings payload from the
    /// GUI/CLI/API carries no authority for this field: both whole-object writers
    /// restore it from live memory (see `build_protected_settings_candidate`).
    #[serde(default)]
    pub rail_collapsed_projects: Vec<String>,
    /// #965 - collapsed state of the rail's cross-project `Favorites` section.
    /// Belongs to no project, which is why collapse cannot live in project-settings.json.
    /// Same protection as `rail_collapsed_projects`.
    #[serde(default)]
    pub rail_favorites_collapsed: bool,
    /// When true, show the Spec Board toolbar button. Defaults off because the
    /// board is an opt-in feature enabled manually from settings.json.
    #[serde(default)]
    pub spec_board_enabled: bool,
    /// #1298 - how many repositories the global git sweeper detects at once.
    /// Default 1, i.e. strictly sequential, which is the property the sweeper
    /// exists for. Read once per round and clamped to 1..=4 at the read site, which
    /// keeps the app from rewriting the user's hand-edited settings.json; changing
    /// it needs a RESTART, since the sweeper reads the in-memory settings state and
    /// nothing reloads that from disk. Raising it to 2 halves worst-case
    /// head-of-line blocking (see INV-1) at the cost of reintroducing bounded
    /// concurrency. No UI: same manual-opt-in shape as `spec_board_enabled`;
    /// documented in docs/reference/settings.md.
    #[serde(default = "default_git_sweep_concurrency")]
    pub git_sweep_concurrency: u8,
    /// #1298 - lower bound, in seconds, on one sweeper round. GUARD, not an
    /// optimization: with an empty work list a round takes ~0ms and an unfloored
    /// loop spins a core. Clamped to 1..=3600 at the read site; 0 is rejected by the
    /// clamp for that reason. The effective period is max(this, round duration), so
    /// on a large workgroup set the round duration dominates and this never fires.
    #[serde(default = "default_git_sweep_min_interval_secs")]
    pub git_sweep_min_interval_secs: u64,
    /// #2064 - whether the remote activity sweeper asks GitHub whether CI is
    /// running on each room repo's exact HEAD. Default on. Read at startup, to
    /// decide whether the sweeper thread exists at all, and once per round
    /// otherwise. Changing it needs a RESTART, because nothing reloads the
    /// in-memory settings state from disk. No UI: hand-edited settings.json.
    #[serde(default = "default_ci_activity_enabled")]
    pub ci_activity_enabled: bool,
    /// #2064 - whether a CI state transition may inject a notice into the room's
    /// orchestrator (Phase B). Subordinate to `ci_activity_enabled`: colour
    /// without interruption must be expressible, because the chip is passive and
    /// an injected notice consumes an agent's context mid-task. Read once per
    /// round; changing it needs a RESTART. No UI.
    #[serde(default = "default_ci_activity_notify_orchestrator")]
    pub ci_activity_notify_orchestrator: bool,
    /// #2064 - whether the sweeper asks whether the repository's default branch
    /// contains commits this checkout does not. A SIBLING of
    /// `ci_activity_enabled`, not a child: two questions, two cadences, two
    /// failure modes, and a user who wants one without the other. Read once per
    /// round; changing it needs a RESTART. No UI.
    #[serde(default = "default_branch_staleness_enabled")]
    pub branch_staleness_enabled: bool,
    /// #2064 - whether a `BranchStale` transition may inject a notice into the
    /// room's orchestrator (Phase B). Subordinate to
    /// `branch_staleness_enabled`, for the same context-budget reason as
    /// `ci_activity_notify_orchestrator`. Read once per round; changing it needs
    /// a RESTART. No UI.
    #[serde(default = "default_branch_staleness_notify_orchestrator")]
    pub branch_staleness_notify_orchestrator: bool,
    /// #2064 - seconds between CI questions for a key that is not currently
    /// running. Read once per round and clamped to 10..=3600 at the read site,
    /// which keeps the app from rewriting the user's hand-edited settings.json;
    /// changing it needs a RESTART. Keys whose last confirmed state is `Running`
    /// use the fixed 10s cadence instead: `Finished` is the transition that
    /// actually unblocks an agent. No UI.
    #[serde(default = "default_ci_sweep_min_interval_secs")]
    pub ci_sweep_min_interval_secs: u64,
    /// #2064 - seconds between staleness questions. Read once per round and
    /// clamped to 10..=3600 at the read site; changing it needs a RESTART. No UI.
    #[serde(default = "default_branch_staleness_interval_secs")]
    pub branch_staleness_interval_secs: u64,
    #[serde(default = "default_resource_monitor_enabled")]
    pub resource_monitor_enabled: bool,
    #[serde(default = "default_max_concurrent_agent_processes")]
    pub max_concurrent_agent_processes: u32,
    #[serde(default = "default_resource_watchdog_action")]
    pub resource_watchdog_action: ResourceWatchdogAction,
    #[serde(default = "default_agent_group_warn_private_bytes")]
    pub agent_group_warn_private_bytes: u64,
    #[serde(default = "default_agent_group_kill_private_bytes")]
    pub agent_group_kill_private_bytes: u64,
    #[serde(default = "default_agent_process_kill_private_bytes")]
    pub agent_process_kill_private_bytes: u64,
    #[serde(default = "default_resource_keep_last_snapshot")]
    pub resource_keep_last_snapshot: bool,
    #[serde(default = "default_resource_backoff_polling")]
    pub resource_backoff_polling: bool,
    /// #552 badge color thresholds (minutes). green < yellow <= value < red <= value.
    #[serde(default = "default_coord_badge_yellow_minutes")]
    pub coordinator_idle_badge_yellow_minutes: u32,
    #[serde(default = "default_coord_badge_red_minutes")]
    pub coordinator_idle_badge_red_minutes: u32,
    /// #552 auto-close lifecycle clock.
    #[serde(default = "default_true")]
    pub coordinator_auto_close_enabled: bool,
    #[serde(default = "default_coord_auto_close_minutes")]
    pub coordinator_auto_close_minutes: u32,
    /// #817 When true, auto-close skips sessions with a persisted Telegram bot
    /// assignment. Default false preserves legacy auto-close behavior.
    #[serde(default)]
    pub coordinator_auto_close_skip_telegram_assigned: bool,
    /// #588 When true, manually closing a coordinator also closes its team
    /// agents (cascade). When false, only the coordinator closes. Default true.
    #[serde(default = "default_true")]
    pub coordinator_cascade_close_enabled: bool,
    /// #609 When true, check npm on startup (<=1x/24h) and notify in-app when
    /// a newer published version is available. Default true.
    #[serde(default = "default_true")]
    pub npm_update_notifications_enabled: bool,
    /// #1925 When true, download the remote blocking-menu patterns on startup
    /// (<=1x/24h); a downloaded file applies at the next start. Default true.
    #[serde(default = "default_true")]
    pub remote_blocking_menus_enabled: bool,
    /// #2133 When true, download the remote per-agent help on startup
    /// (<=1x/24h); a downloaded file applies at the next start. Default true.
    #[serde(default = "default_true")]
    pub remote_agent_help_enabled: bool,
    /// #640 Global master for auto self-handoff-and-clear. Absolute kill switch:
    /// false => off for every agent. When true, the class-aware default applies
    /// (ON for coordinator/Root, OFF for specialists), subject to per-agent
    /// overrides in `auto_self_clear_by_agent`.
    #[serde(default = "default_true")]
    pub auto_self_clear_enabled: bool,
    /// #640 Per-agent override of the class default, keyed by agent name (same
    /// key as `coding_agent_profiles.default_profile_by_agent`). Applies only
    /// while the global master is on; absent = use the class default.
    #[serde(default)]
    pub auto_self_clear_by_agent: std::collections::BTreeMap<String, bool>,
    /// #1327 - per-command auto-update policy for the startup update run, keyed by
    /// the catalog COMMAND string (not the agent id: several profiles can share one
    /// command; the software is the update unit). Absent = never asked (the startup
    /// dialog asks once, default No); present = last answer (true = run this
    /// command's updateCommands at startup, false = never ask again). Replaces the
    /// two inert #1318 agent-id-keyed maps. Read and
    /// written by the #1327 startup update flow only.
    #[serde(default)]
    pub agent_auto_update_by_command: std::collections::BTreeMap<String, bool>,
    /// #930 - when true (default), container coding-agent sessions copy the host
    /// user's credential file for that agent into the replica config dir at spawn
    /// and delete it on teardown. When false, the user supplies credentials
    /// themselves (e.g. a CLAUDE_CODE_OAUTH_TOKEN env row).
    #[serde(default = "default_true")]
    pub container_credentials_from_host: bool,
    /// #1171 - root-level watcher patterns, keyed by watcher id.
    ///
    /// Root-level and not a field on `AgentConfig`, so the 20-plus struct-construction sites
    /// that already had to write `context_regex: None` are untouched, AND so a pattern can
    /// apply to every agent - which is exactly what the per-agent shape cannot express. Same
    /// shape as `auto_self_clear_by_agent` (`:530-531`). `BTreeMap` for a stable on-disk
    /// order and clean diffs, and because the 8-watcher budget resolves in key order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub watchers: BTreeMap<String, WatcherEntry>,
    /// #1171 - geometry of the watcher activity window.
    ///
    /// `skip_serializing_if` and deliberately NOT a copy of `main_geometry` (`:367-369`),
    /// which lacks it: without the skip, `"watchersGeometry": null` would appear in every
    /// user's file on the next save, so configuring nothing would still leave a trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watchers_geometry: Option<WindowGeometry>,
    /// #2482 - where each agent's weekly (7-day) quota reading comes from, keyed by
    /// agent id. Root-level, not a field on `AgentConfig`, for the reason `watchers`
    /// is: the 34 literal `AgentConfig` construction sites stay untouched. Absent or
    /// empty = off for every agent: no reading, no compile, no event.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quota_sources: BTreeMap<String, QuotaSourceEntry>,
    /// #1646 / #1647 - master kill switch for proactive detection of terminal blocking menus.
    #[serde(default = "default_true")]
    pub menu_guard_enabled: bool,
    /// #2336 - seconds the automatic typing hold stays active after the last
    /// qualifying keystroke in a session. Default 30; accepted range 1..=3600,
    /// rejected outside it by `validate_and_repair_settings` so a Settings update
    /// cannot persist a value that would make the hold nonsensical. Read live at
    /// each hold evaluation, so an edit takes effect without a restart.
    #[serde(default = "default_typing_hold_seconds")]
    pub typing_hold_seconds: u32,
    /// User preference; runtime readiness is controlled separately.
    #[serde(default = "default_true")]
    pub response_close_enabled: bool,
    /// Continuous safe idle required after an eligible response, in seconds.
    #[serde(default = "default_response_close_idle_seconds")]
    pub response_close_idle_seconds: u32,
    /// Continuous room inactivity required before confirmation, in seconds.
    #[serde(default = "default_room_idle_confirmation_seconds")]
    pub room_idle_confirmation_seconds: u32,
}

/// #1171 - one entry of the root `watchers` map, or whatever the user wrote there.
///
/// **This wrapper is what stops a malformed watcher from destroying the settings file.**
/// `parse_settings_json` (`:870-871`) deserializes `AppSettings` in ONE shot and
/// `load_settings_from_path` (`:1661-1664`) replaces any failure with
/// `AppSettings::default()`, leaving one log line. A hand-written `"mode": "State"`,
/// `"commands": "claude"` or `"dedupeWindowMs": "2000"` would therefore start
/// AgentsCommander with NO AGENTS CONFIGURED, and every later save would be refused by the
/// #1077 write gate (`read_disk_object_for_write`, `:2565-2591`). With the wrapper the
/// consequence is one skipped watcher.
///
/// `untagged` tries `Valid` first, so `Invalid` only ever catches what `WatcherConfig`
/// rejected. The value is kept verbatim so a save round-trips the user's bytes instead of
/// deleting what it could not read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WatcherEntry {
    Valid(WatcherConfig),
    /// Anything that did not deserialize as a `WatcherConfig`. Skipped by resolution and
    /// logged once per changed value.
    Invalid(serde_json::Value),
}

impl WatcherEntry {
    pub fn valid(&self) -> Option<&WatcherConfig> {
        match self {
            WatcherEntry::Valid(config) => Some(config),
            WatcherEntry::Invalid(_) => None,
        }
    }
}

/// #1171 - one user-configured watcher.
///
/// `mode` and `pattern` stay REQUIRED. A watcher without either is not a watcher, and with
/// the wrapper above the consequence of omitting one is a single skipped entry rather than a
/// lost configuration. A defaulted `mode` would silently run a watcher the user never
/// described.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatcherConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub mode: WatcherMode,
    pub pattern: String,
    /// Absent or null: reaches every configured agent. Present: only entries whose `command`
    /// executable stem matches EXACTLY. Present and empty: reaches none.
    ///
    /// `Option` and not `#[serde(default)] Vec`, because absent and `[]` are opposites here
    /// and only `Option` lets serde tell them apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    #[serde(default)]
    pub dedupe: WatcherDedupe,
    #[serde(default = "default_dedupe_window_ms")]
    pub dedupe_window_ms: u64,
    /// Free text, e.g. "claude 2.1.212". Never validated, never parsed. It exists because
    /// `context_scrape/rows.rs:183-186` documents that a TUI format already had to be
    /// re-captured once, and that fact currently lives only in a Rust comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_against: Option<String>,
}

/// #1171 - what a match means.
///
/// `state` is a reading: idempotent, gated, taken over the whole frame. `occurrence` is an
/// event: every match the frame diff declares evaluable counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatcherMode {
    State,
    Occurrence,
}

/// #1171 - what makes two `occurrence` matches "the same one" inside the dedupe window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatcherDedupe {
    /// The matched logical row text.
    #[default]
    Row,
    /// The joined capture groups: two rows truncated differently that capture the same path
    /// are one event.
    Capture,
    /// Every match counts.
    None,
}

/// #2482 - one entry of the root `quotaSources` map, or whatever the user wrote
/// there. Same wrapper, same reason, as `WatcherEntry`: settings deserialize in
/// one shot and any failure is replaced by `AppSettings::default()`, so a
/// hand-written or newer-AC entry must cost one skipped source and never the whole
/// file. `untagged` tries `Valid` first, so `Invalid` only catches what
/// `QuotaSourceConfig` rejected, and its bytes are kept verbatim so a save
/// round-trips what the user wrote.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QuotaSourceEntry {
    Valid(QuotaSourceConfig),
    Invalid(serde_json::Value),
}

impl QuotaSourceEntry {
    pub fn valid(&self) -> Option<&QuotaSourceConfig> {
        match self {
            QuotaSourceEntry::Valid(config) => Some(config),
            QuotaSourceEntry::Invalid(_) => None,
        }
    }
}

/// #2482 - one configured quota source. **The multi-agent extension point.**
/// `kind` is the discriminant: adding Codex, Gemini or any other agent is one new
/// variant here plus one match arm in `pty::agent_quota::source`. Nothing else in
/// the engine changes, and an unrecognized `kind` from a newer AC lands in
/// `QuotaSourceEntry::Invalid` rather than destroying the file. Capture group 1 of
/// `ScreenRegex` is the USED percentage; group 1 of `ScreenRegexRemaining` is the
/// REMAINING one, which the engine converts to used.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QuotaSourceConfig {
    /// Read the percentage from what the agent draws in its terminal, with the
    /// screen-row scrape #1032 uses for the context badge. Capture group 1 is the
    /// USED percentage of the 7-day window.
    ScreenRegex {
        pattern: String,
        #[serde(default = "default_true")]
        enabled: bool,
    },
    /// #2686 - the same screen-row scrape, for an agent that draws what is LEFT of the
    /// window. Capture group 1 is the REMAINING percentage.
    ScreenRegexRemaining {
        pattern: String,
        #[serde(default = "default_true")]
        enabled: bool,
    },
}

fn default_dedupe_window_ms() -> u64 {
    2000
}

fn default_true() -> bool {
    true
}

fn default_screenshot_capture_hotkey() -> String {
    "Ctrl+Q".to_string()
}

fn default_sidebar_compact_hotkey() -> String {
    "Ctrl+Shift+E".to_string()
}

pub(crate) fn default_room_number_mask() -> String {
    "#".to_string()
}

/// #640 Resolve the effective auto-self-clear flag for an agent.
/// Precedence: the global master `auto_self_clear_enabled` is an absolute kill
/// switch (off => off for all); else an explicit per-agent override
/// (`auto_self_clear_by_agent[name]`) wins; else the class-aware default
/// (`default_on` = true for coordinator/Root, false for specialists), computed
/// at the call site. Master-first so one toggle reliably disables everything.
pub fn resolve_auto_self_clear(settings: &AppSettings, agent_name: &str, default_on: bool) -> bool {
    if !settings.auto_self_clear_enabled {
        return false; // global master kill switch wins
    }
    if let Some(explicit) = settings.auto_self_clear_by_agent.get(agent_name) {
        return *explicit; // explicit per-agent override wins while enabled
    }
    default_on // class-aware default
}

fn default_profiles_schema_version() -> u32 {
    2
}

fn default_profile_slots() -> BTreeMap<String, ProfileSlotConfig> {
    BTreeMap::from([(
        "A".to_string(),
        ProfileSlotConfig {
            label: String::new(),
        },
    )])
}

fn default_telegram_network_first_failure_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Warn
}

fn default_telegram_network_transient_repeat_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Debug
}

fn default_telegram_network_sustained_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Error
}

fn default_telegram_network_sustained_after_seconds() -> u64 {
    60
}

fn default_telegram_network_sustained_repeat_seconds() -> u64 {
    60
}

fn default_telegram_network_recovery_level() -> TelegramPollRecoveryLogLevel {
    TelegramPollRecoveryLogLevel::Info
}

fn default_gemini_model() -> String {
    "gemini-2.5-flash".to_string()
}

fn default_jev_model() -> String {
    "jev-1.13.0".to_string()
}

fn default_jev_endpoint() -> String {
    "https://api.typesafe.ai/v1/systemone".to_string()
}

fn default_jev_timeout_secs() -> u64 {
    20
}

fn default_jev_threshold() -> f32 {
    0.70
}

fn default_jev_margin() -> f32 {
    0.15
}

fn default_restart_resume_orchestrator_prompt() -> String {
    "AgentsCommander was restarted. Continue with the work that was in flight.".to_string()
}

fn default_restart_resume_agent_prompt() -> String {
    ".".to_string()
}

fn default_voice_delay() -> u32 {
    15
}

fn default_zoom() -> f64 {
    1.0
}

fn default_web_port() -> u16 {
    super::profile::web_server_port()
}

fn default_web_bind() -> String {
    "127.0.0.1".to_string()
}

/// #791 - profile-aware default control-plane API port (delegates to
/// `profile::api_server_port`, mirroring `default_web_port`).
fn default_api_port() -> u16 {
    super::profile::api_server_port()
}

/// #791 - default control-plane API bind: loopback, safe-by-default. Mirrors
/// `default_web_bind`. Widening for Docker is a deliberate operator action.
fn default_api_bind() -> String {
    "127.0.0.1".to_string()
}

fn default_sidebar_style() -> String {
    "noir-minimal".to_string()
}

fn default_selected_row_rail_width() -> String {
    "9px".to_string()
}

fn default_selected_row_rail_color() -> String {
    "#FFFFFF".to_string()
}

fn default_main_sidebar_width() -> f64 {
    240.0
}

fn default_git_sweep_concurrency() -> u8 {
    1
}

fn default_git_sweep_min_interval_secs() -> u64 {
    10
}

fn default_ci_activity_enabled() -> bool {
    true
}

fn default_ci_activity_notify_orchestrator() -> bool {
    true
}

fn default_branch_staleness_enabled() -> bool {
    true
}

fn default_branch_staleness_notify_orchestrator() -> bool {
    true
}

fn default_ci_sweep_min_interval_secs() -> u64 {
    30
}

fn default_branch_staleness_interval_secs() -> u64 {
    260
}

fn default_resource_monitor_enabled() -> bool {
    true
}

fn default_terminal_snapshots_enabled() -> bool {
    true
}

fn default_max_concurrent_agent_processes() -> u32 {
    32
}

fn default_coord_badge_yellow_minutes() -> u32 {
    30
}

fn default_coord_badge_red_minutes() -> u32 {
    60
}

fn default_coord_auto_close_minutes() -> u32 {
    60
}

fn default_resource_watchdog_action() -> ResourceWatchdogAction {
    ResourceWatchdogAction::Warn
}

fn default_agent_group_warn_private_bytes() -> u64 {
    8 * 1024 * 1024 * 1024
}

fn default_agent_group_kill_private_bytes() -> u64 {
    12 * 1024 * 1024 * 1024
}

fn default_agent_process_kill_private_bytes() -> u64 {
    12 * 1024 * 1024 * 1024
}

/// #2336 - default `typing_hold_seconds`.
pub const DEFAULT_TYPING_HOLD_SECONDS: u32 = 30;
/// #2336 - accepted `typing_hold_seconds` bounds; outside them the Settings API
/// rejects the update and the live value is left unchanged.
pub const TYPING_HOLD_SECONDS_MIN: u32 = 1;
pub const TYPING_HOLD_SECONDS_MAX: u32 = 3600;

fn default_typing_hold_seconds() -> u32 {
    DEFAULT_TYPING_HOLD_SECONDS
}

const DEFAULT_RESPONSE_CLOSE_IDLE_SECONDS: u32 = 30;
const RESPONSE_CLOSE_IDLE_SECONDS_MIN: u32 = 1;
const RESPONSE_CLOSE_IDLE_SECONDS_MAX: u32 = 3600;

fn default_response_close_idle_seconds() -> u32 {
    DEFAULT_RESPONSE_CLOSE_IDLE_SECONDS
}

/// Disk repair changes only the effective value, without requesting a write.
fn repair_loaded_response_close_settings(settings: &mut AppSettings, source: &str) {
    if !(RESPONSE_CLOSE_IDLE_SECONDS_MIN..=RESPONSE_CLOSE_IDLE_SECONDS_MAX)
        .contains(&settings.response_close_idle_seconds)
    {
        log::warn!(
            "Invalid responseCloseIdleSeconds={} from {source}: expected 1..=3600; using 30 in \
                memory",
            settings.response_close_idle_seconds
        );
        settings.response_close_idle_seconds = DEFAULT_RESPONSE_CLOSE_IDLE_SECONDS;
    }
}

const DEFAULT_ROOM_IDLE_CONFIRMATION_SECONDS: u32 = 15;
const ROOM_IDLE_CONFIRMATION_SECONDS_MIN: u32 = 1;
const ROOM_IDLE_CONFIRMATION_SECONDS_MAX: u32 = 3600;

fn default_room_idle_confirmation_seconds() -> u32 {
    DEFAULT_ROOM_IDLE_CONFIRMATION_SECONDS
}

/// Disk repair changes only the effective value, without requesting a write.
fn repair_loaded_room_idle_confirmation_settings(settings: &mut AppSettings, source: &str) {
    if !(ROOM_IDLE_CONFIRMATION_SECONDS_MIN..=ROOM_IDLE_CONFIRMATION_SECONDS_MAX)
        .contains(&settings.room_idle_confirmation_seconds)
    {
        log::warn!(
            "Invalid roomIdleConfirmationSeconds={} from {source}: expected 1..=3600; using 15 in \
                memory",
            settings.room_idle_confirmation_seconds
        );
        settings.room_idle_confirmation_seconds = DEFAULT_ROOM_IDLE_CONFIRMATION_SECONDS;
    }
}

fn default_resource_keep_last_snapshot() -> bool {
    true
}

fn default_resource_backoff_polling() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        let (default_shell, default_shell_args) = if cfg!(target_os = "windows") {
            ("powershell.exe".to_string(), vec!["-NoLogo".to_string()])
        } else {
            ("/bin/bash".to_string(), vec![])
        };

        Self {
            default_shell,
            default_shell_args,
            agents: vec![],
            coding_agent_profiles: CodingAgentProfilesConfig::default(),
            agent_file_source_by_menu: None,
            telegram_bots: vec![],
            telegram_network_poll_error_logging: TelegramNetworkPollErrorLogging::default(),
            restore_coordinator_wake_state: true,
            legacy_start_only_coordinators: None,
            restart_resume_orchestrator_prompt: default_restart_resume_orchestrator_prompt(),
            restart_resume_agent_prompt: default_restart_resume_agent_prompt(),
            restart_resume_wake_working_agents: true,
            sidebar_always_on_top: false,
            team_idle_beep_enabled: true,
            sounds_enabled: true,
            raise_terminal_on_click: true,
            screenshot_capture_hotkey: default_screenshot_capture_hotkey(),
            sidebar_compact_hotkey: default_sidebar_compact_hotkey(),
            room_number_mask: default_room_number_mask(),
            voice_to_text_enabled: false,
            gemini_api_key: String::new(),
            gemini_model: default_gemini_model(),
            jev_api_key: String::new(),
            jev_model: default_jev_model(),
            jev_endpoint: default_jev_endpoint(),
            jev_timeout_secs: default_jev_timeout_secs(),
            jev_threshold: default_jev_threshold(),
            jev_margin: default_jev_margin(),
            co_managed_enabled: false,
            coding_agent_install_enabled: false,
            container_options_enabled: false,
            voice_auto_execute: true,
            voice_auto_execute_delay: default_voice_delay(),
            sidebar_zoom: default_zoom(),
            terminal_zoom: default_zoom(),
            main_zoom: default_zoom(),
            guide_zoom: default_zoom(),
            darkfactory_zoom: default_zoom(),
            sidebar_geometry: None,
            terminal_geometry: None,
            main_geometry: None,
            main_window_display_state: MainWindowDisplayState::default(),
            main_sidebar_width: default_main_sidebar_width(),
            main_sidebar_side: MainSidebarSide::default(),
            main_always_on_top: false,
            main_resource_monitor_attached: false,
            web_server_enabled: false,
            web_server_port: default_web_port(),
            web_server_bind: default_web_bind(),
            api_server_enabled: false,
            api_server_port: default_api_port(),
            api_server_bind: default_api_bind(),
            terminal_snapshots_enabled: true,
            project_path: None,
            project_paths: vec![],
            archived_project_paths: vec![],
            project_path_state: Arc::default(),
            project_paths_persistence_error: None,
            local_overlay_state: Arc::default(),
            agents_layer: AgentsLayerState::default(),
            coding_agent_identity_physical: None,
            sidebar_style: default_sidebar_style(),
            selected_row_rail_width: default_selected_row_rail_width(),
            selected_row_rail_color: default_selected_row_rail_color(),
            root_token: None,
            onboarding_dismissed: false,
            coord_sort_by_activity: false,
            always_show_selected_workgroup: true,
            log_level: None,
            activity_log_enabled: false,
            auto_generate_task_title: true,
            agent_templates_path: None,
            theme_light: false,
            rail_collapsed_projects: Vec::new(),
            rail_favorites_collapsed: false,
            spec_board_enabled: false,
            git_sweep_concurrency: default_git_sweep_concurrency(),
            git_sweep_min_interval_secs: default_git_sweep_min_interval_secs(),
            ci_activity_enabled: default_ci_activity_enabled(),
            ci_activity_notify_orchestrator: default_ci_activity_notify_orchestrator(),
            branch_staleness_enabled: default_branch_staleness_enabled(),
            branch_staleness_notify_orchestrator: default_branch_staleness_notify_orchestrator(),
            ci_sweep_min_interval_secs: default_ci_sweep_min_interval_secs(),
            branch_staleness_interval_secs: default_branch_staleness_interval_secs(),
            resource_monitor_enabled: default_resource_monitor_enabled(),
            max_concurrent_agent_processes: default_max_concurrent_agent_processes(),
            resource_watchdog_action: default_resource_watchdog_action(),
            agent_group_warn_private_bytes: default_agent_group_warn_private_bytes(),
            agent_group_kill_private_bytes: default_agent_group_kill_private_bytes(),
            agent_process_kill_private_bytes: default_agent_process_kill_private_bytes(),
            resource_keep_last_snapshot: default_resource_keep_last_snapshot(),
            resource_backoff_polling: default_resource_backoff_polling(),
            coordinator_idle_badge_yellow_minutes: default_coord_badge_yellow_minutes(),
            coordinator_idle_badge_red_minutes: default_coord_badge_red_minutes(),
            coordinator_auto_close_enabled: true,
            coordinator_auto_close_minutes: default_coord_auto_close_minutes(),
            coordinator_auto_close_skip_telegram_assigned: false,
            coordinator_cascade_close_enabled: true,
            npm_update_notifications_enabled: true,
            remote_blocking_menus_enabled: true,
            remote_agent_help_enabled: true,
            auto_self_clear_enabled: true,
            auto_self_clear_by_agent: std::collections::BTreeMap::new(),
            agent_auto_update_by_command: std::collections::BTreeMap::new(),
            container_credentials_from_host: true,
            watchers: BTreeMap::new(),
            watchers_geometry: None,
            quota_sources: BTreeMap::new(),
            menu_guard_enabled: true,
            typing_hold_seconds: default_typing_hold_seconds(),
            response_close_enabled: true,
            response_close_idle_seconds: default_response_close_idle_seconds(),
            room_idle_confirmation_seconds: default_room_idle_confirmation_seconds(),
        }
    }
}

/// #1757 - the Codex "Hooks need review" startup dialog. One literal, two consumers:
/// `apply_issue_1757_migration` (installs whose `blockingMenus` array was already materialized
/// against the older default) and test T1 `embedded_content_matches_the_rust_literals`, which
/// pins the embedded `settings-blocking-menus.json` to this constant so the two cannot drift.
pub(crate) const CODEX_HOOKS_REVIEW_PATTERN: &str = r"^[^A-Za-z0-9]*Hooks need review\b";

fn codex_hooks_review_menu() -> BlockingMenuConfig {
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
const EMBEDDED_BLOCKING_MENUS_JSON: &str =
    include_str!("../../resources/blocking-menus/settings-blocking-menus.json");

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
const REMOTE_BLOCKING_MENUS_MAX_ENTRIES: usize = 200;
const REMOTE_PATTERN_MAX_BYTES: usize = 512;
const REMOTE_NOTIFICATION_MAX_BYTES: usize = 200;

/// #1925 (D6) - compile ceiling for one downloaded pattern. The menu guard keeps one
/// compiled copy per accepted pattern, so an accepted file adds at most
/// 200 x 256 KiB = 50 MiB of compiled programs, plus each regex's lazy-DFA cache at
/// the `regex` crate default. Reaching that ceiling needs repo-write access; the real
/// published file compiles to a few KiB.
const REMOTE_PATTERN_REGEX_SIZE_LIMIT: usize = 256 * 1024;

/// #1925 (D7) - every accepted pattern must leave these rows alone. Matching is
/// `Regex::is_match`, unanchored, the way the guard matches, so a catch-all pattern
/// that would silently defer every injection is rejected before it reaches the guard.
/// Menu-like rows (`1. Yes`) are deliberately absent: a legitimate trust-menu pattern
/// may match them.
const REMOTE_BLOCKING_MENUS_BENIGN_ROWS: &[&str] = &[
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

fn default_blocking_menus_schema_version() -> u32 {
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
struct RemoteBlockingMenusCheckStamp {
    last_checked_at: chrono::DateTime<chrono::Utc>,
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
    shipped: &'static BlockingMenusFile,
    remote: BlockingMenusFile,
    local: BlockingMenusFile,
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
const EMBEDDED_AGENT_HELP_JSON: &str = include_str!("../../resources/agent-help/agent-help.json");

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
fn is_https_url(value: &str) -> bool {
    value
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .is_some_and(|host| !host.is_empty())
}

fn check_agent_help_entry_urls(key: &str, entry: &AgentHelpEntry) -> Result<(), String> {
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

fn refresh_shipped_agent_help_file_with_publish(
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
fn check_remote_agent_help_cap(
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

fn check_remote_agent_help_entry(key: &str, entry: &AgentHelpEntry) -> Result<(), String> {
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
struct RemoteAgentHelpCheckStamp {
    last_checked_at: chrono::DateTime<chrono::Utc>,
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
fn abort_blocking_menus_export(local_path: &Path, why: &str) -> bool {
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
fn export_blocking_menus_to_local_file_with_publish(
    settings: &mut AppSettings,
    settings_path: &Path,
    publish: crate::config::local_config_io::ConfigPublisher<'_>,
) -> bool {
    export_blocking_menus_with_startup(settings, settings_path, publish, None)
}

fn export_blocking_menus_with_startup(
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
const AGENTS_INSTANCE_KEYS: [&str; 2] = ["agents", "codingAgentProfiles"];

/// #2716 (B3) - the agents file sits beside the settings file.
fn agents_instance_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENTS_INSTANCE_FILE_NAME)
}

/// #2716 (B3) - `Ok(None)` when the file is absent. A file that is unreadable, is not
/// a JSON object, or holds a group that does not decode as its `AppSettings` field is
/// an `Err`: callers keep what they have and never replace it.
fn read_agents_instance_file(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    read_agents_instance_file_labeled(path, path)
}

fn read_agents_instance_file_labeled(
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
fn parse_agents_instance_contents(
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
fn agents_instance_groups(object: &Map<String, Value>) -> Map<String, Value> {
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
fn agents_unreadable_dirs() -> &'static std::sync::Mutex<HashSet<PathBuf>> {
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
fn try_with_agents_layer_publication_state_in<R>(
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
fn set_agents_layer_state(
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
fn agents_layer_state_at(settings_path: &Path) -> AgentsLayerState {
    agents_layer_state_at_labeled(settings_path, settings_path)
}

fn agents_layer_state_at_labeled(io_path: &Path, settings_path: &Path) -> AgentsLayerState {
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
fn merge_agents_instance_file(settings_path: &Path, base: &mut Value) -> AgentsLayerState {
    merge_agents_instance_file_labeled(settings_path, settings_path, base)
}

fn merge_agents_instance_file_labeled(
    io_path: &Path,
    settings_path: &Path,
    base: &mut Value,
) -> AgentsLayerState {
    merge_agents_instance_file_with_capture(io_path, settings_path, base, None)
}

fn merge_agents_instance_file_with_capture(
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
fn merge_agents_instance_groups(
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
fn write_agents_instance_file(
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
fn restore_agents_groups_for_redecode(written: &mut Value, groups: Map<String, Value>) {
    if let Value::Object(object) = written {
        object.extend(groups);
        object
            .entry(AGENTS_INSTANCE_KEYS[0])
            .or_insert_with(|| Value::Array(Vec::new()));
    }
}

/// Restore only the backend-owned carrier; ordinary profile edits still apply.
/// Absence in the physical base removes a forged client/overlay carrier.
fn restore_codex_home_carrier(out: &mut Map<String, Value>, base_profiles: Option<&Value>) {
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
fn absent_main_overlay_comparison(
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

fn validate_registered_overlay_profile_edits(
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

fn retain_unknown_registered_agent_fields(
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
fn retain_selected_legacy_profile_aliases(
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

fn comparable_registered_profile_draft(
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

fn validate_split_canonical_profile_edits(
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
fn restrict_split_profile_mutation_rows(
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

fn prepare_registered_profile_owner(
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

fn refresh_registered_agent_row_metadata(target: &mut Map<String, Value>) -> Result<(), String> {
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
fn save_agents_instance_file(
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

fn collect_preserved_agents_groups(
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

fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
    token.chars().filter(|c| *c == quote).count() % 2 == 1
}

fn advance_past_config_value(tokens: &[&str], start: usize) -> usize {
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

fn find_provider_token(tokens: &[&str], provider: &str) -> Option<usize> {
    tokens
        .iter()
        .position(|token| command_token_basename(token) == provider)
}

fn antigravity_has_manual_resume(tokens: &[&str], antigravity_idx: usize) -> bool {
    tokens[antigravity_idx + 1..].iter().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower == "-c"
    })
}

fn codex_has_manual_resume(tokens: &[&str], codex_idx: usize) -> bool {
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

/// #1737 (D5): `settings_path` is `Some` on every production loader and `None` on
/// the in-module test call sites. `None` means "no overlay lookup".
#[cfg(test)]
fn parse_settings_json(
    contents: &str,
    source: &str,
    settings_path: Option<&Path>,
) -> Result<(AppSettings, bool), String> {
    match settings_path {
        None => parse_settings_json_with_projection(contents, source, None, None),
        Some(path) => {
            let transaction =
                empty_project_transaction(path, "parse_settings_json").map_err(|error| {
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
                })?;
            let selected = select_project_source_locked(&transaction).map_err(|error| {
                report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
            })?;
            parse_settings_json_with_projection(
                contents,
                source,
                Some(path),
                Some(&selected.projection),
            )
        }
    }
}

#[cfg(test)]
fn parse_settings_json_with_projection(
    contents: &str,
    source: &str,
    settings_path: Option<&Path>,
    projection: Option<&Map<String, Value>>,
) -> Result<(AppSettings, bool), String> {
    parse_settings_json_with_projection_labeled(
        contents,
        source,
        settings_path,
        settings_path,
        projection,
    )
}

fn parse_settings_json_with_projection_labeled(
    contents: &str,
    source: &str,
    io_path: Option<&Path>,
    settings_path: Option<&Path>,
    projection: Option<&Map<String, Value>>,
) -> Result<(AppSettings, bool), String> {
    parse_settings_with_startup(contents, source, io_path, settings_path, projection, None)
}

fn parse_settings_with_startup(
    contents: &str,
    source: &str,
    io_path: Option<&Path>,
    settings_path: Option<&Path>,
    projection: Option<&Map<String, Value>>,
    startup: Option<&mut Option<StartupAgentRows>>,
) -> Result<(AppSettings, bool), String> {
    if let Some(reason) = duplicate_profile_document(
        contents.as_bytes(),
        settings_path.unwrap_or_else(|| Path::new(source)),
    ) {
        log::warn!("{reason}");
    }
    let value: Value = serde_json::from_str(contents)
        .map_err(|e| format!("Failed to parse settings file: {}", e))?;
    // One read of the agents file serves both the physical-group census and the
    // layer-30 merge; the merge label is the settings path, as before.
    let agents = io_path.map(|io| {
        read_agents_instance_file_labeled(
            &agents_instance_path(io),
            &agents_instance_path(settings_path.unwrap_or(io)),
        )
    });
    let (mut settings, legacy_profiles, agents_layer) = compose_settings_parts(
        value,
        source,
        settings_path,
        projection,
        agents,
        |value| match io_path {
            Some(p) => LocalSettingsOverlay::load_and_merge_paired(
                p,
                value,
                OVERLAY_INELIGIBLE_DISK_KEYS,
                OVERLAY_INELIGIBLE_LEGACY_KEYS,
                OVERLAY_DERIVED_ID_CLOSURES,
                CONFIGURATION_COMMAND_PAIRS,
            ),
            None => LocalSettingsOverlay::default(),
        },
        startup,
    )?;
    if let (Some(p), Some(state)) = (settings_path, agents_layer) {
        set_agents_layer_state(&mut settings, p, state);
    }
    repair_loaded_response_close_settings(&mut settings, source);
    repair_loaded_room_idle_confirmation_settings(&mut settings, source);
    Ok((settings, legacy_profiles))
}

/// One read of the agents file: `Ok(None)` is verified absence.
type AgentsInstanceRead = Result<Option<Map<String, Value>>, String>;

/// Settings, the legacy-profiles flag and the agents layer state, unregistered.
type ComposedSettings = (AppSettings, bool, Option<AgentsLayerState>);

/// The pure composition core of `parse_settings_with_startup`: projection, the
/// six-field decode, agents groups, carrier and physical identity, the overlay
/// over the same lists and pairs, serde, order and id hashes. It performs no IO
/// itself; the agents read and the overlay merge are supplied by the caller, so
/// the owner's single capture reuses this exact sequence over captured bytes.
/// `merge_label` is the settings path that enables the agents merge and the
/// undecodable-overlay fallback. The agents layer state is returned, not
/// registered, so the caller decides whether a process-wide registry changes.
fn compose_settings_parts(
    mut value: Value,
    source: &str,
    merge_label: Option<&Path>,
    projection: Option<&Map<String, Value>>,
    agents: Option<AgentsInstanceRead>,
    overlay_merge: impl FnOnce(&mut Value) -> LocalSettingsOverlay,
    mut startup: Option<&mut Option<StartupAgentRows>>,
) -> Result<ComposedSettings, String> {
    let settings_path = merge_label;
    if let (Some(object), Some(projection)) = (value.as_object_mut(), projection) {
        replace_project_projection(object, projection);
    }
    let legacy_profiles = value.as_object().is_some_and(legacy_profiles_shape_present);
    let main_profile_owner = value.as_object().expect("settings object").clone();
    let physical_sidecar = agents
        .as_ref()
        .and_then(|read| read.as_ref().ok())
        .and_then(Option::as_ref)
        .cloned();
    let physical_groups = agents
        .as_ref()
        .and_then(|read| read.as_ref().ok())
        .and_then(Option::as_ref)
        .map(|groups| {
            groups
                .keys()
                .filter(|key| AGENTS_INSTANCE_KEYS.contains(&key.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // #2716 (B3): the agents file is layer 30, under the layer-50 overlay, so it is
    // merged into the raw base object here: after the detection above, so
    // `legacy_profiles` stays a property of the settings file, and before the
    // `pre_merge` clone, so the undecodable fallback carries its groups.
    let agents_layer = settings_path.zip(agents).map(|(label, read)| {
        merge_agents_instance_groups(
            agents_instance_path(label),
            read,
            &mut value,
            startup.as_deref_mut(),
        )
    });
    // #1737 (D21): the clone must be taken BEFORE the merge, because the merge is
    // in place and the overlay's presence is not known until it returns. Gated on
    // `settings_path.is_some()`, which is true on all three production loaders and
    // false on the in-module test call sites. Cost: one Value clone per load.
    let pre_merge = settings_path.map(|_| value.clone());
    // #1737 (D5): the merge point, after the legacy-shape detection and before the
    // project decode. Merging after the detection keeps the `legacy_profiles` flag
    // a property of the base file alone and makes the B7 cross-key edge impossible.
    let physical_base = value.clone();
    let mut overlay = overlay_merge(&mut value);
    let profile_owner = selected_registered_profile_owner(
        &main_profile_owner,
        physical_sidecar.as_ref(),
        !overlay.owns_top_level("codingAgentProfiles"),
    );
    let profile_owner_view = registered_profiles_for_read(&profile_owner);
    if let (Some(object), Some(base)) = (value.as_object_mut(), &pre_merge) {
        restore_codex_home_carrier(object, base.get("codingAgentProfiles"));
    }
    let mut identity_physical =
        coding_agent_identity_physical_from_raw(&value, &physical_groups, &overlay);
    let unmerged = if overlay.is_empty() { None } else { pre_merge };
    // #1077: decode the six project fields and replace the three runtime fields
    // with the SELECTED canonical paths BEFORE deserializing the rest of
    // AppSettings, so a wrong-typed project field cannot fail unrelated settings
    // deserialization and unresolved/conflicting pairs survive in hidden state.
    let base = production_instance_base();
    // #2716 (B3, option C): with the agents file unreadable, the decode error also
    // names it, so the dialog does not name only the intact settings file.
    let unreadable_agents_file = agents_layer
        .as_ref()
        .and_then(AgentsLayerState::unreadable_path)
        .map(|p| format!("; the agents file {} could not be read", p.display()))
        .unwrap_or_default();
    let decode_error = |e: serde_json::Error| {
        format!("Failed to deserialize settings from {source}: {e}{unreadable_agents_file}")
    };
    let mut projected = physical_base.clone();
    if let Some(profiles) = profile_owner_view.get("codingAgentProfiles") {
        projected["codingAgentProfiles"] = profiles.clone();
    }
    if let (Some(effective), Some(base)) = (value.as_object(), projected.as_object_mut()) {
        overlay.reapply_from(effective, base);
    }
    value = projected;
    let state =
        apply_project_decode_to_value(&mut value, base.as_deref(), &projects::FsCandidateResolver);
    let decoded: Result<AppSettings, String> = serde_json::from_value(value).map_err(decode_error);
    // #1737 (D21): a merged value that fails to decode falls back to the BASE
    // value, never to defaults. One wrong-typed leaf in settings.local.json must
    // not replace a valid base configuration.
    let (mut settings, state) = match (decoded, unmerged) {
        (Ok(settings), _) => (settings, state),
        (Err(reason), Some(mut unmerged)) => {
            identity_physical["agents"] = unmerged.get("agents").cloned().unwrap_or(Value::Null);
            identity_physical["codingAgentProfiles"] = unmerged
                .get("codingAgentProfiles")
                .cloned()
                .unwrap_or(Value::Null);
            identity_physical["quotaSources"] =
                unmerged.get("quotaSources").cloned().unwrap_or(Value::Null);
            identity_physical["groupOwnership"]["overlayPaths"] = serde_json::json!([]);
            let base_state = apply_project_decode_to_value(
                &mut unmerged,
                base.as_deref(),
                &projects::FsCandidateResolver,
            );
            if let Some(profiles) = profile_owner_view.get("codingAgentProfiles") {
                unmerged["codingAgentProfiles"] = profiles.clone();
            }
            let settings: AppSettings = serde_json::from_value(unmerged).map_err(decode_error)?;
            overlay = overlay.into_undecodable(reason);
            (settings, base_state)
        }
        (Err(reason), None) => return Err(reason),
    };
    settings.project_path_state = Arc::new(state);
    settings.coding_agent_identity_physical = Some(Arc::new(identity_physical));
    // #2306 P1 - in-memory finalize over the merged view. It is deliberately not
    // part of `needs_save`: a legacy file with no ordinals must load without a
    // disk write or a backup rotation.
    finalize_agent_order(&mut settings.agents);
    refresh_agent_id_hashes(&mut settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    report_overlay_diagnostics(source, &overlay);
    settings.local_overlay_state = Arc::new(overlay);
    if settings.local_overlay_state.owns_top_level("agents") {
        if let Some(startup) = startup {
            *startup = None;
        }
    }
    Ok((settings, legacy_profiles, agents_layer))
}

/// #2015: legacy pre-v2 codingAgentProfiles are no longer migrated. Detection only;
/// never mutates the value.
fn legacy_profiles_shape_present(root: &Map<String, Value>) -> bool {
    let Some(p) = root.get("codingAgentProfiles").and_then(Value::as_object) else {
        return false;
    };
    if ["letters", "matrix", "agentDefaults"]
        .iter()
        .any(|key| p.contains_key(*key))
    {
        return true;
    }
    if matches!(p.get("schemaVersio\
        n"), Some(Value::Number(n)) if n.as_f64().is_some_and(|v| v < 2.0))
    {
        return true;
    }
    if p.get("profileSlots")
        .and_then(Value::as_object)
        .is_some_and(|slots| {
            slots
                .values()
                .any(|slot| slot.as_object().is_some_and(|s| s.contains_key("name")))
        })
    {
        return true;
    }
    p.get("profilesByAgent")
        .and_then(Value::as_object)
        .is_some_and(|by_agent| {
            by_agent.values().filter_map(Value::as_object).any(|cells| {
                cells
                    .values()
                    .filter_map(Value::as_object)
                    .any(|cell| cell.contains_key("argv") || cell.contains_key("args"))
            })
        })
}

// A preview of this owner's profile property only. Retained opaque legacy
// leaves are diagnostics, not a reason to republish canonical data forever.
fn actionable_profile_legacy_work(owner: &Value, allow_main_cleanup: bool) -> Result<bool, String> {
    let Some(profiles) = owner.get("codingAgentProfiles").and_then(Value::as_object) else {
        return Ok(false);
    };
    let stale_active_maps = ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"]
        .iter()
        .any(|key| profiles.contains_key(*key));
    let removable_roots = allow_main_cleanup
        && ["letters", "matrix", "agentDefaults"]
            .iter()
            .any(|key| profiles.contains_key(*key));
    if profiles.get("profileBindingVersion") == Some(&Value::from(1))
        && !stale_active_maps
        && !removable_roots
    {
        return Ok(false);
    }
    let projected = project_physical_profiles(owner, true)?;
    let mut candidate = canonical_profile_source(owner, &projected, true)?;
    if let Some(profiles) = candidate
        .get_mut("codingAgentProfiles")
        .and_then(Value::as_object_mut)
        .filter(|_| allow_main_cleanup)
    {
        for key in ["letters", "matrix", "agentDefaults"] {
            profiles.remove(key);
        }
    }
    Ok(candidate.get("codingAgentProfiles") != owner.get("codingAgentProfiles"))
}

fn startup_profile_legacy_work(
    settings: &AppSettings,
    path: &Path,
    original: Option<&str>,
    main_legacy_detected: bool,
) -> Result<Option<bool>, String> {
    // O2 explicitly retains baseline scheduling for suppressed groups.
    if settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles")
    {
        return Ok(Some(main_legacy_detected));
    }
    let main: Value =
        serde_json::from_str(original.unwrap_or("{}")).map_err(|error| error.to_string())?;
    let sidecar = read_agents_instance_file(&agents_instance_path(path))?;
    let sidecar_profiles = sidecar
        .as_ref()
        .is_some_and(|groups| groups.contains_key("codingAgentProfiles"));
    let owner = selected_registered_profile_owner(
        main.as_object().ok_or("invalidSettingsObject")?,
        sidecar.as_ref(),
        true,
    );
    let encountered = owner
        .get("codingAgentProfiles")
        .and_then(Value::as_object)
        .is_some_and(|profiles| {
            ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"]
                .iter()
                .any(|key| profiles.contains_key(*key))
        });
    // An admitted legacy transfer is ordinary publication work even when
    // ambiguous rows forbid conversion. The locked writer makes that decision.
    let main_transfer = AGENTS_INSTANCE_KEYS.iter().any(|key| {
        if settings.local_overlay_state.owns_top_level(key) {
            return false;
        }
        main.get(*key).is_some()
            && !sidecar
                .as_ref()
                .is_some_and(|groups| groups.contains_key(*key))
    }) && owner.pointer("/codingAgentProfiles/profileBindingVersion")
        != Some(&Value::from(1));
    if main_transfer {
        return Ok(Some(true));
    }
    if owner.get("codingAgentProfiles").is_none()
        && owner.get("agents").is_some()
        && physical_profile_rows(&owner, true).is_ok_and(|rows| !rows.is_empty())
    {
        return Ok(Some(true));
    }
    if !owner.as_object().is_some_and(legacy_profiles_shape_present) && !encountered {
        return Ok(Some(false));
    }
    // Ambiguous raw row paths do not grant new canonical cleanup authority.
    if physical_profile_rows(&owner, true).is_err() {
        return Ok(None);
    }
    actionable_profile_legacy_work(&owner, !sidecar_profiles).map(Some)
}

/// #1737 (D15) - the settings value when the base `settings.json` is absent,
/// unreadable or unparseable. A valid `settings.local.json` still applies, over the
/// serialized defaults, so the file the following `needs_save` write creates holds
/// exactly what a no-overlay fresh instance would have written and deleting the
/// local file restores it.
#[cfg(test)]
fn default_settings_with_overlay(settings_path: &Path, _source: &str) -> AppSettings {
    match capture_project_settings(settings_path, false) {
        Ok((settings, _, _)) => settings,
        Err(error) => unavailable_project_settings(AppSettings::default(), error),
    }
}

fn default_settings_with_overlay_captured(
    settings_path: &Path,
    source: &str,
    projection: &Map<String, Value>,
) -> AppSettings {
    // Total by construction: `AppSettings` is a struct with named fields, so serde
    // always produces an object. The arm exists so the function has no panic.
    let Ok(mut value @ Value::Object(_)) = serde_json::to_value(AppSettings::default()) else {
        return AppSettings::default();
    };
    let overlay = LocalSettingsOverlay::load_and_merge_paired(
        settings_path,
        &mut value,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    let agents_base = read_agents_instance_file(&agents_instance_path(settings_path))
        .ok()
        .flatten();
    restore_codex_home_carrier(
        value.as_object_mut().unwrap(),
        agents_base
            .as_ref()
            .and_then(|base| base.get("codingAgentProfiles")),
    );
    // Unconditionally, and BEFORE the early return: `is_empty()` is also true for a
    // REJECTED overlay, so returning first would swallow the very diagnostics D8
    // mandates on a malformed local file.
    report_overlay_diagnostics(source, &overlay);
    replace_project_projection(value.as_object_mut().unwrap(), projection);
    let state = apply_project_decode_to_value(
        &mut value,
        production_instance_base().as_deref(),
        &projects::FsCandidateResolver,
    );
    match serde_json::from_value::<AppSettings>(value) {
        Ok(mut settings) => {
            settings.project_path_state = Arc::new(state);
            // #2306 P1 - same in-memory finalize as the normal parse, applied to
            // the defaults-plus-overlay view; still no write.
            finalize_agent_order(&mut settings.agents);
            refresh_agent_id_hashes(&mut settings.agents);
            refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
            settings.local_overlay_state = Arc::new(overlay);
            repair_loaded_response_close_settings(&mut settings, source);
            repair_loaded_room_idle_confirmation_settings(&mut settings, source);
            settings
        }
        Err(e) => {
            // D21 with `AppSettings::default()` as the fallback. 4.2e step 5 says
            // "record `MergedValueUndecodable`, render THAT record", singular: the
            // ineligible-key drops were already rendered above and
            // `into_undecodable` preserves them, so rendering this overlay's whole
            // record list would emit every drop a second time. A default overlay
            // carries the new record alone, and the value returned is
            // `AppSettings::default()` either way, so the state the caller adopts
            // is unchanged.
            let undecodable = LocalSettingsOverlay::default().into_undecodable(e.to_string());
            report_overlay_diagnostics(source, &undecodable);
            AppSettings::default()
        }
    }
}

fn replace_project_projection(object: &mut Map<String, Value>, projection: &Map<String, Value>) {
    for key in [
        FIELD_PROJECT_PATH,
        FIELD_PROJECT_PATH_REL,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    ] {
        object.remove(key);
    }
    object.extend(projection.clone());
}

fn unavailable_project_settings(mut settings: AppSettings, error: String) -> AppSettings {
    settings.project_paths.clear();
    settings.project_path = None;
    settings.archived_project_paths.clear();
    settings.project_path_state = Arc::new(ProjectPathPersistenceState::default());
    settings.project_paths_persistence_error = Some(error);
    settings
}

#[cfg(test)]
type SettingsPathHook = Option<Box<dyn Fn(&Path)>>;

#[cfg(test)]
thread_local! {
    static PROJECT_SOURCE_CAPTURE_HOOK: std::cell::RefCell<SettingsPathHook> =
        std::cell::RefCell::new(None);
    static SETTINGS_PRESERVE_LOCK_HOOK: std::cell::RefCell<SettingsPathHook> =
        std::cell::RefCell::new(None);
}

fn capture_project_settings(
    path: &Path,
    migrate: bool,
) -> Result<(AppSettings, bool, Option<String>), String> {
    capture_project_settings_with_startup(path, migrate, None)
}

fn capture_project_settings_with_startup(
    path: &Path,
    migrate: bool,
    mut startup: Option<&mut Option<StartupAgentRows>>,
) -> Result<(AppSettings, bool, Option<String>), String> {
    let lock_error = |reason| {
        SettingsSaveError::semantic(
            SettingsSaveStage::LockAcquire,
            path,
            None,
            reason,
            SettingsSaveLegacyOutward::SettingsLockUnavailable,
        )
    };
    let report = |error: SettingsSaveError| {
        report_settings_save_error(
            error.with_capture_labels(path),
            SettingsSaveReportSurface::GeneralSettings,
        )
    };
    let parent = path
        .parent()
        .ok_or_else(|| report(lock_error(SettingsSaveReason::MissingParentDirectory)))?;
    std::fs::create_dir_all(parent).map_err(|source| {
        report(SettingsSaveError::io(
            SettingsSaveStage::LockAcquire,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsLockUnavailable,
        ))
    })?;
    match crate::path_identity::verify_component_chain_reason(parent) {
        Ok(()) | Err(crate::path_identity::PathRule::ComponentIsLink) => {}
        Err(_) => return Err(report(lock_error(SettingsSaveReason::LockIdentityRejected))),
    }
    let physical_parent = std::fs::canonicalize(parent)
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    // Windows retains a no-share-delete handle. Unix detects persistent
    // replacement, not transient A -> B -> A absolute-path substitution.
    let retained = crate::path_identity::retain_directory(&physical_parent)
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    let leaf = path
        .file_name()
        .ok_or_else(|| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    let physical = retained.identity().canonical_path.join(leaf);
    let transaction = empty_project_transaction(&physical, "capture_project_settings")
        .map_err(|error| report(error.with_capture_labels(path)))?;
    retained
        .verify_current()
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    #[cfg(test)]
    PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| {
        if let Some(callback) = hook.borrow().as_ref() {
            callback(path);
        }
    });
    let raw_profile_capture = collect_registered_profile_gate(&transaction.path);
    if let Some(reason) = raw_profile_capture.ordinary_error() {
        log::warn!("[profiles] {reason}");
    }
    let migration_error = if migrate
        && raw_profile_capture.admits_project_migration(&transaction.path)
    {
        let mut checked = false;
        ensure_project_paths_migrated_at_labeled_path(&transaction.path, path, &mut |stage, _| {
            if !checked
                && matches!(
                    stage,
                    "backup_create" | "destination_before_write" | "source_cleanup"
                )
            {
                recheck_registered_profile_gate(&raw_profile_capture.participant_documents())
                    .map_err(|reason| {
                        project_migration_error_labeled(
                            &transaction.path,
                            path,
                            SettingsSaveStage::ProjectPathsRefreshDiskGate,
                            &reason,
                        )
                    })?;
                checked = true;
            }
            Ok(())
        })
        .err()
    } else {
        None
    };
    let selected = match migration_error {
        Some(error) => Err(error),
        None => select_project_source_at_labeled_path_with_limit(
            &transaction.path,
            path,
            16 * 1024 * 1024,
        ),
    };
    let captured = if raw_profile_capture
        .problems
        .iter()
        .any(|problem| problem.kind == RegisteredProfileProblemKind::DuplicateMember)
        && selected
            .as_ref()
            .err()
            .is_none_or(|error| error.target_path.as_path() == transaction.path.as_path())
    {
        // The ordinary reader remains lenient. The strict project selector must
        // not turn an unrelated duplicate into project persistence failure.
        let contents = std::fs::read_to_string(&transaction.path).unwrap_or_else(|_| {
            serde_json::to_string(&AppSettings::default()).expect("default settings")
        });
        let legacy: Value =
            serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}));
        let mut probe = legacy.clone();
        apply_project_decode_to_value(
            &mut probe,
            production_instance_base().as_deref(),
            &projects::FsCandidateResolver,
        );
        let projection = selected
            .as_ref()
            .ok()
            .map(|selected| selected.projection.clone())
            .unwrap_or_else(|| {
                probe
                    .as_object()
                    .map(|map| {
                        let mut projection = Map::new();
                        for key in [
                            FIELD_PROJECT_PATH,
                            FIELD_PROJECT_PATHS,
                            FIELD_ARCHIVED,
                            FIELD_PROJECT_PATH_REL,
                            FIELD_PROJECT_PATHS_REL,
                            FIELD_ARCHIVED_REL,
                        ] {
                            if let Some(value) = map.get(key) {
                                projection.insert(key.to_owned(), value.clone());
                            }
                        }
                        projection
                    })
                    .unwrap_or_default()
            });
        let parsed = parse_settings_with_startup(
            &contents,
            &path.to_string_lossy(),
            Some(&transaction.path),
            Some(path),
            Some(&projection),
            startup.as_deref_mut(),
        );
        parsed.map(|(settings, legacy)| (settings, legacy, Some(contents)))
    } else {
        (|| {
            match selected {
                Ok(selected) => match selected.legacy {
                    Some(legacy) => {
                        let contents =
                            String::from_utf8(legacy.bytes).map_err(|error| error.to_string())?;
                        match parse_settings_with_startup(
                            &contents,
                            &path.to_string_lossy(),
                            Some(&transaction.path),
                            Some(path),
                            Some(&selected.projection),
                            startup.as_deref_mut(),
                        ) {
                            Ok((settings, legacy_profiles)) => {
                                Ok((settings, legacy_profiles, Some(contents)))
                            }
                            Err(error) => {
                                if let Some(startup) = startup {
                                    *startup = None;
                                }
                                log::error!("{error}");
                                // Selection succeeded; unrelated decode failure must not
                                // mark the authoritative project source unavailable.
                                let mut settings = default_settings_with_overlay_captured(
                                    &transaction.path,
                                    &path.to_string_lossy(),
                                    &selected.projection,
                                );
                                set_agents_layer_state(
                                    &mut settings,
                                    path,
                                    agents_layer_state_at_labeled(&transaction.path, path),
                                );
                                Ok((settings, false, None))
                            }
                        }
                    }
                    None => Ok((
                        default_settings_with_overlay_captured(
                            &transaction.path,
                            &path.to_string_lossy(),
                            &selected.projection,
                        ),
                        false,
                        None,
                    )),
                },
                Err(error) => {
                    let message = report_settings_save_error(
                        error.with_capture_labels(path),
                        SettingsSaveReportSurface::GeneralSettings,
                    );
                    // Preserve readable unrelated values while exposing no project authorization.
                    let mut settings = read_raw_project_source_labeled_with_limit(
                        &transaction.path,
                        path,
                        16 * 1024 * 1024,
                        false,
                    )
                    .ok()
                    .flatten()
                    .and_then(|legacy| {
                        let contents = String::from_utf8(legacy.bytes).ok()?;
                        parse_settings_json_with_projection_labeled(
                            &contents,
                            &path.to_string_lossy(),
                            Some(&transaction.path),
                            Some(path),
                            Some(&Map::new()),
                        )
                        .ok()
                        .map(|pair| pair.0)
                    })
                    .unwrap_or_else(|| {
                        default_settings_with_overlay_captured(
                            &transaction.path,
                            &path.to_string_lossy(),
                            &Map::new(),
                        )
                    });
                    set_agents_layer_state(
                        &mut settings,
                        path,
                        agents_layer_state_at_labeled(&transaction.path, path),
                    );
                    Ok((unavailable_project_settings(settings, message), false, None))
                }
            }
        })()
    };
    // Every fallback passes this check while the same lock is still held.
    if retained.verify_current().is_err() {
        let message = report(lock_error(SettingsSaveReason::LockIdentityRejected));
        return match captured {
            Ok((settings, _, _)) => {
                Ok((unavailable_project_settings(settings, message), false, None))
            }
            Err(_) => Err(message),
        };
    }
    captured
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

fn is_identity_shape(identity: &str) -> bool {
    identity.len() == 16 && identity.bytes().all(|b| b.is_ascii_hexdigit())
}

/// #2450: an agent's profile entry holds nothing real: absent, empty, or exactly
/// the empty `A` that repair creates, with no label overrides.
fn is_profile_gap(profiles: &CodingAgentProfilesConfig, id: &str) -> bool {
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
fn coding_agent_identity_physical_from_raw(
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
fn coding_agent_identity_with_physical_overlay(
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
fn refresh_coding_agent_identity_at_locked_path(settings: &mut AppSettings, path: &Path) {
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
fn write_identity_rows(out: &mut Map<String, Value>, overlay: &LocalSettingsOverlay) {
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

fn validate_codex_home_basic<'a>(value: &'a str, context: &str) -> Result<&'a str, String> {
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

fn reject_unknown_codex_home_template_markers(value: &str, context: &str) -> Result<(), String> {
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

fn validate_env_map(env: &BTreeMap<String, String>, context: &str) -> Result<(), String> {
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

fn normalize_agent_backend_config(
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

fn normalize_agent_backend_configs(settings: &mut AppSettings) -> Result<(), String> {
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

pub fn validate_and_repair_settings(settings: &mut AppSettings) -> Result<(), String> {
    if !(RESPONSE_CLOSE_IDLE_SECONDS_MIN..=RESPONSE_CLOSE_IDLE_SECONDS_MAX)
        .contains(&settings.response_close_idle_seconds)
    {
        return Err("responseCloseIdleSeconds must be between 1 and 3600 seconds".to_string());
    }
    if !(ROOM_IDLE_CONFIRMATION_SECONDS_MIN..=ROOM_IDLE_CONFIRMATION_SECONDS_MAX)
        .contains(&settings.room_idle_confirmation_seconds)
    {
        return Err("roomIdleConfirmationSeconds must be between 1 and 3600 seconds".to_string());
    }
    validate_preflight_timeouts(settings)?;
    adopt_orphaned_profiles(settings);
    normalize_agent_backend_configs(settings)?;
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);
    validate_agent_commands(settings)?;
    refresh_agent_id_hashes(&mut settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    validate_screenshot_hotkey(&settings.screenshot_capture_hotkey)?;
    validate_sidebar_compact_hotkey(&settings.sidebar_compact_hotkey)?;
    validate_room_number_mask(&settings.room_number_mask)?;
    validate_api_server_settings(settings)?;
    validate_typing_hold_settings(settings)?;
    validate_resource_settings(settings)
}

pub const ROOM_NUMBER_MASK_MAX_WIDTH: usize = 9;

/// #2507 Returns the pad width. `Err` blocks the settings save.
pub fn validate_room_number_mask(value: &str) -> Result<usize, String> {
    let reject = |reason: &str| Err(format!("Room number mask: {reason}"));
    if value.is_empty() {
        return reject("must be at least one '#'");
    }
    if !value.chars().all(|c| c == '#') {
        return reject("must contain only '#' characters, e.g. ## or ###");
    }
    if value.len() > ROOM_NUMBER_MASK_MAX_WIDTH {
        return reject(&format!(
            "must be at most {ROOM_NUMBER_MASK_MAX_WIDTH} '#' characters"
        ));
    }
    Ok(value.len())
}

/// #2336 - reject a settings update whose typing-hold window is outside the
/// accepted range. Called on every save path, so the invalid candidate is
/// discarded before `save_settings` and the persisted (and live) value stays
/// untouched.
pub fn validate_typing_hold_settings(settings: &AppSettings) -> Result<(), String> {
    let seconds = settings.typing_hold_seconds;
    if !(TYPING_HOLD_SECONDS_MIN..=TYPING_HOLD_SECONDS_MAX).contains(&seconds) {
        return Err(format!(
            "typingHoldSeconds must be between {} and {} (got {})",
            TYPING_HOLD_SECONDS_MIN, TYPING_HOLD_SECONDS_MAX, seconds
        ));
    }
    Ok(())
}

pub(crate) fn parse_api_server_socket_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let bind = bind.trim();
    if bind.is_empty() {
        return Err("apiServerBind must not be empty".to_string());
    }
    if port == 0 {
        return Err("apiServerPort must be between 1 and 65535".to_string());
    }
    let ip: IpAddr = bind.parse().map_err(|e| {
        format!("apiServerBind must be an IP address such as 127.0.0.1, 0.0.0.0, ::1, or :: ({e})")
    })?;
    Ok(SocketAddr::new(ip, port))
}

pub fn validate_api_server_settings(settings: &mut AppSettings) -> Result<(), String> {
    let bind = settings.api_server_bind.trim().to_string();
    parse_api_server_socket_addr(&bind, settings.api_server_port)?;
    settings.api_server_bind = bind;
    Ok(())
}

/// #714 Reject a screenshot hotkey that the native parser cannot accept. Syntax
/// errors block a settings save; OS-level registration conflicts do not (they are
/// surfaced as runtime status by `screenshot::register_configured_hotkey`).
pub fn validate_screenshot_hotkey(value: &str) -> Result<(), String> {
    crate::screenshot::parse_screenshot_hotkey(value)
        .map(|_| ())
        .map_err(|e| format!("Screenshot hotkey: {}", e))
}

/// #2281 Reject a sidebar compact hotkey that the settings contract cannot
/// accept. Unlike `validate_screenshot_hotkey` this is not an OS registration:
/// it only enforces the shape `Ctrl|Control + Shift + <A-Z>` minus the reserved
/// letters `W`, `R`, `C`, `V` (case-insensitive parts). Digits are rejected:
/// `Ctrl+Shift+2` reaches xterm as `key="@"`, which Keyboard.ts maps to NUL and
/// writes to the PTY. Syntax errors block a settings save; the value is judged
/// byte-identical and never repaired, so a lower-case spelling is stored as-is.
pub fn validate_sidebar_compact_hotkey(value: &str) -> Result<(), String> {
    let reject = |reason: &str| Err(format!("Sidebar compact hotkey: {reason}"));
    let parts: Vec<&str> = value.trim().split('+').map(|p| p.trim()).collect();
    if parts.len() != 3 {
        return reject("expected exactly three '+' separated parts, e.g. Ctrl+Shift+E");
    }
    if !parts[0].eq_ignore_ascii_case("ctrl") && !parts[0].eq_ignore_ascii_case("control") {
        return reject("first part must be Ctrl or Control");
    }
    if !parts[1].eq_ignore_ascii_case("shift") {
        return reject("second part must be Shift");
    }
    let mut chars = parts[2].chars();
    let key = match (chars.next(), chars.next()) {
        (Some(c), None) => c,
        _ => return reject("third part must be exactly one character"),
    };
    if !key.is_ascii_alphabetic() {
        return reject("third part must be an ASCII letter");
    }
    if matches!(key.to_ascii_lowercase(), 'w' | 'r' | 'c' | 'v') {
        return reject("third part must not be one of the reserved letters W, R, C, V");
    }
    Ok(())
}

pub fn merge_protected_coding_agent_settings(
    current: &AppSettings,
    mut incoming: AppSettings,
) -> AppSettings {
    incoming.coding_agent_profiles = current.coding_agent_profiles.clone();

    for incoming_agent in &mut incoming.agents {
        if let Some(current_agent) = current.agents.iter().find(|a| a.id == incoming_agent.id) {
            incoming_agent.envs = current_agent.envs.clone();
            incoming_agent.isolated_home = current_agent.isolated_home;
        }
    }

    incoming
}

/// #2162 - the OS suffix tokens a config-seed template folder may carry.
pub const CONFIG_SEED_OS_TOKENS: [&str; 3] = ["linux", "windows", "macos"];

/// #2162 - `Some(stem)` when `dest` ends with an OS token, where `stem` is the
/// dest it collides with. `stem` is empty when `dest` is exactly `".<token>"`.
pub fn config_seed_dest_os_token_stem(dest: &str) -> Option<String> {
    let lower = dest.trim().to_ascii_lowercase();
    CONFIG_SEED_OS_TOKENS
        .iter()
        .find(|t| lower.ends_with(&format!(".{t}")))
        .map(|t| lower[..lower.len() - t.len() - 1].to_string())
}

/// A config-seed `dest` must be a single, relative folder NAME under the replica
/// root. The convention prefixes (`default`, `default_profile_<letter>`) are
/// concatenated onto this name for the workspace-root tiers, so it must be a
/// single path segment and a legal Windows directory name.
pub fn validate_config_seed_dest(dest: &str) -> Result<(), String> {
    let d = dest.trim();
    if d.is_empty() {
        return Err("config seed destination must not be empty".to_string());
    }
    if d.contains('/') || d.contains('\\') {
        return Err(
            "config seed destination must be a single folder name (no path separators)".to_string(),
        );
    }
    if d == "." || d == ".." || d.contains("..") {
        return Err("config seed destination must not contain '..'".to_string());
    }
    if Path::new(d).is_absolute() {
        return Err("config seed destination must be relative".to_string());
    }
    // M5: ':' makes "C:foo" drive-relative (is_absolute()==false) or "x:y" an ADS.
    if d.contains(':') {
        return Err("config seed destination must not contain ':'".to_string());
    }
    if d.contains('%') || d.contains('$') {
        return Err("config seed destination must not contain placeholder markers".to_string());
    }
    // M5: reject a trailing dot (Windows strips it, causing target drift). A
    // trailing space cannot reach here: `d` was trimmed above.
    if d.ends_with('.') {
        return Err("config seed destination must not end with a dot".to_string());
    }
    // M5: reject Windows reserved device names (case-insensitive, before any extension).
    let stem = d.split('.').next().unwrap_or(d).to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!(
            "config seed destination '{}' is a reserved Windows device name",
            d
        ));
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

pub fn validate_resource_settings(settings: &AppSettings) -> Result<(), String> {
    // Floor at 1 (a value of 0 would block every agent launch); no upper ceiling.
    // The user sets this deliberately and is responsible for sizing it to their machine.
    if settings.max_concurrent_agent_processes == 0 {
        return Err("maxConcurrentAgentProcesses must be at least 1".to_string());
    }
    if settings.agent_group_warn_private_bytes > settings.agent_group_kill_private_bytes {
        return Err(
            "agentGroupWarnPrivateBytes must be less than or equal to agentGroupKillPrivateBytes"
                .to_string(),
        );
    }
    if settings.agent_group_kill_private_bytes == 0 {
        return Err("agentGroupKillPrivateBytes must be greater than 0".to_string());
    }
    if settings.agent_process_kill_private_bytes == 0 {
        return Err("agentProcessKillPrivateBytes must be greater than 0".to_string());
    }
    Ok(())
}

pub(crate) fn settings_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join(SETTINGS_FILE_NAME))
}

/// Load settings from the app config directory (see config_dir()), falling back to defaults.
/// Auto-generates a root_token if missing and persists it.
pub fn load_settings() -> AppSettings {
    let path = match settings_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory, using defaults");
            return AppSettings::default();
        }
    };

    load_settings_from_path(&path)
}

/// `pub(crate)` only so `commands::config`'s #1737 tests can seed a
/// `settings.json` plus `settings.local.json` pair in a tempdir and load it
/// through the real loader instead of reproducing the loader in a test. No
/// production caller outside this module.
pub(crate) fn load_settings_from_path(path: &Path) -> AppSettings {
    load_settings_from_path_with_blocking_menus_publish(path, None)
}

fn load_settings_from_path_with_blocking_menus_publish(
    path: &Path,
    publish: Option<crate::config::local_config_io::ConfigPublisher<'_>>,
) -> AppSettings {
    let mut startup = None;
    let (mut settings, legacy_profiles_detected, legacy_profiles_contents) =
        read_settings_for_startup_with_capture(path, Some(&mut startup));
    if settings.project_paths_persistence_error.is_some() {
        return settings;
    }
    apply_loaded_window_migrations(&mut settings);

    // The main-file diagnostic remains separate for its original-byte backup.
    // A sidecar-owned legacy cause must be reachable without that diagnostic.
    let (legacy_profile_work, profile_preview_rejected) = match startup_profile_legacy_work(
        &settings,
        path,
        legacy_profiles_contents.as_deref(),
        legacy_profiles_detected,
    ) {
        Ok(Some(actionable)) => (actionable, false),
        Ok(None) => {
            log::warn!("[profiles] legacy action awaits unambiguous owner routing");
            (legacy_profiles_detected, false)
        }
        Err(reason) => {
            log::warn!("[profiles] legacy action preview rejected: {reason}");
            (false, true)
        }
    };

    // #248 migration — translates legacy startOnlyCoordinators (if present).
    // Track whether the legacy field was on disk so we can fire a single save
    // at the end of the function (Grinch Z3 — without this, upgrade users
    // with an existing root_token never persist the migration and the legacy
    // key lingers in settings.json, spamming the migration log on every launch).
    let issue_248_migrated = settings.legacy_start_only_coordinators.is_some();
    apply_issue_248_migration(&mut settings);

    // Auto-generate root token if missing.
    let mut needs_save = issue_248_migrated || legacy_profile_work;
    if settings.root_token.is_none() {
        settings.root_token = Some(uuid::Uuid::new_v4().to_string());
        log::info!("Generated new root token");
        needs_save = true;
    }
    let mut startup_writable = if profile_preview_rejected {
        Err("profile migration preview rejected; no startup publication".to_owned())
    } else {
        registered_profile_raw_gate(path)
    };
    if legacy_profile_work
        && startup_writable.is_ok()
        && !settings
            .local_overlay_state
            .owns_top_level("codingAgentProfiles")
    {
        // Enter the admitted Preserve funnel before raw exports/adoption/repair.
        // Its complete source preflight precedes migration and backup publishers.
        match save_settings_value_with_startup(
            &settings,
            path,
            ProjectWriteMode::Preserve,
            startup.as_ref(),
        ) {
            Ok(written) => {
                settings = written;
                needs_save = false;
                // A successful transfer changes the physical row owner. Later
                // independent startup work must use a fresh correspondence image.
                if let Err(reason) =
                    capture_project_settings_with_startup(path, false, Some(&mut startup))
                {
                    startup_writable = Err(reason);
                }
            }
            Err(error) => {
                let reason =
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings);
                startup_writable = Err(reason);
            }
        }
    }
    if let Err(reason) = &startup_writable {
        log::warn!("[profiles] startup publication skipped: {reason}");
    }
    let exported = startup_writable.is_ok()
        && export_blocking_menus_with_startup(
            &mut settings,
            path,
            publish.unwrap_or(&crate::config::local_config_io::publish_temp_config),
            startup.as_mut(),
        );
    if exported {
        log::info!("[settings-migration] #1905 - moved blockingMenus out of settings.json");
        needs_save = true;
    }
    if startup_writable.is_ok() && export_agents_to_instance_file(&mut settings, path) {
        needs_save = true;
    }
    if startup_writable.is_ok() && adopt_orphaned_profiles(&mut settings) {
        needs_save = true;
    }
    if startup_writable.is_ok()
        && repair_coding_agent_profiles_config(
            &mut settings.coding_agent_profiles,
            &settings.agents,
        )
    {
        log::info!("[settings-migration] repaired codingAgentProfiles invariants");
        needs_save = true;
    }
    if needs_save && startup_writable.is_ok() {
        persist_loaded_settings_migrations(
            &mut settings,
            path,
            legacy_profiles_detected,
            &legacy_profiles_contents,
            startup.as_ref(),
        );
    }

    // Startup migrations may move ownership or materialize profiles. Birth seeds
    // must describe the post-migration physical inputs, never the provisional load.
    settings.coding_agent_identity_physical = capture_project_settings(path, false)
        .ok()
        .and_then(|(captured, _, _)| captured.coding_agent_identity_physical);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    settings
}

#[cfg(test)]
fn read_settings_for_startup(path: &Path) -> (AppSettings, bool, Option<String>) {
    read_settings_for_startup_with_capture(path, None)
}

fn read_settings_for_startup_with_capture(
    path: &Path,
    startup: Option<&mut Option<StartupAgentRows>>,
) -> (AppSettings, bool, Option<String>) {
    match capture_project_settings_with_startup(path, true, startup) {
        Ok(captured) => captured,
        Err(error) => {
            let mut settings = AppSettings::default();
            set_agents_layer_state(&mut settings, path, agents_layer_state_at(path));
            (unavailable_project_settings(settings, error), false, None)
        }
    }
}

fn apply_loaded_window_migrations(settings: &mut AppSettings) {
    // 0.8.0 unified-window migration — seed main_* from legacy fields on first load
    // after upgrade. Runs BEFORE root_token auto-gen so the migrated values persist
    // via the same save. The deprecated `sidebar_geometry`/`terminal_geometry` fields
    // are automatically dropped from disk by `skip_serializing_if` on the next save.
    // #1737 (D7c): each of these three migrations writes its destination key from a
    // legacy source on the typed struct AFTER the merge, so an overlay that owns the
    // destination would have its override silently overwritten from the base file.
    // Owning the destination suppresses the migration; with no overlay in force
    // `owns_top_level` is false for every key and the behaviour is unchanged.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        && settings.main_geometry.is_none()
    {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
            log::info!("[settings-migration] seeded main_geometry from legacy terminal_geometry");
        }
    }
    // Seed main_zoom from sidebar_zoom on first boot. EPSILON guard: avoid clobbering
    // a user-set main_zoom=1.0 (which would equal default_zoom) with an effectively-unity
    // sidebar_zoom. See A3.10 / Arb-2.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ZOOM)
        && (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
        log::info!("[settings-migration] seeded main_zoom from legacy sidebar_zoom");
    }
    // Seed main_always_on_top from legacy sidebar_always_on_top.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ALWAYS_ON_TOP)
        && !settings.main_always_on_top
        && settings.sidebar_always_on_top
    {
        settings.main_always_on_top = true;
        log::info!(
            "[settings-migration] seeded main_always_on_top from legacy sidebar_always_on_top"
        );
    }
}

fn persist_loaded_settings_migrations(
    settings: &mut AppSettings,
    path: &Path,
    legacy_profiles_detected: bool,
    legacy_profiles_contents: &Option<String>,
    startup: Option<&StartupAgentRows>,
) {
    let backup_ok = if legacy_profiles_detected {
        match legacy_profiles_contents.as_deref() {
            Some(contents) => match write_pre_384_v1_backup(path, contents) {
                Ok(()) => true,
                Err(e) => {
                    log::error!(
                        "Failed to persist settings v2 migration backup; leaving settings.json \
                            untouched: {}",
                        e
                    );
                    false
                }
            },
            None => true,
        }
    } else {
        true
    };
    if backup_ok {
        // #2306 P1 - the migration write is authorized, so persist explicit
        // contiguous ordinals. Finalize already ran at load; this renumber is
        // the write-side guarantee and cannot re-sort or revive stale values.
        normalize_agent_order(&mut settings.agents);
        // #1077: route the startup root-token/migration write through PRESERVE
        // mode so it cannot erase project companions/conflicts before the
        // first snapshot, and so a present-but-invalid file is never
        // overwritten (the preserve writer's disk gate returns Err). On
        // success adopt the fresh-decoded settings so runtime/hidden agree.
        match save_settings_value_with_startup(settings, path, ProjectWriteMode::Preserve, startup)
        {
            Ok(written) => *settings = written,
            Err(error) => {
                let _ =
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings);
            }
        }
    }
}

fn write_pre_384_v1_backup(settings_path: &Path, contents: &str) -> Result<(), String> {
    let backup_path = settings_path
        .with_file_name(crate::config::instance_artifacts::SETTINGS_MIGRATION_BACKUP_384_V1_NAME);
    if backup_path.exists() {
        return Ok(());
    }
    std::fs::write(&backup_path, contents)
        .map_err(|e| format!("Failed to write {}: {}", backup_path.display(), e))?;
    log::info!(
        "[settings-migration] wrote pre-384 v1 settings backup to {:?}",
        backup_path
    );
    Ok(())
}

/// #2058 - number of previous `settings.json` generations kept beside the live
/// file. Slot 1 is the version the most recent save replaced, slot
/// `SETTINGS_BACKUP_KEEP` the oldest kept; the next rotation drops it.
const SETTINGS_BACKUP_KEEP: u32 = 5;

fn settings_backup_path(settings_path: &Path, index: u32) -> PathBuf {
    settings_path.with_file_name(format!(
        "{SETTINGS_BACKUP_PREFIX}{index}{SETTINGS_BACKUP_SUFFIX}"
    ))
}

/// #2058 - archive `previous` (the bytes a just-completed save replaced) into
/// slot 1, shifting the older slots down and dropping slot
/// `SETTINGS_BACKUP_KEEP`.
///
/// Best effort by design: the caller's save has already succeeded on disk, so a
/// rotation failure is logged and never propagated. Runs with
/// `settings.json.lock` held by the caller, and never touches
/// `settings.pre-*.json`, `settings.local.json`, the lock file or a write
/// temporary. Deliberate local mirror of
/// `sessions_persistence::rotate_orphan_archive`.
fn rotate_settings_backups(settings_path: &Path, previous: &[u8]) {
    rotate_settings_backups_labeled(settings_path, settings_path, previous)
}

fn rotate_settings_backups_labeled(settings_path: &Path, label: &Path, previous: &[u8]) {
    use std::io::Write as _;
    // D8: a non-regular slot is a user-placed symlink or directory; never follow
    // or clobber it.
    for index in 1..=SETTINGS_BACKUP_KEEP {
        let slot = settings_backup_path(settings_path, index);
        match std::fs::symlink_metadata(&slot) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    log::warn!(
                        "[settings] #2058 aborting backup rotation: slot {:?} is not a regular \
                            file",
                        settings_backup_path(label, index)
                    );
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: stat of slot {:?} failed: {}",
                    settings_backup_path(label, index),
                    error
                );
                return;
            }
        }
    }

    // Shift the older slots down; the last rename drops the oldest generation.
    for index in (1..SETTINGS_BACKUP_KEEP).rev() {
        let from = settings_backup_path(settings_path, index);
        let to = settings_backup_path(settings_path, index + 1);
        match std::fs::symlink_metadata(&from) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: stat of slot {:?} failed: {}",
                    settings_backup_path(label, index),
                    error
                );
                return;
            }
        }
        if let Err(error) = std::fs::rename(&from, &to) {
            log::warn!(
                "[settings] #2058 aborting backup rotation: rename {:?} -> {:?} failed: {}",
                settings_backup_path(label, index),
                settings_backup_path(label, index + 1),
                error
            );
            return;
        }
    }

    // Write the just-replaced bytes into slot 1.
    let slot_one = settings_backup_path(settings_path, 1);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    match options.open(&slot_one) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(previous) {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: write to slot {:?} failed: {}",
                    settings_backup_path(label, 1),
                    error
                );
                return;
            }
        }
        Err(error) => {
            log::warn!(
                "[settings] #2058 aborting backup rotation: open of slot {:?} failed: {}",
                settings_backup_path(label, 1),
                error
            );
            return;
        }
    }

    log::info!(
        "[settings] #2058 archived the replaced settings to {:?}",
        settings_backup_path(label, 1)
    );
}

/// CLI-only variant of `load_settings`. Reads disk and applies the same
/// in-memory migrations as `load_settings`, but does NOT auto-generate or
/// persist a `root_token`. Used by CLI verbs that mutate settings
/// (`open-project`, `new-project`) so error paths and pre-validation reads
/// do NOT silently rewrite `settings.json` (Round-1 G5 in #191's plan).
///
/// The CLI verbs do not consume the root_token; if a future verb needs it,
/// `settings.root_token == None` on a brand-new install is fine — the CLI
/// is read-only with respect to it. The GUI still owns root_token
/// generation via the next `load_settings()` call when it boots.
///
/// **Migration duplication is intentional for this PR.** Extracting an
/// `apply_in_memory_migrations(&mut AppSettings)` helper that both loaders
/// share is a clean follow-up, but pulls in scope outside #191 (touches
/// `load_settings`'s control flow). Keep both copies in lockstep until
/// then; if you add a new in-memory migration to `load_settings`, mirror
/// it here too.
pub fn load_settings_for_cli() -> AppSettings {
    match settings_path() {
        Some(path) => load_settings_for_cli_from_path(&path),
        None => {
            log::warn!("[cli] Could not determine home directory, using defaults");
            AppSettings::default()
        }
    }
}

/// #2716 (B3) - `load_settings_for_cli` at an explicit path, so a test can drive
/// the CLI loader over a tempdir.
fn load_settings_for_cli_from_path(path: &Path) -> AppSettings {
    let mut settings = match capture_project_settings(path, false) {
        Ok((settings, _, _)) => settings,
        Err(error) => unavailable_project_settings(AppSettings::default(), error),
    };

    // 0.8.0 unified-window migration — must mirror `load_settings` exactly,
    // EXCEPT for the root_token auto-gen + save_settings call.
    // #1737 (D7c): the same three destination-ownership suppressions as the GUI loader.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        && settings.main_geometry.is_none()
    {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ZOOM)
        && (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ALWAYS_ON_TOP)
        && !settings.main_always_on_top
        && settings.sidebar_always_on_top
    {
        settings.main_always_on_top = true;
    }

    // #248 migration — translate in-memory only. The CLI loader is forbidden
    // from writing settings.json per the §463 contract (load_settings_for_cli
    // is the read-only variant used by mutating verbs like `open-project` and
    // `new-project`; it must not race with the GUI's settings writes). The
    // next GUI launch finalizes the migration to disk via load_settings.
    apply_issue_248_migration(&mut settings);
    adopt_orphaned_profiles(&mut settings);
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);

    // NO root_token auto-gen, NO save_settings call.
    settings
}

/// #786 R1: strict CLI loader for MUTATING paths and `list`/`show`. Identical to
/// `load_settings_for_cli` EXCEPT it distinguishes an ABSENT settings.json (fine:
/// start from default) from a PRESENT-but-unparseable one (Err: refuse to touch
/// it). This closes the silent-wipe hole: `load_settings_for_cli` returns a
/// default on a strict parse error, and a subsequent `save_settings` (which
/// preserves only `project_paths`, via a LENIENT `Value` read) would rewrite
/// settings.json to defaults, destroying every agent/profile/hotkey. NEVER call
/// the silent-default `load_settings_for_cli` for a write.
///
/// Keep the in-memory migration tail in lockstep with `load_settings_for_cli`
/// (see the note above that function).
pub fn load_settings_for_cli_strict() -> Result<AppSettings, String> {
    load_settings_for_cli_strict_from_path(settings_path().as_deref())
}

/// #2716 (B3) - `load_settings_for_cli_strict` at an explicit path, so a test can
/// drive the strict CLI loader over a tempdir.
fn load_settings_for_cli_strict_from_path(path: Option<&Path>) -> Result<AppSettings, String> {
    let mut settings = match path {
        None => AppSettings::default(),
        Some(path) => {
            let (settings, _, _) = capture_project_settings(path, false).map_err(|error| {
                format!(
                    "s\
                ettings source could not be parsed or selected ({error}); refusing to modify it"
                )
            })?;
            if let Some(error) = &settings.project_paths_persistence_error {
                return Err(format!(
                    "settings source could not be parsed or selected ({error}); \
                    refusing to modify it"
                ));
            }
            settings
        }
    };

    // Mirror `load_settings_for_cli`'s in-memory migrations (no disk write).
    // #1737 (D7c): the same three destination-ownership suppressions.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        && settings.main_geometry.is_none()
    {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ZOOM)
        && (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ALWAYS_ON_TOP)
        && !settings.main_always_on_top
        && settings.sidebar_always_on_top
    {
        settings.main_always_on_top = true;
    }
    apply_issue_248_migration(&mut settings);
    adopt_orphaned_profiles(&mut settings);
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);

    Ok(settings)
}

/// One-shot migration for issue #248: translate the legacy
/// `startOnlyCoordinators` field into the new state-sensitive
/// `restore_coordinator_wake_state`. Idempotent — once the legacy carrier is
/// cleared (`.take()`), subsequent calls see `None` and do nothing.
///
/// Translation rules:
///   - legacy `true`  → new `true`  (preserve "smart startup" intent under new semantics).
///   - legacy `false` → new `false` (legacy "wake everything" mode is removed; closest
///     equivalent under the new model is "wake nothing").
///
/// Conflict handling (Grinch Z4): if the user (or a third-party tool) wrote
/// BOTH keys and the new field already differs from the legacy intent, emit a
/// `warn!` and keep the new field's existing value — never silently overwrite
/// a deliberate `restoreCoordinatorWakeState` with a stale legacy value.
fn apply_issue_248_migration(settings: &mut AppSettings) {
    // #1737 (D7c): the overlay owns `restoreCoordinatorWakeState`, so this migration
    // would overwrite the operator's value from the base file's legacy key. Return
    // WITHOUT taking the legacy carrier: dropping it here would delete
    // `startOnlyCoordinators` from the base file while `restore_base` simultaneously
    // restored `restoreCoordinatorWakeState` to its (usually absent) base value, and
    // the migration would be lost rather than deferred. Leaving the carrier in place
    // costs one idempotent save per launch and makes the migration run correctly the
    // first time the overlay is removed. See plan D7c and evidence 2.11b row R4.
    if settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE)
    {
        return;
    }
    if let Some(legacy) = settings.legacy_start_only_coordinators.take() {
        if !settings.restore_coordinator_wake_state {
            settings.restore_coordinator_wake_state = legacy;
            log::info!(
                "[settings-migration] #248 — translated legacy startOnlyCoordinators={} → \
                    restoreCoordinatorWakeState={}",
                legacy,
                settings.restore_coordinator_wake_state
            );
        } else if legacy != settings.restore_coordinator_wake_state {
            log::warn!(
                "[settings-migration] #248 — conflicting state on disk: legacy \
                    startOnlyCoordinators={} but restoreCoordinatorWakeState={} already set; \
                    keeping the new value, dropping legacy",
                legacy,
                settings.restore_coordinator_wake_state
            );
        }
        // else: legacy and new agree → silent drop, no log.
    }
}

/// Read only the `logLevel` field from `settings.json` without triggering migrations,
/// auto-token-gen, or any in-memory mutation. Used by `lib.rs` at logger-init time so
/// the full `load_settings` flow can run post-init with log calls captured.
///
/// Returns `None` on missing file, missing field, malformed JSON, unreadable filesystem,
/// or any other read error — fully read-only and side-effect-free.
/// #1737 (D16): routed through the merged (base plus `settings.local.json`) view.
fn read_log_level_from_path(path: &std::path::Path) -> Option<String> {
    read_merged_top_level_key(path, "logLevel")?
        .as_str()
        .map(String::from)
}

/// See `read_log_level_from_path`. Resolves the canonical settings path and delegates.
pub fn read_log_level_only() -> Option<String> {
    read_log_level_from_path(&settings_path()?)
}

/// #1737 (D16): routed through the merged (base plus `settings.local.json`) view.
fn read_activity_log_enabled_from_path(path: &std::path::Path) -> bool {
    read_merged_top_level_key(path, "activityLogEnabled")
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn read_activity_log_enabled_only() -> bool {
    match settings_path() {
        Some(path) => read_activity_log_enabled_from_path(&path),
        None => false,
    }
}

/// #774: counter feeding the per-call unique temp filename for settings saves.
/// Combined with the PID it makes `settings.json.<pid>.<op_id>.tmp` distinct from
/// any concurrent cross-process save (GUI startup, CLI verbs, closing flush) and
/// from any leftover temp written by a prior crashed run. Mirrors
/// `sessions_persistence::SAVE_OP_ID` (sessions_persistence.rs:47); a separate
/// counter so the two can be reasoned about independently in diagnostics.
static SAVE_OP_ID: AtomicU64 = AtomicU64::new(0);

// ── #1077 six-field project-path codec (JSON extraction / serialization) ────
//
// The three legacy primary fields keep their names/types; each gains a paired
// `…RelativeToInstance` companion (string|null, array-aligned). On load the
// codec resolves both candidates, replaces the runtime fields with the SELECTED
// canonical paths, and stashes the raw pairs/outcomes in the hidden
// `AppSettings.project_path_state`. On write, the preserve/reconcile modes
// re-inject all six fields (companions are `#[serde(skip)]`, so a plain
// AppSettings serialization would otherwise drop them). See plan #1077 §3.2-§4.3.

use super::projects::{
    self, ProjectPathPersistenceState, ProjectSource, RawJsonField, RawPair, RawStringField,
    RepairKind, ResolvedPair, SideOutcome, SideStatus, StructuralIssue,
};

/// An absent-side outcome (used when synthesizing a legacy hidden state).
fn absent_side() -> SideOutcome {
    SideOutcome {
        status: SideStatus::Absent,
        syntactic_path: None,
        canonical_path: None,
        identity: None,
    }
}

const FIELD_PROJECT_PATH: &str = "projectPath";
const FIELD_PROJECT_PATH_REL: &str = "projectPathRelativeToInstance";
const FIELD_PROJECT_PATHS: &str = "projectPaths";
const FIELD_PROJECT_PATHS_REL: &str = "projectPathsRelativeToInstance";
const FIELD_ARCHIVED: &str = "archivedProjectPaths";
const FIELD_ARCHIVED_REL: &str = "archivedProjectPathsRelativeToInstance";
const FIELD_TERMINAL_SNAPSHOTS_ENABLED: &str = "terminalSnapshotsEnabled";
/// #2348 - the two keys owned by `commands::window::set_main_window_placement`.
/// A whole-settings `Preserve` write copies a PRESENT disk value for each key
/// independently, and never removes one that disk does not have.
const FIELD_MAIN_GEOMETRY: &str = "mainGeometry";
const FIELD_MAIN_DISPLAY_STATE: &str = "mainWindowDisplayState";

/// #1737 (D7a) - top-level settings keys whose authority is the on-disk file, not
/// the in-memory struct, and which `settings.local.json` therefore may not override.
pub(crate) const OVERLAY_INELIGIBLE_DISK_KEYS: &[&str] = &[
    "agentFileSourceByMenu",
    FIELD_ARCHIVED,
    FIELD_ARCHIVED_REL,
    FIELD_PROJECT_PATH,
    FIELD_PROJECT_PATH_REL,
    FIELD_PROJECT_PATHS,
    FIELD_PROJECT_PATHS_REL,
    "rootToken",
    FIELD_TERMINAL_SNAPSHOTS_ENABLED,
];

/// #1737 (D7b) - legacy keys that exist only as a one-time migration source. AC
/// translates each into a different key, and one of them is removed from memory by
/// the migration itself, so an override would write a key the restore plan does not
/// own or would resurrect a key the #248 migration exists to delete.
pub(crate) const OVERLAY_INELIGIBLE_LEGACY_KEYS: &[&str] = &[
    "agentFileSourceByMenu",
    "sidebarAlwaysOnTop",
    "sidebarZoom",
    "startOnlyCoordinators",
    "terminalGeometry",
];

/// #1737 (D13) - a repair that creates one object per element id of a source
/// array. When the overlay introduces an id into `source_key` that the base
/// array did not carry, `<derived_prefix>/<id>` joins the restore plan with the
/// base file's value at that path, so a repair driven by an overlay value can
/// never be persisted into the base file. Deliberately narrower than the whole
/// `codingAgentProfiles` key: that key has three other production writers whose
/// payloads the operator did not override. See plan D13 and evidence 2.11.
/// #2450: `identityByAgent` needs its own closure, since a sibling key does not
/// inherit the protection.
pub(crate) const OVERLAY_DERIVED_ID_CLOSURES: &[DerivedIdClosure] = &[
    DerivedIdClosure {
        source_key: "agents",
        id_field: "id",
        derived_prefix: &["codingAgentProfiles", "profilesByAgent"],
    },
    DerivedIdClosure {
        source_key: "agents",
        id_field: "id",
        derived_prefix: &["codingAgentProfiles", "identityByAgent"],
    },
];

/// #1737 (D7c) - migration destination keys. Each names a top-level key that a
/// migration WRITES after the merge, so a migration whose destination the overlay
/// owns would silently overwrite the override in memory. Owning the destination
/// suppresses the migration (plan D7c, evidence 2.11b). Four of the five are
/// written from a legacy source key on the typed struct; `agents` is the
/// exception, rewritten in place by `apply_issue_1757_migration` (#1757 D12).
/// Every suppression names one of these constants; the array is what S29 pins
/// and what a future author greps.
pub(crate) const OVERLAY_KEY_AGENTS: &str = "agents";
pub(crate) const OVERLAY_KEY_MAIN_ALWAYS_ON_TOP: &str = "mainAlwaysOnTop";
pub(crate) const OVERLAY_KEY_MAIN_GEOMETRY: &str = "mainGeometry";
/// #2348 - the placement command refuses while the overlay owns this key or
/// `mainGeometry`, because `restore_base` is the last writer over the base file.
pub(crate) const OVERLAY_KEY_MAIN_DISPLAY_STATE: &str = "mainWindowDisplayState";
pub(crate) const OVERLAY_KEY_MAIN_ZOOM: &str = "mainZoom";
pub(crate) const OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE: &str = "restoreCoordinatorWakeState";
// Read only from test code: the suppressions name the five constants individually.
#[allow(dead_code)]
pub(crate) const OVERLAY_MIGRATION_DESTINATION_KEYS: &[&str] = &[
    OVERLAY_KEY_AGENTS,
    OVERLAY_KEY_MAIN_ALWAYS_ON_TOP,
    OVERLAY_KEY_MAIN_GEOMETRY,
    OVERLAY_KEY_MAIN_ZOOM,
    OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE,
];

/// #1737 (D16) - top-level keys whose value before the project decode differs from
/// the value `parse_settings_json` produces, so `read_merged_top_level_key` may not
/// serve them. The six project keys are rewritten or removed by
/// `apply_project_decode_to_value`. S26 derives this set from that function and fails
/// if it starts writing a key that is not listed.
const OVERLAY_PREMIGRATION_UNSAFE_KEYS: &[&str] = &[
    FIELD_ARCHIVED,
    FIELD_ARCHIVED_REL,
    FIELD_PROJECT_PATH,
    FIELD_PROJECT_PATH_REL,
    FIELD_PROJECT_PATHS,
    FIELD_PROJECT_PATHS_REL,
];

/// #1737 (D22) - the single site in this file that renders overlay diagnostics.
/// `local_overlay` returns typed state and never logs, which is what lets the
/// tests assert the level and the text of every record without installing a
/// global logger. `session_context.rs` has the twin of this three-line match for
/// the Markdown side; those two matches are the only deliberately unasserted
/// code this design leaves.
fn report_overlay_diagnostics(source: &str, overlay: &LocalSettingsOverlay) {
    use crate::config::local_overlay::OverlayDiagnosticLevel;
    for diagnostic in overlay.diagnostics(source) {
        match diagnostic.level() {
            OverlayDiagnosticLevel::Error => log::error!("{}", diagnostic.render()),
            OverlayDiagnosticLevel::Info => log::info!("{}", diagnostic.render()),
        }
    }
}

/// #1737 (D16) - one top-level key of the effective (base plus overlay) settings
/// object, read without migrations, without auto-token-gen and without any save.
/// Deliberately pre-migration AND pre-project-decode, exactly as these readers
/// were before #1737. Valid ONLY for keys outside
/// `OVERLAY_PREMIGRATION_UNSAFE_KEYS`: `apply_project_decode_to_value` rewrites
/// or removes the six project keys, so for those six the value here is not the
/// value `parse_settings_json` produces. Renders no diagnostics: the load path already
/// reports the same records and this runs twice per startup, and
/// `read_log_level_only` additionally runs before the logger exists.
fn read_merged_top_level_key(path: &Path, key: &str) -> Option<Value> {
    debug_assert!(
        !OVERLAY_PREMIGRATION_UNSAFE_KEYS.contains(&key),
        "#1737 D16: {key} is rewritten after the merge; read it through parse_settings_json"
    );
    let mut value = std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let _ = LocalSettingsOverlay::load_and_merge_paired(
        path,
        &mut value,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    value.get(key).cloned()
}

/// The authoritative instance base for path pairing, canonicalized at the codec
/// boundary. `None` in any degraded mode (no base, or a base that fails to
/// canonicalize); never falls back to the process CWD.
fn production_instance_base() -> Option<PathBuf> {
    let base = super::instance_base()?;
    std::fs::canonicalize(&base).ok()
}

/// Presence + JSON value of a raw field (absent vs null vs value).
fn json_field(v: Option<&Value>) -> RawJsonField {
    match v {
        None => RawJsonField {
            present: false,
            value: None,
        },
        Some(Value::Null) => RawJsonField {
            present: true,
            value: None,
        },
        Some(other) => RawJsonField {
            present: true,
            value: Some(other.clone()),
        },
    }
}

/// Extract one plural group (primary + companion arrays) into ordered raw pairs,
/// returning the companion-present bit and any structural corruption. On
/// corruption, no entry is exposed (empty pairs) and the raw values are
/// preserved via the returned `StructuralIssue`.
fn extract_group(
    root: &Map<String, Value>,
    source: ProjectSource,
    primary_key: &str,
    companion_key: &str,
) -> (Vec<RawPair>, bool, Option<StructuralIssue>) {
    let primary = root.get(primary_key);
    let companion = root.get(companion_key);
    let companion_present = companion.is_some();
    let raw_absolute = json_field(primary);
    let raw_relative = json_field(companion);
    let structural = |reason: &str| StructuralIssue {
        source,
        reason: reason.to_string(),
        raw_absolute: raw_absolute.clone(),
        raw_relative: raw_relative.clone(),
    };
    let corrupt = |reason: &str| (Vec::new(), companion_present, Some(structural(reason)));

    // Companion present while primary absent/null → structural corruption.
    if companion_present {
        match primary {
            None => return corrupt("companion present while primary field is absent"),
            Some(Value::Null) => return corrupt("companion present while primary field is null"),
            _ => {}
        }
    }

    // Interpret the primary field.
    let primary_list: Vec<String> = match primary {
        // Absent or null → no active disk truth (a valid empty group here).
        None | Some(Value::Null) => return (Vec::new(), companion_present, None),
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                match el {
                    Value::String(s) => out.push(s.clone()),
                    _ => return corrupt("plural primary contains a non-string element"),
                }
            }
            out
        }
        Some(_) => return corrupt("plural primary is not an array of strings"),
    };

    // Interpret the companion field.
    let companion_list: Option<Vec<RawStringField>> = match companion {
        None => None,
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                match el {
                    Value::String(s) => out.push(RawStringField::string(s.clone())),
                    Value::Null => out.push(RawStringField::null()),
                    _ => return corrupt("plural companion contains a non-string/non-null element"),
                }
            }
            Some(out)
        }
        Some(Value::Null) => return corrupt("plural companion is a present null"),
        Some(_) => return corrupt("plural companion is not an array"),
    };

    if let Some(ref comp) = companion_list {
        if comp.len() != primary_list.len() {
            return corrupt("companion array length differs from primary array");
        }
    }

    let pairs = primary_list
        .into_iter()
        .enumerate()
        .map(|(i, abs)| RawPair {
            source,
            index: Some(i),
            absolute: RawStringField::string(abs),
            relative: companion_list
                .as_ref()
                .map(|c| c[i].clone())
                .unwrap_or_else(RawStringField::absent),
        })
        .collect();

    (pairs, companion_present, None)
}

/// Extract the legacy singular pair, or a structural corruption.
fn extract_singular(root: &Map<String, Value>) -> (Option<RawPair>, Option<StructuralIssue>) {
    let primary = root.get(FIELD_PROJECT_PATH);
    let companion = root.get(FIELD_PROJECT_PATH_REL);
    let raw_absolute = json_field(primary);
    let raw_relative = json_field(companion);
    let structural = |reason: &str| StructuralIssue {
        source: ProjectSource::ProjectPath,
        reason: reason.to_string(),
        raw_absolute: raw_absolute.clone(),
        raw_relative: raw_relative.clone(),
    };

    let primary_field = match primary {
        None => RawStringField::absent(),
        Some(Value::Null) => RawStringField::null(),
        Some(Value::String(s)) => RawStringField::string(s.clone()),
        Some(_) => {
            return (
                None,
                Some(structural("projectPath is not a string or null")),
            )
        }
    };
    let companion_field = match companion {
        None => RawStringField::absent(),
        Some(Value::Null) => RawStringField::null(),
        Some(Value::String(s)) => RawStringField::string(s.clone()),
        Some(_) => {
            return (
                None,
                Some(structural(
                    "projectPathRelativeToInstance is not a string or null",
                )),
            )
        }
    };
    if companion.is_some() && primary.is_none() {
        return (
            None,
            Some(structural(
                "singular companion present while projectPath is absent",
            )),
        );
    }
    if matches!(companion, Some(Value::String(_))) && matches!(primary, Some(Value::Null)) {
        return (
            None,
            Some(structural(
                "non-null singular companion paired with null projectPath",
            )),
        );
    }

    // A pair exists only when the primary is an actual string candidate; a
    // null/absent primary is the valid empty singular.
    let has_primary = primary_field.value.is_some();
    let pair = has_primary.then_some(RawPair {
        source: ProjectSource::ProjectPath,
        index: None,
        absolute: primary_field,
        relative: companion_field,
    });
    (pair, None)
}

/// Decode the six raw project fields from a settings object into the hidden
/// persistence state (selected paths, raw pairs, outcomes, structural issues).
pub(crate) fn decode_project_state(
    root: &Map<String, Value>,
    base: Option<&Path>,
    resolver: &dyn projects::CandidateResolver,
) -> ProjectPathPersistenceState {
    let (active_pairs, active_companion, active_structural) = extract_group(
        root,
        ProjectSource::ProjectPaths,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
    );
    let (archived_pairs, archived_companion, archived_structural) = extract_group(
        root,
        ProjectSource::ArchivedProjectPaths,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    );
    let (singular, singular_structural) = extract_singular(root);

    // A structurally-corrupt active group exposes no active entry at all,
    // including its singular mirror.
    let effective_singular = if active_structural.is_some() {
        None
    } else {
        singular
    };

    let mut state = projects::resolve_registrations(
        &active_pairs,
        effective_singular,
        &archived_pairs,
        base,
        active_companion,
        archived_companion,
        resolver,
    );

    let mut structural = Vec::new();
    structural.extend(active_structural);
    structural.extend(archived_structural);
    structural.extend(singular_structural);
    if !structural.is_empty() {
        // Any structural corruption blocks all reconciliation/mutation.
        state.active_reconcile_eligible = false;
        state.archived_reconcile_eligible = false;
    }
    state.structural_issues = structural;
    state
}

/// Decode the six fields from `value`, then overwrite the three primary runtime
/// fields with the selected values and drop the companion keys so the remainder
/// of `AppSettings` deserializes cleanly even when a project field was
/// wrong-typed. Returns the hidden state to attach.
pub(crate) fn apply_project_decode_to_value(
    value: &mut Value,
    base: Option<&Path>,
    resolver: &dyn projects::CandidateResolver,
) -> ProjectPathPersistenceState {
    let state = match value.as_object() {
        Some(root) => decode_project_state(root, base, resolver),
        None => ProjectPathPersistenceState::default(),
    };
    if let Some(root) = value.as_object_mut() {
        root.insert(
            FIELD_PROJECT_PATH.to_string(),
            state
                .selected_head
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        root.insert(
            FIELD_PROJECT_PATHS.to_string(),
            Value::Array(
                state
                    .active_selected()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        root.insert(
            FIELD_ARCHIVED.to_string(),
            Value::Array(
                state
                    .archived_management_paths()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        // Companion keys are not AppSettings fields; strip so nothing lingers.
        root.remove(FIELD_PROJECT_PATH_REL);
        root.remove(FIELD_PROJECT_PATHS_REL);
        root.remove(FIELD_ARCHIVED_REL);
    }
    state
}

/// The project-field write mode. `Preserve` copies the six raw fields from the
/// fresh disk object (materializing a group only when disk has no truth for it);
/// `Reconcile` rebuilds the requested eligible group(s) from hidden state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectWriteMode {
    Preserve,
    #[cfg(test)]
    Reconcile {
        active: bool,
        archived: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalSnapshotGateWriteMode {
    Preserve,
    Explicit(bool),
}

/// #2348 - the main-window placement write mode, parallel to
/// `TerminalSnapshotGateWriteMode`. `Preserve` is for every whole-settings
/// caller: a present disk value wins per key, while an absent one does not erase
/// the caller value (the load-time `terminalGeometry` -> `mainGeometry`
/// migration synthesizes a value the disk object may not have). `Explicit` is
/// used only by the narrow placement command and writes both keys from its own
/// arguments.
#[derive(Debug, Clone)]
enum MainWindowPlacementWriteMode {
    Preserve,
    Explicit {
        geometry: WindowGeometry,
        display_state: MainWindowDisplayState,
    },
}

/// A slot value for a retained (unresolved/conflict/missing) raw field: string
/// stays a string, absent/null becomes null (inside an array there is no absent).
fn raw_slot_value(field: &RawStringField) -> Value {
    match &field.value {
        Some(s) => Value::String(s.clone()),
        None => Value::Null,
    }
}

/// The companion wire value for a selected pair: encode against the base when
/// available; otherwise preserve any existing raw companion (or null).
fn companion_slot_value(pair: &ResolvedPair, base: Option<&Path>) -> Value {
    match base {
        Some(base) => match &pair.selected_canonical_raw {
            Some(canon) => match projects::encode_instance_relative(Path::new(canon), base) {
                Some(wire) => Value::String(wire),
                None => Value::Null,
            },
            None => raw_slot_value(&pair.raw_relative),
        },
        None => raw_slot_value(&pair.raw_relative),
    }
}

/// Rebuild one group's (primary array, companion array) from its resolved pairs:
/// emit selected pairs (repaired), retain unresolved pairs value-for-value, and
/// drop silent duplicates.
fn rebuild_group_arrays(pairs: &[ResolvedPair], base: Option<&Path>) -> (Vec<Value>, Vec<Value>) {
    let mut primary = Vec::new();
    let mut companion = Vec::new();
    for pair in pairs {
        if let Some(sel) = &pair.selected {
            primary.push(Value::String(sel.clone()));
            companion.push(companion_slot_value(pair, base));
        } else if pair.issue.is_some() {
            primary.push(raw_slot_value(&pair.raw_absolute));
            companion.push(raw_slot_value(&pair.raw_relative));
        }
        // else: silent dedupe drop — emit nothing.
    }
    (primary, companion)
}

/// Insert the four active fields from hidden state.
fn write_active_group(
    out: &mut Map<String, Value>,
    state: &ProjectPathPersistenceState,
    base: Option<&Path>,
) {
    let (primary, companion) = rebuild_group_arrays(state.active_pairs(), base);
    out.insert(
        FIELD_PROJECT_PATH.to_string(),
        primary.first().cloned().unwrap_or(Value::Null),
    );
    out.insert(
        FIELD_PROJECT_PATH_REL.to_string(),
        companion.first().cloned().unwrap_or(Value::Null),
    );
    out.insert(FIELD_PROJECT_PATHS.to_string(), Value::Array(primary));
    out.insert(FIELD_PROJECT_PATHS_REL.to_string(), Value::Array(companion));
}

/// Insert the two archived fields from hidden state.
fn write_archived_group(
    out: &mut Map<String, Value>,
    state: &ProjectPathPersistenceState,
    base: Option<&Path>,
) {
    let (primary, companion) = rebuild_group_arrays(state.archived_pairs(), base);
    out.insert(FIELD_ARCHIVED.to_string(), Value::Array(primary));
    out.insert(FIELD_ARCHIVED_REL.to_string(), Value::Array(companion));
}

/// If `AppSettings` was constructed directly (empty hidden state) yet carries
/// runtime project lists, synthesize a legacy absolute-only state so the writers
/// behave like a legacy-decoded file (companions null, all entries selected).
fn hidden_state_for_write(
    settings: &AppSettings,
) -> std::borrow::Cow<'_, ProjectPathPersistenceState> {
    let state = &settings.project_path_state;
    let empty = state.pairs.is_empty() && state.structural_issues.is_empty();
    let runtime_nonempty = settings.project_path.is_some()
        || !settings.project_paths.is_empty()
        || !settings.archived_project_paths.is_empty();
    if empty && runtime_nonempty {
        std::borrow::Cow::Owned(synthesize_legacy_state(settings))
    } else {
        std::borrow::Cow::Borrowed(state)
    }
}

fn legacy_selected_pair(source: ProjectSource, index: Option<usize>, path: &str) -> ResolvedPair {
    ResolvedPair {
        source,
        index,
        raw_absolute: RawStringField::string(path),
        raw_relative: RawStringField::absent(),
        absolute_side: absent_side(),
        relative_side: absent_side(),
        selected: Some(path.to_string()),
        selected_canonical_raw: None,
        selected_identity: None,
        issue: None,
        repair: RepairKind::None,
    }
}

fn synthesize_legacy_state(settings: &AppSettings) -> ProjectPathPersistenceState {
    let mut pairs: Vec<ResolvedPair> = settings
        .project_paths
        .iter()
        .enumerate()
        .map(|(i, p)| legacy_selected_pair(ProjectSource::ProjectPaths, Some(i), p))
        .collect();
    let active_registration_count = pairs.len();
    for (i, p) in settings.archived_project_paths.iter().enumerate() {
        pairs.push(legacy_selected_pair(
            ProjectSource::ArchivedProjectPaths,
            Some(i),
            p,
        ));
    }
    ProjectPathPersistenceState {
        pairs,
        selected_head: settings.project_paths.first().cloned(),
        active_registration_count,
        archived_registration_count: settings.archived_project_paths.len(),
        active_companion_present: false,
        archived_companion_present: false,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: Vec::new(),
        runtime_authoritative: true,
    }
}

/// A selected pair for a runtime path, canonicalizing so the reconcile write can
/// encode a real instance-relative companion (a non-existent path yields a null
/// companion).
fn resynced_selected_pair(source: ProjectSource, index: usize, path: &str) -> ResolvedPair {
    let canonical_raw = std::fs::canonicalize(path)
        .ok()
        .and_then(|c| c.to_str().map(str::to_string));
    ResolvedPair {
        source,
        index: Some(index),
        raw_absolute: RawStringField::string(path),
        raw_relative: RawStringField::absent(),
        absolute_side: absent_side(),
        relative_side: absent_side(),
        selected: Some(path.to_string()),
        selected_canonical_raw: canonical_raw,
        selected_identity: None,
        issue: None,
        repair: RepairKind::PopulateCompanion,
    }
}

/// #1077: after an explicit project mutation updated the three runtime lists,
/// rebuild the hidden state to match while retaining any unresolved
/// (conflict/missing/invalid) pairs from the pre-mutation state value-for-value,
/// so a register/remove/archive/unarchive never drops a preserved conflict or
/// missing record. The result is runtime-authoritative: a Reconcile write emits
/// the runtime selection plus the retained raw records. Callers must reject a
/// structurally-corrupt or conflict-scoped mutation BEFORE calling this.
pub(crate) fn resync_project_state_from_runtime(settings: &mut AppSettings) {
    let old = settings.project_path_state.clone();
    let retained_active: Vec<ResolvedPair> = old
        .active_pairs()
        .iter()
        .filter(|p| p.issue.is_some())
        .cloned()
        .collect();
    let retained_archived: Vec<ResolvedPair> = old
        .archived_pairs()
        .iter()
        .filter(|p| p.issue.is_some())
        .cloned()
        .collect();

    let active = reconcile_group_with_retained(
        &settings.project_paths,
        ProjectSource::ProjectPaths,
        retained_active,
    );
    let active_registration_count = active.len();
    let archived = reconcile_group_with_retained(
        &settings.archived_project_paths,
        ProjectSource::ArchivedProjectPaths,
        retained_archived,
    );
    let archived_registration_count = archived.len();

    let mut pairs = active;
    pairs.extend(archived);

    settings.project_path_state = Arc::new(ProjectPathPersistenceState {
        pairs,
        selected_head: settings.project_paths.first().cloned(),
        active_registration_count,
        archived_registration_count,
        active_companion_present: old.active_companion_present,
        archived_companion_present: old.archived_companion_present,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: old.structural_issues.clone(),
        runtime_authoritative: true,
    });
}

/// Rebuild one group's pairs from the mutated `runtime` list while reconciling
/// the pre-mutation `retained` unresolved (missing/conflict/invalid) records:
///
/// - a runtime entry that matches a retained record by normalized key reuses
///   that record IN PLACE (preserving its raw values, companion, and issue), so
///   a preserved conflict/missing record is never rebuilt as "selected" nor
///   duplicated;
/// - a runtime entry with no retained match becomes a fresh selected pair;
/// - a retained record whose key is absent from the runtime list was removed by
///   the mutation and is dropped (so "Remove from list" actually removes it).
fn reconcile_group_with_retained(
    runtime: &[String],
    source: ProjectSource,
    mut retained: Vec<ResolvedPair>,
) -> Vec<ResolvedPair> {
    let mut out = Vec::with_capacity(runtime.len());
    for (i, path) in runtime.iter().enumerate() {
        let key = projects::normalize_for_compare(path);
        let matched = retained.iter().position(|r| {
            r.raw_absolute
                .value
                .as_deref()
                .map(projects::normalize_for_compare)
                == Some(key.clone())
        });
        match matched {
            Some(pos) => {
                let mut pair = retained.remove(pos);
                pair.source = source;
                pair.index = Some(i);
                out.push(pair);
            }
            None => out.push(resynced_selected_pair(source, i, path)),
        }
    }
    // Any retained record not matched by the runtime list was removed; drop it.
    out
}

/// Fresh-read the whole disk object for a project-preserving/reconciling write.
/// `None` = absent (materialize). `Err` = present-but-unreadable/invalid JSON,
/// non-object root, or non-project settings that fail to deserialize — in which
const SETTINGS_SAVE_FAILED_CODE: &str = "settings_save_failed";
const SETTINGS_LOCK_UNAVAILABLE_CODE: &str = "settings_lock_unavailable";
const TERMINAL_SNAPSHOT_SETTING_SAVE_FAILED_CODE: &str = "terminal_snapshot_setting_save_failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsSaveReportSurface {
    GeneralSettings,
    TerminalSnapshotCompareAndSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsSaveStage {
    LockAcquire,
    PreserveDiskGate,
    ReconcileDiskGate,
    ProjectPathsRefreshDiskGate,
    Serialize,
    PrepareTarget,
    TempCreate,
    TempWrite,
    TempFlush,
    TempSync,
    TempVerify,
    AtomicReplace,
    #[cfg(unix)]
    TargetPermissions,
    #[cfg(unix)]
    ParentDirectorySync,
    PostWriteRead,
    PostWriteVerify,
    ReDecode,
}

impl SettingsSaveStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::LockAcquire => "lock_acquire",
            Self::PreserveDiskGate => "preserve_disk_gate",
            Self::ReconcileDiskGate => "reconcile_disk_gate",
            Self::ProjectPathsRefreshDiskGate => "project_paths_refresh_disk_gate",
            Self::Serialize => "serialize",
            Self::PrepareTarget => "prepare_target",
            Self::TempCreate => "temp_create",
            Self::TempWrite => "temp_write",
            Self::TempFlush => "temp_flush",
            Self::TempSync => "temp_sync",
            Self::TempVerify => "temp_verify",
            Self::AtomicReplace => "atomic_replace",
            #[cfg(unix)]
            Self::TargetPermissions => "target_permissions",
            #[cfg(unix)]
            Self::ParentDirectorySync => "parent_directory_sync",
            Self::PostWriteRead => "post_write_read",
            Self::PostWriteVerify => "post_write_verify",
            Self::ReDecode => "re_decode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsSaveReason {
    ProjectPathsConflict,
    ProjectPathsBackupMismatch,
    ProjectPathsSourceRejected,
    LockTimedOut,
    LockDeadlineOverflow,
    LockFileNotRegular,
    #[cfg(unix)]
    LockLinkCountRejected,
    #[cfg(not(any(unix, windows)))]
    LockPlatformUnsupported,
    LockIdentityRejected,
    MissingParentDirectory,
    DiskJsonNotObject,
    DiskSettingsValidationRejected,
    SerializedJsonNotObject,
    TargetNotRegularFile,
    TargetIdentityRejected,
    TerminalSnapshotSettingInvalid,
    SerializedPayloadTooLarge,
    TempIdentityRejected,
    PostWriteReadRejected,
    WrittenBytesMismatch,
}

impl SettingsSaveReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProjectPathsConflict => "project_paths_conflict",
            Self::ProjectPathsBackupMismatch => "project_paths_backup_mismatch",
            Self::ProjectPathsSourceRejected => "project_paths_source_rejected",
            Self::LockTimedOut => "lock_timed_out",
            Self::LockDeadlineOverflow => "lock_deadline_overflow",
            Self::LockFileNotRegular => "lock_file_not_regular",
            #[cfg(unix)]
            Self::LockLinkCountRejected => "lock_link_count_rejected",
            #[cfg(not(any(unix, windows)))]
            Self::LockPlatformUnsupported => "lock_platform_unsupported",
            Self::LockIdentityRejected => "lock_identity_rejected",
            Self::MissingParentDirectory => "missing_parent_directory",
            Self::DiskJsonNotObject => "disk_json_not_object",
            Self::DiskSettingsValidationRejected => "disk_settings_validation_rejected",
            Self::SerializedJsonNotObject => "serialized_json_not_object",
            Self::TargetNotRegularFile => "target_not_regular_file",
            Self::TargetIdentityRejected => "target_identity_rejected",
            Self::TerminalSnapshotSettingInvalid => "terminal_snapshot_setting_invalid",
            Self::SerializedPayloadTooLarge => "serialized_payload_too_large",
            Self::TempIdentityRejected => "temp_identity_rejected",
            Self::PostWriteReadRejected => "post_write_read_rejected",
            Self::WrittenBytesMismatch => "written_bytes_mismatch",
        }
    }
}

enum SettingsSaveCause {
    Io(std::io::Error),
    Json(serde_json::Error),
    Semantic(SettingsSaveReason),
}

enum SettingsSaveLegacyOutward {
    SettingsLockUnavailable,
    SettingsSaveFailed,
    DiskRead(String),
    DiskJson(String),
    DiskJsonNotObject(String),
    DiskValidation(String),
    Serialize(String),
    SerializedJsonNotObject(String),
    TerminalSnapshotSettingInvalid(String),
    ReDecode(String),
}

impl SettingsSaveLegacyOutward {
    fn into_general_string(self) -> String {
        match self {
            Self::SettingsLockUnavailable => SETTINGS_LOCK_UNAVAILABLE_CODE.to_string(),
            Self::SettingsSaveFailed => SETTINGS_SAVE_FAILED_CODE.to_string(),
            Self::DiskRead(value)
            | Self::DiskJson(value)
            | Self::DiskJsonNotObject(value)
            | Self::DiskValidation(value)
            | Self::Serialize(value)
            | Self::SerializedJsonNotObject(value)
            | Self::TerminalSnapshotSettingInvalid(value)
            | Self::ReDecode(value) => value,
        }
    }
}

struct SettingsSaveError {
    stage: SettingsSaveStage,
    target_path: Box<PathBuf>,
    temp_path: Option<Box<PathBuf>>,
    pid: u32,
    display_target: Option<Box<PathBuf>>,
    display_temp: Option<Box<PathBuf>>,
    cause: SettingsSaveCause,
    legacy_outward: SettingsSaveLegacyOutward,
}

impl SettingsSaveError {
    fn io(
        stage: SettingsSaveStage,
        target_path: &Path,
        temp_path: Option<&Path>,
        source: std::io::Error,
        legacy_outward: SettingsSaveLegacyOutward,
    ) -> Self {
        Self {
            stage,
            target_path: Box::new(target_path.to_path_buf()),
            temp_path: temp_path.map(|path| Box::new(path.to_path_buf())),
            pid: std::process::id(),
            display_target: None,
            display_temp: None,
            cause: SettingsSaveCause::Io(source),
            legacy_outward,
        }
    }

    fn json(
        stage: SettingsSaveStage,
        target_path: &Path,
        temp_path: Option<&Path>,
        source: serde_json::Error,
        legacy_outward: SettingsSaveLegacyOutward,
    ) -> Self {
        Self {
            stage,
            target_path: Box::new(target_path.to_path_buf()),
            temp_path: temp_path.map(|path| Box::new(path.to_path_buf())),
            pid: std::process::id(),
            display_target: None,
            display_temp: None,
            cause: SettingsSaveCause::Json(source),
            legacy_outward,
        }
    }

    fn semantic(
        stage: SettingsSaveStage,
        target_path: &Path,
        temp_path: Option<&Path>,
        reason: SettingsSaveReason,
        legacy_outward: SettingsSaveLegacyOutward,
    ) -> Self {
        Self {
            stage,
            target_path: Box::new(target_path.to_path_buf()),
            temp_path: temp_path.map(|path| Box::new(path.to_path_buf())),
            pid: std::process::id(),
            display_target: None,
            display_temp: None,
            cause: SettingsSaveCause::Semantic(reason),
            legacy_outward,
        }
    }

    fn with_capture_labels(mut self, settings_label: &Path) -> Self {
        self.display_target = self
            .target_path
            .file_name()
            .map(|name| Box::new(settings_label.with_file_name(name)));
        self.display_temp = self.temp_path.as_deref().and_then(|physical| {
            physical
                .file_name()
                .map(|name| Box::new(settings_label.with_file_name(name)))
        });
        self
    }

    fn general_settings_diagnostic_code(&self) -> &'static str {
        match self.stage {
            SettingsSaveStage::LockAcquire => SETTINGS_LOCK_UNAVAILABLE_CODE,
            _ => SETTINGS_SAVE_FAILED_CODE,
        }
    }

    fn safe_diagnostic(&self, surface: SettingsSaveReportSurface) -> SettingsSaveDiagnostic {
        let (source_kind, reason, io_kind, raw_os_error, json_category, json_line, json_column) =
            match &self.cause {
                SettingsSaveCause::Io(source) => (
                    "io",
                    "none",
                    Some(io_error_kind_as_str(source.kind())),
                    source.raw_os_error(),
                    None,
                    None,
                    None,
                ),
                SettingsSaveCause::Json(source) => (
                    "json",
                    "none",
                    None,
                    None,
                    Some(json_error_category_as_str(source.classify())),
                    Some(source.line()),
                    Some(source.column()),
                ),
                SettingsSaveCause::Semantic(reason) => {
                    ("semantic", reason.as_str(), None, None, None, None, None)
                }
            };

        SettingsSaveDiagnostic {
            code: surface.diagnostic_code(self),
            stage: self.stage.as_str(),
            reason,
            pid: self.pid,
            target_path: render_diagnostic_path(
                self.display_target.as_deref().unwrap_or(&self.target_path),
            ),
            temp_path: self
                .display_temp
                .as_deref()
                .or(self.temp_path.as_deref())
                .map(|path| render_diagnostic_path(path)),
            source_kind,
            io_kind,
            raw_os_error,
            json_category,
            json_line,
            json_column,
        }
    }
}

impl std::fmt::Debug for SettingsSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let diagnostic = self.safe_diagnostic(SettingsSaveReportSurface::GeneralSettings);
        formatter
            .debug_struct("SettingsSaveError")
            .field("code", &diagnostic.code)
            .field("stage", &diagnostic.stage)
            .field("reason", &diagnostic.reason)
            .field("pid", &diagnostic.pid)
            .field("source_kind", &diagnostic.source_kind)
            .field("io_kind", &diagnostic.io_kind)
            .field("raw_os_error", &diagnostic.raw_os_error)
            .field("json_category", &diagnostic.json_category)
            .field("json_line", &diagnostic.json_line)
            .field("json_column", &diagnostic.json_column)
            .finish()
    }
}

impl std::fmt::Display for SettingsSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.general_settings_diagnostic_code())
    }
}

impl std::error::Error for SettingsSaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            SettingsSaveCause::Io(source) => Some(source),
            SettingsSaveCause::Json(source) => Some(source),
            SettingsSaveCause::Semantic(_) => None,
        }
    }
}

impl SettingsSaveReportSurface {
    fn diagnostic_code(self, error: &SettingsSaveError) -> &'static str {
        match self {
            Self::GeneralSettings => error.general_settings_diagnostic_code(),
            Self::TerminalSnapshotCompareAndSet => TERMINAL_SNAPSHOT_SETTING_SAVE_FAILED_CODE,
        }
    }

    fn into_outward_string(self, error: SettingsSaveError) -> String {
        match self {
            Self::GeneralSettings => error.legacy_outward.into_general_string(),
            Self::TerminalSnapshotCompareAndSet => {
                drop(error);
                TERMINAL_SNAPSHOT_SETTING_SAVE_FAILED_CODE.to_string()
            }
        }
    }
}

struct SettingsSaveDiagnostic {
    code: &'static str,
    stage: &'static str,
    reason: &'static str,
    pid: u32,
    target_path: String,
    temp_path: Option<String>,
    source_kind: &'static str,
    io_kind: Option<&'static str>,
    raw_os_error: Option<i32>,
    json_category: Option<&'static str>,
    json_line: Option<usize>,
    json_column: Option<usize>,
}

fn io_error_kind_as_str(kind: std::io::ErrorKind) -> &'static str {
    match kind {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::ConnectionRefused => "connection_refused",
        std::io::ErrorKind::ConnectionReset => "connection_reset",
        std::io::ErrorKind::HostUnreachable => "host_unreachable",
        std::io::ErrorKind::NetworkUnreachable => "network_unreachable",
        std::io::ErrorKind::ConnectionAborted => "connection_aborted",
        std::io::ErrorKind::NotConnected => "not_connected",
        std::io::ErrorKind::AddrInUse => "addr_in_use",
        std::io::ErrorKind::AddrNotAvailable => "addr_not_available",
        std::io::ErrorKind::NetworkDown => "network_down",
        std::io::ErrorKind::BrokenPipe => "broken_pipe",
        std::io::ErrorKind::AlreadyExists => "already_exists",
        std::io::ErrorKind::WouldBlock => "would_block",
        std::io::ErrorKind::NotADirectory => "not_a_directory",
        std::io::ErrorKind::IsADirectory => "is_a_directory",
        std::io::ErrorKind::DirectoryNotEmpty => "directory_not_empty",
        std::io::ErrorKind::ReadOnlyFilesystem => "read_only_filesystem",
        std::io::ErrorKind::StaleNetworkFileHandle => "stale_network_file_handle",
        std::io::ErrorKind::InvalidInput => "invalid_input",
        std::io::ErrorKind::InvalidData => "invalid_data",
        std::io::ErrorKind::TimedOut => "timed_out",
        std::io::ErrorKind::WriteZero => "write_zero",
        std::io::ErrorKind::StorageFull => "storage_full",
        std::io::ErrorKind::NotSeekable => "not_seekable",
        std::io::ErrorKind::QuotaExceeded => "quota_exceeded",
        std::io::ErrorKind::FileTooLarge => "file_too_large",
        std::io::ErrorKind::ResourceBusy => "resource_busy",
        std::io::ErrorKind::ExecutableFileBusy => "executable_file_busy",
        std::io::ErrorKind::Deadlock => "deadlock",
        std::io::ErrorKind::CrossesDevices => "crosses_devices",
        std::io::ErrorKind::TooManyLinks => "too_many_links",
        std::io::ErrorKind::InvalidFilename => "invalid_filename",
        std::io::ErrorKind::ArgumentListTooLong => "argument_list_too_long",
        std::io::ErrorKind::Interrupted => "interrupted",
        std::io::ErrorKind::Unsupported => "unsupported",
        std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
        std::io::ErrorKind::OutOfMemory => "out_of_memory",
        std::io::ErrorKind::Other => "other",
        _ => "other",
    }
}

fn json_error_category_as_str(category: serde_json::error::Category) -> &'static str {
    match category {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    }
}

fn render_diagnostic_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    let mut rendered = String::with_capacity(path.len() + 2);
    rendered.push('"');
    for character in path.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/' | ':') {
            rendered.push(character);
        } else {
            rendered.extend(character.escape_unicode());
        }
    }
    rendered.push('"');
    rendered
}

fn render_settings_save_diagnostic(diagnostic: &SettingsSaveDiagnostic) -> String {
    let raw_os_error = diagnostic
        .raw_os_error
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    let json_line = diagnostic
        .json_line
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    let json_column = diagnostic
        .json_column
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    format!(
        "settings_save_failure code={} stage={} reason={} pid={} target_path={} temp_path={} \
            source_kind={} io_kind={} raw_os_error={} json_category={} json_line={} json_column={}",
        diagnostic.code,
        diagnostic.stage,
        diagnostic.reason,
        diagnostic.pid,
        diagnostic.target_path,
        diagnostic.temp_path.as_deref().unwrap_or("none"),
        diagnostic.source_kind,
        diagnostic.io_kind.unwrap_or("none"),
        raw_os_error,
        diagnostic.json_category.unwrap_or("none"),
        json_line,
        json_column,
    )
}

#[cfg(test)]
type SettingsSaveDiagnosticCapture = Box<dyn Fn(&str)>;

#[cfg(test)]
thread_local! {
    static SETTINGS_SAVE_DIAGNOSTIC_CAPTURE: std::cell::RefCell<
        Option<SettingsSaveDiagnosticCapture>,
    > = std::cell::RefCell::new(None);
}

#[cfg(test)]
fn capture_settings_save_diagnostic(payload: &str) {
    SETTINGS_SAVE_DIAGNOSTIC_CAPTURE.with(|slot| {
        if let Some(capture) = slot.borrow().as_ref() {
            capture(payload);
        }
    });
}

fn report_settings_save_error(
    error: SettingsSaveError,
    surface: SettingsSaveReportSurface,
) -> String {
    let payload = render_settings_save_diagnostic(&error.safe_diagnostic(surface));
    #[cfg(test)]
    capture_settings_save_diagnostic(&payload);
    log::error!("{}", payload);
    surface.into_outward_string(error)
}

fn read_disk_object_for_write_typed(
    path: &Path,
    stage: SettingsSaveStage,
) -> Result<Option<Map<String, Value>>, SettingsSaveError> {
    read_disk_object_and_contents_for_write_typed(path, stage).map(|disk| disk.map(|(map, _)| map))
}

/// Same read and gates as `read_disk_object_for_write_typed`, plus the raw file
/// contents, so a Preserve save can back up a legacy-shaped file verbatim.
#[allow(clippy::type_complexity)]
fn read_disk_object_and_contents_for_write_typed(
    path: &Path,
    stage: SettingsSaveStage,
) -> Result<Option<(Map<String, Value>, String)>, SettingsSaveError> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => {
            let outward = format!(
                "Failed to read {} for a project-preserving save (aborting to avoid dropping a \
                    project): {source}",
                path.display()
            );
            Err(SettingsSaveError::io(
                stage,
                path,
                None,
                source,
                SettingsSaveLegacyOutward::DiskRead(outward),
            ))
        }
        Ok(contents) => {
            let value: Value = serde_json::from_str(&contents).map_err(|source| {
                let outward = format!(
                    "Refusing to overwrite {}: the existing settings file is not valid JSON \
                        ({source})",
                    path.display()
                );
                SettingsSaveError::json(
                    stage,
                    path,
                    None,
                    source,
                    SettingsSaveLegacyOutward::DiskJson(outward),
                )
            })?;
            match value {
                Value::Object(map) => {
                    if let Err(outward) = validate_non_project_settings(&map) {
                        return Err(SettingsSaveError::semantic(
                            stage,
                            path,
                            None,
                            SettingsSaveReason::DiskSettingsValidationRejected,
                            SettingsSaveLegacyOutward::DiskValidation(outward),
                        ));
                    }
                    Ok(Some((map, contents)))
                }
                _ => {
                    let outward = format!(
                        "Refusing to overwrite {}: the existing settings root is not a JSON object",
                        path.display()
                    );
                    Err(SettingsSaveError::semantic(
                        stage,
                        path,
                        None,
                        SettingsSaveReason::DiskJsonNotObject,
                        SettingsSaveLegacyOutward::DiskJsonNotObject(outward),
                    ))
                }
            }
        }
    }
}

/// Confirm the non-project part of a disk object still deserializes as
/// `AppSettings` (after neutralizing the six project fields), so a present file
/// with corrupt non-project settings is never silently overwritten.
fn validate_non_project_settings(disk: &Map<String, Value>) -> Result<(), String> {
    let mut probe = Value::Object(disk.clone());
    if let Some(root) = probe.as_object_mut() {
        root.insert(FIELD_PROJECT_PATH.to_string(), Value::Null);
        root.insert(FIELD_PROJECT_PATHS.to_string(), Value::Array(Vec::new()));
        root.insert(FIELD_ARCHIVED.to_string(), Value::Array(Vec::new()));
        root.remove(FIELD_PROJECT_PATH_REL);
        root.remove(FIELD_PROJECT_PATHS_REL);
        root.remove(FIELD_ARCHIVED_REL);
        // #2716 (B3) trap four: `agents` lives in the agents file once migrated.
        root.entry(AGENTS_INSTANCE_KEYS[0])
            .or_insert_with(|| Value::Array(Vec::new()));
    }
    serde_json::from_value::<AppSettings>(registered_profiles_for_read(&probe))
        .map(|_| ())
        .map_err(|e| {
            format!(
                "Refusing to overwrite present settings whose non-project fields are invalid: {e}"
            )
        })
}

/// #1077 automatic reconciliation boundary (§4.3): reconcile the requested
/// eligible project group(s) from `settings`' hidden state to `path`, returning
/// the fresh-decoded settings. A structurally-corrupt or no-eligible-repair
/// state performs no write and returns the settings unchanged.
#[cfg(test)]
pub(crate) fn reconcile_project_state_to_path(
    settings: &AppSettings,
    path: &Path,
    active: bool,
    archived: bool,
) -> Result<AppSettings, String> {
    let mut transaction = begin_project_paths_transaction_at_path(settings, path)?;
    let written = if project_state_has_structural(settings) {
        transaction.fresh_settings().clone()
    } else {
        transaction.commit_project_groups(settings, active, archived)?
    };
    // Snapshot fixture helper only: preserve historical unrelated fixture seeding.
    // Production project operations call the token's sidecar-only commit directly.
    save_settings_value_locked(
        &written,
        path,
        ProjectWriteMode::Reconcile { active, archived },
        TerminalSnapshotGateWriteMode::Preserve,
        MainWindowPlacementWriteMode::Preserve,
    )
    .map_err(|error| report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings))
}

// Sole project registration authority; callers hold the shared settings lock.
fn project_paths_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(super::instance_artifacts::PROJECT_PATHS_FILE_NAME)
}

fn raw_project_projection(object: &Map<String, Value>) -> Map<String, Value> {
    [
        FIELD_PROJECT_PATH,
        FIELD_PROJECT_PATH_REL,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    ]
    .into_iter()
    .filter_map(|key| {
        object
            .get(key)
            .map(|value| (key.to_string(), value.clone()))
    })
    .collect()
}

fn project_migration_error(
    path: &Path,
    stage: SettingsSaveStage,
    reason: &str,
) -> SettingsSaveError {
    project_migration_error_labeled(path, path, stage, reason)
}

fn project_migration_error_labeled(
    path: &Path,
    label: &Path,
    stage: SettingsSaveStage,
    reason: &str,
) -> SettingsSaveError {
    let kind = if reason.starts_with("project_paths_conflict") {
        SettingsSaveReason::ProjectPathsConflict
    } else if reason.starts_with("project_paths_backup_mismatch") {
        SettingsSaveReason::ProjectPathsBackupMismatch
    } else {
        SettingsSaveReason::ProjectPathsSourceRejected
    };
    SettingsSaveError::semantic(
        stage,
        path,
        None,
        kind,
        SettingsSaveLegacyOutward::DiskRead(format!("{}: {reason}", label.display())),
    )
}

fn read_project_source_bytes_labeled_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
) -> Result<Option<Vec<u8>>, SettingsSaveError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SettingsSaveError::io(
                SettingsSaveStage::ProjectPathsRefreshDiskGate,
                path,
                None,
                error,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ))
        }
        Ok(_) => {}
    }
    crate::path_identity::read_bounded_regular(path, limit)
        .map(|(bytes, _)| Some(bytes))
        .map_err(|reason| {
            project_migration_error_labeled(
                path,
                label,
                SettingsSaveStage::ProjectPathsRefreshDiskGate,
                &reason,
            )
        })
}

#[derive(Debug)]
struct RawProjectSource {
    object: Map<String, Value>,
    bytes: Vec<u8>,
}

fn read_raw_project_source(path: &Path) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    read_raw_project_source_with_limit(path, 16 * 1024 * 1024, false)
}

fn read_raw_project_source_with_limit(
    path: &Path,
    limit: usize,
    legacy_settings_diagnostic: bool,
) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    read_raw_project_source_labeled_with_limit(path, path, limit, legacy_settings_diagnostic)
}

fn read_raw_project_source_labeled_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
    legacy_settings_diagnostic: bool,
) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    let Some(bytes) = read_project_source_bytes_labeled_with_limit(path, label, limit)? else {
        return Ok(None);
    };
    raw_project_source_from_bytes(bytes, path, label, legacy_settings_diagnostic).map(Some)
}

/// The decode half of the raw project-source reader, shared with the owner's
/// single capture so selection judges captured bytes exactly like read ones.
fn raw_project_source_from_bytes(
    bytes: Vec<u8>,
    path: &Path,
    label: &Path,
    legacy_settings_diagnostic: bool,
) -> Result<RawProjectSource, SettingsSaveError> {
    let value = crate::path_identity::parse_json_no_duplicates(&bytes).map_err(|reason| {
        let mut error = project_migration_error_labeled(
            path,
            label,
            SettingsSaveStage::ProjectPathsRefreshDiskGate,
            &reason,
        );
        if legacy_settings_diagnostic {
            error.legacy_outward = SettingsSaveLegacyOutward::DiskRead(format!(
                "settings.json exists but could not be parsed ({}: {reason}); refusing to modify \
                    it - fix or remove the file first",
                label.display()
            ));
        }
        error
    })?;
    let Value::Object(object) = value else {
        return Err(project_migration_error_labeled(
            path,
            label,
            SettingsSaveStage::ProjectPathsRefreshDiskGate,
            "project_paths_source_not_object",
        ));
    };
    Ok(RawProjectSource { object, bytes })
}

#[derive(Debug)]
struct SelectedProjectSource {
    legacy: Option<RawProjectSource>,
    destination: Option<RawProjectSource>,
    projection: Map<String, Value>,
}

// Capture both immutable objects only while the original settings token is held.
fn select_project_source_locked(
    transaction: &ProjectPathsWriteTransaction,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_locked_path(&transaction.path)
}

fn select_project_source_at_locked_path(
    path: &Path,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_locked_path_with_limit(path, 16 * 1024 * 1024)
}

fn select_project_source_at_locked_path_with_limit(
    path: &Path,
    limit: usize,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_labeled_path_with_limit(path, path, limit)
}

fn select_project_source_at_labeled_path_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    let legacy = read_raw_project_source_labeled_with_limit(path, label, limit, true)?;
    let destination_path = project_paths_path(path);
    let destination_label = project_paths_path(label);
    let destination = read_raw_project_source_labeled_with_limit(
        &destination_path,
        &destination_label,
        limit,
        false,
    )?;
    select_project_source_from_raw(
        legacy,
        destination,
        label,
        &destination_path,
        &destination_label,
    )
}

/// The pure selector over both raw sources: the destination wins, and a non-empty
/// legacy projection that differs from it is a conflict, never a silent choice.
fn select_project_source_from_raw(
    legacy: Option<RawProjectSource>,
    destination: Option<RawProjectSource>,
    label: &Path,
    destination_path: &Path,
    destination_label: &Path,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    let legacy_projection = legacy
        .as_ref()
        .map(|source| raw_project_projection(&source.object))
        .unwrap_or_default();
    let projection = match &destination {
        None => legacy_projection,
        Some(source) => {
            let projection = raw_project_projection(&source.object);
            if !legacy_projection.is_empty() && legacy_projection != projection {
                return Err(project_migration_error_labeled(
                    destination_path,
                    destination_label,
                    SettingsSaveStage::ProjectPathsRefreshDiskGate,
                    &format!(
                        "project_paths_conflict; source={}; destination={}; preserve both \
                        files and reconcile raw projections to equality",
                        label.display(),
                        destination_label.display()
                    ),
                ));
            }
            projection
        }
    };
    Ok(SelectedProjectSource {
        legacy,
        destination,
        projection,
    })
}

// Read-only selection acquires before even checking either source's existence.
#[cfg(test)]
fn select_project_source_at_path(path: &Path) -> Result<Map<String, Value>, SettingsSaveError> {
    let lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "select_project_source_at_path",
    )?;
    let transaction = ProjectPathsWriteTransaction {
        _lock: lock,
        path: path.to_path_buf(),
        fresh: AppSettings::default(),
        revalidated_after_failure: false,
    };
    Ok(select_project_source_locked(&transaction)?.projection)
}

fn publish_project_migration_backup_labeled(
    settings_path: &Path,
    label: &Path,
    bytes: &[u8],
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    use std::io::Write as _;
    let backup =
        settings_path.with_file_name(super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    let backup_label =
        label.with_file_name(super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    let mismatch = || {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PostWriteVerify,
            &format!(
                "project_paths_backup_mismatch; source={}; destination={}; backup={}; stop all \
            AC/old processes, preserve copies and orphan temps, rename backup without overwrite to \
            project-paths.pre-migration.recovered-<UTC>-<unique>.no-git.json, verify intended \
            legacy projection and \
            retry",
                label.display(),
                project_paths_path(label).display(),
                backup_label.display()
            ),
        )
    };
    if let Some(existing) =
        read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
    {
        return if existing == bytes {
            Ok(())
        } else {
            Err(mismatch())
        };
    }
    let parent = backup.parent().ok_or_else(|| {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PrepareTarget,
            "missing parent",
        )
    })?;
    crate::path_identity::verify_component_chain(parent).map_err(|reason| {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PrepareTarget,
            &reason,
        )
    })?;
    let mut name = backup.file_name().unwrap().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        SAVE_OP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = parent.join(name);
    checkpoint("backup_create", &temp)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let io_error = |stage, source| {
        SettingsSaveError::io(
            stage,
            &backup,
            Some(&temp),
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    };
    let mut file = options
        .open(&temp)
        .map_err(|error| io_error(SettingsSaveStage::TempCreate, error))?;
    crate::path_identity::verify_opened_regular_file(&temp, &file, false).map_err(|reason| {
        project_migration_error_labeled(
            &temp,
            &label.with_file_name(temp.file_name().unwrap()),
            SettingsSaveStage::TempVerify,
            &reason,
        )
    })?;
    let prefix = bytes.len() / 2;
    file.write_all(&bytes[..prefix])
        .map_err(|error| io_error(SettingsSaveStage::TempWrite, error))?;
    checkpoint("backup_prefix", &temp)?;
    file.write_all(&bytes[prefix..])
        .map_err(|error| io_error(SettingsSaveStage::TempWrite, error))?;
    checkpoint("backup_write", &temp)?;
    file.sync_all()
        .map_err(|error| io_error(SettingsSaveStage::TempSync, error))?;
    checkpoint("backup_sync", &temp)?;
    drop(file);
    if read_project_source_bytes_labeled_with_limit(
        &temp,
        &label.with_file_name(temp.file_name().unwrap()),
        16 * 1024 * 1024,
    )?
    .as_deref()
        != Some(bytes)
    {
        return Err(mismatch());
    }
    checkpoint("backup_before_rename", &temp)?;
    // Never use the replacing atomic writer for the immutable fixed name.
    if let Some(existing) =
        read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
    {
        return if existing == bytes {
            Ok(())
        } else {
            Err(mismatch())
        };
    }
    if let Err(error) = std::fs::rename(&temp, &backup) {
        match read_project_source_bytes_labeled_with_limit(
            &backup,
            &backup_label,
            16 * 1024 * 1024,
        )? {
            Some(existing) if existing == bytes => {}
            Some(_) => return Err(mismatch()),
            None => return Err(io_error(SettingsSaveStage::AtomicReplace, error)),
        }
    }
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(SettingsSaveStage::ParentDirectorySync, error))?;
    if read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
        .as_deref()
        != Some(bytes)
    {
        return Err(mismatch());
    }
    checkpoint("backup_published", &backup)
}

fn ensure_project_paths_migrated_locked(
    transaction: &ProjectPathsWriteTransaction,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_locked_path(&transaction.path, &mut |_, _| Ok(()))
}

#[cfg(test)]
fn ensure_project_paths_migrated_with(
    transaction: &ProjectPathsWriteTransaction,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_locked_path(&transaction.path, checkpoint)
}

fn ensure_project_paths_migrated_at_locked_path(
    path: &Path,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_labeled_path(path, path, checkpoint)
}

fn ensure_project_paths_migrated_at_labeled_path(
    path: &Path,
    label: &Path,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    let selected = select_project_source_at_labeled_path_with_limit(path, label, 16 * 1024 * 1024)?;
    let destination = project_paths_path(path);
    let destination_label = project_paths_path(label);
    let legacy_fields = selected
        .legacy
        .as_ref()
        .is_some_and(|source| !raw_project_projection(&source.object).is_empty());
    if selected.destination.is_none() {
        if legacy_fields {
            publish_project_migration_backup_labeled(
                path,
                label,
                &selected.legacy.as_ref().unwrap().bytes,
                checkpoint,
            )?;
        }
        checkpoint("destination_before_write", &destination)?;
        write_value_atomic_labeled(
            &Value::Object(selected.projection.clone()),
            &destination,
            &destination_label,
        )?;
        checkpoint("destination_published", &destination)?;
    }
    if !legacy_fields {
        return Ok(());
    }
    checkpoint("destination_readback", &destination)?;
    let readback = read_raw_project_source_labeled_with_limit(
        &destination,
        &destination_label,
        16 * 1024 * 1024,
        false,
    )?
    .ok_or_else(|| {
        project_migration_error_labeled(
            &destination,
            &destination_label,
            SettingsSaveStage::PostWriteRead,
            "project_paths_destination_disappeared",
        )
    })?;
    if raw_project_projection(&readback.object) != selected.projection {
        return Err(project_migration_error_labeled(
            &destination,
            &destination_label,
            SettingsSaveStage::PostWriteVerify,
            "project_paths_readback_mismatch",
        ));
    }
    checkpoint("source_cleanup", path)?;
    let original = selected.legacy.unwrap();
    let mut cleaned = original.object;
    for key in selected.projection.keys() {
        cleaned.remove(key);
    }
    let written = write_value_atomic_labeled(&Value::Object(cleaned), path, label)?;
    if original.bytes != written {
        rotate_settings_backups_labeled(path, label, &original.bytes);
    }
    checkpoint("cleanup_complete", path)
}

/// Owns the shared file lock throughout a synchronous project operation.
/// Never carry this token across an await.
pub(crate) struct ProjectPathsWriteTransaction {
    _lock: SettingsFileLock,
    path: PathBuf,
    fresh: AppSettings,
    revalidated_after_failure: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectPathsRepairStage {
    Read,
    Write,
}

#[track_caller]
fn empty_project_transaction(
    path: &Path,
    operation: &'static str,
) -> Result<ProjectPathsWriteTransaction, SettingsSaveError> {
    Ok(ProjectPathsWriteTransaction {
        _lock: SettingsFileLock::acquire_contextual(
            path,
            std::time::Duration::from_secs(2),
            operation,
        )?,
        path: path.to_path_buf(),
        fresh: AppSettings::default(),
        revalidated_after_failure: false,
    })
}

fn apply_selected_project_state(settings: &mut AppSettings, projection: &Map<String, Value>) {
    let state = decode_project_state(
        projection,
        production_instance_base().as_deref(),
        &projects::FsCandidateResolver,
    );
    settings.project_paths = state.active_selected();
    settings.project_path = state.selected_head.clone();
    settings.archived_project_paths = state.archived_management_paths();
    settings.project_path_state = Arc::new(state);
    settings.project_paths_persistence_error = None;
}

pub(crate) fn begin_project_paths_transaction_at_path(
    current: &AppSettings,
    settings_path: &Path,
) -> Result<ProjectPathsWriteTransaction, String> {
    let mut transaction =
        empty_project_transaction(settings_path, "begin_project_paths_transaction_at_path")
            .map_err(|error| {
                report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
            })?;
    ensure_project_paths_migrated_locked(&transaction).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    transaction.fresh = current.clone();
    refresh_coding_agent_identity_at_locked_path(&mut transaction.fresh, settings_path);
    let selected = select_project_source_locked(&transaction).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    apply_selected_project_state(&mut transaction.fresh, &selected.projection);
    Ok(transaction)
}

impl ProjectPathsWriteTransaction {
    pub(crate) fn fresh_settings(&self) -> &AppSettings {
        &self.fresh
    }

    pub(crate) fn commit_project_groups(
        &mut self,
        candidate: &AppSettings,
        active: bool,
        archived: bool,
    ) -> Result<AppSettings, String> {
        if let Some(message) = &candidate.project_paths_persistence_error {
            return Err(message.clone());
        }
        self.revalidated_after_failure = false;
        let result = self.write_project_groups(candidate, active, archived);
        self.handle_commit_result(result)
    }

    pub(crate) fn refresh_raw_projects(&self, settings: &mut AppSettings) -> Result<(), String> {
        let selected = select_project_source_locked(self).map_err(|error| {
            report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
        })?;
        apply_selected_project_state(settings, &selected.projection);
        if settings.project_path_state.has_structural() {
            return Err("project metadata is malformed".to_string());
        }
        if let Some(lists) = project_lists_from_projection(&selected.projection)? {
            settings.project_paths = lists.project_paths;
            settings.project_path = lists.project_path;
            settings.archived_project_paths = lists.archived_project_paths.unwrap_or_default();
        }
        Ok(())
    }

    pub(crate) fn raw_project_projection(&self) -> Result<Map<String, Value>, String> {
        select_project_source_locked(self)
            .map(|selected| selected.projection)
            .map_err(|error| {
                report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
            })
    }

    pub(crate) fn restore_raw_project_projection(
        &mut self,
        projection: &Map<String, Value>,
    ) -> Result<AppSettings, String> {
        let result = (|| {
            let selected = select_project_source_locked(self)?;
            let mut object = selected
                .destination
                .map(|source| source.object)
                .unwrap_or_default();
            replace_project_projection(&mut object, projection);
            write_value_atomic(&Value::Object(object), &project_paths_path(&self.path))?;
            let selected = select_project_source_locked(self)?;
            let mut written = self.fresh.clone();
            refresh_coding_agent_identity_at_locked_path(&mut written, &self.path);
            apply_selected_project_state(&mut written, &selected.projection);
            Ok(written)
        })();
        self.handle_commit_result(result)
    }

    fn write_project_groups(
        &self,
        candidate: &AppSettings,
        active: bool,
        archived: bool,
    ) -> Result<AppSettings, SettingsSaveError> {
        let selected = select_project_source_locked(self)?;
        let destination = project_paths_path(&self.path);
        let mut out = selected
            .destination
            .map(|source| source.object)
            .unwrap_or_default();
        let state = hidden_state_for_write(candidate);
        if state.has_structural() {
            return Err(project_migration_error(
                &destination,
                SettingsSaveStage::ReconcileDiskGate,
                "project metadata is malformed",
            ));
        }
        let base = production_instance_base();
        if active && (state.runtime_authoritative || state.active_reconcile_eligible) {
            write_active_group(&mut out, &state, base.as_deref());
        }
        if archived && (state.runtime_authoritative || state.archived_reconcile_eligible) {
            write_archived_group(&mut out, &state, base.as_deref());
        }
        if out
            != read_raw_project_source(&destination)?
                .map(|source| source.object)
                .unwrap_or_default()
        {
            write_value_atomic(&Value::Object(out), &destination)?;
        }
        let projection = select_project_source_locked(self)?.projection;
        let mut written = candidate.clone();
        refresh_coding_agent_identity_at_locked_path(&mut written, &self.path);
        apply_selected_project_state(&mut written, &projection);
        Ok(written)
    }

    fn handle_commit_result(
        &mut self,
        result: Result<AppSettings, SettingsSaveError>,
    ) -> Result<AppSettings, String> {
        match result {
            Ok(mut written) => {
                written.project_paths_persistence_error = None;
                self.fresh = written.clone();
                Ok(written)
            }
            Err(error) => {
                let uncertain = match error.stage {
                    SettingsSaveStage::AtomicReplace
                    | SettingsSaveStage::PostWriteRead
                    | SettingsSaveStage::PostWriteVerify
                    | SettingsSaveStage::ReDecode => true,
                    #[cfg(unix)]
                    SettingsSaveStage::TargetPermissions
                    | SettingsSaveStage::ParentDirectorySync => true,
                    _ => false,
                };
                let message =
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings);
                if uncertain {
                    // Re-read under this same lock; never roll back a write
                    // whose outcome cannot be established.
                    let validated = select_project_source_locked(self)
                        .map(|selected| {
                            let mut settings = self.fresh.clone();
                            refresh_coding_agent_identity_at_locked_path(&mut settings, &self.path);
                            apply_selected_project_state(&mut settings, &selected.projection);
                            settings
                        })
                        .map_err(|error| {
                            report_settings_save_error(
                                error,
                                SettingsSaveReportSurface::GeneralSettings,
                            )
                        });
                    match validated {
                        Ok(mut settings) => {
                            settings.local_overlay_state = self.fresh.local_overlay_state.clone();
                            settings.project_paths_persistence_error = None;
                            self.fresh = settings;
                        }
                        Err(reason) => {
                            self.fresh.project_paths_persistence_error = Some(format!(
                                "{message}; project state revalidation failed: {reason}"
                            ))
                        }
                    }
                    self.revalidated_after_failure = true;
                }
                Err(message)
            }
        }
    }
}

/// Validate fresh authority even if the cached state reports no repair.
pub(crate) fn repair_fresh_project_paths_at_path(
    current: &mut AppSettings,
    path: &Path,
) -> Result<(), (ProjectPathsRepairStage, String)> {
    let mut transaction = match begin_project_paths_transaction_at_path(current, path) {
        Ok(transaction) => transaction,
        Err(message) => {
            current.project_paths_persistence_error = Some(message.clone());
            return Err((ProjectPathsRepairStage::Read, message));
        }
    };
    let candidate = transaction.fresh_settings().clone();
    let state = &candidate.project_path_state;
    let active = state.active_reconcile_eligible;
    let archived = state.archived_reconcile_eligible;
    if state.has_structural() || !(active || archived) {
        *current = candidate;
        return Ok(());
    }
    match transaction.commit_project_groups(&candidate, active, archived) {
        Ok(written) => {
            *current = written;
            Ok(())
        }
        Err(message) => {
            if transaction.revalidated_after_failure {
                *current = transaction.fresh_settings().clone();
            }
            Err((ProjectPathsRepairStage::Write, message))
        }
    }
}

// Diagnostic transport is prepared before kernel ownership. The worker is the
// only place that formats or writes logs; an owner never waits for its sink.
const SETTINGS_LOCK_DEBUG_CAPACITY: usize = 256;
const SETTINGS_LOCK_DEBUG_PATH_BYTES: usize = 4096;
static SETTINGS_LOCK_DEBUG_TRANSPORT: OnceLock<
    Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>>,
> = OnceLock::new();
static SETTINGS_LOCK_DEBUG_INSTANCE: OnceLock<uuid::Uuid> = OnceLock::new();
static SETTINGS_LOCK_DEBUG_ATTEMPT: AtomicU64 = AtomicU64::new(0);
static SETTINGS_LOCK_DEBUG_LOST: AtomicU64 = AtomicU64::new(0);

struct SettingsLockDebugContext {
    instance: uuid::Uuid,
    pid: u32,
    attempt: u64,
    operation: &'static str,
    caller: &'static std::panic::Location<'static>,
    thread: std::thread::ThreadId,
    target: String,
    sidecar: String,
    path_truncated: bool,
}

struct SettingsLockDebugEvent {
    context: Arc<SettingsLockDebugContext>,
    phase: &'static str,
    utc: chrono::DateTime<chrono::Utc>,
    thread: std::thread::ThreadId,
    preparation_ms: Option<f64>,
    wait_ms: Option<f64>,
    hold_ms: Option<f64>,
    reason: Option<&'static str>,
    raw_os_error: Option<i32>,
}

struct SettingsLockDebugAttempt {
    context: Arc<SettingsLockDebugContext>,
    sender: std::sync::mpsc::SyncSender<SettingsLockDebugEvent>,
    started: std::time::Instant,
    retry_started: Option<std::time::Instant>,
    acquired_at: Option<std::time::Instant>,
}

// Bound the escaped representation, including quotes, without splitting UTF-8.
// A truncated prefix is intentionally not a complete identity for a lock.
fn settings_lock_debug_path(path: &Path) -> (String, bool) {
    let mut escaped = String::with_capacity(SETTINGS_LOCK_DEBUG_PATH_BYTES);
    escaped.push('"');
    for character in path.to_string_lossy().chars() {
        let piece = match character {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            c if c.is_control() => format!("\\u{:04x}", c as u32),
            c => c.to_string(),
        };
        if escaped.len() + piece.len() + 1 > SETTINGS_LOCK_DEBUG_PATH_BYTES {
            escaped.push('"');
            return (escaped, true);
        }
        escaped.push_str(&piece);
    }
    escaped.push('"');
    (escaped, false)
}

fn settings_lock_debug_transport(
    slot: &OnceLock<Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>>>,
    enabled: bool,
    spawn: impl FnOnce(std::sync::mpsc::Receiver<SettingsLockDebugEvent>) -> std::io::Result<()>,
) -> Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>> {
    // In particular, INFO must not freeze None before a runtime change to DEBUG.
    if !enabled {
        return None;
    }
    slot.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(SETTINGS_LOCK_DEBUG_CAPACITY);
        spawn(receiver).ok().map(|()| sender)
    })
    .clone()
}

fn settings_lock_debug_worker(receiver: std::sync::mpsc::Receiver<SettingsLockDebugEvent>) {
    for event in receiver {
        #[cfg(test)]
        settings_lock_debug_test_before_emit(&event);
        settings_lock_debug_emit(&event);
        #[cfg(test)]
        settings_lock_debug_test_after_emit(&event);
    }
}

#[cfg(test)]
struct SettingsLockDebugTestHooks {
    gate_phase: &'static str,
    gate_once: std::sync::atomic::AtomicBool,
    gated: std::sync::mpsc::Sender<()>,
    resume: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    processed: std::sync::mpsc::Sender<serde_json::Value>,
}

#[cfg(test)]
static SETTINGS_LOCK_DEBUG_TEST_HOOKS: OnceLock<SettingsLockDebugTestHooks> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn settings_lock_debug_test_before_emit(event: &SettingsLockDebugEvent) {
    let Some(hooks) = SETTINGS_LOCK_DEBUG_TEST_HOOKS.get() else {
        return;
    };
    if event.phase == hooks.gate_phase && hooks.gate_once.swap(false, Ordering::Relaxed) {
        hooks.gated.send(()).expect("gate observer disconnected");
        hooks
            .resume
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("gate not resumed by harness");
    }
}

#[cfg(test)]
fn settings_lock_debug_test_after_emit(event: &SettingsLockDebugEvent) {
    if let Some(hooks) = SETTINGS_LOCK_DEBUG_TEST_HOOKS.get() {
        let _ = hooks.processed.send(serde_json::json!({
            "phase": event.phase,
            "instance": event.context.instance.to_string(),
            "pid": event.context.pid,
            "attempt": event.context.attempt,
            "operation": event.context.operation,
            "sidecar": event.context.sidecar,
            "target": event.context.target,
            "reason": event.reason,
            "raw_os_error": event.raw_os_error,
            "wait_ms": event.wait_ms,
            "hold_ms": event.hold_ms,
            "preparation_ms": event.preparation_ms,
            "acquisition_thread": format!("{:?}", event.context.thread),
            "event_thread": format!("{:?}", event.thread),
            "event_utc": event.utc.to_rfc3339(),
            "lost_events": SETTINGS_LOCK_DEBUG_LOST.load(Ordering::Relaxed),
        }));
    }
}

fn settings_lock_debug_emit(event: &SettingsLockDebugEvent) {
    // Reevaluate effective app/CLI filtering after a potentially stalled sink.
    if !log::log_enabled!(target: module_path!(), log::Level::Debug) {
        return;
    }
    let context = &event.context;
    log::debug!(target: module_path!(),
        "settings-lock phase={} pid={} instance={} attempt={} operation={} origin={}:{}:{} \
            acquisition_thread={:?} event_thread={:?} target={} sidecar={} path_truncated={} \
            event_utc={} emitted_utc={} preparation_ms={:?} wait_ms={:?} hold_ms={:?} stage={} \
            reason={:?} raw_os_error={:?} handle_closed={} lost_events={}",
        event.phase, context.pid, context.instance, context.attempt,
        context.operation, context.caller.file(), context.caller.line(),
        context.caller.column(), context.thread, event.thread, context.target,
        context.sidecar, context.path_truncated, event.utc.to_rfc3339(),
        chrono::Utc::now().to_rfc3339(), event.preparation_ms, event.wait_ms,
        event.hold_ms, if event.phase == "release" { "unlock" } else { "lock_acquir\
            e" }, event.reason, event.raw_os_error,
        matches!(event.phase, "release" | "timeout" | "acquire-error"),
        SETTINGS_LOCK_DEBUG_LOST.load(Ordering::Relaxed));
}

impl SettingsLockDebugAttempt {
    fn prepare(
        settings_path: &Path,
        lock_name: &str,
        operation: &'static str,
        caller: &'static std::panic::Location<'static>,
    ) -> Option<Self> {
        let sender = settings_lock_debug_transport(
            &SETTINGS_LOCK_DEBUG_TRANSPORT,
            log::log_enabled!(target: module_path!(), log::Level::Debug),
            |receiver| {
                std::thread::Builder::new()
                    .name("settings-lock-debug".into())
                    .spawn(move || settings_lock_debug_worker(receiver))
                    .map(|_| ())
            },
        )?;
        let attempt = SETTINGS_LOCK_DEBUG_ATTEMPT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()?
            + 1;
        let (target, target_truncated) = settings_lock_debug_path(settings_path);
        let lock_path = settings_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(lock_name);
        let (sidecar, sidecar_truncated) = settings_lock_debug_path(&lock_path);
        let debug = Self {
            context: Arc::new(SettingsLockDebugContext {
                instance: *SETTINGS_LOCK_DEBUG_INSTANCE.get_or_init(uuid::Uuid::new_v4),
                pid: std::process::id(),
                attempt,
                operation,
                caller,
                thread: std::thread::current().id(),
                target,
                sidecar,
                path_truncated: target_truncated || sidecar_truncated,
            }),
            sender,
            started: std::time::Instant::now(),
            retry_started: None,
            acquired_at: None,
        };
        debug.event("acquire-start", None, None);
        Some(debug)
    }

    fn event(&self, phase: &'static str, reason: Option<&'static str>, raw_os_error: Option<i32>) {
        self.event_at(phase, reason, raw_os_error, std::time::Instant::now());
    }

    fn event_at(
        &self,
        phase: &'static str,
        reason: Option<&'static str>,
        raw_os_error: Option<i32>,
        at: std::time::Instant,
    ) {
        let event = SettingsLockDebugEvent {
            context: Arc::clone(&self.context),
            phase,
            utc: chrono::Utc::now(),
            thread: std::thread::current().id(),
            preparation_ms: (phase != "acquire-start").then(|| {
                self.retry_started
                    .unwrap_or(at)
                    .duration_since(self.started)
                    .as_secs_f64()
                    * 1000.0
            }),
            wait_ms: self.retry_started.map(|retry| {
                self.acquired_at
                    .unwrap_or(at)
                    .duration_since(retry)
                    .as_secs_f64()
                    * 1000.0
            }),
            hold_ms: self
                .acquired_at
                .filter(|_| phase == "release")
                .map(|acquired| at.duration_since(acquired).as_secs_f64() * 1000.0),
            reason,
            raw_os_error,
        };
        if self.sender.try_send(event).is_err() {
            // Saturation avoids diagnostic counter wraparound too.
            let _ = SETTINGS_LOCK_DEBUG_LOST.fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |value| Some(value.saturating_add(1)),
            );
        }
    }

    fn failure(&self, error: &SettingsSaveError) {
        let (reason, raw) = match &error.cause {
            SettingsSaveCause::Io(source) => ("io_error", source.raw_os_error()),
            SettingsSaveCause::Semantic(reason) => (reason.as_str(), None),
            SettingsSaveCause::Json(_) => ("json_error", None),
        };
        self.event(
            if reason == "lock_timed_out" {
                "timeout"
            } else {
                "acquire-error"
            },
            Some(reason),
            raw,
        );
    }
}

struct SettingsFileLock {
    file: Option<std::fs::File>,
    debug: Option<SettingsLockDebugAttempt>,
}

impl SettingsFileLock {
    #[cfg(test)]
    #[track_caller]
    fn acquire(
        settings_path: &Path,
        timeout: std::time::Duration,
    ) -> Result<Self, SettingsSaveError> {
        Self::acquire_contextual(settings_path, timeout, "test-lock")
    }

    #[track_caller]
    fn acquire_contextual(
        settings_path: &Path,
        timeout: std::time::Duration,
        operation: &'static str,
    ) -> Result<Self, SettingsSaveError> {
        Self::acquire_named(settings_path, SETTINGS_LOCK_FILE_NAME, timeout, operation)
    }

    /// The same acquisition over another stable sidecar beside the settings file.
    #[track_caller]
    fn acquire_named(
        settings_path: &Path,
        lock_name: &str,
        timeout: std::time::Duration,
        operation: &'static str,
    ) -> Result<Self, SettingsSaveError> {
        let mut debug = SettingsLockDebugAttempt::prepare(
            settings_path,
            lock_name,
            operation,
            std::panic::Location::caller(),
        );
        // Inner failure drops every opened file before a diagnostic is enqueued.
        let file = match Self::acquire_file(settings_path, lock_name, timeout, &mut debug) {
            Ok(file) => file,
            Err(error) => {
                if let Some(debug) = &debug {
                    debug.failure(&error);
                }
                return Err(error);
            }
        };
        Ok(Self {
            file: Some(file),
            debug,
        })
    }

    fn acquire_file(
        settings_path: &Path,
        lock_name: &str,
        timeout: std::time::Duration,
        debug: &mut Option<SettingsLockDebugAttempt>,
    ) -> Result<std::fs::File, SettingsSaveError> {
        let parent = settings_path.parent().ok_or_else(|| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::MissingParentDirectory,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        std::fs::create_dir_all(parent).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        crate::path_identity::verify_component_chain(parent).map_err(|_| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockIdentityRejected,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        let lock_path = parent.join(lock_name);
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            options
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options.open(&lock_path).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        let metadata = file.metadata().map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        if !metadata.is_file() {
            return Err(SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockFileNotRegular,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.nlink() != 1 {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    SettingsSaveReason::LockLinkCountRejected,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                ));
            }
            std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).map_err(
                |source| {
                    SettingsSaveError::io(
                        SettingsSaveStage::LockAcquire,
                        settings_path,
                        None,
                        source,
                        SettingsSaveLegacyOutward::SettingsLockUnavailable,
                    )
                },
            )?;
        }
        #[cfg(windows)]
        crate::path_identity::verify_regular_file(&lock_path).map_err(|_| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockIdentityRejected,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;

        let retry_started = std::time::Instant::now();
        if let Some(debug) = debug.as_mut() {
            debug.retry_started = Some(retry_started);
        }
        let deadline = retry_started.checked_add(timeout).ok_or_else(|| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockDeadlineOverflow,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        loop {
            #[cfg(any(unix, windows))]
            let locked = try_lock_settings_file(&file).map_err(|source| {
                SettingsSaveError::io(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    source,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                )
            })?;
            #[cfg(not(any(unix, windows)))]
            let locked = try_lock_settings_file(&file).map_err(|reason| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    reason,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                )
            })?;
            if locked {
                let acquired_at = std::time::Instant::now();
                if let Some(debug) = debug.as_mut() {
                    debug.acquired_at = Some(acquired_at);
                    debug.event("acquired", None, None);
                }
                return Ok(file);
            }
            if std::time::Instant::now() >= deadline {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    SettingsSaveReason::LockTimedOut,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for SettingsFileLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let unlock_at = std::time::Instant::now();
            let result = unlock_settings_file(&file);
            #[cfg(test)]
            let result = if SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR.with(|flag| flag.replace(false)) {
                Err(std::io::Error::from_raw_os_error(5))
            } else {
                result
            };
            drop(file);
            if let Some(debug) = &self.debug {
                let (reason, raw) = match result {
                    Ok(()) => ("unlock_ok", None),
                    Err(error) => ("unlock_error", error.raw_os_error()),
                };
                debug.event_at("release", Some(reason), raw, unlock_at);
            }
        }
    }
}

#[cfg(unix)]
fn try_lock_settings_file(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    let source = std::io::Error::last_os_error();
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    // Compared rather than matched: `EWOULDBLOCK` and `EAGAIN` are the same
    // value on Linux and macOS, which makes the second pattern unreachable.
    let raw = error.raw_os_error();
    if raw == Some(libc::EWOULDBLOCK) || raw == Some(libc::EAGAIN) {
        Ok(false)
    } else {
        Err(source)
    }
}

#[cfg(unix)]
fn unlock_settings_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock_settings_file(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result != 0 {
        return Ok(true);
    }
    const ERROR_LOCK_VIOLATION: i32 = 33;
    let source = std::io::Error::last_os_error();
    if source.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
        Ok(false)
    } else {
        Err(source)
    }
}

#[cfg(windows)]
fn unlock_settings_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    if unsafe { UnlockFileEx(file.as_raw_handle(), 0, u32::MAX, u32::MAX, &mut overlapped) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn try_lock_settings_file(_file: &std::fs::File) -> Result<bool, SettingsSaveReason> {
    Err(SettingsSaveReason::LockPlatformUnsupported)
}

#[cfg(not(any(unix, windows)))]
fn unlock_settings_file(_file: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

/// #2348 - serialize one placement component with the same error shape as the
/// whole-object serializer, so a (theoretically impossible) serialization failure
/// aborts the save instead of writing a partial pair.
fn serialize_settings_component<T: serde::Serialize>(
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
fn preserve_main_window_placement_keys(out: &mut Map<String, Value>, disk: &Map<String, Value>) {
    for key in [FIELD_MAIN_GEOMETRY, FIELD_MAIN_DISPLAY_STATE] {
        match disk.get(key) {
            None | Some(Value::Null) => {}
            Some(value) => {
                out.insert(key.to_string(), value.clone());
            }
        }
    }
}

fn preflight_duplicate_agents(
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
fn save_settings_value(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
) -> Result<AppSettings, SettingsSaveError> {
    save_settings_value_with_startup(settings, path, mode, None)
}

fn save_settings_value_with_startup(
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
fn save_settings_value_locked(
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

fn restore_protected_agents_instance_groups(
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

fn retain_untransferred_registered_main_groups(
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

type RegisteredMainPreparedGroups<'a> = (
    Option<&'a Value>,
    Option<&'a Value>,
    Option<&'a Map<String, Value>>,
    &'a Path,
);

fn prepare_registered_main_agent_groups(
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

fn apply_registered_main_disk_fields(
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

fn save_settings_value_locked_with_startup(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
    terminal_snapshot_gate_mode: TerminalSnapshotGateWriteMode,
    main_window_placement_mode: MainWindowPlacementWriteMode,
    startup: Option<&StartupAgentRows>,
) -> Result<AppSettings, SettingsSaveError> {
    let disk_gate_stage = match &mode {
        ProjectWriteMode::Preserve => SettingsSaveStage::PreserveDiskGate,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => SettingsSaveStage::ReconcileDiskGate,
    };
    let initial_disk = read_disk_object_for_write_typed(path, disk_gate_stage)?;
    let reject_profiles = |reason: String| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    let profile_gate = capture_registered_profile_gate(path).map_err(&reject_profiles)?;
    let physical_sidecar =
        read_agents_instance_file(&agents_instance_path(path)).map_err(&reject_profiles)?;
    // Preparation and ownership use the original bytes checked by this gate,
    // rather than independently captured generations of the two files.
    let captured_main =
        captured_registered_profile_object(&profile_gate, path).map_err(&reject_profiles)?;
    let captured_sidecar =
        captured_registered_profile_object(&profile_gate, &agents_instance_path(path))
            .map_err(&reject_profiles)?;
    if initial_disk != captured_main || physical_sidecar != captured_sidecar {
        return Err(reject_profiles(format!(
            "staleProfileSource: {}",
            path.display()
        )));
    }
    let initial_disk = captured_main;
    let physical_sidecar = captured_sidecar;
    let profiles_in_sidecar = physical_sidecar.as_ref().is_some_and(|map| {
        map.contains_key("codingAgentProfiles")
            || (map.contains_key("agents")
                && initial_disk
                    .as_ref()
                    .is_none_or(|main| !main.contains_key("codingAgentProfiles")))
    });
    let profiles_unprotected = !settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles");
    let profile_before = selected_registered_profile_owner(
        &initial_disk.clone().unwrap_or_default(),
        physical_sidecar.as_ref(),
        profiles_unprotected,
    );
    let directional_split = profiles_in_sidecar
        && physical_sidecar
            .as_ref()
            .is_some_and(|map| !map.contains_key("agents"))
        && profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
            == Some(&Value::from(1));
    let consolidate_legacy = profiles_unprotected
        && profile_before
            .get("codingAgentProfiles")
            .is_some_and(|profiles| profiles.get("profileBindingVersion").is_none())
        && physical_profile_rows(&profile_before, true).is_ok();
    let profiles_in_sidecar = profiles_in_sidecar
        || (profiles_unprotected
            && profile_before
                .get("codingAgentProfiles")
                .is_none_or(|profiles| profiles.get("profileBindingVersion").is_none()));
    // Prove whole-row correspondence before any ID-keyed copying or conversion.
    let prepared_duplicate_agents = if matches!(mode, ProjectWriteMode::Preserve)
        && !settings.local_overlay_state.owns_top_level("agents")
    {
        preflight_duplicate_agents(settings, path, initial_disk.as_ref(), startup)?
    } else {
        None
    };
    let prepared_profiles = if matches!(mode, ProjectWriteMode::Preserve) {
        let (profile_incoming, owner_has_active_agents, duplicate_rows) =
            prepare_registered_profile_incoming(
                (settings, path),
                &profile_before,
                &physical_sidecar,
                &initial_disk,
                &profile_gate,
                (
                    directional_split,
                    profiles_unprotected,
                    profiles_in_sidecar,
                    consolidate_legacy,
                ),
                prepared_duplicate_agents.as_ref(),
            )
            .map_err(&reject_profiles)?;
        let mut prepared_profiles = prepare_registered_profile_owner(
            settings,
            path,
            &profile_before,
            &profile_incoming,
            profiles_in_sidecar,
            startup,
            (owner_has_active_agents && prepared_duplicate_agents.is_some()) || duplicate_rows,
        )
        .map_err(&reject_profiles)?;
        // Original main bytes are backed up below before any cleanup publish.
        if !physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("codingAgentProfiles"))
            && owner_has_active_agents
        {
            if let Some(profiles) = prepared_profiles
                .as_mut()
                .and_then(|prepared| prepared.get_mut("codingAgentProfiles"))
                .and_then(Value::as_object_mut)
            {
                for key in ["letters", "matrix", "agentDefaults"] {
                    profiles.remove(key);
                }
            }
        }
        if let Some(prepared) = &prepared_profiles {
            validate_physical_identity_delta(
                profile_before.as_object().unwrap(),
                prepared.as_object().unwrap(),
                !settings.local_overlay_state.owns_top_level("agents"),
                !directional_split,
            )
            .map_err(&reject_profiles)?;
            if directional_split {
                validate_directional_prepared_identity(&profile_before, prepared)
                    .map_err(&reject_profiles)?;
            }
        }
        if directional_split || settings.local_overlay_state.owns_top_level("agents") {
            if let Some(prepared) = prepared_profiles.as_mut().and_then(Value::as_object_mut) {
                prepared.remove("agents");
            }
        }
        prepared_profiles
    } else {
        None
    };
    // Validate the main-row packet as well before any migration, backup or
    // sidecar publisher. The real publishers below retain their existing order.
    prevalidate_registered_main_packet(
        (settings, path),
        &initial_disk,
        &physical_sidecar,
        prepared_profiles.as_ref(),
        prepared_duplicate_agents.as_ref(),
        (
            &mode,
            disk_gate_stage,
            terminal_snapshot_gate_mode,
            &main_window_placement_mode,
            directional_split,
            profiles_in_sidecar,
        ),
    )?;
    recheck_registered_profile_gate(&profile_gate).map_err(&reject_profiles)?;
    ensure_project_paths_migrated_at_locked_path(path, &mut |_, _| Ok(()))?;
    let selected_projection = select_project_source_at_locked_path(path)?.projection;
    let base = production_instance_base();
    let disk_gate_stage = match &mode {
        ProjectWriteMode::Preserve => SettingsSaveStage::PreserveDiskGate,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => SettingsSaveStage::ReconcileDiskGate,
    };
    let disk_read = read_disk_object_and_contents_for_write_typed(path, disk_gate_stage)?;
    backup_legacy_profiles_before_write(
        path,
        disk_gate_stage,
        &mode,
        disk_read.as_ref().map(|(map, _)| map),
        disk_read.as_ref().map(|(_, contents)| contents.as_str()),
    )?;
    // #2058: the exact bytes this save is about to replace, captured before the
    // tuple is reduced to its object half.
    let previous_contents: Option<String> = disk_read.as_ref().map(|(_, c)| c.clone());
    let disk = disk_read.map(|(map, _)| map);
    let serialize_object = || -> Result<Map<String, Value>, SettingsSaveError> {
        match serde_json::to_value(settings).map_err(|source| {
            let outward = format!("Failed to serialize settings: {source}");
            SettingsSaveError::json(
                SettingsSaveStage::Serialize,
                path,
                None,
                source,
                SettingsSaveLegacyOutward::Serialize(outward),
            )
        })? {
            Value::Object(m) => Ok(m),
            _ => Err(SettingsSaveError::semantic(
                SettingsSaveStage::Serialize,
                path,
                None,
                SettingsSaveReason::SerializedJsonNotObject,
                SettingsSaveLegacyOutward::SerializedJsonNotObject(
                    "settings did not serialize to a JSON object".to_string(),
                ),
            )),
        }
    };

    // Base object: Preserve starts from the incoming settings (non-project edits
    // apply); Reconcile is project-only and starts from the fresh disk object
    // (or the live settings when the file is absent).
    let mut out: Map<String, Value> = match mode {
        ProjectWriteMode::Preserve => serialize_object()?,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => match &disk {
            Some(m) => m.clone(),
            None => serialize_object()?,
        },
    };

    // E2432: only the dedicated preference publisher changes menu choices.
    out.remove("agentFileSourceByMenu");
    if let Some(preferences) = disk
        .as_ref()
        .and_then(|disk| disk.get("agentFileSourceByMenu"))
    {
        out.insert("agentFileSourceByMenu".into(), preferences.clone());
    }
    apply_registered_main_disk_fields(
        &mut out,
        disk.as_ref(),
        path,
        disk_gate_stage,
        terminal_snapshot_gate_mode,
        main_window_placement_mode,
    )?;

    // #1737: the effective view the caller must adopt for overlay-owned paths. `out`
    // is not a usable source: in Reconcile mode it was seeded from the disk object,
    // which already holds base values. Skipped entirely when no overlay is in force,
    // which is what keeps the no-overlay write path byte-identical.
    let effective: Option<Map<String, Value>> = if settings.local_overlay_state.is_empty() {
        None
    } else {
        Some(serialize_object()?)
    };
    // #1737: last writer over `out`, so no earlier stage can leak an overlay value
    // into the base file. Runs in both project write modes: Reconcile seeds `out`
    // from the live settings when the file is absent. The overlay-ineligible sets
    // (D7a, D7b) already exclude every key the #1077 and #1173 stages touch.
    if matches!(mode, ProjectWriteMode::Preserve) {
        write_identity_rows(&mut out, &settings.local_overlay_state);
    }
    settings.local_overlay_state.restore_base(&mut out);

    // #2716 (B3): the agents file is written first; only the keys it now holds
    // leave `out`, in both modes. A failure returns before the settings write.
    let agents_groups = save_agents_instance_file(
        settings,
        path,
        &mode,
        #[cfg(test)]
        disk.as_ref(),
        &out,
        disk_gate_stage,
        (
            prepared_duplicate_agents.as_ref(),
            prepared_profiles.as_ref().filter(|_| profiles_in_sidecar),
            Some(profiles_in_sidecar),
        ),
    )?;
    out = prepare_registered_main_agent_groups(
        out,
        settings,
        disk.as_ref(),
        &agents_groups,
        profiles_in_sidecar,
        (
            prepared_profiles.as_ref(),
            prepared_duplicate_agents.as_ref(),
            physical_sidecar.as_ref(),
            path,
        ),
        &mode,
    )
    .map_err(|reason| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    })?;
    let value = Value::Object(out);
    let written_bytes = write_value_atomic(&value, path)?;
    if let Some(previous) = previous_contents {
        if previous.as_bytes() != written_bytes.as_slice() {
            rotate_settings_backups(path, previous.as_bytes());
        }
    }

    let mut written_value = value;
    replace_project_projection(written_value.as_object_mut().unwrap(), &selected_projection);
    // #2716 (B3): the re-decode sees what a load would, the agents file's groups
    // in the base object, under the overlay reapplied below.
    let physical_groups = agents_groups.keys().cloned().collect::<Vec<_>>();
    let profile_owner = selected_registered_profile_owner(
        written_value.as_object().expect("written settings object"),
        Some(&agents_groups),
        !settings
            .local_overlay_state
            .owns_top_level("codingAgentProfiles"),
    );
    let profile_owner_view = registered_profiles_for_read(&profile_owner);
    let base_profiles = profile_owner.get("codingAgentProfiles").cloned();
    restore_agents_groups_for_redecode(&mut written_value, agents_groups);
    let identity_physical =
        coding_agent_identity_with_physical_overlay(written_value.clone(), path, &physical_groups);
    if let Some(profiles) = profile_owner_view.get("codingAgentProfiles") {
        written_value["codingAgentProfiles"] = profiles.clone();
    }
    // #1737: disk holds the base, memory holds the effective value. Without this the
    // caller would adopt base values for every overlay-owned key on the first save.
    if let (Some(effective), Value::Object(object)) = (&effective, &mut written_value) {
        settings.local_overlay_state.reapply_from(effective, object);
    }
    restore_codex_home_carrier(
        written_value.as_object_mut().unwrap(),
        base_profiles.as_ref(),
    );
    let fresh_state = apply_project_decode_to_value(
        &mut written_value,
        base.as_deref(),
        &projects::FsCandidateResolver,
    );
    let mut written: AppSettings = serde_json::from_value(written_value).map_err(|source| {
        let outward = format!("Failed to re-decode written settings: {source}");
        SettingsSaveError::json(
            SettingsSaveStage::ReDecode,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::ReDecode(outward),
        )
    })?;
    refresh_agent_id_hashes(&mut written.agents);
    refresh_pair_id_hashes(&written.agents, &mut written.coding_agent_profiles);
    written.project_path_state = Arc::new(fresh_state);
    written.coding_agent_identity_physical = Some(Arc::new(identity_physical));
    written.local_overlay_state = settings.local_overlay_state.clone();
    repair_loaded_response_close_settings(&mut written, &path.display().to_string());
    repair_loaded_room_idle_confirmation_settings(&mut written, &path.display().to_string());
    Ok(written)
}

fn prevalidate_registered_main_packet(
    owner: (&AppSettings, &Path),
    initial_disk: &Option<Map<String, Value>>,
    physical_sidecar: &Option<Map<String, Value>>,
    prepared_profiles: Option<&Value>,
    prepared_duplicate_agents: Option<&Value>,
    controls: (
        &ProjectWriteMode,
        SettingsSaveStage,
        TerminalSnapshotGateWriteMode,
        &MainWindowPlacementWriteMode,
        bool,
        bool,
    ),
) -> Result<(), SettingsSaveError> {
    let (settings, path) = owner;
    let (
        mode,
        disk_gate_stage,
        terminal_snapshot_gate_mode,
        main_window_placement_mode,
        directional_split,
        profiles_in_sidecar,
    ) = controls;
    let reject_profiles = |reason: String| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    if prepared_profiles.is_some()
        && (directional_split || settings.local_overlay_state.owns_top_level("agents"))
    {
        let mut main_packet = serde_json::to_value(settings)
            .map_err(|error| reject_profiles(error.to_string()))?
            .as_object()
            .ok_or_else(|| reject_profiles("invalidSettingsObject".into()))?
            .clone();
        apply_registered_main_disk_fields(
            &mut main_packet,
            initial_disk.as_ref(),
            path,
            disk_gate_stage,
            terminal_snapshot_gate_mode,
            main_window_placement_mode.clone(),
        )?;
        settings.local_overlay_state.restore_base(&mut main_packet);
        let mut sidecar_packet = physical_sidecar.clone().unwrap_or_default();
        if let Some(profiles) =
            prepared_profiles.and_then(|prepared| prepared.get("codingAgentProfiles"))
        {
            sidecar_packet.insert("codingAgentProfiles".into(), profiles.clone());
        }
        let main_packet = prepare_registered_main_agent_groups(
            main_packet,
            settings,
            initial_disk.as_ref(),
            &sidecar_packet,
            profiles_in_sidecar,
            (
                prepared_profiles,
                prepared_duplicate_agents,
                physical_sidecar.as_ref(),
                path,
            ),
            mode,
        )
        .map_err(&reject_profiles)?;
        if !settings.local_overlay_state.owns_top_level("agents") {
            validate_physical_identity_delta(
                &initial_disk.clone().unwrap_or_default(),
                &main_packet,
                true,
                false,
            )
            .map_err(&reject_profiles)?;
        }
    }
    Ok(())
}

fn backup_legacy_profiles_before_write(
    path: &Path,
    disk_gate_stage: SettingsSaveStage,
    mode: &ProjectWriteMode,
    map: Option<&Map<String, Value>>,
    contents: Option<&str>,
) -> Result<(), SettingsSaveError> {
    // #2015: Preserve builds from the struct, which ignores legacy pre-v2
    // profile fields; keep the original once before any save drops them.
    if matches!(mode, ProjectWriteMode::Preserve) {
        if let (Some(map), Some(contents)) = (map, contents) {
            if legacy_profiles_shape_present(map) {
                write_pre_384_v1_backup(path, contents).map_err(|e| {
                    SettingsSaveError::io(
                        disk_gate_stage,
                        path,
                        None,
                        std::io::Error::other(e.clone()),
                        SettingsSaveLegacyOutward::DiskRead(format!(
                            "Refusing to overwrite {}: legacy settings backup failed ({e})",
                            path.display()
                        )),
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn prepare_registered_profile_incoming(
    owner: (&AppSettings, &Path),
    profile_before: &Value,
    physical_sidecar: &Option<Map<String, Value>>,
    initial_disk: &Option<Map<String, Value>>,
    profile_gate: &[RegisteredProfileDocument],
    flags: (bool, bool, bool, bool),
    prepared_duplicate_agents: Option<&Value>,
) -> Result<(Value, bool, bool), String> {
    let (settings, path) = owner;
    let (directional_split, profiles_unprotected, profiles_in_sidecar, consolidate_legacy) = flags;
    let mut profile_incoming = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    validate_registered_overlay_profile_edits(
        settings,
        profile_before,
        physical_sidecar,
        initial_disk,
        path,
        profile_gate,
    )?;
    settings
        .local_overlay_state
        .restore_base(profile_incoming.as_object_mut().unwrap());
    // The directional view and initial preparation use selected physical
    // rows. Their borrowed rows are stripped before sidecar publication.
    let owner_has_active_agents = if directional_split
        || (profiles_unprotected
            && profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
                != Some(&Value::from(1)))
    {
        true
    } else if profiles_in_sidecar {
        physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("agents"))
    } else {
        !physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("agents"))
    };
    if !owner_has_active_agents {
        if profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
            == Some(&Value::from(1))
        {
            let selected_rows = physical_sidecar
                .as_ref()
                .and_then(|map| map.get("agents"))
                .or_else(|| initial_disk.as_ref().and_then(|map| map.get("agents")));
            validate_split_canonical_profile_edits(settings, profile_before, selected_rows)?;
        }
        match profile_before.get("agents") {
            Some(rows) => {
                profile_incoming["agents"] = rows.clone();
            }
            None => {
                profile_incoming.as_object_mut().unwrap().remove("agents");
            }
        }
    }
    if settings.local_overlay_state.owns_top_level("agents") {
        match profile_before.get("agents") {
            Some(rows) => profile_incoming["agents"] = rows.clone(),
            None => {
                profile_incoming.as_object_mut().unwrap().remove("agents");
            }
        }
    }
    if consolidate_legacy {
        retain_selected_legacy_profile_aliases(
            profile_before,
            profile_before,
            &mut profile_incoming,
        )?;
    }
    if owner_has_active_agents {
        if let Some(rows) = prepared_duplicate_agents {
            profile_incoming["agents"] = rows.clone();
        } else {
            retain_unknown_registered_agent_fields(profile_before, &mut profile_incoming)?;
        }
    }
    let mut incoming_ids = HashSet::new();
    let duplicate_rows = profile_incoming
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .any(|id| !incoming_ids.insert(id));
    if directional_split {
        restrict_split_profile_mutation_rows(settings, profile_before, &mut profile_incoming)?;
    }
    Ok((profile_incoming, owner_has_active_agents, duplicate_rows))
}

fn validate_directional_prepared_identity(
    profile_before: &Value,
    prepared: &Value,
) -> Result<(), String> {
    // A BASE fork carries its old complete set, including historical
    // duplicate profiles. Compare profile edits under the proposed
    // row grouping; configuration admission above stays independent.
    let mut previous = project_physical_profiles(profile_before, true)?;
    let old_rows = physical_profile_rows(profile_before, true)?;
    let mut target_baseline = profile_before.clone();
    target_baseline["agents"] = prepared["agents"].clone();
    let target_baseline = project_physical_profiles(&target_baseline, true)?;
    for id in physical_profile_rows(prepared, true)?.keys() {
        if !old_rows.contains_key(id) {
            for key in ["profilesByAgent", "profileLabelsByAgent"] {
                previous["codingAgentProfiles"][key][id] =
                    target_baseline["codingAgentProfiles"][key][id].clone();
            }
        }
    }
    previous["agents"] = prepared["agents"].clone();
    let proposed = project_physical_profiles(prepared, true)?;
    let old = physical_identity_violations(previous.as_object().unwrap(), false, true)?;
    let new = physical_identity_violations(proposed.as_object().unwrap(), false, true)?;
    for (signature, (count, error)) in new {
        if count > old.get(&signature).map_or(0, |(count, _)| *count) {
            return Err(error.to_string());
        }
    }
    Ok(())
}

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

fn read_pty_input_project_paths_strict_from_path(
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

fn read_terminal_snapshot_security_settings_strict_from_path(
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

fn compare_and_set_terminal_snapshots_enabled_at_path(
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

fn decode_disk_settings_for_terminal_snapshot_cas(
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

fn read_project_paths_from_disk(path: &Path) -> Result<Option<DiskProjectLists>, String> {
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

fn project_lists_from_projection(
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

/// Save unrelated settings under the shared settings lock. Migration completes
/// first; afterward the sidecar bytes are preserved exactly and all six project
/// keys are excluded from managed settings. The returned runtime view is decoded
/// from the authoritative sidecar, never from a renderer's project draft.
pub fn save_settings(settings: &AppSettings) -> Result<AppSettings, String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join(SETTINGS_FILE_NAME);
    save_settings_to_path_preserving_project_paths(settings, &path)
}

#[cfg(test)]
thread_local! {
    static PROJECT_MIGRATION_ATOMIC_FAULT: std::cell::RefCell<
        Option<(PathBuf, SettingsSaveStage)>,
    > = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn project_atomic_checkpoint(
    path: &Path,
    stage: SettingsSaveStage,
) -> Result<(), SettingsSaveError> {
    PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        if slot
            .borrow()
            .as_ref()
            .is_some_and(|(target, point)| target == path && *point == stage)
        {
            Err(project_migration_error(
                path,
                stage,
                "injected migration atomic failure",
            ))
        } else {
            Ok(())
        }
    })
}

/// #1077: atomic tmp+rename writer over an already-built JSON `Value`. Shared by
/// the raw and the project-aware writers. Preserves the #774 unique-temp +
/// `rename_with_retry` behavior. The temp IS fsynced
/// (`temporary.sync_all()`), and the parent directory is fsynced on unix after
/// the rename. The caller owns #2058 backup rotation of the returned bytes.
fn write_value_atomic(value: &Value, path: &Path) -> Result<Vec<u8>, SettingsSaveError> {
    write_value_atomic_labeled(value, path, path)
}

fn write_value_atomic_labeled(
    value: &Value,
    path: &Path,
    label: &Path,
) -> Result<Vec<u8>, SettingsSaveError> {
    use std::io::Write as _;

    let dir = path.parent().ok_or_else(|| {
        SettingsSaveError::semantic(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            SettingsSaveReason::MissingParentDirectory,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    std::fs::create_dir_all(dir).map_err(|source| {
        SettingsSaveError::io(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    crate::path_identity::verify_component_chain(dir).map_err(|_| {
        SettingsSaveError::semantic(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            SettingsSaveReason::TargetIdentityRejected,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::PrepareTarget,
                    path,
                    None,
                    SettingsSaveReason::TargetNotRegularFile,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                ));
            }
            crate::path_identity::verify_regular_file(path).map_err(|_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::PrepareTarget,
                    path,
                    None,
                    SettingsSaveReason::TargetIdentityRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(SettingsSaveError::io(
                SettingsSaveStage::PrepareTarget,
                path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ));
        }
    }

    let json = serde_json::to_vec_pretty(value).map_err(|source| {
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    if json.len() > 16 * 1024 * 1024 {
        return Err(SettingsSaveError::semantic(
            SettingsSaveStage::Serialize,
            path,
            None,
            SettingsSaveReason::SerializedPayloadTooLarge,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        ));
    }

    let op_id = SAVE_OP_ID.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    // Settings keep their existing instance-qualified temp prefix, including
    // callers supplying a fixture path. The fixed sidecar uses its own name.
    let temp_base = match path.file_name() {
        Some(name)
            if name == std::ffi::OsStr::new(super::instance_artifacts::PROJECT_PATHS_FILE_NAME) =>
        {
            name
        }
        _ => std::ffi::OsStr::new(SETTINGS_FILE_NAME),
    };
    let mut temp_name = temp_base.to_os_string();
    temp_name.push(format!(".{pid}.{op_id}.tmp"));
    let tmp_path = dir.join(temp_name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }

    let result: Result<(), SettingsSaveError> = (|| {
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempCreate)?;
        let mut temporary = options.open(&tmp_path).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempCreate,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempWrite)?;
        temporary.write_all(&json).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempWrite,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        temporary.flush().map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempFlush,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempSync)?;
        temporary.sync_all().map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempSync,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        crate::path_identity::verify_opened_regular_file(&tmp_path, &temporary, false).map_err(
            |_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::TempVerify,
                    path,
                    Some(&tmp_path),
                    SettingsSaveReason::TempIdentityRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            },
        )?;
        drop(temporary);

        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::AtomicReplace)?;
        replace_settings_file_atomic(&tmp_path, path).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::AtomicReplace,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
                |source| {
                    SettingsSaveError::io(
                        SettingsSaveStage::TargetPermissions,
                        path,
                        Some(&tmp_path),
                        source,
                        SettingsSaveLegacyOutward::SettingsSaveFailed,
                    )
                },
            )?;
            std::fs::File::open(dir)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| {
                    SettingsSaveError::io(
                        SettingsSaveStage::ParentDirectorySync,
                        path,
                        Some(&tmp_path),
                        source,
                        SettingsSaveLegacyOutward::SettingsSaveFailed,
                    )
                })?;
        }
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::PostWriteRead)?;
        let (written, _) = crate::path_identity::read_bounded_regular(path, 16 * 1024 * 1024)
            .map_err(|_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::PostWriteRead,
                    path,
                    Some(&tmp_path),
                    SettingsSaveReason::PostWriteReadRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
        if written != json {
            return Err(SettingsSaveError::semantic(
                SettingsSaveStage::PostWriteVerify,
                path,
                Some(&tmp_path),
                SettingsSaveReason::WrittenBytesMismatch,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ));
        }
        Ok(())
    })();
    if result.is_err() && tmp_path.exists() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result?;

    log::debug!("Saved settings to {:?}", label);
    Ok(json)
}

#[cfg(windows)]
fn replace_settings_file_atomic_with_retry<Replace, Sleep>(
    replace: Replace,
    sleep: Sleep,
) -> std::io::Result<()>
where
    Replace: FnMut() -> std::io::Result<()>,
    Sleep: FnMut(std::time::Duration),
{
    crate::config::retry_transient_io_with(
        replace,
        sleep,
        |failed_attempt, total_attempts, error, delay| {
            if let Some(delay) = delay {
                let raw_os_error = error.raw_os_error().unwrap_or_default();
                log::warn!(
                    "Windows settings atomic replace attempt {} of {} failed with raw OS error {}; \
                        retrying in {} ms",
                    failed_attempt,
                    total_attempts,
                    raw_os_error,
                    delay.as_millis()
                );
            }
        },
    )
    .map_err(|failure| failure.error)
}

#[cfg(windows)]
fn replace_settings_file_atomic(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    replace_settings_file_atomic_with_retry(
        || {
            let result = unsafe {
                MoveFileExW(
                    source.as_ptr(),
                    destination.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            };
            if result == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        },
        std::thread::sleep,
    )
}

#[cfg(not(windows))]
fn replace_settings_file_atomic(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

/// Snapshot fixture/migration writer only. Production operations own a token
/// spanning fresh reads, mutation, sidecar commit, revalidation and rollback.
#[cfg(test)]
pub fn save_settings_with_project_paths(settings: &AppSettings) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join(SETTINGS_FILE_NAME);
    save_settings_with_project_paths_to_path(settings, &path)
}

#[cfg(test)]
pub(crate) fn save_settings_with_project_paths_to_path(
    settings: &AppSettings,
    path: &Path,
) -> Result<(), String> {
    reconcile_project_state_to_path(settings, path, true, true).map(|_| ())
}

/// #778/#881 + #1077: the preserve-disk wrapper behind the default
/// `save_settings`. Preserves all six on-disk project fields (materializing a
/// group only when disk has no truth), then hands off to the #774-hardened
/// atomic writer and returns the fresh-decoded settings.
pub(crate) fn save_settings_to_path_preserving_project_paths(
    settings: &AppSettings,
    path: &Path,
) -> Result<AppSettings, String> {
    save_settings_to_path_preserving_project_paths_typed(settings, path).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })
}

fn save_settings_to_path_preserving_project_paths_typed(
    settings: &AppSettings,
    path: &Path,
) -> Result<AppSettings, SettingsSaveError> {
    save_settings_value(settings, path, ProjectWriteMode::Preserve)
}

/// Raw writer: serialize `AppSettings` verbatim (three primary project fields;
/// the companion fields are `#[serde(skip)]`, yielding a legacy-shaped file).
/// Used only for test seeding; production writes go through the preserve/
/// reconcile modes so companions are materialized.
#[cfg(test)]
fn save_settings_to_path(settings: &AppSettings, path: &Path) -> Result<(), SettingsSaveError> {
    let value = serde_json::to_value(settings).map_err(|source| {
        let outward = format!("Failed to serialize settings: {}", source);
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::Serialize(outward),
        )
    })?;
    write_value_atomic(&value, path).map(|_| ())
}

// ---------------------------------------------------------------------------
// #2994 (E2992 P41) - the settings owner
// ---------------------------------------------------------------------------
//
// One authoritative `AppSettings` lives in the owner's `RwLock`. Until P50 the
// product runs the owner in `StagedCompatibility`: bootstrap, CLI and persistence
// keep their current paths through the forwarding methods below, and managed
// operations stay disabled. `Active` owners hold the lifecycle lease and run the
// complete operation contract; only tests construct them before P46.

/// Lifecycle availability of a settings owner. Owner metadata, changed only under
/// its control mutex. `Invalid` is sticky: it denies current reads, mutation and
/// authorization, and leaves only diagnostics and a validated close or restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsAvailability {
    Starting,
    Ready,
    Invalid,
    Draining,
    Closed,
}

/// Errors of the owner contract. Messages carry stages and reason codes only,
/// never payloads, tokens or paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOwnerError {
    /// The owner is not `Ready`, or admission is sealed.
    Unavailable(SettingsAvailability),
    /// A try-acquisition found the state lock held.
    Busy,
    /// Managed operations are not enabled on this owner (StagedCompatibility).
    OperationsNotEnabled,
    /// The lifecycle lock is held by another owner past the wait deadline.
    LifecycleTimedOut,
    /// The lifecycle lock or its directory identity could not be established.
    /// Never evidence that the authority is free.
    Lifecycle(String),
    /// The transactional lock could not be taken for an operation.
    TransactionLock(String),
    /// A source could not be captured; IO, identity or malformation, never absence.
    Capture(String),
    /// The captured sources do not compose under the requested policy.
    Compose(String),
    /// The blocking worker could not be joined.
    Worker(String),
}

impl std::fmt::Display for SettingsOwnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(availability) => {
                write!(formatter, "settings owner unavailable ({availability:?})")
            }
            Self::Busy => formatter.write_str("settings owner state lock is busy"),
            Self::OperationsNotEnabled => {
                formatter.write_str("settings owner managed operations are not enabled")
            }
            Self::LifecycleTimedOut => {
                formatter.write_str("settings owner lifecycle lock is held by another owner")
            }
            Self::Lifecycle(reason) => write!(formatter, "settings owner lifecycle: {reason}"),
            Self::TransactionLock(reason) => {
                write!(formatter, "settings owner transaction lock: {reason}")
            }
            Self::Capture(reason) => write!(formatter, "settings owner capture: {reason}"),
            Self::Compose(reason) => write!(formatter, "settings owner composition: {reason}"),
            Self::Worker(reason) => write!(formatter, "settings owner worker: {reason}"),
        }
    }
}

impl std::error::Error for SettingsOwnerError {}

/// Stage and reason code of a settings save error, without its paths.
fn owner_save_error_text(error: &SettingsSaveError) -> String {
    let reason = match &error.cause {
        SettingsSaveCause::Io(source) => io_error_kind_as_str(source.kind()),
        SettingsSaveCause::Json(source) => json_error_category_as_str(source.classify()),
        SettingsSaveCause::Semantic(reason) => reason.as_str(),
    };
    format!("{error} ({:?}: {reason})", error.stage)
}

fn owner_transaction_error(error: SettingsSaveError) -> SettingsOwnerError {
    SettingsOwnerError::TransactionLock(owner_save_error_text(&error))
}

/// Identity the owner reports upward: no transport, no `AppHandle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsOwnerIdentity {
    pub instance_id: uuid::Uuid,
    pub epoch: u64,
    pub availability: SettingsAvailability,
    pub managed_operations_enabled: bool,
}

/// Project retention captured from RAM. A `source_error` means the lists are
/// unknown, never empty: no consumer may purge on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRetention {
    pub active: Vec<String>,
    pub archived: Vec<String>,
    pub source_error: Option<String>,
}

/// Whether an operation may already have touched a persistent source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationEffect {
    None,
    Possible,
}

/// How an operation ended. `PartialOrUnknown` is an honest error: never a
/// success and never a rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationOutcome {
    Committed,
    Rejected,
    PartialOrUnknown,
}

/// The publishers a locked operation may run. Each one flips the effect to
/// `Possible` before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPublisher {
    PreflightBackup,
    ProjectPathsMigration,
    ProjectPathsBackup,
    ProjectPathsCleanup,
    BlockingMenusExport,
    AgentsExport,
    AgentsSave,
    SettingsSave,
    AuxCache,
    Stamp,
}

/// Composition policy. Startup keeps overlay and agents rejections as metadata;
/// recovery requires every affected source to be valid. Both keep legitimate
/// absence and neither turns IO or a base decode failure into defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePolicy {
    Startup,
    Recovery,
}

/// The result an operation returns once the owner has adopted (or refused) the
/// effective reality.
#[derive(Debug)]
pub struct SettingsOperationReport<T> {
    pub outcome: OperationOutcome,
    pub value: Option<T>,
    pub error: Option<String>,
    pub epoch: u64,
    pub availability: SettingsAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsOwnerStage {
    StagedCompatibility,
    Active,
}

#[derive(Debug)]
struct SettingsOwnerControl {
    availability: SettingsAvailability,
    epoch: u64,
    permits: usize,
    operations: usize,
    sealed: bool,
    managed_operations_enabled: bool,
}

/// Counters for tests and DEBUG diagnostics; no payloads.
#[derive(Debug, Default)]
struct SettingsOwnerCounters {
    transaction_lock_attempts: AtomicU64,
    captures: AtomicU64,
    source_reads: AtomicU64,
    operations: AtomicU64,
}

/// The lifecycle lease: an exclusive lock on the stable owner sidecar inside a
/// retained physical directory. Dropping it unlocks; the sidecar is never
/// unlinked.
struct SettingsOwnerLease {
    _lock: SettingsFileLock,
    retained: crate::path_identity::RetainedDirectory,
    settings_path: PathBuf,
}

#[cfg(all(test, windows))]
struct SettingsOwnerLeaseVerifyFault {
    path: PathBuf,
    armed: bool,
    hits: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(all(test, windows))]
thread_local! {
    static SETTINGS_OWNER_LEASE_VERIFY_FAULT: std::cell::RefCell<
        Option<SettingsOwnerLeaseVerifyFault>,
    > = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, windows))]
struct SettingsOwnerLeaseVerifyFaultGuard {
    path: PathBuf,
    hits: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(all(test, windows))]
impl SettingsOwnerLeaseVerifyFaultGuard {
    fn install(path: PathBuf) -> Self {
        let hits = std::rc::Rc::new(std::cell::Cell::new(0));
        SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(slot.is_none(), "lease verification faults cannot nest");
            *slot = Some(SettingsOwnerLeaseVerifyFault {
                path: path.clone(),
                armed: true,
                hits: std::rc::Rc::clone(&hits),
            });
        });
        Self { path, hits }
    }
}

#[cfg(all(test, windows))]
impl Drop for SettingsOwnerLeaseVerifyFaultGuard {
    fn drop(&mut self) {
        SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|fault| {
                fault.path == self.path && std::rc::Rc::ptr_eq(&fault.hits, &self.hits)
            }) {
                *slot = None;
            }
        });
    }
}

impl SettingsOwnerLease {
    fn acquire(
        settings_path: &Path,
        wait: std::time::Duration,
    ) -> Result<Self, SettingsOwnerError> {
        let identity = |reason: &str| SettingsOwnerError::Lifecycle(reason.to_string());
        let parent = settings_path
            .parent()
            .ok_or_else(|| identity("missing_parent_directory"))?;
        let leaf = settings_path
            .file_name()
            .ok_or_else(|| identity("missing_file_name"))?;
        // The chain check stops at the first link, so a `..` after a tolerated
        // link (macOS `/var`) would otherwise be resolved by canonicalize.
        if parent
            .components()
            .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(identity("lock_identity_rejected"));
        }
        match crate::path_identity::verify_component_chain_reason(parent) {
            Ok(()) | Err(crate::path_identity::PathRule::ComponentIsLink) => {}
            Err(_) => return Err(identity("lock_identity_rejected")),
        }
        let physical_parent =
            std::fs::canonicalize(parent).map_err(|_| identity("lock_identity_rejected"))?;
        let retained = crate::path_identity::retain_directory(&physical_parent)
            .map_err(|_| identity("lock_identity_rejected"))?;
        let physical = retained.identity().canonical_path.join(leaf);
        let lock = SettingsFileLock::acquire_named(
            &physical,
            SETTINGS_OWNER_LOCK_FILE_NAME,
            wait,
            "settings_owner_lifecycle",
        )
        .map_err(|error| match error.cause {
            SettingsSaveCause::Semantic(SettingsSaveReason::LockTimedOut) => {
                SettingsOwnerError::LifecycleTimedOut
            }
            _ => SettingsOwnerError::Lifecycle(owner_save_error_text(&error)),
        })?;
        retained
            .verify_current()
            .map_err(|_| identity("lock_identity_rejected"))?;
        Ok(Self {
            _lock: lock,
            retained,
            settings_path: physical,
        })
    }

    fn verify(&self) -> Result<(), SettingsOwnerError> {
        #[cfg(all(test, windows))]
        if SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(fault) = slot.as_mut() else {
                return false;
            };
            if fault.armed && fault.path == self.settings_path {
                fault.armed = false;
                fault.hits.set(fault.hits.get() + 1);
                true
            } else {
                false
            }
        }) {
            return Err(SettingsOwnerError::Lifecycle(
                "lock_identity_rejected".to_string(),
            ));
        }
        self.retained
            .verify_current()
            .map_err(|_| SettingsOwnerError::Lifecycle("lock_identity_rejected".to_string()))
    }
}

/// The single settings owner. `SettingsState` is `Arc<SettingsOwner>`.
pub struct SettingsOwner {
    settings: Arc<RwLock<AppSettings>>,
    stage: SettingsOwnerStage,
    instance_id: uuid::Uuid,
    control: std::sync::Mutex<SettingsOwnerControl>,
    released: std::sync::Condvar,
    lease: std::sync::Mutex<Option<SettingsOwnerLease>>,
    counters: SettingsOwnerCounters,
}

impl std::fmt::Debug for SettingsOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SettingsOwner")
            .field("stage", &self.stage)
            .field("instance_id", &self.instance_id)
            .field("control", &*self.control())
            .finish_non_exhaustive()
    }
}

/// Admission of one read permit; releasing it wakes a draining owner.
struct SettingsPermitAdmission<'a> {
    owner: &'a SettingsOwner,
}

impl Drop for SettingsPermitAdmission<'_> {
    fn drop(&mut self) {
        self.owner.release_permit();
    }
}

/// Admission of one managed operation. It is released on every path, unwinding
/// included, which is what lets a drain terminate.
struct SettingsOperationAdmission<'a> {
    owner: &'a SettingsOwner,
}

impl Drop for SettingsOperationAdmission<'_> {
    fn drop(&mut self) {
        self.owner.finish_operation();
    }
}

/// Outcome, builder value and error of one classified operation.
type OperationParts<T> = (OperationOutcome, Option<T>, Option<String>);

/// A borrowed, validated read of the current settings.
pub struct SettingsReadGuard<'a> {
    guard: tokio::sync::RwLockReadGuard<'a, AppSettings>,
    _admission: SettingsPermitAdmission<'a>,
}

impl std::ops::Deref for SettingsReadGuard<'_> {
    type Target = AppSettings;

    fn deref(&self) -> &AppSettings {
        &self.guard
    }
}

/// An owned, validated read permit: the state guard plus the owner it belongs to.
/// A PTY route write keeps it until the write ends.
pub struct SettingsReadPermit {
    guard: Option<tokio::sync::OwnedRwLockReadGuard<AppSettings>>,
    owner: Arc<SettingsOwner>,
}

impl SettingsReadPermit {
    pub fn owner(&self) -> &Arc<SettingsOwner> {
        &self.owner
    }
}

impl std::ops::Deref for SettingsReadPermit {
    type Target = AppSettings;

    fn deref(&self) -> &AppSettings {
        self.guard
            .as_deref()
            .expect("a settings read permit holds its guard until drop")
    }
}

impl Drop for SettingsReadPermit {
    fn drop(&mut self) {
        drop(self.guard.take());
        self.owner.release_permit();
    }
}

/// Proof that an operation holds the owner's lifecycle authority and the current
/// transactional lock. Constructed only inside the owner; the locked halves take
/// it instead of reacquiring the transactional lock.
pub struct SettingsOperationGuard<'a> {
    lease: &'a SettingsOwnerLease,
    transaction: ProjectPathsWriteTransaction,
    counters: &'a SettingsOwnerCounters,
    effect: OperationEffect,
}

impl SettingsOperationGuard<'_> {
    /// The physical settings path under the retained directory.
    pub fn settings_path(&self) -> &Path {
        &self.transaction.path
    }

    pub fn effect(&self) -> OperationEffect {
        self.effect
    }

    pub(crate) fn verify_agent_file_context(&self, revision: &str) -> Result<(), String> {
        let captured = capture_sources_locked(self).map_err(|_| "sourceContextUnavailable")?;
        let fresh = compose_captured_sources(&captured, CapturePolicy::Recovery)
            .map_err(|_| "sourceContextUnavailable")?;
        let context = SourceReadContext::from_settings(&fresh, self.settings_path())
            .map_err(|_| "sourceContextUnavailable")?;
        if context.revision != revision {
            return Err("staleSourceContext".into());
        }
        Ok(())
    }

    /// Runs one publisher. The effect becomes `Possible` BEFORE the publisher
    /// starts, so a failure from here on classifies as `PartialOrUnknown`.
    pub fn publish<R>(
        &mut self,
        publisher: SettingsPublisher,
        run: impl FnOnce(&Path) -> Result<R, String>,
    ) -> Result<R, String> {
        self.effect = OperationEffect::Possible;
        log::debug!("[settings-owner] publisher start: {publisher:?}");
        let result = run(&self.transaction.path);
        log::debug!(
            "[settings-owner] publisher end: {publisher:?} ok={}",
            result.is_ok()
        );
        result
    }
}

/// One captured source: verified absence, or its bytes with the identity witness
/// of the object they were read from.
#[derive(Debug, Clone)]
pub(crate) enum CapturedSource {
    Absent,
    Present {
        bytes: Vec<u8>,
        witness: crate::path_identity::VerifiedPathIdentity,
    },
}

impl CapturedSource {
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Absent => None,
            Self::Present { bytes, .. } => Some(bytes),
        }
    }

    /// A short, non-reversible witness label for DEBUG diagnostics.
    fn witness_label(&self) -> String {
        match self {
            Self::Absent => "absent".to_string(),
            Self::Present { witness, .. } => match witness.content_sha256 {
                Some(digest) => digest[..4]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                None => format!("len{}", witness.metadata.len),
            },
        }
    }
}

/// The four settings sources, captured once under the transactional lock.
#[derive(Debug, Clone)]
pub(crate) struct CapturedSettingsSources {
    settings_path: PathBuf,
    base: CapturedSource,
    project_paths: CapturedSource,
    agents: CapturedSource,
    overlay: CapturedSource,
}

const SETTINGS_CAPTURE_LIMIT: usize = 16 * 1024 * 1024;

fn capture_source_file(
    path: &Path,
    counters: &SettingsOwnerCounters,
) -> Result<CapturedSource, SettingsOwnerError> {
    counters.source_reads.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CapturedSource::Absent);
        }
        Err(error) => {
            let kind = io_error_kind_as_str(error.kind());
            return Err(SettingsOwnerError::Capture(format!("{name}: {kind}")));
        }
    }
    // A source that vanishes after the metadata probe is a race, not absence.
    crate::path_identity::read_bounded_regular(path, SETTINGS_CAPTURE_LIMIT)
        .map(|(bytes, witness)| CapturedSource::Present { bytes, witness })
        .map_err(|reason| SettingsOwnerError::Capture(format!("{name}: {reason}")))
}

// Test seam: the next owner capture on this thread panics, so the panic is
// raised outside the builder while the operation's guards are held.
#[cfg(test)]
thread_local! {
    static OWNER_CAPTURE_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Captures base, project-paths, agents and overlay exactly once while the guard
/// holds the transactional lock. No directory creation, migration or write.
fn capture_sources_locked(
    guard: &SettingsOperationGuard<'_>,
) -> Result<CapturedSettingsSources, SettingsOwnerError> {
    guard.counters.captures.fetch_add(1, Ordering::Relaxed);
    #[cfg(test)]
    OWNER_CAPTURE_PANIC.with(|armed| {
        if armed.replace(false) {
            panic!("injected settings owner capture panic");
        }
    });
    let path = guard.settings_path();
    let captured = CapturedSettingsSources {
        settings_path: path.to_path_buf(),
        base: capture_source_file(path, guard.counters)?,
        project_paths: capture_source_file(&project_paths_path(path), guard.counters)?,
        agents: capture_source_file(&agents_instance_path(path), guard.counters)?,
        overlay: capture_source_file(
            &path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            guard.counters,
        )?,
    };
    guard.lease.verify()?;
    log::debug!(
        "[settings-owner] captured base={} project={} agents={} overlay={}",
        captured.base.witness_label(),
        captured.project_paths.witness_label(),
        captured.agents.witness_label(),
        captured.overlay.witness_label()
    );
    Ok(captured)
}

fn recover_composition(
    captured: &CapturedSettingsSources,
) -> Result<AppSettings, SettingsOwnerError> {
    compose_captured_sources(captured, CapturePolicy::Recovery)
}

/// Composes captured bytes through the same selector and parser core the loaders
/// use. No second read, no defaults for a present source that fails.
fn compose_captured_sources(
    captured: &CapturedSettingsSources,
    policy: CapturePolicy,
) -> Result<AppSettings, SettingsOwnerError> {
    let path = captured.settings_path.as_path();
    let destination_path = project_paths_path(path);
    let selection_error = |error: SettingsSaveError| {
        SettingsOwnerError::Compose(format!("selection: {}", owner_save_error_text(&error)))
    };
    let raw = |source: &CapturedSource, label: &Path, legacy: bool| {
        source
            .bytes()
            .map(|bytes| raw_project_source_from_bytes(bytes.to_vec(), label, label, legacy))
            .transpose()
    };
    let legacy = raw(&captured.base, path, true).map_err(selection_error)?;
    let destination = raw(&captured.project_paths, &destination_path, false);
    let destination = destination.map_err(selection_error)?;
    let selected = select_project_source_from_raw(
        legacy,
        destination,
        path,
        &destination_path,
        &destination_path,
    )
    .map_err(selection_error)?;
    let agents_label = agents_instance_path(path);
    let agents = match captured.agents.bytes() {
        None => Ok(None),
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| format!("{} is not valid UTF-8", agents_label.display()))
            .and_then(|contents| parse_agents_instance_contents(contents, &agents_label))
            .map(Some),
    };
    if policy == CapturePolicy::Recovery && agents.is_err() {
        let reason = "agents source is invalid during recovery";
        return Err(SettingsOwnerError::Compose(reason.to_string()));
    }
    let source = path.to_string_lossy();
    // Only a verified-absent base composes over serialized defaults.
    let value = match &selected.legacy {
        Some(legacy) => Value::Object(legacy.object.clone()),
        None => serde_json::to_value(AppSettings::default())
            .map_err(|error| SettingsOwnerError::Compose(format!("defaults: {error}")))?,
    };
    let overlay_bytes = captured.overlay.bytes();
    let (mut settings, _legacy_profiles, agents_layer) = compose_settings_parts(
        value,
        &source,
        Some(path),
        Some(&selected.projection),
        Some(agents),
        |value| match overlay_bytes {
            Some(bytes) => LocalSettingsOverlay::from_overlay_bytes_paired(
                value,
                bytes,
                OVERLAY_INELIGIBLE_DISK_KEYS,
                OVERLAY_INELIGIBLE_LEGACY_KEYS,
                OVERLAY_DERIVED_ID_CLOSURES,
                CONFIGURATION_COMMAND_PAIRS,
            ),
            None => LocalSettingsOverlay::default(),
        },
        None,
    )
    .map_err(|_| SettingsOwnerError::Compose("base settings do not decode".to_string()))?;
    let overlay_rejected = settings.local_overlay_state.rejection().is_some();
    if policy == CapturePolicy::Recovery && overlay_rejected {
        let reason = "overlay rejected during recovery";
        return Err(SettingsOwnerError::Compose(reason.to_string()));
    }
    if let Some(state) = agents_layer {
        settings.agents_layer = state;
    }
    repair_loaded_response_close_settings(&mut settings, &source);
    repair_loaded_room_idle_confirmation_settings(&mut settings, &source);
    Ok(settings)
}

impl SettingsOwner {
    /// StagedCompatibility owner around already-loaded settings: the P40
    /// constructor path. Ready, with managed operations disabled.
    fn staged(settings: AppSettings) -> Self {
        Self::with_stage(
            settings,
            SettingsOwnerStage::StagedCompatibility,
            SettingsAvailability::Ready,
            None,
        )
    }

    fn with_stage(
        settings: AppSettings,
        stage: SettingsOwnerStage,
        availability: SettingsAvailability,
        lease: Option<SettingsOwnerLease>,
    ) -> Self {
        Self {
            settings: Arc::new(RwLock::new(settings)),
            stage,
            instance_id: uuid::Uuid::new_v4(),
            control: std::sync::Mutex::new(SettingsOwnerControl {
                availability,
                epoch: 0,
                permits: 0,
                operations: 0,
                sealed: false,
                managed_operations_enabled: stage == SettingsOwnerStage::Active,
            }),
            released: std::sync::Condvar::new(),
            lease: std::sync::Mutex::new(lease),
            counters: SettingsOwnerCounters::default(),
        }
    }

    /// Opens an Active owner: lifecycle authority first, then the transactional
    /// lock, one capture and a Startup composition before `Ready`. Another owner
    /// holding the authority makes this wait up to `wait`, then fail.
    pub fn open_active(
        settings_path: &Path,
        wait: std::time::Duration,
    ) -> Result<Arc<Self>, SettingsOwnerError> {
        let started = std::time::Instant::now();
        let lease = SettingsOwnerLease::acquire(settings_path, wait)?;
        let mut owner = Self::with_stage(
            AppSettings::default(),
            SettingsOwnerStage::Active,
            SettingsAvailability::Starting,
            Some(lease),
        );
        let initial = {
            let lease = owner.lease_guard();
            let lease = lease.as_ref().ok_or(SettingsOwnerError::Unavailable(
                SettingsAvailability::Closed,
            ))?;
            let guard = owner.operation_guard(lease)?;
            let captured = capture_sources_locked(&guard)?;
            compose_captured_sources(&captured, CapturePolicy::Startup)?
        };
        // Not shared yet: the initial value replaces the placeholder without a lock.
        owner.settings = Arc::new(RwLock::new(initial));
        {
            let mut control = owner.control();
            control.availability = SettingsAvailability::Ready;
            control.epoch = 1;
        }
        log::debug!(
            "[settings-owner] {} ready in {:?}",
            owner.instance_id,
            started.elapsed()
        );
        Ok(Arc::new(owner))
    }

    fn control(&self) -> std::sync::MutexGuard<'_, SettingsOwnerControl> {
        self.control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lease_guard(&self) -> std::sync::MutexGuard<'_, Option<SettingsOwnerLease>> {
        self.lease
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn identity(&self) -> SettingsOwnerIdentity {
        let control = self.control();
        SettingsOwnerIdentity {
            instance_id: self.instance_id,
            epoch: control.epoch,
            availability: control.availability,
            managed_operations_enabled: control.managed_operations_enabled,
        }
    }

    pub fn availability(&self) -> SettingsAvailability {
        self.control().availability
    }

    fn admit_permit(&self) -> Result<SettingsPermitAdmission<'_>, SettingsOwnerError> {
        let mut control = self.control();
        if control.sealed || control.availability != SettingsAvailability::Ready {
            return Err(SettingsOwnerError::Unavailable(control.availability));
        }
        control.permits += 1;
        Ok(SettingsPermitAdmission { owner: self })
    }

    fn release_permit(&self) {
        let mut control = self.control();
        control.permits = control.permits.saturating_sub(1);
        if control.permits == 0 {
            self.released.notify_all();
        }
    }

    fn revalidate(&self) -> Result<(), SettingsOwnerError> {
        let control = self.control();
        if control.availability == SettingsAvailability::Ready {
            Ok(())
        } else {
            Err(SettingsOwnerError::Unavailable(control.availability))
        }
    }

    fn mark_invalid(&self, stage: &str) {
        let mut control = self.control();
        if !matches!(
            control.availability,
            SettingsAvailability::Draining | SettingsAvailability::Closed
        ) {
            control.availability = SettingsAvailability::Invalid;
        }
        let epoch = control.epoch;
        log::debug!(
            "[settings-owner] {} invalid at {stage} epoch={epoch}",
            self.instance_id
        );
    }

    /// A borrowed read validated against availability on admission and again
    /// once the state lock is held.
    pub async fn current_read(&self) -> Result<SettingsReadGuard<'_>, SettingsOwnerError> {
        let admission = self.admit_permit()?;
        let guard = self.settings.read().await;
        self.revalidate()?;
        Ok(SettingsReadGuard {
            guard,
            _admission: admission,
        })
    }

    pub fn try_current_read(&self) -> Result<SettingsReadGuard<'_>, SettingsOwnerError> {
        let admission = self.admit_permit()?;
        let guard = self
            .settings
            .try_read()
            .map_err(|_| SettingsOwnerError::Busy)?;
        self.revalidate()?;
        Ok(SettingsReadGuard {
            guard,
            _admission: admission,
        })
    }

    pub fn blocking_current_read(&self) -> Result<SettingsReadGuard<'_>, SettingsOwnerError> {
        let admission = self.admit_permit()?;
        let guard = self.settings.blocking_read();
        self.revalidate()?;
        Ok(SettingsReadGuard {
            guard,
            _admission: admission,
        })
    }

    /// An owned permit holding the state guard and this owner.
    pub fn try_current_read_owned(
        self: &Arc<Self>,
    ) -> Result<SettingsReadPermit, SettingsOwnerError> {
        let admission = self.admit_permit()?;
        let guard = Arc::clone(&self.settings)
            .try_read_owned()
            .map_err(|_| SettingsOwnerError::Busy)?;
        // The permit now owns the admission and releases it on drop.
        std::mem::forget(admission);
        let permit = SettingsReadPermit {
            guard: Some(guard),
            owner: Arc::clone(self),
        };
        self.revalidate()?;
        Ok(permit)
    }

    /// Clones RAM through a validated guard: no filesystem, no transactional lock.
    pub async fn current_snapshot(&self) -> Result<AppSettings, SettingsOwnerError> {
        self.current_read().await.map(|guard| guard.clone())
    }

    pub fn blocking_current_snapshot(&self) -> Result<AppSettings, SettingsOwnerError> {
        self.blocking_current_read().map(|guard| guard.clone())
    }

    fn retention_of(settings: &AppSettings) -> SettingsRetention {
        SettingsRetention {
            active: settings.project_paths.clone(),
            archived: settings.archived_project_paths.clone(),
            source_error: settings.project_paths_persistence_error.clone(),
        }
    }

    /// Retention from RAM, captured before any sessions lock is taken.
    pub async fn current_retention(&self) -> Result<SettingsRetention, SettingsOwnerError> {
        self.current_read()
            .await
            .map(|guard| Self::retention_of(&guard))
    }

    pub fn blocking_current_retention(&self) -> Result<SettingsRetention, SettingsOwnerError> {
        self.blocking_current_read()
            .map(|guard| Self::retention_of(&guard))
    }

    fn operation_guard<'a>(
        &'a self,
        lease: &'a SettingsOwnerLease,
    ) -> Result<SettingsOperationGuard<'a>, SettingsOwnerError> {
        lease.verify()?;
        let attempts = &self.counters.transaction_lock_attempts;
        attempts.fetch_add(1, Ordering::Relaxed);
        let operation = "settings_owner_operation";
        let transaction = empty_project_transaction(&lease.settings_path, operation);
        let transaction = transaction.map_err(owner_transaction_error)?;
        Ok(SettingsOperationGuard {
            lease,
            transaction,
            counters: &self.counters,
            effect: OperationEffect::None,
        })
    }

    fn admit_operation(&self) -> Result<SettingsOperationAdmission<'_>, SettingsOwnerError> {
        let mut control = self.control();
        if !control.managed_operations_enabled {
            return Err(SettingsOwnerError::OperationsNotEnabled);
        }
        if control.sealed || control.availability != SettingsAvailability::Ready {
            return Err(SettingsOwnerError::Unavailable(control.availability));
        }
        control.operations += 1;
        Ok(SettingsOperationAdmission { owner: self })
    }

    fn finish_operation(&self) {
        let mut control = self.control();
        control.operations = control.operations.saturating_sub(1);
        if control.operations == 0 {
            self.released.notify_all();
        }
    }

    /// Runs one complete synchronous operation: admission, state guard, current
    /// transactional lock, builder over the captured current settings, publishers,
    /// adoption of the effective reality, then release. Host effects belong to
    /// the caller, after this returns. Never nests a runtime.
    ///
    /// The admission is released on every path, panics included, so a drain
    /// always terminates. A panic before the guards are held published nothing
    /// and is a worker error; a panic while they are held is classified under
    /// them (see `run_operation`).
    pub fn with_operation_blocking<T>(
        &self,
        name: &'static str,
        builder: impl FnOnce(&AppSettings, &mut SettingsOperationGuard<'_>) -> Result<T, String>,
    ) -> Result<SettingsOperationReport<T>, SettingsOwnerError> {
        let _admission = self.admit_operation()?;
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_operation(name, builder)
        }));
        match run {
            Ok(result) => result,
            Err(_) => {
                let reason = "operation panicked before its guards were held";
                Err(SettingsOwnerError::Worker(reason.to_string()))
            }
        }
    }

    /// The async form: the same body on a blocking worker that retains its
    /// owner handle. The future must not be awaited while holding any guard.
    pub async fn with_operation<T, F>(
        self: &Arc<Self>,
        name: &'static str,
        builder: F,
    ) -> Result<SettingsOperationReport<T>, SettingsOwnerError>
    where
        T: Send + 'static,
        F: FnOnce(&AppSettings, &mut SettingsOperationGuard<'_>) -> Result<T, String>
            + Send
            + 'static,
    {
        let owner = Arc::clone(self);
        tokio::task::spawn_blocking(move || owner.with_operation_blocking(name, builder))
            .await
            .map_err(|error| SettingsOwnerError::Worker(error.to_string()))?
    }

    fn run_operation<T>(
        &self,
        name: &'static str,
        builder: impl FnOnce(&AppSettings, &mut SettingsOperationGuard<'_>) -> Result<T, String>,
    ) -> Result<SettingsOperationReport<T>, SettingsOwnerError> {
        let operation = self.counters.operations.fetch_add(1, Ordering::Relaxed) + 1;
        let started = std::time::Instant::now();
        let mut state = self.settings.blocking_write();
        let lease = self.lease_guard();
        let lease = lease.as_ref().ok_or(SettingsOwnerError::Unavailable(
            SettingsAvailability::Closed,
        ))?;
        let mut guard = self.operation_guard(lease)?;
        // Everything that reads sources, runs the builder or adopts reality runs
        // under this catch, so a panic anywhere is classified while the state
        // guard, the lease and the transactional lock are all still held.
        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.operation_body(&mut state, &mut guard, builder)
        }));
        let effect = guard.effect;
        let (outcome, value, error) = match body {
            Ok(Ok(parts)) => parts,
            Ok(Err(error)) => return Err(error),
            Err(_) if effect == OperationEffect::None => {
                // No publisher ran and RAM was not replaced: nothing to recover.
                let reason = "operation panicked before any publisher";
                (OperationOutcome::Rejected, None, Some(reason.to_string()))
            }
            Err(_) => {
                // A source may have changed and reality could not be adopted.
                self.mark_invalid("panic");
                let reason = "operation panicked after a publisher";
                (
                    OperationOutcome::PartialOrUnknown,
                    None,
                    Some(reason.to_string()),
                )
            }
        };
        drop(guard);
        drop(state);
        let control = self.control();
        log::debug!(
            "[settings-owner] {} op={operation} {name} {outcome:?}/{effect:?} epoch={} in {:?}",
            self.instance_id,
            control.epoch,
            started.elapsed()
        );
        Ok(SettingsOperationReport {
            outcome,
            value,
            error,
            epoch: control.epoch,
            availability: control.availability,
        })
    }

    /// Capture, builder, classification and adoption, all under the caller's
    /// guards. Admission errors return before any publisher can run.
    fn operation_body<T>(
        &self,
        state: &mut AppSettings,
        guard: &mut SettingsOperationGuard<'_>,
        builder: impl FnOnce(&AppSettings, &mut SettingsOperationGuard<'_>) -> Result<T, String>,
    ) -> Result<OperationParts<T>, SettingsOwnerError> {
        // Sources that do not compose at admission reject the operation before
        // any publisher; RAM keeps the last adopted value.
        let captured = capture_sources_locked(guard)?;
        let current = compose_captured_sources(&captured, CapturePolicy::Recovery)?;
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            builder(&current, &mut *guard)
        }));
        let effect = guard.effect;
        let (value, error) = match built {
            Ok(Ok(value)) => (Some(value), None),
            Ok(Err(error)) => (None, Some(error)),
            Err(_) => (None, Some("operation panicked".to_string())),
        };
        let outcome = if error.is_none() {
            OperationOutcome::Committed
        } else if effect == OperationEffect::None {
            OperationOutcome::Rejected
        } else {
            OperationOutcome::PartialOrUnknown
        };
        // Rejected changed nothing: keep RAM. A success that published nothing
        // adopts the composition it already captured, without a second capture.
        // Otherwise recapture under the same guard and adopt reality, or become
        // Invalid without saving defaults.
        let outcome = if outcome == OperationOutcome::Rejected {
            outcome
        } else if effect == OperationEffect::None {
            *state = current;
            self.control().epoch += 1;
            outcome
        } else {
            let recaptured = capture_sources_locked(guard);
            match recaptured.and_then(|captured| recover_composition(&captured)) {
                Ok(effective) => {
                    *state = effective;
                    self.control().epoch += 1;
                    outcome
                }
                Err(_) => {
                    self.mark_invalid("recapture");
                    OperationOutcome::PartialOrUnknown
                }
            }
        };
        Ok((outcome, value, error))
    }

    /// Seals admission, waits for every permit and operation, then releases the
    /// lifecycle authority LAST. The soft deadline only diagnoses; it never
    /// detaches or unlocks early.
    pub fn drain_blocking(&self, soft_deadline: std::time::Duration) {
        let started = std::time::Instant::now();
        let mut control = self.control();
        control.sealed = true;
        if control.availability != SettingsAvailability::Closed {
            control.availability = SettingsAvailability::Draining;
        }
        let mut reported = false;
        while control.permits > 0 || control.operations > 0 {
            let (next, timeout) = self
                .released
                .wait_timeout(control, std::time::Duration::from_millis(50))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            control = next;
            if timeout.timed_out() && !reported && started.elapsed() >= soft_deadline {
                reported = true;
                log::debug!(
                    "[settings-owner] {} drain past soft deadline permits={} operations={}",
                    self.instance_id,
                    control.permits,
                    control.operations
                );
            }
        }
        control.availability = SettingsAvailability::Closed;
        drop(control);
        let lease = self.lease_guard().take();
        drop(lease);
        log::debug!(
            "[settings-owner] {} closed in {:?}",
            self.instance_id,
            started.elapsed()
        );
    }

    fn staged_forwarding(&self) {
        debug_assert_eq!(
            self.stage,
            SettingsOwnerStage::StagedCompatibility,
            "lock forwarding is StagedCompatibility only"
        );
    }

    // StagedCompatibility forwarding: the pre-owner RwLock surface, kept only so
    // consumers not yet migrated compile and behave exactly as before. P50
    // retires it from the product.

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, AppSettings> {
        self.staged_forwarding();
        self.settings.read().await
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, AppSettings> {
        self.staged_forwarding();
        self.settings.write().await
    }

    pub fn try_read(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, AppSettings>, tokio::sync::TryLockError> {
        self.staged_forwarding();
        self.settings.try_read()
    }

    pub fn try_write(
        &self,
    ) -> Result<tokio::sync::RwLockWriteGuard<'_, AppSettings>, tokio::sync::TryLockError> {
        self.staged_forwarding();
        self.settings.try_write()
    }

    pub fn blocking_read(&self) -> tokio::sync::RwLockReadGuard<'_, AppSettings> {
        self.staged_forwarding();
        self.settings.blocking_read()
    }

    pub fn blocking_write(&self) -> tokio::sync::RwLockWriteGuard<'_, AppSettings> {
        self.staged_forwarding();
        self.settings.blocking_write()
    }

    pub async fn read_owned(self: Arc<Self>) -> tokio::sync::OwnedRwLockReadGuard<AppSettings> {
        self.staged_forwarding();
        Arc::clone(&self.settings).read_owned().await
    }

    pub async fn write_owned(self: Arc<Self>) -> tokio::sync::OwnedRwLockWriteGuard<AppSettings> {
        self.staged_forwarding();
        Arc::clone(&self.settings).write_owned().await
    }

    pub fn try_read_owned(
        self: Arc<Self>,
    ) -> Result<tokio::sync::OwnedRwLockReadGuard<AppSettings>, tokio::sync::TryLockError> {
        self.staged_forwarding();
        Arc::clone(&self.settings).try_read_owned()
    }
}

pub type SettingsState = Arc<SettingsOwner>;

pub fn new_settings_state(settings: AppSettings) -> SettingsState {
    Arc::new(SettingsOwner::staged(settings))
}

#[cfg(test)]
mod tests;

#[allow(dead_code)]
pub(crate) fn source_transition_digest<T: Serialize>(
    value: &T,
) -> Result<String, SourceCoordinationError> {
    let bytes = serde_json::to_vec(value).map_err(|_| SourceCoordinationError::Invalid)?;
    let SourceRevision::Bytes { sha256 } = source_bytes_revision(&bytes) else {
        unreachable!()
    };
    Ok(sha256)
}

/// Validate the retained original packet with existing guarded PAIR verification.
/// Pending is admissible here only after the primitive validates its reachable
/// stage; it never licenses preparation of a replacement packet.
#[allow(dead_code)]
pub(crate) fn recover_source_target_packet(
    target: &SourceInventoryTarget,
    operation_id: &str,
    source_key: &str,
    mappings_digest: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<Option<IdentityReferencePacket>, IdentityReferenceError> {
    for _ in 0..=2 {
        validate()?;
        let physical = read_identity_reference_physical(&target.reservation)?;
        let bytes = match &physical {
            IdentityReferencePhysicalState::Absent => return Ok(None),
            IdentityReferencePhysicalState::Bytes { bytes, .. } => bytes,
        };
        let packet: IdentityReferencePacket =
            serde_json::from_slice(bytes).map_err(|_| IdentityReferenceError::Conflict)?;
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| IdentityReferenceError::Conflict)?;
        if packet.operation_id != operation_id
            || packet.source_physical_key != source_key
            || value.get("mappingsDigest").and_then(Value::as_str) != Some(mappings_digest)
        {
            return Err(IdentityReferenceError::Conflict);
        }
        let before_bytes = serde_json::to_vec(&target.before_protected)
            .map_err(|error| IdentityReferenceError::Io(error.to_string()))?;
        let before = source_bytes_revision(&before_bytes);
        let hashes = packet.protected_revision_digests()?;
        if before
            != (SourceRevision::Bytes {
                sha256: hashes[0].clone(),
            })
        {
            return Err(IdentityReferenceError::Conflict);
        }
        match verify_identity_reference_pair_guarded(
            &target.decisions,
            &target.state,
            &target.reservation,
            operation_id,
            &packet.plan_digest,
            validate,
        ) {
            Ok(_) | Err(IdentityReferenceError::Pending) => {}
            Err(error) => return Err(error),
        }
        validate()?;
        if read_identity_reference_physical(&target.reservation)? == physical {
            return Ok(Some(packet));
        }
    }
    Err(IdentityReferenceError::Conflict)
}

#[allow(dead_code)]
pub(crate) fn verify_completed_source_target(
    target: &SourceInventoryTarget,
    commitment: &SourcePacketRecord,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<(), IdentityReferenceError> {
    validate()?;
    let original_before = serde_json::to_vec(&target.before_protected)
        .map_err(|error| IdentityReferenceError::Io(error.to_string()))?;
    if source_bytes_revision(&original_before) != commitment.before_protected_revision {
        return Err(IdentityReferenceError::Conflict);
    }
    let before = [
        read_identity_reference_physical(&target.decisions)?,
        read_identity_reference_physical(&target.state)?,
    ];
    let protected = identity_reference_protected_revision(&before)?;
    let digest =
        source_transition_digest(&protected).map_err(|_| IdentityReferenceError::Conflict)?;
    if commitment.after_protected_revision != (SourceRevision::Bytes { sha256: digest }) {
        return Err(IdentityReferenceError::Conflict);
    }
    validate()?;
    if before
        != [
            read_identity_reference_physical(&target.decisions)?,
            read_identity_reference_physical(&target.state)?,
        ]
    {
        return Err(IdentityReferenceError::Conflict);
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceOriginalInventoryEvidence {
    discovery: SourceDiscoveryWitness,
    objects: Vec<(PathBuf, (u64, u64))>,
    source_before: IdentityReferencePhysicalState,
    expected_source: SourceRevision,
    after_source: SourceRevision,
    discovered: Vec<SourceDiscoveredTargetEvidence>,
    topology: SourceMembershipTopology,
    inventory_digest: String,
}

#[allow(dead_code)]
pub(crate) fn source_inventory_target(
    root: PathBuf,
    decisions: PathBuf,
    state: PathBuf,
    reservation: PathBuf,
    physical: [IdentityReferencePhysicalState; 2],
    originals: Vec<ConfigurationRef>,
) -> Result<SourceInventoryTarget, IdentityReferenceError> {
    let identity = crate::path_identity::verify_directory(&root)
        .map_err(IdentityReferenceError::Preparation)?;
    if identity.canonical_path != root
        || decisions.parent() != Some(root.as_path())
        || state.parent() != Some(root.as_path())
        || reservation.parent() != Some(root.as_path())
    {
        return Err(IdentityReferenceError::Conflict);
    }
    let before_protected = identity_reference_protected_revision(&physical)?;
    Ok(SourceInventoryTarget {
        root,
        root_object: (identity.object_id.volume, identity.object_id.file),
        decisions,
        state,
        reservation,
        before_protected,
        original_selections: originals,
        original_physical: Some(physical),
    })
}

pub(crate) fn source_original_mappings(
    snapshot: &RawSourceSnapshot,
    prepared: &PreparedSourceCandidate,
) -> Result<Vec<SourceIdentityMapping>, SourcePreparationError> {
    let registered = snapshot.snapshot.source.kind == SourceKind::RegisteredInstance;
    let after: Value = serde_json::from_slice(&prepared.serialized_after)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let (after_agents, after_profiles) = if registered {
        let (agents, profiles) = decode_registered_raw(&after)?;
        (
            agents
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SourcePreparationError::InvalidCandidate)?,
            profiles.profiles_by_agent,
        )
    } else {
        let catalog =
            crate::config::coding_agents_catalog::parse_catalog_schema2(&prepared.serialized_after)
                .map_err(|_| SourcePreparationError::InvalidCandidate)?;
        (
            catalog
                .agents
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SourcePreparationError::InvalidCandidate)?,
            catalog.coding_agent_profiles,
        )
    };
    let key_field = if registered { "id" } else { "key" };
    let old: BTreeMap<_, _> = snapshot
        .snapshot
        .agents
        .iter()
        .map(|agent| {
            agent
                .get(key_field)
                .and_then(Value::as_str)
                .map(|key| (key.to_owned(), agent))
                .ok_or(SourcePreparationError::InvalidCandidate)
        })
        .collect::<Result<_, _>>()?;
    let new: BTreeMap<_, _> = after_agents
        .iter()
        .map(|agent| {
            agent
                .get(key_field)
                .and_then(Value::as_str)
                .map(|key| (key.to_owned(), agent))
                .ok_or(SourcePreparationError::InvalidCandidate)
        })
        .collect::<Result<_, _>>()?;
    let calculated = |value: &&Value| -> Result<String, SourcePreparationError> {
        let command = raw_configuration_command(
            value
                .as_object()
                .ok_or(SourcePreparationError::InvalidCandidate)?,
            "baseCommand",
        )
        .map_err(|_| SourcePreparationError::InvalidCandidate)?
        .ok_or(SourcePreparationError::InvalidCandidate)?;
        let envs: Vec<CodingAgentEnv> = value
            .get("envs")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| SourcePreparationError::InvalidCandidate)?
            .unwrap_or_default();
        Ok(configuration_identity(command, &envs))
    };
    let keys: std::collections::BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
    let mut mappings = Vec::new();
    for key in keys {
        let old_calculated_id = old.get(&key).map(&calculated).transpose()?;
        let new_calculated_id = new.get(&key).map(&calculated).transpose()?;
        let old_cells = snapshot.snapshot.profiles.get(&key);
        let new_cells = after_profiles.get(&key);
        let letters: std::collections::BTreeSet<_> = old_cells
            .into_iter()
            .flat_map(|cells| cells.keys())
            .chain(new_cells.into_iter().flat_map(|cells| cells.keys()))
            .cloned()
            .collect();
        let profiles: BTreeMap<_, _> = letters
            .into_iter()
            .filter_map(|letter| {
                let before = old_cells
                    .and_then(|cells| cells.get(&letter))
                    .map(profile_identity);
                let after = new_cells
                    .and_then(|cells| cells.get(&letter))
                    .map(profile_identity);
                (before != after).then_some((letter, (before, after)))
            })
            .collect();
        if old_calculated_id != new_calculated_id || !profiles.is_empty() {
            let unavailable = new_calculated_id.is_none()
                || !registered
                    && crate::config::coding_agents_catalog::BUILTIN_AGENT_SUPPORT
                        .iter()
                        .any(|(builtin, supported)| *builtin == key && !supported);
            mappings.push(SourceIdentityMapping {
                entry_hint: key,
                old_calculated_id,
                new_calculated_id,
                profiles,
                unavailable,
            });
        }
    }
    Ok(mappings)
}

type SourceMembershipTarget = (PathBuf, (u64, u64), PathBuf, PathBuf, PathBuf);

/// Private, backend-validated topology; never a deserialized authority token.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceMembershipTopology {
    registry: Vec<String>,
    directories: Vec<(PathBuf, (u64, u64))>,
    slots: Vec<(PathBuf, bool)>,
    membership: Vec<(PathBuf, Vec<PathBuf>)>,
    targets: Vec<SourceMembershipTarget>,
    // Slot coordinate is structural: mutable selection values are excluded.
    links: Vec<(PathBuf, String, SourceRef, String)>,
}

fn source_ref_tuple(
    source: &SourceRef,
) -> (SourceFamily, SourceRole, SourceScope, SourceKind, &str) {
    (
        source.family,
        source.role,
        source.scope,
        source.kind,
        &source.context_root,
    )
}

fn source_inventory_projection(
    editor: &str,
    source: &SourceRef,
    physical_source_key: &str,
    context_revision: &str,
    original: &SourceMembershipTopology,
) -> Result<Vec<u8>, SourceCoordinationError> {
    if editor.is_empty() || physical_source_key.is_empty() || context_revision.is_empty() {
        return Err(SourceCoordinationError::Invalid);
    }
    // Registry order is policy-significant. Everything below is set-valued.
    let mut topology = original.clone();
    topology.directories.sort();
    topology.directories.dedup();
    topology.slots.sort();
    topology.slots.dedup();
    for (_, members) in &mut topology.membership {
        members.sort();
        members.dedup();
    }
    topology.membership.sort();
    topology.membership.dedup();
    topology.targets.sort();
    topology.targets.dedup();
    for pair in topology.directories.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.slots.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.membership.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.targets.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    let mut links = topology
        .links
        .iter()
        .map(|(root, slot, linked, key)| {
            if slot.is_empty() || linked != source || key != physical_source_key {
                return Err(SourceCoordinationError::Invalid);
            }
            // Encode enum spellings via serde, not Debug or platform formatting.
            serde_json::to_vec(&(root, slot, source_ref_tuple(linked), key))
                .map(|bytes| ((root.clone(), slot.clone()), bytes))
                .map_err(|_| SourceCoordinationError::Invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    links.sort();
    links.dedup();
    for pair in links.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    let links = links
        .into_iter()
        .map(|(_, bytes)| {
            serde_json::from_slice::<Value>(&bytes).map_err(|_| SourceCoordinationError::Invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_vec(&(
        "p03-member-inventory-v1",
        editor,
        1u32,
        source_ref_tuple(source),
        physical_source_key,
        context_revision,
        topology.registry,
        topology.directories,
        topology.slots,
        topology.membership,
        topology.targets,
        links,
    ))
    .map_err(|_| SourceCoordinationError::Invalid)
}

fn source_inventory_digest(
    participant: &str,
    source: &SourceRef,
    physical_source_key: &str,
    context_revision: &str,
    topology: &SourceMembershipTopology,
) -> Result<String, SourceCoordinationError> {
    let bytes = source_inventory_projection(
        participant,
        source,
        physical_source_key,
        context_revision,
        topology,
    )?;
    match source_bytes_revision(&bytes) {
        SourceRevision::Bytes { sha256 } => Ok(sha256),
        SourceRevision::Absent {} => Err(SourceCoordinationError::Invalid),
    }
}

/// Backend semantic coordinate; history cannot manufacture matrix-default authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum SourceOriginalSlot {
    Decision,
    MatrixDefault,
    ReplicaDefault,
    HistoryKey(String),
    HistoryIndex(usize),
}

impl SourceOriginalSlot {
    fn stored<'a>(&self, target: &'a Value) -> Option<&'a Value> {
        match self {
            Self::Decision => target.pointer("/tooling/configurationRef"),
            Self::MatrixDefault | Self::ReplicaDefault => {
                target.pointer("/tooling/replicaSelectionDefault/configurationRef")
            }
            Self::HistoryKey(key) => target
                .get("tooling")?
                .get("codingAgents")?
                .as_object()?
                .get(key)?
                .get("configurationRef"),
            Self::HistoryIndex(index) => target
                .get("tooling")?
                .get("sourceReferences")?
                .as_array()?
                .get(*index)?
                .get("configurationRef"),
        }
    }

    fn coordinate(&self) -> String {
        match self {
            Self::Decision => "decision/configurationRef".into(),
            Self::MatrixDefault | Self::ReplicaDefault => {
                "decision/replicaSelectionDefault/configurationRef".into()
            }
            Self::HistoryKey(key) => format!(
                "state/codingAgents/{}/configurationRef",
                key.replace('~', "~0").replace('/', "~1")
            ),
            Self::HistoryIndex(index) => format!("state/sourceReferences/{index}/configurationRef"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceDiscoveredTargetEvidence {
    pub(crate) target: SourceInventoryTarget,
    pub(crate) slots: Vec<(SourceOriginalSlot, ConfigurationRef)>,
    pub(crate) affected: bool,
}

pub(crate) struct SourceInventoryCapture {
    discovery: SourceDiscoveryWitness,
    objects: Vec<(PathBuf, (u64, u64))>,
    discovered: Vec<SourceDiscoveredTargetEvidence>,
    topology: SourceMembershipTopology,
    original_links: SourceOriginalProofAggregate,
    classified: bool,
}

impl SourceInventoryCapture {
    pub(crate) fn capture(
        discovery: SourceDiscoveryWitness,
        mut discovered: Vec<SourceDiscoveredTargetEvidence>,
        snapshot: &RawSourceSnapshot,
        origin: &LegacyCompatibilityOrigin,
        physical_source_key: &str,
    ) -> Result<Self, SourcePreparationError> {
        let invalid = || SourcePreparationError::LegacyOriginUnresolved;
        let mut paths = discovery.parents.clone();
        for (root, members) in &discovery.membership {
            paths.push(root.clone());
            paths.extend(members.iter().cloned());
        }
        paths.extend(
            discovered
                .iter()
                .map(|evidence| evidence.target.root.clone()),
        );
        paths.sort();
        paths.dedup();
        let mut objects = Vec::new();
        for path in paths {
            let verified = crate::path_identity::verify_directory(&path).map_err(|_| invalid())?;
            objects.push((
                verified.canonical_path,
                (verified.object_id.volume, verified.object_id.file),
            ));
        }
        objects.sort();
        objects.dedup();
        if objects.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(invalid());
        }
        for (slot, present) in &discovery.slots {
            if *present {
                let verified =
                    crate::path_identity::verify_directory(slot).map_err(|_| invalid())?;
                if !objects.iter().any(|(path, object)| {
                    path == &verified.canonical_path
                        && *object == (verified.object_id.volume, verified.object_id.file)
                }) {
                    return Err(invalid());
                }
            } else {
                let parent = slot.parent().ok_or_else(invalid)?;
                crate::path_identity::verify_directory(parent).map_err(|_| invalid())?;
                match std::fs::symlink_metadata(slot) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err(invalid()),
                }
            }
        }
        discovered.sort_by(|a, b| a.target.decisions.cmp(&b.target.decisions));
        if discovered
            .windows(2)
            .any(|pair| pair[0].target.root == pair[1].target.root)
        {
            return Err(invalid());
        }
        let mut links = Vec::new();
        let mut originals = Vec::new();
        let mut defaults: BTreeMap<
            String,
            (PathBuf, std::collections::BTreeSet<LegacyOriginUnit>),
        > = BTreeMap::new();
        for evidence in &discovered {
            let target = &evidence.target;
            let physical = target.original_physical.as_ref().ok_or_else(invalid)?;
            if identity_reference_protected_revision(physical).map_err(|_| invalid())?
                != target.before_protected
                || !objects
                    .iter()
                    .any(|(root, object)| root == &target.root && object == &target.root_object)
            {
                return Err(invalid());
            }
            for (slot, selection) in &evidence.slots {
                if selection.version != 1 {
                    return Err(invalid());
                }
                if matches!(
                    slot,
                    SourceOriginalSlot::MatrixDefault | SourceOriginalSlot::ReplicaDefault
                ) {
                    let name = target
                        .root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(invalid)?;
                    let matrix = name.starts_with("_agent_");
                    let replica = name.starts_with("__agent_");
                    if matrix != matches!(slot, SourceOriginalSlot::MatrixDefault)
                        || replica != matches!(slot, SourceOriginalSlot::ReplicaDefault)
                    {
                        return Err(invalid());
                    }
                }
                let side = match slot {
                    SourceOriginalSlot::Decision
                    | SourceOriginalSlot::MatrixDefault
                    | SourceOriginalSlot::ReplicaDefault => 0,
                    _ => 1,
                };
                let bytes = match &physical[side] {
                    IdentityReferencePhysicalState::Bytes { bytes, .. } => bytes,
                    _ => return Err(invalid()),
                };
                let raw: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
                let stored: ConfigurationRef =
                    serde_json::from_value(slot.stored(&raw).cloned().ok_or_else(invalid)?)
                        .map_err(|_| invalid())?;
                if stored != *selection {
                    return Err(invalid());
                }
                if selection.source != snapshot.snapshot.source {
                    continue;
                }
                links.push((
                    target.root.clone(),
                    slot.coordinate(),
                    selection.source.clone(),
                    physical_source_key.into(),
                ));
                if snapshot.snapshot.source.kind != SourceKind::RegisteredInstance {
                    continue;
                }
                let matrix = if matches!(slot, SourceOriginalSlot::MatrixDefault) {
                    // Only a real origin Matrix can own N. Replica/history never mint Default.
                    let name = target
                        .root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(invalid)?;
                    Some(
                        name.strip_prefix("_agent_")
                            .filter(|name| !name.is_empty())
                            .ok_or_else(invalid)?,
                    )
                } else {
                    None
                };
                let proof = origin.bind_original_selection_at(
                    snapshot,
                    selection,
                    &snapshot.snapshot.revision,
                    &snapshot.snapshot.context_revision,
                    bytes,
                    bytes,
                    matrix,
                    slot,
                )?;
                if let Some(matrix) = matrix {
                    let provenance = (target.root.clone(), proof.units.clone());
                    if defaults
                        .get(matrix)
                        .is_some_and(|original| original != &provenance)
                    {
                        return Err(invalid());
                    }
                    defaults.insert(matrix.into(), provenance);
                }
                originals.push(proof);
            }
        }
        let topology = SourceMembershipTopology {
            registry: discovery.registry.clone(),
            directories: objects.clone(),
            slots: discovery.slots.clone(),
            membership: discovery.membership.clone(),
            targets: discovered
                .iter()
                .map(|e| {
                    let t = &e.target;
                    (
                        t.root.clone(),
                        t.root_object,
                        t.decisions.clone(),
                        t.state.clone(),
                        t.reservation.clone(),
                    )
                })
                .collect(),
            links,
        };
        Ok(Self {
            discovery,
            objects,
            discovered,
            topology,
            original_links: SourceOriginalProofAggregate { originals },
            classified: false,
        })
    }

    pub(crate) fn original_links(&self) -> &SourceOriginalProofAggregate {
        &self.original_links
    }
}

pub(crate) type SourceOriginalSlotParser<'a> = dyn Fn(
        &SourceInventoryTarget,
    ) -> Result<Vec<(SourceOriginalSlot, ConfigurationRef)>, IdentityReferenceError>
    + 'a;

pub(crate) struct SourceInventoryInputs<'borrow, 'identity> {
    pub(crate) settings: &'borrow AppSettings,
    pub(crate) context: &'borrow SourceReadContext,
    pub(crate) identity: &'borrow IdentityWriteLease,
    pub(crate) lease: &'borrow SourceWriteContext<'identity>,
}

/// Backend-only original operation authority. No retained discovery handles.
pub(crate) struct SourceInventoryProof {
    operation_id: String,
    inventory: SourceLocalInventory,
}

impl SourceInventoryProof {
    pub(crate) fn for_candidate(
        inputs: SourceInventoryInputs<'_, '_>,
        snapshot: &RawSourceSnapshot,
        prepared: &PreparedSourceCandidate,
        origin: &OriginInputs,
        capture: SourceInventoryCapture,
        operation_id: &str,
    ) -> Result<Self, SourcePreparationError> {
        let SourceInventoryInputs {
            settings,
            context,
            identity,
            lease,
        } = inputs;
        if !capture.classified
            || SourceReadContext::from_settings(settings, &context.settings_path)? != *context
            || capture.discovery.registry != settings.project_paths
            || prepared.source != snapshot.snapshot.source
            || prepared.source != *lease.source()
        {
            return Err(SourcePreparationError::ContextChanged);
        }
        prepared.revalidate_before_write(context)?;
        origin.revalidate(context)?;
        let inventory_digest = source_inventory_digest(
            identity.instance_id(),
            lease.source(),
            lease.physical_source_key(),
            lease.context_revision(),
            &capture.topology,
        )
        .map_err(|_| SourcePreparationError::ContextChanged)?;
        let mappings = source_original_mappings(snapshot, prepared)?;
        let source_before = match &prepared.before_bytes {
            Some(bytes) => IdentityReferencePhysicalState::from_bytes(bytes.clone()),
            None => IdentityReferencePhysicalState::Absent,
        };
        let evidence = SourceOriginalInventoryEvidence {
            discovery: capture.discovery,
            objects: capture.objects.clone(),
            source_before,
            expected_source: prepared.expected_revision.clone(),
            after_source: source_bytes_revision(&prepared.serialized_after),
            discovered: capture.discovered.clone(),
            topology: capture.topology,
            inventory_digest,
        };
        let mut targets = Vec::new();
        for classified in &capture.discovered {
            if classified.affected {
                let mut target = classified.target.clone();
                // Original images have one durable location, in full evidence.
                target.original_physical = None;
                targets.push(target);
            }
        }
        let mut inventory = SourceLocalInventory {
            editor_id: identity.instance_id().into(),
            source: prepared.source.clone(),
            physical_source_key: lease.physical_source_key().into(),
            context_revision: context.revision.clone(),
            authorization_revision: String::new(),
            authorized_roots: capture.objects,
            targets,
            mappings,
            origin_binding: Some(origin.binding.clone()),
            original_evidence: Some(evidence),
        };
        inventory.authorization_revision = source_transition_digest(&inventory)
            .map_err(|_| SourcePreparationError::ContextChanged)?;
        let proof = Self {
            operation_id: operation_id.into(),
            inventory,
        };
        proof
            .validate(identity, lease, context)
            .map_err(|_| SourcePreparationError::ContextChanged)?;
        Ok(proof)
    }

    pub(crate) fn inventory(&self) -> &SourceLocalInventory {
        &self.inventory
    }

    pub(crate) fn inventory_digest(&self) -> Result<&str, SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let actual = source_inventory_digest(
            &self.inventory.editor_id,
            &self.inventory.source,
            &self.inventory.physical_source_key,
            &self.inventory.context_revision,
            &original.topology,
        )?;
        if actual != original.inventory_digest {
            return Err(SourceCoordinationError::Invalid);
        }
        Ok(&original.inventory_digest)
    }

    pub(crate) fn stored_intent(
        &self,
        prepared: &PreparedSourceCandidate,
        operation_id: &str,
        request_id: &str,
        payload_digest: &str,
    ) -> Result<SourceStoredIntent, SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if operation_id != self.operation_id
            || prepared.source != self.inventory.source
            || prepared.expected_revision != original.expected_source
            || source_bytes_revision(&prepared.serialized_after) != original.after_source
        {
            return Err(SourceCoordinationError::Invalid);
        }
        Ok(SourceStoredIntent {
            schema_version: 2,
            operation_id: operation_id.into(),
            request_id: request_id.into(),
            payload_digest: payload_digest.into(),
            source: self.inventory.source.clone(),
            expected_revision: prepared.expected_revision.clone(),
            before_bytes: prepared.before_bytes.clone(),
            serialized_after: prepared.serialized_after.clone(),
            mappings: self.inventory.mappings.clone(),
            editor_id: self.inventory.editor_id.clone(),
            context_revision: self.inventory.context_revision.clone(),
            authorization_revision: self.inventory.authorization_revision.clone(),
        })
    }

    /// Pure segment guard: directory/object and original origin checks only.
    /// Full enumeration is orchestrated outside PAIR by mutations.
    pub(crate) fn validate(
        &self,
        identity: &IdentityWriteLease,
        lease: &SourceWriteContext,
        context: &SourceReadContext,
    ) -> Result<(), IdentityReferenceError> {
        #[cfg(test)]
        source_factory_guard_cut()?;
        let conflict = || IdentityReferenceError::Conflict;
        let inventory = &self.inventory;
        let original = inventory.original_evidence.as_ref().ok_or_else(conflict)?;
        if identity.instance_id() != inventory.editor_id
            || lease.source() != &inventory.source
            || lease.physical_source_key() != inventory.physical_source_key
            || context.revision != inventory.context_revision
        {
            return Err(conflict());
        }
        lease.revalidate(context).map_err(|_| conflict())?;
        for (path, object) in &original.objects {
            let current = crate::path_identity::verify_directory(path).map_err(|_| conflict())?;
            if current.canonical_path != *path
                || (current.object_id.volume, current.object_id.file) != *object
            {
                return Err(conflict());
            }
        }
        for (slot, present) in &original.discovery.slots {
            let exists = match std::fs::symlink_metadata(slot) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                _ => return Err(conflict()),
            };
            if exists != *present {
                return Err(conflict());
            }
        }
        let observed = OriginInputs::read(context);
        let binding = inventory.origin_binding.as_ref().ok_or_else(conflict)?;
        if observed.binding.context != binding.context
            || observed.binding.settings != binding.settings
            || observed.binding.overlay != binding.overlay
        {
            return Err(conflict());
        }
        let current_source = lease.source_revision().map_err(|_| conflict())?;
        if current_source != original.expected_source && current_source != original.after_source {
            return Err(conflict());
        }
        if current_source != original.expected_source {
            let (_, journal) = read_own_source_journal(context).map_err(|_| conflict())?;
            let intent = journal
                .intents
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            let progress = journal
                .progress
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            if !progress.packets_prepared
                || intent.source != inventory.source
                || intent.expected_revision != original.expected_source
                || source_bytes_revision(&intent.serialized_after) != original.after_source
                || intent.mappings != inventory.mappings
                || intent.editor_id != identity.instance_id()
            {
                return Err(conflict());
            }
        }
        if observed.binding.registered != binding.registered {
            if inventory.source.kind != SourceKind::RegisteredInstance
                || current_source != original.after_source
                || observed.binding.registered.as_ref() != Some(&original.after_source)
            {
                return Err(conflict());
            }
            let (_, journal) = read_own_source_journal(context).map_err(|_| conflict())?;
            let intent = journal
                .intents
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            if intent.source != inventory.source
                || intent.expected_revision != original.expected_source
                || source_bytes_revision(&intent.serialized_after) != original.after_source
                || intent.mappings != inventory.mappings
                || intent.editor_id != identity.instance_id()
            {
                return Err(conflict());
            }
        }
        Ok(())
    }
}

impl SourceInventoryCapture {
    pub(crate) fn same_original_capture(&self, other: &Self) -> bool {
        self.discovery == other.discovery
            && self.objects == other.objects
            && self.discovered == other.discovered
            && self.topology == other.topology
    }

    pub(crate) fn original_targets(&self) -> &[SourceDiscoveredTargetEvidence] {
        &self.discovered
    }

    pub(crate) fn classify(
        &mut self,
        classifications: Vec<(PathBuf, bool)>,
    ) -> Result<(), SourcePreparationError> {
        if classifications.len() != self.discovered.len() {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        for (evidence, (target, affected)) in self.discovered.iter_mut().zip(classifications) {
            if evidence.target.decisions != target {
                return Err(SourcePreparationError::LegacyOriginUnresolved);
            }
            evidence.affected = affected;
        }
        self.classified = true;
        Ok(())
    }
}

impl SourceLocalInventory {
    fn validate_original_evidence(&self) -> Result<(), SourceCoordinationError> {
        let Some(original) = &self.original_evidence else {
            return Ok(());
        };
        let invalid = || SourceCoordinationError::Invalid;
        let mut unsigned = self.clone();
        unsigned.authorization_revision.clear();
        if source_transition_digest(&unsigned)? != self.authorization_revision
            || original.objects != self.authorized_roots
            || original.topology.directories != original.objects
            || original.topology.registry != original.discovery.registry
            || original.topology.slots != original.discovery.slots
            || original.topology.membership != original.discovery.membership
            || self.origin_binding.is_none()
        {
            return Err(invalid());
        }
        let before = match &original.source_before {
            IdentityReferencePhysicalState::Absent => SourceRevision::Absent {},
            IdentityReferencePhysicalState::Bytes { bytes, .. } => {
                if IdentityReferencePhysicalState::from_bytes(bytes.clone())
                    != original.source_before
                {
                    return Err(invalid());
                }
                source_bytes_revision(bytes)
            }
        };
        if before != original.expected_source
            || source_inventory_digest(
                &self.editor_id,
                &self.source,
                &self.physical_source_key,
                &self.context_revision,
                &original.topology,
            )? != original.inventory_digest
            || original
                .discovered
                .windows(2)
                .any(|pair| pair[0].target.decisions >= pair[1].target.decisions)
        {
            return Err(invalid());
        }
        let mut selected = Vec::new();
        let mut targets = Vec::new();
        let mut links = Vec::new();
        for evidence in &original.discovered {
            let target = &evidence.target;
            let physical = target.original_physical.as_ref().ok_or_else(invalid)?;
            for image in physical {
                if let IdentityReferencePhysicalState::Bytes { bytes, .. } = image {
                    if &IdentityReferencePhysicalState::from_bytes(bytes.clone()) != image {
                        return Err(invalid());
                    }
                }
            }
            if identity_reference_protected_revision(physical).map_err(|_| invalid())?
                != target.before_protected
                || !original
                    .objects
                    .iter()
                    .any(|(path, object)| path == &target.root && object == &target.root_object)
                || target.original_selections
                    != evidence
                        .slots
                        .iter()
                        .map(|(_, selection)| selection.clone())
                        .collect::<Vec<_>>()
            {
                return Err(invalid());
            }
            targets.push((
                target.root.clone(),
                target.root_object,
                target.decisions.clone(),
                target.state.clone(),
                target.reservation.clone(),
            ));
            for (slot, selection) in &evidence.slots {
                if selection.source == self.source {
                    links.push((
                        target.root.clone(),
                        slot.coordinate(),
                        selection.source.clone(),
                        self.physical_source_key.clone(),
                    ));
                }
            }
            if evidence.affected {
                let mut target = target.clone();
                target.original_physical = None;
                selected.push(target);
            }
        }
        if targets != original.topology.targets
            || links != original.topology.links
            || selected != self.targets
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub(crate) fn verify_original_slots(
        &self,
        parse: &SourceOriginalSlotParser<'_>,
    ) -> Result<(), IdentityReferenceError> {
        let original = self
            .original_evidence
            .as_ref()
            .ok_or(IdentityReferenceError::Conflict)?;
        for evidence in &original.discovered {
            let slots = parse(&evidence.target)?;
            if slots != evidence.slots
                || slots
                    .iter()
                    .map(|(_, reference)| reference.clone())
                    .collect::<Vec<_>>()
                    != evidence.target.original_selections
            {
                return Err(IdentityReferenceError::Conflict);
            }
        }
        Ok(())
    }

    /// Shared three-outcome reader: Ok(true) matches, Ok(false) is definite
    /// protected mismatch, Err is uncertainty/invalid authority, never stale.
    fn affected_original_protected_matches(&self) -> Result<bool, SourceCoordinationError> {
        self.validate_original_evidence()?;
        let original = self
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let mut matches = true;
        for evidence in original
            .discovered
            .iter()
            .filter(|evidence| evidence.affected)
        {
            let target = &evidence.target;
            let identity = crate::path_identity::verify_directory(&target.root)
                .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
            if (identity.object_id.volume, identity.object_id.file) != target.root_object {
                return Err(SourceCoordinationError::IdentityUncertain);
            }
            let physical = [
                read_identity_reference_physical(&target.decisions)
                    .map_err(|_| SourceCoordinationError::IdentityUncertain)?,
                read_identity_reference_physical(&target.state)
                    .map_err(|_| SourceCoordinationError::IdentityUncertain)?,
            ];
            let protected = identity_reference_protected_revision(&physical)
                .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
            matches &= protected == target.before_protected;
        }
        Ok(matches)
    }
}

impl SourceInventoryProof {
    pub(crate) fn for_resume(
        inputs: SourceInventoryInputs<'_, '_>,
        operation_id: &str,
        journal: &SourcePrivateJournal,
        parse_slots: &SourceOriginalSlotParser<'_>,
    ) -> Result<Self, SourceCoordinationError> {
        let SourceInventoryInputs {
            settings,
            context,
            identity,
            lease,
        } = inputs;
        let inventory = journal
            .inventories
            .get(operation_id)
            .ok_or(SourceCoordinationError::Invalid)?
            .clone();
        // Recompute from original physical images BEFORE restoring any proof.
        inventory
            .verify_original_slots(parse_slots)
            .map_err(|_| SourceCoordinationError::Invalid)?;
        let original = inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if original.discovery.registry != settings.project_paths
            || SourceReadContext::from_settings(settings, &context.settings_path)
                .map_err(|_| SourceCoordinationError::Unauthorized)?
                != *context
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
        let mut unsigned = inventory.clone();
        unsigned.authorization_revision.clear();
        if source_transition_digest(&unsigned)? != inventory.authorization_revision {
            return Err(SourceCoordinationError::Invalid);
        }
        let intent = journal
            .intents
            .get(operation_id)
            .ok_or(SourceCoordinationError::Invalid)?;
        if intent.editor_id != identity.instance_id()
            || intent.authorization_revision != inventory.authorization_revision
            || intent.expected_revision != original.expected_source
            || source_bytes_revision(&intent.serialized_after) != original.after_source
            || intent.source != inventory.source
            || intent.mappings != inventory.mappings
        {
            return Err(SourceCoordinationError::Invalid);
        }
        let proof = Self {
            operation_id: operation_id.into(),
            inventory,
        };
        proof.inventory_digest()?;
        proof
            .validate(identity, lease, context)
            .map_err(|_| SourceCoordinationError::Stale)?;
        proof.restore_original_links(context)?;
        Ok(proof)
    }

    /// Replay the original pure patch over every discovered before image.
    /// The callback is supplied by the orchestration owner, outside PAIR guards.
    pub(crate) fn verify_original_classification(
        &self,
        classify: &dyn Fn(&SourceInventoryTarget) -> Result<bool, IdentityReferenceError>,
    ) -> Result<(), IdentityReferenceError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(IdentityReferenceError::Conflict)?;
        let mut selected = Vec::new();
        for evidence in &original.discovered {
            if classify(&evidence.target)? != evidence.affected {
                return Err(IdentityReferenceError::Conflict);
            }
            if evidence.affected {
                let mut target = evidence.target.clone();
                target.original_physical = None;
                selected.push(target);
            }
        }
        if selected != self.inventory.targets {
            return Err(IdentityReferenceError::Conflict);
        }
        Ok(())
    }

    /// Fence-only invariant; later PAIR progress legitimately changes revisions.
    pub(crate) fn verify_affected_before_fence(&self) -> Result<(), SourceCoordinationError> {
        if self.inventory.affected_original_protected_matches()? {
            Ok(())
        } else {
            Err(SourceCoordinationError::Stale)
        }
    }

    pub(crate) fn verify_complete_discovery(
        &self,
        witness: &SourceDiscoveryWitness,
        current: &[SourceDiscoveredTargetEvidence],
    ) -> Result<(), SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if witness != &original.discovery || current.len() != original.discovered.len() {
            return Err(SourceCoordinationError::Stale);
        }
        let mut links = Vec::new();
        for (now_evidence, before_evidence) in current.iter().zip(&original.discovered) {
            if !before_evidence.affected
                && now_evidence.target.before_protected != before_evidence.target.before_protected
            {
                return Err(SourceCoordinationError::Stale);
            }
        }
        for (now, before) in current.iter().zip(&original.discovered) {
            let now = &now.target;
            let before = &before.target;
            if now.root != before.root
                || now.root_object != before.root_object
                || now.decisions != before.decisions
                || now.state != before.state
                || now.reservation != before.reservation
            {
                return Err(SourceCoordinationError::Stale);
            }
        }
        for evidence in current {
            for (slot, selection) in &evidence.slots {
                if selection.source == self.inventory.source {
                    links.push((
                        evidence.target.root.clone(),
                        slot.coordinate(),
                        selection.source.clone(),
                        self.inventory.physical_source_key.clone(),
                    ));
                }
            }
        }
        let mut topology = original.topology.clone();
        topology.links = links;
        if source_inventory_digest(
            &self.inventory.editor_id,
            &self.inventory.source,
            &self.inventory.physical_source_key,
            &self.inventory.context_revision,
            &topology,
        )? != original.inventory_digest
        {
            return Err(SourceCoordinationError::Stale);
        }
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static SOURCE_FACTORY_GUARD_CUT: std::cell::Cell<(usize, Option<usize>)> =
        const { std::cell::Cell::new((0, None)) };
}

#[cfg(test)]
pub(crate) fn configure_source_factory_guard_cut(fail_at: Option<usize>) {
    SOURCE_FACTORY_GUARD_CUT.with(|state| state.set((0, fail_at)));
}

#[cfg(test)]
pub(crate) fn source_factory_guard_calls() -> usize {
    SOURCE_FACTORY_GUARD_CUT.with(|state| state.get().0)
}

#[cfg(test)]
fn source_factory_guard_cut() -> Result<(), IdentityReferenceError> {
    SOURCE_FACTORY_GUARD_CUT.with(|state| {
        let (count, fail_at) = state.get();
        let count = count + 1;
        if fail_at == Some(count) {
            state.set((count, None));
            return Err(IdentityReferenceError::Conflict);
        }
        state.set((count, fail_at));
        Ok(())
    })
}

impl SourceInventoryProof {
    /// Rebuild original proof groups from the SAME saved before source and target
    /// images, after current source/packet authority has been independently checked.
    fn restore_original_links(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let raw = match &original.source_before {
            IdentityReferencePhysicalState::Absent => OriginPhysicalInput::Absent,
            IdentityReferencePhysicalState::Bytes { bytes, .. } => {
                if IdentityReferencePhysicalState::from_bytes(bytes.clone())
                    != original.source_before
                {
                    return Err(SourceCoordinationError::Invalid);
                }
                OriginPhysicalInput::Bytes(bytes.clone())
            }
        };
        if raw.revision().as_ref() != Some(&original.expected_source) {
            return Err(SourceCoordinationError::Invalid);
        }
        let binding = self
            .inventory
            .origin_binding
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let mut inputs = OriginInputs::read(context);
        if inputs.binding.settings != binding.settings
            || inputs.binding.overlay != binding.overlay
            || inputs.binding.context != binding.context
        {
            return Err(SourceCoordinationError::Stale);
        }
        if self.inventory.source.kind == SourceKind::RegisteredInstance {
            if binding.registered.as_ref() != Some(&original.expected_source) {
                return Err(SourceCoordinationError::Invalid);
            }
            inputs.registered = raw.clone();
        } else if inputs.binding.registered != binding.registered {
            return Err(SourceCoordinationError::Stale);
        }
        inputs.binding = binding.clone();
        let origin = LegacyCompatibilityOrigin::classify(&inputs);
        let snapshot = source_snapshot_from_original(context, &self.inventory.source, raw)
            .map_err(|_| SourceCoordinationError::Invalid)?;
        let captured = SourceInventoryCapture::capture(
            original.discovery.clone(),
            original.discovered.clone(),
            &snapshot,
            &origin,
            &self.inventory.physical_source_key,
        )
        .map_err(|_| SourceCoordinationError::Invalid)?;
        if captured.objects != original.objects || captured.topology != original.topology {
            return Err(SourceCoordinationError::Stale);
        }
        let selected: Vec<_> = original
            .discovered
            .iter()
            .filter(|target| target.affected)
            .map(|record| {
                let mut target = record.target.clone();
                target.original_physical = None;
                target
            })
            .collect();
        if selected != self.inventory.targets {
            return Err(SourceCoordinationError::Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn source_factory_guard_cut_armed() -> bool {
    SOURCE_FACTORY_GUARD_CUT.with(|state| state.get().1.is_some())
}

#[cfg(test)]
pub(crate) fn source_transition_test_root() -> tempfile::TempDir {
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/i2897-p03");
    std::fs::create_dir_all(&scratch).unwrap();
    tempfile::Builder::new()
        .prefix("source-transition-")
        .tempdir_in(scratch)
        .unwrap()
}

#[cfg(test)]
pub(crate) fn source_transition_test_locator(
    context: &SourceReadContext,
    source: &SourceRef,
) -> Result<PathBuf, SourcePreparationError> {
    context.locator(source)
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

pub(super) fn agent_file_read(path: &Path) -> Result<Vec<u8>, String> {
    crate::path_identity::read_bounded_regular(path, SETTINGS_CAPTURE_LIMIT).map(|(bytes, _)| bytes)
}

fn agent_file_decode(
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

fn snapshot_from_captured_source(
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

fn agent_file_sources_reply(
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
fn deserialize_agent_file_source_preferences<'de, D>(
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
enum BoundedSourceObservation {
    Absent,
    Bytes {
        bytes: Vec<u8>,
        witness: crate::path_identity::VerifiedPathIdentity,
    },
    Failure(String),
}

impl BoundedSourceObservation {
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes { bytes, witness } => {
                let _captured_identity = witness;
                Some(bytes)
            }
            _ => None,
        }
    }

    fn profile_input(&self) -> Result<Option<Vec<u8>>, String> {
        match self {
            Self::Absent => Ok(None),
            Self::Bytes { bytes, .. } => Ok(Some(bytes.clone())),
            Self::Failure(kind) => Err(kind.clone()),
        }
    }
}

fn observe_agent_file(path: &Path) -> BoundedSourceObservation {
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

fn same_captured_source(left: &CapturedSource, right: &CapturedSource) -> bool {
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
struct AgentFileAuthority {
    base: CapturedSource,
    project_paths: CapturedSource,
    context: SourceReadContext,
    root: crate::path_identity::RetainedDirectory,
    raw_main: Map<String, Value>,
}

impl AgentFileAuthority {
    fn capture(guard: &SettingsOperationGuard<'_>) -> Result<Self, String> {
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

    fn recheck(&self, guard: &SettingsOperationGuard<'_>) -> Result<(), String> {
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

    fn preferences(&self) -> Result<AgentFileSourcePreferences, String> {
        self.raw_main
            .get("agentFileSourceByMenu")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "invalidSourcePreferences".into())
            .map(|value| value.unwrap_or_default())
    }

    fn registered_validation(&self) -> (BoundedSourceObservation, RegisteredProfileCapture) {
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

    fn read_agent_files_blocking(
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
    owner: &'op SettingsOwner,
    guard: &'op mut SettingsOperationGuard<'lease>,
    authority: AgentFileAuthority,
    source: SourceRef,
    path: PathBuf,
    expected: SourceRevision,
    adopted: AppSettings,
}

impl CatalogSourceOperationGuard<'_, '_> {
    pub(crate) fn target(&self) -> &Path {
        &self.path
    }

    pub(super) fn verify_target_parent_chain(&self) -> Result<(), String> {
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
type AgentFileTestHook = Box<dyn FnMut(&str, &Path)>;

#[cfg(test)]
thread_local! {
    static AGENT_FILE_CHECKPOINT: std::cell::RefCell<Option<AgentFileTestHook>> =
        const { std::cell::RefCell::new(None) };
}

fn agent_file_checkpoint(stage: &str, path: &Path) {
    #[cfg(test)]
    AGENT_FILE_CHECKPOINT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(stage, path);
        }
    });
    #[cfg(not(test))]
    let _ = (stage, path);
}
