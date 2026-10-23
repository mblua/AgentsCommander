use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::agent_command::is_safe_instructions_filename;
use crate::config::settings::{
    validate_agent_command_text, validate_config_seed_dest, validate_env_rows,
    validate_identity_snapshot, AppSettings, CodingAgentEnv, ConfigSeedConfig,
    IdentitySnapshotEntry, IdentityValidationError, ProfileCellConfig,
};

use super::{
    load_catalog_report, load_catalog_report_for_settings_with_config_dir, local_catalog_path,
    parse_strict_json, project_catalog_path, validate_update_command_string, CatalogUnavailable,
    BUILTIN_TESTED_LEVEL, CATALOG_DIR_NAME, CATALOG_MANIFEST_FILENAME, CATALOG_SCHEMA_VERSION,
    EMBEDDED_DEFAULT_CATALOG_JSON,
};

/// #2736 - wire strings are `"high" | "medium" | "low"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TestedLevel {
    High,
    Medium,
    Low,
}

/// #2736 - the tested level of a built-in key; `None` for any other key.
pub(crate) fn tested_level_for(key: &str) -> Option<TestedLevel> {
    BUILTIN_TESTED_LEVEL
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .map(|(_, level)| *level)
}

/// #2736 - the install command for THIS host: the platform key when present,
/// else `default`. `windows` for target_os = "windows", `macos` for "macos",
/// `linux` for "linux"; every other target uses `default`.
pub(crate) fn resolve_install_command(commands: &InstallCommands) -> &str {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };
    install_command_for_os(commands, os)
}

pub(crate) fn install_command_for_os<'a>(commands: &'a InstallCommands, os: &str) -> &'a str {
    let platform = match os {
        "windows" => commands.windows.as_deref(),
        "macos" => commands.macos.as_deref(),
        "linux" => commands.linux.as_deref(),
        _ => None,
    };
    platform.unwrap_or(&commands.default)
}

/// #2736 - is this catalog entry's command present on this machine? PATH math
/// only: `normalize_legacy_agent_command` reduces the catalog string to its
/// program token, then `agent_command::resolve_program` resolves it (bare name
/// through `effective_search_path` plus PATHEXT on Windows; explicit path by
/// is_file()). NO process is executed and nothing is cached or persisted.
pub(crate) fn command_is_present(command: &str) -> bool {
    command_is_present_with(command, crate::config::agent_command::resolve_program)
}

/// Testable twin of [`command_is_present`] with the resolver injected
/// (mirrors `resolve_command_install_probe_with`).
pub(crate) fn command_is_present_with(
    command: &str,
    resolve: impl FnOnce(&str) -> Option<PathBuf>,
) -> bool {
    match crate::config::agent_command::normalize_legacy_agent_command(command) {
        Ok(normalized) if !normalized.shell.is_empty() => resolve(&normalized.shell).is_some(),
        _ => false,
    }
}

/// #2736 - one welcome-status row. A SEPARATE type from
/// `CodingAgentDefinition`: that struct is persisted and hashed, and a catalog
/// field would be patchable from disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentWelcomeStatus {
    pub key: String,
    pub installed: bool,
    pub tested_level: Option<TestedLevel>,
    pub install_command: Option<String>,
}

/// One row per effective catalog entry, in catalog order.
pub fn welcome_status_for(catalog: &[CodingAgentDefinition]) -> Vec<CodingAgentWelcomeStatus> {
    welcome_status_for_with(catalog, command_is_present)
}

pub(crate) fn welcome_status_for_with(
    catalog: &[CodingAgentDefinition],
    is_present: impl Fn(&str) -> bool,
) -> Vec<CodingAgentWelcomeStatus> {
    catalog
        .iter()
        .map(|def| CodingAgentWelcomeStatus {
            key: def.key.clone(),
            installed: is_present(&def.command),
            tested_level: tested_level_for(&def.key),
            install_command: def
                .install_commands
                .as_ref()
                .map(resolve_install_command)
                .map(str::to_string),
        })
        .collect()
}

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn default_catalog_schema_version() -> u32 {
    CATALOG_SCHEMA_VERSION
}

