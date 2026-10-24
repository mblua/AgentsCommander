use super::*;

#[test]
fn buffer_drains_one_session_without_touching_another() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut buffer = SessionWarningBuffer::default();
    buffer.push(SessionWarning::new(first, "A", "kind", "first"));
    buffer.push(SessionWarning::new(second, "B", "kind", "second"));

    let drained = buffer.drain(Some(&first.to_string()));

    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].message, "first");
    assert_eq!(buffer.drain(Some(&second.to_string())).len(), 1);
}

#[test]
fn session_warning_serializes_frontend_contract() {
    let session_id = Uuid::nil();
    let warning = SessionWarning::new(session_id, "KEY", "kind", "message");

    let value = serde_json::to_value(warning).unwrap();

    assert_eq!(value["sessionId"], session_id.to_string());
    assert_eq!(value["key"], "KEY");
    assert_eq!(value["kind"], "kind");
    assert_eq!(value["message"], "message");
}
