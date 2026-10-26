use super::super::session::{Session, SessionInfo, SessionRepo, SessionStatus};
use super::binding::binding_matches;
use super::carrier::{acquire_carrier_mutation_permit, CarrierMutationPermit};
use super::{
    ManagerAggregateSnapshot, PendingCreateBinding, SessionManager, SessionManagerState,
    UniqueLiveTokenError,
};
use crate::errors::AppError;
use crate::pty::backend::SessionBackendKind;
use crate::session::selection::payload::SessionSelection;
use uuid::Uuid;

#[derive(Clone, PartialEq)]
pub(crate) struct TerminalSnapshotRequesterFact {
    pub id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub working_directory: String,
    pub backend_kind: SessionBackendKind,
    pub is_coordinator: bool,
    pub is_root_agent: bool,
}

impl std::fmt::Debug for TerminalSnapshotRequesterFact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotRequesterFact")
            .field("working_directory_bytes", &self.working_directory.len())
            .field("backend_kind", &self.backend_kind)
            .field("is_coordinator", &self.is_coordinator)
            .field("is_root_agent", &self.is_root_agent)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct TerminalSnapshotSessionFact {
    pub id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub name: String,
    pub status: SessionStatus,
    pub working_directory: String,
    pub backend_kind: SessionBackendKind,
}

impl std::fmt::Debug for TerminalSnapshotSessionFact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotSessionFact")
            .field("name_bytes", &self.name.len())
            .field("status", &self.status)
            .field("working_directory_bytes", &self.working_directory.len())
            .field("backend_kind", &self.backend_kind)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotFactsError {
    TooMany,
}

pub(super) const TERMINAL_SNAPSHOT_MAX_ROWS: usize = 4_096;

pub(super) const TERMINAL_SNAPSHOT_MAX_CWD_BYTES: usize = 32_768;

pub(super) const TERMINAL_SNAPSHOT_MAX_NAME_BYTES: usize = 1_024;

pub(super) const TERMINAL_SNAPSHOT_MAX_AGGREGATE_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn requester_fact(session: &Session) -> Option<TerminalSnapshotRequesterFact> {
    if session.working_directory.len() > TERMINAL_SNAPSHOT_MAX_CWD_BYTES {
        return None;
    }
    Some(TerminalSnapshotRequesterFact {
        id: session.id,
        created_at: session.created_at,
        working_directory: session.working_directory.clone(),
        backend_kind: session.backend_kind,
        is_coordinator: session.is_coordinator,
        is_root_agent: session.is_root_agent,
    })
}

pub(super) fn snapshot_session_fact_by_id(
    state: &SessionManagerState,
    id: Uuid,
) -> Option<TerminalSnapshotSessionFact> {
    state
        .sessions
        .get(&id)
        .filter(|session| !state.pending_create.contains_key(&session.id))
        .filter(|session| {
            session.working_directory.len() <= TERMINAL_SNAPSHOT_MAX_CWD_BYTES
                && session.name.len() <= TERMINAL_SNAPSHOT_MAX_NAME_BYTES
        })
        .map(|session| TerminalSnapshotSessionFact {
            id: session.id,
            created_at: session.created_at,
            name: session.name.clone(),
            status: session.status.clone(),
            working_directory: session.working_directory.clone(),
            backend_kind: session.backend_kind,
        })
}

