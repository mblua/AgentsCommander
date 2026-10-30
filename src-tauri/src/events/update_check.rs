use crate::update_check::{UpdateEvents, UpdateInfo};
use tauri::{AppHandle, Emitter, Runtime};

impl<R: Runtime> UpdateEvents for AppHandle<R> {
    fn npm_update_available(&self, info: UpdateInfo) {
        let _ = self.emit("npm_update_available", &info);
    }
}
