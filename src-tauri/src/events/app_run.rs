use crate::app_run::exit::ExitEvents;
use tauri::{AppHandle, Runtime};

impl<R: Runtime> ExitEvents for AppHandle<R> {
    fn terminal_attached(&self, session_id: String) {
        let _ = tauri::Emitter::emit(
            self,
            "terminal_attached",
            serde_json::json!({ "sessionId": session_id }),
        );
    }
}
