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
//! Login-shell execution belongs to `process::host::login`; this module owns
//! environment reads, the skip decision, caching and search-path composition.
//! The dependency is one-way: login must not import config.

#[cfg(test)]
pub(crate) use crate::process::host::login::parse_sentinel_path;
pub(crate) use crate::process::host::login::probe_login_shell_path;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// #2589 - fixed by user decision, not configurable. Relative entries resolve
/// against the user's home; absolute entries are used as-is. This is the search
/// order AMONG the added dirs; all of them rank after the inherited PATH.
const HOME_RELATIVE_BIN_DIRS: &[&str] = &[".local/bin", ".npm-global/bin", "bin", ".cargo/bin"];
const ABSOLUTE_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

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

/// #2683 - the ONLY place the platform decision is made and the ONLY place
/// the test override is read. `None` on Windows by decision.
fn search_path_to_apply() -> Option<OsString> {
    if cfg!(windows) {
        return None;
    }
    #[cfg(test)]
    if let Some(injected) = test_search_path_override() {
        return Some(injected);
    }
    Some(effective_search_path())
}

#[cfg(test)]
thread_local! {
    static TEST_SEARCH_PATH_OVERRIDE: std::cell::RefCell<Option<OsString>> =
        const { std::cell::RefCell::new(None) };
}

/// #2683 - the search path installed on this thread by a
/// `SearchPathOverrideGuard`, if any.
#[cfg(test)]
pub(crate) fn test_search_path_override() -> Option<OsString> {
    TEST_SEARCH_PATH_OVERRIDE.with(|cell| cell.borrow().clone())
}

/// #2683 - installs a thread-local search path override for the lifetime of
/// the guard; `Drop` restores the previous value, even on panic.
#[cfg(test)]
pub(crate) struct SearchPathOverrideGuard {
    previous: Option<OsString>,
}

#[cfg(test)]
impl SearchPathOverrideGuard {
    pub(crate) fn new(value: impl Into<OsString>) -> Self {
        let previous =
            TEST_SEARCH_PATH_OVERRIDE.with(|cell| cell.borrow_mut().replace(value.into()));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for SearchPathOverrideGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        TEST_SEARCH_PATH_OVERRIDE.with(|cell| *cell.borrow_mut() = previous);
    }
}

/// #2683 - sets `PATH` on a PTY command to the effective search path. A no-op
/// on Windows by decision. The assignment is also what `portable_pty` searches
/// to resolve a bare program name (`cmdbuilder.rs` `resolve_path` reads
/// `get_env("PATH")`), so it both finds the binary and reaches the child.
pub fn apply_search_path_to_pty_command(command: &mut portable_pty::CommandBuilder) {
    if let Some(path) = search_path_to_apply() {
        command.env("PATH", path);
    }
}

/// #2683 - sets `PATH` on a std command to the effective search path. A no-op
/// on Windows by decision.
pub fn apply_search_path_to_std_command(command: &mut std::process::Command) {
    if let Some(path) = search_path_to_apply() {
        command.env("PATH", path);
    }
}

/// #2683 - sets `PATH` on a tokio command to the effective search path. A
/// no-op on Windows by decision.
pub fn apply_search_path_to_tokio_command(command: &mut tokio::process::Command) {
    if let Some(path) = search_path_to_apply() {
        command.env("PATH", path);
    }
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
mod tests;
