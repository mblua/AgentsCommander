use super::*;
use crate::phone::types::{PtyInputPublicStatus, PtyInputResult};
use std::sync::mpsc;
use tauri::Listener;

fn assert_delivery(result: PtyInputResult, expected: serde_json::Value) {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build phone event app");
    let (sender, receiver) = mpsc::sync_channel(2);
    app.listen_any("pty_input_status", move |event| {
        sender
            .try_send(event.payload().to_string())
            .expect("bounded event recorder");
    });
    let original = result.clone();
    assert!(app.handle().pty_input_status(&result), "emission succeeded");
    let payload = receiver.try_recv().expect("synchronous status arrival");
    let actual: serde_json::Value = serde_json::from_str(&payload).expect("event JSON");
    assert_eq!(actual, expected);
    assert_eq!(result, original, "borrowed input unchanged");
    assert!(receiver.try_recv().is_err(), "exactly one status event");
}

#[test]
fn all_statuses_have_exact_minimal_payloads() {
    for (status, literal) in [
        (PtyInputPublicStatus::Queued, "queued"),
        (PtyInputPublicStatus::Actuating, "actuating"),
        (PtyInputPublicStatus::Injected, "injected"),
        (PtyInputPublicStatus::Rejected, "rejected"),
        (PtyInputPublicStatus::Indeterminate, "indeterminate"),
    ] {
        let terminal = matches!(literal, "injected" | "rejected" | "indeterminate");
        assert_delivery(
            PtyInputResult::new("injection".into(), status),
            serde_json::json!({
                "version": 1, "injectionId": "injection", "status": literal,
                "terminal": terminal
            }),
        );
    }
}

#[test]
fn populated_optional_fields_and_reason_are_preserved() {
    let expected = serde_json::json!({
        "version": 1, "injectionId": "injection", "opId": "operation",
        "sender": "sender", "target": "target", "status": "rejected", "terminal": true,
        "payloadBytes": 7, "payloadSha256": "digest", "sourcePlane": "host_cli",
        "selectedSessionId": "session", "selectedBackend": "local",
        "issuedAt": "issued", "expiresAt": "expires", "queuedAt": "queued",
        "actuatingAt": "actuating", "terminalAt": "terminal",
        "reason": {"code": "unsafe_path", "detail": "reason detail"}
    });
    let result = serde_json::from_value(expected.clone()).expect("populated result");
    assert_delivery(result, expected);
}
