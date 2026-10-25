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

    assert_eq!(
        value,
        serde_json::json!({
            "sessionId": session_id.to_string(), "key": "KEY", "kind": "kind", "message": "message"
        })
    );
    assert_eq!(value["sessionId"], session_id.to_string());
    assert_eq!(value["key"], "KEY");
    assert_eq!(value["kind"], "kind");
    assert_eq!(value["message"], "message");
}

#[test]
fn warning_port_is_called_once_before_return_with_exact_contents() {
    #[derive(Default)]
    struct Recorder(std::cell::RefCell<Vec<SessionWarning>>);

    impl SessionWarningEvents for Recorder {
        fn session_warning(&self, warning: SessionWarning) {
            self.0.borrow_mut().push(warning);
        }
    }

    let events = Recorder::default();
    let warning = SessionWarning::new(Uuid::nil(), "KEY", "kind", "message");
    let port: &dyn SessionWarningEvents = &events;
    emit_session_warning(port, warning.clone());
    assert_eq!(*events.0.borrow(), vec![warning]);
}

#[test]
fn capacity_evicts_only_oldest_warning_of_the_full_session() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut buffer = SessionWarningBuffer::default();
    let other = SessionWarning::new(second, "OTHER", "kind", "other");
    buffer.push(other.clone());
    for i in 0..33 {
        buffer.push(SessionWarning::new(first, "KEY", "kind", i.to_string()));
    }
    let drained = buffer.drain(Some(&first.to_string()));
    assert_eq!(drained.len(), 32);
    assert_eq!(
        drained
            .iter()
            .map(|w| w.message.clone())
            .collect::<Vec<_>>(),
        (1..33).map(|i| i.to_string()).collect::<Vec<_>>()
    );
    assert!(buffer.drain(Some(&first.to_string())).is_empty());
    assert_eq!(buffer.drain(None), vec![other]);
}
