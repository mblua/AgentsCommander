//! Construction and bookkeeping of `SelectionCoordinator`: builds `CoordinatorInner`, reports
//! phase and restore state, reserves envelope capacity, and submits `transition`/`destroy` jobs.
//!
//! Also manages the retained-replacements root: `prepare_owned_replacement` and
//! `admit_owned_replacement` (checks retired carrier A, reserves create and delivery),
//! `prepare_lifecycle_owner` and `insert_lifecycle_entry` (capacity-bounded), then
//! `finish_lifecycle_entry`, and sealing and inspecting retained entries at shutdown.
//! Other submitters live in `coordinator_requests`; worker start in `coordinator_runtime`.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

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
    RetainedReplacements, SourceExecutorGuard, SourceExecutorObservation,
};
use super::{
    CoordinatorInner, CoordinatorJob, SelectionCoordinator, SelectionRequest,
    COORDINATOR_ADMISSION_CAPACITY, COORDINATOR_QUEUE_CAPACITY, CREATE_TICKET_CAPACITY,
    IN_SELECTION_WORKER,
};

#[cfg(test)]
use crate::pty::manager::{CreateOwnerReservation, WorkDeliveryReservation};
#[cfg(test)]
use std::sync::OnceLock;

#[cfg(test)]
use super::replacement::FixtureRootReservation;

pub(crate) enum WakeReplacementAdmissionError {
    SourceUnbound,
    Admission(String),
}

impl WakeReplacementAdmissionError {
    pub(crate) fn admission_error(self) -> Option<String> {
        match self {
            Self::SourceUnbound => None,
            Self::Admission(error) => Some(error),
        }
    }
}

#[cfg(test)]
tokio::task_local! {
    static WAKE_BIND_REFUSAL: Arc<OnceLock<Uuid>>;
}

