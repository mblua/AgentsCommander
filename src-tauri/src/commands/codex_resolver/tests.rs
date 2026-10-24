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

    let resolved = resolve_codex_sessions_root_with_effective_home(Some(&home), "codex", &[], "");

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
#[cfg(windows)]
fn canonicalize_cwd_for_codex_matches_ordinary_and_verbatim_unc() {
    assert_eq!(
        canonicalize_cwd_for_codex(r"\\?\UNC\server\share\Repo"),
        canonicalize_cwd_for_codex(r"\\server\share\Repo")
    );
    assert_eq!(
        canonicalize_cwd_for_codex(r"\\?\UNC\server\share\Repo"),
        r"\\server\share\repo"
    );
}

#[test]
#[cfg(not(windows))]
fn canonicalize_cwd_for_codex_lowercases_only_on_unix() {
    assert_eq!(canonicalize_cwd_for_codex("/Home/Foo"), "/home/foo");
}
