use super::*;

#[test]
fn reads_utf8_frontmatter_and_crlf_without_changing_bytes() {
    let matrix = tempfile::tempdir().unwrap();
    let bytes = "---\r\nname: 'á'\r\n---\r\n# Role\r\n";
    let expected = matrix.path().join("Role.md");
    std::fs::write(&expected, bytes).unwrap();

    assert_eq!(role_path(matrix.path()), expected);
    assert_eq!(
        role_path(Path::new("missing/../matrix")),
        Path::new("missing/../matrix/Role.md")
    );
    assert!(role_present(matrix.path()));
    assert_eq!(read_role(matrix.path()).unwrap(), bytes);
    assert_eq!(read_role_if_present(matrix.path()).as_deref(), Some(bytes));
}

#[test]
fn absent_role_preserves_not_found() {
    let matrix = tempfile::tempdir().unwrap();

    assert!(!role_present(matrix.path()));
    assert_eq!(
        read_role(matrix.path()).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(read_role_if_present(matrix.path()), None);
}

#[test]
fn directory_named_role_is_present_but_unreadable() {
    let matrix = tempfile::tempdir().unwrap();
    std::fs::create_dir(matrix.path().join("Role.md")).unwrap();

    assert!(role_present(matrix.path()));
    assert!(read_role(matrix.path()).is_err());
    assert_eq!(read_role_if_present(matrix.path()), None);
}

#[test]
fn invalid_utf8_is_rejected_without_replacing_bytes() {
    let matrix = tempfile::tempdir().unwrap();
    let path = matrix.path().join("Role.md");
    let bytes = b"role\xff\r\n";
    std::fs::write(&path, bytes).unwrap();

    assert!(role_present(matrix.path()));
    assert_eq!(
        read_role(matrix.path()).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(read_role_if_present(matrix.path()), None);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn writes_opaque_bytes_and_overwrites_existing_role() {
    let matrix = tempfile::tempdir().unwrap();
    let expected = matrix.path().join("Role.md");
    let bytes = b"\xef\xbb\xbf\0\xff\r\n";

    assert_eq!(write_created_role(matrix.path(), bytes).unwrap(), expected);
    assert_eq!(std::fs::read(&expected).unwrap(), bytes);
    assert_eq!(
        write_created_role(matrix.path(), b"short").unwrap(),
        expected
    );
    assert_eq!(std::fs::read(expected).unwrap(), b"short");
}

#[test]
fn write_fails_without_creating_missing_matrix() {
    let parent = tempfile::tempdir().unwrap();
    let matrix = parent.path().join("missing");
    let expected = std::fs::write(matrix.join("Role.md"), b"role").unwrap_err();

    assert_eq!(
        write_created_role(&matrix, b"role").unwrap_err(),
        format!("Failed to write Role.md: {}", expected)
    );
    assert!(!matrix.exists());
}
