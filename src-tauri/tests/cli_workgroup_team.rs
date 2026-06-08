use std::path::{Path, PathBuf};
use std::process::Command;

struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Tmp {
    fn new(prefix: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("ac-{}-{}", prefix, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("create tmp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    std::fs::copy(src, &dst).expect("copy binary");
    dst
}

fn config_dir_for_bin(bin: &Path) -> PathBuf {
    let stem = bin
        .file_stem()
        .expect("bin stem")
        .to_string_lossy()
        .to_string();
    bin.parent().expect("bin parent").join(format!(".{}", stem))
}

fn write_settings(config_dir: &Path, project_parent: &Path) {
    std::fs::create_dir_all(config_dir).expect("create config dir");
    let settings = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "projectPaths": [project_parent.to_string_lossy().to_string()]
    });
    std::fs::write(
        config_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings).expect("settings json"),
    )
    .expect("write settings");
}

fn project_refresh_request_paths(config_dir: &Path) -> Vec<PathBuf> {
    let requests_dir = config_dir.join("project-refresh-requests");
    if !requests_dir.exists() {
        return Vec::new();
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&requests_dir)
        .expect("read project-refresh-requests")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    paths.sort();
    paths
}

fn read_project_refresh_requests(config_dir: &Path) -> Vec<serde_json::Value> {
    project_refresh_request_paths(config_dir)
        .iter()
        .map(|path| {
            serde_json::from_str(&std::fs::read_to_string(path).expect("read request"))
                .expect("request json")
        })
        .collect()
}

fn assert_refresh_request(
    request: &serde_json::Value,
    project: &Path,
    changed_path_suffix: &str,
    changed_name: &str,
    reason: &str,
) {
    assert_eq!(
        request["projectPath"].as_str().expect("projectPath"),
        std::fs::canonicalize(project)
            .expect("canonical project")
            .to_string_lossy()
            .as_ref()
    );
    assert!(
        request["changedPath"]
            .as_str()
            .expect("changedPath")
            .replace('\\', "/")
            .ends_with(changed_path_suffix),
        "changedPath {:?} should end with {}",
        request["changedPath"],
        changed_path_suffix
    );
    assert_eq!(request["changedName"], changed_name);
    assert_eq!(request["reason"], reason);
}

fn find_refresh_request<'a>(
    requests: &'a [serde_json::Value],
    reason: &str,
) -> &'a serde_json::Value {
    requests
        .iter()
        .find(|request| request["reason"] == reason)
        .unwrap_or_else(|| panic!("missing refresh request reason {}", reason))
}

fn project_with_agents(tmp: &Path, agents: &[&str]) -> PathBuf {
    let project = tmp.join("ProjectAlpha");
    let workspace_dir = project.join(".ac");
    std::fs::create_dir_all(&workspace_dir).expect("create .ac");
    for agent in agents {
        let dir = workspace_dir.join(format!("_agent_{}", agent));
        std::fs::create_dir_all(dir.join("memory")).expect("agent memory");
        std::fs::write(dir.join("Role.md"), format!("# {}\n", agent)).expect("role");
    }
    project
}

fn run_json(bin: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout json")
}

fn run_json_machine(bin: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(bin)
        .env("AC_MACHINE_OUTPUT", "1")
        .args(args)
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout json")
}

fn run_fail(bin: &Path, args: &[&str]) -> String {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        !out.status.success(),
        "expected failure\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn run_stdout(bin: &Path, args: &[&str]) -> String {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn team_help_describes_existing_agents_and_workgroup_scoped_membership() {
    let tmp = Tmp::new("cli-team-help");
    let bin = copy_binary_into(tmp.path());

    let team_help = run_stdout(&bin, &["team", "--help"]);
    assert!(team_help.contains("Manage teams and scoped workgroup membership"));

    let create_help = run_stdout(&bin, &["team", "create", "--help"]);
    assert!(create_help.contains("Create a team configuration from existing agents"));
    assert!(create_help.contains("Existing agent matrix name or _agent_<name> reference"));
    assert!(create_help.contains("Define a repo available to the team when workgroups are created"));
}

#[test]
fn workgroup_add_help_hides_team_definition_flags() {
    let tmp = Tmp::new("cli-workgroup-help");
    let bin = copy_binary_into(tmp.path());

    let help = run_stdout(&bin, &["workgroup", "add", "--help"]);
    assert!(help.contains("--project"));
    assert!(help.contains("--team"));
    assert!(help.contains("--title"));
    assert!(!help.contains("--coordinator"));
    assert!(!help.contains("--agent"));
    assert!(!help.contains("--repo"));
    assert!(!help.to_ascii_lowercase().contains("deprecated"));
}

