#![deny(clippy::undocumented_unsafe_blocks)]

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
            let child = Command::new(bin)
                .args(["test-reset", "--confirm-testeable"])
                .env(CONFIG_ENV, config)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn reset mutex harness child");
            Self::from_child(child)
        }

        /// Section 3 coordinator children: exact argv and a cleared environment
        /// carrying only the canonical tuple. Section 4.2 replaces the internals of
        /// this one runner with Win32 named pipes; the surface stays as it is.
        fn spawn_exact(exe: &Path, args: &[&str], env: &[(&str, String)]) -> Self {
            let mut command = Command::new(exe);
            command.args(args);
            command.env_clear();
            for (name, value) in env {
                command.env(name, value);
            }
            let child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn coordinator child");
            Self::from_child(child)
        }

        fn from_child(mut child: Child) -> Self {
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

        /// Reaps the child and returns its exit code with the raw stdout and stderr.
        /// Both reader threads are joined before the stdout lines are reassembled, so
        /// nothing is left buffered in the channel.
        fn finish_raw(mut self) -> (Option<i32>, String, String) {
            self.close_stdin();
            let status = self.wait_status();
            self.stdout_reader
                .take()
                .expect("stdout reader")
                .join()
                .expect("join stdout reader")
                .expect("read stdout");
            let stderr = self
                .stderr_reader
                .take()
                .expect("stderr reader")
                .join()
                .expect("join stderr reader")
                .expect("read stderr");
            let stdout: String = self.events.try_iter().collect();
            (status.code(), stdout, stderr)
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

    /// Section 3: launches the ignored helper through the same runner, with the exact
    /// argv and an environment cleared down to the canonical tuple.
    pub fn run_coordinator_child(
        nonce: &str,
        role: &str,
        expect: &str,
        wait_ms: u32,
    ) -> (Option<i32>, String, String) {
        use super::windows as coord;

        let exe = std::fs::canonicalize(std::env::current_exe().expect("current_exe"))
            .expect("canonicalize current_exe");
        let env = [
            (coord::ENV_PROTOCOL, "1".to_string()),
            (coord::ENV_NONCE, nonce.to_string()),
            (coord::ENV_ROLE, role.to_string()),
            (coord::ENV_EXPECT, expect.to_string()),
            (coord::ENV_WAIT_MS, wait_ms.to_string()),
        ];
        HarnessChild::spawn_exact(
            &exe,
            &[
                "--ignored",
                "--exact",
                coord::TEST_NAME,
                "--nocapture",
                "--test-threads=1",
            ],
            &env,
        )
        .finish_raw()
    }

    fn copy_harness_binary(root: &Path) -> PathBuf {
        let source = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
        let target = root.join("reset-mutex-harness-child.exe");
        std::fs::copy(source, &target).expect("copy harness child binary");
        target
    }

    #[test]
    fn reset_mutex_cross_process() {
        let suite = super::coordinator::acquire_suite();
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
        suite.complete().expect("suite complete");
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
    #[cfg(windows)]
    let suite = coordinator::acquire_suite();
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-confirm");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let (code, stdout, stderr) = run(&bin, &["test-reset"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stderr_json(&stderr)["error"], "missing_confirm_testeable");
    #[cfg(windows)]
    suite.complete().expect("suite complete");
}

#[test]
fn non_testable_binary_refuses() {
    #[cfg(windows)]
    let suite = coordinator::acquire_suite();
    let tmp = Tmp::new("reset-non-testable");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        stderr_json(&stderr)["error"],
        "refusing_non_testeable_binary"
    );
    #[cfg(windows)]
    suite.complete().expect("suite complete");
}

#[test]
fn deletes_only_allowed_testable_directories() {
    #[cfg(windows)]
    let suite = coordinator::acquire_suite();
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
    #[cfg(windows)]
    suite.complete().expect("suite complete");
}

#[test]
fn file_target_refuses_and_deletes_nothing() {
    #[cfg(windows)]
    let suite = coordinator::acquire_suite();
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
    #[cfg(windows)]
    suite.complete().expect("suite complete");
}

#[cfg(target_os = "windows")]
#[test]
fn locked_file_refuses_and_reports_delete_plan() {
    let suite = coordinator::acquire_suite();
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
    suite.complete().expect("suite complete");
}

#[test]
fn long_path_target_deletes_only_allowed_directories() {
    #[cfg(windows)]
    let suite = coordinator::acquire_suite();
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
    #[cfg(windows)]
    suite.complete().expect("suite complete");
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

    let suite = coordinator::acquire_suite();
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-active-mutex");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let config_dir = tmp.path().join(".agentscommander_testeable");
    std::fs::create_dir_all(&config_dir).unwrap();

    let mutex_name: Vec<u16> = "Local\\AgentsCommander_SingleInstance_Testeable\0"
        .encode_utf16()
        .collect();
    // SAFETY: `mutex_name` is a NUL-terminated UTF-16 buffer (the trailing `\0` is
    // part of the literal) that outlives the call, and a null attributes pointer
    // selects the default security descriptor. The returned handle is owned here and
    // closed below.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    assert!(!handle.is_null(), "failed to create mutex");

    let (code, stdout, stderr) = run(&bin, &["test-reset", "--confirm-testeable"]);
    // SAFETY: `handle` is the non-null handle asserted above, closed exactly once and
    // never used again. `initial_owner = 0` means this thread never owned the mutex,
    // so no release is owed.
    unsafe {
        let _ = CloseHandle(handle);
    }
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stderr_json(&stderr)["error"], "testable_gui_active");
    assert!(config_dir.exists(), "active GUI refusal should not delete");
    suite.complete().expect("suite complete");
}

