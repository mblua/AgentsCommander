//! Publication, rollback and cleanup steps of `OwnedReplacementContinuation`.
//!
//! Builds the finalized carry capability, publishes and reconciles replacement checkpoints,
//! finishes rolled-back or transferred work, and claims and kills pending runtimes.
//! Also gates lifecycle effect slots via `start_effect`. It mutates the owner state but
//! does not define it (see `replacement`); launch steps and applying the carry live in
//! `continuation_launch`, on-disk writes in `crate::config::sessions_persistence`.

mod checkpoint;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use uuid::Uuid;

use crate::config::sessions_persistence::{
    prepare_owned_replacement_publication, publish_owned_replacement,
    OwnedReplacementPublicationOutcome,
};
use crate::pty::manager::PtyManager;
use crate::session::manager::{
    acquire_carrier_mutation_permit, CarrierMutationPermit, PendingCreateBinding, SessionManager,
};

use super::envelope::FinalizedOwnedCarryCapability;
use super::replacement::{
    LifecycleEffectGuard, LifecycleEffectSlot, OwnedEffectState, OwnedLaunchReceipt,
    OwnedReplacementContinuation, OwnedReplacementDescriptor, PendingCleanupEvidence,
    PendingSourceRefusal, ReplacementCleanupAuthority, ReplacementStage, RollbackCursor,
    RollbackIdentity, RollbackResume, RollbackRow, SourceExecutorObservation, WakePathValidation,
};
#[cfg(test)]
use super::replacement_fixture::{ReplacementFixtureAction, ReplacementFixtureCut};
use super::SelectionCoordinator;

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
}

