//! Tests for Agent Matrix/replica layout invariants, the preflight-rename
//! `delete_workgroup` helper added in the #113 follow-up dispatch, plus
//! the #107 helper `parse_task_title`.

use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering as TestOrdering};

mod agent_delete;
mod agent_delete_blockers;
mod copy_cleanup;
mod matrix_copy;
mod matrix_layout;
mod replica_config;
mod role;
mod room_lifecycle;
mod room_number;
mod stage_e_cross_process;
mod task_snapshot_root_tests;
mod task_snapshot_tests;
mod task_title;
mod team_config;
mod team_copy;
mod workgroup_delete;

fn issue_3068_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("source-project");
    let source = project.join(".ac/_agent_alpha");
    let destination = tmp.path().join("destination/.ac");
    for directory in [
        "memory",
        "memory-archive/old",
        "plans",
        "skills",
        "inbox",
        "outbox",
        "empty",
        "unknown/.hidden",
        "default-tools/.hidden",
    ] {
        std::fs::create_dir_all(source.join(directory)).unwrap();
    }
    std::fs::create_dir_all(&destination).unwrap();
    for (name, bytes) in [
        ("Role.md", b"arbitrary role\0\xff".as_slice()),
        ("config.json", b"{\"custom\":true}".as_slice()),
        ("memory-archive/old/blob", &[0, 255, 1, 128]),
        (".hidden", b"hidden".as_slice()),
        ("unknown/.hidden/data", b"unknown".as_slice()),
        ("default-tools/.hidden/data", b"unknown".as_slice()),
        ("inbox/message", b"inbox".as_slice()),
        ("outbox/message", b"outbox".as_slice()),
    ] {
        std::fs::write(source.join(name), bytes).unwrap();
    }
    (tmp, project, source, destination)
}

fn issue_3068_team_config(agents: &[&str]) -> TeamConfigResult {
    TeamConfigResult {
        agents: agents.iter().map(|agent| (*agent).to_string()).collect(),
        coordinator: agents.first().copied().unwrap_or_default().to_string(),
        repos: vec![RepoAssignment {
            url: "https://example.test/repo.git".into(),
            agents: agents.iter().map(|agent| (*agent).to_string()).collect(),
        }],
        context_alert_percentages: vec![80, 20],
    }
}

fn issue_3068_sources(project: &Path, agents: &[&str]) -> Vec<TeamAgentSource> {
    agents
        .iter()
        .map(|agent| TeamAgentSource {
            agent_ref: (*agent).to_string(),
            source_project_path: project.to_string_lossy().into_owned(),
        })
        .collect()
}

fn issue_3068_create(
    destination: &Path,
    config: TeamConfigResult,
    sources: Option<Vec<TeamAgentSource>>,
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<PathBuf, String> {
    create_team_from_sources(
        destination.parent().unwrap(),
        "copied",
        config,
        sources,
        hooks,
    )
}

// Independent full-tree proof: never use the production selection scanner.
fn issue_3068_inventory(source: &Path) -> BTreeMap<PathBuf, MatrixCopyStamp> {
    fn walk(root: &Path, relative: &Path, inventory: &mut BTreeMap<PathBuf, MatrixCopyStamp>) {
        let path = root.join(relative);
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        let digest = if metadata.is_file() {
            Some(OwnedCopyArtifacts::digest_bytes(
                &std::fs::read(&path).unwrap(),
            ))
        } else {
            None
        };
        inventory.insert(
            relative.to_path_buf(),
            OwnedCopyArtifacts::stamp(&metadata, digest).unwrap(),
        );
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                walk(root, &relative.join(entry.unwrap().file_name()), inventory);
            }
        }
    }
    let mut inventory = BTreeMap::new();
    walk(source, Path::new(""), &mut inventory);
    inventory
}

fn issue_3068_history(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(COPY_PROVENANCE_FILE)).unwrap()).unwrap()
}

// Test-only synchronous wrapper: acquires the mutation guard around
// write_team_config_guarded.
pub(crate) fn write_team_config(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
) -> Result<PathBuf, String> {
    let guard = TeamConfigMutationGuard::acquire(ac_root)?;
    write_team_config_guarded(ac_root, team_name, config, &guard)
}

struct ReplicaCreationFixture {
    _temp: tempfile::TempDir,
    ac_root: PathBuf,
    wg_dir: PathBuf,
    matrix_dir: PathBuf,
}

