//! Helpers for `issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start`.

use std::fs::OpenOptions;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(super) const CHILD_SENTINEL: &str = "AGENTSCOMMANDER_ISSUE_1577_CHILD";
const TEST_NAME: &str =
    "tests::startup::issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start";

pub(super) fn assert_child_refuses_startup(
    summary_owner: &crate::startup::StartupMigrationSummary,
) {
    let executable = std::env::current_exe().unwrap();
    let case_root = executable.parent().unwrap();
    let adjacent = case_root.join(".agentscommander_issue1577_linux_subprocess");

    assert_eq!(crate::config::config_dir(), Some(adjacent.clone()));
    let Some(crate::config::ConfigStartupError::AdjacentDirectoryUnwritable { config_dir, reason }) =
        crate::config::config_startup_error()
    else {
        panic!("an unwritable unmarked adjacent directory must refuse startup");
    };
    assert_eq!(config_dir, adjacent);
    let reason_prefix = format!(
        "write probe could not create configuration directory \"{}\" after 1 attempt(s): ",
        adjacent.display()
    );
    assert!(
        reason.starts_with(&reason_prefix),
        "unexpected reason: {reason}"
    );
    let refusal =
        crate::preflight_config_startup(summary_owner).expect_err("startup must be refused");
    assert_eq!(
        refusal.to_string(),
        format!(
            "AgentsCommander cannot start because it cannot write its configuration \
                directory \"{}\" next to the executable: {}{} Move the executable to a \
                writable folder, or set AGENTSCOMMANDER_CONFIG_DIR to a writable directory, \
                and restart.",
            adjacent.display(),
            reason,
            if reason.ends_with('.') { "" } else { "." }
        )
    );
}

pub(super) fn prepare_unwritable_case_root(
    case_root: &Path,
    copied_executable: &Path,
    adjacent: &Path,
    original_mode: u32,
) -> Result<(), String> {
    let source_executable =
        std::env::current_exe().map_err(|error| format!("current_exe failed: {error}"))?;
    std::fs::copy(&source_executable, copied_executable).map_err(|error| {
        format!(
            "copy {} -> {} failed: {error}",
            source_executable.display(),
            copied_executable.display()
        )
    })?;
    let mut executable_permissions = std::fs::metadata(copied_executable)
        .map_err(|error| format!("copied executable metadata failed: {error}"))?
        .permissions();
    executable_permissions.set_mode(executable_permissions.mode() | 0o111);
    std::fs::set_permissions(copied_executable, executable_permissions)
        .map_err(|error| format!("set executable mode failed: {error}"))?;

    if adjacent.exists() {
        return Err(format!("fixture not fresh adjacent={}", adjacent.display()));
    }

    let mut read_only_permissions = std::fs::metadata(case_root)
        .map_err(|error| format!("case-root metadata failed: {error}"))?
        .permissions();
    read_only_permissions.set_mode(0o555);
    std::fs::set_permissions(case_root, read_only_permissions)
        .map_err(|error| format!("chmod 0555 {} failed: {error}", case_root.display()))?;

    let fixture_probe = case_root.join("fixture-write-preflight.tmp");
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&fixture_probe)
    {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(error) => {
            return Err(format!(
                "0555 preflight returned {:?} instead of PermissionDenied for {}: {error}",
                error.kind(),
                fixture_probe.display()
            ));
        }
        Ok(file) => {
            drop(file);
            let mut restored = std::fs::metadata(case_root)
                .map_err(|error| format!("restore metadata failed: {error}"))?
                .permissions();
            restored.set_mode(original_mode);
            std::fs::set_permissions(case_root, restored)
                .map_err(|error| format!("restore after false preflight failed: {error}"))?;
            std::fs::remove_file(&fixture_probe)
                .map_err(|error| format!("remove false-positive fixture probe failed: {error}"))?;
            return Err(format!(
                "mode 0555 did not block create_new in {}; refusing false pass",
                case_root.display()
            ));
        }
    }

    Ok(())
}

