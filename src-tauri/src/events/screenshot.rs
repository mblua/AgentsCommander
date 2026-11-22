//! Tauri sink for screenshot capture events, built only on Windows, Linux and macOS.
//! `ScreenshotEvents` emits `screenshot_capture_saved` and `screenshot_capture_failed`,
//! plus `screenshot_hotkey_status_changed` on Linux only.
//! It does not capture images; capture lives in `crate::screenshot`.

#![cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]

use tauri::{AppHandle, Emitter, Runtime};

use crate::screenshot::{ScreenshotCaptureEvent, ScreenshotCaptureResult, ScreenshotEvents};

impl<R: Runtime> ScreenshotEvents for AppHandle<R> {
    fn capture_saved(&self, result: ScreenshotCaptureResult) {
        let _ = self.emit("screenshot_capture_saved", result);
    }

    fn capture_failed(&self, event: ScreenshotCaptureEvent) {
        let _ = self.emit("screenshot_capture_failed", event);
    }

    #[cfg(target_os = "linux")]
    fn hotkey_status_changed(&self, status: crate::screenshot::ScreenshotHotkeyStatus) {
        let _ = self.emit("screenshot_hotkey_status_changed", status);
    }
}
