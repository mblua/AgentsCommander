//! #769 Phase 1 - externalized, user-editable coding-agent catalog.
//!
//! The catalog is the "built-in coding agents you can add" list (display name,
//! command, color, instructions filename, ...). Before #769 it was hardcoded in
//! the frontend (`src/shared/agent-presets.ts`). This module makes it a
//! backend-owned, on-disk, user-editable JSON manifest, seeded once from an
//! embedded default and user-owned thereafter.
//!
//! #1318 - the catalog now lives in the project tree at
//! `<project>/.ac/coding-agents/agents.json` (same relative layout as before,
//! including the `_seed/` masters tree), seeded per registered project at boot
//! and on every registration route, with a deterministic migration that copies a
//! legacy `<config_dir>/coding-agents/` catalog byte-for-byte into each
//! registered project. All module paths take the project's `.ac` directory as
//! their parameter; the legacy config-dir location is only ever a read/seed
//! SOURCE, never a target.
//!
//! Phase 1 scope (see `_plans/769-...` §14.2): the manifest scalars + one read
//! command, only. NO per-agent config folders, NO #598 seed tier, NO provenance
//! state file. `settings.agents`, `AgentConfig`, and the spawn/profile/resolver
//! path are untouched.
//!
//! Seed model is **whole-file seed-once** (§14.1): write the embedded default iff
//! `agents.json` is absent, then never touch it. A user who hand-removes a
//! built-in has a *present* file, so the removal sticks (no re-seed path). A
//! present-but-corrupt file is **never overwritten**; since #1967 P4 the read
//! path reports it as `baseInvalid` instead of serving the embedded default in
//! memory.
//!
//! #1912 - code-level support switch for the built-in coding agents:
//! `BUILTIN_AGENT_SUPPORT` below is the ONLY place a built-in is turned on or
//! off. A `false` row is enforced by the persisted READ GATE in the report
//! resolver (the key disappears from every persisted read path: the project and
//! legacy manifests; the embedded table is seed material only) and by the SEED GATE on
//! the embedded manifest bytes `ensure_seeded` writes and on the config-folder
//! masters. Already-seeded user-owned files are NEVER rewritten or trimmed by a
//! `false` row; the read gate covers them.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::de::{self, Visitor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::agent_command::is_safe_instructions_filename;
use crate::config::naming_migration;
use crate::config::seed_manifest::{
    acquire_project_gate_soft, has_catalog_publication, ManifestActivationToken,
    ManifestPathIdentity, ProjectSeedManifestGuard, PublishedManifestRow, SoftProjectGate,
    SEED_MANIFEST_FILENAME,
};
use crate::config::settings::{
    validate_agent_command_text, validate_config_seed_dest, validate_env_rows,
    validate_identity_snapshot, AppSettings, CodingAgentEnv, CodingAgentEnvSource,
    ConfigSeedConfig, IdentitySnapshotEntry, IdentityValidationError, ProfileCellConfig,
};

/// Subdirectory of the config dir holding the catalog artifacts.
const CATALOG_DIR_NAME: &str = crate::config::instance_artifacts::CODING_AGENTS_CATALOG_DIR_NAME;
/// The catalog manifest filename.
const CATALOG_MANIFEST_FILENAME: &str =
    crate::config::instance_artifacts::CODING_AGENTS_BASE_FILENAME;
/// Current manifest schema version.
const CATALOG_SCHEMA_VERSION: u32 = 1;

/// The embedded default catalog, authored byte-equal to the post-#766/#768
/// frontend presets. This is the single source of truth AC ships; it is written
/// to disk once (seed) and, since #1967 P4, is NEVER a runtime command donor:
/// every read resolves the persisted file through the report resolver below and
/// cannot fall back to these bytes.
const EMBEDDED_DEFAULT_CATALOG_JSON: &str =
    include_str!("../../resources/coding-agents/agents.default.json");

/// #1912 - the ONLY place a built-in coding agent is turned on or off. One row
/// per key in `agents.default.json`, same order (a test pins both). `false` =
/// de-supported: dropped by the persisted read gate on EVERY read path (the
/// project and legacy persisted manifests; the embedded table is seed material
/// only), omitted from the embedded bytes `ensure_seeded` writes, and its
/// config-folder master is neither seeded nor re-seedable. Already-seeded files
/// are never rewritten or trimmed; the read gate covers them. A key absent from
/// this table (a user-authored entry) is always kept.
pub(crate) const BUILTIN_AGENT_SUPPORT: &[(&str, bool)] = &[
    ("claude", true),
    ("codex", true),
    ("hermes", true),
    ("cursor", true),
    ("pi", true),
    ("opencode", true),
    ("antigravity", true),
    ("grok", true),
    ("muse", false),
];

/// #2736 - how thoroughly AgentsCommander has tested each SUPPORTED built-in.
/// CODE ONLY: never read from, and never patchable through, any catalog file.
/// One row per ENABLED row of `BUILTIN_AGENT_SUPPORT`, in the same order (a test
/// pins both). A key absent here (every user-authored key) has NO level.
pub(crate) const BUILTIN_TESTED_LEVEL: &[(&str, TestedLevel)] = &[
    ("claude", TestedLevel::High),
    ("codex", TestedLevel::High),
    ("hermes", TestedLevel::Low),
    ("cursor", TestedLevel::Low),
    ("pi", TestedLevel::High),
    ("opencode", TestedLevel::Low),
    ("antigravity", TestedLevel::Medium),
    ("grok", TestedLevel::Low),
];

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

fn install_command_for_os<'a>(commands: &'a InstallCommands, os: &str) -> &'a str {
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
fn command_is_present_with(command: &str, resolve: impl FnOnce(&str) -> Option<PathBuf>) -> bool {
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

fn welcome_status_for_with(
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

/// Unique-suffix counter for the seed temp file (mirrors the pattern in
/// `seeded_context_templates::unique_state_temp_path`).
static SEED_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn default_true() -> bool {
    true
}

fn default_catalog_schema_version() -> u32 {
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
struct CodingAgentDefinitionWire {
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
fn catalog_dir(ac_dir: &Path) -> PathBuf {
    ac_dir.join(CATALOG_DIR_NAME)
}

fn manifest_path(ac_dir: &Path) -> PathBuf {
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
fn validate_definition(def: &CodingAgentDefinition) -> Result<(), String> {
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
fn validate_install_commands(value: &InstallCommands, context: &str) -> Result<(), String> {
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
fn validate_idle_burst_config(config: &IdleBurstConfig, context: &str) -> Result<(), String> {
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
fn is_supported_builtin(key: &str, table: &[(&str, bool)]) -> bool {
    !table.iter().any(|(k, on)| *k == key && !*on)
}

/// The table in force: the shipped const, or the test override on this thread.
fn active_builtin_agent_support() -> &'static [(&'static str, bool)] {
    #[cfg(test)]
    if let Some(table) = BUILTIN_AGENT_SUPPORT_OVERRIDE.with(|cell| cell.get()) {
        return table;
    }
    BUILTIN_AGENT_SUPPORT
}

#[cfg(test)]
thread_local! {
    static BUILTIN_AGENT_SUPPORT_OVERRIDE:
        std::cell::Cell<Option<&'static [(&'static str, bool)]>> =
        const { std::cell::Cell::new(None) };
}

/// #1912 test-only: run `f` with `table` in force instead of
/// `BUILTIN_AGENT_SUPPORT` on the CURRENT THREAD; the previous value is
/// restored when `f` returns or panics. Sync closures only: never use it from
/// a multi-thread `#[tokio::test]`.
#[cfg(test)]
pub(crate) fn with_builtin_agent_support_for_test<R>(
    table: &'static [(&'static str, bool)],
    f: impl FnOnce() -> R,
) -> R {
    struct Restore(Option<&'static [(&'static str, bool)]>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BUILTIN_AGENT_SUPPORT_OVERRIDE.with(|cell| cell.set(self.0));
        }
    }
    let _restore = Restore(BUILTIN_AGENT_SUPPORT_OVERRIDE.with(|cell| cell.replace(Some(table))));
    f()
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
fn load_catalog_for_settings_with_config_dir(
    settings: &AppSettings,
    config_dir: Option<PathBuf>,
) -> Result<Vec<CodingAgentDefinition>, CatalogUnavailable> {
    load_catalog_report_for_settings_with_config_dir(settings, config_dir).into_result()
}

// ---------------------------------------------------------------------------
// #1963 P1 - persisted-only catalog report (read-only diagnostic view).
//
// `get_coding_agent_catalog_report` serves this report so the frontend can show
// persisted catalog availability and warnings BEFORE the managed-catalog
// migration is enabled. The resolver NEVER seeds, refreshes, creates
// directories, writes files, takes locks or backfills from the embedded default:
// there is no embedded command donor on this path. It returns the persisted
// definitions after the same per-entry validation, duplicate-key and built-in
// support filters the array endpoint applies, plus visible diagnostics.
//
// P5 (#1968) extends this same resolver to compose the local layer; the wire
// shape below stays fixed. Since P4 (#1967) the array endpoints and the updater
// resolve through the SAME resolver via `CatalogReport::into_result`, so they
// can never disagree with the report about availability.
// ---------------------------------------------------------------------------

/// Diagnostic codes of the persisted-catalog report. The `code` field is a
/// plain string so P5 can add its own codes without a wire change
/// (`localInvalid`, `refreshFailed`, `migrationConflict`, `managedBaseEdited`,
/// `publicationUntracked`).
const REPORT_CODE_BASE_UNAVAILABLE: &str = "baseUnavailable";
const REPORT_CODE_BASE_INVALID: &str = "baseInvalid";
const REPORT_CODE_INVALID_DEFINITION: &str = "invalidDefinition";
const REPORT_CODE_DUPLICATE_KEY: &str = "duplicateKey";
const REPORT_CODE_MIGRATION_PENDING: &str = "migrationPending";

/// The authored definition fields the current schema recognizes. A legacy row
/// carrying anything else stays READABLE (serde ignores unknown fields) but its
/// unknown data emits migrationPending: P5 must not take ownership over data it
/// cannot safely migrate.
const KNOWN_DEFINITION_FIELDS: &[&str] = &[
    "key",
    "label",
    "description",
    "color",
    "command",
    "baseCommand",
    "instructionsFilename",
    "envs",
    "isolatedHome",
    "configSeed",
    "removable",
    "updateCommands",
    "autoUpdate",
    "idleBurst",
    "installCommands",
];
const KNOWN_CONFIG_SEED_FIELDS: &[&str] = &["enabled", "dest"];
const KNOWN_INSTALL_COMMANDS_FIELDS: &[&str] = &["default", "windows", "macos", "linux"];
const KNOWN_ENV_FIELDS: &[&str] = &["key", "value", "source", "enabled"];
const KNOWN_ROOT_FIELDS: &[&str] = &[
    "schemaVersion",
    "agents",
    "managed",
    "profileBindingVersion",
    "profilesByBaseHash",
    "profileLabelsByBaseHash",
    "profileSlots",
    "legacyProfileOrphans",
    "codingAgentProfiles",
    "profileLabels",
];

/// One visible report warning or unavailability record: a stable code plus the
/// affected path and an actionable reason. Reasons never echo command text or
/// environment values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDiagnostic {
    pub code: String,
    pub path: String,
    pub reason: String,
}

/// The persisted-only catalog report wire shape (additive IPC; P5 extends the
/// resolver behind it, not the shape). `primaryProjectRoot` is the exact
/// trimmed root selected by `primary_project_root` (`None` only in no-project
/// mode); `sourcePath` is the selected agents.json path even when absent
/// (`None` only when the config dir cannot be resolved). `unavailable` present
/// always comes with an empty catalog; a valid empty catalog is a success.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogReport {
    pub primary_project_root: Option<String>,
    pub source_path: Option<String>,
    pub catalog: Vec<CodingAgentDefinition>,
    pub warnings: Vec<CatalogDiagnostic>,
    pub unavailable: Option<CatalogDiagnostic>,
}

impl CatalogReport {
    /// #1967 P4 - the array-shaped view of this report, shared by every array
    /// consumer (CLI, Tauri command, WebSocket, updater). Every warning is
    /// logged (the report IPC carries the same records structurally), a readable
    /// catalog - including a valid empty one - becomes `Ok`, and `unavailable`
    /// becomes the typed error instead of an empty success.
    pub fn into_result(self) -> Result<Vec<CodingAgentDefinition>, CatalogUnavailable> {
        for warning in &self.warnings {
            log::warn!(
                "[coding-agents] {} at {}: {}",
                warning.code,
                warning.path,
                warning.reason
            );
        }
        match self.unavailable {
            Some(unavailable) => Err(unavailable.into()),
            None => Ok(self.catalog),
        }
    }
}

/// #1967 P4 - the typed unavailability of an array-shaped catalog read: the
/// report's diagnostic (stable code, affected path, sanitized reason) as an
/// error. `Display` renders `catalog unavailable (code) at path: reason`; the
/// CLI and IPC boundaries stringify it, so code/path/reason survive intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogUnavailable {
    pub code: String,
    pub path: String,
    pub reason: String,
}

impl std::fmt::Display for CatalogUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "catalog unavailable ({}): {}", self.code, self.reason)
        } else {
            write!(
                f,
                "catalog unavailable ({}) at {}: {}",
                self.code, self.path, self.reason
            )
        }
    }
}

impl std::error::Error for CatalogUnavailable {}

impl From<CatalogDiagnostic> for CatalogUnavailable {
    fn from(diagnostic: CatalogDiagnostic) -> Self {
        Self {
            code: diagnostic.code,
            path: diagnostic.path,
            reason: diagnostic.reason,
        }
    }
}

fn catalog_diagnostic(code: &str, path: &Path, reason: impl Into<String>) -> CatalogDiagnostic {
    CatalogDiagnostic {
        code: code.to_string(),
        path: path.display().to_string(),
        reason: reason.into(),
    }
}

/// The report when no catalog location can be resolved at all (the config dir
/// itself is unavailable): `sourcePath` is null and there is no path to
/// report, so the diagnostic carries an empty path.
fn report_without_source() -> CatalogReport {
    CatalogReport {
        primary_project_root: None,
        source_path: None,
        catalog: Vec::new(),
        warnings: Vec::new(),
        unavailable: Some(CatalogDiagnostic {
            code: REPORT_CODE_BASE_UNAVAILABLE.to_string(),
            path: String::new(),
            reason: "the AgentsCommander config directory could not be resolved, so no persisted \
                catalog location is available"
                .to_string(),
        }),
    }
}

/// Read one optional catalog artifact. `Ok(None)` means the path does not
/// exist; a link, a reparse point, a nonregular entry, an unreadable path or an
/// inspect failure is an error, never a silent absence. Read-only by
/// construction.
fn read_optional_regular_file(path: &Path, what: &str) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || is_reparse_point(&meta) {
                return Err(format!(
                    "the {what} at {} is a symbolic link or reparse point; a regular file is \
                        required",
                    path.display()
                ));
            }
            if !meta.file_type().is_file() {
                return Err(format!(
                    "the {what} at {} is not a regular file; a regular file is required",
                    path.display()
                ));
            }
            std::fs::read(path)
                .map(Some)
                .map_err(|e| format!("the {what} at {} could not be read ({e})", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "the {what} at {} could not be inspected ({e})",
            path.display()
        )),
    }
}

/// The instance legacy source is a REGULAR file or nothing at all: a directory,
/// a link or a missing path means "no instance catalog" and the fresh managed
/// default applies, exactly like the pre-#1968 seed rule. A regular file that
/// cannot be read is an error, never a silent reset.
fn read_instance_legacy_source(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink()
                || is_reparse_point(&meta)
                || !meta.file_type().is_file()
            {
                return Ok(None);
            }
            std::fs::read(path).map(Some).map_err(|e| {
                format!(
                    "the instance catalog at {} could not be read ({e})",
                    path.display()
                )
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "the instance catalog at {} could not be inspected ({error})",
            path.display()
        )),
    }
}

fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        // Reparse points are a Windows attribute; this parameter is only read
        // on Windows, so bind it explicitly to stay lint-clean under the
        // Linux/macOS clippy runs (`-D warnings`).
        let _ = metadata;
        false
    }
}

// ---------------------------------------------------------------------------
// #1968 P5 - managed base, local overrides and recoverable migration.
//
// `agents.json` becomes an AC-managed snapshot (root `managed` marker) of the
// supported shipped defaults; `<catalog>/agents.local.json` is the user-owned
// overrides layer. Ordinary reads compose the two and NEVER write. A one-shot
// migration extracts a legacy user-owned catalog into the local layer before
// publishing the managed base, through an immutable byte-exact backup plus a
// journal, so an interrupted migration is always resumable without inventing,
// overwriting or deleting user data.
// ---------------------------------------------------------------------------

/// Diagnostic codes this phase adds.
const REPORT_CODE_PROJECT_INVALID: &str = "projectInvalid";
const REPORT_CODE_LOCAL_INVALID: &str = "localInvalid";
const REPORT_CODE_REFRESH_FAILED: &str = "refreshFailed";
const REPORT_CODE_MIGRATION_CONFLICT: &str = "migrationConflict";
const REPORT_CODE_MANAGED_BASE_EDITED: &str = "managedBaseEdited";
const REPORT_CODE_PUBLICATION_UNTRACKED: &str = "publicationUntracked";

/// The only managed owner/version this build recognizes. Anything else never
/// grants ownership: it is readable, but neither refreshed nor migrated.
const MANAGED_OWNER: &str = "agentscommander";
const MANAGED_VERSION: u32 = 1;
const MANAGED_MARKER_FIELDS: &[&str] = &["owner", "version", "revision", "contentSha256"];

/// The local override file name and the two migration sidecars, aliased from
/// the leaf registry that also renders their git-ignore rows.
const LOCAL_CATALOG_FILENAME: &str =
    crate::config::instance_artifacts::CODING_AGENTS_LOCAL_FILENAME;
const MIGRATION_BACKUP_FILENAME: &str =
    crate::config::instance_artifacts::CODING_AGENTS_MIGRATION_BACKUP_FILENAME;
const MIGRATION_JOURNAL_FILENAME: &str =
    crate::config::instance_artifacts::CODING_AGENTS_MIGRATION_JOURNAL_FILENAME;
const CATALOG_LOCK_FILENAME: &str = crate::config::instance_artifacts::CODING_AGENTS_LOCK_FILENAME;

/// The create-once local stub: valid, empty, and user-owned after creation.
const LOCAL_STUB_BYTES: &[u8] = b"{\"schemaVersion\":1,\"agents\":[]}\n";

const MIGRATION_JOURNAL_VERSION: u32 = 1;

/// The catalog write lock's polling interval and whole acquire deadline,
/// mirroring `local_config_io`'s existing sidecar-lock behavior.
const CATALOG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const CATALOG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

fn local_catalog_path(ac_dir: &Path) -> PathBuf {
    catalog_dir(ac_dir).join(LOCAL_CATALOG_FILENAME)
}

fn project_catalog_path(ac_dir: &Path) -> PathBuf {
    catalog_dir(ac_dir).join(crate::config::instance_artifacts::CODING_AGENTS_PROJECT_TARGET_NAME)
}

#[derive(Clone, Copy)]
enum LayerDescription {
    Project,
    Local,
}

