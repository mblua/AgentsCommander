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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentDefinition {
    /// Stable identity within the catalog. Must match `^[a-z0-9-]+$` and be
    /// unique; it doubles as a testid/JSON-key/CSS token on the frontend and (in
    /// the Full phase) a directory name.
    pub key: String,
    pub label: String,
    pub description: String,
    pub color: String,
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
    let catalog: CodingAgentCatalogSchema2 = serde_json::from_value(value)
        .map_err(|error| IdentityCatalogError::InvalidFormat(error.to_string()))?;
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

#[cfg(test)]
mod identity_schema2_tests {
    use super::*;
    use crate::config::settings::{
        configuration_identity, profile_identity, IdentityValidationCode,
    };
    use serde_json::{json, Value};

    fn definition(key: &str, command: &str) -> Value {
        json!({"key":key, "label":key, "description":"own", "color":"#123456", "command":command})
    }

    fn parse(value: &Value) -> Result<CodingAgentCatalogSchema2, IdentityCatalogError> {
        parse_catalog_schema2(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn identity_schema2_complete_model_and_local_profiles_roundtrip() {
        let mut agent = definition("own", "custom --base");
        agent["envs"] = json!([{"key":"SECRET", "value":"configured", "enabled":false}]);
        agent["instructionsFilename"] = json!("AGENTS.md");
        agent["configSeed"] = json!({"enabled":true,"dest":".custom"});
        agent["installCommands"] =
            json!({"default":"install custom", "windows":"install custom-windows"});
        agent["updateCommands"] = json!(["update custom"]);
        agent["isolatedHome"] = json!(true);
        let catalog = parse(&json!({"schemaVersion":2,"agents":[agent],
            "codingAgentProfiles":{"own":{"Z":{"command":"--model mine","env":{"MODEL":"mine"},"enabled":false,"notes":"own notes"}}},
            "profileLabels":{"own":{"Z":"Own label"}}
        })).unwrap();
        assert_eq!(catalog.agents[0].command, "custom --base");
        assert!(!catalog.agents[0].envs[0].enabled);
        assert_eq!(
            catalog.agents[0]
                .install_commands
                .as_ref()
                .unwrap()
                .windows
                .as_deref(),
            Some("install custom-windows")
        );
        assert_eq!(catalog.profile_labels["own"]["Z"], "Own label");
        assert!(!catalog.coding_agent_profiles["own"]["Z"].enabled);
        assert!(!catalog.coding_agent_profiles["own"].contains_key("A"));
        assert_eq!(
            parse(&serde_json::to_value(&catalog).unwrap()).unwrap(),
            catalog
        );
        assert!(catalog.validate_identities().is_ok());
    }

    #[test]
    fn identity_schema2_defaults_are_own_and_no_profile_is_invented() {
        let catalog =
            parse(&json!({"schemaVersion":2,"agents":[definition("own", "custom")]})).unwrap();
        let agent = &catalog.agents[0];
        assert!(agent.envs.is_empty());
        assert!(agent.removable);
        assert!(!agent.auto_update);
        assert!(agent.config_seed.is_none());
        assert!(catalog.coding_agent_profiles.is_empty());
        assert!(catalog.profile_labels.is_empty());
        let empty =
            parse(&serde_json::to_value(CodingAgentCatalogSchema2::empty()).unwrap()).unwrap();
        assert!(empty.agents.is_empty());
    }

    #[test]
    fn identity_schema2_requires_version_agents_and_explicit_nonempty_command() {
        for value in [
            json!({"schemaVersion":2}),
            json!({"agents":[]}),
            json!({"schemaVersion":3,"agents":[]}),
            json!({"schemaVersion":"2","agents":[]}),
            json!({"schemaVersion":2,"agents":[{"key":"own","label":"Own","description":"own","color":"#fff"}]}),
            json!({"schemaVersion":2,"agents":[definition("own", " \u{2003}")]}),
        ] {
            assert!(matches!(
                parse(&value),
                Err(IdentityCatalogError::InvalidFormat(_))
            ));
        }
    }

    #[test]
    fn identity_schema2_rejects_schema1_without_patch_composition_or_conversion() {
        for value in [
            json!({"schemaVersion":1,"agents":[]}),
            json!({"schemaVersion":1,"agents":[{"key":"claude","label":"patch only"}]}),
        ] {
            assert_eq!(
                parse(&value).unwrap_err(),
                IdentityCatalogError::IndependentFormatRequired
            );
        }
        // Additive schema2 parsing does not alter the legacy schema1 DTO.
        let legacy: CodingAgentCatalog = serde_json::from_value(json!({"agents":[]})).unwrap();
        assert_eq!(legacy.schema_version, CATALOG_SCHEMA_VERSION);
    }

    #[test]
    fn identity_schema2_unique_keys_and_local_map_ownership() {
        for value in [
            json!({"schemaVersion":2,"agents":[definition("own","one"),definition("own","two")]}),
            json!({"schemaVersion":2,"agents":[definition("INVALID_KEY","one")]}),
            json!({"schemaVersion":2,"agents":[definition("own","one")],"codingAgentProfiles":{"foreign":{"A":{}}}}),
            json!({"schemaVersion":2,"agents":[definition("own","one")],"profileLabels":{"foreign":{"A":"label"}}}),
        ] {
            assert!(matches!(
                parse(&value),
                Err(IdentityCatalogError::InvalidFormat(_))
            ));
        }
    }

    #[test]
    fn identity_schema2_letters_are_exact_a_through_z_in_both_maps() {
        for letter in ["", "a", "AA", "Å", "[", " A"] {
            for field in ["codingAgentProfiles", "profileLabels"] {
                let mut value = json!({"schemaVersion":2,"agents":[definition("own","one")]});
                value[field] = json!({"own":{letter: if field == "profileLabels" { json!("Label") } else { json!({}) }}});
                assert!(
                    matches!(parse(&value), Err(IdentityCatalogError::InvalidFormat(_))),
                    "{field}: {letter:?}"
                );
            }
        }
    }

    #[test]
    fn identity_schema2_duplicate_json_members_and_trailing_data_rejected() {
        for bytes in [
            br#"{"schemaVersion":2,"schemaVersion":2,"agents":[]}"#.as_slice(),
            br#"{"schemaVersion":2,"agents":[]} {}"#,
            br#"{"schemaVersion":2,"agents":[],"profileLabels":{"own":{"A":"one","A":"two"}}}"#,
        ] {
            assert!(matches!(
                parse_catalog_schema2(bytes),
                Err(IdentityCatalogError::InvalidFormat(_))
            ));
        }
    }

    #[test]
    fn identity_schema2_historical_duplicates_diagnosed_with_existing_name() {
        let catalog = parse(&json!({"schemaVersion":2,"agents":[definition("existing","tool"),definition("second","loot")]})).unwrap();
        let error = catalog.validate_identities().unwrap_err();
        assert_eq!(
            error.code,
            IdentityValidationCode::DuplicateConfigurationIdentity
        );
        assert_eq!(error.existing_name, "existing");
        assert_eq!(error.entry_hint, "second");
        assert!(
            parse(&json!({"schemaVersion":2,"agents":[definition("second","loot")]}))
                .unwrap()
                .validate_identities()
                .is_ok()
        );
    }

    #[test]
    fn identity_schema2_pid_only_edit_and_decorative_changes() {
        let mut value = json!({"schemaVersion":2,"agents":[definition("own","tool")],"codingAgentProfiles":{"own":{"A":{"command":"--model one"}}}});
        let before = parse(&value).unwrap();
        value["agents"][0]["label"] = json!("Renamed");
        value["agents"][0]["color"] = json!("#fff");
        value["codingAgentProfiles"]["own"]["A"]["command"] = json!("--model two");
        let after = parse(&value).unwrap();
        assert_eq!(
            configuration_identity(&before.agents[0].command, &before.agents[0].envs),
            configuration_identity(&after.agents[0].command, &after.agents[0].envs)
        );
        assert_ne!(
            profile_identity(&before.coding_agent_profiles["own"]["A"]),
            profile_identity(&after.coding_agent_profiles["own"]["A"])
        );
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
fn validate_catalog_key(key: &str) -> Result<(), String> {
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
const KNOWN_ROOT_FIELDS: &[&str] = &["schemaVersion", "agents", "managed"];

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
            reason: "the AgentsCommander config directory could not be resolved, so no persisted catalog location is available".to_string(),
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
                    "the {what} at {} is a symbolic link or reparse point; a regular file is required",
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
fn managed_content_sha256(definitions: &[CodingAgentDefinition]) -> String {
    match serde_json::to_vec(definitions) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(error) => {
            log::error!(
                "[coding-agents] failed to serialize catalog definitions for the managed revision hash ({error})"
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
    let revision = managed_content_sha256(definitions);
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ManagedBaseWire<'a> {
        schema_version: u32,
        agents: &'a [CodingAgentDefinition],
        managed: ManagedCatalogMarker,
    }
    let root = ManagedBaseWire {
        schema_version: CATALOG_SCHEMA_VERSION,
        agents: definitions,
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
fn parse_strict_json(bytes: &[u8]) -> Result<serde_json::Value, String> {
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
}

const LOCAL_ROOT_FIELDS: &[&str] = &["schemaVersion", "agents", "order"];
const LOCAL_ROW_FIELDS: &[&str] = &[
    "key",
    "label",
    "description",
    "color",
    "command",
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
            "{context}: updateCommands item {index} contains a Unicode control character, U+2028 or U+2029"
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
    if let Some(value) = object.get("command") {
        let serde_json::Value::String(value) = value else {
            return Err(format!("{context}: 'command' must be a string"));
        };
        fields.command = Some(value.clone());
    }
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
    Ok(LocalLayer { rows, order })
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
                    working[position] = apply_local_fields(working[position].clone(), &row.fields);
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
    if let Some(version) = root.get("schemaVersion") {
        if version.as_u64() != Some(CATALOG_SCHEMA_VERSION as u64) {
            return Err(catalog_diagnostic(
                REPORT_CODE_BASE_INVALID,
                base_path,
                format!(
                    "unsupported explicit schemaVersion {version}; only schemaVersion 1 is recognized"
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
    let mut refresh_blocked = false;
    if let Some(names) = unknown_field_names(root.keys(), KNOWN_ROOT_FIELDS) {
        warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_PENDING,
            base_path,
            format!(
                "root field(s) ({names}) are not recognized by the current catalog schema and require managed-catalog migration"
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
                        "entry {index} does not match the coding-agent definition schema and was omitted"
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
                    "coding-agent '{}' was omitted: its key duplicates an earlier entry (the first entry wins)",
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
                    "coding-agent '{}' has no persisted updateCommands; absent update commands are suppressed during reads and require managed-catalog migration on a supported restart",
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
                "the managed catalog content does not match its recorded contentSha256; it stays readable but is never auto-refreshed or auto-migrated",
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
        "the catalog source, project overrides, local overrides and migration journal kept changing while reading; retry the read"
    } else {
        "the catalog source, local overrides and migration journal kept changing while reading; retry the read"
    }.to_string())
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
        format!("a {layer} overrides file exists while the base is not AC-managed; ownership transfer is blocked until the managed migration completes and the {layer} layer is not applied on top of a non-managed base"),
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
                "an interrupted managed-catalog migration is pending; it resumes on the next initialization",
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
                Ok(None) => {},
                Ok(Some(_)) => resolved.warnings.push(catalog_diagnostic(REPORT_CODE_MIGRATION_PENDING, &project_catalog_path(ac_dir), "a project overrides file exists but no managed base has been published yet; the managed base is created on the next initialization")),
                Err(reason) => resolved.warnings.push(catalog_diagnostic(REPORT_CODE_PROJECT_INVALID, &project_catalog_path(ac_dir), reason.clone())),
            }
        }
        match &snapshot.local {
            Ok(None) => {}
            Ok(Some(_)) => resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                &local_path,
                "a local overrides file exists but no managed base has been published yet; the managed base is created on the next initialization",
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
                "the persisted catalog carries an unrecognized managed ownership marker; this build neither refreshes nor migrates it",
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
                "its updateCommands item {index} contains a Unicode control character, U+2028 or U+2029"
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
                "coding-agent '{key}' carries unknown field(s) ({names}) that require managed-catalog migration before ownership transfer"
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
                    "coding-agent '{key}' configSeed carries unknown field(s) ({names}) that require managed-catalog migration"
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
                    "coding-agent '{key}' idleBurst carries unknown field(s) ({names}) that require managed-catalog migration"
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
                    "coding-agent '{key}' installCommands carries unknown field(s) ({names}) that require managed-catalog migration"
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
                        "coding-agent '{key}' envs[{index}] carries unknown field(s) ({names}) that require managed-catalog migration"
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
                "The persisted managed catalog revision differs from this build. Current persisted entries remain usable. Restart retries project catalog refresh."
            }
            Self::Instance => {
                "The persisted instance catalog revision differs from this build. Current persisted entries remain usable. Restart retries instance catalog refresh when no project is registered."
            }
            Self::Direct => {
                "The persisted managed catalog revision differs from this build. Current persisted entries remain usable. Catalog refresh runs during initialization."
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
                            "[coding-agents] seed-manifest bookkeeping query failed for {} ({error})",
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
                        "the seed manifest does not record this managed catalog publication yet; reads remain usable from the verified base and local overrides and the next initialization records it",
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
            "the catalog root {} is not absolute; refusing to create it relative to the process CWD",
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
                        "the project .ac directory {} is not a real directory; refusing to create the catalog",
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
            "exclusive publication of {} failed via a hard link ({error}); the destination was left untouched and no partial bytes were written",
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
                    "catalogLockTimeout: timed out after {} ms waiting for the coding-agent catalog lock '{}'",
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
                    "the coding-agent catalog lock {} could not be acquired ({error}); this filesystem does not support the lock and no process-only fallback is used",
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
struct LocalLayerWire {
    schema_version: u32,
    agents: Vec<LocalRowWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<Vec<String>>,
}

fn local_layer_bytes(rows: Vec<LocalRowWire>, order: Option<Vec<String>>) -> Vec<u8> {
    let layer = LocalLayerWire {
        schema_version: CATALOG_SCHEMA_VERSION,
        agents: rows,
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
        command: raw
            .contains_key("command")
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
                    "reconciliation is blocked: shipped key '{}' is absent from the legacy catalog and is not removable, so no tombstone can be recorded",
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
    Ok(local_layer_bytes(local_rows, Some(order)))
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
            "an existing local overrides file blocks the legacy ownership transfer; it was left untouched"
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
        "[coding-agents] migrated the {source_kind} catalog {} into the managed base with an extracted local layer",
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
            "the migration backup is missing or unreadable ({e}); recovery is blocked and every byte is preserved"
        )
    })?;
    if sha256_hex(&backup) != journal.source_sha256 {
        return Err(
            "the migration backup does not match the journal source hash; recovery is blocked and every byte is preserved"
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
            "the journal saved managed base does not match its recorded revision; recovery is blocked"
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
                "the existing catalog base does not match the interrupted migration; recovery is blocked and every byte is preserved"
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
                        "the local overrides file does not match the interrupted migration; recovery is blocked and every byte is preserved"
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
                                "the instance source changed during the interrupted migration; recovery is blocked"
                                    .to_string(),
                            );
                    }
                }
            }
            publish_base(&paths.base, &managed_bytes, journal.source_kind.as_str())?;
            Utc::now()
        }
        None => {
            let recomputed = extract_legacy_local(&backup, &managed_definitions)?;
            if sha256_hex(&recomputed) != journal.local_sha256
                || recomputed.len() as u64 != journal.local_byte_length
            {
                return Err(
                        "the recomputed extraction does not match the journal local hash; recovery is blocked and every byte is preserved"
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
            "a local overrides file exists without a journal; the interrupted migration cannot be resumed safely"
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
    let source_kind = match (&base_bytes, &instance_bytes) {
        (Some(base), _) if *base == backup => MIGRATION_SOURCE_PROJECT,
        (None, Some(instance)) if *instance == backup => MIGRATION_SOURCE_INSTANCE,
        _ => {
            return Err(
                "the backup does not match the current project base or instance catalog; recovery is blocked and every byte is preserved"
                    .to_string(),
            )
        }
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
    let managed_bytes = build_managed_base_bytes(&shipped);
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
                "The managed catalog base is usable, but its local overrides stub could not be created: {detail}. An absent local file is valid; no automatic stub retry is scheduled."
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
            "[coding-agents] renamed {} to {} in {}; Git shows the tracked {} as deleted and {} as new (a delete plus an add), and AC runs no Git",
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
                                "[coding-agents] migrated legacy master tree {} -> {} ({} bytes verbatim, no size cap)",
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
                                "[coding-agents] failed to copy legacy master {} ({e}); falling back to the embedded default",
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
                                "[coding-agents] seed-manifest bookkeeping query failed for {} ({error}); recording is retried on the next initialization",
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
                "[coding-agents] project gate unavailable for {}: {}; skipping seed to avoid racing a cooperating writer",
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

#[cfg(test)]
mod tests {
    use super::*;
    fn write_project_patch(ac_dir: &Path, value: serde_json::Value) {
        let path = project_catalog_path(ac_dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn project_report(ac_dir: &Path) -> CatalogReport {
        load_catalog_report_with_context(
            ac_dir,
            CatalogSourceContext::Project,
            CatalogReadScope::Project,
        )
        .0
    }

    fn patch_rows(rows: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"schemaVersion":1,"agents":rows})
    }

    fn extra_row(key: &str) -> serde_json::Value {
        let mut row = shipped_def_json(&["claude"]).remove(0);
        row["key"] = serde_json::json!(key);
        row["removable"] = serde_json::json!(true);
        row
    }

    #[test]
    fn project_absence_preserves_direct_report_and_catalog() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(
            dir.path(),
            r#"{"schemaVersion":1,"agents":[{"key":"claude","label":"Personal"}]}"#,
        );
        assert_eq!(
            report_json(&project_report(dir.path())),
            report_json(&load_catalog_report(dir.path()))
        );
        assert!(!project_catalog_path(dir.path()).exists());
    }

    #[test]
    fn project_then_personal_merges_fields_nulls_and_exact_order() {
        let dir = seed_dir();
        write_managed_base(
            dir.path(),
            &shipped_def_json(&["claude", "codex", "pi"]),
            "stale",
            true,
        );
        let mut project = patch_rows(serde_json::json!([
            {"key":"claude","label":"Project","instructionsFilename":null,"idleBurst":{"maxBytes":10,"maxSecs":2},"installCommands":{"default":"install","windows":"win"}}, extra_row("team")
        ]));
        project["order"] = serde_json::json!(["team", "pi", "unknown"]);
        write_project_patch(dir.path(), project);
        let mut personal = patch_rows(serde_json::json!([
            {"key":"claude","label":"Personal","idleBurst":{"maxSecs":3},"installCommands":{"windows":null}}, extra_row("mine")
        ]));
        personal["order"] = serde_json::json!(["mine", "claude"]);
        write_local(dir.path(), &personal.to_string());
        let report = project_report(dir.path());
        assert_eq!(
            keys_of(&report.catalog),
            ["mine", "claude", "team", "pi", "codex"]
        );
        let claude = &report.catalog[1];
        assert_eq!(claude.label, "Personal");
        assert_eq!(claude.instructions_filename, None);
        assert_eq!(claude.idle_burst.as_ref().unwrap().max_bytes, Some(10));
        assert_eq!(claude.idle_burst.as_ref().unwrap().max_secs, Some(3.0));
        assert_eq!(claude.install_commands.as_ref().unwrap().default, "install");
        assert_eq!(claude.install_commands.as_ref().unwrap().windows, None);
        assert!(!report
            .warnings
            .iter()
            .any(|w| w.code == "projectInvalid" || w.code == "localInvalid"));
    }

    #[test]
    fn project_invalid_schema_preserves_base_and_independent_personal() {
        let cases = [
            "<<<<<<< conflict",
            "{",
            r#"{"schemaVersion":2,"agents":[]}"#,
            r#"{"schemaVersion":1,"agents":[],"unexpected":true}"#,
            r#"{"schemaVersion":1,"schemaVersion":1,"agents":[]}"#,
            r#"{"schemaVersion":1,"agents":[{"key":"claude"},{"key":"claude"}]}"#,
            r#"{"schemaVersion":1,"agents":[],"order":["claude","claude"]}"#,
            r#"{"schemaVersion":1,"agents":[{"key":"claude","label":"partial"},{"key":"new","label":"incomplete"}]}"#,
        ];
        for bytes in cases {
            let dir = seed_dir();
            ensure_seeded(dir.path(), None);
            std::fs::write(project_catalog_path(dir.path()), bytes).unwrap();
            write_local(
                dir.path(),
                r#"{"schemaVersion":1,"agents":[{"key":"claude","label":"Personal"}]}"#,
            );
            let report = project_report(dir.path());
            assert_eq!(report.catalog[0].label, "Personal", "{bytes}");
            let warning = report
                .warnings
                .iter()
                .find(|w| w.code == "projectInvalid")
                .unwrap();
            assert_eq!(
                warning.path,
                project_catalog_path(dir.path()).display().to_string()
            );
            assert!(!warning.reason.contains("local"), "{}", warning.reason);
            assert_eq!(
                std::fs::read(project_catalog_path(dir.path())).unwrap(),
                bytes.as_bytes()
            );
        }
    }

    #[test]
    fn project_rejection_has_no_donor_and_personal_rejection_keeps_project() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_project_patch(
            dir.path(),
            patch_rows(serde_json::json!([extra_row("team"), {"key":"claude","command":""}])),
        );
        write_local(
            dir.path(),
            &patch_rows(serde_json::json!([{"key":"team","label":"partial"}])).to_string(),
        );
        let report = project_report(dir.path());
        assert!(!report.catalog.iter().any(|d| d.key == "team"));
        assert_eq!(
            report
                .warnings
                .iter()
                .filter(|w| w.code == "projectInvalid" || w.code == "localInvalid")
                .count(),
            2
        );
        write_local(
            dir.path(),
            &patch_rows(serde_json::json!([extra_row("team")])).to_string(),
        );
        assert!(project_report(dir.path())
            .catalog
            .iter()
            .any(|d| d.key == "team"));
        write_project_patch(
            dir.path(),
            patch_rows(serde_json::json!([{"key":"claude","label":"Team"}])),
        );
        write_local(dir.path(), "invalid");
        assert_eq!(project_report(dir.path()).catalog[0].label, "Team");
    }
    #[test]
    fn project_removability_and_resurrection_use_accepted_lower_vector() {
        let dir = seed_dir();
        let mut row = extra_row("team");
        row["removable"] = serde_json::json!(false);
        write_managed_base(dir.path(), &[row], "stale", true);
        write_project_patch(
            dir.path(),
            patch_rows(serde_json::json!([{"key":"team","remove":true}])),
        );
        assert!(project_report(dir.path())
            .warnings
            .iter()
            .any(|w| w.code == "projectInvalid"));
        write_project_patch(
            dir.path(),
            patch_rows(serde_json::json!([{"key":"team","removable":true}])),
        );
        write_local(
            dir.path(),
            &patch_rows(serde_json::json!([{"key":"team","remove":true}])).to_string(),
        );
        assert!(project_report(dir.path()).catalog.is_empty());
        write_managed_base(dir.path(), &[extra_row("team")], "stale", true);
        write_project_patch(
            dir.path(),
            patch_rows(serde_json::json!([{"key":"team","remove":true}])),
        );
        write_local(
            dir.path(),
            &patch_rows(serde_json::json!([{"key":"team","label":"partial"}])).to_string(),
        );
        let report = project_report(dir.path());
        assert!(report.catalog.is_empty());
        assert!(report.warnings.iter().any(|w| w.code == "localInvalid"));
        write_local(
            dir.path(),
            &patch_rows(serde_json::json!([extra_row("team")])).to_string(),
        );
        assert_eq!(keys_of(&project_report(dir.path()).catalog), ["team"]);
    }

    #[test]
    fn project_snapshot_observes_churn_absence_errors_and_excludes_instance() {
        let dir = seed_dir();
        for scope in [CatalogReadScope::Project, CatalogReadScope::Instance] {
            let mut calls = 0;
            let snapshot = read_catalog_snapshot_with(dir.path(), scope, |path, _| {
                if path == project_catalog_path(dir.path()) {
                    calls += 1;
                }
                Ok(None)
            })
            .unwrap();
            assert_eq!(
                calls,
                if scope == CatalogReadScope::Project {
                    2
                } else {
                    0
                }
            );
            assert_eq!(
                snapshot.project.is_some(),
                scope == CatalogReadScope::Project
            );
        }
        for change in 0..3 {
            let mut calls = 0;
            let result =
                read_catalog_snapshot_with(dir.path(), CatalogReadScope::Project, |path, _| {
                    if path == project_catalog_path(dir.path()) {
                        calls += 1;
                        return match change {
                            0 => Ok(Some(vec![calls])),
                            1 => {
                                if calls % 2 == 0 {
                                    Ok(None)
                                } else {
                                    Ok(Some(vec![]))
                                }
                            }
                            _ => Err(format!("read error {calls}")),
                        };
                    }
                    Ok(None)
                });
            assert_eq!(calls, 3);
            assert!(result.err().unwrap().contains("project overrides"));
        }
    }

    #[test]
    fn project_settings_primary_only_and_direct_instance_matrix() {
        let primary = tempfile::tempdir().unwrap();
        let secondary = tempfile::tempdir().unwrap();
        let instance = tempfile::tempdir().unwrap();
        for root in [
            primary.path().join(".ac"),
            secondary.path().join(".ac"),
            instance.path().to_path_buf(),
        ] {
            ensure_seeded(&root, None);
            write_project_patch(
                &root,
                patch_rows(serde_json::json!([{"key":"claude","label":"Project"}])),
            );
            write_local(
                &root,
                r#"{"schemaVersion":1,"agents":[{"key":"codex","label":"Personal"}]}"#,
            );
            let direct = load_catalog_report(&root);
            assert_eq!(direct.catalog[0].label, "Claude Code");
            assert_eq!(direct.catalog[1].label, "Personal");
            assert_eq!(
                keys_of(&load_catalog(&root).unwrap()),
                keys_of(&direct.catalog)
            );
        }
        let mut settings = AppSettings {
            project_paths: vec![
                " ".into(),
                primary.path().display().to_string(),
                secondary.path().display().to_string(),
            ],
            project_path: Some(secondary.path().display().to_string()),
            ..Default::default()
        };
        let report = load_catalog_report_for_settings_with_config_dir(
            &settings,
            Some(instance.path().to_path_buf()),
        );
        assert_eq!(report.catalog[0].label, "Project");
        assert_eq!(
            report.primary_project_root,
            Some(primary.path().display().to_string())
        );
        std::fs::write(manifest_path(&primary.path().join(".ac")), "bad").unwrap();
        assert!(load_catalog_report_for_settings_with_config_dir(
            &settings,
            Some(instance.path().to_path_buf())
        )
        .unavailable
        .is_some());
        settings.project_paths.clear();
        settings.project_path = None;
        assert_eq!(
            load_catalog_report_for_settings_with_config_dir(
                &settings,
                Some(instance.path().to_path_buf())
            )
            .catalog[0]
                .label,
            "Claude Code"
        );
    }

    #[test]
    fn project_nonregular_and_read_errors_degrade_only_project() {
        fn inventory(
            root: &std::path::Path,
        ) -> Vec<(
            std::path::PathBuf,
            bool,
            u64,
            bool,
            std::time::SystemTime,
            Vec<u8>,
        )> {
            #[cfg(test)]
            type TreeInventoryRows = Vec<(
                std::path::PathBuf,
                bool,
                u64,
                bool,
                std::time::SystemTime,
                Vec<u8>,
            )>;
            fn visit(root: &std::path::Path, path: &std::path::Path, rows: &mut TreeInventoryRows) {
                let metadata = std::fs::symlink_metadata(path).unwrap();
                assert!(!metadata.file_type().is_symlink());
                rows.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    metadata.is_dir(),
                    metadata.len(),
                    metadata.permissions().readonly(),
                    metadata.modified().unwrap(),
                    if metadata.is_file() {
                        std::fs::read(path).unwrap()
                    } else {
                        Vec::new()
                    },
                ));
                if metadata.is_dir() {
                    let mut entries: Vec<_> = std::fs::read_dir(path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path())
                        .collect();
                    entries.sort();
                    for entry in entries {
                        visit(root, &entry, rows);
                    }
                }
            }
            let mut rows = Vec::new();
            visit(root, root, &mut rows);
            rows
        }
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let baseline = project_report(dir.path());
        std::fs::create_dir(project_catalog_path(dir.path())).unwrap();
        std::fs::write(project_catalog_path(dir.path()).join("keep.txt"), b"keep").unwrap();
        let before = inventory(dir.path());
        let report = project_report(dir.path());
        assert_eq!(report.catalog.len(), 8);
        assert!(report.warnings.iter().any(|w| w.code == "projectInvalid"));
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog, baseline.catalog);
        assert_eq!(keys_of(&report.catalog), keys_of(&baseline.catalog));
        assert!(report.warnings.iter().any(|w| w.code == "projectInvalid"
            && w.path == project_catalog_path(dir.path()).display().to_string()
            && !w.reason.is_empty()));
        for _ in 0..3 {
            assert_eq!(project_report(dir.path()), report);
        }
        assert_eq!(inventory(dir.path()), before);
        let snapshot = read_catalog_snapshot_with(
            dir.path(),
            CatalogReadScope::Project,
            |path, description| {
                if path == project_catalog_path(dir.path()) {
                    Err("project read denied".into())
                } else {
                    read_optional_regular_file(path, description)
                }
            },
        )
        .unwrap();
        let resolved =
            resolve_catalog_snapshot(dir.path(), snapshot, CatalogSourceContext::Project);
        assert_eq!(resolved.catalog.len(), 8);
        assert!(resolved
            .warnings
            .iter()
            .any(|w| w.code == "projectInvalid" && w.reason == "project read denied"));
        assert!(resolved.unavailable.is_none());
        assert_eq!(resolved.catalog, baseline.catalog);
        assert!(resolved.warnings.iter().any(|w| w.code == "projectInvalid"
            && w.path == project_catalog_path(dir.path()).display().to_string()
            && w.reason == "project read denied"));
        for _ in 0..3 {
            let snapshot = read_catalog_snapshot_with(
                dir.path(),
                CatalogReadScope::Project,
                |path, description| {
                    if path == project_catalog_path(dir.path()) {
                        Err("project read denied".into())
                    } else {
                        read_optional_regular_file(path, description)
                    }
                },
            )
            .unwrap();
            let repeated =
                resolve_catalog_snapshot(dir.path(), snapshot, CatalogSourceContext::Project);
            assert_eq!(repeated.catalog, resolved.catalog);
            assert_eq!(repeated.warnings, resolved.warnings);
            assert_eq!(repeated.unavailable, resolved.unavailable);
            assert_eq!(
                repeated.base_verified_managed,
                resolved.base_verified_managed
            );
        }
        assert_eq!(inventory(dir.path()), before);
    }
    #[test]
    fn project_symlink_and_dangling_link_remain_invalid_and_preserved() {
        for present in [true, false] {
            let dir = seed_dir();
            ensure_seeded(dir.path(), None);
            let target = dir.path().join("project-target.json");
            if present {
                std::fs::write(&target, LOCAL_STUB_BYTES).unwrap();
            }
            let path = project_catalog_path(dir.path());
            create_manifest_symlink(&target, &path).expect("real project symlink fixture");
            let report = project_report(dir.path());
            assert_eq!(report.catalog.len(), 8);
            assert!(report
                .warnings
                .iter()
                .any(|w| w.code == "projectInvalid" && w.path == path.display().to_string()));
            ensure_seeded(dir.path(), None);
            assert_eq!(std::fs::read_link(path).unwrap(), target);
            if present {
                assert_eq!(std::fs::read(target).unwrap(), LOCAL_STUB_BYTES);
            } else {
                assert!(!target.exists());
            }
        }
    }

    #[test]
    fn project_ownership_support_and_unavailable_base_contract() {
        let dir = seed_dir();
        for owner in ["legacy", "foreign"] {
            write_legacy_base(
                dir.path(),
                &manifest_json(&serde_json::json!([extra_row("team")]).to_string()),
            );
            if owner == "foreign" {
                write_managed_base(dir.path(), &[extra_row("team")], "stale", true);
                let mut base = base_json(dir.path());
                base["managed"]["owner"] = serde_json::json!("other");
                std::fs::write(manifest_path(dir.path()), base.to_string()).unwrap();
            }
            write_project_patch(
                dir.path(),
                patch_rows(serde_json::json!([{"key":"team","label":"ignored"}])),
            );
            let report = project_report(dir.path());
            assert_ne!(report.catalog[0].label, "ignored");
            assert!(report.warnings.iter().any(|w| w.code == "migrationPending"
                && w.path == project_catalog_path(dir.path()).display().to_string()));
            std::fs::write(project_catalog_path(dir.path()), "bad").unwrap();
            assert!(project_report(dir.path())
                .warnings
                .iter()
                .any(|w| w.code == "projectInvalid"));
        }
        std::fs::remove_file(manifest_path(dir.path())).unwrap();
        let report = project_report(dir.path());
        assert!(report.unavailable.is_some());
        assert!(report.catalog.is_empty());
        std::fs::write(manifest_path(dir.path()), "invalid").unwrap();
        let report = project_report(dir.path());
        assert!(report.unavailable.is_some());
        assert!(report.warnings.is_empty());
        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            write_managed_base(dir.path(), &[extra_row("team")], "stale", false);
            write_project_patch(
                dir.path(),
                patch_rows(serde_json::json!([extra_row("muse")])),
            );
            write_local(
                dir.path(),
                &patch_rows(serde_json::json!([{"key":"muse","label":"personal"}])).to_string(),
            );
            let report = project_report(dir.path());
            assert!(!report.catalog.iter().any(|d| d.key == "muse"));
            assert!(!report
                .warnings
                .iter()
                .any(|w| w.code == "localInvalid" || w.code == "projectInvalid"));
            assert!(report
                .warnings
                .iter()
                .any(|w| w.code == "managedBaseEdited"));
        });
    }

    #[test]
    fn project_interrupted_instance_journal_fallback_consumes_neither_patch() {
        let dir = seed_dir();
        let legacy = legacy_dir();
        std::fs::write(
            legacy.path().join("agents.json"),
            manifest_json(&serde_json::json!([extra_row("mine")]).to_string()),
        )
        .unwrap();
        std::fs::create_dir_all(catalog_dir(dir.path())).unwrap();
        write_project_patch(
            dir.path(),
            patch_rows(serde_json::json!([{"key":"mine","label":"Project"}])),
        );
        with_failure_at("after_journal", || {
            ensure_seeded(dir.path(), Some(legacy.path()))
        });
        assert!(!manifest_path(dir.path()).exists());
        // The early legacy-source return ignores even malformed patches.
        write_local(dir.path(), "invalid personal");
        let report = project_report(dir.path());
        assert_eq!(keys_of(&report.catalog), ["mine"]);
        assert_ne!(report.catalog[0].label, "Project");
        assert!(!report
            .warnings
            .iter()
            .any(|w| w.code == "projectInvalid" || w.code == "localInvalid"));
    }

    #[test]
    fn project_bytes_and_registered_snapshot_survive_initialization_refresh_recovery() {
        for contents in [LOCAL_STUB_BYTES, b"<<<<<<< malformed project".as_slice()] {
            let dir = seed_dir();
            std::fs::create_dir_all(catalog_dir(dir.path())).unwrap();
            std::fs::write(project_catalog_path(dir.path()), contents).unwrap();
            let registered = dir.path().join("agents.30.instance.no-git.json");
            let registered_bytes = b"{\"agents\":[{\"id\":\"registered-snapshot\"}]}";
            std::fs::write(&registered, registered_bytes).unwrap();
            ensure_seeded(dir.path(), None);
            reseed_master_for_command(dir.path(), "claude").unwrap();
            assert_eq!(
                std::fs::read(local_catalog_path(dir.path())).unwrap(),
                LOCAL_STUB_BYTES
            );
            assert_eq!(
                std::fs::read(manifest_path(dir.path())).unwrap(),
                build_managed_base_bytes(&supported_shipped_definitions())
            );
            write_managed_base(dir.path(), &shipped_def_json(&["claude"]), "stale", true);
            ensure_seeded(dir.path(), None);
            assert_eq!(
                std::fs::read(project_catalog_path(dir.path())).unwrap(),
                contents
            );
            std::fs::remove_file(local_catalog_path(dir.path())).unwrap();
            // Recovery returns the saved legacy vector before either layer.
            write_legacy_base(
                dir.path(),
                &manifest_json(&serde_json::json!([extra_row("mine")]).to_string()),
            );
            with_failure_at("after_journal", || ensure_seeded(dir.path(), None));
            assert_eq!(keys_of(&project_report(dir.path()).catalog), ["mine"]);
            ensure_seeded(dir.path(), None);
            assert_eq!(
                std::fs::read(project_catalog_path(dir.path())).unwrap(),
                contents
            );
            std::fs::write(manifest_path(dir.path()), "invalid base").unwrap();
            ensure_seeded(dir.path(), None);
            project_report(dir.path());
            assert_eq!(
                std::fs::read(project_catalog_path(dir.path())).unwrap(),
                contents
            );
            assert_eq!(std::fs::read(&registered).unwrap(), registered_bytes);
        }
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        assert!(!project_catalog_path(dir.path()).exists());
    }

    /// (key, label, description, color, command, instructionsFilename, seed dest)
    /// for the nine current presets. The embedded default must match these
    /// values. The frontend keeps a parallel `FALLBACK_CODING_AGENTS` test (E7).
    #[allow(clippy::type_complexity)]
    const EXPECTED_PRESETS: [(&str, &str, &str, &str, &str, Option<&str>, Option<&str>); 9] = [
        (
            "claude",
            "Claude Code",
            "Coding Agent by Anthropic",
            "#d97706",
            "claude",
            Some("CLAUDE.md"),
            Some(".claude"),
        ),
        (
            "codex",
            "Codex",
            "Coding Agent by OpenAI",
            "#10b981",
            "codex",
            Some("AGENTS.md"),
            Some(".codex"),
        ),
        (
            "hermes",
            "Hermes",
            "Coding Agent by Nous Research",
            "#8b5cf6",
            "hermes",
            Some("AGENTS.md"),
            None,
        ),
        (
            "cursor",
            "Cursor CLI",
            "Coding Agent by Cursor",
            "#22d3ee",
            "agent",
            Some("AGENTS.md"),
            None,
        ),
        (
            "pi",
            "Pi",
            "Coding Agent by Earendil Inc",
            "#ec4899",
            "pi",
            Some("AGENTS.md"),
            None,
        ),
        (
            "opencode",
            "OpenCode",
            "Open-source terminal coding agent by Anomaly",
            "#64748b",
            "opencode",
            Some("AGENTS.md"),
            Some(".opencode"),
        ),
        (
            "antigravity",
            "Antigravity",
            "Coding Agent by Google",
            "#4285F4",
            "agy",
            Some("AGENTS.md"),
            None,
        ),
        (
            "grok",
            "Grok Build",
            "Coding Agent by SpaceXAI",
            "#64748b",
            "grok",
            Some("AGENTS.md"),
            None,
        ),
        (
            "muse",
            "Muse Code",
            "Meta terminal coding agent (beta; macOS/Linux host only)",
            "#0668E1",
            "muse",
            None,
            None,
        ),
    ];

    fn manifest_json(agents_json: &str) -> String {
        format!("{{\"schemaVersion\":1,\"agents\":{agents_json}}}")
    }

    fn seed_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// #1912 - support-override test tables: all 9 rows spelled out, in the
    /// shipped order, exactly one `false` each. Reaching the PRODUCTION
    /// wrappers through `with_builtin_agent_support_for_test` is the point: the
    /// controls below prove behavior on the real call chain, not on copies.
    /// `TABLE_MUSE_OFF` mirrors the shipped table (#1999 disables muse); the
    /// all-enabled control drives the opposite direction.
    const TABLE_MUSE_OFF: &[(&str, bool)] = &[
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
    const TABLE_CLAUDE_OFF: &[(&str, bool)] = &[
        ("claude", false),
        ("codex", true),
        ("hermes", true),
        ("cursor", true),
        ("pi", true),
        ("opencode", true),
        ("antigravity", true),
        ("grok", true),
        ("muse", true),
    ];
    const TABLE_ALL_ENABLED: &[(&str, bool)] = &[
        ("claude", true),
        ("codex", true),
        ("hermes", true),
        ("cursor", true),
        ("pi", true),
        ("opencode", true),
        ("antigravity", true),
        ("grok", true),
        ("muse", true),
    ];

    #[test]
    fn embedded_default_parses_with_nine_agents_in_order() {
        let catalog = embedded_default_catalog();
        assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
        let keys: Vec<&str> = catalog.agents.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "claude",
                "codex",
                "hermes",
                "cursor",
                "pi",
                "opencode",
                "antigravity",
                "grok",
                "muse"
            ]
        );
        // Muse stays last, now immediately preceded by Grok.
        let last = catalog.agents.last().unwrap();
        assert_eq!(last.key, "muse");
        assert_eq!(last.command, "muse");
    }

    /// #2736 P1 - the shipped `installCommands` pin, kept OUT of
    /// `embedded_default_matches_current_presets_exactly` so that test stays
    /// under the pinned cognitive complexity threshold of 25.
    fn assert_shipped_install_commands_2736(key: &str, def: &CodingAgentDefinition) {
        match scope_b_expected_commands_2800(key) {
            Some(expected) => assert_eq!(
                serde_json::to_value(
                    def.install_commands
                        .as_ref()
                        .expect("matrix row install commands")
                )
                .unwrap(),
                expected,
                "{key}"
            ),
            None => assert!(
                def.install_commands.is_none(),
                "{key} excluded from install matrix"
            ),
        }
    }

    #[test]
    fn embedded_default_matches_current_presets_exactly() {
        // Drift guard for every current preset field.
        let catalog = embedded_default_catalog();
        assert_eq!(catalog.agents.len(), EXPECTED_PRESETS.len());
        for (def, (key, label, desc, color, command, filename, seed_dest)) in
            catalog.agents.iter().zip(EXPECTED_PRESETS)
        {
            assert_eq!(def.key, key);
            assert_eq!(def.label, label);
            assert_eq!(def.description, desc);
            assert_eq!(def.color, color);
            assert_eq!(def.command, command);
            assert_eq!(def.instructions_filename.as_deref(), filename);
            // #769 P2: Claude/Codex/OpenCode ship an active configSeed; the other
            // six ship none (no master, no re-seed button).
            match seed_dest {
                Some(dest) => {
                    let cs = def
                        .config_seed
                        .as_ref()
                        .unwrap_or_else(|| panic!("{key} must ship configSeed"));
                    assert!(cs.enabled, "{key} configSeed must be enabled");
                    assert_eq!(cs.dest, dest, "{key} configSeed dest");
                }
                None => assert!(
                    def.config_seed.is_none(),
                    "{key} must ship configSeed UNSET"
                ),
            }
            assert!(def.removable, "{key} must be removable");
            assert!(def.envs.is_empty());
            assert!(!def.isolated_home);
            // #2124 - every shipped row carries the conservative burst filter.
            let burst = def
                .idle_burst
                .as_ref()
                .unwrap_or_else(|| panic!("{key} must ship idleBurst"));
            assert_eq!(burst.max_bytes, Some(1024), "{key} idleBurst maxBytes");
            assert_eq!(burst.max_secs, Some(3.0), "{key} idleBurst maxSecs");
            assert_eq!(
                burst.prior_silence_secs,
                Some(60.0),
                "{key} idleBurst priorSilenceSecs"
            );
            assert_shipped_install_commands_2736(key, def);
        }
        let raw: serde_json::Value = serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
        assert_eq!(raw["schemaVersion"], 1);
        assert_eq!(
            raw["agents"].as_array().unwrap().last().unwrap(),
            &serde_json::json!({
                "key": "muse",
                "label": "Muse Code",
                "description": "Meta terminal coding agent (beta; macOS/Linux host only)",
                "color": "#0668E1",
                "command": "muse",
                "envs": [],
                "isolatedHome": false,
                "removable": true,
                "updateCommands": [],
                "autoUpdate": false,
                "idleBurst": {
                    "maxBytes": 1024,
                    "maxSecs": 3.0,
                    "priorSilenceSecs": 60.0
                }
            })
        );
        assert_eq!(
            catalog
                .agents
                .iter()
                .filter(|def| def.config_seed.is_some())
                .count(),
            3
        );
        assert_eq!(
            catalog
                .agents
                .iter()
                .filter(|def| def.config_seed.is_none())
                .count(),
            6
        );
    }

    #[test]
    fn every_embedded_entry_validates() {
        for def in embedded_default_catalog().agents {
            validate_definition(&def).unwrap_or_else(|e| panic!("{}: {e}", def.key));
        }
    }

    /// #2124 T9 - serde, strict local layer and legacy migration for `idleBurst`.
    #[test]
    fn idle_burst_serde_and_local_layer() {
        // A definition without `idleBurst` deserializes `None` and does not
        // re-emit the key.
        let mut plain = shipped_def_json(&["claude"]).remove(0);
        plain.as_object_mut().unwrap().remove("idleBurst");
        let definition: CodingAgentDefinition = serde_json::from_value(plain).unwrap();
        assert!(definition.idle_burst.is_none());
        assert!(!serde_json::to_string(&definition)
            .unwrap()
            .contains("idleBurst"));

        // The embedded default ships the filter on all nine rows.
        let embedded = embedded_default_catalog();
        assert_eq!(embedded.agents.len(), 9);
        for row in &embedded.agents {
            let burst = row
                .idle_burst
                .as_ref()
                .unwrap_or_else(|| panic!("{} must ship idleBurst", row.key));
            assert_eq!(burst.max_bytes, Some(1024), "{}", row.key);
            assert_eq!(burst.max_secs, Some(3.0), "{}", row.key);
            assert_eq!(burst.prior_silence_secs, Some(60.0), "{}", row.key);
        }

        // Compose a managed base (1024/3/60) with a local object patch: the
        // stated subfield replaces, the absent ones inherit.
        let dir = seed_dir();
        let revision = managed_content_sha256(&supported_shipped_definitions());
        write_managed_base(dir.path(), &shipped_def_json(&["claude"]), &revision, true);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxBytes":2048}}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let claude = report
            .catalog
            .iter()
            .find(|def| def.key == "claude")
            .unwrap();
        let burst = claude.idle_burst.as_ref().expect("composed idleBurst");
        assert_eq!(burst.max_bytes, Some(2048));
        assert_eq!(burst.max_secs, Some(3.0), "absent maxSecs inherits");
        assert_eq!(burst.prior_silence_secs, Some(60.0));

        // Local `null` clears the whole object (filter off).
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":null}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report.catalog[0].idle_burst.is_none());

        // Unknown subfields and invalid value shapes disable the whole local
        // layer; the base entry stays intact.
        for local in [
            r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"foo":1}}]}"##,
            r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxBytes":-1}}]}"##,
            r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxSecs":"3"}}]}"##,
            r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxSecs":-1}}]}"##,
        ] {
            write_local(dir.path(), local);
            let report = load_catalog_report(dir.path());
            assert!(
                report
                    .warnings
                    .iter()
                    .any(|warning| warning.code == "localInvalid"),
                "{local}: {:?}",
                report.warnings
            );
            let claude = report
                .catalog
                .iter()
                .find(|def| def.key == "claude")
                .unwrap();
            assert_eq!(
                claude.idle_burst.as_ref().unwrap().max_bytes,
                Some(1024),
                "the base entry stays intact after a rejected local layer"
            );
        }

        // A legacy row that states `idleBurst` explicitly pins it in the
        // extracted local layer, presence only.
        let mut legacy_claude = shipped_def_json(&["claude"]).remove(0);
        legacy_claude["idleBurst"] = serde_json::json!({"maxSecs": 2.0});
        let legacy = serde_json::json!({"schemaVersion": 1, "agents": [legacy_claude]}).to_string();
        let extracted = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
            .expect("legacy extraction with explicit idleBurst");
        let local: serde_json::Value = serde_json::from_slice(&extracted).unwrap();
        assert_eq!(
            local["agents"][0]["idleBurst"],
            serde_json::json!({"maxSecs": 2.0})
        );
    }

    /// #2736 - the shipped `installCommands` value for `key`, if any.
    fn expected_install_2736(key: &str) -> Option<&'static str> {
        match key {
            "claude" | "codex" | "hermes" | "cursor" | "pi" | "opencode" | "antigravity"
            | "grok" => Some("echo No verified installer for this platform 1>&2 && exit 1"),
            _ => None,
        }
    }

    /// #2736 - a managed base of the shipped claude row, current revision.
    fn managed_claude_base_2736(ac_dir: &Path) {
        let revision = managed_content_sha256(&supported_shipped_definitions());
        write_managed_base(ac_dir, &shipped_def_json(&["claude"]), &revision, true);
    }

    fn claude_of(report: &CatalogReport) -> &CodingAgentDefinition {
        report
            .catalog
            .iter()
            .find(|def| def.key == "claude")
            .expect("claude entry")
    }

    #[test]
    fn install_commands_2736_shipped_for_eight_builtins_and_absent_for_muse() {
        let catalog = embedded_default_catalog();
        assert_eq!(catalog.agents.len(), 9);
        for def in &catalog.agents {
            assert_shipped_install_commands_2736(&def.key, def);
            assert_eq!(
                def.install_commands.as_ref().map(|ic| ic.default.as_str()),
                expected_install_2736(&def.key)
            );
        }
        assert_eq!(
            catalog
                .agents
                .iter()
                .filter(|def| def.install_commands.is_some())
                .count(),
            8
        );
    }

    #[test]
    fn install_commands_2736_optional_absent_deserializes_to_none() {
        let mut plain = shipped_def_json(&["claude"]).remove(0);
        plain.as_object_mut().unwrap().remove("installCommands");
        let definition: CodingAgentDefinition = serde_json::from_value(plain).unwrap();
        assert!(definition.install_commands.is_none());
        assert!(validate_definition(&definition).is_ok());
        assert!(!serde_json::to_string(&definition)
            .unwrap()
            .contains("installCommands"));
    }

    #[test]
    fn install_commands_2736_rejects_a_blank_or_control_character_value() {
        let base: CodingAgentDefinition =
            serde_json::from_value(shipped_def_json(&["claude"]).remove(0)).unwrap();
        let cases = [
            (
                InstallCommands {
                    default: "   ".to_string(),
                    windows: None,
                    macos: None,
                    linux: None,
                },
                "   ",
            ),
            (
                InstallCommands {
                    default: "npm i -g ok".to_string(),
                    windows: Some("secretwin\u{7}cmd".to_string()),
                    macos: None,
                    linux: None,
                },
                "secretwin",
            ),
            (
                InstallCommands {
                    default: "npm i -g ok".to_string(),
                    windows: None,
                    macos: Some("secretmac\u{2028}cmd".to_string()),
                    linux: None,
                },
                "secretmac",
            ),
        ];
        for (install, secret) in cases {
            let mut def = base.clone();
            def.install_commands = Some(install);
            let error = validate_definition(&def).expect_err("must be rejected");
            assert!(error.contains("installCommands."), "{error}");
            if !secret.trim().is_empty() {
                assert!(!error.contains(secret), "error echoes the value: {error}");
            }
            let reason = definition_problem_reason(&def);
            assert!(
                !reason.contains(secret) || secret.trim().is_empty(),
                "{reason}"
            );
            let raw = serde_json::to_value(&def).unwrap();
            let problem = raw_install_commands_problem(&raw).expect("raw problem");
            if !secret.trim().is_empty() {
                assert!(!problem.contains(secret), "{problem}");
            }
        }
    }

    #[test]
    fn install_commands_2736_requires_default_when_the_object_is_present() {
        let error = serde_json::from_value::<InstallCommands>(serde_json::json!({"windows": "x"}))
            .expect_err("default is required");
        assert!(
            error.to_string().contains("missing field `default`"),
            "{error}"
        );

        let mut bad = shipped_def_json(&["claude"]).remove(0);
        bad["installCommands"] = serde_json::json!({"windows": "x"});
        let good = shipped_def_json(&["codex"]).remove(0);
        let dir = seed_dir();
        write_legacy_base(
            dir.path(),
            &manifest_json(&serde_json::to_string(&vec![bad, good]).unwrap()),
        );
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
        assert!(report.catalog.iter().all(|def| def.key != "claude"));
        assert!(report.catalog.iter().any(|def| def.key == "codex"));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.code == "invalidDefinition"),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn install_commands_2736_base_entry_with_a_non_object_value_is_omitted_with_a_warning() {
        let mut raw = shipped_def_json(&["claude"]).remove(0);
        raw["installCommands"] = serde_json::json!("npm install -g secret-pkg");
        let problem = raw_install_commands_problem(&raw).expect("non-object is a problem");
        assert!(!problem.contains("secret-pkg"), "{problem}");
        for value in [
            serde_json::json!({"default": 7}),
            serde_json::json!({"default": "ok", "linux": 7}),
            serde_json::json!({"default": "ok", "linux": "  "}),
        ] {
            raw["installCommands"] = value.clone();
            assert!(raw_install_commands_problem(&raw).is_some(), "{value}");
        }
        raw["installCommands"] = serde_json::json!({"default": "ok"});
        assert!(raw_install_commands_problem(&raw).is_none());

        let mut bad = shipped_def_json(&["claude"]).remove(0);
        bad["installCommands"] = serde_json::json!("npm install -g secret-pkg");
        let good = shipped_def_json(&["codex"]).remove(0);
        let dir = seed_dir();
        write_legacy_base(
            dir.path(),
            &manifest_json(&serde_json::to_string(&vec![bad, good]).unwrap()),
        );
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].key, "codex");
        let warning = report
            .warnings
            .iter()
            .find(|warning| warning.code == "invalidDefinition")
            .expect("invalidDefinition");
        assert!(
            warning.reason.contains("installCommands"),
            "{}",
            warning.reason
        );
        assert!(!warning.reason.contains("secret-pkg"), "{}", warning.reason);
    }

    #[test]
    fn install_commands_2736_local_layer_patches_windows_and_inherits_default() {
        let dir = seed_dir();
        managed_claude_base_2736(dir.path());
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":{"windows":"winget install claude"}}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let ic = claude_of(&report)
            .install_commands
            .as_ref()
            .expect("composed");
        assert_eq!(ic.default, expected_install_2736("claude").unwrap());
        assert_eq!(ic.windows.as_deref(), Some("winget install claude"));
        let expected = scope_b_expected_commands_2800("claude").unwrap();
        assert_eq!(ic.macos.as_deref(), expected["macos"].as_str());
        assert_eq!(ic.linux.as_deref(), expected["linux"].as_str());
    }

    #[test]
    fn install_commands_2736_local_null_clears_the_object() {
        let dir = seed_dir();
        managed_claude_base_2736(dir.path());
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":null}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(claude_of(&report).install_commands.is_none());
    }

    #[test]
    fn install_commands_2736_local_null_on_one_os_key_clears_only_that_key() {
        let dir = seed_dir();
        let mut claude = shipped_def_json(&["claude"]).remove(0);
        claude["installCommands"] = serde_json::json!({
            "default": "npm install -g @anthropic-ai/claude-code",
            "windows": "win cmd",
            "linux": "linux cmd"
        });
        write_managed_base(dir.path(), &[claude], "stale-revision", true);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":{"windows":null}}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(
            report.warnings.iter().all(|w| w.code != "localInvalid"),
            "{:?}",
            report.warnings
        );
        let ic = claude_of(&report).install_commands.as_ref().expect("kept");
        assert_eq!(ic.default, "npm install -g @anthropic-ai/claude-code");
        assert!(ic.windows.is_none(), "windows cleared");
        assert_eq!(ic.linux.as_deref(), Some("linux cmd"), "linux inherited");
    }

    #[test]
    fn install_commands_2736_one_bad_local_value_discards_the_entire_local_layer() {
        let dir = seed_dir();
        let revision = managed_content_sha256(&supported_shipped_definitions());
        write_managed_base(
            dir.path(),
            &shipped_def_json(&["claude", "codex"]),
            &revision,
            true,
        );
        for local in [
            r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"windows":"  "}}]}"##,
            r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"default":7}}]}"##,
            r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":"npm i"}]}"##,
            r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"default":""}}]}"##,
        ] {
            write_local(dir.path(), local);
            let report = load_catalog_report(dir.path());
            assert!(
                report.warnings.iter().any(|w| w.code == "localInvalid"),
                "{local}: {:?}",
                report.warnings
            );
            let codex = report
                .catalog
                .iter()
                .find(|def| def.key == "codex")
                .unwrap();
            assert_eq!(
                codex.label, "Codex",
                "{local}: the valid row is NOT applied"
            );
            assert_eq!(
                claude_of(&report)
                    .install_commands
                    .as_ref()
                    .unwrap()
                    .default,
                expected_install_2736("claude").unwrap(),
                "{local}: the base entry stays intact"
            );
        }
    }

    #[test]
    fn install_commands_2736_new_local_key_without_install_commands_is_accepted() {
        let fields = parse_local_fields(
            serde_json::json!({
                "key": "mine", "label": "Mine", "description": "d", "color": "#111",
                "command": "mytool", "envs": [], "isolatedHome": false,
                "removable": true, "updateCommands": [], "autoUpdate": false
            })
            .as_object()
            .unwrap(),
            "local agents[0]",
        )
        .expect("valid new row");
        let definition = build_new_definition("mine", &fields, LayerDescription::Local)
            .expect("not missing anything");
        assert!(definition.install_commands.is_none());

        let dir = seed_dir();
        managed_claude_base_2736(dir.path());
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let mine = report.catalog.iter().find(|def| def.key == "mine").unwrap();
        assert!(mine.install_commands.is_none());
    }

    #[test]
    fn install_commands_2736_unknown_nested_field_is_a_whole_layer_error() {
        let dir = seed_dir();
        managed_claude_base_2736(dir.path());
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MINE","installCommands":{"default":"x","freebsd":"y"}}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(
            report.warnings.iter().any(|w| w.code == "localInvalid"),
            "{:?}",
            report.warnings
        );
        assert_eq!(claude_of(&report).label, "Claude Code");
    }

    #[test]
    fn install_commands_2736_managed_base_refreshes_from_the_previous_revision() {
        let old: Vec<CodingAgentDefinition> = supported_shipped_definitions()
            .into_iter()
            .map(|mut def| {
                def.install_commands = None;
                def
            })
            .collect();
        let old_revision = managed_content_sha256(&old);
        let new_revision = managed_content_sha256(&supported_shipped_definitions());
        assert_ne!(old_revision, new_revision);
        let old_json: Vec<serde_json::Value> = old
            .iter()
            .map(|def| serde_json::to_value(def).unwrap())
            .collect();
        let dir = seed_dir();
        write_managed_base(dir.path(), &old_json, &old_revision, true);

        let report = load_catalog_report(dir.path());
        assert!(
            report.warnings.iter().any(|w| w.code == "refreshFailed"),
            "stale revision is reported: {:?}",
            report.warnings
        );
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "stale base refreshes"
        );
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            build_managed_base_bytes(&supported_shipped_definitions())
        );
        assert_eq!(base_json(dir.path())["managed"]["revision"], new_revision);
        let report = load_catalog_report(dir.path());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(
            claude_of(&report)
                .install_commands
                .as_ref()
                .unwrap()
                .default,
            expected_install_2736("claude").unwrap()
        );
    }

    #[test]
    fn install_commands_2736_edited_managed_base_is_not_refreshed() {
        let old: Vec<CodingAgentDefinition> = supported_shipped_definitions()
            .into_iter()
            .map(|mut def| {
                def.install_commands = None;
                def
            })
            .collect();
        let old_revision = managed_content_sha256(&old);
        let mut edited = old.clone();
        edited[0].color = "#d97707".to_string();
        let root = serde_json::json!({
            "schemaVersion": 1,
            "agents": edited,
            "managed": {
                "owner": "agentscommander",
                "version": 1,
                "revision": old_revision,
                "contentSha256": old_revision,
            },
        });
        let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
        bytes.push(b'\n');
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();

        let report = load_catalog_report(dir.path());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.code == "managedBaseEdited"),
            "{:?}",
            report.warnings
        );
        assert!(ensure_seeded(dir.path(), None).is_none());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn install_commands_2736_unknown_field_on_an_older_build_reports_migration_pending() {
        assert!(!KNOWN_DEFINITION_FIELDS.contains(&"futureField2736"));
        let mut claude = shipped_def_json(&["claude"]).remove(0);
        claude["futureField2736"] = serde_json::json!({"keep": "me"});
        let dir = seed_dir();
        write_legacy_base(
            dir.path(),
            &manifest_json(&serde_json::to_string(&vec![claude]).unwrap()),
        );
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
        assert_eq!(report.catalog.len(), 1, "the entry survives");
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.code == "migrationPending" && w.reason.contains("futureField2736")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn install_commands_2736_pin_wire_keeps_presence_semantics() {
        let definition: CodingAgentDefinition =
            serde_json::from_value(shipped_def_json(&["claude"]).remove(0)).unwrap();
        let mut with_key = serde_json::Map::new();
        with_key.insert("key".to_string(), serde_json::json!("claude"));
        with_key.insert(
            "installCommands".to_string(),
            serde_json::to_value(definition.install_commands.as_ref().unwrap()).unwrap(),
        );
        let mut without_key = serde_json::Map::new();
        without_key.insert("key".to_string(), serde_json::json!("claude"));

        for wire in [
            pin_explicit_legacy_fields(&definition, &with_key),
            materialize_complete_legacy_fields(&definition, &with_key),
        ] {
            let value = serde_json::to_value(&wire).unwrap();
            assert_eq!(
                value["installCommands"],
                serde_json::to_value(definition.install_commands.as_ref().unwrap()).unwrap()
            );
        }
        for wire in [
            pin_explicit_legacy_fields(&definition, &without_key),
            materialize_complete_legacy_fields(&definition, &without_key),
        ] {
            let value = serde_json::to_value(&wire).unwrap();
            assert!(
                value.get("installCommands").is_none(),
                "absent legacy key is never pinned or forced: {value}"
            );
        }

        let mut legacy_claude = shipped_def_json(&["claude"]).remove(0);
        legacy_claude["installCommands"] = serde_json::json!({"default": "npm i -g mine"});
        let legacy = serde_json::json!({"schemaVersion": 1, "agents": [legacy_claude]}).to_string();
        let extracted = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
            .expect("legacy extraction with explicit installCommands");
        let local: serde_json::Value = serde_json::from_slice(&extracted).unwrap();
        assert_eq!(
            local["agents"][0]["installCommands"],
            serde_json::json!({"default": "npm i -g mine"})
        );

        let mut ambiguous = shipped_def_json(&["claude"]).remove(0);
        ambiguous["installCommands"] = serde_json::json!({"default": " "});
        let legacy = serde_json::json!({"schemaVersion": 1, "agents": [ambiguous]}).to_string();
        let error = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
            .expect_err("ambiguous installCommands refuses the migration");
        assert!(error.contains("installCommands"), "{error}");
    }

    #[test]
    fn cursor_cli_command_is_agent() {
        let catalog = embedded_default_catalog();
        let cursor = catalog.agents.iter().find(|a| a.key == "cursor").unwrap();
        assert_eq!(cursor.command, "agent");
    }

    #[test]
    fn validate_catalog_key_accepts_allowlist_and_rejects_others() {
        for ok in ["claude", "cursor-cli", "pi", "a1", "opencode", "x-2-y"] {
            assert!(validate_catalog_key(ok).is_ok(), "should accept {ok:?}");
        }
        for bad in [
            "",
            "Claude",
            "cursor_cli",
            "cursor cli",
            "café",
            "a.b",
            "UP",
            "a/b",
        ] {
            assert!(validate_catalog_key(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn load_missing_manifest_is_unavailable_without_seeding() {
        // #1967 P4: a missing persisted catalog is unavailable; the embedded
        // default is NOT substituted and nothing is created on disk.
        let dir = seed_dir();
        let unavailable = load_catalog(dir.path()).expect_err("absent catalog is unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");
        assert_eq!(
            unavailable.path,
            manifest_path(dir.path()).display().to_string()
        );
        assert!(unavailable.reason.contains("no persisted catalog"));
        assert!(
            !catalog_dir(dir.path()).exists(),
            "a catalog read must never create the catalog directory"
        );
    }

    #[test]
    fn load_corrupt_manifest_is_base_invalid_and_preserves_file() {
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let garbage = b"{ this is not valid json";
        std::fs::write(&path, garbage).unwrap();

        let unavailable = load_catalog(dir.path()).expect_err("corrupt catalog is unavailable");
        assert_eq!(unavailable.code, "baseInvalid");
        assert_eq!(unavailable.path, path.display().to_string());
        // G3: the corrupt file is preserved byte-for-byte, never overwritten.
        assert_eq!(std::fs::read(&path).unwrap(), garbage);
    }

    #[test]
    fn load_skips_invalid_entry_and_keeps_valid_rest() {
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // First entry is valid; second has an illegal key (uppercase); third is
        // valid. Only the two valid entries survive.
        let agents = r##"[
            {"key":"claude","label":"Claude","description":"d","color":"#000","command":"claude","envs":[],"isolatedHome":false,"removable":true},
            {"key":"BAD KEY","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true},
            {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}
        ]"##;
        std::fs::write(&path, manifest_json(agents)).unwrap();

        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        let keys: Vec<&str> = loaded.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(keys, ["claude", "mine"]);
    }

    #[test]
    fn load_dedups_duplicate_keys_first_wins() {
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let agents = r##"[
            {"key":"claude","label":"First","description":"d","color":"#000","command":"claude","envs":[],"isolatedHome":false,"removable":true},
            {"key":"claude","label":"Second","description":"d","color":"#000","command":"claude","envs":[],"isolatedHome":false,"removable":true}
        ]"##;
        std::fs::write(&path, manifest_json(agents)).unwrap();

        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].label, "First");
    }

    #[test]
    fn load_honors_valid_empty_agents_list() {
        // A user who removed every built-in has a valid, empty manifest. It is
        // honored verbatim; the embedded default is NOT resurrected.
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, manifest_json("[]")).unwrap();

        assert!(load_catalog(dir.path())
            .expect("persisted catalog")
            .is_empty());
    }

    #[test]
    fn ensure_seeded_writes_when_absent_then_is_idempotent() {
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        assert!(!path.exists());

        ensure_seeded(dir.path(), None);
        assert!(path.exists(), "seed writes the manifest when absent");
        assert_eq!(load_catalog(dir.path()).expect("seeded catalog").len(), 8);

        // Idempotent + never clobbers a user edit: hand-edit to a single custom
        // agent, re-seed, and confirm the edit is preserved.
        let custom = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        std::fs::write(&path, &custom).unwrap();
        ensure_seeded(dir.path(), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "mine");
    }

    #[test]
    fn seeded_manifest_leaves_no_temp_residue() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let catalog = catalog_dir(dir.path());
        let residue: Vec<_> = std::fs::read_dir(&catalog)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.contains(".tmp"))
                    .unwrap_or(false)
            })
            .collect();
        assert!(residue.is_empty(), "no leftover temp files after seed");
    }

    // ---- #769 Phase 2: masters + re-seed --------------------------------

    fn master(cmd: &str) -> &'static EmbeddedSeedMaster {
        EMBEDDED_SEED_MASTERS
            .iter()
            .find(|m| m.command_basename == cmd)
            .unwrap()
    }

    #[test]
    fn reseedable_commands_are_claude_codex_opencode() {
        let mut got = reseedable_command_basenames();
        got.sort();
        assert_eq!(got, vec!["claude", "codex", "opencode"]);
    }

    #[test]
    fn every_embedded_master_maps_to_a_catalog_def_with_matching_configseed() {
        let catalog = embedded_default_catalog();
        for m in EMBEDDED_SEED_MASTERS {
            let def = catalog
                .agents
                .iter()
                .find(|d| {
                    command_executable_basename(&d.command).as_deref() == Some(m.command_basename)
                })
                .unwrap_or_else(|| panic!("no catalog def for master {}", m.command_basename));
            let cs = def
                .config_seed
                .as_ref()
                .unwrap_or_else(|| panic!("{} def missing configSeed", m.command_basename));
            assert!(cs.enabled);
            assert_eq!(
                cs.dest, m.dest,
                "master dest must match the def configSeed dest"
            );
            assert_eq!(
                def.key, m.key,
                "#1912 master key must match the catalog def key it maps to"
            );
            assert!(
                !m.files.is_empty(),
                "master {} must ship >=1 file",
                m.command_basename
            );
        }
    }

    #[test]
    fn ensure_seeded_masters_creates_nonempty_masters_and_preserves_edits() {
        let dir = seed_dir();
        ensure_seeded_masters(dir.path(), None);
        for cmd in ["claude", "codex", "opencode"] {
            let m = master(cmd);
            let md = master_dir_for_dest(dir.path(), m.dest);
            assert!(
                crate::config::config_seed::is_nonempty_seed_dir(&md),
                "{cmd} master should be non-empty"
            );
            assert_eq!(
                std::fs::read(md.join(m.files[0].rel_path)).unwrap(),
                m.files[0].bytes
            );
        }
        // Idempotent + never clobbers a user edit.
        let m = master("claude");
        let file = master_dir_for_dest(dir.path(), m.dest).join(m.files[0].rel_path);
        std::fs::write(&file, b"USER EDIT").unwrap();
        ensure_seeded_masters(dir.path(), None);
        assert_eq!(std::fs::read(&file).unwrap(), b"USER EDIT");
    }

    #[test]
    fn reseed_installs_embedded_default_and_backs_up_current() {
        let dir = seed_dir();
        ensure_seeded_masters(dir.path(), None);
        let m = master("claude");
        let master_dir = master_dir_for_dest(dir.path(), m.dest);
        let file = master_dir.join(m.files[0].rel_path);
        std::fs::write(&file, b"USER EDITED").unwrap();

        let result = reseed_master_for_command(dir.path(), "claude").unwrap();
        assert_eq!(result.dest, ".claude");
        assert!(!result.backup_path.is_empty());
        // Master restored to the embedded default.
        assert_eq!(std::fs::read(&file).unwrap(), m.files[0].bytes);
        // The `.bak` holds the user's prior edit (verbatim).
        let bak_file = Path::new(&result.backup_path).join(m.files[0].rel_path);
        assert_eq!(std::fs::read(&bak_file).unwrap(), b"USER EDITED");
    }

    #[test]
    fn reseed_when_master_absent_installs_without_backup() {
        let dir = seed_dir();
        // Masters not seeded: the _seed dir is absent.
        let m = master("codex");
        let master_dir = master_dir_for_dest(dir.path(), m.dest);
        assert!(!master_dir.exists());

        let result = reseed_master_for_command(dir.path(), "codex").unwrap();
        assert_eq!(result.dest, ".codex");
        assert!(
            result.backup_path.is_empty(),
            "no backup when master was absent"
        );
        assert_eq!(
            std::fs::read(master_dir.join(m.files[0].rel_path)).unwrap(),
            m.files[0].bytes
        );
    }

    #[test]
    fn reseed_gating_is_exact_basename_never_startswith() {
        let dir = seed_dir();
        ensure_seeded_masters(dir.path(), None);
        // `pi` and `agent` are real built-in commands but ship NO master; `pip`
        // and `clau` must not match `pi`/`claude` via any prefix rule; empty and
        // unknown are rejected.
        for bad in ["pi", "agent", "pip", "clau", "claudex", "notacommand", ""] {
            assert!(
                reseed_master_for_command(dir.path(), bad).is_err(),
                "should reject {bad:?}"
            );
        }
        // Path/extension forms of a real master command still resolve by basename.
        for good in [
            "claude",
            "codex",
            "opencode",
            "claude.exe",
            "codex --model x",
        ] {
            assert!(
                reseed_master_for_command(dir.path(), good).is_ok(),
                "should accept {good:?}"
            );
        }
    }

    #[test]
    fn master_dir_for_dest_is_under_seed_subdir() {
        let dir = seed_dir();
        let p = master_dir_for_dest(dir.path(), ".claude");
        assert_eq!(
            p,
            dir.path()
                .join("coding-agents")
                .join("_seed")
                .join(".claude")
        );
    }

    // ---- #1318 relocation, migration, per-project seeding ----------------

    fn legacy_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// A hand-authored legacy catalog (custom entry, byte-distinct from the
    /// embedded default). The tempdir IS the legacy `<config_dir>/coding-agents`
    /// directory.
    fn legacy_catalog_json() -> Vec<u8> {
        manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        )
        .into_bytes()
    }

    #[test]
    fn managed_catalog_migrates_instance_legacy_into_absent_project_base() {
        // #1968: an absent project base imports ONLY the instance agents.json.
        // The ownership transfer keeps the source bytes exactly in the
        // project-local backup, and the instance source itself is never written.
        let project = seed_dir();
        let legacy = legacy_dir();
        let legacy_bytes = legacy_catalog_json();
        std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();

        let published = ensure_seeded(project.path(), Some(legacy.path()));
        assert!(published.is_some(), "a first migration publishes the base");

        let catalog = catalog_dir(project.path());
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            legacy_bytes,
            "the backup keeps the source bytes exactly"
        );
        let base: serde_json::Value =
            serde_json::from_slice(&std::fs::read(manifest_path(project.path())).unwrap()).unwrap();
        assert_eq!(base["managed"]["owner"], "agentscommander");
        assert_eq!(base["managed"]["version"], 1);
        assert_eq!(base["agents"].as_array().unwrap().len(), 8);

        let journal: serde_json::Value = serde_json::from_slice(
            &std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(journal["sourceKind"], "instance");
        assert_eq!(
            journal["sourcePath"],
            legacy
                .path()
                .join("agents.10.default.json")
                .display()
                .to_string()
        );
        assert_eq!(
            journal["managedBase"]["managed"]["revision"],
            base["managed"]["revision"]
        );

        // Extracted local layer: the custom entry is complete, the eight shipped
        // keys are tombstoned, and the legacy order is pinned.
        let local: serde_json::Value =
            serde_json::from_slice(&std::fs::read(local_catalog_path(project.path())).unwrap())
                .unwrap();
        assert_eq!(local["order"], serde_json::json!(["mine"]));
        let rows = local["agents"].as_array().unwrap();
        assert!(rows
            .iter()
            .any(|row| row["key"] == "mine" && row["command"] == "mytool"));
        assert_eq!(rows.iter().filter(|row| row["remove"] == true).count(), 8);

        let loaded = load_catalog(project.path()).expect("migrated catalog");
        assert_eq!(keys_of(&loaded), ["mine"]);

        // Completed migration is idempotent: no re-extraction, no rewrite.
        let base_before = std::fs::read(manifest_path(project.path())).unwrap();
        let local_before = std::fs::read(local_catalog_path(project.path())).unwrap();
        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
        assert_eq!(
            std::fs::read(manifest_path(project.path())).unwrap(),
            base_before
        );
        assert_eq!(
            std::fs::read(local_catalog_path(project.path())).unwrap(),
            local_before
        );
        assert_eq!(
            std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
            legacy_bytes,
            "the instance source bytes survive the rename, never rewritten"
        );
        assert!(!legacy.path().join("agents.json").exists());
    }

    #[test]
    fn legacy_absent_or_not_a_file_seeds_embedded_default() {
        // Absent legacy dir -> embedded default.
        let project = seed_dir();
        let legacy = legacy_dir();
        ensure_seeded(project.path(), Some(legacy.path()));
        assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);

        // Legacy agents.json is a DIRECTORY -> not a regular file -> embedded.
        let project = seed_dir();
        std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
        ensure_seeded(project.path(), Some(legacy.path()));
        assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);

        // No legacy at all -> embedded default.
        let project = seed_dir();
        ensure_seeded(project.path(), None);
        assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);
    }

    #[test]
    fn project_file_present_never_touched_even_when_legacy_differs() {
        let project = seed_dir();
        let legacy = legacy_dir();
        std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();

        // First seed from legacy, then hand-edit the PROJECT file.
        ensure_seeded(project.path(), Some(legacy.path()));
        let project_file = manifest_path(project.path());
        let custom = manifest_json(
            r##"[{"key":"hand","label":"Hand","description":"d","color":"#222","command":"hand","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        std::fs::write(&project_file, &custom).unwrap();

        // A present project file is user-owned: a differing legacy must not win.
        ensure_seeded(project.path(), Some(legacy.path()));
        assert_eq!(std::fs::read_to_string(&project_file).unwrap(), custom);
    }

    #[test]
    fn legacy_masters_tree_copied_per_builtin_dest_and_embedded_fallback() {
        let project = seed_dir();
        let legacy = legacy_dir();
        let legacy_seed = legacy.path().join("_seed");
        // .claude and .codex masters exist in the legacy tree (one file each,
        // distinct bytes); .opencode does NOT (embedded default must win).
        std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap();
        std::fs::create_dir_all(legacy_seed.join(".codex")).unwrap();
        std::fs::write(legacy_seed.join(".claude/settings.json"), b"LEGACY CLAUDE").unwrap();
        std::fs::write(legacy_seed.join(".codex/config.toml"), b"LEGACY CODEX").unwrap();

        ensure_seeded_masters(project.path(), Some(legacy.path()));

        let claude_dir = master_dir_for_dest(project.path(), ".claude");
        assert_eq!(
            std::fs::read(claude_dir.join("settings.json")).unwrap(),
            b"LEGACY CLAUDE"
        );
        let codex_dir = master_dir_for_dest(project.path(), ".codex");
        assert_eq!(
            std::fs::read(codex_dir.join("config.toml")).unwrap(),
            b"LEGACY CODEX"
        );
        // .opencode: no legacy master -> embedded default staged.
        let opencode_dir = master_dir_for_dest(project.path(), ".opencode");
        let opencode_master = master("opencode");
        assert_eq!(
            std::fs::read(opencode_dir.join(opencode_master.files[0].rel_path)).unwrap(),
            opencode_master.files[0].bytes
        );
        // The legacy tree is untouched and re-running is a no-op (present).
        assert_eq!(
            std::fs::read(legacy_seed.join(".claude/settings.json")).unwrap(),
            b"LEGACY CLAUDE"
        );
        std::fs::write(legacy_seed.join(".claude/settings.json"), b"CHANGED").unwrap();
        ensure_seeded_masters(project.path(), Some(legacy.path()));
        assert_eq!(
            std::fs::read(claude_dir.join("settings.json")).unwrap(),
            b"LEGACY CLAUDE"
        );
    }

    #[test]
    fn primary_project_root_first_entry_wins_legacy_fallback_none() {
        let mut settings = AppSettings::default();
        assert_eq!(primary_project_root(&settings), None);

        // project_paths first non-empty trimmed entry wins, whitespace padded.
        settings.project_paths = vec![
            "  ".to_string(),
            "  C:\\first\\project  ".to_string(),
            "C:\\second\\project".to_string(),
        ];
        assert_eq!(
            primary_project_root(&settings),
            Some(PathBuf::from("C:\\first\\project"))
        );

        // Empty project_paths -> legacy project_path fallback.
        settings.project_paths.clear();
        assert_eq!(primary_project_root(&settings), None);
        settings.project_path = Some("  C:\\legacy\\project ".to_string());
        assert_eq!(
            primary_project_root(&settings),
            Some(PathBuf::from("C:\\legacy\\project"))
        );

        // Whitespace-only legacy path -> None.
        settings.project_path = Some("   ".to_string());
        assert_eq!(primary_project_root(&settings), None);
    }

    #[test]
    fn registered_project_roots_multi_and_legacy_only() {
        let mut settings = AppSettings::default();
        assert!(registered_project_roots(&settings).is_empty());

        settings.project_paths = vec![
            "  ".to_string(),
            "C:\\one".to_string(),
            "C:\\two".to_string(),
        ];
        assert_eq!(
            registered_project_roots(&settings),
            vec![PathBuf::from("C:\\one"), PathBuf::from("C:\\two")]
        );

        // Empty project_paths -> legacy project_path alone.
        settings.project_paths.clear();
        settings.project_path = Some("C:\\legacy".to_string());
        assert_eq!(
            registered_project_roots(&settings),
            vec![PathBuf::from("C:\\legacy")]
        );
        // Whitespace-only legacy path -> no roots.
        settings.project_path = Some(" ".to_string());
        assert!(registered_project_roots(&settings).is_empty());
    }

    #[test]
    fn load_catalog_for_settings_primary_wins_and_never_falls_back() {
        let primary = seed_dir();
        let ac_dir = primary.path().join(".ac");
        let settings = AppSettings {
            project_paths: vec![primary.path().to_string_lossy().to_string()],
            ..AppSettings::default()
        };

        // Primary file absent -> unavailable: never an embedded default, never
        // the legacy instance copy.
        let unavailable = load_catalog_for_settings(&settings)
            .expect_err("absent primary is unavailable, never a self-heal");
        assert_eq!(unavailable.code, "baseUnavailable");
        assert_eq!(
            unavailable.path,
            manifest_path(&ac_dir).display().to_string()
        );
        assert!(!catalog_dir(&ac_dir).exists());

        // Hand-edited primary file is observable (primary wins over everything).
        let custom = manifest_json(
            r##"[{"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        std::fs::create_dir_all(manifest_path(&ac_dir).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(&ac_dir), &custom).unwrap();
        let loaded = load_catalog_for_settings(&settings).expect("persisted primary catalog");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "custom");

        // Primary file DELETED -> unavailable again, never a legacy copy.
        std::fs::remove_file(manifest_path(&ac_dir)).unwrap();
        let unavailable =
            load_catalog_for_settings(&settings).expect_err("deleted primary is unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");
    }

    #[test]
    fn load_catalog_unavailable_display_carries_code_path_and_reason() {
        let dir = seed_dir();
        let unavailable = load_catalog(dir.path()).expect_err("absent catalog");
        let rendered = unavailable.to_string();
        assert!(rendered.contains("baseUnavailable"), "{rendered}");
        assert!(
            rendered.contains(&manifest_path(dir.path()).display().to_string()),
            "{rendered}"
        );
        assert!(rendered.contains("no persisted catalog"), "{rendered}");
        assert_eq!(
            rendered,
            format!(
                "catalog unavailable (baseUnavailable) at {}: {}",
                unavailable.path, unavailable.reason
            )
        );
    }

    #[test]
    fn load_catalog_for_settings_two_projects_switch_order_and_isolation() {
        let alpha = seed_dir();
        let beta = seed_dir();
        write_report_manifest(
            &alpha.path().join(".ac"),
            &manifest_json(
                r##"[{"key":"alpha-sentinel","label":"Alpha","description":"d","color":"#111","command":"alpha-sentinel","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["alpha update"]}]"##,
            ),
        );
        write_report_manifest(
            &beta.path().join(".ac"),
            &manifest_json(
                r##"[{"key":"beta-sentinel","label":"Beta","description":"d","color":"#222","command":"beta-sentinel","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["beta update"]}]"##,
            ),
        );
        let alpha_path = alpha.path().to_string_lossy().to_string();
        let beta_path = beta.path().to_string_lossy().to_string();

        let settings = AppSettings {
            project_paths: vec![alpha_path.clone(), beta_path.clone()],
            ..AppSettings::default()
        };
        let loaded =
            load_catalog_for_settings_with_config_dir(&settings, None).expect("alpha catalog");
        assert_eq!(keys_of(&loaded), ["alpha-sentinel"]);

        // Switch order: the first entry is primary and nothing is merged in.
        let switched = AppSettings {
            project_paths: vec![beta_path.clone(), alpha_path.clone()],
            ..AppSettings::default()
        };
        let loaded =
            load_catalog_for_settings_with_config_dir(&switched, None).expect("beta catalog");
        assert_eq!(keys_of(&loaded), ["beta-sentinel"]);

        // A failing primary never falls back to the second project.
        let missing = seed_dir();
        let failed = AppSettings {
            project_paths: vec![
                missing.path().to_string_lossy().to_string(),
                alpha_path.clone(),
            ],
            ..AppSettings::default()
        };
        let unavailable = load_catalog_for_settings_with_config_dir(&failed, None)
            .expect_err("failing primary is unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");
        assert_eq!(
            unavailable.path,
            manifest_path(&missing.path().join(".ac"))
                .display()
                .to_string()
        );
    }

    #[test]
    fn load_catalog_for_settings_project_path_fallback_and_no_project_instance() {
        let project = seed_dir();
        write_report_manifest(
            &project.path().join(".ac"),
            &manifest_json(
                r##"[{"key":"legacy-root","label":"Legacy root","description":"d","color":"#111","command":"legacy-root","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["legacy-root update"]}]"##,
            ),
        );
        let settings = AppSettings {
            project_paths: vec!["   ".to_string()],
            project_path: Some(format!("  {}  ", project.path().display())),
            ..AppSettings::default()
        };
        let loaded = load_catalog_for_settings_with_config_dir(&settings, None)
            .expect("trimmed legacy project_path catalog");
        assert_eq!(keys_of(&loaded), ["legacy-root"]);

        // No-project mode reads the existing instance catalog read-only.
        let instance = seed_dir();
        write_report_manifest(
            instance.path(),
            &manifest_json(
                r##"[{"key":"instance","label":"Instance","description":"d","color":"#111","command":"instance","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["instance update"]}]"##,
            ),
        );
        let before = std::fs::read(manifest_path(instance.path())).unwrap();
        let loaded = load_catalog_for_settings_with_config_dir(
            &AppSettings::default(),
            Some(instance.path().to_path_buf()),
        )
        .expect("instance catalog");
        assert_eq!(keys_of(&loaded), ["instance"]);
        assert_eq!(
            std::fs::read(manifest_path(instance.path())).unwrap(),
            before
        );

        // Absent instance catalog: unavailable, never another project, and
        // repeated reads create nothing.
        let absent = seed_dir();
        for _ in 0..2 {
            let unavailable = load_catalog_for_settings_with_config_dir(
                &AppSettings::default(),
                Some(absent.path().to_path_buf()),
            )
            .expect_err("absent instance catalog");
            assert_eq!(unavailable.code, "baseUnavailable");
        }
        assert!(!catalog_dir(absent.path()).exists());

        // Unresolvable config dir: unavailable with an empty path.
        let unavailable = load_catalog_for_settings_with_config_dir(&AppSettings::default(), None)
            .expect_err("unresolvable config dir");
        assert_eq!(unavailable.code, "baseUnavailable");
        assert!(unavailable.path.is_empty());
    }

    #[test]
    fn load_catalog_keeps_valid_entry_while_warning_about_a_legacy_one() {
        // A migrationPending warning never blocks usable persisted commands.
        let dir = seed_dir();
        write_report_manifest(
            dir.path(),
            &manifest_json(
                r##"[{"key":"legacy","label":"Legacy","description":"d","color":"#111","command":"legacy","envs":[],"isolatedHome":false,"removable":true},
                 {"key":"fresh","label":"Fresh","description":"d","color":"#222","command":"fresh","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["fresh update"]}]"##,
            ),
        );

        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(keys_of(&loaded), ["legacy", "fresh"]);
        assert!(loaded[0].update_commands.is_empty());
        assert_eq!(loaded[1].update_commands, vec!["fresh update".to_string()]);

        let report = report_for(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(report.warnings[0].code, "migrationPending");
    }

    #[test]
    fn ensure_seeded_for_project_skips_missing_or_relative_root_without_writes() {
        let base = seed_dir();
        // Missing root: nothing must be created.
        let missing = base.path().join("does-not-exist");
        ensure_seeded_for_project(&missing);
        assert!(!missing.exists());

        // Relative root: nothing must be created relative to the process CWD.
        let relative = Path::new("some-relative-project");
        ensure_seeded_for_project(relative);
        assert!(!relative.exists());
    }

    #[test]
    fn ensure_seeded_for_project_steady_state_precheck_skips_gate_and_writes() {
        let project = seed_dir();
        let root = project.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        // First run seeds everything under the (test) ungated path.
        ensure_seeded_for_project(&root);
        assert!(manifest_path(&root.join(".ac")).is_file());
        for m in EMBEDDED_SEED_MASTERS {
            assert!(master_dir_for_dest(&root.join(".ac"), m.dest).is_dir());
        }
        let manifest_bytes = std::fs::read(manifest_path(&root.join(".ac"))).unwrap();

        // Steady state: re-running must not touch a byte (and must not even
        // need the manifest to parse; the pre-check is existence-only).
        ensure_seeded_for_project(&root);
        assert_eq!(
            std::fs::read(manifest_path(&root.join(".ac"))).unwrap(),
            manifest_bytes
        );

        // A missing master dir is re-seeded by the next run (masters self-heal).
        let claude_dir = master_dir_for_dest(&root.join(".ac"), ".claude");
        std::fs::remove_dir_all(&claude_dir).unwrap();
        ensure_seeded_for_project(&root);
        assert!(claude_dir.is_dir());
    }

    #[test]
    fn managed_catalog_corrupt_instance_legacy_blocks_transfer_without_writes() {
        // An unreadable legacy source must not be "migrated" or trashed: the
        // transfer is refused, no base is published, and every byte stays put.
        let project = seed_dir();
        let legacy = legacy_dir();
        let garbage = b"{ this is not valid json".to_vec();
        std::fs::write(legacy.path().join("agents.json"), &garbage).unwrap();

        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
        let catalog = catalog_dir(project.path());
        assert!(
            !manifest_path(project.path()).exists(),
            "no base is published"
        );
        assert!(!catalog.join(MIGRATION_BACKUP_FILENAME).exists());
        assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
        assert!(!local_catalog_path(project.path()).exists());
        assert_eq!(
            std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
            garbage
        );
        let unavailable = load_catalog(project.path()).expect_err("no project base");
        assert_eq!(unavailable.code, "baseUnavailable");

        // Recovery: remove the corrupt source; the next initialization seeds.
        std::fs::remove_file(legacy.path().join("agents.10.default.json")).unwrap();
        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
        assert_eq!(
            load_catalog(project.path())
                .expect("fresh managed base")
                .len(),
            8
        );
    }

    #[test]
    fn legacy_masters_empty_dir_copied_verbatim_and_inert() {
        let project = seed_dir();
        let legacy = legacy_dir();
        let legacy_seed = legacy.path().join("_seed");
        std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap(); // EMPTY

        ensure_seeded_masters(project.path(), Some(legacy.path()));
        let claude_dir = master_dir_for_dest(project.path(), ".claude");
        assert!(
            claude_dir.is_dir(),
            "empty legacy master is copied verbatim"
        );
        assert!(
            !crate::config::config_seed::is_nonempty_seed_dir(&claude_dir),
            "present-but-empty master stays inert for the spawn tier"
        );
        assert_eq!(std::fs::read_dir(&claude_dir).unwrap().count(), 0);
        // The embedded default is NOT seeded (present = user-owned rule); the
        // Settings re-seed button restores it.
        reseed_master_for_command(project.path(), "claude").unwrap();
        assert!(crate::config::config_seed::is_nonempty_seed_dir(
            &claude_dir
        ));
    }

    #[test]
    fn reseed_with_no_primary_targets_legacy_config_dir() {
        // The Settings re-seed button must keep working on pre-migration
        // installs with zero registered projects: no primary root -> the legacy
        // `<config_dir>/coding-agents/_seed/<dest>` masters are the target.
        //
        // #2001: this fixture dir stands in for the per-binary `config_dir()`.
        // This test is the ONLY writer of the real `<config_dir>/coding-agents`
        // tree, and separate test processes share that path; an in-process lock
        // cannot serialize processes, so the real path is never resolved here.
        let config_dir = seed_dir();
        let legacy_master = config_dir
            .path()
            .join("coding-agents")
            .join("_seed")
            .join(".claude");
        std::fs::create_dir_all(&legacy_master).unwrap();
        std::fs::write(legacy_master.join("marker"), b"x").unwrap();

        let result = reseed_master_for_command(config_dir.path(), "claude");
        assert!(result.is_ok(), "legacy reseed works: {result:?}");
        let m = master("claude");
        assert_eq!(
            std::fs::read(legacy_master.join(m.files[0].rel_path)).unwrap(),
            m.files[0].bytes
        );
    }

    #[test]
    fn definition_defaults_update_commands_empty_auto_update_false_when_absent() {
        // An old agents.json (no new fields) parses with the documented
        // defaults: empty update commands, auto-update off.
        let json = r##"
        {
            "schemaVersion": 1,
            "agents": [
                {
                    "key": "old",
                    "label": "Old",
                    "description": "d",
                    "color": "#000",
                    "command": "old",
                    "envs": [],
                    "isolatedHome": false,
                    "removable": true
                }
            ]
        }
        "##;
        let parsed: CodingAgentCatalog = serde_json::from_str(json).expect("old manifest parses");
        let def = &parsed.agents[0];
        assert!(def.update_commands.is_empty());
        assert!(!def.auto_update);

        // camelCase round-trip: always serialize both fields.
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), json).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(loaded[0].update_commands.len(), 0);
        assert!(!loaded[0].auto_update);

        let mut def = loaded[0].clone();
        def.update_commands = vec!["claude --update".to_string()];
        def.auto_update = true;
        let round = serde_json::to_value(&def).expect("serialize def");
        assert_eq!(
            round["updateCommands"],
            serde_json::json!(["claude --update"])
        );
        assert_eq!(round["autoUpdate"], serde_json::json!(true));
    }

    #[test]
    fn load_catalog_never_invents_update_commands_for_legacy_entries() {
        // A catalog seeded before the update-command era (#1325) carries no
        // updateCommands. #1967 P4: nothing is backfilled in memory - the
        // commands stay empty until the P5 migration persists defaults - the
        // persisted bytes are untouched, and the report explains the pending
        // migration per entry.
        let json = manifest_json(
            r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true},
             {"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().all(|def| def.update_commands.is_empty()));
        // No-write proof: the manifest bytes are identical before/after the load.
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            json.as_bytes()
        );

        let report = report_for(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.warnings.len(), 2);
        assert!(report
            .warnings
            .iter()
            .all(|warning| warning.code == "migrationPending"));
    }

    #[test]
    fn load_catalog_preserves_persisted_update_commands() {
        // User-authored sequences are data: they are served exactly as written.
        let json = manifest_json(
            r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["claude --custom"]}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(
            loaded[0].update_commands,
            vec!["claude --custom".to_string()]
        );
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            json.as_bytes()
        );
    }

    #[test]
    fn load_catalog_custom_command_without_sequence_stays_empty() {
        // Custom command `bob` has no persisted sequence -> stays empty (never
        // prompted nor updated). No embedded table is consulted.
        let json = manifest_json(
            r##"[{"key":"bob","label":"Bob","description":"d","color":"#333","command":"bob","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].update_commands.is_empty());
    }

    #[test]
    fn load_catalog_duplicate_commands_keep_their_own_entries() {
        // Two profiles share the `pi` command under different keys; the persisted
        // data is served per entry. The effective (first) entry rule lives in
        // `agent_update`, which never borrows the second entry's sequence.
        let json = manifest_json(
            r##"[{"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true},
             {"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().all(|def| def.update_commands.is_empty()));
    }

    #[test]
    fn load_catalog_mixed_duplicate_sequences_keep_persisted_values() {
        // Data-level preservation: the FIRST `pi` entry stays empty and the
        // SECOND keeps its custom sequence untouched. The read never rewrites
        // one entry from another (the effective first-entry rule is applied by
        // the updater, not by the read).
        let json = manifest_json(
            r##"[{"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true},
             {"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["pi --custom"]}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(loaded.len(), 2);
        assert!(loaded[0].update_commands.is_empty());
        assert_eq!(loaded[1].update_commands, vec!["pi --custom".to_string()]);
    }

    #[test]
    fn embedded_default_ships_update_commands_for_all_but_cursor_and_muse() {
        // #1318/#1325/#1546 drift guard: claude, pi, codex, hermes, opencode,
        // and antigravity ship the update command; cursor, grok and Muse ship
        // none; every entry defaults autoUpdate to false.
        let catalog = embedded_default_catalog();
        assert_eq!(catalog.agents.len(), 9);
        for def in &catalog.agents {
            assert!(
                !def.auto_update,
                "{} autoUpdate must default false",
                def.key
            );
            match def.key.as_str() {
                "claude" => assert_eq!(def.update_commands, vec!["claude --update".to_string()]),
                "pi" => assert_eq!(def.update_commands, vec!["pi update".to_string()]),
                "codex" => assert_eq!(def.update_commands, vec!["codex update".to_string()]),
                "hermes" => {
                    assert_eq!(def.update_commands, vec!["hermes update --yes".to_string()])
                }
                "opencode" => assert_eq!(def.update_commands, vec!["opencode upgrade".to_string()]),
                "antigravity" => assert_eq!(def.update_commands, vec!["agy update".to_string()]),
                "cursor" | "grok" | "muse" => assert!(
                    def.update_commands.is_empty(),
                    "{} must ship no update command",
                    def.key
                ),
                other => panic!("unexpected key {other:?}"),
            }
        }
    }

    // ---- #1912 support switch: read gate + seed gate --------------------

    fn assert_no_key(agents: &[CodingAgentDefinition], key: &str) {
        assert!(!agents.iter().any(|a| a.key == key), "{key} must be absent");
    }

    fn keys_of(agents: &[CodingAgentDefinition]) -> Vec<&str> {
        agents.iter().map(|a| a.key.as_str()).collect()
    }

    #[test]
    fn builtin_agent_support_table_matches_embedded_default_keys_in_order() {
        // R1: the table and the embedded JSON must list the same keys in the
        // same order (exact order implies set equality both ways and no dupes).
        let table_keys: Vec<&str> = BUILTIN_AGENT_SUPPORT.iter().map(|(k, _)| *k).collect();
        let catalog = embedded_default_catalog();
        let json_keys: Vec<&str> = catalog.agents.iter().map(|def| def.key.as_str()).collect();
        assert_eq!(table_keys, json_keys);
    }

    #[test]
    fn builtin_agent_support_ships_only_muse_disabled() {
        // R2: #1999 flips muse off; every other row still ships enabled.
        let disabled: Vec<&str> = BUILTIN_AGENT_SUPPORT
            .iter()
            .filter(|(_, on)| !*on)
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(disabled, ["muse"], "only muse ships disabled");
    }

    #[test]
    fn support_override_scopes_to_closure_and_restores_shipped_table() {
        // R3: the override reaches ensure_seeded + load_catalog and restores the
        // shipped table.
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let before = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(before.len(), 8);
        assert_no_key(&before, "muse");

        with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
            assert!(
                ensure_seeded(dir.path(), None).is_some(),
                "the table is part of the managed revision"
            );
            let inside = load_catalog(dir.path()).expect("seeded catalog");
            assert_eq!(inside.len(), 9);
            assert!(inside.iter().any(|a| a.key == "muse"));
        });

        let after = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(after.len(), 8);
        assert_no_key(&after, "muse");
    }

    #[test]
    fn desupported_row_dropped_from_seeded_manifest_and_corrupt_is_unavailable() {
        // R4 + #1967 P4: the read gate drops a de-supported row from a seeded
        // manifest; a corrupt persisted file is `baseInvalid` (no self-heal) and
        // its bytes are preserved.
        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let dir = seed_dir();
            ensure_seeded(dir.path(), None);
            let agents = load_catalog(dir.path()).expect("seeded catalog");
            assert_eq!(agents.len(), 8);
            assert_no_key(&agents, "muse");
            assert_eq!(agents[0].key, "claude");

            let dir = seed_dir();
            let path = manifest_path(dir.path());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let garbage = b"{ this is not valid json";
            std::fs::write(&path, garbage).unwrap();
            let unavailable = load_catalog(dir.path()).expect_err("corrupt catalog is unavailable");
            assert_eq!(unavailable.code, "baseInvalid");
            assert_eq!(std::fs::read(&path).unwrap(), garbage);
        });
    }

    #[test]
    fn desupported_row_dropped_from_parsed_manifest_and_dedup_cannot_resurrect_it() {
        // R5: a duplicate-bearing user manifest cannot resurrect a de-supported
        // key; a user entry with ANOTHER key and the same command is kept.
        let agents = r##"[
            {"key":"muse","label":"First","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
            {"key":"muse","label":"Second","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
            {"key":"mine","label":"Mine","description":"d","color":"#111","command":"muse","envs":[],"isolatedHome":false,"removable":true}
        ]"##;

        // Control: the all-enabled table keeps muse -> duplicate key dedups
        // first-wins, custom key kept.
        with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
            let dir = seed_dir();
            std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
            std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
            let loaded = load_catalog(dir.path()).expect("persisted catalog");
            assert_eq!(keys_of(&loaded), ["muse", "mine"]);
            assert_eq!(loaded[0].label, "First");
        });

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let dir = seed_dir();
            std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
            std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
            let loaded = load_catalog(dir.path()).expect("persisted catalog");
            assert_eq!(keys_of(&loaded), ["mine"]);
            assert_eq!(loaded[0].command, "muse");
        });
    }

    #[test]
    fn desupported_row_dropped_from_load_catalog_for_settings_primary() {
        // R6: the settings read root (primary project) honors the read gate on
        // the persisted file path, and an absent primary is unavailable.
        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let primary = seed_dir();
            let ac_dir = primary.path().join(".ac");
            let settings = AppSettings {
                project_paths: vec![primary.path().to_string_lossy().to_string()],
                ..AppSettings::default()
            };

            let unavailable =
                load_catalog_for_settings(&settings).expect_err("absent primary is unavailable");
            assert_eq!(unavailable.code, "baseUnavailable");

            let user_file = manifest_json(
                r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
                {"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
            );
            std::fs::create_dir_all(manifest_path(&ac_dir).parent().unwrap()).unwrap();
            std::fs::write(manifest_path(&ac_dir), &user_file).unwrap();
            let loaded = load_catalog_for_settings(&settings).expect("persisted primary");
            assert_eq!(keys_of(&loaded), ["custom"]);
        });
    }

    #[test]
    fn no_embedded_donor_backfills_a_custom_key_bound_to_a_builtin_command() {
        // #1967 P4: the embedded default is no longer a runtime command donor.
        // A custom key bound to the builtin `claude` command, in a manifest with
        // no updateCommands, reads as empty and the persisted bytes are kept;
        // the de-supported table changes nothing about that.
        let manifest = manifest_json(
            r##"[{"key":"my-claude","label":"My Claude","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true}]"##,
        );

        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &manifest).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert!(loaded[0].update_commands.is_empty());
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            manifest.as_bytes()
        );

        with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
            let dir = seed_dir();
            std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
            std::fs::write(manifest_path(dir.path()), &manifest).unwrap();
            let loaded = load_catalog(dir.path()).expect("persisted catalog");
            assert!(loaded[0].update_commands.is_empty());
        });
    }

    #[test]
    fn managed_catalog_fresh_base_bytes_carry_only_enabled_rows() {
        // The fresh managed base carries the enabled shipped rows plus the
        // ownership marker: the shipped table seeds the enabled 8 (grok in,
        // muse out) and its mirror override seeds the same 8 while the read
        // gate hides the key.
        let dir = seed_dir();
        assert!(ensure_seeded(dir.path(), None).is_some());
        let bytes = std::fs::read(manifest_path(dir.path())).unwrap();
        let base: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(base["agents"].as_array().unwrap().len(), 8);
        assert_eq!(base["managed"]["owner"], "agentscommander");
        assert!(
            base["agents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|agent| agent["key"] == "grok"),
            "the fresh base publishes grok"
        );

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let dir = seed_dir();
            assert!(
                ensure_seeded(dir.path(), None).is_some(),
                "a first seed publishes"
            );
            let bytes = std::fs::read(manifest_path(dir.path())).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains("\"muse\""),
                "seeded bytes must not carry the de-supported key"
            );
            let catalog: CodingAgentCatalog =
                serde_json::from_slice(&bytes).expect("seeded manifest parses");
            assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
            assert_eq!(catalog.agents.len(), 8);
            let loaded = load_catalog(dir.path()).expect("seeded catalog");
            assert_eq!(loaded.len(), 8);
            assert_no_key(&loaded, "muse");
        });
    }

    #[test]
    fn managed_catalog_migration_backup_keeps_a_desupported_key_verbatim() {
        // Under a false support row a migrated legacy source is preserved
        // byte-for-byte in the backup; the read gate hides the de-supported key
        // from the effective catalog while the custom key survives.
        let legacy_bytes = manifest_json(
            r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
            {"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
        )
        .into_bytes();

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let project = seed_dir();
            let legacy = legacy_dir();
            std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();
            assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
            assert_eq!(
                std::fs::read(catalog_dir(project.path()).join(MIGRATION_BACKUP_FILENAME)).unwrap(),
                legacy_bytes,
                "the backup is user data: byte-for-byte even with a de-supported key"
            );
            let loaded = load_catalog(project.path()).expect("migrated catalog");
            assert_eq!(keys_of(&loaded), ["custom"]);
        });
    }

    #[test]
    fn managed_catalog_false_row_is_part_of_the_revision_and_refreshes_only_the_base() {
        // #1968/#1999: the shipped false muse row is part of the managed
        // revision, so a base seeded all-enabled refreshes to the enabled
        // shipped set; the user-owned local layer is never touched and the key
        // stays hidden either way.
        let dir = seed_dir();
        let seeded = with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
            ensure_seeded(dir.path(), None);
            assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 9);
            std::fs::read(manifest_path(dir.path())).unwrap()
        });
        let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "a support-gate change is part of the managed revision"
        );
        assert_ne!(std::fs::read(manifest_path(dir.path())).unwrap(), seeded);
        assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 8);
        assert_eq!(
            std::fs::read(local_catalog_path(dir.path())).unwrap(),
            local_before,
            "the user-owned local layer is never touched by a refresh"
        );
        let loaded = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(loaded.len(), 8);
        assert_no_key(&loaded, "muse");

        // Back under the all-enabled table the base returns byte-for-byte.
        with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
            assert!(ensure_seeded(dir.path(), None).is_some());
            assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), seeded);
        });
    }

    #[test]
    fn desupported_master_not_seeded_not_reseedable_and_reseed_refused() {
        // R11: a de-supported key's master is not seeded, is not listed as
        // reseedable, and its re-seed is refused server-side; the other masters
        // are unaffected.
        let dir = seed_dir();
        ensure_seeded_masters(dir.path(), None);
        for dest in [".claude", ".codex", ".opencode"] {
            assert!(
                crate::config::config_seed::is_nonempty_seed_dir(&master_dir_for_dest(
                    dir.path(),
                    dest
                )),
                "{dest} master should be non-empty"
            );
        }
        let mut got = reseedable_command_basenames();
        got.sort();
        assert_eq!(got, vec!["claude", "codex", "opencode"]);

        with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
            let dir = seed_dir();
            ensure_seeded_masters(dir.path(), None);
            assert!(
                !master_dir_for_dest(dir.path(), ".claude").exists(),
                "de-supported master must not be seeded"
            );
            for dest in [".codex", ".opencode"] {
                assert!(
                    crate::config::config_seed::is_nonempty_seed_dir(&master_dir_for_dest(
                        dir.path(),
                        dest
                    )),
                    "{dest} master must still be seeded"
                );
            }
            let mut got = reseedable_command_basenames();
            got.sort();
            assert_eq!(got, vec!["codex", "opencode"]);
            let err = reseed_master_for_command(dir.path(), "claude").unwrap_err();
            assert!(err.contains("not a recognized built-in"), "{err}");
            assert!(reseed_master_for_command(dir.path(), "codex").is_ok());
        });
    }

    #[test]
    fn desupported_master_skips_legacy_tree_copy_too() {
        // R12: the legacy `_seed/<dest>` tree copy is skipped for a de-supported
        // key; supported masters are still copied verbatim; the legacy tree is
        // untouched.
        let project = seed_dir();
        let legacy = legacy_dir();
        let legacy_seed = legacy.path().join("_seed");
        std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap();
        std::fs::create_dir_all(legacy_seed.join(".codex")).unwrap();
        std::fs::write(legacy_seed.join(".claude/settings.json"), b"LEGACY CLAUDE").unwrap();
        std::fs::write(legacy_seed.join(".codex/config.toml"), b"LEGACY CODEX").unwrap();

        with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
            ensure_seeded_masters(project.path(), Some(legacy.path()));
            assert!(
                !master_dir_for_dest(project.path(), ".claude").exists(),
                "de-supported master must not be copied from the legacy tree"
            );
            assert_eq!(
                std::fs::read(master_dir_for_dest(project.path(), ".codex").join("config.toml"))
                    .unwrap(),
                b"LEGACY CODEX"
            );
            assert_eq!(
                std::fs::read(legacy_seed.join(".claude/settings.json")).unwrap(),
                b"LEGACY CLAUDE",
                "the legacy tree itself is never touched"
            );
        });
    }

    #[test]
    fn managed_catalog_all_masters_present_ignores_desupported_master() {
        // R13 (#1968): only the MASTER existence optimization remains, and it
        // requires only SUPPORTED masters, so a claude-off install is steady
        // without `.claude`. The catalog itself always validates under the
        // gate/lock; no existence-only shortcut may skip that (proven by the
        // steady-state test below).
        let project = seed_dir();
        let root = project.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let ac_dir = root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);

        with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
            ensure_seeded(&ac_dir, None);
            ensure_seeded_masters(&ac_dir, None);
            assert!(manifest_path(&ac_dir).is_file());
            assert!(!master_dir_for_dest(&ac_dir, ".claude").exists());
            assert!(crate::config::config_seed::is_nonempty_seed_dir(
                &master_dir_for_dest(&ac_dir, ".codex")
            ));
            assert!(crate::config::config_seed::is_nonempty_seed_dir(
                &master_dir_for_dest(&ac_dir, ".opencode")
            ));
            assert!(
                all_masters_present(&ac_dir),
                "steady inside the override: `.claude` must not be required"
            );

            // A missing SUPPORTED master makes the predicate false and is
            // re-seeded by the plain project entry point; `.claude` stays absent.
            let codex_dir = master_dir_for_dest(&ac_dir, ".codex");
            std::fs::remove_dir_all(&codex_dir).unwrap();
            assert!(!all_masters_present(&ac_dir));
            ensure_seeded_for_project(&root);
            assert!(crate::config::config_seed::is_nonempty_seed_dir(&codex_dir));
            assert!(!master_dir_for_dest(&ac_dir, ".claude").exists());
            assert!(all_masters_present(&ac_dir));
        });

        // Outside the override the same tree has no `.claude`: the shipped
        // table requires it and the predicate is false.
        assert!(!all_masters_present(&ac_dir));
    }

    #[test]
    fn managed_catalog_steady_state_takes_the_gate_and_keeps_bytes() {
        // #1968: the existence-only catalog shortcut is gone. A second
        // tokenized initialization takes the project gate (lock file appears)
        // and the catalog lock, yet an unedited base at the shipped revision is
        // NOT rewritten: byte identity is preserved.
        let project = seed_dir();
        let root = project.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let ac_dir = root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
        ensure_seeded(&ac_dir, None);
        ensure_seeded_masters(&ac_dir, None);

        let base_bytes = std::fs::read(manifest_path(&ac_dir)).unwrap();
        let local_bytes = std::fs::read(local_catalog_path(&ac_dir)).unwrap();
        let gate_lock = ac_dir.join(crate::config::seed_manifest::SEED_MANIFEST_LOCK_FILENAME);
        assert!(!gate_lock.exists(), "no gate lock from the ungated seed");

        let token = ManifestActivationToken::for_test();
        ensure_seeded_for_project_with_token(&root, Some(&token));
        assert!(
            gate_lock.is_file(),
            "the catalog has no existence shortcut: the gate must have been taken"
        );
        assert_eq!(
            std::fs::read(manifest_path(&ac_dir)).unwrap(),
            base_bytes,
            "an unedited base at the shipped revision is never rewritten"
        );
        assert_eq!(
            std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
            local_bytes,
            "the user-owned local layer is never rewritten"
        );

        // Second tokenized run: still byte-identical.
        ensure_seeded_for_project_with_token(&root, Some(&token));
        assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_bytes);
        assert_eq!(
            std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
            local_bytes
        );
    }

    #[test]
    fn already_seeded_master_never_removed_by_a_false_row() {
        // R14: an already-present master is user-owned and never removed by a
        // false row; only the reseedable list reflects the switch.
        let dir = seed_dir();
        ensure_seeded_masters(dir.path(), None);
        let claude_file = master_dir_for_dest(dir.path(), ".claude").join("settings.json");
        let seeded = std::fs::read(&claude_file).unwrap();

        with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
            ensure_seeded_masters(dir.path(), None);
            assert_eq!(
                std::fs::read(&claude_file).unwrap(),
                seeded,
                "an already-present master is never removed or rewritten"
            );
            let mut got = reseedable_command_basenames();
            got.sort();
            assert_eq!(got, vec!["codex", "opencode"]);
        });
    }

    #[test]
    fn managed_catalog_fresh_base_bytes_carry_only_enabled_rows_with_nonregular_legacy() {
        // A non-regular instance source is NOT a source: fresh initialization
        // runs, and under a false support row only the enabled rows land in the
        // managed base.
        for shape in ["absent", "directory"] {
            let project = seed_dir();
            let legacy = legacy_dir();
            if shape == "directory" {
                std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
            }
            assert!(
                ensure_seeded(project.path(), Some(legacy.path())).is_some(),
                "{shape}: a first seed publishes"
            );
            let bytes = std::fs::read(manifest_path(project.path())).unwrap();
            let catalog: CodingAgentCatalog =
                serde_json::from_slice(&bytes).expect("seeded manifest parses");
            assert_eq!(catalog.agents.len(), 8, "{shape}: 8 agents seeded");

            with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
                let project = seed_dir();
                let legacy = legacy_dir();
                if shape == "directory" {
                    std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
                }
                assert!(
                    ensure_seeded(project.path(), Some(legacy.path())).is_some(),
                    "{shape}: a first seed publishes"
                );
                let bytes = std::fs::read(manifest_path(project.path())).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                assert!(
                    !text.contains("\"muse\""),
                    "{shape}: seeded bytes must not carry the de-supported key"
                );
                let catalog: CodingAgentCatalog =
                    serde_json::from_slice(&bytes).expect("seeded manifest parses");
                assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
                assert_eq!(catalog.agents.len(), 8, "{shape}: 8 agents seeded");
                let loaded = load_catalog(project.path()).expect("persisted catalog");
                assert_eq!(loaded.len(), 8);
                assert_no_key(&loaded, "muse");
                assert_eq!(loaded[0].key, "claude");
            });
        }
    }

    // ---- #1963 P1: persisted-only catalog report --------------------------

    fn write_report_manifest(ac_dir: &Path, contents: &str) {
        let path = manifest_path(ac_dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }

    fn report_for(dir: &Path) -> CatalogReport {
        load_catalog_report(dir)
    }

    fn report_json(report: &CatalogReport) -> String {
        serde_json::to_string(report).expect("serialize report")
    }

    #[test]
    fn catalog_report_serializes_exact_camel_case_wire_shape() {
        let dir = seed_dir();
        write_report_manifest(
            dir.path(),
            &manifest_json(
                r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
            ),
        );
        let report = report_for(dir.path());
        assert!(report.unavailable.is_none());
        assert!(report.warnings.is_empty());
        let value = serde_json::to_value(&report).expect("report value");
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "catalog",
                "primaryProjectRoot",
                "sourcePath",
                "unavailable",
                "warnings"
            ]
        );
        assert_eq!(value["primaryProjectRoot"], serde_json::Value::Null);
        assert_eq!(value["unavailable"], serde_json::Value::Null);
        assert_eq!(
            value["sourcePath"],
            serde_json::json!(manifest_path(dir.path()).display().to_string())
        );
        assert_eq!(value["catalog"][0]["key"], "mine");
        assert_eq!(value["catalog"][0]["updateCommands"], serde_json::json!([]));
        let back: CatalogReport = serde_json::from_value(value).expect("round trip");
        assert_eq!(back, report);
    }

    #[test]
    fn catalog_report_missing_manifest_is_unavailable_without_creating_anything() {
        let dir = seed_dir();
        let before: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        let expected = manifest_path(dir.path()).display().to_string();
        let report = report_for(dir.path());
        assert!(report.catalog.is_empty());
        assert_eq!(report.source_path.as_deref(), Some(expected.as_str()));
        let unavailable = report.unavailable.as_ref().expect("unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");
        assert_eq!(unavailable.path, expected);
        // Nothing was created, not even the catalog directory.
        assert!(!catalog_dir(dir.path()).exists());
        let after: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn catalog_report_unreadable_path_is_unavailable() {
        let dir = seed_dir();
        // A regular file where the catalog DIRECTORY should be makes the
        // manifest path unreadable rather than merely missing.
        std::fs::write(dir.path().join("coding-agents"), b"not a directory").unwrap();
        let report = report_for(dir.path());
        assert!(report.catalog.is_empty());
        let unavailable = report.unavailable.as_ref().expect("unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");
        assert_eq!(
            unavailable.path,
            manifest_path(dir.path()).display().to_string()
        );
    }

    #[test]
    fn catalog_report_corrupt_json_is_base_invalid_and_preserves_bytes() {
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        let garbage = b"{ this is not valid json";
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, garbage).unwrap();

        let report = report_for(dir.path());
        assert!(report.catalog.is_empty());
        let unavailable = report.unavailable.as_ref().expect("unavailable");
        assert_eq!(unavailable.code, "baseInvalid");
        assert_eq!(unavailable.path, path.display().to_string());
        assert_eq!(std::fs::read(&path).unwrap(), garbage);
    }

    #[test]
    fn catalog_report_directory_at_manifest_is_unavailable() {
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(&path).unwrap();
        let report = report_for(dir.path());
        assert!(report.catalog.is_empty());
        assert_eq!(
            report.unavailable.as_ref().expect("unavailable").code,
            "baseUnavailable"
        );
    }

    #[cfg(windows)]
    fn create_manifest_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[cfg(unix)]
    fn create_manifest_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[test]
    fn catalog_report_symlink_manifest_is_unavailable() {
        let dir = seed_dir();
        let target = dir.path().join("real-catalog.json");
        std::fs::write(
            &target,
            manifest_json(
                r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
            ),
        )
        .unwrap();
        let link = manifest_path(dir.path());
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        if let Err(e) = create_manifest_symlink(&target, &link) {
            // Explicit, never silent: the link fixture could not be created on
            // this host (Windows symlink privilege), so the link assertion was
            // NOT exercised and must not be counted as a pass.
            eprintln!(
                "[catalog-report test] symlink creation unavailable on this host ({e}); link fixture NOT exercised"
            );
            return;
        }
        let report = report_for(dir.path());
        assert!(report.catalog.is_empty());
        let unavailable = report.unavailable.as_ref().expect("unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");
        assert!(
            unavailable.reason.contains("symbolic link"),
            "{}",
            unavailable.reason
        );
    }

    #[test]
    fn catalog_report_unsupported_schema_is_base_invalid() {
        for raw in [
            r##"{"schemaVersion":2,"agents":[]}"##,
            r##"{"schemaVersion":"1","agents":[]}"##,
            r##"{"schemaVersion":null,"agents":[]}"##,
        ] {
            let dir = seed_dir();
            write_report_manifest(dir.path(), raw);
            let report = report_for(dir.path());
            assert!(report.catalog.is_empty(), "raw={raw}");
            let unavailable = report.unavailable.as_ref().expect("unavailable");
            assert_eq!(unavailable.code, "baseInvalid", "raw={raw}");
            assert!(
                unavailable.reason.contains("schemaVersion"),
                "raw={raw}: {}",
                unavailable.reason
            );
        }
    }

    #[test]
    fn catalog_report_missing_schema_version_and_empty_catalog_are_success() {
        for raw in [
            r##"{}"##,
            r##"{"schemaVersion":1}"##,
            r##"{"schemaVersion":1,"agents":[]}"##,
        ] {
            let dir = seed_dir();
            write_report_manifest(dir.path(), raw);
            let report = report_for(dir.path());
            assert!(report.unavailable.is_none(), "raw={raw}");
            assert!(report.catalog.is_empty(), "raw={raw}");
            assert!(report.warnings.is_empty(), "raw={raw}");
            assert_eq!(
                std::fs::read(manifest_path(dir.path())).unwrap(),
                raw.as_bytes()
            );
        }
    }

    #[test]
    fn catalog_report_invalid_root_shape_is_base_invalid() {
        for raw in [
            r##"[]"##,
            r##""text""##,
            r##"{"schemaVersion":1,"agents":{}}"##,
            r##"{"schemaVersion":1,"agents":"nope"}"##,
        ] {
            let dir = seed_dir();
            write_report_manifest(dir.path(), raw);
            let report = report_for(dir.path());
            assert!(report.catalog.is_empty(), "raw={raw}");
            assert_eq!(
                report.unavailable.as_ref().expect("unavailable").code,
                "baseInvalid",
                "raw={raw}"
            );
        }
    }

    #[test]
    fn catalog_report_missing_update_commands_warns_without_donor() {
        let dir = seed_dir();
        write_report_manifest(
            dir.path(),
            &manifest_json(
                r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true},
                 {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true},
                 {"key":"claude-beta","label":"Claude Beta","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true}]"##,
            ),
        );
        let report = report_for(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(
            report
                .catalog
                .iter()
                .map(|d| d.key.as_str())
                .collect::<Vec<_>>(),
            ["claude", "mine", "claude-beta"]
        );
        assert!(report.catalog.iter().all(|d| d.update_commands.is_empty()));
        assert_eq!(report.warnings.len(), 3);
        let source = report.source_path.clone().expect("source path");
        for (row, warning) in report.catalog.iter().zip(report.warnings.iter()) {
            assert_eq!(warning.code, "migrationPending");
            assert_eq!(warning.path, source);
            assert!(warning.reason.contains(&row.key), "{}", warning.reason);
            assert!(
                warning.reason.contains("suppressed during reads"),
                "{}",
                warning.reason
            );
            assert!(
                warning.reason.contains("managed-catalog migration"),
                "{}",
                warning.reason
            );
        }
        // No embedded command may leak into a persisted-only report.
        let text = report_json(&report);
        for sentinel in [
            "claude --update",
            "pi update",
            "codex update",
            "hermes update --yes",
            "opencode upgrade",
            "agy update",
        ] {
            assert!(
                !text.contains(sentinel),
                "embedded command leaked into the report: {sentinel}"
            );
        }
    }

    #[test]
    fn catalog_report_explicit_empty_has_no_missing_commands_warning() {
        let missing = manifest_json(
            r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true},
             {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true},
             {"key":"claude-beta","label":"Claude Beta","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        let explicit = manifest_json(
            r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
             {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
             {"key":"claude-beta","label":"Claude Beta","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        );
        let missing_dir = seed_dir();
        write_report_manifest(missing_dir.path(), &missing);
        let reported_missing = report_for(missing_dir.path());
        let explicit_dir = seed_dir();
        write_report_manifest(explicit_dir.path(), &explicit);
        let reported_explicit = report_for(explicit_dir.path());

        // Missing versus []: the resolved catalogs are identical; only the
        // missing field emits migrationPending.
        assert_eq!(reported_missing.catalog, reported_explicit.catalog);
        assert_eq!(reported_missing.warnings.len(), 3);
        assert!(reported_explicit.warnings.is_empty());
        assert!(reported_explicit
            .catalog
            .iter()
            .all(|d| d.update_commands.is_empty()));
        assert!(report_json(&reported_explicit).contains(r##""updateCommands":[]"##));

        // Custom and changed commands are preserved exactly, warn-free.
        let custom_dir = seed_dir();
        write_report_manifest(
            custom_dir.path(),
            &manifest_json(
                r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["mytool up --channel beta"]},
                 {"key":"claude-beta","label":"Claude Beta","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["claude update"]}]"##,
            ),
        );
        let reported_custom = report_for(custom_dir.path());
        assert!(reported_custom.warnings.is_empty());
        assert_eq!(
            reported_custom.catalog[0].update_commands,
            vec!["mytool up --channel beta".to_string()]
        );
        assert_eq!(
            reported_custom.catalog[1].update_commands,
            vec!["claude update".to_string()]
        );
        // Serialized transport parity: the wire value round-trips unchanged.
        let value = serde_json::to_value(&reported_custom).expect("report value");
        let back: CatalogReport = serde_json::from_value(value).expect("round trip");
        assert_eq!(back, reported_custom);
    }

    #[test]
    fn catalog_report_duplicate_keys_warn_first_wins() {
        let dir = seed_dir();
        write_report_manifest(
            dir.path(),
            &manifest_json(
                r##"[{"key":"mine","label":"First","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
                 {"key":"mine","label":"Second","description":"d","color":"#222","command":"mytool2","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
            ),
        );
        let report = report_for(dir.path());
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].label, "First");
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(report.warnings[0].code, "duplicateKey");
        assert!(
            report.warnings[0].reason.contains("first entry wins"),
            "{}",
            report.warnings[0].reason
        );
    }

    #[test]
    fn catalog_report_invalid_definitions_drop_rows_without_echoing_commands() {
        let dir = seed_dir();
        write_report_manifest(
            dir.path(),
            &manifest_json(
                r##"[{"key":"BAD KEY","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
                 {"key":"cmd-continue","label":"x","description":"d","color":"#000","command":"claude --continue","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
                 {"key":"nonstring","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["ok",123]},
                 {"key":"blank","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["   "]},
                 {"key":"ctrl","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["a\u0007b"]},
                 {"key":"line-sep","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["a\u2028b"]},
                 {"key":"para-sep","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["a\u2029b"]},
                 {"key":"full","label":"Full","description":"d","color":"#333","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["mytool --flag 'a b' && echo done"]}]"##,
            ),
        );
        let report = report_for(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].key, "full");
        assert_eq!(
            report.catalog[0].update_commands,
            vec!["mytool --flag 'a b' && echo done".to_string()]
        );
        assert_eq!(report.warnings.len(), 7);
        assert!(report
            .warnings
            .iter()
            .all(|w| w.code == "invalidDefinition"));
        let text = report_json(&report);
        for leaked in [
            "BAD KEY",
            "claude --continue",
            "a\u{7}b",
            "a\u{2028}b",
            "a\u{2029}b",
        ] {
            assert!(
                !text.contains(leaked),
                "offending text leaked into the report: {leaked:?}"
            );
        }
        // The valid row's legal full command is present, preserved exactly.
        assert!(text.contains("mytool --flag 'a b' && echo done"));
    }

    #[test]
    fn catalog_report_unknown_fields_warn_separately_without_values() {
        let dir = seed_dir();
        write_report_manifest(
            dir.path(),
            r##"{"schemaVersion":1,"owner":"secret-root-value","agents":[
                {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[{"key":"GOOD_KEY","value":"secret-env-value","enabled":true,"mystery":"secret-env-extra"}],"isolatedHome":false,"removable":true,"updateCommands":[],"customField":"secret-row-value","configSeed":{"enabled":true,"dest":".mine","extra":"secret-seed-value"}}]}"##,
        );
        let report = report_for(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].envs.len(), 1);
        assert_eq!(report.catalog[0].envs[0].value, "secret-env-value");
        let codes: Vec<&str> = report.warnings.iter().map(|w| w.code.as_str()).collect();
        assert!(codes.iter().all(|c| *c == "migrationPending"), "{codes:?}");
        assert_eq!(
            codes.len(),
            4,
            "{:?}",
            report
                .warnings
                .iter()
                .map(|w| &w.reason)
                .collect::<Vec<_>>()
        );
        let joined = report
            .warnings
            .iter()
            .map(|w| w.reason.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("owner"), "{joined}");
        assert!(joined.contains("customField"), "{joined}");
        assert!(joined.contains("extra"), "{joined}");
        assert!(joined.contains("mystery"), "{joined}");
        let text = report_json(&report);
        for secret in [
            "secret-root-value",
            "secret-row-value",
            "secret-seed-value",
            "secret-env-value",
            "secret-env-extra",
        ] {
            // The env VALUE is legitimately part of the resolved catalog; it must
            // never appear in a DIAGNOSTIC (no paths, no values in reasons).
            assert!(
                !joined.contains(secret),
                "value leaked into a diagnostic: {secret}"
            );
        }
        for unknown_value in [
            "secret-root-value",
            "secret-row-value",
            "secret-seed-value",
            "secret-env-extra",
        ] {
            assert!(
                !text.contains(unknown_value),
                "unknown-field value leaked into the report: {unknown_value}"
            );
        }
    }

    #[test]
    fn catalog_report_two_project_isolation_and_no_fallback_after_failure() {
        let primary = seed_dir();
        let secondary = seed_dir();
        write_report_manifest(
            &primary.path().join(".ac"),
            &manifest_json(
                r##"[{"key":"primary","label":"Primary","description":"d","color":"#111","command":"primary","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
            ),
        );
        write_report_manifest(
            &secondary.path().join(".ac"),
            &manifest_json(
                r##"[{"key":"secondary","label":"Secondary","description":"d","color":"#222","command":"secondary","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
            ),
        );
        let primary_str = primary.path().to_string_lossy().to_string();
        let settings = AppSettings {
            project_paths: vec![
                primary_str.clone(),
                secondary.path().to_string_lossy().to_string(),
            ],
            ..AppSettings::default()
        };
        let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
        assert_eq!(
            report.primary_project_root.as_deref(),
            Some(primary_str.as_str())
        );
        assert_eq!(
            report.source_path.as_deref(),
            Some(
                manifest_path(&primary.path().join(".ac"))
                    .display()
                    .to_string()
                    .as_str()
            )
        );
        assert_eq!(
            report
                .catalog
                .iter()
                .map(|d| d.key.as_str())
                .collect::<Vec<_>>(),
            ["primary"]
        );
        assert!(!report_json(&report).contains("secondary"));

        // A failure in the primary project is never retried against the second.
        std::fs::remove_file(manifest_path(&primary.path().join(".ac"))).unwrap();
        let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
        assert!(report.catalog.is_empty());
        assert_eq!(
            report.unavailable.as_ref().expect("unavailable").code,
            "baseUnavailable"
        );
        assert!(!report_json(&report).contains("secondary"));
    }

    #[test]
    fn catalog_report_project_path_fallback_selects_trimmed_legacy_root() {
        let dir = seed_dir();
        write_report_manifest(
            &dir.path().join(".ac"),
            &manifest_json(
                r##"[{"key":"legacy","label":"Legacy","description":"d","color":"#111","command":"legacy","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
            ),
        );
        let trimmed = dir.path().to_string_lossy().to_string();
        let settings = AppSettings {
            project_paths: vec!["   ".to_string()],
            project_path: Some(format!("  {}  ", dir.path().display())),
            ..AppSettings::default()
        };
        let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
        assert_eq!(
            report.primary_project_root.as_deref(),
            Some(trimmed.as_str())
        );
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog[0].key, "legacy");
    }

    #[test]
    fn catalog_report_no_project_instance_read_and_absent_creates_nothing() {
        // Existing instance catalog: allowed and read-only.
        let instance = seed_dir();
        write_report_manifest(
            instance.path(),
            &manifest_json(
                r##"[{"key":"instance","label":"Instance","description":"d","color":"#111","command":"instance","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
            ),
        );
        let expected_source = manifest_path(instance.path()).display().to_string();
        let bytes_before = std::fs::read(manifest_path(instance.path())).unwrap();
        let settings = AppSettings::default();
        let report = load_catalog_report_for_settings_with_config_dir(
            &settings,
            Some(instance.path().to_path_buf()),
        );
        assert_eq!(report.primary_project_root, None);
        assert_eq!(
            report.source_path.as_deref(),
            Some(expected_source.as_str())
        );
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog[0].key, "instance");
        assert_eq!(
            std::fs::read(manifest_path(instance.path())).unwrap(),
            bytes_before
        );

        // Absent instance catalog: unavailable, and repeated reads create
        // NOTHING on disk.
        let absent = seed_dir();
        let before: Vec<_> = std::fs::read_dir(absent.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        for _ in 0..3 {
            let report = load_catalog_report_for_settings_with_config_dir(
                &settings,
                Some(absent.path().to_path_buf()),
            );
            assert!(report.catalog.is_empty());
            assert_eq!(
                report.unavailable.as_ref().expect("unavailable").code,
                "baseUnavailable"
            );
        }
        assert!(!catalog_dir(absent.path()).exists());
        let after: Vec<_> = std::fs::read_dir(absent.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn catalog_report_config_dir_none_has_null_source_path() {
        let report =
            load_catalog_report_for_settings_with_config_dir(&AppSettings::default(), None);
        assert_eq!(report.primary_project_root, None);
        assert_eq!(report.source_path, None);
        assert!(report.catalog.is_empty());
        assert!(report.warnings.is_empty());
        let unavailable = report.unavailable.as_ref().expect("unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");
        assert!(unavailable.path.is_empty());
        let value = serde_json::to_value(&report).expect("report value");
        assert_eq!(value["sourcePath"], serde_json::Value::Null);
        assert_eq!(value["primaryProjectRoot"], serde_json::Value::Null);
    }

    #[test]
    fn catalog_report_repeated_reads_leave_bytes_and_entries_unchanged() {
        let dir = seed_dir();
        write_report_manifest(
            dir.path(),
            &manifest_json(
                r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true},
                 {"key":"mine2","label":"Mine2","description":"d","color":"#222","command":"mytool2","envs":[],"isolatedHome":false,"removable":true}]"##,
            ),
        );
        let path = manifest_path(dir.path());
        let bytes_before = std::fs::read(&path).unwrap();
        let mut entries_before: Vec<_> = std::fs::read_dir(catalog_dir(dir.path()))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        entries_before.sort();
        for _ in 0..3 {
            let report = report_for(dir.path());
            assert!(report.unavailable.is_none());
            assert_eq!(report.catalog.len(), 2);
            assert_eq!(report.warnings.len(), 2);
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes_before);
        let mut entries_after: Vec<_> = std::fs::read_dir(catalog_dir(dir.path()))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        entries_after.sort();
        assert_eq!(entries_before, entries_after);
    }

    #[test]
    fn catalog_report_desupported_builtin_omitted_with_diagnostic() {
        let manifest = manifest_json(
            r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
             {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        );
        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let dir = seed_dir();
            write_report_manifest(dir.path(), &manifest);
            let report = report_for(dir.path());
            assert_eq!(
                report
                    .catalog
                    .iter()
                    .map(|d| d.key.as_str())
                    .collect::<Vec<_>>(),
                ["mine"]
            );
            assert_eq!(report.warnings.len(), 1);
            assert_eq!(report.warnings[0].code, "invalidDefinition");
            assert!(
                report.warnings[0]
                    .reason
                    .contains("not supported by this build"),
                "{}",
                report.warnings[0].reason
            );
        });
    }

    // -----------------------------------------------------------------------
    // #1968 P5 - managed base, local overrides, migration, recovery, locks.
    // -----------------------------------------------------------------------

    fn ac_dir_for(project: &Path) -> PathBuf {
        project.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR)
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
        )
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
    }

    fn read_text(path: &Path) -> String {
        String::from_utf8(
            std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
        )
        .expect("utf-8 fixture")
    }

    fn local_json(ac_dir: &Path) -> serde_json::Value {
        read_json(&local_catalog_path(ac_dir))
    }

    fn base_json(ac_dir: &Path) -> serde_json::Value {
        read_json(&manifest_path(ac_dir))
    }

    fn write_local(ac_dir: &Path, contents: &str) {
        let path = local_catalog_path(ac_dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }

    fn write_legacy_base(ac_dir: &Path, contents: &str) {
        let path = manifest_path(ac_dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }

    /// The shipped definitions for `keys`, as plain JSON values.
    fn shipped_def_json(keys: &[&str]) -> Vec<serde_json::Value> {
        let shipped = supported_shipped_definitions();
        keys.iter()
            .map(|key| {
                let definition = shipped
                    .iter()
                    .find(|definition| definition.key == *key)
                    .unwrap_or_else(|| panic!("no shipped key {key}"));
                serde_json::to_value(definition).unwrap()
            })
            .collect()
    }

    /// A managed base whose marker claims `revision`; the content hash is the
    /// real hash of `agents` unless the caller mangles it.
    fn write_managed_base(
        ac_dir: &Path,
        agents: &[serde_json::Value],
        revision: &str,
        correct_content_hash: bool,
    ) -> Vec<u8> {
        let definitions: Vec<CodingAgentDefinition> = agents
            .iter()
            .cloned()
            .map(|value| serde_json::from_value(value).unwrap())
            .collect();
        let content = if correct_content_hash {
            managed_content_sha256(&definitions)
        } else {
            "0".repeat(64)
        };
        let root = serde_json::json!({
            "schemaVersion": 1,
            "agents": definitions,
            "managed": {
                "owner": "agentscommander",
                "version": 1,
                "revision": revision,
                "contentSha256": content,
            },
        });
        let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
        bytes.push(b'\n');
        let path = manifest_path(ac_dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        bytes
    }

    fn with_failure_at<R>(point: &'static str, f: impl FnOnce() -> R) -> R {
        CATALOG_FAILURE_POINT.with(|cell| cell.set(Some(point)));
        let result = f();
        CATALOG_FAILURE_POINT.with(|cell| cell.set(None));
        result
    }

    fn dir_entries(dir: &Path) -> Vec<std::ffi::OsString> {
        let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(dir)
            .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn managed_catalog_fresh_init_seeds_verified_base_and_exact_stub() {
        let dir = seed_dir();
        assert!(ensure_seeded(dir.path(), None).is_some());
        let base = base_json(dir.path());
        let expected_revision = managed_content_sha256(&supported_shipped_definitions());
        assert_eq!(base["managed"]["owner"], "agentscommander");
        assert_eq!(base["managed"]["version"], 1);
        assert_eq!(base["managed"]["revision"], expected_revision);
        assert_eq!(base["managed"]["contentSha256"], expected_revision);
        assert_eq!(base["agents"].as_array().unwrap().len(), 8);
        assert_eq!(
            std::fs::read(local_catalog_path(dir.path())).unwrap(),
            LOCAL_STUB_BYTES,
            "the local stub is exactly the documented bytes plus LF"
        );

        let base_bytes = std::fs::read(manifest_path(dir.path())).unwrap();
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.catalog.len(), 8);
        assert!(
            ensure_seeded(dir.path(), None).is_none(),
            "a verified base at the shipped revision is never rewritten"
        );
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            base_bytes
        );
        let residue: Vec<std::ffi::OsString> = std::fs::read_dir(catalog_dir(dir.path()))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .map(|entry| entry.file_name())
            .collect();
        assert!(residue.is_empty(), "no publication temporaries survive");
    }

    #[test]
    fn managed_catalog_refresh_reaches_unpinned_values_and_keeps_pinned_local() {
        let dir = seed_dir();
        let mut claude = shipped_def_json(&["claude"]).remove(0);
        claude["label"] = serde_json::json!("OLD LABEL");
        write_managed_base(dir.path(), &[claude], "stale-revision", true);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MINE"}]}"##,
        );
        let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "a base at a stale revision refreshes"
        );
        let base = base_json(dir.path());
        assert_eq!(
            base["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
        assert_eq!(
            base["agents"].as_array().unwrap().len(),
            8,
            "refresh publishes the whole shipped set"
        );
        assert_eq!(
            std::fs::read(local_catalog_path(dir.path())).unwrap(),
            local_before,
            "refresh never touches the local layer or the composed view"
        );

        let loaded = load_catalog(dir.path()).unwrap();
        let claude = loaded.iter().find(|d| d.key == "claude").unwrap();
        assert_eq!(claude.label, "MINE", "the pinned local value wins");
        assert_eq!(
            claude.description, "Coding Agent by Anthropic",
            "an unpinned value reaches the refreshed shipped default"
        );
        let report = load_catalog_report(dir.path());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn managed_catalog_support_gate_change_refreshes_the_base_both_ways() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let full_bytes = std::fs::read(manifest_path(dir.path())).unwrap();
        assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 8);

        with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
            assert!(
                ensure_seeded(dir.path(), None).is_some(),
                "a support-gate change is part of the managed revision"
            );
            let base = base_json(dir.path());
            assert_eq!(base["agents"].as_array().unwrap().len(), 9);
            assert_eq!(
                base["managed"]["revision"],
                managed_content_sha256(&supported_shipped_definitions())
            );
            assert_eq!(load_catalog(dir.path()).unwrap().len(), 9);
        });

        // Back under the shipped table the revision changes again and the base
        // byte-for-byte returns to its original state.
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            full_bytes
        );
        assert_eq!(load_catalog(dir.path()).unwrap().len(), 8);
    }

    #[test]
    fn managed_catalog_edited_base_is_readable_and_never_refreshed() {
        let dir = seed_dir();
        let mut claude = shipped_def_json(&["claude"]).remove(0);
        claude["label"] = serde_json::json!("HAND EDITED");
        let bytes = write_managed_base(dir.path(), &[claude], "stale-revision", false);

        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog[0].label, "HAND EDITED");
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.code == "managedBaseEdited"),
            "{:?}",
            report.warnings
        );
        assert!(
            ensure_seeded(dir.path(), None).is_none(),
            "an edited managed base is never auto-refreshed"
        );
        assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), bytes);
    }

    #[test]
    fn managed_catalog_unknown_managed_fields_block_refresh_without_deletion() {
        let dir = seed_dir();
        let agents = shipped_def_json(&["claude"]);
        let definitions: Vec<CodingAgentDefinition> = agents
            .iter()
            .cloned()
            .map(|value| serde_json::from_value(value).unwrap())
            .collect();
        let content = managed_content_sha256(&definitions);
        let root = serde_json::json!({
            "schemaVersion": 1,
            "agents": definitions,
            "futureRootField": {"keep": "me"},
            "managed": {
                "owner": "agentscommander",
                "version": 1,
                "revision": "stale-revision",
                "contentSha256": content,
            },
        });
        let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
        bytes.push(b'\n');
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();

        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 1);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationPending"));
        assert!(
            ensure_seeded(dir.path(), None).is_none(),
            "unknown data blocks refresh rather than being deleted"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn managed_catalog_foreign_managed_marker_blocks_ownership() {
        let dir = seed_dir();
        let root = serde_json::json!({
            "schemaVersion": 1,
            "agents": shipped_def_json(&["claude"]),
            "managed": {
                "owner": "somebody-else",
                "version": 1,
                "revision": "x",
                "contentSha256": "y",
            },
        });
        let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
        bytes.push(b'\n');
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();

        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 1, "a foreign base stays readable");
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationConflict"));
        assert!(
            ensure_seeded(dir.path(), None).is_none(),
            "a foreign marker neither refreshes nor migrates"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn managed_catalog_existing_local_entries_are_never_clobbered() {
        test09_directory_local_preserved();
        test09_regular_local_and_read_denied_preserved();
    }

    #[cfg(test)]
    type TreeInventoryRows = Vec<(
        std::path::PathBuf,
        bool,
        u64,
        bool,
        std::time::SystemTime,
        Vec<u8>,
    )>;

    fn test09_visit(root: &std::path::Path, path: &std::path::Path, rows: &mut TreeInventoryRows) {
        let metadata = std::fs::symlink_metadata(path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        rows.push((
            path.strip_prefix(root).unwrap().to_path_buf(),
            metadata.is_dir(),
            metadata.len(),
            metadata.permissions().readonly(),
            metadata.modified().unwrap(),
            if metadata.is_file() {
                std::fs::read(path).unwrap()
            } else {
                Vec::new()
            },
        ));
        if metadata.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            entries.sort();
            for entry in entries {
                test09_visit(root, &entry, rows);
            }
        }
    }

    fn test09_inventory(
        root: &std::path::Path,
    ) -> Vec<(
        std::path::PathBuf,
        bool,
        u64,
        bool,
        std::time::SystemTime,
        Vec<u8>,
    )> {
        let mut rows = Vec::new();
        test09_visit(root, root, &mut rows);
        rows
    }

    fn test09_directory_local_preserved() {
        // A directory at the local path is preserved and disables the layer.
        let dir = seed_dir();
        let local = local_catalog_path(dir.path());
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("keep.txt"), b"keep").unwrap();
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert!(local.is_dir());
        assert_eq!(std::fs::read(local.join("keep.txt")).unwrap(), b"keep");
        let before = test09_inventory(dir.path());
        let report = load_catalog_report(dir.path());
        assert_eq!(report.catalog.len(), 8, "the valid base stays usable");
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.code == "localInvalid"),
            "{:?}",
            report.warnings
        );

        assert!(report.unavailable.is_none());
        assert!(report.warnings.iter().any(|w| w.code == "localInvalid"
            && w.path == local.display().to_string()
            && !w.reason.is_empty()));
        for _ in 0..3 {
            assert_eq!(load_catalog_report(dir.path()), report);
        }
        assert_eq!(test09_inventory(dir.path()), before);
    }

    fn test09_regular_local_and_read_denied_preserved() {
        // A user-authored regular local file is preserved byte-for-byte and
        // composes onto the fresh managed base.
        let dir = seed_dir();
        let user = r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##;
        write_local(dir.path(), user);
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert_eq!(read_text(&local_catalog_path(dir.path())), user);
        let before = test09_inventory(dir.path());
        let loaded = load_catalog(dir.path()).unwrap();
        assert_eq!(loaded.len(), 9);
        assert_eq!(loaded.last().unwrap().key, "mine");
        assert_eq!(loaded.last().unwrap().label, "Mine");
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog, loaded);
        for _ in 0..3 {
            assert_eq!(load_catalog_report(dir.path()), report);
        }
        let mut expected = loaded.clone();
        expected.retain(|row| row.key != "mine");
        let mut denied: Option<ResolvedCatalog> = None;
        for _ in 0..3 {
            let snapshot = read_catalog_snapshot_with(
                dir.path(),
                CatalogReadScope::Project,
                |path, description| {
                    if path == local_catalog_path(dir.path()) {
                        Err("local read denied".into())
                    } else {
                        read_optional_regular_file(path, description)
                    }
                },
            )
            .unwrap();
            let degraded =
                resolve_catalog_snapshot(dir.path(), snapshot, CatalogSourceContext::Project);
            assert!(degraded.unavailable.is_none());
            assert_eq!(degraded.catalog, expected);
            assert_eq!(keys_of(&degraded.catalog), keys_of(&expected));
            assert!(!degraded.catalog.iter().any(|row| row.key == "mine"));
            assert!(degraded.warnings.iter().any(|w| w.code == "localInvalid"
                && w.path == local_catalog_path(dir.path()).display().to_string()
                && w.reason == "local read denied"));
            if let Some(previous) = &denied {
                assert_eq!(degraded.catalog, previous.catalog);
                assert_eq!(degraded.warnings, previous.warnings);
                assert_eq!(degraded.unavailable, previous.unavailable);
                assert_eq!(
                    degraded.base_verified_managed,
                    previous.base_verified_managed
                );
            }
            denied = Some(degraded);
        }
        assert_eq!(test09_inventory(dir.path()), before);
    }

    #[test]
    fn managed_catalog_local_composes_every_field_and_explicit_false_empty() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[
                {"key":"claude","label":"My Claude","description":"D","color":"#010203","command":"claude","instructionsFilename":null,"envs":[{"key":"ZZ","value":"1"},{"key":"AA","value":"2","source":"system","enabled":false}],"isolatedHome":true,"configSeed":{"enabled":false},"removable":false,"updateCommands":[],"autoUpdate":true}
            ]}"##,
        );
        let loaded = load_catalog(dir.path()).unwrap();
        let claude = loaded.iter().find(|d| d.key == "claude").unwrap();
        assert_eq!(claude.label, "My Claude");
        assert_eq!(claude.description, "D");
        assert_eq!(claude.color, "#010203");
        assert_eq!(claude.instructions_filename, None, "explicit null clears");
        assert_eq!(claude.envs.len(), 2, "envs replace whole arrays in order");
        assert_eq!(claude.envs[0].key, "ZZ");
        assert_eq!(claude.envs[1].key, "AA");
        assert!(!claude.envs[1].enabled, "explicit false stays explicit");
        assert_eq!(claude.envs[1].source, CodingAgentEnvSource::System);
        assert!(claude.isolated_home);
        let seed = claude.config_seed.as_ref().expect("merged configSeed");
        assert!(!seed.enabled);
        assert_eq!(seed.dest, ".claude", "nested configSeed merges by presence");
        assert!(!claude.removable);
        assert!(claude.update_commands.is_empty(), "explicit [] stays empty");
        assert!(claude.auto_update);
        assert_eq!(loaded.len(), 8, "no other row changed");
    }

    #[test]
    fn managed_catalog_local_new_row_order_and_tombstone() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[
                {"key":"zeta","label":"Zeta","description":"d","color":"#111","command":"zeta","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false},
                {"key":"grok","remove":true}
            ],"order":["zeta","claude"]}"##,
        );
        let loaded = load_catalog(dir.path()).unwrap();
        let keys = keys_of(&loaded);
        assert_eq!(keys[0], "zeta", "listed survivors come first in order");
        assert_eq!(keys[1], "claude");
        assert!(!keys.contains(&"grok"), "the tombstone removes the key");
        assert_eq!(keys.len(), 8, "8 shipped - 1 removed + 1 added");
    }

    #[test]
    fn managed_catalog_local_invalid_layer_falls_back_as_a_whole() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let bad = r##"{"schemaVersion":1,"agents":[
            {"key":"claude","label":"Would Apply"},
            {"key":"broken","label":"Broken"}
        ]}"##;
        write_local(dir.path(), bad);
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 8);
        assert_eq!(
            report.catalog[0].label, "Claude Code",
            "the valid row must not be partially applied"
        );
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "localInvalid"));
        assert_eq!(
            read_text(&local_catalog_path(dir.path())),
            bad,
            "the offending file is never rewritten"
        );
    }

    #[test]
    fn managed_catalog_local_schema_rejections_are_whole_layer() {
        let cases = [
            (
                "duplicate member",
                r##"{"schemaVersion":1,"agents":[{"key":"claude","key":"pi"}]}"##,
            ),
            (
                "unknown field",
                r##"{"schemaVersion":1,"agents":[{"key":"claude","mystery":1}]}"##,
            ),
            (
                "unsupported schema",
                r##"{"schemaVersion":2,"agents":[]}"##,
            ),
            (
                "remove false",
                r##"{"schemaVersion":1,"agents":[{"key":"claude","remove":false}]}"##,
            ),
            (
                "tombstone extra field",
                r##"{"schemaVersion":1,"agents":[{"key":"claude","remove":true,"label":"x"}]}"##,
            ),
            (
                "duplicate local key",
                r##"{"schemaVersion":1,"agents":[{"key":"claude"},{"key":"claude"}]}"##,
            ),
            (
                "duplicate order key",
                r##"{"schemaVersion":1,"agents":[],"order":["claude","claude"]}"##,
            ),
            (
                "control character in update command",
                "{\"schemaVersion\":1,\"agents\":[{\"key\":\"claude\",\"updateCommands\":[\"a\\u0007b\"]}]}",
            ),
        ];
        for (label, contents) in cases {
            let dir = seed_dir();
            ensure_seeded(dir.path(), None);
            write_local(dir.path(), contents);
            let report = load_catalog_report(dir.path());
            assert!(report.unavailable.is_none(), "{label}");
            assert_eq!(report.catalog.len(), 8, "{label}: base rows intact");
            assert_eq!(report.catalog[0].label, "Claude Code", "{label}");
            assert!(
                report
                    .warnings
                    .iter()
                    .any(|warning| warning.code == "localInvalid"),
                "{label}: {:?}",
                report.warnings
            );
            assert_eq!(
                read_text(&local_catalog_path(dir.path())),
                contents,
                "{label}"
            );
        }
    }

    #[test]
    fn managed_catalog_local_removability_is_evaluated_against_the_base() {
        let dir = seed_dir();
        let mut grok = shipped_def_json(&["grok"]).remove(0);
        grok["removable"] = serde_json::json!(false);
        write_managed_base(dir.path(), &[grok], "stale", true);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"grok","remove":true}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "localInvalid"));
        assert!(report.catalog.iter().any(|d| d.key == "grok"));
    }

    #[test]
    fn managed_catalog_local_support_gate_attempts_stay_suppressed() {
        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let dir = seed_dir();
            ensure_seeded(dir.path(), None);
            write_local(
                dir.path(),
                r##"{"schemaVersion":1,"agents":[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##,
            );
            let report = load_catalog_report(dir.path());
            assert!(!report.catalog.iter().any(|d| d.key == "muse"));
            assert!(report.warnings.iter().any(|warning| {
                warning.code == "invalidDefinition" && warning.reason.contains("not supported")
            }));
        });
    }

    #[test]
    fn managed_catalog_local_unknown_tombstone_and_order_keys_are_ignored() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"future-key","remove":true}],"order":["ghost","claude"]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.catalog.len(), 8);
        assert_eq!(
            report.catalog[0].key, "claude",
            "unknown order keys are ignored"
        );
        assert!(
            read_text(&local_catalog_path(dir.path())).contains("future-key"),
            "an unknown tombstone is retained for a future shipped key"
        );
    }

    // ---- #1968 migration, recovery, locks and read-only proof ----------------

    fn migration_journal_json(ac_dir: &Path) -> serde_json::Value {
        read_json(&migration_journal_path(ac_dir))
    }

    fn assert_reads_leave_state_unchanged(ac_dir: &Path) {
        let catalog = catalog_dir(ac_dir);
        let before_entries = dir_entries(&catalog);
        let before_base = std::fs::read(manifest_path(ac_dir)).ok();
        let before_local = std::fs::read(local_catalog_path(ac_dir)).ok();
        let before_backup = std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).ok();
        let before_journal = std::fs::read(migration_journal_path(ac_dir)).ok();
        // Repeated reads are identical INCLUDING warnings: a read derives its
        // warnings from the same snapshot it serves, so nothing may drift.
        let first = report_json(&load_catalog_report(ac_dir));
        for _ in 0..3 {
            assert_eq!(report_json(&load_catalog_report(ac_dir)), first);
        }
        assert_eq!(dir_entries(&catalog), before_entries);
        assert_eq!(std::fs::read(manifest_path(ac_dir)).ok(), before_base);
        assert_eq!(std::fs::read(local_catalog_path(ac_dir)).ok(), before_local);
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).ok(),
            before_backup
        );
        assert_eq!(
            std::fs::read(migration_journal_path(ac_dir)).ok(),
            before_journal
        );
    }

    #[test]
    fn managed_catalog_migrates_project_legacy_with_pins_changes_and_tombstones() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"claude","label":"My Claude","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
             {"key":"pi","label":"Pi Custom","description":"d","color":"#ec4899","command":"pi-custom","envs":[],"isolatedHome":false,"removable":true},
             {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        assert!(ensure_seeded(dir.path(), None).is_some());

        let journal = migration_journal_json(dir.path());
        assert_eq!(journal["version"], 1);
        assert_eq!(journal["sourceKind"], "project");
        assert_eq!(
            journal["sourcePath"],
            std::fs::canonicalize(manifest_path(dir.path()))
                .unwrap()
                .display()
                .to_string()
        );
        assert_eq!(
            std::fs::read(catalog_dir(dir.path()).join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            legacy.as_bytes(),
            "the backup is the byte-exact source"
        );

        let local = local_json(dir.path());
        assert_eq!(local["order"], serde_json::json!(["claude", "pi", "mine"]));
        let rows = local["agents"].as_array().unwrap();
        let claude = rows.iter().find(|row| row["key"] == "claude").unwrap();
        assert_eq!(claude["label"], "My Claude", "explicit values are pinned");
        assert_eq!(claude["updateCommands"], serde_json::json!([]));
        assert!(
            claude.get("instructionsFilename").is_none() && claude.get("configSeed").is_none(),
            "absent fields inherit the shipped default instead of being pinned"
        );
        let pi = rows.iter().find(|row| row["key"] == "pi").unwrap();
        assert_eq!(pi["command"], "pi-custom");
        assert_eq!(pi["updateCommands"], serde_json::json!([]));
        assert!(rows.iter().any(|row| row["key"] == "mine"));
        assert_eq!(
            rows.iter().filter(|row| row["remove"] == true).count(),
            6,
            "the six shipped keys absent from the legacy catalog are tombstoned"
        );

        let loaded = load_catalog(dir.path()).unwrap();
        assert_eq!(keys_of(&loaded), ["claude", "pi", "mine"]);
        assert_eq!(loaded[0].label, "My Claude");
        assert!(loaded[0].update_commands.is_empty(), "explicit [] clears");
        assert_eq!(loaded[1].command, "pi-custom");
        assert!(
            loaded[1].update_commands.is_empty(),
            "a changed command never inherits the shipped sequence"
        );

        // Completed migration: restart is a no-op and preserves every byte.
        let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
        let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();
        assert!(ensure_seeded(dir.path(), None).is_none());
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            base_before
        );
        assert_eq!(
            std::fs::read(local_catalog_path(dir.path())).unwrap(),
            local_before
        );
    }

    #[test]
    fn managed_catalog_migration_refuses_an_existing_local() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let user_local = r##"{"schemaVersion":1,"agents":[]}"##;
        write_local(dir.path(), user_local);

        assert!(ensure_seeded(dir.path(), None).is_none());
        assert_eq!(read_text(&manifest_path(dir.path())), legacy);
        assert_eq!(read_text(&local_catalog_path(dir.path())), user_local);
        assert!(!catalog_dir(dir.path())
            .join(MIGRATION_BACKUP_FILENAME)
            .exists());
        assert!(!catalog_dir(dir.path())
            .join(MIGRATION_JOURNAL_FILENAME)
            .exists());
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationPending"));
    }

    #[test]
    fn managed_catalog_migration_refuses_ambiguous_legacy_sources() {
        let cases = [
            (
                "unknown root field",
                r##"{"schemaVersion":1,"agents":[],"future":1}"##,
            ),
            (
                "unknown row field",
                r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"x","description":"d","color":"#111","command":"claude","mystery":1}]}"##,
            ),
            (
                "invalid definition",
                r##"{"schemaVersion":1,"agents":[{"key":"BAD KEY","label":"x","description":"d","color":"#111","command":"x"}]}"##,
            ),
            (
                "duplicate keys",
                r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"a","description":"d","color":"#111","command":"claude"},{"key":"claude","label":"b","description":"d","color":"#111","command":"claude"}]}"##,
            ),
            ("unsupported schema", r##"{"schemaVersion":2,"agents":[]}"##),
        ];
        for (label, legacy) in cases {
            let dir = seed_dir();
            write_legacy_base(dir.path(), legacy);
            assert!(ensure_seeded(dir.path(), None).is_none(), "{label}");
            assert_eq!(
                read_text(&manifest_path(dir.path())),
                legacy,
                "{label}: bytes preserved"
            );
            assert!(
                !catalog_dir(dir.path())
                    .join(MIGRATION_BACKUP_FILENAME)
                    .exists(),
                "{label}: no backup is written"
            );
            // The legacy base stays readable through the legacy resolver.
            let report = load_catalog_report(dir.path());
            assert!(
                report.unavailable.is_none()
                    || report.unavailable.as_ref().unwrap().code == "baseInvalid",
                "{label}"
            );
        }
    }

    #[test]
    fn managed_catalog_migration_of_an_empty_catalog_tombstones_every_shipped_key() {
        let dir = seed_dir();
        write_legacy_base(dir.path(), r##"{"schemaVersion":1,"agents":[]}"##);
        assert!(ensure_seeded(dir.path(), None).is_some());
        let local = local_json(dir.path());
        let rows = local["agents"].as_array().unwrap();
        assert_eq!(rows.len(), 8);
        assert!(rows.iter().all(|row| row["remove"] == true));
        assert_eq!(local["order"], serde_json::json!([]));
        assert!(load_catalog(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn managed_catalog_migration_pins_explicit_values_even_equal_to_defaults() {
        let dir = seed_dir();
        let claude = shipped_def_json(&["claude"]).remove(0);
        let legacy = serde_json::json!({"schemaVersion": 1, "agents": [claude]}).to_string();
        write_legacy_base(dir.path(), &legacy);
        assert!(ensure_seeded(dir.path(), None).is_some());

        let local = local_json(dir.path());
        let row = local["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == "claude")
            .unwrap();
        assert_eq!(row["label"], "Claude Code");
        assert_eq!(
            row["updateCommands"],
            serde_json::json!(["claude --update"])
        );
        assert_eq!(row["configSeed"]["dest"], ".claude");
        assert_eq!(row["instructionsFilename"], "CLAUDE.md");
        let loaded = load_catalog(dir.path()).unwrap();
        assert_eq!(
            loaded
                .iter()
                .find(|d| d.key == "claude")
                .unwrap()
                .update_commands,
            vec!["claude --update".to_string()]
        );
    }

    #[test]
    fn managed_catalog_migration_never_imports_the_instance_local_layer() {
        let project = seed_dir();
        let legacy = legacy_dir();
        std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();
        let instance_local = r##"{"schemaVersion":1,"agents":[{"key":"secret","label":"Secret","description":"d","color":"#111","command":"secret","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]}"##;
        std::fs::write(legacy.path().join("agents.local.json"), instance_local).unwrap();

        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
        let local = read_text(&local_catalog_path(project.path()));
        assert!(
            !local.contains("secret"),
            "the instance local layer is never imported: {local}"
        );
        assert!(load_catalog(project.path())
            .unwrap()
            .iter()
            .all(|definition| definition.key != "secret"));
        assert_eq!(
            read_text(&legacy.path().join("agents.50.personal.no-git.json")),
            instance_local,
            "the instance local file is read-only"
        );
    }

    #[test]
    fn managed_catalog_interrupt_after_backup_resumes_backup_only() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let _ = with_failure_at("after_backup", || ensure_seeded(dir.path(), None));

        let catalog = catalog_dir(dir.path());
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            legacy.as_bytes()
        );
        assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
        assert!(!local_catalog_path(dir.path()).exists());
        assert_eq!(
            read_text(&manifest_path(dir.path())),
            legacy,
            "base untouched"
        );
        assert!(
            load_catalog(dir.path()).is_ok(),
            "the readable legacy source stays usable while recovery is pending"
        );

        // Restart: the backup equals the current project source, so the
        // backup-only continuation completes deterministically.
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
        assert!(
            !migration_journal_path(dir.path()).exists(),
            "a backup-only resume never invents a journal"
        );
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            legacy.as_bytes(),
            "the backup is retained for audit"
        );
    }

    #[test]
    fn managed_catalog_interrupt_after_local_resumes_from_the_journal() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));

        assert!(migration_journal_path(dir.path()).exists());
        assert!(local_catalog_path(dir.path()).exists());
        assert_eq!(
            read_text(&manifest_path(dir.path())),
            legacy,
            "the base was not published yet"
        );
        let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "the restart completes from the journal"
        );
        assert_eq!(
            std::fs::read(local_catalog_path(dir.path())).unwrap(),
            local_before,
            "no re-extraction: the verified local bytes are kept"
        );
        assert_eq!(
            base_json(dir.path())["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
        assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);

        // Third run: completed, idempotent.
        let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
        assert!(ensure_seeded(dir.path(), None).is_none());
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            base_before
        );
    }

    #[test]
    fn managed_catalog_interrupt_after_journal_recomputes_the_extraction() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let _ = with_failure_at("after_journal", || ensure_seeded(dir.path(), None));

        let catalog = catalog_dir(dir.path());
        assert!(catalog.join(MIGRATION_BACKUP_FILENAME).exists());
        assert!(migration_journal_path(dir.path()).exists());
        assert!(
            !local_catalog_path(dir.path()).exists(),
            "the local layer was not published"
        );
        assert_eq!(
            read_text(&manifest_path(dir.path())),
            legacy,
            "base untouched"
        );
        let backup = std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
        assert_eq!(backup, legacy.as_bytes());

        // Restart: no local exists, so recovery RECOMPUTES the extraction from
        // the backup against the journal's saved managed base and verifies it
        // against the recorded local hash before publishing anything.
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert!(local_catalog_path(dir.path()).exists());
        assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
        assert_eq!(
            base_json(dir.path())["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            backup,
            "the backup is retained for audit"
        );
    }

    #[test]
    fn managed_catalog_interrupt_before_base_resumes_from_the_journal() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let _ = with_failure_at("before_base", || ensure_seeded(dir.path(), None));
        assert!(migration_journal_path(dir.path()).exists());
        assert_eq!(read_text(&manifest_path(dir.path())), legacy);

        assert!(ensure_seeded(dir.path(), None).is_some());
        assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
        assert_eq!(
            base_json(dir.path())["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
    }

    #[test]
    fn managed_catalog_interrupt_after_base_completes_without_re_extraction() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let _ = with_failure_at("after_base", || ensure_seeded(dir.path(), None));
        assert!(migration_journal_path(dir.path()).exists());
        let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();
        let base_before = std::fs::read(manifest_path(dir.path())).unwrap();

        assert!(
            ensure_seeded(dir.path(), None).is_none(),
            "a base matching the journal is already completed"
        );
        assert_eq!(
            std::fs::read(local_catalog_path(dir.path())).unwrap(),
            local_before
        );
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            base_before
        );
        assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
    }

    #[test]
    fn managed_catalog_interrupt_across_a_revision_change_completes_then_refreshes() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let _ = with_failure_at("before_base", || ensure_seeded(dir.path(), None));
        assert_eq!(
            migration_journal_json(dir.path())["managedBase"]["agents"]
                .as_array()
                .unwrap()
                .len(),
            8,
            "the journal saved the enabled shipped base"
        );

        with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
            assert!(
                ensure_seeded(dir.path(), None).is_some(),
                "the journal saved base is completed first"
            );
            let base = base_json(dir.path());
            assert_eq!(
                base["agents"].as_array().unwrap().len(),
                9,
                "then the normal verified refresh runs under the same lock"
            );
            assert!(
                base["agents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|agent| agent["key"] == "muse"),
                "the all-enabled refresh publishes muse"
            );
            assert_eq!(
                base["managed"]["revision"],
                managed_content_sha256(&supported_shipped_definitions())
            );
        });
    }

    #[test]
    fn managed_catalog_interrupt_with_a_local_edit_blocks_recovery() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));
        let edited =
            r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"EDITED BETWEEN RUNS"}]}"##;
        write_local(dir.path(), edited);
        let entries_before = dir_entries(&catalog_dir(dir.path()));

        assert!(ensure_seeded(dir.path(), None).is_none());
        assert_eq!(read_text(&local_catalog_path(dir.path())), edited);
        assert_eq!(read_text(&manifest_path(dir.path())), legacy);
        assert_eq!(
            dir_entries(&catalog_dir(dir.path())),
            entries_before,
            "a conflicting recovery deletes nothing"
        );
        // The read stays readable on the legacy source and reports the pending
        // migration; the conflict itself blocks only continuation.
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationPending"));
    }

    #[test]
    fn managed_catalog_sidecar_collisions_block_without_cleanup() {
        let dir = seed_dir();
        let legacy = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        write_legacy_base(dir.path(), &legacy);
        let catalog = catalog_dir(dir.path());
        std::fs::create_dir_all(&catalog).unwrap();
        std::fs::write(catalog.join(MIGRATION_BACKUP_FILENAME), b"foreign backup").unwrap();
        assert!(ensure_seeded(dir.path(), None).is_none());
        let entries_before = dir_entries(&catalog);
        assert!(ensure_seeded(dir.path(), None).is_none());
        assert_eq!(read_text(&manifest_path(dir.path())), legacy);
        assert_eq!(dir_entries(&catalog), entries_before);
        assert!(load_catalog(dir.path()).is_ok());

        // An invalid journal is also a conflict, and nothing is deleted.
        std::fs::remove_file(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
        std::fs::write(catalog.join(MIGRATION_JOURNAL_FILENAME), b"not json").unwrap();
        let entries_before = dir_entries(&catalog);
        assert!(ensure_seeded(dir.path(), None).is_none());
        assert_eq!(read_text(&manifest_path(dir.path())), legacy);
        assert_eq!(dir_entries(&catalog), entries_before);
    }

    #[test]
    fn managed_catalog_reads_never_write_in_any_state() {
        // Managed base with local overrides.
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"Mine"}]}"##,
        );
        assert_reads_leave_state_unchanged(dir.path());

        // Legacy base.
        let dir = seed_dir();
        write_legacy_base(
            dir.path(),
            &manifest_json(
                r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
            ),
        );
        assert_reads_leave_state_unchanged(dir.path());

        // Absent base and no directory at all.
        let dir = seed_dir();
        assert_reads_leave_state_unchanged(dir.path());

        // Interrupted migration boundary (journal + local, base still legacy).
        let dir = seed_dir();
        write_legacy_base(
            dir.path(),
            &manifest_json(
                r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
            ),
        );
        let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));
        assert_reads_leave_state_unchanged(dir.path());
        let report = load_catalog_report(dir.path());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationPending"));
    }

    #[test]
    fn managed_catalog_no_embedded_commands_escape_a_failed_publication() {
        let project = seed_dir();
        let legacy = legacy_dir();
        std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();
        let _ = with_failure_at("publication_temp_synced", || {
            ensure_seeded(project.path(), Some(legacy.path()))
        });
        assert!(
            !manifest_path(project.path()).exists(),
            "no base was published"
        );
        assert!(!catalog_dir(project.path())
            .join(MIGRATION_JOURNAL_FILENAME)
            .exists());
        let report = load_catalog_report(project.path());
        assert!(report.unavailable.is_some());
        let text = report_json(&report);
        for sentinel in [
            "claude --update",
            "pi update",
            "codex update",
            "hermes update --yes",
            "opencode upgrade",
            "agy update",
        ] {
            assert!(
                !text.contains(sentinel),
                "embedded command leaked: {sentinel}"
            );
        }
        assert!(load_catalog(project.path()).is_err());
    }

    #[test]
    fn managed_catalog_duplicate_command_identities_and_base_order_stay_intact() {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["pi --custom"],"autoUpdate":false}]}"##,
        );
        let loaded = load_catalog(dir.path()).unwrap();
        let keys = keys_of(&loaded);
        assert_eq!(keys[0], "claude");
        assert_eq!(keys[4], "pi");
        assert_eq!(keys[5], "opencode");
        assert_eq!(keys[8], "pi-max", "local additions append after the base");
        let pi = loaded.iter().find(|d| d.key == "pi").unwrap();
        let pi_max = loaded.iter().find(|d| d.key == "pi-max").unwrap();
        assert_eq!(pi.update_commands, vec!["pi update".to_string()]);
        assert_eq!(pi_max.update_commands, vec!["pi --custom".to_string()]);
    }

    #[test]
    fn managed_catalog_untracked_publication_is_retried_without_republishing() {
        let project = seed_dir();
        let root = project.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let ac_dir = ac_dir_for(&root);
        ensure_seeded(&ac_dir, None);
        let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
        assert!(
            !crate::config::seed_manifest::has_catalog_publication(&root).unwrap(),
            "the ungated seed recorded nothing"
        );

        let token = ManifestActivationToken::for_test();
        ensure_seeded_for_project_with_token(&root, Some(&token));
        assert!(
            crate::config::seed_manifest::has_catalog_publication(&root).unwrap(),
            "the verified current base is recorded on the next initialization"
        );
        assert_eq!(
            std::fs::read(manifest_path(&ac_dir)).unwrap(),
            base_before,
            "bookkeeping never republishes or rewrites the base"
        );
    }

    #[test]
    fn managed_catalog_report_flags_an_untracked_verified_base() {
        let tracked = seed_dir();
        let root = tracked.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        ensure_seeded(&ac_dir_for(&root), None);
        let settings = AppSettings {
            project_paths: vec![root.to_string_lossy().to_string()],
            ..AppSettings::default()
        };
        let report = report_json(&load_catalog_report_for_settings(&settings));
        assert!(report.contains("publicationUntracked"), "{report}");

        // A legacy base skips the bookkeeping query entirely.
        let legacy = seed_dir();
        write_legacy_base(
            &ac_dir_for(legacy.path()),
            &manifest_json(
                r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
            ),
        );
        let settings = AppSettings {
            project_paths: vec![legacy.path().to_string_lossy().to_string()],
            ..AppSettings::default()
        };
        let report = report_json(&load_catalog_report_for_settings(&settings));
        assert!(!report.contains("publicationUntracked"), "{report}");
    }

    // -----------------------------------------------------------------------
    // R2 correction fixtures (F1 read warning, F3 OS failures, F5 stub)
    // -----------------------------------------------------------------------

    /// The canonical empty coverage-v2 manifest (the serializer's own bytes),
    /// used as the repaired-manifest fixture.
    const REPAIRED_EMPTY_MANIFEST: &[u8] = concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 1\n",
        "coverage_version = 2\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
        "files = []\n",
    )
    .as_bytes();

    /// Publication temporaries still present in the catalog directory.
    fn temp_residue(ac_dir: &Path) -> Vec<String> {
        std::fs::read_dir(catalog_dir(ac_dir))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.contains(".tmp"))
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---- F1: read-path refresh warning with exact per-surface wording ------

    #[test]
    fn managed_catalog_stale_revision_warning_uses_the_exact_context_reason() {
        let agents = shipped_def_json(&["claude", "grok"]);

        // Direct: the public report wrapper keeps the neutral wording.
        let direct = seed_dir();
        write_managed_base(direct.path(), &agents, "stale-revision", true);
        let report = load_catalog_report(direct.path());
        let warning = report
            .warnings
            .iter()
            .find(|warning| warning.code == "refreshFailed")
            .expect("direct refreshFailed");
        assert_eq!(
            warning.reason,
            "The persisted managed catalog revision differs from this build. Current persisted entries remain usable. Catalog refresh runs during initialization."
        );
        assert_eq!(
            warning.path,
            manifest_path(direct.path()).display().to_string()
        );

        // Project: the settings project branch supplies the project wording.
        let project = seed_dir();
        write_managed_base(&ac_dir_for(project.path()), &agents, "stale-revision", true);
        let settings = AppSettings {
            project_paths: vec![project.path().to_string_lossy().to_string()],
            ..AppSettings::default()
        };
        let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
        let warning = report
            .warnings
            .iter()
            .find(|warning| warning.code == "refreshFailed")
            .expect("project refreshFailed");
        assert_eq!(
            warning.reason,
            "The persisted managed catalog revision differs from this build. Current persisted entries remain usable. Restart retries project catalog refresh."
        );
        assert_eq!(
            warning.path,
            manifest_path(&ac_dir_for(project.path()))
                .display()
                .to_string()
        );

        // Instance: the no-project branch supplies the read-only wording
        // (deliberately NOT the public Direct wrapper).
        let instance = seed_dir();
        write_managed_base(instance.path(), &agents, "stale-revision", true);
        let report = load_catalog_report_for_settings_with_config_dir(
            &AppSettings::default(),
            Some(instance.path().to_path_buf()),
        );
        let warning = report
            .warnings
            .iter()
            .find(|warning| warning.code == "refreshFailed")
            .expect("instance refreshFailed");
        assert_eq!(
            warning.reason,
            "The persisted instance catalog revision differs from this build. Current persisted entries remain usable. Restart retries instance catalog refresh when no project is registered."
        );
        assert_eq!(
            warning.path,
            manifest_path(instance.path()).display().to_string()
        );
    }

    #[test]
    fn managed_catalog_stale_revision_with_a_valid_pin_survives_repeated_reads() {
        let dir = seed_dir();
        write_managed_base(
            dir.path(),
            &shipped_def_json(&["claude"]),
            "stale-revision",
            true,
        );
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"PINNED"}]}"##,
        );
        let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
        let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

        for _ in 0..3 {
            let report = load_catalog_report(dir.path());
            assert!(report.unavailable.is_none());
            assert_eq!(report.catalog.len(), 1);
            assert_eq!(report.catalog[0].label, "PINNED");
            assert!(report
                .warnings
                .iter()
                .any(|warning| warning.code == "refreshFailed"));
        }
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            base_before
        );
        assert_eq!(
            std::fs::read(local_catalog_path(dir.path())).unwrap(),
            local_before
        );
    }

    #[test]
    fn managed_catalog_stale_revision_warning_clears_after_a_successful_restart() {
        let dir = seed_dir();
        write_managed_base(
            dir.path(),
            &shipped_def_json(&["claude"]),
            "stale-revision",
            true,
        );
        let report = load_catalog_report(dir.path());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "refreshFailed"));

        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "the stale base refreshes on initialization"
        );
        let refreshed = load_catalog_report(dir.path());
        assert!(refreshed.unavailable.is_none());
        assert!(refreshed.warnings.is_empty(), "{:?}", refreshed.warnings);
        assert_eq!(
            base_json(dir.path())["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
    }

    #[test]
    fn managed_catalog_stale_revision_with_an_invalid_local_reports_both() {
        let dir = seed_dir();
        write_managed_base(
            dir.path(),
            &shipped_def_json(&["claude"]),
            "stale-revision",
            true,
        );
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"broken"}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 1, "the verified base stays served");
        for code in ["refreshFailed", "localInvalid"] {
            assert!(
                report.warnings.iter().any(|warning| warning.code == code),
                "missing {code}: {:?}",
                report.warnings
            );
        }
    }

    #[test]
    fn managed_catalog_non_stale_and_unverified_bases_never_promise_a_restart() {
        // A current verified base is silent and usable.
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(!report_json(&report).contains("refreshFailed"));

        // Edited content keeps managedBaseEdited and adds no restart promise.
        let dir = seed_dir();
        let mut claude = shipped_def_json(&["claude"]).remove(0);
        claude["label"] = serde_json::json!("HAND EDITED");
        let bytes = write_managed_base(dir.path(), &[claude], "stale-revision", false);
        let report = load_catalog_report(dir.path());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "managedBaseEdited"));
        assert!(report
            .warnings
            .iter()
            .all(|warning| warning.code != "refreshFailed"));
        assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), bytes);

        // A foreign marker keeps migrationConflict and adds no restart promise.
        let dir = seed_dir();
        let root = serde_json::json!({
            "schemaVersion": 1,
            "agents": shipped_def_json(&["claude"]),
            "managed": {
                "owner": "somebody-else",
                "version": 1,
                "revision": "stale-revision",
                "contentSha256": "y",
            },
        });
        let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
        bytes.push(b'\n');
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let report = load_catalog_report(dir.path());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationConflict"));
        assert!(report
            .warnings
            .iter()
            .all(|warning| warning.code != "refreshFailed"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        // Unknown root fields block refresh and are preserved untouched.
        let dir = seed_dir();
        let definitions: Vec<CodingAgentDefinition> = shipped_def_json(&["claude"])
            .iter()
            .cloned()
            .map(|value| serde_json::from_value(value).unwrap())
            .collect();
        let content = managed_content_sha256(&definitions);
        let root = serde_json::json!({
            "schemaVersion": 1,
            "agents": definitions,
            "futureRootField": { "keep": "me" },
            "managed": {
                "owner": "agentscommander",
                "version": 1,
                "revision": "stale-revision",
                "contentSha256": content,
            },
        });
        let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
        bytes.push(b'\n');
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let report = load_catalog_report(dir.path());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationPending"));
        assert!(report
            .warnings
            .iter()
            .all(|warning| warning.code != "refreshFailed"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        // Corrupt bytes stay unavailable with no restart promise.
        let dir = seed_dir();
        std::fs::create_dir_all(catalog_dir(dir.path())).unwrap();
        std::fs::write(manifest_path(dir.path()), b"not a catalog at all").unwrap();
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_some());
        assert!(!report_json(&report).contains("refreshFailed"));
    }

    // ---- F3: real OS failures and injected arm-level boundaries ------------

    #[test]
    fn managed_catalog_local_symlink_and_dangling_link_are_preserved_as_invalid() {
        // A real file symlink at the local path: the layer is disabled, the
        // link identity/target and the target bytes survive every read.
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let target = dir.path().join("user-local-target.json");
        let target_bytes = br##"{"schemaVersion":1,"agents":[{"key":"claude","label":"LINKED"}]}"##;
        std::fs::write(&target, target_bytes).unwrap();
        let local = local_catalog_path(dir.path());
        std::fs::remove_file(&local).unwrap();
        create_manifest_symlink(&target, &local)
            .expect("real local symlink fixture (not a silent skip)");
        assert!(std::fs::symlink_metadata(&local)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&local).unwrap(), target);

        let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
        for _ in 0..2 {
            let report = load_catalog_report(dir.path());
            assert!(report.unavailable.is_none());
            assert_eq!(report.catalog.len(), 8, "the managed base stays usable");
            assert!(report
                .warnings
                .iter()
                .any(|warning| warning.code == "localInvalid"));
            assert!(!report.catalog.iter().any(|def| def.label == "LINKED"));
        }
        assert!(
            std::fs::symlink_metadata(&local)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link is never replaced by a stub"
        );
        assert_eq!(std::fs::read_link(&local).unwrap(), target);
        assert_eq!(std::fs::read(&target).unwrap(), target_bytes);
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            base_before
        );

        // A dangling link: same degradation, link and absence preserved even
        // across a later initialization.
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let local = local_catalog_path(dir.path());
        std::fs::remove_file(&local).unwrap();
        let missing = dir.path().join("no-such-local-target.json");
        create_manifest_symlink(&missing, &local).expect("real dangling local link fixture");
        let report = load_catalog_report(dir.path());
        assert_eq!(report.catalog.len(), 8);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "localInvalid"));
        assert!(std::fs::symlink_metadata(&local)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&local).unwrap(), missing);
        assert!(!missing.exists());
        assert!(
            ensure_seeded(dir.path(), None).is_none(),
            "the verified base needs no rewrite"
        );
        assert!(std::fs::symlink_metadata(&local)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!missing.exists());
    }

    #[test]
    fn managed_catalog_hard_link_unsupported_leaves_prior_artifacts_and_resumes() {
        let project = seed_dir();
        let legacy = legacy_dir();
        let legacy_bytes = legacy_catalog_json();
        std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();
        let base = manifest_path(project.path());
        let local = local_catalog_path(project.path());
        let backup = catalog_dir(project.path()).join(MIGRATION_BACKUP_FILENAME);
        let journal = migration_journal_path(project.path());

        // SIMULATED error result at the link boundary (arm-level proof); real
        // unsupported-filesystem execution is unmeasured. The fault is armed
        // for the BASE destination only, so the earlier backup, journal and
        // local publications run for real.
        arm_catalog_path_fault("hard_link", &base, std::io::ErrorKind::Unsupported);
        let outcome = run_catalog_initialization(project.path(), Some(legacy.path()), None);
        clear_catalog_path_faults();

        assert!(outcome.published_at.is_none(), "no base was published");
        let conflict = outcome
            .warnings
            .iter()
            .find(|warning| warning.code == "migrationConflict")
            .expect("the link failure surfaces");
        assert!(conflict.reason.contains("hard link"), "{}", conflict.reason);
        assert!(!base.exists(), "the destination stays absent");
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            legacy_bytes,
            "the backup keeps the source bytes exactly"
        );
        assert!(journal.is_file(), "the journal survives");
        let local_bytes = std::fs::read(&local).expect("the extracted local survives");
        assert_eq!(
            std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
            legacy_bytes,
            "the source is never modified"
        );
        assert!(
            temp_residue(project.path()).is_empty(),
            "only this run's temp is removed: {:?}",
            temp_residue(project.path())
        );

        // Fault cleared: the interrupted transaction resumes and completes,
        // then a second initialization is idempotent.
        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
        let completed_base = std::fs::read(&base).unwrap();
        assert_eq!(
            std::fs::read(&local).unwrap(),
            local_bytes,
            "recovery never re-extracts or rewrites the local layer"
        );
        assert_eq!(
            std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
            legacy_bytes
        );
        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
        assert_eq!(std::fs::read(&base).unwrap(), completed_base);
        let base_value: serde_json::Value = serde_json::from_slice(&completed_base).unwrap();
        assert_eq!(base_value["managed"]["owner"], "agentscommander");
    }

    #[test]
    fn managed_catalog_exclusive_publication_collision_preserves_the_destination() {
        let scratch = seed_dir();
        let destination = scratch.path().join("agents.local.json");
        let sentinel = b"USER SENTINEL BYTES\n";
        std::fs::write(&destination, sentinel).unwrap();
        let source = scratch.path().join("source-bytes.json");
        std::fs::write(&source, b"REPLACEMENT CONTENT\n").unwrap();

        // The REAL (uninjected) hard link reports AlreadyExists on this pair;
        // the injected Unsupported arm covers the absent destination instead.
        let real = std::fs::hard_link(&source, &destination).unwrap_err();
        assert_eq!(real.kind(), std::io::ErrorKind::AlreadyExists);

        let error = publish_exclusive(&destination, b"new content").unwrap_err();
        assert!(error.contains("exclusive publication"), "{error}");
        assert_eq!(std::fs::read(&destination).unwrap(), sentinel);
        assert!(
            dir_entries(scratch.path())
                .iter()
                .all(|name| !name.to_string_lossy().contains(".tmp")),
            "no publication temp survives"
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"REPLACEMENT CONTENT\n");
    }

    #[cfg(windows)]
    #[test]
    fn managed_catalog_replace_share_denial_keeps_bytes_and_recovers() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let dir = seed_dir();
        write_managed_base(
            dir.path(),
            &shipped_def_json(&["claude"]),
            "stale-revision",
            true,
        );
        let base = manifest_path(dir.path());
        let base_before = std::fs::read(&base).unwrap();
        // A real destination handle that denies replacement: ReplaceFileW
        // fails through the production error path.
        let blocker = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&base)
            .unwrap();

        let outcome = run_catalog_initialization(dir.path(), None, None);
        assert!(outcome.published_at.is_none(), "ReplaceFileW must fail");
        assert!(
            outcome.warnings.iter().any(|warning| {
                warning.code == "refreshFailed" && warning.reason.contains("Failed to replace")
            }),
            "{:?}",
            outcome.warnings
        );
        assert_eq!(
            std::fs::read(&base).unwrap(),
            base_before,
            "the original bytes are unchanged"
        );
        assert!(
            temp_residue(dir.path()).is_empty(),
            "the failed publication removes its own temp: {:?}",
            temp_residue(dir.path())
        );

        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(
            report.catalog.len(),
            1,
            "the stale verified base stays usable"
        );
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "refreshFailed"));

        // Release the denial: two restarts refresh once and then stay idempotent.
        drop(blocker);
        let refreshed = run_catalog_initialization(dir.path(), None, None);
        assert!(refreshed.published_at.is_some(), "release then refresh");
        let base_after = std::fs::read(&base).unwrap();
        let second = run_catalog_initialization(dir.path(), None, None);
        assert!(second.published_at.is_none(), "the refresh is idempotent");
        assert_eq!(std::fs::read(&base).unwrap(), base_after);
        assert_eq!(
            base_json(dir.path())["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
        let report = load_catalog_report(dir.path());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn managed_catalog_degraded_manifest_record_failure_repairs_without_republishing() {
        let temp = seed_dir();
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".ac")).unwrap();
        let ac_dir = ac_dir_for(&root);
        let seed_manifest = root.join(".ac").join(SEED_MANIFEST_FILENAME);

        // A malformed canonical manifest is preserved read-only: the base still
        // publishes, but the row cannot be recorded (PublishedUnrecorded).
        std::fs::write(&seed_manifest, b"not = [valid toml").unwrap();
        ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
        assert!(
            manifest_path(&ac_dir).is_file(),
            "the base published anyway"
        );
        assert!(
            crate::config::seed_manifest::has_catalog_publication(&root).is_err(),
            "a malformed manifest is unreadable"
        );
        {
            let mut guard = ProjectSeedManifestGuard::acquire(&root)
                .expect("a malformed manifest is held read-only, not rejected");
            let row = PublishedManifestRow::coding_agent_catalog(
                ManifestPathIdentity::from_relative_path(Path::new(
                    ".ac/coding-agents/agents.json",
                ))
                .unwrap(),
                Utc::now(),
            )
            .unwrap();
            let outcome = guard
                .publication_permit()
                .record_file(&ManifestActivationToken::for_test(), row);
            assert!(
                matches!(
                    outcome,
                    crate::config::seed_manifest::ManifestRecordOutcome::PublishedUnrecorded(_)
                ),
                "expected PublishedUnrecorded, got {outcome:?}"
            );
            guard.release();
        }

        let settings = AppSettings {
            project_paths: vec![root.to_string_lossy().to_string()],
            ..AppSettings::default()
        };
        let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
        assert!(report.unavailable.is_none());
        assert!(!report.catalog.is_empty(), "the catalog stays usable");
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "publicationUntracked"));
        let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
        let local_before = std::fs::read(local_catalog_path(&ac_dir)).unwrap();

        // Repair only the manifest: two initializations restore the row without
        // republishing the base or rewriting the local layer.
        std::fs::write(&seed_manifest, REPAIRED_EMPTY_MANIFEST).unwrap();
        ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
        assert!(crate::config::seed_manifest::has_catalog_publication(&root).unwrap());
        let manifest_after_repair = std::fs::read(&seed_manifest).unwrap();
        ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
        assert_eq!(
            std::fs::read(&seed_manifest).unwrap(),
            manifest_after_repair,
            "the second initialization neither re-records nor republishes"
        );
        assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_before);
        assert_eq!(
            std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
            local_before
        );
        let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
        assert!(!report_json(&report).contains("publicationUntracked"));
    }

    #[test]
    fn managed_catalog_reads_never_touch_an_interrupted_instance_source() {
        let project = seed_dir();
        let legacy = legacy_dir();
        let legacy_bytes = legacy_catalog_json();
        std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();

        // Stop the migration after the local layer: base absent, backup +
        // journal + local present, and the instance source still the only
        // donor the read may consult.
        let _ = with_failure_at("after_local", || {
            ensure_seeded(project.path(), Some(legacy.path()))
        });
        let catalog = catalog_dir(project.path());
        assert!(catalog.join(MIGRATION_BACKUP_FILENAME).is_file());
        assert!(migration_journal_path(project.path()).is_file());
        assert!(local_catalog_path(project.path()).is_file());
        assert!(!manifest_path(project.path()).exists());

        assert_reads_leave_state_unchanged(project.path());
        let report = load_catalog_report(project.path());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationPending"));
        assert_eq!(
            std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
            legacy_bytes,
            "reads never write the instance source; its bytes survive the rename at the new name"
        );
        assert!(!legacy.path().join("agents.json").exists());
    }

    // ---- F5: transient stub failure, no persistent state -------------------

    #[test]
    fn managed_catalog_stub_failure_is_nonfatal_and_never_retried() {
        let temp = seed_dir();
        let root = temp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let ac_dir = ac_dir_for(&root);
        let local = local_catalog_path(&ac_dir);

        // Destination-aware fault at the STUB publication only: the base link
        // runs for real because it targets a different path.
        arm_catalog_path_fault("hard_link", &local, std::io::ErrorKind::Unsupported);
        let outcome = run_catalog_initialization(&ac_dir, None, None);
        clear_catalog_path_faults();

        assert!(
            outcome.published_at.is_some(),
            "the base publication succeeds"
        );
        let canonical_local = std::fs::canonicalize(catalog_dir(&ac_dir))
            .unwrap()
            .join(LOCAL_CATALOG_FILENAME);
        let warning = outcome
            .warnings
            .iter()
            .find(|warning| warning.code == "refreshFailed")
            .expect("the transient stub warning");
        assert_eq!(warning.path, canonical_local.display().to_string());
        assert!(
            warning.reason.starts_with(
                "The managed catalog base is usable, but its local overrides stub could not be created: "
            ),
            "{}",
            warning.reason
        );
        assert!(
            warning
                .reason
                .ends_with("An absent local file is valid; no automatic stub retry is scheduled."),
            "{}",
            warning.reason
        );
        assert!(!local.exists(), "the stub stays absent");
        assert_eq!(
            load_catalog(&ac_dir).unwrap().len(),
            8,
            "the base stays usable"
        );
        assert!(
            temp_residue(&ac_dir).is_empty(),
            "{:?}",
            temp_residue(&ac_dir)
        );

        // No persistent report warning: the base is already at this revision.
        let report = load_catalog_report(&ac_dir);
        assert!(report.unavailable.is_none());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        // Bookkeeping still records the verified current base without
        // republishing it, and a later initialization leaves the absent local
        // absent (no creation-on-every-start).
        let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
        ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
        assert!(crate::config::seed_manifest::has_catalog_publication(&root).unwrap());
        assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_before);
        assert!(!local.exists());
        let _ = run_catalog_initialization(&ac_dir, None, None);
        assert!(!local.exists());
    }

    #[test]
    fn managed_catalog_lock_timeout_is_bounded_and_named() {
        assert_eq!(CATALOG_LOCK_TIMEOUT, Duration::from_secs(5));
        let dir = seed_dir();
        let catalog = catalog_dir(dir.path());
        std::fs::create_dir_all(&catalog).unwrap();
        let canonical = std::fs::canonicalize(&catalog).unwrap();
        let paths = CatalogPaths::new(&canonical);
        let held = acquire_catalog_lock(&paths).unwrap();

        CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_millis(150))));
        let started = Instant::now();
        let error = acquire_catalog_lock(&paths).unwrap_err();
        let elapsed = started.elapsed();
        CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));

        assert!(error.contains("catalogLockTimeout"), "{error}");
        assert!(error.contains(CATALOG_LOCK_FILENAME), "{error}");
        assert!(error.contains("150 ms"), "{error}");
        assert!(
            elapsed >= Duration::from_millis(150) && elapsed < Duration::from_secs(2),
            "bounded wait, elapsed {elapsed:?}"
        );
        drop(held);
        let reacquired = acquire_catalog_lock(&paths).unwrap();
        drop(reacquired);
        assert!(paths.lock.is_file(), "the lock file is never removed");
    }

    const LOCK_CHILD_ACTION_ENV: &str = "AC_1968_CATALOG_LOCK_CHILD_ACTION";
    const LOCK_CHILD_DIR_ENV: &str = "AC_1968_CATALOG_LOCK_CHILD_DIR";
    const LOCK_CHILD_TEST_FQN: &str =
        "config::coding_agents_catalog::tests::managed_catalog_lock_child";

    fn wait_for_path(path: &Path, timeout: Duration, label: &str) {
        let started = Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < timeout,
                "{label}: timed out waiting for {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn spawn_catalog_lock_child(dir: &Path) -> (PathBuf, std::process::Child) {
        let canonical = std::fs::canonicalize(dir).expect("canonical catalog dir");
        let exe = std::env::current_exe().expect("current test exe");
        let mut command = std::process::Command::new(exe);
        command
            .args([
                "--exact",
                LOCK_CHILD_TEST_FQN,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(LOCK_CHILD_ACTION_ENV, "hold")
            .env(LOCK_CHILD_DIR_ENV, &canonical)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = command.spawn().expect("spawn catalog lock child");
        wait_for_path(
            &canonical.join("child-ready"),
            Duration::from_secs(30),
            "catalog lock child",
        );
        (canonical, child)
    }

    #[test]
    fn managed_catalog_lock_child() {
        let Some(action) = std::env::var_os(LOCK_CHILD_ACTION_ENV) else {
            return;
        };
        let action = action.to_string_lossy().into_owned();
        let dir = PathBuf::from(std::env::var_os(LOCK_CHILD_DIR_ENV).expect("child dir env"));
        let paths = CatalogPaths::new(&dir);
        match action.as_str() {
            "hold" => {
                let _lock = acquire_catalog_lock(&paths).expect("child must acquire the lock");
                std::fs::write(dir.join("child-ready"), b"ready").expect("child announces ready");
                let deadline = Instant::now() + Duration::from_secs(60);
                while !dir.join("child-release").exists() {
                    assert!(
                        Instant::now() < deadline,
                        "child hold exceeded its 60s bound"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            other => panic!("unknown child action {other}"),
        }
        println!("AC_1968_CATALOG_LOCK_CHILD_DONE action={action}");
    }

    #[test]
    fn managed_catalog_process_lock_contention_times_out_and_recovers() {
        let dir = seed_dir();
        let catalog = catalog_dir(dir.path());
        std::fs::create_dir_all(&catalog).unwrap();
        let canonical = std::fs::canonicalize(&catalog).unwrap();
        let (held_dir, mut child) = spawn_catalog_lock_child(&canonical);
        let paths = CatalogPaths::new(&held_dir);

        CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_millis(250))));
        let error = acquire_catalog_lock(&paths).unwrap_err();
        CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));
        assert!(error.contains("catalogLockTimeout"), "{error}");

        std::fs::write(held_dir.join("child-release"), b"release").unwrap();
        let started = Instant::now();
        loop {
            match acquire_catalog_lock(&paths) {
                Ok(_) => break,
                Err(error) => {
                    assert!(
                        started.elapsed() < Duration::from_secs(30),
                        "the released child lock must become acquirable: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        let status = child.wait().expect("reap the lock child");
        assert!(status.success(), "the lock child must exit cleanly");
    }

    #[test]
    fn managed_catalog_process_lock_holder_death_releases_the_lock() {
        let dir = seed_dir();
        let catalog = catalog_dir(dir.path());
        std::fs::create_dir_all(&catalog).unwrap();
        let canonical = std::fs::canonicalize(&catalog).unwrap();
        let exe = std::env::current_exe().expect("current test exe");
        let mut child = std::process::Command::new(exe)
            .args([
                "--exact",
                LOCK_CHILD_TEST_FQN,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(LOCK_CHILD_ACTION_ENV, "hold")
            .env(LOCK_CHILD_DIR_ENV, &canonical)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn catalog lock child");
        wait_for_path(
            &canonical.join("child-ready"),
            Duration::from_secs(30),
            "catalog lock child",
        );
        child.kill().expect("kill the lock holder");
        let _ = child.wait();

        let paths = CatalogPaths::new(&canonical);
        CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_secs(3))));
        let acquired = acquire_catalog_lock(&paths);
        CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));
        assert!(
            acquired.is_ok(),
            "a crashed holder releases the OS lock: {acquired:?}"
        );
    }

    // ---- #2715 (B2): the `agents.*` family rename -------------------------

    const OLD_BASE: &str = "agents.json";
    const OLD_LOCAL: &str = "agents.local.json";

    /// A project root plus a config dir that holds the journal and the legacy
    /// instance catalog, both fixtures the test controls.
    struct B2Fixture {
        project: tempfile::TempDir,
        config: tempfile::TempDir,
    }

    impl B2Fixture {
        fn new() -> Self {
            let fixture = B2Fixture {
                project: tempfile::tempdir().expect("project tempdir"),
                config: tempfile::tempdir().expect("config tempdir"),
            };
            std::fs::create_dir_all(fixture.project_catalog()).unwrap();
            fixture
        }
        fn root(&self) -> &Path {
            self.project.path()
        }
        fn ac_dir(&self) -> PathBuf {
            self.root()
                .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR)
        }
        fn project_catalog(&self) -> PathBuf {
            catalog_dir(&self.ac_dir())
        }
        fn journal_dir(&self) -> &Path {
            self.config.path()
        }
        fn legacy(&self) -> PathBuf {
            self.config.path().join(CATALOG_DIR_NAME)
        }
        fn seed(&self) {
            ensure_seeded_for_project_in(
                self.root(),
                None,
                Some(&self.legacy()),
                Some(self.journal_dir()),
            );
        }
        fn project_key(&self) -> String {
            catalog_scope_key(&std::fs::canonicalize(self.project_catalog()).unwrap())
        }
        fn legacy_key(&self) -> String {
            catalog_scope_key(&std::fs::canonicalize(self.legacy()).unwrap())
        }
        fn record(&self, key: &str) -> naming_migration::ScopeRecord {
            naming_migration::read_journal(Some(self.journal_dir()))
                .expect("journal readable")
                .expect("journal exists")
                .scope(key)
                .cloned()
                .unwrap_or_else(|| panic!("scope {key} recorded"))
        }
    }

    /// A managed base carrying a recognisable extra agent: an EDITED managed
    /// base, so no initialization pass ever refreshes, migrates or replaces it.
    fn preserved_base_bytes(key: &str) -> Vec<u8> {
        let mut value: serde_json::Value =
            serde_json::from_slice(&build_managed_base_bytes(&supported_shipped_definitions()))
                .unwrap();
        value["agents"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "key": key, "label": key, "description": "d", "color": "#111",
                "command": key, "envs": [], "isolatedHome": false, "removable": true
            }));
        let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn local_bytes(tag: &str) -> Vec<u8> {
        format!("{{\"schemaVersion\":1,\"agents\":[],\"_tag\":\"{tag}\"}}\n").into_bytes()
    }

    fn scope_keys(journal_dir: &Path) -> Vec<String> {
        let journal = naming_migration::read_journal(Some(journal_dir))
            .unwrap()
            .expect("journal exists");
        let value = serde_json::to_value(&journal).unwrap();
        value["scopes"]
            .as_object()
            .map(|scopes| scopes.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn deprecated_entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.contains(".deprecated-"))
            .collect();
        names.sort();
        names
    }

    /// Leaves a genuinely interrupted #1968 project migration under the
    /// PRE-migration names: the transaction stops at `point`.
    fn interrupted_1968_migration(catalog: &Path, point: &'static str) -> Vec<u8> {
        let source = legacy_catalog_json();
        std::fs::write(catalog.join(OLD_BASE), &source).unwrap();
        let old = CatalogPaths::with_names(catalog, OLD_BASE, OLD_LOCAL);
        CATALOG_FAILURE_POINT.with(|cell| cell.set(Some(point)));
        let result = migrate_legacy(&old, MIGRATION_SOURCE_PROJECT, &old.base, &source);
        CATALOG_FAILURE_POINT.with(|cell| cell.set(None));
        assert!(
            result.is_err(),
            "the fixture migration must stop at {point}"
        );
        source
    }

    fn local_carries(path: &Path, key: &str) -> bool {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        value["agents"]
            .as_array()
            .is_some_and(|agents| agents.iter().any(|agent| agent["key"] == key))
    }

    #[test]
    fn the_migration_runs_before_the_base_is_seeded_in_one_locked_pass() {
        let fx = B2Fixture::new();
        let catalog = fx.project_catalog();
        let user = preserved_base_bytes("mine");
        std::fs::write(catalog.join(OLD_BASE), &user).unwrap();

        fx.seed();

        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            user,
            "the user's catalog survives at the new name; no fresh base was written"
        );
        assert!(!catalog.join(OLD_BASE).exists());
        assert!(
            deprecated_entries(&catalog).is_empty(),
            "nothing was set aside"
        );
        assert_eq!(
            fx.record(&fx.project_key()).status,
            naming_migration::ScopeStatus::Complete
        );
    }

    #[test]
    fn every_seeding_entry_point_migrates_before_it_creates() {
        // ensure_seeded_for_project_in, gated.
        let fx = B2Fixture::new();
        let user = preserved_base_bytes("mine");
        std::fs::write(fx.project_catalog().join(OLD_BASE), &user).unwrap();
        let token = ManifestActivationToken::for_test();
        ensure_seeded_for_project_in(
            fx.root(),
            Some(&token),
            Some(&fx.legacy()),
            Some(fx.journal_dir()),
        );
        // ensure_seeded_for_project_in, activation None.
        let plain = B2Fixture::new();
        std::fs::write(plain.project_catalog().join(OLD_BASE), &user).unwrap();
        plain.seed();
        for (label, catalog) in [
            ("gated", fx.project_catalog()),
            ("activation None", plain.project_catalog()),
        ] {
            assert_eq!(
                std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
                user,
                "{label}"
            );
            assert!(!catalog.join(OLD_BASE).exists(), "{label}");
            assert!(deprecated_entries(&catalog).is_empty(), "{label}");
        }
        // ensure_seeded_instance_in.
        let instance = tempfile::tempdir().unwrap();
        let catalog = catalog_dir(instance.path());
        std::fs::create_dir_all(&catalog).unwrap();
        std::fs::write(catalog.join(OLD_BASE), &user).unwrap();
        ensure_seeded_instance_in(instance.path(), Some(instance.path()));
        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            user,
            "instance"
        );
        assert!(!catalog.join(OLD_BASE).exists(), "instance");
        assert!(deprecated_entries(&catalog).is_empty(), "instance");
    }

    #[test]
    fn a_recoverable_sidecar_recovers_then_renames_in_one_pass() {
        let fx = B2Fixture::new();
        let catalog = fx.project_catalog();
        interrupted_1968_migration(&catalog, "after_local");
        let backup = std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
        let journal = std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap();

        fx.seed();

        assert!(!catalog.join(OLD_BASE).exists());
        assert!(!catalog.join(OLD_LOCAL).exists());
        let base: serde_json::Value = serde_json::from_slice(
            &std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            base["managed"]["owner"], "agentscommander",
            "the recovered base"
        );
        assert!(
            local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
            "the recovered agent is at the new local name"
        );
        let report = load_catalog_report(&fx.ac_dir());
        assert!(
            report.catalog.iter().any(|agent| agent.key == "mine"),
            "the effective catalog loaded from disk serves the recovered agent: {:?}",
            report.warnings
        );
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            backup
        );
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap(),
            journal
        );
        let record = fx.record(&fx.project_key());
        assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
        for sidecar in [MIGRATION_BACKUP_FILENAME, MIGRATION_JOURNAL_FILENAME] {
            assert!(
                record.notes.iter().any(|note| note.contains(sidecar)),
                "{sidecar} noted: {:?}",
                record.notes
            );
        }
        assert!(deprecated_entries(&catalog).is_empty());
    }

    /// E8e: data already at the new names beside a #1968 sidecar, with no
    /// journal dir. F1 must not resurrect the old names; without its
    /// `has_old_data` guard the set-aside ordinal grows by one every pass.
    #[test]
    fn a_sidecar_without_old_data_runs_no_recovery() {
        let fx = B2Fixture::new();
        let catalog = fx.project_catalog();
        interrupted_1968_migration(&catalog, "after_local");
        // Finish the data move by hand: the old names are gone.
        std::fs::rename(
            catalog.join(OLD_BASE),
            catalog.join(CATALOG_MANIFEST_FILENAME),
        )
        .unwrap();
        std::fs::rename(
            catalog.join(OLD_LOCAL),
            catalog.join(LOCAL_CATALOG_FILENAME),
        )
        .unwrap();
        assert!(catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
        assert!(catalog.join(MIGRATION_BACKUP_FILENAME).exists());
        for pass in 1..=2 {
            ensure_seeded_for_project_in(fx.root(), None, None, None);
            assert!(
                !catalog.join(OLD_BASE).exists(),
                "pass {pass}: agents.json recreated"
            );
            assert!(
                !catalog.join(OLD_LOCAL).exists(),
                "pass {pass}: agents.local.json recreated"
            );
            assert_eq!(
                deprecated_entries(&catalog),
                Vec::<String>::new(),
                "pass {pass}: nothing set aside"
            );
        }
        fx.seed();
        assert_eq!(
            fx.record(&fx.project_key()).status,
            naming_migration::ScopeStatus::Complete
        );
        assert!(deprecated_entries(&catalog).is_empty());
    }

    #[test]
    fn a_blocked_recovery_defers_and_renames_nothing() {
        let fx = B2Fixture::new();
        let catalog = fx.project_catalog();
        let source = interrupted_1968_migration(&catalog, "after_local");
        let backup_path = catalog.join(MIGRATION_BACKUP_FILENAME);
        let backup = std::fs::read(&backup_path).unwrap();
        let local = std::fs::read(catalog.join(OLD_LOCAL)).unwrap();
        std::fs::remove_file(&backup_path).unwrap();

        let outcome = run_catalog_initialization_with_scope(
            &fx.ac_dir(),
            Some(&fx.legacy()),
            Some(fx.journal_dir()),
            InitScope::Project,
        );
        assert!(outcome
            .warnings
            .iter()
            .any(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT));
        assert_eq!(std::fs::read(catalog.join(OLD_BASE)).unwrap(), source);
        assert_eq!(std::fs::read(catalog.join(OLD_LOCAL)).unwrap(), local);
        assert!(
            !catalog.join(CATALOG_MANIFEST_FILENAME).exists(),
            "nothing seeded"
        );
        assert!(
            !catalog.join(LOCAL_CATALOG_FILENAME).exists(),
            "nothing renamed"
        );
        let record = fx.record(&fx.project_key());
        assert_eq!(record.status, naming_migration::ScopeStatus::Deferred);
        assert!(
            record.notes.iter().any(|note| note.contains("backup")),
            "{:?}",
            record.notes
        );

        // Restoring the backup heals it with no journal surgery.
        std::fs::write(&backup_path, &backup).unwrap();
        fx.seed();
        assert!(!catalog.join(OLD_BASE).exists());
        assert!(local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"));
        assert_eq!(
            fx.record(&fx.project_key()).status,
            naming_migration::ScopeStatus::Complete
        );

        // Backup only (follow-up #2709): both renames done, every byte kept,
        // and the pass ends with the one pre-existing MIGRATION_CONFLICT.
        let only = B2Fixture::new();
        let catalog = only.project_catalog();
        let source = interrupted_1968_migration(&catalog, "after_backup");
        assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
        for pass in 1..=2 {
            let outcome = run_catalog_initialization_with_scope(
                &only.ac_dir(),
                Some(&only.legacy()),
                Some(only.journal_dir()),
                InitScope::Project,
            );
            let conflicts: Vec<_> = outcome
                .warnings
                .iter()
                .filter(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
                .collect();
            assert_eq!(conflicts.len(), 1, "pass {pass}: {:?}", outcome.warnings);
            assert!(!catalog.join(OLD_BASE).exists(), "pass {pass}");
            assert!(!catalog.join(OLD_LOCAL).exists(), "pass {pass}");
            assert_eq!(
                std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
                source,
                "pass {pass}"
            );
            let base: serde_json::Value = serde_json::from_slice(
                &std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            )
            .unwrap();
            assert_eq!(base["managed"]["owner"], "agentscommander", "pass {pass}");
            assert!(
                local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
                "pass {pass}"
            );
            assert!(deprecated_entries(&catalog).is_empty(), "pass {pass}");
            assert_eq!(
                only.record(&only.project_key()).status,
                naming_migration::ScopeStatus::Complete,
                "pass {pass}"
            );
        }
    }

    #[test]
    fn the_instance_directory_never_resumes_a_sidecar() {
        let config = tempfile::tempdir().unwrap();
        let catalog = catalog_dir(config.path());
        std::fs::create_dir_all(&catalog).unwrap();
        let base = legacy_catalog_json();
        let local = local_bytes("instance-local");
        let journal = b"{\"not\":\"ours\"}".to_vec();
        std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
        std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();
        std::fs::write(catalog.join(MIGRATION_JOURNAL_FILENAME), &journal).unwrap();

        ensure_seeded_instance_in(config.path(), Some(config.path()));

        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            base
        );
        assert_eq!(
            std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
            local
        );
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap(),
            journal
        );
        assert!(
            !catalog.join(MIGRATION_BACKUP_FILENAME).exists(),
            "no recovery ran"
        );
        let key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
        let record = naming_migration::read_journal(Some(config.path()))
            .unwrap()
            .unwrap()
            .scope(&key)
            .cloned()
            .unwrap();
        assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
        assert!(record
            .notes
            .iter()
            .any(|note| note.contains(MIGRATION_JOURNAL_FILENAME)));
    }

    #[test]
    fn happy_path_is_byte_preserving_and_idempotent() {
        let fx = B2Fixture::new();
        let catalog = fx.project_catalog();
        let base = preserved_base_bytes("mine");
        let local = local_bytes("happy");
        std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
        std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();
        std::fs::write(catalog.join(RETIRED_CATALOG_LOCK_FILENAME), b"").unwrap();

        fx.seed();
        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            base
        );
        assert_eq!(
            std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
            local
        );
        assert!(
            catalog.join(RETIRED_CATALOG_LOCK_FILENAME).is_file(),
            "lock not renamed"
        );
        let record = fx.record(&fx.project_key());
        assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
        assert!(record
            .notes
            .iter()
            .any(|note| note.contains(RETIRED_CATALOG_LOCK_FILENAME)));

        let before = fx.record(&fx.project_key());
        fx.seed();
        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            base
        );
        assert_eq!(
            std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
            local
        );
        assert_eq!(
            fx.record(&fx.project_key()),
            before,
            "a second run is a no-op"
        );
        assert!(deprecated_entries(&catalog).is_empty());
    }

    #[test]
    fn generated_project_policy_tracks_project_catalog_and_ignores_personal() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(temp.path())
                .output()
                .expect("git runs")
        };
        let init = git(&["init", "--quiet"]);
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let ac = temp.path().join(".ac");
        std::fs::create_dir_all(ac.join("coding-agents")).unwrap();
        crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[]).unwrap();
        for (name, expected) in [
            (
                crate::config::instance_artifacts::CODING_AGENTS_PROJECT_TARGET_NAME,
                1,
            ),
            (LOCAL_CATALOG_FILENAME, 0),
        ] {
            let relative = format!(".ac/coding-agents/{name}");
            std::fs::write(temp.path().join(&relative), b"user owned").unwrap();
            let result = git(&["check-ignore", "-q", &relative]);
            assert_eq!(
                result.status.code(),
                Some(expected),
                "{relative}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let result = git(&["check-ignore", "-q", ".ac/.gitignore"]);
        assert_eq!(
            result.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn a_pre_existing_target_wins_and_the_old_catalog_is_set_aside() {
        let fx = B2Fixture::new();
        let catalog = fx.project_catalog();
        let old = preserved_base_bytes("old");
        let winner = preserved_base_bytes("winner");
        std::fs::write(catalog.join(OLD_BASE), &old).unwrap();
        std::fs::write(catalog.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
        crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(
            &fx.ac_dir(),
            &["CLAUDE.md".to_string(), "AGENTS.md".to_string()],
        )
        .unwrap();

        fx.seed();

        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            winner
        );
        assert!(!catalog.join(OLD_BASE).exists());
        let set_aside = catalog.join("agents.json.deprecated-1.no-git");
        assert_eq!(std::fs::read(&set_aside).unwrap(), old);
        assert_eq!(
            deprecated_entries(&catalog),
            ["agents.json.deprecated-1.no-git"]
        );
        let record = fx.record(&fx.project_key());
        assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
        assert!(record.steps.iter().any(
            |step| step.from == OLD_BASE && step.state == naming_migration::StepState::SetAside
        ));

        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(fx.root())
                .output()
                .expect("git runs")
        };
        assert!(git(&["init", "-q"]).status.success());
        let ignored = git(&[
            "check-ignore",
            "-q",
            ".ac/coding-agents/agents.json.deprecated-1.no-git",
        ]);
        assert!(ignored.status.success(), "the set-aside catalog is ignored");
        let tracked = git(&[
            "check-ignore",
            "-q",
            ".ac/coding-agents/agents.10.default.json",
        ]);
        assert_eq!(
            tracked.status.code(),
            Some(1),
            "the new base stays trackable"
        );
    }

    #[test]
    fn an_unreachable_catalog_dir_is_recorded_and_retried() {
        let journal = tempfile::tempdir().unwrap();
        let base = preserved_base_bytes("mine");
        // A root that does not exist, and one that is a file: skipped before
        // the chokepoint, no scope row, nothing Complete.
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("missing");
        let file_root = parent.path().join("file-root");
        std::fs::write(&file_root, b"x").unwrap();
        for root in [&missing, &file_root] {
            ensure_seeded_for_project_in(root, None, None, Some(journal.path()));
        }
        assert!(naming_migration::read_journal(Some(journal.path()))
            .unwrap()
            .is_none());
        // A root whose `.ac` cannot be created: recorded Unreachable.
        let blocked = tempfile::tempdir().unwrap();
        let ac = blocked
            .path()
            .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
        std::fs::write(&ac, b"not a directory").unwrap();
        ensure_seeded_for_project_in(blocked.path(), None, None, Some(journal.path()));
        let keys = scope_keys(journal.path());
        assert_eq!(keys.len(), 1, "{keys:?}");
        let journal_now = naming_migration::read_journal(Some(journal.path()))
            .unwrap()
            .unwrap();
        assert_eq!(
            naming_migration::status(&journal_now, &keys[0]),
            Some(naming_migration::ScopeStatus::Unreachable)
        );

        // Each made reachable migrates on the next pass.
        std::fs::create_dir_all(missing.join(".ac").join(CATALOG_DIR_NAME)).unwrap();
        std::fs::remove_file(&file_root).unwrap();
        std::fs::create_dir_all(file_root.join(".ac").join(CATALOG_DIR_NAME)).unwrap();
        std::fs::remove_file(&ac).unwrap();
        std::fs::create_dir_all(ac.join(CATALOG_DIR_NAME)).unwrap();
        for root in [missing.as_path(), file_root.as_path(), blocked.path()] {
            let catalog = catalog_dir(&root.join(".ac"));
            std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
            ensure_seeded_for_project_in(root, None, None, Some(journal.path()));
            assert_eq!(
                std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
                base,
                "{}",
                root.display()
            );
            let key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
            let journal_now = naming_migration::read_journal(Some(journal.path()))
                .unwrap()
                .unwrap();
            assert_eq!(
                naming_migration::status(&journal_now, &key),
                Some(naming_migration::ScopeStatus::Complete)
            );
        }
    }

    #[test]
    fn a_new_project_still_imports_the_legacy_instance_catalog() {
        let registered = B2Fixture::new();
        let legacy = registered.legacy();
        std::fs::create_dir_all(&legacy).unwrap();
        let instance = legacy_catalog_json();
        std::fs::write(legacy.join(OLD_BASE), &instance).unwrap();
        // A project already registered, with its own catalog.
        std::fs::write(
            registered.project_catalog().join(OLD_BASE),
            preserved_base_bytes("registered"),
        )
        .unwrap();
        registered.seed();

        // A second project created later, same instance.
        let second = tempfile::tempdir().unwrap();
        ensure_seeded_for_project_in(
            second.path(),
            None,
            Some(&legacy),
            Some(registered.journal_dir()),
        );
        let catalog = catalog_dir(&second.path().join(".ac"));
        assert!(
            local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
            "the new project imported the real instance catalog"
        );
        assert_eq!(
            std::fs::read(legacy.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            instance
        );
        assert!(!legacy.join(OLD_BASE).exists());
        for key in [registered.legacy_key(), registered.project_key()] {
            assert_eq!(
                registered.record(&key).status,
                naming_migration::ScopeStatus::Complete
            );
        }
        let second_key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
        assert_eq!(
            registered.record(&second_key).status,
            naming_migration::ScopeStatus::Complete
        );
    }

    #[test]
    fn a_failed_legacy_scope_seeds_nothing() {
        let fx = B2Fixture::new();
        let legacy = fx.legacy();
        std::fs::create_dir_all(&legacy).unwrap();
        let instance = legacy_catalog_json();
        std::fs::write(legacy.join(OLD_BASE), &instance).unwrap();
        // The new lock sidecar is a directory: the scope's lock open fails
        // with an I/O refusal.
        std::fs::create_dir(legacy.join(CATALOG_LOCK_FILENAME)).unwrap();

        let outcome = run_catalog_initialization_with_scope(
            &fx.ac_dir(),
            Some(&legacy),
            Some(fx.journal_dir()),
            InitScope::Project,
        );
        let conflict = outcome
            .warnings
            .iter()
            .find(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
            .expect("a MIGRATION_CONFLICT outcome");
        assert!(conflict.reason.contains(OLD_BASE), "{}", conflict.reason);
        assert!(
            conflict.reason.contains(CATALOG_MANIFEST_FILENAME),
            "{}",
            conflict.reason
        );
        let catalog = fx.project_catalog();
        assert!(
            !catalog.join(CATALOG_MANIFEST_FILENAME).exists(),
            "no base written"
        );
        assert!(!catalog.join(LOCAL_CATALOG_FILENAME).exists());
        assert_eq!(std::fs::read(legacy.join(OLD_BASE)).unwrap(), instance);
        assert!(!legacy.join(CATALOG_MANIFEST_FILENAME).exists());

        // Both names present in the legacy dir: the winner is imported.
        let both = B2Fixture::new();
        let legacy = both.legacy();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(OLD_BASE), legacy_catalog_json()).unwrap();
        let winner = manifest_json(
            r##"[{"key":"winner","label":"Winner","description":"d","color":"#111","command":"win","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        std::fs::write(legacy.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
        both.seed();
        assert_eq!(
            std::fs::read_to_string(legacy.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            winner
        );
        assert_eq!(
            deprecated_entries(&legacy),
            ["agents.json.deprecated-1.no-git"]
        );
        assert!(local_carries(
            &both.project_catalog().join(LOCAL_CATALOG_FILENAME),
            "winner"
        ));
    }

    #[test]
    fn the_two_catalog_locks_never_nest() {
        let fx = B2Fixture::new();
        let legacy = fx.legacy();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(OLD_BASE), legacy_catalog_json()).unwrap();
        let canonical_legacy = std::fs::canonicalize(&legacy).unwrap();
        let project_catalog = std::fs::canonicalize(fx.project_catalog()).unwrap();
        let project_paths = CatalogPaths::new(&project_catalog);
        let armed = naming_migration::pause::arm("before_rename", &canonical_legacy);

        let root = fx.root().to_path_buf();
        let journal = fx.journal_dir().to_path_buf();
        let worker = std::thread::spawn(move || {
            ensure_seeded_for_project_in(&root, None, Some(&legacy), Some(&journal));
        });
        armed
            .reached
            .recv_timeout(Duration::from_secs(30))
            .expect("the legacy scope reached its rename");
        // Leg 1: the project's section has not started.
        assert!(
            !project_paths.lock.exists(),
            "acquire_catalog_lock for the project has not run"
        );
        // Leg 2: the project lock is free while the legacy scope is held.
        let probe = acquire_catalog_lock(&project_paths);
        assert!(probe.is_ok(), "the project lock is free: {:?}", probe.err());
        drop(probe);
        armed.release.send(()).unwrap();
        worker.join().expect("worker completes");
        assert!(project_catalog.join(CATALOG_MANIFEST_FILENAME).exists());
    }

    #[test]
    fn a_recovered_publication_is_still_recorded() {
        for point in ["after_local", "after_backup"] {
            // The pass's outcome carries the recovery's instant.
            let fx = B2Fixture::new();
            interrupted_1968_migration(&fx.project_catalog(), point);
            let outcome = run_catalog_initialization_with_scope(
                &fx.ac_dir(),
                Some(&fx.legacy()),
                Some(fx.journal_dir()),
                InitScope::Project,
            );
            assert!(
                outcome.published_at.is_some(),
                "{point}: {:?}",
                outcome.warnings
            );
            let conflicts = outcome
                .warnings
                .iter()
                .filter(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
                .count();
            assert_eq!(conflicts, usize::from(point == "after_backup"), "{point}");

            // Under the held project gate it is recorded exactly once.
            let gated = B2Fixture::new();
            interrupted_1968_migration(&gated.project_catalog(), point);
            let token = ManifestActivationToken::for_test();
            ensure_seeded_for_project_in(
                gated.root(),
                Some(&token),
                Some(&gated.legacy()),
                Some(gated.journal_dir()),
            );
            assert!(
                has_catalog_publication(gated.root()).unwrap(),
                "{point}: the publication row was recorded"
            );
            let manifest =
                std::fs::read_to_string(gated.ac_dir().join(SEED_MANIFEST_FILENAME)).unwrap();
            assert_eq!(
                manifest.matches("kind = \"coding_agent_catalog\"").count(),
                1,
                "{point}: {manifest}"
            );
        }
    }

    #[test]
    fn the_migration_does_not_re_acquire_the_catalog_lock() {
        let fx = B2Fixture::new();
        let catalog = fx.project_catalog();
        std::fs::write(catalog.join(OLD_BASE), preserved_base_bytes("mine")).unwrap();
        std::fs::write(catalog.join(OLD_LOCAL), local_bytes("lock")).unwrap();
        let started = Instant::now();
        run_catalog_initialization_with_scope(
            &fx.ac_dir(),
            None,
            Some(fx.journal_dir()),
            InitScope::Project,
        );
        assert!(started.elapsed() < naming_migration::MIGRATION_LOCK_BUDGET);
        assert!(catalog.join(CATALOG_MANIFEST_FILENAME).is_file());
        assert!(catalog.join(LOCAL_CATALOG_FILENAME).is_file());
        assert!(!catalog.join(OLD_BASE).exists());
        assert!(!catalog.join(OLD_LOCAL).exists());
    }

    const B2_CHILD_ROOT_ENV: &str = "AC_2715_CATALOG_CHILD_ROOT";
    const B2_CHILD_JOURNAL_ENV: &str = "AC_2715_CATALOG_CHILD_JOURNAL";
    const B2_CHILD_TEST_FQN: &str =
        "config::coding_agents_catalog::tests::b2_catalog_migration_child";

    #[test]
    fn b2_catalog_migration_child() {
        let (Some(root), Some(journal)) = (
            std::env::var_os(B2_CHILD_ROOT_ENV),
            std::env::var_os(B2_CHILD_JOURNAL_ENV),
        ) else {
            return;
        };
        ensure_seeded_for_project_in(Path::new(&root), None, None, Some(Path::new(&journal)));
        println!("AC_2715_CATALOG_CHILD_DONE");
    }

    #[test]
    fn two_catalog_scopes_keep_both_record_sets() {
        let journal = tempfile::tempdir().unwrap();
        let rendezvous = tempfile::tempdir().unwrap();
        let projects = [B2Fixture::new(), B2Fixture::new()];
        let bytes = [
            preserved_base_bytes("first"),
            preserved_base_bytes("second"),
        ];
        for (fx, base) in projects.iter().zip(&bytes) {
            std::fs::write(fx.project_catalog().join(OLD_BASE), base).unwrap();
            std::fs::write(fx.project_catalog().join(OLD_LOCAL), local_bytes("two")).unwrap();
        }
        // The child migrates the first project and pauses mid-scope, holding
        // that project's locks while its first journal records exist.
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                B2_CHILD_TEST_FQN,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(B2_CHILD_ROOT_ENV, projects[0].root())
            .env(B2_CHILD_JOURNAL_ENV, journal.path())
            .env(naming_migration::pause::PAUSE_DIR_ENV, rendezvous.path())
            .env(naming_migration::pause::PAUSE_STAGE_ENV, "after_rename")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn the migration child");
        wait_for_path(
            &rendezvous.path().join(naming_migration::pause::READY_FILE),
            Duration::from_secs(60),
            "migration child",
        );
        // This process migrates the second project in full meanwhile.
        ensure_seeded_for_project_in(projects[1].root(), None, None, Some(journal.path()));
        std::fs::write(
            rendezvous
                .path()
                .join(naming_migration::pause::RELEASE_FILE),
            b"go",
        )
        .unwrap();
        let output = child.wait_with_output().expect("reap the child");
        assert!(
            output.status.success(),
            "child: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("AC_2715_CATALOG_CHILD_DONE"));

        let journal_now = naming_migration::read_journal(Some(journal.path()))
            .unwrap()
            .unwrap();
        for (fx, base) in projects.iter().zip(&bytes) {
            let key = fx.project_key();
            let record = journal_now
                .scope(&key)
                .cloned()
                .expect("both scopes recorded");
            assert_eq!(
                record.status,
                naming_migration::ScopeStatus::Complete,
                "{key}"
            );
            for rename in &CATALOG_FAMILY_RENAMES {
                assert!(
                    record.steps.iter().any(|step| step.from == rename.from
                        && step.state == naming_migration::StepState::Renamed),
                    "{key}: {} step: {:?}",
                    rename.from,
                    record.steps
                );
            }
            assert_eq!(
                std::fs::read(fx.project_catalog().join(CATALOG_MANIFEST_FILENAME)).unwrap(),
                *base
            );
        }
    }

    #[test]
    fn the_instance_directory_has_one_scope_key() {
        let config = tempfile::tempdir().unwrap();
        let catalog = config.path().join(CATALOG_DIR_NAME);
        std::fs::create_dir_all(&catalog).unwrap();
        std::fs::write(catalog.join(OLD_BASE), legacy_catalog_json()).unwrap();
        let mut spellings = vec![
            catalog.clone(),
            config.path().join(".").join(CATALOG_DIR_NAME),
        ];
        if cfg!(windows) {
            spellings.push(PathBuf::from(catalog.to_string_lossy().to_uppercase()));
        }
        for legacy in &spellings {
            let project = tempfile::tempdir().unwrap();
            ensure_seeded_for_project_in(project.path(), None, Some(legacy), Some(config.path()));
        }
        ensure_seeded_instance_in(config.path(), Some(config.path()));
        let instance_key = catalog_scope_key(&ensure_catalog_dir(config.path()).unwrap());
        let keys = scope_keys(config.path());
        let for_instance: Vec<_> = keys
            .iter()
            .filter(|key| key.to_lowercase() == instance_key.to_lowercase())
            .collect();
        assert_eq!(for_instance, [&instance_key], "{keys:?}");
    }

    #[test]
    fn a_complete_scope_whose_old_name_is_back_is_re_run() {
        for both_present in [false, true] {
            let fx = B2Fixture::new();
            let catalog = std::fs::canonicalize(fx.project_catalog()).unwrap();
            let key = fx.project_key();
            naming_migration::update_journal(Some(fx.journal_dir()), |journal| {
                for rename in &CATALOG_FAMILY_RENAMES {
                    journal.record_step(
                        &key,
                        naming_migration::StepRecord::new(
                            &catalog,
                            rename.from,
                            rename.to,
                            naming_migration::StepState::Renamed,
                        ),
                    );
                }
                journal.set_status(&key, naming_migration::ScopeStatus::Complete);
            })
            .unwrap();
            let old = preserved_base_bytes("back");
            std::fs::write(catalog.join(OLD_BASE), &old).unwrap();
            let winner = preserved_base_bytes("winner");
            if both_present {
                std::fs::write(catalog.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
            }

            fx.seed();

            assert!(!catalog.join(OLD_BASE).exists(), "both={both_present}");
            let expected = if both_present { &winner } else { &old };
            assert_eq!(
                &std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
                expected,
                "both={both_present}: no fresh base"
            );
            if both_present {
                assert_eq!(
                    std::fs::read(catalog.join("agents.json.deprecated-1.no-git")).unwrap(),
                    old
                );
            } else {
                assert!(deprecated_entries(&catalog).is_empty());
            }
            assert_eq!(
                fx.record(&key).status,
                naming_migration::ScopeStatus::Complete
            );
        }
    }

    #[test]
    fn no_config_dir_still_migrates_and_never_seeds_a_second_base() {
        let fx = B2Fixture::new();
        let catalog = fx.project_catalog();
        let base = preserved_base_bytes("mine");
        let local = local_bytes("none");
        std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
        std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();

        ensure_seeded_for_project_in(fx.root(), None, None, None);

        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            base
        );
        assert_eq!(
            std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
            local
        );
        assert!(
            !catalog.join(OLD_BASE).exists(),
            "no base seeded beside agents.json"
        );
        let journal_names = |dir: &Path| -> Vec<String> {
            std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| name.contains("naming-migration"))
                .collect()
        };
        assert!(journal_names(&catalog).is_empty());
        assert!(journal_names(&fx.ac_dir()).is_empty());
        assert!(journal_names(fx.journal_dir()).is_empty());

        // A second pass with a real journal finds nothing to do.
        fx.seed();
        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            base
        );
        assert!(deprecated_entries(&catalog).is_empty(), "nothing demoted");
        assert_eq!(
            fx.record(&fx.project_key()).status,
            naming_migration::ScopeStatus::Complete
        );
    }

    // ---- #2021: no-project instance catalog seeding ------------------------

    /// The no-project report as the settings wrapper builds it for `config_dir`.
    fn instance_report(config_dir: &Path) -> CatalogReport {
        load_catalog_report_for_settings_with_config_dir(
            &AppSettings::default(),
            Some(config_dir.to_path_buf()),
        )
    }

    /// Catalog-directory entries minus the lock file that every initialization
    /// creates.
    fn non_lock_entries(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = dir_entries(&catalog_dir(root))
            .into_iter()
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        names.retain(|name| name != CATALOG_LOCK_FILENAME);
        names
    }

    /// AC-1: a fresh config dir with no project seeds the instance managed base
    /// and the no-project report lists exactly the supported built-ins.
    #[test]
    fn ensure_seeded_instance_fresh_config_dir_seeds_the_supported_builtins() {
        let instance = seed_dir();
        let published = ensure_seeded_instance(instance.path());
        assert!(published.is_some(), "an absent instance base is created");

        let report = instance_report(instance.path());
        assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
        let expected: Vec<String> = supported_shipped_definitions()
            .into_iter()
            .map(|definition| definition.key)
            .collect();
        let actual: Vec<String> = report
            .catalog
            .iter()
            .map(|definition| definition.key.clone())
            .collect();
        assert_eq!(
            actual, expected,
            "exactly the supported built-in keys in persisted order"
        );
        assert_eq!(
            std::fs::read(local_catalog_path(instance.path())).unwrap(),
            LOCAL_STUB_BYTES,
            "the create-once local stub is part of a fresh instance seed"
        );
    }

    /// AC-2: a second call leaves the base bytes alone and publishes nothing.
    #[test]
    fn ensure_seeded_instance_is_idempotent() {
        let instance = seed_dir();
        assert!(ensure_seeded_instance(instance.path()).is_some());
        let base_before = std::fs::read(manifest_path(instance.path())).unwrap();
        let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

        assert!(ensure_seeded_instance(instance.path()).is_none());
        assert_eq!(
            std::fs::read(manifest_path(instance.path())).unwrap(),
            base_before
        );
        assert_eq!(
            std::fs::read(local_catalog_path(instance.path())).unwrap(),
            local_before
        );
    }

    /// AC-3: a marker-less legacy instance file is never migrated, rewritten or
    /// accompanied by sidecars.
    #[test]
    fn ensure_seeded_instance_leaves_a_legacy_instance_file_untouched() {
        let instance = seed_dir();
        let legacy = String::from_utf8(legacy_catalog_json()).expect("utf-8 legacy fixture");
        write_legacy_base(instance.path(), &legacy);
        let bytes_before = std::fs::read(manifest_path(instance.path())).unwrap();

        assert!(ensure_seeded_instance(instance.path()).is_none());
        assert_eq!(
            std::fs::read(manifest_path(instance.path())).unwrap(),
            bytes_before
        );
        assert_eq!(
            non_lock_entries(instance.path()),
            vec![
                RETIRED_CATALOG_LOCK_FILENAME.to_string(),
                CATALOG_MANIFEST_FILENAME.to_string(),
            ],
            "a legacy instance file creates no local, backup or journal sidecar; the retired lock sidecar is the scope lock"
        );
        let report = instance_report(instance.path());
        assert!(report.unavailable.is_none());
        assert_eq!(
            report
                .catalog
                .iter()
                .map(|definition| definition.key.as_str())
                .collect::<Vec<_>>(),
            ["mine"],
            "the legacy instance file stays readable as before"
        );
    }

    /// AC-4: corrupt instance bytes are preserved and the report keeps the same
    /// unavailability code before and after the call.
    #[test]
    fn ensure_seeded_instance_leaves_a_corrupt_instance_file_untouched() {
        let instance = seed_dir();
        write_legacy_base(instance.path(), "{ not JSON");
        let bytes_before = std::fs::read(manifest_path(instance.path())).unwrap();
        let code_before = instance_report(instance.path())
            .unavailable
            .as_ref()
            .map(|diagnostic| diagnostic.code.clone());
        assert_eq!(code_before.as_deref(), Some("baseInvalid"));

        assert!(ensure_seeded_instance(instance.path()).is_none());
        assert_eq!(
            std::fs::read(manifest_path(instance.path())).unwrap(),
            bytes_before
        );
        let code_after = instance_report(instance.path())
            .unavailable
            .as_ref()
            .map(|diagnostic| diagnostic.code.clone());
        assert_eq!(code_after, code_before);
    }

    /// AC-5: a MANAGED instance base must not be imported as legacy territory
    /// by a later project; the project gets a fresh managed base instead of
    /// MIGRATION_CONFLICT. This drives the same production initialization path
    /// that `ensure_seeded_for_project_with_token` uses, with the instance
    /// catalog dir injected explicitly because the process-global `config_dir()`
    /// has no test seam.
    #[test]
    fn managed_instance_base_does_not_block_a_later_project_fresh_seed() {
        let instance = seed_dir();
        assert!(ensure_seeded_instance(instance.path()).is_some());
        assert_eq!(
            base_json(instance.path())["managed"]["owner"],
            "agentscommander"
        );

        let project = seed_dir();
        let ac_dir = ac_dir_for(project.path());
        let outcome =
            run_catalog_initialization(&ac_dir, Some(&catalog_dir(instance.path())), None);
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|warning| warning.code == "migrationConflict"),
            "a managed instance base must not be imported as legacy territory: {:?}",
            outcome.warnings
        );
        assert!(
            outcome.published_at.is_some(),
            "the project fresh-seeds its own managed base"
        );

        let base = base_json(&ac_dir);
        assert_eq!(base["managed"]["owner"], "agentscommander");
        assert_eq!(
            std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
            LOCAL_STUB_BYTES,
            "the project's own local stub is published"
        );
        let entries = non_lock_entries(&ac_dir);
        assert!(
            !entries.contains(&MIGRATION_BACKUP_FILENAME.to_string())
                && !entries.contains(&MIGRATION_JOURNAL_FILENAME.to_string()),
            "no project backup or journal may be written: {entries:?}"
        );

        let report = load_catalog_report_for_settings_with_config_dir(
            &AppSettings {
                project_paths: vec![project.path().to_string_lossy().to_string()],
                ..AppSettings::default()
            },
            None,
        );
        assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
        assert!(
            !report
                .warnings
                .iter()
                .any(|warning| warning.code == "migrationConflict"),
            "the managed instance base must not be imported: {:?}",
            report.warnings
        );
        assert_eq!(report.catalog.len(), 8);
    }

    /// AC-8: an existing instance local layer still wins over the seeded base
    /// with no project registered.
    #[test]
    fn ensure_seeded_instance_local_overrides_win_with_no_project() {
        let instance = seed_dir();
        write_local(
            instance.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MY CLAUDE","updateCommands":["my-claude-update"]}]}"##,
        );
        let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

        assert!(ensure_seeded_instance(instance.path()).is_some());
        assert_eq!(
            std::fs::read(local_catalog_path(instance.path())).unwrap(),
            local_before
        );

        let report = instance_report(instance.path());
        assert!(report.unavailable.is_none());
        let claude = report
            .catalog
            .iter()
            .find(|definition| definition.key == "claude")
            .expect("claude present");
        assert_eq!(claude.label, "MY CLAUDE");
        assert_eq!(
            claude.update_commands,
            vec!["my-claude-update".to_string()],
            "the local override wins over the shipped command list"
        );
        let codex = report
            .catalog
            .iter()
            .find(|definition| definition.key == "codex")
            .expect("codex present");
        assert_eq!(codex.label, "Codex", "unpinned agents keep shipped values");
    }

    /// AC-9: a stale verified managed instance base is refreshed to the current
    /// fresh-seed bytes; an edited one is never replaced.
    #[test]
    fn ensure_seeded_instance_refreshes_only_a_stale_verified_base() {
        let stale = seed_dir();
        let mut claude = shipped_def_json(&["claude"]).remove(0);
        claude["label"] = serde_json::json!("OLD LABEL");
        write_managed_base(stale.path(), &[claude], "stale-revision", true);
        assert!(
            ensure_seeded_instance(stale.path()).is_some(),
            "a stale managed instance base refreshes"
        );
        let refreshed = std::fs::read(manifest_path(stale.path())).unwrap();
        assert_eq!(
            base_json(stale.path())["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
        let fresh = seed_dir();
        assert!(ensure_seeded_instance(fresh.path()).is_some());
        assert_eq!(
            refreshed,
            std::fs::read(manifest_path(fresh.path())).unwrap(),
            "the refreshed bytes equal a fresh instance seed"
        );
        let report = instance_report(stale.path());
        assert!(
            !report
                .warnings
                .iter()
                .any(|warning| warning.code == "refreshFailed"),
            "a successful refresh leaves no stale-revision warning: {:?}",
            report.warnings
        );
        assert_eq!(report.catalog.len(), 8);

        // Edited managed base: readable but never replaced.
        let edited = seed_dir();
        write_managed_base(
            edited.path(),
            &shipped_def_json(&["claude"]),
            "stale-revision",
            false,
        );
        let edited_before = std::fs::read(manifest_path(edited.path())).unwrap();
        assert!(ensure_seeded_instance(edited.path()).is_none());
        assert_eq!(
            std::fs::read(manifest_path(edited.path())).unwrap(),
            edited_before
        );
    }

    /// AC-10: an existing local layer with an absent base is kept verbatim and
    /// its overrides show in the report.
    #[test]
    fn ensure_seeded_instance_keeps_an_existing_local_with_an_absent_base() {
        let instance = seed_dir();
        write_local(
            instance.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"pi","label":"PINNED PI"}]}"##,
        );
        let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

        assert!(ensure_seeded_instance(instance.path()).is_some());
        assert_eq!(
            std::fs::read(local_catalog_path(instance.path())).unwrap(),
            local_before,
            "the create-once stub never overwrites an existing local layer"
        );
        let report = instance_report(instance.path());
        assert!(report.unavailable.is_none());
        let pi = report
            .catalog
            .iter()
            .find(|definition| definition.key == "pi")
            .expect("pi present");
        assert_eq!(pi.label, "PINNED PI");
    }

    /// 2.2 instance policy: a foreign journal beside the instance catalog is
    /// never resumed, rewritten or removed; the base still fresh-seeds.
    #[test]
    fn ensure_seeded_instance_never_resumes_a_foreign_sidecar() {
        let instance = seed_dir();
        let journal_path = catalog_dir(instance.path()).join(MIGRATION_JOURNAL_FILENAME);
        std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
        std::fs::write(&journal_path, b"{ foreign journal }").unwrap();
        let journal_before = std::fs::read(&journal_path).unwrap();

        assert!(
            ensure_seeded_instance(instance.path()).is_some(),
            "the instance base still fresh-seeds"
        );
        assert_eq!(
            std::fs::read(&journal_path).unwrap(),
            journal_before,
            "the instance never resumes or rewrites a foreign journal"
        );
    }

    #[test]
    fn command_account_key_drops_arguments_and_keeps_the_program_token() {
        assert_eq!(command_account_key("claude"), Some("claude".to_string()));
        assert_eq!(
            command_account_key("claude --dangerously-skip-permissions"),
            Some("claude".to_string())
        );
    }

    #[test]
    fn command_account_key_separates_two_paths_to_the_same_name() {
        assert_eq!(
            command_account_key("/a/claude"),
            Some("/a/claude".to_string())
        );
        assert_ne!(
            command_account_key("/a/claude"),
            command_account_key("claude")
        );
    }

    #[test]
    fn command_account_key_is_none_for_an_empty_command() {
        assert_eq!(command_account_key(""), None);
        assert_eq!(command_account_key("   "), None);
    }

    #[test]
    fn command_account_key_honours_quoting() {
        assert_eq!(
            command_account_key("\"C:/Program Files/a/claude.exe\" --x"),
            Some("C:/Program Files/a/claude.exe".to_string())
        );
    }

    fn welcome_def_2736(
        key: &str,
        command: &str,
        install: Option<serde_json::Value>,
    ) -> CodingAgentDefinition {
        let mut raw = serde_json::json!({
            "key": key,
            "label": key,
            "description": "d",
            "color": "#000000",
            "command": command,
            "envs": [],
            "isolatedHome": false,
            "removable": true
        });
        if let Some(install) = install {
            raw["installCommands"] = install;
        }
        serde_json::from_value(raw).expect("fixture definition")
    }

    #[test]
    fn tested_level_2736_table_keys_equal_the_enabled_support_rows_in_order() {
        let tested: Vec<&str> = BUILTIN_TESTED_LEVEL.iter().map(|(key, _)| *key).collect();
        let enabled: Vec<&str> = BUILTIN_AGENT_SUPPORT
            .iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(tested, enabled);
    }

    #[test]
    fn tested_level_2736_assigns_high_medium_low_exactly_as_specified() {
        for key in ["claude", "codex", "pi"] {
            assert_eq!(tested_level_for(key), Some(TestedLevel::High), "{key}");
        }
        assert_eq!(tested_level_for("antigravity"), Some(TestedLevel::Medium));
        for key in ["hermes", "cursor", "opencode", "grok"] {
            assert_eq!(tested_level_for(key), Some(TestedLevel::Low), "{key}");
        }
        assert_eq!(tested_level_for("muse"), None);
        assert_eq!(tested_level_for("my-agent"), None);
    }

    #[test]
    fn tested_level_2736_serializes_to_lowercase_wire_strings() {
        assert_eq!(
            serde_json::to_value(TestedLevel::High).unwrap(),
            serde_json::json!("high")
        );
        assert_eq!(
            serde_json::to_value(TestedLevel::Medium).unwrap(),
            serde_json::json!("medium")
        );
        assert_eq!(
            serde_json::to_value(TestedLevel::Low).unwrap(),
            serde_json::json!("low")
        );
    }

    #[test]
    fn install_command_2736_prefers_the_platform_key_over_default() {
        let commands = InstallCommands {
            default: "npm i -g x".to_string(),
            windows: Some("winget install x".to_string()),
            macos: None,
            linux: None,
        };
        assert_eq!(
            install_command_for_os(&commands, "windows"),
            "winget install x"
        );
        assert_eq!(install_command_for_os(&commands, "macos"), "npm i -g x");
        assert_eq!(install_command_for_os(&commands, "linux"), "npm i -g x");
        let all = InstallCommands {
            default: "d".to_string(),
            windows: Some("w".to_string()),
            macos: Some("m".to_string()),
            linux: Some("l".to_string()),
        };
        assert_eq!(install_command_for_os(&all, "windows"), "w");
        assert_eq!(install_command_for_os(&all, "macos"), "m");
        assert_eq!(install_command_for_os(&all, "linux"), "l");
    }

    #[test]
    fn install_command_2736_unknown_os_name_falls_back_to_default() {
        let all = InstallCommands {
            default: "d".to_string(),
            windows: Some("w".to_string()),
            macos: Some("m".to_string()),
            linux: Some("l".to_string()),
        };
        assert_eq!(install_command_for_os(&all, "freebsd"), "d");
        assert_eq!(install_command_for_os(&all, ""), "d");
    }

    #[test]
    fn command_is_present_2736_true_for_a_resolvable_token_false_otherwise() {
        let stub = |token: &str| (token == "claude").then(|| PathBuf::from("/bin/claude"));
        assert!(command_is_present_with("claude", stub));
        assert!(command_is_present_with("claude --flag", stub));
        assert!(!command_is_present_with("codex", stub));
        assert!(!command_is_present_with("", stub));
        assert!(!command_is_present_with("   ", stub));
    }

    #[test]
    fn command_is_present_2736_executes_no_process() {
        // The injected closure is the ONLY resolution step: no version probe
        // (which would spawn a process) sits on this path.
        let mut calls = Vec::new();
        let present = command_is_present_with("\"my tool\" --x", |token| {
            calls.push(token.to_string());
            None
        });
        assert!(!present);
        assert_eq!(calls, vec!["my tool".to_string()]);
        let mut empty_calls = 0;
        assert!(!command_is_present_with("", |_| {
            empty_calls += 1;
            Some(PathBuf::from("x"))
        }));
        assert_eq!(empty_calls, 0);
    }

    #[test]
    fn welcome_status_2736_row_per_entry_in_catalog_order_with_computed_fields() {
        let catalog = vec![
            welcome_def_2736(
                "codex",
                "codex",
                Some(serde_json::json!({ "default": "npm i -g codex" })),
            ),
            welcome_def_2736("claude", "claude --x", None),
            welcome_def_2736(
                "my-agent",
                "my-agent",
                Some(
                    serde_json::json!({ "default": "d", "windows": "d", "macos": "d", "linux": "d" }),
                ),
            ),
        ];
        let rows = welcome_status_for_with(&catalog, |command| command == "claude --x");
        assert_eq!(
            rows,
            vec![
                CodingAgentWelcomeStatus {
                    key: "codex".to_string(),
                    installed: false,
                    tested_level: Some(TestedLevel::High),
                    install_command: Some("npm i -g codex".to_string()),
                },
                CodingAgentWelcomeStatus {
                    key: "claude".to_string(),
                    installed: true,
                    tested_level: Some(TestedLevel::High),
                    install_command: None,
                },
                CodingAgentWelcomeStatus {
                    key: "my-agent".to_string(),
                    installed: false,
                    tested_level: None,
                    install_command: Some("d".to_string()),
                },
            ]
        );
        let wire = serde_json::to_value(&rows).unwrap();
        assert_eq!(wire[0]["testedLevel"], serde_json::json!("high"));
        assert_eq!(wire[1]["installCommand"], serde_json::Value::Null);
        assert_eq!(wire[2]["testedLevel"], serde_json::Value::Null);
    }

    #[test]
    fn welcome_status_2736_never_adds_a_field_to_the_persisted_definition() {
        let def = welcome_def_2736(
            "claude",
            "claude",
            Some(serde_json::json!({ "default": "d" })),
        );
        let value = serde_json::to_value(&def).unwrap();
        let object = value.as_object().expect("definition object");
        assert!(!object.contains_key("testedLevel"));
        assert!(!object.contains_key("installed"));
    }

    const SCOPE_B_EXPECTED_2800: &str = r#####"{"claude":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwBsAGEAdQBkAGUALgBhAGkALwBpAG4AcwB0AGEAbABsAC4AcABzADEAJwAgAC0AVABpAG0AZQBvAHUAdABTAGUAYwAgADEAMgAwACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwADsAIABpAGYAIAAoAFsAcwB0AHIAaQBuAGcAXQA6ADoASQBzAE4AdQBsAGwATwByAFcAaABpAHQAZQBTAHAAYQBjAGUAKAAkAGEAYwBfAGkAbgBzAHQAYQBsAGwAXwBzAGMAcgBpAHAAdAApACkAIAB7ACAAdABoAHIAbwB3ACAAJwBFAG0AcAB0AHkAIABpAG4AcwB0AGEAbABsAGUAcgAgAHIAZQBzAHAAbwBuAHMAZQAnACAAfQA7ACAAJABhAGMAXwBwAGEAdABoAD0ASgBvAGkAbgAtAFAAYQB0AGgAIAAoAFsASQBPAC4AUABhAHQAaABdADoAOgBHAGUAdABUAGUAbQBwAFAAYQB0AGgAKAApACkAIAAoACcAYQBjAC0AaQBuAHMAdABhAGwAbAAtADIANwA4ADcALQAnACsAWwBHAHUAaQBkAF0AOgA6AE4AZQB3AEcAdQBpAGQAKAApAC4AVABvAFMAdAByAGkAbgBnACgAJwBOACcAKQArACcALgBwAHMAMQAnACkAOwAgACQAYQBjAF8AZgBpAGwAZQA9AFsASQBPAC4ARgBpAGwAZQBdADoAOgBPAHAAZQBuACgAJABhAGMAXwBwAGEAdABoACwAWwBJAE8ALgBGAGkAbABlAE0AbwBkAGUAXQA6ADoAQwByAGUAYQB0AGUATgBlAHcALABbAEkATwAuAEYAaQBsAGUAQQBjAGMAZQBzAHMAXQA6ADoAVwByAGkAdABlACwAWwBJAE8ALgBGAGkAbABlAFMAaABhAHIAZQBdADoAOgBOAG8AbgBlACkAOwAgACQAYQBjAF8AYwByAGUAYQB0AGUAZAA9ACQAdAByAHUAZQA7ACAAdAByAHkAIAB7ACAAWwBiAHkAdABlAFsAXQBdACQAYQBjAF8AYgB5AHQAZQBzAD0AWwBUAGUAeAB0AC4ARQBuAGMAbwBkAGkAbgBnAF0AOgA6AFUAVABGADgALgBHAGUAdABQAHIAZQBhAG0AYgBsAGUAKAApACsAWwBUAGUAeAB0AC4ARQBuAGMAbwBkAGkAbgBnAF0AOgA6AFUAVABGADgALgBHAGUAdABCAHkAdABlAHMAKAAkAGEAYwBfAGkAbgBzAHQAYQBsAGwAXwBzAGMAcgBpAHAAdAApADsAIAAkAGEAYwBfAGYAaQBsAGUALgBXAHIAaQB0AGUAKAAkAGEAYwBfAGIAeQB0AGUAcwAsADAALAAkAGEAYwBfAGIAeQB0AGUAcwAuAEwAZQBuAGcAdABoACkAIAB9ACAAZgBpAG4AYQBsAGwAeQAgAHsAIAAkAGEAYwBfAGYAaQBsAGUALgBEAGkAcwBwAG8AcwBlACgAKQAgAH0AOwAgACQAZwBsAG8AYgBhAGwAOgBMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA9ACQAbgB1AGwAbAA7ACAAJgAgACgASgBvAGkAbgAtAFAAYQB0AGgAIAAkAFAAUwBIAE8ATQBFACAAJwBwAG8AdwBlAHIAcwBoAGUAbABsAC4AZQB4AGUAJwApACAALQBOAG8AUAByAG8AZgBpAGwAZQAgAC0ATgBvAG4ASQBuAHQAZQByAGEAYwB0AGkAdgBlACAALQBFAHgAZQBjAHUAdABpAG8AbgBQAG8AbABpAGMAeQAgAEIAeQBwAGEAcwBzACAALQBGAGkAbABlACAAJABhAGMAXwBwAGEAdABoADsAIAAkAGEAYwBfAGUAeABpAHQAPQAkAEwAQQBTAFQARQBYAEkAVABDAE8ARABFADsAIABpAGYAIAAoACQAbgB1AGwAbAAgAC0AZQBxACAAJABhAGMAXwBlAHgAaQB0ACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAYwBhAHQAYwBoACAAewAgAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0AGUATABpAG4AZQAoACQAXwApADsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAaQBmACAAKAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAKQAgAHsAIAB0AHIAeQAgAHsAIABSAGUAbQBvAHYAZQAtAEkAdABlAG0AIAAtAEwAaQB0AGUAcgBhAGwAUABhAHQAaAAgACQAYQBjAF8AcABhAHQAaAAgAC0ARgBvAHIAYwBlACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAaQBmACAAKAAkAGEAYwBfAGUAeABpAHQAIAAtAGUAcQAgADAAKQAgAHsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAH0AIAB9ACAAfQA7ACAAZQB4AGkAdAAgACQAYQBjAF8AZQB4AGkAdAA=","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://claude.ai/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://claude.ai/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"},"codex":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwBoAGEAdABnAHAAdAAuAGMAbwBtAC8AYwBvAGQAZQB4AC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAEYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://chatgpt.com/codex/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | sh","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://chatgpt.com/codex/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | sh","default":"echo No verified installer for this platform 1>&2 && exit 1"},"hermes":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AaABlAHIAbQBlAHMALQBhAGcAZQBuAHQALgBuAG8AdQBzAHIAZQBzAGUAYQByAGMAaAAuAGMAbwBtAC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAAgAC0ATgBvAG4ASQBuAHQAZQByAGEAYwB0AGkAdgBlADsAIAAkAGEAYwBfAGUAeABpAHQAPQAkAEwAQQBTAFQARQBYAEkAVABDAE8ARABFADsAIABpAGYAIAAoACQAbgB1AGwAbAAgAC0AZQBxACAAJABhAGMAXwBlAHgAaQB0ACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAYwBhAHQAYwBoACAAewAgAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0AGUATABpAG4AZQAoACQAXwApADsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAaQBmACAAKAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAKQAgAHsAIAB0AHIAeQAgAHsAIABSAGUAbQBvAHYAZQAtAEkAdABlAG0AIAAtAEwAaQB0AGUAcgBhAGwAUABhAHQAaAAgACQAYQBjAF8AcABhAHQAaAAgAC0ARgBvAHIAYwBlACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAaQBmACAAKAAkAGEAYwBfAGUAeABpAHQAIAAtAGUAcQAgADAAKQAgAHsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAH0AIAB9ACAAfQA7ACAAZQB4AGkAdAAgACQAYQBjAF8AZQB4AGkAdAA=","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://hermes-agent.nousresearch.com/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash -s -- --non-interactive","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://hermes-agent.nousresearch.com/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash -s -- --non-interactive","default":"echo No verified installer for this platform 1>&2 && exit 1"},"cursor":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwB1AHIAcwBvAHIALgBjAG8AbQAvAGkAbgBzAHQAYQBsAGwAPwB3AGkAbgAzADIAPQB0AHIAdQBlACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAEYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://cursor.com/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://cursor.com/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"},"pi":{"windows":"npm install -g --ignore-scripts --include=optional @earendil-works/pi-coding-agent","macos":"npm install -g --ignore-scripts --include=optional @earendil-works/pi-coding-agent","linux":"npm install -g --ignore-scripts --include=optional @earendil-works/pi-coding-agent","default":"echo No verified installer for this platform 1>&2 && exit 1"},"opencode":{"windows":"npm install -g --ignore-scripts=false --include=optional --allow-scripts=opencode-ai opencode-ai","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://opencode.ai/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://opencode.ai/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"},"antigravity":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYQBuAHQAaQBnAHIAYQB2AGkAdAB5AC4AZwBvAG8AZwBsAGUALwBjAGwAaQAvAGkAbgBzAHQAYQBsAGwALgBwAHMAMQAnACAALQBUAGkAbQBlAG8AdQB0AFMAZQBjACAAMQAyADAAIAAtAEUAcgByAG8AcgBBAGMAdABpAG8AbgAgAFMAdABvAHAAOwAgAGkAZgAgACgAWwBzAHQAcgBpAG4AZwBdADoAOgBJAHMATgB1AGwAbABPAHIAVwBoAGkAdABlAFMAcABhAGMAZQAoACQAYQBjAF8AaQBuAHMAdABhAGwAbABfAHMAYwByAGkAcAB0ACkAKQAgAHsAIAB0AGgAcgBvAHcAIAAnAEUAbQBwAHQAeQAgAGkAbgBzAHQAYQBsAGwAZQByACAAcgBlAHMAcABvAG4AcwBlACcAIAB9ADsAIAAkAGEAYwBfAHAAYQB0AGgAPQBKAG8AaQBuAC0AUABhAHQAaAAgACgAWwBJAE8ALgBQAGEAdABoAF0AOgA6AEcAZQB0AFQAZQBtAHAAUABhAHQAaAAoACkAKQAgACgAJwBhAGMALQBpAG4AcwB0AGEAbABsAC0AMgA3ADgANwAtACcAKwBbAEcAdQBpAGQAXQA6ADoATgBlAHcARwB1AGkAZAAoACkALgBUAG8AUwB0AHIAaQBuAGcAKAAnAE4AJwApACsAJwAuAHAAcwAxACcAKQA7ACAAJABhAGMAXwBmAGkAbABlAD0AWwBJAE8ALgBGAGkAbABlAF0AOgA6AE8AcABlAG4AKAAkAGEAYwBfAHAAYQB0AGgALABbAEkATwAuAEYAaQBsAGUATQBvAGQAZQBdADoAOgBDAHIAZQBhAHQAZQBOAGUAdwAsAFsASQBPAC4ARgBpAGwAZQBBAGMAYwBlAHMAcwBdADoAOgBXAHIAaQB0AGUALABbAEkATwAuAEYAaQBsAGUAUwBoAGEAcgBlAF0AOgA6AE4AbwBuAGUAKQA7ACAAJABhAGMAXwBjAHIAZQBhAHQAZQBkAD0AJAB0AHIAdQBlADsAIAB0AHIAeQAgAHsAIABbAGIAeQB0AGUAWwBdAF0AJABhAGMAXwBiAHkAdABlAHMAPQBbAFQAZQB4AHQALgBFAG4AYwBvAGQAaQBuAGcAXQA6ADoAVQBUAEYAOAAuAEcAZQB0AFAAcgBlAGEAbQBiAGwAZQAoACkAKwBbAFQAZQB4AHQALgBFAG4AYwBvAGQAaQBuAGcAXQA6ADoAVQBUAEYAOAAuAEcAZQB0AEIAeQB0AGUAcwAoACQAYQBjAF8AaQBuAHMAdABhAGwAbABfAHMAYwByAGkAcAB0ACkAOwAgACQAYQBjAF8AZgBpAGwAZQAuAFcAcgBpAHQAZQAoACQAYQBjAF8AYgB5AHQAZQBzACwAMAAsACQAYQBjAF8AYgB5AHQAZQBzAC4ATABlAG4AZwB0AGgAKQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgACQAYQBjAF8AZgBpAGwAZQAuAEQAaQBzAHAAbwBzAGUAKAApACAAfQA7ACAAJABnAGwAbwBiAGEAbAA6AEwAQQBTAFQARQBYAEkAVABDAE8ARABFAD0AJABuAHUAbABsADsAIAAmACAAKABKAG8AaQBuAC0AUABhAHQAaAAgACQAUABTAEgATwBNAEUAIAAnAHAAbwB3AGUAcgBzAGgAZQBsAGwALgBlAHgAZQAnACkAIAAtAE4AbwBQAHIAbwBmAGkAbABlACAALQBOAG8AbgBJAG4AdABlAHIAYQBjAHQAaQB2AGUAIAAtAEUAeABlAGMAdQB0AGkAbwBuAFAAbwBsAGkAYwB5ACAAQgB5AHAAYQBzAHMAIAAtAEYAaQBsAGUAIAAkAGEAYwBfAHAAYQB0AGgAOwAgACQAYQBjAF8AZQB4AGkAdAA9ACQATABBAFMAVABFAFgASQBUAEMATwBEAEUAOwAgAGkAZgAgACgAJABuAHUAbABsACAALQBlAHEAIAAkAGEAYwBfAGUAeABpAHQAKQAgAHsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAZgBpAG4AYQBsAGwAeQAgAHsAIABpAGYAIAAoACQAYQBjAF8AYwByAGUAYQB0AGUAZAApACAAewAgAHQAcgB5ACAAewAgAFIAZQBtAG8AdgBlAC0ASQB0AGUAbQAgAC0ATABpAHQAZQByAGEAbABQAGEAdABoACAAJABhAGMAXwBwAGEAdABoACAALQBGAG8AcgBjAGUAIAAtAEUAcgByAG8AcgBBAGMAdABpAG8AbgAgAFMAdABvAHAAIAB9ACAAYwBhAHQAYwBoACAAewAgAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0AGUATABpAG4AZQAoACQAXwApADsAIABpAGYAIAAoACQAYQBjAF8AZQB4AGkAdAAgAC0AZQBxACAAMAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAH0AIAB9ADsAIABlAHgAaQB0ACAAJABhAGMAXwBlAHgAaQB0AA==","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://antigravity.google/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://antigravity.google/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"},"grok":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AeAAuAGEAaQAvAGMAbABpAC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAEYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"}}"#####;
    const SCOPE_B_NON_INSTALL_MAIN_2800: &str = r#####"{"schemaVersion":1,"agents":[{"key":"claude","label":"Claude Code","description":"Coding Agent by Anthropic","color":"#d97706","command":"claude","instructionsFilename":"CLAUDE.md","envs":[],"isolatedHome":false,"configSeed":{"enabled":true,"dest":".claude"},"removable":true,"updateCommands":["claude --update"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"codex","label":"Codex","description":"Coding Agent by OpenAI","color":"#10b981","command":"codex","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"configSeed":{"enabled":true,"dest":".codex"},"removable":true,"updateCommands":["codex update"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"hermes","label":"Hermes","description":"Coding Agent by Nous Research","color":"#8b5cf6","command":"hermes","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["hermes update --yes"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"cursor","label":"Cursor CLI","description":"Coding Agent by Cursor","color":"#22d3ee","command":"agent","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"pi","label":"Pi","description":"Coding Agent by Earendil Inc","color":"#ec4899","command":"pi","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["pi update"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"opencode","label":"OpenCode","description":"Open-source terminal coding agent by Anomaly","color":"#64748b","command":"opencode","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"configSeed":{"enabled":true,"dest":".opencode"},"removable":true,"updateCommands":["opencode upgrade"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"antigravity","label":"Antigravity","description":"Coding Agent by Google","color":"#4285F4","command":"agy","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["agy update"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"grok","label":"Grok Build","description":"Coding Agent by SpaceXAI","color":"#64748b","command":"grok","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"muse","label":"Muse Code","description":"Meta terminal coding agent (beta; macOS/Linux host only)","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false,"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}}]}"#####;

    fn scope_b_expected_commands_2800(key: &str) -> Option<serde_json::Value> {
        let expected: serde_json::Value = serde_json::from_str(SCOPE_B_EXPECTED_2800).unwrap();
        expected.get(key).cloned()
    }

    #[test]
    fn scope_b_catalog_2800_exact_32_cells() {
        let expected: serde_json::Value = serde_json::from_str(SCOPE_B_EXPECTED_2800).unwrap();
        let actual: serde_json::Value =
            serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
        assert_eq!(expected.as_object().unwrap().len(), 8);
        let mut cells = 0;
        for (key, commands) in expected.as_object().unwrap() {
            let row = actual["agents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["key"] == key.as_str())
                .unwrap();
            assert_eq!(row["installCommands"], *commands, "{key}");
            assert_eq!(commands.as_object().unwrap().len(), 4);
            for os in ["windows", "macos", "linux", "default"] {
                assert!(!commands[os].as_str().unwrap().is_empty());
                cells += 1;
            }
            let definition = embedded_default_catalog()
                .agents
                .into_iter()
                .find(|row| row.key == *key)
                .unwrap();
            let typed = definition.install_commands.unwrap();
            for os in ["windows", "macos", "linux", "default"] {
                assert_eq!(
                    install_command_for_os(&typed, os),
                    commands[os].as_str().unwrap()
                );
            }
        }
        assert_eq!(cells, 32);
        embedded_default_matches_current_presets_exactly();
        install_commands_2736_shipped_for_eight_builtins_and_absent_for_muse();
    }

    #[test]
    fn scope_b_catalog_2800_non_install_fields_unchanged() {
        let expected: serde_json::Value =
            serde_json::from_str(SCOPE_B_NON_INSTALL_MAIN_2800).unwrap();
        let mut actual: serde_json::Value =
            serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
        for row in actual["agents"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("installCommands");
        }
        assert_eq!(
            actual, expected,
            "pinned main fields/schema/order/ninth row"
        );
    }

    #[test]
    fn scope_b_catalog_2800_unknown_os_and_missing_override_use_default() {
        let mut commands = InstallCommands {
            default: "default sentinel".to_string(),
            windows: Some("windows sentinel".to_string()),
            macos: Some("macos sentinel".to_string()),
            linux: Some("linux sentinel".to_string()),
        };
        for unknown in ["unknown", "freebsd", "android", ""] {
            assert_eq!(
                install_command_for_os(&commands, unknown),
                "default sentinel"
            );
        }
        commands.windows = None;
        commands.macos = None;
        commands.linux = None;
        for os in ["windows", "macos", "linux"] {
            assert_eq!(install_command_for_os(&commands, os), "default sentinel");
        }
    }

    #[test]
    fn scope_b_catalog_2800_host_selector_matches_supported_cfg() {
        let commands = InstallCommands {
            default: "default sentinel".to_string(),
            windows: Some("windows sentinel".to_string()),
            macos: Some("macos sentinel".to_string()),
            linux: Some("linux sentinel".to_string()),
        };
        let expected = if cfg!(target_os = "windows") {
            "windows sentinel"
        } else if cfg!(target_os = "macos") {
            "macos sentinel"
        } else if cfg!(target_os = "linux") {
            "linux sentinel"
        } else {
            "default sentinel"
        };
        assert_eq!(resolve_install_command(&commands), expected);
    }

    #[test]
    fn scope_b_catalog_2800_project_and_personal_composition_preserved() {
        // Reuse the exact existing main regressions; no copied implementation/harness.
        project_absence_preserves_direct_report_and_catalog();
        project_then_personal_merges_fields_nulls_and_exact_order();
        install_commands_2736_local_layer_patches_windows_and_inherits_default();
        install_commands_2736_one_bad_local_value_discards_the_entire_local_layer();
        install_commands_2736_managed_base_refreshes_from_the_previous_revision();
        install_commands_2736_pin_wire_keeps_presence_semantics();
    }

    #[test]
    fn scope_b_catalog_2800_malformed_project_and_muse_policy_preserved() {
        project_invalid_schema_preserves_base_and_independent_personal();
        project_rejection_has_no_donor_and_personal_rejection_keeps_project();
        project_ownership_support_and_unavailable_base_contract();
        builtin_agent_support_ships_only_muse_disabled();
    }
}
