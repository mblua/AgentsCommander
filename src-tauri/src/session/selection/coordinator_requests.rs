use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[cfg(test)]
use super::admission::BlockingSeedWorkState;
use super::admission::{
    CoordinatorPhase, CriticalAdmissionGuard, CriticalAdmissionKey, CriticalAdmissionKind,
    CriticalAdmissionOutcome, CriticalAdmissionReservation, WatchdogKillOutcome,
};
use super::envelope::{CoordinatorEnvelope, RestoreSubmissionPhaseGuard};
use super::payload::{
    SelectionCause, SelectionCoordinatorError, SelectionMode, SessionSelection,
    TrustedCreateIntent, TrustedResourceIntent,
};
use super::replacement::ReplacementOwner;
use super::{
    begin_container_shutdown, cleanup_pending_creates_after_join,
    seal_and_drain_container_shutdown_work_after_join, AutoCloseBatchTicket,
    ContainerLifecycleSender, CoordinatorJob, CreateFinalizationTicket, RestoreBarrierGuard,
    SelectionCoordinator, SelectionShutdownReport, IN_SELECTION_WORKER,
    SHUTDOWN_CLEANUP_BUDGET_SECS, SHUTDOWN_FINALIZATION_RESERVE_MAX,
};

#[cfg(test)]
use super::replacement_fixture::{ReplacementFixtureControl, ReplacementFixtureCut};

impl SelectionCoordinator {
    pub(super) fn remove_terminal_entry(&self, owner: &Arc<ReplacementOwner>) {
        let removed = match self.inner.retained_replacements.lock() {
            Ok(mut root)
                if !root.sealed
                    && root
                        .entries
                        .get(&owner.operation)
                        .is_some_and(|current| Arc::ptr_eq(current, owner)) =>
            {
                root.entries.remove(&owner.operation)
            }
            _ => None,
        };
        drop(removed);
    }

    pub(crate) fn reserve_auto_close(
        &self,
    ) -> Result<AutoCloseBatchTicket, SelectionCoordinatorError> {
        let (admission, slot) = self.try_reserve_envelope()?;
        Ok(AutoCloseBatchTicket {
            slot: Some(slot),
            admission: Some(admission),
        })
    }

    #[cfg(test)]
    pub(crate) async fn submit_opaque_replacement_fixture(
        &self,
        input: super::OpaqueFixtureInput,
        control: Arc<ReplacementFixtureControl>,
    ) -> Option<super::OpaqueFixtureOutput> {
        let owned = input.continuation.clone();
        let (admission, slot) = match self.try_reserve_envelope() {
            Ok(reserved) => reserved,
            Err(_) => {
                control.record(&owned, ReplacementFixtureCut::SubmitFailed);
                return None;
            }
        };
        let (response, receiver) = oneshot::channel();
        control.submitted.store(true, Ordering::Release);
        control.record(&owned, ReplacementFixtureCut::Submitted);
        drop(slot.send(CoordinatorEnvelope {
            _admission: admission,
            _critical_admission: None,
            _create_ticket: None,
            job: CoordinatorJob::InlineReplacementFixture {
                input,
                control: control.clone(),
                response,
            },
        }));
        if control.drop_receiver.load(Ordering::Acquire) {
            drop(receiver);
            return None;
        }
        match control.receive(&owned, receiver).await {
            Some(result) => {
                control.received.store(true, Ordering::Release);
                let outcome = result.outcome;
                control.record_outcome(
                    &owned,
                    ReplacementFixtureCut::SourceReceived,
                    Some(outcome),
                );
                Some(result)
            }
            None => {
                control.record(&owned, ReplacementFixtureCut::ReceiverLost);
                None
            }
        }
    }

