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
use crate::config::seed_manifest::{
    acquire_project_gate_soft, has_catalog_publication, ManifestActivationToken,
    ManifestPathIdentity, ProjectSeedManifestGuard, PublishedManifestRow, SoftProjectGate,
    SEED_MANIFEST_FILENAME,
};
use crate::config::settings::{
    validate_agent_command_text, validate_config_seed_dest, validate_env_rows, AppSettings,
    CodingAgentEnv, CodingAgentEnvSource, ConfigSeedConfig,
};

/// Subdirectory of the config dir holding the catalog artifacts.
const CATALOG_DIR_NAME: &str = crate::config::instance_artifacts::CODING_AGENTS_CATALOG_DIR_NAME;
/// The catalog manifest filename.
const CATALOG_MANIFEST_FILENAME: &str = "agents.json";
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
];
const KNOWN_CONFIG_SEED_FIELDS: &[&str] = &["enabled", "dest"];
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
];
const LOCAL_ENV_FIELDS: &[&str] = &["key", "value", "source", "enabled"];
const LOCAL_CONFIG_SEED_FIELDS: &[&str] = &["enabled", "dest"];

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
    Ok(fields)
}

/// Parse the local layer STRICTLY: unknown fields, duplicate JSON members,
/// duplicate local identities, duplicate order keys and unsupported schemas are
/// all whole-layer errors, so a partially applied layer is impossible.
fn parse_local_layer(bytes: &[u8]) -> Result<LocalLayer, String> {
    let value = parse_strict_json(bytes)
        .map_err(|reason| format!("the local catalog is not valid JSON ({reason})"))?;
    let root = expect_json_object(&value, "the local catalog root")?;
    reject_unknown_json_fields(root, LOCAL_ROOT_FIELDS, "the local catalog root")?;
    match root.get("schemaVersion") {
        Some(serde_json::Value::Number(version))
            if version.as_u64() == Some(CATALOG_SCHEMA_VERSION as u64) => {}
        Some(version) => {
            return Err(format!(
                "unsupported schemaVersion {version}; only schemaVersion 1 is recognized"
            ))
        }
        None => return Err("the local catalog root must declare schemaVersion 1".to_string()),
    }
    let agents = match root.get("agents") {
        Some(serde_json::Value::Array(agents)) => agents,
        Some(_) => return Err("the local catalog 'agents' value must be a JSON array".to_string()),
        None => return Err("the local catalog root must declare an 'agents' array".to_string()),
    };
    let order = match root.get("order") {
        None => None,
        Some(serde_json::Value::Array(items)) => {
            let mut seen = HashSet::new();
            let mut keys = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let serde_json::Value::String(key) = item else {
                    return Err(format!("local order item {index} must be a string"));
                };
                if validate_catalog_key(key).is_err() {
                    return Err(format!(
                        "local order item {index} is not a valid catalog key"
                    ));
                }
                if !seen.insert(key.clone()) {
                    return Err(format!("duplicate local order key '{key}'"));
                }
                keys.push(key.clone());
            }
            Some(keys)
        }
        Some(_) => return Err("the local catalog 'order' value must be a JSON array".to_string()),
    };

    let mut seen_keys = HashSet::new();
    let mut rows = Vec::with_capacity(agents.len());
    for (index, raw) in agents.iter().enumerate() {
        let context = format!("local agents[{index}]");
        let object = expect_json_object(raw, &context)?;
        let key = expect_json_string(object, "key", &context)?.to_string();
        validate_catalog_key(&key).map_err(|reason| format!("{context}: {reason}"))?;
        if !seen_keys.insert(key.clone()) {
            return Err(format!("duplicate local coding-agent key '{key}'"));
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
    definition
}

/// A NEW key requires every authored field explicitly; `instructionsFilename`
/// and `configSeed` stay optional (absent or `null`).
fn build_new_definition(
    key: &str,
    fields: &LocalFieldPatch,
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
            "new local coding-agent '{key}' is missing required field(s): {}",
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
) -> Result<Vec<CodingAgentDefinition>, String> {
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
                            "local remove row targets nonremovable coding agent '{}'",
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
                local_added.push(build_new_definition(&row.key, &row.fields)?);
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
            validate_update_command_string(command, "local composition", index).map_err(
                |reason| {
                    format!(
                        "coding agent '{}' failed validation after composition: {reason}",
                        definition.key
                    )
                },
            )?;
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
        if let Some(reason) = raw_update_commands_problem(raw) {
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

struct CatalogSnapshot {
    /// `Err` carries a present-but-unreadable entry (link, reparse point or
    /// nonregular path); the resolver degrades that artifact instead of failing
    /// the whole read.
    base: Result<Option<Vec<u8>>, String>,
    local: Result<Option<Vec<u8>>, String>,
    journal: Result<Option<Vec<u8>>, String>,
}

/// Snapshot the source, local layer and journal. The three files move together
/// only while a migration publishes, so a snapshot whose reads straddle a
/// publication is retried; two consecutive equal snapshots prove a boundary
/// view. After three attempts the read reports unavailability with retry
/// guidance instead of composing a torn state.
fn read_catalog_snapshot(ac_dir: &Path) -> Result<CatalogSnapshot, String> {
    let base_path = manifest_path(ac_dir);
    let local_path = local_catalog_path(ac_dir);
    let journal_path = migration_journal_path(ac_dir);
    type Fingerprint = (
        Result<Option<Vec<u8>>, String>,
        Result<Option<Vec<u8>>, String>,
        Result<Option<Vec<u8>>, String>,
    );
    let mut previous: Option<Fingerprint> = None;
    for _attempt in 0..3 {
        let snapshot = CatalogSnapshot {
            base: read_optional_regular_file(&base_path, "persisted catalog"),
            local: read_optional_regular_file(&local_path, "local coding-agent catalog"),
            journal: read_optional_regular_file(&journal_path, "coding-agent migration journal"),
        };
        let fingerprint = (
            snapshot.base.clone(),
            snapshot.local.clone(),
            snapshot.journal.clone(),
        );
        if previous.as_ref() == Some(&fingerprint) {
            return Ok(snapshot);
        }
        previous = Some(fingerprint);
    }
    Err(
        "the catalog source, local overrides and migration journal kept changing while reading; retry the read"
            .to_string(),
    )
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
) {
    match local {
        Ok(None) => return,
        Ok(Some(local_bytes)) => {
            if let Err(reason) = parse_local_layer(local_bytes) {
                warnings.push(catalog_diagnostic(
                    REPORT_CODE_LOCAL_INVALID,
                    local_path,
                    reason,
                ));
            }
        }
        Err(reason) => warnings.push(catalog_diagnostic(
            REPORT_CODE_LOCAL_INVALID,
            local_path,
            reason.clone(),
        )),
    }
    warnings.push(catalog_diagnostic(
        REPORT_CODE_MIGRATION_PENDING,
        local_path,
        "a local overrides file exists while the base is not AC-managed; ownership transfer is blocked until the managed migration completes and the local layer is not applied on top of a non-managed base",
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
            match &snapshot.local {
                Ok(Some(local_bytes)) => match parse_local_layer(local_bytes) {
                    Ok(layer) => match compose_local_layer(&effective, &layer) {
                        Ok(composed) => effective = composed,
                        Err(reason) => resolved.warnings.push(catalog_diagnostic(
                            REPORT_CODE_LOCAL_INVALID,
                            &local_path,
                            reason,
                        )),
                    },
                    Err(reason) => resolved.warnings.push(catalog_diagnostic(
                        REPORT_CODE_LOCAL_INVALID,
                        &local_path,
                        reason,
                    )),
                },
                Ok(None) => {}
                Err(reason) => resolved.warnings.push(catalog_diagnostic(
                    REPORT_CODE_LOCAL_INVALID,
                    &local_path,
                    reason.clone(),
                )),
            }
            resolved.catalog = apply_support_gate(effective, &base_path, &mut resolved.warnings);
        }
        CatalogBaseKind::ForeignManaged => {
            resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_CONFLICT,
                &base_path,
                "the persisted catalog carries an unrecognized managed ownership marker; this build neither refreshes nor migrates it",
            ));
            describe_local_presence(&snapshot.local, &local_path, &mut resolved.warnings);
            resolved.catalog =
                apply_support_gate(analysis.entries, &base_path, &mut resolved.warnings);
        }
        CatalogBaseKind::Legacy => {
            describe_local_presence(&snapshot.local, &local_path, &mut resolved.warnings);
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
/// project catalogs refresh during every startup/registration, instance
/// catalogs are read-only and never initialize or seed, and direct callers
/// receive the neutral wording.
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
                "The persisted instance catalog revision differs from this build. Current persisted entries remain usable. Instance catalogs are read-only; select a project to initialize or refresh its catalog."
            }
            Self::Direct => {
                "The persisted managed catalog revision differs from this build. Current persisted entries remain usable. Project catalog refresh runs during initialization; instance catalogs remain read-only."
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
/// READ-ONLY: never seeds, creates directories, refreshes, locks or writes.
/// `unavailable` is set (with an empty catalog) for a missing/unreadable/
/// nonregular/link source, invalid JSON, an unsupported explicit schemaVersion
/// or an invalid root shape; a valid empty catalog is a success. A managed base
/// composes its local layer behind the same resolver; a legacy base stays
/// readable and reports the pending migration. Definitions keep persisted order
/// after filtering; no embedded donor is ever consulted.
pub fn load_catalog_report(ac_dir: &Path) -> CatalogReport {
    load_catalog_report_with_context(ac_dir, CatalogSourceContext::Direct).0
}

fn load_catalog_report_with_context(
    ac_dir: &Path,
    context: CatalogSourceContext,
) -> (CatalogReport, bool) {
    let path = manifest_path(ac_dir);
    let mut report = CatalogReport {
        primary_project_root: None,
        source_path: Some(path.display().to_string()),
        catalog: Vec::new(),
        warnings: Vec::new(),
        unavailable: None,
    };
    let snapshot = match read_catalog_snapshot(ac_dir) {
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
/// `<config_dir>/coding-agents/agents.json` is read (read-only, never seeded).
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
            let (mut report, verified_managed_base) =
                load_catalog_report_with_context(&ac_dir, CatalogSourceContext::Project);
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
            // Direct wrapper: the no-project surface never seeds or migrates
            // the instance, so its restart guidance must say so.
            Some(dir) => load_catalog_report_with_context(&dir, CatalogSourceContext::Instance).0,
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
        Self {
            base: dir.join(CATALOG_MANIFEST_FILENAME),
            local: dir.join(LOCAL_CATALOG_FILENAME),
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
        if let Some(problem) = raw_update_commands_problem(raw) {
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
    let layer = parse_local_layer(&local_bytes)?;
    compose_local_layer(&shipped, &layer)?;
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
    let layer = parse_local_layer(&local_bytes)?;
    compose_local_layer(&shipped, &layer)?;
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

/// One initialization pass UNDER THE HELD CATALOG LOCK: recover an interrupted
/// transaction, then refresh, migrate or fresh-seed exactly one base. Order is
/// fixed by the plan: recovery first, then the base state machine.
fn initialize_catalog_under_lock(
    paths: &CatalogPaths,
    legacy_catalog_dir: Option<&Path>,
) -> CatalogInitOutcome {
    let mut outcome = CatalogInitOutcome::default();

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
            match (instance_bytes, instance_path) {
                (Some(bytes), Some(path)) => {
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
) -> CatalogInitOutcome {
    let dir = match ensure_catalog_dir(ac_dir) {
        Ok(dir) => dir,
        Err(reason) => {
            return CatalogInitOutcome {
                published_at: None,
                base_verified_managed: false,
                warnings: vec![catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    ac_dir,
                    reason,
                )],
            }
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
    let outcome = initialize_catalog_under_lock(&paths, legacy_catalog_dir);
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

/// Initialize the catalog for `ac_dir` under the catalog lock: recover or
/// refresh a managed base, migrate a legacy catalog, or fresh-seed the managed
/// defaults plus the create-once local stub. Returns the `Utc::now()`
/// publication time sampled at the commit point when the managed base was
/// actually written or replaced; `None` means no base publication. Fail-soft:
/// every failure is logged and surfaced as a warning, never a panic.
pub fn ensure_seeded(ac_dir: &Path, legacy_catalog_dir: Option<&Path>) -> Option<DateTime<Utc>> {
    run_catalog_initialization(ac_dir, legacy_catalog_dir).published_at
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
    let legacy = crate::config::config_dir().map(|dir| dir.join(CATALOG_DIR_NAME));

    let Some(token) = activation else {
        run_catalog_initialization(&ac_dir, legacy.as_deref());
        ensure_seeded_masters(&ac_dir, legacy.as_deref());
        return;
    };

    match acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(mut guard) => {
            let outcome = run_catalog_initialization(&ac_dir, legacy.as_deref());
            ensure_seeded_masters(&ac_dir, legacy.as_deref());
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
            run_catalog_initialization(&ac_dir, legacy.as_deref());
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
            legacy.path().join("agents.json").display().to_string()
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
            std::fs::read(legacy.path().join("agents.json")).unwrap(),
            legacy_bytes,
            "the instance source stays read-only"
        );
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
            std::fs::read(legacy.path().join("agents.json")).unwrap(),
            garbage
        );
        let unavailable = load_catalog(project.path()).expect_err("no project base");
        assert_eq!(unavailable.code, "baseUnavailable");

        // Recovery: remove the corrupt source; the next initialization seeds.
        std::fs::remove_file(legacy.path().join("agents.json")).unwrap();
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
        ensure_seeded(dir.path(), None);
        let before = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(before.len(), 8);
        assert!(before.iter().any(|a| a.key == "muse"));

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let inside = load_catalog(dir.path()).expect("seeded catalog");
            assert_eq!(inside.len(), 7);
            assert_no_key(&inside, "muse");
        });

        let after = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(after.len(), 8);
        assert!(after.iter().any(|a| a.key == "muse"));
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
            assert_eq!(agents.len(), 7);
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

        // Control: no override -> duplicate key dedups first-wins, custom key kept.
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(keys_of(&loaded), ["muse", "mine"]);
        assert_eq!(loaded[0].label, "First");

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
        // ownership marker; an all-enabled control seeds all 8 and a false row
        // seeds 7 while the read gate hides the key.
        let dir = seed_dir();
        assert!(ensure_seeded(dir.path(), None).is_some());
        let bytes = std::fs::read(manifest_path(dir.path())).unwrap();
        let base: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(base["agents"].as_array().unwrap().len(), 8);
        assert_eq!(base["managed"]["owner"], "agentscommander");

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
            assert_eq!(catalog.agents.len(), 7);
            let loaded = load_catalog(dir.path()).expect("seeded catalog");
            assert_eq!(loaded.len(), 7);
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
        // #1968: the support table is part of the managed revision, so a false
        // row refreshes the BASE to the enabled shipped set; the user-owned
        // local layer is never touched and the key stays hidden either way.
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let seeded = std::fs::read(manifest_path(dir.path())).unwrap();
        assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 8);
        let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            assert!(
                ensure_seeded(dir.path(), None).is_some(),
                "a support-gate change is part of the managed revision"
            );
            assert_ne!(std::fs::read(manifest_path(dir.path())).unwrap(), seeded);
            assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 7);
            assert_eq!(
                std::fs::read(local_catalog_path(dir.path())).unwrap(),
                local_before,
                "the user-owned local layer is never touched by a refresh"
            );
            let loaded = load_catalog(dir.path()).expect("seeded catalog");
            assert_eq!(loaded.len(), 7);
            assert_no_key(&loaded, "muse");
        });

        // Back under the shipped table the base returns byte-for-byte.
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), seeded);
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
                assert_eq!(catalog.agents.len(), 7, "{shape}: 7 agents seeded");
                let loaded = load_catalog(project.path()).expect("persisted catalog");
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

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            assert!(
                ensure_seeded(dir.path(), None).is_some(),
                "a support-gate change is part of the managed revision"
            );
            let base = base_json(dir.path());
            assert_eq!(base["agents"].as_array().unwrap().len(), 7);
            assert_eq!(
                base["managed"]["revision"],
                managed_content_sha256(&supported_shipped_definitions())
            );
            assert_eq!(load_catalog(dir.path()).unwrap().len(), 7);
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
        // A directory at the local path is preserved and disables the layer.
        let dir = seed_dir();
        let local = local_catalog_path(dir.path());
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("keep.txt"), b"keep").unwrap();
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert!(local.is_dir());
        assert_eq!(std::fs::read(local.join("keep.txt")).unwrap(), b"keep");
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

        // A user-authored regular local file is preserved byte-for-byte and
        // composes onto the fresh managed base.
        let dir = seed_dir();
        let user = r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##;
        write_local(dir.path(), user);
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert_eq!(read_text(&local_catalog_path(dir.path())), user);
        let loaded = load_catalog(dir.path()).unwrap();
        assert_eq!(loaded.len(), 9);
        assert_eq!(loaded.last().unwrap().key, "mine");
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
                {"key":"muse","remove":true}
            ],"order":["zeta","claude"]}"##,
        );
        let loaded = load_catalog(dir.path()).unwrap();
        let keys = keys_of(&loaded);
        assert_eq!(keys[0], "zeta", "listed survivors come first in order");
        assert_eq!(keys[1], "claude");
        assert!(!keys.contains(&"muse"), "the tombstone removes the key");
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
        let mut muse = shipped_def_json(&["muse"]).remove(0);
        muse["removable"] = serde_json::json!(false);
        write_managed_base(dir.path(), &[muse], "stale", true);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"muse","remove":true}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "localInvalid"));
        assert!(report.catalog.iter().any(|d| d.key == "muse"));
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
            read_text(&legacy.path().join("agents.local.json")),
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
            "the journal saved the full-table base"
        );

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            assert!(
                ensure_seeded(dir.path(), None).is_some(),
                "the journal saved base is completed first"
            );
            let base = base_json(dir.path());
            assert_eq!(
                base["agents"].as_array().unwrap().len(),
                7,
                "then the normal verified refresh runs under the same lock"
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
        let agents = shipped_def_json(&["claude", "muse"]);

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
            "The persisted managed catalog revision differs from this build. Current persisted entries remain usable. Project catalog refresh runs during initialization; instance catalogs remain read-only."
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
            "The persisted instance catalog revision differs from this build. Current persisted entries remain usable. Instance catalogs are read-only; select a project to initialize or refresh its catalog."
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
        let outcome = run_catalog_initialization(project.path(), Some(legacy.path()));
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
            std::fs::read(legacy.path().join("agents.json")).unwrap(),
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
            std::fs::read(legacy.path().join("agents.json")).unwrap(),
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

        let outcome = run_catalog_initialization(dir.path(), None);
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
        let refreshed = run_catalog_initialization(dir.path(), None);
        assert!(refreshed.published_at.is_some(), "release then refresh");
        let base_after = std::fs::read(&base).unwrap();
        let second = run_catalog_initialization(dir.path(), None);
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
            std::fs::read(legacy.path().join("agents.json")).unwrap(),
            legacy_bytes,
            "the instance source is never written by reads"
        );
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
        let outcome = run_catalog_initialization(&ac_dir, None);
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
        let _ = run_catalog_initialization(&ac_dir, None);
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
}
