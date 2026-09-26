//! #2589 - the search path used to resolve and run coding-agent binaries.
//!
//! A GUI-launched app inherits a PATH without the user's login-profile entries
//! (`~/.local/bin`, `~/.npm-global/bin`, nvm, ...). This module builds one
//! search path from two layers, unioned and never substituted:
//!
//! - Phase 1, deterministic: the inherited PATH followed by a fixed list of user
//!   bin dirs that exist on disk. Executes nothing.
//! - Phase 2, one bounded probe of the user's login-shell PATH, cached per
//!   process. Its failure leaves Phase 1 in force.
//!
//! Leaf module: it must not import any other in-crate module, tests included,
//! so it can never join the module SCC.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// #2589 - fixed by user decision, not configurable. Relative entries resolve
/// against the user's home; absolute entries are used as-is. This is the search
/// order AMONG the added dirs; all of them rank after the inherited PATH.
const HOME_RELATIVE_BIN_DIRS: &[&str] = &[".local/bin", ".npm-global/bin", "bin", ".cargo/bin"];
const ABSOLUTE_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

#[cfg_attr(not(unix), allow(dead_code))]
const PATH_BEGIN: &str = "__AC_PATH_BEGIN__";
#[cfg_attr(not(unix), allow(dead_code))]
const PATH_END: &str = "__AC_PATH_END__";
const LOGIN_SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

static LOGIN_SHELL_PATH: tokio::sync::OnceCell<Option<OsString>> =
    tokio::sync::OnceCell::const_new();

/// Phase 1 candidates that exist as directories, in fixed order.
/// Empty on Windows and empty when `home` is `None`. Pure.
pub(crate) fn phase1_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    if cfg!(windows) {
        return Vec::new();
    }
    let Some(home) = home else {
        return Vec::new();
    };
    HOME_RELATIVE_BIN_DIRS
        .iter()
        .map(|dir| home.join(dir))
        .chain(ABSOLUTE_BIN_DIRS.iter().map(PathBuf::from))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// Everything the resolver needs, so tests never read process state.
#[derive(Clone, Debug, Default)]
pub(crate) struct SearchPathInputs {
    pub inherited: Option<OsString>,
    pub home: Option<PathBuf>,
    /// Phase 2 result; `None` when absent, skipped or failed.
    pub login_shell_path: Option<OsString>,
}

/// Pure and total: the ONLY place the search path is built. The inherited value
/// is kept byte for byte as the prefix; added dirs are appended only when not
/// already present, so an inherited winner always keeps winning.
pub(crate) fn compose(inputs: &SearchPathInputs) -> OsString {
    let inherited = inputs.inherited.clone().unwrap_or_default();
    let mut present: Vec<PathBuf> = std::env::split_paths(&inherited).collect();
    let mut added: Vec<PathBuf> = Vec::new();
    let phase2 = inputs
        .login_shell_path
        .as_deref()
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default();
    for dir in phase1_dirs(inputs.home.as_deref())
        .into_iter()
        .chain(phase2)
    {
        if dir.as_os_str().is_empty() || present.contains(&dir) {
            continue;
        }
        // An entry holding the separator cannot be joined; skip it.
        if std::env::join_paths([&dir]).is_err() {
            continue;
        }
        present.push(dir.clone());
        added.push(dir);
    }
    if added.is_empty() {
        return inherited;
    }
    let mut composed = inherited;
    let separator = if cfg!(windows) { ";" } else { ":" };
    for dir in added {
        if !composed.is_empty() {
            composed.push(separator);
        }
        composed.push(dir.as_os_str());
    }
    composed
}

fn inputs_from_process() -> SearchPathInputs {
    SearchPathInputs {
        inherited: std::env::var_os("PATH"),
        home: std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from),
        login_shell_path: LOGIN_SHELL_PATH.get().cloned().flatten(),
    }
}

