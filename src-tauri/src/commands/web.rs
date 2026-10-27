use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::Manager;
use uuid::Uuid;

use crate::agent_update::{
    AgentUpdateCancelAllResponse, AgentUpdateCancelResponse, AgentUpdateOverviewRow,
    AgentUpdateStatus,
};
use crate::commands::ac_discovery::AcDiscoveryResult;
use crate::commands::config::{
    ApplyCodingAgentProfileSelectionRequest, ApplyCodingAgentProfileSelectionResult,
};
use crate::config::projects::ProjectRegistration;
use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::web::commands::{AgentUpdateOperations, WebOperations, WebSessionCreate};

impl crate::web::commands::WsState {
    pub(crate) fn new(
        session_mgr: std::sync::Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>,
        pty_mgr: std::sync::Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>,
        settings: crate::config::settings::SettingsState,
        broadcaster: crate::web::broadcast::WsBroadcaster,
        app_handle: tauri::AppHandle,
    ) -> Self {
        Self {
            session_mgr,
            pty_mgr,
            settings,
            broadcaster,
            operations: Arc::new(app_handle.clone()),
            desktop_events: Arc::new(app_handle),
        }
    }
}

impl AgentUpdateOperations for tauri::AppHandle {
    fn agent_update_status(&self) -> Result<AgentUpdateStatus, String> {
        crate::commands::config::agent_update_status_inner(self)
    }

    fn agent_update_answer<'a>(
        &'a self,
        settings: &'a SettingsState,
        command: String,
        enabled: bool,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::config::agent_update_answer_inner(self, settings, command, enabled)
                .await
        })
    }

    fn agent_update_cancel<'a>(
        &'a self,
        command: String,
    ) -> Pin<Box<dyn Future<Output = Result<AgentUpdateCancelResponse, String>> + Send + 'a>> {
        Box::pin(
            async move { crate::commands::config::agent_update_cancel_inner(self, command).await },
        )
    }

    fn agent_updates_cancel_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<AgentUpdateCancelAllResponse, String>> + Send + 'a>>
    {
        Box::pin(async move { crate::commands::config::agent_updates_cancel_all_inner(self).await })
    }

    fn agent_update_overview<'a>(
        &'a self,
        settings: &'a SettingsState,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AgentUpdateOverviewRow>, String>> + Send + 'a>>
    {
        Box::pin(async move {
            crate::commands::config::agent_update_overview_inner(self, settings).await
        })
    }

    fn install_coding_agent<'a>(
        &'a self,
        settings: &'a SettingsState,
        key: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::config::install_coding_agent_inner(self, settings, key).await
        })
    }
}

impl WebOperations for tauri::AppHandle {
    fn selection(&self) -> &crate::session::selection::SelectionCoordinator {
        self.state::<crate::session::selection::SelectionCoordinator>()
            .inner()
    }

    fn warnings(&self) -> &crate::session::warnings::SessionWarningState {
        self.state::<crate::session::warnings::SessionWarningState>()
            .inner()
    }

    fn purge_guard(&self) -> Option<&Arc<crate::session::purge_guard::PurgeGuard>> {
        self.try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .map(|state| state.inner())
    }

    fn response_close(&self) -> Option<&crate::pty::manager::ResponseCloseState> {
        self.try_state::<crate::pty::manager::ResponseCloseState>()
            .map(|state| state.inner())
    }

    fn detached(&self) -> &crate::app_state::DetachedSessionsState {
        self.state::<crate::app_state::DetachedSessionsState>()
            .inner()
    }

    fn restore(&self) -> Option<&Arc<crate::app_state::RestoreInProgress>> {
        self.try_state::<Arc<crate::app_state::RestoreInProgress>>()
            .map(|state| state.inner())
    }

    fn emit_agent_file_save_events(&self, reply: &crate::config::settings::AgentFileSaveReply) {
        crate::commands::config::emit_agent_file_save_events(self, reply);
    }

    fn create_session<'a>(
        &'a self,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        request: WebSessionCreate,
    ) -> Pin<Box<dyn Future<Output = Result<SessionInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::session::create_session_inner(
                self,
                session_mgr,
                pty_mgr,
                request.shell,
                request.shell_args,
                request.cwd,
                request.session_name,
                request.agent_id,
                request.agent_label,
                false,
                Vec::new(),
                true,
                request.resolved_spawn,
                request.resolved_agent_host_shell,
                None,
                crate::commands::session::CreateSelectionIntent::User,
            )
            .await
        })
    }

    fn destroy_session<'a>(
        &'a self,
        uuid: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { crate::commands::session::destroy_session_inner(self, uuid).await })
    }

    fn mark_successful_pty_write_busy<'a>(
        &'a self,
        uuid: Uuid,
        byte_count: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::pty::mark_successful_pty_write_busy(self, uuid, byte_count).await
        })
    }

    fn note_user_message_to_session<'a>(
        &'a self,
        uuid: Uuid,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::pty::note_user_message_to_session(
                self,
                uuid,
                crate::commands::pty::UserInputSource::Web(data),
            )
            .await
        })
    }

    fn apply_coding_agent_profile_selection<'a>(
        &'a self,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        settings: &'a SettingsState,
        request: ApplyCodingAgentProfileSelectionRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<(ApplyCodingAgentProfileSelectionResult, Value), String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            crate::commands::config::apply_coding_agent_profile_selection_inner(
                self,
                session_mgr,
                pty_mgr,
                settings,
                request,
            )
            .await
        })
    }

    fn discover_project<'a>(
        &'a self,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        path: &'a str,
        settings: &'a SettingsState,
    ) -> Pin<Box<dyn Future<Output = Result<AcDiscoveryResult, String>> + Send + 'a>> {
        Box::pin(async move {
            let watcher =
                self.state::<Arc<crate::commands::ac_discovery::DiscoveryBranchWatcher>>();
            let clocks = self.state::<crate::config::coordinator_clocks::CoordinatorClocksState>();
            crate::commands::ac_discovery::discover_project_inner(
                self,
                session_mgr,
                path,
                settings,
                watcher.inner(),
                clocks.inner(),
            )
            .await
        })
    }

    fn open_project<'a>(
        &'a self,
        settings: &'a SettingsState,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ProjectRegistration, String>> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::ac_discovery::open_project_inner(self, settings, path).await
        })
    }

    fn remove_project<'a>(
        &'a self,
        settings: &'a SettingsState,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::ac_discovery::remove_project_inner(self, settings, path).await
        })
    }

    fn archive_project<'a>(
        &'a self,
        settings: &'a SettingsState,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::ac_discovery::archive_project_inner(
                self,
                settings,
                session_mgr,
                pty_mgr,
                path,
            )
            .await
        })
    }

    fn unarchive_project<'a>(
        &'a self,
        settings: &'a SettingsState,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ProjectRegistration, String>> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::ac_discovery::unarchive_project_inner(self, settings, path).await
        })
    }

    fn agency_status<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let status =
                tauri::async_runtime::spawn_blocking(crate::cli::agency_templates::status_cache)
                    .await
                    .map_err(|e| format!("Agency template status task failed: {}", e))??;
            serde_json::to_value(status).map_err(|e| e.to_string())
        })
    }

    fn agency_update<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let result = tauri::async_runtime::spawn_blocking(|| {
                crate::cli::agency_templates::update_cache(
                    crate::cli::agency_templates::AgencyTemplatesUpdateArgs::default_ui_update(),
                )
            })
            .await
            .map_err(|e| format!("Agency template update task failed: {}", e))??;
            serde_json::to_value(result).map_err(|e| e.to_string())
        })
    }
}

#[cfg(test)]
mod tests;
