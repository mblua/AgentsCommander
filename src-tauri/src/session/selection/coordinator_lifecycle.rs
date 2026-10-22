use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot, Notify, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::{PendingCreateBinding, SessionManager};
use crate::session::session::SessionInfo;

use super::admission::{BlockingSeedWorkTracker, CoordinatorPhase};
use super::envelope::CoordinatorEnvelope;
use super::payload::{SelectionCoordinatorError, SessionSelection};
use super::replacement::{
    EntryCreateStage, EntryReleaseReceipt, EntryTerminal, EntryTerminalDisposition,
    OwnedEffectState, OwnedReplacementContinuation, OwnedReplacementDescriptor,
    PreparedOwnedReplacement, ReplacementAdmissionStep, ReplacementCleanupAuthority,
    ReplacementOwner, ReplacementProgress, ReplacementStage, ReplacementState, ResolvedReplacement,
    RetainedReplacements, SourceExecutorObservation,
};
use super::{
    CoordinatorInner, CoordinatorJob, SelectionCoordinator, SelectionRequest,
    COORDINATOR_ADMISSION_CAPACITY, COORDINATOR_QUEUE_CAPACITY, CREATE_TICKET_CAPACITY,
    IN_SELECTION_WORKER,
};

#[cfg(test)]
use super::replacement::FixtureRootReservation;

impl SelectionCoordinator {
    pub fn new(
        manager: Arc<tokio::sync::RwLock<SessionManager>>,
        shutdown: CancellationToken,
    ) -> Self {
        Self::with_retained_root(manager, shutdown, RetainedReplacements::new())
    }

    #[cfg(test)]
    pub(crate) fn with_fixture_root_reservation(
        manager: Arc<tokio::sync::RwLock<SessionManager>>,
        shutdown: CancellationToken,
        reservation: FixtureRootReservation,
    ) -> Self {
        let demand = match reservation {
            FixtureRootReservation::Normal65 => COORDINATOR_ADMISSION_CAPACITY,
            FixtureRootReservation::RejectCapacity => usize::MAX,
        };
        Self::with_retained_root(manager, shutdown, RetainedReplacements::reserve(demand))
    }

    pub(super) fn with_retained_root(
        manager: Arc<tokio::sync::RwLock<SessionManager>>,
        shutdown: CancellationToken,
        root: RetainedReplacements,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(COORDINATOR_QUEUE_CAPACITY);
        Self {
            inner: Arc::new(CoordinatorInner {
                retained_replacements: Mutex::new(root),
                sender,
                receiver: Mutex::new(Some(receiver)),
                admission: Arc::new(Semaphore::new(COORDINATOR_ADMISSION_CAPACITY)),
                create_tickets: Arc::new(Semaphore::new(CREATE_TICKET_CAPACITY)),
                manager,
                phase: AtomicU8::new(CoordinatorPhase::Bootstrapping as u8),
                blocking_seed_work: Arc::new(BlockingSeedWorkTracker::new()),
                critical_keys: Mutex::new(HashSet::new()),
                shutdown,
                worker: Mutex::new(None),
                retained_workers: Mutex::new(Vec::new()),
                retained_cleanup_tasks: Mutex::new(Vec::new()),
                retained_rollback_tasks: Mutex::new(Vec::new()),
                cleanup_pty: Mutex::new(None),
                cleanup_container_backend: Mutex::new(None),
            }),
        }
    }

    /// Exclusion only: false never grants an inline restore capability.
    pub(crate) fn restore_mint_exclusion(&self) -> Option<Arc<AtomicBool>> {
        Some(
            self.inner
                .retained_replacements
                .lock()
                .ok()?
                .restore_active
                .clone(),
        )
    }

    pub(crate) fn restore_is_active(&self) -> bool {
        self.restore_mint_exclusion()
            .is_none_or(|active| active.load(Ordering::Acquire))
    }

