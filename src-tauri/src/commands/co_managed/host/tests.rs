//! Capability compatibility and co-managed publication controls.
use super::*;
use crate::co_managed::host::CoManagedEvents;
use crate::co_managed::{
    co_managed_jev_settings, emit_co_managed_state, emit_session_idle_edge,
    surface_co_managed_user_message, CoManagedSupervisorHandle, CoManagedTrigger,
};
use crate::config::settings::new_settings_state;
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;
use tauri::{test::MockRuntime, Listener};
use uuid::Uuid;

fn app() -> tauri::App<MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

fn required_missing<T>(get: impl FnOnce() -> T, name: &str) {
    let panic = catch_unwind(AssertUnwindSafe(get))
        .err()
        .expect("required state panic");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("panic text");
    assert!(
        message.contains("state() called before manage()"),
        "{message}"
    );
    assert!(message.contains(name), "{message}");
}

#[tokio::test]
async fn issue_3950_co_missing() {
    let app = app();
    let host = app.handle();
    assert!(host.sessions_optional().is_none());
    assert!(host.input_optional().is_none());
    assert!(host.registry_optional().is_none());
    surface_co_managed_user_message(host, Uuid::nil(), "missing".into()).await;
    required_missing(|| host.sessions_required(), "SessionManager");
    required_missing(|| host.settings_required(), "SettingsOwner");
    required_missing(|| host.network_required(), "OutboundNetwork");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_missing");
}

fn settings() -> crate::config::settings::AppSettings {
    crate::config::settings::AppSettings {
        jev_api_key: "test-key".into(),
        jev_model: "test-model".into(),
        jev_endpoint: "https://example.invalid/jev".into(),
        jev_timeout_secs: 37,
        jev_threshold: 0.81,
        jev_margin: 0.23,
        ..Default::default()
    }
}