#[cfg(target_os = "windows")]
#[test]
fn junction_target_refuses_and_deletes_nothing() {
    let suite = coordinator::acquire_suite();
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
    suite.complete().expect("suite complete");
}

#[cfg(target_os = "windows")]
#[test]
fn reset_test_coordinator_private_handoff() {
    use reset_mutex_cross_process_harness::run_coordinator_child;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex};

    let suite = coordinator::acquire_suite();

    // Both run under the same suite hold: the synthetic parser cases of section 3 and
    // the zero-depth probe of section 2.
    windows::assert_env_parser_rejects_before_open();
    coordinator::assert_zero_recursion_depth();

    // The private probe is created owned, so the timeout child cannot take it.
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("{}{nonce}", windows::PROBE_PREFIX);
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call, and a
    // null attributes pointer selects the default security descriptor.
    // `initial_owner = 1` makes this thread the owner, which is the point of the
    // probe: the timeout child must not be able to take it. The handle is released
    // and closed below, both on this same thread.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide.as_ptr()) };
    // SAFETY: `GetLastError` takes no arguments and only reads this thread's
    // last-error value, set by the call above. It is read unconditionally here
    // because `ERROR_ALREADY_EXISTS` is a success-path code that proves the nonce
    // name was not fresh.
    let create_last_error = unsafe { GetLastError() };
    assert!(
        !handle.is_null(),
        "private probe handle: GetLastError={create_last_error}"
    );
    assert_ne!(
        create_last_error, ERROR_ALREADY_EXISTS,
        "the nonce name must be fresh"
    );

    let (code, stdout, stderr) = run_coordinator_child(
        &nonce,
        windows::ROLE_TIMEOUT,
        windows::EXPECT_TIMEOUT,
        windows::WAIT_MS_TIMEOUT,
    );
    assert_eq!(
        code,
        Some(0),
        "timeout child\nstdout={stdout}\nstderr={stderr}"
    );
    windows::assert_libtest_envelope(&stdout);
    windows::assert_records(
        &windows::parse_coord_records(&stderr),
        &nonce,
        windows::ROLE_TIMEOUT,
        windows::WAIT_MS_TIMEOUT,
    );

    // Only after that child is validated and reaped: release, but do not close.
    // SAFETY: `handle` is the non-null probe handle asserted above, still open, and
    // owned by this thread since `CreateMutexW` took ownership on it. This is the
    // only release, and it runs on the owning thread, which is what `ReleaseMutex`
    // requires. The handle stays open on purpose so the acquire child contends for
    // the same kernel object rather than a recreated one.
    let released = unsafe { ReleaseMutex(handle) };
    // SAFETY: `GetLastError` takes no arguments and only reads this thread's
    // last-error value, set by the release above.
    let release_error = unsafe { GetLastError() };
    assert_ne!(released, 0, "parent release: GetLastError={release_error}");

    let (code, stdout, stderr) = run_coordinator_child(
        &nonce,
        windows::ROLE_ACQUIRE,
        windows::EXPECT_ACQUIRED,
        windows::WAIT_MS_ACQUIRE,
    );
    assert_eq!(
        code,
        Some(0),
        "acquire child\nstdout={stdout}\nstderr={stderr}"
    );
    windows::assert_libtest_envelope(&stdout);
    windows::assert_records(
        &windows::parse_coord_records(&stderr),
        &nonce,
        windows::ROLE_ACQUIRE,
        windows::WAIT_MS_ACQUIRE,
    );

    // Only after that child is validated and reaped: close.
    // SAFETY: `handle` is the probe handle, already released above and not used
    // after this point, so it is closed exactly once.
    let closed = unsafe { CloseHandle(handle) };
    // SAFETY: `GetLastError` takes no arguments and only reads this thread's
    // last-error value, set by the close above.
    let close_error = unsafe { GetLastError() };
    assert_ne!(closed, 0, "parent close: GetLastError={close_error}");

    suite.complete().expect("suite complete");
}