/// #2124 - optional per-agent idle-burst filter tuning. A present object on the
/// effective catalog entry activates the filter for mapped sessions; absent or
/// `null` leaves it off. Every subfield is optional: an absent one takes the
/// default declared in `session/profile.rs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IdleBurstConfig {
    /// Byte total (printable or not) that confirms a pending output burst.
    /// `0` disables the filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Longest gap a pending burst may span before it is discarded as a short
    /// burst. `0` disables the filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_secs: Option<f64>,
    /// Silence age a chunk must find before it may open a burst candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_silence_secs: Option<f64>,
}

/// #2736 - optional per-OS install command for a coding agent. Every value is
/// ONE complete shell command string, never argv tokens. `default` applies to
/// any platform without its own key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstallCommands {
    pub default: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macos: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<String>,
}

/// One catalog entry: a built-in (or user-added) coding agent the user can pick
/// from. Maps cleanly onto `Omit<AgentConfig,"id">` plus `{key, description,
/// removable}` on the frontend. `removable`, `envs`, and `isolated_home` are
/// serde-guaranteed present (the frontend types them as required);
/// `instructions_filename` and `config_seed` are optional.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentDefinition {
    /// Stable identity within the catalog. Must match `^[a-z0-9-]+$` and be
    /// unique; it doubles as a testid/JSON-key/CSS token on the frontend and (in
    /// the Full phase) a directory name.
    pub key: String,
    pub label: String,
    pub description: String,
    pub color: String,
    #[serde(rename = "baseCommand")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    #[serde(default)]
    pub envs: Vec<CodingAgentEnv>,
    #[serde(default)]
    pub isolated_home: bool,
    /// Optional per-agent config-folder seed. Passthrough/authoring data in
    /// Phase 1: the embedded default ships it UNSET, so no agent seeds a folder
    /// and spawn behavior is byte-unchanged. Phase 2 wires it to the #598 tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed: Option<ConfigSeedConfig>,
    /// May the user delete this built-in from the catalog? Ships `true` for all
    /// built-ins; defaults `true` so a hand-authored entry omitting it stays
    /// deletable.
    #[serde(default = "default_true")]
    pub removable: bool,
    /// #1318/#1323 - per-agent update-command sequence seeded by AC: an ORDERED
    /// array of COMPLETE command strings, each executed sequentially
    /// (updateCommands[0], then [1], ...) to install a new agent version, e.g.
    /// `["claude --update"]` or `["claude --update", "npm i -g @scope/cli"]`.
    /// NOT argv tokens: each element is one full shell command passed as-is,
    /// in array order. If a vendor changes its update command, updates stop
    /// working until a new release or the user edits the seeded file. Empty =
    /// no update command (agent cannot auto-update). Consumed by the
    /// follow-up update-check feature only.
    #[serde(default)]
    pub update_commands: Vec<String>,
    /// #1318 - stable catalog default for auto-update. Newly registered agents
    /// default to false ("No"). The per-user choice lives in
    /// `AppSettings.agent_auto_update_by_command`, keyed by command. Inert: the
    /// runtime reads only the settings map.
    #[serde(default)]
    pub auto_update: bool,
    /// #2124 - optional idle-burst filter for this agent's sessions. Declared
    /// after the pre-#2124 fields on purpose: field order is serialization
    /// order, and the managed revision hash is taken over this serialization.
    /// `install_commands` (#2736) now follows it for the same reason, so a new
    /// field never reorders the bytes of an existing one. NOT part of the
    /// `settings.agents[]` snapshot: it is resolved from the effective catalog
    /// at every spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_burst: Option<IdleBurstConfig>,
    /// #2736 - optional per-OS install command. One COMPLETE shell command string
    /// per platform key; `default` is required when the object is present. Declared
    /// LAST for the same reason as `idle_burst`: field order is serialization order
    /// and the managed revision hash is taken over this serialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_commands: Option<InstallCommands>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodingAgentDefinitionWire {
    pub key: String,
    pub label: String,
    pub description: String,
    pub color: String,
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
    pub instructions_filename: Option<String>,
    #[serde(default)]
    pub envs: Vec<CodingAgentEnv>,
    #[serde(default)]
    pub isolated_home: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed: Option<ConfigSeedConfig>,
    #[serde(default = "default_true")]
    pub removable: bool,
    #[serde(default)]
    pub update_commands: Vec<String>,
    #[serde(default)]
    pub auto_update: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_burst: Option<IdleBurstConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_commands: Option<InstallCommands>,
}

