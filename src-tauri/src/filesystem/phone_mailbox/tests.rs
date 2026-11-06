use super::*;

#[test]
fn mailbox_path_try_exists_existing_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("existing.json");
    std::fs::write(&path, b"{}").unwrap();

    assert!(matches!(mailbox_path_try_exists(&path), Ok(true)));
}

#[test]
fn mailbox_path_try_exists_missing_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("missing.json");

    assert!(matches!(mailbox_path_try_exists(&path), Ok(false)));
}

#[test]
fn mailbox_path_try_exists_invalid_path() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("invalid\0.json");

    assert!(mailbox_path_try_exists(&path).is_err());
}
