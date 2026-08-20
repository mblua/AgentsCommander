use std::path::{Path, PathBuf};
use std::process::Command;

use agentscommander_lib::config::sessions_persistence::{
    load_sessions_raw_from_dir_for_test, PersistedSession,
};
use agentscommander_lib::session::session::SessionStatus;

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

/// Assert a CLI-emitted path string points to the same directory as `expected`.
/// #1077: `workgroup add` prints the SELECTED CANONICAL path (§3.4), so a raw
/// `tmp.path()` expectation breaks on a CI runner whose %TEMP% is an 8.3 short
/// name (`RUNNER~1` vs the canonical `runneradmin`). Canonicalizing both sides
/// compares filesystem identity, robust to 8.3 short-name and Windows
/// verbatim-prefix differences. Both paths exist at every call site (the verb
/// created them), so `canonicalize` always resolves.
fn assert_same_path(actual: &str, expected: &Path) {
    let canon_actual = std::fs::canonicalize(actual)
        .unwrap_or_else(|e| panic!("canonicalize actual path {actual:?}: {e}"));
    let canon_expected = std::fs::canonicalize(expected)
        .unwrap_or_else(|e| panic!("canonicalize expected path {expected:?}: {e}"));
    assert_eq!(canon_actual, canon_expected);
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

fn has_refresh_reason(config_dir: &Path, reason: &str) -> bool {
    read_project_refresh_requests(config_dir)
        .iter()
        .any(|request| request["reason"] == reason)
}

fn create_outside_sentinel(project: &Path, name: &str) -> PathBuf {
    let sentinel = project
        .join(format!("outside-sentinel-{name}"))
        .join("keep.txt");
    std::fs::create_dir_all(sentinel.parent().expect("sentinel parent"))
        .expect("create outside sentinel parent");
    std::fs::write(&sentinel, "keep").expect("write outside sentinel");
    sentinel
}

fn assert_outside_sentinel_survives(sentinel: &Path) {
    assert!(
        sentinel.is_file(),
        "outside sentinel should survive destructive operation: {}",
        sentinel.display()
    );
}

fn project_with_agents(tmp: &Path, agents: &[&str]) -> PathBuf {
    let project = tmp.join("ProjectAlpha");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    for agent in agents {
        let dir = ac_root.join(format!("_agent_{}", agent));
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

#[cfg(target_os = "windows")]
fn run_fail_output(bin: &Path, args: &[&str]) -> (String, String) {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        !out.status.success(),
        "expected failure\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
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

fn create_basic_workgroup(tmp: &Path, bin: &Path, config_dir: &Path) -> (PathBuf, PathBuf) {
    write_settings(config_dir, tmp);
    let project = project_with_agents(tmp, &["architect"]);
    let created = run_json(
        bin,
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
    assert_same_path(created["path"].as_str().expect("path"), &wg_dir);
    (project, wg_dir)
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn create_dirty_repo(repo_dir: &Path) {
    std::fs::create_dir_all(repo_dir).expect("create repo");
    let init = Command::new("git")
        .arg("init")
        .current_dir(repo_dir)
        .output()
        .expect("git init");
    assert!(
        init.status.success(),
        "git init failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    std::fs::write(repo_dir.join("untracked.txt"), "dirty").expect("write dirty file");
}

fn write_live_session(config_dir: &Path, working_directory: &Path) {
    let sessions = vec![PersistedSession {
        name: "Live Rust".to_string(),
        shell: "powershell.exe".to_string(),
        shell_args: Vec::new(),
        working_directory: working_directory.to_string_lossy().to_string(),
        id: Some(uuid::Uuid::new_v4().to_string()),
        status: Some(SessionStatus::Running),
        waiting_for_input: Some(false),
        created_at: Some("2026-06-13T00:00:00Z".to_string()),
        ..PersistedSession::default()
    }];
    std::fs::write(
        config_dir.join("sessions.json"),
        serde_json::to_string_pretty(&sessions).expect("sessions json"),
    )
    .expect("write sessions");
}

#[cfg(target_os = "windows")]
fn create_junction(junction: &Path, target: &Path) -> Result<(), String> {
    let output = Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            junction.to_str().expect("junction path"),
            target.to_str().expect("target path"),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

#[cfg(target_os = "windows")]
fn open_without_delete_share(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x00000001;

    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .expect("open with restricted share mode")
}

fn assert_no_side_effect_dirs(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                assert!(
                    !(name.starts_with("wg-")
                        || name.starts_with("_team_")
                        || name.starts_with("_agent_")),
                    "unexpected side-effect directory {}",
                    path.display()
                );
                stack.push(path);
            }
        }
    }
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
fn workgroup_list_missing_project_errors_without_writes() {
    let tmp = Tmp::new("cli-workgroup-list-missing-project");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let config_sentinel = config_dir.join("sentinel.txt");
    std::fs::write(&config_sentinel, "keep").expect("write config sentinel");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let stderr = run_fail(&bin, &["workgroup", "list", "--project", "MissingProject"]);

    assert!(stderr.contains("MissingProject"), "stderr was:\n{}", stderr);
    assert!(project_refresh_request_paths(&config_dir).is_empty());
    assert_eq!(
        std::fs::read_to_string(&config_sentinel).expect("read config sentinel"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_no_side_effect_dirs(tmp.path());
}

#[test]
fn team_list_missing_project_errors_without_writes() {
    let tmp = Tmp::new("cli-team-list-missing-project");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let config_sentinel = config_dir.join("sentinel.txt");
    std::fs::write(&config_sentinel, "keep").expect("write config sentinel");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let stderr = run_fail(&bin, &["team", "list", "--project", "MissingProject"]);

    assert!(stderr.contains("MissingProject"), "stderr was:\n{}", stderr);
    assert!(project_refresh_request_paths(&config_dir).is_empty());
    assert_eq!(
        std::fs::read_to_string(&config_sentinel).expect("read config sentinel"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_no_side_effect_dirs(tmp.path());
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
    assert_eq!(
        created_team["contextAlertPercentages"],
        serde_json::json!([])
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
    assert_same_path(json["path"].as_str().expect("path"), &wg_dir);
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
    assert_eq!(
        team_config["contextAlertPercentages"],
        serde_json::json!([])
    );

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
    assert_same_path(created["path"].as_str().expect("path"), &wg_dir);
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
fn workgroup_remove_refuses_dirty_repo_without_force() {
    if !git_available() {
        println!("skipping dirty repo workgroup regression because git is unavailable");
        return;
    }

    let tmp = Tmp::new("cli-workgroup-dirty-refuse");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "dirty-refuse");
    let dirty_file = wg_dir.join("repo-dirty").join("untracked.txt");
    create_dirty_repo(dirty_file.parent().expect("dirty repo parent"));

    let stderr = run_fail(
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
    assert!(stderr.contains("pending work"));
    assert!(stderr.contains("repo-dirty"));
    assert!(
        wg_dir.is_dir(),
        "dirty refusal should leave workgroup intact"
    );
    assert!(dirty_file.is_file(), "dirty repo file should remain");
    assert_outside_sentinel_survives(&outside);
    assert!(!has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[test]
fn workgroup_remove_force_dirty_deletes_dirty_repo() {
    if !git_available() {
        println!("skipping force dirty workgroup regression because git is unavailable");
        return;
    }

    let tmp = Tmp::new("cli-workgroup-dirty-force");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "dirty-force");
    create_dirty_repo(&wg_dir.join("repo-dirty"));

    let removed = run_json_machine(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
            "--force-dirty",
        ],
    );
    assert_eq!(removed["removed"], true);
    assert!(!wg_dir.exists(), "force-dirty should delete workgroup");
    assert_outside_sentinel_survives(&outside);
    assert!(has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[test]
fn workgroup_remove_refuses_live_persisted_session_even_with_force() {
    let tmp = Tmp::new("cli-workgroup-live-refuse");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "live-refuse");
    let session_cwd = wg_dir.join("__agent_architect");
    write_live_session(&config_dir, &session_cwd);

    let parsed = load_sessions_raw_from_dir_for_test(&config_dir);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].status, Some(SessionStatus::Running));
    assert!(parsed[0].id.is_some(), "live session fixture must carry id");

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
            "--force-dirty",
        ],
    );
    assert!(stderr.contains("Cannot delete while live sessions exist"));
    assert!(stderr.contains("Live Rust"));
    assert!(
        wg_dir.is_dir(),
        "live session refusal should leave workgroup intact"
    );
    assert_outside_sentinel_survives(&outside);
    assert!(!has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[cfg(target_os = "windows")]
#[test]
fn workgroup_remove_locked_file_reports_blockers_without_refresh() {
    let tmp = Tmp::new("cli-workgroup-locked");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "locked");
    let locked = wg_dir.join("locked.bin");
    std::fs::write(&locked, b"locked").expect("write locked file");
    let _handle = open_without_delete_share(&locked);

    let (_stdout, stderr) = run_fail_output(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
            "--force-dirty",
        ],
    );
    assert!(stderr.contains("file in use") || stderr.contains("Failed to delete workgroup"));
    assert!(wg_dir.is_dir(), "blocked workgroup should remain");
    assert!(locked.is_file(), "locked file should remain");
    assert_outside_sentinel_survives(&outside);
    assert!(!has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[test]
fn workgroup_remove_deletes_long_path_tree() {
    let tmp = Tmp::new("cli-workgroup-long-path");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (_project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let mut deep = wg_dir.clone();
    for idx in 0..10 {
        deep = deep.join(format!("long-segment-{idx:02}-abcdef"));
    }
    if let Err(e) = std::fs::create_dir_all(&deep) {
        println!(
            "skipping long path workgroup regression; see docs/testing/destructive-filesystem-regression.md#workgroup-long-path-check: {}",
            e
        );
        return;
    }
    std::fs::write(deep.join("payload.txt"), "payload").expect("write payload");
    let keep = wg_dir.parent().expect("wg parent").join("keep-sibling");
    std::fs::create_dir_all(&keep).expect("create keep sibling");

    let removed = run_json_machine(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
            "--force-dirty",
        ],
    );
    assert_eq!(removed["removed"], true);
    assert!(!wg_dir.exists(), "workgroup should be deleted");
    assert!(keep.is_dir(), "sibling outside workgroup should remain");
}

#[cfg(target_os = "windows")]
#[test]
fn workgroup_remove_refuses_reparse_root() {
    let tmp = Tmp::new("cli-workgroup-reparse-root");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "reparse-root");
    std::fs::remove_dir_all(&wg_dir).expect("remove real workgroup");
    let real = tmp.path().join("real-wg-target");
    std::fs::create_dir_all(&real).expect("create real target");
    std::fs::write(real.join("sentinel.txt"), "target").expect("write sentinel");
    if let Err(e) = create_junction(&wg_dir, &real) {
        println!(
            "skipping reparse root workgroup regression; see docs/testing/destructive-filesystem-regression.md#workgroup-reparse-root-check: {}",
            e
        );
        return;
    }

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
            "--force-dirty",
        ],
    );
    assert!(stderr.contains("delete_root_is_reparse_point"));
    assert!(wg_dir.exists(), "junction should remain");
    assert!(real.join("sentinel.txt").is_file(), "target should remain");
    assert_outside_sentinel_survives(&outside);
    assert!(!has_refresh_reason(&config_dir, "workgroupRemoved"));
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

// #1088: a live session whose name+cwd match a WG peer surfaces its context-usage
// percent as `contextPercent` in both `list-peers` and `list-peers-lean` (read from
// sessions.json on disk, no daemon), and peers without a matching reading omit it.
#[test]
fn list_peers_surfaces_context_percent_for_matching_live_session() {
    let tmp = Tmp::new("cli-peer-context-percent");
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
    let _added = run_json(
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
    assert!(replica.join("config.json").is_file());

    // A live session whose name (`<wg>/<agent>`) and cwd match the dev-rust WG peer,
    // carrying a context reading. `..PersistedSession::default()` leaves every other
    // field (incl. legacy) at its default; only context_percent is set here.
    let sessions = vec![PersistedSession {
        name: "wg-1-dev-team/dev-rust".to_string(),
        shell: "codex".to_string(),
        shell_args: Vec::new(),
        working_directory: replica.to_string_lossy().to_string(),
        id: Some(uuid::Uuid::new_v4().to_string()),
        status: Some(SessionStatus::Running),
        waiting_for_input: Some(false),
        created_at: Some("2026-07-21T00:00:00Z".to_string()),
        context_percent: Some(63),
        ..PersistedSession::default()
    }];
    std::fs::write(
        config_dir.join("sessions.json"),
        serde_json::to_string_pretty(&sessions).expect("sessions json"),
    )
    .expect("write sessions");

    let sender_root = project
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_architect");

    for verb in ["list-peers", "list-peers-lean"] {
        let out = Command::new(&bin)
            .args([
                verb,
                "--root",
                &sender_root.to_string_lossy(),
                "--token",
                "00000000-0000-0000-0000-000000000000",
            ])
            .output()
            .expect("run peer verb");
        assert!(
            out.status.success(),
            "{verb} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let peers: serde_json::Value = serde_json::from_slice(&out.stdout).expect("peers json");
        let mut saw_dev = false;
        for peer in peers.as_array().expect("array") {
            if peer["name"] == "ProjectAlpha:wg-1-dev-team/dev-rust" {
                saw_dev = true;
                assert_eq!(
                    peer["contextPercent"], 63,
                    "{verb}: matching live session must surface contextPercent: {peer}"
                );
            } else {
                assert!(
                    peer.get("contextPercent").is_none(),
                    "{verb}: peer {} without a matching reading must omit contextPercent: {peer}",
                    peer["name"]
                );
            }
        }
        assert!(
            saw_dev,
            "{verb}: dev-rust peer missing from output: {peers}"
        );
    }
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
    assert!(stderr.contains("Team config at"));
    assert!(stderr.contains("is malformed"));
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

// #1065 Stage F: these projects never spawn a session, so config seed never runs
// and no config scope is recorded. CLI removal therefore prunes nothing and, by the
// empty-transaction rule, never spuriously creates a seed manifest. The
// `workgroup_remove_prunes_recorded_config_scope` test below proves the prune fires
// when a scope IS recorded. Black-box: public outcomes only, no token or hook.
#[test]
fn workgroup_remove_does_not_create_a_manifest_when_no_scope_was_recorded() {
    let tmp = Tmp::new("cli-workgroup-remove-dormant");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);

    run_json(
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
    assert!(
        !project.join(".ac").join("seed-manifest.toml").exists(),
        "removing a workgroup with no recorded config scope must not create a seed manifest"
    );
}

#[test]
fn team_remove_member_does_not_create_a_manifest_when_no_scope_was_recorded() {
    let tmp = Tmp::new("cli-team-remove-member-noop");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    run_json(
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
    run_json(
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
    run_json(
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
    assert!(replica.join("config.json").is_file());

    let removed = run_json_machine(
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
    assert!(
        !project.join(".ac").join("seed-manifest.toml").exists(),
        "removing a team member with no recorded config scope must not create a seed manifest"
    );
}

// #1065 Stage F: with a pre-recorded config scope under the member's replica,
// production CLI `team remove-member` acquires the project gate (then the
// team-config guard), removes the replica, and prunes that scope's rows in the same
// transaction (acceptance items 10/22/43). End-to-end proof that the sole production
// activation token is threaded through CLI member removal. Public-outcome only.
#[test]
fn team_remove_member_prunes_recorded_replica_config_scope() {
    let tmp = Tmp::new("cli-team-remove-member-prune");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    run_json(
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
    run_json(
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
    run_json(
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
    assert!(replica.join("config.json").is_file());
    let manifest = project.join(".ac").join("seed-manifest.toml");
    write_recorded_config_manifest(&manifest, "wg-1-dev-team", "dev-rust");

    let removed = run_json_machine(
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
    let after = std::fs::read_to_string(&manifest).expect("manifest persists after member removal");
    assert!(
        !after.contains("__agent_dev-rust"),
        "the member's replica config scope must be pruned: {after}"
    );
    assert!(
        after.contains("files = []"),
        "pruning the only row leaves a valid empty v1 manifest: {after}"
    );
}

// #1065 Stage F: creating a workgroup records no manifest (config seed runs at
// session spawn, not at workgroup creation), and removing it with no recorded scope
// is a byte-stable no-op (plan section 8.2 cli_workgroup_team row, acceptance items
// 10/22/46). Public-outcome only, no token or hook.
#[test]
fn workgroup_creation_and_scopeless_removal_leave_no_manifest() {
    let tmp = Tmp::new("cli-wg-remove-noop");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let manifest = project.join(".ac").join("seed-manifest.toml");
    assert!(
        !manifest.exists(),
        "workgroup creation must not emit a seed manifest"
    );

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
    assert!(!wg_dir.exists(), "the workgroup directory is removed");
    assert!(
        !manifest.exists(),
        "removing a workgroup with no recorded scope must not create or prune a manifest"
    );
}

/// Write a valid v1 seed manifest that records a single replica config-folder scope
/// under `<workgroup>/__agent_<agent>/.claude`, exactly as a real config-seed spawn
/// would have. Used to prove Stage F lifecycle removal prunes a recorded scope.
///
/// The literal is a v1 manifest on purpose: the upgrade path must prove it
/// handles a pre-upgrade manifest written by an old build.
///
/// LEGACY-ALIAS(#1451): `source = "workspace_base"` below is deliberately NOT
/// renamed to `ac_root_base`. It is the only end-to-end proof in the repository
/// that a real pre-#1451 manifest still parses through the production CLI, and
/// rewriting it would leave every gate green while destroying that proof.
fn write_recorded_config_manifest(manifest_path: &Path, workgroup: &str, agent: &str) {
    let manifest = format!(
        concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 1\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
            "\n",
            "[[files]]\n",
            "path = \".ac/{workgroup}/__agent_{agent}/.claude/settings.json\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"replica_config_file\"\n",
            "scope = \"config:.ac/{workgroup}/__agent_{agent}/.claude\"\n",
            "source = \"workspace_base\"\n",
            "last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n"
        ),
        workgroup = workgroup,
        agent = agent
    );
    std::fs::write(manifest_path, manifest).expect("write recorded config manifest");
}

// #1065 Stage F: with a pre-recorded config scope under the workgroup, production
// CLI removal acquires the project gate, removes the workgroup, and prunes that
// scope's rows in the same transaction. Because it was the only row, the manifest
// becomes a valid empty v1 file. This is the end-to-end proof that the sole
// production activation token is threaded through CLI workgroup removal (acceptance
// items 10/22). Public-outcome only.
#[test]
fn workgroup_remove_prunes_recorded_config_scope() {
    let tmp = Tmp::new("cli-wg-remove-prune");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let manifest = project.join(".ac").join("seed-manifest.toml");
    write_recorded_config_manifest(&manifest, "wg-1-dev-team", "architect");

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
    assert!(!wg_dir.exists(), "the workgroup directory is removed");
    let after = std::fs::read_to_string(&manifest).expect("manifest persists after removal");
    assert!(
        !after.contains("wg-1-dev-team"),
        "the workgroup's config scope must be pruned: {after}"
    );
    assert!(
        after.contains("files = []"),
        "pruning the only row leaves a valid empty v1 manifest: {after}"
    );
}

// ---------------------------------------------------------------------------
// #1065 Stage F - real-boundary activation coverage for the lifecycle and
// config-seed boundaries (Grinch Finding 1).
//
// The CLI removals above are the end-to-end (a) proof for LifecycleReplicaRemoval
// and LifecycleWorkgroupRemoval. The boundaries below cannot be driven by any
// test: the config-seed publish and the session-spawn context/self-heal/
// coordinator sync sit behind `create_session_inner`, which needs an `AppHandle`
// plus SessionManager/PtyManager state and a real spawned agent session (these
// CLI suites deliberately never spawn one), and the GUI delete commands are Tauri
// commands taking `AppHandle` + `State` with no CLI path. They therefore get the
// sanctioned source-scrape wiring assertion (same form as `commands/session.rs`
// `create_session_inner_keeps_both_archive_activation_gates`): each pins the
// boundary's production `ManifestActivationToken::production()` threading, so
// removing the call - or flipping the gated activation to `None` while still
// constructing the token - reds a test.
//
// The project-context boundaries are covered in
// `tests/cli_project_registration.rs`.
// ---------------------------------------------------------------------------

/// Read a crate-relative source file, drop its `#[cfg(test)] mod tests` block,
/// and collapse ALL whitespace. Dropping the test block matches the cited
/// precedent and makes the "production only" property ENFORCED rather than
/// assumed: a future `#[cfg(test)]` use of `production()` can no longer offset a
/// removed production wiring. Collapsing whitespace makes assertions match token
/// sequences irrespective of rustfmt line-wrapping.
fn normalized_production_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let normalized = body.split_whitespace().collect::<String>();
    let end = normalized.find("#[cfg(test)]modtests{").unwrap_or_else(|| {
        panic!("{relative}: no `#[cfg(test)] mod tests` block found to split off")
    });
    normalized[..end].to_string()
}

/// Count production activation-token constructions in a scraped production
/// slice. The token string carries no interior whitespace, so it survives
/// normalization.
fn production_token_count(production: &str) -> usize {
    production
        .matches("ManifestActivationToken::production()")
        .count()
}

// (b) `commands/session.rs`: the config-seed publish chokepoint
// (ConfigExactPublish / ConfigOverBoundPublish / ConfigFailedRestore) and the
// session-spawn context read/self-heal plus coordinator v2/v3->v4 sync
// (ContextSelfHeal, CoordinatorStatelessV2ToV4, CoordinatorStatelessV3ToV4,
// CoordinatorSeededV3ToV4).
#[test]
fn session_rs_threads_production_tokens_for_config_seed_and_context() {
    let production = normalized_production_source("src/commands/session.rs");

    // Two `#[cfg(not(test))]` activation sites. Deleting either block drops the
    // count and reds this test.
    assert_eq!(
        production_token_count(&production),
        2,
        "session.rs must construct exactly two production activation tokens \
         (config-seed publish + session-spawn context sync)"
    );

    // Each assertion pins the `activation.as_ref()` ARGUMENT, not just the adapter
    // name. That catches both mutation forms: reverting to the still-present
    // non-recording `perform_config_seed` (the adapter name disappears) AND passing
    // `None` while still constructing the token (the count stays 2, but the pinned
    // argument disappears).
    assert!(
        production
            .contains("perform_config_seed_recorded(&seed,&id.to_string(),activation.as_ref()"),
        "the config-seed publish (ConfigExactPublish/OverBoundPublish/FailedRestore) must call \
         the recording adapter perform_config_seed_recorded AND thread the activation token"
    );
    assert!(
        production.contains(
            "materialize_agent_context_file_with_filename_activated(&cwd,&target_filename,\
             &managed_filenames,is_coordinator,auto_self_clear,container_repos.as_ref(),\
             activation.as_ref()"
        ),
        "the session-spawn context sync (ContextSelfHeal + Coordinator v2/v3->v4) must call the \
         activation-threading adapter materialize_agent_context_file_with_filename_activated AND \
         thread the activation token"
    );
}

// (b) `commands/entity_creation.rs`: the GUI Tauri delete commands, invoked by no
// test - delete_team (LifecycleTeamDeletion), delete_agent_matrix
// (LifecycleAgentMatrixDeletion plus its pending-inclusive staged-rollback prune,
// PendingInclusiveDeleteProtection), and delete_workgroup (the GUI site for
// LifecycleWorkgroupRemoval, whose CLI site is covered end-to-end above). Each
// OUTER command must construct and thread the token into its inner transaction.
#[test]
fn entity_creation_rs_threads_production_tokens_through_the_delete_commands() {
    let production = normalized_production_source("src/commands/entity_creation.rs");

    assert_eq!(
        production_token_count(&production),
        3,
        "entity_creation.rs must construct exactly three production activation tokens \
         (delete_team + delete_agent_matrix + delete_workgroup)"
    );

    // The inner-fn names also appear in unit calls and definitions, so the token
    // argument is what pins each assertion to the production command.
    assert!(
        production.contains(
            "delete_team_inner(&app,session_mgr.inner(),&project_path,&team_name,\
             Some(ManifestActivationToken::production()),"
        ),
        "delete_team (LifecycleTeamDeletion) must thread Some(production()) into delete_team_inner"
    );
    assert!(
        production.contains(
            "delete_agent_matrix_inner(&app,session_mgr.inner(),settings.inner(),&project_path,\
             &agent_path,Some(ManifestActivationToken::production()),"
        ),
        "delete_agent_matrix (LifecycleAgentMatrixDeletion + PendingInclusiveDeleteProtection) \
         must thread Some(production()) into delete_agent_matrix_inner"
    );
    assert!(
        production.contains(
            "letactivation=ManifestActivationToken::production();gated_workgroup_delete_with(\
             &project_root,&workgroup,Some(&activation),"
        ),
        "delete_workgroup (LifecycleWorkgroupRemoval GUI site) must construct and thread the token \
         into gated_workgroup_delete_with"
    );
}
