//! #714 Screenshot capture Tauri commands.
//!
//! Thin IPC wrappers over `crate::screenshot`. All capture-state mutation,
//! overlay teardown, and clipboard/clock side effects live in the screenshot
//! module; these handlers only marshal arguments and managed state.

use tauri::{AppHandle, Manager, State};

use crate::config::settings::SettingsState;
use crate::screenshot::{
    ScreenshotCaptureResult, ScreenshotCaptureState, ScreenshotHotkeyState, ScreenshotHotkeyStatus,
    ScreenshotOverlayState, ScreenshotSelection,
};

/// Return the frozen monitor image + metadata for one overlay window.
#[tauri::command]
pub async fn screenshot_get_overlay_state(
    app: AppHandle,
    state: State<'_, ScreenshotCaptureState>,
    capture_id: String,
    monitor_id: u32,
) -> Result<ScreenshotOverlayState, String> {
    crate::screenshot::overlay_state(&app, state.inner(), &capture_id, monitor_id).await
}

/// Confirm a selection: crop, save the PNG into the replica root, copy the path,
/// and tear down overlays. Backend owns all cleanup.
#[tauri::command]
pub async fn screenshot_confirm_selection(
    app: AppHandle,
    state: State<'_, ScreenshotCaptureState>,
    selection: ScreenshotSelection,
) -> Result<ScreenshotCaptureResult, String> {
    crate::screenshot::confirm_selection(&app, state.inner(), selection).await
}

/// Cancel an in-progress capture (Escape / overlay close). Idempotent.
#[tauri::command]
pub async fn screenshot_cancel_capture(
    app: AppHandle,
    state: State<'_, ScreenshotCaptureState>,
    capture_id: String,
) -> Result<(), String> {
    crate::screenshot::cancel_capture(&app, state.inner(), &capture_id).await
}

/// Current global-hotkey registration status (configured combo + whether the OS
/// accepted it + any conflict error). The sidebar reads this on mount.
#[tauri::command]
pub fn screenshot_get_hotkey_status(
    state: State<'_, ScreenshotHotkeyState>,
) -> ScreenshotHotkeyStatus {
    state.lock().unwrap().status.clone()
}

/// Re-read the configured hotkey from settings and (re-)register it. Useful after
/// a Settings save and in manual testing; returns the resulting status. A failed
/// OS registration is reflected in the status, not returned as an error.
#[tauri::command]
pub async fn screenshot_reload_hotkey(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<ScreenshotHotkeyStatus, String> {
    let configured = settings.read().await.screenshot_capture_hotkey.clone();
    // Ignore the Err: an OS conflict is captured in the status below; settings
    // validation already rejected any syntactically invalid value.
    let _ = crate::screenshot::register_configured_hotkey(&app, &configured);
    let status = app
        .state::<ScreenshotHotkeyState>()
        .lock()
        .unwrap()
        .status
        .clone();
    Ok(status)
}
