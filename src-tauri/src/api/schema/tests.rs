use super::*;

#[test]
fn send_request_rejects_unknown_top_level_field() {
    // The forbidden identity fields are "unknown" to the DTO, so
    // deny_unknown_fields rejects them at parse time.
    for forbidden in ["from", "root", "token", "command", "action"] {
        let json = format!(
            include_str!("tests/fixtures/forbidden-field.json"),
            forbidden
        );
        let parsed: Result<SendRequest, _> = serde_json::from_str(&json);
        assert!(
            parsed.is_err(),
            "body field '{}' must be rejected by deny_unknown_fields",
            forbidden
        );
    }
}

#[test]
fn send_message_rejects_unknown_field() {
    let json = r#"{"apiVersion":"1","opId":"o","to":"a/b","message":{"send":"f.md","bogus":1}}"#;
    let parsed: Result<SendRequest, _> = serde_json::from_str(json);
    assert!(parsed.is_err(), "unknown message field must be rejected");
}

#[test]
fn send_request_parses_minimal_valid_body() {
    let json =
        r#"{"apiVersion":"1","opId":"o","to":"proj/agent","message":{"send":"20260101-x.md"}}"#;
    let req: SendRequest = serde_json::from_str(json).expect("valid body should parse");
    assert_eq!(req.api_version, "1");
    assert_eq!(req.op_id, "o");
    assert_eq!(req.to, "proj/agent");
    assert_eq!(req.message.send.as_deref(), Some("20260101-x.md"));
    assert!(req.message.inline.is_none());
    assert!(req.message.content_type.is_none());
}

#[test]
fn health_response_is_exactly_ok_true() {
    let json = serde_json::to_value(HealthResponse { ok: true }).unwrap();
    assert_eq!(json, serde_json::json!({ "ok": true }));
    // No other field may serialize (§0.5 G9).
    assert_eq!(json.as_object().unwrap().len(), 1);
}

#[test]
fn send_response_queued_has_message_id_and_camel_case() {
    let json = serde_json::to_value(SendResponse::queued("op1", "proj/agent", "msg1")).unwrap();
    assert_eq!(json["apiVersion"], "1");
    assert_eq!(json["opId"], "op1");
    assert_eq!(json["status"], "queued");
    assert_eq!(json["to"], "proj/agent");
    assert_eq!(json["messageId"], "msg1");
    assert!(json["detail"].is_null());
}