fn replica_creation_fixture() -> ReplicaCreationFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("AgentsCommander_ac");
    let ac_root = project.join(".ac");
    let matrix_dir = ac_root.join("_agent_tech-lead");
    let wg_dir = ac_root.join("wg-2-dev-team");
    std::fs::create_dir_all(&matrix_dir).expect("create matrix");
    std::fs::create_dir_all(&wg_dir).expect("create wg");
    std::fs::write(matrix_dir.join(ROLE_MD_FILENAME), "# Tech Lead\n").expect("write role");
    ReplicaCreationFixture {
        _temp: temp,
        ac_root,
        wg_dir,
        matrix_dir,
    }
}

fn write_matrix_default(fixture: &ReplicaCreationFixture, value: serde_json::Value) {
    std::fs::write(
        fixture.matrix_dir.join("config.json"),
        serde_json::to_string(&value).expect("serialize"),
    )
    .expect("write matrix config");
}

fn create_replica(fixture: &ReplicaCreationFixture) -> Result<PathBuf, String> {
    create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
        ac_root: fixture.ac_root.clone(),
        wg_dir: fixture.wg_dir.clone(),
        agent_path: "_agent_tech-lead".to_string(),
        team_repos: Vec::new(),
    })
}

fn replica_config(replica_dir: &Path) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(replica_dir.join("config.json")).expect("read config"),
    )
    .expect("parse config")
}

/// #2816 (C3) - the state file beside the replica's `config.json`.
fn replica_state(replica_dir: &Path) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            replica_dir.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
        )
        .expect("read state file"),
    )
    .expect("parse state file")
}

fn create_test_ac_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("Project").join(".ac")).expect("create .ac");
    tmp
}

fn test_base(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("Project").join(".ac")
}

fn create_test_agent(base: &Path, name: &str, role_display: &str) -> PathBuf {
    let agent_dir = base.join(format!("_agent_{}", name));
    std::fs::create_dir_all(&agent_dir).expect("create agent");
    std::fs::write(
        agent_dir.join("Role.md"),
        format!("---\nname: '{}'\n---\n", role_display),
    )
    .expect("write role");
    agent_dir
}

fn write_team_value(base: &Path, team_name: &str, value: serde_json::Value) -> (PathBuf, Vec<u8>) {
    let team_dir = base.join(format!("_team_{}", team_name));
    std::fs::create_dir_all(&team_dir).expect("create team");
    let config_path = team_dir.join("config.json");
    let mut bytes = serde_json::to_vec_pretty(&value).expect("serialize team");
    bytes.push(b'\n');
    std::fs::write(&config_path, &bytes).expect("write team config");
    (config_path, bytes)
}

fn settings_state(settings: AppSettings) -> SettingsState {
    crate::config::settings::new_settings_state(settings)
}

fn profile_cell(command: &str) -> crate::config::settings::ProfileCellConfig {
    crate::config::settings::ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        enabled: true,
        command: command.to_string(),
        env: BTreeMap::new(),
        notes: "keep".to_string(),
    }
}

fn test_manager() -> Arc<tokio::sync::RwLock<SessionManager>> {
    Arc::new(tokio::sync::RwLock::new(SessionManager::new()))
}

fn empty_team_config(context_alert_percentages: Vec<u8>) -> TeamConfigResult {
    TeamConfigResult {
        agents: Vec::new(),
        coordinator: String::new(),
        repos: Vec::new(),
        context_alert_percentages,
    }
}

