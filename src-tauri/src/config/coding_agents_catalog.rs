//! #769 Phase 1 - externalized, user-editable coding-agent catalog.
//!
//! The catalog is the "built-in coding agents you can add" list (display name,
//! command, color, instructions filename, ...). Before #769 it was hardcoded in
//! the frontend (`src/shared/agent-presets.ts`). This module makes it a
//! backend-owned, on-disk, user-editable JSON manifest at
//! `config_dir()/coding-agents/agents.json`, seeded once from an embedded default
//! and user-owned thereafter.
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

use serde::{Deserialize, Serialize};

use crate::config::agent_command::is_safe_instructions_filename;
use crate::config::settings::{
    validate_agent_command_text, validate_config_seed_dest, validate_env_rows, CodingAgentEnv,
    ConfigSeedConfig,
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

fn catalog_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(CATALOG_DIR_NAME)
}

fn manifest_path(config_dir: &Path) -> PathBuf {
    catalog_dir(config_dir).join(CATALOG_MANIFEST_FILENAME)
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
            return Err(format!(
                "{context}: unsafe instructions filename '{name}'"
            ));
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
    validate_and_filter(embedded_default_catalog().agents, "embedded default catalog")
}

/// Load the catalog for the `get_coding_agent_catalog` command.
///
/// Contract (§14.2): NEVER errors. Returns the validated on-disk agents when the
/// manifest parses; a valid empty list is honored verbatim (the user removed all
/// built-ins). A **missing** or **unparseable** manifest self-heals to the
/// embedded default IN MEMORY only, never writing to disk (G3 corrupt-preserve).
pub fn load_catalog(config_dir: &Path) -> Vec<CodingAgentDefinition> {
    let path = manifest_path(config_dir);
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

/// Seed the manifest ONCE at boot: write the embedded default iff `agents.json`
/// is absent, then never touch it (§14.1 whole-file seed-once). Fail-soft: logs
/// and returns on any error; it must never panic or abort boot.
pub fn ensure_seeded(config_dir: &Path) {
    let dir = catalog_dir(config_dir);
    let path = manifest_path(config_dir);

    // Seed-once: any existing entry (file, dir, or link) means the catalog is
    // user-owned; leave it strictly alone.
    match std::fs::symlink_metadata(&path) {
        Ok(_) => return,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log::warn!(
                "[coding-agents] cannot stat {} ({e}); skipping catalog seed",
                path.display()
            );
            return;
        }
    }

    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!(
            "[coding-agents] failed to create {} ({e}); skipping catalog seed",
            dir.display()
        );
        return;
    }

    match write_manifest_atomic(&path, EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes()) {
        Ok(()) => log::info!("[coding-agents] seeded default catalog at {}", path.display()),
        Err(e) => log::warn!("[coding-agents] failed to seed {} ({e})", path.display()),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// (key, label, description, color, command, instructionsFilename) for the 6
    /// current presets. Drift guard: the embedded default must equal these exact
    /// values so the externalized catalog is byte-for-byte the pre-#769 list. The
    /// frontend keeps a parallel `FALLBACK_CODING_AGENTS` test (E7).
    const EXPECTED_PRESETS: [(&str, &str, &str, &str, &str, &str); 6] = [
        ("claude", "Claude Code", "Coding Agent by Anthropic", "#d97706", "claude", "CLAUDE.md"),
        ("codex", "Codex", "Coding Agent by OpenAI", "#10b981", "codex", "AGENTS.md"),
        ("hermes", "Hermes", "Coding Agent by Nous Research", "#8b5cf6", "hermes", "AGENTS.md"),
        ("cursor", "Cursor CLI", "Coding Agent by Cursor", "#22d3ee", "agent", "AGENTS.md"),
        ("pi", "Pi", "Coding Agent by Earendil Inc", "#ec4899", "pi", "AGENTS.md"),
        (
            "opencode",
            "OpenCode",
            "Open-source terminal coding agent by Anomaly",
            "#64748b",
            "opencode",
            "AGENTS.md",
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
        assert_eq!(keys, ["claude", "codex", "hermes", "cursor", "pi", "opencode"]);
        // OpenCode is last and carries the #768 "by Anomaly" subtitle.
        let last = catalog.agents.last().unwrap();
        assert_eq!(last.key, "opencode");
        assert_eq!(last.description, "Open-source terminal coding agent by Anomaly");
    }

    #[test]
    fn embedded_default_matches_current_presets_exactly() {
        // Drift guard vs the pre-#769 AGENT_PRESETS field values.
        let catalog = embedded_default_catalog();
        assert_eq!(catalog.agents.len(), EXPECTED_PRESETS.len());
        for (def, (key, label, desc, color, command, filename)) in
            catalog.agents.iter().zip(EXPECTED_PRESETS)
        {
            assert_eq!(def.key, key);
            assert_eq!(def.label, label);
            assert_eq!(def.description, desc);
            assert_eq!(def.color, color);
            assert_eq!(def.command, command);
            assert_eq!(def.instructions_filename.as_deref(), Some(filename));
            // Phase-1 invariants: no folder seed ships; every built-in removable;
            // no base env rows; not isolated.
            assert!(def.config_seed.is_none(), "{key} must ship configSeed UNSET");
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
        for bad in ["", "Claude", "cursor_cli", "cursor cli", "café", "a.b", "UP", "a/b"] {
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
        assert_eq!(agents.len(), 6, "corrupt file self-heals to embedded default");
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

        ensure_seeded(dir.path());
        assert!(path.exists(), "seed writes the manifest when absent");
        assert_eq!(load_catalog(dir.path()).len(), 6);

        // Idempotent + never clobbers a user edit: hand-edit to a single custom
        // agent, re-seed, and confirm the edit is preserved.
        let custom = manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        std::fs::write(&path, &custom).unwrap();
        ensure_seeded(dir.path());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
        let loaded = load_catalog(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "mine");
    }

    #[test]
    fn seeded_manifest_leaves_no_temp_residue() {
        let dir = seed_dir();
        ensure_seeded(dir.path());
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
}
