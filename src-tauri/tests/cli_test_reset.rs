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
#[ignore = "Manual smoke: requires Windows junction creation support in the test runner"]
fn junction_target_refuses_and_deletes_nothing() {
    let _guard = testable_reset_identity_lock();
    let tmp = Tmp::new("reset-junction");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let real = tmp.path().join("real-dir");
    let junction = tmp.path().join(".agentscommander_testeable");
    let project_dir = tmp.path().join("agentscommander_testeable");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();

    let status = Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            junction.to_str().unwrap(),
            real.to_str().unwrap(),
        ])
        .status()
        .expect("create junction");
    assert!(status.success(), "mklink /J failed");

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