// ---------------------------------------------------------------------------
// #1343 section 2: suite guard for the Windows matrix.
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod coordinator {
    use std::marker::PhantomData;
    use std::rc::Rc;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

    /// Fixed suite name. Every active Windows test in this binary serialises on it.
    pub const SUITE_NAME: &str = "Local\\AgentsCommander.Ac1343.CliTestReset.Suite.v1";
    pub const SUITE_WAIT_MS: u32 = 120_000;
    /// Emitted by `Drop` when best-effort cleanup fails. Any appearance rejects the
    /// run even when libtest reports green.
    pub const CLEANUP_FAILED: &str = "AC1343_SUITE_CLEANUP_FAILED";

    #[derive(Debug, PartialEq, Eq)]
    pub enum SuiteAcquireError {
        Create { last_error: u32 },
        Wait { raw: u32 },
        Complete { message: String },
    }

    /// The `Rc` marker makes the guard `!Send + !Sync`, so `ReleaseMutex` can only
    /// run on the thread that owns the mutex.
    pub struct ResetTestCoordinator {
        handle: HANDLE,
        owned: bool,
        handle_open: bool,
        // Section 2 requires waitCount and releaseCount to be recorded.
        wait_count: u32,
        release_count: u32,
        _not_send: PhantomData<Rc<()>>,
    }

    pub fn acquire_suite() -> ResetTestCoordinator {
        match try_acquire(SUITE_NAME, SUITE_WAIT_MS, false) {
            Ok(guard) => guard,
            Err(error) => panic!("suite acquire failed: {error:?}"),
        }
    }

    /// The single parameterised constructor. The suite uses it with the fixed name
    /// and `initial_owner=false`; the zero-depth probe reuses it with a nonce name
    /// and flips only that flag for its negative subcase.
    pub fn try_acquire(
        name: &str,
        wait_ms: u32,
        initial_owner: bool,
    ) -> Result<ResetTestCoordinator, SuiteAcquireError> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call, and
        // a null attributes pointer selects the default security descriptor. This is
        // the guard's only `CreateMutexW`, and the guard built below owns whatever
        // handle it returns, on every path including the failed wait.
        let handle =
            unsafe { CreateMutexW(std::ptr::null(), i32::from(initial_owner), wide.as_ptr()) };
        // SAFETY: `GetLastError` takes no arguments and only reads this thread's
        // last-error value. It is captured in the statement immediately after the
        // create so nothing can overwrite it before the null check below.
        let last_error = unsafe { GetLastError() };
        if handle.is_null() {
            return Err(SuiteAcquireError::Create { last_error });
        }
        // Constructed with owned=false and handle_open=true before any wait, so Drop
        // owns the handle from this point on whatever the wait returns.
        let mut guard = ResetTestCoordinator {
            handle,
            owned: false,
            handle_open: true,
            wait_count: 0,
            release_count: 0,
            _not_send: PhantomData,
        };
        // SAFETY: `guard.handle` is the non-null handle checked above and the guard
        // that owns it is alive across the call, so the handle cannot be closed
        // underneath the wait. `wait_ms` is finite, so this cannot block forever, and
        // `WaitForSingleObject` only reads the handle.
        let raw = unsafe { WaitForSingleObject(guard.handle, wait_ms) };
        guard.wait_count += 1;
        if raw == WAIT_OBJECT_0 || raw == WAIT_ABANDONED {
            guard.owned = true;
            Ok(guard)
        } else {
            // Any other result closes the handle; dropping the guard does exactly that
            // once, because owned stays false.
            drop(guard);
            Err(SuiteAcquireError::Wait { raw })
        }
    }

    impl ResetTestCoordinator {
        pub fn wait_count(&self) -> u32 {
            self.wait_count
        }

        pub fn complete(mut self) -> Result<(), String> {
            let mut primary = None;
            if self.owned {
                // SAFETY: `owned` is set only for `WAIT_OBJECT_0` or `WAIT_ABANDONED`,
                // so the mutex really is held here, and it is cleared immediately
                // below so this release happens at most once. The `PhantomData<Rc<()>>`
                // marker makes the guard `!Send`, so it cannot have travelled off the
                // thread that took ownership in `try_acquire`: `ReleaseMutex` is
                // therefore called on the owning thread, which is its requirement.
                let released = unsafe { ReleaseMutex(self.handle) };
                // SAFETY: `GetLastError` takes no arguments and only reads this
                // thread's last-error value, set by the release above.
                let last_error = unsafe { GetLastError() };
                self.release_count += 1;
                self.owned = false;
                if released == 0 {
                    primary = Some(format!("ReleaseMutex failed: GetLastError={last_error}"));
                }
            }
            let mut secondary = None;
            if self.handle_open {
                // SAFETY: `handle_open` is set at construction and cleared immediately
                // below, so the handle is closed at most once and is not used after.
                let closed = unsafe { CloseHandle(self.handle) };
                // SAFETY: `GetLastError` takes no arguments and only reads this
                // thread's last-error value, set by the close above.
                let last_error = unsafe { GetLastError() };
                self.handle_open = false;
                if closed == 0 {
                    secondary = Some(format!("CloseHandle failed: GetLastError={last_error}"));
                }
            }
            if primary.is_none() && secondary.is_none() {
                return Ok(());
            }
            // Both errors are preserved, together with the recorded counts.
            Err(format!(
                "suite complete failed: primary={primary:?} secondary={secondary:?} \
                 waitCount={} releaseCount={}",
                self.wait_count, self.release_count
            ))
        }
    }

    impl Drop for ResetTestCoordinator {
        fn drop(&mut self) {
            // Best effort, no panic, and no second operation: `complete` leaves both
            // flags false, so this is a no-op on the normal path.
            if self.owned {
                // SAFETY: same invariant as `complete`. `owned` is still true only if
                // no `complete` ran, the mutex is held, and the `!Send` marker means
                // this drop is on the owning thread. `owned` is cleared below, so the
                // release happens at most once even though `complete` may have run a
                // partial cleanup before failing.
                let released = unsafe { ReleaseMutex(self.handle) };
                // SAFETY: `GetLastError` takes no arguments and only reads this
                // thread's last-error value, set by the release above.
                let last_error = unsafe { GetLastError() };
                self.release_count += 1;
                self.owned = false;
                if released == 0 {
                    eprintln!(
                        "{CLEANUP_FAILED} ReleaseMutex GetLastError={last_error} releaseCount={}",
                        self.release_count
                    );
                }
            }
            if self.handle_open {
                // SAFETY: `handle_open` is still true only if nothing closed the handle
                // yet, and it is cleared below, so the handle is closed at most once
                // across `complete` and this drop combined.
                let closed = unsafe { CloseHandle(self.handle) };
                // SAFETY: `GetLastError` takes no arguments and only reads this
                // thread's last-error value, set by the close above.
                let last_error = unsafe { GetLastError() };
                self.handle_open = false;
                if closed == 0 {
                    eprintln!(
                        "{CLEANUP_FAILED} CloseHandle GetLastError={last_error} waitCount={}",
                        self.wait_count
                    );
                }
            }
        }
    }

    /// Acquires and completes on a fresh thread. The guard is `!Send`, so it is built
    /// and finished entirely inside that thread; running the second acquirer off the
    /// creating thread is what makes the depth observable at all, because a thread
    /// that already owns a mutex re-enters it without waiting.
    fn acquire_on_fresh_thread(name: &str, wait_ms: u32) -> Result<u32, SuiteAcquireError> {
        let owned_name = name.to_string();
        std::thread::spawn(move || {
            let guard = try_acquire(&owned_name, wait_ms, false)?;
            let waits = guard.wait_count();
            guard
                .complete()
                .map_err(|message| SuiteAcquireError::Complete { message })?;
            Ok(waits)
        })
        .join()
        .expect("join second acquirer")
    }

    /// Section 2 zero-depth probe. Both subcases are structurally identical and differ
    /// only in the `initial_owner` flag, which is exactly the FALSE/TRUE seam.
    pub fn assert_zero_recursion_depth() {
        for initial_owner in [false, true] {
            let name = format!(
                "Local\\AgentsCommander.Ac1343.CliTestReset.Depth.v1.{}",
                uuid::Uuid::new_v4().simple()
            );
            // First creator, then a second anchor handle that keeps the name alive
            // after the first guard closes its own handle.
            let first = try_acquire(&name, SUITE_WAIT_MS, initial_owner)
                .unwrap_or_else(|error| panic!("first acquirer ({initial_owner}): {error:?}"));
            assert_eq!(first.wait_count(), 1, "exactly one wait");

            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call,
            // and a null attributes pointer selects the default security descriptor.
            // `initial_owner = 0` means this handle never takes ownership: it only
            // keeps the named object alive after the first guard closes its own
            // handle. It is closed at the end of the iteration.
            let anchor = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
            assert!(!anchor.is_null(), "anchor handle");

            first
                .complete()
                .unwrap_or_else(|error| panic!("first complete ({initial_owner}): {error}"));

            let second = acquire_on_fresh_thread(&name, 5_000);
            if initial_owner {
                // TRUE leaves one recursion level held by the creating thread.
                assert_eq!(
                    second,
                    Err(SuiteAcquireError::Wait {
                        raw: windows_sys::Win32::Foundation::WAIT_TIMEOUT
                    }),
                    "initial_owner=true must leave the second acquirer in WAIT_TIMEOUT"
                );
            } else {
                assert_eq!(
                    second,
                    Ok(1),
                    "initial_owner=false must let the second acquirer take it with one wait"
                );
            }

            // SAFETY: `anchor` is the non-null handle asserted above, never owned by
            // this thread and closed exactly once per iteration, after the second
            // acquirer has already been joined.
            unsafe {
                let _ = CloseHandle(anchor);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// #1343 section 3: private mutex, child helper and protocol.
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub mod windows {
    use std::io::Write;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

    pub const RECORD_SENTINEL: &str = "AC1343_COORD_V1";
    pub const PROBE_PREFIX: &str = "Local\\AgentsCommander.Ac1343.CliTestReset.Probe.v1.";
    pub const TEST_NAME: &str = "windows::reset_test_coordinator_child";

    pub const ENV_PROTOCOL: &str = "AC1343_COORD_PROTOCOL";
    pub const ENV_NONCE: &str = "AC1343_COORD_NONCE";
    pub const ENV_ROLE: &str = "AC1343_COORD_ROLE";
    pub const ENV_EXPECT: &str = "AC1343_COORD_EXPECT";
    pub const ENV_WAIT_MS: &str = "AC1343_COORD_WAIT_MS";
    const ENV_KNOWN: [&str; 5] = [ENV_PROTOCOL, ENV_NONCE, ENV_ROLE, ENV_EXPECT, ENV_WAIT_MS];

    pub const ROLE_TIMEOUT: &str = "timeout-probe";
    pub const ROLE_ACQUIRE: &str = "acquire";
    pub const EXPECT_TIMEOUT: &str = "timeout";
    pub const EXPECT_ACQUIRED: &str = "acquired";
    pub const WAIT_MS_TIMEOUT: u32 = 250;
    pub const WAIT_MS_ACQUIRE: u32 = 5_000;

    /// Closed key set of the stderr protocol. Field order is the declaration order,
    /// and `deny_unknown_fields` makes the parent reject any extra key.
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct CoordRecord {
        pub protocol: u32,
        pub nonce: String,
        pub role: String,
        pub pid: u32,
        pub event: String,
        pub mutex_name: String,
        pub wait_ms: u32,
        pub create_last_error: Option<u32>,
        pub wait_result: Option<String>,
        pub wait_raw: Option<u32>,
        pub acquired: bool,
        pub owned: bool,
        pub released: bool,
        pub handle_open: bool,
        pub closed: bool,
        pub cleanup: String,
        pub win32_error: Option<u32>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CoordConfig {
        pub nonce: String,
        pub role: String,
        pub expect: String,
        pub wait_ms: u32,
    }

    /// Pure parser over every environment entry. It runs before any Win32 call, so a
    /// rejection here provably precedes the mutex open.
    pub fn parse_coord_env(entries: &[(String, String)]) -> Result<CoordConfig, String> {
        let mut seen: [Option<&str>; 5] = [None; 5];
        for (name, value) in entries {
            if name.is_empty()
                || !name.is_ascii()
                || !value.is_ascii()
                || name.contains('\0')
                || value.contains('\0')
            {
                return Err(format!("AC1343_ENV_NON_ASCII_OR_NUL {name:?}"));
            }
            let index = ENV_KNOWN
                .iter()
                .position(|known| known.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("AC1343_ENV_UNKNOWN {name:?}"))?;
            if name != ENV_KNOWN[index] {
                return Err(format!("AC1343_ENV_CASE_ALIAS {name:?}"));
            }
            if seen[index].is_some() {
                return Err(format!("AC1343_ENV_DUPLICATE {name:?}"));
            }
            seen[index] = Some(value.as_str());
        }
        let take = |index: usize| -> Result<&str, String> {
            seen[index].ok_or_else(|| format!("AC1343_ENV_MISSING {}", ENV_KNOWN[index]))
        };
        let protocol = take(0)?;
        if protocol != "1" {
            return Err(format!("AC1343_ENV_VALUE {ENV_PROTOCOL}={protocol:?}"));
        }
        let nonce = take(1)?;
        if nonce.len() != 32
            || !nonce
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(format!("AC1343_ENV_VALUE {ENV_NONCE}={nonce:?}"));
        }
        let role = take(2)?;
        let expect = take(3)?;
        let wait_ms_text = take(4)?;
        let wait_ms: u32 = wait_ms_text
            .parse()
            .map_err(|_| format!("AC1343_ENV_VALUE {ENV_WAIT_MS}={wait_ms_text:?}"))?;
        // Exactly two admissible tuples; the role fixes the other two values.
        let admissible = match role {
            ROLE_TIMEOUT => (EXPECT_TIMEOUT, WAIT_MS_TIMEOUT),
            ROLE_ACQUIRE => (EXPECT_ACQUIRED, WAIT_MS_ACQUIRE),
            other => return Err(format!("AC1343_ENV_VALUE {ENV_ROLE}={other:?}")),
        };
        if expect != admissible.0 || wait_ms != admissible.1 {
            return Err(format!(
                "AC1343_ENV_TUPLE role={role:?} expect={expect:?} waitMs={wait_ms}"
            ));
        }
        Ok(CoordConfig {
            nonce: nonce.to_string(),
            role: role.to_string(),
            expect: expect.to_string(),
            wait_ms,
        })
    }

    fn emit(record: &CoordRecord) {
        let json = serde_json::to_string(record).expect("serialize coord record");
        assert!(!json.contains('\r'), "record must not contain CR");
        let line = format!("{RECORD_SENTINEL} {json}\n");
        let stderr = std::io::stderr();
        let mut locked = stderr.lock();
        locked
            .write_all(line.as_bytes())
            .expect("write coord record");
        locked.flush().expect("flush coord record");
    }

    /// The child body. `open_calls` is incremented at the exact point the mutex is
    /// opened, so synthetic rejection cases can assert it stayed at zero.
    pub fn child_protocol(
        entries: &[(String, String)],
        open_calls: &mut u32,
    ) -> Result<(), String> {
        let config = parse_coord_env(entries)?;
        let mutex_name = format!("{PROBE_PREFIX}{}", config.nonce);
        let started = CoordRecord {
            protocol: 1,
            nonce: config.nonce.clone(),
            role: config.role.clone(),
            pid: std::process::id(),
            event: "started".to_string(),
            mutex_name: mutex_name.clone(),
            wait_ms: config.wait_ms,
            create_last_error: None,
            wait_result: None,
            wait_raw: None,
            acquired: false,
            owned: false,
            released: false,
            handle_open: false,
            closed: false,
            cleanup: "pending".to_string(),
            win32_error: None,
        };
        emit(&started);

        let wide: Vec<u16> = mutex_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        *open_calls += 1;
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call, and
        // a null attributes pointer selects the default security descriptor. This is
        // the child's only open of the name, counted by `open_calls`, and
        // `initial_owner = 0` means the wait below is the only way to take ownership.
        let handle: HANDLE = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        // SAFETY: `GetLastError` takes no arguments and only reads this thread's
        // last-error value. It is captured in the statement immediately after the
        // create, because `ERROR_ALREADY_EXISTS` here is the proof that the parent's
        // named object was opened rather than a fresh one created.
        let create_last_error = unsafe { GetLastError() };
        if handle.is_null() {
            return Err(format!("CreateMutexW returned NULL: {create_last_error}"));
        }
        if create_last_error != ERROR_ALREADY_EXISTS {
            // SAFETY: `handle` is non-null per the check above, was never owned by this
            // thread, and this early return is the only path that reaches the close, so
            // it happens exactly once.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(format!(
                "expected ERROR_ALREADY_EXISTS ({ERROR_ALREADY_EXISTS}), observed {create_last_error}"
            ));
        }

        // SAFETY: `handle` is the non-null handle checked above and nothing closes it
        // before this point, so it is still open for the wait. `config.wait_ms` is
        // finite and validated by the harness parser, so this cannot block forever.
        let wait_raw = unsafe { WaitForSingleObject(handle, config.wait_ms) };
        // WAIT_ABANDONED is not admissible here: the parent always releases cleanly.
        let (wait_result, acquired) = match (config.role.as_str(), wait_raw) {
            (ROLE_TIMEOUT, WAIT_TIMEOUT) => ("WAIT_TIMEOUT", false),
            (ROLE_ACQUIRE, WAIT_OBJECT_0) => ("WAIT_OBJECT_0", true),
            (role, raw) => {
                // SAFETY: `handle` is still open on this off-contract path, and the
                // early return means it is closed exactly once. The arm is only
                // reached for wait results that did not grant ownership, so no
                // release is owed.
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(format!("role {role:?} observed wait {raw:#010x}"));
            }
        };

        let mut released = false;
        let mut win32_error = None;
        if acquired {
            // SAFETY: `acquired` is true only for `WAIT_OBJECT_0` on the wait above, so
            // this thread owns the mutex, and the whole child body runs on the single
            // libtest thread the child is launched with (`--test-threads=1`), so the
            // release is on the owning thread. This branch runs at most once.
            let ok = unsafe { ReleaseMutex(handle) };
            // SAFETY: `GetLastError` takes no arguments and only reads this thread's
            // last-error value, set by the release above.
            let last_error = unsafe { GetLastError() };
            released = ok != 0;
            if !released {
                win32_error = Some(last_error);
            }
        }
        // SAFETY: `handle` is still open on this path, since the two early returns
        // above are the only other closes and both leave the function, so this closes
        // it exactly once and it is not used afterwards.
        let closed_ok = unsafe { CloseHandle(handle) } != 0;
        if !closed_ok && win32_error.is_none() {
            // SAFETY: `GetLastError` takes no arguments and only reads this thread's
            // last-error value, set by the failed close above.
            win32_error = Some(unsafe { GetLastError() });
        }

        emit(&CoordRecord {
            event: "completed".to_string(),
            create_last_error: Some(create_last_error),
            wait_result: Some(wait_result.to_string()),
            wait_raw: Some(wait_raw),
            acquired,
            owned: false,
            released,
            handle_open: false,
            closed: closed_ok,
            cleanup: if win32_error.is_none() {
                "ok".to_string()
            } else {
                "failed".to_string()
            },
            win32_error,
            ..started
        });

        match win32_error {
            None => Ok(()),
            Some(code) => Err(format!("child cleanup failed: GetLastError={code}")),
        }
    }

    #[test]
    #[ignore = "spawned by reset_test_coordinator_private_handoff"]
    fn reset_test_coordinator_child() {
        // Lossy conversion turns any non-UTF-8 name or value into a non-ASCII
        // character, which the parser then rejects.
        let entries: Vec<(String, String)> = std::env::vars_os()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let mut open_calls = 0_u32;
        if let Err(error) = child_protocol(&entries, &mut open_calls) {
            panic!("{error}");
        }
    }

    /// The child's stdout must be the closed libtest envelope and nothing else.
    pub fn assert_libtest_envelope(stdout: &str) {
        assert!(!stdout.contains('\r'), "stdout must not contain CR");
        let significant: Vec<&str> = stdout.split('\n').filter(|l| !l.is_empty()).collect();
        assert_eq!(significant.len(), 3, "envelope lines: {significant:?}");
        assert_eq!(significant[0], "running 1 test");
        assert_eq!(significant[1], format!("test {TEST_NAME} ... ok"));
        let prefix = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; \
                      10 filtered out; finished in ";
        let summary = significant[2];
        assert!(summary.starts_with(prefix), "summary: {summary:?}");
        assert!(summary.ends_with('s'), "summary: {summary:?}");
    }

    /// The child's stderr is reserved to exactly two protocol records.
    pub fn parse_coord_records(stderr: &str) -> Vec<CoordRecord> {
        assert!(!stderr.contains('\r'), "stderr must not contain CR");
        assert!(stderr.ends_with('\n'), "stderr must end with LF");
        let body = stderr.strip_suffix('\n').expect("checked LF");
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(lines.len(), 2, "exactly two records: {lines:?}");
        lines
            .iter()
            .map(|line| {
                let json = line
                    .strip_prefix(&format!("{RECORD_SENTINEL} "))
                    .unwrap_or_else(|| panic!("bad record sentinel: {line:?}"));
                serde_json::from_str(json)
                    .unwrap_or_else(|error| panic!("bad record {json:?}: {error}"))
            })
            .collect()
    }

    pub fn assert_records(records: &[CoordRecord], nonce: &str, role: &str, wait_ms: u32) {
        let name = format!("{PROBE_PREFIX}{nonce}");
        let started = &records[0];
        let completed = &records[1];

        assert_eq!(started.protocol, 1);
        assert_eq!(started.nonce, nonce);
        assert_eq!(started.role, role);
        assert_eq!(started.event, "started");
        assert_eq!(started.mutex_name, name);
        assert_eq!(started.wait_ms, wait_ms);
        // `started` precedes every Win32 call: null results, false flags, pending.
        assert_eq!(started.create_last_error, None);
        assert_eq!(started.wait_result, None);
        assert_eq!(started.wait_raw, None);
        assert!(!started.acquired && !started.owned && !started.released);
        assert!(!started.handle_open && !started.closed);
        assert_eq!(started.cleanup, "pending");
        assert_eq!(started.win32_error, None);
        assert_ne!(started.pid, std::process::id(), "child runs out of process");

        assert_eq!(completed.protocol, 1);
        assert_eq!(
            completed.pid, started.pid,
            "both records share the child pid"
        );
        assert_eq!(completed.nonce, nonce);
        assert_eq!(completed.role, role);
        assert_eq!(completed.event, "completed");
        assert_eq!(completed.mutex_name, name);
        assert_eq!(completed.wait_ms, wait_ms);
        assert_eq!(completed.create_last_error, Some(ERROR_ALREADY_EXISTS));
        assert!(!completed.owned && !completed.handle_open);
        assert!(completed.closed);
        assert_eq!(completed.cleanup, "ok");
        assert_eq!(completed.win32_error, None);
        match role {
            ROLE_TIMEOUT => {
                assert_eq!(completed.wait_result.as_deref(), Some("WAIT_TIMEOUT"));
                assert_eq!(completed.wait_raw, Some(WAIT_TIMEOUT));
                assert!(!completed.acquired && !completed.released);
            }
            ROLE_ACQUIRE => {
                assert_eq!(completed.wait_result.as_deref(), Some("WAIT_OBJECT_0"));
                assert_eq!(completed.wait_raw, Some(WAIT_OBJECT_0));
                assert!(completed.acquired && completed.released);
            }
            other => panic!("unexpected role {other:?}"),
        }
    }

    type EnvEntries = Vec<(String, String)>;

    /// Synthetic parser cases. Every one must reject, and every one must leave the
    /// open-callback counter at zero.
    pub fn assert_env_parser_rejects_before_open() {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let valid: EnvEntries = vec![
            (ENV_PROTOCOL.to_string(), "1".to_string()),
            (ENV_NONCE.to_string(), nonce.clone()),
            (ENV_ROLE.to_string(), ROLE_TIMEOUT.to_string()),
            (ENV_EXPECT.to_string(), EXPECT_TIMEOUT.to_string()),
            (ENV_WAIT_MS.to_string(), WAIT_MS_TIMEOUT.to_string()),
        ];
        assert!(
            parse_coord_env(&valid).is_ok(),
            "the canonical tuple must parse"
        );

        let mutate = |edit: &dyn Fn(&mut EnvEntries)| -> EnvEntries {
            let mut entries = valid.clone();
            edit(&mut entries);
            entries
        };
        let cases: Vec<(&str, EnvEntries)> = vec![
            (
                "lower-alias",
                mutate(&|e| e[2].0 = ENV_ROLE.to_ascii_lowercase()),
            ),
            (
                "mixed-alias",
                mutate(&|e| e[2].0 = "Ac1343_Coord_Role".to_string()),
            ),
            (
                "duplicate",
                mutate(&|e| e.push((ENV_ROLE.to_string(), ROLE_TIMEOUT.to_string()))),
            ),
            (
                "unknown",
                mutate(&|e| e.push(("AC1343_COORD_EXTRA".to_string(), "1".to_string()))),
            ),
            ("nul-in-value", mutate(&|e| e[1].1.push('\0'))),
            ("non-ascii-name", mutate(&|e| e[2].0.push('ñ'))),
            (
                "missing",
                mutate(&|e| {
                    e.remove(4);
                }),
            ),
            ("bad-protocol", mutate(&|e| e[0].1 = "2".to_string())),
            ("bad-nonce", mutate(&|e| e[1].1 = "NOTHEX".to_string())),
            (
                "uppercase-nonce",
                mutate(&|e| e[1].1 = nonce.to_ascii_uppercase()),
            ),
            ("bad-role", mutate(&|e| e[2].1 = "holder".to_string())),
            (
                "tuple-mismatch",
                mutate(&|e| e[4].1 = WAIT_MS_ACQUIRE.to_string()),
            ),
            ("bad-wait-ms", mutate(&|e| e[4].1 = "250ms".to_string())),
        ];
        for (label, entries) in cases {
            let mut open_calls = 0_u32;
            let outcome = child_protocol(&entries, &mut open_calls);
            assert!(outcome.is_err(), "synthetic case {label} must reject");
            assert_eq!(open_calls, 0, "synthetic case {label} opened a mutex");
        }
    }
}
