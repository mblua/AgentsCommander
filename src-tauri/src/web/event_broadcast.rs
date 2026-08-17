//! The dual-transport event emitter: one call reaches both the Tauri windows and
//! the connected WebSocket clients.
//!
//! #1265: this module exists so a Tauri command never has to reach sideways into
//! the browser command dispatcher just to announce a change. The dispatcher and
//! `commands::project_settings` both depend downward on this module, which owns
//! the emitter, so neither of them depends on the other in order to emit.
//!
//! It sits beside `web::broadcast` rather than inside it because `web::broadcast`
//! is the WebSocket fan-out and knows nothing about Tauri. Handing it an
//! `AppHandle` and an `Emitter` would make the WebSocket transport depend on the
//! desktop one, which trades one layering inversion for another.

use serde_json::Value;

use crate::web::broadcast::WsBroadcaster;

/// Emit event to both Tauri windows and WebSocket clients.
pub fn broadcast_all(
    app: &tauri::AppHandle,
    broadcaster: &WsBroadcaster,
    event: &str,
    payload: &Value,
) {
    let _ = tauri::Emitter::emit(app, event, payload.clone());
    broadcaster.broadcast_event(event, payload);
}

#[cfg(test)]
mod tests {
    use super::broadcast_all;
    use crate::web::broadcast::{WsBroadcaster, WsOutMsg};
    use serde_json::{json, Value};

    #[test]
    fn broadcast_all_sends_to_explicit_websocket_broadcaster() {
        let managed = WsBroadcaster::new();
        let explicit = WsBroadcaster::new();
        let mut receiver = explicit.subscribe();
        // Deliberately NOT `crate::test_support::test_builder()`. This file is
        // the #1265 emitter home, and `the_emitter_home_names_nothing_but_the_
        // websocket_fan_out` pins the exact set of crate modules it may name.
        // Tauri declares `Builder::any_thread` only for Windows and Linux, so
        // the predicate is written out here rather than imported.
        #[cfg(any(windows, target_os = "linux"))]
        let builder = tauri::Builder::default().any_thread();
        #[cfg(not(any(windows, target_os = "linux")))]
        let builder = tauri::Builder::default();
        let app = builder
            .manage(managed)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let payload = json!({ "path": "C:/project", "archived": false });

        broadcast_all(app.handle(), &explicit, "project_archive_changed", &payload);

        let event = match receiver.try_recv().expect("broadcast event") {
            WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
            other => panic!("expected text event, got {other:?}"),
        };
        assert_eq!(event["event"], json!("project_archive_changed"));
        assert_eq!(event["payload"], payload);
    }
}
