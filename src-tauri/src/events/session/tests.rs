use super::*;
use crate::session::warnings::{emit_session_warning, new_session_warning_state};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use crate::pty::context_scrape::ContextSample;
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
    let mut buffer = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(buffer.drain(None).is_empty());
}

struct GuardRows;

impl crate::pty::context_scrape::ScreenRowsSource for GuardRows {
    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::Unavailable
    }
}

struct GuardPatterns;

impl crate::pty::context_scrape::ContextPatternSource for GuardPatterns {
    fn patterns(&self) -> BoxFuture<'_, Option<HashMap<String, String>>> {
        Box::pin(async { Some(HashMap::new()) })
    }
}

struct GuardEvents;

impl crate::pty::context_scrape::ContextEventSink for GuardEvents {
    fn emit(&self, _payload: crate::pty::context_scrape::ContextUsagePayload) {}
}

struct GuardSamples;

impl crate::pty::context_scrape::ContextSampleSink for GuardSamples {
    fn observe(&self, _sample: ContextSample) {}
}

struct GuardPersist;

impl crate::pty::context_scrape::ContextPersistSink for GuardPersist {
    fn commit(&self, _changed: Vec<(Uuid, Option<u8>)>) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[tokio::test]
async fn context_alert_adapter_reads_state_managed_after_construction_and_future_creation() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let services = ContextAlertTauriServices {
        app: app.handle().clone(),
        messages: Arc::new(MessageWarnings::new()),
    };
    assert!(services.public_session(Uuid::nil()).await.is_none());
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/synthetic".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let id = session.id;
    let pending = services.public_session(id);
    assert!(app.manage(Arc::new(tokio::sync::RwLock::new(manager))));
    assert_eq!(pending.await.unwrap().id, id);
    assert!(!services.registered(id));
    services.retire_sample_registration(id);

    let temp = tempfile::tempdir().unwrap();
    let replica = temp
        .path()
        .join("project/.ac/room-1-team/__agent_coordinator");
    std::fs::create_dir_all(&replica).unwrap();
    let target = InternalSystemTarget::for_context_alert(
        "project:room-1-team/coordinator".to_string(),
        replica,
    )
    .unwrap();
    assert_eq!(
        services.validate_delivery_state(id, &target).unwrap_err(),
        "Context scraper is unavailable",
    );
    let scraper = ContextScraper::new(
        Arc::new(GuardRows),
        Arc::new(GuardPatterns),
        Arc::new(GuardEvents),
        Arc::new(GuardSamples),
        Arc::new(GuardPersist),
    );
    assert!(app.manage(Arc::clone(&scraper)));
    assert_eq!(
        services.validate_delivery_state(id, &target).unwrap_err(),
        "Sampled session is no longer registered",
    );
    scraper.register_session(id, "shell".to_string());
    assert!(services.registered(id));
    services.validate_delivery_state(id, &target).unwrap();
    services.retire_sample_registration(id);
    assert!(!services.registered(id));
    assert_eq!(
        services.validate_delivery_state(id, &target).unwrap_err(),
        "Sampled session is no longer registered",
    );
}

#[tokio::test]
async fn context_alert_adapter_observes_late_purge_and_checks_it_before_registration() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let services = ContextAlertTauriServices {
        app: app.handle().clone(),
        messages: Arc::new(MessageWarnings::new()),
    };
    let id = Uuid::new_v4();
    let scraper = ContextScraper::new(
        Arc::new(GuardRows),
        Arc::new(GuardPatterns),
        Arc::new(GuardEvents),
        Arc::new(GuardSamples),
        Arc::new(GuardPersist),
    );
    scraper.register_session(id, "shell".to_string());
    assert!(app.manage(Arc::clone(&scraper)));
    let temp = tempfile::tempdir().unwrap();
    let replica = temp
        .path()
        .join("project/.ac/room-1-team/__agent_coordinator");
    std::fs::create_dir_all(&replica).unwrap();
    let target = InternalSystemTarget::for_context_alert(
        "project:room-1-team/coordinator".to_string(),
        replica,
    )
    .unwrap();
    services.validate_delivery_state(id, &target).unwrap();
    let purge = Arc::new(PurgeGuard::default());
    assert!(app.manage(Arc::clone(&purge)));
    let lease = purge
        .acquire(HashSet::new(), HashSet::from([target.fqn().to_string()]))
        .await;
    let expected = format!("purge-room blocks '{}'", target.fqn());
    assert_eq!(
        services.validate_delivery_state(id, &target),
        Err(expected.clone())
    );
    drop(lease);
    services.validate_delivery_state(id, &target).unwrap();
    let lease = purge
        .acquire(HashSet::new(), HashSet::from([target.fqn().to_string()]))
        .await;
    services.retire_sample_registration(id);
    assert_eq!(services.validate_delivery_state(id, &target), Err(expected));
    drop(lease);
    assert_eq!(
        services.validate_delivery_state(id, &target).unwrap_err(),
        "Sampled session is no longer registered",
    );
}

