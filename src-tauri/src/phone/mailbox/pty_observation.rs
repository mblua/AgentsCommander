use crate::app_state::RestoreInProgress;
use crate::config::settings::SettingsState;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use std::sync::{Arc, Mutex};

/// Borrowed host PTY authority; each getter observes state only when requested.
pub(crate) trait PtyAuthorityState: Sync {
    fn pty_sessions(&self) -> &Arc<tokio::sync::RwLock<SessionManager>>;
    fn pty_routes(&self) -> &Arc<Mutex<PtyManager>>;
    fn pty_settings(&self) -> &SettingsState;
}

/// Borrowed target observations; optional registration remains optional.
pub(crate) trait PtyTargetState: PtyAuthorityState {
    fn pty_readiness(&self) -> &Arc<IdleDetector>;
    fn pty_restore(&self) -> Option<&Arc<RestoreInProgress>>;
    fn pty_purge(&self) -> Option<&Arc<PurgeGuard>>;
}

/// Borrowed readiness observations; menu registration remains optional.
pub(crate) trait PtyReadinessState: PtyTargetState {
    fn pty_menu(&self) -> Option<&Arc<crate::pty::menu_guard::MenuGuard>>;
}

/// Borrowed wake-delivery observations; optional registration remains optional.
pub(crate) trait WakeDeliveryState: PtyReadinessState {
    fn message_store(&self) -> Option<&crate::api::message_store::MessageStoreState>;
    /// Security credential: compare with `matches()`; never log it.
    fn master_token(&self) -> Option<&crate::MasterToken>;
    fn selection(&self) -> &crate::session::selection::SelectionCoordinator;
    /// Optional route registry: `None` when no `PtyManager` is registered.
    fn pty_routes_opt(&self) -> Option<&Arc<Mutex<PtyManager>>>;
    /// Optional readiness detector: `None` when no `IdleDetector` is registered.
    fn pty_readiness_opt(&self) -> Option<&Arc<IdleDetector>>;
}

#[cfg(test)]
mod tests;
