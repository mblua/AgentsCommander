use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

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
    support::copy_executable(src, &dst);
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

fn run_with_env(bin: &Path, env_value: &str) -> (Option<i32>, String, String) {
    let out = Command::new(bin)
        .env("AC_TEST_WINDOW_PLACEMENT", env_value)
        .output()
        .expect("spawn binary");
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