impl<'de> Deserialize<'de> for CodingAgentDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = CodingAgentDefinitionWire::deserialize(deserializer)?;
        let command = crate::config::settings::select_configuration_command(
            wire.canonical_command.as_deref(),
            wire.command.as_deref(),
        )
        .ok_or_else(|| <D::Error as serde::de::Error>::missing_field("baseCommand"))?
        .to_owned();
        Ok(Self {
            key: wire.key,
            label: wire.label,
            description: wire.description,
            color: wire.color,
            command,
            instructions_filename: wire.instructions_filename,
            envs: wire.envs,
            isolated_home: wire.isolated_home,
            config_seed: wire.config_seed,
            removable: wire.removable,
            update_commands: wire.update_commands,
            auto_update: wire.auto_update,
            idle_burst: wire.idle_burst,
            install_commands: wire.install_commands,
        })
    }
}

/// The manifest file shape: a schema version plus the ordered agent list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentCatalog {
    #[serde(default = "default_catalog_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub agents: Vec<CodingAgentDefinition>,
}

/// #2892: an independent schema2 candidate, never a schema1 patch/composition.
/// Existing readers, seeders and writers continue to use CodingAgentCatalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentCatalogSchema2 {
    pub schema_version: u32,
    pub agents: Vec<CodingAgentDefinition>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub coding_agent_profiles:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, ProfileCellConfig>>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub profile_labels:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityCatalogError {
    #[error("independentFormatRequired")]
    IndependentFormatRequired,
    #[error("invalid independent catalog: {0}")]
    InvalidFormat(String),
}

impl CodingAgentCatalogSchema2 {
    /// Explicit editor template only; not a seed, migration or reader fallback.
    pub fn empty() -> Self {
        Self {
            schema_version: 2,
            agents: Vec::new(),
            coding_agent_profiles: std::collections::BTreeMap::new(),
            profile_labels: std::collections::BTreeMap::new(),
        }
    }

    /// Kept separate from decoding: historical identities may be diagnosed
    /// without losing a structurally valid file or treating it as absent.
    pub fn validate_identities(&self) -> Result<(), IdentityValidationError> {
        let entries: Vec<_> = self
            .agents
            .iter()
            .map(|agent| IdentitySnapshotEntry {
                key: &agent.key,
                name: &agent.label,
                command: &agent.command,
                envs: &agent.envs,
                profiles: self.coding_agent_profiles.get(&agent.key),
            })
            .collect();
        validate_identity_snapshot(&entries)
    }
}

