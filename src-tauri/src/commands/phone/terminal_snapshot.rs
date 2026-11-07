use crate::config::settings::SettingsState;
use crate::phone::terminal_snapshot::task::{SnapshotTask, SnapshotTaskHost};
use crate::phone::terminal_snapshot::TerminalSnapshotHost;
use crate::pty::manager::PtyManager;
use crate::pty::terminal_snapshot::TerminalSnapshotState;
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use crate::RestoreInProgress;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tauri::Manager;

#[allow(dead_code, reason = "3946 phased consumer migration")]
struct TauriSnapshotTask {
    inner: tauri::async_runtime::JoinHandle<()>,
}

impl Future for TauriSnapshotTask {
    type Output = Result<(), String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.inner)
            .poll(cx)
            .map_err(|error| error.to_string())
    }
}

impl SnapshotTask for TauriSnapshotTask {
    fn abort(&self) {
        self.inner.abort();
    }
}

impl<R: tauri::Runtime> SnapshotTaskHost for tauri::AppHandle<R> {
    fn spawn_snapshot_task(
        &self,
        future: futures::future::BoxFuture<'static, ()>,
    ) -> Pin<Box<dyn SnapshotTask>> {
        Box::pin(TauriSnapshotTask {
            inner: tauri::async_runtime::spawn(future),
        })
    }
}

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
