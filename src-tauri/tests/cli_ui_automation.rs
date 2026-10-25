#[path = "cli_ui_automation/pointer_and_query.rs"]
mod pointer_and_query;

use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn command_for_binary(bin: &Path) -> Command {
    let mut command = Command::new(bin);
    let stem = bin.file_stem().expect("bin stem").to_string_lossy();
    if !stem.contains('_') {
        command.env("AGENTSCOMMANDER_CONFIG_DIR", config_dir_for(bin));
    }
    command
}

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
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let dst = tmp.join(name);
    std::fs::copy(src, &dst).expect("copy binary");
    dst
}

#[test]
fn default_capability_allows_resource_monitor_window() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities/default.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let capability: Value = serde_json::from_str(&raw).unwrap();
    let windows = capability["windows"]
        .as_array()
        .expect("capability windows must be an array");

    assert!(
        windows.iter().any(|window| window == "resource-monitor"),
        "resource-monitor must be in default capability windows so its frontend can mark UI automation ready"
    );
}

fn run(bin: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = command_for_binary(bin)
        .args(args)
        .output()
        .expect("spawn binary");
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
    let out = command_for_binary(bin)
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

/// #1773: `std::process::Child` does not kill on drop, so a panic between spawn and
/// wait leaks the child. This guard kills and reaps it on every exit path. After a
/// normal exit both calls are no-ops: `kill` errors on an exited child and `wait`
/// returns the cached status.
struct ReapOnDrop(std::process::Child);

impl Drop for ReapOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_without_draining_output_until_exit(
    bin: &Path,
    args: &[&str],
    timeout: Duration,
) -> (Option<i32>, String, String, bool) {
    let mut guard = ReapOnDrop(
        command_for_binary(bin)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn binary"),
    );
    let child = &mut guard.0;
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

/// Backstop for a watcher whose owner never called `finish()` and whose `Drop`
/// never ran (a process-level abort), and the hang cap for bounds that have no
/// request-file anchor yet. The normal exit is the stop flag.
const WATCHER_HARD_CAP: Duration = Duration::from_secs(60);

/// What the watcher thread observed. `LateRequest` exists so that a request
/// observed only after the stop flag is reported, never acted on.
enum WatchOutcome {
    /// Acted on the request; the `Instant` is when the file was first observed.
    Served(Instant),
    /// A request file was present on the scan that followed the stop flag.
    LateRequest(PathBuf),
    /// No request file ever appeared.
    NeverArrived,
}

/// A worker that watches the automation requests directory while the CLI runs.
struct RequestWatcher {
    handle: Option<thread::JoinHandle<WatchOutcome>>,
    stop: Arc<AtomicBool>,
}

impl RequestWatcher {
    /// Call after the CLI run has returned. Signals that the CLI can no longer
    /// write a request, joins, and asserts the worker actually served one.
    /// Returns the instant the request file was first observed: the only
    /// load-independent zero point for a "did not hang" assertion.
    fn finish(mut self) -> Instant {
        self.stop.store(true, Ordering::SeqCst);
        let handle = self.handle.take().expect("watcher joined twice");
        match handle.join().expect("request watcher panicked") {
            WatchOutcome::Served(seen_at) => seen_at,
            WatchOutcome::LateRequest(path) => panic!(
                "request file observed only after the stop flag, not served: {}",
                path.display()
            ),
            WatchOutcome::NeverArrived => {
                panic!("request watcher saw no request file before the CLI exited")
            }
        }
    }
}

/// A test that panics before `finish()` must not leave a detached thread polling
/// a directory that `Tmp`'s drop is about to delete. Setting the flag retires
/// the worker within one poll interval. Drop does not join: the owner may
/// already be unwinding, and blocking there would hide the real failure.
impl Drop for RequestWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn spawn_request_watcher<F>(requests_dir: PathBuf, mut act: F) -> RequestWatcher
where
    F: FnMut(&Path, &Value) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let hard_cap = Instant::now() + WATCHER_HARD_CAP;
    let handle = thread::spawn(move || loop {
        // Read the stop flag BEFORE scanning, so the scan that follows a set
        // flag is a complete final scan of the state the CLI left behind.
        let stopped = worker_stop.load(Ordering::SeqCst);
        // An unreadable directory is treated as "no entries": after the owner is
        // gone, `Tmp` may already have deleted it, and an orphan worker must
        // retire quietly instead of panicking into a later test's output.
        let entries: Vec<PathBuf> = std::fs::read_dir(&requests_dir)
            .map(|dir| {
                dir.filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| {
                        path.extension().and_then(|extension| extension.to_str()) == Some("json")
                            && !path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| name.ends_with(".inflight.json"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(path) = entries.first() {
            // Acting once the flag has been observed would strand an
            // `.inflight.json` and break the `remaining.is_empty()` assertion.
            // Report the file instead.
            if stopped {
                return WatchOutcome::LateRequest(path.clone());
            }
            let seen_at = Instant::now();
            let request: Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            act(path, &request);
            return WatchOutcome::Served(seen_at);
        }
        if stopped || Instant::now() >= hard_cap {
            return WatchOutcome::NeverArrived;
        }
        thread::sleep(Duration::from_millis(10));
    });
    RequestWatcher {
        handle: Some(handle),
        stop,
    }
}

fn spawn_fake_responder<F>(bin: &Path, make_response: F) -> RequestWatcher
where
    F: Fn(&Value) -> Value + Send + 'static,
{
    let automation_dir = config_dir_for(bin).join("ui-automation");
    let responses_dir = automation_dir.join("responses");
    spawn_request_watcher(automation_dir.join("requests"), move |_path, request| {
        let request_id = request["requestId"].as_str().unwrap().to_string();
        let response = make_response(request);
        std::fs::write(
            responses_dir.join(format!("{request_id}.json")),
            serde_json::to_string(&response).unwrap(),
        )
        .unwrap();
    })
}

/// Renames the request to `<id>.inflight.json`, i.e. the frontend accepted it
/// but never answered. Used by the two timeout tests.
fn spawn_inflight_mover(bin: &Path, expected_action: &'static str) -> RequestWatcher {
    let requests_dir = config_dir_for(bin).join("ui-automation").join("requests");
    let mover_dir = requests_dir.clone();
    spawn_request_watcher(requests_dir, move |path, request| {
        assert_eq!(request["action"], expected_action);
        let request_id = request["requestId"].as_str().unwrap();
        std::fs::rename(path, mover_dir.join(format!("{request_id}.inflight.json"))).unwrap();
    })
}

fn assert_bounded_terminal_output(stdout: &str, stderr: &str) -> Value {
    assert_empty_output("stderr", stderr);
    assert!(
        stdout.len() < 4096,
        "terminal stdout should stay below 4096 bytes, got {}",
        stdout.len()
    );
    first_json(stdout)
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
fn normal_binary_refuses_ui_context_click_with_json_only_stdout() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-context-click-non-testable");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-context-click",
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
fn normal_binary_refuses_ui_terminal_with_bounded_json_and_silent_stderr() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-terminal-non-testable");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-terminal",
            "--window",
            "main",
            "--session-id",
            "session-a",
            "query",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    let parsed = assert_bounded_terminal_output(&stdout, &stderr);
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error"], "refusing_non_testeable_binary");
}

#[test]
fn fake_response_makes_ui_terminal_succeed_with_exact_request_and_target() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-terminal-fake-response");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let expected_target = json!({
        "sessionId": "session-a",
        "baseY": 83,
        "viewportY": 0,
        "length": 122,
        "cols": 122,
        "rows": 39,
        "type": "normal",
        "atBottom": false,
        "layoutPulse": {
            "version": 1,
            "requestId": 7,
            "sessionId": "session-a",
            "attachGeneration": 3,
            "status": "completed",
            "reason": "completed",
            "original": {
                "sidebarWidth": 440,
                "hostWidth": 900,
                "cols": 120,
                "rows": 39,
                "baselineObservedEpoch": null,
                "completedObserverAck": null
            },
            "expanded": {
                "sidebarWidth": 424,
                "hostWidth": 916,
                "cols": 122,
                "rows": 39,
                "baselineObservedEpoch": 10,
                "completedObserverAck": {
                    "epoch": 11,
                    "first": { "hostWidth": 916, "cols": 122, "rows": 39 },
                    "second": { "hostWidth": 916, "cols": 122, "rows": 39 }
                }
            },
            "restored": {
                "sidebarWidth": 440,
                "hostWidth": 900,
                "cols": 120,
                "rows": 39,
                "baselineObservedEpoch": 12,
                "completedObserverAck": {
                    "epoch": 13,
                    "first": { "hostWidth": 900, "cols": 120, "rows": 39 },
                    "second": { "hostWidth": 900, "cols": 120, "rows": 39 }
                }
            },
            "dwellMs": 208,
            "settingsWritesDelta": 0
        }
    });
    let responder_target = expected_target.clone();
    let responder = spawn_fake_responder(&bin, move |request| {
        assert_eq!(request["window"], "main");
        assert_eq!(request["action"], "terminal");
        assert_eq!(request["selector"], "terminal.session.session-a");
        assert_eq!(request["value"], "top");
        assert!(request["expiresAtUnixMs"].as_i64().is_some());
        json!({
            "ok": true,
            "requestId": request["requestId"],
            "window": "main",
            "action": "terminal",
            "selector": "terminal.session.session-a",
            "target": responder_target.clone()
        })
    });

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-terminal",
            "--window",
            "main",
            "--session-id",
            "session-a",
            "--timeout-ms",
            "3000",
            "top",
        ],
    );
    responder.finish();
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    let parsed = assert_bounded_terminal_output(&stdout, &stderr);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["target"], expected_target);
}