impl LayerDescription {
    fn name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Local => "local",
        }
    }
    fn invalid_code(self) -> &'static str {
        match self {
            Self::Project => REPORT_CODE_PROJECT_INVALID,
            Self::Local => REPORT_CODE_LOCAL_INVALID,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CatalogReadScope {
    Project,
    Instance,
}

fn migration_journal_path(ac_dir: &Path) -> PathBuf {
    catalog_dir(ac_dir).join(MIGRATION_JOURNAL_FILENAME)
}

/// The four publication temporaries share one formula,
/// `.{destination}.{pid}.{counter}.tmp`, published into the destination's own
/// directory and named from its own file name. The formula itself lives in the
/// leaf registry (which also renders the ignore rows); this alias keeps the
/// writer and the policy on the same single source.
fn publication_temp_name_for_destination(
    destination_file_name: &str,
    pid: u32,
    counter: u64,
) -> String {
    crate::config::instance_artifacts::publication_temp_name_for_destination(
        destination_file_name,
        pid,
        counter,
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// The revision/content identity of a supported shipped definition array: the
/// SHA-256 of its DETERMINISTIC COMPACT UTF-8 serialization. The struct's own
/// field order is the serialization order, every authored field is emitted, and
/// `updateCommands` is always explicit (including `[]`), so formatting-free
/// round trips reproduce the hash and a whitespace-only edit does not count as
/// a data edit. `serde_json::Value` map iteration and raw bytes are never
/// consulted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyDefinitionHash<'a> {
    key: &'a String,
    label: &'a String,
    description: &'a String,
    color: &'a String,
    command: &'a String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instructions_filename: &'a Option<String>,
    #[serde(default)]
    envs: &'a Vec<CodingAgentEnv>,
    #[serde(default)]
    isolated_home: &'a bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_seed: &'a Option<ConfigSeedConfig>,
    #[serde(default = "default_true")]
    removable: &'a bool,
    #[serde(default)]
    update_commands: &'a Vec<String>,
    #[serde(default)]
    auto_update: &'a bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    idle_burst: &'a Option<IdleBurstConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_commands: &'a Option<InstallCommands>,
}

impl<'a> From<&'a CodingAgentDefinition> for LegacyDefinitionHash<'a> {
    fn from(value: &'a CodingAgentDefinition) -> Self {
        Self {
            key: &value.key,
            label: &value.label,
            description: &value.description,
            color: &value.color,
            command: &value.command,
            instructions_filename: &value.instructions_filename,
            envs: &value.envs,
            isolated_home: &value.isolated_home,
            config_seed: &value.config_seed,
            removable: &value.removable,
            update_commands: &value.update_commands,
            auto_update: &value.auto_update,
            idle_burst: &value.idle_burst,
            install_commands: &value.install_commands,
        }
    }
}

fn managed_content_sha256(definitions: &[CodingAgentDefinition]) -> String {
    let historical: Vec<_> = definitions.iter().map(LegacyDefinitionHash::from).collect();
    match serde_json::to_vec(&historical) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(error) => {
            log::error!(
                "[coding-agents] failed to serialize catalog definitions for the managed revision \
                    hash ({error})"
            );
            String::new()
        }
    }
}

/// The shipped definitions the support table in force allows, in shipped order.
fn supported_shipped_definitions() -> Vec<CodingAgentDefinition> {
    let table = active_builtin_agent_support();
    embedded_default_catalog()
        .agents
        .into_iter()
        .filter(|definition| is_supported_builtin(&definition.key, table))
        .collect()
}

/// The complete managed base bytes for `definitions`: pretty JSON, one trailing
/// LF, schemaVersion 1, the definitions, and the ownership marker whose
/// `revision` and `contentSha256` are the same content identity. No timestamp
/// is part of the content identity.
fn build_managed_base_bytes(definitions: &[CodingAgentDefinition]) -> Vec<u8> {
    build_managed_base_bytes_with_original(definitions, None)
}

fn original_catalog_command<'a>(
    original: Option<&'a serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    original?
        .get("agents")?
        .as_array()?
        .iter()
        .find(|row| row.get("key").and_then(serde_json::Value::as_str) == Some(key))?
        .get("command")
}

fn build_managed_base_bytes_with_original(
    definitions: &[CodingAgentDefinition],
    original: Option<&serde_json::Value>,
) -> Vec<u8> {
    let revision = managed_content_sha256(definitions);
    #[derive(Serialize)]
    struct DefinitionWire<'a> {
        #[serde(flatten)]
        definition: &'a CodingAgentDefinition,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<&'a serde_json::Value>,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ManagedBaseWire<'a> {
        schema_version: u32,
        agents: Vec<DefinitionWire<'a>>,
        managed: ManagedCatalogMarker,
    }
    let root = ManagedBaseWire {
        schema_version: CATALOG_SCHEMA_VERSION,
        agents: definitions
            .iter()
            .map(|definition| DefinitionWire {
                definition,
                command: original_catalog_command(original, &definition.key),
            })
            .collect(),
        managed: ManagedCatalogMarker {
            owner: MANAGED_OWNER.to_string(),
            version: MANAGED_VERSION,
            revision: revision.clone(),
            content_sha256: revision,
        },
    };
    let mut bytes = match serde_json::to_vec_pretty(&root) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::error!("[coding-agents] failed to serialize the managed catalog base ({error})");
            Vec::new()
        }
    };
    bytes.push(b'\n');
    bytes
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManagedCatalogMarker {
    owner: String,
    version: u32,
    revision: String,
    content_sha256: String,
}

// ---------------------------------------------------------------------------
// Strict JSON: duplicate object members are rejected at every depth.
// ---------------------------------------------------------------------------

struct StrictValue(serde_json::Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value whose objects have no duplicate members")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(StrictValue(
            serde_json::Number::from_f64(value)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
        ))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::String(value)))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Null))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        StrictValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut items = Vec::new();
        while let Some(item) = access.next_element::<StrictValue>()? {
            items.push(item.0);
        }
        Ok(StrictValue(serde_json::Value::Array(items)))
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut members = serde_json::Map::new();
        while let Some(key) = access.next_key::<String>()? {
            if members.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object member '{key}'"
                )));
            }
            let value = access.next_value::<StrictValue>()?;
            members.insert(key, value.0);
        }
        Ok(StrictValue(serde_json::Value::Object(members)))
    }
}

/// Parse JSON while rejecting duplicate object members at every depth. A raw
/// `Value` parse cannot detect those, and a duplicate member makes the document
/// ambiguous, which is exactly the case a destructive migration must refuse.
pub(crate) fn parse_strict_json(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue::deserialize(&mut deserializer).map_err(|error| error.to_string())?;
    deserializer
        .end()
        .map_err(|error| format!("trailing data after the JSON document ({error})"))?;
    Ok(value.0)
}

fn expect_json_object<'a>(
    value: &'a serde_json::Value,
    context: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be a JSON object"))
}

fn expect_json_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
    context: &str,
) -> Result<&'a str, String> {
    match object.get(field) {
        Some(serde_json::Value::String(value)) => Ok(value),
        Some(_) => Err(format!("{context}: '{field}' must be a string")),
        None => Err(format!("{context}: '{field}' is required")),
    }
}

fn reject_unknown_json_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    context: &str,
) -> Result<(), String> {
    if let Some(names) = unknown_field_names(object.keys(), allowed) {
        return Err(format!("{context}: unknown field(s) ({names})"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The strict local layer schema and its composition.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct LocalFieldPatch {
    label: Option<String>,
    description: Option<String>,
    color: Option<String>,
    command: Option<String>,
    /// `None` = absent; `Some(None)` = explicit `null`.
    instructions_filename: Option<Option<String>>,
    envs: Option<Vec<CodingAgentEnv>>,
    isolated_home: Option<bool>,
    /// `None` = absent; `Some(None)` = explicit `null`; `Some(Some(_))` = object.
    config_seed: Option<Option<ConfigSeedPatch>>,
    removable: Option<bool>,
    update_commands: Option<Vec<String>>,
    auto_update: Option<bool>,
    /// `None` = absent; `Some(None)` = explicit `null`; `Some(Some(_))` = object.
    idle_burst: Option<Option<IdleBurstConfig>>,
    /// `None` = absent; `Some(None)` = explicit `null`; `Some(Some(_))` = object.
    install_commands: Option<Option<InstallCommandsPatch>>,
}

/// #2736 - a local `installCommands` patch. For the three per-OS keys,
/// `None` = absent (inherit), `Some(None)` = explicit `null` (clear).
#[derive(Debug, Clone, Default)]
struct InstallCommandsPatch {
    default: Option<String>,
    windows: Option<Option<String>>,
    macos: Option<Option<String>>,
    linux: Option<Option<String>>,
}

#[derive(Debug, Clone, Default)]
struct ConfigSeedPatch {
    enabled: Option<bool>,
    dest: Option<String>,
}

#[derive(Debug, Clone)]
struct LocalRow {
    key: String,
    remove: bool,
    fields: LocalFieldPatch,
}

#[derive(Debug, Clone)]
struct LocalLayer {
    rows: Vec<LocalRow>,
    order: Option<Vec<String>>,
    complete_definitions: bool,
}

const LOCAL_ROOT_FIELDS: &[&str] = &[
    "schemaVersion",
    "agents",
    "order",
    "profileBindingVersion",
    "profilesByBaseHash",
    "profileLabelsByBaseHash",
    "profileSlots",
    "legacyProfileOrphans",
    "codingAgentProfiles",
    "profileLabels",
];
const LOCAL_ROW_FIELDS: &[&str] = &[
    "key",
    "label",
    "description",
    "color",
    "command",
    "baseCommand",
    "instructionsFilename",
    "envs",
    "isolatedHome",
    "configSeed",
    "removable",
    "updateCommands",
    "autoUpdate",
    "idleBurst",
    "installCommands",
];
const LOCAL_ENV_FIELDS: &[&str] = &["key", "value", "source", "enabled"];
const LOCAL_CONFIG_SEED_FIELDS: &[&str] = &["enabled", "dest"];
const LOCAL_IDLE_BURST_FIELDS: &[&str] = &["maxBytes", "maxSecs", "priorSilenceSecs"];
const LOCAL_INSTALL_COMMANDS_FIELDS: &[&str] = &["default", "windows", "macos", "linux"];

fn validate_update_command_string(
    command: &str,
    context: &str,
    index: usize,
) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err(format!("{context}: updateCommands item {index} is blank"));
    }
    if command
        .chars()
        .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
    {
        return Err(format!(
            "{context}: updateCommands item {index} contains a Unicode control character, U+2028 \
                or U+2029"
        ));
    }
    Ok(())
}

fn parse_update_commands_value(
    value: &serde_json::Value,
    context: &str,
) -> Result<Vec<String>, String> {
    let serde_json::Value::Array(items) = value else {
        return Err(format!(
            "{context}: updateCommands must be an array of complete command strings"
        ));
    };
    let mut commands = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let serde_json::Value::String(command) = item else {
            return Err(format!(
                "{context}: updateCommands item {index} must be a string"
            ));
        };
        validate_update_command_string(command, context, index)?;
        commands.push(command.clone());
    }
    Ok(commands)
}

fn parse_local_envs(
    value: &serde_json::Value,
    context: &str,
) -> Result<Vec<CodingAgentEnv>, String> {
    let serde_json::Value::Array(items) = value else {
        return Err(format!("{context}: envs must be an array"));
    };
    let mut envs = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let row_context = format!("{context} envs[{index}]");
        let object = expect_json_object(item, &row_context)?;
        reject_unknown_json_fields(object, LOCAL_ENV_FIELDS, &row_context)?;
        let key = expect_json_string(object, "key", &row_context)?.to_string();
        let value = expect_json_string(object, "value", &row_context)?.to_string();
        let source = match object.get("source") {
            None => CodingAgentEnvSource::User,
            Some(serde_json::Value::String(source)) => match source.as_str() {
                "user" => CodingAgentEnvSource::User,
                "system" | "agentsCommander" => CodingAgentEnvSource::System,
                _ => {
                    return Err(format!(
                        "{row_context}: 'source' is not a recognized env source value"
                    ))
                }
            },
            Some(_) => return Err(format!("{row_context}: 'source' must be a string")),
        };
        let enabled = match object.get("enabled") {
            None => true,
            Some(serde_json::Value::Bool(enabled)) => *enabled,
            Some(_) => return Err(format!("{row_context}: 'enabled' must be a boolean")),
        };
        envs.push(CodingAgentEnv {
            key,
            value,
            source,
            enabled,
        });
    }
    Ok(envs)
}

fn parse_config_seed_patch(
    value: &serde_json::Value,
    context: &str,
) -> Result<ConfigSeedPatch, String> {
    let seed_context = format!("{context} configSeed");
    let object = expect_json_object(value, &seed_context)?;
    reject_unknown_json_fields(object, LOCAL_CONFIG_SEED_FIELDS, &seed_context)?;
    let mut patch = ConfigSeedPatch::default();
    if let Some(enabled) = object.get("enabled") {
        let serde_json::Value::Bool(enabled) = enabled else {
            return Err(format!("{seed_context}: 'enabled' must be a boolean"));
        };
        patch.enabled = Some(*enabled);
    }
    if let Some(dest) = object.get("dest") {
        let serde_json::Value::String(dest) = dest else {
            return Err(format!("{seed_context}: 'dest' must be a string"));
        };
        patch.dest = Some(dest.clone());
    }
    Ok(patch)
}

fn parse_idle_burst_patch(
    value: &serde_json::Value,
    context: &str,
) -> Result<IdleBurstConfig, String> {
    let burst_context = format!("{context} idleBurst");
    let object = expect_json_object(value, &burst_context)?;
    reject_unknown_json_fields(object, LOCAL_IDLE_BURST_FIELDS, &burst_context)?;
    let mut config = IdleBurstConfig::default();
    if let Some(max_bytes) = object.get("maxBytes") {
        let Some(max_bytes) = max_bytes.as_u64() else {
            return Err(format!(
                "{burst_context}: 'maxBytes' must be a non-negative integer"
            ));
        };
        config.max_bytes = Some(max_bytes);
    }
    if let Some(max_secs) = object.get("maxSecs") {
        config.max_secs = Some(parse_idle_burst_secs(max_secs, "maxSecs", &burst_context)?);
    }
    if let Some(prior_silence_secs) = object.get("priorSilenceSecs") {
        config.prior_silence_secs = Some(parse_idle_burst_secs(
            prior_silence_secs,
            "priorSilenceSecs",
            &burst_context,
        )?);
    }
    Ok(config)
}

/// #2736 - the strict local `installCommands` patch: an object whose only
/// members are the four platform keys; `default` must be a string, each
/// per-OS key a string or `null`. Values are checked after composition.
fn parse_install_commands_patch(
    value: &serde_json::Value,
    context: &str,
) -> Result<InstallCommandsPatch, String> {
    let install_context = format!("{context} installCommands");
    let object = expect_json_object(value, &install_context)?;
    reject_unknown_json_fields(object, LOCAL_INSTALL_COMMANDS_FIELDS, &install_context)?;
    let mut patch = InstallCommandsPatch::default();
    if let Some(default) = object.get("default") {
        let serde_json::Value::String(default) = default else {
            return Err(format!("{install_context}: 'default' must be a string"));
        };
        patch.default = Some(default.clone());
    }
    for (name, slot) in [
        ("windows", &mut patch.windows),
        ("macos", &mut patch.macos),
        ("linux", &mut patch.linux),
    ] {
        if let Some(value) = object.get(name) {
            *slot = Some(match value {
                serde_json::Value::Null => None,
                serde_json::Value::String(value) => Some(value.clone()),
                _ => {
                    return Err(format!(
                        "{install_context}: '{name}' must be a string or null"
                    ))
                }
            });
        }
    }
    Ok(patch)
}

/// One `idleBurst` seconds subfield, strictly: a JSON number, finite and >= 0.
fn parse_idle_burst_secs(
    value: &serde_json::Value,
    field: &str,
    context: &str,
) -> Result<f64, String> {
    let parsed = value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0);
    parsed.ok_or_else(|| format!("{context}: '{field}' must be a finite number >= 0"))
}

fn parse_local_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    context: &str,
) -> Result<LocalFieldPatch, String> {
    reject_unknown_json_fields(object, LOCAL_ROW_FIELDS, context)?;
    let mut fields = LocalFieldPatch::default();
    if let Some(value) = object.get("label") {
        let serde_json::Value::String(value) = value else {
            return Err(format!("{context}: 'label' must be a string"));
        };
        fields.label = Some(value.clone());
    }
    if let Some(value) = object.get("description") {
        let serde_json::Value::String(value) = value else {
            return Err(format!("{context}: 'description' must be a string"));
        };
        fields.description = Some(value.clone());
    }
    if let Some(value) = object.get("color") {
        let serde_json::Value::String(value) = value else {
            return Err(format!("{context}: 'color' must be a string"));
        };
        fields.color = Some(value.clone());
    }
    for name in ["baseCommand", "command"] {
        if object.get(name).is_some_and(|value| !value.is_string()) {
            return Err(format!("{context}: '{name}' must be a string"));
        }
    }
    fields.command = crate::config::settings::select_configuration_command(
        object
            .get("baseCommand")
            .and_then(serde_json::Value::as_str),
        object.get("command").and_then(serde_json::Value::as_str),
    )
    .map(str::to_owned);
    if let Some(value) = object.get("instructionsFilename") {
        fields.instructions_filename = Some(match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(value) => Some(value.clone()),
            _ => {
                return Err(format!(
                    "{context}: 'instructionsFilename' must be a string or null"
                ))
            }
        });
    }
    if let Some(value) = object.get("envs") {
        fields.envs = Some(parse_local_envs(value, context)?);
    }
    if let Some(value) = object.get("isolatedHome") {
        let serde_json::Value::Bool(value) = value else {
            return Err(format!("{context}: 'isolatedHome' must be a boolean"));
        };
        fields.isolated_home = Some(*value);
    }
    if let Some(value) = object.get("configSeed") {
        fields.config_seed = Some(match value {
            serde_json::Value::Null => None,
            serde_json::Value::Object(_) => Some(parse_config_seed_patch(value, context)?),
            _ => return Err(format!("{context}: 'configSeed' must be an object or null")),
        });
    }
    if let Some(value) = object.get("removable") {
        let serde_json::Value::Bool(value) = value else {
            return Err(format!("{context}: 'removable' must be a boolean"));
        };
        fields.removable = Some(*value);
    }
    if let Some(value) = object.get("updateCommands") {
        fields.update_commands = Some(parse_update_commands_value(value, context)?);
    }
    if let Some(value) = object.get("autoUpdate") {
        let serde_json::Value::Bool(value) = value else {
            return Err(format!("{context}: 'autoUpdate' must be a boolean"));
        };
        fields.auto_update = Some(*value);
    }
    if let Some(value) = object.get("idleBurst") {
        fields.idle_burst = Some(match value {
            serde_json::Value::Null => None,
            serde_json::Value::Object(_) => Some(parse_idle_burst_patch(value, context)?),
            _ => return Err(format!("{context}: 'idleBurst' must be an object or null")),
        });
    }
    if let Some(value) = object.get("installCommands") {
        fields.install_commands = Some(match value {
            serde_json::Value::Null => None,
            serde_json::Value::Object(_) => Some(parse_install_commands_patch(value, context)?),
            _ => {
                return Err(format!(
                    "{context}: 'installCommands' must be an object or null"
                ))
            }
        });
    }
    Ok(fields)
}

