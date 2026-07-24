//! Daemon PID file: written by the daemon at startup, removed on graceful exit.
//! Used by CLI verbs (`list-sessions`) to detect whether `sessions.json` is
//! authoritative or stale. See #231.
//!
//! Format: a single line containing the daemon's PID as a u32, no whitespace.

use std::path::{Path, PathBuf};

fn pid_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("daemon.pid"))
}

/// Write the current process PID to `daemon.pid`. Idempotent (overwrites).
/// Called once at daemon startup, after `config_dir` has been resolved.
pub fn write_pid_file() -> Result<(), crate::errors::StartupError> {
    let path = pid_path().ok_or_else(|| crate::errors::StartupError::MissingConfigDir {
        executable: super::current_executable(),
    })?;
    let payload = std::process::id().to_string();

    #[cfg(target_os = "linux")]
    {
        let root = super::linux_state::prepared_secure_config_root("write daemon PID")?;
        root.atomic_publish(
            std::ffi::OsStr::new("daemon.pid"),
            payload.as_bytes(),
            "write daemon PID",
        )?;
    }

    #[cfg(not(target_os = "linux"))]
    {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                crate::errors::StartupError::io("create daemon PID directory", parent, source)
            })?;
        }
        std::fs::write(&path, payload.as_bytes())
            .map_err(|source| crate::errors::StartupError::io("write daemon PID", &path, source))?;
    }
    log::info!(
        "[daemon-pid] Wrote pid {} to {:?}",
        std::process::id(),
        path
    );
    Ok(())
}

/// Remove the pid file on graceful shutdown. Best-effort.
pub fn remove_pid_file() {
    let Some(path) = pid_path() else {
        return;
    };
    #[cfg(target_os = "linux")]
    let result = super::linux_state::prepared_secure_config_root("remove daemon PID")
        .and_then(|root| {
            root.unlink_private_file_if_present(
                std::ffi::OsStr::new("daemon.pid"),
                "remove daemon PID",
            )
            .map(|_| ())
        })
        .map_err(|error| error.to_string());
    #[cfg(not(target_os = "linux"))]
    let result = std::fs::remove_file(&path)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|error| error.to_string());
    if let Err(error) = result {
        log::warn!(
            "[daemon-pid] Failed to remove pid file at {}: {}",
            path.display(),
            error
        );
    }
}

/// State of the daemon as observable from the CLI side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonState {
    /// PID file present and the PID corresponds to a live process (OR a process
    /// we lack rights to query — see `is_pid_alive` `ACCESS_DENIED` handling
    /// for the elevated-daemon / non-elevated-CLI case).
    Running { pid: u32 },
    /// PID file missing — no daemon has started, or it was force-killed and
    /// the file was cleaned by a prior CLI invocation. (We do NOT delete the
    /// stale pid file from the CLI — that would race a daemon coming up.)
    NoPidFile,
    /// PID file present but the PID does not correspond to a live process.
    StalePidFile { pid: u32 },
    /// PID file present but malformed (not a parseable u32).
    MalformedPidFile,
}

/// Public entry point: resolves and validates the production PID marker.
/// True absence returns `NoPidFile`; Linux config-resolution or marker-safety
/// failures stay typed, while non-Linux keeps the legacy missing-config result.
pub fn detect_daemon_state() -> Result<DaemonState, crate::errors::StartupError> {
    let Some(contents) = super::runtime_files::read_daemon_pid()? else {
        return Ok(DaemonState::NoPidFile);
    };
    Ok(detect_daemon_state_from_contents(&contents))
}

/// Path-parameterized inner detector. Test against this with a `tempfile::TempDir`
/// so unit tests do NOT touch the live binary's `<config_dir>/daemon.pid`
/// (writing to the live path would corrupt a running daemon's signal for the
/// rest of its lifetime, race parallel test runs, and pollute the developer's
/// home dir on machines where AC is not installed).
pub fn detect_daemon_state_at(path: &Path) -> DaemonState {
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return DaemonState::NoPidFile,
    };
    detect_daemon_state_from_contents(&contents)
}

