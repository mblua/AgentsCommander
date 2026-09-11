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
//! present-but-corrupt file is **never overwritten**; the command serves the
//! embedded default in memory for that session (self-heal is a return value, not
//! a disk write).
//!
//! #1912 - code-level support switch for the built-in coding agents:
//! `BUILTIN_AGENT_SUPPORT` below is the ONLY place a built-in is turned on or
//! off. A `false` row is enforced by the READ GATE in `validate_and_filter` (the
//! key disappears from every read path: embedded default, project and legacy
//! manifests, backfill source) and by the SEED GATE on the embedded manifest
//! bytes `ensure_seeded` writes and on the config-folder masters. Already-seeded
//! user-owned files are NEVER rewritten or trimmed by a `false` row; the read
//! gate covers them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::agent_command::is_safe_instructions_filename;
use crate::config::seed_manifest::{
    acquire_project_gate_soft, ManifestActivationToken, ManifestPathIdentity,
    ProjectSeedManifestGuard, PublishedManifestRow, SoftProjectGate,
};
use crate::config::settings::{
    validate_agent_command_text, validate_config_seed_dest, validate_env_rows, AppSettings,
    CodingAgentEnv, ConfigSeedConfig,
};

/// Subdirectory of the config dir holding the catalog artifacts.
const CATALOG_DIR_NAME: &str = crate::config::instance_artifacts::CODING_AGENTS_CATALOG_DIR_NAME;
/// The catalog manifest filename.
const CATALOG_MANIFEST_FILENAME: &str = "agents.json";
/// Current manifest schema version.
const CATALOG_SCHEMA_VERSION: u32 = 1;

/// The embedded default catalog, authored byte-equal to the post-#766/#768
/// frontend presets. This is the single source of truth AC ships; it is written
/// to disk once (seed) and also served in memory when the on-disk file is missing
/// or unparseable.
const EMBEDDED_DEFAULT_CATALOG_JSON: &str =
    include_str!("../../resources/coding-agents/agents.default.json");

/// #1912 - the ONLY place a built-in coding agent is turned on or off. One row
/// per key in `agents.default.json`, same order (a test pins both). `false` =
/// de-supported: dropped by `validate_and_filter` on EVERY read path (embedded
/// default, project manifest, legacy manifest, backfill source), omitted from
/// the embedded bytes `ensure_seeded` writes, and its config-folder master is
/// neither seeded nor re-seedable. Already-seeded files are never rewritten or
/// trimmed; the read gate covers them. A key absent from this table (a
/// user-authored entry) is always kept.
pub(crate) const BUILTIN_AGENT_SUPPORT: &[(&str, bool)] = &[
    ("claude", true),
    ("codex", true),
    ("hermes", true),
    ("cursor", true),
    ("pi", true),
    ("opencode", true),
    ("antigravity", true),
    ("muse", true),
];

/// Unique-suffix counter for the seed temp file (mirrors the pattern in
/// `seeded_context_templates::unique_state_temp_path`).
static SEED_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn default_true() -> bool {
    true
}

fn default_catalog_schema_version() -> u32 {
    CATALOG_SCHEMA_VERSION
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
/// IPC paths). RAW and UNGATED by design: the seed and the tests read it; every
/// consumer-facing read goes through `validate_and_filter`.
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

/// Validate entries per-entry (G7): a bad entry is logged and skipped, the valid
/// rest are kept. Duplicate keys are dropped (first wins). `source` labels the
/// origin in log lines (the manifest path, or the embedded default). #1912 read
/// gate: a key with a `false` row in `BUILTIN_AGENT_SUPPORT` (the table in
/// force) is dropped here too, whatever the file carries.
fn validate_and_filter(
    agents: Vec<CodingAgentDefinition>,
    source: &str,
) -> Vec<CodingAgentDefinition> {
    let mut out: Vec<CodingAgentDefinition> = Vec::with_capacity(agents.len());
    let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    let table = active_builtin_agent_support();
    for def in agents {
        if let Err(e) = validate_definition(&def) {
            log::warn!("[coding-agents] skipping invalid entry in {source}: {e}");
            continue;
        }
        if !seen_keys.insert(def.key.clone()) {
            log::warn!(
                "[coding-agents] skipping duplicate key '{}' in {source}",
                def.key
            );
            continue;
        }
        if !is_supported_builtin(&def.key, table) {
            log::info!(
                "[coding-agents] skipping de-supported built-in '{}' in {source}",
                def.key
            );
            continue;
        }
        out.push(def);
    }
    out
}

/// The embedded default, validated (defensive; also dedups). Used as the
/// in-memory fallback when the on-disk manifest is missing or unparseable.
/// #1912: a `false` row in `BUILTIN_AGENT_SUPPORT` is dropped here.
fn validated_embedded_default() -> Vec<CodingAgentDefinition> {
    validate_and_filter(
        embedded_default_catalog().agents,
        "embedded default catalog",
    )
}

/// #1546 - in-memory backfill: for every entry whose `update_commands` is
/// EMPTY, copy the sequence from the FIRST embedded-default entry with the
/// same `command` and a non-empty sequence. Matching by `command` (not key),
/// mirroring `build_update_plan`'s command-keyed binding rule. User-authored
/// non-empty sequences ALWAYS win (never overwritten). Entries with no
/// embedded match (custom commands, cursor's `agent`) stay empty. Never
/// writes to disk: the catalog is user-owned after the first seed (G3).
/// #1912: a de-supported built-in no longer donates its sequence.
fn backfill_update_commands_from_embedded_default(
    agents: Vec<CodingAgentDefinition>,
) -> Vec<CodingAgentDefinition> {
    let defaults = validated_embedded_default();
    agents
        .into_iter()
        .map(|mut def| {
            if !def.update_commands.is_empty() {
                return def;
            }
            if let Some(src) = defaults
                .iter()
                .find(|d| d.command == def.command && !d.update_commands.is_empty())
            {
                def.update_commands = src.update_commands.clone();
            }
            def
        })
        .collect()
}

/// Load the catalog for the `get_coding_agent_catalog` command.
///
/// Contract (§14.2): NEVER errors. Returns the validated on-disk agents when the
/// manifest parses; a valid empty list is honored verbatim (the user removed all
/// built-ins). On the parsed path, entries with empty `updateCommands` are
/// backfilled IN MEMORY from the embedded default (first entry with the same
/// `command` and a non-empty sequence); user-authored sequences always win;
/// nothing is written to disk. A **missing** or **unparseable** manifest
/// self-heals to the embedded default IN MEMORY only, never writing to disk
/// (G3 corrupt-preserve). #1912: a `false` row in `BUILTIN_AGENT_SUPPORT` is
/// dropped on every read path here - embedded default, parsed manifest, and
/// legacy read alike.
/// `ac_dir` is the project's `.ac` directory (or, for the legacy read fallback,
/// the legacy config dir, which yields `<config_dir>/coding-agents/agents.json`
/// through the same relative layout).
///
/// Bounded handoff (#1963 P1): this array-returning loader remains temporary
/// compatibility code until the managed-catalog migration lands; P4 (#1967)
/// removes it together with `load_catalog_for_settings`. New diagnostics go
/// through `load_catalog_report` below.
pub fn load_catalog(ac_dir: &Path) -> Vec<CodingAgentDefinition> {
    let path = manifest_path(ac_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::info!(
                "[coding-agents] {} absent; serving embedded default catalog",
                path.display()
            );
            return validated_embedded_default();
        }
        Err(e) => {
            log::warn!(
                "[coding-agents] failed to read {} ({e}); serving embedded default catalog",
                path.display()
            );
            return validated_embedded_default();
        }
    };
    match serde_json::from_slice::<CodingAgentCatalog>(&bytes) {
        Ok(catalog) => backfill_update_commands_from_embedded_default(validate_and_filter(
            catalog.agents,
            &path.display().to_string(),
        )),
        Err(e) => {
            // G3: never overwrite a present-but-corrupt file. Preserve it as-is
            // and serve the built-in defaults for this session only.
            log::warn!(
                "[coding-agents] {} is not valid catalog JSON ({e}); preserving the file untouched and using built-in defaults for this session",
                path.display()
            );
            validated_embedded_default()
        }
    }
}

