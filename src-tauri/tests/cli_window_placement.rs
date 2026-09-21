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

fn config_dir_for_bin(bin: &Path) -> PathBuf {
    let stem = bin.file_stem().expect("bin stem").to_string_lossy();
    bin.parent().expect("bin parent").join(format!(".{stem}"))
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
    {
        let _guard = spawn_lock();
        std::fs::copy(src, &dst).expect("copy binary");
    }
    dst
}

fn run(bin: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run_with_env(bin: &Path, env_value: &str) -> (Option<i32>, String, String) {
    let mut command = command_for_binary(bin);
    command
        .env("AC_TEST_WINDOW_PLACEMENT", env_value)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn normal_binary_refuses_cli_placement_input() {
    let tmp = Tmp::new("placement-prod-cli");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "--app",
            "--window-x",
            "-1",
            "--window-y",
            "0",
            "--window-width",
            "100",
            "--window-height",
            "100",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("test_placement_requires_testeable_binary"));
}

#[test]
fn workgroup_binary_refuses_cli_placement_input() {
    let tmp = Tmp::new("placement-wg-cli");
    let bin = copy_binary_as(tmp.path(), "agentscommander_wg1-dev-team.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "--app",
            "--window-x",
            "-1",
            "--window-y",
            "0",
            "--window-width",
            "100",
            "--window-height",
            "100",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("test_placement_requires_testeable_binary"));
}

#[test]
fn normal_binary_refuses_env_placement_input_before_parsing() {
    let tmp = Tmp::new("placement-prod-env");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run_with_env(&bin, "not-json");
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("test_placement_requires_testeable_binary"));
    assert!(!stderr.contains("invalid_AC_TEST_WINDOW_PLACEMENT_json"));
}

#[test]
fn workgroup_binary_refuses_env_placement_input_before_parsing() {
    let tmp = Tmp::new("placement-wg-env");
    let bin = copy_binary_as(tmp.path(), "agentscommander_wg1-dev-team.exe");
    let (code, stdout, stderr) = run_with_env(&bin, "not-json");
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("test_placement_requires_testeable_binary"));
    assert!(!stderr.contains("invalid_AC_TEST_WINDOW_PLACEMENT_json"));
}

#[test]
fn testable_binary_reports_malformed_env() {
    let tmp = Tmp::new("placement-testable-env");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let (code, stdout, stderr) = run_with_env(&bin, "not-json");
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("invalid_AC_TEST_WINDOW_PLACEMENT_json"));
}

#[test]
fn window_maximized_alone_counts_as_testable_only_input() {
    let tmp = Tmp::new("placement-maximized");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(&bin, &["--app", "--window-maximized"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("test_placement_requires_testeable_binary"));
}
