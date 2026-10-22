use super::*;

mod builtin_support;
mod catalog_rename_migration;
mod catalog_report;
mod e2432_catalog_profile_tests;
mod e2432_p02_catalog_publication_tests;
mod embedded_default;
mod identity_schema2_tests;
mod install_commands_2736;
mod instance_seed;
mod legacy_migration;
mod load_and_seed;
mod managed_catalog_base;
mod managed_catalog_migration;
mod managed_catalog_publication;
mod p02_catalog_schema_tests;
mod project_layers;
mod scope_b_2800;
mod update_commands;
mod welcome_2736;

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

/// #2736 - the shipped `installCommands` value for `key`, if any.
fn expected_install_2736(key: &str) -> Option<&'static str> {
    match key {
        "claude" | "codex" | "hermes" | "cursor" | "pi" | "opencode" | "antigravity" | "grok" => {
            Some("echo No verified installer for this platform 1>&2 && exit 1")
        }
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

// ---- #769 Phase 2: masters + re-seed --------------------------------

fn master(cmd: &str) -> &'static EmbeddedSeedMaster {
    EMBEDDED_SEED_MASTERS
        .iter()
        .find(|m| m.command_basename == cmd)
        .unwrap()
}

// ---- #1318 relocation, migration, per-project seeding ----------------

fn legacy_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// A hand-authored legacy catalog (custom entry, byte-distinct from the
/// embedded default). The tempdir IS the legacy `<config_dir>/coding-agents`
/// directory.
fn legacy_catalog_json() -> Vec<u8> {
    manifest_json(concat!(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","##,
        r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    ))
    .into_bytes()
}

// ---- #1912 support switch: read gate + seed gate --------------------

fn assert_no_key(agents: &[CodingAgentDefinition], key: &str) {
    assert!(!agents.iter().any(|a| a.key == key), "{key} must be absent");
}

