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

/// Verified host directory and exact launch profiles; IO stays with admission.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PromptKey {
    pub canonical_cwd: String,
    pub agent_id: String,
    pub requested_profile: Option<String>,
    pub effective_profile: String,
}

impl PromptKey {
    fn matches(&self, row: &Session) -> bool {
        row.agent_id.as_ref() == Some(&self.agent_id)
            && row.requested_profile == self.requested_profile
            && row.effective_profile.as_ref() == Some(&self.effective_profile)
            && row.backend_kind == SessionBackendKind::LocalProcess
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RetainedPrompt {
    pub last_prompt: Option<String>,
    pub blocked: bool,
}

struct PendingPrompt {
    key: PromptKey,
    continuation: bool,
    fresh: bool,
    allow_retained_lookup: bool,
}

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

/// The process publication gate is shared with P05 persistence. It holds no
/// directory protection map or settings state.
pub(crate) fn process_carrier_publication_gate() -> &'static Arc<tokio::sync::Mutex<()>> {
    static GATE: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
}

pub(crate) struct CarrierMutationPermit {
    gate: Arc<tokio::sync::Mutex<()>>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

pub(crate) async fn acquire_carrier_mutation_permit() -> CarrierMutationPermit {
    let gate = process_carrier_publication_gate().clone();
    let guard = gate.clone().lock_owned().await;
    CarrierMutationPermit {
        gate,
        _guard: guard,
    }
}

impl CarrierMutationPermit {
    fn proves(&self) -> bool {
        Arc::ptr_eq(&self.gate, process_carrier_publication_gate())
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CarrierDisposition {
    Protected,
    Foreign,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    Unknown,
}

pub(crate) struct RaiseMutationReceipt {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    uuid: Uuid,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    nonce: Uuid,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    version: u64,
}

pub(crate) enum RaiseCarrierOutcome {
    // Dormant until P05 integration; remove with real callers.
    Changed(
        SessionCommunication,
        #[allow(dead_code)] RaiseMutationReceipt,
    ),
    AlreadyVisible(SessionCommunication),
    NotRaisable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) enum RaiseRollbackOutcome {
    Reverted,
    Superseded,
    Missing,
}

// Only pure comparisons and tracker publication run at guard disposal. No
// manager callback, await, persistence or acquisition of the publication gate.
struct CarrierStateWrite<'a> {
    state: tokio::sync::RwLockWriteGuard<'a, SessionManagerState>,
    before: HashMap<Uuid, Session>,
    pending_before: HashSet<Uuid>,
    selection_before: SessionSelection,
    tracker: &'a crate::pty::manager::ResponseCloseState,
}
impl std::ops::Deref for CarrierStateWrite<'_> {
    type Target = SessionManagerState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
impl std::ops::DerefMut for CarrierStateWrite<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

fn stable_carrier_equal(a: &Session, b: &Session) -> bool {
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
impl Drop for CarrierStateWrite<'_> {
    fn drop(&mut self) {
        let incompatible: Vec<_> = self
            .state
            .prompt_keys
            .iter()
            .filter(|(id, key)| {
                self.state
                    .sessions
                    .get(id)
                    .is_none_or(|row| !key.matches(row))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in incompatible {
            self.state.prompt_keys.remove(&id);
        }
        // Clearing/fresh intent invalidates retained text in the same write.
        // Re-arming alone never copies the old row back into the cache.
        let fresh_keys: Vec<_> = self
            .state
            .sessions
            .values()
            .filter(|row| {
                row.start_fresh_on_restore
                    && self
                        .before
                        .get(&row.id)
                        .is_some_and(|old| !old.start_fresh_on_restore)
            })
            .filter_map(|row| self.state.prompt_keys.get(&row.id).cloned())
            .collect();
        for key in fresh_keys {
            let owners = self
                .state
                .prompt_keys
                .values()
                .filter(|candidate| **candidate == key)
                .count();
            self.state.retained_prompts.insert(
                key,
                RetainedPrompt {
                    last_prompt: None,
                    blocked: owners > 1,
                },
            );
        }
        let cleared_rows: Vec<_> = self
            .state
            .sessions
            .values()
            .filter(|row| {
                row.start_fresh_on_restore
                    && self
                        .before
                        .get(&row.id)
                        .is_some_and(|old| !old.start_fresh_on_restore)
            })
            .map(|row| row.id)
            .collect();
        for id in cleared_rows {
            self.state.prompt_seed_blocked.insert(id);
            if let Some(row) = self.state.sessions.get_mut(&id) {
                row.last_prompt = None;
            }
        }
        let now = std::time::Instant::now();
        let removed: Vec<Uuid> = self
            .before
            .keys()
            .filter(|id| !self.state.sessions.contains_key(id))
            .copied()
            .collect();
        for id in &removed {
            self.state.prompt_keys.remove(id);
            self.state.pending_prompts.remove(id);
            self.state.prompt_seed_blocked.remove(id);
            self.tracker.invalidate_manager_row(*id, now);
            // Retain the version tombstone: same-UUID recreation never resets it.
            if let Some(next) = self
                .state
                .carrier_versions
                .get(id)
                .copied()
                .unwrap_or(0)
                .checked_add(1)
            {
                self.state.carrier_versions.insert(*id, next);
            } else {
                self.tracker.fail_closed();
            }
            self.state.raise_nonces.remove(id);
        }
        let rows: Vec<Session> = self.state.sessions.values().cloned().collect();
        for row in rows {
            let pending = self.state.pending_create.contains_key(&row.id);
            let changed = self
                .before
                .get(&row.id)
                .is_none_or(|old| !stable_carrier_equal(old, &row))
                || self.pending_before.contains(&row.id) != pending
                || ((self.selection_before.id() != self.state.selection.id()
                    || self.selection_before.mode() != self.state.selection.mode())
                    && (self.selection_before.id() == Some(row.id)
                        || self.state.selection.id() == Some(row.id)));
            let mut version = self
                .state
                .carrier_versions
                .get(&row.id)
                .copied()
                .unwrap_or(0);
            if changed {
                self.tracker.invalidate_manager_row(row.id, now);
                let Some(next) = version.checked_add(1) else {
                    self.tracker.fail_closed();
                    continue;
                };
                version = next;
                self.state.carrier_versions.insert(row.id, version);
                self.state.raise_nonces.remove(&row.id);
            }
            let Some(generation) = self.tracker.generation(row.id) else {
                continue;
            };
            // Canonical verified target names are supplied by upper admission.
            // This publisher preserves an existing verified target; it cannot
            // promote a CWD/label into authenticated routing authority.
            self.tracker.publish_manager_row(
                row.id,
                row.token,
                generation,
                version,
                !pending
                    && !row.is_coordinator
                    && !row.is_root_agent
                    && row.agent_kind.is_some()
                    && !matches!(row.status, SessionStatus::Exited(_)),
                crate::session::session::is_working(&row),
                now,
            );
        }
    }
}

#[derive(Clone)]
pub struct SessionManager {
    response_close: crate::pty::manager::ResponseCloseState,
    state: Arc<RwLock<SessionManagerState>>,
    pub(crate) prompt_initialization: Arc<tokio::sync::OnceCell<()>>,
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

#[derive(Debug, Clone)]
pub(crate) struct CodingAgentBinding {
    pub id: String,
    pub seed: Option<serde_json::Value>,
    pub origin: Option<serde_json::Value>,
}

struct SessionManagerState {
    retained_prompts: HashMap<PromptKey, RetainedPrompt>,
    prompt_keys: HashMap<Uuid, PromptKey>,
    pending_prompts: HashMap<Uuid, PendingPrompt>,
    prompt_seed_blocked: HashSet<Uuid>,
    prompts_writable: bool,
    prompts_reusable: bool,
    coding_agent_bindings: HashMap<Uuid, String>,
    binding_seeds: HashMap<String, serde_json::Value>,
    binding_origins: HashMap<String, serde_json::Value>,
    failed_binding_recipes: HashMap<String, serde_json::Value>,
    carrier_versions: HashMap<Uuid, u64>,
    raise_nonces: HashMap<Uuid, Uuid>,
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
    pub coding_agent_bindings: HashMap<Uuid, String>,
    pub binding_seeds: HashMap<String, serde_json::Value>,
    pub binding_origins: HashMap<String, serde_json::Value>,
    pub failed_binding_recipes: HashMap<String, serde_json::Value>,
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

/// Owned restart capability; metadata remains private until the atomic commit.
#[derive(Debug)]
pub(crate) struct RestartBindingSeedTransition {
    target: PendingCreateBinding,
    retiring_runtime: Uuid,
    coding_agent_id: String,
    expected_seed: Option<serde_json::Value>,
    expected_origin: Option<serde_json::Value>,
    desired_seed: serde_json::Value,
}

#[derive(Debug, Default)]
pub(crate) struct LifecycleMutations {
    removals: Vec<Uuid>,
    mark_exited: Vec<(Uuid, i32)>,
    detached_intent: Vec<(Uuid, bool)>,
    finalizations: Vec<FinalizeMutation>,
    restart_binding: Option<RestartBindingSeedTransition>,
    prompt_transfers: Vec<(Uuid, PendingCreateBinding)>,
}

impl LifecycleMutations {
    pub(crate) fn set_restart_binding(
        &mut self,
        transition: RestartBindingSeedTransition,
    ) -> Result<(), String> {
        if self.restart_binding.is_some() {
            return Err("duplicate restart binding transition".into());
        }
        self.restart_binding = Some(transition);
        Ok(())
    }

    pub(crate) fn transfer_prompt(&mut self, source: Uuid, target: PendingCreateBinding) {
        self.prompt_transfers.push((source, target));
    }

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
    pub(crate) fn with_response_close(state: crate::pty::manager::ResponseCloseState) -> Self {
        state.bind_publisher(crate::pty::manager::ResponseClosePublisher::Manager);
        Self {
            response_close: state,
            ..Self::new()
        }
    }
    pub(crate) fn response_close_state(&self) -> crate::pty::manager::ResponseCloseState {
        self.response_close.clone()
    }
    async fn carrier_state_write<'a>(
        &'a self,
        permit: &CarrierMutationPermit,
    ) -> CarrierStateWrite<'a> {
        assert!(permit.proves(), "foreign carrier mutation permit");
        let state = self.state.write().await;
        let before = state.sessions.clone();
        let pending_before = state.pending_create.keys().copied().collect();
        let selection_before = state.selection.clone();
        CarrierStateWrite {
            state,
            before,
            pending_before,
            selection_before,
            tracker: &self.response_close,
        }
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) async fn with_current_carrier_snapshot<R>(
        &self,
        permit: &CarrierMutationPermit,
        f: impl FnOnce(&CarrierSnapshot) -> R,
    ) -> R {
        assert!(permit.proves(), "foreign carrier snapshot permit");
        let state = self.state.read().await;
        let snapshot = CarrierSnapshot {
            rows: state.sessions.values().cloned().collect(),
            versions: state.carrier_versions.clone(),
            pending_ids: state.pending_create.keys().copied().collect(),
            order: state.order.clone(),
            selection: state.selection.clone(),
        };
        f(&snapshot)
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) async fn with_response_close_facts<R>(
        &self,
        f: impl FnOnce(&CarrierSnapshot) -> R,
    ) -> R {
        let state = self.state.read().await;
        let snapshot = CarrierSnapshot {
            rows: state.sessions.values().cloned().collect(),
            versions: state.carrier_versions.clone(),
            pending_ids: state.pending_create.keys().copied().collect(),
            order: state.order.clone(),
            selection: state.selection.clone(),
        };
        f(&snapshot)
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn reserve_carrier_create_binding(&self) -> PendingCreateBinding {
        PendingCreateBinding::new(Uuid::new_v4(), Uuid::new_v4())
    }
    pub fn new() -> Self {
        let epoch = Uuid::new_v4();
        Self {
            response_close: crate::pty::manager::ResponseCloseState::default(),
            prompt_initialization: Arc::new(tokio::sync::OnceCell::new()),
            state: Arc::new(RwLock::new(SessionManagerState {
                retained_prompts: HashMap::new(),
                prompt_keys: HashMap::new(),
                pending_prompts: HashMap::new(),
                prompt_seed_blocked: HashSet::new(),
                prompts_writable: false,
                prompts_reusable: false,
                coding_agent_bindings: HashMap::new(),
                binding_seeds: HashMap::new(),
                binding_origins: HashMap::new(),
                failed_binding_recipes: HashMap::new(),
                carrier_versions: HashMap::new(),
                raise_nonces: HashMap::new(),
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.insert_new_pending_session_with_carrier_permit(
            &carrier_permit,
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
    async fn insert_new_pending_session_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
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
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.sessions.contains_key(&id) {
            return Err(AppError::Other(
                "pending create binding already used".to_string(),
            ));
        }
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
            match_tier: None,
            original_profile_letter: None,
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

    #[allow(clippy::too_many_arguments)]
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) async fn create_pending_session_with_binding_and_carrier_permit(
        &self,
        binding: PendingCreateBinding,
        carrier_permit: &CarrierMutationPermit,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<Session, AppError> {
        self.insert_new_pending_session_with_carrier_permit(
            carrier_permit,
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

    pub async fn rename_session(&self, id: Uuid, name: String) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.rename_session_with_carrier_permit(&carrier_permit, id, name)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn rename_session_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        name: String,
    ) -> Result<(), AppError> {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.rename_pending_session_with_carrier_permit(&carrier_permit, binding, name)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn rename_pending_session_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        name: String,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.name = name
        })
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.mark_idle_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn mark_idle_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) {
        let record = {
            let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.mark_busy_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn mark_busy_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) {
        let record = {
            let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.arm_agent_turn_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn arm_agent_turn_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.prepare_pty_input_boundary_with_carrier_permit(
            &carrier_permit,
            id,
            expected_target,
            expected_backend,
            authority_id,
            expected_sender,
            authority_backend,
            authority_route,
            permit,
            idle_detector,
            settings,
            final_recipe_check,
            final_external_check,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_pty_input_boundary_with_carrier_permit<'a, F, G>(
        &self,
        carrier_permit: &CarrierMutationPermit,
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
                .try_current_read_owned()
                .map_err(|_| Failure::RouteUnavailable)?;
            let mut state = self.carrier_state_write(carrier_permit).await;
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
        // #1682 - the privileged exact-agent-input plane ARMS the turn here, and the
        // clear keys on that arming, not on proven delivery: R7 is the branch that
        // arms without delivering. It never reaches `mark_successful_pty_write_busy`,
        // so it sets no mark of its own; this cancels one a preceding control write
        // left, so a focus report before a `send --pty-input` cannot suppress its edge.
        idle_detector.clear_control_write(id);
        Ok(route_guard)
    }

    pub async fn set_last_prompt(&self, id: Uuid, prompt: String) {
        let permit = acquire_carrier_mutation_permit().await;
        let mut state = self.carrier_state_write(&permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.last_prompt = Some(prompt.clone());
            state.prompt_seed_blocked.remove(&id);
            if let Some(key) = state.prompt_keys.get(&id).cloned() {
                let exclusive = prompt_domain_exclusive(&state, &key, Some(id));
                let entry = state.retained_prompts.entry(key).or_default();
                if exclusive && !entry.blocked {
                    entry.last_prompt = Some(prompt);
                } else {
                    entry.last_prompt = None;
                    entry.blocked = true;
                }
            }
        }
    }

    pub(crate) async fn install_retained_prompts(
        &self,
        entries: Option<HashMap<PromptKey, RetainedPrompt>>,
    ) {
        let mut state = self.state.write().await;
        state.prompts_writable = entries.is_some();
        state.prompts_reusable = entries.is_some();
        state.retained_prompts = entries.unwrap_or_default();
    }

    pub(crate) async fn retained_prompt_snapshot(
        &self,
    ) -> Option<HashMap<PromptKey, RetainedPrompt>> {
        let state = self.state.read().await;
        state
            .prompts_writable
            .then(|| state.retained_prompts.clone())
    }

    pub(crate) async fn disable_prompt_reuse(&self) {
        self.state.write().await.prompts_reusable = false;
    }

    pub(crate) async fn forget_retained_prompt(&self, key: &PromptKey) {
        self.state.write().await.retained_prompts.remove(key);
    }

    pub(crate) async fn block_retained_prompt_domains(&self, keys: Vec<PromptKey>) {
        let mut state = self.state.write().await;
        for key in keys {
            state.retained_prompts.insert(
                key,
                RetainedPrompt {
                    last_prompt: None,
                    blocked: true,
                },
            );
        }
    }

    pub(crate) async fn initialize_pending_prompt(
        &self,
        binding: PendingCreateBinding,
        key: Option<PromptKey>,
        initial: Option<String>,
        continuation: bool,
        fresh: bool,
        allow_retained_lookup: bool,
    ) -> Result<(), String> {
        let permit = acquire_carrier_mutation_permit().await;
        let mut state = self.carrier_state_write(&permit).await;
        if !binding_matches(&state, binding) {
            return Err("pending prompt capability mismatch".to_string());
        }
        let row = state
            .sessions
            .get_mut(&binding.session_id)
            .ok_or_else(|| "pending prompt row missing".to_string())?;
        row.last_prompt = if continuation && !fresh {
            initial
        } else {
            None
        };
        if let Some(key) = key.filter(|key| key.matches(row)) {
            state.pending_prompts.insert(
                binding.session_id,
                PendingPrompt {
                    key,
                    continuation,
                    fresh,
                    allow_retained_lookup,
                },
            );
        }
        Ok(())
    }

    pub(crate) async fn seed_if_absent_with_carrier_permit(
        &self,
        permit: &CarrierMutationPermit,
        id: Uuid,
        key: &PromptKey,
        prompt: String,
    ) -> bool {
        let mut state = self.carrier_state_write(permit).await;
        if state.pending_create.contains_key(&id)
            || state.prompt_seed_blocked.contains(&id)
            || !prompt_domain_exclusive(&state, key, Some(id))
            || state
                .retained_prompts
                .get(key)
                .is_some_and(|entry| entry.blocked)
        {
            return false;
        }
        let Some(row) = state.sessions.get_mut(&id) else {
            return false;
        };
        if !key.matches(row) || row.start_fresh_on_restore || row.last_prompt.is_some() {
            return false;
        }
        row.last_prompt = Some(prompt.clone());
        state.prompt_keys.insert(id, key.clone());
        state.retained_prompts.insert(
            key.clone(),
            RetainedPrompt {
                last_prompt: Some(prompt),
                blocked: false,
            },
        );
        true
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        match self
            .raise_hand_with_carrier_permit(&carrier_permit, id, updated_at)
            .await
        {
            RaiseCarrierOutcome::Changed(communication, _) => Some((true, communication)),
            RaiseCarrierOutcome::AlreadyVisible(communication) => Some((false, communication)),
            RaiseCarrierOutcome::NotRaisable => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn raise_hand_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> RaiseCarrierOutcome {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return RaiseCarrierOutcome::NotRaisable;
        }
        let Some(session) = state.sessions.get_mut(&id) else {
            return RaiseCarrierOutcome::NotRaisable;
        };
        if !session.is_coordinator || matches!(session.status, SessionStatus::Exited(_)) {
            return RaiseCarrierOutcome::NotRaisable;
        }

        if let Some(existing) = session.communication.as_ref() {
            if existing.kind == SessionCommunicationKind::RaiseHand && existing.visible {
                return RaiseCarrierOutcome::AlreadyVisible(existing.clone());
            }
        }

        let communication = SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: updated_at.to_rfc3339(),
            message: None,
        };
        session.communication = Some(communication.clone());
        drop(state);
        let mut state = self.state.write().await;
        let nonce = Uuid::new_v4();
        let version = state.carrier_versions.get(&id).copied().unwrap_or(0);
        state.raise_nonces.insert(id, nonce);
        RaiseCarrierOutcome::Changed(
            communication,
            RaiseMutationReceipt {
                uuid: id,
                nonce,
                version,
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) async fn rollback_raise_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        receipt: RaiseMutationReceipt,
    ) -> RaiseRollbackOutcome {
        let mut state = self.carrier_state_write(carrier_permit).await;
        let Some(session) = state.sessions.get(&receipt.uuid) else {
            return RaiseRollbackOutcome::Missing;
        };
        if state.raise_nonces.get(&receipt.uuid) != Some(&receipt.nonce)
            || state.carrier_versions.get(&receipt.uuid) != Some(&receipt.version)
            || !session.communication.as_ref().is_some_and(|comm| {
                comm.kind == SessionCommunicationKind::RaiseHand && comm.visible
            })
        {
            return RaiseRollbackOutcome::Superseded;
        }
        state.sessions.get_mut(&receipt.uuid).unwrap().communication = None;
        state.raise_nonces.remove(&receipt.uuid);
        RaiseRollbackOutcome::Reverted
    }

    pub async fn clear_communication_if_kind(
        &self,
        id: Uuid,
        kind: SessionCommunicationKind,
    ) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_communication_if_kind_with_carrier_permit(&carrier_permit, id, kind)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_communication_if_kind_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        kind: SessionCommunicationKind,
    ) -> bool {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_blocked_menu_with_carrier_permit(&carrier_permit, id, message, updated_at)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_blocked_menu_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        message: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_blocked_menu_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_blocked_menu_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) -> bool {
        self.clear_communication_if_kind_with_carrier_permit(
            carrier_permit,
            id,
            SessionCommunicationKind::BlockedMenu,
        )
        .await
    }

    /// #2232 phase 7 - sets `session.communication` to `CoManaged` with the
    /// visible text the Co-managed feature produced (reason, excerpt and the
    /// messaging-file path). Same shape and dedup rule as `set_blocked_menu`:
    /// re-setting the identical visible message reports `changed == false`.
    ///
    /// The slot is single and shared with `raise_hand` / `set_blocked_menu`:
    /// the newer writer overwrites, which is the documented phase-7 behaviour.
    pub async fn set_co_managed(
        &self,
        id: Uuid,
        message: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_co_managed_with_carrier_permit(&carrier_permit, id, message, updated_at)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_co_managed_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        message: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return None;
        }
        let session = state.sessions.get_mut(&id)?;
        if matches!(session.status, SessionStatus::Exited(_)) {
            return None;
        }

        if let Some(existing) = session.communication.as_ref() {
            if existing.kind == SessionCommunicationKind::CoManaged
                && existing.visible
                && existing.message.as_deref() == Some(&message)
            {
                return Some((false, existing.clone()));
            }
        }

        let communication = SessionCommunication {
            kind: SessionCommunicationKind::CoManaged,
            visible: true,
            updated_at: updated_at.to_rfc3339(),
            message: Some(message),
        };
        session.communication = Some(communication.clone());
        Some((true, communication))
    }

    /// #2232 phase 7 - clears the session's communication **only when the
    /// single slot currently holds a visible Co-managed message**, leaving any
    /// other kind untouched; returns whether it cleared one.
    ///
    /// Nothing in production calls this today. Plan section 7 makes the
    /// Co-managed communication the user's reading surface (reason, excerpt
    /// and message-file path) and gives the slot the single-slot overwrite
    /// lifecycle: `raise_hand`, `set_blocked_menu` or a newer Co-managed
    /// message replaces the current entry. The readiness-loss clear of plan
    /// 9.3 clears the armed flag and emits its event; it deliberately does not
    /// erase this user-visible pointer while the referenced message file stays
    /// on disk.
    pub async fn clear_co_managed(&self, id: Uuid) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_co_managed_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_co_managed_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) -> bool {
        self.clear_communication_if_kind_with_carrier_permit(
            carrier_permit,
            id,
            SessionCommunicationKind::CoManaged,
        )
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.restore_communication_with_carrier_permit(&carrier_permit, id, communication)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn restore_communication_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        communication: SessionCommunication,
    ) -> bool {
        if communication.kind != SessionCommunicationKind::RaiseHand || !communication.visible {
            return false;
        }
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_start_fresh_on_restore_with_carrier_permit(&carrier_permit, id, value)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_start_fresh_on_restore_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        value: bool,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_start_fresh_on_restore_with_carrier_permit(&carrier_permit, binding, value)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_start_fresh_on_restore_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        value: bool,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.start_fresh_on_restore = value;
        })
        .await
    }

    pub(crate) async fn set_pending_communication(
        &self,
        binding: PendingCreateBinding,
        communication: SessionCommunication,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_communication_with_carrier_permit(&carrier_permit, binding, communication)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_communication_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        communication: SessionCommunication,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.communication = Some(communication);
        })
        .await
    }

    /// (#630/#631) Re-arm on first real user message: clear the fresh intent.
    /// Returns true ONLY on the `true -> false` transition, so the caller persists
    /// exactly once (not on every subsequent keystroke).
    pub async fn clear_start_fresh_on_restore_if_set(&self, id: Uuid) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_start_fresh_on_restore_if_set_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_start_fresh_on_restore_if_set_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) -> bool {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_start_fresh_on_restore_if_unset_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_start_fresh_on_restore_if_unset_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) -> bool {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_user_input_transitions_with_carrier_permit(&carrier_permit, id, clear_fresh)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_user_input_transitions_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        clear_fresh: bool,
    ) -> (bool, bool) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_agent_kind_with_carrier_permit(&carrier_permit, id, kind)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_agent_kind_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        kind: Option<CodingAgentKind>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_agent_kind_with_carrier_permit(&carrier_permit, binding, kind)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_agent_kind_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        kind: Option<CodingAgentKind>,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.agent_kind = kind
        })
        .await
    }

    pub(crate) async fn set_pending_trusted_configured_spawn(
        &self,
        binding: PendingCreateBinding,
        trusted: bool,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_trusted_configured_spawn_with_carrier_permit(
            &carrier_permit,
            binding,
            trusted,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_trusted_configured_spawn_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        trusted: bool,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
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
        match_tier: Option<String>,
        original_profile_letter: Option<String>,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_profile_metadata_with_carrier_permit(
            &carrier_permit,
            id,
            requested_profile,
            effective_profile,
            profile_fallback_chain,
            profile_fallback_applied,
            match_tier,
            original_profile_letter,
            effective_codex_home,
            profile_content_hash,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_profile_metadata_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        requested_profile: Option<String>,
        effective_profile: Option<String>,
        profile_fallback_chain: Vec<String>,
        profile_fallback_applied: bool,
        match_tier: Option<String>,
        original_profile_letter: Option<String>,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.requested_profile = requested_profile;
            s.effective_profile = effective_profile;
            s.profile_fallback_chain = profile_fallback_chain;
            s.profile_fallback_applied = profile_fallback_applied;
            s.match_tier = match_tier;
            s.original_profile_letter = original_profile_letter;
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
        match_tier: Option<String>,
        original_profile_letter: Option<String>,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_profile_metadata_with_carrier_permit(
            &carrier_permit,
            binding,
            requested_profile,
            effective_profile,
            profile_fallback_chain,
            profile_fallback_applied,
            match_tier,
            original_profile_letter,
            effective_codex_home,
            profile_content_hash,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_profile_metadata_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        requested_profile: Option<String>,
        effective_profile: Option<String>,
        profile_fallback_chain: Vec<String>,
        profile_fallback_applied: bool,
        match_tier: Option<String>,
        original_profile_letter: Option<String>,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.requested_profile = requested_profile;
            session.effective_profile = effective_profile;
            session.profile_fallback_chain = profile_fallback_chain;
            session.profile_fallback_applied = profile_fallback_applied;
            session.match_tier = match_tier;
            session.original_profile_letter = original_profile_letter;
            session.effective_codex_home = effective_codex_home;
            session.profile_content_hash = profile_content_hash;
        })
        .await
    }

    pub async fn set_resolved_claude_projects_dir(&self, id: Uuid, path: Option<PathBuf>) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_resolved_claude_projects_dir_with_carrier_permit(&carrier_permit, id, path)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_resolved_claude_projects_dir_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        path: Option<PathBuf>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_resolved_claude_projects_dir_with_carrier_permit(
            &carrier_permit,
            binding,
            path,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_resolved_claude_projects_dir_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        path: Option<PathBuf>,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.resolved_claude_projects_dir = path;
        })
        .await
    }

    /// Persisted Telegram ON/OFF state for the session. Some(bot_id) means the
    /// bridge should be reattached when the session is restored or woken.
    pub async fn set_telegram_bot_id(&self, id: Uuid, bot_id: Option<String>) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_telegram_bot_id_with_carrier_permit(&carrier_permit, id, bot_id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_telegram_bot_id_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        bot_id: Option<String>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_was_detached_with_carrier_permit(&carrier_permit, id, detached)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_was_detached_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        detached: bool,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_effective_shell_args_with_carrier_permit(&carrier_permit, id, args)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_effective_shell_args_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        args: Vec<String>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_effective_shell_args_with_carrier_permit(&carrier_permit, binding, args)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_effective_shell_args_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        args: Vec<String>,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.effective_shell_args = Some(args);
        })
        .await
    }