/// Pure, duplicate-key-aware schema2 decoder. It supplies only the model's own
/// serde defaults and never reads a second source, adapts schema1 or writes IO.
pub fn parse_catalog_schema2(
    bytes: &[u8],
) -> Result<CodingAgentCatalogSchema2, IdentityCatalogError> {
    let value = parse_strict_json(bytes).map_err(IdentityCatalogError::InvalidFormat)?;
    match value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
    {
        Some(2) => {}
        Some(1) => return Err(IdentityCatalogError::IndependentFormatRequired),
        _ => {
            return Err(IdentityCatalogError::InvalidFormat(
                "schemaVersion must be 2".into(),
            ));
        }
    }
    let projected = if value.get("profileBindingVersion").is_some() {
        crate::config::settings::project_physical_profiles(&value, false)
            .map_err(IdentityCatalogError::InvalidFormat)?
    } else {
        crate::config::settings::physical_profile_sets(&value, false)
            .map_err(IdentityCatalogError::InvalidFormat)?;
        let mut legacy = value;
        let keys: HashSet<String> = legacy
            .get("agents")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                row.get("key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        for field in ["codingAgentProfiles", "profileLabels"] {
            if let Some(map) = legacy
                .get_mut(field)
                .and_then(serde_json::Value::as_object_mut)
            {
                map.retain(|key, _| keys.contains(key));
            }
        }
        legacy
    };
    let mut catalog: CodingAgentCatalogSchema2 = serde_json::from_value(projected)
        .map_err(|error| IdentityCatalogError::InvalidFormat(error.to_string()))?;
    for agent in &catalog.agents {
        for cell in catalog
            .coding_agent_profiles
            .get_mut(&agent.key)
            .into_iter()
            .flat_map(|cells| cells.values_mut())
        {
            cell.id_hash = Some(crate::config::settings::pair_identity(
                &agent.command,
                &agent.envs,
                cell,
            ));
            cell.id_hash_debug = Some(crate::config::settings::pair_identity_debug(
                &agent.command,
                &agent.envs,
                cell,
            ));
        }
    }
    let mut keys = HashSet::new();
    for agent in &catalog.agents {
        if agent.command.trim().is_empty() {
            return Err(IdentityCatalogError::InvalidFormat(format!(
                "catalog key '{}' requires an explicit nonempty command",
                agent.key
            )));
        }
        validate_definition(agent).map_err(IdentityCatalogError::InvalidFormat)?;
        if !keys.insert(agent.key.as_str()) {
            return Err(IdentityCatalogError::InvalidFormat(format!(
                "duplicate catalog key '{}'",
                agent.key
            )));
        }
    }
    for (key, letters) in catalog
        .coding_agent_profiles
        .iter()
        .map(|(key, cells)| (key, cells.keys().collect::<Vec<_>>()))
        .chain(
            catalog
                .profile_labels
                .iter()
                .map(|(key, labels)| (key, labels.keys().collect::<Vec<_>>())),
        )
    {
        if !keys.contains(key.as_str()) {
            return Err(IdentityCatalogError::InvalidFormat(format!(
                "profile map references unknown catalog key '{key}'"
            )));
        }
        if letters
            .iter()
            .any(|letter| letter.len() != 1 || !letter.as_bytes()[0].is_ascii_uppercase())
        {
            return Err(IdentityCatalogError::InvalidFormat(format!(
                "profile letters for '{key}' must be A through Z"
            )));
        }
    }
    Ok(catalog)
}

/// #2893: source-local decoding. Schema1 is a complete base only; project and
/// personal patches cannot donate commands or inherit another source's fields.
#[allow(dead_code)] // P02 reader; production activation is a later cut.
pub(crate) fn parse_independent_catalog(
    bytes: &[u8],
    base: bool,
) -> Result<CodingAgentCatalogSchema2, IdentityCatalogError> {
    let value = parse_strict_json(bytes).map_err(IdentityCatalogError::InvalidFormat)?;
    if value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        == Some(2)
    {
        return parse_catalog_schema2(bytes);
    }
    if !base
        || value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
    {
        return Err(IdentityCatalogError::IndependentFormatRequired);
    }
    let catalog: CodingAgentCatalog = serde_json::from_value(value)
        .map_err(|e| IdentityCatalogError::InvalidFormat(e.to_string()))?;
    let mut keys = HashSet::new();
    for agent in &catalog.agents {
        if agent.command.trim().is_empty() || !keys.insert(&agent.key) {
            return Err(IdentityCatalogError::InvalidFormat(
                "incomplete or duplicate base entry".into(),
            ));
        }
        validate_definition(agent).map_err(IdentityCatalogError::InvalidFormat)?;
    }
    Ok(CodingAgentCatalogSchema2 {
        schema_version: 2,
        agents: catalog.agents,
        coding_agent_profiles: Default::default(),
        profile_labels: Default::default(),
    })
}

#[allow(dead_code)] // P02 reader; production activation is a later cut.
pub(crate) fn independent_catalog_path(
    ac_dir: &Path,
    kind: crate::config::settings::SourceKind,
) -> Option<PathBuf> {
    use crate::config::settings::SourceKind;
    match kind {
        SourceKind::CatalogBase => Some(catalog_dir(ac_dir).join(CATALOG_MANIFEST_FILENAME)),
        SourceKind::CatalogProject => Some(project_catalog_path(ac_dir)),
        SourceKind::CatalogPersonal => Some(local_catalog_path(ac_dir)),
        SourceKind::RegisteredInstance => None,
    }
}