/// The first non-empty trimmed entry of `project_paths`, else the legacy
/// `project_path` (non-empty trimmed), else `None`. Single deterministic head
/// rule, mirroring the canonical `selected_head: project_paths.first()`
/// semantics (`settings.rs`). No canonicalization: a stale raw path simply
/// self-heals at read time (absent file -> embedded default, absent dir ->
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
/// legacy location is NEVER consulted (a user who deletes the primary file to
/// reset gets the embedded default, not the legacy copy). With NO registered
/// project the LEGACY `<config_dir>/coding-agents` catalog is served when one
/// exists (read-only, never written; pre-migration installs with zero projects
/// keep today's read behavior), self-healing to the embedded default when
/// absent/unparseable.
///
/// Bounded handoff (#1963 P1): temporary compatibility code, removed in
/// P4 (#1967); `load_catalog_report_for_settings` below is the persisted-only
/// diagnostic resolver that P5 (#1968) extends.
pub fn load_catalog_for_settings(settings: &AppSettings) -> Vec<CodingAgentDefinition> {
    match primary_project_root(settings) {
        Some(root) => load_catalog(&root.join(".ac")),
        None => crate::config::config_dir()
            .map(|dir| load_catalog(&dir))
            .unwrap_or_else(validated_embedded_default),
    }
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
// shape below stays fixed. The existing array endpoint and updater keep their
// current behavior until P4 (#1967).
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
];
const KNOWN_CONFIG_SEED_FIELDS: &[&str] = &["enabled", "dest"];
const KNOWN_ENV_FIELDS: &[&str] = &["key", "value", "source", "enabled"];
const KNOWN_ROOT_FIELDS: &[&str] = &["schemaVersion", "agents"];

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

