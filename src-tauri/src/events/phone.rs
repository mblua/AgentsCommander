//! Synchronous phone event delivery; business decisions belong to phone.

use crate::phone::events::{MailboxDeliveryEvents, PtyInputEvents};
use tauri::{AppHandle, Emitter, Runtime};

impl<R: Runtime> PtyInputEvents for AppHandle<R> {
    fn pty_input_status(&self, result: &crate::phone::types::PtyInputResult) -> bool {
        self.emit("pty_input_status", result).is_ok()
    }
}

impl<R: Runtime> MailboxDeliveryEvents for AppHandle<R> {
    fn message_delivered(&self, payload: &serde_json::Value) -> bool {
        self.emit("message_delivered", payload).is_ok()
    }
}

#[cfg(test)]
mod tests;
