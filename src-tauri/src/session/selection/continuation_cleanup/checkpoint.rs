//! Checkpoint helpers for `OwnedReplacementContinuation` used by `continuation_cleanup`.
//!
//! Starts a checkpoint publication after source validation, records the pending carrier,
//! captures transferred work once the finalized publish is fully settled, and checks
//! pending-cleanup claims and runtime absence. State is read from the continuation's owner
//! lock; the publish, reconcile and cleanup flows themselves live in the parent module.

use std::sync::{Arc, Mutex};

use uuid::Uuid;

use super::super::replacement::{
    LifecycleEffectGuard, LifecycleEffectSlot, OwnedEffectState, OwnedLaunchReceipt,
    OwnedReplacementContinuation, OwnedReplacementDescriptor, PendingCleanupEvidence,
    PendingRollback, ReplacementCleanupAuthority, ReplacementStage, ReplacementState,
    RollbackCause, RollbackCursor, RollbackRow, SourceExecutorObservation,
};
use super::super::SelectionCoordinator;
use crate::config::sessions_persistence::PublishedCarrier;
use crate::pty::manager::{PtyManager, ResponseCloseState};
use crate::session::manager::{
    acquire_carrier_mutation_permit, CarrierMutationPermit, CarrierSnapshot, PendingCreateBinding,
    SessionManager,
};

#[cfg(test)]
use super::super::replacement_fixture::ReplacementFixtureCut;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[cfg(test)]
#[derive(Default)]
pub(crate) struct WakeRollbackFixture {
    pub(crate) before_claim: AtomicUsize,
    pub(crate) before_close: AtomicUsize,
    pub(crate) before_release: AtomicUsize,
    pub(crate) refuse_precreate_source: AtomicBool,
    pub(crate) refuse_pending_source: AtomicBool,
    pub(crate) generic_retained: AtomicBool,
    pub(crate) removed: AtomicUsize,
}

#[cfg(test)]
static WAKE_ROLLBACK_FIXTURE: Mutex<Option<Arc<WakeRollbackFixture>>> = Mutex::new(None);

#[cfg(test)]
fn wake_rollback_fixture() -> Option<Arc<WakeRollbackFixture>> {
    WAKE_ROLLBACK_FIXTURE.lock().unwrap().clone()
}

#[cfg(test)]
fn fixture_hold_pty(
    pty: &Mutex<PtyManager>,
    counter: impl FnOnce(&WakeRollbackFixture) -> &AtomicUsize,
) -> Option<std::sync::MutexGuard<'_, PtyManager>> {
    let fixture = wake_rollback_fixture()?;
    counter(&fixture)
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .ok()?;
    Some(pty.lock().unwrap())
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WakeRollbackObservation {
    pub(crate) attempt: Option<Uuid>,
    pub(crate) nonce: Option<Uuid>,
    pub(crate) cursor: Option<&'static str>,
    pub(crate) terminal: bool,
    pub(crate) typed_refusal: bool,
    pub(crate) backend_submitted: bool,
    pub(crate) finalizer_accepted: bool,
    pub(crate) recorded_error: bool,
    pub(crate) holds_create_and_work: bool,
}

const ROLLBACK_EFFECT: usize = LifecycleEffectSlot::PendingRollback as usize;

struct RollbackIdentity {
    binding: PendingCreateBinding,
    carrier: PublishedCarrier,
    launch: Option<OwnedLaunchReceipt>,
    tracker: ResponseCloseState,
}

impl RollbackIdentity {
    async fn pending_row(&self, manager: &SessionManager) -> RollbackRow {
        manager
            .get_pending_session(self.binding)
            .await
            .as_ref()
            .map_or(
                RollbackRow::InitialNoRow,
                OwnedReplacementContinuation::exact_rollback_row,
            )
    }

