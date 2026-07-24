use serde_json::Value;
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
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        let path = std::env::temp_dir().join(format!(
            "ac-{}-{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            h.finish()
        ));
        std::fs::create_dir_all(&path).expect("create tmp dir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("make tmp dir private");
        }
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    support::copy_executable(src, &dst);
    dst
}

fn config_dir_for(bin: &Path) -> PathBuf {
    let stem = bin.file_stem().unwrap().to_string_lossy().to_string();
    bin.parent().unwrap().join(format!(".{}", stem))
}

fn output_with_exec_retry(command: &mut Command) -> std::io::Result<std::process::Output> {
    let mut attempts = 0;
    loop {
        match command.output() {
            #[cfg(target_os = "linux")]
            Err(error) if error.raw_os_error() == Some(26) && attempts < 20 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

fn run(bin: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = output_with_exec_retry(Command::new(bin).args(args)).expect("spawn binary");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn log_lines(bin: &Path) -> Vec<Value> {
    let path = config_dir_for(bin).join("logs").join("harness.log");
    let body = std::fs::read_to_string(path).expect("read harness log");
    body.lines()
        .map(|line| serde_json::from_str(line).expect("parse log line"))
        .collect()
}

#[test]
fn harness_argv_with_spaces_preserves_boundaries() {
    let tmp = Tmp::new("harness-argv");
    let bin = copy_binary_into(tmp.path());

    #[cfg(target_os = "windows")]
    let script = {
        let script = tmp.path().join("check-arg.bat");
        std::fs::write(
            &script,
            "@echo off\r\nif \"%~1\"==\"a b\" (exit /b 0) else (exit /b 9)\r\n",
        )
        .expect("write argv probe");
        script
    };

    #[cfg(target_os = "windows")]
    let script_arg = script.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    let args = ["harness", "--", script_arg.as_str(), "a b"];

    #[cfg(not(target_os = "windows"))]
    let args = [
        "harness",
        "--",
        "sh",
        "-c",
        "test \"$1\" = 'a b'",
        "sh",
        "a b",
    ];

    let (code, stdout, stderr) = run(&bin, &args);
    assert_eq!(code, Some(0), "stdout: {}\nstderr: {}", stdout, stderr);
}

#[test]
fn harness_raw_command_chains_execute_via_shell() {
    let tmp = Tmp::new("harness-raw");
    let bin = copy_binary_into(tmp.path());

    #[cfg(target_os = "windows")]
    let raw = "echo first && exit 0";
    #[cfg(not(target_os = "windows"))]
    let raw = "echo first && exit 0";

    let (code, stdout, stderr) = run(&bin, &["harness", "--raw-command", raw]);
    assert_eq!(code, Some(0), "stdout: {}\nstderr: {}", stdout, stderr);
    assert!(stdout.contains("first"), "stdout: {}", stdout);
}

#[test]
fn harness_secret_redaction_is_applied_in_logs() {
    let tmp = Tmp::new("harness-redact");
    let bin = copy_binary_into(tmp.path());
    let secret = "super-secret-token-value";
    let raw = format!("echo _authToken={}", secret);

    let (code, stdout, stderr) = run(&bin, &["harness", "--dry-run", "--raw-command", &raw]);
    assert_eq!(code, Some(0), "stdout: {}\nstderr: {}", stdout, stderr);

    let lines = log_lines(&bin);
    let command = lines[0]["command"].as_str().unwrap();
    assert!(command.contains("_authToken=[REDACTED]"));
    assert!(!command.contains(secret));
}

#[test]
fn harness_argv_secret_redaction_is_applied_in_logs() {
    let tmp = Tmp::new("harness-argv-redact");
    let bin = copy_binary_into(tmp.path());
    let secret = "secret-leak-check";

    let (code, stdout, stderr) = run(
        &bin,
        &["harness", "--dry-run", "--", "echo", "--token", secret],
    );
    assert_eq!(code, Some(0), "stdout: {}\nstderr: {}", stdout, stderr);

    let lines = log_lines(&bin);
    let command = lines[0]["command"].as_str().unwrap();
    assert!(command.contains("--token\u{1f}[REDACTED]"));
    assert!(!command.contains(secret));
}

#[test]
fn harness_log_uses_config_dir_logs_harness_log() {
    let tmp = Tmp::new("harness-log-path");
    let bin = copy_binary_into(tmp.path());
    let cfg_dir = config_dir_for(&bin);

    let (code, stdout, stderr) = run(&bin, &["harness", "--explain", "--raw-command", "echo hi"]);
    assert_eq!(code, Some(0), "stdout: {}\nstderr: {}", stdout, stderr);

    assert!(
        cfg_dir.join("logs").join("harness.log").exists(),
        "missing harness log under {}",
        cfg_dir.display()
    );
}

#[test]
fn harness_deny_exit_code_is_one() {
    let tmp = Tmp::new("harness-deny");
    let bin = copy_binary_into(tmp.path());

    let (code, stdout, stderr) = run(&bin, &["harness", "--raw-command", "echo ok && rm -rf /"]);
    assert_eq!(code, Some(1), "stdout: {}\nstderr: {}", stdout, stderr);
}

#[test]
fn harness_raw_destructive_flag_variants_are_denied() {
    let tmp = Tmp::new("harness-raw-deny-variants");
    let bin = copy_binary_into(tmp.path());

    for raw in [
        "rm -fr /",
        "rm -r -f /",
        "rd /q /s C:\\",
        "echo ok && rm -fr /",
        "echo ok\nrm -fr /",
        "Remove-Item -r -fo C:\\",
    ] {
        let (code, stdout, stderr) = run(&bin, &["harness", "--dry-run", "--raw-command", raw]);
        assert_eq!(
            code,
            Some(1),
            "raw: {}\nstdout: {}\nstderr: {}",
            raw,
            stdout,
            stderr
        );
        assert!(
            stderr.contains("harness: deny"),
            "raw: {}\nstdout: {}\nstderr: {}",
            raw,
            stdout,
            stderr
        );
    }
}

#[test]
fn harness_child_exit_code_is_propagated() {
    let tmp = Tmp::new("harness-exit");
    let bin = copy_binary_into(tmp.path());

    #[cfg(target_os = "windows")]
    let raw = "exit 7";
    #[cfg(not(target_os = "windows"))]
    let raw = "exit 7";

    let (code, stdout, stderr) = run(&bin, &["harness", "--raw-command", raw]);
    assert_eq!(code, Some(7), "stdout: {}\nstderr: {}", stdout, stderr);
}

#[test]
fn harness_dry_run_does_not_spawn_child() {
    let tmp = Tmp::new("harness-dry-run");
    let bin = copy_binary_into(tmp.path());
    let sentinel = tmp.path().join("sentinel.txt");

    #[cfg(target_os = "windows")]
    let raw = format!("echo created > \"{}\"", sentinel.display());
    #[cfg(not(target_os = "windows"))]
    let raw = format!("touch '{}'", sentinel.display());

    let (code, stdout, stderr) = run(&bin, &["harness", "--dry-run", "--raw-command", &raw]);
    assert_eq!(code, Some(0), "stdout: {}\nstderr: {}", stdout, stderr);
    assert!(!sentinel.exists(), "dry-run spawned child command");
}
