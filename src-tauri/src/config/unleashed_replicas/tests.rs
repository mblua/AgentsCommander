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
