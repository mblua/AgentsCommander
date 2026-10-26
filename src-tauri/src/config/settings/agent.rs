mod blocking_menu;
mod command;
mod file;
mod help;
mod instance;
mod profile;

use super::{default_true, AppSettings, ConfigSeedConfig};
use crate::config::placeholders::AC_PLACEHOLDER_TOKENS;
use crate::pty::backend::SessionBackendKind;
pub(crate) use blocking_menu::{
    accept_remote_blocking_menus_response, export_blocking_menus_with_startup, pretty_json_bytes,
    read_remote_blocking_menus_check_stamp, write_remote_blocking_menus_cache,
    write_remote_blocking_menus_check_stamp, REMOTE_BLOCKING_MENUS_MAX_BYTES,
};
pub use blocking_menu::{
    apply_issue_1757_migration, default_blocking_menus_for_command, shipped_blocking_menus,
    BlockingMenuConfig, BlockingMenuEntry, BlockingMenusFile, BlockingMenusStore,
    BLOCKING_MENUS_SCHEMA_VERSION,
};
#[cfg(test)]
pub(crate) use blocking_menu::{
    blocking_menus_local_path, blocking_menus_remote_check_path, blocking_menus_remote_path,
    blocking_menus_shipped_path, codex_hooks_review_menu, export_blocking_menus_to_local_file,
    load_local_blocking_menus_file, load_remote_blocking_menus_file, parse_blocking_menus_file,
    refresh_shipped_blocking_menus_file, validate_remote_blocking_menus_file,
    CODEX_HOOKS_REVIEW_PATTERN, EMBEDDED_BLOCKING_MENUS_JSON, REMOTE_BLOCKING_MENUS_SOURCE_REF,
};
#[cfg(test)]
pub(crate) use command::known_agent_field;
pub use command::validate_agent_commands;
pub(crate) use command::{
    command_token_basename, comparable_agent, deserialize_present_command,
    raw_configuration_command, retain_configuration_commands,
    retain_configuration_commands_with_startup, select_configuration_command,
    validate_agent_command_text, StartupAgentRows, CONFIGURATION_COMMAND_PAIRS,
};
pub(crate) use file::{
    agent_file_projection, deserialize_agent_file_source_preferences, persist_agent_file_edits,
    remember_agent_file_locked, AgentFileReadReply, AgentFileReadRequest,
    CatalogSourceOperationGuard,
};
#[cfg(test)]
pub(crate) use file::{
    agent_file_read, list_agent_files, prepare_agent_file_edits, read_agent_file_snapshot,
    save_agent_file_locked, AGENT_FILE_CHECKPOINT,
};
pub use file::{
    AgentFileEdits, AgentFileProfileEdit, AgentFileRow, AgentFileRowAction, AgentFileRowEdit,
    AgentFileRowKind, AgentFileSaveOutcome, AgentFileSaveReply, AgentFileSourceFormat,
    AgentFileSourceMenu, AgentFileSourceOption, AgentFileSourcePreferences, AgentFileSources,
    SelectedAgentFileSnapshot,
};
pub(crate) use help::{
    accept_remote_agent_help_response, load_local_agent_help_file, load_remote_agent_help_file,
    read_remote_agent_help_check_stamp, write_remote_agent_help_cache,
    write_remote_agent_help_check_stamp, REMOTE_AGENT_HELP_MAX_BYTES,
};
#[cfg(test)]
pub(crate) use help::{
    agent_help_local_path, agent_help_remote_check_path, agent_help_remote_path,
    agent_help_shipped_path, parse_agent_help_file, refresh_shipped_agent_help_file,
    refresh_shipped_agent_help_file_with_publish, validate_remote_agent_help_file,
    EMBEDDED_AGENT_HELP_JSON, REMOTE_AGENT_HELP_SOURCE_REF,
};
pub use help::{
    refresh_shipped_agent_help_from_config_dir, shipped_agent_help, AgentHelpEntry, AgentHelpFile,
    AgentHelpLink, AgentHelpTip, AGENT_HELP_SCHEMA_VERSION,
};
#[cfg(test)]
pub(crate) use instance::{
    absent_main_overlay_comparison, agents_unreadable_startup_notice,
    hold_agents_layer_state_for_test, try_with_agents_layer_publication_state_in,
    AgentsPublicationLockError,
};
pub(crate) use instance::{
    agents_instance_groups, agents_instance_path, agents_layer_state_at,
    agents_layer_state_at_labeled, agents_layer_unreadable_for_dir, agents_unreadable_cli_error,
    agents_unreadable_session_error, export_agents_to_instance_file, merge_agents_instance_file,
    merge_agents_instance_groups, parse_agents_instance_contents, prepare_registered_profile_owner,
    read_agents_instance_file, read_agents_instance_file_labeled,
    restore_agents_groups_for_redecode, restore_codex_home_carrier,
    restrict_split_profile_mutation_rows, retain_selected_legacy_profile_aliases,
    retain_unknown_registered_agent_fields, save_agents_instance_file, set_agents_layer_state,
    try_with_agents_layer_publication_state, validate_registered_overlay_profile_edits,
    validate_split_canonical_profile_edits, AgentsLayerState, AGENTS_INSTANCE_KEYS,
};
pub(crate) use profile::{
    adopt_orphaned_profiles, bootstrap_binding_revision, canonical_profile_source,
    capture_registered_profile_gate, captured_registered_profile_object,
    classify_registered_profile_inputs, coding_agent_identity_physical_from_raw,
    coding_agent_identity_with_physical_overlay, collect_registered_profile_gate,
    decode_physical_catalog, duplicate_profile_document, normalized_profile_cells,
    physical_profile_rows, physical_profile_sets, profile_container, project_physical_profiles,
    recheck_registered_profile_gate, refresh_coding_agent_identity_at_locked_path,
    refresh_physical_source_rows, registered_profile_raw_gate, registered_profiles_for_read,
    selected_registered_profile_owner, write_identity_rows, RegisteredProfileCapture,
    RegisteredProfileDocument, RegisteredProfileProblemKind,
};
pub use profile::{
    agent_profile_identity, empty_profile_cell, is_valid_profile_letter, normalize_profile_letter,
    repair_coding_agent_profiles_config, ProfileCellConfig, ProfileSlotConfig,
};
#[cfg(test)]
pub(crate) use profile::{bootstrap_identity_revision, is_profile_gap, validate_profile_orphans};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

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

pub use crate::pty::container_paths::normalize_env_key_for_platform;

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
