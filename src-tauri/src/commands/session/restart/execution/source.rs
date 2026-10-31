use super::{RestartPreparation, RestartSource};
use crate::config::sessions_persistence::CreationGateEnforcement;
use crate::session::selection::{RestartJobRequest, TrustedRestartIntent};
use crate::session::session::{SessionRepo, SessionStatus};

pub(crate) struct RestartContinuation {
    pub(crate) uuid: uuid::Uuid,
    pub(crate) name: String,
    pub(crate) git_repos: Vec<SessionRepo>,
    pub(crate) is_root_agent: bool,
    pub(crate) telegram_bot_id: Option<String>,
    pub(crate) old_status: SessionStatus,
    pub(crate) old_selected: bool,
    pub(crate) activate_after: bool,
    pub(crate) intent: TrustedRestartIntent,
    pub(crate) enforcement: CreationGateEnforcement,
}

impl RestartSource {
    pub(crate) fn into_preparation(
        self,
        request: RestartJobRequest,
    ) -> (RestartPreparation, RestartContinuation) {
        let RestartJobRequest {
            session_id: uuid,
            agent_id,
            requested_profile,
            skip_auto_resume,
            activate_after,
            intent,
            communication_override,
            enforcement,
        } = request;

        let RestartSource {
            shell,
            shell_args,
            cwd,
            name,
            stored_agent_id,
            stored_agent_label,
            git_repos,
            is_root_agent,
            telegram_bot_id,
            stored_requested_profile,
            stored_start_fresh,
            stored_communication,
            old_status,
            old_selected,
        } = self;
        let input = RestartPreparation {
            shell,
            shell_args,
            cwd,
            stored_agent_id,
            stored_agent_label,
            is_root_agent,
            stored_requested_profile,
            stored_start_fresh,
            stored_communication,
            agent_id,
            requested_profile,
            skip_auto_resume,
            communication_override,
            enforcement,
        };
        let continuation = RestartContinuation {
            uuid,
            name,
            git_repos,
            is_root_agent,
            telegram_bot_id,
            old_status,
            old_selected,
            activate_after,
            intent,
            enforcement,
        };
        (input, continuation)
    }
}
