use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::{spawn_lock, Mode, CHILD_TIMEOUT, REAP_TIMEOUT};

pub(crate) fn inject_injected_home(command: &mut Command, mode: Mode, home: &Path) {
    if mode == Mode::InjectedHome {
        command.env("HOME", home);
    }
}

pub(crate) fn strip_inherited_config_env(command: &mut Command) {
    command
        .env_remove("AGENTSCOMMANDER_CONFIG_DIR")
        .env_remove("AGENTSCOMMANDER_TEST_CONFIG_DIR")
        .env_remove("AC_MACHINE_OUTPUT");
}

pub(crate) fn copy_product_binary(bin_dir: &Path) -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "agentscommander.exe"
    } else {
        "agentscommander"
    };
    let source = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let destination = bin_dir.join(name);
    {
        let _guard = spawn_lock();
        fs::copy(source, &destination).map_err(|error| {
            format!(
                "copy {} -> {} failed: {error}",
                source.display(),
                destination.display()
            )
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&destination)
            .map_err(|error| format!("copied binary metadata failed: {error}"))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(&destination, permissions)
            .map_err(|error| format!("set executable mode failed: {error}"))?;
    }
    Ok(destination)
}

pub(crate) struct ChildRun {
    pub(crate) status: Option<i32>,
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn run_child(command: &mut Command, label: &str) -> Result<ChildRun, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = {
        let _guard = spawn_lock();
        command
            .spawn()
            .map_err(|error| format!("spawn child for {label} failed: {error}"))?
    };
    let deadline = Instant::now() + CHILD_TIMEOUT;
    let mut status_poll_error = None;
    let terminal_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => break None,
            Err(error) => {
                status_poll_error = Some(error);
                break None;
            }
        }
    };
    if terminal_status.is_none() {
        let kill_error = child.kill().err();
        let reap_deadline = Instant::now() + REAP_TIMEOUT;
        let reaped = loop {
            match child.try_wait() {
                Ok(Some(_)) => break true,
                Ok(None) if Instant::now() < reap_deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) | Err(_) => break false,
            }
        };
        if !reaped {
            return Err(format!(
                "{label}: child did not reap within 5 seconds; kill_error={kill_error:?} \
                 poll_error={status_poll_error:?}"
            ));
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("{label}: timed-out child final reap failed: {error}"))?;
        return Err(format!(
            "{label}: child did not reach a clean terminal status; \
             kill_error={kill_error:?} poll_error={status_poll_error:?} \
             status={:?} stdout={:?} stderr={:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("{label}: child output collection failed: {error}"))?;
    Ok(ChildRun {
        status: output.status.code(),
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub(crate) fn print_retained(label: &str, run: &ChildRun) {
    println!("[child {label}] status={:?}", run.status);
    for line in run.stdout.lines() {
        println!("[child {label} stdout] {line}");
    }
    for line in run.stderr.lines() {
        println!("[child {label} stderr] {line}");
    }
}
