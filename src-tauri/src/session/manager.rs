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

pub(crate) enum RetiredCarrierRouteObservation {
    Absent,
    Present,
    Unknown,
}

impl From<Result<crate::pty::manager::PtySnapshotRouteProof, AppError>>
    for RetiredCarrierRouteObservation
{
    fn from(value: Result<crate::pty::manager::PtySnapshotRouteProof, AppError>) -> Self {
        match value {
            Err(AppError::SessionNotFound(_)) => Self::Absent,
            Ok(_) => Self::Present,
            Err(_) => Self::Unknown,
        }
    }
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

#[derive(Debug, Default)]
pub(crate) struct LifecycleMutations {
    removals: Vec<Uuid>,
    mark_exited: Vec<(Uuid, i32)>,
    detached_intent: Vec<(Uuid, bool)>,
    finalizations: Vec<FinalizeMutation>,
    prompt_transfers: Vec<(Uuid, PendingCreateBinding)>,
}

impl LifecycleMutations {
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
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
        let binding = ticket.bind(id).map_err(AppError::Other)?;
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
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

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
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

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 production paths use the explicit-permit initializer"
        )
    )]
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
        self.initialize_pending_prompt_with_carrier_permit(
            &permit,
            binding,
            key,
            initial,
            continuation,
            fresh,
            allow_retained_lookup,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn initialize_pending_prompt_with_carrier_permit(
        &self,
        permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        key: Option<PromptKey>,
        initial: Option<String>,
        continuation: bool,
        fresh: bool,
        allow_retained_lookup: bool,
    ) -> Result<(), String> {
        let mut state = self.carrier_state_write(permit).await;
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

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
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

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
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

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
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

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
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

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
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

    /// The upper continuation owns the A/Preparing/launch check. This one
    /// interior mutation accepts only its exact finalized B and initial version.
    pub(super) async fn apply_finalized_owned_replacement_carry(
        &self,
        permit: &CarrierMutationPermit,
        capability: super::selection::FinalizedOwnedCarryCapability,
    ) -> super::selection::FinalizedOwnedCarryOutcome {
        use super::selection::FinalizedOwnedCarryOutcome;
        if !capability.bound_operation() {
            return FinalizedOwnedCarryOutcome::Recapture;
        }
        let id = capability.session_id();
        let token = capability.token();
        let version = capability.expected_version();
        if version == u64::MAX {
            return FinalizedOwnedCarryOutcome::Recapture;
        }
        let telegram = capability.telegram_bot_id().clone();
        let communication = capability.communication().clone();
        let fresh = capability.start_fresh_on_restore();
        let mut state = self.carrier_state_write(permit).await;
        if state.pending_create.contains_key(&id)
            || state.carrier_versions.get(&id) != Some(&version)
            || !state
                .sessions
                .get(&id)
                .is_some_and(|row| row.token == token)
        {
            drop(state);
            return FinalizedOwnedCarryOutcome::Recapture;
        }
        if let Some(row) = state.sessions.get_mut(&id) {
            row.telegram_bot_id = telegram;
            row.communication = communication;
            row.start_fresh_on_restore = fresh;
        }
        // Drop publishes the actual version and invalidation while U remains
        // borrowed by upper. The capability/owner is dropped outside the write.
        drop(state);
        drop(capability);
        FinalizedOwnedCarryOutcome::Applied
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

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn install_pending_coding_agent_binding(
        &self,
        pending: PendingCreateBinding,
        binding: Option<CodingAgentBinding>,
    ) -> Result<(), String> {
        let permit = acquire_carrier_mutation_permit().await;
        self.install_pending_coding_agent_binding_with_carrier_permit(&permit, pending, binding)
            .await
    }

    pub(crate) async fn install_pending_coding_agent_binding_with_carrier_permit(
        &self,
        permit: &CarrierMutationPermit,
        pending: PendingCreateBinding,
        binding: Option<CodingAgentBinding>,
    ) -> Result<(), String> {
        let mut state = self.carrier_state_write(permit).await;
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn refresh_failed_binding_recipes(
        &self,
        recipes: Vec<(CodingAgentBinding, serde_json::Value)>,
    ) {
        let permit = acquire_carrier_mutation_permit().await;
        self.refresh_failed_binding_recipes_with_carrier_permit(&permit, recipes)
            .await
    }

    pub(crate) async fn refresh_failed_binding_recipes_with_carrier_permit(
        &self,
        permit: &CarrierMutationPermit,
        recipes: Vec<(CodingAgentBinding, serde_json::Value)>,
    ) {
        let mut state = self.carrier_state_write(permit).await;
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

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
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

        // Every fallible check runs before either row receives transferred text.
        if !matches!(
            (target_mode, target_id, target_status.as_ref()),
            (SelectionMode::None, None, _)
                | (SelectionMode::Live, Some(_), _)
                | (
                    SelectionMode::Dormant,
                    Some(_),
                    Some(SessionStatus::Exited(_))
                )
        ) {
            return Err("invalid canonical selection target".to_string());
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
                        .expect("selection target validated before mutations");
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
                _ => unreachable!("selection target prevalidated"),
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
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
        let permit = acquire_carrier_mutation_permit().await;
        let mut state = self.carrier_state_write(&permit).await;
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
mod tests;

#[cfg(test)]
mod response_close_carrier_tests;
