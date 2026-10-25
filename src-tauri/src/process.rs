pub(crate) mod git;
pub(crate) mod harness;
pub(crate) mod host;

use std::process::{self, Command};

#[cfg(windows)]
pub(crate) fn is_running(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }
    let filter = format!("PID eq {}", pid);
    // #992 - reachable from the GUI process (Tauri get_agency_templates_status /
    // update_agency_templates -> CacheLock::acquire -> recover_stale_lock_if_possible),
    // which owns no console: without the flag Windows pops a Windows Terminal tab for
    // tasklist.exe. Both pipes are captured, so nothing would ever be shown in it.
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = Command::new("tasklist");
    cmd.args(["/FI", &filter, "/NH", "/FO", "CSV"])
        .creation_flags(CREATE_NO_WINDOW);
    let Ok(output) = cmd.output() else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().any(|line| {
        line.split(',')
            .any(|field| field.trim_matches('"').trim() == pid.to_string())
    })
}

#[cfg(unix)]
pub(crate) fn is_running(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(true)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn is_running(pid: u32) -> bool {
    pid == process::id()
}

#[cfg(test)]
mod tests;
