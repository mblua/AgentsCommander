use crate::testability::ui_automation::{UiAutomationEvents, UiAutomationRequest};
use tauri::{AppHandle, Emitter, Runtime};

impl<R: Runtime> UiAutomationEvents for AppHandle<R> {
    fn automation_request(
        &self,
        window: &str,
        request: &UiAutomationRequest,
    ) -> Result<(), String> {
        self.emit_to(window, "ui_automation_request", request)
            .map_err(|error| error.to_string())
    }
}
