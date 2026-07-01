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
//! - `windows` owns the native runtime: `xcap` capture, `image` crop/encode,
//!   clipboard, global shortcut, overlay windows, and the capture lifecycle.
//! - `unsupported` is the non-Windows stub: same public surface, no native
//!   screenshot crates, every capture path reports an unsupported status.
//!
//! `pub use <cfg-module>::*` lets callers use `crate::screenshot::*` without
//! per-call `cfg` blocks; the two cfg modules each provide
//! `ScreenshotCaptureLifecycle`, `ScreenshotHotkeyRuntime`, and the runtime
//! functions referenced by the type aliases and command layer below.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(target_os = "windows"))]
mod unsupported;
#[cfg(not(target_os = "windows"))]
pub use unsupported::*;

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
mod parser_tests {
    use super::*;

    #[test]
    fn accepts_ctrl_q_case_insensitive() {
        for value in ["Ctrl+Q", "ctrl+q", "Control+Q", "CTRL+Q", "  Ctrl + q "] {
            let parsed = parse_screenshot_hotkey(value)
                .unwrap_or_else(|e| panic!("expected '{value}' to parse: {e}"));
            assert_eq!(parsed.modifier, HotkeyModifier::Ctrl);
            assert_eq!(parsed.key, 'Q', "value {value}");
        }
    }

    #[test]
    fn accepts_alphanumeric_keys() {
        assert_eq!(parse_screenshot_hotkey("Ctrl+1").unwrap().key, '1');
        assert_eq!(parse_screenshot_hotkey("Control+a").unwrap().key, 'A');
    }

    #[test]
    fn rejects_empty_and_whitespace() {
        assert!(parse_screenshot_hotkey("").is_err());
        assert!(parse_screenshot_hotkey("   ").is_err());
    }

    #[test]
    fn rejects_missing_modifier_or_key() {
        assert!(parse_screenshot_hotkey("Q").is_err());
        assert!(parse_screenshot_hotkey("Ctrl+").is_err());
        assert!(parse_screenshot_hotkey("+Q").is_err());
    }

    #[test]
    fn rejects_unknown_modifier() {
        assert!(parse_screenshot_hotkey("Hyper+Q").is_err());
        assert!(parse_screenshot_hotkey("Alt+Q").is_err());
        assert!(parse_screenshot_hotkey("Shift+Q").is_err());
    }

    #[test]
    fn rejects_multi_modifier_and_multi_key() {
        // MVP accepts exactly one modifier plus one key.
        assert!(parse_screenshot_hotkey("Ctrl+Shift+Q").is_err());
        assert!(parse_screenshot_hotkey("Ctrl+QQ").is_err());
        assert!(parse_screenshot_hotkey("Ctrl+F1").is_err());
    }
}
