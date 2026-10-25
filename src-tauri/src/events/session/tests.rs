use super::*;
use crate::session::warnings::{emit_session_warning, new_session_warning_state};
use std::sync::mpsc;
use std::time::Duration;
use tauri::Listener;
use uuid::Uuid;

fn warning() -> SessionWarning {
    SessionWarning::new(Uuid::nil(), "KEY", "kind", "message")
}

fn assert_event(receiver: &mpsc::Receiver<String>, expected: &SessionWarning) {
    let payload = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("live event");
    let actual: serde_json::Value = serde_json::from_str(&payload).expect("event JSON");
    assert_eq!(
        actual,
        serde_json::json!({
            "sessionId": expected.session_id, "key": expected.key,
            "kind": expected.kind, "message": expected.message
        })
    );
    assert!(receiver.try_recv().is_err(), "exactly one live event");
}

#[test]
fn healthy_buffer_precedes_live_event_and_releases_its_lock() {
    let state = new_session_warning_state();
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build warning app");
    let (sender, receiver) = mpsc::channel();
    let (order_sender, order_receiver) = mpsc::channel();
    let observed_state = state.clone();
    let expected = warning();
    let observed_warning = expected.clone();
    app.listen_any("session_env_warning", move |event| {
        let buffered = observed_state.try_lock().ok().map(|mut buffer| {
            let drained = buffer.drain(Some(&observed_warning.session_id));
            let matches = drained == vec![observed_warning.clone()];
            for warning in drained {
                buffer.push(warning);
            }
            matches
        });
        let _ = order_sender.send(buffered);
        let _ = sender.send(event.payload().to_string());
    });

    emit_session_warning(app.handle(), expected.clone());

    assert_eq!(
        order_receiver.try_recv().expect("synchronous callback"),
        Some(true)
    );
    assert_event(&receiver, &expected);
    let mut buffer = state.lock().expect("healthy buffer");
    assert_eq!(buffer.drain(None), vec![expected]);
    assert!(buffer.drain(None).is_empty());
}

#[test]
fn missing_state_still_emits_once_synchronously() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build warning app");
    let (sender, receiver) = mpsc::channel();
    app.listen_any("session_env_warning", move |event| {
        let _ = sender.send(event.payload().to_string());
    });
    let expected = warning();

    emit_session_warning(app.handle(), expected.clone());

    assert_event(&receiver, &expected);
}

#[test]
fn poisoned_buffer_still_emits_without_buffering_or_panicking() {
    let state = new_session_warning_state();
    let poisoned = state.clone();
    let result = std::panic::catch_unwind(move || {
        let _guard = poisoned.lock().expect("initial healthy buffer");
        panic!("poison warning buffer for live-only delivery");
    });
    assert!(result.is_err());
    assert!(state.is_poisoned());
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build warning app");
    let (sender, receiver) = mpsc::channel();
    app.listen_any("session_env_warning", move |event| {
        let _ = sender.send(event.payload().to_string());
    });
    let expected = warning();

    emit_session_warning(app.handle(), expected.clone());

    assert_event(&receiver, &expected);
    let mut buffer = state.lock().unwrap_or_else(|err| err.into_inner());
    assert!(buffer.drain(None).is_empty());
}