pub(super) fn run_child(
    copied_executable: &Path,
    case_root: &Path,
    home_root: &Path,
) -> Result<Output, String> {
    let mut child = Command::new(copied_executable)
        .arg(TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env("HOME", home_root)
        .env_remove("AGENTSCOMMANDER_CONFIG_DIR")
        .env_remove("AGENTSCOMMANDER_TEST_CONFIG_DIR")
        .env(CHILD_SENTINEL, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!(
                "spawn copied child {} failed: {error}",
                copied_executable.display()
            )
        })?;

    let (terminal_status, status_poll_error) = wait_for_exit(&mut child, Duration::from_secs(15));

    if terminal_status.is_none() {
        let kill_error = child.kill().err();
        let reaped = wait_for_exit(&mut child, Duration::from_secs(5))
            .0
            .is_some();
        if !reaped {
            return Err(format!(
                "child did not reap within 5 seconds; kill_error={kill_error:?} \
                    poll_error={status_poll_error:?} case_root={} home_root={}",
                case_root.display(),
                home_root.display()
            ));
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("timed-out child final reap failed: {error}"))?;
        return Err(format!(
            "child did not reach a clean terminal status; kill_error={kill_error:?} \
                poll_error={status_poll_error:?} status={:?} stdout={:?} stderr={:?} \
                case_root={} home_root={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            case_root.display(),
            home_root.display()
        ));
    }

    child
        .wait_with_output()
        .map_err(|error| format!("child output collection failed: {error}"))
}

pub(super) fn verify_child_output(
    output: &Output,
    case_root: &Path,
    home_root: &Path,
    copied_executable: &Path,
    adjacent: &Path,
) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "child failed status={:?} stdout={stdout:?} stderr={stderr:?} case_root={} \
                home_root={}",
            output.status.code(),
            case_root.display(),
            home_root.display()
        ));
    }
    for text in [&*stdout, &*stderr] {
        if text.contains("panicked at") || text.contains("stack backtrace:") {
            return Err(format!(
                "child emitted panic/backtrace text stdout={stdout:?} stderr={stderr:?}"
            ));
        }
    }

    let home_entries: Vec<_> = std::fs::read_dir(home_root)
        .map_err(|error| format!("read home failed: {error}"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()
        .map_err(|error| format!("read home entry failed: {error}"))?;
    if !home_entries.is_empty() {
        return Err(format!(
            "a suffixed build wrote into HOME: {home_entries:?}"
        ));
    }
    if adjacent.exists() {
        return Err(format!(
            "adjacent state appeared adjacent={}",
            adjacent.display()
        ));
    }
    let case_entries: Vec<_> = std::fs::read_dir(case_root)
        .map_err(|error| format!("read case root failed: {error}"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()
        .map_err(|error| format!("read case-root entry failed: {error}"))?;
    if case_entries
        != [copied_executable
            .file_name()
            .expect("copied executable file name")
            .to_os_string()]
    {
        return Err(format!("unexpected case-root entries: {case_entries:?}"));
    }
    Ok(())
}

pub(super) fn restore_case_root_mode(case_root: &Path, original_mode: u32) -> Result<(), String> {
    let mut permissions = std::fs::metadata(case_root)
        .map_err(|error| format!("mode-restore metadata failed: {error}"))?
        .permissions();
    permissions.set_mode(original_mode);
    std::fs::set_permissions(case_root, permissions).map_err(|error| {
        format!(
            "restore exact mode on {} failed: {error}",
            case_root.display()
        )
    })
}

/// Polls `child` until it exits, `timeout` passes, or polling fails.
fn wait_for_exit(
    child: &mut Child,
    timeout: Duration,
) -> (Option<ExitStatus>, Option<std::io::Error>) {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (Some(status), None),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => return (None, None),
            Err(error) => return (None, Some(error)),
        }
    }
}