/// Parse the local layer STRICTLY: unknown fields, duplicate JSON members,
/// duplicate local identities, duplicate order keys and unsupported schemas are
/// all whole-layer errors, so a partially applied layer is impossible.
fn parse_local_layer(bytes: &[u8], description: LayerDescription) -> Result<LocalLayer, String> {
    let layer = description.name();
    let value = parse_strict_json(bytes)
        .map_err(|reason| format!("the {layer} catalog is not valid JSON ({reason})"))?;
    let root = expect_json_object(&value, &format!("the {layer} catalog root"))?;
    if root
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        == Some(2)
    {
        let catalog = parse_catalog_schema2(bytes).map_err(|e| e.to_string())?;
        let rows = catalog
            .agents
            .iter()
            .map(|agent| {
                let raw = serde_json::to_value(agent).map_err(|e| e.to_string())?;
                let fields =
                    parse_local_fields(raw.as_object().expect("definition"), "schema2 definition")?;
                Ok(LocalRow {
                    key: agent.key.clone(),
                    remove: false,
                    fields,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        return Ok(LocalLayer {
            rows,
            order: None,
            complete_definitions: true,
        });
    }
    crate::config::settings::physical_profile_sets(&value, false)?;
    reject_unknown_json_fields(
        root,
        LOCAL_ROOT_FIELDS,
        &format!("the {layer} catalog root"),
    )?;
    match root.get("schemaVersion") {
        Some(serde_json::Value::Number(version))
            if version.as_u64() == Some(CATALOG_SCHEMA_VERSION as u64) => {}
        Some(version) => {
            return Err(format!(
                "unsupported schemaVersion {version}; only schemaVersion 1 is recognized"
            ))
        }
        None => {
            return Err(format!(
                "the {layer} catalog root must declare schemaVersion 1"
            ))
        }
    }
    let agents = match root.get("agents") {
        Some(serde_json::Value::Array(agents)) => agents,
        Some(_) => {
            return Err(format!(
                "the {layer} catalog 'agents' value must be a JSON array"
            ))
        }
        None => {
            return Err(format!(
                "the {layer} catalog root must declare an 'agents' array"
            ))
        }
    };
    let order = match root.get("order") {
        None => None,
        Some(serde_json::Value::Array(items)) => {
            let mut seen = HashSet::new();
            let mut keys = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let serde_json::Value::String(key) = item else {
                    return Err(format!("{layer} order item {index} must be a string"));
                };
                if validate_catalog_key(key).is_err() {
                    return Err(format!(
                        "{layer} order item {index} is not a valid catalog key"
                    ));
                }
                if !seen.insert(key.clone()) {
                    return Err(format!("duplicate {layer} order key '{key}'"));
                }
                keys.push(key.clone());
            }
            Some(keys)
        }
        Some(_) => {
            return Err(format!(
                "the {layer} catalog 'order' value must be a JSON array"
            ))
        }
    };

    let mut seen_keys = HashSet::new();
    let mut rows = Vec::with_capacity(agents.len());
    for (index, raw) in agents.iter().enumerate() {
        let context = format!("{layer} agents[{index}]");
        let object = expect_json_object(raw, &context)?;
        let key = expect_json_string(object, "key", &context)?.to_string();
        validate_catalog_key(&key).map_err(|reason| format!("{context}: {reason}"))?;
        if !seen_keys.insert(key.clone()) {
            return Err(format!("duplicate {layer} coding-agent key '{key}'"));
        }
        let remove = match object.get("remove") {
            None => false,
            Some(serde_json::Value::Bool(true)) => true,
            Some(serde_json::Value::Bool(false)) => {
                return Err(format!(
                    "{context}: remove:false is rejected; omit remove for ordinary patches"
                ))
            }
            Some(_) => return Err(format!("{context}: 'remove' must be true")),
        };
        if remove {
            reject_unknown_json_fields(object, &["key", "remove"], &context)?;
            rows.push(LocalRow {
                key,
                remove: true,
                fields: LocalFieldPatch::default(),
            });
        } else {
            let fields = parse_local_fields(object, &context)?;
            rows.push(LocalRow {
                key,
                remove: false,
                fields,
            });
        }
    }
    Ok(LocalLayer {
        rows,
        order,
        complete_definitions: false,
    })
}

fn merge_config_seed(
    current: Option<ConfigSeedConfig>,
    patch: &ConfigSeedPatch,
) -> ConfigSeedConfig {
    let mut merged = current.unwrap_or(ConfigSeedConfig {
        enabled: true,
        dest: String::new(),
    });
    if let Some(enabled) = patch.enabled {
        merged.enabled = enabled;
    }
    if let Some(dest) = &patch.dest {
        merged.dest = dest.clone();
    }
    merged
}

/// #2124 - merge the local `idleBurst` patch by presence: an absent subfield
/// inherits the base value; an object after a missing or `null` base starts from
/// all-absent, i.e. the launch-time defaults.
fn merge_idle_burst(current: Option<IdleBurstConfig>, patch: &IdleBurstConfig) -> IdleBurstConfig {
    let mut merged = current.unwrap_or_default();
    if patch.max_bytes.is_some() {
        merged.max_bytes = patch.max_bytes;
    }
    if patch.max_secs.is_some() {
        merged.max_secs = patch.max_secs;
    }
    if patch.prior_silence_secs.is_some() {
        merged.prior_silence_secs = patch.prior_silence_secs;
    }
    merged
}

/// #2736 - merge the local `installCommands` patch by presence onto the base
/// object, or onto an empty `default` when there is none. A blank resulting
/// `default` fails the composed re-validation.
fn merge_install_commands(
    base: Option<InstallCommands>,
    patch: &InstallCommandsPatch,
) -> InstallCommands {
    let mut merged = base.unwrap_or(InstallCommands {
        default: String::new(),
        windows: None,
        macos: None,
        linux: None,
    });
    if let Some(default) = &patch.default {
        merged.default = default.clone();
    }
    if let Some(value) = &patch.windows {
        merged.windows = value.clone();
    }
    if let Some(value) = &patch.macos {
        merged.macos = value.clone();
    }
    if let Some(value) = &patch.linux {
        merged.linux = value.clone();
    }
    merged
}

fn apply_local_fields(
    mut definition: CodingAgentDefinition,
    fields: &LocalFieldPatch,
) -> CodingAgentDefinition {
    if let Some(value) = &fields.label {
        definition.label = value.clone();
    }
    if let Some(value) = &fields.description {
        definition.description = value.clone();
    }
    if let Some(value) = &fields.color {
        definition.color = value.clone();
    }
    if let Some(value) = &fields.command {
        definition.command = value.clone();
    }
    if let Some(value) = &fields.instructions_filename {
        definition.instructions_filename = value.clone();
    }
    if let Some(value) = &fields.envs {
        definition.envs = value.clone();
    }
    if let Some(value) = fields.isolated_home {
        definition.isolated_home = value;
    }
    if let Some(patch) = &fields.config_seed {
        definition.config_seed = match patch {
            None => None,
            Some(patch) => Some(merge_config_seed(definition.config_seed.clone(), patch)),
        };
    }
    if let Some(value) = fields.removable {
        definition.removable = value;
    }
    if let Some(value) = &fields.update_commands {
        definition.update_commands = value.clone();
    }
    if let Some(value) = fields.auto_update {
        definition.auto_update = value;
    }
    if let Some(patch) = &fields.idle_burst {
        definition.idle_burst = match patch {
            None => None,
            Some(patch) => Some(merge_idle_burst(definition.idle_burst.clone(), patch)),
        };
    }
    if let Some(patch) = &fields.install_commands {
        definition.install_commands = match patch {
            None => None,
            Some(patch) => Some(merge_install_commands(
                definition.install_commands.clone(),
                patch,
            )),
        };
    }
    definition
}

/// A NEW key requires every authored field explicitly; `instructionsFilename`
/// `configSeed`, `idleBurst` and `installCommands` stay optional (absent or
/// `null`).
fn build_new_definition(
    key: &str,
    fields: &LocalFieldPatch,
    description: LayerDescription,
) -> Result<CodingAgentDefinition, String> {
    let mut missing = Vec::new();
    if fields.label.is_none() {
        missing.push("label");
    }
    if fields.description.is_none() {
        missing.push("description");
    }
    if fields.color.is_none() {
        missing.push("color");
    }
    if fields.command.is_none() {
        missing.push("command");
    }
    if fields.envs.is_none() {
        missing.push("envs");
    }
    if fields.isolated_home.is_none() {
        missing.push("isolatedHome");
    }
    if fields.removable.is_none() {
        missing.push("removable");
    }
    if fields.update_commands.is_none() {
        missing.push("updateCommands");
    }
    if fields.auto_update.is_none() {
        missing.push("autoUpdate");
    }
    if !missing.is_empty() {
        return Err(format!(
            "new {} coding-agent '{key}' is missing required field(s): {}",
            description.name(),
            missing.join(", ")
        ));
    }
    let config_seed = match &fields.config_seed {
        None | Some(None) => None,
        Some(Some(patch)) => Some(merge_config_seed(None, patch)),
    };
    Ok(CodingAgentDefinition {
        key: key.to_string(),
        label: fields.label.clone().unwrap_or_default(),
        description: fields.description.clone().unwrap_or_default(),
        color: fields.color.clone().unwrap_or_default(),
        command: fields.command.clone().unwrap_or_default(),
        instructions_filename: fields.instructions_filename.clone().unwrap_or(None),
        envs: fields.envs.clone().unwrap_or_default(),
        isolated_home: fields.isolated_home.unwrap_or(false),
        config_seed,
        removable: fields.removable.unwrap_or(true),
        update_commands: fields.update_commands.clone().unwrap_or_default(),
        auto_update: fields.auto_update.unwrap_or(false),
        idle_burst: match &fields.idle_burst {
            None | Some(None) => None,
            Some(Some(config)) => Some(config.clone()),
        },
        install_commands: match &fields.install_commands {
            None | Some(None) => None,
            Some(Some(patch)) => Some(merge_install_commands(None, patch)),
        },
    })
}

/// Compose the local layer onto the base definitions. Rows merge by stable key
/// (omission inherits, present values replace); local additions append in local
/// order; the optional root `order` moves listed survivors first while every
/// unlisted survivor keeps base-then-local-add order. The whole candidate is
/// validated before anything is returned, and one bad definition discards the
/// ENTIRE local layer. The built-in support gate is applied by the caller so
/// the unedited-content check can still see the full base.
fn compose_local_layer(
    base: &[CodingAgentDefinition],
    local: &LocalLayer,
    description: LayerDescription,
) -> Result<Vec<CodingAgentDefinition>, String> {
    let layer = description.name();
    let composition = format!("{layer} composition");
    let mut working: Vec<CodingAgentDefinition> = base.to_vec();
    let mut index: HashMap<String, usize> = HashMap::with_capacity(working.len());
    for (position, definition) in working.iter().enumerate() {
        index.insert(definition.key.clone(), position);
    }
    let mut removed = vec![false; working.len()];
    let mut local_added: Vec<CodingAgentDefinition> = Vec::new();

    for row in &local.rows {
        match index.get(&row.key).copied() {
            Some(position) => {
                if row.remove {
                    if !working[position].removable {
                        return Err(format!(
                            "{layer} remove row targets nonremovable coding agent '{}'",
                            row.key
                        ));
                    }
                    removed[position] = true;
                } else {
                    working[position] = if local.complete_definitions {
                        build_new_definition(&row.key, &row.fields, description)?
                    } else {
                        apply_local_fields(working[position].clone(), &row.fields)
                    };
                }
            }
            None => {
                // An unknown tombstone is valid and retained for a future
                // shipped key; only a new ordinary row needs a complete def.
                if row.remove {
                    continue;
                }
                local_added.push(build_new_definition(&row.key, &row.fields, description)?);
            }
        }
    }

    let mut effective: Vec<CodingAgentDefinition> =
        Vec::with_capacity(working.len() + local_added.len());
    for (position, definition) in working.into_iter().enumerate() {
        if !removed[position] {
            effective.push(definition);
        }
    }
    effective.extend(local_added);

    for definition in &effective {
        if validate_definition(definition).is_err() {
            return Err(format!(
                "coding agent '{}' failed validation after composition: {}",
                definition.key,
                definition_problem_reason(definition)
            ));
        }
        for (index, command) in definition.update_commands.iter().enumerate() {
            validate_update_command_string(command, &composition, index).map_err(|reason| {
                format!(
                    "coding agent '{}' failed validation after composition: {reason}",
                    definition.key
                )
            })?;
        }
        if let Some(install) = definition.install_commands.as_ref() {
            validate_install_commands(install, &composition).map_err(|reason| {
                format!(
                    "coding agent '{}' failed validation after composition: {reason}",
                    definition.key
                )
            })?;
        }
    }

    if let Some(order) = &local.order {
        let position: HashMap<&str, usize> = effective
            .iter()
            .enumerate()
            .map(|(index, definition)| (definition.key.as_str(), index))
            .collect();
        let mut taken = vec![false; effective.len()];
        let mut ordered = Vec::with_capacity(effective.len());
        for key in order {
            if let Some(&position) = position.get(key.as_str()) {
                if !taken[position] {
                    taken[position] = true;
                    ordered.push(effective[position].clone());
                }
            }
        }
        for (index, definition) in effective.into_iter().enumerate() {
            if !taken[index] {
                ordered.push(definition);
            }
        }
        effective = ordered;
    }

    Ok(effective)
}

/// Apply the shipped support gate to a resolved catalog, warning once per
/// suppressed built-in key.
fn apply_support_gate(
    definitions: Vec<CodingAgentDefinition>,
    path: &Path,
    warnings: &mut Vec<CatalogDiagnostic>,
) -> Vec<CodingAgentDefinition> {
    let table = active_builtin_agent_support();
    let mut suppressed: Vec<String> = Vec::new();
    let mut kept = Vec::with_capacity(definitions.len());
    for definition in definitions {
        if is_supported_builtin(&definition.key, table) {
            kept.push(definition);
        } else if !suppressed.contains(&definition.key) {
            suppressed.push(definition.key.clone());
            warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                path,
                format!(
                    "coding-agent '{}' is not supported by this build and was omitted",
                    definition.key
                ),
            ));
        }
    }
    kept
}

// ---------------------------------------------------------------------------
// Base analysis: ownership, content identity and readability.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogBaseKind {
    /// The managed marker carries this build's owner and version.
    Managed,
    /// A `managed` marker exists but is not ours (or is malformed): readable,
    /// but neither refreshed nor migrated.
    ForeignManaged,
    /// No marker at all: legacy user-owned territory.
    Legacy,
}

struct BaseAnalysis {
    kind: CatalogBaseKind,
    marker: Option<ManagedCatalogMarker>,
    entries: Vec<CodingAgentDefinition>,
    edited: bool,
    refresh_blocked: bool,
    warnings: Vec<CatalogDiagnostic>,
}