#[cfg(test)]
fn fixture_wake_binding(
    continuation: OwnedReplacementContinuation,
    binding: PendingCreateBinding,
    // Borrow both handles so the probe is tied to this attempt's live reservations.
    _held: (&CreateOwnerReservation, &WorkDeliveryReservation),
) -> OwnedReplacementContinuation {
    WAKE_BIND_REFUSAL
        .try_with(|observed| {
            observed
                .set(binding.session_id())
                .expect("wake bind probe reached once");
            continuation.fixture_mismatched_wake_executor()
        })
        .unwrap_or(continuation)
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WakeSourceFixtureObservation {
    pub(crate) active: bool,
    pub(crate) stopped: bool,
    pub(crate) effects_started: usize,
}

#[cfg(test)]
impl OwnedReplacementContinuation {
    pub(crate) fn fixture_mismatched_wake_executor(&self) -> Self {
        Self {
            executor_nonce: Uuid::new_v4(),
            ..self.clone()
        }
    }

    pub(crate) fn fixture_wake_source_observation(&self) -> WakeSourceFixtureObservation {
        let state = self.owner.state.lock().unwrap();
        let source = state.replacement.as_ref().unwrap().source_executor;
        WakeSourceFixtureObservation {
            active: source == SourceExecutorObservation::Active,
            stopped: source == SourceExecutorObservation::Stopped,
            effects_started: state
                .effects
                .iter()
                .filter(|effect| !matches!(effect, OwnedEffectState::NotStarted))
                .count(),
        }
    }
}

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
        if IN_SELECTION_WORKER.try_with(|()| ()).is_ok() {
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
        request: crate::session::selection::DestroyRequest,
    ) -> Result<crate::session::selection::DestroyOutcome, String> {
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
    #[expect(
        clippy::unused_self,
        reason = "API de coordinator existente: conservar método y callers de preparación \
            antes de U; sin alterar operación/nonces."
    )]
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
            rollback_completed: false,
            rollback: None,
            pending_source_refusal: None,
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

    fn replacement_admission_candidate(
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        tracker: &crate::pty::manager::ResponseCloseState,
        pty: &Arc<Mutex<PtyManager>>,
        descriptor: &ResolvedReplacement,
    ) -> Option<(
        crate::config::sessions_persistence::PublishedCarrier,
        crate::session::session::Session,
        crate::pty::manager::CreateClassification,
    )> {
        use crate::pty::manager::{CreateClassification, ResponseRouteIdentity};
        let carrier = crate::config::sessions_persistence::retained_carrier_for_replacement(
            permit,
            &descriptor.directory,
            &descriptor.target,
            descriptor.previous.id,
            current,
        );
        let carrier = carrier.filter(|carrier| {
            carrier.nonce == descriptor.carrier.nonce && carrier.token == descriptor.previous.token
        })?;
        if descriptor.backend != crate::pty::backend::SessionBackendKind::LocalProcess
            || !descriptor
                .replica_identity
                .as_ref()
                .is_some_and(|physical| carrier.retired_local_replica_matches(physical))
        {
            return None;
        }
        let generation = carrier.retired_local_generation()?;
        let previous = current
            .rows
            .iter()
            .find(|row| row.id == carrier.session_id)
            .cloned()?;
        let absent = pty.try_lock().is_ok_and(|pty| {
            pty.backend_kind(carrier.session_id).is_none()
                && tracker.generation(carrier.session_id).is_none()
        });
        if !absent {
            return None;
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
            _ => return None,
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
        Some((carrier, previous, classification))
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
        self.admit_owned_replacement_with_source(permit, current, tracker, pty, prepared, None)
            .map_err(|error| {
                error
                    .admission_error()
                    .unwrap_or_else(|| "source_unbound".into())
            })
    }

    pub(crate) fn admit_owned_wake_replacement(
        &self,
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        tracker: &crate::pty::manager::ResponseCloseState,
        pty: &Arc<Mutex<PtyManager>>,
        prepared: PreparedOwnedReplacement,
        source: &mut SourceExecutorGuard,
    ) -> Result<ReplacementAdmissionStep, WakeReplacementAdmissionError> {
        self.admit_owned_replacement_with_source(
            permit,
            current,
            tracker,
            pty,
            prepared,
            Some(source),
        )
    }

    pub(crate) async fn recover_wake_precommit(
        &self,
        target: &str,
        previous: Uuid,
        cwd: &crate::session::manager::CarrierPhysicalIdentity,
        replica: &crate::session::manager::CarrierPhysicalIdentity,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) {
        let matches_path =
            |expected: &crate::session::manager::CarrierPhysicalIdentity,
             observed: &crate::session::manager::CarrierPhysicalIdentity| {
                expected.canonical_path() == observed.canonical_path()
                    && expected.same_object(observed)
            };
        let candidates = {
            let Ok(root) = self.inner.retained_replacements.lock() else {
                return;
            };
            if root.sealed || root.unavailable {
                return;
            }
            root.entries
                .values()
                .filter(|owner| match &owner.descriptor {
                    OwnedReplacementDescriptor::Replacement(d) => {
                        d.target == target
                            && d.previous.id == previous
                            && matches_path(&d.cwd_identity, cwd)
                            && d.replica_identity
                                .as_ref()
                                .is_some_and(|expected| matches_path(expected, replica))
                    }
                    _ => false,
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut candidate = None;
        for owner in candidates {
            let Ok(state) = owner.state.lock() else {
                return;
            };
            let terminal = state.replacement_terminal_is_verified();
            let executor_nonce = state.executor_nonce;
            drop(state);
            if terminal {
                self.remove_terminal_entry(&owner);
                continue;
            }
            if candidate.replace((owner, executor_nonce)).is_some() {
                return;
            }
        }
        if let Some((owner, executor_nonce)) = candidate {
            OwnedReplacementContinuation {
                owner,
                coordinator: Arc::downgrade(&self.inner),
                executor_nonce,
                source_validation: None,
                #[cfg(test)]
                fixture: None,
            }
            .settle_wake_precommit(manager, pty, None)
            .await;
        }
    }

    pub(super) fn owns_rollback_entry(&self, owner: &Arc<ReplacementOwner>) -> bool {
        self.inner.retained_replacements.lock().is_ok_and(|root| {
            !root.unavailable
                && root
                    .entries
                    .get(&owner.operation)
                    .is_some_and(|entry| Arc::ptr_eq(entry, owner))
        })
    }

    #[cfg(test)]
    pub(crate) async fn fixture_refuse_wake_binding<T>(
        observed: Arc<OnceLock<Uuid>>,
        wake: impl std::future::Future<Output = T>,
    ) -> T {
        WAKE_BIND_REFUSAL.scope(observed, wake).await
    }

    fn admit_owned_replacement_with_source(
        &self,
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        tracker: &crate::pty::manager::ResponseCloseState,
        pty: &Arc<Mutex<PtyManager>>,
        prepared: PreparedOwnedReplacement,
        mut source: Option<&mut SourceExecutorGuard>,
    ) -> Result<ReplacementAdmissionStep, WakeReplacementAdmissionError> {
        use crate::pty::manager::CreateAdmission;
        use WakeReplacementAdmissionError as Refusal;
        let OwnedReplacementDescriptor::Replacement(descriptor) = &prepared.owner.descriptor else {
            return Err(Refusal::Admission(
                SelectionCoordinatorError::Unavailable.to_string(),
            ));
        };
        let Some((carrier, previous, classification)) =
            Self::replacement_admission_candidate(permit, current, tracker, pty, descriptor)
        else {
            return Ok(ReplacementAdmissionStep::Waiting(prepared));
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
            .ok_or_else(|| {
                Refusal::Admission(SelectionCoordinatorError::Unavailable.to_string())
            })?;
        let continuation = OwnedReplacementContinuation {
            owner: prepared.owner,
            coordinator: Arc::downgrade(&self.inner),
            executor_nonce: prepared.executor_nonce,
            source_validation: None,
            #[cfg(test)]
            fixture: None,
        };
        #[cfg(test)]
        let continuation = if source.is_some() {
            fixture_wake_binding(continuation, binding, (&reservation, &work))
        } else {
            continuation
        };
        if source
            .as_mut()
            .is_some_and(|guard| !guard.bind(&continuation))
        {
            drop((reservation, work));
            return Err(Refusal::SourceUnbound);
        }
        if let Err(error) = self.insert_lifecycle_entry(&continuation.owner) {
            // No Preparing, row binding publication or native effect occurred.
            // Drop only this refused attempt's claims, outside owner/root locks.
            drop(reservation);
            drop(work);
            let released = tracker.publishers_bound();
            if let Ok(mut state) = continuation.owner.state.lock() {
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
            if let Some(source) = source {
                drop(std::mem::replace(source, SourceExecutorGuard::unbound()));
            }
            return Err(Refusal::Admission(error));
        }
        let begun = crate::config::sessions_persistence::begin_carrier_replacement(
            permit,
            &carrier,
            continuation.owner.operation,
            binding.session_id(),
            current,
        );
        if let Ok(mut state) = continuation.owner.state.lock() {
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
        Ok(ReplacementAdmissionStep::Accepted(continuation))
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
        request: crate::session::selection::RestartJobRequest,
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

    fn lifecycle_results_match(
        recorded: &Result<SessionInfo, String>,
        actual: &Result<SessionInfo, String>,
    ) -> bool {
        match (recorded, actual) {
            (Ok(recorded), Ok(actual)) => serde_json::to_value(recorded)
                .ok()
                .zip(serde_json::to_value(actual).ok())
                .is_some_and(|(recorded, actual)| recorded == actual),
            (Err(recorded), Err(actual)) => recorded == actual,
            _ => false,
        }
    }

    fn lifecycle_binding_released(
        current: &crate::session::manager::CarrierSnapshot,
        binding: PendingCreateBinding,
        stage: EntryCreateStage,
    ) -> bool {
        let id = binding.session_id();
        match stage {
            EntryCreateStage::Reserved | EntryCreateStage::RolledBack => {
                !current.pending_ids.contains(&id) && !current.rows.iter().any(|row| row.id == id)
            }
            EntryCreateStage::Finalized => !current.pending_ids.contains(&id),
            _ => false,
        }
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
        let claim_reached = owner.state.lock().is_ok_and(|state| state.claim_reached);
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
                        Self::lifecycle_binding_released(current, binding, stage)
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
                        Self::lifecycle_results_match(recorded, &terminal_result)
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
                        disposition: Self::entry_terminal_disposition(&state, &terminal_result),
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

    fn entry_terminal_disposition(
        state: &ReplacementState,
        terminal_result: &Result<SessionInfo, String>,
    ) -> EntryTerminalDisposition {
        if terminal_result.is_ok() {
            EntryTerminalDisposition::Success
        } else if state
            .entry_create
            .as_ref()
            .is_some_and(|create| create.stage == EntryCreateStage::RolledBack)
        {
            EntryTerminalDisposition::RolledBack
        } else {
            EntryTerminalDisposition::Failure
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
            let safe = owner.state.try_lock().is_ok_and(|state| {
                state.replacement_terminal_is_verified()
                    || state.entry_terminal_is_verified(operation)
            });
            if !safe {
                let detail = owner.state.try_lock().ok().map_or_else(
                    || "detail=owner-state-unreadable".into(),
                    |state| state.retained_effect_diagnostic(),
                );
                retained.push(format!(
                    "reason=retained-lifecycle-entry operation={operation} state=unsafe {detail}"
                ));
            }
        }
    }
}
