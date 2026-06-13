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