fn analyze_base_bytes(base_path: &Path, bytes: &[u8]) -> Result<BaseAnalysis, CatalogDiagnostic> {
    let value = parse_strict_json(bytes).map_err(|reason| {
        catalog_diagnostic(
            REPORT_CODE_BASE_INVALID,
            base_path,
            format!("the persisted catalog is not valid JSON ({reason})"),
        )
    })?;
    let Some(root) = value.as_object() else {
        return Err(catalog_diagnostic(
            REPORT_CODE_BASE_INVALID,
            base_path,
            "the catalog root must be a JSON object",
        ));
    };
    if root
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        == Some(2)
    {
        let catalog = parse_catalog_schema2(bytes)
            .map_err(|e| catalog_diagnostic(REPORT_CODE_BASE_INVALID, base_path, e.to_string()))?;
        return Ok(BaseAnalysis {
            kind: CatalogBaseKind::ForeignManaged,
            marker: None,
            entries: catalog.agents,
            edited: false,
            refresh_blocked: true,
            warnings: Vec::new(),
        });
    }
    if let Some(version) = root.get("schemaVersion") {
        if version.as_u64() != Some(CATALOG_SCHEMA_VERSION as u64) {
            return Err(catalog_diagnostic(
                REPORT_CODE_BASE_INVALID,
                base_path,
                format!(
                    "unsupported explicit schemaVersion {version}; only schemaVersion 1 is \
                        recognized"
                ),
            ));
        }
    }
    let empty_rows = Vec::new();
    let rows = match root.get("agents") {
        None => &empty_rows,
        Some(serde_json::Value::Array(rows)) => rows,
        Some(_) => {
            return Err(catalog_diagnostic(
                REPORT_CODE_BASE_INVALID,
                base_path,
                "the catalog 'agents' value must be a JSON array",
            ))
        }
    };

    let mut warnings = Vec::new();
    let mut refresh_blocked =
        root.contains_key("profileBindingVersion") || root.contains_key("legacyProfileOrphans");
    if let Some(names) = unknown_field_names(root.keys(), KNOWN_ROOT_FIELDS) {
        warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_PENDING,
            base_path,
            format!(
                "root field(s) ({names}) are not recognized by the current catalog schema and \
                    require managed-catalog migration"
            ),
        ));
        refresh_blocked = true;
    }

    let (kind, marker) = match root.get("managed") {
        None => (CatalogBaseKind::Legacy, None),
        Some(serde_json::Value::Object(object)) => {
            if unknown_field_names(object.keys(), MANAGED_MARKER_FIELDS).is_some() {
                refresh_blocked = true;
            }
            let owner = object.get("owner").and_then(serde_json::Value::as_str);
            let version = object.get("version").and_then(serde_json::Value::as_u64);
            if owner == Some(MANAGED_OWNER) && version == Some(MANAGED_VERSION as u64) {
                match (
                    object.get("revision").and_then(serde_json::Value::as_str),
                    object
                        .get("contentSha256")
                        .and_then(serde_json::Value::as_str),
                ) {
                    (Some(revision), Some(content_sha256)) => (
                        CatalogBaseKind::Managed,
                        Some(ManagedCatalogMarker {
                            owner: MANAGED_OWNER.to_string(),
                            version: MANAGED_VERSION,
                            revision: revision.to_string(),
                            content_sha256: content_sha256.to_string(),
                        }),
                    ),
                    _ => (CatalogBaseKind::ForeignManaged, None),
                }
            } else {
                (CatalogBaseKind::ForeignManaged, None)
            }
        }
        Some(_) => (CatalogBaseKind::ForeignManaged, None),
    };

    let mut entries: Vec<CodingAgentDefinition> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    for (index, raw) in rows.iter().enumerate() {
        let Some(raw_object) = raw.as_object() else {
            warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                base_path,
                format!("entry {index} is not a JSON object and was omitted"),
            ));
            continue;
        };
        if let Some(reason) =
            raw_update_commands_problem(raw).or_else(|| raw_install_commands_problem(raw))
        {
            warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                base_path,
                format!("entry {index} was omitted: {reason}"),
            ));
            continue;
        }
        let definition: CodingAgentDefinition = match serde_json::from_value(raw.clone()) {
            Ok(definition) => definition,
            Err(_) => {
                warnings.push(catalog_diagnostic(
                    REPORT_CODE_INVALID_DEFINITION,
                    base_path,
                    format!(
                        "entry {index} does not match the coding-agent definition schema and was \
                            omitted"
                    ),
                ));
                continue;
            }
        };
        if validate_definition(&definition).is_err() {
            let label = if validate_catalog_key(&definition.key).is_ok() {
                format!("coding-agent '{}'", definition.key)
            } else {
                format!("entry {index}")
            };
            warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                base_path,
                format!(
                    "{label} was omitted: {}",
                    definition_problem_reason(&definition)
                ),
            ));
            continue;
        }
        if !seen_keys.insert(definition.key.clone()) {
            warnings.push(catalog_diagnostic(
                REPORT_CODE_DUPLICATE_KEY,
                base_path,
                format!(
                    "coding-agent '{}' was omitted: its key duplicates an earlier entry (the first \
                        entry wins)",
                    definition.key
                ),
            ));
            continue;
        }
        if !raw_object.contains_key("updateCommands") {
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                base_path,
                format!(
                    "coding-agent '{}' has no persisted updateCommands; absent update commands are \
                        suppressed during reads and require managed-catalog migration on a \
                        supported restart",
                    definition.key
                ),
            ));
        }
        if push_unknown_field_warnings(raw_object, &definition.key, base_path, &mut warnings) {
            refresh_blocked = true;
        }
        entries.push(definition);
    }

    let mut edited = false;
    if let Some(marker) = &marker {
        if managed_content_sha256(&entries) != marker.content_sha256 {
            edited = true;
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MANAGED_BASE_EDITED,
                base_path,
                "the managed catalog content does not match its recorded contentSha256; it stays \
                    readable but is never auto-refreshed or auto-migrated",
            ));
        }
    }

    Ok(BaseAnalysis {
        kind,
        marker,
        entries,
        edited,
        refresh_blocked,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// Read snapshots and resolution.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq)]
struct CatalogSnapshot {
    base: Result<Option<Vec<u8>>, String>,
    // None is explicit exclusion: Instance/Direct never observe the project path.
    project: Option<Result<Option<Vec<u8>>, String>>,
    local: Result<Option<Vec<u8>>, String>,
    journal: Result<Option<Vec<u8>>, String>,
}

/// Bounded consecutive observations, not atomic publication by external editors.
fn read_catalog_snapshot(
    ac_dir: &Path,
    scope: CatalogReadScope,
) -> Result<CatalogSnapshot, String> {
    read_catalog_snapshot_with(ac_dir, scope, read_optional_regular_file)
}

fn read_catalog_snapshot_with(
    ac_dir: &Path,
    scope: CatalogReadScope,
    mut read: impl FnMut(&Path, &str) -> Result<Option<Vec<u8>>, String>,
) -> Result<CatalogSnapshot, String> {
    let mut previous = None;
    for _attempt in 0..3 {
        let snapshot = CatalogSnapshot {
            base: read(&manifest_path(ac_dir), "persisted catalog"),
            project: if scope == CatalogReadScope::Project {
                Some(read(
                    &project_catalog_path(ac_dir),
                    "project coding-agent catalog",
                ))
            } else {
                None
            },
            local: read(&local_catalog_path(ac_dir), "local coding-agent catalog"),
            journal: read(
                &migration_journal_path(ac_dir),
                "coding-agent migration journal",
            ),
        };
        if previous.as_ref() == Some(&snapshot) {
            return Ok(snapshot);
        }
        previous = Some(snapshot);
    }
    Err(if scope == CatalogReadScope::Project {
        "the catalog source, project overrides, local overrides and migration journal kept \
            changing while reading; retry the read"
    } else {
        "the catalog source, local overrides and migration journal kept changing while reading; \
            retry the read"
    }
    .to_string())
}

struct ResolvedCatalog {
    catalog: Vec<CodingAgentDefinition>,
    warnings: Vec<CatalogDiagnostic>,
    unavailable: Option<CatalogDiagnostic>,
    base_verified_managed: bool,
}

/// A local file that has NOT reached a managed base does not silently become
/// effective: it is diagnosed, and a legacy read stays legacy.
fn describe_local_presence(
    local: &Result<Option<Vec<u8>>, String>,
    local_path: &Path,
    warnings: &mut Vec<CatalogDiagnostic>,
    description: LayerDescription,
) {
    let layer = description.name();
    match local {
        Ok(None) => return,
        Ok(Some(local_bytes)) => {
            if let Err(reason) = parse_local_layer(local_bytes, description) {
                warnings.push(catalog_diagnostic(
                    description.invalid_code(),
                    local_path,
                    reason,
                ));
            }
        }
        Err(reason) => warnings.push(catalog_diagnostic(
            description.invalid_code(),
            local_path,
            reason.clone(),
        )),
    }
    warnings.push(catalog_diagnostic(
        REPORT_CODE_MIGRATION_PENDING,
        local_path,
        format!(
            "a {layer} overrides file exists while the base is not AC-managed; ownership \
            transfer is blocked until the managed migration completes and the {layer} layer is not \
            applied on top of a non-managed base"
        ),
    ));
}

/// Serve the verified legacy instance source recorded by an interrupted
/// instance import, when no managed base has been published yet.
fn journal_legacy_source(
    journal_bytes: &[u8],
) -> Result<Option<Vec<CodingAgentDefinition>>, String> {
    let journal = parse_migration_journal(journal_bytes)?;
    if journal.source_kind != MIGRATION_SOURCE_INSTANCE {
        return Ok(None);
    }
    let source_path = PathBuf::from(&journal.source_path);
    let Some(source_bytes) = read_instance_legacy_source(&source_path)? else {
        return Ok(None);
    };
    if sha256_hex(&source_bytes) != journal.source_sha256 {
        return Err(
            "the recorded migration source no longer matches the journal hash; recovery is blocked"
                .to_string(),
        );
    }
    let analysis = analyze_base_bytes(&source_path, &source_bytes).map_err(|diagnostic| {
        format!(
            "the recorded migration source is not readable: {}",
            diagnostic.code
        )
    })?;
    if analysis.kind != CatalogBaseKind::Legacy {
        return Ok(None);
    }
    Ok(Some(analysis.entries))
}

/// The single eligibility predicate for "this managed base is behind (or ahead
/// of) the current shipped revision": recognized ownership, a verified content
/// hash (not edited), no unknown fields blocking refresh, and a recorded
/// revision that differs from this build's shipped revision. The read-path
/// `refreshFailed` warning and `refresh_managed_base` both derive from this
/// predicate, so a warning can never promise a refresh the initializer would
/// refuse to perform.
fn managed_base_is_stale(analysis: &BaseAnalysis, shipped_revision: &str) -> bool {
    matches!(analysis.kind, CatalogBaseKind::Managed)
        && !analysis.edited
        && !analysis.refresh_blocked
        && analysis
            .marker
            .as_ref()
            .is_some_and(|marker| marker.revision != shipped_revision)
}

fn resolve_catalog_snapshot(
    ac_dir: &Path,
    snapshot: CatalogSnapshot,
    context: CatalogSourceContext,
) -> ResolvedCatalog {
    let base_path = manifest_path(ac_dir);
    let local_path = local_catalog_path(ac_dir);
    let journal_path = migration_journal_path(ac_dir);
    let mut resolved = ResolvedCatalog {
        catalog: Vec::new(),
        warnings: Vec::new(),
        unavailable: None,
        base_verified_managed: false,
    };

    let base_bytes = match &snapshot.base {
        Ok(Some(bytes)) => Some(bytes.clone()),
        Ok(None) => None,
        Err(reason) => {
            resolved.unavailable = Some(catalog_diagnostic(
                REPORT_CODE_BASE_UNAVAILABLE,
                &base_path,
                reason.clone(),
            ));
            return resolved;
        }
    };
    let journal_present = matches!(&snapshot.journal, Ok(Some(_)));

    let Some(base_bytes) = base_bytes else {
        if snapshot.journal.is_err() {
            let reason = snapshot.journal.as_ref().err().cloned().unwrap_or_default();
            resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_CONFLICT,
                &journal_path,
                reason,
            ));
        }
        if journal_present {
            resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                &journal_path,
                "an interrupted managed-catalog migration is pending; it resumes on the next \
                    initialization",
            ));
            let journal_bytes = snapshot
                .journal
                .as_ref()
                .ok()
                .and_then(|value| value.as_deref());
            if let Some(journal_bytes) = journal_bytes {
                match journal_legacy_source(journal_bytes) {
                    Ok(Some(entries)) => {
                        resolved.catalog =
                            apply_support_gate(entries, &base_path, &mut resolved.warnings);
                        return resolved;
                    }
                    Ok(None) => {}
                    Err(reason) => resolved.warnings.push(catalog_diagnostic(
                        REPORT_CODE_MIGRATION_CONFLICT,
                        &journal_path,
                        reason,
                    )),
                }
            }
        }
        if let Some(project) = &snapshot.project {
            match project {
                Ok(None) => {}
                Ok(Some(_)) => resolved.warnings.push(catalog_diagnostic(
                    REPORT_CODE_MIGRATION_PENDING,
                    &project_catalog_path(ac_dir),
                    "a project overrides file exists but no managed base has been published yet; \
                    the managed base is created on the next initialization",
                )),
                Err(reason) => resolved.warnings.push(catalog_diagnostic(
                    REPORT_CODE_PROJECT_INVALID,
                    &project_catalog_path(ac_dir),
                    reason.clone(),
                )),
            }
        }
        match &snapshot.local {
            Ok(None) => {}
            Ok(Some(_)) => resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                &local_path,
                "a local overrides file exists but no managed base has been published yet; the \
                    managed base is created on the next initialization",
            )),
            Err(reason) => resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_LOCAL_INVALID,
                &local_path,
                reason.clone(),
            )),
        }
        resolved.unavailable = Some(catalog_diagnostic(
            REPORT_CODE_BASE_UNAVAILABLE,
            &base_path,
            "no persisted catalog exists at this path; no embedded defaults are substituted",
        ));
        return resolved;
    };

    let analysis = match analyze_base_bytes(&base_path, &base_bytes) {
        Ok(analysis) => analysis,
        Err(diagnostic) => {
            resolved.unavailable = Some(diagnostic);
            return resolved;
        }
    };
    resolved.warnings.extend(analysis.warnings.iter().cloned());
    resolved.base_verified_managed = analysis.kind == CatalogBaseKind::Managed && !analysis.edited;
    // Read-path counterpart of the initialization refresh: a verified base at a
    // different shipped revision keeps serving its persisted entries (and the
    // local layer) while the restart guidance names the surface that can
    // actually act on it. A failed refresh needs no persistent marker - the
    // revision comparison itself recomputes this warning on every read.
    if managed_base_is_stale(
        &analysis,
        &managed_content_sha256(&supported_shipped_definitions()),
    ) {
        resolved.warnings.push(catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            &base_path,
            context.stale_revision_reason(),
        ));
    }

    match analysis.kind {
        CatalogBaseKind::Managed => {
            let mut effective = analysis.entries;
            for (observation, path, description) in snapshot
                .project
                .as_ref()
                .map(|value| {
                    (
                        value,
                        project_catalog_path(ac_dir),
                        LayerDescription::Project,
                    )
                })
                .into_iter()
                .chain(std::iter::once((
                    &snapshot.local,
                    local_path.clone(),
                    LayerDescription::Local,
                )))
            {
                let result = match observation {
                    Ok(Some(bytes)) => parse_local_layer(bytes, description)
                        .and_then(|layer| compose_local_layer(&effective, &layer, description)),
                    Ok(None) => continue,
                    Err(reason) => Err(reason.clone()),
                };
                match result {
                    Ok(candidate) => effective = candidate,
                    Err(reason) => resolved.warnings.push(catalog_diagnostic(
                        description.invalid_code(),
                        &path,
                        reason,
                    )),
                }
            }
            resolved.catalog = apply_support_gate(effective, &base_path, &mut resolved.warnings);
        }
        CatalogBaseKind::ForeignManaged => {
            resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_CONFLICT,
                &base_path,
                "the persisted catalog carries an unrecognized managed ownership marker; this \
                    build neither refreshes nor migrates it",
            ));
            if let Some(project) = &snapshot.project {
                describe_local_presence(
                    project,
                    &project_catalog_path(ac_dir),
                    &mut resolved.warnings,
                    LayerDescription::Project,
                );
            }
            describe_local_presence(
                &snapshot.local,
                &local_path,
                &mut resolved.warnings,
                LayerDescription::Local,
            );
            resolved.catalog =
                apply_support_gate(analysis.entries, &base_path, &mut resolved.warnings);
        }
        CatalogBaseKind::Legacy => {
            if let Some(project) = &snapshot.project {
                describe_local_presence(
                    project,
                    &project_catalog_path(ac_dir),
                    &mut resolved.warnings,
                    LayerDescription::Project,
                );
            }
            describe_local_presence(
                &snapshot.local,
                &local_path,
                &mut resolved.warnings,
                LayerDescription::Local,
            );
            resolved.catalog =
                apply_support_gate(analysis.entries, &base_path, &mut resolved.warnings);
        }
    }

    resolved
}
/// Per-item validation of a raw `updateCommands` value BEFORE deserialization,
/// so a non-string or unsafe item reports on that definition instead of failing
/// the whole file as malformed JSON. Returns a reason on the first offending
/// item; the offending text is never echoed. Accepted strings are preserved
/// exactly by serde (spaces, quoting, shell operators and flags stay intact).
fn raw_update_commands_problem(raw: &serde_json::Value) -> Option<String> {
    let value = raw.get("updateCommands")?;
    let serde_json::Value::Array(items) = value else {
        return Some(
            "its updateCommands value must be an array of complete command strings".to_string(),
        );
    };
    for (index, item) in items.iter().enumerate() {
        let serde_json::Value::String(command) = item else {
            return Some(format!("its updateCommands item {index} is not a string"));
        };
        if command.trim().is_empty() {
            return Some(format!("its updateCommands item {index} is blank"));
        }
        if command
            .chars()
            .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
        {
            return Some(format!(
                "its updateCommands item {index} contains a Unicode control character, U+2028 or \
                    U+2029"
            ));
        }
    }
    None
}

/// #2736 - the raw `installCommands` counterpart of
/// `raw_update_commands_problem`: a sanitized reason for a present value that
/// is not an object, lacks a string `default`, carries a non-string per-OS
/// value, or a blank or control-character value. Text is never echoed.
fn raw_install_commands_problem(raw: &serde_json::Value) -> Option<String> {
    let value = raw.get("installCommands")?;
    let serde_json::Value::Object(object) = value else {
        return Some("its installCommands value must be an object".to_string());
    };
    if !matches!(object.get("default"), Some(serde_json::Value::String(_))) {
        return Some("its installCommands.default must be a string".to_string());
    }
    for name in KNOWN_INSTALL_COMMANDS_FIELDS {
        let Some(item) = object.get(*name) else {
            continue;
        };
        let serde_json::Value::String(command) = item else {
            return Some(format!("its installCommands.{name} is not a string"));
        };
        if command.trim().is_empty() {
            return Some(format!("its installCommands.{name} is blank"));
        }
        if command
            .chars()
            .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
        {
            return Some(format!(
                "its installCommands.{name} contains a Unicode control character, U+2028 or U+2029"
            ));
        }
    }
    None
}

/// A sanitized reason for a definition the existing validator rejects. The
/// validator's own message can embed the command text, which must never appear
/// in a diagnostic; this classifier only names the failing field category.
fn definition_problem_reason(def: &CodingAgentDefinition) -> String {
    if validate_catalog_key(&def.key).is_err() {
        return "its key is invalid (keys must be non-empty and match ^[a-z0-9-]+$)".to_string();
    }
    let context = format!("Coding agent '{}'", def.key);
    if validate_agent_command_text(&context, &def.command).is_err() {
        return "its command is rejected by the current coding-agent command rules".to_string();
    }
    if validate_env_rows(&def.envs, &context).is_err() {
        return "its environment rows are invalid (duplicate or unsafe keys)".to_string();
    }
    if let Some(name) = def.instructions_filename.as_deref() {
        if !is_safe_instructions_filename(name) {
            return "its instructions filename is not a safe file name".to_string();
        }
    }
    if let Some(cfg) = def.config_seed.as_ref() {
        if !cfg.dest.trim().is_empty() && validate_config_seed_dest(&cfg.dest).is_err() {
            return "its config-seed destination is not a valid folder name".to_string();
        }
    }
    if let Some(burst) = def.idle_burst.as_ref() {
        if validate_idle_burst_config(burst, "Coding agent").is_err() {
            return "its idleBurst values are invalid (numbers must be finite and >= 0)"
                .to_string();
        }
    }
    if let Some(install) = def.install_commands.as_ref() {
        if validate_install_commands(install, "Coding agent").is_err() {
            return "its installCommands values are invalid (blank or control characters)"
                .to_string();
        }
    }
    "it failed the current catalog definition validation rules".to_string()
}

/// The unknown field NAMES of `keys` relative to `known`, joined for a reason
/// string; `None` when every field is known. Values are never read here.
fn unknown_field_names<'a>(
    keys: impl Iterator<Item = &'a String>,
    known: &[&str],
) -> Option<String> {
    let names: Vec<&str> = keys
        .map(String::as_str)
        .filter(|name| !known.contains(name))
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

/// migrationPending warnings for unknown fields on an ACCEPTED row: the row
/// itself, its `configSeed` object and its `envs` rows. Only field names are
/// reported; field VALUES (which may be commands or environment values) never
/// appear.
fn push_unknown_field_warnings(
    raw: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
    warnings: &mut Vec<CatalogDiagnostic>,
) -> bool {
    let mut found = false;
    if let Some(names) = unknown_field_names(raw.keys(), KNOWN_DEFINITION_FIELDS) {
        found = true;
        warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_PENDING,
            path,
            format!(
                "coding-agent '{key}' carries unknown field(s) ({names}) that require \
                    managed-catalog migration before ownership transfer"
            ),
        ));
    }
    if let Some(serde_json::Value::Object(seed)) = raw.get("configSeed") {
        if let Some(names) = unknown_field_names(seed.keys(), KNOWN_CONFIG_SEED_FIELDS) {
            found = true;
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                path,
                format!(
                    "coding-agent '{key}' configSeed carries unknown field(s) ({names}) that \
                        require managed-catalog migration"
                ),
            ));
        }
    }
    if let Some(serde_json::Value::Object(burst)) = raw.get("idleBurst") {
        if let Some(names) = unknown_field_names(burst.keys(), LOCAL_IDLE_BURST_FIELDS) {
            found = true;
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                path,
                format!(
                    "coding-agent '{key}' idleBurst carries unknown field(s) ({names}) that \
                        require managed-catalog migration"
                ),
            ));
        }
    }
    if let Some(serde_json::Value::Object(install)) = raw.get("installCommands") {
        if let Some(names) = unknown_field_names(install.keys(), KNOWN_INSTALL_COMMANDS_FIELDS) {
            found = true;
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                path,
                format!(
                    "coding-agent '{key}' installCommands carries unknown field(s) ({names}) that \
                        require managed-catalog migration"
                ),
            ));
        }
    }
    if let Some(serde_json::Value::Array(envs)) = raw.get("envs") {
        for (index, entry) in envs.iter().enumerate() {
            let serde_json::Value::Object(map) = entry else {
                continue;
            };
            if let Some(names) = unknown_field_names(map.keys(), KNOWN_ENV_FIELDS) {
                found = true;
                warnings.push(catalog_diagnostic(
                    REPORT_CODE_MIGRATION_PENDING,
                    path,
                    format!(
                        "coding-agent '{key}' envs[{index}] carries unknown field(s) ({names}) \
                            that require managed-catalog migration"
                    ),
                ));
            }
        }
    }
    found
}

/// Which surface asked for a catalog report. The wire shape never changes;
/// only the restart guidance attached to a stale managed revision differs:
/// project catalogs refresh during every startup/registration, the instance
/// catalog is initialized or refreshed at startup only when no project is
/// registered (every read stays read-only), and direct callers receive the
/// neutral wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogSourceContext {
    Project,
    Instance,
    Direct,
}

impl CatalogSourceContext {
    /// Exact `refreshFailed` reason for this surface (reviewed wording; the
    /// tests assert all three strings verbatim).
    fn stale_revision_reason(self) -> &'static str {
        match self {
            Self::Project => {
                "The persisted managed catalog revision differs from this build. Current persisted \
                    entries remain usable. Restart retries project catalog refresh."
            }
            Self::Instance => {
                "The persisted instance catalog revision differs from this build. Current \
                    persisted entries remain usable. Restart retries instance catalog refresh when \
                    no project is registered."
            }
            Self::Direct => {
                "The persisted managed catalog revision differs from this build. Current persisted \
                    entries remain usable. Catalog refresh runs during initialization."
            }
        }
    }
}

/// Load the persisted-only catalog report for one catalog root directory (a
/// project's `.ac` dir, or the legacy `<config_dir>` root in no-project mode).
/// `primaryProjectRoot` is left `None` here; the settings wrapper fills it.
/// This public entry point speaks for DIRECT callers (the array adapter and
/// context-free CLI/IPC reads), so it always uses the neutral wording; the
/// settings wrapper threads its own project/instance context privately.
/// Direct reads resolve 10/50 only; callers needing the primary project layer
/// use [`load_catalog_report_for_settings`].
/// READ-ONLY: never seeds, creates directories, refreshes, locks or writes.
/// `unavailable` is set (with an empty catalog) for a missing/unreadable/
/// nonregular/link source, invalid JSON, an unsupported explicit schemaVersion
/// or an invalid root shape; a valid empty catalog is a success. A managed base
/// composes its local layer behind the same resolver; a legacy base stays
/// readable and reports the pending migration. Definitions keep persisted order
/// after filtering; no embedded donor is ever consulted.
pub fn load_catalog_report(ac_dir: &Path) -> CatalogReport {
    load_catalog_report_with_context(
        ac_dir,
        CatalogSourceContext::Direct,
        CatalogReadScope::Instance,
    )
    .0
}

fn load_catalog_report_with_context(
    ac_dir: &Path,
    context: CatalogSourceContext,
    scope: CatalogReadScope,
) -> (CatalogReport, bool) {
    let path = manifest_path(ac_dir);
    let mut report = CatalogReport {
        primary_project_root: None,
        source_path: Some(path.display().to_string()),
        catalog: Vec::new(),
        warnings: Vec::new(),
        unavailable: None,
    };
    let snapshot = match read_catalog_snapshot(ac_dir, scope) {
        Ok(snapshot) => snapshot,
        Err(reason) => {
            report.unavailable = Some(catalog_diagnostic(
                REPORT_CODE_BASE_UNAVAILABLE,
                &path,
                reason,
            ));
            return (report, false);
        }
    };
    let resolved = resolve_catalog_snapshot(ac_dir, snapshot, context);
    report.catalog = resolved.catalog;
    report.warnings = resolved.warnings;
    report.unavailable = resolved.unavailable;
    (report, resolved.base_verified_managed)
}

/// Settings-level resolver: the primary project root (first nonblank
/// `project_paths` entry, else the legacy `project_path`) selects the project's
/// `.ac` dir; with no project the INSTANCE catalog at
/// `<config_dir>/coding-agents/agents.json` is read (reads never write; the
/// no-project boot path initializes or refreshes that file).
/// A failure in the primary project is never retried against another project.
/// P5 (#1968) extends this resolver to compose the local layer.
pub fn load_catalog_report_for_settings(settings: &AppSettings) -> CatalogReport {
    load_catalog_report_for_settings_with_config_dir(settings, crate::config::config_dir())
}

