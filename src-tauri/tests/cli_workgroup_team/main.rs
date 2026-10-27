use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

use agentscommander_lib::config::sessions_persistence::{
    load_sessions_raw_from_dir_for_test, PersistedSession,
};
use agentscommander_lib::session::session::SessionStatus;

/// Excludes one test's open write descriptor on a freshly copied binary from
/// overlapping another test's fork/exec.
///
/// These tests run in parallel and each copies the binary into its own temp dir
/// before exec'ing it. `Command::spawn` forks, and the child inherits the write
/// descriptor another thread still holds on *its* copy; exec'ing a binary that
/// any process holds open for writing fails with `ETXTBSY`. Covering both the
/// copy and the spawn closes that window. The lock is released before output is
/// collected, so the binary runs themselves still overlap.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn spawn_lock() -> MutexGuard<'static, ()> {
    // A test that panics elsewhere must not disable the guard for the rest.
    SPAWN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn command_for_binary(bin: &Path) -> Command {
    let mut command = Command::new(bin);
    let stem = bin.file_stem().expect("bin stem").to_string_lossy();
    if !stem.contains('_') {
        command.env("AGENTSCOMMANDER_CONFIG_DIR", config_dir_for_bin(bin));
    }
    command
}

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
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    {
        let _guard = spawn_lock();
        std::fs::copy(src, &dst).expect("copy binary");
    }
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
        config_dir.join("settings.30.instance.no-git.json"),
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
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
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
    let mut command = command_for_binary(bin);
    command
        .env("AC_MACHINE_OUTPUT", "1")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
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
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
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
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
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
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
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
    let wg_dir = project.join(".ac").join("room-1-dev-team");
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
                    !(name.starts_with("room-")
                        || name.starts_with("wg-")
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

mod activity;
mod membership;
mod production;
mod room;
mod scope;
mod team;
mod workgroup;
