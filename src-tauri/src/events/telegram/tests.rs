use super::*;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::Listener;
use tokio::sync::mpsc::error::TryRecvError;

use crate::web::broadcast::WsBroadcaster;

#[test]
fn bridge_error_matches_legacy_payload_and_synchronous_desktop_delivery() {
    let broadcaster = WsBroadcaster::new();
    let mut websocket = broadcaster.subscribe();
    let app = tauri::test::mock_builder()
        .manage(broadcaster)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    let received = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&received);
    app.listen_any("telegram_bridge_error", move |event| {
        let payload: Value = serde_json::from_str(event.payload()).expect("event JSON");
        observed.lock().unwrap().push(payload);
    });
    let error = "falló ñ: https://api.telegram.org/bot***/sendMessage";
    for kind in [Some("network"), None] {
        let expected = match kind {
            Some(_) => json!({
                "sessionId": "sesión-ñ", "error": error, "kind": "network"
            }),
            None => json!({ "sessionId": "sesión-ñ", "error": error }),
        };
        received.lock().unwrap().clear();
        app.handle()
            .emit("telegram_bridge_error", expected.clone())
            .expect("legacy emit");
        assert_eq!(*received.lock().unwrap(), vec![expected.clone()]);
        received.lock().unwrap().clear();

        app.handle().bridge_error("sesión-ñ", error, kind);

        // Delivery finishes before the port returns, exactly once.
        assert_eq!(*received.lock().unwrap(), vec![expected]);
        assert!(matches!(websocket.try_recv(), Err(TryRecvError::Empty)));
    }
}