/// Raw read of the persisted catalog source. A missing file, an unreadable
/// path, a nonregular entry and a symbolic link all yield `baseUnavailable`:
/// none of them is a reason to substitute embedded defaults on this path.
/// Read-only by construction (metadata + read, never a create).
fn read_catalog_source(path: &Path) -> Result<Vec<u8>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(
                    "the persisted catalog path is a symbolic link; a regular persisted file is required"
                        .to_string(),
                );
            }
            if !meta.file_type().is_file() {
                return Err(
                    "the persisted catalog path is not a regular file; a regular persisted file is required"
                        .to_string(),
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(
                "no persisted catalog exists at this path; no embedded defaults are substituted"
                    .to_string(),
            );
        }
        Err(e) => {
            return Err(format!(
                "the persisted catalog path could not be inspected ({e})"
            ));
        }
    }
    std::fs::read(path).map_err(|e| format!("the persisted catalog could not be read ({e})"))
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
) {
    if let Some(names) = unknown_field_names(raw.keys(), KNOWN_DEFINITION_FIELDS) {
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
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                path,
                format!(
                    "coding-agent '{key}' configSeed carries unknown field(s) ({names}) that require managed-catalog migration"
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
}

/// Load the persisted-only catalog report for one catalog root directory (a
/// project's `.ac` dir, or the legacy `<config_dir>` root in no-project mode).
/// `primaryProjectRoot` is left `None` here; the settings wrapper fills it.
/// READ-ONLY: never seeds, creates directories, refreshes, locks or writes.
/// `unavailable` is set (with an empty catalog) for a missing/unreadable/
/// nonregular/link source, invalid JSON, an unsupported explicit schemaVersion
/// or an invalid root shape; a valid empty catalog is a success. Missing
/// `schemaVersion` means 1; missing `agents` keeps the empty-list meaning.
/// Definitions keep persisted order after filtering; no embedded donor is ever
/// consulted and no backfill is applied.
pub fn load_catalog_report(ac_dir: &Path) -> CatalogReport {
    let path = manifest_path(ac_dir);
    let mut report = CatalogReport {
        primary_project_root: None,
        source_path: Some(path.display().to_string()),
        catalog: Vec::new(),
        warnings: Vec::new(),
        unavailable: None,
    };

    let bytes = match read_catalog_source(&path) {
        Ok(bytes) => bytes,
        Err(reason) => {
            report.unavailable = Some(catalog_diagnostic(
                REPORT_CODE_BASE_UNAVAILABLE,
                &path,
                reason,
            ));
            return report;
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(e) => {
            report.unavailable = Some(catalog_diagnostic(
                REPORT_CODE_BASE_INVALID,
                &path,
                format!("the persisted catalog is not valid JSON ({e})"),
            ));
            return report;
        }
    };
    let serde_json::Value::Object(root) = value else {
        report.unavailable = Some(catalog_diagnostic(
            REPORT_CODE_BASE_INVALID,
            &path,
            "the catalog root must be a JSON object",
        ));
        return report;
    };
    if let Some(version) = root.get("schemaVersion") {
        if version.as_u64() != Some(CATALOG_SCHEMA_VERSION as u64) {
            report.unavailable = Some(catalog_diagnostic(
                REPORT_CODE_BASE_INVALID,
                &path,
                format!(
                    "unsupported explicit schemaVersion {version}; only schemaVersion 1 is recognized"
                ),
            ));
            return report;
        }
    }
    let rows = match root.get("agents") {
        None => Vec::new(),
        Some(serde_json::Value::Array(rows)) => rows.clone(),
        Some(_) => {
            report.unavailable = Some(catalog_diagnostic(
                REPORT_CODE_BASE_INVALID,
                &path,
                "the catalog 'agents' value must be a JSON array",
            ));
            return report;
        }
    };

    if let Some(names) = unknown_field_names(root.keys(), KNOWN_ROOT_FIELDS) {
        report.warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_PENDING,
            &path,
            format!(
                "root field(s) ({names}) are not recognized by the current catalog schema and require managed-catalog migration"
            ),
        ));
    }

    let table = active_builtin_agent_support();
    let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (index, raw) in rows.iter().enumerate() {
        let Some(raw_object) = raw.as_object() else {
            report.warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                &path,
                format!("entry {index} is not a JSON object and was omitted"),
            ));
            continue;
        };
        if let Some(reason) = raw_update_commands_problem(raw) {
            report.warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                &path,
                format!("entry {index} was omitted: {reason}"),
            ));
            continue;
        }
        let def: CodingAgentDefinition = match serde_json::from_value(raw.clone()) {
            Ok(def) => def,
            Err(_) => {
                report.warnings.push(catalog_diagnostic(
                    REPORT_CODE_INVALID_DEFINITION,
                    &path,
                    format!(
                        "entry {index} does not match the coding-agent definition schema and was omitted"
                    ),
                ));
                continue;
            }
        };
        if validate_definition(&def).is_err() {
            let label = if validate_catalog_key(&def.key).is_ok() {
                format!("coding-agent '{}'", def.key)
            } else {
                format!("entry {index}")
            };
            report.warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                &path,
                format!("{label} was omitted: {}", definition_problem_reason(&def)),
            ));
            continue;
        }
        if !seen_keys.insert(def.key.clone()) {
            report.warnings.push(catalog_diagnostic(
                REPORT_CODE_DUPLICATE_KEY,
                &path,
                format!(
                    "coding-agent '{}' was omitted: its key duplicates an earlier entry (the first entry wins)",
                    def.key
                ),
            ));
            continue;
        }
        if !is_supported_builtin(&def.key, table) {
            report.warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                &path,
                format!(
                    "coding-agent '{}' is not supported by this build and was omitted",
                    def.key
                ),
            ));
            continue;
        }
        if !raw_object.contains_key("updateCommands") {
            report.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                &path,
                format!(
                    "coding-agent '{}' has no persisted updateCommands; absent update commands are suppressed during reads and require managed-catalog migration on a supported restart",
                    def.key
                ),
            ));
        }
        push_unknown_field_warnings(raw_object, &def.key, &path, &mut report.warnings);
        report.catalog.push(def);
    }

    report
}

/// Settings-level resolver: the primary project root (first nonblank
/// `project_paths` entry, else the legacy `project_path`) selects the project's
/// `.ac` dir; with no project the INSTANCE catalog at
/// `<config_dir>/coding-agents/agents.json` is read (read-only, never seeded).
/// A failure in the primary project is never retried against another project.
/// P5 (#1968) extends this resolver to compose the local layer.
pub fn load_catalog_report_for_settings(settings: &AppSettings) -> CatalogReport {
    load_catalog_report_for_settings_with_config_dir(settings, crate::config::config_dir())
}

/// Testable twin of [`load_catalog_report_for_settings`] with the resolved
/// config dir injected, so the no-project and config-dir-none arms are
/// coverable without depending on the process-global instance location.
fn load_catalog_report_for_settings_with_config_dir(
    settings: &AppSettings,
    config_dir: Option<PathBuf>,
) -> CatalogReport {
    match primary_project_root(settings) {
        Some(root) => {
            let mut report = load_catalog_report(&root.join(".ac"));
            report.primary_project_root = Some(root.to_string_lossy().to_string());
            report
        }
        None => match config_dir {
            Some(dir) => load_catalog_report(&dir),
            None => report_without_source(),
        },
    }
}

/// #1912 - the bytes `ensure_seeded` writes when seeding from the embedded
/// default. Every row enabled (the shipped state): the raw resource, byte-
/// identical to today. Otherwise the enabled rows, re-serialized (pretty, one
/// trailing newline). The unreachable serialization error (the struct round-
/// trips in `embedded_default_matches_current_presets_exactly`) logs `error`
/// and falls back to the raw bytes: a seeded-but-hidden key is recoverable
/// through the read gate, an unseeded project is not better.
fn embedded_seed_bytes() -> Vec<u8> {
    let table = active_builtin_agent_support();
    if table.iter().all(|(_, on)| *on) {
        return EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes().to_vec();
    }
    let mut catalog = embedded_default_catalog();
    catalog
        .agents
        .retain(|def| is_supported_builtin(&def.key, table));
    match serde_json::to_vec_pretty(&catalog) {
        Ok(mut bytes) => {
            bytes.push(b'\n');
            bytes
        }
        Err(e) => {
            log::error!("[coding-agents] failed to serialize the filtered embedded catalog ({e}); seeding the raw resource");
            EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes().to_vec()
        }
    }
}

