use super::super::session::{Session, SessionCommunication, SessionStatus};
use super::{PendingCreateBinding, RetainedPrompt, SessionManager, SessionManagerState};
use crate::errors::AppError;
use crate::pty::backend::SessionBackendKind;
use crate::session::selection::payload::SessionSelection;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

/// The process publication gate is shared with P05 persistence. It holds no
/// directory protection map or settings state.
struct ProcessCarrierPublicationGate {
    mutex: Arc<tokio::sync::Mutex<()>>,
}

impl ProcessCarrierPublicationGate {
    async fn acquire(&self) -> CarrierMutationPermit {
        let gate = self.mutex.clone();
        let guard = gate.clone().lock_owned().await;
        CarrierMutationPermit {
            gate,
            _guard: guard,
        }
    }

    fn proves(&self, permit: &CarrierMutationPermit) -> bool {
        Arc::ptr_eq(&permit.gate, &self.mutex)
    }

    fn compatibility_mutex(&self) -> &Arc<tokio::sync::Mutex<()>> {
        &self.mutex
    }
}

fn process_carrier_publication_gate_owner() -> &'static ProcessCarrierPublicationGate {
    static PUBLICATION_GATE: std::sync::OnceLock<ProcessCarrierPublicationGate> =
        std::sync::OnceLock::new();
    PUBLICATION_GATE.get_or_init(|| ProcessCarrierPublicationGate {
        mutex: Arc::new(tokio::sync::Mutex::new(())),
    })
}

pub(crate) fn process_carrier_publication_gate() -> &'static Arc<tokio::sync::Mutex<()>> {
    process_carrier_publication_gate_owner().compatibility_mutex()
}

pub(crate) struct CarrierMutationPermit {
    gate: Arc<tokio::sync::Mutex<()>>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

pub(crate) async fn acquire_carrier_mutation_permit() -> CarrierMutationPermit {
    process_carrier_publication_gate_owner().acquire().await
}

impl CarrierMutationPermit {
    fn proves(&self) -> bool {
        process_carrier_publication_gate_owner().proves(self)
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

impl<T> From<Result<T, AppError>> for RetiredCarrierRouteObservation {
    fn from(value: Result<T, AppError>) -> Self {
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
    pub(super) uuid: Uuid,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(super) nonce: Uuid,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(super) version: u64,
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
pub(super) struct CarrierStateWrite<'a> {
    state: tokio::sync::RwLockWriteGuard<'a, SessionManagerState>,
    before: HashMap<Uuid, Session>,
    pending_before: HashSet<Uuid>,
    selection_before: SessionSelection,
    tracker: &'a crate::pty::response_close::ResponseCloseState,
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

pub(super) fn stable_carrier_equal(a: &Session, b: &Session) -> bool {
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

impl SessionManager {
    pub(crate) fn response_close_state(&self) -> crate::pty::response_close::ResponseCloseState {
        self.response_close.clone()
    }

    pub(super) async fn carrier_state_write<'a>(
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
}