    pub(crate) async fn resource_kill(
        &self,
        session_id: Uuid,
        intent: TrustedResourceIntent,
    ) -> Result<crate::resource_monitor::ResourceKillResult, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::ResourceKill {
                session_id,
                intent,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) async fn detach(
        &self,
        session_id: Uuid,
        geometry: Option<crate::config::settings::WindowGeometry>,
        suppress_selection: bool,
    ) -> Result<String, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Detach {
                session_id,
                geometry,
                suppress_selection,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) async fn attach(&self, session_id: Uuid) -> Result<(), String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Attach {
                session_id,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub async fn snapshot(&self) -> Result<SessionSelection, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Snapshot { response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub async fn reserve_create(
        &self,
        intent: TrustedCreateIntent,
    ) -> Result<CreateFinalizationTicket, SelectionCoordinatorError> {
        self.reserve_create_internal(intent, true).await
    }

    pub(crate) async fn reserve_suppressed_create(
        &self,
    ) -> Result<CreateFinalizationTicket, SelectionCoordinatorError> {
        self.reserve_create_internal(TrustedCreateIntent::Background, false)
            .await
    }

    pub(super) async fn reserve_create_internal(
        &self,
        intent: TrustedCreateIntent,
        allow_auto_select: bool,
    ) -> Result<CreateFinalizationTicket, SelectionCoordinatorError> {
        self.check_external_submission()?;
        let create_ticket = Arc::clone(&self.inner.create_tickets)
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let admission = Arc::clone(&self.inner.admission)
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let slot = self
            .inner
            .sender
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Closed(_) => SelectionCoordinatorError::Unavailable,
                mpsc::error::TrySendError::Full(_) => SelectionCoordinatorError::Busy,
            })?;

        let manager = self.inner.manager.read().await.clone();
        let selection = manager.selection_payload().await;
        let auto_select_precondition = (allow_auto_select
            && selection.mode() == SelectionMode::None)
            .then(|| (selection.epoch().to_string(), selection.revision()));
        Ok(CreateFinalizationTicket {
            owned_replacement: None,
            slot: Some(slot),
            admission: Some(admission),
            create_ticket: Some(create_ticket),
            binding: None,
            cause: SelectionCause::SessionCreated(intent),
            auto_select_precondition,
            completed: false,
        })
    }

    pub async fn submit_restore_first(&self) -> Result<RestoreBarrierGuard, String> {
        fn busy(reason: &str) -> String {
            log::debug!(
                target: "agentscommander_lib::selection_bootstrap",
                "restore bootstrap busy: cause={reason}"
            );
            #[cfg(test)]
            eprintln!("restore bootstrap busy: cause={reason}");
            SelectionCoordinatorError::Busy.to_string()
        }
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
        }
        self.inner
            .phase
            .compare_exchange(
                CoordinatorPhase::Bootstrapping as u8,
                CoordinatorPhase::Restoring as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| busy("phase"))?;
        let _phase_guard = RestoreSubmissionPhaseGuard {
            phase: &self.inner.phase,
        };
        let carrier_permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        #[cfg(not(test))]
        let scopes_empty =
            crate::config::sessions_persistence::restore_carrier_scopes_are_empty(&carrier_permit);
        #[cfg(test)]
        let scopes_empty = self.restore_mint_exclusion().and_then(|scope| {
            crate::config::sessions_persistence::restore_test_carrier_scopes_are_empty(
                &carrier_permit,
                &scope,
            )
        });
        match scopes_empty {
            Some(true) => {}
            Some(false) => return Err(busy("carrier_protections")),
            None => return Err(SelectionCoordinatorError::Unavailable.to_string()),
        }
        let admission = match Arc::clone(&self.inner.admission).try_acquire_owned() {
            Ok(admission) => admission,
            Err(_) => {
                return Err(busy("admission"));
            }
        };
        let slot = match self.inner.sender.clone().try_reserve_owned() {
            Ok(slot) => slot,
            Err(_) => {
                return Err(busy("channel"));
            }
        };
        let nonce = Uuid::new_v4();
        {
            let root = self
                .inner
                .retained_replacements
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?;
            if root.restore_nonce.is_some() || !root.restore_active.load(Ordering::Acquire) {
                return Err(SelectionCoordinatorError::Unavailable.to_string());
            }
        }
        let (started, started_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RestoreBarrier {
                nonce,
                started,
                release: release_rx,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        // Restore now owns FIFO position one. Normal and critical work may be
        // admitted, but the worker cannot execute it until restore releases the
        // barrier. In particular, unsolicited route loss must wait here instead
        // of being rejected after the external route has already disappeared.
        let _ = self.inner.phase.compare_exchange(
            CoordinatorPhase::Restoring as u8,
            CoordinatorPhase::Running as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        drop(carrier_permit);
        started_rx
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?;
        Ok(RestoreBarrierGuard {
            coordinator: Arc::downgrade(&self.inner),
            nonce,
            release: Some(release),
        })
    }

    pub(super) async fn reserve_critical_admission(
        &self,
        key: CriticalAdmissionKey,
    ) -> Result<Option<CriticalAdmissionReservation>, String> {
        let guard = {
            let mut keys = self
                .inner
                .critical_keys
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?;
            if !keys.insert(key) {
                return Ok(None);
            }
            CriticalAdmissionGuard::new(&self.inner, key)
        };

        let admission = tokio::select! {
            permit = Arc::clone(&self.inner.admission).acquire_owned() => {
                permit.map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
            }
            _ = self.inner.shutdown.cancelled() => {
                return Err(SelectionCoordinatorError::Unavailable.to_string());
            }
        };
        let slot = tokio::select! {
            permit = self.inner.sender.clone().reserve_owned() => {
                permit.map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
            }
            _ = self.inner.shutdown.cancelled() => {
                return Err(SelectionCoordinatorError::Unavailable.to_string());
            }
        };
        Ok(Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }))
    }

