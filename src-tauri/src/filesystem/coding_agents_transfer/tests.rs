use super::*;

#[test]
fn write_then_read_round_trips_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agents.json");
    write_transfer_file(&path, "{\"a\":\"ñ\"}").unwrap();
    assert_eq!(read_transfer_file(&path, 1024).unwrap(), "{\"a\":\"ñ\"}");
}

#[test]
fn write_replaces_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agents.json");
    std::fs::write(&path, b"old old old").unwrap();
    write_transfer_file(&path, "new").unwrap();
    assert_eq!(read_transfer_file(&path, 1024).unwrap(), "new");
}

#[test]
fn read_refuses_file_over_cap() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("big.json");
    std::fs::write(&big, b"12345678901").unwrap();
    let err = read_transfer_file(&big, 10).unwrap_err();
    assert!(err.starts_with("Import file is too large"), "{err}");
    let exact = dir.path().join("exact.json");
    std::fs::write(&exact, b"1234567890").unwrap();
    assert_eq!(read_transfer_file(&exact, 10).unwrap(), "1234567890");
}

#[test]
fn read_rejects_invalid_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(&path, [0xff_u8, 0xfe]).unwrap();
    assert_eq!(
        read_transfer_file(&path, 1024).unwrap_err(),
        "Import file is not valid UTF-8"
    );
}

#[test]
fn read_missing_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let err = read_transfer_file(&dir.path().join("none.json"), 1024).unwrap_err();
    assert!(err.starts_with("Failed to read import file"), "{err}");
}

#[test]
fn write_into_missing_directory_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing").join("agents.json");
    let err = write_transfer_file(&path, "x").unwrap_err();
    assert!(err.starts_with("Failed to write export file"), "{err}");
}
