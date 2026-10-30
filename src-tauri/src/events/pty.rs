//! PTY event transport and task scheduling at the Tauri edge.

use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::pty::menu_guard::{MenuGuard, MenuGuardEvents};
use crate::pty::output::{PtyOutputEmitError, PtyOutputTarget, PtyOutputTask};
use crate::session::manager::SessionManager;
use crate::session::session::SessionCommunication;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

/// Emits once per attached label and schedules flushes on the Tauri-owned runtime.
pub(crate) fn output_target<R: tauri::Runtime>(app: AppHandle<R>) -> PtyOutputTarget {
    PtyOutputTarget::new(
        move |labels, payload| {
            let mut result = Ok(());
            for label in labels {
                if app
                    .emit_to(label.as_str(), "pty_output", payload.clone())
                    .is_err()
                {
                    result = Err(PtyOutputEmitError::Emit);
                }
            }
            result
        },
        spawn_output_task,
    )
}

fn spawn_output_task(task: PtyOutputTask) {
    tauri::async_runtime::spawn(task);
}

impl<R: tauri::Runtime> MenuGuardEvents for AppHandle<R> {
    fn publish(&self, id: Uuid, communication: Option<&SessionCommunication>) {
        crate::session::selection::publish_session_communication(self, id, communication);
    }
}

impl MenuGuard {
    pub async fn scan_tick<R: tauri::Runtime>(&self, app: &AppHandle<R>) {
        let Some(settings_state) = app.try_state::<SettingsState>() else {
            return;
        };
        let settings = match settings_state.current_read().await {
            Ok(settings) => settings.clone(),
            Err(_) => {
                log::debug!("[MenuGuard] settings unavailable; skipping scan");
                return;
            }
        };

        let Some(session_mgr_state) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        else {
            return;
        };

        if !settings.menu_guard_enabled {
            self.scan_tick_with(&settings, session_mgr_state.inner(), None, app)
                .await;
            return;
        }

        let Some(pty_mgr_state) = app.try_state::<Arc<std::sync::Mutex<PtyManager>>>() else {
            return;
        };

        self.scan_tick_with(
            &settings,
            session_mgr_state.inner(),
            Some(pty_mgr_state.inner()),
            app,
        )
        .await;
    }

    pub fn start(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        shutdown: crate::shutdown::ShutdownSignal,
    ) {
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.token().cancelled() => break,
                    _ = interval.tick() => {
                        this.scan_tick(&app).await;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
