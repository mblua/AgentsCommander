//! Integration tests for project registration CLI refresh requests.
//!
//! Each test copies the binary under test into a temp directory so
//! `config_dir()` resolves to an isolated sibling config directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

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

/// Drain both pipes while waiting so output cannot defeat the child deadline.
fn bounded_output(mut child: std::process::Child) -> std::process::Output {
    use std::io::Read;
    use std::time::{Duration, Instant};

    let stdout = child.stdout.take().expect("stdout pipe");
    let stderr = child.stderr.take().expect("stderr pipe");
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    };
    let stdout = drain(Box::new(stdout));
    let stderr = drain(Box::new(stderr));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => {
                let reason = format!("child deadline/wait failure: {result:?}");
                let _ = child.kill();
                let reaped = child.wait();
                break Err(format!("{reason}; reap: {reaped:?}"));
            }
        }
    };
    let stdout = stdout.join().expect("stdout reader").expect("read stdout");
    let stderr = stderr.join().expect("stderr reader").expect("read stderr");
    std::process::Output {
        status: status.unwrap_or_else(|error| {
            panic!(
                "{error}
stdout: {}
stderr: {}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            )
        }),
        stdout,
        stderr,
    }
}

fn run_success(bin: &Path, args: &[&str]) {
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
    let out = bounded_output(child);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Success capture that also exposes stderr: #1967 asserts the CLI diagnostics
/// (stdout JSON plus stderr warnings) while keeping this file's frozen route
/// set (the issue_1867 isolation contract counts `command_for_binary` routes).
fn run_json(bin: &Path, args: &[&str]) -> (serde_json::Value, String) {
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
    let out = bounded_output(child);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (
        serde_json::from_slice(&out.stdout).expect("stdout json"),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Failure capture returning the raw output so the caller can assert the exact
/// exit status, stdout and stderr (the #1967 unavailable diagnostic).
fn run_failure(bin: &Path, args: &[&str]) -> std::process::Output {
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
    let out = bounded_output(child);
    assert!(
        !out.status.success(),
        "expected failure\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
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

// P05: production consumers must never finalize GUI migrations while reading.
const P05_TOKEN: &str = "a873f0dd-732b-46b7-abd4-f690dc37080a";
const P05_MESSAGE: &str = "20261009-150000-room01-peer-to-room01-coord-p05.md";

struct ReadonlyFixture {
    _tmp: Tmp,
    bin: PathBuf,
    config: PathBuf,
    project: PathBuf,
    coordinator: String,
    peer: String,
    origin: String,
}

impl ReadonlyFixture {
    fn new() -> Self {
        let tmp = Tmp::new("p05-readonly");
        let bin = copy_binary_into(tmp.path());
        let config = config_dir_for_bin(&bin);
        let project = tmp.path().join("RegisteredRemote");
        let ac = project.join(".ac");
        let room = ac.join("room-01-devs");
        for name in ["coord", "peer"] {
            let origin = ac.join(format!("_agent_{name}"));
            let replica = room.join(format!("__agent_{name}"));
            std::fs::create_dir_all(&origin).unwrap();
            std::fs::create_dir_all(&replica).unwrap();
            std::fs::write(
                origin.join("config.json"),
                r#"{"tooling":{"lastCodingAgent":"codex"}}"#,
            )
            .unwrap();
            std::fs::write(
                replica.join("config.json"),
                format!(r#"{{"identity":"../../_agent_{name}"}}"#),
            )
            .unwrap();
        }
        let team = ac.join("_team_devs");
        std::fs::create_dir_all(&team).unwrap();
        std::fs::write(
            team.join("config.json"),
            r#"{"agents":["_agent_coord","_agent_peer"],"coordinator":"_agent_coord","repos":[]}"#,
        )
        .unwrap();
        let messaging = room.join("messaging");
        std::fs::create_dir_all(&messaging).unwrap();
        std::fs::write(messaging.join(P05_MESSAGE), "P05 message").unwrap();
        write_settings(&config, &[&project]);
        // Use the registered folder name required by CLI project resolution.
        // Settle naming journal/lock before any managed-byte snapshot or probe.
        let _ = run_json(
            &bin,
            &["workgroup", "list", "--project", "RegisteredRemote"],
        );
        let value = serde_json::json!({
            "defaultShell":"powershell.exe", "defaultShellArgs":[],
            "agents":[], "projectPaths":[project],
            "startOnlyCoordinators":true, "sidebarZoom":1.25
        });
        std::fs::write(
            config.join("settings.30.instance.no-git.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        Self {
            bin,
            config,
            coordinator: room.join("__agent_coord").to_string_lossy().into_owned(),
            peer: room.join("__agent_peer").to_string_lossy().into_owned(),
            origin: ac.join("_agent_coord").to_string_lossy().into_owned(),
            project,
            _tmp: tmp,
        }
    }

    fn managed_bytes(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(&self.config)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                // Lock/journal/log files have separate, existing authority.
                path.is_file()
                    && (name.starts_with("settings")
                        || name.starts_with("agents")
                        || name.starts_with("project-paths"))
                    && !name.ends_with(".lock")
            })
            .map(|path| {
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read(path).unwrap(),
                )
            })
            .collect()
    }

    fn outbox_for_root(&self, root: &str) -> PathBuf {
        Path::new(root)
            .join(self.config.file_name().unwrap())
            .join("outbox")
    }

    fn assert_no_outbox(&self) {
        for root in [&self.coordinator, &self.peer] {
            let path = self.outbox_for_root(root);
            assert!(!path.exists() || std::fs::read_dir(path).unwrap().next().is_none());
        }
    }

    fn assert_reader(&self, args: &[&str]) -> serde_json::Value {
        let before = self.managed_bytes();
        let (value, _) = run_json(&self.bin, args);
        assert_eq!(self.managed_bytes(), before, "managed writes from {args:?}");
        value
    }

    fn assert_denied(&self, args: &[&str], diagnostic: &str) {
        let before = self.managed_bytes();
        let out = run_failure(&self.bin, args);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(diagnostic),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains("Queued:"));
        assert!(
            serde_json::from_slice::<serde_json::Value>(&out.stdout).is_err(),
            "source/authorization refusal emitted success JSON"
        );
        assert_eq!(self.managed_bytes(), before, "managed writes from {args:?}");
        self.assert_no_outbox();
    }
}

fn write_p05_custom_settings(fixture: &ReadonlyFixture) {
    let value = serde_json::json!({
        "defaultShell":"powershell.exe", "defaultShellArgs":[],
        "projectPaths":[fixture.project], "projectPathsRelativeToInstance":[null],
        "projectPath":fixture.project, "projectPathRelativeToInstance":null,
        "archivedProjectPaths":[], "archivedProjectPathsRelativeToInstance":[],
        "npmUpdateNotificationsEnabled":false,
        "sidebarZoom":1.0, "mainZoom":1.0,
        "agents":[{"id":"p05","label":"P05 custom","command":"codex","color":"#123456",
            "instructionsFilename":"P05-INSTRUCTIONS.md"}]
    });
    std::fs::write(
        fixture.config.join("settings.30.instance.no-git.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

mod activation;
mod catalog;
mod p03;
mod p05;
mod registration;
mod source_pins;
use source_pins::{normalized_production_source, production_token_count};