    fn preparing_matches(
        &self,
        permit: &CarrierMutationPermit,
        current: &CarrierSnapshot,
        operation: Uuid,
        row: RollbackRow,
    ) -> bool {
        let id = self.binding.session_id();
        let present = current.rows.iter().find(|row| row.id == id);
        let row_matches = match row {
            RollbackRow::InitialNoRow => present.is_none() && !current.pending_ids.contains(&id),
            RollbackRow::ExactRemoval { token, backend } => {
                current.pending_ids.contains(&id)
                    && present.is_some_and(|row| row.token == token && row.backend_kind == backend)
            }
        };
        row_matches
            && crate::config::sessions_persistence::renew_carrier_replacement(
                permit,
                &self.carrier,
                operation,
                self.carrier.session_id,
                id,
                current,
            )
            .is_some()
    }

    fn tracker_matches(&self, pty: &PtyManager) -> bool {
        self.tracker.same_state(&pty.response_close_state()) && self.tracker.is_healthy()
    }
}

struct RollbackResume<'a> {
    continuation: &'a OwnedReplacementContinuation,
    binding: PendingCreateBinding,
    nonce: Uuid,
    attempt: Uuid,
}

impl RollbackResume<'_> {
    fn with_claim<T>(&self, apply: impl FnOnce(&mut ReplacementState) -> Option<T>) -> Option<T> {
        let mut state = self.continuation.owner.state.lock().ok()?;
        let progress = state.replacement.as_ref()?;
        let rollback = progress.rollback?;
        if state.executor_nonce != self.continuation.executor_nonce
            || !matches!(
                state.stage,
                ReplacementStage::Admitted | ReplacementStage::PendingCreate
            )
            || progress.binding != self.binding
            || progress.preparing_nonce != self.continuation.owner.operation
            || progress.cleanup != ReplacementCleanupAuthority::Claimed(self.nonce)
            || progress.committed.is_some()
            || progress.initial_b.is_some()
            || progress.pending_carrier.is_some()
            || !rollback.in_flight
            || rollback.attempt != self.attempt
            || !matches!(&state.effects[ROLLBACK_EFFECT],
                OwnedEffectState::Running(attempt) if *attempt == self.attempt)
        {
            return None;
        }
        apply(&mut state)
    }

    fn advance(&self, expected: RollbackCursor, next: RollbackCursor) -> bool {
        self.with_claim(|state| {
            let rollback = state.replacement.as_mut()?.rollback.as_mut()?;
            if rollback.cursor != expected {
                return None;
            }
            rollback.cursor = next;
            Some(())
        })
        .is_some()
    }
}

impl Drop for RollbackResume<'_> {
    fn drop(&mut self) {
        let _ = self.with_claim(|state| {
            let progress = state.replacement.as_mut()?;
            let rollback = progress.rollback.as_mut()?;
            rollback.in_flight = false;
            if matches!(
                rollback.cursor,
                RollbackCursor::CloseSubmitted | RollbackCursor::PreparingReleased
            ) {
                state.effects[ROLLBACK_EFFECT] = OwnedEffectState::Ambiguous(self.attempt);
                progress.cleanup = ReplacementCleanupAuthority::Unknown;
                state.stage = ReplacementStage::Ambiguous;
            }
            Some(())
        });
    }
}

impl OwnedReplacementContinuation {
    #[cfg(test)]
    pub(crate) fn install_wake_rollback_fixture() -> Arc<WakeRollbackFixture> {
        let fixture = Arc::new(WakeRollbackFixture::default());
        *WAKE_ROLLBACK_FIXTURE.lock().unwrap() = Some(fixture.clone());
        fixture
    }

    #[cfg(test)]
    pub(crate) fn fixture_source_validator(
        &self,
        refuse: impl FnOnce(&WakeRollbackFixture) -> &AtomicBool,
    ) -> Self {
        let mut result = self.clone();
        if let Some(fixture) = wake_rollback_fixture() {
            if refuse(&fixture).swap(false, Ordering::Relaxed) {
                let original = self
                    .source_validation
                    .clone()
                    .expect("actual wake validator");
                result.source_validation = Some(Arc::new(move || {
                    let check = original();
                    Box::pin(async move {
                        check.await?;
                        Err(super::super::replacement::OwnedSourceStop::StoreTransient)
                    })
                }));
            }
        }
        result
    }

