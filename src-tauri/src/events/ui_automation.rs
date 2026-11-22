//! Tauri sink for UI automation: `UiAutomationEvents` sends a `UiAutomationRequest`
//! to one window as `ui_automation_request` and returns any emit error as a `String`.
//! It holds no state; request handling lives in `crate::testability::ui_automation`.

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
