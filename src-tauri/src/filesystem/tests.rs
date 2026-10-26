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
fn transcript_path_preserves_canonical_and_removed_non_ascii_spellings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(".").join("transcripción-ñ.jsonl");
    std::fs::write(&path, "transcript").unwrap();

    assert_eq!(
        directories::canonical_transcript_path(&path),
        std::fs::canonicalize(&path).unwrap()
    );

    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        directories::canonical_transcript_path(&path).as_os_str(),
        path.as_os_str()
    );
}

#[test]
fn project_children_visit_immediately_and_stop_before_another_callback() {
    use std::ops::ControlFlow;

    let directory = tempfile::tempdir().unwrap();
    for name in ["first", "second", "third"] {
        std::fs::write(directory.path().join(name), "before").unwrap();
    }
    let mut visited = Vec::new();
    let outcome = projects::visit_project_kind_children(directory.path(), |entry| {
        let path = entry.unwrap();
        assert!(visited.is_empty());
        std::fs::write(&path, "during callback").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "during callback");
        visited.push(path);
        ControlFlow::Break(())
    })
    .unwrap();

    assert_eq!(outcome, ControlFlow::Break(()));
    assert_eq!(visited.len(), 1);
    let changed = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .filter(|contents| contents == "during callback")
        .count();
    assert_eq!(changed, 1);
}

#[test]
fn project_children_continue_after_a_child_metadata_error() {
    use std::ops::ControlFlow;

    let directory = tempfile::tempdir().unwrap();
    for name in ["one", "two", "three"] {
        std::fs::write(directory.path().join(name), "child").unwrap();
    }
    let mut failures = Vec::new();
    let outcome = projects::visit_project_kind_children(directory.path(), |entry| {
        let path = entry.unwrap();
        std::fs::remove_file(&path).unwrap();
        failures.push(projects::project_path_metadata(&path).unwrap_err().kind());
        ControlFlow::Continue(())
    })
    .unwrap();

    assert_eq!(outcome, ControlFlow::Continue(()));
    assert_eq!(failures, vec![std::io::ErrorKind::NotFound; 3]);
}

#[test]
fn project_children_preserve_open_errors_without_calling_the_visitor() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing");
    let expected = std::fs::read_dir(&path).unwrap_err();
    let actual = projects::visit_project_kind_children(&path, |_| {
        panic!("an open error must not reach the callback")
    })
    .unwrap_err();

    assert_eq!(actual.kind(), expected.kind());
    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
    assert_eq!(actual.to_string(), expected.to_string());
}