#[test]
fn ui_terminal_invalid_session_and_operation_create_no_request_file() {
    let _guard = test_lock();
    for (label, session_id, operation, expected) in [
        ("session", "", "query", "invalid_terminal_session_id"),
        (
            "operation",
            "session-a",
            "line:01",
            "invalid_terminal_operation",
        ),
    ] {
        let tmp = Tmp::new(&format!("ui-terminal-invalid-{label}"));
        let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
        let (code, stdout, stderr) = run(
            &bin,
            &[
                "ui-terminal",
                "--window",
                "main",
                "--session-id",
                session_id,
                operation,
            ],
        );
        assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
        let parsed = assert_bounded_terminal_output(&stdout, &stderr);
        assert_eq!(parsed["error"], expected);

        let requests_dir = config_dir_for(&bin).join("ui-automation").join("requests");
        let request_count = std::fs::read_dir(&requests_dir)
            .map(|entries| entries.filter_map(Result::ok).count())
            .unwrap_or(0);
        assert_eq!(request_count, 0, "invalid {label} created a request file");
    }
}

#[test]
fn ui_terminal_passes_frontend_failures_through_as_bounded_json() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    for error in [
        "target_hidden",
        "terminal_target_mismatch",
        "terminal_entry_stale",
        "terminal_session_not_visible",
        "terminal_controller_unavailable",
    ] {
        let tmp = Tmp::new(&format!("ui-terminal-{error}"));
        let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
        write_session(&bin, pid, &["main"]);
        let error_owned = error.to_string();
        let responder = spawn_fake_responder(&bin, move |request| {
            assert_eq!(request["action"], "terminal");
            json!({
                "ok": false,
                "requestId": request["requestId"],
                "window": "main",
                "action": "terminal",
                "selector": "terminal.session.session-a",
                "error": error_owned,
                "message": "terminal request failed"
            })
        });

        let (code, stdout, stderr) = run(
            &bin,
            &[
                "ui-terminal",
                "--session-id",
                "session-a",
                "--timeout-ms",
                "3000",
                "query",
            ],
        );
        responder.finish();
        assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
        let parsed = assert_bounded_terminal_output(&stdout, &stderr);
        assert_eq!(parsed["error"], error);
        assert_eq!(parsed["action"], "terminal");
        assert_eq!(parsed["selector"], "terminal.session.session-a");
    }
}

