use super::*;

#[test]
fn web_dist_candidate_directory() {
    let directory = tempfile::tempdir().unwrap();
    assert!(web_dist_candidate_is_directory(directory.path()));
}

#[test]
fn web_dist_candidate_file() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("dist");
    std::fs::write(&file, b"distribution").unwrap();
    assert!(!web_dist_candidate_is_directory(&file));
}

#[test]
fn web_dist_candidate_missing() {
    let directory = tempfile::tempdir().unwrap();
    assert!(!web_dist_candidate_is_directory(
        &directory.path().join("missing")
    ));
}

#[test]
fn resolve_web_dist_candidate_existing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_path_buf();
    assert_eq!(
        resolve_web_dist_candidate(path.clone()),
        std::fs::canonicalize(path).unwrap()
    );
}

#[test]
fn resolve_web_dist_candidate_missing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing");
    assert_eq!(resolve_web_dist_candidate(path.clone()), path);
}