    /// Overwrite `git_repos` atomically. Bumps `git_repos_gen`. Invariant:
    /// callers preserve insertion order (replica config.json `repos` array order).
    pub async fn set_git_repos(&self, id: Uuid, repos: Vec<SessionRepo>) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_git_repos_with_carrier_permit(&carrier_permit, id, repos)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_git_repos_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        repos: Vec<SessionRepo>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_git_repos_if_gen_with_carrier_permit(&carrier_permit, id, repos, expected_gen)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_git_repos_if_gen_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        repos: Vec<SessionRepo>,
        expected_gen: u64,
    ) -> bool {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_is_coordinator_with_carrier_permit(&carrier_permit, id, is_coordinator)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_is_coordinator_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        is_coordinator: bool,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.refresh_coordinator_flags_with_carrier_permit(&carrier_permit, teams)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn refresh_coordinator_flags_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        teams: &[crate::config::teams::DiscoveredTeam],
    ) -> Vec<(Uuid, bool)> {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.refresh_git_repos_for_sessions_with_carrier_permit(&carrier_permit, updates)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn refresh_git_repos_for_sessions_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        updates: &[(String, Vec<SessionRepo>)],
    ) -> Vec<(Uuid, Vec<SessionRepo>)> {
        let mut state = self.carrier_state_write(carrier_permit).await;
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

    /// Only pending rows accept a birth binding; capture its seed before insertion.
    pub(crate) async fn install_pending_coding_agent_binding(
        &self,
        pending: PendingCreateBinding,
        binding: Option<CodingAgentBinding>,
    ) -> Result<(), String> {
        let permit = acquire_carrier_mutation_permit().await;
        let mut state = self.carrier_state_write(&permit).await;
        if !binding_matches(&state, pending) {
            return Err("pending coding-agent binding capability mismatch".to_string());
        }
        if let Some(binding) = binding {
            Uuid::parse_str(&binding.id).map_err(|_| "invalid_coding_agent_binding".to_string())?;
            if let Some(seed) = binding.seed.as_ref() {
                let valid_seed = ["identityRevision", "originalAgentId"].iter().all(|key| {
                    seed.get(key)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| !value.is_empty())
                });
                if !valid_seed {
                    return Err("invalid_coding_agent_binding_seed".to_string());
                }
            }
            if let (Some(before), Some(after)) =
                (state.binding_seeds.get(&binding.id), binding.seed.as_ref())
            {
                if before != after {
                    return Err("coding_agent_binding_seed_conflict".to_string());
                }
            }
            if let Some(seed) = binding.seed {
                state.binding_seeds.insert(binding.id.clone(), seed);
            }
            if let Some(origin) = binding.origin {
                state.binding_origins.insert(binding.id.clone(), origin);
            }
            state
                .coding_agent_bindings
                .insert(pending.session_id, binding.id);
        }
        Ok(())
    }

