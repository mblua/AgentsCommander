use std::sync::atomic::Ordering;

use std::sync::{Arc, Mutex, Weak};

use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;

use crate::config::sessions_persistence::persist_current_state_result;
use crate::pty::manager::PtyManager;
use crate::session::manager::{
    CommitDecision, CommitResult, LifecycleMutations, ManagerAggregateSnapshot,
    PendingCreateBinding, SessionManager,
};
use crate::session::session::SessionInfo;
use crate::DetachedSessionsState;

use super::admission::CoordinatorPhase;
use super::authority::CommitCapability;
use super::payload::{
    DormantRuntimeWitness, LiveRuntimeWitness, RuntimeSelectionSnapshot, SelectionCause,
    SelectionCoordinatorError, SelectionSource,
};
use super::replacement::{
    EntryCreateBinding, EntryCreateStage, EntryReaderDemandReceipt, LifecycleEffectGuard,
    LifecycleEffectSlot, OwnedEffectState, OwnedLifecycleEntry, OwnedReplacementDescriptor,
    ReplacementOwner, ReplacementStage,
};
use super::{CoordinatorInner, LifecycleExecutor};

pub(crate) struct SelectionTransaction<R: Runtime> {
    pub(super) app: AppHandle<R>,
    pub(super) capability: CommitCapability,
    pub(super) lifecycle_entry: Option<(Arc<ReplacementOwner>, Uuid)>,
    pub(super) coordinator: Weak<CoordinatorInner>,
    pub(super) restore_nonce: Option<Uuid>,
    executor: Option<Arc<dyn LifecycleExecutor<SelectionTransaction<R>>>>,
}

impl<R: Runtime> Clone for SelectionTransaction<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            capability: self.capability.clone(),
            lifecycle_entry: self.lifecycle_entry.clone(),
            coordinator: self.coordinator.clone(),
            restore_nonce: self.restore_nonce,
            executor: self.executor.clone(),
        }
    }
}

impl<R: Runtime> SelectionTransaction<R> {
    pub(super) fn new(app: AppHandle<R>, coordinator: Weak<CoordinatorInner>) -> Self {
        Self {
            app,
            capability: CommitCapability {
                _private: super::seal::Token,
            },
            lifecycle_entry: None,
            coordinator,
            restore_nonce: None,
            executor: None,
        }
    }

    pub(crate) fn with_lifecycle_executor(
        mut self,
        executor: Arc<dyn LifecycleExecutor<Self>>,
    ) -> Self {
        self.executor = Some(executor);
        self
    }

    pub(super) fn lifecycle_executor(&self) -> Option<Arc<dyn LifecycleExecutor<Self>>> {
        self.executor.clone()
    }

    #[cfg(test)]
    pub(crate) fn for_test(app: AppHandle<R>) -> Self {
        Self {
            app,
            capability: CommitCapability::for_test(),
            lifecycle_entry: None,
            coordinator: Weak::new(),
            restore_nonce: None,
            executor: None,
        }
    }

    pub(super) fn for_lifecycle_entry(&self, entry: &OwnedLifecycleEntry) -> Self {
        let mut transaction = self.clone();
        transaction.lifecycle_entry = Some((entry.owner.clone(), entry.executor_nonce));
        if let Ok(mut state) = entry.owner.state.lock() {
            if state.executor_nonce == entry.executor_nonce
                && state.stage == ReplacementStage::Queued
            {
                state.stage = ReplacementStage::Executing;
            }
        }
        transaction
    }

    /// Only the original exact-UUID lookup failure calls this before any
    /// teardown/create/native prerequisite. No error string implies this fact.
    pub(crate) fn observe_lifecycle_missing_source(&self) {
        let Some((owner, nonce)) = &self.lifecycle_entry else {
            return;
        };
        if let Ok(mut state) = owner.state.lock() {
            if state.executor_nonce == *nonce
                && state.stage == ReplacementStage::Executing
                && state
                    .effects
                    .iter()
                    .all(|effect| matches!(effect, OwnedEffectState::NotStarted))
            {
                state.pre_teardown_failure = true;
            }
        }
    }

