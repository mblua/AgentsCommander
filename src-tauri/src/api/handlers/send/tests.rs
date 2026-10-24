use super::*;

#[test]
fn issue_3081_archive_failure_preserves_durable_queue_and_response() {
    let temp = tempfile::tempdir().unwrap();
    let room = temp.path().join(".ac/wg-1-team");
    let replica = room.join("__agent_a");
    let messaging = room.join("messaging");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::create_dir_all(&messaging).unwrap();
    std::fs::write(temp.path().join(".ac/message-archive"), b"blocked").unwrap();
    let name = "20261017-000000-wg1-a-to-wg1-b-snapshot.md";
    std::fs::write(messaging.join(name), b"\xef\xbb\xbfhello\r\n").unwrap();
    let req: SendRequest = serde_json::from_value(serde_json::json!({
        "apiVersion":"1", "opId":"archive-op", "to":"proj:wg-1-team/b", "message":{"send":name}
    }))
    .unwrap();
    let payload = extract_payload(replica.to_str().unwrap(), &req).unwrap();
    assert!(payload
        .archive_warning
        .as_ref()
        .unwrap()
        .contains("Message archive warning"));
    assert_eq!(payload.body.as_bytes(), b"\xef\xbb\xbfhello\r\n");
    let store =
        crate::api::message_store::MessageStore::open(temp.path().join("queue.sqlite")).unwrap();
    let result = store
        .enqueue(EnqueueRequest {
            sender_fqn: "proj:wg-1-team/a".into(),
            target_fqn: req.to.clone(),
            op_id: req.op_id.clone(),
            content_type: DEFAULT_CONTENT_TYPE.into(),
            body: payload.body,
            source_plane: payload.source_plane,
            source_ref: payload.source_ref,
        })
        .unwrap();
    let (status, response) = queued_response(
        &result.op_id,
        &result.target_fqn,
        &result.message_id,
        payload.archive_warning,
    );
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(response.status, "queued");
    assert_eq!(response.op_id, req.op_id);
    assert_eq!(response.to, req.to);
    assert!(!response.message_id.is_empty());
    assert!(response.detail.unwrap().contains("Message archive warning"));
}

#[test]
fn issue_3081_inline_and_enqueue_errors_keep_their_contract() {
    let req: SendRequest = serde_json::from_str(
        r#"{"apiVersion":"1","opId":"inline","to":"proj/a","message":{"inline":"body"}}"#,
    )
    .unwrap();
    let payload = extract_payload("unused", &req).unwrap();
    assert!(payload.archive_warning.is_none());
    assert!(
        queued_response("inline", "proj/a", "id", payload.archive_warning)
            .1
            .detail
            .is_none()
    );
    let error = store_error_with_archive_warning(
        MessageStoreError::BodyTooLarge,
        Some("Message archive warning: conflict"),
    );
    assert_eq!(error.status(), StatusCode::PAYLOAD_TOO_LARGE);
    match error {
        ApiError::PayloadTooLarge(detail) => {
            assert!(detail.contains("inline cap"));
            assert!(detail.contains("archive warning"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn extract_payload_accepts_inline() {
    let req: SendRequest = serde_json::from_str(
        r#"{"apiVersion":"1","opId":"o","to":"proj/agent","message":{"inline":"hello"}}"#,
    )
    .unwrap();

    let payload = extract_payload("unused", &req).unwrap();

    assert_eq!(payload.body, "hello");
    assert_eq!(payload.source_plane, "api_inline");
    assert_eq!(payload.source_ref, None);
}

#[test]
fn extract_payload_rejects_both_send_and_inline() {
    let req: SendRequest =
        serde_json::from_str(include_str!("tests/fixtures/send-and-inline.json")).unwrap();

    let err = extract_payload("unused", &req).unwrap_err();

    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn extract_payload_rejects_oversize_inline_with_413() {
    let req = SendRequest {
        api_version: crate::api::schema::API_VERSION.to_string(),
        op_id: "o".to_string(),
        to: "proj/agent".to_string(),
        message: crate::api::schema::SendMessage {
            send: None,
            inline: Some("x".repeat(INLINE_BODY_MAX_BYTES + 1)),
            content_type: None,
        },
    };

    let err = extract_payload("unused", &req).unwrap_err();

    assert_eq!(err.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
