use super::*;

/// Run git in `dir`. `false` when git is missing OR the command failed; the
/// callers that need to tell those apart early-return instead of asserting.
/// `.output()` (not `.status()`) keeps `git init`'s chatter out of the test log.
fn git_in(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

const ORIGIN: &str = "https://github.com/mblua/AgentsCommander.git";

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

/// T1 - a path that does not exist yields None, and never spawns git: the
/// `.git` stat fails first.
#[tokio::test]
async fn git_remote_url_missing_path_is_none() {
    assert_eq!(
        git_remote_url_inner("C:/definitely/not/here/943").await,
        None
    );
}

/// T2 - real repo, real origin.
#[tokio::test]
async fn git_remote_url_reads_origin_from_a_temp_repo() {
    let dir = tempfile::tempdir().expect("tempdir");
    if !git_in(dir.path(), &["init", "--quiet"]) {
        return; // git unavailable: keep a git-less CI image green
    }
    assert!(git_in(dir.path(), &["remote", "add", "origin", ORIGIN]));

    let url = git_remote_url_inner(&dir.path().to_string_lossy())
        .await
        .expect("origin");
    // Containment, not equality: a dev with a `url.<x>.insteadOf` rewrite gets
    // the `git@github.com:` form back, and an equality assertion would fail
    // locally while CI stayed green.
    assert!(
        url.contains("mblua/AgentsCommander"),
        "unexpected origin: {url}"
    );
}

/// T3 - a work tree with no `origin` is "no browse target", not an error.
#[tokio::test]
async fn git_remote_url_without_origin_is_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    if !git_in(dir.path(), &["init", "--quiet"]) {
        return;
    }
    assert_eq!(
        git_remote_url_inner(&dir.path().to_string_lossy()).await,
        None
    );
}

/// T4 - the command's only error branch.
#[tokio::test]
async fn git_remote_url_rejects_empty_path() {
    assert!(git_remote_url(String::new()).await.is_err());
    assert!(git_remote_url("   ".to_string()).await.is_err());
}

/// T5' - pins the S5 fix. A plain dir nested inside a work tree must NOT
/// inherit the ancestor's origin: without the `.git` gate git walks up, and
/// Browse Main would confidently open an unrelated GitHub repo.
#[tokio::test]
async fn git_remote_url_nested_non_work_tree_is_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    if !git_in(dir.path(), &["init", "--quiet"]) {
        return;
    }
    assert!(git_in(dir.path(), &["remote", "add", "origin", ORIGIN]));

    let inner = dir.path().join("inner");
    std::fs::create_dir_all(&inner).expect("inner dir");

    assert_eq!(git_remote_url_inner(&inner.to_string_lossy()).await, None);
}