    /// Stage only the pending runtime association; seed, origin and recipes stay intact.
    pub(crate) async fn stage_pending_restart_binding(
        &self,
        target: PendingCreateBinding,
        retiring_runtime: Uuid,
        expected: CodingAgentBinding,
        desired_seed: serde_json::Value,
    ) -> Result<RestartBindingSeedTransition, String> {
        let permit = acquire_carrier_mutation_permit().await;
        let mut state = self.carrier_state_write(&permit).await;
        let transition = RestartBindingSeedTransition {
            target,
            retiring_runtime,
            coding_agent_id: expected.id,
            expected_seed: expected.seed,
            expected_origin: expected.origin,
            desired_seed,
        };
        validate_restart_binding(&state, &transition, false)?;
        state
            .coding_agent_bindings
            .insert(target.session_id, transition.coding_agent_id.clone());
        Ok(transition)
    }

    pub(crate) async fn coding_agent_binding(&self, id: Uuid) -> Option<CodingAgentBinding> {
        let state = self.state.read().await;
        let binding = state.coding_agent_bindings.get(&id)?;
        Some(CodingAgentBinding {
            id: binding.clone(),
            seed: state.binding_seeds.get(binding).cloned(),
            origin: state.binding_origins.get(binding).cloned(),
        })
    }

    /// D09 enrolls receipt-affected failures; ordinary retention stays unchanged.
    pub(crate) async fn refresh_failed_binding_recipes(
        &self,
        recipes: Vec<(CodingAgentBinding, serde_json::Value)>,
    ) {
        let permit = acquire_carrier_mutation_permit().await;
        let mut state = self.carrier_state_write(&permit).await;
        for (binding, recipe) in recipes {
            if let Some(seed) = binding.seed {
                state
                    .binding_seeds
                    .entry(binding.id.clone())
                    .or_insert(seed);
            }
            if let Some(origin) = binding.origin {
                state.binding_origins.insert(binding.id.clone(), origin);
            }
            if let Some(affected) = state.failed_binding_recipes.get_mut(&binding.id) {
                *affected = recipe;
            }
        }
    }

