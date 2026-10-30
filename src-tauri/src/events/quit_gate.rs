use tauri::{AppHandle, Runtime};

pub(crate) fn emit_to<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    event: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    tauri::Emitter::emit_to(app, label, event, payload).map_err(|e| e.to_string())
}