#[test]
fn context_alert_start_on_without_ambient_tokio() {
    assert!(tokio::runtime::Handle::try_current().is_err());
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let cancellation = CancellationToken::new();
    assert!(tokio::runtime::Handle::try_current().is_err());
    let monitor = start_on(
        app.handle().clone(),
        cancellation.clone(),
        Arc::new(MessageWarnings::new()),
    );
    cancellation.cancel();
    tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(5), monitor.close_and_join())
            .await
            .expect("bounded actor shutdown")
            .expect("joined actor shutdown");
        monitor.close_and_join().await.expect("repeated close");
    });
}

#[derive(Default)]
struct SelectionDeliveries(std::cell::RefCell<Vec<(&'static str, serde_json::Value)>>);

impl crate::session::selection::SelectionEvents for SelectionDeliveries {
    fn selection_event(&self, event: &'static str, value: serde_json::Value) {
        self.0.borrow_mut().push((event, value));
    }
}

struct CountedSelectionPayload<'a> {
    calls: &'a std::cell::Cell<usize>,
    fail: bool,
}

impl serde::Serialize for CountedSelectionPayload<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.calls.set(self.calls.get() + 1);
        if self.fail {
            return Err(serde::ser::Error::custom("issue3950-serialization-failure"));
        }
        serde_json::json!({"selection": 3950}).serialize(serializer)
    }
}

fn serialized_once(
    calls: usize,
    deliveries: &[(&str, serde_json::Value)],
) -> Result<(), &'static str> {
    if calls != 1 {
        return Err("selection payload must serialize exactly once");
    }
    if deliveries != [("issue3950-event", serde_json::json!({"selection": 3950}))] {
        return Err("selection must deliver the same JSON exactly once");
    }
    Ok(())
}

#[test]
fn issue_3950_serialize_once() {
    let calls = std::cell::Cell::new(0);
    let events = SelectionDeliveries::default();
    let payload = CountedSelectionPayload {
        calls: &calls,
        fail: false,
    };
    crate::session::selection::port_test_publish_lifecycle_event(
        &events,
        "issue3950-event",
        &payload,
    );
    assert_eq!(serialized_once(calls.get(), &events.0.borrow()), Ok(()));
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_serialize_once");

    // A double that repeats the real serializer must fail the same one-pass assertion.
    calls.set(0);
    events.0.borrow_mut().clear();
    for _ in 0..2 {
        crate::session::selection::port_test_publish_lifecycle_event(
            &events,
            "issue3950-event",
            &payload,
        );
    }
    assert_eq!(
        serialized_once(calls.get(), &events.0.borrow()),
        Err("selection payload must serialize exactly once")
    );
    println!("\nISSUE3950_MUTANT_REJECTED case=double_serialize");
}

#[test]
fn issue_3950_serialize_error() {
    let calls = std::cell::Cell::new(0);
    let events = SelectionDeliveries::default();
    crate::session::selection::port_test_publish_lifecycle_event(
        &events,
        "issue3950-event",
        &CountedSelectionPayload {
            calls: &calls,
            fail: true,
        },
    );
    assert_eq!(calls.get(), 1, "failing payload serialized exactly once");
    assert!(
        events.0.borrow().is_empty(),
        "serialization failure has zero sink calls"
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_serialize_error");
}

type SelectionDeliveryTrace = Vec<(&'static str, &'static str, serde_json::Value)>;

fn ordered_selection_delivery(trace: &SelectionDeliveryTrace) -> Result<(), &'static str> {
    let value = serde_json::json!({"selection": 3950});
    if trace
        != &[
            ("native", "issue3950-event", value.clone()),
            ("websocket", "issue3950-event", value),
        ]
    {
        return Err("native must precede WebSocket with the same event and JSON");
    }
    Ok(())
}

fn selection_delivery_trace(fail_native: bool, short_circuit: bool) -> SelectionDeliveryTrace {
    let trace = std::cell::RefCell::new(Vec::new());
    let native_failed = std::cell::Cell::new(false);
    deliver_selection_event(
        "issue3950-event",
        serde_json::json!({"selection": 3950}),
        |event, value| {
            trace.borrow_mut().push(("native", event, value));
            native_failed.set(fail_native);
            if fail_native {
                Err("issue3950-native-failure".into())
            } else {
                Ok(())
            }
        },
        |event, value| {
            // Bad sink double models short-circuiting after native failure.
            if !(short_circuit && native_failed.get()) {
                trace.borrow_mut().push(("websocket", event, value.clone()));
            }
        },
    );
    trace.into_inner()
}

#[test]
fn issue_3950_native_ws_order() {
    assert_eq!(
        ordered_selection_delivery(&selection_delivery_trace(false, false)),
        Ok(())
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_native_ws_order");
}

#[test]
fn issue_3950_native_fail_ws() {
    assert_eq!(
        ordered_selection_delivery(&selection_delivery_trace(true, false)),
        Ok(())
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_native_fail_ws");
    assert_eq!(
        ordered_selection_delivery(&selection_delivery_trace(true, true)),
        Err("native must precede WebSocket with the same event and JSON")
    );
    // The short-circuit double's successful-native opposite control still delivers both.
    assert_eq!(
        ordered_selection_delivery(&selection_delivery_trace(false, true)),
        Ok(())
    );
    println!("\nISSUE3950_MUTANT_REJECTED case=native_short_circuit");
}
