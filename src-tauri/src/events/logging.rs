//! Error-log ping delivery; buffering and timing belong to the core.

use crate::logging::ErrorLogEvents;
use tauri::{AppHandle, Emitter, Runtime};

impl<R: Runtime> ErrorLogEvents for AppHandle<R> {
    fn error_log_event(&self) {
        let _ = self.emit("error_log_event", ());
    }
}

#[cfg(test)]
mod tests;
