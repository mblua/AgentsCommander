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
/// On Windows: lowercase + normalize `/` → `\` + strip the `\\?\` extended-
/// length prefix that `std::fs::canonicalize` sometimes returns.
/// On Unix: lowercase only.
///
/// **Must be applied to BOTH sides** inside `find_session_file` (H6): AC's
/// `expected_cwd` AND the file's `session_meta.cwd`. Tests cover both the
/// forward-slash-AC-input vs backslash-Codex-input case and the canonical
/// prefix-stripped case.
pub(crate) fn canonicalize_cwd_for_codex(cwd: &str) -> String {
    #[cfg(windows)]
    {
        let stripped = cwd.strip_prefix(r"\\?\").unwrap_or(cwd);
        stripped.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        cwd.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_when_sessions_root_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join(".codex").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let resolved = resolve_codex_sessions_root_at(tmp.path(), "codex", &[], "");
        assert_eq!(resolved.as_deref(), Some(sessions.as_path()));
    }

    #[test]
    fn returns_none_when_sessions_root_missing() {
        let tmp = tempfile::tempdir().unwrap();
        // No `.codex/sessions/` created.
        let resolved = resolve_codex_sessions_root_at(tmp.path(), "codex", &[], "");
        assert!(resolved.is_none());
    }

    #[test]
    fn effective_codex_home_returns_sessions_path_before_it_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("isolated-codex");
        std::fs::create_dir_all(&home).unwrap();

        let resolved =
            resolve_codex_sessions_root_with_effective_home(Some(&home), "codex", &[], "");

        assert_eq!(resolved, Some(home.join("sessions")));
    }

    #[test]
    fn resolve_codex_sessions_root_uses_home_dir_join_path() {
        let tmp = tempfile::tempdir().unwrap();
        let expected = tmp.path().join(".codex").join("sessions");
        std::fs::create_dir_all(&expected).unwrap();
        let resolved = resolve_codex_sessions_root_at(tmp.path(), "codex", &[], "");
        assert_eq!(resolved.unwrap(), expected);
    }

    #[test]
    #[cfg(windows)]
    fn canonicalize_cwd_for_codex_lowercases_and_normalizes() {
        assert_eq!(canonicalize_cwd_for_codex("C:/Users/Foo"), "c:\\users\\foo");
        assert_eq!(
            canonicalize_cwd_for_codex(r"C:\Users\Foo"),
            "c:\\users\\foo"
        );
    }

    #[test]
    #[cfg(windows)]
    fn canonicalize_cwd_for_codex_strips_extended_prefix() {
        assert_eq!(
            canonicalize_cwd_for_codex(r"\\?\C:\Users\Foo"),
            "c:\\users\\foo"
        );
        assert_eq!(
            canonicalize_cwd_for_codex(r"\\?\C:/Users/Foo"),
            "c:\\users\\foo"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn canonicalize_cwd_for_codex_lowercases_only_on_unix() {
        assert_eq!(canonicalize_cwd_for_codex("/Home/Foo"), "/home/foo");
    }
}
