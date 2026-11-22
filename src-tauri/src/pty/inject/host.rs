//! The `InjectionHost` trait: the state and effects the serialized PTY injection path
//! needs (sessions, routes, menu guard, typing hold, held-wake notes, busy marking). It only
//! declares the boundary; implementations and the injection flow live elsewhere.

use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::RwLock;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::pty::menu_guard::MenuGuard;
use crate::session::manager::SessionManager;

/// State and effects used at the serialized injection boundary.
pub trait InjectionHost: Sync {
    fn sessions(&self) -> &Arc<RwLock<SessionManager>>;
    fn routes(&self) -> &Arc<Mutex<PtyManager>>;
    fn menu(&self) -> Option<&Arc<MenuGuard>>;
    fn typing_hold_defers_injection(&self, id: Uuid) -> impl Future<Output = bool> + Send;
    fn note_held_wake(&self, id: Uuid, message: &str);
    fn clear_held_wake(&self, id: Uuid, message: &str);
    fn mark_successful_pty_write_busy(
        &self,
        id: Uuid,
        bytes: usize,
    ) -> impl Future<Output = ()> + Send;
    fn clear_control_write_mark(&self, id: Uuid);
}
