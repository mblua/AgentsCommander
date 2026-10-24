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
mod tests;