async fn add_live_session(manager: &Arc<tokio::sync::RwLock<SessionManager>>, working_dir: &Path) {
    let guard = manager.read().await;
    guard
        .create_session(
            "shell".to_string(),
            Vec::new(),
            working_dir.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create live session");
}

async fn run_agent_delete_for_test(
    base: &Path,
    agent_path: &Path,
    settings: &SettingsState,
    manager: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<PreparedAgentDeleteMetadata, String> {
    run_agent_delete_for_test_with(
        base,
        agent_path,
        settings,
        manager,
        None,
        |_| {},
        |candidate: &AppSettings| Ok(candidate.clone()),
    )
    .await
}

/// Drive the #1063 blocking Agent Matrix delete transaction from an async test:
/// async preflight/prepare, then the synchronous transaction on a blocking
/// thread with an injectable activation token, lock-order barrier, and settings
/// saver.
async fn run_agent_delete_for_test_with(
    base: &Path,
    agent_path: &Path,
    settings: &SettingsState,
    manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    activation: Option<ManifestActivationToken>,
    after_project_before_team: impl FnOnce(&Path) + Send + 'static,
    save_settings_fn: impl Fn(&AppSettings) -> Result<AppSettings, String> + Send + 'static,
) -> Result<PreparedAgentDeleteMetadata, String> {
    let plan = {
        let prelim_guard = TeamConfigMutationGuard::acquire(base)?;
        let plan = collect_agent_delete_plan_guarded(base, agent_path, &prelim_guard)?;
        drop(prelim_guard);
        plan
    };
    preflight_agent_delete(&plan, manager).await?;
    let metadata = prepare_agent_delete_metadata(&plan, settings).await?;

    let base_owned = base.to_path_buf();
    let project_root = base.parent().unwrap_or(base).to_path_buf();
    let plan_owned = plan.clone();
    let metadata_owned = metadata.clone();
    let settings_owned = settings.clone();
    let manager_owned = manager.clone();
    let agent_name = plan.agent_name.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        run_agent_delete_transaction(
            &base_owned,
            &project_root,
            &plan_owned,
            &metadata_owned,
            &settings_owned,
            &manager_owned,
            &agent_name,
            activation.as_ref(),
            after_project_before_team,
            save_settings_fn,
        )
    })
    .await
    .map_err(|e| format!("blocking join error: {e}"))?;

    match outcome {
        AgentDeleteTransactionOutcome::Committed { cleanup, .. } => {
            cleanup?;
            Ok(metadata)
        }
        AgentDeleteTransactionOutcome::Blocked { raw_error, .. } => {
            Err(format!("BLOCKERS:{}", raw_error))
        }
        AgentDeleteTransactionOutcome::Aborted { error } => Err(error),
    }
}

/// Helper: create an empty directory at `<root>/<name>` for the test.
fn touch_dir(root: &Path, name: &str) {
    std::fs::create_dir(root.join(name)).unwrap_or_else(|e| panic!("create_dir {}: {}", name, e));
}

/// C4 (#2470): the seeded state file of every row that seeds one. All four
/// state keys, plus the exact D7 marker `stamp_split_marker` writes.
#[rustfmt::skip]
const C4_SEEDED_STATE: &str =
    r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;

/// C4 (#2470): the state file beside a tracked `config.json`.
fn c4_state_path(tracked: &std::path::Path) -> std::path::PathBuf {
    tracked.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

/// C4 (#2470): after one writer, the tracked file carries none of the four
/// state keys. This says nothing at all about the state file: `c4_state_is`
/// is the only helper that reads it, and the row calls them in that order.
fn c4_tracked_is_clean(tracked: &std::path::Path, site: &str) {
    let text = std::fs::read_to_string(tracked).expect("tracked file");
    let value: serde_json::Value = serde_json::from_str(&text).expect("tracked json");
    let tooling = value.get("tooling");
    c4_absent(tooling, "lastCodingAgent", site, &value);
    c4_absent(tooling, "codingAgents", site, &value);
    c4_absent(tooling, "lastAgentMessageAt", site, &value);
    c4_absent(tooling, "profileContentHash", site, &value);
}

fn c4_absent(t: Option<&serde_json::Value>, key: &str, site: &str, all: &serde_json::Value) {
    assert!(
        t.and_then(|t| t.get(key)).is_none(),
        "{site} left {key}: {all}"
    );
}

/// C4 (#2470): the state file EXISTS and is exactly `expected`, compared
/// as parsed JSON so the pretty-printing of `publish_pair_side` is never
/// the subject. `expected` is always a literal written in the row. Absence
/// is NOT admitted here and no longer reads as JSON `null`:
/// `c4_no_state_file` is the only helper that may conclude absence.
fn c4_state_is(state: &std::path::Path, expected: &str, site: &str) {
    let want: serde_json::Value = serde_json::from_str(expected).expect("expected json");
    let raw = std::fs::read_to_string(state).expect("state file");
    let got: serde_json::Value = serde_json::from_str(&raw).expect("state json");
    assert_eq!(got, want, "{site} state file");
}
