use super::*;
use std::fs;

const FIXTURE: &str = r#"{"lastCheckedAt":"2026-10-01T12:34:56Z","latestVersion":"1.2.3-beta.4"}"#;
const PRETTY: &str = concat!(
    "{\n",
    "  \"lastCheckedAt\": \"2026-10-01T12:34:56Z\",\n",
    "  \"latestVersion\": \"1.2.3-beta.4\"\n",
    "}"
);

fn cache() -> Cache {
    serde_json::from_str(FIXTURE).unwrap()
}

#[test]
fn reads_existing_camel_case_cache_with_timestamp_and_version() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("update-check.json"), FIXTURE).unwrap();

    let actual = read(Some(dir.path())).unwrap();
    assert_eq!(
        actual.last_checked_at.to_rfc3339(),
        "2026-10-01T12:34:56+00:00"
    );
    assert_eq!(actual.latest_version, "1.2.3-beta.4");
}

#[test]
fn absent_directory_or_cache_is_a_miss_and_write_does_not_create_directories() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read(None).is_none());
    write(None, &cache());
    assert!(read(Some(dir.path())).is_none());
    let missing = dir.path().join("missing");
    assert!(read(Some(&missing)).is_none());
    write(Some(&missing), &cache());
    assert!(!missing.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn corrupt_json_invalid_utf8_and_non_file_are_cache_misses() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("update-check.json");
    fs::write(&path, b"{broken").unwrap();
    assert!(read(Some(dir.path())).is_none());
    fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(read(Some(dir.path())).is_none());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(read(Some(dir.path())).is_none());
}

#[test]
fn writes_exact_pretty_bytes_and_consumes_stage() {
    let dir = tempfile::tempdir().unwrap();
    write(Some(dir.path()), &cache());

    assert_eq!(
        fs::read(dir.path().join("update-check.json")).unwrap(),
        PRETTY.as_bytes()
    );
    assert!(!dir.path().join("update-check.json.tmp").exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn replaces_previous_destination_and_fixed_stage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("update-check.json");
    let stage = dir.path().join("update-check.json.tmp");
    fs::write(&path, b"old destination").unwrap();
    fs::write(
        &stage,
        b"old stage bytes that must be truncated and replaced",
    )
    .unwrap();

    write(Some(dir.path()), &cache());

    assert_eq!(fs::read(&path).unwrap(), PRETTY.as_bytes());
    assert!(!stage.exists());
    assert_eq!(
        read(Some(dir.path())).unwrap().latest_version,
        "1.2.3-beta.4"
    );
}

#[test]
fn failed_stage_write_preserves_destination_and_stage_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("update-check.json");
    let stage = dir.path().join("update-check.json.tmp");
    fs::write(&path, b"old destination").unwrap();
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("sentinel"), b"keep").unwrap();

    write(Some(dir.path()), &cache());

    assert_eq!(fs::read(&path).unwrap(), b"old destination");
    assert_eq!(fs::read(stage.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn failed_rename_leaves_written_stage_and_destination_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("update-check.json");
    let stage = dir.path().join("update-check.json.tmp");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("sentinel"), b"keep").unwrap();

    write(Some(dir.path()), &cache());

    assert_eq!(fs::read(&stage).unwrap(), PRETTY.as_bytes());
    assert_eq!(fs::read(path.join("sentinel")).unwrap(), b"keep");
}
