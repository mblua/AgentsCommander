use crate::api::message_store::PtyInputTargetOwnership;
use crate::app_state::RestoreInProgress;
use crate::commands::pty::BoundaryMetadataOutcome;
use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::config::sessions_persistence::CreationGateEnforcement;
use crate::config::settings::SettingsState;
use crate::phone::mailbox::browser::snapshot::WebBrowserSnapshot;
use crate::phone::mailbox::host::{BrowserHost, MailboxHost, MailboxSessionCreate};
use crate::pty::manager::PtyManager;
use crate::session::create_contracts::{
    OwnedReplacementCreateRequest, OwnedReplacementCreateStep, PtyTargetCreateStep,
};
use crate::session::maintenance::PendingSelfClear;
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use crate::session::selection::{
    OwnedReplacementContinuation, SelectionCoordinator, TrustedRestartIntent,
};
use crate::session::session::{SessionCommunication, SessionInfo};
use crate::shutdown::ShutdownSignal;
use crate::{AppOutbox, MasterToken};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tokio::sync::RwLock;
use uuid::Uuid;

impl<R: tauri::Runtime> MailboxHost for tauri::AppHandle<R> {
    #[cfg(test)]
    type CoordinatorTicket = crate::commands::session::CoordinatorCreateTicket<R>;

    #[cfg(test)]
    fn begin_coordinator_create(&self, cwd: &str) -> Option<Self::CoordinatorTicket> {
        crate::commands::session::note_coordinator_create_on_app(self, cwd)
    }

    fn master_required(&self) -> &MasterToken {
        self.state::<MasterToken>().inner()
    }

    fn selection_opt(&self) -> Option<&SelectionCoordinator> {
        self.try_state::<SelectionCoordinator>()
            .map(|state| state.inner())
    }

    fn restore_required(&self) -> &Arc<RestoreInProgress> {
        self.state::<Arc<RestoreInProgress>>().inner()
    }

    fn purge_required(&self) -> &Arc<PurgeGuard> {
        self.state::<Arc<PurgeGuard>>().inner()
    }

    fn pending_clear(&self) -> &Arc<PendingSelfClear> {
        self.state::<Arc<PendingSelfClear>>().inner()
    }

    fn clocks(&self) -> Option<&CoordinatorClocksState> {
        self.try_state::<CoordinatorClocksState>()
            .map(|state| state.inner())
    }

    fn shutdown_opt(&self) -> Option<&ShutdownSignal> {
        self.try_state::<ShutdownSignal>()
            .map(|state| state.inner())
    }

    fn outbox(&self) -> &AppOutbox {
        self.state::<AppOutbox>().inner()
    }

    async fn create_session_inner<'a>(
        &'a self,
        request: MailboxSessionCreate<'a>,
    ) -> Result<SessionInfo, String> {
        crate::commands::session::create_session_inner(
            self,
            request.session_mgr,
            request.pty_mgr,
            request.shell,
            request.shell_args,
            request.cwd,
            request.session_name,
            request.agent_id,
            request.agent_label,
            request.skip_tooling_save,
            request.git_repos,
            request.skip_auto_resume,
            request.resolved_spawn,
            request.resolved_agent_host_shell,
            request.viewport,
            request.selection_intent,
        )
        .await
    }

    async fn create_session_inner_with_pty_target_ownership<'a, 'guard>(
        &'a self,
        request: MailboxSessionCreate<'a>,
        target_ownership: &'a PtyInputTargetOwnership<'guard>,
    ) -> Result<PtyTargetCreateStep, String>
    where
        'guard: 'a,
    {
        crate::commands::session::create_session_inner_with_pty_target_ownership(
            self,
            request.session_mgr,
            request.pty_mgr,
            request.shell,
            request.shell_args,
            request.cwd,
            request.session_name,
            request.agent_id,
            request.agent_label,
            request.skip_tooling_save,
            request.git_repos,
            request.skip_auto_resume,
            request.resolved_spawn,
            request.resolved_agent_host_shell,
            request.viewport,
            request.selection_intent,
            target_ownership,
        )
        .await
    }

    async fn create_owned_response_replacement<'a, 'guard>(
        &'a self,
        request: OwnedReplacementCreateRequest,
        continuation: OwnedReplacementContinuation,
        target: &'a PtyInputTargetOwnership<'guard>,
    ) -> Result<OwnedReplacementCreateStep, String>
    where
        'guard: 'a,
    {
        crate::commands::session::create_owned_response_replacement(
            self,
            request,
            continuation,
            target,
        )
        .await
    }

    async fn background_destroy_session_inner(&self, uuid: Uuid) -> Result<(), String> {
        crate::commands::session::background_destroy_session_inner(self, uuid).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn restart_session_inner_with_intent<'a>(
        &'a self,
        _session_mgr: &'a Arc<RwLock<SessionManager>>,
        _pty_mgr: &'a Arc<Mutex<PtyManager>>,
        _settings: &'a SettingsState,
        uuid: Uuid,
        agent_id: Option<String>,
        requested_profile: Option<String>,
        skip_auto_resume: Option<bool>,
        activate_after: bool,
        intent: TrustedRestartIntent,
        communication_override: Option<SessionCommunication>,
        enforcement: CreationGateEnforcement,
    ) -> Result<SessionInfo, String> {
        crate::commands::session::restart_session_inner_with_intent(
            self,
            _session_mgr,
            _pty_mgr,
            _settings,
            uuid,
            agent_id,
            requested_profile,
            skip_auto_resume,
            activate_after,
            intent,
            communication_override,
            enforcement,
        )
        .await
    }

    async fn attach_persisted_telegram_if_configured<'a>(
        &'a self,
        session_id: Uuid,
        bot_id: Option<&'a str>,
    ) {
        crate::commands::session::attach_persisted_telegram_if_configured(self, session_id, bot_id)
            .await;
    }

    async fn stamp_fresh_boundary_to_session(&self, session_id: Uuid) -> BoundaryMetadataOutcome {
        crate::commands::pty::stamp_fresh_boundary_to_session(self, session_id).await
    }

    async fn note_post_boundary_content_to_session(
        &self,
        session_id: Uuid,
    ) -> BoundaryMetadataOutcome {
        crate::commands::pty::note_post_boundary_content_to_session(self, session_id).await
    }

    fn spawn_background(&self, future: BoxFuture<'static, ()>) {
        drop(tauri::async_runtime::spawn(future));
    }
}

impl<R: tauri::Runtime> BrowserHost for tauri::AppHandle<R> {
    fn agent_navigate(&self, url: &str) -> Result<WebBrowserSnapshot, String> {
        crate::commands::web_browser::agent_navigate(self, url)
    }

    fn reload(&self) -> Result<WebBrowserSnapshot, String> {
        crate::commands::web_browser::reload(self)
    }

    fn snapshot(&self) -> WebBrowserSnapshot {
        crate::commands::web_browser::snapshot(self)
    }
}

#[cfg(test)]
impl<R: tauri::Runtime> crate::phone::mailbox::host::CoordinatorTicket
    for crate::commands::session::CoordinatorCreateTicket<R>
{
    fn succeeded(self) {
        crate::commands::session::CoordinatorCreateTicket::succeeded(self);
    }

    fn failed(self) {
        crate::commands::session::CoordinatorCreateTicket::failed(self);
    }
}

#[cfg(test)]
mod tests;