/// The catalog subdirectory of a project's `.ac` dir.
pub(crate) fn catalog_dir(ac_dir: &Path) -> PathBuf {
    ac_dir.join(CATALOG_DIR_NAME)
}

pub(crate) fn manifest_path(ac_dir: &Path) -> PathBuf {
    catalog_dir(ac_dir).join(CATALOG_MANIFEST_FILENAME)
}

/// Parse the compiled-in default catalog. The content is authored valid and a
/// unit test guards it, so the error branch is unreachable in practice; it logs
/// and returns an empty catalog rather than panicking (this runs on the boot and
/// IPC paths). RAW and UNGATED by design: seed material and test expectations
/// only; every consumer-facing read goes through the persisted-only report
/// resolver.
pub(crate) fn embedded_default_catalog() -> CodingAgentCatalog {
    serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap_or_else(|e| {
        log::error!("[coding-agents] embedded default catalog failed to parse: {e}");
        CodingAgentCatalog {
            schema_version: CATALOG_SCHEMA_VERSION,
            agents: Vec::new(),
        }
    })
}

/// G6: a catalog `key` must be a non-empty `^[a-z0-9-]+$` token. Stricter than
/// `validate_config_seed_dest` on purpose: the key is used verbatim as a testid,
/// a JSON object key, and a CSS token on the frontend (and, in the Full phase, a
/// directory name), so it is restricted to an unambiguous ASCII allowlist.
pub(crate) fn validate_catalog_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("coding-agent key must not be empty".to_string());
    }
    if !key
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(format!(
            "coding-agent key '{key}' must match ^[a-z0-9-]+$ (lowercase letters, digits, hyphen)"
        ));
    }
    Ok(())
}

/// Per-entry validation (G7). Reuses the same validators the settings save path
/// uses so the catalog and `settings.agents` cannot disagree about what is legal.
pub(crate) fn validate_definition(def: &CodingAgentDefinition) -> Result<(), String> {
    validate_catalog_key(&def.key)?;
    let context = format!("Coding agent '{}'", def.key);
    validate_agent_command_text(&context, &def.command)?;
    validate_env_rows(&def.envs, &context)?;
    if let Some(name) = def.instructions_filename.as_deref() {
        if !is_safe_instructions_filename(name) {
            return Err(format!("{context}: unsafe instructions filename '{name}'"));
        }
    }
    if let Some(cfg) = def.config_seed.as_ref() {
        if !cfg.dest.trim().is_empty() {
            validate_config_seed_dest(&cfg.dest)?;
        }
    }
    if let Some(burst) = def.idle_burst.as_ref() {
        validate_idle_burst_config(burst, &context)?;
    }
    if let Some(install) = def.install_commands.as_ref() {
        validate_install_commands(install, &context)?;
    }
    Ok(())
}

/// #2736 - every present `installCommands` value obeys the per-string
/// `updateCommands` rules (non-blank, no control character, U+2028 or U+2029).
/// The field name goes in the context; the command text is never echoed.
pub(crate) fn validate_install_commands(
    value: &InstallCommands,
    context: &str,
) -> Result<(), String> {
    for (name, command) in [
        ("default", Some(&value.default)),
        ("windows", value.windows.as_ref()),
        ("macos", value.macos.as_ref()),
        ("linux", value.linux.as_ref()),
    ] {
        if let Some(command) = command {
            validate_update_command_string(
                command,
                &format!("{context} installCommands.{name}"),
                0,
            )?;
        }
    }
    Ok(())
}

