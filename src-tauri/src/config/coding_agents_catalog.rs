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
const CATALOG_DIR_NAME: &str = "coding-agents";
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
/// IPC paths).
pub fn embedded_default_catalog() -> CodingAgentCatalog {
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

/// Validate entries per-entry (G7): a bad entry is logged and skipped, the valid
/// rest are kept. Duplicate keys are dropped (first wins). `source` labels the
/// origin in log lines (the manifest path, or the embedded default).
fn validate_and_filter(
    agents: Vec<CodingAgentDefinition>,
    source: &str,
) -> Vec<CodingAgentDefinition> {
    let mut out: Vec<CodingAgentDefinition> = Vec::with_capacity(agents.len());
    let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
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
        out.push(def);
    }
    out
}

/// The embedded default, validated (defensive; also dedups). Used as the
/// in-memory fallback when the on-disk manifest is missing or unparseable.
fn validated_embedded_default() -> Vec<CodingAgentDefinition> {
    validate_and_filter(
        embedded_default_catalog().agents,
        "embedded default catalog",
    )
}

/// Load the catalog for the `get_coding_agent_catalog` command.
///
/// Contract (§14.2): NEVER errors. Returns the validated on-disk agents when the
/// manifest parses; a valid empty list is honored verbatim (the user removed all
/// built-ins). A **missing** or **unparseable** manifest self-heals to the
/// embedded default IN MEMORY only, never writing to disk (G3 corrupt-preserve).
/// `ac_dir` is the project's `.ac` directory (or, for the legacy read fallback,
/// the legacy config dir, which yields `<config_dir>/coding-agents/agents.json`
/// through the same relative layout).
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
        Ok(catalog) => validate_and_filter(catalog.agents, &path.display().to_string()),
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
pub fn load_catalog_for_settings(settings: &AppSettings) -> Vec<CodingAgentDefinition> {
    match primary_project_root(settings) {
        Some(root) => load_catalog(&root.join(".ac")),
        None => crate::config::config_dir()
            .map(|dir| load_catalog(&dir))
            .unwrap_or_else(validated_embedded_default),
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
/// dir/symlink) seeds the embedded default. Returns the `Utc::now()` publication
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
                        EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes().to_vec()
                    }
                },
                // Absent, a directory, or a symlink: not a regular file -> the
                // embedded default wins.
                _ => EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes().to_vec(),
            }
        }
        None => EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes().to_vec(),
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
        command_basename: "claude",
        dest: ".claude",
        files: &[EmbeddedMasterFile {
            rel_path: "settings.json",
            bytes: include_bytes!("../../resources/coding-agents/_seed/.claude/settings.json"),
        }],
    },
    EmbeddedSeedMaster {
        command_basename: "codex",
        dest: ".codex",
        files: &[EmbeddedMasterFile {
            rel_path: "config.toml",
            bytes: include_bytes!("../../resources/coding-agents/_seed/.codex/config.toml"),
        }],
    },
    EmbeddedSeedMaster {
        command_basename: "opencode",
        dest: ".opencode",
        files: &[EmbeddedMasterFile {
            rel_path: "opencode.json",
            bytes: include_bytes!("../../resources/coding-agents/_seed/.opencode/opencode.json"),
        }],
    },
];

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
    EMBEDDED_SEED_MASTERS
        .iter()
        .find(|m| m.command_basename == basename)
}

