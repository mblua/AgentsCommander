// Codex CLI session-file path resolver.
//
// Codex stores rollouts under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
// The filename is UUID-keyed, so the actual file must be discovered per-poll
// inside the watcher by reading each candidate's `session_meta.cwd` and
// matching against AC's canonicalized cwd.

use std::path::{Path, PathBuf};

/// Returns the `~/.codex/sessions/` root if it exists.
///
/// Returns `None` if `~/.codex/sessions/` does not exist (Codex never run on
/// this machine) or `dirs::home_dir()` is unavailable.
///
/// The `_shell` / `_shell_args` params are unused today but kept for symmetry
/// with `resolve_claude_projects_dir` (`commands/session.rs:277`) and to allow
/// a future env-var override (e.g. `CODEX_HOME`) without an attach-site
/// refactor.
pub(crate) fn resolve_codex_sessions_root(
    shell: &str,
    args: &[String],
    cwd: &str,
) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    resolve_codex_sessions_root_at(&home, shell, args, cwd)
}

pub(crate) fn resolve_codex_sessions_root_with_effective_home(
    effective_codex_home: Option<&Path>,
    shell: &str,
    args: &[String],
    cwd: &str,
) -> Option<PathBuf> {
    if let Some(home) = effective_codex_home {
        return Some(home.join("sessions"));
    }
    resolve_codex_sessions_root(shell, args, cwd)
}

pub(crate) fn resolve_codex_sessions_root_at(
    home: &Path,
    _shell: &str,
    _args: &[String],
    _cwd: &str,
) -> Option<PathBuf> {
    let root = home.join(".codex").join("sessions");
    if root.is_dir() {
        Some(root)
    } else {
        None
    }
}

/// Canonicalize a path-string for cwd comparison against Codex's
/// `session_meta.cwd` field.
///
/// On Windows: lowercase, convert `/` to `\`, and strip verbatim drive or
/// verbatim UNC prefixes that `std::fs::canonicalize` sometimes returns.
/// On Unix: lowercase only.
///
/// **Must be applied to BOTH sides** inside `find_session_file` (H6): AC's
/// `expected_cwd` AND the file's `session_meta.cwd`. Tests cover both the
/// forward-slash-AC-input vs backslash-Codex-input case and the canonical
/// prefix-stripped case.
pub(crate) fn canonicalize_cwd_for_codex(cwd: &str) -> String {
    #[cfg(windows)]
    {
        let normalized = crate::path_utils::normalize_windows_verbatim_path(cwd);
        normalized.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        cwd.to_lowercase()
    }
}

#[cfg(test)]
mod tests;
