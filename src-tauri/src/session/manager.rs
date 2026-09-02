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
    /// #1149 - activity records built under the write guard and appended by
    /// `SelectionCoordinator::commit` once the guard is gone. Carried out rather
    /// than appended in place because the guard is a local of
    /// `commit_selection_transition` and no append may hold it.
    pub activity: Vec<crate::config::activity_log::ActivityRecord>,
}

const TERMINAL_SNAPSHOT_MAX_ROWS: usize = 4_096;
const TERMINAL_SNAPSHOT_MAX_CWD_BYTES: usize = 32_768;
const TERMINAL_SNAPSHOT_MAX_NAME_BYTES: usize = 1_024;
const TERMINAL_SNAPSHOT_MAX_AGGREGATE_BYTES: usize = 4 * 1024 * 1024;

fn requester_fact(session: &Session) -> Option<TerminalSnapshotRequesterFact> {
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

fn snapshot_session_fact_by_id(
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
            agent_turn_armed: false,
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
            context_percent: None,
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

    /// #1149 - the guard added here is emit-only. Every mutation below runs
    /// exactly as it did before, on every call, in every state; only the
    /// `log::info!` and the activity record are conditional. That is what turns
    /// 55,816 `[session-state]` lines into the roughly 8,054 real transitions
    /// among them without changing any behavior.
    pub async fn mark_idle(&self, id: Uuid) {
        let record = {
            let mut state = self.state.write().await;
            if state.pending_create.contains_key(&id) {
                return;
            }
            let Some(s) = state.sessions.get_mut(&id) else {
                return;
            };
            let was_working = crate::session::session::is_working(s);
            // MUST stay above the assignment: the literal `false` is correct only
            // because this line reads the pre-mutation state under `was_working`.
            // Below the assignment it would print `true → true` forever.
            if was_working {
                log::info!(
                    "[session-state] {} '{}': waiting_for_input false → true",
                    &id.to_string()[..8],
                    s.name
                );
            }
            s.waiting_for_input = true;
            if matches!(s.status, SessionStatus::Running) {
                s.status = SessionStatus::Idle;
            }
            if was_working && !crate::session::session::is_working(s) {
                // Built under the same guard that stamps `at`, so the block
                // annotation can never disagree with its own timestamp.
                Some(crate::config::activity_log::build_idle(
                    id,
                    s,
                    crate::config::activity_log::IdleReason::MarkIdle,
                ))
            } else {
                None
            }
        }; // write guard released here
        if let Some(record) = record {
            crate::config::activity_log::append(record);
        }
    }

    /// Mirror of [`Self::mark_idle`]. See the emit-only note there.
    pub async fn mark_busy(&self, id: Uuid) {
        let record = {
            let mut state = self.state.write().await;
            if state.pending_create.contains_key(&id) {
                return;
            }
            let Some(s) = state.sessions.get_mut(&id) else {
                return;
            };
            let was_working = crate::session::session::is_working(s);
            // The post-mutation predicate, computable before the mutation: this
            // clears `waiting_for_input` and promotes `Idle` to `Running`, so the
            // session ends up working iff it is not `Exited`. The `Exited` term is
            // required, not decorative: `mark_exited` leaves `waiting_for_input`
            // untouched, so a session that died while working sits at
            // `(Exited, false)`, where the literal `true` below would be a lie and
            // the line would repeat a target value A9 forbids repeating.
            let becomes_working = !matches!(s.status, SessionStatus::Exited(_));
            // MUST stay above the assignment; see `mark_idle`.
            if !was_working && becomes_working {
                log::info!(
                    "[session-state] {} '{}': waiting_for_input true → false",
                    &id.to_string()[..8],
                    s.name
                );
            }
            s.waiting_for_input = false;
            if matches!(s.status, SessionStatus::Idle) {
                s.status = SessionStatus::Running;
            }
            // The post-check is load-bearing here, unlike in `mark_idle` where
            // `was_working` implies it: on an `Exited` session this clears
            // `waiting_for_input` and leaves the status alone, so `is_working`
            // stays false and no record may be emitted.
            if !was_working && crate::session::session::is_working(s) {
                Some(crate::config::activity_log::build_busy(
                    id,
                    s,
                    crate::config::activity_log::BusyReason::MarkBusy,
                ))
            } else {
                None
            }
        }; // write guard released here
        if let Some(record) = record {
            crate::config::activity_log::append(record);
        }
    }

    /// #1682 - record that a message write reached an arming site for `id`, NOT that a message was delivered
    /// and submitted: R7 and R8 both arm with nothing submitted. Idempotent, and never undone: see this phase's
    /// D2. Deliberately NOT called from `mark_busy`, whose output-driven caller would arm a startup repaint.
    pub async fn arm_agent_turn(&self, id: Uuid) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        let Some(s) = state.sessions.get_mut(&id) else {
            return;
        };
        s.agent_turn_armed = true;
    }

    /// #1682 - where to persist the stamp for `id` on a busy->idle edge, or
    /// `None` when no message ever armed this session or it has no coding
    /// agent. Read-only: the latch is not consumed, so it never blocks a later
    /// edge; the value converges on the last edge the stamp gates let through.
    pub async fn agent_stamp_target(&self, id: Uuid) -> Option<(String, String)> {
        let state = self.state.read().await;
        if state.pending_create.contains_key(&id) {
            return None;
        }
        let s = state.sessions.get(&id)?;
        if !s.agent_turn_armed {
            return None;
        }
        let agent_id = s.agent_id.clone()?;
        Some((s.working_directory.clone(), agent_id))
    }

    /// #1149 - the working-session set, sampled without an await.
    ///
    /// `RunEvent::Exit` runs on the main thread and needs this set while the
    /// session map is still populated, but the only public read path,
    /// `list_sessions`, is `async` and unbounded, and the outer
    /// `Arc<RwLock<SessionManager>>` has no production writer so spinning on it
    /// would bound nothing. `None` means a writer holds or has merely queued for
    /// `state` (`tokio::sync::RwLock` is write-preferring), and the caller is
    /// expected to bound its own retry rather than block.
    ///
    /// Uses `is_working` internally so exactly one definition of "working"
    /// exists, and carries only the four fields an `idle` record needs: no
    /// `token`, no `last_prompt`, no shell args.
    ///
    /// `pub fn`, not `pub async fn`, deliberately: the architecture guard rejects
    /// new public async manager mutators, and a shutdown caller must not await.
    pub fn try_snapshot_working_sessions(
        &self,
    ) -> Option<Vec<crate::config::activity_log::WorkingSessionSnapshot>> {
        let state = self.state.try_read().ok()?;
        Some(
            state
                .order
                .iter()
                .filter(|id| !state.pending_create.contains_key(id))
                .filter_map(|id| state.sessions.get(id))
                .filter(|session| crate::session::session::is_working(session))
                .map(
                    |session| crate::config::activity_log::WorkingSessionSnapshot {
                        id: session.id,
                        name: session.name.clone(),
                        cwd: session.working_directory.clone(),
                        agent_kind: session.agent_kind,
                    },
                )
                .collect(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_pty_input_boundary<'a, F, G>(
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
        settings: &crate::config::settings::SettingsState,
        final_recipe_check: G,
        final_external_check: F,
    ) -> Result<
        crate::pty::manager::PtyRouteWriteGuard<'a>,
        crate::pty::idle_detector::PtyInputBoundaryFailure,
    >
    where
        F: FnOnce() -> bool,
        G: FnOnce(&Session, &crate::config::settings::AppSettings) -> bool,
    {
        use crate::pty::idle_detector::PtyInputBoundaryFailure as Failure;

        let (route_guard, was_idle, activity) = {
            let settings_guard = Arc::clone(settings)
                .try_read_owned()
                .map_err(|_| Failure::RouteUnavailable)?;
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
                || !final_recipe_check(session, &settings_guard)
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
            // #1682 - the boundary transaction arms this session, so its busy->idle
            // edges stamp `tooling.lastAgentMessageAt`. It keys on that arming, not on
            // proven delivery: R7 is the branch that arms without delivering. This site
            // is required because the plane bypasses `mark_busy` (see the #1149 note
            // below). It arms BEFORE the caller's write at `phone/mailbox.rs:6029`,
            // because this block, not that write, is this function's point of no return.
            session.agent_turn_armed = true;
            // #1149 - the third working-state mutation site, and the one that
            // bypasses `mark_busy`: `waiting_for_input` is already false by the
            // time `notify_pty_input_busy` reaches it, so hooking only
            // `mark_idle`/`mark_busy` would silently lose every inter-agent
            // injection edge.
            //
            // Unconditional here, unlike the other two sites: the checks above
            // rejected this call unless the session was non-`Exited` and waiting
            // for input, and `SessionStatus` has exactly four variants, so the
            // session is necessarily `Active` or `Running` with
            // `waiting_for_input == false` now. Nothing below can discard the
            // record either: no `?` and no early return remains in this block.
            let activity = crate::config::activity_log::build_busy(
                id,
                session,
                crate::config::activity_log::BusyReason::PtyInputBoundary,
            );
            let (mut route_guard, was_idle) = prepared;
            route_guard.retain_authority_guard(authority_route_guard);
            route_guard.retain_settings_guard(settings_guard);
            (route_guard, was_idle, activity)
        }; // write guard released here
        crate::config::activity_log::append(activity);
        // The `mark_busy` this reaches through the idle callback then sees no
        // edge and emits nothing, so there is exactly one record per injection.
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

    /// #1088 - write the scraper's latest context-usage percent onto one
    /// session so it rides `Session -> SessionInfo -> snapshot_sessions` into
    /// `sessions.json` for the disk-reading CLI. No logging (unlike
    /// `mark_idle`/`mark_busy`) to avoid a log line up to once per 5s per
    /// changing session. A write for an absent/pending id is a silent no-op,
    /// matching this mutator family and the scraper's "session may have ended
    /// between sample and commit" reality.
    pub async fn set_context_percent(&self, id: Uuid, percent: Option<u8>) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.context_percent = percent;
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
            message: None,
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

    /// #1646 / #1647 - Sets `session.communication` to `BlockedMenu` with a custom message.
    /// Returns `Some((changed, communication))` or `None` if the session was not found or is exited.
    pub async fn set_blocked_menu(
        &self,
        id: Uuid,
        message: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return None;
        }
        let session = state.sessions.get_mut(&id)?;
        if matches!(session.status, SessionStatus::Exited(_)) {
            return None;
        }

        if let Some(existing) = session.communication.as_ref() {
            if existing.kind == SessionCommunicationKind::BlockedMenu
                && existing.visible
                && existing.message.as_deref() == Some(&message)
            {
                return Some((false, existing.clone()));
            }
        }

        let communication = SessionCommunication {
            kind: SessionCommunicationKind::BlockedMenu,
            visible: true,
            updated_at: updated_at.to_rfc3339(),
            message: Some(message),
        };
        session.communication = Some(communication.clone());
        Some((true, communication))
    }

    /// #1646 / #1647 - Clears `session.communication` if currently `Some` with `kind == SessionCommunicationKind::BlockedMenu` and `visible == true`.
    /// Returns `true` if cleared.
    pub async fn clear_blocked_menu(&self, id: Uuid) -> bool {
        self.clear_communication_if_kind(id, SessionCommunicationKind::BlockedMenu)
            .await
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
        // #1149 - ids only. The record itself is built after every fallible exit
        // below, so a later `?` can never strand a record for a session this
        // commit did not actually finalize.
        let mut finalized_live_ids: Vec<Uuid> = Vec::new();
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
                        finalized_live_ids.push(binding.session_id);
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

        // #1149 - the opening edge at session birth, deferred past every fallible
        // exit above so no record can survive a commit that returned `Err`. The
        // write guard is still held, which is what keeps the coalescer exclusive.
        //
        // The re-read is also semantically better than emitting inside the
        // finalization arm: a session finalized and then removed or marked exited
        // reads back as absent or `Exited`, so `is_working` is false and nothing
        // is recorded. Such a session was never observably working. One promoted
        // to `Active` still reads as working and is recorded.
        for id in finalized_live_ids {
            if let Some(record) = state.sessions.get(&id) {
                if crate::session::session::is_working(record) {
                    result
                        .activity
                        .push(crate::config::activity_log::build_busy(
                            id,
                            record,
                            crate::config::activity_log::BusyReason::SessionStart,
                        ));
                }
            }
        }

        Ok(result)
    }

    /// §1295 5.6 — remove dormant orphan rows by id (production impl). The
    /// ORACLE is re-verified right here against the LIVE row: a row is removed
    /// only if it exists AND `status == Exited(_)` AND it has no pending create.
    /// This also skips a row that was `Exited` at snapshot time but got restarted
    /// in between.
    ///
    /// The section between acquiring and dropping the interior write lock is
    /// PURELY SYNCHRONOUS (no awaits), mirroring `destroy_session` (dev-rust
    /// caveat A). Selection repair mirrors `destroy_session` but with
    /// `SelectionCause::BackgroundCleanup` (NOT ManualClose): the prune is not
    /// user-initiated, so the next selection is not attributed to a close.
    ///
    /// Missing ids are not an error. Returns the number of rows removed. The
    /// method never calls any persist helper, never touches the archive, and
    /// never awaits while holding the interior write. A benign race: a
    /// concurrent `close-session` that listed an id just before the prune fails
    /// once with `AppError::SessionNotFound` and reports a now-missing row;
    /// one-shot, no retry loop (dev-rust caveat B).
    pub(crate) async fn remove_exited_sessions(&self, ids: &[Uuid]) -> usize {
        let mut state = self.state.write().await;
        let mut removed = 0usize;
        let mut removed_selection = false;
        let mut removed_ids: Vec<Uuid> = Vec::new();
        for &id in ids {
            let Some(session) = state.sessions.get(&id) else {
                continue;
            };
            if !matches!(session.status, SessionStatus::Exited(_)) {
                continue;
            }
            if state.pending_create.contains_key(&id) {
                continue;
            }
            state.sessions.remove(&id);
            removed += 1;
            removed_ids.push(id);
            if state.selection.id() == Some(id) {
                removed_selection = true;
            }
        }
        if !removed_ids.is_empty() {
            state
                .order
                .retain(|candidate| !removed_ids.contains(candidate));
        }
        if removed_selection {
            state.revision += 1;
            let next = state
                .order
                .iter()
                .copied()
                .find(|candidate| !state.pending_create.contains_key(candidate));
            if let Some(next_id) = next {
                if let Some(session) = state.sessions.get_mut(&next_id) {
                    session.status = SessionStatus::Active;
                }
                state.selection = SessionSelection::live(
                    state.epoch,
                    state.revision,
                    SelectionCause::BackgroundCleanup,
                    next_id,
                );
            } else {
                state.selection = SessionSelection::none(
                    state.epoch,
                    state.revision,
                    SelectionCause::BackgroundCleanup,
                );
            }
        }
        removed
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

    /// #1063: insert a pending create (hidden from public reads, visible to the
    /// deletion-only snapshot) at `working_directory`, for cross-module deletion
    /// race tests.
    pub(crate) async fn insert_pending_session_for_test(&self, working_directory: String) -> Uuid {
        let (session, _binding) = self
            .create_transaction_pending_session(
                "sh".to_string(),
                Vec::new(),
                working_directory,
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("insert pending session for test");
        session.id
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
            agent_turn_armed: false,
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
            context_percent: None,
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

    #[test]
    fn terminal_snapshot_fact_debug_uses_only_structural_fields() {
        const NAME_CANARY: &str = "NAME_1173_S2K9";
        const PATH_CANARY: &str = r"C:\PATH_1173_S2K9\replica";
        let id = Uuid::parse_str("11730000-0000-4000-8000-00000000c229").unwrap();
        let requester = TerminalSnapshotRequesterFact {
            id,
            created_at: chrono::Utc::now(),
            working_directory: PATH_CANARY.to_string(),
            backend_kind: SessionBackendKind::ContainerTransport,
            is_coordinator: true,
            is_root_agent: false,
        };
        let session = TerminalSnapshotSessionFact {
            id,
            created_at: chrono::Utc::now(),
            name: NAME_CANARY.to_string(),
            status: SessionStatus::Running,
            working_directory: PATH_CANARY.to_string(),
            backend_kind: SessionBackendKind::LocalProcess,
        };
        let diagnostic = format!("{requester:?}\n{session:?}");
        let id_text = id.to_string();
        for forbidden in [NAME_CANARY, PATH_CANARY, id_text.as_str()] {
            assert!(!diagnostic.contains(forbidden));
        }
        for structural in [
            "working_directory_bytes",
            "name_bytes",
            "status: Running",
            "backend_kind: ContainerTransport",
        ] {
            assert!(diagnostic.contains(structural));
        }
    }

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

    // #1063: the deletion-only snapshot is pending-inclusive (unlike every public
    // read), excludes `Exited` rows, and returns working directories only.
    #[tokio::test]
    async fn deletion_snapshot_is_pending_inclusive_and_excludes_exited() {
        let mgr = SessionManager::new();
        let _live = mgr
            .create_session(
                "sh".to_string(),
                Vec::new(),
                "C:\\live".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create live");
        let (_pending, _binding) = mgr
            .create_transaction_pending_session(
                "sh".to_string(),
                Vec::new(),
                "C:\\pending".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create pending");
        let exited = mgr
            .create_session(
                "sh".to_string(),
                Vec::new(),
                "C:\\exited".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create exited");
        mgr.state
            .write()
            .await
            .sessions
            .get_mut(&exited.id)
            .expect("exited session present")
            .status = SessionStatus::Exited(0);

        let public: Vec<String> = mgr
            .list_sessions()
            .await
            .into_iter()
            .map(|session| session.working_directory)
            .collect();
        assert!(public.contains(&"C:\\live".to_string()));
        assert!(
            !public.contains(&"C:\\pending".to_string()),
            "public reads must hide the pending create"
        );

        let snapshot = {
            let mgr = mgr.clone();
            tokio::task::spawn_blocking(move || {
                mgr.live_working_directories_for_deletion_blocking()
            })
            .await
            .unwrap()
        };
        assert!(
            snapshot.contains(&"C:\\live".to_string()),
            "snapshot must include the live session"
        );
        assert!(
            snapshot.contains(&"C:\\pending".to_string()),
            "snapshot must include the pending create (deletion-only)"
        );
        assert!(
            !snapshot.contains(&"C:\\exited".to_string()),
            "snapshot must exclude exited sessions"
        );
    }

    // Stage E (#1064) deletion-snapshot mutation sentinel (plan section 10.3,
    // 10.6, acceptance item 32): the pending-inclusive workdir snapshot must
    // include EVERY pending create (not just one) and stays workdir-only while
    // public listing keeps every pending hidden. A mutation that mirrored
    // `list_sessions` or filtered `pending_create` fails here.
    #[tokio::test]
    async fn stage_e_deletion_snapshot_includes_every_pending_and_is_workdir_only() {
        let mgr = SessionManager::new();
        let _live = mgr
            .create_session(
                "sh".to_string(),
                Vec::new(),
                "C:\\live".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create live");
        let _pending_a = mgr
            .insert_pending_session_for_test("C:\\pending-a".to_string())
            .await;
        let _pending_b = mgr
            .insert_pending_session_for_test("C:\\pending-b".to_string())
            .await;
        let exited = mgr
            .create_session(
                "sh".to_string(),
                Vec::new(),
                "C:\\exited".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create exited");
        mgr.state
            .write()
            .await
            .sessions
            .get_mut(&exited.id)
            .expect("exited present")
            .status = SessionStatus::Exited(0);

        let snapshot = {
            let mgr = mgr.clone();
            tokio::task::spawn_blocking(move || {
                mgr.live_working_directories_for_deletion_blocking()
            })
            .await
            .unwrap()
        };
        for workdir in ["C:\\live", "C:\\pending-a", "C:\\pending-b"] {
            assert!(
                snapshot.contains(&workdir.to_string()),
                "snapshot must include the non-exited workdir {workdir}"
            );
        }
        assert!(!snapshot.contains(&"C:\\exited".to_string()));
        assert_eq!(
            snapshot.len(),
            3,
            "exactly the three non-exited workdirs, no metadata rows"
        );

        let public: Vec<String> = mgr
            .list_sessions()
            .await
            .into_iter()
            .map(|session| session.working_directory)
            .collect();
        assert!(!public.contains(&"C:\\pending-a".to_string()));
        assert!(!public.contains(&"C:\\pending-b".to_string()));
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
            message: None,
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
            message: None,
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

    // ── #1088: set_context_percent mutator ──

    #[tokio::test]
    async fn set_context_percent_round_trips_including_zero_and_clear() {
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

        // Fresh session has no reading.
        assert_eq!(
            mgr.get_session(session.id).await.unwrap().context_percent,
            None
        );

        mgr.set_context_percent(session.id, Some(42)).await;
        assert_eq!(
            mgr.get_session(session.id).await.unwrap().context_percent,
            Some(42)
        );

        // `0` is a valid reading, stored as Some(0), never coerced to None.
        mgr.set_context_percent(session.id, Some(0)).await;
        assert_eq!(
            mgr.get_session(session.id).await.unwrap().context_percent,
            Some(0)
        );

        // None clears it.
        mgr.set_context_percent(session.id, None).await;
        assert_eq!(
            mgr.get_session(session.id).await.unwrap().context_percent,
            None
        );
    }

    #[tokio::test]
    async fn set_context_percent_noops_on_unknown_id() {
        let mgr = SessionManager::new();
        // Must not panic; the id is not present, so the write is a silent no-op.
        mgr.set_context_percent(Uuid::new_v4(), Some(5)).await;
    }

    #[tokio::test]
    async fn set_context_percent_noops_on_pending_create_id() {
        let mgr = SessionManager::new();
        let (pending, _binding) = pending_fixture(&mgr, false).await;
        // The pending guard suppresses the write; the row stays invisible and
        // nothing panics (mirrors mark_idle's pending_create guard).
        mgr.set_context_percent(pending.id, Some(5)).await;
        assert!(mgr.get_session(pending.id).await.is_none());
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

    /// The unstated invariant that makes `commit_selection_transition`'s
    /// `-> Active` promotion neutral for the activity signal: a session is never
    /// `Idle` while `waiting_for_input` is false. If it ever were, focusing that
    /// session would flip `is_working` false to true with no record behind it.
    #[tokio::test]
    async fn idle_status_implies_waiting_for_input() {
        let manager = SessionManager::new();
        // The first session takes the selection and becomes Active; the second
        // stays Running, which is the only status `mark_idle` demotes.
        let _selected = manager
            .create_session(
                "sh".to_string(),
                Vec::new(),
                "C:\\selected".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create the selected session");
        let subject = manager
            .create_session(
                "sh".to_string(),
                Vec::new(),
                "C:\\subject".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create the subject session");

        assert_eq!(subject.status, SessionStatus::Running);
        assert!(
            !subject.waiting_for_input,
            "a session is born (Running, false)"
        );

        let mut saw_idle = false;
        // Both writers, repeated, so the no-op paths are covered too.
        for action in ["idle", "idle", "busy", "busy", "idle"] {
            match action {
                "idle" => manager.mark_idle(subject.id).await,
                _ => manager.mark_busy(subject.id).await,
            }
            let observed = manager
                .get_session(subject.id)
                .await
                .expect("the subject session survives");
            if matches!(observed.status, SessionStatus::Idle) {
                saw_idle = true;
                assert!(
                    observed.waiting_for_input,
                    "after {action}: Idle with waiting_for_input false manufactures an unrecorded ON edge"
                );
            }
        }
        assert!(
            saw_idle,
            "the sequence must actually reach Idle or it proves nothing"
        );
    }

    fn activity_json(record: &crate::config::activity_log::ActivityRecord) -> serde_json::Value {
        serde_json::to_value(record).expect("an activity record serializes")
    }

    /// Everything the emission sites appended on this thread since the last call.
    /// No sink is configured in a test process, so this observes the calls
    /// without a file.
    fn emitted() -> Vec<serde_json::Value> {
        crate::config::activity_log::capture::drain()
            .iter()
            .map(activity_json)
            .collect()
    }

    /// A live, working session: the first `create_session` takes the selection
    /// and becomes `Active`, so the returned one stays `Running`.
    async fn working_session(manager: &SessionManager, cwd: &str) -> Session {
        if manager.get_active().await.is_none() {
            manager
                .create_session(
                    "sh".to_string(),
                    Vec::new(),
                    "C:\\selected".to_string(),
                    None,
                    None,
                    Vec::new(),
                    false,
                    SessionBackendKind::LocalProcess,
                )
                .await
                .expect("create the selected session");
        }
        manager
            .create_session(
                "sh".to_string(),
                Vec::new(),
                cwd.to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create a working session")
    }

    #[tokio::test]
    async fn mark_idle_on_a_working_session_yields_exactly_one_idle_record() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;
        let _ = emitted();

        manager.mark_idle(subject.id).await;

        let records = emitted();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["event"], "idle");
        assert_eq!(records[0]["reason"], "mark_idle");
        assert_eq!(records[0]["sessionId"], subject.id.to_string());
        assert_eq!(records[0]["cwd"], "C:\\subject");
        assert_eq!(records[0]["idleThresholdMs"], serde_json::json!(2_500));
    }

    #[tokio::test]
    async fn mark_idle_on_an_already_idle_session_yields_no_record() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;
        manager.mark_idle(subject.id).await;
        let _ = emitted();

        manager.mark_idle(subject.id).await;

        assert!(
            emitted().is_empty(),
            "a call that changes nothing produces no record"
        );
    }

    #[tokio::test]
    async fn mark_busy_on_an_idle_session_yields_exactly_one_busy_record() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;
        manager.mark_idle(subject.id).await;
        let _ = emitted();

        manager.mark_busy(subject.id).await;

        let records = emitted();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["event"], "busy");
        assert_eq!(records[0]["reason"], "mark_busy");
        assert_eq!(records[0]["sessionId"], subject.id.to_string());
        assert_eq!(
            records[0]["continuesBlock"],
            serde_json::json!(true),
            "the idle a moment ago is inside the block window"
        );
        assert!(records[0]["gapMs"].is_u64());
    }

    #[tokio::test]
    async fn mark_busy_twice_yields_one_record() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;
        manager.mark_idle(subject.id).await;
        let _ = emitted();

        manager.mark_busy(subject.id).await;
        manager.mark_busy(subject.id).await;

        assert_eq!(emitted().len(), 1, "only the edge is recorded");
    }

    #[tokio::test]
    async fn mark_busy_on_an_exited_session_yields_no_record() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;
        // Exiting leaves `waiting_for_input` untouched, so the session sits at
        // `(Exited, false)`: `mark_busy` changes nothing observable and
        // `is_working` stays false.
        manager.mark_exited(subject.id, 0).await;
        let stored = manager
            .get_session(subject.id)
            .await
            .expect("the session still exists");
        assert!(!stored.waiting_for_input);
        let _ = emitted();

        manager.mark_busy(subject.id).await;

        assert!(
            emitted().is_empty(),
            "a dead session must not open an interval"
        );
    }

    #[tokio::test]
    async fn mark_idle_still_demotes_running_to_idle_when_already_waiting_for_input() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;
        // Waiting for input while still `Running`: the guard must not skip the
        // demotion just because there is no edge to record.
        manager.mark_idle(subject.id).await;
        manager.mark_busy(subject.id).await;
        manager
            .state
            .write()
            .await
            .sessions
            .get_mut(&subject.id)
            .expect("session")
            .waiting_for_input = true;
        let _ = emitted();

        manager.mark_idle(subject.id).await;

        let stored = manager.get_session(subject.id).await.expect("session");
        assert_eq!(stored.status, SessionStatus::Idle);
        assert!(stored.waiting_for_input);
        assert!(emitted().is_empty(), "no edge, so no record");
    }

    #[tokio::test]
    async fn mark_idle_still_sets_waiting_for_input_on_every_call() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;
        for round in 0..3 {
            manager.mark_idle(subject.id).await;
            let stored = manager.get_session(subject.id).await.expect("session");
            assert!(
                stored.waiting_for_input,
                "round {round}: the mutation is unconditional"
            );
        }
    }

    #[tokio::test]
    async fn mark_idle_on_a_pending_create_session_yields_no_record() {
        let manager = SessionManager::new();
        let (pending, _binding) = pending_fixture(&manager, false).await;
        let _ = emitted();

        manager.mark_idle(pending.id).await;
        manager.mark_busy(pending.id).await;

        assert!(
            emitted().is_empty(),
            "a session that does not exist publicly yet emits nothing"
        );
    }

    /// `commands/session.rs` hands the PTY spawn `idle_tuning_for(agent_kind)`,
    /// and the detector registers exactly that. The record must therefore report
    /// the same threshold for every kind, not a hardcoded constant.
    #[tokio::test]
    async fn idle_record_carries_the_same_threshold_the_detector_registered() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;

        for kind in [
            None,
            Some(CodingAgentKind::Claude),
            Some(CodingAgentKind::Codex),
            Some(CodingAgentKind::Antigravity),
            Some(CodingAgentKind::Pi),
        ] {
            let mut session = subject.clone();
            session.agent_kind = kind;
            let record = activity_json(&crate::config::activity_log::build_idle(
                session.id,
                &session,
                crate::config::activity_log::IdleReason::MarkIdle,
            ));
            let registered = crate::session::profile::idle_tuning_for(kind)
                .idle_threshold
                .as_millis() as u64;
            assert_eq!(
                record["idleThresholdMs"],
                serde_json::json!(registered),
                "kind={kind:?}"
            );
        }

        // And through the real mutation site, for the kind the fixture carries.
        let _ = emitted();
        manager.mark_idle(subject.id).await;
        let records = emitted();
        let registered = crate::session::profile::idle_tuning_for(subject.agent_kind)
            .idle_threshold
            .as_millis() as u64;
        assert_eq!(records[0]["idleThresholdMs"], serde_json::json!(registered));
    }

    /// The defect the birth edge exists to remove: without it the first
    /// `mark_idle` after finalization is an orphan `idle` with no opening `busy`.
    #[tokio::test]
    async fn the_first_idle_after_finalization_has_a_matching_busy() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (pending, binding) = pending_fixture(&manager, false).await;
        let live = live_witness(pending.id);
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, live);
        let result = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::UserSwitch,
                mutations,
            )
            .await
            .expect("finalize live");
        let opening: Vec<serde_json::Value> = result.activity.iter().map(activity_json).collect();
        let _ = emitted();

        manager.mark_idle(pending.id).await;
        let closing = emitted();

        assert_eq!(opening.len(), 1);
        assert_eq!(opening[0]["event"], "busy");
        assert_eq!(closing.len(), 1);
        assert_eq!(closing[0]["event"], "idle");
        assert_eq!(
            opening[0]["sessionId"], closing[0]["sessionId"],
            "the closing edge must pair with the opening one"
        );
    }

    #[tokio::test]
    async fn try_snapshot_working_sessions_excludes_pending_create() {
        let manager = SessionManager::new();
        let live = working_session(&manager, "C:\\live").await;
        let (pending, _binding) = pending_fixture(&manager, false).await;

        let rows = manager
            .try_snapshot_working_sessions()
            .expect("no writer holds state");

        let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
        assert!(ids.contains(&live.id));
        assert!(
            !ids.contains(&pending.id),
            "a session that does not exist publicly yet must not get a synthetic close"
        );
    }

    #[tokio::test]
    async fn try_snapshot_working_sessions_returns_none_rather_than_blocking_while_a_writer_holds_state(
    ) {
        let manager = SessionManager::new();
        let _live = working_session(&manager, "C:\\live").await;

        let writer = manager.state.write().await;
        assert!(
            manager.try_snapshot_working_sessions().is_none(),
            "the shutdown path must never block on the session map"
        );
        drop(writer);

        assert!(
            manager.try_snapshot_working_sessions().is_some(),
            "the snapshot succeeds again once the writer is gone"
        );
    }

    #[tokio::test]
    async fn try_snapshot_working_sessions_uses_the_same_predicate_as_is_working() {
        let manager = SessionManager::new();
        let subject = working_session(&manager, "C:\\subject").await;

        // Every state the two production writers and the exit path can leave a
        // session in, checked against the shared predicate rather than a second
        // definition over `SessionInfo`.
        for step in ["born", "idle", "busy", "exited"] {
            match step {
                "born" => {}
                "idle" => manager.mark_idle(subject.id).await,
                "busy" => manager.mark_busy(subject.id).await,
                _ => {
                    manager.mark_exited(subject.id, 0).await;
                }
            }
            let stored = manager.get_session(subject.id).await.expect("session");
            let listed = manager
                .try_snapshot_working_sessions()
                .expect("no writer holds state")
                .iter()
                .any(|row| row.id == subject.id);
            assert_eq!(
                listed,
                crate::session::session::is_working(&stored),
                "step {step}: status={:?} waiting_for_input={}",
                stored.status,
                stored.waiting_for_input
            );
        }
    }

    #[tokio::test]
    async fn finalize_live_yields_exactly_one_session_start_busy_record() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (pending, binding) = pending_fixture(&manager, false).await;
        let live = live_witness(pending.id);
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, live);

        let result = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::UserSwitch,
                mutations,
            )
            .await
            .expect("finalize live");

        assert_eq!(
            result.activity.len(),
            1,
            "session birth is one opening edge, no more and no less"
        );
        let record = activity_json(&result.activity[0]);
        assert_eq!(record["event"], "busy");
        assert_eq!(record["reason"], "session_start");
        assert_eq!(record["sessionId"], pending.id.to_string());
        assert_eq!(record["cwd"], "C:/work");
        assert_eq!(record["continuesBlock"], serde_json::json!(false));
        assert!(record.get("gapMs").is_none());
    }

    #[tokio::test]
    async fn finalize_dormant_yields_no_activity_record() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (pending, binding) = pending_fixture(&manager, false).await;
        let dormant = dormant_witness(pending.id);
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_dormant(binding, dormant, 0);

        let result = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Dormant(dormant),
                SelectionCause::UserSwitch,
                mutations,
            )
            .await
            .expect("finalize dormant");

        assert!(
            result.activity.is_empty(),
            "a dormant finalization never opens an interval"
        );
    }

    // The conflict validation rejects remove+finalize outright, so the deferred
    // re-read is defense in depth rather than the active mechanism here. Either
    // way no record reaches the caller, which is the required behavior.
    #[tokio::test]
    async fn a_session_finalized_and_removed_in_the_same_commit_yields_no_activity_record() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (pending, binding) = pending_fixture(&manager, false).await;
        let live = live_witness(pending.id);
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, live);
        mutations.remove(pending.id);

        let error = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::UserSwitch,
                mutations,
            )
            .await
            .expect_err("remove+finalize is rejected");

        assert!(
            error.contains("remove+finalize conflict"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn a_session_finalized_and_marked_exited_in_the_same_commit_yields_no_activity_record() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (pending, binding) = pending_fixture(&manager, false).await;
        let live = live_witness(pending.id);
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, live);
        mutations.mark_exited(pending.id, 0);

        let error = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::UserSwitch,
                mutations,
            )
            .await
            .expect_err("markExited+finalize is rejected");

        assert!(
            error.contains("markExited+finalize conflict"),
            "unexpected error: {error}"
        );
    }

    /// The records are built after every fallible exit, so an `Err` returns
    /// before any `CommitResult` reaches the caller and nothing can be appended
    /// for a session this commit did not finalize.
    #[tokio::test]
    async fn a_failed_commit_yields_no_activity_records() {
        let manager = SessionManager::new();
        let capability = CommitCapability::for_test();
        let (session, binding) = pending_fixture(&manager, false).await;
        manager.state.write().await.revision = u64::MAX;
        let live = live_witness(session.id);
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, live);

        let outcome = manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(live),
                SelectionCause::UserSwitch,
                mutations,
            )
            .await;

        assert!(
            outcome.is_err(),
            "no CommitResult, therefore no activity records, may reach the caller"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // §1295 — remove_exited_sessions
    // ──────────────────────────────────────────────────────────────────────

    /// Test 9: of a mixed set (Running, Exited, pending) only the Exited
    /// non-pending row is removed; the selection is repaired to the next
    /// non-pending row with `SelectionCause::BackgroundCleanup`; count is
    /// correct.
    #[tokio::test]
    async fn remove_exited_sessions_removes_only_exited_non_pending() {
        let mgr = SessionManager::new();
        let a = mgr
            .create_session(
                "running".into(),
                vec![],
                "C:/x/a".into(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create running");
        let b = mgr
            .create_session(
                "exited".into(),
                vec![],
                "C:/x/b".into(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create exited");
        let c_pending = mgr.insert_pending_session_for_test("C:/x/c".into()).await;
        mgr.mark_exited(b.id, 0).await;
        // Make the soon-to-remove Exited row the current selection.
        mgr.set_active_only(b.id).await.expect("select b");
        assert_eq!(mgr.selection_payload().await.id(), Some(b.id));

        let removed = mgr.remove_exited_sessions(&[a.id, b.id, c_pending]).await;
        assert_eq!(removed, 1, "only the Exited non-pending row is removed");

        // b gone; a (Running) and c (pending) remain.
        assert_eq!(mgr.get_session(b.id).await.map(|s| s.name), None);
        assert!(mgr.get_session(a.id).await.is_some());
        // Pending rows are hidden from the public read path; assert the pending
        // create is still present instead.
        assert!(mgr.contains_public_or_pending(c_pending).await);

        // Selection repaired to the next non-pending row (a) with BackgroundCleanup.
        let selection = mgr.selection_payload().await;
        assert_eq!(selection.id(), Some(a.id));
        assert_eq!(
            selection.source(),
            crate::session::selection::SelectionSource::BackgroundCleanup
        );
    }

    /// Test 10: rows that are live (Running) or pending are never removed.
    #[tokio::test]
    async fn remove_exited_sessions_never_removes_live_or_pending() {
        let mgr = SessionManager::new();
        let a = mgr
            .create_session(
                "running-a".into(),
                vec![],
                "C:/x/a".into(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create a");
        let b = mgr
            .create_session(
                "running-b".into(),
                vec![],
                "C:/x/b".into(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create b");
        let c_pending = mgr.insert_pending_session_for_test("C:/x/c".into()).await;

        let removed = mgr.remove_exited_sessions(&[a.id, b.id, c_pending]).await;
        assert_eq!(removed, 0, "no live/pending row is ever removed");
        assert!(mgr.get_session(a.id).await.is_some());
        assert!(mgr.get_session(b.id).await.is_some());
        // Pending rows are hidden from the public read path (by design), so assert
        // the pending create is still present rather than via get_session.
        assert!(mgr.contains_public_or_pending(c_pending).await);
    }

    #[tokio::test]
    async fn an_armed_session_yields_the_same_target_on_every_edge() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "codex".into(),
                vec![],
                "C:/x/armed".into(),
                Some("codex".to_string()),
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create the armed session");

        // Control, and the whole assertion for the unarmed gate.
        assert_eq!(mgr.agent_stamp_target(session.id).await, None);

        mgr.arm_agent_turn(session.id).await;
        let expected = Some((session.working_directory.clone(), "codex".to_string()));
        // The latch is never consumed, so every later busy->idle edge sees the
        // same target rather than only the first.
        assert_eq!(mgr.agent_stamp_target(session.id).await, expected);
        assert_eq!(mgr.agent_stamp_target(session.id).await, expected);
        assert_eq!(mgr.agent_stamp_target(session.id).await, expected);
    }

    #[tokio::test]
    async fn a_session_without_an_agent_id_never_yields_a_target() {
        let mgr = SessionManager::new();
        let plain = mgr
            .create_session(
                "pwsh".into(),
                vec![],
                "C:/x/plain".into(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create the agentless session");
        mgr.arm_agent_turn(plain.id).await;
        assert_eq!(mgr.agent_stamp_target(plain.id).await, None);
        assert_eq!(mgr.agent_stamp_target(plain.id).await, None);

        // In-test control: without it this passes against an
        // `agent_stamp_target` that always returns `None`.
        let agent = mgr
            .create_session(
                "codex".into(),
                vec![],
                "C:/x/agent".into(),
                Some("codex".to_string()),
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create the agent session");
        mgr.arm_agent_turn(agent.id).await;
        assert_eq!(
            mgr.agent_stamp_target(agent.id).await,
            Some((agent.working_directory.clone(), "codex".to_string()))
        );
    }
}