/// Testable twin of [`load_catalog_report_for_settings`] with the resolved
/// config dir injected, so the no-project and config-dir-none arms are
/// coverable without depending on the process-global instance location. A
/// verified managed PROJECT base additionally asks the seed manifest whether
/// the base publication is recorded; when it is not, the report carries
/// `publicationUntracked` while the verified base and local layer stay usable.
fn load_catalog_report_for_settings_with_config_dir(
    settings: &AppSettings,
    config_dir: Option<PathBuf>,
) -> CatalogReport {
    match primary_project_root(settings) {
        Some(root) => {
            let ac_dir = root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
            let (mut report, verified_managed_base) = load_catalog_report_with_context(
                &ac_dir,
                CatalogSourceContext::Project,
                CatalogReadScope::Project,
            );
            report.primary_project_root = Some(root.to_string_lossy().to_string());
            if verified_managed_base {
                let untracked = match has_catalog_publication(&root) {
                    Ok(true) => false,
                    Ok(false) => true,
                    Err(error) => {
                        log::debug!(
                            "[coding-agents] seed-manifest bookkeeping query failed for {} \
                                ({error})",
                            root.display()
                        );
                        true
                    }
                };
                if untracked {
                    let manifest_path = ac_dir.join(SEED_MANIFEST_FILENAME);
                    report.warnings.push(catalog_diagnostic(
                        REPORT_CODE_PUBLICATION_UNTRACKED,
                        &manifest_path,
                        "the seed manifest does not record this managed catalog publication yet; \
                            reads remain usable from the verified base and local overrides and the \
                            next initialization records it",
                    ));
                }
            }
            report
        }
        None => match config_dir {
            // Deliberately the private context-aware builder, NOT the public
            // Direct wrapper: reads never seed or migrate the instance (the
            // no-project boot path initializes it), so its restart guidance
            // must say so.
            Some(dir) => {
                load_catalog_report_with_context(
                    &dir,
                    CatalogSourceContext::Instance,
                    CatalogReadScope::Instance,
                )
                .0
            }
            None => report_without_source(),
        },
    }
}

// ---------------------------------------------------------------------------
// #1968 write machinery: directory/lock discipline, exclusive publication,
// migration, recovery, refresh and initialization.
// ---------------------------------------------------------------------------

/// The catalog publication targets and the lock, resolved once.
struct CatalogPaths {
    base: PathBuf,
    local: PathBuf,
    backup: PathBuf,
    journal: PathBuf,
    lock: PathBuf,
}

impl CatalogPaths {
    fn new(dir: &Path) -> Self {
        Self::with_names(dir, CATALOG_MANIFEST_FILENAME, LOCAL_CATALOG_FILENAME)
    }

    /// #2715 - the same composition over explicit data-file names, so an
    /// interrupted #1968 migration is recovered over the PRE-migration names.
    fn with_names(dir: &Path, base: &str, local: &str) -> Self {
        Self {
            base: dir.join(base),
            local: dir.join(local),
            backup: dir.join(MIGRATION_BACKUP_FILENAME),
            journal: dir.join(MIGRATION_JOURNAL_FILENAME),
            lock: dir.join(CATALOG_LOCK_FILENAME),
        }
    }
}

