//! Bounded login-shell PATH probe and its process lifetime.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

#[cfg(any(unix, test))]
const PATH_BEGIN: &str = "__AC_PATH_BEGIN__";
#[cfg(any(unix, test))]
const PATH_END: &str = "__AC_PATH_END__";

/// The Phase 2 command, built and not spawned.
///
/// It must NOT set a process group: std runs `setpgid` before the `pre_exec`
/// closures, which makes the child a group leader, and `setsid` then fails
/// with `EPERM`.
#[cfg(unix)]
fn build_login_shell_probe_command(shell: &Path) -> tokio::process::Command {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let mut command = tokio::process::Command::new(shell);
    command
        .arg("-l")
        .arg("-i")
        .arg("-c")
        .arg(format!("printf '{PATH_BEGIN}%s{PATH_END}' \"$PATH\""));
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    // SAFETY: async-signal-safe single syscall between fork and exec. It detaches
    // the child from AC's controlling terminal so an interactive shell cannot stop
    // on SIGTTIN. The error is propagated, never swallowed: a silent EPERM here
    // would restore the stall invisibly. After setsid the child's pgid equals its
    // pid, so killing the group on timeout still reaps the whole tree.
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

/// Phase 2 inner: no cache, no env read, no skip logic. Assumes the caller
/// already applied `login_shell_to_probe`. Any failure yields `None`.
#[cfg(unix)]
pub(crate) async fn probe_login_shell_path(shell: &Path, timeout: Duration) -> Option<OsString> {
    let child = match build_login_shell_probe_command(shell).spawn() {
        Ok(child) => child,
        Err(error) => {
            log::warn!(
                "[agent-path] login shell probe spawn failed for {}: {error}",
                shell.display()
            );
            return None;
        }
    };
    let pid = child.id();
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            log::warn!("[agent-path] login shell probe failed: {error}");
            return None;
        }
        Err(_) => {
            if let Some(pid) = pid {
                // SAFETY: plain syscall; after setsid the child's pgid equals
                // its pid, so this kills the probe's whole tree and nothing else.
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
            }
            log::warn!(
                "[agent-path] login shell probe timed out after {timeout:?} for {}",
                shell.display()
            );
            return None;
        }
    };
    if !output.status.success() {
        log::warn!(
            "[agent-path] login shell probe exited with {} for {}",
            output.status,
            shell.display()
        );
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_sentinel_path(&stdout).map(OsString::from)
}

#[cfg(not(unix))]
pub(crate) async fn probe_login_shell_path(_shell: &Path, _timeout: Duration) -> Option<OsString> {
    None
}

/// Pure parse of the probe's stdout: the text between the sentinels, if any
/// and non-empty.
#[cfg(any(unix, test))]
pub(crate) fn parse_sentinel_path(stdout: &str) -> Option<&str> {
    let start = stdout.find(PATH_BEGIN)? + PATH_BEGIN.len();
    let rest = &stdout[start..];
    let end = rest.find(PATH_END)?;
    let value = &rest[..end];
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests;