    pub(super) async fn submit_route_loss(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Result<CriticalAdmissionOutcome<()>, String> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
        }
        match CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire)) {
            CoordinatorPhase::Bootstrapping | CoordinatorPhase::Restoring => {
                return Err(SelectionCoordinatorError::Busy.to_string())
            }
            CoordinatorPhase::Closing => {
                return Err(SelectionCoordinatorError::Unavailable.to_string())
            }
            CoordinatorPhase::Running => {}
        }
        let manager = self.inner.manager.read().await.clone();
        if !manager.contains_public_or_pending(session_id).await {
            return Ok(CriticalAdmissionOutcome::Completed(()));
        }
        let key = CriticalAdmissionKey {
            session_id,
            kind: CriticalAdmissionKind::RouteLoss,
        };
        let Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }) = self.reserve_critical_admission(key).await?
        else {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        };
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RouteLoss {
                session_id,
                exit_code,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(guard),
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())??;
        Ok(CriticalAdmissionOutcome::Completed(()))
    }

    pub(crate) async fn watchdog_resource_kill(
        &self,
        session_id: Uuid,
    ) -> Result<WatchdogKillOutcome, String> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
        }
        match CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire)) {
            CoordinatorPhase::Bootstrapping | CoordinatorPhase::Restoring => {
                return Err(SelectionCoordinatorError::Busy.to_string())
            }
            CoordinatorPhase::Closing => {
                return Err(SelectionCoordinatorError::Unavailable.to_string())
            }
            CoordinatorPhase::Running => {}
        }
        let manager = self.inner.manager.read().await.clone();
        if !manager.contains_public_or_pending(session_id).await {
            return Ok(WatchdogKillOutcome::NoPublicSession);
        }
        let key = CriticalAdmissionKey {
            session_id,
            kind: CriticalAdmissionKind::WatchdogKill,
        };
        let Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }) = self.reserve_critical_admission(key).await?
        else {
            return Ok(WatchdogKillOutcome::AlreadyInFlight);
        };
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::ResourceKill {
                session_id,
                intent: TrustedResourceIntent::Watchdog,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(guard),
        }));
        let result = receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())??;
        Ok(WatchdogKillOutcome::Completed(result))
    }

    pub(crate) async fn background_destroy(
        &self,
        session_id: Uuid,
    ) -> Result<CriticalAdmissionOutcome<super::DestroyOutcome>, String> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
        }
        match CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire)) {
            CoordinatorPhase::Bootstrapping | CoordinatorPhase::Restoring => {
                return Err(SelectionCoordinatorError::Busy.to_string())
            }
            CoordinatorPhase::Closing => {
                return Err(SelectionCoordinatorError::Unavailable.to_string())
            }
            CoordinatorPhase::Running => {}
        }
        let manager = self.inner.manager.read().await.clone();
        if !manager.contains_public_or_pending(session_id).await {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        }
        let key = CriticalAdmissionKey {
            session_id,
            kind: CriticalAdmissionKind::BackgroundCleanup,
        };
        let Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }) = self.reserve_critical_admission(key).await?
        else {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        };
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Destroy {
                request: super::DestroyRequest {
                    ids: vec![session_id],
                    source: super::DestructionSource::BackgroundCleanup,
                    force_destroy_root: false,
                },
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(guard),
        }));
        let result = receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())??;
        Ok(CriticalAdmissionOutcome::Completed(result))
    }

    #[cfg(test)]
    pub(crate) fn critical_key_registered_for_test(
        &self,
        session_id: Uuid,
        kind: CriticalAdmissionKind,
    ) -> bool {
        self.inner
            .critical_keys
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&CriticalAdmissionKey { session_id, kind })
    }

    #[cfg(test)]
    pub(super) async fn critical_probe_for_test(
        &self,
        session_id: Uuid,
        kind: CriticalAdmissionKind,
    ) -> Result<CriticalAdmissionOutcome<()>, String> {
        let manager = self.inner.manager.read().await.clone();
        if !manager.contains_public_or_pending(session_id).await {
            return Ok(CriticalAdmissionOutcome::Completed(()));
        }
        let key = CriticalAdmissionKey { session_id, kind };
        let Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }) = self.reserve_critical_admission(key).await?
        else {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        };
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Snapshot { response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(guard),
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())??;
        Ok(CriticalAdmissionOutcome::Completed(()))
    }

    pub fn container_lifecycle_sender(&self) -> ContainerLifecycleSender {
        ContainerLifecycleSender::new(Arc::new(super::tickets::WeakContainerLifecycle {
            inner: Arc::downgrade(&self.inner),
        }))
    }

    pub async fn close_and_join(&self) -> SelectionShutdownReport {
        self.close_and_join_with_budget(Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS))
            .await
    }

    #[cfg(test)]
    pub(crate) fn fixture_native_work_counts(&self) -> Option<(usize, usize)> {
        let state = self.inner.blocking_seed_work.state.lock().ok()?;
        Some((
            state.work.len(),
            state
                .work
                .values()
                .filter(|work| matches!(work, BlockingSeedWorkState::Started))
                .count(),
        ))
    }

    pub(super) async fn close_and_join_with_budget(
        &self,
        budget: Duration,
    ) -> SelectionShutdownReport {
        let started_at = Instant::now();
        let deadline = started_at.checked_add(budget).unwrap_or(started_at);
        let finalization_reserve = (budget / 4).min(SHUTDOWN_FINALIZATION_RESERVE_MAX);
        let worker_deadline = deadline
            .checked_sub(finalization_reserve)
            .unwrap_or(started_at);
        self.seal_retained_entries();
        let queued_aborts = self.inner.blocking_seed_work.seal();
        for abort in queued_aborts {
            abort.abort();
        }
        self.inner.admission.close();
        self.inner.create_tickets.close();
        self.inner.shutdown.cancel();
        let mut retained = Vec::new();
        if !begin_container_shutdown(&self.inner, deadline) {
            retained.push("reason=container-shutdown-signal state=retained".to_string());
        }

        let handle = match self.inner.worker.try_lock() {
            Ok(mut worker) => worker.take(),
            Err(error) => {
                log::error!(
                    "[selection] worker handle lock unavailable during bounded shutdown: {error}"
                );
                retained.push("reason=coordinator-worker-handle state=retained".to_string());
                None
            }
        };
        if let Some(mut handle) = handle {
            let remaining = worker_deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    log::error!("[selection] coordinator worker join failed: {error}");
                }
                Err(_) => {
                    let critical_count = self
                        .inner
                        .critical_keys
                        .lock()
                        .map(|keys| keys.len())
                        .unwrap_or_default();
                    log::error!(
                        "[selection] coordinator worker phase exhausted before shared deadline \
                            budget={:?} finalizationReserve={:?} outstandingCriticalKeys={}",
                        budget,
                        finalization_reserve,
                        critical_count
                    );
                    handle.abort();
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match tokio::time::timeout(remaining, &mut handle).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            log::debug!("[selection] aborted worker join result: {error}");
                        }
                        Err(_) => {
                            log::error!(
                                "[selection] aborted coordinator worker retained after absolute \
                                    deadline state=retained"
                            );
                            retained
                                .push("reason=coordinator-worker-abort state=retained".to_string());
                            self.inner
                                .retained_workers
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .push(handle);
                        }
                    }
                }
            }
        }

        if !self
            .inner
            .blocking_seed_work
            .wait_quiescent_until(worker_deadline)
            .await
        {
            retained.push("reason=blocking-seed-transaction-await state=retained".to_string());
        }

        retained.extend(cleanup_pending_creates_after_join(&self.inner, deadline).await);
        retained
            .extend(seal_and_drain_container_shutdown_work_after_join(&self.inner, deadline).await);
        match self.inner.critical_keys.try_lock() {
            Ok(mut keys) => keys.clear(),
            Err(error) => {
                log::error!("[selection] critical key lock unavailable after worker join: {error}");
                retained.push("reason=critical-key-clear state=retained".to_string());
            }
        }
        self.inspect_retained_entries(&mut retained);
        retained.sort();
        retained.dedup();
        SelectionShutdownReport {
            persistence_safe: retained.is_empty(),
            retained,
        }
    }
}
