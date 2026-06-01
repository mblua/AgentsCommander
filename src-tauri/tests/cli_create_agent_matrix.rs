//! Integration tests for the `create-agent-matrix` CLI verb.
//!
//! Each test copies the binary under test into a temp directory so
//! `config_dir()` resolves to an isolated sibling config directory.

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
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        let path = std::env::temp_dir().join(format!(
            "ac-{}-{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            h.finish()
        ));
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

fn write_settings(config_dir: &Path, settings: serde_json::Value) {
    std::fs::create_dir_all(config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings).expect("settings json"),
    )
    .expect("write settings.json");
}

fn settings_with_project_paths(paths: &[&Path]) -> serde_json::Value {
    serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "projectPaths": paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>(),
    })
}

fn project_with_workspace_name(tmp: &Path, workspace_name: &str) -> PathBuf {
    let project = tmp.join("ProjectAlpha");
    std::fs::create_dir_all(project.join(workspace_name)).expect("create workspace");
    project
}

fn project_with_workspace(tmp: &Path) -> PathBuf {
    project_with_workspace_name(tmp, ".ac")
}

fn project_with_workspaces(tmp: &Path, canonical: bool, legacy: bool) -> PathBuf {
    let project = tmp.join("ProjectAlpha");
    if canonical {
        std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
    }
    if legacy {
        std::fs::create_dir_all(project.join(".ac-new")).expect("create .ac-new");
    }
    project
}

fn session_request_paths(config_dir: &Path) -> Vec<PathBuf> {
    let requests_dir = config_dir.join("session-requests");
    if !requests_dir.exists() {
        return Vec::new();
    }
    std::fs::read_dir(&requests_dir)
        .expect("read session-requests")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect()
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

fn assert_single_project_refresh_request_for_agent(
    config_dir: &Path,
    project: &Path,
    agent_dir: &Path,
) -> serde_json::Value {
    let requests = project_refresh_request_paths(config_dir);
    assert_eq!(
        requests.len(),
        1,
        "expected exactly one project refresh request"
    );
    let request: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&requests[0]).expect("read request"))
            .expect("project refresh request json");
    uuid::Uuid::parse_str(request["id"].as_str().expect("id")).expect("id should be a uuid");
    assert!(!request["timestamp"].as_str().expect("timestamp").is_empty());
    assert_eq!(
        request["projectPath"].as_str().expect("projectPath"),
        std::fs::canonicalize(project)
            .expect("canonical project")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        request["agentPath"].as_str().expect("agentPath"),
        std::fs::canonicalize(agent_dir)
            .expect("canonical agent dir")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(request["agentName"], "ProjectAlpha/architect");
    assert_eq!(request["reason"], "createAgentMatrix");
    request
}

fn assert_single_project_refresh_request(config_dir: &Path, project: &Path) -> serde_json::Value {
    assert_single_project_refresh_request_for_agent(
        config_dir,
        project,
        &project.join(".ac").join("_agent_architect"),
    )
}

#[test]
fn create_agent_matrix_success_prints_json_and_writes_layout() {
    let tmp = Tmp::new("cli-create-agent-matrix-success");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    let agent_dir = project.join(".ac").join("_agent_architect");
    assert_eq!(
        json["agentPath"].as_str().expect("agentPath"),
        agent_dir.to_string_lossy().as_ref()
    );
    assert_eq!(json["agentName"], "ProjectAlpha/architect");
    assert_eq!(
        json["rolePath"].as_str().expect("rolePath"),
        agent_dir.join("Role.md").to_string_lossy().as_ref()
    );
    assert_eq!(json["launched"], false);
    assert!(json["launchAgent"].is_null());
    assert!(agent_dir.join("Role.md").is_file());
    assert!(agent_dir.join("config.json").is_file());
    for dir in ["memory", "plans", "skills", "inbox", "outbox"] {
        assert!(agent_dir.join(dir).is_dir(), "missing {}", dir);
    }
}

