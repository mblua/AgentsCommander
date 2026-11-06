use std::sync::{Arc, Mutex};

use tauri::Manager;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::pty::inject::InjectionHost;
use crate::pty::manager::PtyManager;
use crate::pty::menu_guard::MenuGuard;
use crate::session::manager::SessionManager;

impl<R: tauri::Runtime> InjectionHost for tauri::AppHandle<R> {
    fn sessions(&self) -> &Arc<RwLock<SessionManager>> {
        self.state::<Arc<RwLock<SessionManager>>>().inner()
    }

    fn routes(&self) -> &Arc<Mutex<PtyManager>> {
        self.state::<Arc<Mutex<PtyManager>>>().inner()
    }

    fn menu(&self) -> Option<&Arc<MenuGuard>> {
        self.try_state::<Arc<MenuGuard>>()
            .map(|state| state.inner())
    }

    async fn typing_hold_defers_injection(&self, id: Uuid) -> bool {
        super::typing_hold_defers_injection(self, id).await
    }

    fn note_held_wake(&self, id: Uuid, message: &str) {
        super::note_held_wake(self, id, message);
    }

    fn clear_held_wake(&self, id: Uuid, message: &str) {
        super::clear_held_wake(self, id, message);
    }

    async fn mark_successful_pty_write_busy(&self, id: Uuid, bytes: usize) {
        super::mark_successful_pty_write_busy(self, id, bytes).await;
    }

    fn clear_control_write_mark(&self, id: Uuid) {
        super::clear_control_write_mark(self, id);
    }
}