impl SessionManager {
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let state = self.state.read().await;
        state
            .order
            .iter()
            .filter(|id| !state.pending_create.contains_key(id))
            .filter_map(|id| state.sessions.get(id).map(SessionInfo::from))
            .collect()
    }

    /// Deletion-only pending-inclusive working-directory snapshot (#1063, plan
    /// sections 5.4/6.3/11.32). The caller has already acquired the outer
    /// `Arc<tokio::sync::RwLock<SessionManager>>::blocking_read`; this method takes
    /// exactly one inner `SessionManagerState::blocking_read`, walks the stable
    /// `order`, and returns cloned working directories for every non-`Exited` row.
    ///
    /// Unlike every public read (e.g. `list_sessions`), it deliberately does NOT
    /// filter `pending_create`: a pending create whose workdir is reserved under a
    /// directory being deleted must still block a reversible delete. It returns no
    /// id, name, status, pending flag, or `SessionInfo` and never crosses IPC. It
    /// must have exactly one production reference (Agent Matrix deletion) and must
    /// never become a second manager lock hierarchy or a selection owner.
    pub(crate) fn live_working_directories_for_deletion_blocking(&self) -> Vec<String> {
        let state = self.state.blocking_read();
        state
            .order
            .iter()
            .filter_map(|id| state.sessions.get(id))
            .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
            .map(|session| session.working_directory.clone())
            .collect()
    }

    pub async fn set_is_root_agent(&self, id: Uuid, value: bool) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_is_root_agent_with_carrier_permit(&carrier_permit, id, value)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_is_root_agent_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        value: bool,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if !state.pending_create.contains_key(&id) {
            if let Some(session) = state.sessions.get_mut(&id) {
                session.is_root_agent = value;
            }
        }
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn set_pending_is_root_agent(
        &self,
        binding: PendingCreateBinding,
        value: bool,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_is_root_agent_with_carrier_permit(&carrier_permit, binding, value)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_is_root_agent_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        value: bool,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.is_root_agent = value;
        })
        .await
    }

    pub async fn get_session(&self, id: Uuid) -> Option<Session> {
        let state = self.state.read().await;
        (!state.pending_create.contains_key(&id))
            .then(|| state.sessions.get(&id).cloned())
            .flatten()
    }

    pub(crate) async fn get_pending_session(
        &self,
        binding: PendingCreateBinding,
    ) -> Option<Session> {
        let state = self.state.read().await;
        binding_matches(&state, binding)
            .then(|| state.sessions.get(&binding.session_id).cloned())?
    }

    pub(in crate::session) async fn pending_create_bindings(&self) -> Vec<PendingCreateBinding> {
        let state = self.state.read().await;
        state
            .order
            .iter()
            .filter_map(|session_id| {
                state
                    .pending_create
                    .get(session_id)
                    .map(|nonce| PendingCreateBinding::new(*session_id, *nonce))
            })
            .collect()
    }

    pub async fn get_shell(&self, id: Uuid) -> Option<String> {
        self.get_session(id).await.map(|s| s.shell)
    }

    /// Return the working_directory iff the session exists AND is a coordinator.
    /// Cheap (no full Session clone) for the user-message hot path. (#552)
    pub async fn coordinator_cwd(&self, id: Uuid) -> Option<String> {
        let state = self.state.read().await;
        state
            .sessions
            .get(&id)
            .filter(|_| !state.pending_create.contains_key(&id))
            .filter(|s| s.is_coordinator)
            .map(|s| s.working_directory.clone())
    }

    /// (#552) AGENT-OWNED sessions only, paired with their auto-close team key
    /// `<project>:<wg>` (the agent FQN minus the trailing `/agent`). Ad-hoc user
    /// shells (no agent_id, not a coordinator) are excluded so they are never
    /// auto-closed. Sessions whose cwd does not yield a `/`-bearing FQN are skipped.
    pub async fn agent_team_members(&self) -> Vec<(Uuid, String)> {
        let state = self.state.read().await;
        state
            .sessions
            .values()
            .filter(|s| !state.pending_create.contains_key(&s.id))
            .filter(|s| s.is_coordinator || s.agent_id.is_some())
            .filter_map(|s| {
                // agent_fqn_from_path returns String (teams.rs:80), not Option,
                // so no `?` on it. The `?` sits on rsplit_once, which is None only
                // for a `/`-less FQN (non-team fallback) -> skipped. The
                // is_coordinator || agent_id guard above is the real scope gate.
                // `s.id` is already a Uuid (Copy), so no parse is needed.
                let fqn = crate::config::teams::agent_fqn_from_path(&s.working_directory);
                let team_key = fqn
                    .rsplit_once('/')
                    .map(|(team, _agent)| team.to_string())?;
                Some((s.id, team_key))
            })
            .collect()
    }

    /// (#552 auto-closed badge) team key `<project>:<wg>` -> (coordinator FQN,
    /// coordinator working_directory) for every coordinator session record. The
    /// auto-close task snapshots this BEFORE destroying so it can set the
    /// "auto-closed" marker on the correct coordinator row: the FQN keys the
    /// store, the cwd is the event `replicaPath`. One coordinator per team; on
    /// the unlikely duplicate, last writer wins.
    pub async fn coordinator_refs_by_team(
        &self,
    ) -> std::collections::HashMap<String, (String, String)> {
        let state = self.state.read().await;
        let mut out: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new();
        for s in state
            .sessions
            .values()
            .filter(|s| !state.pending_create.contains_key(&s.id))
        {
            if !s.is_coordinator {
                continue;
            }
            let fqn = crate::config::teams::agent_fqn_from_path(&s.working_directory);
            if let Some((team, _agent)) = fqn.rsplit_once('/') {
                out.insert(team.to_string(), (fqn.clone(), s.working_directory.clone()));
            }
        }
        out
    }

    /// (#589) team key `<project>:<wg>` -> the coordinator session's `Uuid`, over
    /// the SAME coordinator records that `coordinator_refs_by_team` keys. The
    /// auto-close task uses this to mark a coordinator row AUTO-CLOSED only when
    /// the coordinator's OWN session was the one destroyed, not when any sibling
    /// member was reaped while the coordinator survived. One coordinator per team;
    /// on the unlikely duplicate, last writer wins (mirrors `coordinator_refs_by_team`).
    pub async fn coordinator_ids_by_team(&self) -> std::collections::HashMap<String, Uuid> {
        let state = self.state.read().await;
        let mut out: std::collections::HashMap<String, Uuid> = std::collections::HashMap::new();
        for s in state
            .sessions
            .values()
            .filter(|s| !state.pending_create.contains_key(&s.id))
        {
            if !s.is_coordinator {
                continue;
            }
            let fqn = crate::config::teams::agent_fqn_from_path(&s.working_directory);
            if let Some((team, _agent)) = fqn.rsplit_once('/') {
                out.insert(team.to_string(), s.id);
            }
        }
        out
    }

    /// Per-session view for the `GitWatcher` fan-out. Returns (session_id, repos, gen).
    /// The generation snapshot lets the watcher call `set_git_repos_if_gen` for its
    /// write, skipping the write+emit if a refresh landed during detection.
    pub async fn get_sessions_repos(&self) -> Vec<(Uuid, Vec<SessionRepo>, u64)> {
        let state = self.state.read().await;
        state
            .sessions
            .iter()
            .filter(|(id, _)| !state.pending_create.contains_key(id))
            .map(|(id, s)| (*id, s.git_repos.clone(), s.git_repos_gen))
            .collect()
    }

    /// (session_id, working_directory) view for callers that only need the CWD
    /// (e.g. mailbox outbox scanning, agent-name resolution).
    pub async fn get_sessions_working_dirs(&self) -> Vec<(Uuid, String)> {
        let state = self.state.read().await;
        state
            .sessions
            .iter()
            .filter(|(id, _)| !state.pending_create.contains_key(id))
            .map(|(id, s)| (*id, s.working_directory.clone()))
            .collect()
    }

    /// Find a session by its display name. Returns its UUID if found.
    pub async fn find_by_name(&self, name: &str) -> Option<Uuid> {
        let state = self.state.read().await;
        state
            .sessions
            .iter()
            .filter(|(id, _)| !state.pending_create.contains_key(id))
            .find(|(_, s)| s.name == name)
            .map(|(id, _)| *id)
    }

    /// Find a session by its authentication token. Linear scan is fine for 10-20 sessions.
    pub async fn find_by_token(&self, token: Uuid) -> Option<SessionInfo> {
        let state = self.state.read().await;
        state
            .sessions
            .values()
            .filter(|s| !state.pending_create.contains_key(&s.id))
            .find(|s| s.token == token)
            .map(SessionInfo::from)
    }

    /// Privileged lookup requiring exactly one non-pending, non-exited owner.
    pub async fn find_unique_live_by_token(
        &self,
        token: Uuid,
    ) -> Result<SessionInfo, UniqueLiveTokenError> {
        let state = self.state.read().await;
        let mut matches = state
            .sessions
            .values()
            .filter(|session| !state.pending_create.contains_key(&session.id))
            .filter(|session| session.token == token)
            .filter(|session| !matches!(session.status, SessionStatus::Exited(_)));
        let first = matches.next().ok_or(UniqueLiveTokenError::NotFound)?;
        if matches.next().is_some() {
            return Err(UniqueLiveTokenError::Ambiguous);
        }
        Ok(SessionInfo::from(first))
    }

    /// Secret-free projection for the terminal snapshot requester boundary.
    /// No `SessionInfo`, token, shell, prompt, task file, or repository state
    /// leaves the manager guard.
    pub(crate) async fn find_unique_live_snapshot_requester_by_token(
        &self,
        token: Uuid,
    ) -> Result<TerminalSnapshotRequesterFact, UniqueLiveTokenError> {
        let state = self.state.read().await;
        let mut matches = state
            .sessions
            .values()
            .filter(|session| !state.pending_create.contains_key(&session.id))
            .filter(|session| session.token == token)
            .filter(|session| !matches!(session.status, SessionStatus::Exited(_)));
        let first = matches.next().ok_or(UniqueLiveTokenError::NotFound)?;
        if matches.next().is_some() {
            return Err(UniqueLiveTokenError::Ambiguous);
        }
        requester_fact(first).ok_or(UniqueLiveTokenError::NotFound)
    }

    pub(crate) async fn live_snapshot_requester_by_id(
        &self,
        id: Uuid,
    ) -> Option<TerminalSnapshotRequesterFact> {
        let state = self.state.read().await;
        state
            .sessions
            .get(&id)
            .filter(|session| !state.pending_create.contains_key(&session.id))
            .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
            .and_then(requester_fact)
    }

    /// Blocking projection used only by the dedicated host snapshot finalizer.
    pub(crate) fn find_unique_live_snapshot_requester_by_token_blocking(
        &self,
        token: Uuid,
    ) -> Result<TerminalSnapshotRequesterFact, UniqueLiveTokenError> {
        let state = self.state.blocking_read();
        let mut matches = state
            .sessions
            .values()
            .filter(|session| !state.pending_create.contains_key(&session.id))
            .filter(|session| session.token == token)
            .filter(|session| !matches!(session.status, SessionStatus::Exited(_)));
        let first = matches.next().ok_or(UniqueLiveTokenError::NotFound)?;
        if matches.next().is_some() {
            return Err(UniqueLiveTokenError::Ambiguous);
        }
        requester_fact(first).ok_or(UniqueLiveTokenError::NotFound)
    }

    /// Blocking projection used only by the dedicated host snapshot finalizer.
    pub(crate) fn live_snapshot_requester_by_id_blocking(
        &self,
        id: Uuid,
    ) -> Option<TerminalSnapshotRequesterFact> {
        let state = self.state.blocking_read();
        state
            .sessions
            .get(&id)
            .filter(|session| !state.pending_create.contains_key(&session.id))
            .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
            .and_then(requester_fact)
    }

    /// One capped, typed selection boundary for terminal snapshots. This takes
    /// one manager guard and performs no filesystem work.
    pub(crate) async fn terminal_snapshot_session_facts(
        &self,
    ) -> Result<Vec<TerminalSnapshotSessionFact>, TerminalSnapshotFactsError> {
        let state = self.state.read().await;
        let row_count = state
            .sessions
            .values()
            .filter(|session| !state.pending_create.contains_key(&session.id))
            .count();
        if row_count > TERMINAL_SNAPSHOT_MAX_ROWS {
            return Err(TerminalSnapshotFactsError::TooMany);
        }
        let mut facts = Vec::new();
        facts
            .try_reserve_exact(row_count)
            .map_err(|_| TerminalSnapshotFactsError::TooMany)?;
        let mut aggregate_bytes = 0usize;
        for session in state.sessions.values() {
            if state.pending_create.contains_key(&session.id) {
                continue;
            }
            if session.working_directory.len() > TERMINAL_SNAPSHOT_MAX_CWD_BYTES
                || session.name.len() > TERMINAL_SNAPSHOT_MAX_NAME_BYTES
            {
                return Err(TerminalSnapshotFactsError::TooMany);
            }
            aggregate_bytes = aggregate_bytes
                .checked_add(session.working_directory.len())
                .and_then(|bytes| bytes.checked_add(session.name.len()))
                .filter(|bytes| *bytes <= TERMINAL_SNAPSHOT_MAX_AGGREGATE_BYTES)
                .ok_or(TerminalSnapshotFactsError::TooMany)?;
            facts.push(TerminalSnapshotSessionFact {
                id: session.id,
                created_at: session.created_at,
                name: session.name.clone(),
                status: session.status.clone(),
                working_directory: session.working_directory.clone(),
                backend_kind: session.backend_kind,
            });
        }
        Ok(facts)
    }

    pub(crate) async fn terminal_snapshot_session_fact_by_id(
        &self,
        id: Uuid,
    ) -> Option<TerminalSnapshotSessionFact> {
        let state = self.state.read().await;
        snapshot_session_fact_by_id(&state, id)
    }

    /// Blocking projection used only by the dedicated host snapshot finalizer.
    pub(crate) fn terminal_snapshot_session_fact_by_id_blocking(
        &self,
        id: Uuid,
    ) -> Option<TerminalSnapshotSessionFact> {
        let state = self.state.blocking_read();
        snapshot_session_fact_by_id(&state, id)
    }

    pub(crate) async fn selection_payload(&self) -> SessionSelection {
        self.state.read().await.selection.clone()
    }

    pub(crate) async fn aggregate_snapshot(&self) -> ManagerAggregateSnapshot {
        let state = self.state.read().await;
        let sessions = state
            .order
            .iter()
            .filter(|id| !state.pending_create.contains_key(id))
            .filter_map(|id| state.sessions.get(id).cloned())
            .collect();
        ManagerAggregateSnapshot {
            coding_agent_bindings: state.coding_agent_bindings.clone(),
            binding_seeds: state.binding_seeds.clone(),
            binding_origins: state.binding_origins.clone(),
            failed_binding_recipes: state.failed_binding_recipes.clone(),
            sessions,
            order: state.order.clone(),
            pending_ids: state.pending_create.keys().copied().collect(),
            selection: state.selection.clone(),
        }
    }
}
