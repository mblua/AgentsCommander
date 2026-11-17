use super::*;

fn names(base: &Path) -> Vec<(String, bool)> {
    let mut found: Vec<_> = list_child_entries(base)
        .expect("list")
        .into_iter()
        .map(|entry| (entry.name, entry.real_dir))
        .collect();
    found.sort();
    found
}

#[test]
fn list_marks_only_real_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join("temp-room-tr01")).expect("dir");
    std::fs::write(tmp.path().join("temp-room-tr02"), b"x").expect("file");
    assert_eq!(
        names(tmp.path()),
        vec![
            ("temp-room-tr01".to_string(), true),
            ("temp-room-tr02".to_string(), false),
        ]
    );
}

#[cfg(unix)]
#[test]
fn list_marks_a_symlink_to_a_dir_as_not_real() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("target");
    std::fs::create_dir(&target).expect("dir");
    std::os::unix::fs::symlink(&target, tmp.path().join("temp-room-tr03")).expect("link");
    assert_eq!(
        names(tmp.path()),
        vec![
            ("target".to_string(), true),
            ("temp-room-tr03".to_string(), false),
        ]
    );
}

#[test]
fn list_of_a_missing_base_is_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(names(&tmp.path().join("missing")).is_empty());
}

#[test]
fn create_dir_exclusive_reports_an_existing_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("nested").join("temp-room-tr01");
    create_dir_exclusive(&dir).expect("first create");
    assert!(dir.is_dir());
    let err = create_dir_exclusive(&dir).expect_err("second create");
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
}