fn detect_daemon_state_from_contents(contents: &str) -> DaemonState {
    let pid: u32 = match contents.trim().parse() {
        Ok(n) => n,
        Err(_) => return DaemonState::MalformedPidFile,
    };
    if is_pid_alive(pid) {
        DaemonState::Running { pid }
    } else {
        DaemonState::StalePidFile { pid }
    }
}

#[cfg(target_os = "windows")]
fn is_pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, FALSE, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: All three Win32 calls receive scalar arguments only (a raw u32
    // PID, the BOOL constant FALSE, and feature-flag constants); no pointers
    // into our address space are aliased, no buffers are shared across calls.
    // The handle returned by OpenProcess is owned by this function: it is
    // consumed by GetExitCodeProcess and then closed by CloseHandle before we
    // return. `code` is a stack u32 written by GetExitCodeProcess; no escape.
    // GetLastError is thread-local.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if handle.is_null() {
            // Distinguish "process does not exist" from "access denied".
            // Elevated daemon + non-elevated CLI is a real scenario; treating
            // ACCESS_DENIED as dead would emit a false stale warning and tempt
            // the user toward `taskkill`. Conservative choice: assume alive
            // when we cannot tell, so the warning is silenced.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code: u32 = 0;
        let got_code = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        // STILL_ACTIVE (259) means the process is running. Any other code
        // means it has exited — treat the pid as dead even if the handle
        // opened (could be a zombie kept alive by another handle holder).
        got_code != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(target_os = "windows"))]
fn is_pid_alive(_pid: u32) -> bool {
    // Preserve the pre-#1111 non-Windows behavior. PID liveness is only a
    // quality-of-life warning and is not part of the Linux secure-state
    // authority decision.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        // On Windows, OpenProcess against our own PID succeeds and
        // GetExitCodeProcess returns STILL_ACTIVE. On non-Windows, the stub
        // returns true unconditionally — assertion still holds.
        assert!(is_pid_alive(std::process::id()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn extremely_high_pid_is_dead_on_windows() {
        // A 32-bit max-value pid is essentially guaranteed not to be a live
        // process. The Windows path returns false. The non-Windows stub would
        // return true (it ignores its argument), so this assertion is
        // Windows-only.
        assert!(!is_pid_alive(u32::MAX));
    }

    #[test]
    fn missing_pid_file_yields_no_pid_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("daemon.pid");
        // Do NOT create the file.
        assert_eq!(detect_daemon_state_at(&path), DaemonState::NoPidFile);
    }

    #[test]
    fn malformed_pid_file_detected_against_tempdir() {
        // This test MUST NOT write to the live binary's
        // <config_dir>/daemon.pid (`pid_path()`). Use a TempDir and the
        // path-parameterized `detect_daemon_state_at` instead.
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("daemon.pid");
        std::fs::write(&path, "not-a-pid").unwrap();
        assert_eq!(detect_daemon_state_at(&path), DaemonState::MalformedPidFile);
    }

    #[test]
    fn living_pid_file_yields_running_against_tempdir() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("daemon.pid");
        std::fs::write(&path, std::process::id().to_string()).unwrap();
        match detect_daemon_state_at(&path) {
            DaemonState::Running { pid } => assert_eq!(pid, std::process::id()),
            other => panic!("expected Running, got {:?}", other),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dead_pid_file_yields_stale_against_tempdir_on_windows() {
        // Same Windows-only caveat as `extremely_high_pid_is_dead_on_windows`.
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("daemon.pid");
        std::fs::write(&path, u32::MAX.to_string()).unwrap();
        match detect_daemon_state_at(&path) {
            DaemonState::StalePidFile { pid } => assert_eq!(pid, u32::MAX),
            other => panic!("expected StalePidFile, got {:?}", other),
        }
    }
}