/// Lowercased command executable basenames that ship a non-empty embedded default
/// config-folder master. The frontend enables the re-seed button only for a
/// catalog def whose command reduces to one of these; the reseed command
/// re-checks server-side. Derived from the shipped masters, so it stays in sync.
pub fn reseedable_command_basenames() -> Vec<String> {
    EMBEDDED_SEED_MASTERS
        .iter()
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

/// Seed the dest-keyed masters ONCE at boot: for each built-in master, if
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
/// wins).
pub fn ensure_seeded_masters(ac_dir: &Path, legacy_catalog_dir: Option<&Path>) {
    for master in EMBEDDED_SEED_MASTERS {
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

/// Seed the catalog + masters for one registered project root, then record the
/// catalog publication in that project's seed manifest.
///
/// Preconditions (enforced before ANY filesystem effect, in both entry points):
/// a non-absolute root (a hand-edited relative settings entry must never seed
/// relative to the process CWD) or a missing root (a deleted/stale registered
/// root must never be resurrected by the seed's `create_dir_all`) is logged and
/// skipped. Steady-state pre-check BEFORE gate acquisition: when the catalog
/// manifest AND every built-in master dir exist, return immediately (no lock,
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
    let ac_dir = project_root.join(crate::config::ac_root::CANONICAL_WORKSPACE_DIR);

    // Steady-state pre-check: everything already seeded -> nothing to publish;
    // no lock file, no canonicalize, no bounded manifest read, no write.
    if std::fs::symlink_metadata(manifest_path(&ac_dir)).is_ok()
        && EMBEDDED_SEED_MASTERS.iter().all(|master| {
            std::fs::symlink_metadata(master_dir_for_dest(&ac_dir, master.dest)).is_ok()
        })
    {
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

    /// (key, label, description, color, command, instructionsFilename) for the 6
    /// current presets. Drift guard: the embedded default must equal these exact
    /// values so the externalized catalog is byte-for-byte the pre-#769 list. The
    /// frontend keeps a parallel `FALLBACK_CODING_AGENTS` test (E7).
    #[allow(clippy::type_complexity)]
    const EXPECTED_PRESETS: [(&str, &str, &str, &str, &str, &str, Option<&str>); 6] = [
        (
            "claude",
            "Claude Code",
            "Coding Agent by Anthropic",
            "#d97706",
            "claude",
            "CLAUDE.md",
            Some(".claude"),
        ),
        (
            "codex",
            "Codex",
            "Coding Agent by OpenAI",
            "#10b981",
            "codex",
            "AGENTS.md",
            Some(".codex"),
        ),
        (
            "hermes",
            "Hermes",
            "Coding Agent by Nous Research",
            "#8b5cf6",
            "hermes",
            "AGENTS.md",
            None,
        ),
        (
            "cursor",
            "Cursor CLI",
            "Coding Agent by Cursor",
            "#22d3ee",
            "agent",
            "AGENTS.md",
            None,
        ),
        (
            "pi",
            "Pi",
            "Coding Agent by Earendil Inc",
            "#ec4899",
            "pi",
            "AGENTS.md",
            None,
        ),
        (
            "opencode",
            "OpenCode",
            "Open-source terminal coding agent by Anomaly",
            "#64748b",
            "opencode",
            "AGENTS.md",
            Some(".opencode"),
        ),
    ];

    fn manifest_json(agents_json: &str) -> String {
        format!("{{\"schemaVersion\":1,\"agents\":{agents_json}}}")
    }

    fn seed_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn embedded_default_parses_with_six_agents_in_order() {
        let catalog = embedded_default_catalog();
        assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
        let keys: Vec<&str> = catalog.agents.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(
            keys,
            ["claude", "codex", "hermes", "cursor", "pi", "opencode"]
        );
        // OpenCode is last and carries the #768 "by Anomaly" subtitle.
        let last = catalog.agents.last().unwrap();
        assert_eq!(last.key, "opencode");
        assert_eq!(
            last.description,
            "Open-source terminal coding agent by Anomaly"
        );
    }

    #[test]
    fn embedded_default_matches_current_presets_exactly() {
        // Drift guard vs the pre-#769 AGENT_PRESETS field values.
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
            assert_eq!(def.instructions_filename.as_deref(), Some(filename));
            // #769 P2: Claude/Codex/OpenCode ship an active configSeed; the other
            // three ship none (no master, no re-seed button).
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
        assert_eq!(agents.len(), 6);
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
            6,
            "corrupt file self-heals to embedded default"
        );
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
        assert_eq!(load_catalog(dir.path()).len(), 6);

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
        assert_eq!(load_catalog(project.path()).len(), 6);

        // Legacy agents.json is a DIRECTORY -> not a regular file -> embedded.
        let project = seed_dir();
        std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
        ensure_seeded(project.path(), Some(legacy.path()));
        assert_eq!(load_catalog(project.path()).len(), 6);

        // No legacy at all -> embedded default.
        let project = seed_dir();
        ensure_seeded(project.path(), None);
        assert_eq!(load_catalog(project.path()).len(), 6);
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
        assert_eq!(load_catalog_for_settings(&settings).len(), 6);

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
        assert_eq!(load_catalog_for_settings(&settings).len(), 6);
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
        assert_eq!(load_catalog(project.path()).len(), 6);
        // Recovery: deleting the project file re-seeds the embedded default.
        std::fs::remove_file(&project_file).unwrap();
        ensure_seeded(project.path(), Some(legacy.path()));
        assert_eq!(load_catalog(project.path()).len(), 6);
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
    fn embedded_default_ships_claude_pi_and_codex_update_commands() {
        // #1318/#1325 drift guard: claude, pi, and codex ship the update
        // command; the other three ship none; every entry defaults autoUpdate
        // to false.
        let catalog = embedded_default_catalog();
        assert_eq!(catalog.agents.len(), 6);
        for def in &catalog.agents {
            assert!(
                !def.auto_update,
                "{} autoUpdate must default false",
                def.key
            );
            if def.key == "claude" {
                assert_eq!(def.update_commands, vec!["claude --update".to_string()]);
            } else if def.key == "pi" {
                assert_eq!(def.update_commands, vec!["pi update".to_string()]);
            } else if def.key == "codex" {
                assert_eq!(def.update_commands, vec!["codex update".to_string()]);
            } else {
                assert!(
                    def.update_commands.is_empty(),
                    "{} must ship no update command",
                    def.key
                );
            }
        }
    }
}
