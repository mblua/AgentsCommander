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

use std::sync::atomic::AtomicU64;
use std::time::Duration;

#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::Instant;

#[cfg(test)]
use chrono::Utc;

#[cfg(test)]
use crate::config::naming_migration;
#[cfg(test)]
use crate::config::seed_manifest::{
    has_catalog_publication, ManifestActivationToken, ManifestPathIdentity,
    ProjectSeedManifestGuard, PublishedManifestRow, SEED_MANIFEST_FILENAME,
};
#[cfg(test)]
use crate::config::settings::{AppSettings, CodingAgentEnvSource};

mod definition;
mod initialization;
mod layer_local;
mod layer_managed;
mod migration;
mod publication;
mod report;
mod seed_master;
mod snapshot;

use definition::{
    catalog_dir, is_supported_builtin, manifest_path, validate_definition,
    validate_idle_burst_config, validate_install_commands,
};
#[cfg(test)]
use definition::{
    command_is_present_with, install_command_for_os, load_catalog_for_settings_with_config_dir,
    parse_independent_catalog, tested_level_for, welcome_status_for_with,
};
pub(crate) use definition::{
    embedded_default_catalog, independent_catalog_path, primary_project_root,
    registered_project_roots, resolve_install_command, validate_catalog_key,
};
pub use definition::{
    load_catalog, load_catalog_for_settings, parse_catalog_schema2, welcome_status_for,
    CodingAgentCatalog, CodingAgentCatalogSchema2, CodingAgentDefinition, CodingAgentWelcomeStatus,
    IdentityCatalogError, IdleBurstConfig, InstallCommands, TestedLevel,
};
pub use initialization::ensure_seeded;
#[cfg(test)]
use initialization::{
    catalog_scope_key, ensure_seeded_for_project_in, ensure_seeded_instance_in,
    run_catalog_initialization, run_catalog_initialization_with_scope, InitScope,
    CATALOG_FAMILY_RENAMES, RETIRED_CATALOG_LOCK_FILENAME,
};
pub(crate) use initialization::{ensure_seeded_for_project_with_token, ensure_seeded_instance};
use layer_local::{
    apply_support_gate, compose_local_layer, parse_local_layer, validate_update_command_string,
    ConfigSeedPatch, InstallCommandsPatch, LOCAL_IDLE_BURST_FIELDS,
};
#[cfg(test)]
use layer_local::{build_new_definition, parse_local_fields};
pub(crate) use layer_managed::parse_strict_json;
use layer_managed::{
    build_managed_base_bytes, build_managed_base_bytes_with_original, expect_json_object,
    expect_json_string, local_catalog_path, managed_content_sha256, migration_journal_path,
    original_catalog_command, project_catalog_path, publication_temp_name_for_destination,
    reject_unknown_json_fields, sha256_hex, supported_shipped_definitions, CatalogReadScope,
    LayerDescription, LocalFieldPatch, ManagedCatalogMarker, CATALOG_LOCK_FILENAME,
    CATALOG_LOCK_POLL_INTERVAL, CATALOG_LOCK_TIMEOUT, LOCAL_CATALOG_FILENAME, LOCAL_STUB_BYTES,
    MANAGED_MARKER_FIELDS, MANAGED_OWNER, MANAGED_VERSION, MIGRATION_BACKUP_FILENAME,
    MIGRATION_JOURNAL_FILENAME, MIGRATION_JOURNAL_VERSION, REPORT_CODE_LOCAL_INVALID,
    REPORT_CODE_MANAGED_BASE_EDITED, REPORT_CODE_MIGRATION_CONFLICT, REPORT_CODE_PROJECT_INVALID,
    REPORT_CODE_PUBLICATION_UNTRACKED, REPORT_CODE_REFRESH_FAILED,
};
#[cfg(test)]
use migration::{
    extract_legacy_local, extract_legacy_local_mode, local_layer_bytes,
    local_layer_bytes_with_original, materialize_complete_legacy_fields,
    pin_explicit_legacy_fields, LocalRowWire,
};
use migration::{
    fresh_initialize_catalog, migrate_legacy, parse_migration_journal,
    recover_interrupted_migration, refresh_managed_base, resume_backup_only_migration,
    CatalogInitOutcome, MIGRATION_SOURCE_INSTANCE, MIGRATION_SOURCE_PROJECT,
};
use publication::{
    acquire_catalog_lock, ensure_catalog_dir, publish_exclusive, publish_replace, CatalogPaths,
};
#[cfg(test)]
use report::load_catalog_report_with_context;
use report::{
    catalog_diagnostic, is_reparse_point, load_catalog_report_for_settings_with_config_dir,
    read_instance_legacy_source, read_optional_regular_file, CatalogSourceContext,
    KNOWN_CONFIG_SEED_FIELDS, KNOWN_DEFINITION_FIELDS, KNOWN_ENV_FIELDS,
    KNOWN_INSTALL_COMMANDS_FIELDS, KNOWN_ROOT_FIELDS, REPORT_CODE_BASE_INVALID,
    REPORT_CODE_BASE_UNAVAILABLE, REPORT_CODE_DUPLICATE_KEY, REPORT_CODE_INVALID_DEFINITION,
    REPORT_CODE_MIGRATION_PENDING,
};
pub use report::{
    load_catalog_report, load_catalog_report_for_settings, CatalogDiagnostic, CatalogReport,
    CatalogUnavailable,
};
#[cfg(test)]
use seed_master::{all_masters_present, EmbeddedSeedMaster, EMBEDDED_SEED_MASTERS};
pub(crate) use seed_master::{
    command_account_key, command_executable_basename, ensure_seeded_for_project,
    publish_selected_personal, replace_selected_personal_bytes,
};
pub use seed_master::{
    ensure_seeded_masters, master_dir_for_dest, reseed_master_for_command,
    reseedable_command_basenames, ReseedResult,
};
use snapshot::{
    analyze_base_bytes, definition_problem_reason, managed_base_is_stale,
    push_unknown_field_warnings, raw_install_commands_problem, raw_update_commands_problem,
    read_catalog_snapshot, resolve_catalog_snapshot, unknown_field_names, BaseAnalysis,
    CatalogBaseKind,
};
#[cfg(test)]
use snapshot::{read_catalog_snapshot_with, CatalogSnapshot, ResolvedCatalog};

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

/// Unique-suffix counter for the seed temp file (mirrors the pattern in
/// `seeded_context_templates::unique_state_temp_path`).
static SEED_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

/// Serialize all re-seeds (rare, user-initiated) so two never race on a master
/// mid-swap. Stricter than strictly per-dest, which is safe (never a partial or
/// racy state).
static RESEED_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn hold_selected_catalog_test_lock(path: &Path) -> impl std::fmt::Debug {
    acquire_catalog_lock(&CatalogPaths::new(path.parent().expect("catalog parent"))).unwrap()
}

#[cfg(test)]
mod tests;
