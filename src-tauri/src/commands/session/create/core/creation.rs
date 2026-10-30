use crate::commands::session::create::contracts::CreateSelectionIntent;
use crate::pty::backend::SessionBackendKind;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionRepo;
use std::sync::Arc;

pub(crate) struct CreateInput {
    pub(crate) session_label: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) session_name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) git_repos: Vec<SessionRepo>,
    pub(crate) skip_auto_resume: bool,
    pub(crate) pending_communication: Option<SessionCommunication>,
    pub(crate) enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
    pub(crate) coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
    pub(crate) selection_intent: CreateSelectionIntent,
    pub(crate) create_target_replica_identity: Option<crate::path_identity::VerifiedPathIdentity>,
}
pub(crate) struct LaunchState {
    pub(crate) is_coordinator: bool,
    pub(crate) is_root_agent: bool,
    pub(crate) fresh_forced_by_mirror: bool,
    pub(crate) agent_kind: Option<CodingAgentKind>,
    pub(crate) resource_monitor: Arc<ResourceMonitorState>,
    pub(crate) resource_permit: Option<crate::resource_monitor::AgentLaunchPermit>,
    pub(crate) mgr: SessionManager,
    pub(crate) backend_kind: SessionBackendKind,
    pub(crate) container_path_context:
        Option<crate::commands::session::resume_probe::ContainerPathContext>,
    pub(crate) container_repos: Option<crate::pty::container_repos::RepoMountResolution>,
}
pub(crate) struct PendingState {
    pub(crate) is_coordinator: bool,
    pub(crate) is_root_agent: bool,
    pub(crate) fresh_forced_by_mirror: bool,
    pub(crate) agent_kind: Option<CodingAgentKind>,
    pub(crate) resource_monitor: Arc<ResourceMonitorState>,
    pub(crate) resource_permit: Option<crate::resource_monitor::AgentLaunchPermit>,
    pub(crate) mgr: SessionManager,
    pub(crate) container_path_context:
        Option<crate::commands::session::resume_probe::ContainerPathContext>,
    pub(crate) container_repos: Option<crate::pty::container_repos::RepoMountResolution>,
    pub(crate) session: crate::session::session::Session,
    pub(crate) pending_binding: crate::session::manager::PendingCreateBinding,
    pub(crate) spawn_mark: crate::pty::manager::SpawnMark,
}
pub(crate) struct MetadataInput {
    pub(crate) session_label: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) session_name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) skip_auto_resume: bool,
    pub(crate) pending_communication: Option<SessionCommunication>,
    pub(crate) enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
    pub(crate) selection_intent: CreateSelectionIntent,
    pub(crate) create_target_replica_identity: Option<crate::path_identity::VerifiedPathIdentity>,
}

pub(crate) fn context_target(
    agent_kind: Option<CodingAgentKind>,
) -> Option<crate::config::session_context::ManagedContextTarget> {
    match agent_kind {
        Some(CodingAgentKind::Claude) => {
            Some(crate::config::session_context::ManagedContextTarget::Claude)
        }
        Some(CodingAgentKind::Codex) => {
            Some(crate::config::session_context::ManagedContextTarget::Codex)
        }
        Some(CodingAgentKind::Antigravity) => {
            Some(crate::config::session_context::ManagedContextTarget::Antigravity)
        }
        Some(CodingAgentKind::Pi) => Some(crate::config::session_context::ManagedContextTarget::Pi),
        // #1873 - Muse has no managed context target; a configured agent still
        // resolves its instructions filename (AGENTS.md) via `agent_command`.
        Some(CodingAgentKind::Muse) => None,
        None => None,
    }
}
