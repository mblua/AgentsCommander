use crate::session::session::{Session, SessionStatus};
use crate::pty::backend::SessionBackendKind;
use crate::session::selection::payload::SessionSelection;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Clone)]
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct CarrierSnapshot {
    pub rows: Vec<Session>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub versions: HashMap<Uuid, u64>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub pending_ids: HashSet<Uuid>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub order: Vec<Uuid>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub selection: SessionSelection,
}

impl CarrierSnapshot {
    pub(crate) fn is_local_interactive_carrier(row: &Session) -> bool {
        row.backend_kind == SessionBackendKind::LocalProcess
            && !row.is_root_agent
            && !row.is_coordinator
            && !row.was_detached
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CarrierPhysicalIdentity(crate::path_identity::VerifiedPathIdentity);

impl From<crate::path_identity::VerifiedPathIdentity> for CarrierPhysicalIdentity {
    fn from(value: crate::path_identity::VerifiedPathIdentity) -> Self {
        Self(value)
    }
}

impl CarrierPhysicalIdentity {
    pub(crate) fn canonical_path(&self) -> &std::path::Path {
        &self.0.canonical_path
    }

    pub(crate) fn is_dir(&self) -> bool {
        self.0.metadata.is_dir
    }

    pub(crate) fn same_object(&self, other: &Self) -> bool {
        self.0.object_id == other.0.object_id
    }

    pub(crate) fn matches_route_observations(
        &self,
        cwd: Option<&crate::path_identity::VerifiedPathIdentity>,
        replica: Option<&crate::path_identity::VerifiedPathIdentity>,
        expected_replica: Option<&Self>,
    ) -> bool {
        cwd.is_some_and(|cwd| cwd.object_id == self.0.object_id)
            && match (replica, expected_replica) {
                (Some(actual), Some(expected)) => actual.object_id == expected.0.object_id,
                (None, None) => true,
                _ => false,
            }
    }
}

pub(crate) fn stable_carrier_equal(a: &Session, b: &Session) -> bool {
    a.id == b.id
        && a.created_at == b.created_at
        && a.token == b.token
        && a.name == b.name
        && a.working_directory == b.working_directory
        && a.shell == b.shell
        && a.shell_args == b.shell_args
        && a.backend_kind == b.backend_kind
        && a.effective_shell_args == b.effective_shell_args
        && a.agent_id == b.agent_id
        && a.agent_label == b.agent_label
        && a.git_repos.len() == b.git_repos.len()
        && a.git_repos
            .iter()
            .zip(&b.git_repos)
            .all(|(a, b)| a.label == b.label && a.source_path == b.source_path)
        && a.is_coordinator == b.is_coordinator
        && a.is_root_agent == b.is_root_agent
        && a.agent_kind == b.agent_kind
        && a.requested_profile == b.requested_profile
        && a.effective_profile == b.effective_profile
        && a.profile_fallback_chain == b.profile_fallback_chain
        && a.profile_fallback_applied == b.profile_fallback_applied
        && a.match_tier == b.match_tier
        && a.original_profile_letter == b.original_profile_letter
        && a.effective_codex_home == b.effective_codex_home
        && a.resolved_claude_projects_dir == b.resolved_claude_projects_dir
        && a.profile_content_hash == b.profile_content_hash
        && a.trusted_configured_spawn == b.trusted_configured_spawn
        && a.telegram_bot_id == b.telegram_bot_id
        && a.was_detached == b.was_detached
        && a.start_fresh_on_restore == b.start_fresh_on_restore
        && a.communication == b.communication
        && matches!(a.status, SessionStatus::Active) == matches!(b.status, SessionStatus::Active)
        && match (&a.status, &b.status) {
            (SessionStatus::Exited(a), SessionStatus::Exited(b)) => a == b,
            (SessionStatus::Exited(_), _) | (_, SessionStatus::Exited(_)) => false,
            _ => true,
        }
}