#[test]
fn create_agent_matrix_uses_legacy_workspace_when_canonical_absent() {
    let tmp = Tmp::new("cli-create-agent-matrix-legacy");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace_name(tmp.path(), ".ac-new");
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    let agent_dir = project.join(".ac-new").join("_agent_architect");
    assert_eq!(
        json["agentPath"].as_str().expect("agentPath"),
        agent_dir.to_string_lossy().as_ref()
    );
    assert!(agent_dir.join("Role.md").is_file());
    assert_single_project_refresh_request_for_agent(&config_dir, &project, &agent_dir);
}

#[test]
fn create_agent_matrix_prefers_ac_when_both_workspaces_exist() {
    let tmp = Tmp::new("cli-create-agent-matrix-both");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspaces(tmp.path(), true, true);
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    let agent_dir = project.join(".ac").join("_agent_architect");
    assert_eq!(
        json["agentPath"].as_str().expect("agentPath"),
        agent_dir.to_string_lossy().as_ref()
    );
    assert!(agent_dir.join("Role.md").is_file());
    assert!(!project.join(".ac-new").join("_agent_architect").exists());
    assert_single_project_refresh_request_for_agent(&config_dir, &project, &agent_dir);
}

#[test]
fn create_agent_matrix_resolves_project_name_from_settings_for_unrelated_root() {
    let tmp = Tmp::new("cli-create-agent-matrix-project-name-root");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );
    let project = project_with_workspace(tmp.path());
    let root = tmp.path().join("root-agent");
    std::fs::create_dir_all(&root).expect("root dir");
    let root_s = root.to_string_lossy().to_string();

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--root",
            &root_s,
            "--token",
            "00000000-0000-0000-0000-000000000000",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    let agent_dir = project.join(".ac").join("_agent_architect");
    assert_eq!(
        json["agentPath"].as_str().expect("agentPath"),
        agent_dir.to_string_lossy().as_ref()
    );
    assert_eq!(json["agentName"], "ProjectAlpha/architect");
    assert!(agent_dir.join("Role.md").is_file());
    assert_single_project_refresh_request(&config_dir, &project);
}

#[test]
fn create_agent_matrix_rejects_ambiguous_project_name_without_writing() {
    let tmp = Tmp::new("cli-create-agent-matrix-ambiguous-name");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let parent_a = tmp.path().join("a");
    let parent_b = tmp.path().join("b");
    let project_a = parent_a.join("ProjectAlpha");
    let project_b = parent_b.join("ProjectAlpha");
    std::fs::create_dir_all(project_a.join(".ac")).expect("project a");
    std::fs::create_dir_all(project_b.join(".ac")).expect("project b");
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "projectPaths": [
                parent_a.to_string_lossy().to_string(),
                parent_b.to_string_lossy().to_string()
            ]
        }),
    );

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ambiguous"));
    assert!(!project_a.join(".ac").join("_agent_architect").exists());
    assert!(!project_b.join(".ac").join("_agent_architect").exists());
    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn create_agent_project_mode_requires_description() {
    let tmp = Tmp::new("cli-create-agent-project-mode-description");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );
    let project = project_with_workspace(tmp.path());

    let out = Command::new(&bin)
        .args([
            "create-agent",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        !out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("required"));
    assert!(!project.join(".ac").join("_agent_architect").exists());
}