#[test]
fn workgroup_add_creates_task_messaging_replicas_and_lists() {
    let tmp = Tmp::new("cli-workgroup-add");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let created_team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
        ],
    );
    assert_eq!(created_team["team"], "dev-team");
    assert_eq!(
        created_team["agents"],
        serde_json::json!(["_agent_architect", "_agent_dev-rust"])
    );

    let team_config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let team_config_before = std::fs::read(&team_config_path).expect("team config before");

    let json = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build the thing",
        ],
    );

    let wg_dir = project.join(".ac").join("wg-1-dev-team");
    assert_eq!(json["path"], wg_dir.to_string_lossy().as_ref());
    assert!(wg_dir.join("TASK.md").is_file());
    assert!(wg_dir.join("messaging").is_dir());
    assert!(wg_dir
        .join("__agent_architect")
        .join("config.json")
        .is_file());
    assert!(wg_dir
        .join("__agent_dev-rust")
        .join("config.json")
        .is_file());
    assert_eq!(
        std::fs::read(&team_config_path).expect("team config after"),
        team_config_before
    );
    let team_config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(team_config_path).expect("config"))
            .expect("team config json");
    assert_eq!(
        team_config["agents"],
        serde_json::json!(["_agent_architect", "_agent_dev-rust"])
    );
    assert_eq!(team_config["coordinator"], "_agent_architect");

    let list = run_json(&bin, &["workgroup", "list", "--project", "ProjectAlpha"]);
    assert_eq!(list.as_array().expect("array").len(), 1);
    assert_eq!(list[0]["name"], "wg-1-dev-team");
    assert_eq!(list[0]["team"], "dev-team");
    assert_eq!(list[0]["hasTask"], true);
    assert_eq!(list[0]["hasMessaging"], true);

    let requests = read_project_refresh_requests(&config_dir);
    assert_eq!(requests.len(), 2);
    assert_refresh_request(
        find_refresh_request(&requests, "teamCreated"),
        &project,
        ".ac/_team_dev-team",
        "dev-team",
        "teamCreated",
    );
    assert_refresh_request(
        find_refresh_request(&requests, "workgroupCreated"),
        &project,
        ".ac/wg-1-dev-team",
        "wg-1-dev-team",
        "workgroupCreated",
    );
}

#[test]
fn workgroup_add_uses_global_lowest_free_number() {
    let tmp = Tmp::new("cli-workgroup-numbering");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);
    std::fs::create_dir_all(project.join(".ac").join("wg-1-dev-team")).expect("wg1");

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "QA Team",
            "--coordinator",
            "architect",
        ],
    );
    let json = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "QA Team",
            "--title",
            "Test it",
        ],
    );

    assert!(json["path"]
        .as_str()
        .expect("path")
        .ends_with("wg-2-qa-team"));
}

#[test]
fn workgroup_remove_deletes_and_reuses_number() {
    let tmp = Tmp::new("cli-workgroup-remove");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);

    let created = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
            "--coordinator",
            "architect",
        ],
    );
    let wg_dir = project.join(".ac").join("wg-1-dev-team");
    assert_eq!(created["path"], wg_dir.to_string_lossy().as_ref());
    assert!(wg_dir.is_dir());

    let removed = run_json_machine(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
        ],
    );
    assert_eq!(removed["removed"], true);
    assert!(!wg_dir.exists());
    let requests = read_project_refresh_requests(&config_dir);
    assert_eq!(requests.len(), 2);
    assert_refresh_request(
        find_refresh_request(&requests, "workgroupRemoved"),
        &project,
        ".ac/wg-1-dev-team",
        "wg-1-dev-team",
        "workgroupRemoved",
    );

    let recreated = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "QA Team",
            "--title",
            "Test it",
            "--coordinator",
            "architect",
        ],
    );
    assert!(recreated["path"]
        .as_str()
        .expect("path")
        .ends_with("wg-1-qa-team"));
}

#[test]
fn workgroup_add_normalizes_include_and_exclude_repo_assignments() {
    let tmp = Tmp::new("cli-workgroup-repos");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust", "qa"]);

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
            "--agent",
            "qa",
            "--repo",
            "https://example.test/all.git",
            "--repo-agents",
            "https://example.test/include.git=architect,dev-rust",
            "--repo-exclude-agents",
            "https://example.test/exclude.git=qa",
        ],
    );
    let _json = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
        ],
    );

    let config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).expect("config"))
            .expect("config json");
    let repos = config["repos"].as_array().expect("repos");
    assert_eq!(repos.len(), 3);
    assert_eq!(repos[0]["agents"].as_array().expect("agents").len(), 3);
    assert_eq!(
        repos[1]["agents"],
        serde_json::json!(["_agent_architect", "_agent_dev-rust"])
    );
    let exclude_agents = repos[2]["agents"].as_array().expect("exclude agents");
    assert_eq!(exclude_agents.len(), 2);
    assert!(exclude_agents
        .iter()
        .any(|agent| agent == "_agent_architect"));
    assert!(exclude_agents
        .iter()
        .any(|agent| agent == "_agent_dev-rust"));
}

