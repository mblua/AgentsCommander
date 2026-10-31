use super::*;

#[test]
fn write_publishes_exact_bytes_and_replaces_existing_role() {
    let fixture = tempfile::tempdir().unwrap();
    let replica = fixture.path().join("__agent_variant");
    let expected = replica.join("Role.md");
    for bytes in [b"first\r\nrole".as_slice(), b"replacement\n\0role".as_slice()] {
        write(&replica, bytes).unwrap();
        assert_eq!(std::fs::read(&expected).unwrap(), bytes);
    }
}

#[test]
fn existing_role_preserves_native_existence_and_path() {
    let fixture = tempfile::tempdir().unwrap();
    let room = fixture.path().join("x-fixture");
    let replica = room.join("__agent_variant");
    let expected = replica.join("Role.md");
    assert_eq!(existing_in_room(&room, "variant"), None);
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(&expected, b"role").unwrap();
    assert_eq!(existing_in_room(&room, "variant"), Some(expected.clone()));
    assert_eq!(existing_in_room(&room, "other"), None);
    std::fs::remove_file(&expected).unwrap();
    std::fs::create_dir(&expected).unwrap();
    assert_eq!(existing_in_room(&room, "variant"), Some(expected));
    assert!(write(&replica, b"role").is_err());
}

#[test]
fn recognition_preserves_component_windows_and_platform_separators() {
    let cases = [
        (".ac/x-fixture/__agent_variant/Role.md", true),
        (".ac/y-fixture/__agent_variant/Role.md", true),
        ("prefix/.ac/x-fixture/__agent_variant/Role.md", true),
        (".ac/x-fixture/__agent_variant/Role.md/suffix", true),
        (".ac/x-role-exp-fixture/__agent_variant/Role.md", true),
        (".ac/x-fixture/__agent_/Role.md", true),
        (".ac/x-fixture/_agent_variant/Role.md", false),
        (".AC/x-fixture/__agent_variant/Role.md", false),
        (".ac/x-fixture/__agent_variant/Role.MD", false),
        (".ac/x-fixture/__agent_variant/Other.md", false),
        (".ac/z-fixture/__agent_variant/Role.md", false),
        ("x-fixture/__agent_variant/Role.md", false),
        (".ac/z-no/__agent_a/Role.md/.ac/y-yes/__agent_b/Role.md", true),
        (r".ac\x-fixture\__agent_variant\Role.md", cfg!(windows)),
        (r".ac/x-fixture\__agent_variant/Role.md", cfg!(windows)),
    ];
    for (path, expected) in cases {
        assert_eq!(
            is_replica_role_file(Path::new(path), &["x-", "y-"]),
            expected,
            "{path}"
        );
    }
    let path = Path::new(".ac/x-fixture/__agent_variant/Role.md");
    assert!(!is_replica_role_file(path, &[]));
    assert!(!is_replica_role_file(path, &["y-"]));
}
