//! Lazy native state access for selection's neutral host contract.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, Runtime};

use crate::app_state::DetachedSessionsState;
use crate::config::settings::SettingsState;
use crate::pty::manager::{PtyManager, ResponseCloseState};
use crate::session::manager::SessionManager;
use crate::session::selection::task::SelectionTasks;
use crate::session::selection::{
    SelectionContext, SelectionCoordinator, SelectionHost, SelectionTransaction,
};

use super::turn::NATIVE_SELECTION_TASKS;

impl<R: Runtime> SelectionContext for R {
    type Host = AppHandle<R>;
}

impl<R: Runtime> SelectionHost<R> for AppHandle<R> {
    fn sessions_required(&self) -> &Arc<tokio::sync::RwLock<SessionManager>> {
        self.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .inner()
    }

    fn pty_required(&self) -> &Arc<Mutex<PtyManager>> {
        self.state::<Arc<Mutex<PtyManager>>>().inner()
    }

    fn pty_optional(&self) -> Option<&Arc<Mutex<PtyManager>>> {
        self.try_state::<Arc<Mutex<PtyManager>>>()
            .map(|state| state.inner())
    }

    fn detached_required(&self) -> &DetachedSessionsState {
        self.state::<DetachedSessionsState>().inner()
    }

    fn settings_required(&self) -> &SettingsState {
        self.state::<SettingsState>().inner()
    }

    fn response_close_required(&self) -> &ResponseCloseState {
        self.state::<ResponseCloseState>().inner()
    }

    fn response_close_optional(&self) -> Option<&ResponseCloseState> {
        self.try_state::<ResponseCloseState>()
            .map(|state| state.inner())
    }

    fn coordinator_optional(&self) -> Option<&SelectionCoordinator> {
        self.try_state::<SelectionCoordinator>()
            .map(|state| state.inner())
    }

    fn tasks(&self) -> &dyn SelectionTasks {
        &NATIVE_SELECTION_TASKS
    }

    fn focus_window(&self, label: &str) -> Option<Result<(), String>> {
        self.get_webview_window(label)
            .map(|window| window.set_focus().map_err(|error| error.to_string()))
    }

    fn destroy_window(&self, label: &str) -> Option<Result<(), String>> {
        self.get_webview_window(label)
            .map(|window| window.destroy().map_err(|error| error.to_string()))
    }
}

impl<R: Runtime> SelectionTransaction<R> {
    pub(crate) fn app(&self) -> &AppHandle<R> {
        self.host()
    }

    #[cfg(test)]
    pub(crate) fn for_test(app: AppHandle<R>) -> Self {
        Self::for_test_host(app)
    }
}