#[tokio::test]
async fn issue_3950_co_six_fields() {
    let app = app();
    assert!(app.manage(new_settings_state(settings())));
    let actual = co_managed_jev_settings(app.handle()).await.unwrap();
    assert_eq!(
        actual,
        crate::capture::jev::JevSettings {
            api_key: "test-key".into(),
            model: "test-model".into(),
            endpoint: "https://example.invalid/jev".into(),
            timeout_secs: 37,
            threshold: 0.81,
            margin: 0.23,
        }
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_six_fields");
}

#[tokio::test]
async fn issue_3950_co_fresh_settings() {
    let app = app();
    let state = new_settings_state(settings());
    assert!(app.manage(state.clone()));
    let before = co_managed_jev_settings(app.handle()).await.unwrap();
    {
        let mut guard = state.write().await;
        guard.jev_api_key = "next-key".into();
        guard.jev_model = "next-model".into();
        guard.jev_endpoint = "https://example.invalid/next".into();
        guard.jev_timeout_secs = 51;
        guard.jev_threshold = 0.93;
        guard.jev_margin = 0.34;
    }
    let after = co_managed_jev_settings(app.handle()).await.unwrap();
    assert_ne!(before, after);
    assert_eq!(
        after,
        crate::capture::jev::JevSettings {
            api_key: "next-key".into(),
            model: "next-model".into(),
            endpoint: "https://example.invalid/next".into(),
            timeout_secs: 51,
            threshold: 0.93,
            margin: 0.34,
        }
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_fresh_settings");
}

struct IdleSink {
    receiver: Mutex<tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>>,
    calls: Mutex<Vec<(&'static str, Value)>>,
    fail: bool,
}

impl CoManagedEvents for IdleSink {
    fn co_managed_event(&self, event: &'static str, value: Value) {
        assert!(
            self.receiver.lock().unwrap().try_recv().is_err(),
            "emit before notify"
        );
        let _ = self.native(event, value);
    }
}

impl IdleSink {
    fn native(&self, event: &'static str, value: Value) -> Result<(), &'static str> {
        self.calls.lock().unwrap().push((event, value));
        if self.fail {
            Err("native failure")
        } else {
            Ok(())
        }
    }
}

fn idle_control(armed: bool, fail: bool) {
    let (supervisor, receiver) = CoManagedSupervisorHandle::new();
    let id = Uuid::new_v4();
    if armed {
        supervisor.armed.arm(&id.to_string());
    }
    let sink = IdleSink {
        receiver: Mutex::new(receiver),
        calls: Mutex::new(Vec::new()),
        fail,
    };
    assert_eq!(
        emit_session_idle_edge(&sink, &supervisor.armed, Some(&supervisor), id),
        armed
    );
    assert_eq!(
        *sink.calls.lock().unwrap(),
        vec![(
            "session_idle",
            json!({"id": id.to_string(), "comanaged": armed})
        )]
    );
    assert_eq!(
        sink.receiver.lock().unwrap().try_recv().unwrap(),
        CoManagedTrigger::IdleEdge(id)
    );
    assert!(sink.receiver.lock().unwrap().try_recv().is_err());
}

fn publication_order(deliver: impl FnOnce(&mut Vec<&'static str>)) -> bool {
    let mut recording = Vec::new();
    deliver(&mut recording);
    recording == ["native", "notify"]
}

#[test]
fn issue_3950_co_idle_order() {
    idle_control(false, false);
    idle_control(true, false);
    assert!(publication_order(|r| {
        r.push("native");
        r.push("notify");
    }));
    assert!(!publication_order(|r| {
        r.push("notify");
        r.push("native");
    }));
    println!("\nISSUE3950_MUTANT_REJECTED case=notify_first");
    assert!(!publication_order(|r| {
        r.push("native");
        r.push("ws");
        r.push("notify");
    }));
    println!("\nISSUE3950_MUTANT_REJECTED case=add_ws");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_idle_order");
}

#[test]
fn issue_3950_co_idle_error() {
    idle_control(false, true);
    idle_control(true, true);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_idle_error");
}

#[test]
fn issue_3950_co_state_native_only() {
    let app = app();
    let broadcaster = crate::web::broadcast::WsBroadcaster::new();
    let mut ws = broadcaster.subscribe();
    assert!(app.manage(broadcaster));
    let first = tauri::WebviewWindowBuilder::new(
        app.handle(),
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .unwrap();
    let second = tauri::WebviewWindowBuilder::new(
        app.handle(),
        "detached",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    for (label, window) in [("main", first), ("detached", second)] {
        let received = received.clone();
        window.listen("session_comanaged_state", move |event| {
            received.lock().unwrap().push((
                label,
                serde_json::from_str::<Value>(event.payload()).unwrap(),
            ));
        });
    }
    let id = Uuid::new_v4();
    emit_co_managed_state(app.handle(), id, true, Some("Ready"));
    let mut actual = received.lock().unwrap().clone();
    actual.sort_by_key(|(label, _)| *label);
    let expected = json!({"id": id.to_string(), "active": true, "reason": "Ready"});
    assert_eq!(
        actual,
        vec![("detached", expected.clone()), ("main", expected)]
    );
    assert!(ws.try_recv().is_err(), "co-managed event stays native only");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_state_native_only");
}

#[tokio::test]
async fn issue_3950_co_communication() {
    let app = app();
    let manager = Arc::new(RwLock::new(SessionManager::new()));
    let id = manager
        .read()
        .await
        .create_session(
            "sh".into(),
            Vec::new(),
            "C:/work".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
        .id;
    assert!(app.manage(manager));
    let received = Arc::new(Mutex::new(Vec::new()));
    let recording = received.clone();
    app.listen_any("session_communication_changed", move |event| {
        recording
            .lock()
            .unwrap()
            .push(serde_json::from_str::<Value>(event.payload()).unwrap());
    });
    surface_co_managed_user_message(app.handle(), id, "first".into()).await;
    assert_eq!(received.lock().unwrap().len(), 1);
    let initial = received.lock().unwrap()[0].clone();
    assert_eq!(initial["sessionId"], id.to_string());
    assert_eq!(initial["communication"]["message"], "first");
    surface_co_managed_user_message(app.handle(), id, "first".into()).await;
    assert_eq!(*received.lock().unwrap(), vec![initial]);
    surface_co_managed_user_message(app.handle(), id, "second".into()).await;
    assert_eq!(received.lock().unwrap().len(), 2);
    assert_eq!(
        received.lock().unwrap()[1]["communication"]["message"],
        "second"
    );
    surface_co_managed_user_message(app.handle(), Uuid::new_v4(), "unknown".into()).await;
    assert_eq!(received.lock().unwrap().len(), 2);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_communication");
}
