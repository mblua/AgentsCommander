use super::*;
use std::sync::mpsc;
use std::time::Duration;
use tauri::Listener;

#[test]
fn error_ping_is_delivered_once_with_null_payload() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build logging app");
    let (sender, receiver) = mpsc::channel();
    app.listen_any("error_log_event", move |event| {
        let _ = sender.send(event.payload().to_string());
    });

    app.handle().error_log_event();

    let payload = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("error ping");
    let actual: serde_json::Value = serde_json::from_str(&payload).expect("event JSON");
    assert_eq!(actual, serde_json::Value::Null);
    assert!(receiver.try_recv().is_err(), "exactly one ping");
}
