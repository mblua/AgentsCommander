//! External process helpers (agent, Docker, Git, GitHub, harness, host) and
//! the platform-specific `is_running` PID check.

pub(crate) mod agent;
pub(crate) mod docker;
pub(crate) mod git;
pub(crate) mod github;
pub(crate) mod harness;
pub(crate) mod host;

use std::process::{self, Command};

#[cfg(windows)]
pub(crate) fn is_running(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }
    let filter = format!("PID eq {pid}");
    // #992 - reachable from the GUI process (Tauri get_agency_templates_status /
    // update_agency_templates -> CacheLock::acquire -> recover_stale_lock_if_possible),
    // which owns no console: without the flag Windows pops a Windows Terminal tab for
    // tasklist.exe. Both pipes are captured, so nothing would ever be shown in it.
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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

/// #2774 Run `hyprctl <args>`; stdout on success. 5 s timeout, no stdin, killed on drop.
#[cfg(target_os = "linux")]
pub(crate) async fn hyprctl(args: &[&str]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("hyprctl");
    cmd.args(args)
        .stdin(process::Stdio::null())
        .kill_on_drop(true);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    let timeout = std::time::Duration::from_secs(5);
    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => return Err("hyprctl timed out".to_string()),
        Ok(Err(e)) => return Err(format!("could not run hyprctl: {e}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!("hyprctl exited with {}", output.status))
    } else {
        Err(stderr)
    }
}

/// #4843 What GStreamer says about `autoaudiosink`, the sink WebKitGTK plays sound through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AudioSinkStatus {
    Ok,
    Missing,
    Unknown,
}

/// #4843 The GStreamer inspector used to look up `autoaudiosink`.
const GST_INSPECT: &str = "gst-inspect-1.0";

/// #4843 `gst-inspect-1.0 --exists` exit code: 0 present, 1 missing, anything else unknown.
fn status_from_exit(code: Option<i32>) -> AudioSinkStatus {
    match code {
        Some(0) => AudioSinkStatus::Ok,
        Some(1) => AudioSinkStatus::Missing,
        _ => AudioSinkStatus::Unknown,
    }
}

/// #4843 `Ok` off Linux; `Unknown` inside an AppImage (its bundled GStreamer may differ).
pub(crate) async fn audio_sink_status(in_appimage: bool) -> AudioSinkStatus {
    if cfg!(not(target_os = "linux")) {
        return AudioSinkStatus::Ok;
    }
    if in_appimage {
        return AudioSinkStatus::Unknown;
    }
    probe_audio_sink(std::ffi::OsStr::new(GST_INSPECT)).await
}

/// #4843 Run `<program> --exists autoaudiosink` with the app's own environment (no
/// login-shell PATH). 5 s timeout, no stdin, output discarded, killed on drop.
async fn probe_audio_sink(program: &std::ffi::OsStr) -> AudioSinkStatus {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(["--exists", "autoaudiosink"])
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .kill_on_drop(true);
    let timeout = std::time::Duration::from_secs(5);
    match tokio::time::timeout(timeout, cmd.status()).await {
        Ok(Ok(status)) => status_from_exit(status.code()),
        _ => AudioSinkStatus::Unknown,
    }
}

#[cfg(test)]
mod tests;
