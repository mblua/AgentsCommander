//! Integration tests for project registration CLI refresh requests.
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

fn write_settings(config_dir: &Path, project_paths: &[&Path]) {
    std::fs::create_dir_all(config_dir).expect("create config dir");
    let settings = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "projectPaths": project_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
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

fn clear_project_refresh_requests(config_dir: &Path) {
    let requests_dir = config_dir.join("project-refresh-requests");
    if requests_dir.exists() {
        std::fs::remove_dir_all(requests_dir).expect("clear requests");
    }
}

fn read_single_project_refresh_request(config_dir: &Path) -> serde_json::Value {
    let requests = project_refresh_request_paths(config_dir);
    assert_eq!(requests.len(), 1, "expected one project refresh request");
    serde_json::from_str(&std::fs::read_to_string(&requests[0]).expect("read request"))
        .expect("request json")
}

fn assert_registration_request(request: &serde_json::Value, project: &Path) {
    uuid::Uuid::parse_str(request["id"].as_str().expect("id")).expect("request id");
    assert!(!request["timestamp"].as_str().expect("timestamp").is_empty());
    assert_eq!(
        request["projectPath"].as_str().expect("projectPath"),
        std::fs::canonicalize(project)
            .expect("canonical project")
            .to_string_lossy()
            .as_ref()
    );
    assert!(request
        .get("changedPath")
        .is_none_or(|value| value.is_null()));
    assert!(request
        .get("changedName")
        .is_none_or(|value| value.is_null()));
    assert_eq!(request["reason"], "projectRegistered");
}

fn run_success(bin: &Path, args: &[&str]) {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_failure(bin: &Path, args: &[&str]) {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        !out.status.success(),
        "expected failure\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_no_ac_side_effect_dirs(root: &Path) {
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
fn new_project_writes_project_registered_refresh() {
    let tmp = Tmp::new("cli-new-project-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["new-project", &project_arg]);

    assert!(project.join(".ac").is_dir());
    let request = read_single_project_refresh_request(&config_dir);
    assert_registration_request(&request, &project);
}

#[test]
fn open_project_writes_project_registered_refresh_for_new_registration() {
    let tmp = Tmp::new("cli-open-project-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac")).expect("create project");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["open-project", &project_arg]);

    let request = read_single_project_refresh_request(&config_dir);
    assert_registration_request(&request, &project);
}

#[test]
fn open_project_noop_does_not_write_refresh() {
    let tmp = Tmp::new("cli-open-project-noop-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac")).expect("create project");
    write_settings(&config_dir, &[&project]);
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["open-project", &project_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn new_project_bad_parent_path_does_not_write_settings_or_refresh() {
    let tmp = Tmp::new("cli-new-project-bad-parent");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let config_sentinel = config_dir.join("sentinel.txt");
    std::fs::write(&config_sentinel, "keep").expect("write config sentinel");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let parent_file = tmp.path().join("parent-file");
    std::fs::write(&parent_file, "not a dir").expect("write parent file");
    let bad_project = parent_file.join("ChildProject");
    let bad_project_arg = bad_project.to_string_lossy().to_string();

    run_failure(&bin, &["new-project", &bad_project_arg]);

    assert!(!bad_project.exists());
    assert!(
        !config_dir.join("settings.json").exists(),
        "settings.json should not be written on failed new-project"
    );
    assert!(project_refresh_request_paths(&config_dir).is_empty());
    assert_eq!(
        std::fs::read_to_string(&config_sentinel).expect("read config sentinel"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_no_ac_side_effect_dirs(tmp.path());
}

#[test]
fn new_project_noop_does_not_write_refresh() {
    let tmp = Tmp::new("cli-new-project-noop-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();
    run_success(&bin, &["new-project", &project_arg]);
    clear_project_refresh_requests(&config_dir);

    run_success(&bin, &["new-project", &project_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn open_project_invalid_path_does_not_write_refresh() {
    let tmp = Tmp::new("cli-open-project-invalid-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let missing = tmp.path().join("MissingProject");
    let missing_arg = missing.to_string_lossy().to_string();

    run_failure(&bin, &["open-project", &missing_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}
