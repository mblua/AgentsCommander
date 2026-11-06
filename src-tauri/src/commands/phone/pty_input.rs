use crate::app_state::RestoreInProgress;
use crate::config::settings::SettingsState;
use crate::phone::mailbox::pty_observation::{
    PtyAuthorityState, PtyReadinessState, PtyTargetState, WakeDeliveryState,
};
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use std::sync::{Arc, Mutex};
use tauri::Manager;

impl<R: tauri::Runtime> PtyAuthorityState for tauri::AppHandle<R> {
    fn pty_sessions(&self) -> &Arc<tokio::sync::RwLock<SessionManager>> {
        self.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .inner()
    }

    fn pty_routes(&self) -> &Arc<Mutex<PtyManager>> {
        self.state::<Arc<Mutex<PtyManager>>>().inner()
    }

    fn pty_settings(&self) -> &SettingsState {
        self.state::<SettingsState>().inner()
    }
}

impl<R: tauri::Runtime> PtyTargetState for tauri::AppHandle<R> {
    fn pty_readiness(&self) -> &Arc<IdleDetector> {
        self.state::<Arc<IdleDetector>>().inner()
    }

    fn pty_restore(&self) -> Option<&Arc<RestoreInProgress>> {
        self.try_state::<Arc<RestoreInProgress>>()
            .map(|state| state.inner())
    }

    fn pty_purge(&self) -> Option<&Arc<PurgeGuard>> {
        self.try_state::<Arc<PurgeGuard>>()
            .map(|state| state.inner())
    }
}

impl<R: tauri::Runtime> PtyReadinessState for tauri::AppHandle<R> {
    fn pty_menu(&self) -> Option<&Arc<crate::pty::menu_guard::MenuGuard>> {
        self.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>()
            .map(|state| state.inner())
    }
}

impl<R: tauri::Runtime> WakeDeliveryState for tauri::AppHandle<R> {
    fn message_store(&self) -> Option<&crate::api::message_store::MessageStoreState> {
        self.try_state::<crate::api::message_store::MessageStoreState>()
            .map(|state| state.inner())
    }

    fn master_token(&self) -> Option<&crate::MasterToken> {
        self.try_state::<crate::MasterToken>()
            .map(|state| state.inner())
    }

    fn selection(&self) -> &crate::session::selection::SelectionCoordinator {
        self.state::<crate::session::selection::SelectionCoordinator>()
            .inner()
    }

    fn pty_routes_opt(&self) -> Option<&Arc<Mutex<PtyManager>>> {
        self.try_state::<Arc<Mutex<PtyManager>>>()
            .map(|state| state.inner())
    }

    fn pty_readiness_opt(&self) -> Option<&Arc<IdleDetector>> {
        self.try_state::<Arc<IdleDetector>>()
            .map(|state| state.inner())
    }
}

#[cfg(test)]
mod tests;
