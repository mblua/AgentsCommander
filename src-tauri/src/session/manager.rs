use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::profile::CodingAgentKind;
use super::session::{
    Session, SessionCommunication, SessionCommunicationKind, SessionInfo, SessionRepo,
    SessionStatus,
};
use crate::config::settings::WindowGeometry;
use crate::errors::AppError;
use crate::pty::backend::SessionBackendKind;
use crate::session::selection::{
    CommitCapability, DormantRuntimeWitness, LiveRuntimeWitness, SelectionCause, SelectionMode,
    SessionSelection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PendingCreateBinding {
    session_id: Uuid,
    nonce: Uuid,
}

impl PendingCreateBinding {
    pub(super) const fn new(session_id: Uuid, nonce: Uuid) -> Self {
        Self { session_id, nonce }
    }

    pub const fn session_id(self) -> Uuid {
        self.session_id
    }
}

#[derive(Clone)]
pub struct SessionManager {
    state: Arc<RwLock<SessionManagerState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniqueLiveTokenError {
    NotFound,
    Ambiguous,
}

struct SessionManagerState {
    sessions: HashMap<Uuid, Session>,
    order: Vec<Uuid>,
    pending_create: HashMap<Uuid, Uuid>,
    next_number: u32,
    epoch: Uuid,
    revision: u64,
    selection: SessionSelection,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagerAggregateSnapshot {
    pub sessions: Vec<Session>,
    pub order: Vec<Uuid>,
    pub pending_ids: HashSet<Uuid>,
    pub selection: SessionSelection,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CommitDecision {
    Keep,
    Clear,
    Live(LiveRuntimeWitness),
    Dormant(DormantRuntimeWitness),
}

#[derive(Debug, Clone, Copy)]
enum FinalizeMutation {
    Live(PendingCreateBinding, LiveRuntimeWitness),
    Dormant(PendingCreateBinding, DormantRuntimeWitness, i32),
}

#[derive(Debug, Default)]
pub(crate) struct LifecycleMutations {
    removals: Vec<Uuid>,
    mark_exited: Vec<(Uuid, i32)>,
    detached_intent: Vec<(Uuid, bool)>,
    finalizations: Vec<FinalizeMutation>,
}

impl LifecycleMutations {
    pub(crate) fn remove(&mut self, session_id: Uuid) {
        self.removals.push(session_id);
    }

    pub(crate) fn mark_exited(&mut self, session_id: Uuid, exit_code: i32) {
        self.mark_exited.push((session_id, exit_code));
    }

    pub(crate) fn set_detached_intent(&mut self, session_id: Uuid, value: bool) {
        self.detached_intent.push((session_id, value));
    }

    pub(crate) fn finalize_live(
        &mut self,
        binding: PendingCreateBinding,
        witness: LiveRuntimeWitness,
    ) {
        self.finalizations
            .push(FinalizeMutation::Live(binding, witness));
    }

    pub(crate) fn finalize_dormant(
        &mut self,
        binding: PendingCreateBinding,
        witness: DormantRuntimeWitness,
        exit_code: i32,
    ) {
        self.finalizations
            .push(FinalizeMutation::Dormant(binding, witness, exit_code));
    }
}

#[derive(Debug, Default)]
pub(crate) struct CommitResult {
    pub removed_rows: Vec<SessionInfo>,
    pub missing_ids: Vec<Uuid>,
    pub changed_rows: Vec<SessionInfo>,
    pub finalized_rows: Vec<SessionInfo>,
    pub cleared_raise_hand_ids: Vec<Uuid>,
    pub selection: Option<SessionSelection>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        let epoch = Uuid::new_v4();
        Self {
            state: Arc::new(RwLock::new(SessionManagerState {
                sessions: HashMap::new(),
                order: Vec::new(),
                pending_create: HashMap::new(),
                next_number: 1,
                epoch,
                revision: 0,
                selection: SessionSelection::initial(epoch),
            })),
        }
    }

    // Session record is created with the full set of fields up front; splitting
    // into a builder would just defer the same parameter list.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_pending_session(
        &self,
        ticket: &mut crate::session::selection::CreateFinalizationTicket,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<Session, AppError> {
        let id = Uuid::new_v4();
        let binding = ticket.bind(id);
        self.insert_new_pending_session(
            binding,
            shell,
            shell_args,
            working_directory,
            agent_id,
            agent_label,
            git_repos,
            is_coordinator,
            backend_kind,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn create_transaction_pending_session(
        &self,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<(Session, PendingCreateBinding), AppError> {
        let binding = PendingCreateBinding::new(Uuid::new_v4(), Uuid::new_v4());
        let session = self
            .insert_new_pending_session(
                binding,
                shell,
                shell_args,
                working_directory,
                agent_id,
                agent_label,
                git_repos,
                is_coordinator,
                backend_kind,
            )
            .await?;
        Ok((session, binding))
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_new_pending_session(
        &self,
        binding: PendingCreateBinding,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<Session, AppError> {
        let id = binding.session_id;
        let mut state = self.state.write().await;
        let name = format!("Session {}", state.next_number);
        state.next_number = state
            .next_number
            .checked_add(1)
            .ok_or_else(|| AppError::Other("session number overflow".to_string()))?;

        let session = Session {
            id,
            name,
            shell,
            shell_args,
            backend_kind,
            effective_shell_args: None,
            created_at: chrono::Utc::now(),
            working_directory,
            status: SessionStatus::Running,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id,
            agent_label,
            git_repos,
            is_coordinator,
            is_root_agent: false,
            git_repos_gen: 0,
            token: Uuid::new_v4(),
            agent_kind: None,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            resolved_claude_projects_dir: None,
            profile_content_hash: None,
            trusted_configured_spawn: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
        };
        state.sessions.insert(id, session.clone());
        state.order.push(id);
        state.pending_create.insert(id, binding.nonce);

        Ok(session)
    }

    pub async fn rename_session(&self, id: Uuid, name: String) -> Result<(), AppError> {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return Err(AppError::SessionNotFound(id.to_string()));
        }
        let session = state
            .sessions
            .get_mut(&id)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        session.name = name;
        Ok(())
    }

    pub(crate) async fn rename_pending_session(
        &self,
        binding: PendingCreateBinding,
        name: String,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| session.name = name)
            .await
    }

    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let state = self.state.read().await;
        state
            .order
            .iter()
            .filter(|id| !state.pending_create.contains_key(id))
            .filter_map(|id| state.sessions.get(id).map(SessionInfo::from))
            .collect()
    }

    pub async fn set_is_root_agent(&self, id: Uuid, value: bool) {
        let mut state = self.state.write().await;
        if !state.pending_create.contains_key(&id) {
            if let Some(session) = state.sessions.get_mut(&id) {
                session.is_root_agent = value;
            }
        }
    }

    pub(crate) async fn set_pending_is_root_agent(
        &self,
        binding: PendingCreateBinding,
        value: bool,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| {
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

    pub(super) async fn pending_create_bindings(&self) -> Vec<PendingCreateBinding> {
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

    pub async fn mark_idle(&self, id: Uuid) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            log::info!(
                "[session-state] {} '{}': waiting_for_input {} → true",
                &id.to_string()[..8],
                s.name,
                s.waiting_for_input
            );
            s.waiting_for_input = true;
            if matches!(s.status, SessionStatus::Running) {
                s.status = SessionStatus::Idle;
            }
        }
    }

    pub async fn mark_busy(&self, id: Uuid) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            log::info!(
                "[session-state] {} '{}': waiting_for_input {} → false",
                &id.to_string()[..8],
                s.name,
                s.waiting_for_input
            );
            s.waiting_for_input = false;
            if matches!(s.status, SessionStatus::Idle) {
                s.status = SessionStatus::Running;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_pty_input_boundary<'a, F>(
        &self,
        id: Uuid,
        expected_target: &crate::config::teams::VerifiedPtyInputIdentity,
        expected_backend: SessionBackendKind,
        authority_id: Uuid,
        expected_sender: &crate::config::teams::VerifiedPtyInputIdentity,
        authority_backend: SessionBackendKind,
        authority_route: &'a crate::pty::manager::PtyAuthorityRouteProof,
        permit: &'a crate::pty::manager::PtyInputPermit,
        idle_detector: &crate::pty::idle_detector::IdleDetector,
        final_external_check: F,
    ) -> Result<
        crate::pty::manager::PtyRouteWriteGuard<'a>,
        crate::pty::idle_detector::PtyInputBoundaryFailure,
    >
    where
        F: FnOnce() -> bool,
    {
        use crate::pty::idle_detector::PtyInputBoundaryFailure as Failure;

        let (route_guard, was_idle) = {
            let mut state = self.state.write().await;
            if state.pending_create.contains_key(&id)
                || state.pending_create.contains_key(&authority_id)
            {
                return Err(Failure::RouteUnavailable);
            }
            let authority = state
                .sessions
                .get(&authority_id)
                .ok_or(Failure::RouteUnavailable)?;
            if matches!(authority.status, SessionStatus::Exited(_))
                || authority.backend_kind != authority_backend
            {
                return Err(Failure::RouteUnavailable);
            }
            if authority_backend == SessionBackendKind::LocalProcess {
                let live_token_matches = state
                    .sessions
                    .values()
                    .filter(|candidate| !state.pending_create.contains_key(&candidate.id))
                    .filter(|candidate| candidate.token == authority.token)
                    .filter(|candidate| !matches!(candidate.status, SessionStatus::Exited(_)))
                    .count();
                if live_token_matches != 1 {
                    return Err(Failure::RouteUnavailable);
                }
            }
            let authority_cwd = crate::path_identity::verify_directory(std::path::Path::new(
                &authority.working_directory,
            ))
            .map_err(|_| Failure::RouteUnavailable)?;
            let expected_authority_replica = if expected_sender.canonical_fqn
                == crate::config::root_agent::ROOT_AGENT_SENDER
            {
                let root = crate::config::root_agent::verify_live_root_agent_path(
                    std::path::Path::new(&authority.working_directory),
                )
                .map_err(|_| Failure::RouteUnavailable)?;
                if !authority.is_root_agent
                    || !crate::path_identity::same_object(&root, &expected_sender.replica_identity)
                {
                    return Err(Failure::RouteUnavailable);
                }
                None
            } else {
                let sender = crate::config::teams::verify_pty_input_replica_cwd(
                    std::path::Path::new(&authority.working_directory),
                )
                .map_err(|_| Failure::RouteUnavailable)?;
                if authority.is_root_agent
                    || sender.canonical_fqn != expected_sender.canonical_fqn
                    || sender.authority_fingerprint != expected_sender.authority_fingerprint
                    || !crate::path_identity::same_object(
                        &sender.replica_identity,
                        &expected_sender.replica_identity,
                    )
                {
                    return Err(Failure::RouteUnavailable);
                }
                Some(sender.replica_identity)
            };
            let authority_route_guard = authority_route
                .lock_verified(
                    authority_backend,
                    &authority_cwd,
                    expected_authority_replica.as_ref(),
                )
                .map_err(|_| Failure::RouteUnavailable)?;
            let session = state
                .sessions
                .get_mut(&id)
                .ok_or(Failure::RouteUnavailable)?;
            if matches!(session.status, SessionStatus::Exited(_))
                || !session.waiting_for_input
                || session.backend_kind != expected_backend
                || session.pty_submission_agent().is_none()
            {
                return Err(Failure::Busy);
            }
            let cwd_identity = crate::path_identity::verify_directory(std::path::Path::new(
                &session.working_directory,
            ))
            .map_err(|_| Failure::RouteUnavailable)?;
            let current_target = crate::config::teams::verify_pty_input_replica_cwd(
                std::path::Path::new(&session.working_directory),
            )
            .map_err(|_| Failure::RouteUnavailable)?;
            if current_target.canonical_fqn != expected_target.canonical_fqn
                || current_target.authority_fingerprint != expected_target.authority_fingerprint
                || !crate::path_identity::same_object(
                    &current_target.replica_identity,
                    &expected_target.replica_identity,
                )
            {
                return Err(Failure::RouteUnavailable);
            }
            let prepared = idle_detector.prepare_pty_input_boundary(id, || {
                let route = crate::pty::manager::PtyManager::lock_route_for_verified_write(
                    permit,
                    expected_backend,
                    &cwd_identity,
                    &expected_target.replica_identity,
                )?;
                if !final_external_check() {
                    return Err(crate::errors::AppError::PtyError(
                        "pty_authority_changed".to_string(),
                    ));
                }
                Ok(route)
            })?;
            session.waiting_for_input = false;
            if matches!(session.status, SessionStatus::Idle) {
                session.status = SessionStatus::Running;
            }
            let (mut route_guard, was_idle) = prepared;
            route_guard.retain_authority_guard(authority_route_guard);
            (route_guard, was_idle)
        };
        idle_detector.notify_pty_input_busy(id, was_idle);
        Ok(route_guard)
    }

    pub async fn set_last_prompt(&self, id: Uuid, prompt: String) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.last_prompt = Some(prompt);
        }
    }

    pub async fn raise_hand(
        &self,
        id: Uuid,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return None;
        }
        let session = state.sessions.get_mut(&id)?;
        if !session.is_coordinator || matches!(session.status, SessionStatus::Exited(_)) {
            return None;
        }

        if let Some(existing) = session.communication.as_ref() {
            if existing.kind == SessionCommunicationKind::RaiseHand && existing.visible {
                return Some((false, existing.clone()));
            }
        }

        let communication = SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: updated_at.to_rfc3339(),
        };
        session.communication = Some(communication.clone());
        Some((true, communication))
    }

    pub async fn clear_communication_if_kind(
        &self,
        id: Uuid,
        kind: SessionCommunicationKind,
    ) -> bool {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        let Some(session) = state.sessions.get_mut(&id) else {
            return false;
        };
        let should_clear = session
            .communication
            .as_ref()
            .is_some_and(|communication| communication.kind == kind && communication.visible);
        if should_clear {
            session.communication = None;
        }
        should_clear
    }

    /// (#747) Re-apply a persisted raise-hand onto a restored session record.
    /// Unlike `raise_hand`, this deliberately ACCEPTS records in
    /// `SessionStatus::Exited(_)`: the startup defer arm restores dormant
    /// placeholders that must keep their raised hand until real user input
    /// (issue #747 supersedes #676's ephemeral-only rule). Gates: coordinator
    /// records only, visible `RaiseHand` payloads only. Preserves the caller's
    /// `updated_at` (the original raise time) so the indicator's age stays
    /// truthful. Returns true when the communication was applied.
    pub async fn restore_communication(
        &self,
        id: Uuid,
        communication: SessionCommunication,
    ) -> bool {
        if communication.kind != SessionCommunicationKind::RaiseHand || !communication.visible {
            return false;
        }
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        let Some(session) = state.sessions.get_mut(&id) else {
            return false;
        };
        if !session.is_coordinator {
            return false;
        }
        session.communication = Some(communication);
        true
    }

    /// (#630/#631) Stamp the durable resume intent. Used by the restart path and
    /// the restore wake/defer paths.
    pub async fn set_start_fresh_on_restore(&self, id: Uuid, value: bool) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.start_fresh_on_restore = value;
        }
    }

    pub(crate) async fn set_pending_start_fresh_on_restore(
        &self,
        binding: PendingCreateBinding,
        value: bool,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| {
            session.start_fresh_on_restore = value;
        })
        .await
    }

    pub(crate) async fn set_pending_communication(
        &self,
        binding: PendingCreateBinding,
        communication: SessionCommunication,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| {
            session.communication = Some(communication);
        })
        .await
    }

    /// (#630/#631) Re-arm on first real user message: clear the fresh intent.
    /// Returns true ONLY on the `true -> false` transition, so the caller persists
    /// exactly once (not on every subsequent keystroke).
    pub async fn clear_start_fresh_on_restore_if_set(&self, id: Uuid) -> bool {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            if s.start_fresh_on_restore {
                log::info!(
                    "[session-state] {} '{}': start_fresh_on_restore true -> false (re-armed)",
                    &id.to_string()[..8],
                    s.name
                );
                s.start_fresh_on_restore = false;
                return true;
            }
        }
        false
    }

    /// (#756) Stamp the durable fresh intent on an AC-driven clear boundary.
    /// Returns true ONLY on the `false -> true` transition, so the caller
    /// persists exactly once. Missing id -> false. Counterpart of
    /// `clear_start_fresh_on_restore_if_set` (re-arm direction).
    pub async fn set_start_fresh_on_restore_if_unset(&self, id: Uuid) -> bool {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            if !s.start_fresh_on_restore {
                log::info!(
                    "[session-state] {} '{}': start_fresh_on_restore false -> true (AC clear boundary, #756)",
                    &id.to_string()[..8],
                    s.name
                );
                s.start_fresh_on_restore = true;
                return true;
            }
        }
        false
    }

    /// #698: clear BOTH user-input session-state transitions in a SINGLE
    /// critical section. `clear_fresh` gates re-arming the durable resume intent
    /// (`start_fresh_on_restore` true -> false, #871); lowering any visible
    /// raise-hand remains unconditional. Returns
    /// `(cleared_start_fresh, cleared_raise_hand)` so the caller can persist once
    /// and emit the raise-hand clear event only when a hand was actually lowered.
    ///
    /// Doing both under one `sessions.write()` acquisition is the MEDIUM grinch
    /// fix: with the previous two-call shape a concurrent snapshot could observe a
    /// half-applied state (`start_fresh_on_restore` already cleared while the hand
    /// was still raised). Mutating both fields atomically removes that window.
    pub async fn clear_user_input_transitions(&self, id: Uuid, clear_fresh: bool) -> (bool, bool) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return (false, false);
        }
        let Some(s) = state.sessions.get_mut(&id) else {
            return (false, false);
        };

        let cleared_start_fresh = if clear_fresh && s.start_fresh_on_restore {
            log::debug!(
                "[session-state] {} '{}': start_fresh_on_restore true -> false (substantive user message, re-armed)",
                &id.to_string()[..8],
                s.name
            );
            s.start_fresh_on_restore = false;
            true
        } else {
            false
        };

        let cleared_raise_hand = s
            .communication
            .as_ref()
            .is_some_and(|c| c.kind == SessionCommunicationKind::RaiseHand && c.visible);
        if cleared_raise_hand {
            s.communication = None;
        }

        (cleared_start_fresh, cleared_raise_hand)
    }

    /// Set the resolved coding-agent identity. Called once by
    /// `create_session_inner` immediately after `CodingAgentKind::detect`.
    pub async fn set_agent_kind(&self, id: Uuid, kind: Option<CodingAgentKind>) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.agent_kind = kind;
        }
    }

    pub(crate) async fn set_pending_agent_kind(
        &self,
        binding: PendingCreateBinding,
        kind: Option<CodingAgentKind>,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| session.agent_kind = kind)
            .await
    }

    pub(crate) async fn set_pending_trusted_configured_spawn(
        &self,
        binding: PendingCreateBinding,
        trusted: bool,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| {
            session.trusted_configured_spawn = trusted;
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_profile_metadata(
        &self,
        id: Uuid,
        requested_profile: Option<String>,
        effective_profile: Option<String>,
        profile_fallback_chain: Vec<String>,
        profile_fallback_applied: bool,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.requested_profile = requested_profile;
            s.effective_profile = effective_profile;
            s.profile_fallback_chain = profile_fallback_chain;
            s.profile_fallback_applied = profile_fallback_applied;
            s.effective_codex_home = effective_codex_home;
            s.profile_content_hash = profile_content_hash;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_profile_metadata(
        &self,
        binding: PendingCreateBinding,
        requested_profile: Option<String>,
        effective_profile: Option<String>,
        profile_fallback_chain: Vec<String>,
        profile_fallback_applied: bool,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| {
            session.requested_profile = requested_profile;
            session.effective_profile = effective_profile;
            session.profile_fallback_chain = profile_fallback_chain;
            session.profile_fallback_applied = profile_fallback_applied;
            session.effective_codex_home = effective_codex_home;
            session.profile_content_hash = profile_content_hash;
        })
        .await
    }

    pub async fn set_resolved_claude_projects_dir(&self, id: Uuid, path: Option<PathBuf>) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.resolved_claude_projects_dir = path;
        }
    }

    pub(crate) async fn set_pending_resolved_claude_projects_dir(
        &self,
        binding: PendingCreateBinding,
        path: Option<PathBuf>,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| {
            session.resolved_claude_projects_dir = path;
        })
        .await
    }

    /// Persisted Telegram ON/OFF state for the session. Some(bot_id) means the
    /// bridge should be reattached when the session is restored or woken.
    pub async fn set_telegram_bot_id(&self, id: Uuid, bot_id: Option<String>) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.telegram_bot_id = bot_id;
        }
    }

    /// True when the session currently has a persisted Telegram bot assignment.
    /// Used by auto-close; does not expose the bot id.
    pub async fn session_has_telegram_bot(&self, id: Uuid) -> bool {
        let state = self.state.read().await;
        !state.pending_create.contains_key(&id)
            && state
                .sessions
                .get(&id)
                .is_some_and(|s| s.telegram_bot_id.is_some())
    }

    /// Set `was_detached` on the session. Authoritative store for persistence under
    /// Fix A (plan §A3.2). Mutated ONLY by `detach_terminal_inner` (→true) and
    /// `attach_terminal` (→false). See plan §10 rule — the `WindowEvent::Destroyed`
    /// handler must NOT call this.
    pub async fn set_was_detached(&self, id: Uuid, detached: bool) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.was_detached = detached;
        }
    }

    /// Record the detached window's last-known geometry. Called by the frontend on
    /// drag/resize via the `set_detached_geometry` Tauri command. Read at spawn
    /// time by `detach_terminal_inner` (including the Phase 3 restore path).
    pub async fn set_detached_geometry(&self, id: Uuid, geometry: WindowGeometry) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.detached_geometry = Some(geometry);
        }
    }

    /// Register the effective arg vector actually handed to portable-pty
    /// at spawn time. Called by `create_session_inner` immediately before
    /// `pty_mgr.spawn`. Idempotent: callers write the final vec once per
    /// session lifetime. Overwrites on re-call (defensive; not expected in
    /// normal flow).
    pub async fn set_effective_shell_args(&self, id: Uuid, args: Vec<String>) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.effective_shell_args = Some(args);
        }
    }

    pub(crate) async fn set_pending_effective_shell_args(
        &self,
        binding: PendingCreateBinding,
        args: Vec<String>,
    ) -> Result<(), AppError> {
        self.update_pending(binding, |session| {
            session.effective_shell_args = Some(args);
        })
        .await
    }

    /// Overwrite `git_repos` atomically. Bumps `git_repos_gen`. Invariant:
    /// callers preserve insertion order (replica config.json `repos` array order).
    pub async fn set_git_repos(&self, id: Uuid, repos: Vec<SessionRepo>) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.git_repos = repos;
            s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
        }
    }

    /// Compare-and-swap variant for the watcher. Only writes if `expected_gen` still
    /// matches `git_repos_gen`. On mismatch a concurrent refresh has landed; the watcher
    /// discards its stale detection to prevent emit reordering (see §2.1.d / Grinch #14).
    /// Returns true on successful write.
    pub async fn set_git_repos_if_gen(
        &self,
        id: Uuid,
        repos: Vec<SessionRepo>,
        expected_gen: u64,
    ) -> bool {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            if s.git_repos_gen == expected_gen {
                s.git_repos = repos;
                s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
                return true;
            }
        }
        false
    }

    /// Snapshot the current `git_repos_gen` for a session. Used by watchers to capture
    /// generation at the start of a poll so `set_git_repos_if_gen` can detect a race.
    pub async fn get_git_repos_gen(&self, id: Uuid) -> Option<u64> {
        let state = self.state.read().await;
        (!state.pending_create.contains_key(&id))
            .then(|| state.sessions.get(&id).map(|s| s.git_repos_gen))
            .flatten()
    }

    /// Overwrite `is_coordinator`. Use after a team-config refresh.
    pub async fn set_is_coordinator(&self, id: Uuid, is_coordinator: bool) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.is_coordinator = is_coordinator;
        }
    }

    /// Recompute `is_coordinator` for every session using the current team snapshot.
    /// Returns the list of (session_id, new_value) pairs whose flag actually changed,
    /// so callers can emit a single event batch.
    pub async fn refresh_coordinator_flags(
        &self,
        teams: &[crate::config::teams::DiscoveredTeam],
    ) -> Vec<(Uuid, bool)> {
        let mut state = self.state.write().await;
        let pending = state.pending_create.keys().copied().collect::<HashSet<_>>();
        let mut changes = Vec::new();
        for (id, s) in state
            .sessions
            .iter_mut()
            .filter(|(id, _)| !pending.contains(id))
        {
            let new_val = crate::config::teams::is_coordinator_for_cwd(&s.working_directory, teams);
            if s.is_coordinator != new_val {
                s.is_coordinator = new_val;
                changes.push((*id, new_val));
            }
        }
        changes
    }

    /// Replace `git_repos` for sessions whose name matches. Bumps `git_repos_gen` on every
    /// write so an in-flight `GitWatcher::poll` that captured the pre-refresh snapshot
    /// cannot overwrite us (see §2.1.d / Grinch #14).
    /// Returns the list of (session_id, new_repos) pairs where a write actually happened.
    pub async fn refresh_git_repos_for_sessions(
        &self,
        updates: &[(String, Vec<SessionRepo>)],
    ) -> Vec<(Uuid, Vec<SessionRepo>)> {
        let mut state = self.state.write().await;
        let pending = state.pending_create.keys().copied().collect::<HashSet<_>>();
        let mut changed = Vec::new();
        for (name, repos) in updates {
            if let Some((id, s)) = state
                .sessions
                .iter_mut()
                .filter(|(id, _)| !pending.contains(id))
                .find(|(_, s)| &s.name == name)
            {
                if &s.git_repos != repos {
                    s.git_repos = repos.clone();
                    s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
                    changed.push((*id, repos.clone()));
                }
            }
        }
        changed
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
            sessions,
            order: state.order.clone(),
            pending_ids: state.pending_create.keys().copied().collect(),
            selection: state.selection.clone(),
        }
    }

    pub(crate) async fn contains_public_or_pending(&self, id: Uuid) -> bool {
        self.state.read().await.sessions.contains_key(&id)
    }

    async fn update_pending<F>(
        &self,
        binding: PendingCreateBinding,
        update: F,
    ) -> Result<(), AppError>
    where
        F: FnOnce(&mut Session),
    {
        let mut state = self.state.write().await;
        if !binding_matches(&state, binding) {
            return Err(AppError::Other(format!(
                "pending create capability mismatch: {}",
                binding.session_id
            )));
        }
        let session = state
            .sessions
            .get_mut(&binding.session_id)
            .ok_or_else(|| AppError::SessionNotFound(binding.session_id.to_string()))?;
        update(session);
        Ok(())
    }

    pub(super) async fn rollback_pending_create(
        &self,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        let mut state = self.state.write().await;
        if !binding_matches(&state, binding) {
            if !state.sessions.contains_key(&binding.session_id) {
                return Ok(());
            }
            return Err(format!(
                "pending create capability mismatch: {}",
                binding.session_id
            ));
        }
        state.pending_create.remove(&binding.session_id);
        state.sessions.remove(&binding.session_id);
        state.order.retain(|id| *id != binding.session_id);
        Ok(())
    }

    pub(super) async fn insert_transaction_pending_record(
        &self,
        session: Session,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        if session.id != binding.session_id {
            return Err("transaction pending binding targets another session".to_string());
        }
        let mut state = self.state.write().await;
        if state.sessions.contains_key(&session.id) {
            return Err(format!("session already exists: {}", session.id));
        }
        state.next_number = state
            .next_number
            .checked_add(1)
            .ok_or_else(|| "session number overflow".to_string())?;
        state.order.push(session.id);
        state.pending_create.insert(session.id, binding.nonce);
        state.sessions.insert(session.id, session);
        Ok(())
    }

    pub(super) async fn commit_selection_transition(
        &self,
        _capability: &CommitCapability,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        let mut state = self.state.write().await;
        let old_selection = state.selection.clone();
        let old_id = old_selection.id();
        let old_mode = old_selection.mode();

        validate_unique_ids("removal", mutations.removals.iter().copied())?;
        validate_unique_ids(
            "markExited",
            mutations.mark_exited.iter().map(|(id, _)| *id),
        )?;
        validate_unique_ids(
            "detachedIntent",
            mutations.detached_intent.iter().map(|(id, _)| *id),
        )?;
        validate_unique_ids(
            "finalization",
            mutations
                .finalizations
                .iter()
                .map(|mutation| match mutation {
                    FinalizeMutation::Live(binding, _)
                    | FinalizeMutation::Dormant(binding, _, _) => binding.session_id,
                }),
        )?;

        let removals = mutations.removals.iter().copied().collect::<HashSet<_>>();
        let exits = mutations
            .mark_exited
            .iter()
            .map(|(id, _)| *id)
            .collect::<HashSet<_>>();
        let finalizations = mutations
            .finalizations
            .iter()
            .map(|mutation| match mutation {
                FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                    binding.session_id
                }
            })
            .collect::<HashSet<_>>();

        if let Some(overlap) = removals.intersection(&exits).next() {
            return Err(format!(
                "lifecycle mutation remove+markExited conflict: {overlap}"
            ));
        }
        if let Some(overlap) = removals.intersection(&finalizations).next() {
            return Err(format!(
                "lifecycle mutation remove+finalize conflict: {overlap}"
            ));
        }
        if let Some(overlap) = exits.intersection(&finalizations).next() {
            return Err(format!(
                "lifecycle mutation markExited+finalize conflict: {overlap}"
            ));
        }
        for id in removals.iter().chain(exits.iter()) {
            if state.pending_create.contains_key(id) {
                return Err(format!(
                    "public lifecycle mutation cannot address pending create: {id}"
                ));
            }
        }

        for mutation in &mutations.finalizations {
            match *mutation {
                FinalizeMutation::Live(binding, witness) => {
                    if !binding_matches(&state, binding) {
                        return Err(format!(
                            "create finalization capability mismatch: {}",
                            binding.session_id
                        ));
                    }
                    if witness.session_id != binding.session_id
                        || !witness.has_pty
                        || witness.detached
                    {
                        return Err(format!(
                            "live create finalization has invalid runtime witness: {}",
                            binding.session_id
                        ));
                    }
                    let record = state.sessions.get(&binding.session_id).ok_or_else(|| {
                        format!("pending create record missing: {}", binding.session_id)
                    })?;
                    if matches!(record.status, SessionStatus::Exited(_)) {
                        return Err(format!(
                            "live create finalization targets exited record: {}",
                            binding.session_id
                        ));
                    }
                }
                FinalizeMutation::Dormant(binding, witness, _) => {
                    if !binding_matches(&state, binding)
                        || witness.session_id != binding.session_id
                        || witness.detached
                    {
                        return Err(format!(
                            "dormant create finalization has invalid capability/witness: {}",
                            binding.session_id
                        ));
                    }
                }
            }
        }

        let projected_status = |id: Uuid| -> Option<SessionStatus> {
            if removals.contains(&id) {
                return None;
            }
            if let Some(mutation) = mutations
                .finalizations
                .iter()
                .find(|mutation| match mutation {
                    FinalizeMutation::Live(binding, _)
                    | FinalizeMutation::Dormant(binding, _, _) => binding.session_id == id,
                })
            {
                return Some(match mutation {
                    FinalizeMutation::Live(_, _) => SessionStatus::Running,
                    FinalizeMutation::Dormant(_, _, code) => SessionStatus::Exited(*code),
                });
            }
            let current = state.sessions.get(&id)?.status.clone();
            if matches!(current, SessionStatus::Exited(_)) {
                return Some(current);
            }
            mutations
                .mark_exited
                .iter()
                .find(|(candidate, _)| *candidate == id)
                .map(|(_, code)| SessionStatus::Exited(*code))
                .or(Some(current))
        };

        let (target_mode, target_id, target_status) = match decision {
            CommitDecision::Keep => {
                if old_id.is_some_and(|id| {
                    removals.contains(&id)
                        || exits.contains(&id)
                        || mutations
                            .detached_intent
                            .iter()
                            .any(|(candidate, detached)| *candidate == id && *detached)
                }) {
                    return Err(
                        "Keep cannot remove, exit, or detach the selected record".to_string()
                    );
                }
                (old_mode, old_id, old_selection.status().cloned())
            }
            CommitDecision::Clear => (SelectionMode::None, None, None),
            CommitDecision::Live(witness) => {
                let id = witness.session_id;
                if !witness.has_pty || witness.detached {
                    return Err(format!("live target has invalid runtime witness: {id}"));
                }
                if state.pending_create.contains_key(&id) && !finalizations.contains(&id) {
                    return Err(format!("live target is still pending: {id}"));
                }
                if removals.contains(&id) {
                    return Err(format!("live target is also removed: {id}"));
                }
                match projected_status(id) {
                    Some(SessionStatus::Exited(_)) => {
                        return Err(format!("live target is exited: {id}"));
                    }
                    Some(_) => {}
                    None => return Err(format!("live target is missing: {id}")),
                }
                (SelectionMode::Live, Some(id), Some(SessionStatus::Active))
            }
            CommitDecision::Dormant(witness) => {
                let id = witness.session_id;
                if witness.detached {
                    return Err(format!("dormant target is detached: {id}"));
                }
                if state.pending_create.contains_key(&id) && !finalizations.contains(&id) {
                    return Err(format!("dormant target is still pending: {id}"));
                }
                if removals.contains(&id) {
                    return Err(format!("dormant target is also removed: {id}"));
                }
                let status = projected_status(id)
                    .ok_or_else(|| format!("dormant target is missing: {id}"))?;
                if !matches!(status, SessionStatus::Exited(_)) {
                    return Err(format!("dormant target is not exited: {id}"));
                }
                (SelectionMode::Dormant, Some(id), Some(status))
            }
        };

        let selection_changed = target_mode != old_mode
            || target_id != old_id
            || (target_mode == SelectionMode::Dormant
                && target_status.as_ref() != old_selection.status());
        let next_revision = if selection_changed {
            state.revision.checked_add(1).ok_or_else(|| {
                log::error!(
                    "[selection] revision overflow epoch={} oldId={:?} source={}",
                    state.epoch,
                    old_id,
                    cause.source()
                );
                "selection revision overflow".to_string()
            })?
        } else {
            state.revision
        };

        let mut result = CommitResult::default();
        let mut changed_ids = HashSet::new();
        for mutation in mutations.finalizations {
            match mutation {
                FinalizeMutation::Live(binding, _) => {
                    state.pending_create.remove(&binding.session_id);
                    if let Some(record) = state.sessions.get_mut(&binding.session_id) {
                        if matches!(record.status, SessionStatus::Exited(_)) {
                            return Err(format!(
                                "live finalization became exited before commit: {}",
                                binding.session_id
                            ));
                        }
                        record.status = SessionStatus::Running;
                        result.finalized_rows.push(SessionInfo::from(&*record));
                    }
                }
                FinalizeMutation::Dormant(binding, _, exit_code) => {
                    state.pending_create.remove(&binding.session_id);
                    if let Some(record) = state.sessions.get_mut(&binding.session_id) {
                        record.status = SessionStatus::Exited(exit_code);
                        result.finalized_rows.push(SessionInfo::from(&*record));
                    }
                }
            }
        }

        for (id, exit_code) in mutations.mark_exited {
            let Some(record) = state.sessions.get_mut(&id) else {
                result.missing_ids.push(id);
                continue;
            };
            if matches!(record.status, SessionStatus::Exited(_)) {
                continue;
            }
            let cleared_raise_hand = record.communication.as_ref().is_some_and(|communication| {
                communication.kind == SessionCommunicationKind::RaiseHand && communication.visible
            });
            if cleared_raise_hand {
                record.communication = None;
                result.cleared_raise_hand_ids.push(id);
            }
            record.status = SessionStatus::Exited(exit_code);
            changed_ids.insert(id);
        }

        for (id, detached) in mutations.detached_intent {
            let Some(record) = state.sessions.get_mut(&id) else {
                result.missing_ids.push(id);
                continue;
            };
            if record.was_detached != detached {
                record.was_detached = detached;
                changed_ids.insert(id);
            }
        }

        for id in mutations.removals {
            if let Some(record) = state.sessions.remove(&id) {
                state.order.retain(|candidate| *candidate != id);
                state.pending_create.remove(&id);
                result.removed_rows.push(SessionInfo::from(&record));
            } else {
                result.missing_ids.push(id);
            }
        }

        if selection_changed {
            if let Some(previous_id) = old_id {
                let previous_remains_live_target =
                    target_mode == SelectionMode::Live && target_id == Some(previous_id);
                if !previous_remains_live_target {
                    if let Some(previous) = state.sessions.get_mut(&previous_id) {
                        if previous.status == SessionStatus::Active {
                            previous.status = SessionStatus::Running;
                            changed_ids.insert(previous_id);
                        }
                    }
                }
            }

            state.revision = next_revision;
            state.selection = match (target_mode, target_id, target_status) {
                (SelectionMode::None, None, _) => {
                    SessionSelection::none(state.epoch, next_revision, cause)
                }
                (SelectionMode::Live, Some(id), _) => {
                    let target = state
                        .sessions
                        .get_mut(&id)
                        .ok_or_else(|| format!("live target disappeared during commit: {id}"))?;
                    target.status = SessionStatus::Active;
                    changed_ids.insert(id);
                    SessionSelection::live(state.epoch, next_revision, cause, id)
                }
                (SelectionMode::Dormant, Some(id), Some(SessionStatus::Exited(code))) => {
                    let has_pty = match decision {
                        CommitDecision::Dormant(witness) => witness.has_pty,
                        _ => false,
                    };
                    SessionSelection::dormant(state.epoch, next_revision, cause, id, code, has_pty)
                }
                _ => return Err("invalid canonical selection target".to_string()),
            };
            result.selection = Some(state.selection.clone());
        }

        result.changed_rows = state
            .order
            .iter()
            .filter(|id| changed_ids.contains(id) && !state.pending_create.contains_key(id))
            .filter_map(|id| state.sessions.get(id).map(SessionInfo::from))
            .collect();

        log::info!(
            "[selection] epoch={} revision={} oldId={:?} newId={:?} source={} userInitiated={} oldMode={} newMode={} oldStatus={:?} newStatus={:?} targetHasPty={} targetDetached={} result={}",
            state.epoch,
            state.revision,
            old_id,
            state.selection.id(),
            cause.source(),
            cause.user_initiated(),
            old_mode,
            state.selection.mode(),
            old_selection.status(),
            state.selection.status(),
            state.selection.has_pty(),
            state.selection.detached(),
            if selection_changed { "committed" } else { "noop" },
        );

        Ok(result)
    }
}

