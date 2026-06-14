use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn test_lock() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Tmp {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ac-{}-{}-{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).expect("create tmp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn copy_binary_as(tmp: &Path, name: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let dst = tmp.join(name);
    std::fs::copy(src, &dst).expect("copy binary");
    dst
}

fn run(bin: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(bin).args(args).output().expect("spawn binary");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run_with_env(
    bin: &Path,
    env_name: &str,
    env_value: &str,
    args: &[&str],
) -> (Option<i32>, String, String) {
    let out = Command::new(bin)
        .env(env_name, env_value)
        .args(args)
        .output()
        .expect("spawn binary");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run_without_draining_output_until_exit(
    bin: &Path,
    args: &[&str],
    timeout: Duration,
) -> (Option<i32>, String, String, bool) {
    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn binary");
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("poll child") {
            Some(status) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    pipe.read_to_string(&mut stdout).expect("read stdout");
                }
                if let Some(mut pipe) = child.stderr.take() {
                    pipe.read_to_string(&mut stderr).expect("read stderr");
                }
                return (status.code(), stdout, stderr, false);
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let status = child.wait().expect("wait killed child");
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                return (status.code(), stdout, stderr, true);
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn first_json(stderr_or_stdout: &str) -> Value {
    let lines: Vec<&str> = stderr_or_stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one JSON output line, got: {stderr_or_stdout}"
    );
    serde_json::from_str(lines[0]).expect("parse json")
}

fn assert_empty_output(name: &str, output: &str) {
    assert!(
        output.trim().is_empty(),
        "{name} should be empty, got: {output}"
    );
}

fn config_dir_for(bin: &Path) -> PathBuf {
    let stem = bin.file_stem().unwrap().to_string_lossy();
    bin.parent().unwrap().join(format!(".{stem}"))
}

fn write_session(bin: &Path, pid: u32, ready_windows: &[&str]) {
    let config_dir = config_dir_for(bin);
    let automation_dir = config_dir.join("ui-automation");
    std::fs::create_dir_all(automation_dir.join("requests")).unwrap();
    std::fs::create_dir_all(automation_dir.join("responses")).unwrap();
    let session = json!({
        "pid": pid,
        "token": "00000000-0000-0000-0000-000000000497",
        "exePath": bin.to_string_lossy(),
        "configDir": config_dir.to_string_lossy(),
        "windowLabels": ["main"],
        "readyWindowLabels": ready_windows,
        "startedAtUnixMs": 1
    });
    std::fs::write(
        automation_dir.join("session.json"),
        serde_json::to_string(&session).unwrap(),
    )
    .unwrap();
}

fn write_daemon_pid(bin: &Path, pid: u32) {
    let config_dir = config_dir_for(bin);
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("daemon.pid"), pid.to_string()).unwrap();
}

#[cfg(target_os = "windows")]
fn fake_live_pid() -> Option<u32> {
    let pid = 4u32;
    agentscommander_lib::testability::ui_automation::pid_is_alive(pid).then_some(pid)
}

#[cfg(not(target_os = "windows"))]
fn fake_live_pid() -> Option<u32> {
    Some(std::process::id())
}

#[test]
fn normal_binary_refuses_ui_click_with_json_only_stdout() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-non-testable");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-click",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "refusing_non_testeable_binary");
}

#[test]
fn workgroup_binary_refuses_ui_click() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-workgroup-refuse");
    let bin = copy_binary_as(tmp.path(), "agentscommander_wg1-dev-team.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-click",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert_eq!(
        first_json(&stdout)["error"],
        "refusing_non_testeable_binary"
    );
}

#[test]
fn normal_gui_binary_refuses_ui_automation_flag() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-gui-flag-refuse");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(&bin, &["--app", "--ui-automation"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stdout", &stdout);
    assert_eq!(
        first_json(&stderr)["error"],
        "refusing_non_testeable_binary"
    );
}

#[test]
fn normal_gui_binary_refuses_ui_automation_env() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-gui-env-refuse");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run_with_env(&bin, "AC_UI_AUTOMATION", "1", &[]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stdout", &stdout);
    assert_eq!(
        first_json(&stderr)["error"],
        "refusing_non_testeable_binary"
    );
}

