//! Daemon PID file: written by the daemon at startup, removed on graceful exit.
//! Used by CLI verbs (`list-sessions`) to detect whether `sessions.json` is
//! authoritative or stale. See #231.
//!
//! Format: a single line containing the daemon's PID as a u32, no whitespace.

use std::path::{Path, PathBuf};

use crate::filesystem::daemon_pid as pid_io;

fn pid_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("daemon.pid"))
}

/// Write the current process PID to `daemon.pid`. Idempotent (overwrites).
/// Called once at daemon startup, after `config_dir` has been resolved.
pub fn write_pid_file() {
    if let Some(path) = pid_path() {
        // Best-effort write. If the daemon can't write the pid file (perms?),
        // log it but do not abort startup — the CLI just won't get its warning.
        if let Some(parent) = path.parent() {
            let _ = pid_io::ensure_pid_parent(parent);
        }
        if let Err(e) = pid_io::write_pid_document(&path, std::process::id().to_string()) {
            log::warn!("[daemon-pid] Failed to write pid file at {:?}: {}", path, e);
        } else {
            log::info!(
                "[daemon-pid] Wrote pid {} to {:?}",
                std::process::id(),
                path
            );
        }
    }
}

/// Remove the pid file on graceful shutdown. Best-effort.
pub fn remove_pid_file() {
    if let Some(path) = pid_path() {
        let _ = pid_io::remove_pid_document(&path);
    }
}

/// State of the daemon as observable from the CLI side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonState {
    /// PID file present and the PID corresponds to a live process (OR a process
    /// we lack rights to query — see `is_pid_alive` `ACCESS_DENIED` handling
    /// for the elevated-daemon / non-elevated-CLI case). On Unix, `EPERM` from
    /// `kill(pid, 0)` is treated the same way.
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

/// Public entry point — resolves the pid path from `config_dir()` and
/// delegates to `detect_daemon_state_at`. Returns `NoPidFile` if `config_dir`
/// is unavailable (no home dir).
pub fn detect_daemon_state() -> DaemonState {
    match pid_path() {
        Some(p) => detect_daemon_state_at(&p),
        None => DaemonState::NoPidFile,
    }
}

/// Path-parameterized inner detector. Test against this with a `tempfile::TempDir`
/// so unit tests do NOT touch the live binary's `<config_dir>/daemon.pid`
/// (writing to the live path would corrupt a running daemon's signal for the
/// rest of its lifetime, race parallel test runs, and pollute the developer's
/// home dir on machines where AC is not installed).
pub fn detect_daemon_state_at(path: &Path) -> DaemonState {
    let contents = match pid_io::read_pid_document(path) {
        Ok(s) => s,
        Err(_) => return DaemonState::NoPidFile,
    };
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

/// #2394: Unix liveness shares the #2382 probe (`kill(pid, 0)`, EPERM = alive,
/// Linux zombie = dead, pid 0 and non-`pid_t` values = dead). `testability`
/// is compiled in every build, so this is not a test-only dependency.
#[cfg(not(target_os = "windows"))]
fn is_pid_alive(pid: u32) -> bool {
    crate::testability::ui_automation::pid_is_alive(pid)
}

#[cfg(test)]
mod tests;