fn binding_matches(state: &SessionManagerState, binding: PendingCreateBinding) -> bool {
    state.pending_create.get(&binding.session_id).copied() == Some(binding.nonce)
}

fn validate_unique_ids(label: &str, ids: impl IntoIterator<Item = Uuid>) -> Result<(), String> {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(format!("duplicate {label} lifecycle mutation: {id}"));
        }
    }
    Ok(())
}

#[cfg(test)]
impl SessionManager {
    pub async fn set_communication_for_test(&self, id: Uuid, communication: SessionCommunication) {
        if let Some(session) = self.state.write().await.sessions.get_mut(&id) {
            session.communication = Some(communication);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_session(
        &self,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<Session, AppError> {
        let id = Uuid::new_v4();
        let mut state = self.state.write().await;
        let name = format!("Session {}", state.next_number);
        state.next_number = state.next_number.saturating_add(1);
        let mut session = Session {
            id,
            name,
            shell,
            shell_args,
            backend_kind,
            effective_shell_args: None,
            created_at: chrono::Utc::now(),
            working_directory,
            status: SessionStatus::Running,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id,
            agent_label,
            git_repos,
            is_coordinator,
            is_root_agent: false,
            git_repos_gen: 0,
            token: Uuid::new_v4(),
            agent_kind: None,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            resolved_claude_projects_dir: None,
            profile_content_hash: None,
            trusted_configured_spawn: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
        };
        if state.selection.mode() == SelectionMode::None {
            session.status = SessionStatus::Active;
            state.revision += 1;
            state.selection =
                SessionSelection::live(state.epoch, state.revision, SelectionCause::UserSwitch, id);
        }
        state.order.push(id);
        state.sessions.insert(id, session.clone());
        Ok(session)
    }

    pub async fn get_active(&self) -> Option<Uuid> {
        self.state.read().await.selection.id()
    }

    pub async fn switch_session(&self, id: Uuid) -> Result<(), AppError> {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) || !state.sessions.contains_key(&id) {
            return Err(AppError::SessionNotFound(id.to_string()));
        }
        if let Some(old_id) = state.selection.id() {
            if old_id != id {
                if let Some(old) = state.sessions.get_mut(&old_id) {
                    if old.status == SessionStatus::Active {
                        old.status = SessionStatus::Running;
                    }
                }
            }
        }
        if let Some(target) = state.sessions.get_mut(&id) {
            target.status = SessionStatus::Active;
        }
        state.revision += 1;
        state.selection =
            SessionSelection::live(state.epoch, state.revision, SelectionCause::UserSwitch, id);
        Ok(())
    }

    pub async fn set_active_only(&self, id: Uuid) -> Result<(), AppError> {
        let mut state = self.state.write().await;
        let status = state
            .sessions
            .get(&id)
            .filter(|_| !state.pending_create.contains_key(&id))
            .map(|session| session.status.clone())
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        if let Some(old_id) = state.selection.id() {
            if let Some(old) = state.sessions.get_mut(&old_id) {
                if old.status == SessionStatus::Active {
                    old.status = SessionStatus::Running;
                }
            }
        }
        state.revision += 1;
        state.selection = match status {
            SessionStatus::Exited(code) => SessionSelection::dormant(
                state.epoch,
                state.revision,
                SelectionCause::Restore,
                id,
                code,
                false,
            ),
            _ => {
                SessionSelection::live(state.epoch, state.revision, SelectionCause::UserSwitch, id)
            }
        };
        Ok(())
    }

    pub async fn clear_active(&self) {
        let mut state = self.state.write().await;
        if let Some(old_id) = state.selection.id() {
            if let Some(old) = state.sessions.get_mut(&old_id) {
                if old.status == SessionStatus::Active {
                    old.status = SessionStatus::Running;
                }
            }
            state.revision += 1;
            state.selection = SessionSelection::none(
                state.epoch,
                state.revision,
                SelectionCause::BackgroundCleanup,
            );
        }
    }

    pub async fn clear_active_if(&self, id: Uuid) {
        if self.get_active().await == Some(id) {
            self.clear_active().await;
        }
    }

    pub async fn mark_exited(&self, id: Uuid, code: i32) -> bool {
        let mut state = self.state.write().await;
        let Some(session) = state.sessions.get_mut(&id) else {
            return false;
        };
        let cleared = session.communication.as_ref().is_some_and(|communication| {
            communication.kind == SessionCommunicationKind::RaiseHand && communication.visible
        });
        if cleared {
            session.communication = None;
        }
        session.status = SessionStatus::Exited(code);
        cleared
    }

    pub async fn destroy_session(&self, id: Uuid) -> Result<Option<Uuid>, AppError> {
        let mut state = self.state.write().await;
        if state.sessions.remove(&id).is_none() {
            return Err(AppError::SessionNotFound(id.to_string()));
        }
        state.order.retain(|candidate| *candidate != id);
        if state.selection.id() != Some(id) {
            return Ok(None);
        }
        let next = state
            .order
            .iter()
            .copied()
            .find(|candidate| !state.pending_create.contains_key(candidate));
        state.revision += 1;
        if let Some(next_id) = next {
            if let Some(session) = state.sessions.get_mut(&next_id) {
                session.status = SessionStatus::Active;
            }
            state.selection = SessionSelection::live(
                state.epoch,
                state.revision,
                SelectionCause::ManualClose,
                next_id,
            );
        } else {
            state.selection =
                SessionSelection::none(state.epoch, state.revision, SelectionCause::ManualClose);
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::selection::TrustedCreateIntent;

    #[tokio::test]
    async fn set_effective_shell_args_writes_field() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude-mb".to_string(),
                vec!["--dangerously-skip-permissions".to_string()],
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        assert!(session.effective_shell_args.is_none());

        let effective = vec![
            "--dangerously-skip-permissions".to_string(),
            "--continue".to_string(),
        ];
        mgr.set_effective_shell_args(session.id, effective.clone())
            .await;

        let stored = mgr
            .get_session(session.id)
            .await
            .expect("session should still exist");
        assert_eq!(stored.effective_shell_args, Some(effective));
    }

    #[tokio::test]
    async fn set_effective_shell_args_no_op_on_missing_session() {
        let mgr = SessionManager::new();
        let missing = Uuid::new_v4();
        mgr.set_effective_shell_args(missing, vec!["--continue".to_string()])
            .await;
        assert!(mgr.get_session(missing).await.is_none());
    }

    // (#630/#631) Re-arm on first user message: a fresh session clears its intent
    // exactly once (true -> false), and a second call is a no-op (one-shot gate).
    #[tokio::test]
    async fn clear_start_fresh_on_restore_is_one_shot() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude-mb".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        // Constructor default is the resume intent (false).
        assert!(!session.start_fresh_on_restore);

        // Stamp the durable fresh intent (as the restart path would).
        mgr.set_start_fresh_on_restore(session.id, true).await;
        let stamped = mgr
            .get_session(session.id)
            .await
            .expect("session should still exist");
        assert!(stamped.start_fresh_on_restore);

        // First user message clears it and reports the true -> false transition.
        assert!(
            mgr.clear_start_fresh_on_restore_if_set(session.id).await,
            "first clear must report the transition so the caller persists once"
        );
        let cleared = mgr
            .get_session(session.id)
            .await
            .expect("session should still exist");
        assert!(!cleared.start_fresh_on_restore);

        // Second message is a no-op: no transition, so the caller does not re-persist.
        assert!(
            !mgr.clear_start_fresh_on_restore_if_set(session.id).await,
            "second clear must be a no-op (one-shot)"
        );
    }

    #[tokio::test]
    async fn clear_start_fresh_on_restore_no_op_on_missing_session() {
        let mgr = SessionManager::new();
        assert!(
            !mgr.clear_start_fresh_on_restore_if_set(Uuid::new_v4())
                .await
        );
    }

    // (#756) Stamp on an AC-driven clear boundary: transitions exactly once
    // (false -> true), a second call is a no-op, and a missing id reports false.
    #[tokio::test]
    async fn set_start_fresh_on_restore_if_unset_is_one_shot() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude-mb".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        // Constructor default is the resume intent (false).
        assert!(!session.start_fresh_on_restore);

        // First stamp reports the false -> true transition.
        assert!(
            mgr.set_start_fresh_on_restore_if_unset(session.id).await,
            "first stamp must report the transition so the caller persists once"
        );
        let stamped = mgr
            .get_session(session.id)
            .await
            .expect("session should still exist");
        assert!(stamped.start_fresh_on_restore);

        // Second stamp is a no-op: no transition, so the caller does not re-persist.
        assert!(
            !mgr.set_start_fresh_on_restore_if_unset(session.id).await,
            "second stamp must be a no-op (one-shot)"
        );
    }

    #[tokio::test]
    async fn set_start_fresh_on_restore_if_unset_no_op_on_missing_session() {
        let mgr = SessionManager::new();
        assert!(
            !mgr.set_start_fresh_on_restore_if_unset(Uuid::new_v4())
                .await
        );
    }

    #[tokio::test]
    async fn raise_hand_coordinator_running_session_stores_communication() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-28T17:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let raised = mgr.raise_hand(session.id, now).await;

        let Some((changed, communication)) = raised else {
            panic!("coordinator should accept raise-hand");
        };
        assert!(changed);
        assert_eq!(communication.kind, SessionCommunicationKind::RaiseHand);
        assert!(communication.visible);
        assert_eq!(communication.updated_at, now.to_rfc3339());
        let stored = mgr.get_session(session.id).await.unwrap();
        assert_eq!(stored.communication, Some(communication.clone()));
        let infos = mgr.list_sessions().await;
        assert_eq!(infos[0].communication, Some(communication));
    }