/// Seed the manifest ONCE at boot: write the embedded default iff `agents.json`
/// is absent, then never touch it (§14.1 whole-file seed-once). Fail-soft: logs
/// and returns `None` on any error; it must never panic or abort boot.
///
/// #1318 - `ac_dir` is the project's `.ac` directory; `legacy_catalog_dir` is
/// the legacy `<config_dir>/coding-agents` directory (the migration source). On
/// a first seed with the project file ABSENT and a legacy REGULAR-file catalog
/// present, the legacy bytes are copied VERBATIM (a present-but-corrupt legacy
/// file is copied too: corrupt content is user data, the read path self-heals;
/// the legacy original is never touched). Any other legacy shape (absent,
/// dir/symlink) seeds the embedded default, whose bytes are the ENABLED rows of
/// `BUILTIN_AGENT_SUPPORT` only (`embedded_seed_bytes`; byte-identical to the
/// raw resource while every row is `true`). #1912: a `false` row never rewrites
/// or trims an already-seeded user-owned file, and the legacy copy stays
/// verbatim. Returns the `Utc::now()` publication
/// time sampled at the commit point of the atomic write, or `None` when nothing
/// was written.
pub fn ensure_seeded(ac_dir: &Path, legacy_catalog_dir: Option<&Path>) -> Option<DateTime<Utc>> {
    let dir = catalog_dir(ac_dir);
    let path = manifest_path(ac_dir);

    // Seed-once: any existing entry (file, dir, or link) means the catalog is
    // user-owned; leave it strictly alone.
    match std::fs::symlink_metadata(&path) {
        Ok(_) => return None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log::warn!(
                "[coding-agents] cannot stat {} ({e}); skipping catalog seed",
                path.display()
            );
            return None;
        }
    }

    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!(
            "[coding-agents] failed to create {} ({e}); skipping catalog seed",
            dir.display()
        );
        return None;
    }

    // #1318 migration: absent project file + legacy REGULAR-file catalog ->
    // verbatim copy; anything else -> embedded default.
    let mut legacy_source: Option<PathBuf> = None;
    let bytes: Vec<u8> = match legacy_catalog_dir {
        Some(legacy) => {
            let legacy_path = legacy.join(CATALOG_MANIFEST_FILENAME);
            match std::fs::symlink_metadata(&legacy_path) {
                Ok(meta) if meta.is_file() => match std::fs::read(&legacy_path) {
                    Ok(bytes) => {
                        legacy_source = Some(legacy_path);
                        bytes
                    }
                    Err(e) => {
                        log::warn!(
                            "[coding-agents] failed to read legacy catalog {} ({e}); seeding embedded default",
                            legacy_path.display()
                        );
                        embedded_seed_bytes()
                    }
                },
                // Absent, a directory, or a symlink: not a regular file -> the
                // embedded default wins.
                _ => embedded_seed_bytes(),
            }
        }
        None => embedded_seed_bytes(),
    };

    // A verbatim legacy copy is log-checked, never a decision: corrupt content
    // is user data and was copied deliberately; the read path self-heals and
    // deleting the project file re-seeds the embedded default at the next boot.
    if let Some(ref legacy_path) = legacy_source {
        if serde_json::from_slice::<CodingAgentCatalog>(&bytes).is_err() {
            log::warn!(
                "[coding-agents] migrated a legacy catalog that does not parse: {} -> {}; project reads serve the embedded default until the file is fixed or deleted",
                legacy_path.display(),
                path.display()
            );
        }
    }

    match write_manifest_atomic(&path, &bytes) {
        Ok(()) => {
            log::info!(
                "[coding-agents] seeded {} catalog at {}",
                if legacy_source.is_some() {
                    "migrated legacy"
                } else {
                    "default"
                },
                path.display()
            );
            Some(Utc::now())
        }
        Err(e) => {
            log::warn!("[coding-agents] failed to seed {} ({e})", path.display());
            None
        }
    }
}

