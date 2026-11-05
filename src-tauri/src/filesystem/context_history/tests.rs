use super::*;

#[test]
fn ensure_creates_only_the_directory_and_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("history");
    ensure_history_directory(&directory).unwrap();
    assert!(directory.is_dir());
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
    ensure_history_directory(&directory).unwrap();
    assert!(directory.is_dir());
}

#[test]
fn ensure_does_not_create_missing_ancestors() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("missing").join("history");
    assert!(ensure_history_directory(&directory).is_err());
    assert!(!root.path().join("missing").exists());
}

#[test]
fn ensure_rejects_a_regular_file() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("history");
    fs::write(&directory, b"x").unwrap();
    assert!(ensure_history_directory(&directory).is_err());
}

#[test]
fn inspect_missing_returns_none_without_creating() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("history");
    let database = directory.join("history.sqlite3");
    assert_eq!(inspect_history_database(&database).unwrap(), None);
    assert!(!directory.exists());
    fs::create_dir(&directory).unwrap();
    assert_eq!(inspect_history_database(&database).unwrap(), None);
    assert!(!database.exists());
}

#[test]
fn inspect_reads_full_100_byte_header() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("history.sqlite3");
    let bytes: Vec<u8> = (0..100u8).collect();
    fs::write(&database, &bytes).unwrap();
    let inspection = inspect_history_database(&database).unwrap().unwrap();
    assert_eq!(inspection.length, 100);
    assert_eq!(inspection.header, bytes);
}

#[test]
fn inspect_reports_short_header_without_padding() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("history.sqlite3");
    fs::write(&database, b"SQLite").unwrap();
    let inspection = inspect_history_database(&database).unwrap().unwrap();
    assert_eq!(inspection.length, 6);
    assert_eq!(inspection.header, b"SQLite");
}

#[test]
fn inspect_large_file_reads_only_first_100_bytes() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("history.sqlite3");
    let bytes: Vec<u8> = (0..1_000_000u32).map(|value| (value % 251) as u8).collect();
    fs::write(&database, &bytes).unwrap();
    let inspection = inspect_history_database(&database).unwrap().unwrap();
    assert_eq!(inspection.length, 1_000_000);
    assert_eq!(inspection.header, &bytes[..HISTORY_HEADER_MAX_BYTES]);
}

#[test]
fn inspect_rejects_a_directory() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("history.sqlite3");
    fs::create_dir(&database).unwrap();
    assert!(inspect_history_database(&database).is_err());
}

#[cfg(unix)]
#[test]
fn inspect_rejects_unreadable_file() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("history.sqlite3");
    fs::write(&database, b"SQLite").unwrap();
    fs::set_permissions(&database, fs::Permissions::from_mode(0o000)).unwrap();
    // Root bypasses permission bits; only assert where they apply.
    if File::open(&database).is_err() {
        assert!(inspect_history_database(&database).is_err());
    }
}

#[cfg(unix)]
#[test]
fn symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let target_dir = root.path().join("target");
    fs::create_dir(&target_dir).unwrap();
    let target_file = target_dir.join("real.sqlite3");
    fs::write(&target_file, b"SQLite").unwrap();

    let linked_dir = root.path().join("history");
    symlink(&target_dir, &linked_dir).unwrap();
    assert!(ensure_history_directory(&linked_dir).is_err());
    assert!(inspect_history_database(&linked_dir.join("real.sqlite3")).is_err());
    assert!(ensure_history_directory(&linked_dir.join("nested")).is_err());
    assert!(!target_dir.join("nested").exists());

    let linked_file = target_dir.join("history.sqlite3");
    symlink(&target_file, &linked_file).unwrap();
    assert!(inspect_history_database(&linked_file).is_err());
}

#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .expect("failed to invoke mklink");
    assert!(output.status.success(), "mklink /J failed");
}

#[cfg(windows)]
#[test]
fn junctions_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let target_dir = root.path().join("target");
    fs::create_dir(&target_dir).unwrap();
    fs::write(target_dir.join("real.sqlite3"), b"SQLite").unwrap();

    let linked_dir = root.path().join("history");
    create_junction(&linked_dir, &target_dir);
    assert!(ensure_history_directory(&linked_dir).is_err());
    assert!(inspect_history_database(&linked_dir.join("real.sqlite3")).is_err());
    assert!(ensure_history_directory(&linked_dir.join("nested")).is_err());
    assert!(!target_dir.join("nested").exists());
}