/// The search path for resolving and running agent binaries. Includes the
/// Phase 2 value once `warm_login_shell_path` has completed.
pub fn effective_search_path() -> OsString {
    compose(&inputs_from_process())
}

/// Pure skip decision for Phase 2. `None` means do not probe: windows, `$SHELL`
/// unset or empty, or a `fish` shell (its `$PATH` is a list, so the printed
/// value would be unusable; fish users are served by Phase 1).
pub(crate) fn login_shell_to_probe(shell: Option<&OsStr>) -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let shell = shell.filter(|shell| !shell.is_empty())?;
    let shell = PathBuf::from(shell);
    if shell.file_name() == Some(OsStr::new("fish")) {
        return None;
    }
    Some(shell)
}

/// The Phase 2 command, built and not spawned.
///
/// It must NOT set a process group: std runs `setpgid` before the `pre_exec`
/// closures, which makes the child a group leader, and `setsid` then fails
/// with `EPERM`.
#[cfg(unix)]
pub(crate) fn build_login_shell_probe_command(shell: &Path) -> tokio::process::Command {
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
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn parse_sentinel_path(stdout: &str) -> Option<&str> {
    let start = stdout.find(PATH_BEGIN)? + PATH_BEGIN.len();
    let rest = &stdout[start..];
    let end = rest.find(PATH_END)?;
    let value = &rest[..end];
    (!value.is_empty()).then_some(value)
}

/// Cached wrapper: reads `$SHELL`, applies `login_shell_to_probe`, and runs the
/// probe at most once per process. A failure is cached as `None`.
pub async fn warm_login_shell_path() {
    LOGIN_SHELL_PATH
        .get_or_init(|| async {
            let shell = login_shell_to_probe(std::env::var_os("SHELL").as_deref())?;
            probe_login_shell_path(&shell, LOGIN_SHELL_PROBE_TIMEOUT).await
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).expect("join test paths")
    }

    #[test]
    fn agent_path_2589_phase1_dirs_keeps_only_existing_dirs_in_fixed_order() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".local/bin")).unwrap();
        std::fs::create_dir_all(home.path().join("bin")).unwrap();

        let dirs = phase1_dirs(Some(home.path()));

        if cfg!(windows) {
            assert!(dirs.is_empty());
            return;
        }
        let mut expected = vec![home.path().join(".local/bin"), home.path().join("bin")];
        expected.extend(
            ABSOLUTE_BIN_DIRS
                .iter()
                .map(PathBuf::from)
                .filter(|dir| dir.is_dir()),
        );
        assert_eq!(dirs, expected);
    }

    #[test]
    fn agent_path_2589_compose_appends_new_entries_after_inherited_and_dedupes() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        let c = root.path().join("c");
        let inherited = join(&[&a, &b]);
        let composed = compose(&SearchPathInputs {
            inherited: Some(inherited.clone()),
            home: None,
            login_shell_path: Some(join(&[&b, &c, &c, &a])),
        });

        let mut expected = inherited;
        expected.push(if cfg!(windows) { ";" } else { ":" });
        expected.push(c.as_os_str());
        assert_eq!(composed, expected);
    }

    #[test]
    fn agent_path_2589_compose_adds_phase2_entries_not_already_present() {
        let root = tempfile::tempdir().unwrap();
        let inherited_dir = root.path().join("inherited");
        let nvm = root.path().join("nvm/bin");
        let volta = root.path().join("volta/bin");
        let composed = compose(&SearchPathInputs {
            inherited: Some(join(&[&inherited_dir])),
            home: None,
            login_shell_path: Some(join(&[&nvm, &inherited_dir, &volta])),
        });

        let entries: Vec<PathBuf> = std::env::split_paths(&composed).collect();
        assert_eq!(entries, vec![inherited_dir, nvm, volta]);
    }

    #[test]
    fn agent_path_2589_phase1_dirs_is_empty_without_home() {
        assert!(phase1_dirs(None).is_empty());
        assert!(compose(&SearchPathInputs::default()).is_empty());
    }

    #[test]
    fn agent_path_2589_parses_the_sentinel_value_between_banners() {
        let stdout = "Welcome banner\nnvm: using node 20\n__AC_PATH_BEGIN__/a b/bin:/usr/bin__AC_PATH_END__trailing motd\n";
        assert_eq!(parse_sentinel_path(stdout), Some("/a b/bin:/usr/bin"));
    }

    #[test]
    fn agent_path_2589_compose_returns_inherited_unchanged_when_extra_is_empty() {
        let inherited = OsString::from(
            r#"C:\Windows\system32;"C:\Program Files\Tool";C:\Windows\system32;C:\Users\u\AppData\Roaming\npm;"#,
        );
        let composed = compose(&SearchPathInputs {
            inherited: Some(inherited.clone()),
            home: None,
            login_shell_path: None,
        });
        assert_eq!(composed, inherited);
    }

    #[test]
    fn agent_path_2589_sentinel_parse_rejects_missing_end_marker_and_empty_value() {
        assert_eq!(parse_sentinel_path("__AC_PATH_BEGIN__/usr/bin"), None);
        assert_eq!(
            parse_sentinel_path("__AC_PATH_BEGIN____AC_PATH_END__"),
            None
        );
        assert_eq!(parse_sentinel_path("/usr/bin__AC_PATH_END__"), None);
        assert_eq!(parse_sentinel_path(""), None);
    }

    #[cfg(unix)]
    fn fake_shell(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn agent_path_2589_login_shell_to_probe_skips_fish_and_empty_shell() {
        assert_eq!(
            login_shell_to_probe(Some(OsStr::new("/usr/local/bin/fish"))),
            None
        );
        assert_eq!(login_shell_to_probe(Some(OsStr::new(""))), None);
        assert_eq!(login_shell_to_probe(None), None);
        assert_eq!(
            login_shell_to_probe(Some(OsStr::new("/bin/bash"))),
            Some(PathBuf::from("/bin/bash"))
        );
    }

    /// Fallback branch of plan 6.3.T10: the spawn succeeding through the real
    /// builder proves `setsid` did not fail, which a re-introduced process group
    /// setting would make it do (EPERM, propagated as a spawn error).
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_path_2589_login_shell_probe_child_is_a_session_leader() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "fake-shell",
            "printf '__AC_PATH_BEGIN__/session/leader__AC_PATH_END__'",
        );
        let child = build_login_shell_probe_command(&shell)
            .spawn()
            .expect("setsid must succeed, so the spawn must succeed");
        let output = child.wait_with_output().await.expect("probe output");
        assert!(output.status.success());
        assert_eq!(
            parse_sentinel_path(&String::from_utf8_lossy(&output.stdout)),
            Some("/session/leader")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_path_2589_login_shell_probe_reads_the_value_past_a_banner() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "fake-shell",
            "echo 'Last login: today'\necho 'no job control in this shell' >&2\nprintf '__AC_PATH_BEGIN__/home/u/.nvm/bin:/usr/bin__AC_PATH_END__'\necho 'bye'",
        );
        let value = probe_login_shell_path(&shell, Duration::from_secs(5)).await;
        assert_eq!(value, Some(OsString::from("/home/u/.nvm/bin:/usr/bin")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_path_2589_login_shell_probe_times_out_and_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "slow-shell", "sleep 30");
        let started = std::time::Instant::now();
        let value = probe_login_shell_path(&shell, LOGIN_SHELL_PROBE_TIMEOUT).await;
        let elapsed = started.elapsed();
        assert_eq!(value, None);
        assert!(elapsed >= LOGIN_SHELL_PROBE_TIMEOUT, "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_path_2589_login_shell_probe_skips_a_missing_shell() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-shell");
        assert_eq!(
            probe_login_shell_path(&missing, Duration::from_secs(5)).await,
            None
        );
    }
}