/// Create or validate the catalog directory and return its CANONICAL form, so
/// every cooperating writer locks one place even through raw, dot-segment or
/// case aliases. Creation may only happen under an existing real project `.ac`
/// directory; an existing catalog directory that is a symlink/reparse point or
/// a non-directory fails visibly.
fn ensure_catalog_dir(ac_dir: &Path) -> Result<PathBuf, String> {
    if !ac_dir.is_absolute() {
        return Err(format!(
            "the catalog root {} is not absolute; refusing to create it relative to the \
                process CWD",
            ac_dir.display()
        ));
    }
    let dir = catalog_dir(ac_dir);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || is_reparse_point(&meta) {
                return Err(format!(
                    "the catalog directory {} is a symbolic link or reparse point",
                    dir.display()
                ));
            }
            if !meta.is_dir() {
                return Err(format!(
                    "the catalog path {} exists and is not a directory",
                    dir.display()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // `.ac` itself may be absent (a direct caller owns the path); an
            // EXISTING `.ac` must be a real directory, never a link, and the
            // catalog directory is created inside it.
            if let Ok(parent) = std::fs::symlink_metadata(ac_dir) {
                if parent.file_type().is_symlink() || is_reparse_point(&parent) || !parent.is_dir()
                {
                    return Err(format!(
                        "the project .ac directory {} is not a real directory; refusing to create \
                            the catalog",
                        ac_dir.display()
                    ));
                }
            }
            std::fs::create_dir_all(&dir).map_err(|e| {
                format!(
                    "failed to create the catalog directory {} ({e})",
                    dir.display()
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "failed to inspect the catalog directory {} ({error})",
                dir.display()
            ))
        }
    }
    std::fs::canonicalize(&dir).map_err(|e| {
        format!(
            "failed to canonicalize the catalog directory {} ({e})",
            dir.display()
        )
    })
}

fn sync_parent_directory(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn write_synced_temp(temp: &Path, bytes: &[u8]) -> Result<(), String> {
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp)
            .map_err(|e| format!("create temp {} ({e})", temp.display()))?;
        file.write_all(bytes)
            .map_err(|e| format!("write temp {} ({e})", temp.display()))?;
        file.flush()
            .map_err(|e| format!("flush temp {} ({e})", temp.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync temp {} ({e})", temp.display()))?;
        drop(file);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// The hard-link publication boundary. Production compiles to exactly
/// `std::fs::hard_link(source, destination)`; the `#[cfg(test)]` arm lets a
/// destination-aware fixture drive the production cleanup/error-return code
/// with a filesystem-style `ErrorKind::Unsupported` result. That is arm-level
/// behavioral evidence, never a claim of measured unsupported-filesystem
/// support.
fn publish_hard_link(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(kind) = take_catalog_path_fault("hard_link", destination) {
        return Err(std::io::Error::new(kind, "injected hard-link failure"));
    }
    std::fs::hard_link(source, destination)
}

fn publication_temp_path(parent: &Path, destination_file_name: &str) -> PathBuf {
    let counter = SEED_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(publication_temp_name_for_destination(
        destination_file_name,
        std::process::id(),
        counter,
    ))
}

/// Publish `bytes` at a destination that must NOT exist, without ever exposing
/// partial bytes: a same-directory create-new temp is written, flushed and
/// fsynced, then hard-linked into place, and only this run's temp is unlinked.
/// A filesystem without hard links fails visibly; there is deliberately no
/// copy-to-destination fallback that could expose a partially written file.
fn publish_exclusive(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("publication path {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("publication path {} has no file name", path.display()))?;
    let temp = publication_temp_path(parent, name);
    write_synced_temp(&temp, bytes)?;
    inject_catalog_failure("publication_temp_synced")?;
    let result = publish_hard_link(&temp, path);
    let _ = std::fs::remove_file(&temp);
    match result {
        Ok(()) => {
            sync_parent_directory(path);
            Ok(())
        }
        Err(error) => Err(format!(
            "exclusive publication of {} failed via a hard link ({error}); the destination was \
                left untouched and no partial bytes were written",
            path.display()
        )),
    }
}

/// Replace an existing destination atomically through the vetted
/// `root_agent::atomic_replace_existing` primitive (plain rename when absent,
/// `ReplaceFileW` when present), after a same-directory create-new temp is
/// written, flushed and fsynced. The temp is removed on every failure path.
fn publish_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("publication path {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("publication path {} has no file name", path.display()))?;
    let temp = publication_temp_path(parent, name);
    write_synced_temp(&temp, bytes)?;
    inject_catalog_failure("publication_temp_synced")?;
    if let Err(error) = crate::config::root_agent::atomic_replace_existing(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    sync_parent_directory(path);
    Ok(())
}

/// A held catalog lock. The `File` IS the lock: dropping the guard closes the
/// handle and releases the OS lock, including on panic or process death. The
/// sidecar file itself is deliberately never deleted.
#[derive(Debug)]
struct CatalogLockGuard {
    _file: File,
}

#[cfg(test)]
thread_local! {
    static CATALOG_LOCK_TIMEOUT_OVERRIDE: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

fn catalog_lock_timeout() -> Duration {
    #[cfg(test)]
    if let Some(timeout) = CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.get()) {
        return timeout;
    }
    CATALOG_LOCK_TIMEOUT
}

/// Acquire the catalog sidecar lock: nontruncating create-once, 50 ms polling
/// against a 5 s deadline, with a distinct `catalogLockTimeout` failure. An OS
/// error that is not ordinary contention stops immediately with a visible
/// message: a filesystem without lock support must not silently degrade to
/// process-only exclusion.
fn acquire_catalog_lock(paths: &CatalogPaths) -> Result<CatalogLockGuard, String> {
    match std::fs::symlink_metadata(&paths.lock) {
        Ok(meta) if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_file() => {
            return Err(format!(
                "the coding-agent catalog lock {} must be a regular non-symlink file",
                paths.lock.display()
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "failed to inspect the coding-agent catalog lock {} ({error})",
                paths.lock.display()
            ))
        }
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&paths.lock)
        .map_err(|e| {
            format!(
                "failed to open the coding-agent catalog lock {} ({e})",
                paths.lock.display()
            )
        })?;
    if !file.metadata().map(|meta| meta.is_file()).unwrap_or(false) {
        return Err(format!(
            "the coding-agent catalog lock {} must be a regular file",
            paths.lock.display()
        ));
    }
    let timeout = catalog_lock_timeout();
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(CatalogLockGuard { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() >= timeout => {
                return Err(format!(
                    "catalogLockTimeout: timed out after {} ms waiting for the coding-agent \
                        catalog lock '{}'",
                    timeout.as_millis(),
                    paths.lock.display()
                ))
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(CATALOG_LOCK_POLL_INTERVAL.min(remaining));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!(
                    "the coding-agent catalog lock {} could not be acquired ({error}); this \
                        filesystem does not support the lock and no process-only fallback is used",
                    paths.lock.display()
                ))
            }
        }
    }
}

// Failure injection is catalog-local test plumbing, not a production runner
// framework: the hooks are `#[cfg(test)]`-only and compile to no-ops elsewhere.
#[cfg(test)]
thread_local! {
    static CATALOG_FAILURE_POINT: std::cell::Cell<Option<&'static str>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn inject_catalog_failure(point: &str) -> Result<(), String> {
    if CATALOG_FAILURE_POINT.with(|cell| cell.get()) == Some(point) {
        CATALOG_FAILURE_POINT.with(|cell| cell.set(None));
        return Err(format!("injected catalog failure at {point}"));
    }
    Ok(())
}

#[cfg(not(test))]
fn inject_catalog_failure(_point: &str) -> Result<(), String> {
    Ok(())
}

// Destination-aware failure injection: one-shot, TEST-THREAD-scoped, and
// matched by point name plus destination path, so a fault armed for one
// destination can never divert another publication (base vs local stub, base
// vs backup/journal). Same catalog-local plumbing contract as the point hooks
// above: no global state, no production cost.
#[cfg(test)]
#[derive(Debug)]
struct CatalogPathFault {
    point: &'static str,
    path: PathBuf,
    kind: std::io::ErrorKind,
}

#[cfg(test)]
thread_local! {
    static CATALOG_PATH_FAULTS: std::cell::RefCell<Vec<CatalogPathFault>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn arm_catalog_path_fault(point: &'static str, path: &Path, kind: std::io::ErrorKind) {
    CATALOG_PATH_FAULTS.with(|faults| {
        faults.borrow_mut().push(CatalogPathFault {
            point,
            path: path.to_path_buf(),
            kind,
        })
    });
}

#[cfg(test)]
fn clear_catalog_path_faults() {
    CATALOG_PATH_FAULTS.with(|faults| faults.borrow_mut().clear());
}

#[cfg(test)]
fn take_catalog_path_fault(point: &str, path: &Path) -> Option<std::io::ErrorKind> {
    CATALOG_PATH_FAULTS.with(|faults| {
        let mut faults = faults.borrow_mut();
        let index = faults.iter().position(|fault| {
            fault.point == point && catalog_fault_paths_match(&fault.path, path)
        })?;
        Some(faults.remove(index).kind)
    })
}

/// Compare the armed destination with the publishing destination even when the
/// catalog directory was canonicalized by `ensure_catalog_dir` (the armed path
/// may still carry the temp-dir spelling). Both parents exist by publication
/// time.
#[cfg(test)]
fn catalog_fault_paths_match(armed: &Path, publishing: &Path) -> bool {
    if armed == publishing {
        return true;
    }
    let armed_parent = armed
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok());
    let publishing_parent = publishing
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok());
    match (armed_parent, publishing_parent) {
        (Some(armed_parent), Some(publishing_parent)) => {
            armed_parent == publishing_parent && armed.file_name() == publishing.file_name()
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Legacy extraction, migration, recovery and refresh.
// ---------------------------------------------------------------------------

const MIGRATION_SOURCE_PROJECT: &str = "project";
const MIGRATION_SOURCE_INSTANCE: &str = "instance";

/// The immutable migration sidecar. It records the source identity, the local
/// layer identity and the COMPLETE intended managed base (with its own
/// revision/content digest), so an interrupted transaction can be recomputed
/// deterministically without consulting current embedded defaults. It is not a
/// secret store: diagnostics never print its body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationJournalWire {
    version: u32,
    source_kind: String,
    source_path: String,
    source_sha256: String,
    local_sha256: String,
    local_byte_length: u64,
    managed_revision: String,
    managed_content_sha256: String,
    managed_base: serde_json::Value,
}

fn parse_migration_journal(bytes: &[u8]) -> Result<MigrationJournalWire, String> {
    let value = parse_strict_json(bytes)
        .map_err(|reason| format!("the migration journal is not valid JSON ({reason})"))?;
    let journal: MigrationJournalWire = serde_json::from_value(value)
        .map_err(|error| format!("the migration journal does not match the v1 shape ({error})"))?;
    if journal.version != MIGRATION_JOURNAL_VERSION {
        return Err(format!(
            "unsupported migration journal version {}; only version 1 is recognized",
            journal.version
        ));
    }
    if journal.source_kind != MIGRATION_SOURCE_PROJECT
        && journal.source_kind != MIGRATION_SOURCE_INSTANCE
    {
        return Err("the migration journal carries an unrecognized sourceKind".to_string());
    }
    Ok(journal)
}

/// The serialized local layer. Only authored fields are emitted, so an
/// extracted pin carries exactly the legacy presence it represents.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct LocalRowWire {
    key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    remove: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "baseCommand")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions_filename: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    envs: Option<Vec<CodingAgentEnv>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    isolated_home: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_seed: Option<Option<ConfigSeedWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update_commands: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_update: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idle_burst: Option<Option<IdleBurstConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_commands: Option<Option<InstallCommandsWire>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoricalLocalRowWire<'a> {
    key: &'a String,
    #[serde(skip_serializing_if = "Option::is_none")]
    remove: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions_filename: &'a Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    envs: &'a Option<Vec<CodingAgentEnv>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    isolated_home: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_seed: &'a Option<Option<ConfigSeedWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removable: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update_commands: &'a Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_update: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idle_burst: &'a Option<Option<IdleBurstConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_commands: &'a Option<Option<InstallCommandsWire>>,
}
impl<'a> From<&'a LocalRowWire> for HistoricalLocalRowWire<'a> {
    fn from(row: &'a LocalRowWire) -> Self {
        Self {
            key: &row.key,
            remove: &row.remove,
            label: &row.label,
            description: &row.description,
            color: &row.color,
            command: &row.command,
            instructions_filename: &row.instructions_filename,
            envs: &row.envs,
            isolated_home: &row.isolated_home,
            config_seed: &row.config_seed,
            removable: &row.removable,
            update_commands: &row.update_commands,
            auto_update: &row.auto_update,
            idle_burst: &row.idle_burst,
            install_commands: &row.install_commands,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct InstallCommandsWire {
    default: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    windows: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    macos: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    linux: Option<String>,
}

impl From<&InstallCommands> for InstallCommandsWire {
    fn from(value: &InstallCommands) -> Self {
        Self {
            default: value.default.clone(),
            windows: value.windows.clone(),
            macos: value.macos.clone(),
            linux: value.linux.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConfigSeedWire {
    enabled: bool,
    dest: String,
}

impl From<&ConfigSeedConfig> for ConfigSeedWire {
    fn from(config: &ConfigSeedConfig) -> Self {
        Self {
            enabled: config.enabled,
            dest: config.dest.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalLayerWire<'a> {
    schema_version: u32,
    agents: Vec<LocalRowWithLegacy<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
struct LocalRowWithLegacy<'a> {
    #[serde(flatten)]
    row: &'a LocalRowWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<&'a serde_json::Value>,
}

fn local_layer_bytes(rows: Vec<LocalRowWire>, order: Option<Vec<String>>) -> Vec<u8> {
    local_layer_bytes_with_original(rows, order, None)
}

fn local_layer_bytes_with_original(
    rows: Vec<LocalRowWire>,
    order: Option<Vec<String>>,
    original: Option<&serde_json::Value>,
) -> Vec<u8> {
    let layer = LocalLayerWire {
        schema_version: CATALOG_SCHEMA_VERSION,
        agents: rows
            .iter()
            .map(|row| LocalRowWithLegacy {
                row,
                command: original_catalog_command(original, &row.key),
            })
            .collect(),
        order,
    };
    let mut bytes = match serde_json::to_vec_pretty(&layer) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::error!("[coding-agents] failed to serialize the local catalog layer ({error})");
            Vec::new()
        }
    };
    bytes.push(b'\n');
    bytes
}

/// Pin exactly the fields the legacy row states EXPLICITLY: presence, never
/// value equality with a current default, is the intent signal. An absent
/// `updateCommands` therefore inherits the newly persisted shipped default,
/// while an explicit `[]` stays empty.
fn pin_explicit_legacy_fields(
    definition: &CodingAgentDefinition,
    raw: &serde_json::Map<String, serde_json::Value>,
) -> LocalRowWire {
    LocalRowWire {
        key: definition.key.clone(),
        remove: None,
        label: raw.contains_key("label").then(|| definition.label.clone()),
        description: raw
            .contains_key("description")
            .then(|| definition.description.clone()),
        color: raw.contains_key("color").then(|| definition.color.clone()),
        command: (raw.contains_key("baseCommand") || raw.contains_key("command"))
            .then(|| definition.command.clone()),
        instructions_filename: raw
            .contains_key("instructionsFilename")
            .then(|| definition.instructions_filename.clone()),
        envs: raw.contains_key("envs").then(|| definition.envs.clone()),
        isolated_home: raw
            .contains_key("isolatedHome")
            .then_some(definition.isolated_home),
        config_seed: raw
            .contains_key("configSeed")
            .then(|| definition.config_seed.as_ref().map(ConfigSeedWire::from)),
        removable: raw
            .contains_key("removable")
            .then_some(definition.removable),
        update_commands: raw
            .contains_key("updateCommands")
            .then(|| definition.update_commands.clone()),
        auto_update: raw
            .contains_key("autoUpdate")
            .then_some(definition.auto_update),
        idle_burst: raw
            .contains_key("idleBurst")
            .then(|| definition.idle_burst.clone()),
        install_commands: raw.contains_key("installCommands").then(|| {
            definition
                .install_commands
                .as_ref()
                .map(InstallCommandsWire::from)
        }),
    }
}

/// Materialize a COMPLETE local definition for a custom key or a changed
/// command, using the values the legacy read produced (explicit or default) and
/// forcing an absent `updateCommands` to `[]` so the new shipped sequence never
/// leaks into a command the user owns.
fn materialize_complete_legacy_fields(
    definition: &CodingAgentDefinition,
    raw: &serde_json::Map<String, serde_json::Value>,
) -> LocalRowWire {
    LocalRowWire {
        key: definition.key.clone(),
        remove: None,
        label: Some(definition.label.clone()),
        description: Some(definition.description.clone()),
        color: Some(definition.color.clone()),
        command: Some(definition.command.clone()),
        instructions_filename: raw
            .contains_key("instructionsFilename")
            .then(|| definition.instructions_filename.clone()),
        envs: Some(definition.envs.clone()),
        isolated_home: Some(definition.isolated_home),
        config_seed: raw
            .contains_key("configSeed")
            .then(|| definition.config_seed.as_ref().map(ConfigSeedWire::from)),
        removable: Some(definition.removable),
        update_commands: Some(if raw.contains_key("updateCommands") {
            definition.update_commands.clone()
        } else {
            Vec::new()
        }),
        auto_update: Some(definition.auto_update),
        idle_burst: raw
            .contains_key("idleBurst")
            .then(|| definition.idle_burst.clone()),
        install_commands: raw.contains_key("installCommands").then(|| {
            definition
                .install_commands
                .as_ref()
                .map(InstallCommandsWire::from)
        }),
    }
}

/// Compute the extracted local layer for a readable legacy catalog against the
/// intended (shipped) managed base. Every ambiguity refuses the transfer: a
/// duplicate member (already rejected by the strict parser), an unknown
/// field, an invalid definition, a duplicate key or a non-removable missing
/// shipped key. Nothing is written here.
fn extract_legacy_local(
    source_bytes: &[u8],
    shipped: &[CodingAgentDefinition],
) -> Result<Vec<u8>, String> {
    extract_legacy_local_mode(source_bytes, shipped, false)
}

fn extract_legacy_local_mode(
    source_bytes: &[u8],
    shipped: &[CodingAgentDefinition],
    historical: bool,
) -> Result<Vec<u8>, String> {
    let value = parse_strict_json(source_bytes)
        .map_err(|reason| format!("the legacy catalog is not valid JSON ({reason})"))?;
    let root = expect_json_object(&value, "the legacy catalog root")?;
    reject_unknown_json_fields(
        root,
        &["schemaVersion", "agents"],
        "the legacy catalog root",
    )?;
    if let Some(version) = root.get("schemaVersion") {
        if version.as_u64() != Some(CATALOG_SCHEMA_VERSION as u64) {
            return Err(format!(
                "unsupported legacy schemaVersion {version}; migration is refused"
            ));
        }
    }
    let empty_rows = Vec::new();
    let rows = match root.get("agents") {
        None => &empty_rows,
        Some(serde_json::Value::Array(rows)) => rows,
        Some(_) => return Err("the legacy 'agents' value must be a JSON array".to_string()),
    };

    let shipped_by_key: HashMap<&str, &CodingAgentDefinition> = shipped
        .iter()
        .map(|definition| (definition.key.as_str(), definition))
        .collect();

    let mut seen_keys = HashSet::new();
    let mut legacy: Vec<(
        &CodingAgentDefinition,
        &serde_json::Map<String, serde_json::Value>,
    )> = Vec::with_capacity(rows.len());
    let mut parsed: Vec<CodingAgentDefinition> = Vec::with_capacity(rows.len());
    for (index, raw) in rows.iter().enumerate() {
        let context = format!("legacy entry {index}");
        let object = expect_json_object(raw, &context)?;
        reject_unknown_json_fields(object, KNOWN_DEFINITION_FIELDS, &context)?;
        if let Some(problem) =
            raw_update_commands_problem(raw).or_else(|| raw_install_commands_problem(raw))
        {
            return Err(format!("{context} is ambiguous: {problem}"));
        }
        let definition: CodingAgentDefinition = serde_json::from_value(raw.clone())
            .map_err(|_| format!("{context} does not match the coding-agent definition schema"))?;
        if validate_definition(&definition).is_err() {
            return Err(format!(
                "{context} fails validation: {}",
                definition_problem_reason(&definition)
            ));
        }
        let mut nested = Vec::new();
        if push_unknown_field_warnings(object, &definition.key, Path::new("legacy"), &mut nested) {
            return Err(format!(
                "{context} carries unknown nested field(s) that cannot be migrated safely"
            ));
        }
        if !seen_keys.insert(definition.key.clone()) {
            return Err(format!(
                "duplicate legacy coding-agent key '{}'",
                definition.key
            ));
        }
        parsed.push(definition);
    }
    for (index, raw) in rows.iter().enumerate() {
        let object = raw
            .as_object()
            .expect("validated as an object in the first pass");
        legacy.push((&parsed[index], object));
    }

    let mut local_rows: Vec<LocalRowWire> = Vec::with_capacity(parsed.len() + shipped.len());
    let mut order: Vec<String> = Vec::with_capacity(parsed.len());
    for (definition, object) in &legacy {
        order.push(definition.key.clone());
        match shipped_by_key.get(definition.key.as_str()) {
            Some(shipped_definition) if shipped_definition.command == definition.command => {
                local_rows.push(pin_explicit_legacy_fields(definition, object));
            }
            _ => local_rows.push(materialize_complete_legacy_fields(definition, object)),
        }
    }
    for shipped_definition in shipped {
        if !seen_keys.contains(&shipped_definition.key) {
            if !shipped_definition.removable {
                return Err(format!(
                    "reconciliation is blocked: shipped key '{}' is absent from the legacy catalog \
                        and is not removable, so no tombstone can be recorded",
                    shipped_definition.key
                ));
            }
            local_rows.push(LocalRowWire {
                key: shipped_definition.key.clone(),
                remove: Some(true),
                ..LocalRowWire::default()
            });
        }
    }
    if historical {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct HistoricalLayer<'a> {
            schema_version: u32,
            agents: Vec<HistoricalLocalRowWire<'a>>,
            order: Option<Vec<String>>,
        }
        let layer = HistoricalLayer {
            schema_version: CATALOG_SCHEMA_VERSION,
            agents: local_rows
                .iter()
                .map(HistoricalLocalRowWire::from)
                .collect(),
            order: Some(order),
        };
        let mut bytes = serde_json::to_vec_pretty(&layer).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        return Ok(bytes);
    }
    if local_rows
        .iter()
        .all(|row| original_catalog_command(Some(&value), &row.key).is_none())
    {
        Ok(local_layer_bytes(local_rows, Some(order)))
    } else {
        Ok(local_layer_bytes_with_original(
            local_rows,
            Some(order),
            Some(&value),
        ))
    }
}

/// Execute one migration transaction under the held catalog lock. Order:
/// durable exclusive backup, durable journal, exclusive local layer (verified),
/// source/target re-check, then the base. Every failure leaves the earlier
/// artifacts in place for recovery and never publishes partial bytes.
fn migrate_legacy(
    paths: &CatalogPaths,
    source_kind: &'static str,
    source_path: &Path,
    source_bytes: &[u8],
) -> Result<Option<DateTime<Utc>>, String> {
    if read_optional_regular_file(&paths.local, "local coding-agent catalog")?.is_some() {
        return Err(
            "an existing local overrides file blocks the legacy ownership transfer; it was left \
                untouched"
                .to_string(),
        );
    }
    if read_optional_regular_file(&paths.backup, "coding-agent migration backup")?.is_some()
        || read_optional_regular_file(&paths.journal, "coding-agent migration journal")?.is_some()
    {
        return Err(
            "an existing migration sidecar blocks a new migration; recover or reconcile it first"
                .to_string(),
        );
    }

    let shipped = supported_shipped_definitions();
    let managed_bytes = build_managed_base_bytes(&shipped);
    let local_bytes = extract_legacy_local(source_bytes, &shipped)?;
    // All extraction is computed and strictly validated before ANY write.
    let layer = parse_local_layer(&local_bytes, LayerDescription::Local)?;
    compose_local_layer(&shipped, &layer, LayerDescription::Local)?;
    let managed_base_value: serde_json::Value = serde_json::from_slice(&managed_bytes)
        .map_err(|e| format!("the intended managed base did not serialize to JSON ({e})"))?;
    let revision = managed_content_sha256(&shipped);
    let journal = MigrationJournalWire {
        version: MIGRATION_JOURNAL_VERSION,
        source_kind: source_kind.to_string(),
        source_path: source_path.display().to_string(),
        source_sha256: sha256_hex(source_bytes),
        local_sha256: sha256_hex(&local_bytes),
        local_byte_length: local_bytes.len() as u64,
        managed_revision: revision.clone(),
        managed_content_sha256: revision,
        managed_base: managed_base_value,
    };
    let mut journal_bytes = serde_json::to_vec_pretty(&journal)
        .map_err(|e| format!("the migration journal did not serialize ({e})"))?;
    journal_bytes.push(b'\n');

    publish_exclusive(&paths.backup, source_bytes)?;
    inject_catalog_failure("after_backup")?;
    publish_exclusive(&paths.journal, &journal_bytes)?;
    inject_catalog_failure("after_journal")?;
    publish_exclusive(&paths.local, &local_bytes)?;
    let written = std::fs::read(&paths.local)
        .map_err(|e| format!("the published local layer could not be re-read ({e})"))?;
    if written.len() as u64 != journal.local_byte_length
        || sha256_hex(&written) != journal.local_sha256
    {
        return Err(
            "the published local layer did not verify byte-for-byte; the base was not published"
                .to_string(),
        );
    }
    inject_catalog_failure("after_local")?;

    if source_kind == MIGRATION_SOURCE_INSTANCE {
        if read_optional_regular_file(&paths.base, "persisted catalog")?.is_some() {
            return Err(
                "a managed base appeared during the migration; the base was not published"
                    .to_string(),
            );
        }
        let current = std::fs::read(source_path)
            .map_err(|e| format!("the instance source could not be re-read ({e})"))?;
        if current != source_bytes {
            return Err(
                "the instance source changed during the migration; the base was not published"
                    .to_string(),
            );
        }
    } else {
        let current = std::fs::read(&paths.base)
            .map_err(|e| format!("the project base could not be re-read ({e})"))?;
        if current != source_bytes {
            return Err(
                "the project base changed during the migration; it was left untouched".to_string(),
            );
        }
    }
    inject_catalog_failure("before_base")?;
    let published_at = Utc::now();
    if source_kind == MIGRATION_SOURCE_PROJECT {
        publish_replace(&paths.base, &managed_bytes)?;
    } else {
        publish_exclusive(&paths.base, &managed_bytes)?;
    }
    inject_catalog_failure("after_base")?;
    log::info!(
        "[coding-agents] migrated the {source_kind} catalog {} into the managed base with an \
            extracted local layer",
        source_path.display()
    );
    Ok(Some(published_at))
}

/// Resume an interrupted transaction recorded by a journal: complete when the
/// published base already matches the journal identity, otherwise recompute and
/// verify the local layer before finishing. Any mismatch preserves every byte
/// and blocks with a reconciliation reason.
fn recover_interrupted_migration(paths: &CatalogPaths) -> Result<Option<DateTime<Utc>>, String> {
    let journal_bytes = std::fs::read(&paths.journal)
        .map_err(|e| format!("the migration journal could not be read ({e})"))?;
    let journal = parse_migration_journal(&journal_bytes)?;
    let backup = std::fs::read(&paths.backup).map_err(|e| {
        format!(
            "the migration backup is missing or unreadable ({e}); recovery is blocked and every \
                byte is preserved"
        )
    })?;
    if sha256_hex(&backup) != journal.source_sha256 {
        return Err(
            "the migration backup does not match the journal source hash; recovery is blocked and \
                every byte is preserved"
                .to_string(),
        );
    }
    let managed_base_value = journal.managed_base.clone();
    let saved_agents = managed_base_value
        .get("agents")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let managed_definitions: Vec<CodingAgentDefinition> = serde_json::from_value(saved_agents)
        .map_err(|_| {
            "the journal saved managed base is not a coding-agent definition array".to_string()
        })?;
    let saved_revision = managed_base_value
        .get("managed")
        .and_then(|marker| marker.get("revision"))
        .and_then(|value| value.as_str());
    let saved_content = managed_base_value
        .get("managed")
        .and_then(|marker| marker.get("contentSha256"))
        .and_then(|value| value.as_str());
    if saved_revision != Some(journal.managed_revision.as_str())
        || saved_content != Some(journal.managed_content_sha256.as_str())
    {
        return Err(
            "the journal saved managed base does not match its recorded revision; recovery is \
                blocked"
                .to_string(),
        );
    }
    let mut managed_bytes = serde_json::to_vec_pretty(&managed_base_value)
        .map_err(|e| format!("the journal saved managed base did not serialize ({e})"))?;
    managed_bytes.push(b'\n');

    if let Some(base_bytes) = read_optional_regular_file(&paths.base, "persisted catalog")? {
        if let Ok(analysis) = analyze_base_bytes(&paths.base, &base_bytes) {
            if let Some(marker) = analysis.marker.as_ref() {
                if marker.revision == journal.managed_revision
                    && marker.content_sha256 == journal.managed_content_sha256
                {
                    // Completed: never re-extract or overwrite the local layer.
                    return Ok(None);
                }
            }
        }
        if journal.source_kind == MIGRATION_SOURCE_INSTANCE || base_bytes != backup {
            return Err(
                "the existing catalog base does not match the interrupted migration; recovery is \
                    blocked and every byte is preserved"
                    .to_string(),
            );
        }
        // Project migration: the base still holds the exact source bytes, so
        // the base was never replaced; continue below and finish the sequence.
    }

    let published_at = match read_optional_regular_file(&paths.local, "local coding-agent catalog")?
    {
        Some(local_bytes) => {
            if sha256_hex(&local_bytes) != journal.local_sha256
                || local_bytes.len() as u64 != journal.local_byte_length
            {
                return Err(
                    "the local overrides file does not match the interrupted migration; \
                            recovery is blocked and every byte is preserved"
                        .to_string(),
                );
            }
            if journal.source_kind == MIGRATION_SOURCE_INSTANCE {
                let source_path = PathBuf::from(&journal.source_path);
                if let Some(source_bytes) =
                    read_optional_regular_file(&source_path, "recorded migration source")?
                {
                    if sha256_hex(&source_bytes) != journal.source_sha256 {
                        return Err(
                            "the instance source changed during the interrupted migration; \
                                    recovery is blocked"
                                .to_string(),
                        );
                    }
                }
            }
            publish_base(&paths.base, &managed_bytes, journal.source_kind.as_str())?;
            Utc::now()
        }
        None => {
            let mut recomputed = extract_legacy_local(&backup, &managed_definitions)?;
            if sha256_hex(&recomputed) != journal.local_sha256
                || recomputed.len() as u64 != journal.local_byte_length
            {
                recomputed = extract_legacy_local_mode(&backup, &managed_definitions, true)?;
            }
            if sha256_hex(&recomputed) != journal.local_sha256
                || recomputed.len() as u64 != journal.local_byte_length
            {
                return Err(
                    "the recomputed extraction does not match the journal local hash; recovery \
                            is blocked and every byte is preserved"
                        .to_string(),
                );
            }
            publish_exclusive(&paths.local, &recomputed)?;
            inject_catalog_failure("after_local")?;
            publish_base(&paths.base, &managed_bytes, journal.source_kind.as_str())?;
            Utc::now()
        }
    };
    inject_catalog_failure("after_base")?;
    log::info!("[coding-agents] resumed and completed an interrupted managed-catalog migration");
    Ok(Some(published_at))
}

/// Finish a journal recovery's base publication: a project migration replaces
/// the source file in place, while an instance import publishes into a
/// destination that must still be absent.
fn publish_base(path: &Path, bytes: &[u8], source_kind: &str) -> Result<(), String> {
    if source_kind == MIGRATION_SOURCE_PROJECT {
        publish_replace(path, bytes)
    } else {
        publish_exclusive(path, bytes)
    }
}

/// Backup-only recovery: no journal, so the only admissible continuation is the
/// case where the backup is byte-equal to the CURRENT original source and no
/// local file exists. The extraction is recomputed against the shipped defaults
/// because no saved base exists to recompute against.
fn resume_backup_only_migration(
    paths: &CatalogPaths,
    legacy_catalog_dir: Option<&Path>,
) -> Result<Option<DateTime<Utc>>, String> {
    let backup = std::fs::read(&paths.backup)
        .map_err(|e| format!("the migration backup could not be read ({e})"))?;
    if read_optional_regular_file(&paths.local, "local coding-agent catalog")?.is_some() {
        return Err(
            "a local overrides file exists without a journal; the interrupted migration cannot be \
                resumed safely"
                .to_string(),
        );
    }
    let shipped = supported_shipped_definitions();
    let base_bytes = read_optional_regular_file(&paths.base, "persisted catalog")?;
    let instance_path = legacy_catalog_dir.map(|dir| dir.join(CATALOG_MANIFEST_FILENAME));
    let instance_bytes = match instance_path.as_ref() {
        Some(path) => read_instance_legacy_source(path)?,
        None => None,
    };
    let source_kind =
        match (&base_bytes, &instance_bytes) {
            (Some(base), _) if *base == backup => MIGRATION_SOURCE_PROJECT,
            (None, Some(instance)) if *instance == backup => MIGRATION_SOURCE_INSTANCE,
            _ => return Err(
                "the backup does not match the current project base or instance catalog; recovery \
                    is blocked and every byte is preserved"
                    .to_string(),
            ),
        };
    let managed_bytes = build_managed_base_bytes(&shipped);
    let local_bytes = extract_legacy_local(&backup, &shipped)?;
    let layer = parse_local_layer(&local_bytes, LayerDescription::Local)?;
    compose_local_layer(&shipped, &layer, LayerDescription::Local)?;
    publish_exclusive(&paths.local, &local_bytes)?;
    let published_at = Utc::now();
    if source_kind == MIGRATION_SOURCE_PROJECT {
        publish_replace(&paths.base, &managed_bytes)?;
    } else {
        publish_exclusive(&paths.base, &managed_bytes)?;
    }
    log::info!("[coding-agents] resumed a backup-only interrupted migration");
    Ok(Some(published_at))
}

/// Refresh a verified, unedited managed base when the current shipped revision
/// differs from the base's recorded revision (a support-gate change is part of
/// that revision). A base whose content hash does not match its marker is NEVER
/// refreshed, and neither is one carrying unknown fields: refresh replaces only
/// a verified managed base.
fn refresh_managed_base(
    paths: &CatalogPaths,
    analysis: &BaseAnalysis,
) -> Result<Option<DateTime<Utc>>, String> {
    let shipped = supported_shipped_definitions();
    let shipped_revision = managed_content_sha256(&shipped);
    if !managed_base_is_stale(analysis, &shipped_revision) {
        return Ok(None);
    }
    let before = read_optional_regular_file(&paths.base, "persisted catalog")?
        .ok_or("catalog disappeared before refresh")?;
    let before = parse_strict_json(&before)?;
    let managed_bytes = build_managed_base_bytes_with_original(&shipped, Some(&before));
    publish_replace(&paths.base, &managed_bytes)?;
    log::info!(
        "[coding-agents] refreshed the managed catalog base revision at {}",
        paths.base.display()
    );
    Ok(Some(Utc::now()))
}

/// Fresh initialization: exclusive managed defaults plus the create-once local
/// stub. The base is written first; a stub failure leaves the valid base
/// published and usable, and NO automatic stub retry is scheduled - an absent
/// local file is valid, so an intentional deletion stays deleted. The failure
/// is returned alongside the successful publication so the caller's existing
/// initialization logging carries it; nothing durable records it.
fn fresh_initialize_catalog(
    paths: &CatalogPaths,
) -> Result<(Option<DateTime<Utc>>, Option<CatalogDiagnostic>), String> {
    let shipped = supported_shipped_definitions();
    let managed_bytes = build_managed_base_bytes(&shipped);
    let published_at = if read_optional_regular_file(&paths.base, "persisted catalog")?.is_none() {
        publish_exclusive(&paths.base, &managed_bytes)?;
        Some(Utc::now())
    } else {
        None
    };
    let stub_warning = |detail: String| {
        Some(catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            &paths.local,
            format!(
                "The managed catalog base is usable, but its local overrides stub could not be \
                    created: {detail}. An absent local file is valid; no automatic stub retry is \
                    scheduled."
            ),
        ))
    };
    let stub_warning = match std::fs::symlink_metadata(&paths.local) {
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match publish_exclusive(&paths.local, LOCAL_STUB_BYTES) {
                Ok(()) => None,
                Err(stub_error) => stub_warning(stub_error),
            }
        }
        Err(error) => stub_warning(error.to_string()),
    };
    Ok((published_at, stub_warning))
}

#[derive(Default)]
struct CatalogInitOutcome {
    published_at: Option<DateTime<Utc>>,
    base_verified_managed: bool,
    warnings: Vec<CatalogDiagnostic>,
}

/// Which surface is being initialized. `Project` keeps the complete project
/// state machine above it (recovery, migration, seed-manifest ownership);
/// `Instance` is the #2021 no-project instance catalog, which may only
/// fresh-seed an absent base or refresh a stale managed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitScope {
    Project,
    Instance,
}

/// One initialization pass UNDER THE HELD CATALOG LOCK: recover an interrupted
/// transaction, then refresh, migrate or fresh-seed exactly one base. Order is
/// fixed by the plan: recovery first, then the base state machine. `scope`
/// selects the #2021 instance policy: instance initialization never resumes a
/// migration journal or backup, and never migrates a legacy file in place.
fn initialize_catalog_under_lock(
    paths: &CatalogPaths,
    legacy_catalog_dir: Option<&Path>,
    scope: InitScope,
) -> CatalogInitOutcome {
    let mut outcome = CatalogInitOutcome::default();

    // #2021: no production caller has ever run this initialization on the
    // config dir, so a journal or backup beside an instance catalog cannot be
    // ours. Leave any such sidecar untouched and never resume it.
    if scope == InitScope::Project {
        let journal_exists = match std::fs::symlink_metadata(&paths.journal) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                outcome.warnings.push(catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    &paths.journal,
                    format!("the migration journal could not be inspected ({error})"),
                ));
                return outcome;
            }
        };
        let backup_exists = match std::fs::symlink_metadata(&paths.backup) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                outcome.warnings.push(catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    &paths.backup,
                    format!("the migration backup could not be inspected ({error})"),
                ));
                return outcome;
            }
        };

        if journal_exists {
            match recover_interrupted_migration(paths) {
                Ok(published_at) => outcome.published_at = published_at,
                Err(reason) => {
                    outcome.warnings.push(catalog_diagnostic(
                        REPORT_CODE_MIGRATION_CONFLICT,
                        &paths.journal,
                        reason,
                    ));
                    return outcome;
                }
            }
        } else if backup_exists {
            match resume_backup_only_migration(paths, legacy_catalog_dir) {
                Ok(published_at) => outcome.published_at = published_at,
                Err(reason) => {
                    outcome.warnings.push(catalog_diagnostic(
                        REPORT_CODE_MIGRATION_CONFLICT,
                        &paths.backup,
                        reason,
                    ));
                    return outcome;
                }
            }
        }
    }

    let base_bytes = match read_optional_regular_file(&paths.base, "persisted catalog") {
        Ok(bytes) => bytes,
        Err(reason) => {
            outcome.warnings.push(catalog_diagnostic(
                REPORT_CODE_REFRESH_FAILED,
                &paths.base,
                reason,
            ));
            return outcome;
        }
    };

    match base_bytes {
        Some(bytes) => {
            let analysis = match analyze_base_bytes(&paths.base, &bytes) {
                Ok(analysis) => analysis,
                Err(diagnostic) => {
                    // Corrupt bytes are preserved and unavailable; nothing is
                    // rewritten, refreshed or migrated.
                    outcome.warnings.push(diagnostic);
                    return outcome;
                }
            };
            outcome.base_verified_managed =
                analysis.kind == CatalogBaseKind::Managed && !analysis.edited;
            outcome.warnings.extend(analysis.warnings.iter().cloned());
            match analysis.kind {
                CatalogBaseKind::Managed => {
                    if !analysis.edited && !analysis.refresh_blocked {
                        match refresh_managed_base(paths, &analysis) {
                            Ok(Some(published_at)) => outcome.published_at = Some(published_at),
                            Ok(None) => {}
                            Err(reason) => {
                                outcome.warnings.push(catalog_diagnostic(
                                    REPORT_CODE_REFRESH_FAILED,
                                    &paths.base,
                                    reason,
                                ));
                            }
                        }
                    }
                }
                CatalogBaseKind::ForeignManaged => {}
                CatalogBaseKind::Legacy => {
                    // #2021: a legacy instance file is the migration source for
                    // future projects; the instance scope never migrates or
                    // rewrites it in place.
                    if scope == InitScope::Project {
                        match migrate_legacy(paths, MIGRATION_SOURCE_PROJECT, &paths.base, &bytes) {
                            Ok(published_at) => {
                                outcome.published_at = published_at.or(outcome.published_at)
                            }
                            Err(reason) => outcome.warnings.push(catalog_diagnostic(
                                REPORT_CODE_MIGRATION_CONFLICT,
                                &paths.base,
                                reason,
                            )),
                        }
                    }
                }
            }
        }
        None => {
            // An absent project base may import ONLY the instance agents.json.
            let instance_path = legacy_catalog_dir.map(|dir| dir.join(CATALOG_MANIFEST_FILENAME));
            let instance_bytes = match instance_path.as_ref() {
                Some(path) => match read_instance_legacy_source(path) {
                    Ok(bytes) => bytes,
                    Err(reason) => {
                        outcome.warnings.push(catalog_diagnostic(
                            REPORT_CODE_MIGRATION_CONFLICT,
                            path,
                            reason,
                        ));
                        return outcome;
                    }
                },
                None => None,
            };
            // #2021 2.3: a MANAGED (or foreign-managed) instance base is not
            // importable legacy territory; importing it would fail with
            // MIGRATION_CONFLICT and leave the project without a catalog, so
            // fresh-seed the project's own managed base instead. A marker-less
            // legacy file, or bytes this build cannot parse, stay on the
            // existing migration path unchanged (it already reports corrupt
            // input).
            let instance_is_managed = match (instance_bytes.as_deref(), instance_path.as_ref()) {
                (Some(bytes), Some(path)) => matches!(
                    analyze_base_bytes(path, bytes),
                    Ok(analysis)
                        if matches!(
                            analysis.kind,
                            CatalogBaseKind::Managed | CatalogBaseKind::ForeignManaged
                        )
                ),
                _ => false,
            };
            match (instance_bytes, instance_path) {
                (Some(bytes), Some(path)) if !instance_is_managed => {
                    match migrate_legacy(paths, MIGRATION_SOURCE_INSTANCE, &path, &bytes) {
                        Ok(published_at) => outcome.published_at = published_at,
                        Err(reason) => outcome.warnings.push(catalog_diagnostic(
                            REPORT_CODE_MIGRATION_CONFLICT,
                            &path,
                            reason,
                        )),
                    }
                }
                _ => match fresh_initialize_catalog(paths) {
                    Ok((published_at, stub_warning)) => {
                        outcome.published_at = published_at;
                        outcome.warnings.extend(stub_warning);
                    }
                    Err(reason) => outcome.warnings.push(catalog_diagnostic(
                        REPORT_CODE_REFRESH_FAILED,
                        &paths.base,
                        reason,
                    )),
                },
            }
        }
    }

    outcome
}

