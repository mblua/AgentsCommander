use super::*;

#[test]
fn strip_userinfo_removes_credentials_from_scheme_urls() {
    assert_eq!(
        strip_userinfo("https://u:tok@github.com/o/r.git"),
        "https://github.com/o/r.git"
    );
    assert_eq!(
        strip_userinfo("ssh://git@github.com/o/r.git"),
        "ssh://github.com/o/r.git"
    );
}

#[test]
fn strip_userinfo_leaves_everything_else_alone() {
    // No userinfo.
    assert_eq!(
        strip_userinfo("https://github.com/o/r.git"),
        "https://github.com/o/r.git"
    );
    // scp-like: no `://`, and the `git@` is an ssh user name, not a secret.
    assert_eq!(
        strip_userinfo("git@github.com:o/r.git"),
        "git@github.com:o/r.git"
    );
    // `@` in the path, none in the authority: must not be treated as userinfo.
    assert_eq!(
        strip_userinfo("https://github.com/o/r@2.git"),
        "https://github.com/o/r@2.git"
    );
    // Degenerate inputs must not panic.
    assert_eq!(strip_userinfo(""), "");
    assert_eq!(strip_userinfo("https://"), "https://");
}

#[test]
fn derive_repo_name_preserves_path_names() {
    assert_eq!(derive_repo_name(Path::new("/")), None);
    assert_eq!(
        derive_repo_name(Path::new("parent/repo")),
        Some("parent/repo".into())
    );
    assert_eq!(
        derive_repo_name(Path::new("padre/日本語")),
        Some("padre/日本語".into())
    );
}
