use super::*;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tauri::Listener;
use tokio::sync::mpsc::error::TryRecvError;

use crate::web::broadcast::WsOutMsg;
use crate::web::event_broadcast::broadcast_all;

fn receive(receiver: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) -> Value {
    match receiver.try_recv().expect("WebSocket event") {
        WsOutMsg::Text(text) => serde_json::from_str(&text).expect("event JSON"),
        other => panic!("expected text event, got {other:?}"),
    }
}

#[test]
fn explicit_broadcast_preserves_desktop_order_payload_and_supplied_broadcaster() {
    let managed = WsBroadcaster::new();
    let mut managed_receiver = managed.subscribe();
    let explicit = WsBroadcaster::new();
    let receiver = Arc::new(Mutex::new(explicit.subscribe()));
    let app = tauri::test::mock_builder()
        .manage(managed)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    let desktop = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&desktop);
    let desktop_receiver = Arc::clone(&receiver);
    app.listen_any("project_archive_changed", move |event| {
        assert!(matches!(
            desktop_receiver.lock().unwrap().try_recv(),
            Err(TryRecvError::Empty)
        ));
        let payload = serde_json::from_str::<Value>(event.payload()).expect("desktop JSON");
        observed.lock().unwrap().push(payload);
    });
    let payload = json!({ "path": "C:/proyecto/ñ", "archived": false, "extra": [null, 3] });

    broadcast_all(app.handle(), &explicit, "project_archive_changed", &payload);

    assert_eq!(*desktop.lock().unwrap(), vec![payload.clone()]);
    assert_eq!(
        receive(&mut receiver.lock().unwrap()),
        json!({ "event": "project_archive_changed", "payload": payload })
    );
    assert!(matches!(
        managed_receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
    assert!(matches!(
        receiver.lock().unwrap().try_recv(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn managed_absence_still_emits_exact_desktop_payload_and_returns_false() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    let desktop = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&desktop);
    app.listen_any("project_archive_changed", move |event| {
        let payload = serde_json::from_str::<Value>(event.payload()).expect("desktop JSON");
        observed.lock().unwrap().push(payload);
    });
    let payload = json!({ "path": "C:/project", "archived": true });

    assert!(!app
        .handle()
        .managed_event("project_archive_changed", &payload));

    assert_eq!(*desktop.lock().unwrap(), vec![payload]);
}

#[test]
fn managed_lookup_occurs_after_desktop_and_observes_new_registration() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    let broadcaster = WsBroadcaster::new();
    let receiver = Arc::new(Mutex::new(broadcaster.subscribe()));
    let desktop_receiver = Arc::clone(&receiver);
    let handle = app.handle().clone();
    let desktop = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&desktop);
    app.listen_any("project_archive_changed", move |event| {
        assert!(matches!(
            desktop_receiver.lock().unwrap().try_recv(),
            Err(TryRecvError::Empty)
        ));
        let payload = serde_json::from_str::<Value>(event.payload()).expect("desktop JSON");
        observed.lock().unwrap().push(payload);
        assert!(handle.manage(broadcaster.clone()));
    });
    let payload = json!({ "path": "C:/project", "archived": true });

    assert!(app
        .handle()
        .managed_event("project_archive_changed", &payload));

    assert_eq!(*desktop.lock().unwrap(), vec![payload.clone()]);
    assert_eq!(
        receive(&mut receiver.lock().unwrap()),
        json!({ "event": "project_archive_changed", "payload": payload })
    );
    assert!(matches!(
        receiver.lock().unwrap().try_recv(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn failed_desktop_emission_still_delivers_explicit_and_managed_websocket_events() {
    let managed = WsBroadcaster::new();
    let mut managed_receiver = managed.subscribe();
    let explicit = WsBroadcaster::new();
    let mut explicit_receiver = explicit.subscribe();
    let app = tauri::test::mock_builder()
        .manage(managed)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    // Invalid desktop event names return an error before any desktop delivery.
    let event = "invalid desktop event";
    let payload = json!({ "value": [true, null, "ñ"] });
    assert!(app.handle().emit(event, payload.clone()).is_err());

    broadcast_all(app.handle(), &explicit, event, &payload);

    let expected = json!({ "event": event, "payload": payload });
    assert_eq!(receive(&mut explicit_receiver), expected);
    assert!(matches!(
        managed_receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
    assert!(app.handle().managed_event(event, &payload));
    assert_eq!(receive(&mut managed_receiver), expected);
    assert!(matches!(
        explicit_receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
}