    #[tokio::test]
    async fn raise_hand_second_request_returns_existing_without_mutation() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        let first = chrono::DateTime::parse_from_rfc3339("2026-06-28T17:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let second = chrono::DateTime::parse_from_rfc3339("2026-06-28T18:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let (_, first_comm) = mgr.raise_hand(session.id, first).await.unwrap();
        let second_result = mgr.raise_hand(session.id, second).await;

        assert_eq!(second_result, Some((false, first_comm.clone())));
        let stored = mgr.get_session(session.id).await.unwrap();
        assert_eq!(stored.communication, Some(first_comm));
    }

    #[tokio::test]
    async fn raise_hand_non_coordinator_returns_none() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        assert!(mgr
            .raise_hand(session.id, chrono::Utc::now())
            .await
            .is_none());
        assert!(mgr
            .get_session(session.id)
            .await
            .unwrap()
            .communication
            .is_none());
    }

    #[tokio::test]
    async fn raise_hand_exited_coordinator_returns_none() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        assert!(!mgr.mark_exited(session.id, 0).await);

        assert!(mgr
            .raise_hand(session.id, chrono::Utc::now())
            .await
            .is_none());
        assert!(mgr
            .get_session(session.id)
            .await
            .unwrap()
            .communication
            .is_none());
    }

    #[tokio::test]
    async fn raise_hand_clear_removes_visible_raise_hand_once() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.raise_hand(session.id, chrono::Utc::now())
            .await
            .unwrap();

        assert!(
            mgr.clear_communication_if_kind(session.id, SessionCommunicationKind::RaiseHand)
                .await
        );
        assert!(mgr
            .get_session(session.id)
            .await
            .unwrap()
            .communication
            .is_none());
        assert!(
            !mgr.clear_communication_if_kind(session.id, SessionCommunicationKind::RaiseHand)
                .await
        );
        assert!(
            !mgr.clear_communication_if_kind(Uuid::new_v4(), SessionCommunicationKind::RaiseHand)
                .await
        );
    }

    #[tokio::test]
    async fn raise_hand_mark_exited_clears_visible_state() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.raise_hand(session.id, chrono::Utc::now())
            .await
            .unwrap();

        assert!(mgr.mark_exited(session.id, 7).await);
        let stored = mgr.get_session(session.id).await.unwrap();
        assert!(matches!(stored.status, SessionStatus::Exited(7)));
        assert!(stored.communication.is_none());
        let infos = mgr.list_sessions().await;
        assert!(infos[0].communication.is_none());
    }

    #[tokio::test]
    async fn raise_hand_mark_exited_returns_false_without_visible_state() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        assert!(!mgr.mark_exited(session.id, 0).await);
        assert!(mgr
            .get_session(session.id)
            .await
            .unwrap()
            .communication
            .is_none());
    }

    /// (#747) The restore seam accepts an Exited coordinator record (the defer
    /// arm calls it right after `mark_exited`) and keeps the original raise
    /// time so the indicator's age stays truthful.
    #[tokio::test]
    async fn restore_communication_applies_visible_raise_hand_to_dormant_coordinator() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.mark_exited(session.id, 0).await;

        let original_raise_time = "2026-06-30T11:00:00+00:00".to_string();
        let communication = SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: original_raise_time.clone(),
        };

        assert!(
            mgr.restore_communication(session.id, communication.clone())
                .await,
            "dormant coordinator must accept the restored hand"
        );
        let stored = mgr.get_session(session.id).await.unwrap();
        assert!(matches!(stored.status, SessionStatus::Exited(0)));
        let restored = stored.communication.expect("restored hand stored");
        assert_eq!(restored.kind, SessionCommunicationKind::RaiseHand);
        assert!(restored.visible);
        assert_eq!(
            restored.updated_at, original_raise_time,
            "restore must preserve the ORIGINAL raise time, not re-stamp it"
        );
    }

    /// (#747) Restore gates: non-coordinator records, hidden payloads, and
    /// unknown ids are all rejected without mutating state.
    #[tokio::test]
    async fn restore_communication_rejects_non_coordinator_hidden_payload_and_unknown_id() {
        let mgr = SessionManager::new();
        let visible_hand = SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: "2026-06-30T11:00:00+00:00".to_string(),
        };

        // Non-coordinator record: rejected, communication stays None.
        let member = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        assert!(
            !mgr.restore_communication(member.id, visible_hand.clone())
                .await
        );
        assert!(mgr
            .get_session(member.id)
            .await
            .unwrap()
            .communication
            .is_none());

        // Hidden payload on a coordinator: rejected.
        let coordinator = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp2".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        let hidden_hand = SessionCommunication {
            visible: false,
            ..visible_hand.clone()
        };
        assert!(!mgr.restore_communication(coordinator.id, hidden_hand).await);
        assert!(mgr
            .get_session(coordinator.id)
            .await
            .unwrap()
            .communication
            .is_none());

        // Unknown id: rejected.
        assert!(
            !mgr.restore_communication(Uuid::new_v4(), visible_hand)
                .await
        );
    }

    /// #698 MEDIUM fix: `clear_user_input_transitions` lowers a visible raise-hand
    /// and, when gated, re-arms the fresh intent in ONE critical section,
    /// reporting which fields transitioned. A second call is a no-op, and an
    /// unknown id is a no-op.
    #[tokio::test]
    async fn clear_user_input_transitions_clears_both_fields_atomically() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.set_start_fresh_on_restore(session.id, true).await;
        mgr.raise_hand(session.id, chrono::Utc::now())
            .await
            .expect("raise_hand should succeed");

        // Both transitions happen together and are reported.
        let (cleared_start_fresh, cleared_raise_hand) =
            mgr.clear_user_input_transitions(session.id, true).await;
        assert!(cleared_start_fresh);
        assert!(cleared_raise_hand);

        let stored = mgr
            .get_session(session.id)
            .await
            .expect("session should still exist");
        assert!(!stored.start_fresh_on_restore);
        assert!(stored.communication.is_none());

        // Idempotent: a second call transitions nothing.
        assert_eq!(
            mgr.clear_user_input_transitions(session.id, true).await,
            (false, false)
        );

        // Unknown id is a no-op.
        assert_eq!(
            mgr.clear_user_input_transitions(Uuid::new_v4(), true).await,
            (false, false)
        );
    }

    /// #698: the two transitions are independent. `clear_user_input_transitions`
    /// reports each field's own true/false, so a session with only one set re-arms
    /// only that one.
    #[tokio::test]
    async fn clear_user_input_transitions_reports_each_field_independently() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        // Only the fresh intent is set -> (true, false).
        mgr.set_start_fresh_on_restore(session.id, true).await;
        assert_eq!(
            mgr.clear_user_input_transitions(session.id, true).await,
            (true, false)
        );

        // Only a raised hand is set -> (false, true).
        mgr.raise_hand(session.id, chrono::Utc::now())
            .await
            .expect("raise_hand should succeed");
        assert_eq!(
            mgr.clear_user_input_transitions(session.id, true).await,
            (false, true)
        );
    }

    #[tokio::test]
    async fn clear_user_input_transitions_preserves_fresh_when_gate_is_false() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.set_start_fresh_on_restore(session.id, true).await;
        mgr.raise_hand(session.id, chrono::Utc::now())
            .await
            .expect("raise_hand should succeed");

        assert_eq!(
            mgr.clear_user_input_transitions(session.id, false).await,
            (false, true)
        );

        let stored = mgr
            .get_session(session.id)
            .await
            .expect("session should still exist");
        assert!(stored.start_fresh_on_restore);
        assert!(stored.communication.is_none());
    }

    #[tokio::test]
    async fn set_telegram_bot_id_writes_and_clears_field() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        assert!(!mgr.session_has_telegram_bot(session.id).await);

        mgr.set_telegram_bot_id(session.id, Some("bot-1".to_string()))
            .await;
        assert!(mgr.session_has_telegram_bot(session.id).await);
        assert_eq!(
            mgr.get_session(session.id)
                .await
                .unwrap()
                .telegram_bot_id
                .as_deref(),
            Some("bot-1")
        );

        mgr.set_telegram_bot_id(session.id, None).await;
        assert!(!mgr.session_has_telegram_bot(session.id).await);
        assert!(mgr
            .get_session(session.id)
            .await
            .unwrap()
            .telegram_bot_id
            .is_none());
    }

    #[tokio::test]
    async fn set_effective_shell_args_overwrites_on_recall() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude-mb".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        mgr.set_effective_shell_args(session.id, vec!["--continue".to_string()])
            .await;
        mgr.set_effective_shell_args(
            session.id,
            vec!["--continue".to_string(), "--debug".to_string()],
        )
        .await;

        let stored = mgr.get_session(session.id).await.unwrap();
        assert_eq!(
            stored.effective_shell_args,
            Some(vec!["--continue".to_string(), "--debug".to_string()])
        );
    }

    #[tokio::test]
    async fn clear_active_removes_active_and_demotes_status() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        assert_eq!(mgr.get_active().await, Some(session.id));
        mgr.clear_active().await;

        assert_eq!(mgr.get_active().await, None);
        let stored = mgr.get_session(session.id).await.unwrap();
        assert_eq!(stored.status, SessionStatus::Running);
    }

    #[tokio::test]
    async fn clear_active_if_preserves_non_matching_active_session() {
        let mgr = SessionManager::new();
        let first = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp\\one".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create first session");
        let second = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp\\two".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create second session");

        mgr.switch_session(second.id)
            .await
            .expect("switch to second session");
        mgr.clear_active_if(first.id).await;

        assert_eq!(mgr.get_active().await, Some(second.id));
        let first_stored = mgr.get_session(first.id).await.unwrap();
        let second_stored = mgr.get_session(second.id).await.unwrap();
        assert_eq!(first_stored.status, SessionStatus::Running);
        assert_eq!(second_stored.status, SessionStatus::Active);
    }

    // ── Issue #248 — set_active_only (Fix A) ──

    #[tokio::test]
    async fn set_active_only_preserves_dormant_status() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".into(),
                vec![],
                "C:\\proj".into(),
                None,
                None,
                vec![],
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        // After create_session, the session is auto-activated (status = Active);
        // call mark_exited to put it in the dormant state under test.
        mgr.mark_exited(session.id, 0).await;
        // clear_active_if to drop the now-stale active pointer.
        mgr.clear_active_if(session.id).await;
        assert_eq!(mgr.get_active().await, None);

        // The behavior under test: select the dormant session without flipping
        // its status.
        mgr.set_active_only(session.id).await.unwrap();
        assert_eq!(mgr.get_active().await, Some(session.id));
        let s = mgr.get_session(session.id).await.unwrap();
        assert!(matches!(s.status, SessionStatus::Exited(0))); // PRESERVED, not Active
    }

    #[tokio::test]
    async fn set_active_only_demotes_previously_active() {
        let mgr = SessionManager::new();
        let live = mgr
            .create_session(
                "c".into(),
                vec![],
                "C:\\a".into(),
                None,
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        // First session auto-activates → status = Active, active_session = live.id
        assert_eq!(mgr.get_active().await, Some(live.id));
        let live_state = mgr.get_session(live.id).await.unwrap();
        assert_eq!(live_state.status, SessionStatus::Active);

        // Create + mark-exited a second session for the dormant-select scenario.
        let dormant = mgr
            .create_session(
                "c".into(),
                vec![],
                "C:\\b".into(),
                None,
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        mgr.mark_exited(dormant.id, 0).await;

        mgr.set_active_only(dormant.id).await.unwrap();

        // Active pointer moved.
        assert_eq!(mgr.get_active().await, Some(dormant.id));
        // Previously-active demoted: Active → Running.
        let live_after = mgr.get_session(live.id).await.unwrap();
        assert_eq!(live_after.status, SessionStatus::Running);
        // New active preserved as Exited.
        let dormant_after = mgr.get_session(dormant.id).await.unwrap();
        assert!(matches!(dormant_after.status, SessionStatus::Exited(0)));
    }

    #[tokio::test]
    async fn set_active_only_returns_session_not_found_for_unknown_id() {
        let mgr = SessionManager::new();
        let bogus = uuid::Uuid::new_v4();
        let err = mgr.set_active_only(bogus).await.unwrap_err();
        assert!(matches!(err, AppError::SessionNotFound(_)));
        // Active pointer untouched.
        assert_eq!(mgr.get_active().await, None);
    }

    // ── Issue #248 / Grinch Z9 — defer + set_active_only + list_sessions chain ──

    #[tokio::test]
    async fn issue_248_defer_set_active_only_list_sessions_chain() {
        let mgr = SessionManager::new();
        // Simulate the defer arm of lib.rs §3.4: create a session, mark_exited,
        // clear_active_if.
        let session = mgr
            .create_session(
                "claude".into(),
                vec![],
                "C:\\proj\\.ac\\_agent_architect".into(),
                Some("aid".into()),
                Some("Architect".into()),
                vec![],
                true, // is_coordinator
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        mgr.mark_exited(session.id, 0).await;
        mgr.clear_active_if(session.id).await;

        // Simulate the post-loop active-switch (§3.7) with the dormant branch.
        mgr.set_active_only(session.id).await.unwrap();

        // The wire payload (what list_sessions IPC returns to the frontend).
        let infos = mgr.list_sessions().await;
        assert_eq!(infos.len(), 1);
        let json = serde_json::to_value(&infos[0]).unwrap();

        // The critical assertion — Round-2 Z1 blocker.
        // Before Fix A, this would be `"status":"active"` and the FE would
        // render the live dot, taking the wrong click path. With Fix A, status
        // round-trips as the object form for SessionStatus::Exited.
        assert_eq!(json["status"], serde_json::json!({ "exited": 0 }));

        // Active pointer correctly reflects the selection.
        assert_eq!(mgr.get_active().await, Some(session.id));
    }

    /// #260 G1 — pins `mark_idle`'s contract: the terminal mutation the
    /// idle-detector `on_idle` callback performs. NOTE: `create_session`
    /// auto-activates the first session (status `Active`), and `mark_idle`
    /// only transitions `Running → Idle` — so demote via `clear_active` first.
    /// Without that step the status assertion below would be vacuous.
    #[tokio::test]
    async fn mark_idle_sets_waiting_for_input_and_running_to_idle() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "codex".into(),
                vec![],
                "C:\\proj".into(),
                None,
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        mgr.clear_active().await; // Active → Running
        let before = mgr.get_session(session.id).await.unwrap();
        assert_eq!(before.status, SessionStatus::Running);
        assert!(!before.waiting_for_input);

        mgr.mark_idle(session.id).await;

        let after = mgr.get_session(session.id).await.unwrap();
        assert!(
            after.waiting_for_input,
            "mark_idle must set waiting_for_input = true"
        );
        assert_eq!(
            after.status,
            SessionStatus::Idle,
            "mark_idle must transition Running → Idle"
        );
    }

    // #552: a WG replica cwd that agent_fqn_from_path resolves to
    // `<project>:<wg>/<agent>`. team key = `<project>:<wg>`.
    const COORD_CWD: &str = "C:\\repos\\myproj\\.ac\\wg-1-team\\__agent_lead";
    const RUST_CWD: &str = "C:\\repos\\myproj\\.ac\\wg-1-team\\__agent_rust";
    const TEAM_KEY: &str = "myproj:wg-1-team";

    #[tokio::test]
    async fn coordinator_cwd_returns_cwd_only_for_coordinators() {
        let mgr = SessionManager::new();
        let coord = mgr
            .create_session(
                "claude".into(),
                vec![],
                COORD_CWD.into(),
                None,
                None,
                vec![],
                true, // is_coordinator
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let plain = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                "C:\\tmp".into(),
                None,
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();

        assert_eq!(
            mgr.coordinator_cwd(coord.id).await.as_deref(),
            Some(COORD_CWD)
        );
        assert!(mgr.coordinator_cwd(plain.id).await.is_none());
        assert!(mgr.coordinator_cwd(Uuid::new_v4()).await.is_none());
    }

    #[tokio::test]
    async fn agent_team_members_includes_agent_owned_excludes_adhoc() {
        let mgr = SessionManager::new();
        let coord = mgr
            .create_session(
                "claude".into(),
                vec![],
                COORD_CWD.into(),
                None,
                None,
                vec![],
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        // agent_id set, not a coordinator -> still agent-owned.
        let rust = mgr
            .create_session(
                "codex".into(),
                vec![],
                RUST_CWD.into(),
                Some("codex".into()),
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        // ad-hoc user shell: no agent_id, not a coordinator -> excluded.
        let _shell = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                "C:\\repos\\myproj\\.ac\\wg-1-team\\scratch".into(),
                None,
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();

        let members: std::collections::HashMap<Uuid, String> =
            mgr.agent_team_members().await.into_iter().collect();

        assert_eq!(members.len(), 2, "only the two agent-owned sessions");
        assert_eq!(members.get(&coord.id).map(String::as_str), Some(TEAM_KEY));
        assert_eq!(members.get(&rust.id).map(String::as_str), Some(TEAM_KEY));
    }

    #[tokio::test]
    async fn coordinator_refs_by_team_maps_team_to_coordinator() {
        let mgr = SessionManager::new();
        let coord = mgr
            .create_session(
                "claude".into(),
                vec![],
                COORD_CWD.into(),
                None,
                None,
                vec![],
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        // A non-coordinator agent must NOT appear in the refs map.
        let _rust = mgr
            .create_session(
                "codex".into(),
                vec![],
                RUST_CWD.into(),
                Some("codex".into()),
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();

        let refs = mgr.coordinator_refs_by_team().await;
        assert_eq!(refs.len(), 1, "exactly one coordinator team");
        let (fqn, cwd) = refs.get(TEAM_KEY).expect("team key present");
        assert_eq!(fqn, "myproj:wg-1-team/lead");
        assert_eq!(cwd, COORD_CWD);
        let _ = coord;
    }

    #[tokio::test]
    async fn coordinator_ids_by_team_maps_team_to_coordinator_id() {
        let mgr = SessionManager::new();
        let coord = mgr
            .create_session(
                "claude".into(),
                vec![],
                COORD_CWD.into(),
                None,
                None,
                vec![],
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        // A non-coordinator agent on the same team must NOT appear (only the coordinator).
        let _rust = mgr
            .create_session(
                "codex".into(),
                vec![],
                RUST_CWD.into(),
                Some("codex".into()),
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();

        let ids = mgr.coordinator_ids_by_team().await;
        assert_eq!(ids.len(), 1, "exactly one coordinator team");
        assert_eq!(
            ids.get(TEAM_KEY),
            Some(&coord.id),
            "team key maps to the coordinator session id, not a member"
        );
    }

    async fn pending_fixture(
        manager: &SessionManager,
        coordinator: bool,
    ) -> (Session, PendingCreateBinding) {
        manager
            .create_transaction_pending_session(
                "shell".to_string(),
                Vec::new(),
                "C:/work".to_string(),
                None,
                None,
                Vec::new(),
                coordinator,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("pending session")
    }

    fn live_witness(session_id: Uuid) -> LiveRuntimeWitness {
        LiveRuntimeWitness {
            session_id,
            has_pty: true,
            detached: false,
        }
    }

    fn dormant_witness(session_id: Uuid) -> DormantRuntimeWitness {
        DormantRuntimeWitness {
            session_id,
            has_pty: false,
            detached: false,
        }
    }

    #[tokio::test]
    async fn pending_create_is_invisible_until_atomic_live_finalization() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (pending, binding) = pending_fixture(&manager, false).await;

        assert!(manager.list_sessions().await.is_empty());
        assert!(manager.get_session(pending.id).await.is_none());
        let before = manager.aggregate_snapshot().await;
        assert!(before.sessions.is_empty());
        assert!(before.pending_ids.contains(&pending.id));
        assert!(before.order.contains(&pending.id));
        assert_eq!(before.selection.revision(), 0);

        let live = live_witness(pending.id);
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, live);
        let committed = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::SessionCreated(TrustedCreateIntent::User),
                mutations,
            )
            .await
            .expect("finalize pending create");
        assert_eq!(committed.finalized_rows.len(), 1);
        let visible = manager
            .get_session(pending.id)
            .await
            .expect("finalized row visible");
        assert_eq!(visible.status, SessionStatus::Active);
        let selection = manager.selection_payload().await;
        assert_eq!(selection.id(), Some(pending.id));
        assert_eq!(selection.revision(), 1);

        let mut duplicate = LifecycleMutations::default();
        duplicate.finalize_live(binding, live);
        assert!(manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::SessionCreated(TrustedCreateIntent::User),
                duplicate,
            )
            .await
            .is_err());
        assert_eq!(manager.selection_payload().await.revision(), 1);
    }

    #[tokio::test]
    async fn invalid_finalization_matrix_rejects_without_partial_mutation() {
        let capability = CommitCapability::for_test();

        for (has_pty, detached) in [(false, false), (true, true)] {
            let manager = SessionManager::new();
            let (pending, binding) = pending_fixture(&manager, false).await;
            let witness = LiveRuntimeWitness {
                session_id: pending.id,
                has_pty,
                detached,
            };
            let mut mutations = LifecycleMutations::default();
            mutations.finalize_live(binding, witness);
            assert!(manager
                .commit_selection_transition(
                    &capability,
                    CommitDecision::Keep,
                    SelectionCause::Restore,
                    mutations,
                )
                .await
                .is_err());
            assert!(manager.get_pending_session(binding).await.is_some());
            assert_eq!(manager.selection_payload().await.revision(), 0);
        }

        let manager = SessionManager::new();
        let (pending, binding) = pending_fixture(&manager, false).await;
        let wrong_binding = PendingCreateBinding::new(pending.id, Uuid::new_v4());
        let mut wrong = LifecycleMutations::default();
        wrong.finalize_live(wrong_binding, live_witness(pending.id));
        assert!(manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Keep,
                SelectionCause::Restore,
                wrong,
            )
            .await
            .is_err());
        assert!(manager.get_pending_session(binding).await.is_some());

        for conflict in ["remove", "exit"] {
            let manager = SessionManager::new();
            let (pending, binding) = pending_fixture(&manager, false).await;
            let mut mutations = LifecycleMutations::default();
            mutations.finalize_live(binding, live_witness(pending.id));
            if conflict == "remove" {
                mutations.remove(pending.id);
            } else {
                mutations.mark_exited(pending.id, 9);
            }
            assert!(manager
                .commit_selection_transition(
                    &capability,
                    CommitDecision::Keep,
                    SelectionCause::Restore,
                    mutations,
                )
                .await
                .is_err());
            assert!(manager.get_pending_session(binding).await.is_some());
        }

        let manager = SessionManager::new();
        let (pending, binding) = pending_fixture(&manager, false).await;
        manager
            .rollback_pending_create(binding)
            .await
            .expect("remove pending fixture");
        let mut missing = LifecycleMutations::default();
        missing.finalize_live(binding, live_witness(pending.id));
        assert!(manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Keep,
                SelectionCause::Restore,
                missing,
            )
            .await
            .is_err());
        assert!(manager.aggregate_snapshot().await.sessions.is_empty());
    }

    #[tokio::test]
    async fn dormant_finalization_and_live_transition_invariants_are_atomic() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (dormant, dormant_binding) = pending_fixture(&manager, false).await;
        let dormant_runtime = dormant_witness(dormant.id);
        let mut dormant_mutations = LifecycleMutations::default();
        dormant_mutations.finalize_dormant(dormant_binding, dormant_runtime, 37);
        manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Dormant(dormant_runtime),
                SelectionCause::Restore,
                dormant_mutations,
            )
            .await
            .expect("finalize dormant");
        assert_eq!(
            manager.get_session(dormant.id).await.unwrap().status,
            SessionStatus::Exited(37)
        );
        assert_eq!(
            manager.selection_payload().await.status(),
            Some(&SessionStatus::Exited(37))
        );

        let (first, first_binding) = pending_fixture(&manager, false).await;
        let first_live = live_witness(first.id);
        let mut first_finalize = LifecycleMutations::default();
        first_finalize.finalize_live(first_binding, first_live);
        manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(first_live),
                SelectionCause::UserSwitch,
                first_finalize,
            )
            .await
            .expect("select first live");

        let (second, second_binding) = pending_fixture(&manager, false).await;
        let second_live = live_witness(second.id);
        let mut second_finalize = LifecycleMutations::default();
        second_finalize.finalize_live(second_binding, second_live);
        manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(second_live),
                SelectionCause::UserSwitch,
                second_finalize,
            )
            .await
            .expect("select second live");
        assert_eq!(
            manager.get_session(first.id).await.unwrap().status,
            SessionStatus::Running
        );
        assert_eq!(
            manager.get_session(second.id).await.unwrap().status,
            SessionStatus::Active
        );

        let revision = manager.selection_payload().await.revision();
        for invalid in [
            LiveRuntimeWitness {
                session_id: Uuid::new_v4(),
                has_pty: true,
                detached: false,
            },
            LiveRuntimeWitness {
                session_id: first.id,
                has_pty: false,
                detached: false,
            },
            LiveRuntimeWitness {
                session_id: first.id,
                has_pty: true,
                detached: true,
            },
        ] {
            assert!(manager
                .commit_selection_transition(
                    &capability,
                    CommitDecision::Live(invalid),
                    SelectionCause::UserSwitch,
                    LifecycleMutations::default(),
                )
                .await
                .is_err());
        }
        assert_eq!(manager.selection_payload().await.revision(), revision);
    }

    #[tokio::test]
    async fn mark_exited_preserves_first_code_and_updates_selection_and_raise_hand_once() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (session, binding) = pending_fixture(&manager, true).await;
        let live = live_witness(session.id);
        let mut finalize = LifecycleMutations::default();
        finalize.finalize_live(binding, live);
        manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::SessionCreated(TrustedCreateIntent::User),
                finalize,
            )
            .await
            .unwrap();
        assert!(manager
            .raise_hand(session.id, chrono::Utc::now())
            .await
            .is_some());

        let dormant = dormant_witness(session.id);
        let mut exit = LifecycleMutations::default();
        exit.mark_exited(session.id, 7);
        let first = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Dormant(dormant),
                SelectionCause::LivenessReconcile,
                exit,
            )
            .await
            .expect("first exit");
        assert_eq!(first.cleared_raise_hand_ids, vec![session.id]);
        assert_eq!(
            manager.get_session(session.id).await.unwrap().status,
            SessionStatus::Exited(7)
        );
        assert!(manager
            .get_session(session.id)
            .await
            .unwrap()
            .communication
            .is_none());
        let revision = manager.selection_payload().await.revision();

        let mut duplicate = LifecycleMutations::default();
        duplicate.mark_exited(session.id, 99);
        let second = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Dormant(dormant),
                SelectionCause::LivenessReconcile,
                duplicate,
            )
            .await
            .expect("duplicate exit is idempotent");
        assert!(second.selection.is_none());
        assert!(second.changed_rows.is_empty());
        assert_eq!(manager.selection_payload().await.revision(), revision);
        assert_eq!(
            manager.get_session(session.id).await.unwrap().status,
            SessionStatus::Exited(7)
        );

        let mut keep_selected = LifecycleMutations::default();
        keep_selected.mark_exited(session.id, 101);
        assert!(manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Keep,
                SelectionCause::LivenessReconcile,
                keep_selected,
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn detached_intent_is_atomic_and_preserves_geometry() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (session, binding) = pending_fixture(&manager, false).await;
        let live = live_witness(session.id);
        let mut finalize = LifecycleMutations::default();
        finalize.finalize_live(binding, live);
        manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Keep,
                SelectionCause::Restore,
                finalize,
            )
            .await
            .unwrap();
        let geometry = WindowGeometry {
            x: 12.0,
            y: 34.0,
            width: 800.0,
            height: 600.0,
        };
        manager
            .set_detached_geometry(session.id, geometry.clone())
            .await;

        let mut detach = LifecycleMutations::default();
        detach.set_detached_intent(session.id, true);
        let changed = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Keep,
                SelectionCause::Detach,
                detach,
            )
            .await
            .unwrap();
        assert_eq!(changed.changed_rows.len(), 1);
        let stored = manager.get_session(session.id).await.unwrap();
        assert!(stored.was_detached);
        let stored_geometry = stored.detached_geometry.expect("stored geometry");
        assert_eq!(stored_geometry.x, geometry.x);
        assert_eq!(stored_geometry.y, geometry.y);
        assert_eq!(stored_geometry.width, geometry.width);
        assert_eq!(stored_geometry.height, geometry.height);

        let revision = manager.selection_payload().await.revision();
        let mut repeat = LifecycleMutations::default();
        repeat.set_detached_intent(session.id, true);
        let repeated = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Keep,
                SelectionCause::Detach,
                repeat,
            )
            .await
            .unwrap();
        assert!(repeated.changed_rows.is_empty());
        assert_eq!(manager.selection_payload().await.revision(), revision);
    }

    #[tokio::test]
    async fn revision_overflow_rejects_before_any_mutation() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (session, binding) = pending_fixture(&manager, false).await;
        manager.state.write().await.revision = u64::MAX;
        let live = live_witness(session.id);
        let mut finalize = LifecycleMutations::default();
        finalize.finalize_live(binding, live);
        let error = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::UserSwitch,
                finalize,
            )
            .await
            .expect_err("revision must not wrap");
        assert_eq!(error, "selection revision overflow");
        assert!(manager.get_pending_session(binding).await.is_some());
        assert!(manager.list_sessions().await.is_empty());
    }
}