fn run_catalog_initialization(
    ac_dir: &Path,
    legacy_catalog_dir: Option<&Path>,
    journal_dir: Option<&Path>,
) -> CatalogInitOutcome {
    run_catalog_initialization_with_scope(
        ac_dir,
        legacy_catalog_dir,
        journal_dir,
        InitScope::Project,
    )
}

/// #2021: initialize or refresh the INSTANCE catalog at
/// `<config_dir>/coding-agents/agents.json` when no project is registered, so
/// the first-run Welcome surface lists the built-in coding agents.
///
/// Same lock discipline, path resolution and fail-soft logging as
/// [`run_catalog_initialization`], with instance semantics: a fresh or stale
/// managed base is initialized or refreshed, while a legacy, foreign-managed,
/// edited or corrupt instance file is never rewritten and never migrated. The
/// instance neither imports from nor publishes to another location. Returns
/// the publication time when the managed base was actually written.
pub(crate) fn ensure_seeded_instance(config_dir: &Path) -> Option<DateTime<Utc>> {
    ensure_seeded_instance_in(config_dir, Some(config_dir))
}

/// [`ensure_seeded_instance`] with the naming-migration journal directory
/// injected (#2715), so a test never touches the process-wide `config_dir()`.
fn ensure_seeded_instance_in(
    config_dir: &Path,
    journal_dir: Option<&Path>,
) -> Option<DateTime<Utc>> {
    run_catalog_initialization_with_scope(config_dir, None, journal_dir, InitScope::Instance)
        .published_at
}

fn run_catalog_initialization_with_scope(
    ac_dir: &Path,
    legacy_catalog_dir: Option<&Path>,
    journal_dir: Option<&Path>,
    scope: InitScope,
) -> CatalogInitOutcome {
    // #2715 5.4: the legacy instance catalog first, before any project lock
    // exists for this pass, so the two catalog locks never nest. A failure
    // seeds nothing: a project must not get defaults beside a source that
    // could not be moved.
    if let Some(legacy) = legacy_catalog_dir {
        if let Err(diagnostic) = migrate_legacy_catalog_scope(legacy, journal_dir) {
            return fail_soft_outcome(diagnostic);
        }
    }
    let dir = match ensure_catalog_dir(ac_dir) {
        Ok(dir) => dir,
        Err(reason) => {
            // Recorded and retried: an unreachable directory is never settled.
            let _ = naming_migration::update_journal(journal_dir, |journal| {
                journal.set_status(
                    &catalog_scope_key(&catalog_dir(ac_dir)),
                    naming_migration::ScopeStatus::Unreachable,
                );
            });
            return CatalogInitOutcome {
                published_at: None,
                base_verified_managed: false,
                warnings: vec![catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    ac_dir,
                    reason,
                )],
            };
        }
    };
    let paths = CatalogPaths::new(&dir);
    let _lock = match acquire_catalog_lock(&paths) {
        Ok(lock) => lock,
        Err(reason) => {
            return CatalogInitOutcome {
                published_at: None,
                base_verified_managed: false,
                warnings: vec![catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    &paths.lock,
                    reason,
                )],
            }
        }
    };
    // #2715 5.1: the family rename under the held catalog lock, immediately
    // before the base can be created. `lock_scope_under_held` adds only the old
    // sidecar: re-opening the new one would block on this pass's own handle.
    let migrated = naming_migration::lock_scope_under_held(
        &dir,
        Some(RETIRED_CATALOG_LOCK_FILENAME),
        naming_migration::MIGRATION_LOCK_BUDGET,
    )
    .map_err(|refusal| {
        catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            &dir.join(RETIRED_CATALOG_LOCK_FILENAME),
            format!(
                "the naming migration could not lock the retired catalog sidecar: {}",
                naming_refusal_reason(refusal)
            ),
        )
    })
    .and_then(|held| migrate_catalog_family(&dir, legacy_catalog_dir, journal_dir, scope, &held));
    let migration_published_at = match migrated {
        Ok(published_at) => published_at,
        Err(diagnostic) => return fail_soft_outcome(diagnostic),
    };
    let mut outcome = initialize_catalog_under_lock(&paths, legacy_catalog_dir, scope);
    outcome.published_at = outcome.published_at.or(migration_published_at);
    for warning in &outcome.warnings {
        log::warn!(
            "[coding-agents] {} at {}: {}",
            warning.code,
            warning.path,
            warning.reason
        );
    }
    outcome
}

// ---------------------------------------------------------------------------
// #2715 (phase B2 of #2703) - the `agents.*` family rename, per catalog
// directory, inside the one function every seeding entry point reaches.
// ---------------------------------------------------------------------------

/// The pre-migration names of the two catalog data files, in rename order.
const CATALOG_FAMILY_RENAMES: [naming_migration::Rename; 2] = [
    naming_migration::Rename {
        from: "agents.json",
        to: CATALOG_MANIFEST_FILENAME,
    },
    naming_migration::Rename {
        from: "agents.local.json",
        to: LOCAL_CATALOG_FILENAME,
    },
];

/// The pre-migration catalog lock sidecar. Held beside the new one for the
/// whole scope, so a pre-migration binary is still excluded; never renamed.
const RETIRED_CATALOG_LOCK_FILENAME: &str = ".agents.json.lock";

/// One scope per canonical catalog directory, whether it is reached as a
/// project `ac_dir` or as the legacy instance directory.
fn catalog_scope_key(canonical_dir: &Path) -> String {
    format!("catalog:{}", canonical_dir.display())
}

fn naming_refusal_reason(refusal: naming_migration::Refusal) -> String {
    match refusal {
        naming_migration::Refusal::LockUnavailable => "lock unavailable".to_string(),
        naming_migration::Refusal::Io(message) => message,
    }
}

fn catalog_path_present(path: &Path) -> Result<bool, CatalogDiagnostic> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            path,
            format!("the naming migration could not inspect this path ({error})"),
        )),
    }
}

/// Move the catalog family in `dir` (canonical) to its target names under the
/// caller's held locks. Rule F1: in a project directory an interrupted #1968
/// migration is finished first, over the PRE-migration names, and a blocked
/// one defers the scope and renames nothing. The instance directory never
/// resumes a sidecar (#2021). Returns the recovery's publication instant.
fn migrate_catalog_family(
    dir: &Path,
    legacy_catalog_dir: Option<&Path>,
    journal_dir: Option<&Path>,
    scope: InitScope,
    held: &naming_migration::ScopeLock,
) -> Result<Option<DateTime<Utc>>, CatalogDiagnostic> {
    let key = catalog_scope_key(dir);
    let journal_error = |refusal| {
        catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            dir,
            format!(
                "the naming-migration journal failed: {}",
                naming_refusal_reason(refusal)
            ),
        )
    };
    // `scope_is_settled`, never `is_complete`: a `Complete` the disk
    // contradicts is re-run.
    let journal = naming_migration::read_journal(journal_dir).map_err(journal_error)?;
    if naming_migration::scope_is_settled(journal.as_ref(), &key) {
        return Ok(None);
    }

    let old = CatalogPaths::with_names(
        dir,
        CATALOG_FAMILY_RENAMES[0].from,
        CATALOG_FAMILY_RENAMES[1].from,
    );
    let journal_present = catalog_path_present(&old.journal)?;
    let backup_present = catalog_path_present(&old.backup)?;
    // With neither old data file on disk there is no rename to protect, so F1
    // has nothing to do; `initialize_catalog_under_lock` recovers over the new
    // names as before.
    let has_old_data = catalog_path_present(&old.base)? || catalog_path_present(&old.local)?;
    let mut published_at = None;
    if scope == InitScope::Project && has_old_data && (journal_present || backup_present) {
        let (recovered, sidecar) = if journal_present {
            (recover_interrupted_migration(&old), &old.journal)
        } else {
            (
                resume_backup_only_migration(&old, legacy_catalog_dir),
                &old.backup,
            )
        };
        match recovered {
            Ok(at) => published_at = at,
            Err(reason) => {
                // Blocked: the genuinely in-flight case. Rename nothing; the
                // next pass retries, so restoring the backup heals it.
                let _ = naming_migration::update_journal(journal_dir, |j| {
                    j.note(&key, &format!("deferred: {reason}"));
                    j.set_status(&key, naming_migration::ScopeStatus::Deferred);
                });
                return Err(catalog_diagnostic(
                    REPORT_CODE_MIGRATION_CONFLICT,
                    sidecar,
                    reason,
                ));
            }
        }
    }

    let mut base_moved = false;
    for (index, rename) in CATALOG_FAMILY_RENAMES.iter().enumerate() {
        match naming_migration::rename_step(dir, rename, &key, journal_dir, held) {
            naming_migration::Outcome::Refused(refusal) => {
                return Err(catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    &dir.join(rename.from),
                    format!(
                        "the naming migration could not move {} to {}: {}",
                        rename.from,
                        rename.to,
                        naming_refusal_reason(refusal)
                    ),
                ))
            }
            naming_migration::Outcome::Renamed | naming_migration::Outcome::SetAside(_) => {
                base_moved |= index == 0;
            }
            naming_migration::Outcome::AlreadyDone | naming_migration::Outcome::SourceAbsent => {}
        }
    }

    // A lock sidecar and the two immutable #1968 sidecars are never renamed
    // or deleted, only noted.
    let mut notes = Vec::new();
    for (name, why) in [
        (
            RETIRED_CATALOG_LOCK_FILENAME,
            "a lock sidecar is never renamed or deleted",
        ),
        (
            MIGRATION_JOURNAL_FILENAME,
            "an immutable #1968 migration sidecar is never renamed or deleted",
        ),
        (
            MIGRATION_BACKUP_FILENAME,
            "an immutable #1968 migration sidecar is never renamed or deleted",
        ),
    ] {
        if matches!(catalog_path_present(&dir.join(name)), Ok(true)) {
            notes.push(format!("left on disk: {name} ({why})"));
        }
    }
    naming_migration::update_journal(journal_dir, |j| {
        for note in &notes {
            j.note(&key, note);
        }
        j.set_status(&key, naming_migration::ScopeStatus::Complete);
    })
    .map_err(journal_error)?;

    if base_moved && scope == InitScope::Project {
        log::warn!(
            "[coding-agents] renamed {} to {} in {}; Git shows the tracked {} as deleted and {} as \
                new (a delete plus an add), and AC runs no Git",
            CATALOG_FAMILY_RENAMES[0].from,
            CATALOG_FAMILY_RENAMES[0].to,
            dir.display(),
            CATALOG_FAMILY_RENAMES[0].from,
            CATALOG_FAMILY_RENAMES[0].to,
        );
    }
    Ok(published_at)
}

/// #2715 5.4 - the legacy instance catalog is its own scope, migrated before
/// any project lock is taken, so a new project imports the real instance
/// catalog. It holds no caller lock, so it takes both sidecars itself. A
/// directory that does not exist holds nothing to rename and is not created.
fn migrate_legacy_catalog_scope(
    legacy_catalog_dir: &Path,
    journal_dir: Option<&Path>,
) -> Result<(), CatalogDiagnostic> {
    let conflict = |reason: String| {
        catalog_diagnostic(
            REPORT_CODE_MIGRATION_CONFLICT,
            legacy_catalog_dir,
            format!(
                "the instance catalog {} could not be moved to {}: {reason}",
                legacy_catalog_dir
                    .join(CATALOG_FAMILY_RENAMES[0].from)
                    .display(),
                legacy_catalog_dir
                    .join(CATALOG_FAMILY_RENAMES[0].to)
                    .display()
            ),
        )
    };
    let dir = match std::fs::canonicalize(legacy_catalog_dir) {
        Ok(dir) => dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(conflict(error.to_string())),
    };
    let key = catalog_scope_key(&dir);
    // Settled already: skip without contending for the instance lock.
    let journal = naming_migration::read_journal(journal_dir)
        .map_err(|refusal| conflict(naming_refusal_reason(refusal)))?;
    if naming_migration::scope_is_settled(journal.as_ref(), &key) {
        return Ok(());
    }
    let held = naming_migration::lock_scope(
        &dir,
        CATALOG_LOCK_FILENAME,
        Some(RETIRED_CATALOG_LOCK_FILENAME),
        naming_migration::MIGRATION_LOCK_BUDGET,
    )
    .map_err(|r| conflict(naming_refusal_reason(r)))?;
    migrate_catalog_family(&dir, None, journal_dir, InitScope::Instance, &held)
        .map(|_| ())
        .map_err(|diagnostic| conflict(diagnostic.reason))
}

/// A fail-soft outcome carrying one diagnostic: nothing published, nothing
/// verified, and `initialize_catalog_under_lock` not reached.
fn fail_soft_outcome(diagnostic: CatalogDiagnostic) -> CatalogInitOutcome {
    log::warn!(
        "[coding-agents] {} at {}: {}",
        diagnostic.code,
        diagnostic.path,
        diagnostic.reason
    );
    CatalogInitOutcome {
        published_at: None,
        base_verified_managed: false,
        warnings: vec![diagnostic],
    }
}

/// Initialize the catalog for `ac_dir` under the catalog lock: recover or
/// refresh a managed base, migrate a legacy catalog, or fresh-seed the managed
/// defaults plus the create-once local stub. Returns the `Utc::now()`
/// publication time sampled at the commit point when the managed base was
/// actually written or replaced; `None` means no base publication. Fail-soft:
/// every failure is logged and surfaced as a warning, never a panic.
pub fn ensure_seeded(ac_dir: &Path, legacy_catalog_dir: Option<&Path>) -> Option<DateTime<Utc>> {
    run_catalog_initialization(ac_dir, legacy_catalog_dir, None).published_at
}

// ---------------------------------------------------------------------------
// #769 Phase 2 - dest-keyed config-folder masters + the re-seed button.
//
// A "master" is the shipped default config folder for a built-in, stored at
// `<config_dir>/coding-agents/_seed/<dest>/` (e.g. `_seed/.claude/`). It is
// seeded once at boot (create-if-absent, from the compiled-in embedded default),
// user-editable thereafter, and used two ways:
//   - the absent-only #598 `CatalogDefault` tier fills a NEW replica's `<dest>`
//     from it (never overwriting existing config), and
//   - the Settings re-seed button restores it to the embedded default (`.bak`
//     first, then a trash-first atomic swap).
// Masters are stored VERBATIM (raw `%AC_...%` tokens); substitution happens only
// when the master is copied into a replica. Only Claude/Codex/OpenCode ship a
// master (D5): agents with no verified config-folder convention get none.
// ---------------------------------------------------------------------------

/// Subdirectory of `coding-agents/` holding the dest-keyed masters.
const SEED_MASTERS_DIR_NAME: &str = "_seed";

/// One embedded default file within a master: a forward-slash relative path plus
/// its raw bytes (compiled in via `include_bytes!`).
struct EmbeddedMasterFile {
    rel_path: &'static str,
    bytes: &'static [u8],
}

/// A dest-keyed embedded master: the shipped default config folder for a built-in.
struct EmbeddedSeedMaster {
    /// The catalog `key` this master belongs to (the #1912 support identity).
    key: &'static str,
    /// The command executable basename (lowercase) this master belongs to. Used
    /// for the re-seed button's exact-basename gating.
    command_basename: &'static str,
    /// The dest folder NAME (e.g. `.claude`): both the `_seed/<dest>/` master dir
    /// and the replica fill destination.
    dest: &'static str,
    files: &'static [EmbeddedMasterFile],
}

