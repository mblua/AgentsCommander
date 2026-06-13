use tauri::{State, WebviewWindow};

#[tauri::command]
pub fn ui_automation_enabled(
    state: State<'_, crate::testability::ui_automation::UiAutomationState>,
) -> bool {
    state.enabled()
}

#[tauri::command]
pub fn ui_automation_frontend_ready(
    webview: WebviewWindow,
    window: Option<String>,
    state: State<'_, crate::testability::ui_automation::UiAutomationState>,
) -> Result<(), String> {
    state.mark_frontend_ready(webview.label(), window.as_deref())
}

#[tauri::command]
pub fn ui_automation_complete(
    webview: WebviewWindow,
    result: crate::testability::ui_automation::UiAutomationResponse,
    state: State<'_, crate::testability::ui_automation::UiAutomationState>,
) -> Result<(), String> {
    state.complete(webview.label(), result)
}
