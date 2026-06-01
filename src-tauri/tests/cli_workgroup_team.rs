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

#[derive(Clone, Copy)]
enum WorkspaceLayout {
    Canonical,
    Legacy,
    Both,
}

fn active_workspace(project: &Path, layout: WorkspaceLayout) -> PathBuf {
    match layout {
        WorkspaceLayout::Canonical | WorkspaceLayout::Both => project.join(".ac"),
        WorkspaceLayout::Legacy => project.join(".ac-new"),
    }
}

fn project_with_agents_layout(
    tmp: &Path,
    agents: &[&str],
    layout: WorkspaceLayout,
) -> (PathBuf, PathBuf) {
    let project = tmp.join("ProjectAlpha");
    if matches!(layout, WorkspaceLayout::Canonical | WorkspaceLayout::Both) {
        std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
    }
    if matches!(layout, WorkspaceLayout::Legacy | WorkspaceLayout::Both) {
        std::fs::create_dir_all(project.join(".ac-new")).expect("create .ac-new");
    }

    let workspace_dir = active_workspace(&project, layout);
    for agent in agents {
        let dir = workspace_dir.join(format!("_agent_{}", agent));
        std::fs::create_dir_all(dir.join("memory")).expect("agent memory");
        std::fs::write(dir.join("Role.md"), format!("# {}\n", agent)).expect("role");
    }

    (project, workspace_dir)
}

fn project_with_agents(tmp: &Path, agents: &[&str]) -> PathBuf {
    let (project, _) = project_with_agents_layout(tmp, agents, WorkspaceLayout::Canonical);
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

#[test]
fn workgroup_add_creates_task_messaging_replicas_and_lists() {
    let tmp = Tmp::new("cli-workgroup-add");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

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
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
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
    let team_config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let team_config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(team_config_path).expect("config"))
            .expect("team config json");
    assert_eq!(
        team_config["agents"],
        serde_json::json!(["_agent_dev-rust", "_agent_architect"])
    );
    assert_eq!(team_config["coordinator"], "_agent_architect");

    let list = run_json(&bin, &["workgroup", "list", "--project", "ProjectAlpha"]);
    assert_eq!(list.as_array().expect("array").len(), 1);
    assert_eq!(list[0]["name"], "wg-1-dev-team");
    assert_eq!(list[0]["team"], "dev-team");
    assert_eq!(list[0]["hasTask"], true);
    assert_eq!(list[0]["hasMessaging"], true);
}

#[test]
fn workgroup_add_uses_legacy_workspace_when_canonical_absent() {
    let tmp = Tmp::new("cli-workgroup-add-legacy");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let (_project, workspace) = project_with_agents_layout(
        tmp.path(),
        &["architect", "dev-rust"],
        WorkspaceLayout::Legacy,
    );

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
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
        ],
    );

    let wg_dir = workspace.join("wg-1-dev-team");
    assert_eq!(json["path"], wg_dir.to_string_lossy().as_ref());
    assert!(wg_dir.join("TASK.md").is_file());

    let list = run_json(&bin, &["workgroup", "list", "--project", "ProjectAlpha"]);
    assert_eq!(list.as_array().expect("array").len(), 1);
    assert_eq!(list[0]["path"], wg_dir.to_string_lossy().as_ref());
}

#[test]
fn workgroup_add_and_list_prefer_ac_when_both_workspaces_exist() {
    let tmp = Tmp::new("cli-workgroup-both-add");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let (project, workspace) =
        project_with_agents_layout(tmp.path(), &["architect"], WorkspaceLayout::Both);
    let legacy = project.join(".ac-new");
    std::fs::create_dir_all(legacy.join("wg-1-stale-team")).expect("stale wg");

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
            "Build",
            "--coordinator",
            "architect",
        ],
    );

    let wg_dir = workspace.join("wg-1-dev-team");
    assert_eq!(json["path"], wg_dir.to_string_lossy().as_ref());
    assert!(wg_dir.is_dir());
    assert!(!legacy.join("wg-1-dev-team").exists());

    let list = run_json(&bin, &["workgroup", "list", "--project", "ProjectAlpha"]);
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1, "legacy stale workgroup must be ignored");
    assert_eq!(items[0]["name"], "wg-1-dev-team");
    assert_eq!(items[0]["path"], wg_dir.to_string_lossy().as_ref());
}

#[test]
fn workgroup_add_uses_global_lowest_free_number() {
    let tmp = Tmp::new("cli-workgroup-numbering");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);
    std::fs::create_dir_all(project.join(".ac").join("wg-1-dev-team")).expect("wg1");

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
            "--coordinator",
            "architect",
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
            "--coordinator",
            "architect",
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
}

#[test]
fn team_list_prefers_ac_over_stale_legacy_config() {
    let tmp = Tmp::new("cli-team-both-list");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let (project, _workspace) =
        project_with_agents_layout(tmp.path(), &["architect"], WorkspaceLayout::Both);

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
            "--coordinator",
            "architect",
        ],
    );

    let legacy_team_dir = project.join(".ac-new").join("_team_dev-team");
    std::fs::create_dir_all(&legacy_team_dir).expect("legacy team dir");
    std::fs::write(
        legacy_team_dir.join("config.json"),
        r#"{"agents":["_agent_stale"],"coordinator":"_agent_stale","repos":[]}"#,
    )
    .expect("legacy config");

    let teams = run_json(&bin, &["team", "list", "--project", "ProjectAlpha"]);
    assert_eq!(teams.as_array().expect("teams").len(), 1);
    assert_eq!(teams[0]["team"], "dev-team");
    assert_eq!(teams[0]["coordinator"], "_agent_architect");
    assert_ne!(teams[0]["coordinator"], "_agent_stale");
}