#[test]
fn team_add_member_creates_replica_and_peer_is_reachable() {
    let tmp = Tmp::new("cli-team-add-member");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
        ],
    );
    let _wg = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
        ],
    );

    let added = run_json(
        &bin,
        &[
            "team",
            "add-member",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
            "--agent",
            "dev-rust",
        ],
    );
    let replica = project
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_dev-rust");
    assert_eq!(added["added"], true);
    assert!(replica.join("config.json").is_file());

    let sender_root = project
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_architect");
    let out = Command::new(&bin)
        .args([
            "list-peers-lean",
            "--root",
            &sender_root.to_string_lossy(),
            "--token",
            "00000000-0000-0000-0000-000000000000",
        ])
        .output()
        .expect("list peers");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let peers: serde_json::Value = serde_json::from_slice(&out.stdout).expect("peers");
    let found = peers.as_array().expect("array").iter().any(|peer| {
        peer["name"] == "ProjectAlpha:wg-1-dev-team/dev-rust" && peer["reachable"] == true
    });
    assert!(found, "added peer should be reachable: {}", peers);
    let requests = read_project_refresh_requests(&config_dir);
    assert_eq!(requests.len(), 3);
    assert_refresh_request(
        find_refresh_request(&requests, "teamMembershipChanged"),
        &project,
        ".ac/wg-1-dev-team/__agent_dev-rust",
        "wg-1-dev-team/dev-rust",
        "teamMembershipChanged",
    );

    let removed = run_json(
        &bin,
        &[
            "team",
            "remove-member",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
            "--agent",
            "dev-rust",
        ],
    );
    assert_eq!(removed["removed"], true);
    assert!(!replica.exists());
    let requests = read_project_refresh_requests(&config_dir);
    assert_eq!(requests.len(), 4);
    assert_refresh_request(
        find_refresh_request(&requests, "teamMembershipRemoved"),
        &project,
        ".ac/wg-1-dev-team/__agent_dev-rust",
        "wg-1-dev-team/dev-rust",
        "teamMembershipRemoved",
    );
}

#[test]
fn team_create_refuses_existing_team() {
    let tmp = Tmp::new("cli-team-create-existing");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let _project = project_with_agents(tmp.path(), &["architect"]);

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
        ],
    );
    let stderr = run_fail(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
        ],
    );
    assert!(stderr.contains("Team 'dev-team' already exists"));
}

#[test]
fn team_create_refuses_preexisting_empty_team_dir() {
    let tmp = Tmp::new("cli-team-create-empty-dir");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);
    std::fs::create_dir_all(project.join(".ac").join("_team_dev-team")).expect("team dir");

    let stderr = run_fail(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
        ],
    );
    assert!(stderr.contains("Team 'dev-team' already exists"));
}

#[test]
fn team_create_outputs_coordinator_first_roster() {
    let tmp = Tmp::new("cli-team-create-roster");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust", "qa"]);

    let json = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
            "--agent",
            "architect",
            "--agent",
            "qa",
        ],
    );
    let expected = serde_json::json!(["_agent_architect", "_agent_dev-rust", "_agent_qa"]);
    assert_eq!(json["agents"], expected);

    let config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).expect("config"))
            .expect("config json");
    assert_eq!(config["agents"], expected);
}

#[test]
fn workgroup_add_existing_team_refuses_legacy_flags_without_rewrite() {
    let tmp = Tmp::new("cli-wg-existing-refuses-legacy");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
        ],
    );
    let config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let before = std::fs::read(&config_path).expect("before");
    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Second",
            "--coordinator",
            "dev-rust",
        ],
    );
    assert!(stderr.contains("workgroup add` no longer updates team configuration"));
    assert_eq!(std::fs::read(&config_path).expect("after"), before);
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).expect("config"))
            .expect("config json");
    assert_eq!(config["coordinator"], "_agent_architect");
}

#[test]
fn workgroup_add_existing_invalid_team_config_errors_without_rewrite() {
    let tmp = Tmp::new("cli-wg-invalid-team");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);
    let team_dir = project.join(".ac").join("_team_dev-team");
    std::fs::create_dir_all(&team_dir).expect("team dir");
    let config_path = team_dir.join("config.json");
    let original = b"{ not json".to_vec();
    std::fs::write(&config_path, &original).expect("write invalid config");

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Second",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
        ],
    );
    assert!(stderr.contains("Failed to parse config.json"));
    assert_eq!(std::fs::read(&config_path).expect("after"), original);
}

#[test]
fn workgroup_add_legacy_missing_team_still_bootstraps_with_warning() {
    let tmp = Tmp::new("cli-wg-legacy-bootstrap");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let out = Command::new(&bin)
        .args([
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
        ])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("created missing team configuration"));
    assert!(!stderr.to_ascii_lowercase().contains("deprecated"));
    assert!(project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json")
        .is_file());
}

#[test]
fn workgroup_add_missing_team_without_legacy_flags_errors() {
    let tmp = Tmp::new("cli-wg-missing-team");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let _project = project_with_agents(tmp.path(), &["architect"]);

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Missing Team",
            "--title",
            "Build",
        ],
    );
    assert!(stderr.contains("Team 'missing-team' config not found"));
    assert!(stderr.contains("Create it first with `team create`"));
}
