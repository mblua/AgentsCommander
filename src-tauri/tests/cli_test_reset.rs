use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

static TESTABLE_RESET_IDENTITY_LOCK: Mutex<()> = Mutex::new(());

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

fn testable_reset_identity_lock() -> MutexGuard<'static, ()> {
    TESTABLE_RESET_IDENTITY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn run(bin: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(bin).args(args).output().expect("spawn binary");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[cfg(target_os = "windows")]
mod reset_mutex_cross_process_harness {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, ChildStdin, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    const CONFIG_ENV: &str = "AGENTSCOMMANDER_TEST_RESET_MUTEX_HARNESS_CONFIG_V1";
    const EVENT_SENTINEL: &str = "AGENTSCOMMANDER_TEST_RESET_MUTEX_HARNESS_EVENT_V1";
    const DEADLINE: Duration = Duration::from_millis(10_000);
    const POLL: Duration = Duration::from_millis(10);
    const CANDIDATES: [&str; 2] = [".agentscommander_testeable", "agentscommander_testeable"];

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct HarnessEvent {
        version: u32,
        nonce: String,
        role: String,
        phase: String,
        value: serde_json::Value,
    }

    struct HarnessChild {
        child: Child,
        stdin: Option<ChildStdin>,
        events: Receiver<String>,
        stdout_reader: Option<JoinHandle<Result<(), String>>>,
        stderr_reader: Option<JoinHandle<Result<String, String>>>,
    }

    impl HarnessChild {
        fn spawn(bin: &Path, role: &str, nonce: &str, timeout_ms: u32) -> Self {
            let config = serde_json::json!({
                "version": 1,
                "role": role,
                "nonce": nonce,
                "timeoutMs": timeout_ms,
            })
            .to_string();
            let mut child = Command::new(bin)
                .args(["test-reset", "--confirm-testeable"])
                .env(CONFIG_ENV, config)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn reset mutex harness child");
            let stdin = child.stdin.take().expect("child stdin");
            let stdout = child.stdout.take().expect("child stdout");
            let stderr = child.stderr.take().expect("child stderr");
            let (send, events) = mpsc::channel();
            let stdout_reader = std::thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) => return Ok(()),
                        Ok(_) => send
                            .send(line)
                            .map_err(|error| format!("send child event: {error}"))?,
                        Err(error) => return Err(format!("read child stdout: {error}")),
                    }
                }
            });
            let stderr_reader = std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut text = String::new();
                reader
                    .read_to_string(&mut text)
                    .map_err(|error| format!("read child stderr: {error}"))?;
                Ok(text)
            });
            Self {
                child,
                stdin: Some(stdin),
                events,
                stdout_reader: Some(stdout_reader),
                stderr_reader: Some(stderr_reader),
            }
        }

        fn close_stdin(&mut self) {
            self.stdin.take();
        }

        fn send_holder_command(&mut self, nonce: &str, phase: &str) {
            let record = serde_json::json!({
                "version": 1,
                "role": "holder",
                "nonce": nonce,
                "phase": phase,
            })
            .to_string();
            assert!(record.len() < 256, "command record exceeds limit");
            let stdin = self.stdin.as_mut().expect("holder stdin remains open");
            stdin.write_all(record.as_bytes()).expect("write command");
            stdin.write_all(b"\n").expect("write command newline");
            stdin.flush().expect("flush command");
        }

        fn expect_event(&self, nonce: &str, role: &str, phase: &str, value: serde_json::Value) {
            let deadline = Instant::now() + DEADLINE;
            let line = loop {
                match self.events.try_recv() {
                    Ok(line) => break line,
                    Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                        std::thread::sleep(POLL);
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        panic!("timed out waiting for {role}/{phase}")
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        panic!("stdout closed before {role}/{phase}")
                    }
                }
            };
            assert!(line.ends_with('\n'), "event lacks LF framing: {line:?}");
            assert!(!line.ends_with("\r\n"), "event used CRLF framing");
            let line = line.strip_suffix('\n').expect("checked LF suffix");
            let json = line
                .strip_prefix(&format!("{EVENT_SENTINEL} "))
                .unwrap_or_else(|| panic!("invalid event sentinel: {line:?}"));
            let event: HarnessEvent = serde_json::from_str(json)
                .unwrap_or_else(|error| panic!("invalid event JSON {json:?}: {error}"));
            assert_eq!(event.version, 1);
            assert_eq!(event.nonce, nonce);
            assert_eq!(event.role, role);
            assert_eq!(event.phase, phase);
            assert_eq!(event.value, value);
        }

        fn wait_status(&mut self) -> std::process::ExitStatus {
            let deadline = Instant::now() + DEADLINE;
            loop {
                match self.child.try_wait().expect("poll child") {
                    Some(status) => return status,
                    None if Instant::now() < deadline => std::thread::sleep(POLL),
                    None => {
                        self.child.kill().expect("kill timed-out child");
                        let _ = self.child.wait();
                        panic!("child process deadline exceeded");
                    }
                }
            }
        }

        fn finish(mut self, expected_code: i32) {
            self.close_stdin();
            let status = self.wait_status();
            let stdout_result = self
                .stdout_reader
                .take()
                .expect("stdout reader")
                .join()
                .expect("join stdout reader");
            let stderr = self
                .stderr_reader
                .take()
                .expect("stderr reader")
                .join()
                .expect("join stderr reader")
                .expect("read stderr");
            stdout_result.expect("read stdout");
            let trailing: Vec<_> = self.events.try_iter().collect();
            assert!(
                trailing.is_empty(),
                "unexpected trailing events: {trailing:?}"
            );
            assert_eq!(status.code(), Some(expected_code), "stderr={stderr:?}");
            assert_harness_stderr(&stderr);
        }

        fn kill_and_reap(mut self) -> u32 {
            let pid = self.child.id();
            self.child.kill().expect("kill exact holder child");
            let _status = self.child.wait().expect("reap exact holder child");
            self.close_stdin();
            self.stdout_reader
                .take()
                .expect("stdout reader")
                .join()
                .expect("join stdout reader")
                .expect("read stdout");
            let _stderr = self
                .stderr_reader
                .take()
                .expect("stderr reader")
                .join()
                .expect("join stderr reader")
                .expect("read stderr");
            pid
        }
    }

    fn fresh_nonce() -> String {
        format!("n-{}", uuid::Uuid::new_v4().simple())
    }

    fn assert_harness_stderr(stderr: &str) {
        for line in stderr.lines() {
            assert!(
                line.starts_with("[log] file logging to "),
                "unexpected stderr line: {line:?}"
            );
        }
    }

    fn seed_candidates(root: &Path) {
        for candidate in CANDIDATES {
            let dir = root.join(candidate);
            std::fs::create_dir_all(&dir).expect("create reset candidate");
            std::fs::write(dir.join("marker.txt"), b"keep until reset")
                .expect("write reset marker");
        }
    }

    fn assert_candidates(root: &Path, expected: bool) {
        for candidate in CANDIDATES {
            assert_eq!(
                root.join(candidate).exists(),
                expected,
                "candidate {candidate} presence mismatch"
            );
        }
    }

    fn copy_harness_binary(root: &Path) -> PathBuf {
        let source = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
        let target = root.join("reset-mutex-harness-child.exe");
        std::fs::copy(source, &target).expect("copy harness child binary");
        target
    }

    #[test]
    fn reset_mutex_cross_process() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bin = copy_harness_binary(tmp.path());
        seed_candidates(tmp.path());

        let contention_nonce = fresh_nonce();
        let mut holder = HarnessChild::spawn(&bin, "holder", &contention_nonce, 5_000);
        holder.expect_event(
            &contention_nonce,
            "holder",
            "wait_started",
            serde_json::json!({"timeoutMs": 5_000}),
        );
        holder.expect_event(
            &contention_nonce,
            "holder",
            "acquired",
            serde_json::json!({"waitResult": "WAIT_OBJECT_0"}),
        );

        let mut timed_out = HarnessChild::spawn(&bin, "contender", &contention_nonce, 100);
        timed_out.close_stdin();
        timed_out.expect_event(
            &contention_nonce,
            "contender",
            "wait_started",
            serde_json::json!({"timeoutMs": 100}),
        );
        timed_out.expect_event(
            &contention_nonce,
            "contender",
            "timeout",
            serde_json::json!({
                "code": "reset_process_lock_timeout",
                "message": "timed out waiting for test-reset mutex",
                "raw": {"timeoutMs": 100},
            }),
        );
        timed_out.finish(2);
        assert_candidates(tmp.path(), true);

        holder.send_holder_command(&contention_nonce, "release");
        holder.expect_event(
            &contention_nonce,
            "holder",
            "released",
            serde_json::json!({}),
        );
        holder.send_holder_command(&contention_nonce, "exit");
        holder.close_stdin();
        holder.expect_event(
            &contention_nonce,
            "holder",
            "exited",
            serde_json::json!({"code": 0}),
        );
        holder.finish(0);

        let success_nonce = fresh_nonce();
        let mut contender = HarnessChild::spawn(&bin, "contender", &success_nonce, 5_000);
        contender.close_stdin();
        contender.expect_event(
            &success_nonce,
            "contender",
            "wait_started",
            serde_json::json!({"timeoutMs": 5_000}),
        );
        contender.expect_event(
            &success_nonce,
            "contender",
            "acquired",
            serde_json::json!({"waitResult": "WAIT_OBJECT_0"}),
        );
        contender.expect_event(
            &success_nonce,
            "contender",
            "released",
            serde_json::json!({}),
        );
        contender.expect_event(
            &success_nonce,
            "contender",
            "exited",
            serde_json::json!({"code": 0}),
        );
        contender.finish(0);
        assert_candidates(tmp.path(), false);

        seed_candidates(tmp.path());
        let abandoned_nonce = fresh_nonce();
        let abandoned_holder = HarnessChild::spawn(&bin, "holder", &abandoned_nonce, 100);
        abandoned_holder.expect_event(
            &abandoned_nonce,
            "holder",
            "wait_started",
            serde_json::json!({"timeoutMs": 100}),
        );
        abandoned_holder.expect_event(
            &abandoned_nonce,
            "holder",
            "acquired",
            serde_json::json!({"waitResult": "WAIT_OBJECT_0"}),
        );
        let mut abandoned = HarnessChild::spawn(&bin, "contender", &abandoned_nonce, 5_000);
        abandoned.close_stdin();
        abandoned.expect_event(
            &abandoned_nonce,
            "contender",
            "wait_started",
            serde_json::json!({"timeoutMs": 5_000}),
        );
        let killed_pid = abandoned_holder.kill_and_reap();
        assert_ne!(killed_pid, 0);
        abandoned.expect_event(
            &abandoned_nonce,
            "contender",
            "acquired",
            serde_json::json!({"waitResult": "WAIT_ABANDONED"}),
        );
        abandoned.expect_event(
            &abandoned_nonce,
            "contender",
            "released",
            serde_json::json!({}),
        );
        abandoned.expect_event(
            &abandoned_nonce,
            "contender",
            "exited",
            serde_json::json!({"code": 0}),
        );
        abandoned.finish(0);
        assert_candidates(tmp.path(), false);
    }
}

