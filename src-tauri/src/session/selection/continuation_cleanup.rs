use std::sync::{Arc, Mutex};
use std::time::Duration;

use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::{PendingCreateBinding, SessionManager};

use super::envelope::FinalizedOwnedCarryCapability;
use super::replacement::{
    LifecycleEffectGuard, LifecycleEffectSlot, OwnedEffectState, OwnedLaunchReceipt,
    OwnedReplacementContinuation, OwnedReplacementDescriptor, PendingCleanupEvidence,
    ReplacementCleanupAuthority, ReplacementStage, SourceExecutorObservation,
};
use super::SelectionCoordinator;

#[cfg(test)]
use super::replacement_fixture::{ReplacementFixtureAction, ReplacementFixtureCut};

impl OwnedReplacementContinuation {
    /// Some(None) is the explicit newer-B path, never a replacement baseline.
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
        let fresh_matches = self.owner.state.lock().ok().is_some_and(|state| {
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
        if !self.validate_source().await {
            return false;
        }
        use crate::config::sessions_persistence::{
            prepare_owned_replacement_publication, publish_owned_replacement,
            OwnedReplacementPublicationOutcome,
        };
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return false;
        };
        let Some(binding) = self.binding() else {
            return false;
        };
        self.reset_recheckpoint_effects();
        let Some(rename) = self.start_effect(LifecycleEffectSlot::CarrierRename) else {
            return false;
        };
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforePublication)
            .await
        {
            return false;
        }
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
            if let Some(ReplacementFixtureAction::PublicationLeaf(kind)) =
                self.fixture.as_ref().and_then(|control| {
                    control.take_action(self, |action| {
                        matches!(action, ReplacementFixtureAction::PublicationLeaf(_))
                    })
                })
            {
                prepared.attach_fixture_action(self.owner.operation, &descriptor.directory, kind);
            }
            if !self
                .fixture_reached(ReplacementFixtureCut::PublicationPrepared)
                .await
            {
                return false;
            }
        }
        if let Ok(mut state) = self.owner.state.lock() {
            if let Some(progress) = state.replacement.as_mut() {
                progress.pending_carrier = Some(prepared.destination_key());
            }
        } else {
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
                    return true;
                }
                OwnedReplacementPublicationOutcome::Pending(carrier) => {
                    rename.complete(Ok(Some(carrier.session_id.to_string())));
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
                        let permit =
                            crate::session::manager::acquire_carrier_mutation_permit().await;
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
                                    descriptor.previous.id,
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
                        state.retained_reason = Some(
                            "replacement native rename observed; strict readback unavailable"
                                .into(),
                        );
                    }
                    return false;
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

    /// The transport supplies only an actually observed submit/terminal or
    /// conditional source-handoff Result, never byte acquisition or expiry.
    pub(crate) fn precommit_rollback_completed(&self) -> bool {
        self.owner.state.lock().ok().is_some_and(|state| {
            state.executor_nonce == self.executor_nonce
                && matches!(state.completion, Some(Err(_)))
                && state.replacement.as_ref().is_some_and(|progress| {
                    progress.create_failed
                        && progress.rollback_completed
                        && progress.preparing_released
                        && progress.committed.is_none()
                })
        })
    }

    /// Called after the producer receives the real failed create result and
    /// observes its existing source handoff, with lexical target guards gone.
    pub(crate) async fn finish_rolled_back_work(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        effect: LifecycleEffectGuard,
        observed: bool,
    ) -> bool {
        if !observed || !self.precommit_rollback_completed() {
            effect.complete(Err("replacement rollback handoff not observed".into()));
            return false;
        }
        effect.complete(Ok(None));
        let released = self.reservation_release_is_observed(pty);
        if !released {
            return false;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let captured = self.owner.state.lock().ok().and_then(|state| {
            let progress = state.replacement.as_ref()?;
            if state.executor_nonce != self.executor_nonce
                || !matches!(
                    state.stage,
                    ReplacementStage::Admitted | ReplacementStage::PendingCreate
                )
                || !progress.rollback_completed
                || !progress.preparing_released
                || progress.create_reservation.is_some()
                || progress.work_reservation.is_some()
                || !matches!(
                    progress.context_prerequisite,
                    None | Some(OwnedEffectState::Completed { .. })
                )
                || !state.effects.iter().all(|effect| {
                    matches!(
                        effect,
                        OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }
                    )
                })
            {
                return None;
            }
            Some((progress.previous_carrier.clone(), progress.binding))
        });
        let Some((previous, binding)) = captured else {
            return false;
        };
        let current = manager
            .with_current_carrier_snapshot(&permit, |current| {
                !current
                    .rows
                    .iter()
                    .any(|row| row.id == binding.session_id())
                    && crate::config::sessions_persistence::current_protected_carrier(
                        &permit, &previous, current,
                    )
                    .is_some()
                    && pty.try_lock().ok().is_some_and(|pty| {
                        !pty.has_session(binding.session_id())
                            && pty.backend_kind(binding.session_id()).is_none()
                            && pty
                                .response_close_state()
                                .generation(binding.session_id())
                                .is_none()
                    })
            })
            .await;
        if !current {
            return false;
        }
        let verified = self.owner.state.lock().ok().is_some_and(|mut state| {
            let Some(progress) = state.replacement.as_mut() else {
                return false;
            };
            progress.work_transferred = true;
            progress.reservations_released = true;
            progress.terminal_verified = true;
            state.publication_observed = true;
            state.envelope_released = true;
            state.stage = ReplacementStage::RolledBack;
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
            .ok()
            .is_some_and(|state| state.publication_observed && state.envelope_released)
        {
            return false;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let captured = self.owner.state.lock().ok().and_then(|state| {
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
        });
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
        let verified = self.owner.state.lock().ok().is_some_and(|mut state| {
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
    pub(crate) async fn rollback_failed_create(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        error: &str,
    ) -> bool {
        self.claim_and_cleanup_pending(
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
        self.claim_and_cleanup_pending(
            manager,
            pty,
            PendingCleanupEvidence::ObservedSourceStoppedOrReturnedFailure,
        )
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

    // The caller holds U; this decision holds only the owner lock and never awaits.
    pub(super) fn claim_pending_cleanup_state(
        &self,
        pending_exists: bool,
        launch_generation: Option<u64>,
        cleanup_nonce: Uuid,
        evidence: PendingCleanupEvidence<'_>,
    ) -> Option<crate::config::sessions_persistence::PublishedCarrier> {
        self.owner.state.lock().ok().and_then(|mut state| {
            if state.executor_nonce != self.executor_nonce
                || !matches!(
                    state.stage,
                    ReplacementStage::Admitted | ReplacementStage::PendingCreate
                )
                || state.effects.iter().any(|effect| {
                    matches!(
                        effect,
                        OwnedEffectState::Running(_) | OwnedEffectState::Ambiguous(_)
                    )
                })
            {
                return None;
            }
            let stage = state.stage;
            let progress = state.replacement.as_mut()?;
            let stopped = matches!(
                progress.source_executor,
                SourceExecutorObservation::Stopped
                    | SourceExecutorObservation::Returned { failed: true }
            );
            if matches!(
                evidence,
                PendingCleanupEvidence::ObservedSourceStoppedOrReturnedFailure
            ) && (!stopped || stage != ReplacementStage::PendingCreate)
            {
                return None;
            }
            if progress.cleanup != ReplacementCleanupAuthority::Executor(self.executor_nonce)
                || progress.initial_b.is_some()
                || progress.committed.is_some()
                || progress.pending_carrier.is_some()
                || progress.launch.as_ref().map(|launch| launch.generation) != launch_generation
                || (stage == ReplacementStage::PendingCreate && !pending_exists)
            {
                return None;
            }
            progress.create_failed = true;
            progress.cleanup = ReplacementCleanupAuthority::Claimed(cleanup_nonce);
            let carrier = progress.previous_carrier.clone();
            if let PendingCleanupEvidence::ActualSetupFailure(error) = evidence {
                state.completion = Some(Err(error.to_owned()));
            }
            Some(carrier)
        })
    }

    pub(super) async fn claim_and_cleanup_pending(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
        evidence: PendingCleanupEvidence<'_>,
    ) -> bool {
        let Some(binding) = self.binding() else {
            return false;
        };
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let pending = manager.get_pending_session(binding).await;
        let launch = match self.owner.state.lock() {
            Ok(state) => state
                .replacement
                .as_ref()
                .and_then(|progress| progress.launch.clone()),
            Err(_) => return false,
        };
        if let Some(launch) = launch.as_ref() {
            if !pending
                .as_ref()
                .is_some_and(|row| row.token == launch.token && row.backend_kind == launch.backend)
            {
                return false;
            }
        }
        let runtime_matches = pty.try_lock().ok().is_some_and(|pty| {
            self.pending_cleanup_runtime_matches(binding, launch.as_ref(), &pty)
        });
        if !runtime_matches {
            return false;
        }
        let cleanup_nonce = Uuid::new_v4();
        let claim = self.claim_pending_cleanup_state(
            pending.is_some(),
            launch.as_ref().map(|launch| launch.generation),
            cleanup_nonce,
            evidence,
        );
        drop(permit);
        let Some(carrier) = claim else {
            return false;
        };
        let Some(effect) = self.start_effect(LifecycleEffectSlot::PendingRollback) else {
            return false;
        };
        let killed = match pty.try_lock() {
            Ok(pty) => match pty.backend_kind(binding.session_id()) {
                Some(kind)
                    if pending.as_ref().is_some_and(|row| row.backend_kind == kind)
                        && self.pending_cleanup_runtime_matches(binding, launch.as_ref(), &pty) =>
                {
                    pty.kill_for_kind(binding.session_id(), kind)
                        .map_err(|error| error.to_string())
                }
                None if self.pending_cleanup_runtime_matches(binding, launch.as_ref(), &pty) => {
                    Ok(())
                }
                _ => Err("replacement pending runtime identity changed".into()),
            },
            Err(_) => Err("replacement pending runtime unavailable".into()),
        };
        if let Err(error) = killed {
            effect.complete(Err(error));
            return false;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let claimed = self.owner.state.lock().ok().is_some_and(|state| {
            state.executor_nonce == self.executor_nonce
                && state.replacement.as_ref().is_some_and(|progress| {
                    progress.cleanup == ReplacementCleanupAuthority::Claimed(cleanup_nonce)
                        && progress.committed.is_none()
                })
        });
        if !claimed {
            return false;
        }
        let runtime_absent = pty
            .try_lock()
            .ok()
            .is_some_and(|pty| self.pending_cleanup_runtime_matches(binding, None, &pty));
        if !runtime_absent {
            effect.complete(Err(
                "replacement pending runtime changed after cleanup".into()
            ));
            return false;
        }
        if let Err(error) = manager
            .rollback_pending_create_with_carrier_permit(&permit, binding)
            .await
        {
            effect.complete(Err(error));
            return false;
        }
        let released = manager
            .with_current_carrier_snapshot(&permit, |current| {
                crate::config::sessions_persistence::abort_owned_carrier_replacement(
                    &permit,
                    &carrier,
                    self.owner.operation,
                    binding.session_id(),
                    current,
                )
            })
            .await;
        if !released {
            effect.complete(Err(
                "replacement Preparing rollback requires reconciliation".into(),
            ));
            return false;
        }
        let reservations = self.owner.state.lock().ok().and_then(|mut state| {
            let progress = state.replacement.as_mut()?;
            progress.preparing_released = true;
            Some((
                progress.create_reservation.take(),
                progress.work_reservation.take(),
            ))
        });
        drop(permit);
        drop(reservations);
        effect.complete(Ok(None));
        if let Ok(mut state) = self.owner.state.lock() {
            if let Some(progress) = state.replacement.as_mut() {
                progress.rollback_completed = true;
            }
        }
        true
    }

    pub(crate) fn start_effect(&self, slot: LifecycleEffectSlot) -> Option<LifecycleEffectGuard> {
        let mut state = self.owner.state.lock().ok()?;
        let progress = state.replacement.as_ref()?;
        let cleanup_allows = match progress.cleanup {
            ReplacementCleanupAuthority::Executor(nonce) => nonce == self.executor_nonce,
            ReplacementCleanupAuthority::Claimed(_) => {
                (slot == LifecycleEffectSlot::PendingRollback && !progress.rollback_completed)
                    || (slot == LifecycleEffectSlot::WorkTransfer && progress.rollback_completed)
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
}
