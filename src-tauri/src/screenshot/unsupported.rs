//! #714 screenshot stub for every target other than Windows, Linux and macOS.
//!
//! Compiled on every target other than Windows, Linux and macOS. It uses NO
//! native screenshot crates (`xcap`, `image`, clipboard, global shortcut). It
//! mirrors the public surface of `super::native` so the command layer and
//! `lib.rs` wiring are identical across targets: every capture path reports an
//! unsupported status, and hotkey "registration" only validates syntax.

use super::{
    ScreenshotCaptureResult, ScreenshotCaptureState, ScreenshotHotkeyOwner, ScreenshotOverlayState,
    ScreenshotSelection,
};

const UNSUPPORTED: &str = concat!(
    "Screenshot capture is not available on this platform; ",
    "it is supported on Windows, macOS and Linux/X11"
);

/// The non-Windows lifecycle can never leave `Idle`, but the variant must exist
/// so the `ScreenshotCaptureState` alias and `lib.rs` (`...::Idle`) compile.
pub enum ScreenshotCaptureLifecycle {
    Idle,
}

/// Mirror of the Windows runtime without the native `Shortcut` handle.
#[derive(Default)]
pub struct ScreenshotHotkeyRuntime {
    pub status: super::ScreenshotHotkeyStatus,
}

/// Validate hotkey syntax (so bad config is still rejected at settings time) and
/// record an unsupported status. Returns `Ok(())` so startup does not warn.
pub fn register_configured_hotkey<H: ScreenshotHotkeyOwner>(
    app: &H,
    configured: &str,
) -> Result<(), String> {
    // Surface a genuine syntax error; otherwise record the unsupported status.
    super::parse_screenshot_hotkey(configured)?;
    let state = app.hotkey_state();
    let mut runtime = state.lock().unwrap();
    runtime.status.configured = configured.to_string();
    runtime.status.registered = false;
    runtime.status.error = Some(UNSUPPORTED.to_string());
    Ok(())
}

/// No-op: the global-shortcut plugin is never registered off Windows.
pub fn begin_capture_from_hotkey<H>(_app: H) {}

pub async fn begin_capture<H>(_app: H) -> Result<(), String> {
    Err(UNSUPPORTED.to_string())
}

pub async fn overlay_state<H>(
    _app: &H,
    _state: &ScreenshotCaptureState,
    _capture_id: &str,
    _monitor_id: u32,
) -> Result<ScreenshotOverlayState, String> {
    Err(UNSUPPORTED.to_string())
}

pub async fn confirm_selection<H>(
    _app: &H,
    _state: &ScreenshotCaptureState,
    _selection: ScreenshotSelection,
) -> Result<ScreenshotCaptureResult, String> {
    Err(UNSUPPORTED.to_string())
}

pub async fn cancel_capture<H>(
    _app: &H,
    _state: &ScreenshotCaptureState,
    _capture_id: &str,
) -> Result<(), String> {
    // Nothing can be active, so cancelling is a successful no-op.
    Ok(())
}

pub async fn clear_capture_and_destroy_overlays<H>(
    _app: &H,
    _state: &ScreenshotCaptureState,
    _capture_id: Option<uuid::Uuid>,
    _reason: &str,
) -> Result<(), String> {
    Ok(())
}

pub async fn handle_overlay_window_destroyed<H>(_app: H, _label: String) {}

#[cfg(test)]
mod tests;