fn last_stdout_json(stdout: &str) -> Value {
    let line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("stdout json line");
    serde_json::from_str(line).expect("parse stdout json")
}

fn stderr_json(stderr: &str) -> Value {
    let line = stderr
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .expect("stderr json line");
    serde_json::from_str(line).expect("parse stderr json")
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

#[test]
fn missing_confirm_refuses() {
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-confirm");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let (code, stdout, stderr) = run(&bin, &["test-reset"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stderr_json(&stderr)["error"], "missing_confirm_testeable");
}

#[test]
fn non_testable_binary_refuses() {
    let tmp = Tmp::new("reset-non-testable");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        stderr_json(&stderr)["error"],
        "refusing_non_testeable_binary"
    );
}

#[test]
fn deletes_only_allowed_testable_directories() {
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-delete");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let config_dir = tmp.path().join(".agentscommander_testeable");
    let project_dir = tmp.path().join("agentscommander_testeable");
    let keep_dir = tmp.path().join(".agentscommander_other");
    std::fs::create_dir_all(config_dir.join("nested")).unwrap();
    std::fs::create_dir_all(project_dir.join("nested")).unwrap();
    std::fs::create_dir_all(&keep_dir).unwrap();

    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    let final_json = last_stdout_json(&stdout);
    assert_eq!(final_json["ok"], true);
    assert!(!config_dir.exists(), "config dir should be deleted");
    assert!(!project_dir.exists(), "project dir should be deleted");
    assert!(keep_dir.exists(), "unrelated dir should remain");
}

