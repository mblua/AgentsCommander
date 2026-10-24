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