#[test]
fn create_agent_rejects_parent_argument() {
    let tmp = Tmp::new("cli-create-agent-reject-parent");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let parent = tmp.path().join("Agents");
    std::fs::create_dir_all(&parent).expect("create parent");
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));

    let out = Command::new(&bin)
        .args([
            "create-agent",
            "--parent",
            &parent.to_string_lossy(),
            "--name",
            "Bot",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        !out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
    assert!(!parent.join("Bot").exists());
}

#[test]
fn create_agent_rejects_project_path_input_without_writing() {
    let tmp = Tmp::new("cli-create-agent-reject-project-path");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));
    let project_path = project.to_string_lossy().to_string();

    let out = Command::new(&bin)
        .args([
            "create-agent",
            "--project",
            &project_path,
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("was not found"));
    assert!(!project.join(".ac").join("_agent_architect").exists());
    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn create_agent_matrix_rejects_project_path_input_without_writing() {
    let tmp = Tmp::new("cli-create-agent-matrix-reject-project-path");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));
    let project_path = project.to_string_lossy().to_string();

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            &project_path,
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("was not found"));
    assert!(!project.join(".ac").join("_agent_architect").exists());
    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn create_agent_matrix_success_writes_project_refresh_request() {
    let tmp = Tmp::new("cli-create-agent-matrix-success-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    assert_single_project_refresh_request(&config_dir, &project);
}

#[test]
fn create_agent_matrix_project_name_resolves_from_settings_not_cwd() {
    let tmp = Tmp::new("cli-create-agent-matrix-project-name");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let registered_parent = tmp.path().join("registered");
    let registered_project = registered_parent.join("ProjectAlpha");
    std::fs::create_dir_all(registered_project.join(".ac")).expect("registered project");
    let caller_cwd = tmp.path().join("caller-cwd");
    let cwd_project = caller_cwd.join("ProjectAlpha");
    std::fs::create_dir_all(cwd_project.join(".ac")).expect("cwd project");
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "projectPaths": [registered_parent.to_string_lossy().to_string()]
        }),
    );

    let out = Command::new(&bin)
        .current_dir(&caller_cwd)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    let registered_agent_dir = registered_project.join(".ac").join("_agent_architect");
    let cwd_agent_dir = cwd_project.join(".ac").join("_agent_architect");
    assert_eq!(
        json["agentPath"].as_str().expect("agentPath"),
        registered_agent_dir.to_string_lossy().as_ref()
    );
    assert!(registered_agent_dir.join("Role.md").is_file());
    assert!(
        !cwd_agent_dir.exists(),
        "bare project name must not use cwd"
    );
    assert_single_project_refresh_request(&config_dir, &registered_project);
}

#[test]
fn create_agent_project_mode_delegates_to_matrix_creation() {
    let tmp = Tmp::new("cli-create-agent-project-mode");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let out = Command::new(&bin)
        .args([
            "create-agent",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    let agent_dir = project.join(".ac").join("_agent_architect");
    assert_eq!(
        json["agentPath"].as_str().expect("agentPath"),
        agent_dir.to_string_lossy().as_ref()
    );
    assert_eq!(json["agentName"], "ProjectAlpha/architect");
    assert_eq!(
        json["rolePath"].as_str().expect("rolePath"),
        agent_dir.join("Role.md").to_string_lossy().as_ref()
    );
    assert!(json.get("claudeMd").is_none());
    assert!(agent_dir.join("Role.md").is_file());
    assert!(agent_dir.join("config.json").is_file());
    assert_single_project_refresh_request(&config_dir, &project);
}

#[test]
fn create_agent_matrix_local_template_writes_role_and_skills() {
    let tmp = Tmp::new("cli-create-agent-matrix-local-template");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));
    let template_dir = config_dir.join("agent-templates").join("planner");
    std::fs::create_dir_all(template_dir.join("skills").join("demo"))
        .expect("create local template skills");
    std::fs::write(
        template_dir.join("Role.md"),
        "---\nname: Planner\n---\n\n# Planner Template\n\nPlan carefully.\n",
    )
    .expect("write local template role");
    std::fs::write(
        template_dir.join("skills").join("demo").join("SKILL.md"),
        "# Demo Skill\n",
    )
    .expect("write local template skill");

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--role-template",
            "local:planner",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let agent_dir = project.join(".ac").join("_agent_architect");
    let role = std::fs::read_to_string(agent_dir.join("Role.md")).expect("read Role.md");
    assert!(role.contains("## Role Profile"));
    assert!(role.contains("# Planner Template"));
    assert!(agent_dir
        .join("skills")
        .join("demo")
        .join("SKILL.md")
        .is_file());
}

