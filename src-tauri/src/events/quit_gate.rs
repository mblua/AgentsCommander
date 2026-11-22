//! Tauri emit helper for the quit gate: `emit_to` sends one JSON event to one window label.
//! It maps the Tauri error to a `String`; it holds no state and makes no quit decisions.
//! The gate logic itself lives elsewhere.

use tauri::{AppHandle, Runtime};

pub(crate) fn emit_to<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    event: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    tauri::Emitter::emit_to(app, label, event, payload).map_err(|e| e.to_string())
}
