use super::admission::CoordinatorCreateTicket;
use super::admission::OwnedCreateAdmission;
use crate::commands::session::lifecycle::LifecycleEntryAuthority;
use crate::commands::session::resume_probe::ContainerPathContext;
use crate::config::agent_command::AgentSpawnCommand;
use crate::config::session_context::ManagedContextTarget;
use crate::pty::backend::PtyViewport;
use crate::pty::backend::ResolvedAgentHostShell;
use crate::pty::container_credentials::ContainerCredentialPlan;
use crate::pty::container_paths::ContainerEnvWarning;
use crate::pty::container_repos::RepoMountResolution;
use crate::pty::manager::{PtyManager, SpawnMark};
use crate::resource_monitor::{AgentLaunchPermit, ResourceMonitorState};
use crate::session::manager::PendingCreateBinding;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::selection::{
    CreateFinalizationTicket, OwnedReplacementContinuation, SelectionCause, SelectionCoordinator,
    SelectionTransaction,
};
use crate::session::session::Session;
use crate::session::session::SessionInfo;
use crate::session::warnings::SessionWarning;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;

pub use crate::session::create_contracts::CreateSelectionIntent;
pub(crate) use crate::session::create_contracts::{
    OwnedReplacementCreateRequest, OwnedReplacementCreateStep,
};

pub(crate) struct DeferredCreateOutput {
    pub(crate) info: SessionInfo,
    pub(crate) binding: PendingCreateBinding,
    pub(crate) warnings: Vec<SessionWarning>,
    // Keep admission through the enclosing retained transaction's finalization.
    pub(crate) _create_admission: Arc<Mutex<OwnedCreateAdmission>>,
    pub(crate) effective_continuation: bool,
}

#[derive(Clone)]
pub(crate) struct InitialLastPrompt {
    pub(crate) text: Option<String>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    effective_profile: Option<String>,
}

impl InitialLastPrompt {
    pub(crate) fn from_persisted(
        row: &crate::config::sessions_persistence::PersistedSession,
    ) -> Self {
        Self {
            text: row.last_prompt.clone(),
            agent_id: row.agent_id.clone(),
            requested_profile: row.requested_profile.clone(),
            effective_profile: row.effective_profile.clone(),
        }
    }

    pub(crate) fn compatible(&self, row: &crate::session::session::Session) -> bool {
        self.agent_id.is_some()
            && self.agent_id == row.agent_id
            && self.requested_profile == row.requested_profile
            && self.effective_profile == row.effective_profile
    }
}

pub(crate) enum CreateCompletion {
    Retained,
    Finalized(SessionInfo),
    Deferred(DeferredCreateOutput),
}

impl CreateCompletion {
    pub(crate) async fn into_finalized(self) -> Result<SessionInfo, String> {
        match self {
            Self::Retained => std::future::pending().await,
            Self::Finalized(info) => Ok(info),
            Self::Deferred(_) => Err("create unexpectedly deferred finalization".to_string()),
        }
    }
}

pub(crate) struct CreateBodyContext<'a, R: tauri::Runtime> {
    pub(crate) app: &'a AppHandle<R>,
    pub(crate) session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
    pub(crate) pty_mgr: &'a Arc<Mutex<PtyManager>>,
    pub(crate) shell: &'a str,
    pub(crate) cwd: &'a str,
    pub(crate) resolved_spawn: Option<&'a AgentSpawnCommand>,
    pub(crate) resolved_agent_host_shell: Option<&'a ResolvedAgentHostShell>,
    pub(crate) viewport: Option<PtyViewport>,
    pub(crate) skip_tooling_save: bool,
    pub(crate) inline_transaction: Option<&'a SelectionTransaction<R>>,
    pub(crate) inline_cause: Option<SelectionCause>,
    pub(crate) defer_inline_finalization: bool,
    pub(crate) pending_start_fresh: Option<bool>,
    pub(crate) initial_last_prompt: Option<&'a InitialLastPrompt>,
    pub(crate) owned_replacement: Option<&'a OwnedReplacementContinuation>,
    pub(crate) lifecycle_domain: Option<&'a LifecycleEntryAuthority>,
    pub(crate) create_owner: &'a Arc<Mutex<OwnedCreateAdmission>>,
    pub(crate) create_manager: &'a SessionManager,
    pub(crate) coordinator: &'a SelectionCoordinator,
    pub(crate) create_ticket: &'a Arc<Mutex<Option<CreateFinalizationTicket>>>,
    pub(crate) marker_ticket: &'a Mutex<Option<CoordinatorCreateTicket<R>>>,
    pub(crate) inline_pending_for_body: &'a Mutex<Option<PendingCreateBinding>>,
}

pub(crate) struct PendingLaunch {
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) agent_kind: Option<CodingAgentKind>,
    pub(crate) is_coordinator: bool,
    pub(crate) is_root_agent: bool,
    pub(crate) fresh_forced_by_mirror: bool,
    pub(crate) skip_auto_resume: bool,
    pub(crate) pending_binding: PendingCreateBinding,
    pub(crate) shell_args: Vec<String>,
    pub(crate) container_path_context: Option<ContainerPathContext>,
    pub(crate) container_repos: Option<RepoMountResolution>,
    pub(crate) context_target: Option<ManagedContextTarget>,
    pub(crate) full_cmd: String,
    pub(crate) configured_identity: bool,
    pub(crate) session: Session,
    pub(crate) spawn_mark: SpawnMark,
    pub(crate) mgr: SessionManager,
    pub(crate) resource_permit: Option<AgentLaunchPermit>,
    pub(crate) resource_monitor: Arc<ResourceMonitorState>,
}

pub(crate) struct ContextReady {
    pub(crate) effective_continuation: bool,
    pub(crate) spawn_cwd: String,
    pub(crate) container_credential: Option<ContainerCredentialPlan>,
    pub(crate) injected_claude_config_dir: Option<String>,
    pub(crate) session_env_warnings: Vec<ContainerEnvWarning>,
    pub(crate) pending: PendingLaunch,
}

pub(crate) struct PublicationInput {
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) agent_kind: Option<CodingAgentKind>,
    pub(crate) is_coordinator: bool,
    pub(crate) is_root_agent: bool,
    pub(crate) fresh_forced_by_mirror: bool,
    pub(crate) skip_auto_resume: bool,
    pub(crate) configured_identity: bool,
    pub(crate) effective_continuation: bool,
    pub(crate) pending_binding: PendingCreateBinding,
    pub(crate) session_env_warnings: Vec<ContainerEnvWarning>,
    pub(crate) session: Session,
    pub(crate) mgr: SessionManager,
    pub(crate) resource_permit: Option<AgentLaunchPermit>,
    pub(crate) resource_monitor: Arc<ResourceMonitorState>,
}
