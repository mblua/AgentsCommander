//! #714 Active-agent screenshot capture.
//!
//! Native, Greenshot-style screenshot flow: a global hotkey freezes the desktop,
//! the user drags a rectangle on a per-monitor overlay, and the crop is saved as
//! a PNG into the owning WG replica root of the session currently selected in
//! AgentsCommander, with the absolute path copied to the clipboard.
//!
//! Module layout (see plan `_plans/714-active-agent-screenshot-capture.md`):
//! - This file owns the serializable IPC types shared with the frontend, the
//!   target-independent hotkey parser, and the managed-state type aliases.
//! - `native` (Windows, Linux/X11 and macOS) owns the native runtime: `xcap`
//!   capture, `image` crop/encode, clipboard, global shortcut, overlay windows,
//!   and the capture lifecycle. Only its window-capture region stays Windows-only.
//! - `unsupported` is the every-other-target stub: same public surface, no
//!   native screenshot crates, every capture path reports an unsupported status.
//!
//! Explicit reexports let callers use `crate::screenshot` without
//! per-call `cfg` blocks; the two cfg modules each provide
//! `ScreenshotCaptureLifecycle`, `ScreenshotHotkeyRuntime`, and the runtime
//! functions referenced by the type aliases and command layer below.
//!
//! The `any(target_os = "windows", target_os = "linux", target_os = "macos")`
//! predicate is duplicated in `Cargo.toml` (the target dependency tables), in
//! `lib.rs` (the plugin cfg) and in `session/selection.rs` (the test-only
//! helpers); they must agree.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod native;
#[cfg(target_os = "macos")]
pub use native::MACOS_SCREEN_RECORDING_REQUIRED;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub use native::{
    begin_capture, begin_capture_from_hotkey, cancel_capture, clear_capture_and_destroy_overlays,
    confirm_selection, handle_overlay_window_destroyed, overlay_state, register_configured_hotkey,
    ActiveScreenshotCapture, BeginOutcome, CapturedMonitor, ScreenshotCaptureLifecycle,
    ScreenshotCaptureStarting, ScreenshotHotkeyRuntime,
};
#[cfg(target_os = "windows")]
pub(crate) use native::{capture_window_png, parse_window_id, WindowScreenshotCaptureError};
#[cfg(target_os = "linux")]
pub use native::{
    classify_display_server, display_env_snapshot, DisplayEnvSnapshot, LinuxDisplayServer,
    WAYLAND_UNSUPPORTED,
};

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub use unsupported::{
    begin_capture, begin_capture_from_hotkey, cancel_capture, clear_capture_and_destroy_overlays,
    confirm_selection, handle_overlay_window_destroyed, overlay_state, register_configured_hotkey,
    ScreenshotCaptureLifecycle, ScreenshotHotkeyRuntime,
};

/// Short-lived capture lifecycle, guarded by an async mutex. `Starting` is
/// reserved under the lock before the (synchronous, off-thread) desktop capture
/// so two rapid hotkey presses cannot both enter monitor capture.
pub type ScreenshotCaptureState = Arc<tokio::sync::Mutex<ScreenshotCaptureLifecycle>>;

/// Global-hotkey registration runtime (configured string, registration result,
/// and on Windows the currently-registered `Shortcut`). A plain `std::sync::Mutex`
/// because no work is held across an await point while it is locked.
pub type ScreenshotHotkeyState = Arc<std::sync::Mutex<ScreenshotHotkeyRuntime>>;

/// Frozen monitor image plus metadata handed to one overlay window so it can
/// render the selection UI and map pointer coordinates to physical image pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotOverlayState {
    pub capture_id: String,
    pub monitor_id: u32,
    pub monitor_x: i32,
    pub monitor_y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub image_data_url: String,
    pub session_id: String,
    pub session_name: String,
    pub target_directory: String,
}

/// Physical, monitor-local crop rectangle sent from the overlay on mouse release.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotSelection {
    pub capture_id: String,
    pub monitor_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Success payload for `screenshot_capture_saved` and the confirm command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotCaptureResult {
    pub path: String,
    pub session_id: String,
    pub session_name: String,
}

/// Failure payload for the `screenshot_capture_failed` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotCaptureEvent {
    pub message: String,
}

/// Typed screenshot notifications, delivered by the native event adapter.
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub trait ScreenshotEvents {
    fn capture_saved(&self, result: ScreenshotCaptureResult);
    fn capture_failed(&self, event: ScreenshotCaptureEvent);
}

/// Hotkey registration status exposed to the frontend. Never carries the
/// internal `Shortcut`; commands clone this out of `ScreenshotHotkeyRuntime`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotHotkeyStatus {
    pub configured: String,
    pub registered: bool,
    pub error: Option<String>,
}

impl Default for ScreenshotHotkeyStatus {
    fn default() -> Self {
        Self {
            // Mirrors `settings::default_screenshot_capture_hotkey`; overwritten by
            // the first `register_configured_hotkey` call at startup.
            configured: "Ctrl+Q".to_string(),
            registered: false,
            error: None,
        }
    }
}

/// The single modifier accepted in this issue. Kept as an enum so additional
/// modifiers can be added later without changing the parser's return shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyModifier {
    Ctrl,
}

/// A validated "one modifier + one key" hotkey. Target-independent so settings
/// validation works on every platform; Windows converts this into a plugin
/// `Shortcut` at registration time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHotkey {
    pub modifier: HotkeyModifier,
    /// Normalized uppercase key, e.g. `'Q'` or `'1'`.
    pub key: char,
}

/// Parse `"Ctrl+Q"`, `"control+q"`, `"CTRL+Q"`, etc. into a [`ParsedHotkey`].
///
/// This is intentionally OS-free: it never touches the global-shortcut plugin so
/// `settings::validate_screenshot_hotkey` can run headless and on non-Windows
/// builds. For this issue exactly one modifier (Ctrl/Control) plus one
/// alphanumeric key is accepted; anything else is rejected.
pub fn parse_screenshot_hotkey(value: &str) -> Result<ParsedHotkey, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("hotkey must not be empty".to_string());
    }
    let parts: Vec<&str> = trimmed.split('+').map(|p| p.trim()).collect();
    if parts.len() != 2 {
        return Err(format!(
            "hotkey '{value}' must be one modifier plus one key, e.g. Ctrl+Q"
        ));
    }
    let modifier = match parts[0].to_ascii_lowercase().as_str() {
        "ctrl" | "control" => HotkeyModifier::Ctrl,
        other => {
            return Err(format!(
                "unsupported modifier '{other}'; only Ctrl is supported"
            ))
        }
    };
    let key_str = parts[1];
    let mut chars = key_str.chars();
    let key = match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase(),
        _ => {
            return Err(format!(
                "hotkey key '{key_str}' must be a single letter or digit"
            ))
        }
    };
    Ok(ParsedHotkey { modifier, key })
}

#[cfg(test)]
mod tests;
