//! Tauri sink for update checks: `UpdateEvents` emits `npm_update_available` with the
//! `UpdateInfo` payload.
//! It does not check for updates; that lives in `crate::update_check`.

use crate::update_check::{UpdateEvents, UpdateInfo};
use tauri::{AppHandle, Emitter, Runtime};

impl<R: Runtime> UpdateEvents for AppHandle<R> {
    fn npm_update_available(&self, info: UpdateInfo) {
        let _ = self.emit("npm_update_available", &info);
    }
}