    pub(crate) async fn contains_public_or_pending(&self, id: Uuid) -> bool {
        self.state.read().await.sessions.contains_key(&id)
    }

    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    async fn update_pending<F>(
        &self,
        binding: PendingCreateBinding,
        update: F,
    ) -> Result<(), AppError>
    where
        F: FnOnce(&mut Session),
    {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.update_pending_with_carrier_permit(&carrier_permit, binding, update)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn update_pending_with_carrier_permit<F>(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        update: F,
    ) -> Result<(), AppError>
    where
        F: FnOnce(&mut Session),
    {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.rollback_pending_create_with_carrier_permit(&carrier_permit, binding)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn rollback_pending_create_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        state.pending_prompts.remove(&binding.session_id);
        state.sessions.remove(&binding.session_id);
        retire_coding_agent_binding(&mut state, binding.session_id);
        state.order.retain(|id| *id != binding.session_id);
        Ok(())
    }

    pub(super) async fn insert_transaction_pending_record(
        &self,
        session: Session,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.insert_transaction_pending_record_with_carrier_permit(
            &carrier_permit,
            session,
            binding,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn insert_transaction_pending_record_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        session: Session,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        if session.id != binding.session_id {
            return Err("transaction pending binding targets another session".to_string());
        }
        let mut state = self.carrier_state_write(carrier_permit).await;
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
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.commit_selection_transition_with_carrier_permit(
            &carrier_permit,
            _capability,
            decision,
            cause,
            mutations,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn commit_selection_transition_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        _capability: &CommitCapability,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        let old_selection = state.selection.clone();
        let old_id = old_selection.id();
        let old_mode = old_selection.mode();

        let lifecycle_sets = validate_lifecycle_mutations(&state, &mutations)?;

        let (target_mode, target_id, target_status) = resolve_selection_commit_target(
            &state,
            decision,
            &mutations,
            &lifecycle_sets,
            &old_selection,
            old_id,
            old_mode,
        )?;

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

        // Prevalidate the projection while the same guard protects application.
        match (target_mode, target_id, target_status.as_ref()) {
            (SelectionMode::None, None, _) => {}
            (SelectionMode::Live, Some(id), _) if state.sessions.contains_key(&id) => {}
            (SelectionMode::Dormant, Some(id), Some(SessionStatus::Exited(_)))
                if state.sessions.contains_key(&id) => {}
            _ => return Err("invalid canonical selection target".into()),
        }
        if let Some(transition) = mutations.restart_binding.as_ref() {
            validate_restart_binding(&state, transition, true)?;
            if !mutations.removals.contains(&transition.retiring_runtime)
                || !mutations.finalizations.iter().any(|mutation| {
                    matches!(mutation,
                    FinalizeMutation::Live(binding, _) if *binding == transition.target)
                })
            {
                return Err("restart binding requires atomic replacement".into());
            }
        }

        apply_commit_prompt_domains(&mut state, &mutations, cause);
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
                    clear_failed_recipe_for_runtime_binding(&mut state, binding.session_id);
                    if let Some(record) = state.sessions.get_mut(&binding.session_id) {
                        record.status = SessionStatus::Running;
                        finalized_live_ids.push(binding.session_id);
                        result.finalized_rows.push(SessionInfo::from(&*record));
                    }
                }
                FinalizeMutation::Dormant(binding, _, exit_code) => {
                    state.pending_create.remove(&binding.session_id);
                    clear_failed_recipe_for_runtime_binding(&mut state, binding.session_id);
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
                state.prompt_keys.remove(&id);
                state.prompt_seed_blocked.remove(&id);
                state.pending_prompts.remove(&id);
                state.pending_create.remove(&id);
                retire_coding_agent_binding(&mut state, id);
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
                        .expect("live selection target prevalidated under the same guard");
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
                _ => unreachable!("canonical selection projection prevalidated"),
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
        append_finalized_live_activity(&state, finalized_live_ids, &mut result);

        // Last publication: no fallible operation or await follows.
        if let Some(transition) = mutations.restart_binding {
            state
                .binding_seeds
                .insert(transition.coding_agent_id, transition.desired_seed);
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
        let permit = acquire_carrier_mutation_permit().await;
        self.remove_exited_sessions_with_carrier_permit(ids, &permit, |_, _| {
            CarrierDisposition::Foreign
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn remove_exited_sessions_with_carrier_permit(
        &self,
        ids: &[Uuid],
        permit: &CarrierMutationPermit,
        mut classify: impl FnMut(&Session, u64) -> CarrierDisposition,
    ) -> usize {
        let mut state = self.carrier_state_write(permit).await;
        let dispositions: HashMap<Uuid, CarrierDisposition> = state
            .sessions
            .iter()
            .map(|(id, row)| {
                (
                    *id,
                    classify(row, state.carrier_versions.get(id).copied().unwrap_or(0)),
                )
            })
            .collect();
        let clear_surviving = state.selection.id().is_some_and(|id| {
            ids.contains(&id) && dispositions.get(&id) == Some(&CarrierDisposition::Protected)
        });
        let will_remove_selected = state.selection.id().is_some_and(|id| {
            ids.contains(&id)
                && dispositions.get(&id) == Some(&CarrierDisposition::Foreign)
                && !state.pending_create.contains_key(&id)
                && state
                    .sessions
                    .get(&id)
                    .is_some_and(|row| matches!(row.status, SessionStatus::Exited(_)))
        });
        let next_revision = if clear_surviving || will_remove_selected {
            let Some(next) = state.revision.checked_add(1) else {
                self.response_close.fail_closed();
                return 0;
            };
            next
        } else {
            state.revision
        };
        let mut removed = 0usize;
        let mut removed_selection = clear_surviving;
        let mut removed_ids: Vec<Uuid> = Vec::new();
        for &id in ids {
            if dispositions.get(&id) != Some(&CarrierDisposition::Foreign) {
                continue;
            }
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
            retire_coding_agent_binding(&mut state, id);
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
            if clear_surviving {
                if let Some(id) = state.selection.id() {
                    if let Some(row) = state.sessions.get_mut(&id) {
                        if row.status == SessionStatus::Active {
                            row.status = SessionStatus::Running;
                        }
                    }
                }
            }
            state.revision = next_revision;
            let next = state.order.iter().copied().find(|candidate| {
                !state.pending_create.contains_key(candidate)
                    && dispositions.get(candidate) == Some(&CarrierDisposition::Foreign)
            });
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

fn append_finalized_live_activity(
    state: &SessionManagerState,
    finalized_live_ids: Vec<Uuid>,
    result: &mut CommitResult,
) {
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
}

/// Lifecycle mutation sets in removals, exits, finalizations order.
type LifecycleMutationSets = (HashSet<Uuid>, HashSet<Uuid>, HashSet<Uuid>);

fn validate_restart_binding(
    state: &SessionManagerState,
    transition: &RestartBindingSeedTransition,
    staged: bool,
) -> Result<(), String> {
    let id = &transition.coding_agent_id;
    if Uuid::parse_str(id).is_err()
        || !["identityRevision", "originalAgentId"].iter().all(|key| {
            transition
                .desired_seed
                .get(key)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
    {
        return Err("invalid_coding_agent_binding_seed".into());
    }
    if !binding_matches(state, transition.target)
        || transition.target.session_id == transition.retiring_runtime
        || !state.sessions.contains_key(&transition.retiring_runtime)
        || state
            .pending_create
            .contains_key(&transition.retiring_runtime)
        || state
            .coding_agent_bindings
            .get(&transition.retiring_runtime)
            != Some(id)
        || state
            .coding_agent_bindings
            .get(&transition.target.session_id)
            != if staged { Some(id) } else { None }
        || state
            .coding_agent_bindings
            .iter()
            .any(|(runtime, candidate)| {
                candidate == id
                    && *runtime != transition.retiring_runtime
                    && (!staged || *runtime != transition.target.session_id)
            })
    {
        return Err("restart coding-agent binding owner/capability mismatch".into());
    }
    if state.binding_seeds.get(id) != transition.expected_seed.as_ref()
        || state.binding_origins.get(id) != transition.expected_origin.as_ref()
    {
        return Err("restart coding-agent binding metadata changed".into());
    }
    Ok(())
}

fn prompt_domain_exclusive(
    state: &SessionManagerState,
    key: &PromptKey,
    owner: Option<Uuid>,
) -> bool {
    state
        .prompt_keys
        .iter()
        .all(|(id, candidate)| Some(*id) == owner || candidate != key)
}

fn apply_commit_prompt_domains(
    state: &mut SessionManagerState,
    mutations: &LifecycleMutations,
    cause: SelectionCause,
) {
    let mut predecessors: HashMap<_, _> = mutations
        .prompt_transfers
        .iter()
        .map(|(source, target)| (target.session_id, *source))
        .collect();
    // Fresh restart is also one succession, even though it transfers no text.
    if matches!(cause, SelectionCause::Restart(_)) {
        for mutation in &mutations.finalizations {
            let binding = match mutation {
                FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                    binding
                }
            };
            if let Some(target) = state.pending_prompts.get(&binding.session_id) {
                if let Some(source) = mutations
                    .removals
                    .iter()
                    .find(|id| state.prompt_keys.get(id) == Some(&target.key))
                {
                    predecessors.insert(binding.session_id, *source);
                }
            }
        }
    }
    for (source, target) in &mutations.prompt_transfers {
        let latest = state
            .sessions
            .get(source)
            .and_then(|row| row.last_prompt.clone());
        state
            .sessions
            .get_mut(&target.session_id)
            .expect("validated pending target")
            .last_prompt = latest;
    }
    for mutation in &mutations.finalizations {
        let binding = match mutation {
            FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                *binding
            }
        };
        finalize_prompt_domain(
            state,
            binding.session_id,
            predecessors.get(&binding.session_id).copied(),
            !matches!(cause, SelectionCause::Restore),
        );
    }
}

fn finalize_prompt_domain(
    state: &mut SessionManagerState,
    id: Uuid,
    predecessor: Option<Uuid>,
    allow_fresh_reopen: bool,
) {
    let Some(prepared) = state.pending_prompts.remove(&id) else {
        return;
    };
    let Some(row) = state.sessions.get(&id) else {
        return;
    };
    if !prepared.key.matches(row) {
        return;
    }
    let exclusive = prompt_domain_exclusive(state, &prepared.key, predecessor);
    let entry = state
        .retained_prompts
        .entry(prepared.key.clone())
        .or_default();
    let row = state.sessions.get_mut(&id).expect("pending row exists");
    if !exclusive {
        entry.blocked = true;
        entry.last_prompt = None;
    } else if !prepared.continuation && !prepared.fresh {
        // Admission reserves this pair for configured, unattributed identity.
        // A durable fresh marker cannot establish that external selector.
        entry.blocked = true;
        entry.last_prompt = None;
        row.last_prompt = None;
    } else if prepared.fresh || row.start_fresh_on_restore {
        state.prompt_seed_blocked.insert(id);
        // Startup fresh markers cannot disprove persisted duplicate recipes
        // whose manager rows have not been restored yet.
        if allow_fresh_reopen {
            entry.blocked = false;
        }
        entry.last_prompt = None;
        row.last_prompt = None;
    } else if prepared.continuation {
        if prepared.allow_retained_lookup
            && row.last_prompt.is_none()
            && !entry.blocked
            && state.prompts_reusable
        {
            row.last_prompt = entry.last_prompt.clone();
        }
        if !entry.blocked && row.last_prompt.is_some() {
            entry.last_prompt = row.last_prompt.clone();
        }
    }
    state.prompt_keys.insert(id, prepared.key);
    if let Some(source) = predecessor {
        state.prompt_keys.remove(&source);
    }
}

fn validate_lifecycle_mutations(
    state: &SessionManagerState,
    mutations: &LifecycleMutations,
) -> Result<LifecycleMutationSets, String> {
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
                FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                    binding.session_id
                }
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
                if !binding_matches(state, binding) {
                    return Err(format!(
                        "create finalization capability mismatch: {}",
                        binding.session_id
                    ));
                }
                if witness.session_id != binding.session_id || !witness.has_pty || witness.detached
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
                if !binding_matches(state, binding)
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
    validate_unique_ids(
        "prompt transfer source",
        mutations.prompt_transfers.iter().map(|(id, _)| *id),
    )?;
    validate_unique_ids(
        "prompt transfer target",
        mutations
            .prompt_transfers
            .iter()
            .map(|(_, binding)| binding.session_id),
    )?;
    for (source, target) in &mutations.prompt_transfers {
        let from = state
            .sessions
            .get(source)
            .ok_or("prompt transfer source missing")?;
        let to = state
            .sessions
            .get(&target.session_id)
            .ok_or("prompt transfer target missing")?;
        if !removals.contains(source)
            || !finalizations.contains(&target.session_id)
            || !binding_matches(state, *target)
            || from.start_fresh_on_restore
            || to.start_fresh_on_restore
            || from.agent_id != to.agent_id
            || from.requested_profile != to.requested_profile
            || from.effective_profile != to.effective_profile
            || from.working_directory != to.working_directory
        {
            return Err("invalid prompt transfer".to_string());
        }
        if let (Some(from), Some(to)) = (
            state.prompt_keys.get(source),
            state.pending_prompts.get(&target.session_id),
        ) {
            if *from != to.key || !to.continuation || to.fresh {
                return Err("incompatible prompt transfer domain".to_string());
            }
        }
    }
    Ok((removals, exits, finalizations))
}

fn resolve_selection_commit_target(
    state: &SessionManagerState,
    decision: CommitDecision,
    mutations: &LifecycleMutations,
    lifecycle_sets: &LifecycleMutationSets,
    old_selection: &SessionSelection,
    old_id: Option<Uuid>,
    old_mode: SelectionMode,
) -> Result<(SelectionMode, Option<Uuid>, Option<SessionStatus>), String> {
    let (removals, exits, finalizations) = lifecycle_sets;

    let projected_status = |id: Uuid| -> Option<SessionStatus> {
        if removals.contains(&id) {
            return None;
        }
        if let Some(mutation) = mutations
            .finalizations
            .iter()
            .find(|mutation| match mutation {
                FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                    binding.session_id == id
                }
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
                return Err("Keep cannot remove, exit, or detach the selected record".to_string());
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
            let status =
                projected_status(id).ok_or_else(|| format!("dormant target is missing: {id}"))?;
            if !matches!(status, SessionStatus::Exited(_)) {
                return Err(format!("dormant target is not exited: {id}"));
            }
            (SelectionMode::Dormant, Some(id), Some(status))
        }
    };

    Ok((target_mode, target_id, target_status))
}

fn clear_failed_recipe_for_runtime_binding(state: &mut SessionManagerState, runtime: Uuid) {
    if let Some(id) = state.coding_agent_bindings.get(&runtime).cloned() {
        state.failed_binding_recipes.remove(&id);
    }
}

fn retire_coding_agent_binding(state: &mut SessionManagerState, runtime: Uuid) {
    if let Some(binding) = state.coding_agent_bindings.remove(&runtime) {
        if !state
            .coding_agent_bindings
            .values()
            .any(|id| id == &binding)
            && !state.failed_binding_recipes.contains_key(&binding)
        {
            state.binding_seeds.remove(&binding);
            state.binding_origins.remove(&binding);
        }
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
    pub(crate) async fn enroll_failed_binding_recipe_for_test(
        &self,
        binding: String,
        recipe: serde_json::Value,
    ) {
        self.state
            .write()
            .await
            .failed_binding_recipes
            .insert(binding, recipe);
    }

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
            match_tier: None,
            original_profile_letter: None,
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
        retire_coding_agent_binding(&mut state, id);
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

    async fn issue_3145_pending(
        manager: &SessionManager,
        cwd: &str,
        initial: Option<&str>,
        fresh: bool,
    ) -> (Session, PendingCreateBinding, PromptKey) {
        let (row, binding) = manager
            .create_transaction_pending_session(
                "codex".into(),
                Vec::new(),
                cwd.into(),
                Some("codex".into()),
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        manager
            .set_pending_profile_metadata(
                binding,
                Some("A".into()),
                Some("B".into()),
                Vec::new(),
                true,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let key = PromptKey {
            canonical_cwd: cwd.into(),
            agent_id: "codex".into(),
            requested_profile: Some("A".into()),
            effective_profile: "B".into(),
        };
        manager
            .initialize_pending_prompt(
                binding,
                Some(key.clone()),
                initial.map(str::to_string),
                !fresh,
                fresh,
                true,
            )
            .await
            .unwrap();
        (row, binding, key)
    }

    async fn issue_3145_publish(
        manager: &SessionManager,
        binding: PendingCreateBinding,
    ) -> SessionInfo {
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, live_witness(binding.session_id));
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                mutations,
            )
            .await
            .unwrap()
            .finalized_rows
            .remove(0)
    }

    #[tokio::test]
    async fn issue_3145_restart_commit_uses_latest_source_and_rejects_partial_failure() {
        let manager = SessionManager::new();
        manager.install_retained_prompts(Some(HashMap::new())).await;
        let (source, binding, _) = issue_3145_pending(&manager, "domain", Some("A"), false).await;
        issue_3145_publish(&manager, binding).await;
        let (target, pending, _) = issue_3145_pending(&manager, "domain", None, false).await;
        // An external input between pending preparation and commit must win.
        manager.set_last_prompt(source.id, "B".into()).await;
        let mut invalid = LifecycleMutations::default();
        invalid.finalize_live(pending, live_witness(target.id));
        invalid.transfer_prompt(source.id, pending); // missing removal
        assert!(manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                invalid
            )
            .await
            .is_err());
        assert_eq!(
            manager
                .get_pending_session(pending)
                .await
                .unwrap()
                .last_prompt,
            None
        );
        assert_eq!(
            manager
                .get_session(source.id)
                .await
                .unwrap()
                .last_prompt
                .as_deref(),
            Some("B")
        );
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(pending, live_witness(target.id));
        mutations.remove(source.id);
        mutations.transfer_prompt(source.id, pending);
        let result = manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                mutations,
            )
            .await
            .unwrap();
        assert_eq!(result.finalized_rows[0].last_prompt.as_deref(), Some("B"));
        assert!(manager.get_session(source.id).await.is_none());
        assert_eq!(
            manager.list_sessions().await[0].last_prompt.as_deref(),
            Some("B")
        );
        assert_eq!(
            crate::config::sessions_persistence::snapshot_sessions(&manager).await[0]
                .last_prompt
                .as_deref(),
            Some("B")
        );
    }

    #[tokio::test]
    async fn issue_3145_pending_rollback_never_associates_or_accepts_public_input() {
        let manager = SessionManager::new();
        manager.install_retained_prompts(Some(HashMap::new())).await;
        let (row, binding, _) = issue_3145_pending(&manager, "domain", Some("A"), false).await;
        manager.set_last_prompt(row.id, "B".into()).await;
        assert_eq!(
            manager
                .get_pending_session(binding)
                .await
                .unwrap()
                .last_prompt
                .as_deref(),
            Some("A")
        );
        manager.rollback_pending_create(binding).await.unwrap();
        assert!(manager.retained_prompt_snapshot().await.unwrap().is_empty());
        assert!(manager.state.read().await.pending_prompts.is_empty());
    }

    #[tokio::test]
    async fn issue_3145_failed_checkpoint_disables_lookup_but_direct_input_still_works() {
        let manager = SessionManager::new();
        let (row, binding, key) = issue_3145_pending(&manager, "domain", None, false).await;
        manager
            .install_retained_prompts(Some(HashMap::from([(
                key.clone(),
                RetainedPrompt {
                    last_prompt: Some("A".into()),
                    blocked: false,
                },
            )])))
            .await;
        manager.disable_prompt_reuse().await;
        assert_eq!(
            issue_3145_publish(&manager, binding).await.last_prompt,
            None
        );
        manager.set_last_prompt(row.id, "B".into()).await;
        assert_eq!(
            manager
                .get_session(row.id)
                .await
                .unwrap()
                .last_prompt
                .as_deref(),
            Some("B")
        );
        assert_eq!(
            manager.retained_prompt_snapshot().await.unwrap()[&key]
                .last_prompt
                .as_deref(),
            Some("B")
        );
    }

    #[tokio::test]
    async fn issue_3145_ambiguity_is_sticky_and_fresh_exclusive_reopens_domain() {
        let manager = SessionManager::new();
        manager.install_retained_prompts(Some(HashMap::new())).await;
        let (first, binding, key) = issue_3145_pending(&manager, "domain", Some("A"), false).await;
        issue_3145_publish(&manager, binding).await;
        let (second, pending, _) = issue_3145_pending(&manager, "domain", None, false).await;
        let mut dormant = LifecycleMutations::default();
        dormant.finalize_dormant(pending, dormant_witness(second.id), 0);
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                dormant,
            )
            .await
            .unwrap();
        assert!(manager.retained_prompt_snapshot().await.unwrap()[&key].blocked);
        let mut remove = LifecycleMutations::default();
        remove.remove(second.id);
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                remove,
            )
            .await
            .unwrap();
        manager.set_last_prompt(first.id, "B".into()).await;
        assert!(manager.retained_prompt_snapshot().await.unwrap()[&key].blocked);
        let (fresh, pending, _) = issue_3145_pending(&manager, "domain", Some("old"), true).await;
        let mut replace = LifecycleMutations::default();
        replace.finalize_live(pending, live_witness(fresh.id));
        replace.remove(first.id);
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restart(crate::session::selection::TrustedRestartIntent::User),
                replace,
            )
            .await
            .unwrap();
        let snapshot = manager.retained_prompt_snapshot().await.unwrap();
        assert!(!snapshot[&key].blocked);
        assert!(snapshot[&key].last_prompt.is_none());
        manager.set_last_prompt(fresh.id, String::new()).await;
        assert_eq!(
            manager.retained_prompt_snapshot().await.unwrap()[&key]
                .last_prompt
                .as_deref(),
            Some("")
        );
        manager.set_start_fresh_on_restore(fresh.id, true).await;
        manager.clear_start_fresh_on_restore_if_set(fresh.id).await;
        assert_eq!(
            manager.retained_prompt_snapshot().await.unwrap()[&key].last_prompt,
            None
        );
        assert_eq!(
            manager.get_session(fresh.id).await.unwrap().last_prompt,
            None
        );
    }

    #[tokio::test]
    async fn issue_3145_seed_and_setter_share_permit_and_empty_is_existing() {
        let manager = SessionManager::new();
        manager.install_retained_prompts(Some(HashMap::new())).await;
        let (row, binding, key) = issue_3145_pending(&manager, "domain", None, false).await;
        issue_3145_publish(&manager, binding).await;
        let permit = acquire_carrier_mutation_permit().await;
        assert!(
            manager
                .seed_if_absent_with_carrier_permit(&permit, row.id, &key, "A".into())
                .await
        );
        let mut setter = Box::pin(manager.set_last_prompt(row.id, "N".into()));
        use std::future::Future;
        let waker = futures::task::noop_waker();
        let mut context = std::task::Context::from_waker(&waker);
        assert!(setter.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            manager
                .get_session(row.id)
                .await
                .unwrap()
                .last_prompt
                .as_deref(),
            Some("A")
        );
        drop(permit);
        setter.await;
        let permit = acquire_carrier_mutation_permit().await;
        assert!(
            !manager
                .seed_if_absent_with_carrier_permit(&permit, row.id, &key, "old".into())
                .await
        );
        drop(permit);
        manager.set_last_prompt(row.id, String::new()).await;
        let permit = acquire_carrier_mutation_permit().await;
        assert!(
            !manager
                .seed_if_absent_with_carrier_permit(&permit, row.id, &key, "old".into())
                .await
        );
    }
    use crate::session::selection::TrustedCreateIntent;

    struct P44WriteBackend {
        entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        bytes: std::sync::Mutex<Vec<u8>>,
    }

    impl crate::pty::backend::PtyBackend for P44WriteBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn spawn(
            &self,
            _: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
            panic!("unexpected spawn")
        }
        fn write(
            &self,
            _: &crate::pty::manager::BackendWriteAuthority,
            _: Uuid,
            bytes: &[u8],
        ) -> Result<(), AppError> {
            self.entered
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            self.bytes.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }
        fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), AppError> {
            panic!("unexpected resize")
        }
        fn kill(&self, _: Uuid) -> Result<(), AppError> {
            panic!("unexpected kill")
        }
        fn has_session(&self, _: Uuid) -> bool {
            true
        }
        fn get_screen_snapshot(&self, _: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }
        fn get_pty_size(&self, _: Uuid) -> Option<(u16, u16)> {
            None
        }
        fn get_screen_rows(&self, _: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::Unavailable
        }
        fn register_response_watcher(&self, _: Uuid, _: String, _: PathBuf) {
            panic!("unexpected watcher")
        }
        fn terminate_job_for_session(&self, _: Uuid) -> bool {
            panic!("unexpected terminate")
        }
        fn kill_all_jobs(&self) -> (usize, usize) {
            panic!("unexpected kill all")
        }
    }

    #[tokio::test]
    async fn e2992_p44_retained_settings_permit() {
        use crate::config::settings::{
            AppSettings, OperationOutcome, SettingsAvailability, SettingsOwner, SettingsOwnerError,
            SettingsPublisher,
        };
        use crate::pty::idle_detector::{IdleDetector, PtyInputBoundaryFailure};
        use crate::pty::manager::PtyManager;
        use std::sync::Mutex;
        use std::time::Duration;

        let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/2992-evidence/P44");
        std::fs::create_dir_all(&evidence).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("permit-")
            .tempdir_in(evidence)
            .unwrap();
        let collection = temp.path().join("projects");
        let ac = collection.join("p44-project/.ac");
        std::fs::create_dir_all(ac.join("_team_p44-team")).unwrap();
        std::fs::write(ac.join("_team_p44-team/config.json"),
            br#"{"agents":["../_agent_worker","../_agent_coordinator"],"coordinator":"../_agent_coordinator"}"#).unwrap();
        let manager = SessionManager::new();
        let mut sessions = Vec::new();
        let mut paths = Vec::new();
        for (index, name) in ["coordinator", "worker"].iter().enumerate() {
            std::fs::create_dir_all(ac.join(format!("_agent_{name}"))).unwrap();
            let root = ac.join(format!("room-01-p44-team/__agent_{name}"));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(
                root.join("config.json"),
                format!(r#"{{"identity":"../../_agent_{name}"}}"#),
            )
            .unwrap();
            sessions.push(
                manager
                    .create_session(
                        "shell".into(),
                        Vec::new(),
                        root.to_string_lossy().into_owned(),
                        None,
                        None,
                        Vec::new(),
                        index == 0,
                        SessionBackendKind::LocalProcess,
                    )
                    .await
                    .unwrap(),
            );
            paths.push(root);
        }
        let cfg = AppSettings {
            project_paths: vec![collection.to_string_lossy().into_owned()],
            ..AppSettings::default()
        };
        let settings_path = temp.path().join("settings.30.instance.no-git.json");
        std::fs::write(&settings_path, serde_json::to_vec(&cfg).unwrap()).unwrap();
        let owner = tokio::task::spawn_blocking(move || {
            SettingsOwner::open_active(&settings_path, Duration::from_secs(5)).unwrap()
        })
        .await
        .unwrap();
        let route = crate::config::teams::verify_pty_input_route(
            &paths[0],
            false,
            "p44-project:room-01-p44-team/worker",
            &cfg.project_paths,
        )
        .unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let backend = Arc::new(P44WriteBackend {
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(release_rx),
            bytes: Mutex::new(Vec::new()),
        });
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        for (session, path) in sessions.iter().zip(&paths) {
            let identity = crate::path_identity::verify_directory(path).unwrap();
            pty.lock()
                .unwrap()
                .record_route_with_identities(
                    session.id,
                    SessionBackendKind::LocalProcess,
                    Some(identity.clone()),
                    Some(identity),
                )
                .unwrap();
        }
        let sender = sessions[0].id;
        let target = sessions[1].id;
        manager.mark_idle(target).await;
        let idle = IdleDetector::new(|_| {}, |_| {});
        idle.register_session(target, crate::session::profile::IdleTuning::DEFAULT);
        idle.set_pty_input_ready_for_test(target);
        macro_rules! boundary {
            ($authority:expr, $permit:expr) => {
                manager.prepare_pty_input_boundary(
                    target,
                    &route.target,
                    SessionBackendKind::LocalProcess,
                    sender,
                    &route.sender,
                    SessionBackendKind::LocalProcess,
                    $authority,
                    $permit,
                    &idle,
                    &owner,
                    |_, _| true,
                    || true,
                )
            };
        }
        let authority = PtyManager::authority_route_proof(&pty, sender).unwrap();
        let permit = PtyManager::acquire_input_writer(&pty, target)
            .await
            .unwrap();
        // Contention uses a legitimate owner operation, without a raw writer.
        let (builder_tx, builder_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let operation = {
            let owner = owner.clone();
            tokio::spawn(async move {
                owner
                    .with_operation("p44-contention", move |_, _| {
                        builder_tx.send(()).unwrap();
                        finish_rx
                            .recv_timeout(Duration::from_secs(10))
                            .map_err(|e| e.to_string())?;
                        Ok(())
                    })
                    .await
                    .unwrap()
            })
        };
        tokio::time::timeout(Duration::from_secs(5), builder_rx)
            .await
            .unwrap()
            .unwrap();
        let denied = boundary!(&authority, &permit).await;
        assert!(matches!(
            denied,
            Err(PtyInputBoundaryFailure::RouteUnavailable)
        ));
        drop(denied);
        assert!(backend.bytes.lock().unwrap().is_empty());
        finish_tx.send(()).unwrap();
        assert_eq!(
            operation.await.unwrap().outcome,
            OperationOutcome::Committed
        );
        drop(permit);
        drop(authority);

        // The bounded backend rendezvous is after validation, inside the real write.
        let worker = {
            let manager = manager.clone();
            let pty = pty.clone();
            let idle = idle.clone();
            let owner = owner.clone();
            let handle = tokio::runtime::Handle::current();
            let route = route.clone();
            tokio::task::spawn_blocking(move || {
                let authority = PtyManager::authority_route_proof(&pty, sender).unwrap();
                let permit = handle
                    .block_on(PtyManager::acquire_input_writer(&pty, target))
                    .unwrap();
                let guard = handle
                    .block_on(manager.prepare_pty_input_boundary(
                        target,
                        &route.target,
                        SessionBackendKind::LocalProcess,
                        sender,
                        &route.sender,
                        SessionBackendKind::LocalProcess,
                        &authority,
                        &permit,
                        &idle,
                        &owner,
                        |_, _| true,
                        || true,
                    ))
                    .unwrap();
                guard.write(b"p44 exact bytes").unwrap();
                drop(guard);
            })
        };
        tokio::time::timeout(Duration::from_secs(5), entered_rx)
            .await
            .unwrap()
            .unwrap();
        let (builder_tx, builder_rx) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let operation = {
            let owner = owner.clone();
            let backend = backend.clone();
            tokio::spawn(async move {
                started_tx.send(()).unwrap();
                owner
                    .with_operation("p44-retained", move |_, _| {
                        assert_eq!(*backend.bytes.lock().unwrap(), b"p44 exact bytes");
                        builder_tx.send(()).unwrap();
                        Ok(())
                    })
                    .await
                    .unwrap()
            })
        };
        started_rx.await.unwrap();
        let no_entry = tokio::task::spawn_blocking(move || {
            assert!(matches!(
                builder_rx.recv_timeout(Duration::from_millis(250)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            builder_rx
        })
        .await
        .unwrap();
        assert!(!operation.is_finished());
        assert!(backend.bytes.lock().unwrap().is_empty());
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), operation)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            OperationOutcome::Committed
        );
        no_entry.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(*backend.bytes.lock().unwrap(), b"p44 exact bytes");

        let report = owner
            .with_operation("p44-permit-invalid", |_, guard| {
                guard.publish(SettingsPublisher::SettingsSave, |path| {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                            .map_err(|e| e.to_string())?;
                    value["defaultShell"] = serde_json::json!(42);
                    std::fs::write(path, serde_json::to_vec(&value).unwrap())
                        .map_err(|e| e.to_string())
                })
            })
            .await
            .unwrap();
        assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
        assert_eq!(report.availability, SettingsAvailability::Invalid);
        assert!(matches!(
            owner.current_read().await,
            Err(SettingsOwnerError::Unavailable(
                SettingsAvailability::Invalid
            ))
        ));
        manager.mark_idle(target).await;
        idle.set_pty_input_ready_for_test(target);
        let authority = PtyManager::authority_route_proof(&pty, sender).unwrap();
        let permit = PtyManager::acquire_input_writer(&pty, target)
            .await
            .unwrap();
        let denied = boundary!(&authority, &permit).await;
        assert!(matches!(
            denied,
            Err(PtyInputBoundaryFailure::RouteUnavailable)
        ));
        drop(denied);
        assert_eq!(*backend.bytes.lock().unwrap(), b"p44 exact bytes");
        drop(permit);
        drop(authority);
        tokio::task::spawn_blocking(move || {
            owner.drain_blocking(Duration::from_secs(5));
            assert_eq!(owner.availability(), SettingsAvailability::Closed);
        })
        .await
        .unwrap();
    }

    fn birth_binding() -> CodingAgentBinding {
        CodingAgentBinding {
            id: Uuid::new_v4().to_string(),
            seed: Some(
                serde_json::json!({"identityRevision": format!("r1:0:{}", "a".repeat(64)), "originalAgentId":"original"}),
            ),
            origin: Some(
                serde_json::json!({"sessionsPath":"C:/sessions.json","rawFileSHA256":"witness","originalOrdinal":3}),
            ),
        }
    }

    #[tokio::test]
    async fn derived_agent_id_binding_pending_rollback_retires_only_its_binding() {
        let manager = SessionManager::new();
        let (first, pending) = pending_fixture(&manager, false).await;
        let (other, other_pending) = pending_fixture(&manager, false).await;
        let birth = birth_binding();
        let other_birth = birth_binding();
        manager
            .install_pending_coding_agent_binding(pending, Some(birth.clone()))
            .await
            .unwrap();
        manager
            .install_pending_coding_agent_binding(other_pending, Some(other_birth.clone()))
            .await
            .unwrap();
        assert!(manager.aggregate_snapshot().await.sessions.is_empty());
        manager.rollback_pending_create(pending).await.unwrap();
        assert!(manager.coding_agent_binding(first.id).await.is_none());
        assert_eq!(
            manager.coding_agent_binding(other.id).await.unwrap().id,
            other_birth.id
        );
        assert!(!manager
            .aggregate_snapshot()
            .await
            .binding_seeds
            .contains_key(&birth.id));
        manager
            .rollback_pending_create(other_pending)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn derived_agent_id_binding_failed_affected_survives_live_only_snapshot_until_success() {
        let manager = SessionManager::new();
        let birth = birth_binding();
        let failed = serde_json::json!({
            "name":"affected failure", "shell":"shell", "workingDirectory":"C:/failed", "shellArgs":[],
            "agentId":"original", "codingAgentBindingId":birth.id, "codingAgentBindingSeed":birth.seed,
        });
        // Fixture represents enrollment by a receipt consumer (D09). D07 does
        // not activate receipts or extend general failed-legacy retention.
        manager
            .state
            .write()
            .await
            .failed_binding_recipes
            .insert(birth.id.clone(), serde_json::to_value(&failed).unwrap());
        manager
            .refresh_failed_binding_recipes(vec![
                (birth.clone(), failed.clone()),
                (birth_binding(), failed.clone()),
            ])
            .await;
        for _ in 0..2 {
            let snapshot = manager.aggregate_snapshot().await;
            assert_eq!(snapshot.failed_binding_recipes.len(), 1);
            assert_eq!(snapshot.failed_binding_recipes[&birth.id], failed);
            assert!(snapshot.sessions.is_empty());
            assert!(manager.list_sessions().await.is_empty());
        }
        let (restored, pending) = pending_fixture(&manager, false).await;
        manager
            .install_pending_coding_agent_binding(pending, Some(birth))
            .await
            .unwrap();
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_dormant(pending, dormant_witness(restored.id), 7);
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                mutations,
            )
            .await
            .unwrap();
        assert!(manager
            .aggregate_snapshot()
            .await
            .failed_binding_recipes
            .is_empty());
    }

    #[tokio::test]
    async fn derived_agent_id_binding_ram_birth_aggregate_preserves_original_when_metadata_changes()
    {
        let manager = SessionManager::new();
        let (session, pending) = pending_fixture(&manager, false).await;
        let birth = birth_binding();
        manager
            .install_pending_coding_agent_binding(pending, Some(birth.clone()))
            .await
            .unwrap();
        let live = live_witness(session.id);
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(pending, live);
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Live(live),
                SelectionCause::SessionCreated(TrustedCreateIntent::User),
                mutations,
            )
            .await
            .unwrap();
        // Before a successful disk write, birth metadata remains owned by RAM.
        // A later disk configuration cannot become this binding's birth seed.
        manager
            .state
            .write()
            .await
            .sessions
            .get_mut(&session.id)
            .unwrap()
            .agent_id = Some("reused-id-after-external-rekey".into());
        let snapshot = manager.aggregate_snapshot().await;
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.coding_agent_bindings[&session.id], birth.id);
        assert_eq!(
            snapshot.binding_seeds[&birth.id]["originalAgentId"],
            "original"
        );
        assert_eq!(
            snapshot.binding_origins.get(&birth.id),
            birth.origin.as_ref()
        );
    }

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
    async fn set_profile_metadata_carries_the_match_fields() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude-mb".to_string(),
                Vec::new(),
                "C:\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        assert!(session.match_tier.is_none());
        assert!(session.original_profile_letter.is_none());

        mgr.set_profile_metadata(
            session.id,
            Some("B".to_string()),
            Some("A".to_string()),
            Vec::new(),
            false,
            Some("hash".to_string()),
            Some("B".to_string()),
            None,
            None,
        )
        .await;

        let stored = mgr
            .get_session(session.id)
            .await
            .expect("session should still exist");
        assert_eq!(stored.match_tier.as_deref(), Some("hash"));
        assert_eq!(stored.original_profile_letter.as_deref(), Some("B"));
    }

    #[tokio::test]
    async fn pending_session_starts_without_match_fields_and_pending_setter_carries_them() {
        let mgr = SessionManager::new();
        let (session, binding) = mgr
            .create_transaction_pending_session(
                "claude-mb".to_string(),
                Vec::new(),
                "C:\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("pending session should be created");
        assert!(session.match_tier.is_none());
        assert!(session.original_profile_letter.is_none());

        mgr.set_pending_profile_metadata(
            binding,
            Some("B".to_string()),
            Some("A".to_string()),
            Vec::new(),
            false,
            Some("commandAndLetter".to_string()),
            Some("B".to_string()),
            None,
            None,
        )
        .await
        .expect("pending metadata should be set");

        let stored = mgr
            .get_pending_session(binding)
            .await
            .expect("pending session should still exist");
        assert_eq!(stored.match_tier.as_deref(), Some("commandAndLetter"));
        assert_eq!(stored.original_profile_letter.as_deref(), Some("B"));
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

    /// #2232 phase 7: the Co-managed communication follows the same shape and
    /// dedup rule as `set_blocked_menu`, and its kind serializes as `coManaged`.
    #[tokio::test]
    async fn set_co_managed_round_trips_and_dedups_identical_messages() {
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
        let now = chrono::Utc::now();

        let (changed, communication) = mgr
            .set_co_managed(session.id, "Co-managed: routed".to_string(), now)
            .await
            .expect("the session exists and is live");
        assert!(changed);
        assert_eq!(communication.kind, SessionCommunicationKind::CoManaged);
        assert!(communication.visible);
        assert_eq!(communication.message.as_deref(), Some("Co-managed: routed"));

        // Re-setting the identical visible message reports unchanged.
        let (changed, again) = mgr
            .set_co_managed(
                session.id,
                "Co-managed: routed".to_string(),
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        assert!(!changed);
        assert_eq!(again.updated_at, communication.updated_at);

        // The wire form of the new variant is camelCase, like its siblings.
        assert_eq!(
            serde_json::to_string(&SessionCommunicationKind::CoManaged).unwrap(),
            "\"coManaged\""
        );

        assert!(mgr.clear_co_managed(session.id).await);
        assert!(mgr
            .get_session(session.id)
            .await
            .unwrap()
            .communication
            .is_none());
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

    async fn e2432_manager_fingerprint(manager: &SessionManager) -> serde_json::Value {
        let snapshot = manager.aggregate_snapshot().await;
        let mut pending: Vec<_> = snapshot.pending_ids.into_iter().collect();
        pending.sort();
        serde_json::json!({
            "sessions": snapshot.sessions, "order": snapshot.order, "pending": pending,
            "bindings": snapshot.coding_agent_bindings, "seeds": snapshot.binding_seeds,
            "origins": snapshot.binding_origins, "recipes": snapshot.failed_binding_recipes,
            "selection": snapshot.selection,
        })
    }

    async fn e2432_bound_old(
        manager: &SessionManager,
        seed: Option<serde_json::Value>,
        origin: Option<serde_json::Value>,
    ) -> (Uuid, CodingAgentBinding) {
        let (old, pending) = pending_fixture(manager, false).await;
        let binding = CodingAgentBinding {
            id: Uuid::new_v4().to_string(),
            seed,
            origin,
        };
        manager
            .install_pending_coding_agent_binding(pending, Some(binding.clone()))
            .await
            .unwrap();
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_dormant(pending, dormant_witness(old.id), 0);
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                mutations,
            )
            .await
            .unwrap();
        (old.id, binding)
    }

    #[tokio::test]
    async fn e2432_restart_binding_stages_without_publication_and_commits_last() {
        for seed in [
            None,
            Some(
                serde_json::json!({"identityRevision":"old", "originalAgentId":"same", "unknown":null}),
            ),
        ] {
            for origin in [
                None,
                Some(serde_json::Value::Null),
                Some(serde_json::json!({"ordinal":7})),
            ] {
                let manager = SessionManager::new();
                let (old, expected) = e2432_bound_old(&manager, seed.clone(), origin).await;
                let (replacement, pending) = pending_fixture(&manager, false).await;
                let desired =
                    serde_json::json!({"identityRevision":"fresh", "originalAgentId":"same"});
                let transition = manager
                    .stage_pending_restart_binding(pending, old, expected.clone(), desired.clone())
                    .await
                    .unwrap();
                assert_eq!(
                    manager.coding_agent_binding(old).await.unwrap().seed,
                    expected.seed
                );
                assert_eq!(
                    manager
                        .coding_agent_binding(replacement.id)
                        .await
                        .unwrap()
                        .seed,
                    expected.seed
                );
                let snapshot = manager.aggregate_snapshot().await;
                assert!(snapshot.sessions.iter().any(|row| row.id == old));
                assert!(!snapshot.sessions.iter().any(|row| row.id == replacement.id));
                let mut mutations = LifecycleMutations::default();
                mutations.finalize_live(pending, live_witness(replacement.id));
                mutations.remove(old);
                mutations.set_restart_binding(transition).unwrap();
                manager
                    .commit_selection_transition(
                        &CommitCapability::for_test(),
                        CommitDecision::Keep,
                        SelectionCause::Restore,
                        mutations,
                    )
                    .await
                    .unwrap();
                assert!(manager.get_session(old).await.is_none());
                assert!(manager.get_session(replacement.id).await.is_some());
                let final_binding = manager.coding_agent_binding(replacement.id).await.unwrap();
                assert_eq!(final_binding.id, expected.id);
                assert_eq!(final_binding.origin, expected.origin);
                assert_eq!(final_binding.seed, Some(desired));
            }
        }
    }

    #[tokio::test]
    async fn e2432_restart_binding_real_refresh_o1_o2_rejects_without_stale_rollback() {
        for refresh_after_stage in [false, true] {
            let manager = SessionManager::new();
            let (old, expected) =
                e2432_bound_old(&manager, None, Some(serde_json::json!("O1"))).await;
            let (_, pending) = pending_fixture(&manager, false).await;
            let desired =
                serde_json::json!({"identityRevision":"desired", "originalAgentId":"same"});
            let transition = if refresh_after_stage {
                Some(
                    manager
                        .stage_pending_restart_binding(
                            pending,
                            old,
                            expected.clone(),
                            desired.clone(),
                        )
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let concurrent = CodingAgentBinding {
                id: expected.id.clone(),
                seed: Some(serde_json::json!({"identityRevision":"O2", "originalAgentId":"same"})),
                origin: Some(serde_json::json!("O2")),
            };
            manager
                .refresh_failed_binding_recipes(vec![(
                    concurrent.clone(),
                    serde_json::json!({"recipe":"O2"}),
                )])
                .await;
            manager.set_last_prompt(old, "B".into()).await;
            let before_seeds = manager.aggregate_snapshot().await.binding_seeds;
            assert_eq!(before_seeds.get(&concurrent.id), concurrent.seed.as_ref());
            let before_rejection = e2432_manager_fingerprint(&manager).await;
            if let Some(transition) = transition {
                let mut mutations = LifecycleMutations::default();
                mutations.finalize_live(pending, live_witness(pending.session_id));
                mutations.remove(old);
                mutations.transfer_prompt(old, pending);
                mutations.set_restart_binding(transition).unwrap();
                assert!(manager
                    .commit_selection_transition(
                        &CommitCapability::for_test(),
                        CommitDecision::Keep,
                        SelectionCause::Restore,
                        mutations
                    )
                    .await
                    .is_err());
            } else {
                assert!(manager
                    .stage_pending_restart_binding(pending, old, expected, desired)
                    .await
                    .is_err());
            }
            assert_eq!(e2432_manager_fingerprint(&manager).await, before_rejection);
            // The rejected commit has returned and dropped its guard; rollback has not run.
            assert_eq!(
                manager.aggregate_snapshot().await.binding_seeds,
                before_seeds
            );
            assert_eq!(
                manager
                    .get_session(old)
                    .await
                    .unwrap()
                    .last_prompt
                    .as_deref(),
                Some("B")
            );
            assert!(manager
                .get_pending_session(pending)
                .await
                .unwrap()
                .last_prompt
                .is_none());
            manager.rollback_pending_create(pending).await.unwrap();
            let retained = manager.coding_agent_binding(old).await.unwrap();
            assert_eq!(retained.seed, concurrent.seed);
            assert_eq!(retained.origin, concurrent.origin);
            assert!(manager
                .aggregate_snapshot()
                .await
                .failed_binding_recipes
                .is_empty());
        }
    }

    #[tokio::test]
    async fn e2432_restart_binding_stage_rejections_are_nonmutating() {
        for case in 0..8 {
            let manager = SessionManager::new();
            let (old, expected) = e2432_bound_old(&manager, None, None).await;
            let (_, pending) = pending_fixture(&manager, false).await;
            let mut target = pending;
            let mut retiring = old;
            let mut captured = expected.clone();
            let mut desired =
                serde_json::json!({"identityRevision":"new", "originalAgentId":"same"});
            match case {
                0 => target.nonce = Uuid::new_v4(),
                1 => retiring = Uuid::new_v4(),
                2 => captured.id = Uuid::new_v4().to_string(),
                3 => captured.seed = Some(serde_json::Value::Null),
                4 => captured.origin = Some(serde_json::Value::Null),
                5 => desired = serde_json::json!({"identityRevision":"", "originalAgentId":"same"}),
                6 => {
                    let (_, extra) = pending_fixture(&manager, false).await;
                    manager
                        .install_pending_coding_agent_binding(extra, Some(expected.clone()))
                        .await
                        .unwrap();
                }
                7 => manager
                    .install_pending_coding_agent_binding(pending, Some(expected.clone()))
                    .await
                    .unwrap(),
                _ => unreachable!(),
            }
            let before = e2432_manager_fingerprint(&manager).await;
            assert!(
                manager
                    .stage_pending_restart_binding(target, retiring, captured, desired)
                    .await
                    .is_err(),
                "case {case}"
            );
            assert_eq!(
                e2432_manager_fingerprint(&manager).await,
                before,
                "case {case}"
            );
            manager.rollback_pending_create(pending).await.unwrap();
            assert!(manager.get_session(old).await.is_some());
        }
    }

    #[tokio::test]
    async fn e2432_restart_binding_commit_prevalidation_has_no_partial_publication() {
        for case in 0..5 {
            let manager = SessionManager::new();
            let (old, expected) = e2432_bound_old(&manager, None, None).await;
            let (replacement, pending) = pending_fixture(&manager, false).await;
            let desired = serde_json::json!({"identityRevision":"new", "originalAgentId":"same"});
            let transition = manager
                .stage_pending_restart_binding(pending, old, expected.clone(), desired.clone())
                .await
                .unwrap();
            let mut witness = live_witness(replacement.id);
            let mut decision = CommitDecision::Live(witness);
            match case {
                0 => witness.has_pty = false,
                1 => {
                    manager
                        .state
                        .write()
                        .await
                        .sessions
                        .get_mut(&replacement.id)
                        .unwrap()
                        .status = SessionStatus::Exited(1)
                }
                2 => decision = CommitDecision::Live(live_witness(Uuid::new_v4())),
                3 => manager.state.write().await.revision = u64::MAX,
                4 => {
                    manager
                        .refresh_failed_binding_recipes(vec![(
                            CodingAgentBinding {
                                id: expected.id.clone(),
                                seed: None,
                                origin: Some(serde_json::json!("O2")),
                            },
                            serde_json::json!({}),
                        )])
                        .await
                }
                _ => unreachable!(),
            }
            let before = e2432_manager_fingerprint(&manager).await;
            let mut mutations = LifecycleMutations::default();
            mutations.finalize_live(pending, witness);
            mutations.remove(old);
            mutations.set_restart_binding(transition).unwrap();
            assert!(
                manager
                    .commit_selection_transition(
                        &CommitCapability::for_test(),
                        decision,
                        SelectionCause::Restore,
                        mutations
                    )
                    .await
                    .is_err(),
                "case {case}"
            );
            assert_eq!(
                e2432_manager_fingerprint(&manager).await,
                before,
                "case {case}"
            );
            manager.rollback_pending_create(pending).await.unwrap();
            assert!(manager.get_session(old).await.is_some());
            assert_eq!(
                manager.coding_agent_binding(old).await.unwrap().seed,
                expected.seed
            );
        }
    }

    #[tokio::test]
    async fn e2432_restart_binding_orphan_refresh_is_not_a_retiring_owner() {
        let manager = SessionManager::new();
        let orphan = CodingAgentBinding {
            id: Uuid::new_v4().to_string(),
            seed: Some(serde_json::json!({"identityRevision":"current", "originalAgentId":"same"})),
            origin: Some(serde_json::json!({"ordinal":2})),
        };
        manager
            .refresh_failed_binding_recipes(vec![(orphan.clone(), serde_json::json!({}))])
            .await;
        let snapshot = manager.aggregate_snapshot().await;
        assert!(snapshot.sessions.is_empty());
        assert!(snapshot.failed_binding_recipes.is_empty());
        assert!(snapshot.coding_agent_bindings.is_empty());
        let (_, pending) = pending_fixture(&manager, false).await;
        let before = e2432_manager_fingerprint(&manager).await;
        assert!(manager
            .stage_pending_restart_binding(
                pending,
                Uuid::new_v4(),
                orphan.clone(),
                orphan.seed.clone().unwrap()
            )
            .await
            .is_err());
        assert_eq!(e2432_manager_fingerprint(&manager).await, before);
        manager.rollback_pending_create(pending).await.unwrap();
        let persisted =
            serde_json::json!({"id":orphan.id, "seed":orphan.seed, "origin":orphan.origin});
        let persisted: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&persisted).unwrap()).unwrap();
        for live in [false, true] {
            let cold = SessionManager::new();
            let (_, birth) = pending_fixture(&cold, false).await;
            cold.install_pending_coding_agent_binding(
                birth,
                Some(CodingAgentBinding {
                    id: persisted["id"].as_str().unwrap().to_string(),
                    seed: Some(persisted["seed"].clone()),
                    origin: Some(persisted["origin"].clone()),
                }),
            )
            .await
            .unwrap();
            let mut mutations = LifecycleMutations::default();
            if live {
                mutations.finalize_live(birth, live_witness(birth.session_id));
            } else {
                mutations.finalize_dormant(birth, dormant_witness(birth.session_id), 0);
            }
            cold.commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                mutations,
            )
            .await
            .unwrap();
            let restored = cold.coding_agent_binding(birth.session_id).await.unwrap();
            assert_eq!(restored.id, orphan.id);
            assert_eq!(restored.seed, orphan.seed);
            assert_eq!(restored.origin, orphan.origin);
            assert!(cold.get_session(birth.session_id).await.is_some());
        }
    }

    #[tokio::test]
    async fn e2432_restart_binding_second_transition_and_last_owner_retirement() {
        let manager = SessionManager::new();
        let (old, expected) = e2432_bound_old(
            &manager,
            Some(serde_json::json!({
                "identityRevision":"old", "originalAgentId":"same",
            })),
            Some(serde_json::json!("original")),
        )
        .await;
        let (_, pending) = pending_fixture(&manager, false).await;
        let desired = serde_json::json!({"identityRevision":"desired", "originalAgentId":"same"});
        let transition = manager
            .stage_pending_restart_binding(pending, old, expected.clone(), desired.clone())
            .await
            .unwrap();
        let before = e2432_manager_fingerprint(&manager).await;
        assert!(manager
            .stage_pending_restart_binding(pending, old, expected.clone(), desired.clone())
            .await
            .is_err());
        assert_eq!(e2432_manager_fingerprint(&manager).await, before);
        let mut mutations = LifecycleMutations::default();
        mutations.set_restart_binding(transition).unwrap();
        let other = SessionManager::new();
        let (other_old, other_expected) = e2432_bound_old(&other, None, None).await;
        let (_, other_pending) = pending_fixture(&other, false).await;
        let second = other
            .stage_pending_restart_binding(other_pending, other_old, other_expected, desired)
            .await
            .unwrap();
        assert!(mutations.set_restart_binding(second).is_err());
        manager.rollback_pending_create(pending).await.unwrap();
        assert_eq!(
            manager.coding_agent_binding(old).await.unwrap().seed,
            expected.seed
        );
        let mut remove = LifecycleMutations::default();
        remove.remove(old);
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                remove,
            )
            .await
            .unwrap();
        let retired = manager.aggregate_snapshot().await;
        assert!(!retired.binding_seeds.contains_key(&expected.id));
        assert!(!retired.binding_origins.contains_key(&expected.id));
        assert!(retired.coding_agent_bindings.is_empty());
        other.rollback_pending_create(other_pending).await.unwrap();
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

    /// `commands/session.rs` hands the PTY spawn `resolve_launch_idle_tuning`,
    /// whose `idle_threshold` is `idle_tuning_for(agent_kind)`'s (only the
    /// #2124 burst subfields can differ), and the detector registers exactly
    /// that. The record must therefore report the same threshold for every
    /// kind, not a hardcoded constant.
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

#[cfg(test)]
mod response_close_carrier_tests {
    use super::*;

    async fn row(manager: &SessionManager, name: &str, coordinator: bool) -> Session {
        manager
            .create_session(
                "shell".into(),
                vec![],
                name.into(),
                None,
                None,
                vec![],
                coordinator,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn response_close_f1_excludes_protected_unknown_and_pending_before_foreign_fallback() {
        let manager = SessionManager::new();
        let protected = row(&manager, "protected", false).await;
        let unknown = row(&manager, "unknown", false).await;
        let pending = manager
            .insert_pending_session_for_test("pending".into())
            .await;
        let foreign = row(&manager, "foreign", false).await;
        let selected = row(&manager, "selected", false).await;
        manager.mark_exited(protected.id, 0).await;
        manager.mark_exited(selected.id, 0).await;
        manager.set_active_only(selected.id).await.unwrap();
        let permit = acquire_carrier_mutation_permit().await;
        let mut classified = HashSet::new();
        let removed = manager
            .remove_exited_sessions_with_carrier_permit(
                &[selected.id, protected.id],
                &permit,
                |row, _| {
                    classified.insert(row.id);
                    if row.id == protected.id {
                        CarrierDisposition::Protected
                    } else if row.id == unknown.id {
                        CarrierDisposition::Unknown
                    } else {
                        CarrierDisposition::Foreign
                    }
                },
            )
            .await;
        assert_eq!(removed, 1);
        assert_eq!(classified.len(), 5);
        assert!(classified.contains(&pending));
        assert_eq!(manager.selection_payload().await.id(), Some(foreign.id));
        assert!(matches!(
            manager.get_session(protected.id).await.unwrap().status,
            SessionStatus::Exited(0)
        ));
    }

    #[tokio::test]
    async fn response_close_f1_clear_surviving_protected_target_and_no_foreign_fallback() {
        let manager = SessionManager::new();
        let protected = row(&manager, "protected", false).await;
        let other = row(&manager, "unknown", false).await;
        let permit = acquire_carrier_mutation_permit().await;
        let removed = manager
            .remove_exited_sessions_with_carrier_permit(&[protected.id], &permit, |row, _| {
                if row.id == protected.id {
                    CarrierDisposition::Protected
                } else {
                    CarrierDisposition::Unknown
                }
            })
            .await;
        assert_eq!(removed, 0);
        assert_eq!(
            manager.selection_payload().await.mode(),
            SelectionMode::None
        );
        assert!(manager.get_session(protected.id).await.is_some());
        assert!(manager.get_session(other.id).await.is_some());
        assert!(manager
            .state
            .read()
            .await
            .carrier_versions
            .contains_key(&protected.id));
    }

    #[tokio::test]
    async fn response_close_f1_without_protection_preserves_legacy_exited_fallback_order() {
        let manager = SessionManager::new();
        let first = row(&manager, "first", false).await;
        let second = row(&manager, "second", false).await;
        let selected = row(&manager, "selected", false).await;
        manager.mark_exited(first.id, 0).await;
        manager.mark_exited(selected.id, 0).await;
        manager.set_active_only(selected.id).await.unwrap();
        assert_eq!(manager.remove_exited_sessions(&[selected.id]).await, 1);
        assert_eq!(manager.selection_payload().await.id(), Some(first.id));
        assert_eq!(
            manager.get_session(first.id).await.unwrap().status,
            SessionStatus::Active
        );
        assert!(manager.get_session(second.id).await.is_some());
    }

    #[tokio::test]
    async fn response_close_binder_waits_for_publication_before_exposing_identity_change() {
        use std::future::Future;
        let manager = SessionManager::new();
        let session = row(&manager, "original", false).await;
        let permit = acquire_carrier_mutation_permit().await;
        let mut mutation = std::pin::pin!(manager.rename_session(session.id, "updated".into()));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(mutation.as_mut().poll(&mut context).is_pending());
        manager
            .with_current_carrier_snapshot(&permit, |snapshot| {
                assert_eq!(
                    snapshot
                        .rows
                        .iter()
                        .find(|r| r.id == session.id)
                        .unwrap()
                        .name,
                    session.name
                );
            })
            .await;
        drop(permit);
        mutation.await.unwrap();
        assert_eq!(
            manager.get_session(session.id).await.unwrap().name,
            "updated"
        );
    }
    #[tokio::test]
    async fn response_close_carrier_versions_ignore_observations_noops_and_fail_closed_on_overflow()
    {
        let manager = SessionManager::new();
        let session = row(&manager, "row", false).await;
        manager
            .set_git_repos(
                session.id,
                vec![SessionRepo {
                    label: "repo".into(),
                    source_path: "path".into(),
                    branch: None,
                    dirty: None,
                }],
            )
            .await;
        let before = manager.state.read().await.carrier_versions[&session.id];
        manager
            .set_git_repos(
                session.id,
                vec![SessionRepo {
                    label: "repo".into(),
                    source_path: "path".into(),
                    branch: Some("branch".into()),
                    dirty: Some(true),
                }],
            )
            .await;
        manager.set_context_percent(session.id, Some(20)).await;
        manager.set_last_prompt(session.id, "prompt".into()).await;
        manager
            .rename_session(session.id, session.name.clone())
            .await
            .unwrap();
        assert_eq!(
            manager.state.read().await.carrier_versions[&session.id],
            before
        );
        manager.set_is_coordinator(session.id, true).await;
        assert_eq!(
            manager.state.read().await.carrier_versions[&session.id],
            before + 1
        );
        manager
            .state
            .write()
            .await
            .carrier_versions
            .insert(session.id, u64::MAX);
        manager
            .rename_session(session.id, "overflow".into())
            .await
            .unwrap();
        assert_eq!(
            manager.state.read().await.carrier_versions[&session.id],
            u64::MAX
        );
        assert!(manager.response_close.generation(session.id).is_none());
    }

    #[tokio::test]
    async fn response_close_raise_rollback_cannot_clear_successor_communication() {
        let manager = SessionManager::new();
        let session = row(&manager, "coordinator", true).await;
        let permit = acquire_carrier_mutation_permit().await;
        let RaiseCarrierOutcome::Changed(_, receipt) = manager
            .raise_hand_with_carrier_permit(&permit, session.id, chrono::Utc::now())
            .await
        else {
            panic!("expected raise");
        };
        manager
            .clear_communication_if_kind_with_carrier_permit(
                &permit,
                session.id,
                SessionCommunicationKind::RaiseHand,
            )
            .await;
        let RaiseCarrierOutcome::Changed(_, successor) = manager
            .raise_hand_with_carrier_permit(&permit, session.id, chrono::Utc::now())
            .await
        else {
            panic!("expected successor");
        };
        assert_eq!(
            manager
                .rollback_raise_with_carrier_permit(&permit, receipt)
                .await,
            RaiseRollbackOutcome::Superseded
        );
        assert!(
            manager
                .get_session(session.id)
                .await
                .unwrap()
                .communication
                .unwrap()
                .visible
        );
        assert_eq!(
            manager
                .rollback_raise_with_carrier_permit(&permit, successor)
                .await,
            RaiseRollbackOutcome::Reverted
        );
        assert!(manager
            .get_session(session.id)
            .await
            .unwrap()
            .communication
            .is_none());
    }

    #[tokio::test]
    async fn response_close_prepared_create_binding_and_borrowed_pending_aliases_keep_exact_uuid_nonce(
    ) {
        let manager = SessionManager::new();
        let binding = manager.reserve_carrier_create_binding();
        let permit = acquire_carrier_mutation_permit().await;
        let session = manager
            .create_pending_session_with_binding_and_carrier_permit(
                binding,
                &permit,
                "shell".into(),
                vec![],
                "cwd".into(),
                None,
                None,
                vec![],
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        assert_eq!(session.id, binding.session_id());
        manager
            .rename_pending_session_with_carrier_permit(&permit, binding, "pending".into())
            .await
            .unwrap();
        assert!(manager
            .rename_pending_session_with_carrier_permit(
                &permit,
                PendingCreateBinding::new(binding.session_id, Uuid::new_v4()),
                "wrong".into()
            )
            .await
            .is_err());
        assert_eq!(
            manager.get_pending_session(binding).await.unwrap().name,
            "pending"
        );
        manager
            .rollback_pending_create_with_carrier_permit(&permit, binding)
            .await
            .unwrap();
        assert!(!manager.contains_public_or_pending(session.id).await);
    }
}
