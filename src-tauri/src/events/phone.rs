//! Synchronous PTY status delivery; business decisions belong to phone.

use crate::phone::events::PtyInputEvents;
use tauri::{AppHandle, Emitter, Runtime};

impl<R: Runtime> PtyInputEvents for AppHandle<R> {
    fn pty_input_status(&self, result: &crate::phone::types::PtyInputResult) -> bool {
        self.emit("pty_input_status", result).is_ok()
    }
}

#[cfg(test)]
mod tests;