    pub(crate) async fn wait_for_restore_release(&self) -> Result<(), String> {
        while self.restore_is_active() {
            if self.inner.shutdown.is_cancelled() {
                return Err(SelectionCoordinatorError::Unavailable.to_string());
            }
            tokio::task::yield_now().await;
        }
        Ok(())
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing fixture consumes the unchanged coordinator shutdown token"
        )
    )]
    pub(crate) fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }

    pub(super) fn check_external_submission(&self) -> Result<(), SelectionCoordinatorError> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission);
        }
        match CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire)) {
            CoordinatorPhase::Running => Ok(()),
            CoordinatorPhase::Bootstrapping | CoordinatorPhase::Restoring => {
                Err(SelectionCoordinatorError::Busy)
            }
            CoordinatorPhase::Closing => Err(SelectionCoordinatorError::Unavailable),
        }
    }

    pub(super) fn try_reserve_envelope(
        &self,
    ) -> Result<
        (OwnedSemaphorePermit, mpsc::OwnedPermit<CoordinatorEnvelope>),
        SelectionCoordinatorError,
    > {
        self.check_external_submission()?;
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
        Ok((admission, slot))
    }

    pub async fn transition(
        &self,
        request: SelectionRequest,
    ) -> Result<Option<SessionSelection>, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Transition { request, response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) async fn destroy(
        &self,
        request: crate::commands::session::DestroyRequest,
    ) -> Result<crate::commands::session::DestroyOutcome, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Destroy { request, response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    /// E2 is available only to an actual still-retained external entry. Restore
    /// nonces and generic requests do not create a record in this root.
    pub(crate) fn retains_lifecycle_operation(&self, operation: Uuid) -> bool {
        let owner = {
            let Ok(root) = self.inner.retained_replacements.lock() else {
                return false;
            };
            if root.sealed || root.unavailable {
                return false;
            }
            root.entries.get(&operation).cloned()
        };
        let Some(owner) = owner else {
            return false;
        };
        let Ok(state) = owner.state.lock() else {
            return false;
        };
        matches!(
            state.stage,
            ReplacementStage::EntryWaiting | ReplacementStage::Queued | ReplacementStage::Executing
        ) && state.completion.is_none()
            && state.entry_terminal.is_none()
    }

    /// Prepare all owned data and the independent result cell before U.
    pub(crate) fn prepare_owned_replacement(
        &self,
        descriptor: ResolvedReplacement,
        binding: PendingCreateBinding,
    ) -> PreparedOwnedReplacement {
        let operation = Uuid::new_v4();
        let executor_nonce = Uuid::new_v4();
        let progress = ReplacementProgress {
            source_executor: SourceExecutorObservation::NotStarted,
            current_previous: descriptor.previous.clone(),
            launch_start_fresh: None,
            previous_carrier: descriptor.carrier.clone(),
            binding,
            preparing_nonce: operation,
            committed: None,
            warnings: Vec::new(),
            activity_started: false,
            activity_completed: false,
            initial_b: None,
            launch: None,
            current_checkpoint: None,
            first_checkpoint: None,
            pending_carrier: None,
            retirement_result: None,
            retirement_cleanup_completed: false,
            preparing_released: false,
            create_failed: false,
            rollback_completed: false,
            create_reservation: None,
            work_reservation: None,
            tracker: None,
            accepted_work: descriptor.work.clone(),
            cleanup: ReplacementCleanupAuthority::Executor(executor_nonce),
            preparing_started: false,
            retired: false,
            work_transferred: false,
            reservations_released: false,
            terminal_verified: false,
            source_stop: None,
            context_prerequisite: None,
        };
        PreparedOwnedReplacement {
            executor_nonce,
            owner: Arc::new(ReplacementOwner {
                changed: Notify::new(),
                operation,
                descriptor: OwnedReplacementDescriptor::Replacement(Box::new(descriptor)),
                state: Mutex::new(ReplacementState {
                    replacement: Some(progress),
                    effects: std::array::from_fn(|_| OwnedEffectState::NotStarted),
                    entry_terminal: None,
                    stage: ReplacementStage::Admitted,
                    executor_nonce,
                    completion: None,
                    publication_observed: false,
                    envelope_released: false,
                    sidecar_released: false,
                    claim_reached: false,
                    claim_released: false,
                    pre_teardown_failure: false,
                    reuse_live_result: None,
                    body_result: None,
                    entry_create: None,
                    retained_reason: None,
                }),
            }),
        }
    }

    /// Bootstrap from current retired Confirmed A and a real healthy reservation.
    pub(crate) fn admit_owned_replacement(
        &self,
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        tracker: &crate::pty::manager::ResponseCloseState,
        pty: &Arc<Mutex<PtyManager>>,
        prepared: PreparedOwnedReplacement,
    ) -> Result<ReplacementAdmissionStep, String> {
        use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseRouteIdentity};
        let OwnedReplacementDescriptor::Replacement(descriptor) = &prepared.owner.descriptor else {
            return Err(SelectionCoordinatorError::Unavailable.to_string());
        };
        let carrier = crate::config::sessions_persistence::retained_carrier_for_replacement(
            permit,
            &descriptor.directory,
            &descriptor.target,
            descriptor.previous.id,
            current,
        );
        let Some(carrier) = carrier.filter(|carrier| {
            carrier.nonce == descriptor.carrier.nonce && carrier.token == descriptor.previous.token
        }) else {
            return Ok(ReplacementAdmissionStep::Waiting(prepared));
        };
        if descriptor.backend != crate::pty::backend::SessionBackendKind::LocalProcess
            || !descriptor
                .replica_identity
                .as_ref()
                .is_some_and(|physical| carrier.retired_local_replica_matches(physical))
        {
            return Ok(ReplacementAdmissionStep::Waiting(prepared));
        }
        let Some(generation) = carrier.retired_local_generation() else {
            return Ok(ReplacementAdmissionStep::Waiting(prepared));
        };
        let Some(previous) = current
            .rows
            .iter()
            .find(|row| row.id == carrier.session_id)
            .cloned()
        else {
            return Ok(ReplacementAdmissionStep::Waiting(prepared));
        };
        let absent = pty.try_lock().ok().is_some_and(|pty| {
            pty.backend_kind(carrier.session_id).is_none()
                && tracker.generation(carrier.session_id).is_none()
        });
        if !absent {
            return Ok(ReplacementAdmissionStep::Waiting(prepared));
        }
        let epoch = match crate::config::sessions_persistence::classify_create_carrier_scope(
            permit,
            &descriptor.target,
            std::path::Path::new(&descriptor.cwd),
        ) {
            crate::config::sessions_persistence::CreateCarrierScope::Protected {
                epoch,
                preparing_owner: None,
                ..
            } => epoch,
            _ => return Ok(ReplacementAdmissionStep::Waiting(prepared)),
        };
        let classification = CreateClassification::Protected {
            identity: ResponseRouteIdentity {
                target: descriptor.target.clone(),
                session_id: carrier.session_id,
                token: carrier.token,
                generation,
                backend: crate::pty::backend::SessionBackendKind::LocalProcess,
                carrier_version: carrier.version,
            },
            publication_epoch: epoch,
        };
        let reservation = match tracker.reserve_create_owner(
            &descriptor.target,
            prepared.owner.operation,
            classification,
        ) {
            Ok(CreateAdmission::Reserved(reservation)) => reservation,
            _ => return Ok(ReplacementAdmissionStep::Waiting(prepared)),
        };
        let work = match tracker.begin_delivery(&descriptor.target) {
            Ok(work) => work,
            Err(_) => return Ok(ReplacementAdmissionStep::Waiting(prepared)),
        };
        let binding = prepared
            .owner
            .state
            .lock()
            .ok()
            .and_then(|state| state.replacement.as_ref().map(|progress| progress.binding))
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        if let Err(error) = self.insert_lifecycle_entry(&prepared.owner) {
            // No Preparing, row binding publication or native effect occurred.
            // Drop only this refused attempt's claims, outside owner/root locks.
            drop(reservation);
            drop(work);
            let released = tracker.publishers_bound();
            if let Ok(mut state) = prepared.owner.state.lock() {
                state.completion = Some(Err(error.clone()));
                state.publication_observed = true;
                state.envelope_released = true;
                if let Some(progress) = state.replacement.as_mut() {
                    progress.reservations_released = released;
                    progress.terminal_verified = released;
                }
                state.stage = if released {
                    ReplacementStage::Refused
                } else {
                    ReplacementStage::Ambiguous
                };
            }
            return Err(error);
        }
        let begun = crate::config::sessions_persistence::begin_carrier_replacement(
            permit,
            &carrier,
            prepared.owner.operation,
            binding.session_id(),
            current,
        );
        if let Ok(mut state) = prepared.owner.state.lock() {
            if let Some(progress) = state.replacement.as_mut() {
                progress.current_previous = previous;
                progress.previous_carrier = carrier;
                progress.create_reservation = Some(reservation);
                progress.work_reservation = Some(work);
                progress.tracker = Some(tracker.clone());
                progress.preparing_started = begun;
            }
            if !begun {
                state.stage = ReplacementStage::Ambiguous;
                state.retained_reason =
                    Some("replacement Preparing unavailable after acceptance".into());
            }
        }
        Ok(ReplacementAdmissionStep::Accepted(
            OwnedReplacementContinuation {
                owner: prepared.owner,
                coordinator: Arc::downgrade(&self.inner),
                executor_nonce: prepared.executor_nonce,
                source_validation: None,
                #[cfg(test)]
                fixture: None,
            },
        ))
    }

    pub(super) fn insert_lifecycle_entry(
        &self,
        owner: &Arc<ReplacementOwner>,
    ) -> Result<(), String> {
        let result = {
            let mut root = match self.inner.retained_replacements.lock() {
                Ok(root) => root,
                Err(error) => {
                    let mut root = error.into_inner();
                    root.sealed = true;
                    root.unavailable = true;
                    return Err(SelectionCoordinatorError::Unavailable.to_string());
                }
            };
            if root.sealed
                || root.unavailable
                || CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire))
                    != CoordinatorPhase::Running
            {
                Err(SelectionCoordinatorError::Unavailable)
            } else if root.entries.len()
                >= if matches!(
                    &owner.descriptor,
                    OwnedReplacementDescriptor::Replacement(_)
                ) {
                    COORDINATOR_ADMISSION_CAPACITY
                } else {
                    COORDINATOR_QUEUE_CAPACITY
                }
                || root.entries.contains_key(&owner.operation)
            {
                Err(SelectionCoordinatorError::Busy)
            } else {
                root.entries.insert(owner.operation, owner.clone());
                Ok(())
            }
        };
        result.map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub(crate) fn fixture_root_count(&self) -> Option<usize> {
        Some(self.inner.retained_replacements.lock().ok()?.entries.len())
    }

    #[cfg(test)]
    pub(crate) fn fixture_poison_root(&self) {
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _root = self
                .inner
                .retained_replacements
                .lock()
                .expect("real fixture root");
            panic!("selected actual retained-root poison stimulus");
        }));
        assert!(unwind.is_err());
        assert!(self.inner.retained_replacements.is_poisoned());
    }

    pub(super) fn prepare_lifecycle_owner(
        descriptor: OwnedReplacementDescriptor,
    ) -> (Arc<ReplacementOwner>, Uuid) {
        let operation = Uuid::new_v4();
        let executor_nonce = Uuid::new_v4();
        let owner = Arc::new(ReplacementOwner {
            changed: Notify::new(),
            operation,
            descriptor,
            state: Mutex::new(ReplacementState {
                replacement: None,
                effects: std::array::from_fn(|_| OwnedEffectState::NotStarted),
                entry_terminal: None,
                stage: ReplacementStage::EntryWaiting,
                executor_nonce,
                completion: None,
                publication_observed: false,
                envelope_released: false,
                sidecar_released: false,
                claim_reached: false,
                claim_released: false,
                pre_teardown_failure: false,
                reuse_live_result: None,
                body_result: None,
                entry_create: None,
                retained_reason: None,
            }),
        });
        (owner, executor_nonce)
    }

    #[cfg(test)]
    pub(crate) fn fixture_initial_reservation_refusal(
        &self,
        request: crate::commands::session::RestartJobRequest,
    ) -> Result<(), String> {
        {
            let root = self
                .inner
                .retained_replacements
                .lock()
                .expect("actual initial root");
            assert!(root.unavailable && root.entries.is_empty() && !root.sealed);
            assert_eq!(
                self.inner.phase.load(Ordering::Acquire),
                CoordinatorPhase::Bootstrapping as u8
            );
        }
        let (owner, _) =
            Self::prepare_lifecycle_owner(OwnedReplacementDescriptor::EntryRestart(request));
        self.insert_lifecycle_entry(&owner)
    }

    pub(super) async fn finish_lifecycle_entry(
        &self,
        owner: &Arc<ReplacementOwner>,
        executor_nonce: Uuid,
        (result, never_sent): (Result<SessionInfo, String>, bool),
        publication: oneshot::Sender<Result<SessionInfo, String>>,
    ) {
        let manager = self.inner.manager.read().await.clone();
        let binding = owner.state.lock().ok().and_then(|state| {
            state
                .entry_create
                .as_ref()
                .map(|create| (create.binding, create.stage))
        });
        let claim_reached = owner
            .state
            .lock()
            .ok()
            .is_some_and(|state| state.claim_reached);
        let permit = if claim_reached || binding.is_some() {
            Some(crate::session::manager::acquire_carrier_mutation_permit().await)
        } else {
            None
        };
        let binding_released = match (binding, permit.as_ref()) {
            (None, _) => true,
            (Some((binding, stage)), Some(permit)) => {
                manager
                    .with_current_carrier_snapshot(permit, |current| {
                        let id = binding.session_id();
                        match stage {
                            EntryCreateStage::Reserved | EntryCreateStage::RolledBack => {
                                !current.pending_ids.contains(&id)
                                    && !current.rows.iter().any(|row| row.id == id)
                            }
                            EntryCreateStage::Finalized => !current.pending_ids.contains(&id),
                            _ => false,
                        }
                    })
                    .await
            }
            _ => false,
        };
        if let Ok(mut state) = owner.state.lock() {
            state.claim_released = !state.claim_reached || state.sidecar_released;
            state.completion = Some(result.clone());
            state.envelope_released = true;
        }
        let terminal_result = result.clone();
        let _ = publication.send(result);
        let removable = match owner.state.lock() {
            Ok(mut state) if state.executor_nonce == executor_nonce => {
                state.publication_observed = true;
                let reuse_live_returned =
                    state.reuse_live_result.as_ref().is_some_and(|recorded| {
                        match (recorded, &terminal_result) {
                            (Ok(recorded), Ok(actual)) => serde_json::to_value(recorded)
                                .ok()
                                .zip(serde_json::to_value(actual).ok())
                                .is_some_and(|(recorded, actual)| recorded == actual),
                            (Err(recorded), Err(actual)) => recorded == actual,
                            _ => false,
                        }
                    });
                let body_terminal = state.entry_body_terminal(&terminal_result);
                let worker_released = state.stage == ReplacementStage::Returned
                    && state.sidecar_released
                    && (state.pre_teardown_failure || reuse_live_returned || body_terminal);
                if (never_sent && state.stage == ReplacementStage::EntryWaiting || worker_released)
                    && (terminal_result.is_err() || reuse_live_returned || body_terminal)
                    && state.claim_released
                    && binding_released
                    && state.prerequisite_attempt().is_ok()
                    && state.effects.iter().all(|effect| {
                        matches!(
                            effect,
                            OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }
                        )
                    })
                {
                    let reached_effects =
                        std::array::from_fn(|index| match &state.effects[index] {
                            OwnedEffectState::Completed { attempt, .. } => Some(*attempt),
                            _ => None,
                        });
                    state.entry_terminal = Some(EntryTerminal {
                        disposition: if terminal_result.is_ok() {
                            EntryTerminalDisposition::Success
                        } else if state
                            .entry_create
                            .as_ref()
                            .is_some_and(|create| create.stage == EntryCreateStage::RolledBack)
                        {
                            EntryTerminalDisposition::RolledBack
                        } else {
                            EntryTerminalDisposition::Failure
                        },
                        actual_result: terminal_result,
                        release_receipt: EntryReleaseReceipt {
                            operation: owner.operation,
                            executor_nonce,
                            envelope_never_sent: never_sent,
                            worker_returned_and_sidecar_released: worker_released,
                            reached_effects,
                            prerequisite_attempt: state.prerequisite_attempt().ok().flatten(),
                            binding,
                            reader_demand: state
                                .entry_create
                                .as_ref()
                                .and_then(|create| create.reader_demand),
                        },
                    });
                    state.stage = ReplacementStage::EntryTerminal;
                    true
                } else {
                    // The actual legacy result is observed, but individual reached
                    // effects must still be instrumented before terminal certification.
                    state.stage = ReplacementStage::Ambiguous;
                    state.retained_reason =
                        Some("lifecycle reached-effect receipts incomplete".into());
                    false
                }
            }
            _ => false,
        };
        drop(permit);
        if removable {
            self.remove_terminal_entry(owner);
        }
    }

    pub(super) fn seal_retained_entries(&self) {
        let mut root = match self.inner.retained_replacements.lock() {
            Ok(root) => root,
            Err(error) => {
                let mut root = error.into_inner();
                root.unavailable = true;
                root
            }
        };
        root.sealed = true;
        self.inner
            .phase
            .store(CoordinatorPhase::Closing as u8, Ordering::Release);
    }

    pub(super) fn inspect_retained_entries(&self, retained: &mut Vec<String>) {
        let mut cursor = None;
        loop {
            let (unavailable, next) = match self.inner.retained_replacements.try_lock() {
                Ok(root) => (
                    root.unavailable,
                    root.entries
                        .iter()
                        .filter(|(key, _)| cursor.is_none_or(|last| **key > last))
                        .min_by_key(|(key, _)| **key)
                        .map(|(key, owner)| (*key, owner.clone())),
                ),
                Err(error) => {
                    drop(error);
                    retained.push("reason=retained-root-unreadable state=retained".into());
                    return;
                }
            };
            if unavailable {
                retained.push("reason=retained-root-unavailable state=retained".into());
            }
            let Some((operation, owner)) = next else {
                return;
            };
            cursor = Some(operation);
            let safe = owner.state.try_lock().ok().is_some_and(|state| {
                state.replacement_terminal_is_verified()
                    || state.entry_terminal_is_verified(operation)
            });
            if !safe {
                let detail = owner
                    .state
                    .try_lock()
                    .ok()
                    .map(|state| state.retained_effect_diagnostic())
                    .unwrap_or_else(|| "detail=owner-state-unreadable".into());
                retained.push(format!(
                    "reason=retained-lifecycle-entry operation={} state=unsafe {}",
                    operation, detail
                ));
            }
        }
    }
}