    #[cfg(test)]
    pub(super) fn fixture_suppresses_pending_receipt(&self) -> bool {
        wake_rollback_fixture().is_some_and(|fixture| {
            fixture.generic_retained.load(Ordering::Relaxed) && self.source_stop().is_some()
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture_wake_rollback(&self) -> WakeRollbackObservation {
        let state = self.owner.state.lock().unwrap();
        let progress = state.replacement.as_ref().unwrap();
        WakeRollbackObservation {
            attempt: progress.rollback.map(|claim| claim.attempt),
            nonce: match progress.cleanup {
                ReplacementCleanupAuthority::Claimed(nonce) => Some(nonce),
                _ => None,
            },
            cursor: progress.rollback.map(|claim| match claim.cursor {
                RollbackCursor::ClaimedBeforeSubmit => "claimed",
                RollbackCursor::CloseSubmitted => "close-submitted",
                RollbackCursor::RuntimeAbsent => "runtime-absent",
                RollbackCursor::RowAbsent => "row-absent",
                RollbackCursor::PreparingReleased => "preparing-released",
                RollbackCursor::Completed => "completed",
            }),
            terminal: state.replacement_terminal_is_verified(),
            typed_refusal: progress
                .pending_source_refusal
                .as_ref()
                .is_some_and(|refusal| refusal.returned_to_wake),
            backend_submitted: !matches!(
                state.effects[LifecycleEffectSlot::BackendSpawn as usize],
                OwnedEffectState::NotStarted
            ),
            finalizer_accepted: progress.initial_b.is_some(),
            recorded_error: matches!(state.completion, Some(Err(_))),
            holds_create_and_work: progress.create_reservation.is_some()
                && progress.work_reservation.is_some(),
        }
    }

    #[cfg(test)]
    pub(crate) async fn fixture_replace_rollback_preparing(&self, manager: &SessionManager) {
        let permit = acquire_carrier_mutation_permit().await;
        let identity = self.rollback_identity().unwrap();
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                use crate::config::sessions_persistence::{
                    abort_owned_carrier_replacement, begin_carrier_replacement,
                };
                assert!(abort_owned_carrier_replacement(
                    &permit,
                    &identity.carrier,
                    self.operation(),
                    identity.binding.session_id(),
                    current,
                ));
                assert!(begin_carrier_replacement(
                    &permit,
                    &identity.carrier,
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    current,
                ));
            })
            .await;
    }

    pub(super) async fn begin_checkpoint_publication(
        &self,
    ) -> Option<(
        &super::super::ResolvedReplacement,
        PendingCreateBinding,
        LifecycleEffectGuard,
    )> {
        if !self.validate_source().await {
            return None;
        }
        let (descriptor, binding, rename) = self.checkpoint_publication()?;
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforePublication)
            .await
        {
            return None;
        }
        Some((descriptor, binding, rename))
    }

    pub(super) fn record_pending_carrier(
        &self,
        carrier: impl FnOnce() -> PublishedCarrier,
    ) -> bool {
        match self.owner.state.lock() {
            Ok(mut state) => {
                if let Some(progress) = state.replacement.as_mut() {
                    progress.pending_carrier = Some(carrier());
                }
            }
            _ => {
                return false;
            }
        }
        true
    }

