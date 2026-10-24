use super::*;

#[test]
fn issue_3081_bound_room_archives_exact_validated_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let room = temp.path().join("project/.ac/wg-1-team");
    let replica = room.join("__agent_a");
    let messaging = room.join("messaging");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::create_dir_all(&messaging).unwrap();
    let name = "20261017-000000-wg1-a-to-wg1-b-snapshot.md";
    let bytes = b"\xef\xbb\xbf# hello\r\n\x00";
    std::fs::write(messaging.join(name), bytes).unwrap();
    let content = read_send_file_content(replica.to_str().unwrap(), name).unwrap();
    assert_eq!(content.body.as_bytes(), bytes);
    assert!(
        content.archive_warning.is_none(),
        "{:?}",
        content.archive_warning
    );
    let archived = room
        .parent()
        .unwrap()
        .join("message-archive/20261017")
        .join(name);
    assert_eq!(std::fs::read(&archived).unwrap(), bytes);
    std::fs::remove_dir_all(&room).unwrap();
    assert_eq!(std::fs::read(archived).unwrap(), bytes);
}

#[test]
fn reject_if_root_blocks_root_target_and_sender() {
    let root = crate::config::root_agent::ROOT_AGENT_SENDER;
    assert!(reject_if_root("proj:wg-1/dev", "proj:wg-1/lead").is_ok());
    assert!(reject_if_root(root, "proj:wg-1/lead").is_err());
    assert!(reject_if_root("proj:wg-1/dev", root).is_err());
    assert_eq!(
        reject_if_root(root, "x").unwrap_err().status(),
        axum::http::StatusCode::FORBIDDEN
    );
}

#[test]
fn build_send_body_resolves_existing_message_and_formats_pointer() {
    // Real WG-replica layout with a messaging file present.
    let temp = tempfile::TempDir::new().unwrap();
    let wg_root = temp.path().join("proj-x").join(".ac").join("wg-1-team");
    let replica = wg_root.join("__agent_dev-rust");
    let messaging = wg_root.join("messaging");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::create_dir_all(&messaging).unwrap();
    let fname = "20260704-000000-wg1-a-to-wg1-b-hello.md";
    std::fs::write(messaging.join(fname), "hi").unwrap();

    let body = build_send_body(
        replica.to_str().unwrap(),
        fname,
        "proj-x:wg-1-team/dev-rust",
    )
    .expect("existing message should resolve");
    assert!(body.starts_with(crate::phone::messaging::FILE_NOTIFICATION_PREFIX));
    assert!(body.contains(fname));
    // The container path is never embedded: the body carries a host path.
    assert!(!body.contains("/workspace/"));
}

/// #1614 requirement (D), section 9.1 "API and snapshot fixture twins".
/// The `wg-` fixture above is deliberately NOT converted (Rule P2); the
/// Room case is added beside it, carrying the `room1-` short token that
/// section 5.9's `parse_wg_prefix` now produces for a Room replica.
#[test]
fn build_send_body_resolves_a_room_replica_message() {
    let temp = tempfile::TempDir::new().unwrap();
    let room_root = temp.path().join("proj-x").join(".ac").join("room-1-team");
    let replica = room_root.join("__agent_dev-rust");
    let messaging = room_root.join("messaging");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::create_dir_all(&messaging).unwrap();
    let fname = "20260704-000000-room1-a-to-room1-b-hello.md";
    std::fs::write(messaging.join(fname), "hi").unwrap();

    let body = build_send_body(
        replica.to_str().unwrap(),
        fname,
        "proj-x:room-1-team/dev-rust",
    )
    .expect("existing Room message should resolve");
    assert!(body.starts_with(crate::phone::messaging::FILE_NOTIFICATION_PREFIX));
    assert!(body.contains(fname));
    assert!(!body.contains("/workspace/"));
}

#[test]
fn build_send_body_rejects_path_traversal_basename() {
    let temp = tempfile::TempDir::new().unwrap();
    let replica = temp
        .path()
        .join("proj")
        .join(".ac")
        .join("wg-1")
        .join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    let err =
        build_send_body(replica.to_str().unwrap(), "..\\secret.md", "proj:wg-1/x").unwrap_err();
    assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[test]
fn build_send_body_rejects_missing_message_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let wg_root = temp.path().join("proj").join(".ac").join("wg-1");
    let replica = wg_root.join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::create_dir_all(wg_root.join("messaging")).unwrap();
    let err = build_send_body(replica.to_str().unwrap(), "nope.md", "proj:wg-1/x").unwrap_err();
    assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[test]
fn read_send_file_content_ingests_utf8_body_without_host_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let wg_root = temp.path().join("proj-x").join(".ac").join("wg-1-team");
    let replica = wg_root.join("__agent_dev-rust");
    let messaging = wg_root.join("messaging");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::create_dir_all(&messaging).unwrap();
    let fname = "20260704-000000-wg1-a-to-wg1-b-hello.md";
    std::fs::write(messaging.join(fname), "hello body").unwrap();

    let got = read_send_file_content(replica.to_str().unwrap(), fname).unwrap();

    assert_eq!(got.body, "hello body");
    assert_eq!(got.source_ref, fname);
    assert!(!got.body.contains("Process this inter-agent message:"));
}

#[test]
fn read_send_file_content_rejects_oversize_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let wg_root = temp.path().join("proj-x").join(".ac").join("wg-1-team");
    let replica = wg_root.join("__agent_dev-rust");
    let messaging = wg_root.join("messaging");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::create_dir_all(&messaging).unwrap();
    let fname = "20260704-000000-wg1-a-to-wg1-b-large.md";
    std::fs::write(messaging.join(fname), vec![b'x'; INLINE_BODY_MAX_BYTES + 1]).unwrap();

    let err = read_send_file_content(replica.to_str().unwrap(), fname).unwrap_err();

    assert_eq!(err.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
}
