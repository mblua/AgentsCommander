use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::config::injected_messages::MessageWarnings;
use crate::config::settings::SettingsState;
use crate::phone::mailbox::{
    InternalNoticeGuard, InternalSystemNotice, InternalSystemTarget, MailboxPoller,
};
use crate::pty::context_scrape::ContextScraper;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::auto_close::{AutoClose, AutoCloseServices};
use crate::session::context_alerts::{ContextAlertMonitor, ContextAlertServices};
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use crate::session::selection::SelectionCoordinator;
use crate::session::session::Session;
use crate::shutdown::ShutdownSignal;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::session::warnings::{
    SessionWarning, SessionWarningEvents, SessionWarningState, SESSION_ENV_WARNING_EVENT,
};

impl<R: Runtime> SessionWarningEvents for AppHandle<R> {
    fn session_warning(&self, warning: SessionWarning) {
        if let Some(state) = self.try_state::<SessionWarningState>() {
            match state.lock() {
                Ok(mut buffer) => buffer.push(warning.clone()),
                Err(err) => {
                    log::warn!(
                        target: "agentscommander_lib::session::warnings",
                        "[session-warning] warning buffer lock poisoned; live emit only: {}",
                        err
                    );
                }
            }
        } else {
            log::warn!(
                target: "agentscommander_lib::session::warnings",
                "[session-warning] warning buffer state missing; live emit only"
            );
        }
        let _ = self.emit(SESSION_ENV_WARNING_EVENT, warning);
    }
}

struct ContextAlertTauriServices<R: Runtime> {
    app: AppHandle<R>,
    messages: Arc<MessageWarnings>,
}

impl<R: Runtime> ContextAlertServices for ContextAlertTauriServices<R> {
    fn public_session(&self, id: Uuid) -> BoxFuture<'static, Option<Session>> {
        let app = self.app.clone();
        Box::pin(async move {
            let state = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()?;
            let manager = state.read().await.clone();
            manager.get_session(id).await
        })
    }

    fn registered(&self, id: Uuid) -> bool {
        self.app
            .try_state::<Arc<ContextScraper>>()
            .is_some_and(|scraper| scraper.is_session_registered(id))
    }

    fn retire_sample_registration(&self, id: Uuid) {
        if let Some(scraper) = self.app.try_state::<Arc<ContextScraper>>() {
            scraper.retire_session(id);
        }
    }

    fn validate_delivery_state(
        &self,
        id: Uuid,
        target: &InternalSystemTarget,
    ) -> Result<(), String> {
        if self
            .app
            .try_state::<Arc<PurgeGuard>>()
            .is_some_and(|guard| guard.blocks_agent(target.fqn()))
        {
            return Err(format!("purge-room blocks '{}'", target.fqn()));
        }
        let scraper = self
            .app
            .try_state::<Arc<ContextScraper>>()
            .ok_or_else(|| "Context scraper is unavailable".to_string())?;
        if !scraper.is_session_registered(id) {
            return Err("Sampled session is no longer registered".to_string());
        }
        Ok(())
    }

    fn deliver(
        &self,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        cancellation: CancellationToken,
        guard: InternalNoticeGuard,
    ) -> BoxFuture<'static, Result<(), String>> {
        let app = self.app.clone();
        let messages = Arc::clone(&self.messages);
        Box::pin(async move {
            MailboxPoller::new(messages)
                .deliver_internal_system_notice(&app, target, notice, cancellation, guard)
                .await
        })
    }
}

impl ContextAlertMonitor {
    pub(crate) fn start(
        app: AppHandle,
        cancellation: CancellationToken,
        messages: Arc<MessageWarnings>,
    ) -> Arc<Self> {
        start_on(app, cancellation, messages)
    }
}

fn start_on<R: Runtime>(
    app: AppHandle<R>,
    cancellation: CancellationToken,
    messages: Arc<MessageWarnings>,
) -> Arc<ContextAlertMonitor> {
    ContextAlertMonitor::start_with_services(
        Arc::new(ContextAlertTauriServices { app, messages }),
        cancellation,
        tauri::async_runtime::handle().inner().clone(),
    )
}

fn auto_close_state<R: Runtime, T: Clone + Send + Sync + 'static>(app: &AppHandle<R>) -> T {
    app.try_state::<T>()
        .unwrap_or_else(|| {
            panic!(
                "state() called before manage() for {}",
                std::any::type_name::<T>()
            )
        })
        .inner()
        .clone()
}

impl<R: Runtime> AutoCloseServices for AppHandle<R> {
    fn settings(&self) -> SettingsState {
        auto_close_state(self)
    }

    fn session_manager(&self) -> Arc<tokio::sync::RwLock<SessionManager>> {
        auto_close_state(self)
    }

    fn pty_manager(&self) -> Arc<Mutex<PtyManager>> {
        auto_close_state(self)
    }

    fn idle_detector(&self) -> Arc<IdleDetector> {
        auto_close_state(self)
    }

    fn selection_coordinator(&self) -> SelectionCoordinator {
        auto_close_state(self)
    }

    fn coordinator_clocks(&self) -> Option<CoordinatorClocksState> {
        self.try_state::<CoordinatorClocksState>()
            .map(|s| s.inner().clone())
    }

    fn idle_badge(&self, cwd: &str, at: DateTime<Utc>) {
        let _ = self.emit(
            "coordinator_clock_updated",
            serde_json::json!({ "replicaPath": cwd, "lastUserMessageAt": at.to_rfc3339() }),
        );
    }

    fn auto_closed_badge(&self, cwd: &str, at: DateTime<Utc>) {
        let _ = self.emit(
            "coordinator_auto_close_changed",
            serde_json::json!({ "replicaPath": cwd, "autoClosedAt": at.to_rfc3339() }),
        );
    }
}

impl AutoClose {
    pub(crate) fn start(app: AppHandle, shutdown: ShutdownSignal) {
        tauri::async_runtime::spawn(async move {
            AutoClose::run(app, shutdown).await;
        });
    }
}

#[cfg(test)]
mod tests;