/// The built-in default config-folder masters. Only agents with a real, minimal,
/// non-intrusive default ship one; Hermes/Pi/Cursor CLI ship none (no verified
/// convention) and therefore get no re-seed button. Content is provisional
/// (Maria approves before land); swapping it is a resource-file-only change.
const EMBEDDED_SEED_MASTERS: &[EmbeddedSeedMaster] = &[
    EmbeddedSeedMaster {
        key: "claude",
        command_basename: "claude",
        dest: ".claude",
        files: &[EmbeddedMasterFile {
            rel_path: "settings.json",
            bytes: include_bytes!("../../resources/coding-agents/_seed/.claude/settings.json"),
        }],
    },
    EmbeddedSeedMaster {
        key: "codex",
        command_basename: "codex",
        dest: ".codex",
        files: &[EmbeddedMasterFile {
            rel_path: "config.toml",
            bytes: include_bytes!("../../resources/coding-agents/_seed/.codex/config.toml"),
        }],
    },
    EmbeddedSeedMaster {
        key: "opencode",
        command_basename: "opencode",
        dest: ".opencode",
        files: &[EmbeddedMasterFile {
            rel_path: "opencode.json",
            bytes: include_bytes!("../../resources/coding-agents/_seed/.opencode/opencode.json"),
        }],
    },
];

/// #1912 - the masters whose key is enabled in the table in force.
fn supported_embedded_masters() -> impl Iterator<Item = &'static EmbeddedSeedMaster> {
    let table = active_builtin_agent_support();
    EMBEDDED_SEED_MASTERS
        .iter()
        .filter(move |m| is_supported_builtin(m.key, table))
}

/// Result of a re-seed, returned to the frontend for the success toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReseedResult {
    pub dest: String,
    pub backup_path: String,
}

/// Serialize all re-seeds (rare, user-initiated) so two never race on a master
/// mid-swap. Stricter than strictly per-dest, which is safe (never a partial or
/// racy state).
static RESEED_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Absolute path to the dest-keyed master: `<ac_dir>/coding-agents/_seed/<dest>/`.
pub fn master_dir_for_dest(ac_dir: &Path, dest: &str) -> PathBuf {
    catalog_dir(ac_dir).join(SEED_MASTERS_DIR_NAME).join(dest)
}

fn embedded_master_for_command_basename(basename: &str) -> Option<&'static EmbeddedSeedMaster> {
    supported_embedded_masters().find(|m| m.command_basename == basename)
}

/// Lowercased command executable basenames that ship a non-empty embedded default
/// config-folder master AND whose key is enabled in `BUILTIN_AGENT_SUPPORT` (the
/// table in force). The frontend enables the re-seed button only for a catalog
/// def whose command reduces to one of these; the reseed command
/// re-checks server-side. Derived from the supported (enabled) masters, so it
/// stays in sync.
pub fn reseedable_command_basenames() -> Vec<String> {
    supported_embedded_masters()
        .map(|m| m.command_basename.to_string())
        .collect()
}

/// Reduce a coding-agent command to its executable basename (lowercase), mirroring
/// the settings save path. `None` if the command does not tokenize.
///
/// #1171 promoted this from private to `pub(crate)`. It is now the ONLY stem rule in the
/// tree and a second one must not be written, in Rust or in TypeScript: the `starts_with`
/// rule in the frontend's `suggestedContextRegex` must not be ported here or reused, for the
/// reason `reseed_master_for_command` states below - `pi` and `agent` false-match under a
/// prefix rule. The watcher Settings UI gets its reach from `preview_watcher_reach` rather
/// than reimplementing this.
pub(crate) fn command_executable_basename(command: &str) -> Option<String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command).ok()?;
    Some(crate::config::settings::command_token_basename(
        &normalized.shell,
    ))
}

/// #2566 - the ACCOUNT identity of a configured agent command: its program token,
/// verbatim. Same key means same account and one shared weekly-quota reading. NOT
/// `command_executable_basename` above, whose basename step folds `/a/claude` onto
/// `claude`, and NOT resolved through PATH or the filesystem: see #2566 D1/D2.
pub(crate) fn command_account_key(command: &str) -> Option<String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command).ok()?;
    Some(normalized.shell)
}

/// Write a master's embedded files verbatim into `dir` (creating it and any
/// parents). Rejects a rel_path with empty/`.`/`..` segments.
fn write_embedded_files_into(dir: &Path, master: &EmbeddedSeedMaster) -> Result<(), String> {
    for f in master.files {
        let mut path = dir.to_path_buf();
        for seg in f.rel_path.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(format!("invalid embedded master rel_path '{}'", f.rel_path));
            }
            path.push(seg);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, f.bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Seed the dest-keyed masters ONCE at boot: for each SUPPORTED built-in
/// master (its key is enabled in `BUILTIN_AGENT_SUPPORT`), if
/// `_seed/<dest>/` is absent, stage the embedded default into a sibling temp dir
/// and atomically rename it into place (first-writer-wins across a first-run
/// race). Present masters are user-owned and never touched. Fail-soft: logs and
/// continues; never panics or aborts boot.
///
/// #1318 - `ac_dir` is the project's `.ac` directory; `legacy_catalog_dir` is
/// the legacy `<config_dir>/coding-agents` directory. When the project master is
/// ABSENT and the legacy `<legacy>/_seed/<dest>/` is a real directory, the tree
/// is copied VERBATIM (`copy_tree`, substitution OFF, symlinks skipped); an
/// EMPTY legacy master dir is copied verbatim too (present-but-empty master:
/// the spawn tier stays inert and the embedded default is NOT seeded, per the
/// present = user-owned rule; the Settings re-seed button restores it). On a
/// legacy-copy ERROR the partial destination is removed and the embedded master
/// is staged instead (a partial copy must never win). No size cap: a large
/// legacy master is copied into every registered project (the verbatim promise
/// wins). #1912: a de-supported master (a `false` row) is skipped ENTIRELY from
/// every source - embedded staging and legacy `_seed/<dest>` tree copy alike.
pub fn ensure_seeded_masters(ac_dir: &Path, legacy_catalog_dir: Option<&Path>) {
    if all_masters_present(ac_dir) {
        return;
    }
    for master in supported_embedded_masters() {
        let dir = master_dir_for_dest(ac_dir, master.dest);
        match std::fs::symlink_metadata(&dir) {
            Ok(_) => continue, // present (any form) -> user-owned, leave alone
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                log::warn!(
                    "[coding-agents] cannot stat master {} ({e}); skipping seed",
                    dir.display()
                );
                continue;
            }
        }

        // #1318 migration: absent project master + legacy REAL-directory master
        // -> verbatim tree copy.
        let mut copied_from_legacy = false;
        if let Some(legacy) = legacy_catalog_dir {
            let legacy_master = legacy.join(SEED_MASTERS_DIR_NAME).join(master.dest);
            match std::fs::symlink_metadata(&legacy_master) {
                Ok(meta) if meta.is_dir() => {
                    match crate::config::config_seed::copy_tree(
                        &legacy_master,
                        &dir,
                        0,
                        None,
                        false,
                    ) {
                        Ok(()) => {
                            copied_from_legacy = true;
                            let bytes = tree_byte_count(&dir);
                            log::info!(
                                "[coding-agents] migrated legacy master tree {} -> {} ({} bytes \
                                    verbatim, no size cap)",
                                legacy_master.display(),
                                dir.display(),
                                bytes
                            );
                        }
                        Err(e) => {
                            // A partial copy must never win over the embedded
                            // default; the legacy tree itself is never touched.
                            let _ = std::fs::remove_dir_all(&dir);
                            log::warn!(
                                "[coding-agents] failed to copy legacy master {} ({e}); falling \
                                    back to the embedded default",
                                legacy_master.display()
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        if copied_from_legacy {
            continue;
        }

        let sfx = unique_suffix();
        let staging = staging_sibling(&dir, "seedtmp", &sfx);
        let _ = std::fs::remove_dir_all(&staging);
        let result = write_embedded_files_into(&staging, master)
            .and_then(|()| rename_into_place(&staging, &dir));
        match result {
            Ok(()) => log::info!(
                "[coding-agents] seeded default config master at {}",
                dir.display()
            ),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                log::warn!(
                    "[coding-agents] failed to seed master {} ({e})",
                    dir.display()
                );
            }
        }
    }
}

/// #1912 - existence-only steady-state check for the MASTER dirs only. A
/// de-supported master is never required. The catalog itself deliberately has
/// no existence shortcut: every startup/registration must validate ownership
/// and revision and recover or refresh, so only master staging keeps this
/// optimization.
fn all_masters_present(ac_dir: &Path) -> bool {
    supported_embedded_masters()
        .all(|m| std::fs::symlink_metadata(master_dir_for_dest(ac_dir, m.dest)).is_ok())
}

/// Seed the catalog + masters for one registered project root, then record the
/// catalog publication in that project's seed manifest.
///
/// Preconditions (enforced before ANY filesystem effect, in both entry points):
/// a non-absolute root (a hand-edited relative settings entry must never seed
/// relative to the process CWD) or a missing root (a deleted/stale registered
/// root must never be resurrected by the seed's `create_dir_all`) is logged and
/// skipped. The CATALOG has no existence shortcut: every startup/registration
/// takes the catalog lock (and, in production, the soft project gate) to
/// validate ownership/revision and to recover, refresh or migrate. Only the
/// MASTER staging keeps its existence optimization.
pub(crate) fn ensure_seeded_for_project(project_root: &Path) {
    #[cfg(not(test))]
    let activation = Some(ManifestActivationToken::production());
    #[cfg(test)]
    let activation: Option<ManifestActivationToken> = None;
    ensure_seeded_for_project_with_token(project_root, activation.as_ref());
}

/// Token-injectable twin of [`ensure_seeded_for_project`], mirroring
/// `perform_config_seed_recorded` (`config_seed.rs`): a `None` activation runs
/// the plain ungated initialization under the catalog lock; under the soft
/// project gate the Held arm runs the catalog initialization and the masters,
/// then records the catalog row when a base was published - or records the
/// VERIFIED CURRENT base when the manifest has no catalog row yet (the retry
/// that repairs a failed recording without republishing anything).
/// `DegradedUntracked` still takes the catalog lock but records nothing;
/// `Unavailable` logs and skips every catalog mutation (never race a
/// cooperating writer).
pub(crate) fn ensure_seeded_for_project_with_token(
    project_root: &Path,
    activation: Option<&ManifestActivationToken>,
) {
    // #2715: the one place this seeding chain reads `config_dir()`; it names
    // both the legacy instance catalog and the naming-migration journal.
    let config_dir = crate::config::config_dir();
    let legacy = config_dir.as_ref().map(|dir| dir.join(CATALOG_DIR_NAME));
    ensure_seeded_for_project_in(
        project_root,
        activation,
        legacy.as_deref(),
        config_dir.as_deref(),
    );
}

/// [`ensure_seeded_for_project_with_token`] with the legacy catalog and the
/// naming-migration journal directory injected (#2715).
fn ensure_seeded_for_project_in(
    project_root: &Path,
    activation: Option<&ManifestActivationToken>,
    legacy: Option<&Path>,
    journal_dir: Option<&Path>,
) {
    if !project_root.is_absolute() {
        log::warn!(
            "[coding-agents] skipping seed for non-absolute registered project root {}",
            project_root.display()
        );
        return;
    }
    if !project_root.is_dir() {
        log::warn!(
            "[coding-agents] skipping seed for missing registered project root {}",
            project_root.display()
        );
        return;
    }
    let ac_dir = project_root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);

    let Some(token) = activation else {
        run_catalog_initialization(&ac_dir, legacy, journal_dir);
        ensure_seeded_masters(&ac_dir, legacy);
        return;
    };

    match acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(mut guard) => {
            let outcome = run_catalog_initialization(&ac_dir, legacy, journal_dir);
            ensure_seeded_masters(&ac_dir, legacy);
            let recorded_at = match outcome.published_at {
                Some(published_at) => Some(published_at),
                None if outcome.base_verified_managed => {
                    match has_catalog_publication(project_root) {
                        Ok(true) => None,
                        Ok(false) => Some(Utc::now()),
                        Err(error) => {
                            log::debug!(
                                "[coding-agents] seed-manifest bookkeeping query failed for {} \
                                    ({error}); recording is retried on the next initialization",
                                project_root.display()
                            );
                            Some(Utc::now())
                        }
                    }
                }
                None => None,
            };
            if let Some(recorded_at) = recorded_at {
                record_catalog_publication(&mut guard, token, recorded_at);
            }
            guard.release();
        }
        SoftProjectGate::DegradedUntracked => {
            run_catalog_initialization(&ac_dir, legacy, journal_dir);
            ensure_seeded_masters(&ac_dir, legacy);
        }
        SoftProjectGate::Unavailable(error) => {
            log::warn!(
                "[coding-agents] project gate unavailable for {}: {}; skipping seed to avoid \
                    racing a cooperating writer",
                project_root.display(),
                error
            );
        }
    }
}

/// Record a catalog publication into the project seed manifest under an
/// already-held gate, mirroring `session_context::record_project_context_publication`
/// step for step. `published_at` is the `Utc::now()` sampled inside
/// `ensure_seeded` at the commit point of the atomic write; the recorder never
/// re-samples a later clock. The row records the WRITE, not content validity.
/// Fail-soft (log-only) on every error path; never blocks or retracts the seed.
pub(crate) fn record_catalog_publication(
    guard: &mut ProjectSeedManifestGuard,
    activation: &ManifestActivationToken,
    published_at: DateTime<Utc>,
) {
    let identity = match ManifestPathIdentity::from_relative_path(Path::new(
        ".ac/coding-agents/agents.json",
    )) {
        Ok(identity) => identity,
        Err(error) => {
            log::warn!(
                "[coding-agents] seed-manifest catalog row rejected path error={}",
                error
            );
            return;
        }
    };
    let row = match PublishedManifestRow::coding_agent_catalog(identity, published_at) {
        Ok(row) => row,
        Err(error) => {
            log::warn!(
                "[coding-agents] seed-manifest catalog row rejected error={}",
                error
            );
            return;
        }
    };
    let outcome = guard.publication_permit().record_file(activation, row);
    log::debug!(
        "[coding-agents] seed-manifest catalog publication outcome={:?}",
        outcome
    );
}

fn tree_byte_count(dir: &Path) -> u64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(&entry.path(), total);
            } else if meta.is_file() {
                *total = total.saturating_add(meta.len());
            }
        }
    }
    let mut total = 0_u64;
    walk(dir, &mut total);
    total
}

/// Re-seed the master for `command` back to AC's embedded default (the Settings
/// button). Gating is re-checked server-side: `command`'s executable basename must
/// EXACTLY equal a built-in that ships a master (never `starts_with`, so `pi` and
/// `agent` cannot false-match). On success: a timestamped `.bak` of the current
/// master is made FIRST, then a trash-first atomic swap installs the embedded
/// default. On failure the prior master is restored; never a partial state.
pub fn reseed_master_for_command(ac_dir: &Path, command: &str) -> Result<ReseedResult, String> {
    let basename = command_executable_basename(command)
        .ok_or_else(|| format!("'{command}' is not a valid coding-agent command"))?;
    let master = embedded_master_for_command_basename(&basename).ok_or_else(|| {
        format!("'{command}' is not a recognized built-in with a shipped default config folder")
    })?;
    let dir = master_dir_for_dest(ac_dir, master.dest);

    let _guard = RESEED_LOCK
        .lock()
        .map_err(|_| "re-seed lock poisoned".to_string())?;

    // 1. `.bak` the current master (verbatim), BEFORE any swap.
    let backup_path = if dir.exists() {
        Some(backup_master_dir(&dir)?)
    } else {
        None
    };

    // 2. Trash-first atomic swap of the embedded default into the master.
    let sfx = unique_suffix();
    let staging = staging_sibling(&dir, "reseedtmp", &sfx);
    let trash = staging_sibling(&dir, "reseedold", &sfx);
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&trash);

    write_embedded_files_into(&staging, master)?;

    if dir.exists() {
        if let Err(e) = std::fs::rename(&dir, &trash) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "master {} is in use ({e}); re-seed aborted, master unchanged",
                dir.display()
            ));
        }
    }
    if let Err(e) = std::fs::rename(&staging, &dir) {
        // Restore the prior master so we never leave a hole.
        if !dir.exists() && trash.exists() {
            let _ = std::fs::rename(&trash, &dir);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "failed to install re-seeded master {} ({e}); prior config restored",
            dir.display()
        ));
    }
    let _ = std::fs::remove_dir_all(&trash);

    Ok(ReseedResult {
        dest: master.dest.to_string(),
        backup_path: backup_path
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
    })
}

/// Verbatim, timestamped `.bak` copy of a master dir (unique-suffix loop, mirrors
/// `seeded_context_templates::create_backup`). Copies with substitution OFF so the
/// raw `%AC_...%` tokens in the master are preserved in the backup.
fn backup_master_dir(dir: &Path) -> Result<PathBuf, String> {
    let parent = dir
        .parent()
        .ok_or_else(|| format!("master {} has no parent", dir.display()))?;
    let name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("master {} has no name", dir.display()))?;
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%SZ").to_string();
    for index in 0..1000u32 {
        let bak_name = match index {
            0 => format!("{name}.bak-{ts}"),
            n => format!("{name}.bak-{ts}.{n}"),
        };
        let bak = parent.join(&bak_name);
        if bak.exists() {
            continue;
        }
        crate::config::config_seed::copy_tree(dir, &bak, 0, None, false)
            .map_err(|e| format!("backup {} -> {}: {e}", dir.display(), bak.display()))?;
        return Ok(bak);
    }
    Err(format!(
        "could not find a unique .bak path for {}",
        dir.display()
    ))
}

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SEED_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn staging_sibling(dir: &Path, tag: &str, sfx: &str) -> PathBuf {
    let parent = dir.parent().unwrap_or_else(|| Path::new("."));
    let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("_seed");
    parent.join(format!("{name}.{tag}-{sfx}"))
}

/// Publish a staged dir into `dest` (dest expected absent). Cleans staging on
/// failure. `std::fs::rename` is first-writer-wins if `dest` appeared concurrently.
fn rename_into_place(staging: &Path, dest: &Path) -> Result<(), String> {
    std::fs::rename(staging, dest).map_err(|e| format!("install {} ({e})", dest.display()))
}

// E2432: narrow physical IO, no catalog initialization, migration or refresh.
pub(crate) fn replace_selected_personal_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let paths = CatalogPaths::new(path.parent().ok_or("sourceContextUnavailable")?);
    if path != paths.local {
        return Err("sourceReadOnly".into());
    }
    publish_replace(path, bytes)
}

pub(crate) fn publish_selected_personal(
    proof: &mut crate::config::settings::CatalogSourceOperationGuard<'_, '_>,
    edits: &crate::config::settings::AgentFileEdits,
) -> crate::config::settings::AgentFileSaveReply {
    use crate::config::settings::{AgentFileSaveOutcome, OperationEffect};
    let path = proof.target().to_path_buf();
    let paths = CatalogPaths::new(path.parent().expect("authorized catalog parent"));
    let lock = (|| {
        if path != paths.local {
            return Err("sourceReadOnly".into());
        }
        proof.verify_target_parent_chain()?;
        // Locking never initializes a catalog or creates its parent directory.
        acquire_catalog_lock(&paths)
    })();
    let _lock = match lock {
        Ok(lock) => lock,
        Err(reason) => return proof.finish(AgentFileSaveOutcome::Rejected, Some(&reason)),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let bytes = proof.prepare(edits)?;
        proof.recheck()?;
        proof.publish(&bytes)
    }));
    let reason = match result {
        Ok(Ok(())) => None,
        Ok(Err(reason)) => Some(reason),
        Err(_) => Some("catalogPublicationPanicked".into()),
    };
    let outcome = if reason.is_none() {
        AgentFileSaveOutcome::Committed
    } else if proof.effect() == OperationEffect::Possible {
        AgentFileSaveOutcome::PartialOrUnknown
    } else {
        AgentFileSaveOutcome::Rejected
    };
    // Both panic containment and recovery remain INSIDE the catalog lock scope.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        proof.finish(outcome, reason.as_deref())
    }))
    .unwrap_or_else(|_| proof.interrupted())
}

#[cfg(test)]
pub(crate) fn hold_selected_catalog_test_lock(path: &Path) -> impl std::fmt::Debug {
    acquire_catalog_lock(&CatalogPaths::new(path.parent().expect("catalog parent"))).unwrap()
}

#[cfg(test)]
mod tests;
