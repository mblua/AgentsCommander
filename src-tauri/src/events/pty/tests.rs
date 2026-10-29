use super::*;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::SessionIoFanout;
use crate::session::profile::IdleTuning;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Listener;
use uuid::Uuid;

const WINDOW: &str = "main";
const SECOND_WINDOW: &str = "terminal-2";

/// Criterion P, at the Tauri event layer. `emit` delivers to EVERY open webview, listener
/// or not, so an unattached terminal window and the sidebar would each pay a receive-side
/// deserialization before discarding the payload. `emit_to` per attached label is what makes
/// the bridge multiplier the number of (session, attached window) pairs.
#[test]
fn pty_output_reaches_only_the_attached_webview() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build attachment app");
    let attached_webview = tauri::WebviewWindowBuilder::new(&app, WINDOW, Default::default())
        .build()
        .expect("attached webview");
    let unattached_webview =
        tauri::WebviewWindowBuilder::new(&app, SECOND_WINDOW, Default::default())
            .build()
            .expect("unattached webview");
    let attached_events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let unattached_events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let any_target_events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // Label-shaped registrations, which is what the frontend makes: its `listen` passes
    // `{ target: <this window's label> }`, and Tauri matches an `AnyLabel` candidate
    // against an `emit_to` on the same label exactly as it matches this one.
    let payloads = Arc::new(Mutex::new(Vec::new()));
    let received_payloads = Arc::clone(&payloads);
    let attached_counter = Arc::clone(&attached_events);
    attached_webview.listen("pty_output", move |event| {
        received_payloads
            .lock()
            .unwrap()
            .push(serde_json::from_str::<serde_json::Value>(event.payload()).expect("PTY payload"));
        attached_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let unattached_counter = Arc::clone(&unattached_events);
    unattached_webview.listen("pty_output", move |_| {
        unattached_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    // The guard this test exists for is only worth anything while that stays true. An
    // `EventTarget::Any` registration - which is what a bare JS `listen(event, handler)`
    // with no options sends - SHORT-CIRCUITS the label filter: `match_any_or_filter`
    // returns true for `Any` before the filter is ever consulted, so `emit_to` delivers to
    // it anyway. That is asserted below rather than described, so that dropping the
    // target option on the frontend cannot quietly turn criterion P back into a lie.
    let any_target_counter = Arc::clone(&any_target_events);
    unattached_webview.listen_any("pty_output", move |_| {
        any_target_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    let fanout = SessionIoFanout::new(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        None,
    );
    let id = Uuid::new_v4();
    let token = fanout
        .register_session(
            id,
            IdleTuning::DEFAULT,
            30,
            120,
            output_target(app.handle().clone()),
        )
        .expect("register app-handle session");

    fanout
        .activate_terminal_output(id, WINDOW, true)
        .expect("attach");
    fanout.handle_output(
        &token,
        &id.to_string(),
        b"only the attached window".to_vec(),
    );
    fanout.flush_terminal_output_for_test(id);

    assert_eq!(attached_events.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        *payloads.lock().unwrap(),
        vec![serde_json::json!({
            "sessionId": id.to_string(),
            "data": b"only the attached window".to_vec(),
            "sequence": 1
        })]
    );
    assert_eq!(
        unattached_events.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        any_target_events.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

/// The production factory must schedule from a reader thread without ambient Tokio.
#[test]
fn the_armed_timer_emits_with_no_explicit_flush() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build timer app");
    let webview = tauri::WebviewWindowBuilder::new(&app, WINDOW, Default::default())
        .build()
        .expect("attached webview");
    let payloads = Arc::new(Mutex::new(Vec::new()));
    let received_payloads = Arc::clone(&payloads);
    webview.listen("pty_output", move |event| {
        received_payloads
            .lock()
            .unwrap()
            .push(serde_json::from_str::<serde_json::Value>(event.payload()).expect("PTY payload"));
    });
    let fanout = SessionIoFanout::new(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        None,
    );
    fanout.enable_output_timer_for_test();
    let id = Uuid::new_v4();
    let token = fanout
        .register_session(
            id,
            IdleTuning::DEFAULT,
            30,
            120,
            output_target(app.handle().clone()),
        )
        .expect("register timer session");
    fanout
        .activate_terminal_output(id, WINDOW, true)
        .expect("attach");

    std::thread::spawn(move || {
        fanout.handle_output(&token, &id.to_string(), b"armed by the ingest".to_vec());
    })
    .join()
    .expect("PTY reader thread");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while payloads.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        *payloads.lock().unwrap(),
        vec![serde_json::json!({
            "sessionId": id.to_string(),
            "data": b"armed by the ingest".to_vec(),
            "sequence": 1
        })]
    );
}
