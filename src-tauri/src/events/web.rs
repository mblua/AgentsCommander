use serde_json::Value;
use tauri::{Emitter, Manager};

use crate::web::broadcast::WsBroadcaster;
use crate::web::event_broadcast::{DesktopBroadcastEvents, ManagedBroadcastEvents};

impl<R: tauri::Runtime> DesktopBroadcastEvents for tauri::AppHandle<R> {
    fn desktop_event(&self, event: &str, payload: &Value) {
        let _ = self.emit(event, payload.clone());
    }
}

impl<R: tauri::Runtime> ManagedBroadcastEvents for tauri::AppHandle<R> {
    fn managed_event(&self, event: &str, payload: &Value) -> bool {
        self.desktop_event(event, payload);
        if let Some(broadcaster) = self.try_state::<WsBroadcaster>() {
            broadcaster.broadcast_event(event, payload);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests;
