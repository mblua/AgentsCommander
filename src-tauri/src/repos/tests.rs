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

#[test]
fn p17_search_filters_markers_dedup_sort() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("projects");
    std::fs::create_dir(&base).unwrap();
    for name in ["repo-Z", "repo-a", ".hidden", "DEPRECATED-old"] {
        std::fs::create_dir(base.join(name)).unwrap();
    }
    std::fs::create_dir(base.join("repo-a/.claude")).unwrap();
    std::fs::write(base.join("repo-a/CLAUDE.md"), "").unwrap();
    std::fs::write(base.join("repo-a/.codex"), "").unwrap();
    std::fs::write(base.join("repo-Z/CLAUDE.md"), "").unwrap();
    std::fs::write(base.join("ordinary-file"), "").unwrap();
    let configured = vec![base.to_string_lossy().into_owned(); 2];
    let results = search_repositories(&configured, "");
    assert_eq!(
        results.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
        vec![&base, &base.join("repo-a"), &base.join("repo-Z")]
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(results[0].name, derive_repo_name(&base).unwrap());
    assert_eq!(results[1].name, "projects/repo-a");
    assert_eq!(results[2].name, "projects/repo-Z");
    assert!(results[0].agents.is_empty());
    assert_eq!(results[1].agents, vec!["Claude", "Codex"]);
    assert_eq!(results[2].agents, vec!["Claude"]);
    let filtered = search_repositories(&configured, "RePo-A");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].path, results[1].path);
    assert_eq!(filtered[0].agents, results[1].agents);
}

#[tokio::test]
async fn p17_spec_roots_skip_dedup_sort() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("projects");
    std::fs::create_dir(&base).unwrap();
    for name in ["repo-z", "repo-a", ".repo-hidden", "ordinary", "unmarked"] {
        std::fs::create_dir(base.join(name)).unwrap();
    }
    let file = base.join("file");
    std::fs::write(&file, "").unwrap();
    let missing = base.join("missing");
    let configured = [&base, &base, &missing, &file]
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let roots = valid_spec_repo_roots(Some(&configured[0]), &configured)
        .await
        .unwrap();
    assert_eq!(
        roots,
        vec![
            std::fs::canonicalize(base.join("repo-a")).unwrap(),
            std::fs::canonicalize(base.join("repo-z")).unwrap(),
        ]
    );
    assert!(is_valid_spec_repo_root(&base.join("repo-a")).await.unwrap());
    let ordinary = base.join("ordinary");
    std::fs::write(ordinary.join(".git"), "gitdir: elsewhere").unwrap();
    assert!(is_valid_spec_repo_root(&ordinary).await.unwrap());
    std::fs::remove_file(ordinary.join(".git")).unwrap();
    std::fs::create_dir(ordinary.join(".git")).unwrap();
    assert!(is_valid_spec_repo_root(&ordinary).await.unwrap());
    assert!(!is_valid_spec_repo_root(&base.join("unmarked"))
        .await
        .unwrap());
    assert!(!is_valid_spec_repo_root(&missing).await.unwrap());
}
