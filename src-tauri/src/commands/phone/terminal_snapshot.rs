use crate::config::settings::SettingsState;
use crate::phone::terminal_snapshot::TerminalSnapshotHost;
use crate::pty::manager::PtyManager;
use crate::pty::terminal_snapshot::TerminalSnapshotState;
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use crate::RestoreInProgress;
use std::sync::Arc;
use tauri::Manager;

impl<R: tauri::Runtime> TerminalSnapshotHost for tauri::AppHandle<R> {
    fn snapshot_state_opt(&self) -> Option<&Arc<TerminalSnapshotState>> {
        self.try_state::<Arc<TerminalSnapshotState>>()
            .map(|state| state.inner())
    }

    fn session_manager_opt(&self) -> Option<&Arc<tokio::sync::RwLock<SessionManager>>> {
        self.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .map(|state| state.inner())
    }

    fn pty_manager_opt(&self) -> Option<&Arc<std::sync::Mutex<PtyManager>>> {
        self.try_state::<Arc<std::sync::Mutex<PtyManager>>>()
            .map(|state| state.inner())
    }

    fn settings_opt(&self) -> Option<&SettingsState> {
        self.try_state::<SettingsState>().map(|state| state.inner())
    }

    fn restore_opt(&self) -> Option<&Arc<RestoreInProgress>> {
        self.try_state::<Arc<RestoreInProgress>>()
            .map(|state| state.inner())
    }

    fn purge_opt(&self) -> Option<&Arc<PurgeGuard>> {
        self.try_state::<Arc<PurgeGuard>>()
            .map(|state| state.inner())
    }
}
