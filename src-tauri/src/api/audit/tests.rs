use super::*;

#[test]
fn sanitize_strips_newlines_and_tabs() {
    assert_eq!(sanitize("a\nb\tc\rd"), "a b c d");
}

#[test]
fn pty_metadata_serialization_has_no_payload_or_secret_field() {
    let metadata = PtyInputAuditMetadata {
        event: "enqueue".into(),
        injection_id: Some(uuid::Uuid::new_v4().to_string()),
        op_id: Some(uuid::Uuid::new_v4().to_string()),
        sender_fqn: Some("project:wg-1-team/lead".into()),
        target_fqn: Some("project:wg-1-team/dev".into()),
        payload_bytes: Some(12),
        payload_sha256: Some("a".repeat(64)),
        source_plane: Some("container_api".into()),
        selected_session_id: None,
        selected_backend: None,
        status: "queued".into(),
        reason_code: None,
        timestamp: "2026-01-01T00:00:00.000Z".into(),
    };
    let value = serde_json::to_value(metadata).unwrap();
    assert!(value.get("payload").is_none());
    assert!(value.get("text").is_none());
    assert!(value.get("token").is_none());
    assert_eq!(value["payloadBytes"], 12);
}

#[test]
fn snapshot_metadata_has_no_content_secret_or_path_fields() {
    let metadata = TerminalSnapshotAuditMetadata {
        event: "terminal_snapshot".into(),
        request_id: Some(uuid::Uuid::new_v4().to_string()),
        requester_fqn: Some("project:wg-1-team/lead".into()),
        target_fqn: Some("project:wg-1-team/dev".into()),
        source_plane: "host_cli".into(),
        format: Some("json".into()),
        selected_session_id: Some(uuid::Uuid::new_v4().to_string()),
        selected_backend: Some("localProcess".into()),
        rows: Some(30),
        columns: Some(120),
        sequence: Some(7),
        captured_at: Some("2026-01-01T00:00:00.000Z".into()),
        payload_bytes: Some(123),
        accepted_at: Some("2026-01-01T00:00:00.000Z".into()),
        completed_at: "2026-01-01T00:00:01.000Z".into(),
        status: "succeeded".into(),
        reason_code: None,
    };
    let diagnostic = format!("{metadata:?}");
    for forbidden in [
        metadata.request_id.as_deref().unwrap(),
        metadata.requester_fqn.as_deref().unwrap(),
        metadata.target_fqn.as_deref().unwrap(),
        metadata.selected_session_id.as_deref().unwrap(),
        metadata.captured_at.as_deref().unwrap(),
        metadata.accepted_at.as_deref().unwrap(),
        metadata.completed_at.as_str(),
        metadata.status.as_str(),
    ] {
        assert!(!diagnostic.contains(forbidden));
    }
    assert!(diagnostic.contains("rows: Some(30)"));
    assert!(diagnostic.contains("payload_bytes: Some(123)"));

    let value = serde_json::to_value(metadata).unwrap();
    for forbidden in [
        "token",
        "nonce",
        "confirmationTag",
        "text",
        "cells",
        "lines",
        "pngBase64",
        "output",
        "path",
        "workingDirectory",
    ] {
        assert!(
            value.get(forbidden).is_none(),
            "forbidden audit field {forbidden}"
        );
    }
}

#[test]
fn append_then_rotate_moves_to_dot_one() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("api-audit.log");
    // Write >10MB worth so rotation triggers on the next check.
    let big = "x".repeat(1024);
    {
        let mut f = std::fs::File::create(&path).unwrap();
        for _ in 0..(AUDIT_MAX_BYTES / 1024 + 1) {
            f.write_all(big.as_bytes()).unwrap();
        }
    }
    assert!(std::fs::metadata(&path).unwrap().len() >= AUDIT_MAX_BYTES);
    rotate_if_needed(&path);
    let rotated = path.with_extension("log.1");
    assert!(rotated.exists(), "rotated .1 file must exist");
    assert!(!path.exists(), "live log must be moved away on rotation");
}

#[test]
fn append_creates_and_appends() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("api-audit.log");
    append(&path, "line1\n").unwrap();
    append(&path, "line2\n").unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "line1\nline2\n");
}

#[test]
fn window_screenshot_statuses_are_redacted_fixed_values() {
    let cases = [
        (WindowScreenshotAuditStatus::Succeeded, "succeeded"),
        (
            WindowScreenshotAuditStatus::InvalidWindowId,
            "invalid_window_id",
        ),
        (
            WindowScreenshotAuditStatus::WindowNotFound,
            "window_not_found",
        ),
        (WindowScreenshotAuditStatus::CaptureBusy, "capture_busy"),
        (
            WindowScreenshotAuditStatus::CaptureTooLarge,
            "capture_too_large",
        ),
        (
            WindowScreenshotAuditStatus::CaptureUnavailable,
            "capture_unavailable",
        ),
    ];

    for (status, expected) in cases {
        let serialized = match serde_json::to_string(&status) {
            Ok(serialized) => serialized,
            Err(error) => panic!("audit status must serialize: {error}"),
        };
        assert_eq!(serialized, format!("\"{expected}\""));
    }
}

#[test]
fn window_screenshot_audit_line_has_only_fixed_redacted_fields() {
    let metadata = WindowScreenshotAuditMetadata::new(WindowScreenshotAuditStatus::Succeeded);
    let line = match serde_json::to_string(&metadata) {
        Ok(line) => line,
        Err(error) => panic!("screenshot audit line must serialize: {error}"),
    };
    let value: serde_json::Value = match serde_json::from_str(&line) {
        Ok(value) => value,
        Err(error) => panic!("screenshot audit line must be JSON: {error}"),
    };
    let object = match value.as_object() {
        Some(object) => object,
        None => panic!("screenshot audit line must be a JSON object"),
    };

    assert_eq!(
        object.get("event").and_then(serde_json::Value::as_str),
        Some("window_screenshot")
    );
    assert_eq!(
        object.get("operation").and_then(serde_json::Value::as_str),
        Some("window_screenshot")
    );
    assert_eq!(
        object.get("status").and_then(serde_json::Value::as_str),
        Some("succeeded")
    );
    assert!(object.contains_key("timestamp"));
    assert_eq!(object.len(), 4);
    for sensitive_field in [
        "window_id",
        "title",
        "pid",
        "token",
        "credential",
        "native_diagnostic",
        "image",
        "image_bytes",
    ] {
        assert!(!object.contains_key(sensitive_field));
    }
}
