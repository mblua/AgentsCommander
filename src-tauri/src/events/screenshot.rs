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
}