/// Atomic temp+rename write, mirroring `seeded_context_templates::persist_state`:
/// create-new a unique sibling temp, write+flush+fsync, then publish via the
/// vetted `atomic_replace_existing` primitive (plain rename when the dest is
/// absent, `ReplaceFileW` when it exists). Cleans up the temp on any failure.
fn write_manifest_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let parent = path
        .parent()
        .ok_or_else(|| format!("manifest path {} has no parent", path.display()))?;
    let counter = SEED_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{CATALOG_MANIFEST_FILENAME}.{}.{counter}.tmp",
        std::process::id()
    ));

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("create temp {}: {e}", temp.display()))?;
    if let Err(e) = file.write_all(bytes) {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write temp {}: {e}", temp.display()));
    }
    if let Err(e) = file.flush() {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("flush temp {}: {e}", temp.display()));
    }
    if let Err(e) = file.sync_all() {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("sync temp {}: {e}", temp.display()));
    }
    drop(file);

    if let Err(e) = crate::config::root_agent::atomic_replace_existing(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    Ok(())
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

/// #1912 - existence-only steady-state check: the catalog manifest and every
/// SUPPORTED master dir exist. A de-supported master is never required, or
/// every boot would take the project gate for nothing.
fn all_seeds_present(ac_dir: &Path) -> bool {
    std::fs::symlink_metadata(manifest_path(ac_dir)).is_ok()
        && supported_embedded_masters()
            .all(|m| std::fs::symlink_metadata(master_dir_for_dest(ac_dir, m.dest)).is_ok())
}

/// Seed the catalog + masters for one registered project root, then record the
/// catalog publication in that project's seed manifest.
///
/// Preconditions (enforced before ANY filesystem effect, in both entry points):
/// a non-absolute root (a hand-edited relative settings entry must never seed
/// relative to the process CWD) or a missing root (a deleted/stale registered
/// root must never be resurrected by the seed's `create_dir_all`) is logged and
/// skipped. Steady-state pre-check BEFORE gate acquisition: when the catalog
/// manifest AND every SUPPORTED built-in master dir exist (#1912: a de-supported
/// master is never required), return immediately (no lock,
/// no canonicalize, no manifest read, no write), keeping boot cheap and free of
/// gate contention for the common already-seeded case; masters self-heal is
/// preserved (the pre-check covers masters too).
pub(crate) fn ensure_seeded_for_project(project_root: &Path) {
    #[cfg(not(test))]
    let activation = Some(ManifestActivationToken::production());
    #[cfg(test)]
    let activation: Option<ManifestActivationToken> = None;
    ensure_seeded_for_project_with_token(project_root, activation.as_ref());
}

/// Token-injectable twin of [`ensure_seeded_for_project`], mirroring
/// `perform_config_seed_recorded` (`config_seed.rs`): a `None` activation runs
/// the plain ungated seeds; under the soft project gate the Held arm runs BOTH
/// seeds FIRST and records the catalog row only when `ensure_seeded` actually
/// published (the permit auto-downgrades a held-but-degraded guard to
/// `PublishedUnrecorded`, so no false rows over guaranteed completeness).
/// `DegradedUntracked` runs both seeds ungated (published, unrecorded);
/// `Unavailable` logs and skips (never race a cooperating writer).
pub(crate) fn ensure_seeded_for_project_with_token(
    project_root: &Path,
    activation: Option<&ManifestActivationToken>,
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

    // Steady-state pre-check: everything already seeded -> nothing to publish;
    // no lock file, no canonicalize, no bounded manifest read, no write.
    if all_seeds_present(&ac_dir) {
        return;
    }

    let legacy = crate::config::config_dir().map(|dir| dir.join(CATALOG_DIR_NAME));
    let Some(token) = activation else {
        ensure_seeded(&ac_dir, legacy.as_deref());
        ensure_seeded_masters(&ac_dir, legacy.as_deref());
        return;
    };

    match acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(mut guard) => {
            let published_at = ensure_seeded(&ac_dir, legacy.as_deref());
            ensure_seeded_masters(&ac_dir, legacy.as_deref());
            if let Some(published_at) = published_at {
                record_catalog_publication(&mut guard, token, published_at);
            }
            guard.release();
        }
        SoftProjectGate::DegradedUntracked => {
            ensure_seeded(&ac_dir, legacy.as_deref());
            ensure_seeded_masters(&ac_dir, legacy.as_deref());
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

    /// (key, label, description, color, command, instructionsFilename, seed dest)
    /// for the eight current presets. The embedded default must match these
    /// values. The frontend keeps a parallel `FALLBACK_CODING_AGENTS` test (E7).
    #[allow(clippy::type_complexity)]
    const EXPECTED_PRESETS: [(&str, &str, &str, &str, &str, Option<&str>, Option<&str>); 8] = [
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

    /// #1912 - support-override test tables: all 8 rows spelled out, in the
    /// shipped order, exactly one `false` each. Reaching the PRODUCTION
    /// wrappers through `with_builtin_agent_support_for_test` is the point: the
    /// controls below prove behavior on the real call chain, not on copies.
    const TABLE_MUSE_OFF: &[(&str, bool)] = &[
        ("claude", true),
        ("codex", true),
        ("hermes", true),
        ("cursor", true),
        ("pi", true),
        ("opencode", true),
        ("antigravity", true),
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
        ("muse", true),
    ];

    #[test]
    fn embedded_default_parses_with_eight_agents_in_order() {
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
                "muse"
            ]
        );
        // Muse is last, immediately after Antigravity.
        let last = catalog.agents.last().unwrap();
        assert_eq!(last.key, "muse");
        assert_eq!(last.command, "muse");
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
            // five ship none (no master, no re-seed button).
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
                "autoUpdate": false
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
            5
        );
    }

    #[test]
    fn every_embedded_entry_validates() {
        for def in embedded_default_catalog().agents {
            validate_definition(&def).unwrap_or_else(|e| panic!("{}: {e}", def.key));
        }
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
    fn load_missing_manifest_returns_embedded_default() {
        let dir = seed_dir();
        let agents = load_catalog(dir.path());
        assert_eq!(agents.len(), 8);
        assert_eq!(agents.last().unwrap().key, "muse");
        assert_eq!(agents[0].key, "claude");
    }

    #[test]
    fn load_corrupt_manifest_returns_embedded_and_preserves_file() {
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let garbage = b"{ this is not valid json";
        std::fs::write(&path, garbage).unwrap();

        let agents = load_catalog(dir.path());
        assert_eq!(
            agents.len(),
            8,
            "corrupt file self-heals to embedded default"
        );
        assert_eq!(agents.last().unwrap().key, "muse");
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

        let loaded = load_catalog(dir.path());
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

        let loaded = load_catalog(dir.path());
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

        assert!(load_catalog(dir.path()).is_empty());
    }

    #[test]
    fn ensure_seeded_writes_when_absent_then_is_idempotent() {
        let dir = seed_dir();
        let path = manifest_path(dir.path());
        assert!(!path.exists());

        ensure_seeded(dir.path(), None);
        assert!(path.exists(), "seed writes the manifest when absent");
        assert_eq!(load_catalog(dir.path()).len(), 8);

        // Idempotent + never clobbers a user edit: hand-edit to a single custom
        // agent, re-seed, and confirm the edit is preserved.
        let custom = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        std::fs::write(&path, &custom).unwrap();
        ensure_seeded(dir.path(), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
        let loaded = load_catalog(dir.path());
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
    fn legacy_catalog_is_copied_verbatim_when_project_file_absent() {
        let project = seed_dir();
        let legacy = legacy_dir();
        let legacy_bytes = legacy_catalog_json();
        std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();

        let published = ensure_seeded(project.path(), Some(legacy.path()));
        assert!(published.is_some(), "a first seed publishes");
        let project_file = manifest_path(project.path());
        assert_eq!(
            std::fs::read(&project_file).unwrap(),
            legacy_bytes,
            "legacy catalog must be copied byte-for-byte"
        );
        // The legacy original is untouched.
        assert_eq!(
            std::fs::read(legacy.path().join("agents.json")).unwrap(),
            legacy_bytes
        );
        // Second seed run is a no-op (seed-once), even if the legacy differs.
        std::fs::write(legacy.path().join("agents.json"), b"CHANGED LATER").unwrap();
        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
        assert_eq!(std::fs::read(&project_file).unwrap(), legacy_bytes);
    }

    #[test]
    fn legacy_absent_or_not_a_file_seeds_embedded_default() {
        // Absent legacy dir -> embedded default.
        let project = seed_dir();
        let legacy = legacy_dir();
        ensure_seeded(project.path(), Some(legacy.path()));
        assert_eq!(load_catalog(project.path()).len(), 8);

        // Legacy agents.json is a DIRECTORY -> not a regular file -> embedded.
        let project = seed_dir();
        std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
        ensure_seeded(project.path(), Some(legacy.path()));
        assert_eq!(load_catalog(project.path()).len(), 8);

        // No legacy at all -> embedded default.
        let project = seed_dir();
        ensure_seeded(project.path(), None);
        assert_eq!(load_catalog(project.path()).len(), 8);
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
    fn load_catalog_for_settings_primary_wins_and_self_heals() {
        let primary = seed_dir();
        let ac_dir = primary.path().join(".ac");
        let settings = AppSettings {
            project_paths: vec![primary.path().to_string_lossy().to_string()],
            ..AppSettings::default()
        };

        // Primary file absent -> embedded default (self-heal).
        assert_eq!(load_catalog_for_settings(&settings).len(), 8);

        // Hand-edited primary file is observable (primary wins over everything).
        let custom = manifest_json(
            r##"[{"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        std::fs::create_dir_all(manifest_path(&ac_dir).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(&ac_dir), &custom).unwrap();
        let loaded = load_catalog_for_settings(&settings);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "custom");

        // Primary file DELETED -> embedded default, never a legacy copy.
        std::fs::remove_file(manifest_path(&ac_dir)).unwrap();
        assert_eq!(load_catalog_for_settings(&settings).len(), 8);
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
    fn legacy_catalog_corrupt_is_copied_verbatim_and_reads_self_heal() {
        let project = seed_dir();
        let legacy = legacy_dir();
        let garbage = b"{ this is not valid json".to_vec();
        std::fs::write(legacy.path().join("agents.json"), &garbage).unwrap();

        ensure_seeded(project.path(), Some(legacy.path()));
        let project_file = manifest_path(project.path());
        assert_eq!(
            std::fs::read(&project_file).unwrap(),
            garbage,
            "corrupt legacy content is user data and is copied verbatim"
        );
        // The read path self-heals to the embedded default in memory.
        assert_eq!(load_catalog(project.path()).len(), 8);
        // Recovery: deleting the project file re-seeds the embedded default.
        std::fs::remove_file(&project_file).unwrap();
        ensure_seeded(project.path(), Some(legacy.path()));
        assert_eq!(load_catalog(project.path()).len(), 8);
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
        let Some(config_dir) = crate::config::config_dir() else {
            return;
        };
        let legacy_master = config_dir
            .join("coding-agents")
            .join("_seed")
            .join(".claude");
        let _ = std::fs::remove_dir_all(config_dir.join("coding-agents"));
        std::fs::create_dir_all(&legacy_master).unwrap();
        std::fs::write(legacy_master.join("marker"), b"x").unwrap();

        let result = reseed_master_for_command(&config_dir, "claude");
        assert!(result.is_ok(), "legacy reseed works: {result:?}");
        let m = master("claude");
        assert_eq!(
            std::fs::read(legacy_master.join(m.files[0].rel_path)).unwrap(),
            m.files[0].bytes
        );
        let _ = std::fs::remove_dir_all(config_dir.join("coding-agents"));
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
        let loaded = load_catalog(dir.path());
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
    fn load_catalog_backfills_empty_update_commands_from_embedded_default() {
        // A catalog seeded before the update-command era (#1325) carries no
        // updateCommands; load_catalog backfills them IN MEMORY from the
        // embedded default, matching by command, and never writes the file.
        let json = manifest_json(
            r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true},
             {"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true},
             {"key":"codex","label":"Codex","description":"d","color":"#10b981","command":"codex","envs":[],"isolatedHome":false,"removable":true},
             {"key":"hermes","label":"Hermes","description":"d","color":"#8b5cf6","command":"hermes","envs":[],"isolatedHome":false,"removable":true},
             {"key":"opencode","label":"OpenCode","description":"d","color":"#64748b","command":"opencode","envs":[],"isolatedHome":false,"removable":true},
             {"key":"antigravity","label":"Antigravity","description":"d","color":"#4285F4","command":"agy","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path());
        assert_eq!(loaded.len(), 6);
        let by_key = |key: &str| loaded.iter().find(|d| d.key == key).unwrap();
        assert_eq!(
            by_key("claude").update_commands,
            vec!["claude --update".to_string()]
        );
        assert_eq!(by_key("pi").update_commands, vec!["pi update".to_string()]);
        assert_eq!(
            by_key("codex").update_commands,
            vec!["codex update".to_string()]
        );
        assert_eq!(
            by_key("hermes").update_commands,
            vec!["hermes update --yes".to_string()]
        );
        assert_eq!(
            by_key("opencode").update_commands,
            vec!["opencode upgrade".to_string()]
        );
        assert_eq!(
            by_key("antigravity").update_commands,
            vec!["agy update".to_string()]
        );

        // No-write proof: the manifest bytes are identical before/after the load.
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            json.as_bytes()
        );
    }

    #[test]
    fn load_catalog_never_overwrites_user_update_commands() {
        // User-authored non-empty sequences ALWAYS win, even a one-element one.
        let json = manifest_json(
            r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["claude --custom"]}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path());
        assert_eq!(
            loaded[0].update_commands,
            vec!["claude --custom".to_string()]
        );
    }

    #[test]
    fn load_catalog_backfill_no_embedded_match_leaves_entry_intact() {
        // Custom command `bob` has no embedded match -> stays empty (never
        // prompted nor updated), unchanged behavior.
        let json = manifest_json(
            r##"[{"key":"bob","label":"Bob","description":"d","color":"#333","command":"bob","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path());
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].update_commands.is_empty());
    }

    #[test]
    fn load_catalog_backfill_matches_by_command_for_duplicate_commands() {
        // Two profiles share the `pi` command under different keys; both are
        // backfilled per-entry, consistent with build_update_plan's
        // command-keyed binding rule.
        let json = manifest_json(
            r##"[{"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true},
             {"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path());
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].update_commands, vec!["pi update".to_string()]);
        assert_eq!(loaded[1].update_commands, vec!["pi update".to_string()]);
    }

    #[test]
    fn load_catalog_backfill_mixed_duplicates_first_empty_second_custom() {
        // Mixed duplicate commands: the FIRST `pi` entry is empty -> backfilled;
        // the SECOND keeps its custom sequence UNTOUCHED (data-level never
        // overwritten). build_update_plan's first-non-empty `find` then selects
        // the FIRST entry's sequence for command `pi` - the backfilled
        // ["pi update"] wins over the custom one (pre-existing command-keyed
        // first-wins rule; before the backfill the find skipped the empty first
        // entry and used the custom sequence).
        let json = manifest_json(
            r##"[{"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true},
             {"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["pi --custom"]}]"##,
        );
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &json).unwrap();

        let loaded = load_catalog(dir.path());
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].update_commands, vec!["pi update".to_string()]);
        assert_eq!(loaded[1].update_commands, vec!["pi --custom".to_string()]);
    }

    #[test]
    fn embedded_default_ships_update_commands_for_all_but_cursor_and_muse() {
        // #1318/#1325/#1546 drift guard: claude, pi, codex, hermes, opencode,
        // and antigravity ship the update command; cursor and Muse ship none; every
        // entry defaults autoUpdate to false.
        let catalog = embedded_default_catalog();
        assert_eq!(catalog.agents.len(), 8);
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
                "cursor" | "muse" => assert!(
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
    fn builtin_agent_support_ships_every_row_enabled() {
        // R2: shipped state is all-true; the flip-time follow-up edits this test.
        assert!(
            BUILTIN_AGENT_SUPPORT.iter().all(|(_, on)| *on),
            "every row must ship enabled"
        );
    }

    #[test]
    fn support_override_scopes_to_closure_and_restores_shipped_table() {
        // R3: the override reaches load_catalog and restores the shipped table.
        let dir = seed_dir();
        let before = load_catalog(dir.path());
        assert_eq!(before.len(), 8);
        assert!(before.iter().any(|a| a.key == "muse"));

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let inside = load_catalog(dir.path());
            assert_eq!(inside.len(), 7);
            assert_no_key(&inside, "muse");
        });

        let after = load_catalog(dir.path());
        assert_eq!(after.len(), 8);
        assert!(after.iter().any(|a| a.key == "muse"));
    }

    #[test]
    fn desupported_row_dropped_from_embedded_self_heal_paths() {
        // R4: missing and corrupt manifests self-heal to the embedded default
        // with the de-supported row dropped; the corrupt file is preserved.
        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let dir = seed_dir();
            let agents = load_catalog(dir.path());
            assert_eq!(agents.len(), 7);
            assert_no_key(&agents, "muse");
            assert_eq!(agents[0].key, "claude");

            let dir = seed_dir();
            let path = manifest_path(dir.path());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let garbage = b"{ this is not valid json";
            std::fs::write(&path, garbage).unwrap();
            let agents = load_catalog(dir.path());
            assert_eq!(agents.len(), 7);
            assert_no_key(&agents, "muse");
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

        // Control: no override -> duplicate key dedups first-wins, custom key kept.
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
        let loaded = load_catalog(dir.path());
        assert_eq!(keys_of(&loaded), ["muse", "mine"]);
        assert_eq!(loaded[0].label, "First");

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let dir = seed_dir();
            std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
            std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
            let loaded = load_catalog(dir.path());
            assert_eq!(keys_of(&loaded), ["mine"]);
            assert_eq!(loaded[0].command, "muse");
        });
    }

    #[test]
    fn desupported_row_dropped_from_load_catalog_for_settings_primary() {
        // R6: the settings read root (primary project) honors the read gate on
        // both the self-heal path and the parsed user file path.
        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let primary = seed_dir();
            let ac_dir = primary.path().join(".ac");
            let settings = AppSettings {
                project_paths: vec![primary.path().to_string_lossy().to_string()],
                ..AppSettings::default()
            };

            let loaded = load_catalog_for_settings(&settings);
            assert_eq!(loaded.len(), 7);
            assert_no_key(&loaded, "muse");

            let user_file = manifest_json(
                r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
                {"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
            );
            std::fs::create_dir_all(manifest_path(&ac_dir).parent().unwrap()).unwrap();
            std::fs::write(manifest_path(&ac_dir), &user_file).unwrap();
            let loaded = load_catalog_for_settings(&settings);
            assert_eq!(keys_of(&loaded), ["custom"]);
        });
    }

    #[test]
    fn desupported_builtin_no_longer_donates_update_commands_in_backfill() {
        // R7: the in-memory backfill source is gated, so a de-supported built-in
        // no longer donates its update sequence to a same-command entry.
        let manifest = manifest_json(
            r##"[{"key":"my-claude","label":"My Claude","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true}]"##,
        );

        // Control: without the override the embedded claude donates its sequence.
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &manifest).unwrap();
        let loaded = load_catalog(dir.path());
        assert_eq!(
            loaded[0].update_commands,
            vec!["claude --update".to_string()]
        );

        with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
            let dir = seed_dir();
            std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
            std::fs::write(manifest_path(dir.path()), &manifest).unwrap();
            let loaded = load_catalog(dir.path());
            assert!(loaded[0].update_commands.is_empty());
        });
    }

    #[test]
    fn desupported_row_absent_from_seeded_manifest_bytes() {
        // R8: `ensure_seeded` with no legacy dir (arm `:434`) writes only the
        // enabled rows under a false row; the all-enabled control keeps seeding
        // the raw resource byte-for-byte.
        let dir = seed_dir();
        let published = ensure_seeded(dir.path(), None);
        assert!(published.is_some());
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes(),
            "all rows enabled: seeded bytes must equal the raw resource"
        );

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let dir = seed_dir();
            let published = ensure_seeded(dir.path(), None);
            assert!(published.is_some(), "a first seed publishes");
            let bytes = std::fs::read(manifest_path(dir.path())).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains("\"muse\""),
                "seeded bytes must not carry the de-supported key"
            );
            let catalog: CodingAgentCatalog =
                serde_json::from_slice(&bytes).expect("seeded manifest parses");
            assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
            assert_eq!(catalog.agents.len(), 7);
            let loaded = load_catalog(dir.path());
            assert_eq!(loaded.len(), 7);
            assert_no_key(&loaded, "muse");
        });
    }

    #[test]
    fn legacy_catalog_copied_verbatim_even_when_it_carries_a_desupported_key() {
        // R9: the legacy REGULAR-file copy stays verbatim: user bytes are never
        // filtered; the read gate hides the de-supported key from consumers.
        let legacy_bytes = manifest_json(
            r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
            {"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
        )
        .into_bytes();

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let project = seed_dir();
            let legacy = legacy_dir();
            std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();
            let published = ensure_seeded(project.path(), Some(legacy.path()));
            assert!(published.is_some());
            assert_eq!(
                std::fs::read(manifest_path(project.path())).unwrap(),
                legacy_bytes,
                "legacy bytes are user data: copied verbatim even with a de-supported key"
            );
            let loaded = load_catalog(project.path());
            assert_eq!(keys_of(&loaded), ["custom"]);
        });
    }

    #[test]
    fn already_seeded_manifest_never_trimmed_by_a_false_row() {
        // R10: seed-once is absolute: a false row never rewrites or trims an
        // already-seeded user-owned file; the read gate hides the key only.
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let seeded = std::fs::read(manifest_path(dir.path())).unwrap();

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            assert!(
                ensure_seeded(dir.path(), None).is_none(),
                "present manifest: no rewrite at all"
            );
            assert_eq!(
                std::fs::read(manifest_path(dir.path())).unwrap(),
                seeded,
                "bytes must be untouched"
            );
            let loaded = load_catalog(dir.path());
            assert_eq!(loaded.len(), 7);
            assert_no_key(&loaded, "muse");
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
    fn all_seeds_present_ignores_desupported_master() {
        // R13: the steady-state pre-check requires only SUPPORTED masters, so a
        // claude-off install is steady without `.claude`; the gate-wiring probe
        // shows the tokenized entry returns BEFORE `acquire_project_gate_soft`
        // when steady (no lock file), and falls through (lock created) when a
        // supported master is missing.
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
                all_seeds_present(&ac_dir),
                "steady inside the override: `.claude` must not be required"
            );

            // A missing SUPPORTED master makes the predicate false and is
            // re-seeded by the plain project entry point; `.claude` stays absent.
            let codex_dir = master_dir_for_dest(&ac_dir, ".codex");
            std::fs::remove_dir_all(&codex_dir).unwrap();
            assert!(!all_seeds_present(&ac_dir));
            ensure_seeded_for_project(&root);
            assert!(crate::config::config_seed::is_nonempty_seed_dir(&codex_dir));
            assert!(!master_dir_for_dest(&ac_dir, ".claude").exists());
            assert!(all_seeds_present(&ac_dir));

            // Gate-wiring probe: steady again -> the tokenized entry returns at
            // the pre-check, BEFORE acquire_project_gate_soft: no lock file.
            let lock_path = ac_dir.join(crate::config::seed_manifest::SEED_MANIFEST_LOCK_FILENAME);
            assert!(!lock_path.exists(), "no lock from the ungated seeds above");
            let token = ManifestActivationToken::for_test();
            ensure_seeded_for_project_with_token(&root, Some(&token));
            assert!(
                !lock_path.exists(),
                "steady pre-check must return before the gate: no lock file created"
            );

            // Contrast: a missing supported master falls through the pre-check,
            // acquires the gate (lock file created and persistent), re-seeds
            // `.codex`, still never `.claude`, manifest bytes untouched.
            let manifest_bytes = std::fs::read(manifest_path(&ac_dir)).unwrap();
            std::fs::remove_dir_all(&codex_dir).unwrap();
            ensure_seeded_for_project_with_token(&root, Some(&token));
            assert!(lock_path.is_file(), "the gate must have been acquired");
            assert!(crate::config::config_seed::is_nonempty_seed_dir(&codex_dir));
            assert!(!master_dir_for_dest(&ac_dir, ".claude").exists());
            assert_eq!(
                std::fs::read(manifest_path(&ac_dir)).unwrap(),
                manifest_bytes,
                "manifest bytes unchanged by the fall-through re-seed"
            );
        });

        // Outside the override the same tree is NOT steady: `.claude` is
        // required by the shipped table and absent.
        assert!(!all_seeds_present(&ac_dir));
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
    fn desupported_row_absent_from_seeded_manifest_bytes_in_legacy_dir_arms() {
        // R15: the legacy-dir arms (`:431` non-regular-file branches) seed the
        // enabled rows only under a false row. Shape (a): legacy dir present but
        // `agents.json` absent. Shape (b): `legacy/agents.json` is a DIRECTORY.
        for shape in ["absent", "directory"] {
            // Control (both shapes, no override): the raw resource is seeded
            // byte-for-byte - arm `:431` keeps writing the raw bytes while every
            // row is enabled.
            let project = seed_dir();
            let legacy = legacy_dir();
            if shape == "directory" {
                std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
            }
            let published = ensure_seeded(project.path(), Some(legacy.path()));
            assert!(published.is_some(), "{shape}: a first seed publishes");
            assert_eq!(
                std::fs::read(manifest_path(project.path())).unwrap(),
                EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes(),
                "{shape}: all rows enabled -> raw resource bytes"
            );

            with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
                let project = seed_dir();
                let legacy = legacy_dir();
                if shape == "directory" {
                    std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
                }
                let published = ensure_seeded(project.path(), Some(legacy.path()));
                assert!(published.is_some(), "{shape}: a first seed publishes");
                let bytes = std::fs::read(manifest_path(project.path())).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                assert!(
                    !text.contains("\"muse\""),
                    "{shape}: seeded bytes must not carry the de-supported key"
                );
                let catalog: CodingAgentCatalog =
                    serde_json::from_slice(&bytes).expect("seeded manifest parses");
                assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
                assert_eq!(catalog.agents.len(), 7, "{shape}: 7 agents seeded");
                let loaded = load_catalog(project.path());
                assert_eq!(loaded.len(), 7);
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
            r#"{"schemaVersion":2,"agents":[]}"#,
            r#"{"schemaVersion":"1","agents":[]}"#,
            r#"{"schemaVersion":null,"agents":[]}"#,
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
            r#"{}"#,
            r#"{"schemaVersion":1}"#,
            r#"{"schemaVersion":1,"agents":[]}"#,
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
            r#"[]"#,
            r#""text""#,
            r#"{"schemaVersion":1,"agents":{}}"#,
            r#"{"schemaVersion":1,"agents":"nope"}"#,
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
        assert!(report_json(&reported_explicit).contains(r#""updateCommands":[]"#));

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
}