    /// The original resolver/probe/native preparation returned its observed
    /// error before retirement or pending-runtime creation. Native residues stay.
    pub(crate) fn observe_lifecycle_preparation_failure(&self) {
        let Some((owner, nonce)) = &self.lifecycle_entry else {
            return;
        };
        if let Ok(mut state) = owner.state.lock() {
            let context_failed = matches!(
                &state.effects[LifecycleEffectSlot::ContextPreparation as usize],
                OwnedEffectState::Completed {
                    outcome: Err(_),
                    ..
                }
            );
            let no_later_effect = state.effects.iter().enumerate().all(|(index, effect)| {
                if index == LifecycleEffectSlot::RootDirectory as usize
                    || index == LifecycleEffectSlot::ContextPreparation as usize
                {
                    matches!(
                        effect,
                        OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }
                    )
                } else {
                    matches!(effect, OwnedEffectState::NotStarted)
                }
            });
            if state.executor_nonce == *nonce
                && state.stage == ReplacementStage::Executing
                && context_failed
                && no_later_effect
            {
                state.pre_teardown_failure = true;
            }
        }
    }

    /// Only the actual ReuseLive return can certify this path. Restart/create
    /// paths never call this and still require their own reached-effect proof.
    pub(crate) fn observe_reuse_live_result(&self, result: &Result<SessionInfo, String>) {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return;
        };
        let Ok(mut state) = owner.state.lock() else {
            return;
        };
        if state.executor_nonce == *nonce
            && state.stage == ReplacementStage::Executing
            && matches!(&owner.descriptor, OwnedReplacementDescriptor::EntryRoot(_))
            && state.reuse_live_result.is_none()
            && state.entry_create.is_none()
            && state.effects.iter().enumerate().all(|(index, effect)| {
                if index == LifecycleEffectSlot::RootDirectory as usize
                    || index == LifecycleEffectSlot::Publication as usize
                {
                    matches!(effect, OwnedEffectState::Completed { .. })
                } else {
                    matches!(effect, OwnedEffectState::NotStarted)
                }
            })
        {
            state.reuse_live_result = Some(result.clone());
        }
    }

    pub(crate) fn retain_entry_create_binding(&self, binding: PendingCreateBinding) -> bool {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return true;
        };
        let Ok(mut state) = owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != *nonce || state.stage != ReplacementStage::Executing {
            return false;
        }
        if state.entry_create.is_some() {
            return false;
        }
        let preparation_receipt =
            match &state.effects[LifecycleEffectSlot::ContextPreparation as usize] {
                OwnedEffectState::NotStarted => None,
                receipt @ OwnedEffectState::Completed { .. } => Some(receipt.clone()),
                _ => return false,
            };
        state.effects[LifecycleEffectSlot::ContextPreparation as usize] =
            OwnedEffectState::NotStarted;
        state.entry_create = Some(EntryCreateBinding {
            binding,
            stage: EntryCreateStage::Reserved,
            preparation_receipt,
            reader_demand: None,
        });
        true
    }

    pub(crate) fn observe_entry_reader_demand(
        &self,
        binding: PendingCreateBinding,
        receipt: EntryReaderDemandReceipt,
    ) -> bool {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return true;
        };
        let Ok(mut state) = owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != *nonce || state.stage != ReplacementStage::Executing {
            return false;
        }
        let Some(create) = state.entry_create.as_mut() else {
            return false;
        };
        if create.binding != binding
            || create.stage != EntryCreateStage::Finalized
            || create.reader_demand.is_some()
        {
            return false;
        }
        create.reader_demand = Some(receipt);
        true
    }

    pub(crate) fn observe_entry_pending_binding(&self, binding: PendingCreateBinding) -> bool {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return true;
        };
        let Ok(mut state) = owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != *nonce || state.stage != ReplacementStage::Executing {
            return false;
        }
        let Some(create) = state.entry_create.as_mut() else {
            return false;
        };
        if create.binding != binding || create.stage != EntryCreateStage::Reserved {
            return false;
        }
        create.stage = EntryCreateStage::Pending;
        true
    }

    /// The actual commit and synchronous disarm share U. A lost result after
    /// this transition never turns finalized B into pending cleanup authority.
    pub(crate) async fn commit_entry_create(
        &self,
        binding: PendingCreateBinding,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return self.commit(decision, cause, mutations).await;
        };
        let manager = self.manager().await;
        let activity_owner = manager.activity_owner();
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let authorized = owner.state.lock().ok().is_some_and(|state| {
            state.executor_nonce == *nonce
                && state.stage == ReplacementStage::Executing
                && state.entry_create.as_ref().is_some_and(|create| {
                    create.binding == binding && create.stage == EntryCreateStage::Pending
                })
        });
        if !authorized {
            return Err(SelectionCoordinatorError::Unavailable.to_string());
        }
        let committed = manager
            .commit_selection_transition_with_carrier_permit(
                &permit,
                &self.capability,
                decision,
                cause,
                mutations,
            )
            .await?;
        // No await/fallible exit between real manager commit and disarm.
        let disarmed = match owner.state.lock() {
            Ok(mut state) => {
                let same_executor = state.executor_nonce == *nonce;
                match state.entry_create.as_mut() {
                    Some(create)
                        if same_executor
                            && create.binding == binding
                            && create.stage == EntryCreateStage::Pending =>
                    {
                        create.stage = EntryCreateStage::Finalized;
                        true
                    }
                    Some(create) => {
                        create.stage = EntryCreateStage::Unknown;
                        state.stage = ReplacementStage::Ambiguous;
                        false
                    }
                    None => {
                        state.stage = ReplacementStage::Ambiguous;
                        false
                    }
                }
            }
            Err(_) => false,
        };
        drop(permit);
        if !disarmed {
            // Actual B committed: never turn missing owner bookkeeping into an
            // ordinary Err which could trigger pending-runtime cleanup.
            return std::future::pending().await;
        }
        crate::config::activity_log::append_batch(&committed.activity, &activity_owner);
        Ok(committed)
    }

    pub(crate) fn has_lifecycle_entry(&self) -> bool {
        self.lifecycle_entry.is_some()
    }

    pub(crate) fn start_lifecycle_effect(
        &self,
        slot: LifecycleEffectSlot,
    ) -> Option<LifecycleEffectGuard> {
        let (owner, nonce) = self.lifecycle_entry.as_ref()?;
        let mut state = owner.state.lock().ok()?;
        if state.executor_nonce != *nonce
            || state.stage != ReplacementStage::Executing
            || !matches!(state.effects[slot as usize], OwnedEffectState::NotStarted)
        {
            state.stage = ReplacementStage::Ambiguous;
            return None;
        }
        let attempt = Uuid::new_v4();
        state.effects[slot as usize] = OwnedEffectState::Running(attempt);
        Some(LifecycleEffectGuard {
            owner: owner.clone(),
            slot,
            attempt,
            completed: false,
        })
    }

    pub(crate) fn lifecycle_root_directory_outcome(&self) -> Option<Result<String, String>> {
        let (owner, nonce) = self.lifecycle_entry.as_ref()?;
        let state = owner.state.lock().ok()?;
        if state.executor_nonce != *nonce {
            return None;
        }
        match &state.effects[LifecycleEffectSlot::RootDirectory as usize] {
            OwnedEffectState::Completed {
                outcome: Ok(Some(path)),
                ..
            } => Some(Ok(path.clone())),
            OwnedEffectState::Completed {
                outcome: Err(error),
                ..
            } => Some(Err(error.clone())),
            _ => None,
        }
    }

    pub(crate) fn has_accepted_lifecycle_core(&self) -> bool {
        self.lifecycle_entry.as_ref().is_some_and(|(owner, nonce)| {
            owner.state.lock().ok().is_some_and(|state| {
                state.executor_nonce == *nonce && state.stage == ReplacementStage::Executing
            })
        })
    }

    pub(crate) fn current_restore_nonce(&self) -> Option<Uuid> {
        self.has_active_restore_capability()
            .then_some(self.restore_nonce)
            .flatten()
    }

    pub(crate) fn has_restore_context(&self) -> bool {
        self.restore_nonce.is_some()
    }

    /// Contradictory mid-effect authority invalidates only this real restore
    /// capability. Keep its actual barrier active until the existing owner
    /// releases it, and latch persistence unsafe rather than resetting history.
    pub(crate) fn invalidate_restore_ownership(&self) {
        let (Some(nonce), Some(inner)) = (self.restore_nonce, self.coordinator.upgrade()) else {
            return;
        };
        if let Ok(mut root) = inner.retained_replacements.lock() {
            if root.restore_nonce == Some(nonce) && root.restore_active.load(Ordering::Acquire) {
                root.unavailable = true;
            }
        };
    }

    pub(crate) fn has_active_restore_capability(&self) -> bool {
        let Some(nonce) = self.restore_nonce else {
            return false;
        };
        let Some(inner) = self.coordinator.upgrade() else {
            return false;
        };
        let Ok(root) = inner.retained_replacements.lock() else {
            return false;
        };
        !root.unavailable
            && root.restore_nonce == Some(nonce)
            && root.restore_active.load(Ordering::Acquire)
            && CoordinatorPhase::from_u8(inner.phase.load(Ordering::Acquire))
                == CoordinatorPhase::Running
            && !inner.shutdown.is_cancelled()
    }

    pub(super) fn begin_restore(&self, nonce: Uuid) -> bool {
        let Some(inner) = self.coordinator.upgrade() else {
            return false;
        };
        let Ok(mut root) = inner.retained_replacements.lock() else {
            return false;
        };
        if !root.restore_active.load(Ordering::Acquire)
            || root.restore_nonce.is_some()
            || CoordinatorPhase::from_u8(inner.phase.load(Ordering::Acquire))
                == CoordinatorPhase::Closing
        {
            return false;
        }
        root.restore_nonce = Some(nonce);
        true
    }

    pub(super) fn release_restore(&self, nonce: Uuid) {
        let Some(inner) = self.coordinator.upgrade() else {
            return;
        };
        let Ok(mut root) = inner.retained_replacements.lock() else {
            return;
        };
        if root.restore_nonce == Some(nonce) {
            root.restore_nonce = None;
            root.restore_active.store(false, Ordering::Release);
        }
    }

    pub(crate) fn app(&self) -> &AppHandle<R> {
        &self.app
    }

    pub(crate) async fn manager(&self) -> SessionManager {
        self.app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone()
    }

    /// Uses the caller's preallocated binding and the current selection turn.
    /// This reserves no queue slot and performs no manager access.
    pub(crate) fn prepare_inline_create_binding(
        &self,
        binding: PendingCreateBinding,
        reserved: &mut Option<PendingCreateBinding>,
    ) -> Result<PendingCreateBinding, String> {
        if reserved.is_some() {
            return Err("inline create binding was already prepared".to_string());
        }
        *reserved = Some(binding);
        Ok(binding)
    }

    #[allow(clippy::too_many_arguments)]
    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn create_pending_session(
        &self,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<crate::session::session::SessionRepo>,
        is_coordinator: bool,
        backend_kind: crate::pty::backend::SessionBackendKind,
    ) -> Result<(crate::session::session::Session, PendingCreateBinding), String> {
        self.manager()
            .await
            .create_transaction_pending_session(
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
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn aggregate_snapshot(&self) -> ManagerAggregateSnapshot {
        self.manager().await.aggregate_snapshot().await
    }

    pub(crate) fn runtime_snapshot(&self, session_id: Uuid) -> RuntimeSelectionSnapshot {
        let detached = self
            .app
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&session_id);
        let has_pty = self
            .app
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(session_id);
        RuntimeSelectionSnapshot {
            session_id,
            has_pty,
            detached,
        }
    }

    pub(crate) fn live_decision(&self, session_id: Uuid) -> Option<CommitDecision> {
        LiveRuntimeWitness::from_snapshot(self.runtime_snapshot(session_id))
            .map(CommitDecision::Live)
    }

    pub(crate) fn dormant_decision(&self, session_id: Uuid) -> Option<CommitDecision> {
        DormantRuntimeWitness::from_snapshot(self.runtime_snapshot(session_id))
            .map(CommitDecision::Dormant)
    }

    pub(crate) async fn commit(
        &self,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        // #1149 - the single funnel that returns a `CommitResult` outward, so the
        // only place the birth-edge records can be appended.
        //
        // The `let` binding is load-bearing. As a tail expression the temporary
        // `SessionManager` would live across the append. It is harmless today
        // only because `manager()` returns by value, cloning out of the outer
        // lock and releasing that guard before it returns, so `append_batch`
        // holds neither the outer nor the inner lock. That stops being true if
        // `manager()` is ever changed to return a guard.
        //
        // The `?` means an `Err` returns before the append and drops `result`
        // whole, so no record survives for a session that was never finalized.
        let (result, activity_owner) = {
            let manager = self.manager().await;
            let activity_owner = manager.activity_owner();
            let result = manager
                .commit_selection_transition(&self.capability, decision, cause, mutations)
                .await?;
            (result, activity_owner)
        };
        crate::config::activity_log::append_batch(&result.activity, &activity_owner);
        Ok(result)
    }

    pub(crate) async fn commit_response_close_retained(
        &self,
        id: Uuid,
    ) -> Result<CommitResult, String> {
        let manager = self.manager().await;
        let activity_owner = manager.activity_owner();
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let decision = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if current.selection.id() == Some(id) {
                    CommitDecision::Clear
                } else {
                    CommitDecision::Keep
                }
            })
            .await;
        let mut mutations = LifecycleMutations::default();
        mutations.set_detached_intent(id, false);
        mutations.mark_exited(id, 0);
        let result = manager
            .commit_selection_transition_with_carrier_permit(
                &permit,
                &self.capability,
                decision,
                SelectionCause::BackgroundCleanup,
                mutations,
            )
            .await?;
        drop(permit);
        crate::config::activity_log::append_batch(&result.activity, &activity_owner);
        Ok(result)
    }

    pub(crate) async fn persist(&self, source: SelectionSource, session_id: Option<Uuid>) {
        let manager = self.manager().await;
        if let Err(error) = persist_current_state_result(&manager).await {
            log::warn!(
                "[selection] persistence failed source={} session={:?}: {}",
                source,
                session_id,
                error
            );
        }
    }
}