#[test]
fn create_agent_matrix_invalid_template_exits_1_without_target_dir() {
    let tmp = Tmp::new("cli-create-agent-matrix-invalid-template");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(&config_dir, settings_with_project_paths(&[tmp.path()]));

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--role-template",
            "agency:not-real",
        ])
        .output()
        .expect("spawn binary");

    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Unknown built-in role template"));
    assert!(!project.join(".ac").join("_agent_architect").exists());
    assert!(
        project_refresh_request_paths(&config_dir).is_empty(),
        "invalid template must not write a project refresh request"
    );
}

#[test]
fn create_agent_matrix_honors_settings_flags() {
    let tmp = Tmp::new("cli-create-agent-matrix-settings");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "claude",
                "label": "Claude",
                "command": "claude",
                "color": "#ffffff",
                "excludeGlobalClaudeMd": true
            }],
            "injectRtkHook": true,
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let settings_path = project
        .join(".ac")
        .join("_agent_architect")
        .join(".claude")
        .join("settings.local.json");
    let settings = std::fs::read_to_string(settings_path).expect("read settings.local.json");
    assert!(settings.contains("claudeMdExcludes"));
    assert!(settings.contains("PreToolUse"));
}

#[test]
fn create_agent_matrix_launch_writes_session_request() {
    let tmp = Tmp::new("cli-create-agent-matrix-launch");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex --ask-for-approval never",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "codex",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(json["launched"], true);
    assert_eq!(json["launchAgent"], "codex");

    let requests = session_request_paths(&config_dir);
    assert_eq!(requests.len(), 1, "expected exactly one session request");

    let request: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&requests[0]).expect("read request"))
            .expect("request json");
    let agent_dir = project.join(".ac").join("_agent_architect");
    assert_eq!(
        request["cwd"].as_str().expect("cwd"),
        agent_dir.to_string_lossy().as_ref()
    );
    assert_eq!(request["sessionName"], "ProjectAlpha/architect");
    assert_eq!(request["agentId"], "codex");

    assert_single_project_refresh_request(&config_dir, &project);
}

#[test]
fn create_agent_matrix_whitespace_launch_command_warns_without_request() {
    let tmp = Tmp::new("cli-create-agent-matrix-blank-launch");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let _project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "   \t  ",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "codex",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(json["launched"], false);
    assert!(json["launchAgent"].is_null());
    assert!(String::from_utf8_lossy(&out.stderr).contains("empty command"));
    assert!(
        session_request_paths(&config_dir).is_empty(),
        "blank command must not write a launch request"
    );
}

#[test]
fn create_agent_matrix_empty_launch_request_warns_without_request() {
    let tmp = Tmp::new("cli-create-agent-matrix-empty-launch-request");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let _project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex --ask-for-approval never",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let out = Command::new(&bin)
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(json["launched"], false);
    assert!(json["launchAgent"].is_null());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found in settings"));
    assert!(
        session_request_paths(&config_dir).is_empty(),
        "empty launch request must not write a launch request"
    );
}

#[test]
fn create_agent_project_mode_blank_launch_command_warns_without_request() {
    let tmp = Tmp::new("cli-create-agent-project-blank-launch");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let out = Command::new(&bin)
        .args([
            "create-agent",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "codex",
        ])
        .output()
        .expect("spawn binary");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(json["launched"], false);
    assert!(json["launchAgent"].is_null());
    assert_eq!(json["agentName"], "ProjectAlpha/architect");
    assert!(String::from_utf8_lossy(&out.stderr).contains("empty command"));
    assert!(project.join(".ac").join("_agent_architect").is_dir());
    assert!(
        session_request_paths(&config_dir).is_empty(),
        "blank command must not write a launch request"
    );
}
