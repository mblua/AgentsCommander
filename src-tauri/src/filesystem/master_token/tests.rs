use super::*;

#[test]
fn no_config_directory_does_not_match() {
    assert!(!matches(None, "token"));
    assert!(!matches(None, ""));
}

#[test]
fn missing_file_does_not_match() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!matches(Some(dir.path()), "token"));
    assert!(!matches(Some(dir.path()), ""));
}

#[test]
fn directory_instead_of_file_does_not_match() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(FILE_NAME)).unwrap();
    assert!(!matches(Some(dir.path()), "token"));
    assert!(!matches(Some(dir.path()), ""));
}

#[test]
fn invalid_utf8_does_not_match() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE_NAME), [0xff, 0xfe]).unwrap();
    assert!(!matches(Some(dir.path()), "token"));
    assert!(!matches(Some(dir.path()), ""));
}

#[test]
fn comparison_is_exact_and_case_sensitive() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE_NAME), "Token").unwrap();
    assert!(matches(Some(dir.path()), "Token"));
    assert!(!matches(Some(dir.path()), "token"));
    assert!(!matches(Some(dir.path()), "other"));
}

#[test]
fn only_persisted_whitespace_is_trimmed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(FILE_NAME),
        " \t\r\n\u{2003}Token\u{2003}\r\n ",
    )
    .unwrap();
    assert!(matches(Some(dir.path()), "Token"));
    assert!(!matches(Some(dir.path()), " Token"));
    assert!(!matches(Some(dir.path()), "Token\r\n"));
    assert!(!matches(Some(dir.path()), "\u{2003}Token"));
}

#[test]
fn empty_and_blank_files_preserve_empty_token_matching() {
    let dir = tempfile::tempdir().unwrap();
    for text in ["", " \t\r\n\u{2003}"] {
        std::fs::write(dir.path().join(FILE_NAME), text).unwrap();
        assert!(matches(Some(dir.path()), ""));
        assert!(!matches(Some(dir.path()), "token"));
    }
}

#[test]
fn each_call_reads_the_current_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::write(&path, "first").unwrap();
    assert!(matches(Some(dir.path()), "first"));
    std::fs::write(&path, "second").unwrap();
    assert!(!matches(Some(dir.path()), "first"));
    assert!(matches(Some(dir.path()), "second"));
}