#[test]
fn file_target_refuses_and_deletes_nothing() {
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-file");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let config_path = tmp.path().join(".agentscommander_testeable");
    let project_dir = tmp.path().join("agentscommander_testeable");
    std::fs::write(&config_path, "not a dir").unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();

    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        stderr_json(&stderr)["error"],
        "reset_candidate_not_directory"
    );
    assert!(config_path.exists(), "file target should remain");
    assert!(
        project_dir.exists(),
        "other candidate should not be deleted after refusal"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn locked_file_refuses_and_reports_delete_plan() {
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-locked-file");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let config_dir = tmp.path().join(".agentscommander_testeable");
    let project_dir = tmp.path().join("agentscommander_testeable");
    let outside = tmp.path().join("outside-sentinel").join("keep.txt");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::create_dir_all(outside.parent().expect("outside parent")).unwrap();
    std::fs::write(&outside, "keep").unwrap();
    let locked = config_dir.join("locked.txt");
    std::fs::write(&locked, b"locked").unwrap();
    let _handle = open_without_delete_share(&locked);

    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    let err = stderr_json(&stderr);
    assert_eq!(err["error"], "remove_dir_all_failed");
    assert!(!err["message"].as_str().expect("message").is_empty());
    assert!(err["path"]
        .as_str()
        .expect("path")
        .contains(".agentscommander_testeable"));
    assert_eq!(err["exeParent"], tmp.path().to_string_lossy().as_ref());
    assert_eq!(
        err["plannedDelete"]
            .as_array()
            .expect("plannedDelete")
            .len(),
        2
    );
    assert!(config_dir.exists(), "locked config dir should remain");
    assert!(
        project_dir.exists(),
        "project dir should not be deleted after failure"
    );
    assert!(
        outside.is_file(),
        "outside sentinel should remain after reset failure"
    );
}

#[test]
fn long_path_target_deletes_only_allowed_directories() {
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-long-path");
    let mut base = tmp.path().to_path_buf();
    for idx in 0..10 {
        base = base.join(format!("long-segment-{idx:02}-abcdef"));
    }
    if let Err(e) = std::fs::create_dir_all(&base) {
        println!(
            "skipping long path reset regression; see docs/testing/destructive-filesystem-regression.md#reset-long-path-check: {}",
            e
        );
        return;
    }
    let bin = copy_binary_as(&base, "agentscommander_testeable.exe");
    if let Err(e) = Command::new(&bin).arg("--help").output() {
        println!(
            "skipping long path reset regression; see docs/testing/destructive-filesystem-regression.md#reset-long-path-check: {}",
            e
        );
        return;
    }
    let config_dir = base.join(".agentscommander_testeable");
    let project_dir = base.join("agentscommander_testeable");
    let keep_dir = base.join(".agentscommander_other");
    std::fs::create_dir_all(config_dir.join("nested")).unwrap();
    std::fs::create_dir_all(project_dir.join("nested")).unwrap();
    std::fs::create_dir_all(&keep_dir).unwrap();

    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    let plan = stdout
        .lines()
        .find(|line| line.contains("\"plannedDelete\""))
        .map(|line| serde_json::from_str::<Value>(line).expect("plan json"))
        .expect("plan json line");
    assert_eq!(
        plan["plannedDelete"]
            .as_array()
            .expect("plannedDelete")
            .len(),
        2
    );
    assert!(!config_dir.exists(), "config dir should be deleted");
    assert!(!project_dir.exists(), "project dir should be deleted");
    assert!(keep_dir.exists(), "unrelated dir should remain");
}

#[cfg(unix)]
#[test]
fn symlink_target_refuses_and_deletes_nothing() {
    use std::os::unix::fs::symlink;

    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-symlink");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let real = tmp.path().join("real-dir");
    let link = tmp.path().join(".agentscommander_testeable");
    let project_dir = tmp.path().join("agentscommander_testeable");
    std::fs::create_dir_all(&real).unwrap();
    symlink(&real, &link).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();

    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stderr_json(&stderr)["error"], "reset_candidate_is_symlink");
    assert!(link.exists(), "symlink should remain");
    assert!(real.exists(), "symlink target should remain");
    assert!(
        project_dir.exists(),
        "other candidate should not be deleted after refusal"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn held_testable_mutex_refuses_and_deletes_nothing() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-active-mutex");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let config_dir = tmp.path().join(".agentscommander_testeable");
    std::fs::create_dir_all(&config_dir).unwrap();

    let mutex_name: Vec<u16> = "Local\\AgentsCommander_SingleInstance_Testeable\0"
        .encode_utf16()
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    assert!(!handle.is_null(), "failed to create mutex");

    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    unsafe {
        let _ = CloseHandle(handle);
    }
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stderr_json(&stderr)["error"], "testable_gui_active");
    assert!(config_dir.exists(), "active GUI refusal should not delete");
}

#[cfg(target_os = "windows")]
#[test]
fn junction_target_refuses_and_deletes_nothing() {
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-junction");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let real = tmp.path().join("real-dir");
    let junction = tmp.path().join(".agentscommander_testeable");
    let project_dir = tmp.path().join("agentscommander_testeable");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();

    if let Err(e) = create_junction(&junction, &real) {
        println!(
            "skipping junction reset regression; see docs/testing/destructive-filesystem-regression.md#reset-junction-reparse-check: {}",
            e
        );
        return;
    }

    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        stderr_json(&stderr)["error"],
        "reset_candidate_is_reparse_point"
    );
    assert!(junction.exists(), "junction should remain");
    assert!(real.exists(), "junction target should remain");
    assert!(
        project_dir.exists(),
        "other candidate should not be deleted after refusal"
    );
}
