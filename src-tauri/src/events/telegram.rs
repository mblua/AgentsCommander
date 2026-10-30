use tauri::Emitter;

use crate::telegram::output::TelegramEvents;

impl<R: tauri::Runtime> TelegramEvents for tauri::AppHandle<R> {
    fn bridge_error(&self, session_id: &str, error: &str, kind: Option<&str>) {
        let mut payload = serde_json::json!({ "sessionId": session_id, "error": error });
        if let Some(kind) = kind {
            payload["kind"] = serde_json::json!(kind);
        }
        let _ = self.emit("telegram_bridge_error", payload);
    }
}

#[cfg(test)]
mod tests;
