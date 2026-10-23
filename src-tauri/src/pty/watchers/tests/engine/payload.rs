use super::*;

/// 9.6.74 and 9.6.75 - **the Rust half of the TypeScript mirror.** The exact camelCase
/// JSON, field for field, with EVERY field present: no `skip_serializing_if` anywhere, so
/// absent can never become a third state beside null and the value.
#[test]
fn the_payload_serializes_to_the_exact_camel_case_contract() {
    let batch = WatcherMatchBatch {
        session_id: "11111111-2222-3333-4444-555555555555".to_string(),
        matches: vec![WatcherMatchPayload {
            session_id: "11111111-2222-3333-4444-555555555555".to_string(),
            seq: 7,
            watcher_id: "permission-prompt".to_string(),
            mode: WatcherMode::Occurrence,
            at: chrono::DateTime::parse_from_rfc3339("2026-07-30T22:31:05Z")
                .expect("fixed instant")
                .with_timezone(&chrono::Utc),
            captures: vec![Some("C:/repo/main.rs".to_string()), None],
            row: "Read (C:/repo/main.rs)".to_string(),
            row_truncated: false,
        }],
    };

    assert_eq!(
        serde_json::to_value(&batch).expect("serializes"),
        serde_json::json!({
            "sessionId": "11111111-2222-3333-4444-555555555555",
            "matches": [{
                "sessionId": "11111111-2222-3333-4444-555555555555",
                "seq": 7,
                "watcherId": "permission-prompt",
                "mode": "occurrence",
                "at": "2026-07-30T22:31:05Z",
                "captures": ["C:/repo/main.rs", null],
                "row": "Read (C:/repo/main.rs)",
                "rowTruncated": false
            }]
        })
    );
}

/// The empty cases are present too, and are not elided: `captures: []` is a pattern with
/// no groups, and the UI falls back to the raw row on it.
#[test]
fn an_empty_capture_list_and_a_false_flag_are_both_written() {
    let payload = WatcherMatchPayload {
        session_id: "s".to_string(),
        seq: 1,
        watcher_id: "w".to_string(),
        mode: WatcherMode::State,
        at: chrono::DateTime::parse_from_rfc3339("2026-07-30T00:00:00Z")
            .expect("fixed instant")
            .with_timezone(&chrono::Utc),
        captures: Vec::new(),
        row: "Permission required".to_string(),
        row_truncated: false,
    };

    let json = serde_json::to_value(&payload).expect("serializes");
    let object = json.as_object().expect("object");
    assert_eq!(object.len(), 8, "every field, always: {json}");
    assert_eq!(object["captures"], serde_json::json!([]));
    assert_eq!(object["rowTruncated"], serde_json::json!(false));
    assert_eq!(object["mode"], serde_json::json!("state"));
}