impl OwnedReplacementContinuation {
    /// Some(None) is the explicit newer-B path, never a replacement baseline.
    #[expect(
        clippy::option_option,
        reason = "None denies; Some(None): newer-B; Some(Some(capability)): finalized capability"
    )]
    pub(super) fn finalized_carry_capability(
        &self,
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> Option<Option<FinalizedOwnedCarryCapability>> {
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return None;
        };
        let (launch, carrier, token, version, binding, nonce, generation) = {
            let state = self.owner.state.lock().ok()?;
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedCarry
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            let initial = progress.initial_b.as_ref()?;
            if initial.operation != self.owner.operation
                || initial.preparing_nonce != progress.preparing_nonce
                || initial.binding != progress.binding
            {
                return None;
            }
            (
                progress.launch.clone()?,
                progress.previous_carrier.clone(),
                initial.token,
                initial.carrier_version,
                initial.binding,
                initial.preparing_nonce,
                initial.launch_generation,
            )
        };
        if launch.token != token
            || launch.generation != generation
            || launch.operation != self.owner.operation
            || launch.preparing_nonce != nonce
            || !Self::launch_is_current(&launch, current, pty)
            || current.pending_ids.contains(&binding.session_id())
        {
            return None;
        }
        let previous_carrier = crate::config::sessions_persistence::renew_carrier_replacement(
            permit,
            &carrier,
            nonce,
            descriptor.previous.id,
            binding.session_id(),
            current,
        )?;
        previous_carrier.retired_local_generation()?;
        descriptor
            .replica_identity
            .as_ref()
            .filter(|physical| previous_carrier.retired_local_replica_matches(physical))?;
        let current_version = *current.versions.get(&binding.session_id())?;
        if current_version == u64::MAX {
            return None;
        }
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.previous_carrier = previous_carrier;
            progress.current_previous = current
                .rows
                .iter()
                .find(|row| row.id == descriptor.previous.id)?
                .clone();
        }
        if current_version != version {
            return Some(None);
        }
        let previous = current.rows.iter().find(|row| {
            row.id == descriptor.previous.id && row.token == descriptor.previous.token
        })?;
        let fresh_matches = self.owner.state.lock().is_ok_and(|state| {
            state.replacement.as_ref().is_some_and(|progress| {
                progress.launch_start_fresh == Some(previous.start_fresh_on_restore)
            })
        });
        if !fresh_matches {
            self.note_retained_work(
                "replacement launch fresh decision changed after spawn; current A/B preserved"
                    .into(),
            );
            return None;
        }
        Some(Some(FinalizedOwnedCarryCapability {
            owner: self.owner.clone(),
            operation: self.owner.operation,
            preparing_nonce: nonce,
            previous: previous.id,
            binding,
            token,
            expected_version: version,
            telegram_bot_id: previous.telegram_bot_id.clone(),
            communication: previous.communication.clone(),
            start_fresh_on_restore: previous.start_fresh_on_restore,
        }))
    }

    pub(super) fn publication_region_current(
        &self,
        current: &crate::session::manager::CarrierSnapshot,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        let captured = self.owner.state.lock().ok().and_then(|state| {
            let progress = state.replacement.as_ref()?;
            Some((
                progress.launch.clone()?,
                progress.previous_carrier.clone(),
                progress.retired,
            ))
        });
        let Some((launch, previous, retired)) = captured else {
            return false;
        };
        Self::launch_is_current(&launch, current, pty)
            && if retired {
                !current.rows.iter().any(|row| row.id == previous.session_id)
            } else {
                current.versions.get(&previous.session_id) == Some(&previous.version)
                    && current
                        .rows
                        .iter()
                        .any(|row| row.id == previous.session_id && row.token == previous.token)
            }
    }

    fn reset_recheckpoint_effects(&self) {
        // Recheckpoint is a new required native publication; retain the first
        // strict receipt separately before reusing this fixed effect slot.
        if let Ok(mut state) = self.owner.state.lock() {
            if state.stage == ReplacementStage::FinalizedRecheckpoint {
                state.effects[LifecycleEffectSlot::CarrierRename as usize] =
                    OwnedEffectState::NotStarted;
                state.effects[LifecycleEffectSlot::Readback as usize] =
                    OwnedEffectState::NotStarted;
            }
        }
    }

    pub(super) async fn publish_checkpoint(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        project_paths: &[String],
        source: crate::config::sessions_persistence::PublishedCarrier,
    ) -> bool {
        let Some((descriptor, binding, rename)) = self.begin_checkpoint_publication().await else {
            return false;
        };
        let mut prepared = match prepare_owned_replacement_publication(
            manager,
            &source,
            self.owner.operation,
            descriptor.previous.id,
            binding.session_id(),
            project_paths,
            || self.validate_source(),
        )
        .await
        {
            Ok(Some(prepared)) => prepared,
            Ok(None) => {
                rename.complete(Err("replacement publication requires recapture".into()));
                return false;
            }
            Err(error) => {
                rename.complete(Err(error));
                return false;
            }
        };
        #[cfg(test)]
        {
            if let Some(kind) = self.publication_fixture_action() {
                prepared.attach_fixture_action(self.owner.operation, &descriptor.directory, kind);
            }
            if !self
                .fixture_reached(ReplacementFixtureCut::PublicationPrepared)
                .await
            {
                return false;
            }
        }
        if !self.record_pending_carrier(|| prepared.destination_key()) {
            return false;
        }
        let Some(readback) = self.start_effect(LifecycleEffectSlot::Readback) else {
            return false;
        };
        for attempt in 0..4 {
            if !self.validate_source().await {
                return false;
            }
            let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
            let valid = manager
                .with_current_carrier_snapshot(&permit, |current| {
                    self.publication_region_current(current, pty)
                })
                .await;
            if !valid {
                rename.complete(Err("replacement identity changed before publication".into()));
                readback.skip();
                return false;
            }
            let outcome = publish_owned_replacement(manager, &permit, &mut prepared, |current| {
                self.publication_region_current(current, pty)
            })
            .await;
            drop(permit);
            match outcome {
                OwnedReplacementPublicationOutcome::Confirmed(carrier) => {
                    return self.confirm_checkpoint(carrier, rename, readback);
                }
                OwnedReplacementPublicationOutcome::Pending(carrier) => {
                    rename.complete(Ok(Some(carrier.session_id.to_string())));
                    return self
                        .reconcile_pending_checkpoint(
                            manager,
                            pty,
                            descriptor.previous.id,
                            carrier,
                            readback,
                        )
                        .await;
                }
                OwnedReplacementPublicationOutcome::Recapture => {
                    rename.complete(Err("replacement publication stale before rename".into()));
                    readback.skip();
                    return false;
                }
                OwnedReplacementPublicationOutcome::Failed(error) => {
                    if attempt == 3 {
                        rename.complete(Err(error));
                        readback.skip();
                        return false;
                    }
                }
            }
            // Exact existing native backoffs, with no U/M/lifecycle/leaf guard.
            tokio::time::sleep(Duration::from_millis([10, 50, 200][attempt])).await;
        }
        false
    }

    async fn reconcile_pending_checkpoint(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        previous: Uuid,
        carrier: crate::config::sessions_persistence::PublishedCarrier,
        readback: LifecycleEffectGuard,
    ) -> bool {
        {
            let Ok(mut state) = self.owner.state.lock() else {
                return false;
            };
            let Some(progress) = state.replacement.as_mut() else {
                return false;
            };
            progress.pending_carrier = Some(carrier.clone());
            state.stage = ReplacementStage::FinalizedPendingReadback;
        }
        // The rename is observed. Native recovery reads this same
        // publication only; it never repeats preparation or rename.
        for delay in [10, 50, 200] {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if !self.validate_source().await {
                return false;
            }
            let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
            let confirmed = manager
                .with_current_carrier_snapshot(&permit, |current| {
                    if !self.publication_region_current(current, pty) {
                        return None;
                    }
                    use crate::config::sessions_persistence as persistence;
                    persistence::reconcile_owned_replacement_readback(
                        &permit,
                        &carrier,
                        self.owner.operation,
                        previous,
                        current,
                    )
                })
                .await;
            drop(permit);
            if let Some(confirmed) = confirmed {
                readback.complete(Ok(Some(confirmed.nonce.to_string())));
                let Ok(mut state) = self.owner.state.lock() else {
                    return false;
                };
                let Some(progress) = state.replacement.as_mut() else {
                    return false;
                };
                if progress.first_checkpoint.is_none() {
                    progress.first_checkpoint = Some(confirmed.clone());
                }
                progress.current_checkpoint = Some(confirmed);
                progress.pending_carrier = None;
                return true;
            }
        }
        readback.complete(Err("replacement readback is Pending".into()));
        if let Ok(mut state) = self.owner.state.lock() {
            state.retained_reason =
                Some("replacement native rename observed; strict readback unavailable".into());
        }
        false
    }

    pub(crate) fn record_pending_source_refusal(&self, binding: PendingCreateBinding, token: Uuid) {
        #[cfg(test)]
        if self.fixture_suppresses_pending_receipt() {
            return;
        }
        let Ok(mut state) = self.owner.state.lock() else {
            return;
        };
        if state.executor_nonce != self.executor_nonce
            || state.stage != ReplacementStage::PendingCreate
        {
            return;
        }
        if let Some(progress) = state.replacement.as_mut() {
            if progress.binding == binding
                && progress.cleanup == ReplacementCleanupAuthority::Executor(self.executor_nonce)
                && progress.source_executor == SourceExecutorObservation::Active
                && progress.source_stop.is_some()
            {
                progress.pending_source_refusal = Some(PendingSourceRefusal {
                    token,
                    returned_to_wake: false,
                });
            }
        }
    }

    pub(crate) fn relay_pending_source_refusal(&self) {
        if let Ok(mut state) = self.owner.state.lock() {
            if state.executor_nonce == self.executor_nonce {
                if let Some(refusal) = state
                    .replacement
                    .as_mut()
                    .and_then(|progress| progress.pending_source_refusal.as_mut())
                {
                    refusal.returned_to_wake = true;
                }
            }
        }
    }

    pub(crate) fn precommit_rollback_completed(&self) -> bool {
        self.owner.state.lock().is_ok_and(|state| {
            state.executor_nonce == self.executor_nonce
                && state.replacement.as_ref().is_some_and(|progress| {
                    progress.precommit_rollback_verified(matches!(state.completion, Some(Err(_))))
                })
        })
    }

    pub(crate) async fn settle_wake_precommit(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        actual_error: Option<&str>,
        paths_current: Option<WakePathValidation<'_>>,
    ) {
        #[cfg(test)]
        if paths_current.is_some() {
            if let Some(fixture) = Self::fixture_wake_rollback_state() {
                *fixture.probe_candidate.lock().unwrap() = Some(self.owner.operation);
            }
        }
        let released = self.owner.state.lock().is_ok_and(|state| {
            state.executor_nonce == self.executor_nonce
                && state.replacement.as_ref().is_some_and(|progress| {
                    matches!(
                        progress.source_executor,
                        SourceExecutorObservation::Stopped
                            | SourceExecutorObservation::Returned { .. }
                    )
                })
        });
        if !released {
            return;
        }
        if !self.precommit_rollback_completed() {
            let evidence = actual_error.map_or(
                PendingCleanupEvidence::WakeHandoff(paths_current),
                PendingCleanupEvidence::ActualSetupFailure,
            );
            if !self.progress_pending_rollback(manager, pty, evidence).await {
                return;
            }
        }
        #[cfg(test)]
        if let Some(fixture) = Self::fixture_wake_rollback_state() {
            fixture.before_terminal.reach().await;
        }
        if let Some(effect) = self.start_effect(LifecycleEffectSlot::WorkTransfer) {
            effect.complete(Ok(None));
        }
        self.finish_rollback_terminal().await;
    }

    /// The PTY source supplies its existing observed handoff receipt.
    pub(crate) async fn finish_rolled_back_work(
        &self,
        _manager: &SessionManager,
        _pty: &Arc<Mutex<PtyManager>>,
        effect: LifecycleEffectGuard,
        observed: bool,
    ) -> bool {
        if !observed || !self.precommit_rollback_completed() {
            effect.complete(Err("replacement rollback handoff not observed".into()));
            return false;
        }
        effect.complete(Ok(None));
        self.finish_rollback_terminal().await
    }

    async fn finish_rollback_terminal(&self) -> bool {
        let verified = self.owner.state.lock().is_ok_and(|mut state| {
            if state.executor_nonce != self.executor_nonce {
                return false;
            }
            if state.replacement_terminal_is_verified() {
                return state.stage == ReplacementStage::RolledBack;
            }
            if !matches!(
                state.stage,
                ReplacementStage::Admitted | ReplacementStage::PendingCreate
            ) || !matches!(
                state.effects[LifecycleEffectSlot::WorkTransfer as usize],
                OwnedEffectState::Completed { outcome: Ok(_), .. }
            ) || !state.effects.iter().all(|effect| {
                matches!(
                    effect,
                    OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }
                )
            }) {
                return false;
            }
            let actual_err = matches!(state.completion, Some(Err(_)));
            let Some(progress) = state.replacement.as_mut() else {
                return false;
            };
            if !progress.precommit_rollback_verified(actual_err)
                || progress.create_reservation.is_some()
                || progress.work_reservation.is_some()
                || !matches!(
                    progress.context_prerequisite,
                    None | Some(OwnedEffectState::Completed { .. })
                )
            {
                return false;
            }
            progress.work_transferred = true;
            progress.terminal_verified = true;
            state.publication_observed = true;
            state.envelope_released = true;
            state.stage = ReplacementStage::RolledBack;
            true
        });
        if verified {
            #[cfg(test)]
            if !self
                .fixture_reached(ReplacementFixtureCut::BeforeTerminalRemoval)
                .await
            {
                return verified;
            }
            if let Some(inner) = self.coordinator.upgrade() {
                SelectionCoordinator { inner }.remove_terminal_entry(&self.owner);
            }
        }
        verified
    }

    pub(crate) fn record_observed_work_transfer(
        &self,
        effect: LifecycleEffectGuard,
        observed: bool,
    ) -> bool {
        if !observed {
            effect.complete(Err("replacement work transfer was not observed".into()));
            return false;
        }
        effect.complete(Ok(None));
        let reservations = self.owner.state.lock().ok().and_then(|mut state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            let progress = state.replacement.as_mut()?;
            progress.work_transferred = true;
            Some((
                progress.create_reservation.take(),
                progress.work_reservation.take(),
            ))
        });
        let Some(reservations) = reservations else {
            return false;
        };
        drop(reservations);
        true
    }

    pub(crate) async fn finish_observed_work_transfer(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        effect: LifecycleEffectGuard,
        observed: bool,
    ) -> bool {
        if !self.record_observed_work_transfer(effect, observed) {
            return false;
        }
        let released = self.reservation_release_is_observed(pty);
        if !released {
            return false;
        }
        if let Ok(mut state) = self.owner.state.lock() {
            if let Some(progress) = state.replacement.as_mut() {
                progress.reservations_released = true;
            }
        }
        // Receipt of the finalizer result precedes a successful work transfer.
        // Unknown publication stays retained without parking another task.
        if !self
            .owner
            .state
            .lock()
            .is_ok_and(|state| state.publication_observed && state.envelope_released)
        {
            return false;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let captured = self.capture_transferred_work();
        let Some((carrier, launch, previous)) = captured else {
            return false;
        };
        let current = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !Self::launch_is_current(&launch, current, pty)
                    || current.rows.iter().any(|row| row.id == previous)
                {
                    return None;
                }
                crate::config::sessions_persistence::current_protected_carrier(
                    &permit, &carrier, current,
                )
            })
            .await;
        let Some(current) = current else {
            return false;
        };
        let verified = self.owner.state.lock().is_ok_and(|mut state| {
            if state.stage != ReplacementStage::FinalizedPublish {
                return false;
            }
            let Some(progress) = state.replacement.as_mut() else {
                return false;
            };
            progress.current_checkpoint = Some(current);
            progress.terminal_verified = true;
            state.stage = ReplacementStage::Completed;
            true
        });
        drop(permit);
        if verified {
            #[cfg(test)]
            if !self
                .fixture_reached(ReplacementFixtureCut::BeforeTerminalRemoval)
                .await
            {
                return verified;
            }
            if let Some(inner) = self.coordinator.upgrade() {
                SelectionCoordinator { inner }.remove_terminal_entry(&self.owner);
            }
        }
        verified
    }

    /// Called only after this executor's setup body returned a real Err.
    /// Waiter loss or ticket Drop never enters this cleanup path.
    /// Also called by the owned wake core at its two Admitted-stage exits,
    /// before any create call (#3263).
    pub(crate) async fn rollback_failed_create(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        error: &str,
    ) -> bool {
        self.progress_pending_rollback(
            manager,
            pty,
            PendingCleanupEvidence::ActualSetupFailure(error),
        )
        .await
    }

    pub(super) async fn cleanup_stopped_source(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        self.progress_pending_rollback(
            manager,
            pty,
            PendingCleanupEvidence::ObservedSourceStoppedOrReturnedFailure,
        )
        .await
    }

    pub(super) async fn progress_pending_rollback(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        evidence: PendingCleanupEvidence<'_>,
    ) -> bool {
        let paths_current = evidence.wake_path_validation();
        #[cfg(test)]
        if paths_current.is_some() {
            if let Some(fixture) = Self::fixture_wake_rollback_state() {
                fixture.before_probe_permit.notify_one();
            }
        }
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
                        self.rollback_paths_are_current(paths_current)
                            && identity.preparing_matches(
                                &permit,
                                current,
                                self.owner.operation,
                                row,
                            )
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
                .close_rollback_runtime(manager, pty, &identity, row, &resume, paths_current)
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
                .remove_rollback_row(
                    manager,
                    pty,
                    &identity,
                    row,
                    &resume,
                    (&permit, paths_current),
                )
                .await
        {
            return false;
        }
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                self.rollback_paths_are_current(paths_current)
                    && self.release_rollback_preparing(pty, &identity, &resume, &permit, current)
            })
            .await
    }

    async fn close_rollback_runtime(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        identity: &RollbackIdentity,
        row: RollbackRow,
        resume: &RollbackResume<'_>,
        paths_current: Option<WakePathValidation<'_>>,
    ) -> bool {
        use RollbackCursor::{ClaimedBeforeSubmit, CloseSubmitted, RuntimeAbsent};
        let permit = acquire_carrier_mutation_permit().await;
        if !self.rollback_owner_is_current()
            || identity.pending_row(manager).await != row
            || !manager
                .with_current_carrier_snapshot(&permit, |current| {
                    self.rollback_paths_are_current(paths_current)
                        && identity.preparing_matches(&permit, current, self.owner.operation, row)
                })
                .await
        {
            return false;
        }
        #[cfg(test)]
        let _contended = Self::fixture_hold_before_close(pty);
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
        authority: (&CarrierMutationPermit, Option<WakePathValidation<'_>>),
    ) -> bool {
        let (permit, paths_current) = authority;
        let preparing = manager
            .with_current_carrier_snapshot(permit, |current| {
                self.rollback_paths_are_current(paths_current)
                    && identity.preparing_matches(permit, current, self.owner.operation, row)
                    && (row != RollbackRow::InitialNoRow
                        || self.record_initial_no_rollback_row(pty, identity, resume))
            })
            .await;
        if !preparing || row == RollbackRow::InitialNoRow {
            return preparing;
        }
        manager
            .rollback_exact_pending_with_receipt(permit, identity.binding, |pending, remove| {
                self.rollback_paths_are_current(paths_current)
                    && row == Self::exact_rollback_row(pending)
                    && self.record_removed_rollback_row(pty, identity, resume, remove)
            })
            .await
    }

    pub(super) fn pending_cleanup_runtime_matches(
        &self,
        binding: PendingCreateBinding,
        launch: Option<&OwnedLaunchReceipt>,
        pty: &PtyManager,
    ) -> bool {
        let id = binding.session_id();
        let Some(kind) = pty.backend_kind(id) else {
            let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
                return false;
            };
            // A partially failed spawn can leave a backend handle without a
            // route. Absence of routing alone never proves absence of runtime.
            return !pty.has_session(id)
                && !pty.backend_for_kind(descriptor.backend).has_session(id);
        };
        let Some(launch) = launch else {
            // A runtime without this executor's actual spawn receipt is not
            // cleanup authority, even when its UUID/backend happens to match.
            return false;
        };
        let Some((Some(cwd), replica, generation)) = pty.route_identities(id) else {
            return false;
        };
        launch.binding == binding
            && launch.backend == kind
            && launch.generation == generation
            && launch.cwd_identity.matches_route_observations(
                Some(&cwd),
                replica.as_ref(),
                launch.replica_identity.as_ref(),
            )
    }

    fn exact_rollback_row(row: &crate::session::session::Session) -> RollbackRow {
        RollbackRow::ExactRemoval {
            token: row.token,
            backend: row.backend_kind,
        }
    }

    pub(crate) fn start_effect(&self, slot: LifecycleEffectSlot) -> Option<LifecycleEffectGuard> {
        let mut state = self.owner.state.lock().ok()?;
        let progress = state.replacement.as_ref()?;
        let cleanup_allows = match progress.cleanup {
            ReplacementCleanupAuthority::Executor(nonce) => nonce == self.executor_nonce,
            ReplacementCleanupAuthority::Claimed(_) => {
                slot == LifecycleEffectSlot::WorkTransfer && progress.rollback_completed
            }
            ReplacementCleanupAuthority::Disarmed => !matches!(
                slot,
                LifecycleEffectSlot::RootDirectory
                    | LifecycleEffectSlot::ContextPreparation
                    | LifecycleEffectSlot::ConfigSeed
                    | LifecycleEffectSlot::BackendSpawn
                    | LifecycleEffectSlot::PendingRollback
            ),
            ReplacementCleanupAuthority::Unknown => false,
        };
        if state.executor_nonce != self.executor_nonce
            || !cleanup_allows
            || matches!(
                state.stage,
                ReplacementStage::Completed
                    | ReplacementStage::Refused
                    | ReplacementStage::RolledBack
                    | ReplacementStage::Ambiguous
            )
            || !matches!(state.effects[slot as usize], OwnedEffectState::NotStarted)
        {
            return None;
        }
        let attempt = Uuid::new_v4();
        state.effects[slot as usize] = OwnedEffectState::Running(attempt);
        Some(LifecycleEffectGuard {
            owner: self.owner.clone(),
            slot,
            attempt,
            completed: false,
        })
    }

    fn confirm_checkpoint(
        &self,
        carrier: crate::config::sessions_persistence::PublishedCarrier,
        rename: LifecycleEffectGuard,
        readback: LifecycleEffectGuard,
    ) -> bool {
        rename.complete(Ok(Some(carrier.session_id.to_string())));
        readback.complete(Ok(Some(carrier.nonce.to_string())));
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        let Some(progress) = state.replacement.as_mut() else {
            return false;
        };
        if progress.first_checkpoint.is_none() {
            progress.first_checkpoint = Some(carrier.clone());
        }
        progress.current_checkpoint = Some(carrier);
        progress.pending_carrier = None;
        true
    }

    #[cfg(test)]
    fn publication_fixture_action(
        &self,
    ) -> Option<crate::config::sessions_persistence::FixturePublicationActionKind> {
        match self.fixture.as_ref().and_then(|control| {
            control.take_action(self, |action| {
                matches!(action, ReplacementFixtureAction::PublicationLeaf(_))
            })
        }) {
            Some(ReplacementFixtureAction::PublicationLeaf(kind)) => Some(kind),
            _ => None,
        }
    }

    fn checkpoint_publication(
        &self,
    ) -> Option<(
        &super::ResolvedReplacement,
        PendingCreateBinding,
        LifecycleEffectGuard,
    )> {
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return None;
        };
        let binding = self.binding()?;
        self.reset_recheckpoint_effects();
        let rename = self.start_effect(LifecycleEffectSlot::CarrierRename)?;
        Some((descriptor, binding, rename))
    }
}
