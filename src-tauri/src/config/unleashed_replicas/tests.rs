use super::*;

fn store_in(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join(UNLEASHED_REPLICAS_FILE_NAME)
}

#[test]
fn replica_key_normalizes_windows_and_posix_shapes() {
    assert_eq!(
        replica_key(r"C:\A\.ac\room-1-x\__agent_b\"),
        replica_key("c:/a/.ac/room-1-x/__agent_b")
    );
    assert_eq!(
        replica_key("c:/a/.ac/room-1-x/__agent_b"),
        "c:/a/.ac/room-1-x/__agent_b"
    );
    // The verbatim prefix is stripped only on Windows (normalize_windows_verbatim_path).
    #[cfg(windows)]
    assert_eq!(
        replica_key(r"\\?\C:\A\__agent_b"),
        replica_key("c:/a/__agent_b")
    );
    assert_eq!(replica_key("/home/u/__agent_b/"), "/home/u/__agent_b");
    assert_eq!(replica_key("/"), "/");
}

#[test]
fn set_then_load_round_trips_and_sorts() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    set_unleashed_in(&path, "/w/.ac/room-2-t/__agent_z", true).unwrap();
    set_unleashed_in(&path, "/w/.ac/room-1-t/__agent_a", true).unwrap();
    assert_eq!(
        load_from(&path),
        vec!["/w/.ac/room-1-t/__agent_a", "/w/.ac/room-2-t/__agent_z"]
    );
    assert!(is_unleashed_in(&path, "/w/.ac/room-1-t/__agent_a/"));
    let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(root["schemaVersion"], 1);
}

#[test]
fn unset_removes_only_that_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    set_unleashed_in(&path, "/w/__agent_a", true).unwrap();
    set_unleashed_in(&path, "/w/__agent_b", true).unwrap();
    set_unleashed_in(&path, "/w/__agent_a", false).unwrap();
    assert_eq!(load_from(&path), vec!["/w/__agent_b"]);
    assert!(!is_unleashed_in(&path, "/w/__agent_a"));
    assert!(is_unleashed_in(&path, "/w/__agent_b"));
}

#[test]
fn set_twice_keeps_one_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    set_unleashed_in(&path, "/w/__agent_a", true).unwrap();
    set_unleashed_in(&path, "/w/__agent_a/", true).unwrap();
    assert_eq!(load_from(&path), vec!["/w/__agent_a"]);
}

#[test]
fn non_replica_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    for root in ["/w/.ac/ac-root-agent", "/w/.ac/_agent_x"] {
        assert_eq!(
            set_unleashed_in(&path, root, true),
            Err("unleashNotAReplica".to_string())
        );
        assert!(!is_unleashed_in(&path, root));
    }
    assert!(!path.exists());
}

#[test]
fn corrupt_or_wrong_schema_file_reads_as_empty_without_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    for bytes in [
        &b"not json"[..],
        br#"{"schemaVersion": 2, "replicas": ["/w/__agent_a"]}"#,
        br#"{"schemaVersion": 1, "replicas": ["/w/__agent_a", 7]}"#,
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(load_from(&path).is_empty());
        assert!(!is_unleashed_in(&path, "/w/__agent_a"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn missing_file_is_leashed() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    assert!(load_from(&path).is_empty());
    assert!(!is_unleashed_in(&path, "/w/__agent_a"));
    assert!(!path.exists());
}

// #4752 P07: writes the given keys as a store file.
fn store_with(dir: &tempfile::TempDir, keys: &[&str]) -> PathBuf {
    let path = store_in(dir);
    for key in keys {
        set_unleashed_in(&path, key, true).unwrap();
    }
    path
}

#[test]
fn forget_under_removes_room_and_its_replicas_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_with(
        &dir,
        &[
            "/w/.ac/room-1-a/__agent_x",
            "/w/.ac/room-1-a/__agent_y",
            "/w/.ac/room-1-ab/__agent_x",
        ],
    );
    assert_eq!(forget_under_in(&path, "/w/.ac/room-1-a").unwrap(), 2);
    assert_eq!(load_from(&path), vec!["/w/.ac/room-1-ab/__agent_x"]);
}

#[test]
fn forget_under_removes_one_replica() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_with(
        &dir,
        &["/w/.ac/room-1-a/__agent_x", "/w/.ac/room-1-a/__agent_y"],
    );
    assert_eq!(
        forget_under_in(&path, "/w/.ac/room-1-a/__agent_x").unwrap(),
        1
    );
    assert_eq!(load_from(&path), vec!["/w/.ac/room-1-a/__agent_y"]);
}

#[test]
fn forget_under_matches_windows_path_variants() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_with(&dir, &["c:/a/.ac/room-1-a/__agent_x"]);
    assert_eq!(forget_under_in(&path, r"C:\A\.ac\room-1-a\").unwrap(), 1);
    assert!(load_from(&path).is_empty());
}

#[test]
fn forget_under_missing_store_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    assert_eq!(forget_under_in(&path, "/w/.ac/room-1-a").unwrap(), 0);
    assert!(!path.exists());
}

#[test]
fn temp_room_in_store_dir_can_be_unleashed_and_leashed() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    let room = dir.path().join("temp-room-tr03");
    let room = room.to_string_lossy();
    set_unleashed_in(&path, &room, true).unwrap();
    assert!(is_unleashed_in(&path, &room));
    set_unleashed_in(&path, &room, false).unwrap();
    assert!(!is_unleashed_in(&path, &room));
}

#[test]
fn temp_room_under_other_base_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    let room = other.path().join("temp-room-tr03");
    let room = room.to_string_lossy();
    assert_eq!(
        set_unleashed_in(&path, &room, true).unwrap_err(),
        "unleashNotAReplica"
    );
    let doc = serde_json::json!({ "schemaVersion": 1, "replicas": [replica_key(&room)] });
    std::fs::write(&path, doc.to_string()).unwrap();
    assert!(!is_unleashed_in(&path, &room));
}

#[test]
fn non_canonical_temp_room_name_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = store_in(&dir);
    for name in ["temp-room-tr1", "temp-room-tr00"] {
        let room = dir.path().join(name);
        let err = set_unleashed_in(&path, &room.to_string_lossy(), true).unwrap_err();
        assert_eq!(err, "unleashNotAReplica", "{name}");
    }
}
