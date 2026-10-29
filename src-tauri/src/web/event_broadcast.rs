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

pub trait DesktopBroadcastEvents: Send + Sync {
    fn desktop_event(&self, event: &str, payload: &Value);
}

pub trait ManagedBroadcastEvents: Send + Sync {
    fn managed_event(&self, event: &str, payload: &Value) -> bool;
}

/// Emit to desktop first, then always to the supplied WebSocket broadcaster.
pub fn broadcast_all<E: DesktopBroadcastEvents + ?Sized>(
    app: &E,
    broadcaster: &WsBroadcaster,
    event: &str,
    payload: &Value,
) {
    app.desktop_event(event, payload);
    broadcaster.broadcast_event(event, payload);
}

#[cfg(test)]
mod tests;