fn keys_of(agents: &[CodingAgentDefinition]) -> Vec<&str> {
    agents.iter().map(|a| a.key.as_str()).collect()
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

#[cfg(windows)]
fn create_manifest_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn create_manifest_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
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
    let user = concat!(
        r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","##,
        r##""color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"##,
        r##""updateCommands":[],"autoUpdate":false}]}"##,
    );
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

// -----------------------------------------------------------------------
// R2 correction fixtures (F1 read warning, F3 OS failures, F5 stub)
// -----------------------------------------------------------------------

/// The canonical empty coverage-v2 manifest (the serializer's own bytes),
/// used as the repaired-manifest fixture.
const REPAIRED_EMPTY_MANIFEST: &[u8] = concat!(
    "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
    "schema_version = 1\n",
    "coverage_version = 2\n",
    "coverage = [\"project_context_templates\", \"replica_config_folders\", \
        \"coding_agent_catalog\"]\n",
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

// ---- F3: real OS failures and injected arm-level boundaries ------------

// ---- F5: transient stub failure, no persistent state -------------------

const LOCK_CHILD_ACTION_ENV: &str = "AC_1968_CATALOG_LOCK_CHILD_ACTION";
const LOCK_CHILD_DIR_ENV: &str = "AC_1968_CATALOG_LOCK_CHILD_DIR";
const LOCK_CHILD_TEST_FQN: &str =
    "config::coding_agents_catalog::tests::managed_catalog_publication::managed_catalog_lock_child";

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
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    value["agents"]
        .as_array()
        .is_some_and(|agents| agents.iter().any(|agent| agent["key"] == key))
}

const B2_CHILD_ROOT_ENV: &str = "AC_2715_CATALOG_CHILD_ROOT";
const B2_CHILD_JOURNAL_ENV: &str = "AC_2715_CATALOG_CHILD_JOURNAL";
const B2_CHILD_TEST_FQN: &str =
    "config::coding_agents_catalog::tests::catalog_rename_migration::b2_catalog_migration_child";

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

const SCOPE_B_EXPECTED_2800: &str = concat!(
    r#####"{"claude":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy B"#####,
    r#####"ypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBl"#####,
    r#####"AGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7AC"#####,
    r#####"AAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQA"#####,
    r#####"TQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwBsAGEAdQBkAGUALgBhAGkALw"#####,
    r#####"BpAG4AcwB0AGEAbABsAC4AcABzADEAJwAgAC0AVABpAG0AZQBvAHUAdABTAGUAYwAgADEAMgAwACAALQBF"#####,
    r#####"AHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwADsAIABpAGYAIAAoAFsAcwB0AHIAaQBuAGcAXQA6AD"#####,
    r#####"oASQBzAE4AdQBsAGwATwByAFcAaABpAHQAZQBTAHAAYQBjAGUAKAAkAGEAYwBfAGkAbgBzAHQAYQBsAGwA"#####,
    r#####"XwBzAGMAcgBpAHAAdAApACkAIAB7ACAAdABoAHIAbwB3ACAAJwBFAG0AcAB0AHkAIABpAG4AcwB0AGEAbA"#####,
    r#####"BsAGUAcgAgAHIAZQBzAHAAbwBuAHMAZQAnACAAfQA7ACAAJABhAGMAXwBwAGEAdABoAD0ASgBvAGkAbgAt"#####,
    r#####"AFAAYQB0AGgAIAAoAFsASQBPAC4AUABhAHQAaABdADoAOgBHAGUAdABUAGUAbQBwAFAAYQB0AGgAKAApAC"#####,
    r#####"kAIAAoACcAYQBjAC0AaQBuAHMAdABhAGwAbAAtADIANwA4ADcALQAnACsAWwBHAHUAaQBkAF0AOgA6AE4A"#####,
    r#####"ZQB3AEcAdQBpAGQAKAApAC4AVABvAFMAdAByAGkAbgBnACgAJwBOACcAKQArACcALgBwAHMAMQAnACkAOw"#####,
    r#####"AgACQAYQBjAF8AZgBpAGwAZQA9AFsASQBPAC4ARgBpAGwAZQBdADoAOgBPAHAAZQBuACgAJABhAGMAXwBw"#####,
    r#####"AGEAdABoACwAWwBJAE8ALgBGAGkAbABlAE0AbwBkAGUAXQA6ADoAQwByAGUAYQB0AGUATgBlAHcALABbAE"#####,
    r#####"kATwAuAEYAaQBsAGUAQQBjAGMAZQBzAHMAXQA6ADoAVwByAGkAdABlACwAWwBJAE8ALgBGAGkAbABlAFMA"#####,
    r#####"aABhAHIAZQBdADoAOgBOAG8AbgBlACkAOwAgACQAYQBjAF8AYwByAGUAYQB0AGUAZAA9ACQAdAByAHUAZQ"#####,
    r#####"A7ACAAdAByAHkAIAB7ACAAWwBiAHkAdABlAFsAXQBdACQAYQBjAF8AYgB5AHQAZQBzAD0AWwBUAGUAeAB0"#####,
    r#####"AC4ARQBuAGMAbwBkAGkAbgBnAF0AOgA6AFUAVABGADgALgBHAGUAdABQAHIAZQBhAG0AYgBsAGUAKAApAC"#####,
    r#####"sAWwBUAGUAeAB0AC4ARQBuAGMAbwBkAGkAbgBnAF0AOgA6AFUAVABGADgALgBHAGUAdABCAHkAdABlAHMA"#####,
    r#####"KAAkAGEAYwBfAGkAbgBzAHQAYQBsAGwAXwBzAGMAcgBpAHAAdAApADsAIAAkAGEAYwBfAGYAaQBsAGUALg"#####,
    r#####"BXAHIAaQB0AGUAKAAkAGEAYwBfAGIAeQB0AGUAcwAsADAALAAkAGEAYwBfAGIAeQB0AGUAcwAuAEwAZQBu"#####,
    r#####"AGcAdABoACkAIAB9ACAAZgBpAG4AYQBsAGwAeQAgAHsAIAAkAGEAYwBfAGYAaQBsAGUALgBEAGkAcwBwAG"#####,
    r#####"8AcwBlACgAKQAgAH0AOwAgACQAZwBsAG8AYgBhAGwAOgBMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA9ACQA"#####,
    r#####"bgB1AGwAbAA7ACAAJgAgACgASgBvAGkAbgAtAFAAYQB0AGgAIAAkAFAAUwBIAE8ATQBFACAAJwBwAG8Adw"#####,
    r#####"BlAHIAcwBoAGUAbABsAC4AZQB4AGUAJwApACAALQBOAG8AUAByAG8AZgBpAGwAZQAgAC0ATgBvAG4ASQBu"#####,
    r#####"AHQAZQByAGEAYwB0AGkAdgBlACAALQBFAHgAZQBjAHUAdABpAG8AbgBQAG8AbABpAGMAeQAgAEIAeQBwAG"#####,
    r#####"EAcwBzACAALQBGAGkAbABlACAAJABhAGMAXwBwAGEAdABoADsAIAAkAGEAYwBfAGUAeABpAHQAPQAkAEwA"#####,
    r#####"QQBTAFQARQBYAEkAVABDAE8ARABFADsAIABpAGYAIAAoACQAbgB1AGwAbAAgAC0AZQBxACAAJABhAGMAXw"#####,
    r#####"BlAHgAaQB0ACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAYwBhAHQAYwBoACAAewAg"#####,
    r#####"AFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0AGUATABpAG4AZQAoACQAXwApAD"#####,
    r#####"sAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAaQBmACAAKAAkAGEA"#####,
    r#####"YwBfAGMAcgBlAGEAdABlAGQAKQAgAHsAIAB0AHIAeQAgAHsAIABSAGUAbQBvAHYAZQAtAEkAdABlAG0AIA"#####,
    r#####"AtAEwAaQB0AGUAcgBhAGwAUABhAHQAaAAgACQAYQBjAF8AcABhAHQAaAAgAC0ARgBvAHIAYwBlACAALQBF"#####,
    r#####"AHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAH"#####,
    r#####"MAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAaQBmACAA"#####,
    r#####"KAAkAGEAYwBfAGUAeABpAHQAIAAtAGUAcQAgADAAKQAgAHsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQ"#####,
    r#####"AgAH0AIAB9ACAAfQA7ACAAZQB4AGkAdAAgACQAYQBjAF8AZQB4AGkAdAA=","#####,
    r#####""macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://claude.ai/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac"#####,
    r#####"_install_script\" | bash","#####,
    r#####""linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://claude.ai/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac"#####,
    r#####"_install_script\" | bash","#####,
    r#####""default":"echo No verified installer for this platform 1>&2 && exit 1"},"#####,
    r#####""codex":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Byp"#####,
    r#####"ass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAG"#####,
    r#####"EAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAA"#####,
    r#####"JABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQ"#####,
    r#####"BlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwBoAGEAdABnAHAAdAAuAGMAbwBt"#####,
    r#####"AC8AYwBvAGQAZQB4AC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAG"#####,
    r#####"MAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMA"#####,
    r#####"dAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXw"#####,
    r#####"BpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5"#####,
    r#####"ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAH"#####,
    r#####"QAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0A"#####,
    r#####"cABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARw"#####,
    r#####"B1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAn"#####,
    r#####"AC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAG"#####,
    r#####"UAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEA"#####,
    r#####"dABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQ"#####,
    r#####"BPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABl"#####,
    r#####"AGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AG"#####,
    r#####"UAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUA"#####,
    r#####"YQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARw"#####,
    r#####"BlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABh"#####,
    r#####"AGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAH"#####,
    r#####"kAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkA"#####,
    r#####"bABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQ"#####,
    r#####"BUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABP"#####,
    r#####"AE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAG"#####,
    r#####"UAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwA"#####,
    r#####"aQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXw"#####,
    r#####"BlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAt"#####,
    r#####"AGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAG"#####,
    r#####"MAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwA"#####,
    r#####"aQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAew"#####,
    r#####"AgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2"#####,
    r#####"AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAE"#####,
    r#####"YAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgA"#####,
    r#####"IAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJA"#####,
    r#####"BfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBl"#####,
    r#####"AHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","#####,
    r#####""macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://chatgpt.com/codex/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\"#####,
    r#####"n' \"$ac_install_script\" | sh","#####,
    r#####""linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://chatgpt.com/codex/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\"#####,
    r#####"n' \"$ac_install_script\" | sh","#####,
    r#####""default":"echo No verified installer for this platform 1>&2 && exit 1"},"#####,
    r#####""hermes":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy By"#####,
    r#####"pass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlA"#####,
    r#####"GEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACA"#####,
    r#####"AJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQAT"#####,
    r#####"QBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AaABlAHIAbQBlAHMALQBhAGcAZQB"#####,
    r#####"uAHQALgBuAG8AdQBzAHIAZQBzAGUAYQByAGMAaAAuAGMAbwBtAC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxA"#####,
    r#####"CcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACA"#####,
    r#####"AUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAa"#####,
    r#####"QB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewA"#####,
    r#####"gAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzA"#####,
    r#####"GUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFA"#####,
    r#####"AYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAY"#####,
    r#####"QBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwB"#####,
    r#####"TAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbA"#####,
    r#####"EkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGw"#####,
    r#####"AZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAc"#####,
    r#####"wBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQA"#####,
    r#####"pADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5A"#####,
    r#####"HQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADo"#####,
    r#####"AOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZ"#####,
    r#####"ABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbAB"#####,
    r#####"sAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiA"#####,
    r#####"HkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGE"#####,
    r#####"AbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAb"#####,
    r#####"ABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwB"#####,
    r#####"pAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlA"#####,
    r#####"CcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0"#####,
    r#####"ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAY"#####,
    r#####"QBjAF8AcABhAHQAaAAgAC0ATgBvAG4ASQBuAHQAZQByAGEAYwB0AGkAdgBlADsAIAAkAGEAYwBfAGUAeAB"#####,
    r#####"pAHQAPQAkAEwAQQBTAFQARQBYAEkAVABDAE8ARABFADsAIABpAGYAIAAoACQAbgB1AGwAbAAgAC0AZQBxA"#####,
    r#####"CAAJABhAGMAXwBlAHgAaQB0ACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAYwBhAHQ"#####,
    r#####"AYwBoACAAewAgAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0AGUATABpAG4AZ"#####,
    r#####"QAoACQAXwApADsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAaQB"#####,
    r#####"mACAAKAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAKQAgAHsAIAB0AHIAeQAgAHsAIABSAGUAbQBvAHYAZQAtA"#####,
    r#####"EkAdABlAG0AIAAtAEwAaQB0AGUAcgBhAGwAUABhAHQAaAAgACQAYQBjAF8AcABhAHQAaAAgAC0ARgBvAHI"#####,
    r#####"AYwBlACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwACAAfQAgAGMAYQB0AGMAaAAgAHsAI"#####,
    r#####"ABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA"#####,
    r#####"7ACAAaQBmACAAKAAkAGEAYwBfAGUAeABpAHQAIAAtAGUAcQAgADAAKQAgAHsAIAAkAGEAYwBfAGUAeABpA"#####,
    r#####"HQAPQAxACAAfQAgAH0AIAB9ACAAfQA7ACAAZQB4AGkAdAAgACQAYQBjAF8AZQB4AGkAdAA=","#####,
    r#####""macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://hermes-agent.nousresearch.com/install.sh') && [ -n \"$ac_install_script\" ] && "#####,
    r#####"printf '%s\\n' \"$ac_install_script\" | bash -s -- --non-interactive","#####,
    r#####""linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://hermes-agent.nousresearch.com/install.sh') && [ -n \"$ac_install_script\" ] && "#####,
    r#####"printf '%s\\n' \"$ac_install_script\" | bash -s -- --non-interactive","#####,
    r#####""default":"echo No verified installer for this platform 1>&2 && exit 1"},"#####,
    r#####""cursor":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy By"#####,
    r#####"pass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlA"#####,
    r#####"GEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACA"#####,
    r#####"AJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQAT"#####,
    r#####"QBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwB1AHIAcwBvAHIALgBjAG8AbQA"#####,
    r#####"vAGkAbgBzAHQAYQBsAGwAPwB3AGkAbgAzADIAPQB0AHIAdQBlACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlA"#####,
    r#####"GMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHM"#####,
    r#####"AdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAX"#####,
    r#####"wBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB"#####,
    r#####"5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhA"#####,
    r#####"HQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0"#####,
    r#####"AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsAR"#####,
    r#####"wB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwA"#####,
    r#####"nAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwA"#####,
    r#####"GUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGE"#####,
    r#####"AdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsAS"#####,
    r#####"QBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdAB"#####,
    r#####"lAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0A"#####,
    r#####"GUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGU"#####,
    r#####"AYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4AR"#####,
    r#####"wBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJAB"#####,
    r#####"hAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiA"#####,
    r#####"HkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGk"#####,
    r#####"AbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgAS"#####,
    r#####"QBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASAB"#####,
    r#####"PAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsA"#####,
    r#####"GUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGw"#####,
    r#####"AaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAX"#####,
    r#####"wBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAA"#####,
    r#####"tAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgA"#####,
    r#####"GMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEw"#####,
    r#####"AaQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAe"#####,
    r#####"wAgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB"#####,
    r#####"2AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtA"#####,
    r#####"EYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGg"#####,
    r#####"AIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJ"#####,
    r#####"ABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwB"#####,
    r#####"lAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","#####,
    r#####""macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://cursor.com/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_i"#####,
    r#####"nstall_script\" | bash","#####,
    r#####""linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://cursor.com/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_i"#####,
    r#####"nstall_script\" | bash","#####,
    r#####""default":"echo No verified installer for this platform 1>&2 && exit 1"},"#####,
    r#####""pi":{"windows":"npm install -g --ignore-scripts --include=optional @earendil-work"#####,
    r#####"s/pi-coding-agent","#####,
    r#####""macos":"npm install -g --ignore-scripts --include=optional @earendil-works/pi-cod"#####,
    r#####"ing-agent","#####,
    r#####""linux":"npm install -g --ignore-scripts --include=optional @earendil-works/pi-cod"#####,
    r#####"ing-agent","#####,
    r#####""default":"echo No verified installer for this platform 1>&2 && exit 1"},"#####,
    r#####""opencode":{"windows":"npm install -g --ignore-scripts=false --include=optional --"#####,
    r#####"allow-scripts=opencode-ai opencode-ai","#####,
    r#####""macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://opencode.ai/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_"#####,
    r#####"install_script\" | bash","#####,
    r#####""linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://opencode.ai/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_"#####,
    r#####"install_script\" | bash","#####,
    r#####""default":"echo No verified installer for this platform 1>&2 && exit 1"},"#####,
    r#####""antigravity":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPoli"#####,
    r#####"cy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMA"#####,
    r#####"cgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIA"#####,
    r#####"B7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBz"#####,
    r#####"AHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYQBuAHQAaQBnAHIAYQB2AG"#####,
    r#####"kAdAB5AC4AZwBvAG8AZwBsAGUALwBjAGwAaQAvAGkAbgBzAHQAYQBsAGwALgBwAHMAMQAnACAALQBUAGkA"#####,
    r#####"bQBlAG8AdQB0AFMAZQBjACAAMQAyADAAIAAtAEUAcgByAG8AcgBBAGMAdABpAG8AbgAgAFMAdABvAHAAOw"#####,
    r#####"AgAGkAZgAgACgAWwBzAHQAcgBpAG4AZwBdADoAOgBJAHMATgB1AGwAbABPAHIAVwBoAGkAdABlAFMAcABh"#####,
    r#####"AGMAZQAoACQAYQBjAF8AaQBuAHMAdABhAGwAbABfAHMAYwByAGkAcAB0ACkAKQAgAHsAIAB0AGgAcgBvAH"#####,
    r#####"cAIAAnAEUAbQBwAHQAeQAgAGkAbgBzAHQAYQBsAGwAZQByACAAcgBlAHMAcABvAG4AcwBlACcAIAB9ADsA"#####,
    r#####"IAAkAGEAYwBfAHAAYQB0AGgAPQBKAG8AaQBuAC0AUABhAHQAaAAgACgAWwBJAE8ALgBQAGEAdABoAF0AOg"#####,
    r#####"A6AEcAZQB0AFQAZQBtAHAAUABhAHQAaAAoACkAKQAgACgAJwBhAGMALQBpAG4AcwB0AGEAbABsAC0AMgA3"#####,
    r#####"ADgANwAtACcAKwBbAEcAdQBpAGQAXQA6ADoATgBlAHcARwB1AGkAZAAoACkALgBUAG8AUwB0AHIAaQBuAG"#####,
    r#####"cAKAAnAE4AJwApACsAJwAuAHAAcwAxACcAKQA7ACAAJABhAGMAXwBmAGkAbABlAD0AWwBJAE8ALgBGAGkA"#####,
    r#####"bABlAF0AOgA6AE8AcABlAG4AKAAkAGEAYwBfAHAAYQB0AGgALABbAEkATwAuAEYAaQBsAGUATQBvAGQAZQ"#####,
    r#####"BdADoAOgBDAHIAZQBhAHQAZQBOAGUAdwAsAFsASQBPAC4ARgBpAGwAZQBBAGMAYwBlAHMAcwBdADoAOgBX"#####,
    r#####"AHIAaQB0AGUALABbAEkATwAuAEYAaQBsAGUAUwBoAGEAcgBlAF0AOgA6AE4AbwBuAGUAKQA7ACAAJABhAG"#####,
    r#####"MAXwBjAHIAZQBhAHQAZQBkAD0AJAB0AHIAdQBlADsAIAB0AHIAeQAgAHsAIABbAGIAeQB0AGUAWwBdAF0A"#####,
    r#####"JABhAGMAXwBiAHkAdABlAHMAPQBbAFQAZQB4AHQALgBFAG4AYwBvAGQAaQBuAGcAXQA6ADoAVQBUAEYAOA"#####,
    r#####"AuAEcAZQB0AFAAcgBlAGEAbQBiAGwAZQAoACkAKwBbAFQAZQB4AHQALgBFAG4AYwBvAGQAaQBuAGcAXQA6"#####,
    r#####"ADoAVQBUAEYAOAAuAEcAZQB0AEIAeQB0AGUAcwAoACQAYQBjAF8AaQBuAHMAdABhAGwAbABfAHMAYwByAG"#####,
    r#####"kAcAB0ACkAOwAgACQAYQBjAF8AZgBpAGwAZQAuAFcAcgBpAHQAZQAoACQAYQBjAF8AYgB5AHQAZQBzACwA"#####,
    r#####"MAAsACQAYQBjAF8AYgB5AHQAZQBzAC4ATABlAG4AZwB0AGgAKQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAew"#####,
    r#####"AgACQAYQBjAF8AZgBpAGwAZQAuAEQAaQBzAHAAbwBzAGUAKAApACAAfQA7ACAAJABnAGwAbwBiAGEAbAA6"#####,
    r#####"AEwAQQBTAFQARQBYAEkAVABDAE8ARABFAD0AJABuAHUAbABsADsAIAAmACAAKABKAG8AaQBuAC0AUABhAH"#####,
    r#####"QAaAAgACQAUABTAEgATwBNAEUAIAAnAHAAbwB3AGUAcgBzAGgAZQBsAGwALgBlAHgAZQAnACkAIAAtAE4A"#####,
    r#####"bwBQAHIAbwBmAGkAbABlACAALQBOAG8AbgBJAG4AdABlAHIAYQBjAHQAaQB2AGUAIAAtAEUAeABlAGMAdQ"#####,
    r#####"B0AGkAbwBuAFAAbwBsAGkAYwB5ACAAQgB5AHAAYQBzAHMAIAAtAEYAaQBsAGUAIAAkAGEAYwBfAHAAYQB0"#####,
    r#####"AGgAOwAgACQAYQBjAF8AZQB4AGkAdAA9ACQATABBAFMAVABFAFgASQBUAEMATwBEAEUAOwAgAGkAZgAgAC"#####,
    r#####"gAJABuAHUAbABsACAALQBlAHEAIAAkAGEAYwBfAGUAeABpAHQAKQAgAHsAIAAkAGEAYwBfAGUAeABpAHQA"#####,
    r#####"PQAxACAAfQAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8Acg"#####,
    r#####"AuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAZgBp"#####,
    r#####"AG4AYQBsAGwAeQAgAHsAIABpAGYAIAAoACQAYQBjAF8AYwByAGUAYQB0AGUAZAApACAAewAgAHQAcgB5AC"#####,
    r#####"AAewAgAFIAZQBtAG8AdgBlAC0ASQB0AGUAbQAgAC0ATABpAHQAZQByAGEAbABQAGEAdABoACAAJABhAGMA"#####,
    r#####"XwBwAGEAdABoACAALQBGAG8AcgBjAGUAIAAtAEUAcgByAG8AcgBBAGMAdABpAG8AbgAgAFMAdABvAHAAIA"#####,
    r#####"B9ACAAYwBhAHQAYwBoACAAewAgAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0"#####,
    r#####"AGUATABpAG4AZQAoACQAXwApADsAIABpAGYAIAAoACQAYQBjAF8AZQB4AGkAdAAgAC0AZQBxACAAMAApAC"#####,
    r#####"AAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAH0AIAB9ADsAIABlAHgAaQB0ACAAJABhAGMA"#####,
    r#####"XwBlAHgAaQB0AA==","#####,
    r#####""macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://antigravity.google/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf "#####,
    r#####"'%s\\n' \"$ac_install_script\" | bash","#####,
    r#####""linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://antigravity.google/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf "#####,
    r#####"'%s\\n' \"$ac_install_script\" | bash","#####,
    r#####""default":"echo No verified installer for this platform 1>&2 && exit 1"},"#####,
    r#####""grok":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypa"#####,
    r#####"ss -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGE"#####,
    r#####"AdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJ"#####,
    r#####"ABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQB"#####,
    r#####"lAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AeAAuAGEAaQAvAGMAbABpAC8AaQBuA"#####,
    r#####"HMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHI"#####,
    r#####"AbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAc"#####,
    r#####"wBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwB"#####,
    r#####"jAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlA"#####,
    r#####"HIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGE"#####,
    r#####"AdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAK"#####,
    r#####"AAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwB"#####,
    r#####"HAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkA"#####,
    r#####"GEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQ"#####,
    r#####"AaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8AL"#####,
    r#####"gBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQB"#####,
    r#####"yAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgA"#####,
    r#####"HQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEU"#####,
    r#####"AbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAV"#####,
    r#####"ABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJAB"#####,
    r#####"hAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByA"#####,
    r#####"GkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQ"#####,
    r#####"AaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZ"#####,
    r#####"QAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQB"#####,
    r#####"sAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByA"#####,
    r#####"HMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGU"#####,
    r#####"AcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAc"#####,
    r#####"wAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwB"#####,
    r#####"UAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4A"#####,
    r#####"GkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEM"#####,
    r#####"AbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAJ"#####,
    r#####"ABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAgACgAJABhAGMAXwB"#####,
    r#####"jAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AGUALQBJAHQAZQBtACAALQBMA"#####,
    r#####"GkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAEYAbwByAGMAZQAgAC0ARQByAHI"#####,
    r#####"AbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8Ab"#####,
    r#####"ABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJAB"#####,
    r#####"hAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9A"#####,
    r#####"CAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","#####,
    r#####""macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_"#####,
    r#####"install_script\" | bash","#####,
    r#####""linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https"#####,
    r#####"://x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_"#####,
    r#####"install_script\" | bash","#####,
    r#####""default":"echo No verified installer for this platform 1>&2 && exit 1"}}"#####,
);
const SCOPE_B_NON_INSTALL_MAIN_2800: &str = concat!(
    r#####"{"schemaVersion":1,"agents":[{"key":"claude","label":"Claude Code","#####,
    r#####""description":"Coding Agent by Anthropic","color":"#d97706","#####,
    r#####""baseCommand":"claude","instructionsFilename":"CLAUDE.md","envs":[],"#####,
    r#####""isolatedHome":false,"configSeed":{"enabled":true,"dest":".claude"},"#####,
    r#####""removable":true,"updateCommands":["claude --update"],"#####,
    r#####""idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},"#####,
    r#####"{"key":"codex","label":"Codex","description":"Coding Agent by OpenAI","#####,
    r#####""color":"#10b981","baseCommand":"codex","instructionsFilename":"AGENTS.md","#####,
    r#####""envs":[],"isolatedHome":false,"configSeed":{"enabled":true,"dest":".codex"},"#####,
    r#####""removable":true,"updateCommands":["codex update"],"idleBurst":{"maxBytes":1024,"#####,
    r#####""maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"hermes","label":"Hermes","#####,
    r#####""description":"Coding Agent by Nous Research","color":"#8b5cf6","#####,
    r#####""baseCommand":"hermes","instructionsFilename":"AGENTS.md","envs":[],"#####,
    r#####""isolatedHome":false,"removable":true,"updateCommands":["hermes update --yes"],"#####,
    r#####""idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},"#####,
    r#####"{"key":"cursor","label":"Cursor CLI","description":"Coding Agent by Cursor","#####,
    r#####""color":"#22d3ee","baseCommand":"agent","instructionsFilename":"AGENTS.md","#####,
    r#####""envs":[],"isolatedHome":false,"removable":true,"idleBurst":{"maxBytes":1024,"#####,
    r#####""maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"pi","label":"Pi","#####,
    r#####""description":"Coding Agent by Earendil Inc","color":"#ec4899","baseCommand":"pi","#####,
    r#####""instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"#####,
    r#####""removable":true,"updateCommands":["pi update"],"idleBurst":{"maxBytes":1024,"#####,
    r#####""maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"opencode","label":"OpenCode","#####,
    r#####""description":"Open-source terminal coding agent by Anomaly","color":"#64748b","#####,
    r#####""baseCommand":"opencode","instructionsFilename":"AGENTS.md","envs":[],"#####,
    r#####""isolatedHome":false,"configSeed":{"enabled":true,"dest":".opencode"},"#####,
    r#####""removable":true,"updateCommands":["opencode upgrade"],"#####,
    r#####""idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},"#####,
    r#####"{"key":"antigravity","label":"Antigravity","description":"Coding Agent by Google","#####,
    r#####""color":"#4285F4","baseCommand":"agy","instructionsFilename":"AGENTS.md","#####,
    r#####""envs":[],"isolatedHome":false,"removable":true,"updateCommands":["agy update"],"#####,
    r#####""idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},"#####,
    r#####"{"key":"grok","label":"Grok Build","description":"Coding Agent by SpaceXAI","#####,
    r#####""color":"#64748b","baseCommand":"grok","instructionsFilename":"AGENTS.md","#####,
    r#####""envs":[],"isolatedHome":false,"removable":true,"idleBurst":{"maxBytes":1024,"#####,
    r#####""maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"muse","label":"Muse Code","#####,
    r#####""description":"Meta terminal coding agent (beta; macOS/Linux host only)","#####,
    r#####""color":"#0668E1","baseCommand":"muse","envs":[],"isolatedHome":false,"#####,
    r#####""removable":true,"updateCommands":[],"autoUpdate":false,"#####,
    r#####""idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}}]}"#####,
);

fn scope_b_expected_commands_2800(key: &str) -> Option<serde_json::Value> {
    let expected: serde_json::Value = serde_json::from_str(SCOPE_B_EXPECTED_2800).unwrap();
    expected.get(key).cloned()
}
