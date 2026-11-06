use crate::api::message_store::PtyInputTargetOwnership;
use crate::app_state::RestoreInProgress;
use crate::config::agent_command::AgentSpawnCommand;
use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::config::sessions_persistence::CreationGateEnforcement;
use crate::config::settings::SettingsState;
use crate::phone::events::{MailboxDeliveryEvents, PtyInputEvents};
use crate::phone::mailbox::browser::snapshot::WebBrowserSnapshot;
use crate::phone::mailbox::pty_observation::WakeDeliveryState;
use crate::phone::mailbox::response_observation::{
    ResponseAdmissionState, ResponseCompletionState,
};
use crate::pty::backend::{PtyViewport, ResolvedAgentHostShell};
use crate::pty::inject::host::InjectionHost;
use crate::pty::inject::BoundaryMetadataOutcome;
use crate::pty::manager::PtyManager;
use crate::pty::menu_guard::MenuGuardEvents;
use crate::pty::terminal_snapshot::TerminalSnapshotState;
use crate::session::create_contracts::{
    CreateSelectionIntent, OwnedReplacementCreateRequest, OwnedReplacementCreateStep,
    PtyTargetCreateStep,
};
use crate::session::maintenance::PendingSelfClear;
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use crate::session::selection::{
    OwnedReplacementContinuation, SelectionCoordinator, TrustedRestartIntent,
};
use crate::session::session::{SessionCommunication, SessionInfo, SessionRepo};
use crate::session::warnings::SessionWarningEvents;
use crate::shutdown::ShutdownSignal;
use crate::{AppOutbox, MasterToken};
use futures::future::BoxFuture;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Settlement of the native coordinator create ticket used by mailbox tests.
#[cfg(test)]
#[allow(dead_code, reason = "3946 phased consumer migration")]
pub(crate) trait CoordinatorTicket {
    fn succeeded(self);
    fn failed(self);
}

/// Native capabilities available to captured mailbox work.
pub(crate) trait MailboxHost:
    WakeDeliveryState
    + ResponseAdmissionState
    + ResponseCompletionState
    + InjectionHost
    + PtyInputEvents
    + MailboxDeliveryEvents
    + MenuGuardEvents
    + SessionWarningEvents
    + Clone
    + Send
    + Sync
    + 'static
{
    #[cfg(test)]
    type CoordinatorTicket: CoordinatorTicket;

    #[cfg(test)]
    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn begin_coordinator_create(&self, cwd: &str) -> Option<Self::CoordinatorTicket>;

    fn master_required(&self) -> &MasterToken;

    #[cfg_attr(not(test), allow(dead_code, reason = "3946 phased consumer migration"))]
    fn selection_opt(&self) -> Option<&SelectionCoordinator>;

    fn restore_required(&self) -> &Arc<RestoreInProgress>;

    fn purge_required(&self) -> &Arc<PurgeGuard>;

    fn pending_clear(&self) -> &Arc<PendingSelfClear>;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn clocks(&self) -> Option<&CoordinatorClocksState>;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn shutdown_opt(&self) -> Option<&ShutdownSignal>;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn outbox(&self) -> &AppOutbox;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn snapshot_state(&self) -> Option<&Arc<TerminalSnapshotState>>;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    #[allow(clippy::too_many_arguments)]
    fn create_session_inner<'a>(
        &'a self,
        session_mgr: &'a Arc<RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        shell: String,
        shell_args: Vec<String>,
        cwd: String,
        session_name: Option<String>,
        agent_id: Option<String>,
        agent_label: Option<String>,
        skip_tooling_save: bool,
        git_repos: Vec<SessionRepo>,
        skip_auto_resume: bool,
        resolved_spawn: Option<AgentSpawnCommand>,
        resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
        viewport: Option<PtyViewport>,
        selection_intent: CreateSelectionIntent,
    ) -> impl Future<Output = Result<SessionInfo, String>> + Send + 'a;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    #[allow(clippy::too_many_arguments)]
    fn create_session_inner_with_pty_target_ownership<'a, 'guard>(
        &'a self,
        session_mgr: &'a Arc<RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        shell: String,
        shell_args: Vec<String>,
        cwd: String,
        session_name: Option<String>,
        agent_id: Option<String>,
        agent_label: Option<String>,
        skip_tooling_save: bool,
        git_repos: Vec<SessionRepo>,
        skip_auto_resume: bool,
        resolved_spawn: Option<AgentSpawnCommand>,
        resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
        viewport: Option<PtyViewport>,
        selection_intent: CreateSelectionIntent,
        target_ownership: &'a PtyInputTargetOwnership<'guard>,
    ) -> impl Future<Output = Result<PtyTargetCreateStep, String>> + Send + 'a
    where
        'guard: 'a;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn create_owned_response_replacement<'a, 'guard>(
        &'a self,
        request: OwnedReplacementCreateRequest,
        continuation: OwnedReplacementContinuation,
        target: &'a PtyInputTargetOwnership<'guard>,
    ) -> impl Future<Output = Result<OwnedReplacementCreateStep, String>> + Send + 'a
    where
        'guard: 'a;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn background_destroy_session_inner<'a>(
        &'a self,
        uuid: Uuid,
    ) -> impl Future<Output = Result<(), String>> + Send + 'a;

    #[allow(clippy::too_many_arguments)]
    fn restart_session_inner_with_intent<'a>(
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
    ) -> impl Future<Output = Result<SessionInfo, String>> + Send + 'a;

    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn attach_persisted_telegram_if_configured<'a>(
        &'a self,
        session_id: Uuid,
        bot_id: Option<&'a str>,
    ) -> impl Future<Output = ()> + Send + 'a;

    fn stamp_fresh_boundary_to_session<'a>(
        &'a self,
        session_id: Uuid,
    ) -> impl Future<Output = BoundaryMetadataOutcome> + Send + 'a;

    fn note_post_boundary_content_to_session<'a>(
        &'a self,
        session_id: Uuid,
    ) -> impl Future<Output = BoundaryMetadataOutcome> + Send + 'a;

    fn spawn_background(&self, future: BoxFuture<'static, ()>);
}

/// Synchronous browser effects used by mailbox policy.
#[allow(dead_code, reason = "3946 phased consumer migration")]
pub(crate) trait BrowserHost: MailboxHost {
    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn agent_navigate(&self, url: &str) -> Result<WebBrowserSnapshot, String>;
    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn reload(&self) -> Result<WebBrowserSnapshot, String>;
    #[allow(dead_code, reason = "3946 phased consumer migration")]
    fn snapshot(&self) -> WebBrowserSnapshot;
}
