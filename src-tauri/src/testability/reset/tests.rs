use super::*;

#[test]
fn missing_candidates_are_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join(TESTABLE_CONFIG_DIR);
    validate_candidate(tmp.path(), &candidate).unwrap();
}

#[test]
fn file_candidate_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join(TESTABLE_CONFIG_DIR);
    std::fs::write(&candidate, "not a dir").unwrap();
    assert_eq!(
        validate_candidate(tmp.path(), &candidate).unwrap_err(),
        "reset_candidate_not_directory"
    );
}

#[test]
fn issue_3355_reset_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    for name in [TESTABLE_CONFIG_DIR, TESTABLE_PROJECT_DIR] {
        let candidate = tmp.path().join(name);
        std::fs::create_dir(&candidate).unwrap();
        validate_candidate(tmp.path(), &candidate).unwrap();
    }
    assert_eq!(
        validate_candidate(tmp.path(), &tmp.path().join("other")).unwrap_err(),
        "reset_candidate_name_not_allowed"
    );
    assert_eq!(
        validate_candidate(
            tmp.path(),
            &tmp.path().join("other").join(TESTABLE_CONFIG_DIR)
        )
        .unwrap_err(),
        "reset_candidate_parent_mismatch"
    );
}

#[test]
fn issue_3355_reset_ports_preserve_native_results() {
    use crate::filesystem::directories as port;
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join(TESTABLE_CONFIG_DIR);
    let file = tmp.path().join("ordinary-file");
    std::fs::write(&file, "file").unwrap();
    std::fs::create_dir(&candidate).unwrap();
    let error = |e: std::io::Error| (e.kind(), e.raw_os_error(), e.to_string());
    for path in [&candidate, &file] {
        let native = std::fs::symlink_metadata(path).unwrap();
        let observed = port::inspect_test_reset_candidate(path).unwrap();
        assert_eq!(observed.is_dir(), native.is_dir());
        assert_eq!(
            observed.file_type().is_symlink(),
            native.file_type().is_symlink()
        );
        assert_eq!(observed.len(), native.len());
        assert_eq!(
            port::canonical_test_reset_path(path).unwrap(),
            path.canonicalize().unwrap()
        );
        assert_eq!(port::test_reset_candidate_exists(path), path.exists());
    }
    for path in [tmp.path().join("missing"), file.join("child")] {
        assert_eq!(
            error(port::inspect_test_reset_candidate(&path).unwrap_err()),
            error(std::fs::symlink_metadata(&path).unwrap_err())
        );
        assert_eq!(
            error(port::canonical_test_reset_path(&path).unwrap_err()),
            error(path.canonicalize().unwrap_err())
        );
        assert_eq!(port::test_reset_candidate_exists(&path), path.exists());
    }
    let sentinel = tmp.path().join("sentinel");
    std::fs::write(&sentinel, "keep").unwrap();
    let parent = tmp.path().canonicalize().unwrap();
    let resolved = candidate.canonicalize().unwrap();
    assert!(resolved.is_absolute());
    assert_eq!(resolved.parent(), Some(parent.as_path()));
    assert_eq!(resolved, parent.join(TESTABLE_CONFIG_DIR));
    validate_candidate(&parent, &resolved).unwrap();
    port::remove_test_reset_candidate(&resolved).unwrap();
    assert!(!candidate.exists());
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep");
}