/// #2124 - the `idleBurst` value rules, shared by the base path
/// (`validate_definition`) and, by construction, the strict local parser:
/// `maxBytes` is a non-negative integer and is already typed `u64` here;
/// `maxSecs` and `priorSilenceSecs` must be finite and >= 0. Absent is legal
/// and means "use the `session/profile.rs` default".
pub(crate) fn validate_idle_burst_config(
    config: &IdleBurstConfig,
    context: &str,
) -> Result<(), String> {
    for (name, value) in [
        ("maxSecs", config.max_secs),
        ("priorSilenceSecs", config.prior_silence_secs),
    ] {
        if let Some(value) = value {
            if !value.is_finite() || value < 0.0 {
                return Err(format!(
                    "{context}: 'idleBurst.{name}' must be a finite number >= 0"
                ));
            }
        }
    }
    Ok(())
}

/// `false` only for a key present in `table` with a `false` row.
pub(crate) fn is_supported_builtin(key: &str, table: &[(&str, bool)]) -> bool {
    !table.iter().any(|(k, on)| *k == key && !*on)
}

/// #1967 P4 - the array-returning read adapter over the persisted-only report.
/// `Ok` carries the validated persisted definitions exactly as the report
/// selected them (a valid empty list is honored verbatim: the user removed all
/// built-ins); `Err` carries the report's unavailability diagnostic (code +
/// path + reason). The embedded default is NEVER substituted on this path and
/// nothing is written, seeded or created. Every report warning is logged so
/// array consumers get the same diagnostics the report IPC carries
/// structurally. `ac_dir` is the project's `.ac` directory (or, in
/// no-project mode, the legacy config dir, which yields
/// `<config_dir>/coding-agents/agents.json` through the same relative layout).
pub fn load_catalog(ac_dir: &Path) -> Result<Vec<CodingAgentDefinition>, CatalogUnavailable> {
    load_catalog_report(ac_dir).into_result()
}

/// The first non-empty trimmed entry of `project_paths`, else the legacy
/// `project_path` (non-empty trimmed), else `None`. Single deterministic head
/// rule, mirroring the canonical `selected_head: project_paths.first()`
/// semantics (`settings.rs`). No canonicalization: a stale raw path is reported
/// as `baseUnavailable` at read time (absent file -> unavailable, absent dir ->
/// fail-soft seed skip). Archived projects are never the primary.
pub(crate) fn primary_project_root(settings: &AppSettings) -> Option<PathBuf> {
    for entry in &settings.project_paths {
        let trimmed = entry.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    settings
        .project_path
        .as_deref()
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
}

/// Every non-empty trimmed `project_paths` entry (order preserved); when the
/// list is empty, the legacy `project_path` alone (non-empty). Archived projects
/// are never seeded.
pub(crate) fn registered_project_roots(settings: &AppSettings) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = settings
        .project_paths
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect();
    if roots.is_empty() {
        if let Some(path) = settings
            .project_path
            .as_deref()
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            roots.push(PathBuf::from(path));
        }
    }
    roots
}

/// The catalog read root for the UI/CLI read commands. With a primary project
/// root the primary project's `.ac/coding-agents` catalog is served and the
/// legacy location is NEVER consulted (a user who deletes the primary file gets
/// `baseUnavailable`, never the legacy copy or the embedded default). With NO
/// registered project the LEGACY `<config_dir>/coding-agents` catalog is served
/// when one exists (read-only, never written; pre-migration installs with zero
/// projects keep today's read behavior); an absent instance catalog is
/// `baseUnavailable`, never the embedded default.
///
/// #1967 P4 - Result adapter over `load_catalog_report_for_settings`: one
/// settings snapshot selects the root and the untouched report drives both the
/// selection and the diagnostics. P5 (#1968) extends the resolver behind this
/// same adapter.
pub fn load_catalog_for_settings(
    settings: &AppSettings,
) -> Result<Vec<CodingAgentDefinition>, CatalogUnavailable> {
    load_catalog_for_settings_with_config_dir(settings, crate::config::config_dir())
}

/// Testable twin of [`load_catalog_for_settings`] with the resolved config dir
/// injected, mirroring `load_catalog_report_for_settings_with_config_dir`.
pub(crate) fn load_catalog_for_settings_with_config_dir(
    settings: &AppSettings,
    config_dir: Option<PathBuf>,
) -> Result<Vec<CodingAgentDefinition>, CatalogUnavailable> {
    load_catalog_report_for_settings_with_config_dir(settings, config_dir).into_result()
}
