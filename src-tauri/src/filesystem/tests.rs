use super::*;

#[test]
fn reads_the_complete_utf8_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    let document = "{\"categories\":{}}\nñ";
    std::fs::write(&path, document).unwrap();

    assert_eq!(read_capture_catalog(&path).unwrap(), document);
}

#[test]
fn preserves_a_missing_file_error() {
    let directory = tempfile::tempdir().unwrap();
    let error = read_capture_catalog(&directory.path().join("missing.json")).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn preserves_an_invalid_utf8_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    std::fs::write(&path, [0xff]).unwrap();
    let expected = std::fs::read_to_string(&path).unwrap_err();
    let actual = read_capture_catalog(&path).unwrap_err();

    assert_eq!(actual.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(actual.to_string(), expected.to_string());
    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
}

#[test]
fn preserves_a_directory_read_error() {
    let directory = tempfile::tempdir().unwrap();
    let expected = std::fs::read_to_string(directory.path()).unwrap_err();
    let actual = read_capture_catalog(directory.path()).unwrap_err();

    assert_ne!(actual.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(actual.kind(), expected.kind());
    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
    assert_eq!(actual.to_string(), expected.to_string());
}

#[cfg(unix)]
#[test]
fn follows_a_symlink_to_the_document() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("catalog.json");
    let link = directory.path().join("catalog-link.json");
    std::fs::write(&target, "catalog contents").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert_eq!(read_capture_catalog(&link).unwrap(), "catalog contents");
}

#[test]
fn issue_3278_sessions_publish_complete_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.json");
    let tmp = dir.path().join("snapshot.tmp");
    for document in [r#"{"sessions":[]}"#, r#"{"sessions":[{"name":"ñ"}]}"#] {
        write_session_container_stage(&tmp, document.to_owned()).unwrap();
        assert_eq!(std::fs::read(&tmp).unwrap(), document.as_bytes());
        replace_session_container_file(&tmp, &path).unwrap();
        let actual = std::fs::read(&path).unwrap();
        assert_eq!(actual, document.as_bytes());
        serde_json::from_slice::<serde_json::Value>(&actual).unwrap();
        assert!(!tmp.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn issue_3278_sessions_rename_failure_preserves_stage() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("snapshot.tmp");
    let dst = dir.path().join("destination");
    std::fs::create_dir(&dst).unwrap();
    let sentinel = dst.join("previous");
    std::fs::write(&sentinel, b"previous").unwrap();
    write_session_container_stage(&tmp, r#"{"sessions":[]}"#.to_owned()).unwrap();
    assert!(replace_session_container_file(&tmp, &dst).is_err());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"previous");
    assert!(tmp.is_file());
}

#[cfg(unix)]
#[test]
fn issue_3278_parent_directory_sync_success() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("snapshot.tmp");
    let dst = dir.path().join("snapshot.json");
    write_session_container_stage(&tmp, r#"{"sessions":[]}"#.to_owned()).unwrap();
    replace_session_container_file(&tmp, &dst).unwrap();
    sync_session_container_parent(&dst).unwrap();
}

#[cfg(unix)]
#[test]
fn issue_3278_parent_directory_sync_missing() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("missing").join("snapshot.json");
    assert!(sync_session_container_parent(&dst).is_err());
}

#[cfg(windows)]
struct Issue3278SyncProbe {
    directory: std::path::PathBuf,
    expected: Vec<u8>,
    observed: Option<std::path::PathBuf>,
}

#[cfg(windows)]
thread_local! {
    static ISSUE_3278_SYNC_PROBE: std::cell::RefCell<Option<Issue3278SyncProbe>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(windows)]
struct Issue3278SyncGuard;

#[cfg(windows)]
impl Issue3278SyncGuard {
    fn arm(directory: &std::path::Path, expected: Vec<u8>) -> Self {
        ISSUE_3278_SYNC_PROBE.with_borrow_mut(|slot| {
            assert!(slot.is_none());
            *slot = Some(Issue3278SyncProbe {
                directory: directory.to_owned(),
                expected,
                observed: None,
            });
        });
        Self
    }

    fn assert_observed(&self) {
        ISSUE_3278_SYNC_PROBE.with_borrow(|slot| {
            assert!(slot.as_ref().unwrap().observed.is_some());
        });
    }
}

#[cfg(windows)]
impl Drop for Issue3278SyncGuard {
    fn drop(&mut self) {
        ISSUE_3278_SYNC_PROBE.with_borrow_mut(|slot| *slot = None);
    }
}

#[cfg(windows)]
pub(super) fn issue_3278_sync_handle(
    path: &std::path::Path,
    file: std::fs::File,
) -> std::io::Result<std::fs::File> {
    ISSUE_3278_SYNC_PROBE.with_borrow_mut(|slot| {
        let Some(probe) = slot.as_mut() else {
            return Ok(file);
        };
        if path.parent() != Some(probe.directory.as_path()) {
            return Ok(file);
        }
        assert!(probe.observed.is_none());
        assert_eq!(path, probe.directory.join("snapshot.tmp"));
        assert_eq!(std::fs::read(path)?, probe.expected);
        drop(file);
        let readonly = std::fs::File::open(path)?;
        probe.observed = Some(path.to_owned());
        Ok(readonly)
    })
}

#[cfg(windows)]
#[test]
fn issue_3278_sync_sessions_failure_keeps_previous() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.json");
    let previous = br#"{"previous":true}"#;
    std::fs::write(&path, previous).unwrap();
    // Positive control: a successful readonly open must fail at sync, not open/write.
    let readonly = std::fs::File::open(&path).unwrap();
    let control_error = readonly.sync_all().unwrap_err();
    assert_eq!(control_error.raw_os_error(), Some(5));
    drop(readonly);
    let document = r#"{"sessions":[{"name":"ñ"}]}"#.to_owned();
    let expected = document.as_bytes().to_vec();
    let tmp = dir.path().join("snapshot.tmp");
    let guard = Issue3278SyncGuard::arm(dir.path(), expected);
    let result = write_session_container_stage(&tmp, document)
        .and_then(|()| replace_session_container_file(&tmp, &path));
    let error = result.unwrap_err();
    assert_eq!(error.raw_os_error(), control_error.raw_os_error());
    guard.assert_observed();
    // Match the existing session caller: cleanup only its stage on prepare error.
    remove_session_container_stage(&tmp).unwrap();
    assert!(!tmp.exists());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