#[test]
fn ui_terminal_timeout_is_bounded_and_cleans_the_request() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-terminal-timeout");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let requests_dir = config_dir_for(&bin).join("ui-automation").join("requests");

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-terminal",
            "--session-id",
            "session-a",
            "--timeout-ms",
            "100",
            "query",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    let parsed = assert_bounded_terminal_output(&stdout, &stderr);
    assert_eq!(parsed["error"], "timeout");
    assert_eq!(parsed["phase"], "awaiting_gui_poller");
    assert!(
        std::fs::read_dir(requests_dir)
            .unwrap()
            .filter_map(Result::ok)
            .next()
            .is_none(),
        "terminal timeout should remove its request file"
    );
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

    let responder = spawn_fake_responder(&bin, move |request| {
        assert_eq!(request["window"], "main");
        assert_eq!(request["action"], "query");
        assert_eq!(request["selector"], "onboarding.confirm");
        let request_id = request["requestId"].as_str().unwrap();
        json!({
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
        })
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
    responder.finish();
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["target"]["testId"], "onboarding.confirm");
}

#[test]
fn fake_response_makes_ui_context_click_succeed() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-context-click-fake-response");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);

    let responder = spawn_fake_responder(&bin, move |request| {
        assert_eq!(request["window"], "main");
        assert_eq!(request["action"], "contextClick");
        assert_eq!(request["selector"], "project.loops.header.test");
        let request_id = request["requestId"].as_str().unwrap();
        json!({
            "ok": true,
            "requestId": request_id,
            "window": "main",
            "action": "contextClick",
            "selector": "project.loops.header.test",
            "target": {
                "testId": "project.loops.header.test",
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
        })
    });

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-context-click",
            "--window",
            "main",
            "--selector",
            "project.loops.header.test",
            "--timeout-ms",
            "3000",
        ],
    );
    responder.finish();
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["target"]["testId"], "project.loops.header.test");
}
