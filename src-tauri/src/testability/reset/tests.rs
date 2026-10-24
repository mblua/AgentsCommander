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