#[test]
fn ui_automation_env_does_not_affect_cli_subcommands() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-env-cli-subcommand");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run_with_env(&bin, "AC_UI_AUTOMATION", "1", &["list-sessions"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        !stderr.contains("refusing_non_testeable_binary"),
        "stderr: {stderr}"
    );
    serde_json::from_str::<Value>(&stdout).expect("list-sessions stdout should be JSON");
}

#[test]
fn missing_session_file_reports_automation_session_missing() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-missing-session");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.modal",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert_eq!(first_json(&stdout)["error"], "automation_session_missing");
}

#[test]
fn dead_session_pid_reports_stale_session() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-stale-session");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, u32::MAX, &["main"]);
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.modal",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert_eq!(first_json(&stdout)["error"], "automation_session_stale");
}

#[test]
fn fake_response_makes_ui_query_succeed() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-fake-response");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let automation_dir = config_dir_for(&bin).join("ui-automation");
    let requests_dir = automation_dir.join("requests");
    let responses_dir = automation_dir.join("responses");

    let responder = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let entries: Vec<PathBuf> = std::fs::read_dir(&requests_dir)
                .unwrap()
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
                .collect();
            if let Some(path) = entries.first() {
                let raw = std::fs::read_to_string(path).unwrap();
                let request: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(request["window"], "main");
                assert_eq!(request["action"], "query");
                assert_eq!(request["selector"], "onboarding.confirm");
                let request_id = request["requestId"].as_str().unwrap();
                let response = json!({
                    "ok": true,
                    "requestId": request_id,
                    "window": "main",
                    "action": "query",
                    "selector": "onboarding.confirm",
                    "target": {
                        "testId": "onboarding.confirm",
                        "role": "button",
                        "state": "ready",
                        "tag": "button",
                        "visible": true,
                        "disabled": false,
                        "checked": null,
                        "selected": null,
                        "pressed": null,
                        "expanded": null,
                        "rect": null
                    }
                });
                std::fs::write(
                    responses_dir.join(format!("{request_id}.json")),
                    serde_json::to_string(&response).unwrap(),
                )
                .unwrap();
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for request");
            thread::sleep(Duration::from_millis(25));
        }
    });

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
            "--timeout-ms",
            "3000",
        ],
    );
    responder.join().unwrap();
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["target"]["testId"], "onboarding.confirm");
}

#[test]
fn ui_query_timeout_reports_awaiting_gui_poller_phase() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-timeout-poller");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
            "--timeout-ms",
            "100",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "timeout");
    assert_eq!(parsed["phase"], "awaiting_gui_poller");
}

#[test]
fn ui_query_timeout_after_frontend_accepts_request_returns_bounded_stdout() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-query-inflight-timeout");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let automation_dir = config_dir_for(&bin).join("ui-automation");
    let requests_dir = automation_dir.join("requests");

    let mover_dir = requests_dir.clone();
    let mover = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let entries: Vec<PathBuf> = std::fs::read_dir(&mover_dir)
                .unwrap()
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension().and_then(|e| e.to_str()) == Some("json")
                        && !path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|name| name.ends_with(".inflight.json"))
                })
                .collect();
            if let Some(path) = entries.first() {
                let raw = std::fs::read_to_string(path).unwrap();
                let request: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(request["action"], "query");
                let request_id = request["requestId"].as_str().unwrap();
                std::fs::rename(path, mover_dir.join(format!("{request_id}.inflight.json")))
                    .unwrap();
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for request");
            thread::sleep(Duration::from_millis(10));
        }
    });

    let start = Instant::now();
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "does.not.exist",
            "--timeout-ms",
            "250",
        ],
    );
    mover.join().unwrap();

    assert!(
        start.elapsed() < Duration::from_secs(5),
        "ui-query should not hang"
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "timeout");
    assert_eq!(parsed["phase"], "awaiting_frontend_response");
}