    pub(super) fn capture_transferred_work(
        &self,
    ) -> Option<(PublishedCarrier, OwnedLaunchReceipt, Uuid)> {
        self.owner.state.lock().ok().and_then(|state| {
            let progress = state.replacement.as_ref()?;
            let launch = progress.launch.as_ref()?;
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
                || !progress.work_transferred
                || !progress.preparing_released
                || !progress.reservations_released
                || !progress.retired
                || !progress.retirement_cleanup_completed
                || !progress.activity_completed
                || progress.initial_b.is_none()
                || !matches!(
                    progress.context_prerequisite,
                    None | Some(OwnedEffectState::Completed { .. })
                )
                || progress.create_reservation.is_some()
                || progress.work_reservation.is_some()
                || !matches!(state.completion.as_ref(),
                    Some(Ok(info)) if info.id == launch.binding.session_id().to_string())
                || !state.effects.iter().all(|effect| {
                    matches!(
                        effect,
                        OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }
                    )
                })
            {
                return None;
            }
            let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
                return None;
            };
            Some((
                progress.current_checkpoint.clone()?,
                launch.clone(),
                descriptor.previous.id,
            ))
        })
    }

    fn rollback_identity(&self) -> Option<RollbackIdentity> {
        let state = self.owner.state.lock().ok()?;
        let progress = state.replacement.as_ref()?;
        if state.executor_nonce != self.executor_nonce
            || !progress.preparing_started
            || progress.preparing_nonce != self.owner.operation
            || progress.initial_b.is_some()
            || progress.committed.is_some()
            || progress.pending_carrier.is_some()
            || !matches!(
                state.stage,
                ReplacementStage::Admitted | ReplacementStage::PendingCreate
            )
        {
            return None;
        }
        Some(RollbackIdentity {
            binding: progress.binding,
            carrier: progress.previous_carrier.clone(),
            launch: progress.launch.clone(),
            tracker: progress.tracker.clone()?,
        })
    }

    fn rollback_owner_is_current(&self) -> bool {
        self.coordinator
            .upgrade()
            .is_some_and(|inner| SelectionCoordinator { inner }.owns_rollback_entry(&self.owner))
    }

    fn begin_rollback(
        &self,
        identity: &RollbackIdentity,
        row: RollbackRow,
        evidence: PendingCleanupEvidence<'_>,
    ) -> Option<RollbackResume<'_>> {
        let mut state = self.owner.state.lock().ok()?;
        let stage = state.stage;
        let progress = state.replacement.as_ref()?;
        if state.executor_nonce != self.executor_nonce
            || progress.binding != identity.binding
            || progress.launch.as_ref().map(|launch| launch.generation)
                != identity.launch.as_ref().map(|launch| launch.generation)
            || !matches!(
                (stage, row),
                (ReplacementStage::Admitted, RollbackRow::InitialNoRow)
                    | (
                        ReplacementStage::PendingCreate,
                        RollbackRow::ExactRemoval { .. }
                    )
            )
            || !matches!(state.effects[ROLLBACK_EFFECT], OwnedEffectState::NotStarted)
            || state.effects.iter().any(|effect| {
                matches!(
                    effect,
                    OwnedEffectState::Running(_) | OwnedEffectState::Ambiguous(_)
                )
            })
            || progress.cleanup != ReplacementCleanupAuthority::Executor(self.executor_nonce)
        {
            return None;
        }
        let cause = match evidence {
            PendingCleanupEvidence::ActualSetupFailure(error) => {
                state.completion = Some(Err(error.to_owned()));
                RollbackCause::ActualSetupFailure
            }
            PendingCleanupEvidence::ObservedSourceStoppedOrReturnedFailure
                if stage == ReplacementStage::PendingCreate
                    && matches!(
                        progress.source_executor,
                        SourceExecutorObservation::Stopped
                            | SourceExecutorObservation::Returned { failed: true }
                    ) =>
            {
                RollbackCause::ShutdownStopped
            }
            PendingCleanupEvidence::WakeHandoff => Self::no_submit_rollback_cause(&state, row)?,
            PendingCleanupEvidence::ObservedSourceStoppedOrReturnedFailure => return None,
        };
        let nonce = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        let progress = state.replacement.as_mut()?;
        progress.cleanup = ReplacementCleanupAuthority::Claimed(nonce);
        progress.rollback = Some(PendingRollback {
            attempt,
            cause,
            row,
            cursor: RollbackCursor::ClaimedBeforeSubmit,
            in_flight: true,
        });
        state.effects[ROLLBACK_EFFECT] = OwnedEffectState::Running(attempt);
        Some(RollbackResume {
            continuation: self,
            binding: identity.binding,
            nonce,
            attempt,
        })
    }

    fn no_submit_rollback_cause(
        state: &ReplacementState,
        row: RollbackRow,
    ) -> Option<RollbackCause> {
        let progress = state.replacement.as_ref()?;
        if progress.launch.is_some()
            || !state
                .effects
                .iter()
                .all(|effect| matches!(effect, OwnedEffectState::NotStarted))
        {
            return None;
        }
        match (state.stage, progress.source_executor, row) {
            (
                ReplacementStage::Admitted,
                SourceExecutorObservation::Stopped,
                RollbackRow::InitialNoRow,
            ) if progress.context_prerequisite.is_none() => Some(RollbackCause::AdmittedStopped),
            (
                ReplacementStage::PendingCreate,
                SourceExecutorObservation::Returned { failed: false },
                RollbackRow::ExactRemoval { token, .. },
            ) if progress.source_stop.is_some()
                && progress
                    .pending_source_refusal
                    .as_ref()
                    .is_some_and(|refusal| refusal.token == token && refusal.returned_to_wake)
                && matches!(
                    progress.context_prerequisite,
                    None | Some(OwnedEffectState::Completed { outcome: Ok(_), .. })
                ) =>
            {
                Some(RollbackCause::PendingSourceRefusedBeforeSubmit)
            }
            _ => None,
        }
    }

    fn resume_rollback(
        &self,
        binding: PendingCreateBinding,
        evidence: &PendingCleanupEvidence<'_>,
    ) -> Result<Option<(RollbackResume<'_>, PendingRollback)>, ()> {
        let mut state = self.owner.state.lock().map_err(|_| ())?;
        if state.executor_nonce != self.executor_nonce {
            return Err(());
        }
        let progress = state.replacement.as_ref().ok_or(())?;
        let Some(rollback) = progress.rollback else {
            return Ok(None);
        };
        let ReplacementCleanupAuthority::Claimed(nonce) = progress.cleanup else {
            return Err(());
        };
        if rollback.in_flight
            || progress.binding != binding
            || !matches!(
                rollback.cursor,
                RollbackCursor::ClaimedBeforeSubmit
                    | RollbackCursor::RuntimeAbsent
                    | RollbackCursor::RowAbsent
            )
            || (matches!(evidence, PendingCleanupEvidence::WakeHandoff)
                && !matches!(
                    progress.source_executor,
                    SourceExecutorObservation::Stopped | SourceExecutorObservation::Returned { .. }
                ))
            || !matches!(&state.effects[ROLLBACK_EFFECT],
                OwnedEffectState::Running(attempt) if *attempt == rollback.attempt)
            || state.effects.iter().enumerate().any(|(slot, effect)| {
                slot != ROLLBACK_EFFECT
                    && matches!(
                        effect,
                        OwnedEffectState::Running(_) | OwnedEffectState::Ambiguous(_)
                    )
            })
        {
            return Err(());
        }
        state
            .replacement
            .as_mut()
            .and_then(|progress| progress.rollback.as_mut())
            .ok_or(())?
            .in_flight = true;
        Ok(Some((
            RollbackResume {
                continuation: self,
                binding,
                nonce,
                attempt: rollback.attempt,
            },
            rollback,
        )))
    }

    pub(super) async fn progress_pending_rollback(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        evidence: PendingCleanupEvidence<'_>,
    ) -> bool {
        let permit = acquire_carrier_mutation_permit().await;
        let Some(identity) = self.rollback_identity() else {
            return false;
        };
        if !self.rollback_owner_is_current() {
            return false;
        }
        let (resume, row, cursor) = match self.resume_rollback(identity.binding, &evidence) {
            Ok(Some((resume, claim))) => (resume, claim.row, claim.cursor),
            Ok(None) => {
                let row = identity.pending_row(manager).await;
                if !manager
                    .with_current_carrier_snapshot(&permit, |current| {
                        identity.preparing_matches(&permit, current, self.owner.operation, row)
                    })
                    .await
                    || !self.rollback_runtime_is_current(&identity, row, pty)
                {
                    return false;
                }
                let Some(resume) = self.begin_rollback(&identity, row, evidence) else {
                    return false;
                };
                (resume, row, RollbackCursor::ClaimedBeforeSubmit)
            }
            Err(()) => return false,
        };
        drop(permit);
        if cursor == RollbackCursor::ClaimedBeforeSubmit
            && !self
                .close_rollback_runtime(manager, pty, &identity, row, &resume)
                .await
        {
            return false;
        }
        let permit = acquire_carrier_mutation_permit().await;
        if !self.rollback_owner_is_current() {
            return false;
        }
        if cursor != RollbackCursor::RowAbsent
            && !self
                .remove_rollback_row(manager, pty, &identity, row, &resume, &permit)
                .await
        {
            return false;
        }
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                self.release_rollback_preparing(pty, &identity, &resume, &permit, current)
            })
            .await
    }

    fn rollback_runtime_is_current(
        &self,
        identity: &RollbackIdentity,
        row: RollbackRow,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        #[cfg(test)]
        let _contended = fixture_hold_pty(pty, |fixture| &fixture.before_claim);
        pty.try_lock().is_ok_and(|pty| {
            identity.tracker_matches(&pty) && self.rollback_launch_matches(identity, row, &pty)
        })
    }

    fn rollback_launch_matches(
        &self,
        identity: &RollbackIdentity,
        row: RollbackRow,
        pty: &PtyManager,
    ) -> bool {
        let id = identity.binding.session_id();
        let generation = identity.tracker.generation(id);
        match identity.launch.as_ref() {
            Some(launch) => {
                let expected_generation = pty.backend_kind(id).map(|_| launch.generation);
                row == (RollbackRow::ExactRemoval {
                    token: launch.token,
                    backend: launch.backend,
                }) && launch.operation == self.owner.operation
                    && launch.binding == identity.binding
                    && launch.preparing_nonce == self.owner.operation
                    && generation == expected_generation
                    && self.pending_cleanup_runtime_matches(identity.binding, Some(launch), pty)
            }
            None => {
                generation.is_none()
                    && self.pending_cleanup_runtime_matches(identity.binding, None, pty)
            }
        }
    }

    fn rollback_runtime_absent(&self, identity: &RollbackIdentity, pty: &PtyManager) -> bool {
        identity.tracker_matches(pty)
            && identity
                .tracker
                .generation(identity.binding.session_id())
                .is_none()
            && self.pending_cleanup_runtime_matches(identity.binding, None, pty)
    }

    async fn close_rollback_runtime(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        identity: &RollbackIdentity,
        row: RollbackRow,
        resume: &RollbackResume<'_>,
    ) -> bool {
        use RollbackCursor::{ClaimedBeforeSubmit, CloseSubmitted, RuntimeAbsent};
        let permit = acquire_carrier_mutation_permit().await;
        if !self.rollback_owner_is_current()
            || identity.pending_row(manager).await != row
            || !manager
                .with_current_carrier_snapshot(&permit, |current| {
                    identity.preparing_matches(&permit, current, self.owner.operation, row)
                })
                .await
        {
            return false;
        }
        #[cfg(test)]
        let _contended = fixture_hold_pty(pty, |fixture| &fixture.before_close);
        let Ok(pty) = pty.try_lock() else {
            return false;
        };
        if !identity.tracker_matches(&pty) || !self.rollback_launch_matches(identity, row, &pty) {
            return false;
        }
        let id = identity.binding.session_id();
        let Some(kind) = pty.backend_kind(id) else {
            return self.rollback_runtime_absent(identity, &pty)
                && resume.advance(ClaimedBeforeSubmit, RuntimeAbsent);
        };
        if !resume.advance(ClaimedBeforeSubmit, CloseSubmitted) {
            return false;
        }
        drop(permit);
        pty.kill_for_kind(id, kind).is_ok()
            && self.rollback_runtime_absent(identity, &pty)
            && resume.advance(CloseSubmitted, RuntimeAbsent)
    }

    async fn remove_rollback_row(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        identity: &RollbackIdentity,
        row: RollbackRow,
        resume: &RollbackResume<'_>,
        permit: &CarrierMutationPermit,
    ) -> bool {
        use RollbackCursor::{RowAbsent, RuntimeAbsent};
        let preparing = manager
            .with_current_carrier_snapshot(permit, |current| {
                identity.preparing_matches(permit, current, self.owner.operation, row)
                    && (row != RollbackRow::InitialNoRow
                        || pty.try_lock().is_ok_and(|pty| {
                            self.rollback_runtime_absent(identity, &pty)
                                && resume.advance(RuntimeAbsent, RowAbsent)
                        }))
            })
            .await;
        if !preparing || row == RollbackRow::InitialNoRow {
            return preparing;
        }
        manager
            .rollback_exact_pending_with_receipt(permit, identity.binding, |pending, remove| {
                if row != Self::exact_rollback_row(pending) {
                    return false;
                }
                pty.try_lock().is_ok_and(|pty| {
                    self.rollback_runtime_absent(identity, &pty)
                        && resume
                            .with_claim(|state| {
                                let receipt = state.replacement.as_mut()?.rollback.as_mut()?;
                                if receipt.cursor != RuntimeAbsent {
                                    return None;
                                }
                                remove();
                                receipt.cursor = RowAbsent;
                                #[cfg(test)]
                                if let Some(fixture) = wake_rollback_fixture() {
                                    fixture.removed.fetch_add(1, Ordering::Relaxed);
                                }
                                Some(())
                            })
                            .is_some()
                })
            })
            .await
    }

    fn release_rollback_preparing(
        &self,
        pty: &Arc<Mutex<PtyManager>>,
        identity: &RollbackIdentity,
        resume: &RollbackResume<'_>,
        permit: &CarrierMutationPermit,
        current: &CarrierSnapshot,
    ) -> bool {
        #[cfg(test)]
        let _contended = fixture_hold_pty(pty, |fixture| &fixture.before_release);
        let Ok(pty) = pty.try_lock() else {
            return false;
        };
        let id = identity.binding.session_id();
        if !self.rollback_runtime_absent(identity, &pty)
            || !identity.tracker.publishers_bound()
            || current.pending_ids.contains(&id)
            || current.rows.iter().any(|row| row.id == id)
        {
            return false;
        }
        let reservations = resume.with_claim(|state| {
            let progress = state.replacement.as_mut()?;
            let receipt = progress.rollback.as_mut()?;
            if receipt.cursor != RollbackCursor::RowAbsent
                || !crate::config::sessions_persistence::abort_owned_carrier_replacement(
                    permit,
                    &identity.carrier,
                    self.owner.operation,
                    id,
                    current,
                )
            {
                return None;
            }
            receipt.cursor = RollbackCursor::PreparingReleased;
            progress.preparing_released = true;
            Some((
                progress.create_reservation.take(),
                progress.work_reservation.take(),
            ))
        });
        if reservations.is_none() {
            return false;
        }
        drop(reservations);
        if !identity.tracker_matches(&pty) {
            return false;
        }
        resume
            .with_claim(|state| {
                let progress = state.replacement.as_mut()?;
                let receipt = progress.rollback.as_mut()?;
                if receipt.cursor != RollbackCursor::PreparingReleased {
                    return None;
                }
                receipt.cursor = RollbackCursor::Completed;
                receipt.in_flight = false;
                progress.rollback_completed = true;
                progress.reservations_released = true;
                state.effects[ROLLBACK_EFFECT] = OwnedEffectState::Completed {
                    attempt: resume.attempt,
                    outcome: Ok(None),
                };
                Some(())
            })
            .is_some()
    }
}
