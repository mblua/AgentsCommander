// Gemini CLI session-file path resolver.
//
// Gemini stores sessions under `~/.gemini/tmp/<slug>/chats/session-*.jsonl`,
// where `<slug>` is keyed off the cwd via `~/.gemini/projects.json`. Under
// the softened H1 contract, the resolver only checks that `~/.gemini/`
// exists; the cwd-to-slug lookup is deferred to the watcher's per-poll
// `lookup_chats_dir_for_cwd` because Gemini's startup race may not yet
// have written the `projects.json` entry for a freshly-launched cwd.

use std::path::{Path, PathBuf};

/// Returns `Some(~/.gemini)` if that directory exists. Returns `None` only
/// when Gemini has never been installed on this machine.
///
/// The cwd-to-slug lookup is deferred to `lookup_chats_dir_for_cwd` below
/// (H1 softened contract — see plan §4.3).
pub(crate) fn resolve_gemini_home(shell: &str, args: &[String], cwd: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    resolve_gemini_home_at(&home, shell, args, cwd)
}

pub(crate) fn resolve_gemini_home_at(
    home: &Path,
    _shell: &str,
    _args: &[String],
    _cwd: &str,
) -> Option<PathBuf> {
    let gemini = home.join(".gemini");
    if gemini.is_dir() {
        Some(gemini)
    } else {
        None
    }
}

/// Per-poll cwd-to-chats-dir lookup. Returns the chats dir if `projects.json`
/// contains a mapping for the canonicalized `cwd`, or `None` if the mapping is
/// not yet present (the caller polls again next tick) or `projects.json` is
/// missing/malformed.
pub(crate) fn lookup_chats_dir_for_cwd(gemini_home: &Path, cwd: &str) -> Option<PathBuf> {
    let projects_path = gemini_home.join("projects.json");
    let contents = std::fs::read_to_string(&projects_path).ok()?;
    let projects: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let map = projects.get("projects")?.as_object()?;
    let normalized = canonicalize_cwd_for_gemini(cwd);
    let slug = map.get(&normalized)?.as_str()?;
    Some(gemini_home.join("tmp").join(slug).join("chats"))
}

/// Canonicalize a path-string for cwd comparison against `projects.json` keys.
/// On Windows: strip `\\?\` prefix + normalize `/` -> `\` + lowercase. On Unix:
/// lowercase only.
#[cfg(windows)]
pub(crate) fn canonicalize_cwd_for_gemini(cwd: &str) -> String {
    let stripped = cwd.strip_prefix(r"\\?\").unwrap_or(cwd);
    stripped.replace('/', "\\").to_lowercase()
}

#[cfg(not(windows))]
pub(crate) fn canonicalize_cwd_for_gemini(cwd: &str) -> String {
    cwd.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolve_gemini_home_returns_some_if_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let gemini = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        let resolved = resolve_gemini_home_at(tmp.path(), "gemini", &[], "");
        assert_eq!(resolved.as_deref(), Some(gemini.as_path()));
    }

    #[test]
    fn resolve_gemini_home_returns_none_if_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_gemini_home_at(tmp.path(), "gemini", &[], "");
        assert!(resolved.is_none());
    }

    #[test]
    fn lookup_chats_dir_for_cwd_resolves_from_projects_json() {
        let tmp = tempfile::tempdir().unwrap();
        let gemini = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        let projects = serde_json::json!({
            "projects": {
                "c:\\foo": "myslug",
            }
        });
        fs::write(gemini.join("projects.json"), projects.to_string()).unwrap();

        let chats = lookup_chats_dir_for_cwd(&gemini, "c:\\foo");
        let expected = gemini.join("tmp").join("myslug").join("chats");
        assert_eq!(chats, Some(expected));
    }

    #[test]
    #[cfg(windows)]
    fn lookup_chats_dir_for_cwd_canonicalization_is_lowercase_on_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let gemini = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        let projects = serde_json::json!({
            "projects": {
                "c:\\users\\foo": "slug-foo",
            }
        });
        fs::write(gemini.join("projects.json"), projects.to_string()).unwrap();

        let chats = lookup_chats_dir_for_cwd(&gemini, r"C:\Users\Foo");
        assert!(chats.is_some());
    }

    #[test]
    #[cfg(windows)]
    fn lookup_chats_dir_for_cwd_with_forward_slashes_canonicalizes_to_backslash() {
        let tmp = tempfile::tempdir().unwrap();
        let gemini = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        let projects = serde_json::json!({
            "projects": {
                "c:\\users\\foo": "slug-foo",
            }
        });
        fs::write(gemini.join("projects.json"), projects.to_string()).unwrap();

        let chats = lookup_chats_dir_for_cwd(&gemini, "C:/Users/Foo");
        assert!(chats.is_some());
    }

    #[test]
    fn lookup_chats_dir_for_cwd_returns_none_when_cwd_not_in_projects_json() {
        let tmp = tempfile::tempdir().unwrap();
        let gemini = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        let projects = serde_json::json!({"projects": {}});
        fs::write(gemini.join("projects.json"), projects.to_string()).unwrap();

        let chats = lookup_chats_dir_for_cwd(&gemini, "c:\\users\\foo");
        assert!(chats.is_none());
    }

    #[test]
    fn lookup_chats_dir_for_cwd_returns_none_when_projects_json_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let gemini = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        // No projects.json file written.

        let chats = lookup_chats_dir_for_cwd(&gemini, "c:\\users\\foo");
        assert!(chats.is_none());
    }

    #[test]
    fn lookup_chats_dir_for_cwd_returns_none_when_projects_json_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let gemini = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        fs::write(gemini.join("projects.json"), "not json").unwrap();

        let chats = lookup_chats_dir_for_cwd(&gemini, "c:\\users\\foo");
        assert!(chats.is_none());
    }

    #[test]
    #[cfg(not(windows))]
    fn canonicalize_cwd_for_gemini_lowercases_only_on_unix() {
        assert_eq!(canonicalize_cwd_for_gemini("/Home/Foo"), "/home/foo");
    }
}