#[test]
fn ui_query_large_missing_selector_response_is_bounded_stdout() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-large-missing-selector");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let automation_dir = config_dir_for(&bin).join("ui-automation");
    let requests_dir = automation_dir.join("requests");
    let responses_dir = automation_dir.join("responses");

    let responder = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let entries: Vec<PathBuf> = std::fs::read_dir(&requests_dir)
                .unwrap()
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
                .collect();
            if let Some(path) = entries.first() {
                let raw = std::fs::read_to_string(path).unwrap();
                let request: Value = serde_json::from_str(&raw).unwrap();
                let request_id = request["requestId"].as_str().unwrap();
                let available: Vec<Value> = (0..256)
                    .map(|i| {
                        json!({
                            "testId": format!("target.{i}"),
                            "role": "button",
                            "state": "ready",
                            "tag": "button",
                            "text": "x".repeat(2000),
                            "visible": true,
                            "disabled": false,
                            "checked": null,
                            "selected": null,
                            "pressed": null,
                            "expanded": null,
                            "rect": {
                                "x": i,
                                "y": i,
                                "width": 100,
                                "height": 30
                            }
                        })
                    })
                    .collect();
                let response = json!({
                    "ok": false,
                    "requestId": request_id,
                    "window": "main",
                    "action": "query",
                    "selector": "does.not.exist",
                    "error": "missing_selector",
                    "message": "No automation target matched data-ac-testid=\"does.not.exist\" in window \"main\".",
                    "available": available,
                    "diagnostics": {
                        "devicePixelRatio": 1,
                        "viewport": { "width": 1280, "height": 720 }
                    }
                });
                std::fs::write(
                    responses_dir.join(format!("{request_id}.json")),
                    serde_json::to_string(&response).unwrap(),
                )
                .unwrap();
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for request");
            thread::sleep(Duration::from_millis(10));
        }
    });

    let start = Instant::now();
    let (code, stdout, stderr, timed_out) = run_without_draining_output_until_exit(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "does.not.exist",
            "--timeout-ms",
            "3000",
        ],
        Duration::from_secs(5),
    );
    responder.join().unwrap();

    assert!(!timed_out, "ui-query should not hang");
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert!(
        stdout.len() < 4096,
        "stdout should stay below common pipe buffers, got {} bytes",
        stdout.len()
    );
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "missing_selector");
    assert_eq!(parsed["available"].as_array().unwrap().len(), 8);
    assert_eq!(parsed["diagnostics"]["availableTotal"], 256);
    assert_eq!(parsed["diagnostics"]["availableLimit"], 8);
    assert_eq!(parsed["diagnostics"]["availableTruncated"], true);
    let text = parsed["available"][0]["text"].as_str().unwrap();
    assert!(text.ends_with("..."));
    assert!(text.len() <= 83);
}

#[test]
fn ui_click_timeout_removes_inflight_request_with_json_only_stdout() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-timeout-inflight");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &[]);
    let automation_dir = config_dir_for(&bin).join("ui-automation");
    let requests_dir = automation_dir.join("requests");

    let mover_dir = requests_dir.clone();
    let mover = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let entries: Vec<PathBuf> = std::fs::read_dir(&mover_dir)
                .unwrap()
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension().and_then(|e| e.to_str()) == Some("json")
                        && !path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|name| name.ends_with(".inflight.json"))
                })
                .collect();
            if let Some(path) = entries.first() {
                let raw = std::fs::read_to_string(path).unwrap();
                let request: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(request["action"], "click");
                let request_id = request["requestId"].as_str().unwrap();
                std::fs::rename(path, mover_dir.join(format!("{request_id}.inflight.json")))
                    .unwrap();
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for request");
            thread::sleep(Duration::from_millis(10));
        }
    });

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-click",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
            "--timeout-ms",
            "250",
        ],
    );
    mover.join().unwrap();
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "timeout");
    assert_eq!(parsed["phase"], "awaiting_frontend_ready");

    let remaining: Vec<PathBuf> = std::fs::read_dir(&requests_dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    assert!(
        remaining.is_empty(),
        "timed-out request files should be removed: {remaining:?}"
    );
}

#[test]
fn ui_query_retries_transient_missing_session_file() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-session-read-race");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let writer_bin = bin.clone();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        write_session(&writer_bin, pid, &["main"]);
    });

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
            "--timeout-ms",
            "100",
        ],
    );
    writer.join().unwrap();

    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "timeout");
    assert_ne!(parsed["error"], "automation_session_missing");
}

#[test]
fn stale_prior_session_with_running_daemon_reports_automation_not_enabled_on_stdout() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-disabled-stale-session");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, u32::MAX, &["main"]);
    write_daemon_pid(&bin, pid);

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.modal",
        ],
    );

    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "automation_not_enabled");
}
