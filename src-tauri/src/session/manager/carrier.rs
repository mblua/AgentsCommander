#![expect(dead_code, reason = "P02 owned receipts activate in later phases")]
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
pub(crate) fn process_carrier_publication_gate() -> &'static Arc<tokio::sync::Mutex<()>> {
    static GATE: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
}

pub(crate) struct CarrierMutationPermit {
    gate: Arc<tokio::sync::Mutex<()>>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
    owned: Option<OwnedMutationClaim>,
}

pub(crate) async fn acquire_carrier_mutation_permit() -> CarrierMutationPermit {
    let gate = process_carrier_publication_gate().clone();
    let guard = gate.clone().lock_owned().await;
    CarrierMutationPermit {
        gate,
        _guard: guard,
        owned: None,
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
    manager_state: Arc<tokio::sync::RwLock<SessionManagerState>>,
    owned: Option<(Arc<OwnedMutationCell>, OwnedAction)>,
    owned_before: Vec<OwnedRowIdentity>,
    result: Option<super::CommitResult>,
    finished: bool,
    #[cfg(test)]
    probe: Option<crate::config::sessions_persistence::OperationScope>,
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

impl CarrierStateWrite<'_> {
    fn publish_carriers(&mut self) {
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
        let owned = permit
            .owned
            .as_ref()
            .map(|claim| (claim.cell.clone(), claim.action.clone()));
        let state = self.state.write().await;
        let owned_before = owned
            .as_ref()
            .map(|(cell, _)| {
                [
                    cell.reservation.original_a.id,
                    cell.reservation.header.binding.session_id,
                ]
                .into_iter()
                .filter_map(|id| {
                    state.sessions.get(&id).map(|row| OwnedRowIdentity {
                        id,
                        token: row.token,
                        version: state.carrier_versions.get(&id).copied().unwrap_or(0),
                    })
                })
                .collect()
            })
            .unwrap_or_default();
        let before = state.sessions.clone();
        let pending_before = state.pending_create.keys().copied().collect();
        let selection_before = state.selection.clone();
        CarrierStateWrite {
            state,
            before,
            pending_before,
            selection_before,
            tracker: &self.response_close,
            manager_state: self.state.clone(),
            owned,
            owned_before,
            result: None,
            finished: false,
            #[cfg(test)]
            probe: permit.owned.as_ref().and_then(|claim| claim.probe.clone()),
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

impl Drop for CarrierStateWrite<'_> {
    fn drop(&mut self) {
        if self.finished {
            if std::thread::panicking() {
                self.publish_owned(false);
            }
        } else {
            self.finish();
        }
    }
}

// Additive owned actions remain unactivated until the selection owner adopts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnedActionRole {
    FinalizeB,
    RollbackPendingB,
    RemoveA,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OwnedMutationHeader {
    pub operation: Uuid,
    pub executor_nonce: Uuid,
    pub preparing_nonce: Uuid,
    pub binding: PendingCreateBinding,
    pub role: OwnedActionRole,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OwnedRowIdentity {
    pub id: Uuid,
    pub token: Uuid,
    pub version: u64,
}
pub(crate) struct OwnedMutationReservation {
    pub header: OwnedMutationHeader,
    pub original_a: OwnedRowIdentity,
}
#[derive(Clone)]
pub(crate) enum OwnedAction {
    FinalizeB {
        token: Uuid,
        generation: u64,
    },
    RollbackPendingB {
        launch: Option<(Uuid, u64)>,
    },
    RemoveA {
        committed_b: Arc<OwnedMutationReceipt>,
    },
}
pub(crate) struct OwnedActionFacts {
    pub header: OwnedMutationHeader,
    pub action: OwnedAction,
}
#[derive(Debug)]
pub(crate) struct WitnessConflict;
pub(crate) struct OwnedActionWitness {
    cell: Arc<OwnedMutationCell>,
    action: OwnedAction,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnedMutationFailure {
    Conflict,
    CancelledBeforeMutation,
    Validation,
    AbsentWithoutReceipt,
    PartialMutation,
    TrackerUnavailable,
}
#[derive(Clone, Debug)]
pub(crate) enum OwnedMutationObservation {
    NotEntered,
    InProgress,
    NotApplied(OwnedMutationFailure),
    Applied(Arc<OwnedMutationReceipt>),
    ExitedUnknown(OwnedMutationFailure),
}
#[derive(Debug)]
pub(crate) struct OwnedMutationReceipt {
    cell: std::sync::Weak<OwnedMutationCell>,
    pub result: Option<super::CommitResult>,
    pub removed_row: Option<Session>,
    pub before: Vec<OwnedRowIdentity>,
    pub after: Vec<(OwnedRowIdentity, bool, bool)>,
    pub pending_before: bool,
    pub pending_after: bool,
    pub initial_b: Option<(Uuid, u64, u64)>,
    // Pending is additive manager publication, never full tracker settlement proof.
    pub tracker_pending: bool,
}
struct OwnedMutationState {
    manager: Option<std::sync::Weak<tokio::sync::RwLock<SessionManagerState>>>,
    action: Option<OwnedAction>,
    observation: OwnedMutationObservation,
    partial: Option<Arc<OwnedMutationReceipt>>,
}
pub(crate) struct OwnedMutationCell {
    reservation: OwnedMutationReservation,
    step_nonce: Uuid,
    slot: std::sync::Weak<tokio::sync::RwLock<SessionManager>>,
    state: std::sync::Mutex<OwnedMutationState>,
    started: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    pub(super) panic_point: std::sync::atomic::AtomicU8,
}
pub(crate) struct OwnedMutationClaim {
    cell: Arc<OwnedMutationCell>,
    action: OwnedAction,
    terminal: bool,
    failure: OwnedMutationFailure,
    #[cfg(test)]
    probe: Option<crate::config::sessions_persistence::OperationScope>,
}
impl OwnedMutationCell {
    pub(crate) fn partial_receipt(&self) -> Option<Arc<OwnedMutationReceipt>> {
        self.state.lock().ok()?.partial.clone()
    }
    pub(crate) fn reserve(
        reservation: OwnedMutationReservation,
        canonical_manager_slot: &Arc<tokio::sync::RwLock<SessionManager>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            reservation,
            step_nonce: Uuid::new_v4(),
            slot: Arc::downgrade(canonical_manager_slot),
            started: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            panic_point: std::sync::atomic::AtomicU8::new(0),
            state: std::sync::Mutex::new(OwnedMutationState {
                manager: None,
                action: None,
                observation: OwnedMutationObservation::NotEntered,
                partial: None,
            }),
        })
    }
}
impl OwnedAction {
    pub(super) fn launch(&self) -> Option<(Uuid, u64)> {
        match self {
            Self::FinalizeB { token, generation } => Some((*token, *generation)),
            Self::RollbackPendingB { launch } => *launch,
            Self::RemoveA { committed_b } => committed_b
                .initial_b
                .map(|(token, generation, _)| (token, generation)),
        }
    }
}
impl OwnedActionWitness {
    pub(crate) fn from_trusted_owner(
        _capability: &super::CommitCapability,
        cell: &Arc<OwnedMutationCell>,
        facts: OwnedActionFacts,
    ) -> Result<Self, WitnessConflict> {
        let role = match &facts.action {
            OwnedAction::FinalizeB { .. } => OwnedActionRole::FinalizeB,
            OwnedAction::RollbackPendingB { .. } => OwnedActionRole::RollbackPendingB,
            OwnedAction::RemoveA { committed_b } => {
                let original = committed_b.cell.upgrade().ok_or(WitnessConflict)?;
                let mut expected = cell.reservation.header;
                expected.role = OwnedActionRole::FinalizeB;
                if original.reservation.header != expected
                    || original.reservation.original_a != cell.reservation.original_a
                    || !std::sync::Weak::ptr_eq(&original.slot, &cell.slot)
                    || committed_b.initial_b.is_none()
                {
                    return Err(WitnessConflict);
                }
                OwnedActionRole::RemoveA
            }
        };
        if facts.header != cell.reservation.header || facts.header.role != role {
            return Err(WitnessConflict);
        }
        Ok(Self {
            cell: cell.clone(),
            action: facts.action,
        })
    }
}
impl SessionManager {
    pub(crate) fn observe_owned_mutation(
        &self,
        cell: &OwnedMutationCell,
    ) -> OwnedMutationObservation {
        cell.state
            .lock()
            .map(|state| state.observation.clone())
            .unwrap_or(OwnedMutationObservation::ExitedUnknown(
                OwnedMutationFailure::Conflict,
            ))
    }
    pub(crate) fn claim_owned_mutation(
        &self,
        cell: &Arc<OwnedMutationCell>,
        witness: OwnedActionWitness,
    ) -> Result<OwnedMutationClaim, OwnedMutationObservation> {
        let conflict = || OwnedMutationObservation::ExitedUnknown(OwnedMutationFailure::Conflict);
        if !Arc::ptr_eq(cell, &witness.cell) {
            return Err(conflict());
        }
        let slot = cell.slot.upgrade().ok_or_else(conflict)?;
        let current = slot
            .try_read()
            .map_err(|_| self.observe_owned_mutation(cell))?;
        if !Arc::ptr_eq(&self.state, &current.state) {
            return Err(conflict());
        }
        drop(current);
        if let OwnedAction::RemoveA { committed_b } = &witness.action {
            let original = committed_b.cell.upgrade().ok_or_else(conflict)?;
            let state = original.state.lock().map_err(|_| conflict())?;
            if !state
                .manager
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .is_some_and(|manager| Arc::ptr_eq(&manager, &self.state))
                || !matches!(&state.observation, OwnedMutationObservation::Applied(receipt)
                    if Arc::ptr_eq(receipt, committed_b))
            {
                return Err(conflict());
            }
        }
        let mut state = cell.state.lock().map_err(|_| conflict())?;
        if state
            .action
            .as_ref()
            .is_some_and(|action| !action.same(&witness.action))
            || state.manager.as_ref().is_some_and(|manager| {
                !manager
                    .upgrade()
                    .is_some_and(|manager| Arc::ptr_eq(&manager, &self.state))
            })
        {
            return Err(conflict());
        }
        let terminal = match &state.observation {
            OwnedMutationObservation::NotEntered => false,
            OwnedMutationObservation::Applied(_) => true,
            other => return Err(other.clone()),
        };
        if !terminal {
            state.manager = Some(Arc::downgrade(&self.state));
            state.action = Some(witness.action.clone());
            state.observation = OwnedMutationObservation::InProgress;
        }
        drop(state);
        Ok(OwnedMutationClaim {
            cell: cell.clone(),
            action: witness.action,
            terminal,
            failure: OwnedMutationFailure::CancelledBeforeMutation,
            #[cfg(test)]
            probe: crate::config::sessions_persistence::OperationScope::current(),
        })
    }
}
impl Drop for OwnedMutationClaim {
    fn drop(&mut self) {
        if self.terminal {
            return;
        }
        let mut state = self
            .cell
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if matches!(state.observation, OwnedMutationObservation::InProgress) {
            state.observation = if self.cell.started.load(std::sync::atomic::Ordering::Acquire) {
                OwnedMutationObservation::ExitedUnknown(OwnedMutationFailure::PartialMutation)
            } else {
                OwnedMutationObservation::NotApplied(self.failure)
            };
        }
    }
}
impl CarrierMutationPermit {
    pub(super) fn attach(&mut self, claim: OwnedMutationClaim) {
        self.owned = Some(claim);
    }
    pub(super) fn fail_unapplied(&mut self, reason: OwnedMutationFailure) {
        if let Some(claim) = &mut self.owned {
            claim.failure = reason;
        }
    }
    pub(super) fn owned_cell(&self) -> Option<Arc<OwnedMutationCell>> {
        self.owned.as_ref().map(|claim| claim.cell.clone())
    }
    pub(super) fn owned_request(
        &self,
    ) -> Option<(OwnedMutationHeader, OwnedRowIdentity, &OwnedAction)> {
        let claim = self.owned.as_ref()?;
        Some((
            claim.cell.reservation.header,
            claim.cell.reservation.original_a,
            &claim.action,
        ))
    }
    pub(super) fn terminal_receipt(
        &self,
        manager: &SessionManager,
    ) -> Option<Result<Arc<OwnedMutationReceipt>, OwnedMutationObservation>> {
        let claim = self.owned.as_ref().filter(|claim| claim.terminal)?;
        let conflict = || OwnedMutationObservation::ExitedUnknown(OwnedMutationFailure::Conflict);
        let valid = claim.cell.slot.upgrade().is_some_and(|slot| {
            slot.try_read()
                .is_ok_and(|current| Arc::ptr_eq(&current.state, &manager.state))
        });
        if !valid {
            return Some(Err(conflict()));
        }
        Some(match manager.observe_owned_mutation(&claim.cell) {
            OwnedMutationObservation::Applied(receipt) => Ok(receipt),
            other => Err(other),
        })
    }
}
impl CarrierStateWrite<'_> {
    pub(super) fn validate_owned_identity(&self) -> Result<(), String> {
        let Some((cell, action)) = &self.owned else {
            return Ok(());
        };
        let fail = || "owned manager mutation conflict".to_string();
        let slot = cell.slot.upgrade().ok_or_else(fail)?;
        let manager = slot.try_read().map_err(|_| fail())?;
        if !Arc::ptr_eq(&manager.state, &self.manager_state) {
            return Err(fail());
        }
        drop(manager);
        let mut a = cell.reservation.original_a;
        a.version = action.expected_a_version(a).ok_or_else(fail)?;
        let b = cell.reservation.header.binding;
        if !self
            .state
            .sessions
            .get(&a.id)
            .is_some_and(|row| row.token == a.token && !row.is_root_agent && !row.is_coordinator)
            || self.state.carrier_versions.get(&a.id) != Some(&a.version)
            || a.id == b.session_id
            || self
                .state
                .sessions
                .get(&b.session_id)
                .is_some_and(|row| row.is_root_agent || row.is_coordinator)
            || (self.state.sessions.contains_key(&b.session_id)
                && !self.state.carrier_versions.contains_key(&b.session_id))
            || cell.state.is_poisoned()
            || !self.tracker.is_healthy()
        {
            return Err(fail());
        }
        if let Some((token, generation)) = action.launch() {
            if !self
                .state
                .sessions
                .get(&b.session_id)
                .is_some_and(|row| row.token == token)
                || self
                    .tracker
                    .generation(b.session_id)
                    .is_some_and(|actual| actual != generation)
            {
                return Err(fail());
            }
        }
        Ok(())
    }
    pub(super) fn owned_request(
        &self,
    ) -> Option<(OwnedMutationHeader, OwnedRowIdentity, &OwnedAction)> {
        self.owned
            .as_ref()
            .map(|(cell, action)| (cell.reservation.header, cell.reservation.original_a, action))
    }
    pub(super) fn start_owned_mutation(&self) {
        if let Some((cell, _)) = &self.owned {
            cell.started
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
    pub(super) fn copy_owned_result(&mut self, result: &super::CommitResult) {
        if self.owned.is_some() {
            #[cfg(test)]
            self.inject_panic(1);
            self.result = Some(result.clone());
        }
    }
}
impl CarrierStateWrite<'_> {
    pub(super) fn finish(&mut self) {
        if self.finished {
            return;
        }
        // Mark first: a partial panic must never replay tracker/version publication.
        self.finished = true;
        self.publish_carriers();
        self.publish_owned(!std::thread::panicking());
    }
    fn publish_owned(&mut self, completed: bool) {
        let Some((cell, action)) = &self.owned else {
            return;
        };
        if !cell.started.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let binding = cell.reservation.header.binding;
        let removed_id = match action {
            OwnedAction::RemoveA { .. } => cell.reservation.original_a.id,
            _ => binding.session_id,
        };
        let after = self
            .owned_before
            .iter()
            .map(|before| {
                let row = self.state.sessions.get(&before.id);
                (
                    OwnedRowIdentity {
                        id: before.id,
                        token: row.map_or(before.token, |row| row.token),
                        version: self
                            .state
                            .carrier_versions
                            .get(&before.id)
                            .copied()
                            .unwrap_or(before.version),
                    },
                    row.is_some(),
                    self.state.pending_create.contains_key(&before.id),
                )
            })
            .collect();
        let receipt = Arc::new(OwnedMutationReceipt {
            cell: Arc::downgrade(cell),
            result: self.result.take(),
            removed_row: if self.state.sessions.contains_key(&removed_id) {
                None
            } else {
                self.before.get(&removed_id).cloned()
            },
            before: self.owned_before.clone(),
            after,
            pending_before: self.pending_before.contains(&binding.session_id),
            pending_after: self.state.pending_create.contains_key(&binding.session_id),
            initial_b: match action {
                OwnedAction::FinalizeB { token, generation } => self
                    .state
                    .carrier_versions
                    .get(&binding.session_id)
                    .map(|version| (*token, *generation, *version)),
                _ => None,
            },
            tracker_pending: true,
        });
        let healthy = self.tracker.is_healthy();
        let mut slot = cell.state.lock().unwrap_or_else(|error| error.into_inner());
        if !matches!(slot.observation, OwnedMutationObservation::InProgress) {
            return;
        }
        let receipt = slot.partial.get_or_insert(receipt).clone();
        drop(slot);
        #[cfg(test)]
        if completed {
            self.inject_panic(2);
        }
        let mut slot = cell.state.lock().unwrap_or_else(|error| error.into_inner());
        if !matches!(slot.observation, OwnedMutationObservation::InProgress) {
            return;
        }
        slot.observation = if completed && healthy {
            OwnedMutationObservation::Applied(receipt)
        } else {
            OwnedMutationObservation::ExitedUnknown(if healthy {
                OwnedMutationFailure::PartialMutation
            } else {
                OwnedMutationFailure::TrackerUnavailable
            })
        };
        drop(slot);
        #[cfg(test)]
        if completed && healthy {
            if let Some(probe) = &self.probe {
                probe.record(
                    crate::config::sessions_persistence::ProbeEvent::ManagerReceiptFinalized,
                );
            }
        }
    }
    #[cfg(test)]
    fn inject_panic(&self, point: u8) {
        if self.owned.as_ref().is_some_and(|(cell, _)| {
            cell.panic_point
                .compare_exchange(
                    point,
                    0,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_ok()
        }) {
            panic!("owned manager injected panic at {point}");
        }
    }
}
impl OwnedMutationClaim {
    #[cfg(test)]
    pub(super) fn probe(&self) -> Option<crate::config::sessions_persistence::OperationScope> {
        self.probe.clone()
    }
}
