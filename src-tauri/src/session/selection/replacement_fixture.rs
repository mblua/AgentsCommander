use std::sync::atomic::Ordering;

use std::sync::{Arc, Mutex, Weak};

use tokio::sync::oneshot;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::{PendingCreateBinding, SessionManager};

use super::envelope::EnvelopeHold;
use super::replacement::{LifecycleEffectSlot, OwnedEffectState, ReplacementOwner};
use super::replacement::{
    OwnedReplacementContinuation, OwnedSourceStop, ReplacementCleanupAuthority,
    SourceExecutorObservation,
};
use super::{SelectionCoordinator, COORDINATOR_ADMISSION_CAPACITY};

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplacementFixtureCut {
    TicketSubmitted,
    TicketDrained,
    BeforeTerminalRemoval,
    SourceRetainedReturn,
    Inserted,
    BeforeSourceValidation,
    BeforeBackendSpawn,
    ContextNativeEnter,
    ContextNativeReturned,
    SeedNativeEnter,
    SeedNativeReturned,
    SourceCoreEntered,
    BeforeFinalizer,
    BeforeCommit,
    AfterDisarm,
    BeforeCarry,
    BeforePublication,
    PublicationPrepared,
    BeforeRetirement,
    AdapterOutcomeBeforeLeaseDrop,
    AfterReleaseBeforeSend,
    Submitted,
    SubmitFailed,
    Baseline,
    AdapterReturned,
    AdmissionReleased,
    SourceReceived,
    GateLost,
    Drained,
    ReceiverLost,
    WorkerEntered,
    WorkerUnwound,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum ReplacementFixtureAction {
    Pause {
        cut: ReplacementFixtureCut,
        resume: oneshot::Receiver<()>,
    },
    PanicOnce {
        cut: ReplacementFixtureCut,
    },
    CommitBeforeDisarmPanicOnce,
    FixtureStopExactB,
    SimulatedPrecommitFailure,
    BindPreparedTwice,
    PublicationLeaf(crate::config::sessions_persistence::FixturePublicationActionKind),
}

#[cfg(test)]
pub(super) struct ReplacementFixtureEnvelopeTrace<'a> {
    pub(super) hold: &'a mut EnvelopeHold,
    pub(super) control: &'a ReplacementFixtureControl,
    pub(super) owned: &'a OwnedReplacementContinuation,
    pub(super) released: bool,
    pub(super) outcome: Option<ReplacementFixtureOutcome>,
}

#[cfg(test)]
impl ReplacementFixtureEnvelopeTrace<'_> {
    pub(super) fn release(&mut self) {
        self.hold.release();
        self.released = true;
        self.control.record_outcome(
            self.owned,
            ReplacementFixtureCut::AdmissionReleased,
            self.outcome,
        );
    }
}

#[cfg(test)]
impl Drop for ReplacementFixtureEnvelopeTrace<'_> {
    fn drop(&mut self) {
        if !self.released {
            self.control
                .record(self.owned, ReplacementFixtureCut::WorkerUnwound);
            self.release();
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplacementFixtureOutcome {
    Finalized,
    Retained,
    Error,
}

#[cfg(test)]
pub(crate) struct ReplacementFixtureOwnerWeak(Weak<ReplacementOwner>);

#[cfg(test)]
impl ReplacementFixtureOwnerWeak {
    pub(crate) fn is_alive(&self) -> bool {
        self.0.upgrade().is_some()
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ReplacementFixtureOwnerObservation {
    pub(crate) source_stop: Option<OwnedSourceStop>,
    pub(crate) source_unwound: bool,
    pub(crate) source_stopped: bool,
    pub(crate) source_active: bool,
    pub(crate) source_returned_failed: Option<bool>,
    pub(crate) cleanup_disarmed: bool,
    pub(crate) cleanup_claimed: bool,
    pub(crate) rollback_completed: bool,
    pub(crate) initial_version: Option<u64>,
    pub(crate) terminal: bool,
    pub(crate) transferred: bool,
    pub(crate) carry_attempts: usize,
    pub(crate) carry_recaptured: bool,
    pub(crate) effects_started: usize,
    pub(crate) rename_failed: bool,
    pub(crate) checkpoint_confirmed: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplacementFixtureReceipt {
    pub(crate) operation: Uuid,
    pub(crate) binding: Option<PendingCreateBinding>,
    pub(crate) cut: ReplacementFixtureCut,
    pub(crate) initial_version: Option<u64>,
    pub(crate) outcome: Option<ReplacementFixtureOutcome>,
}

/// Per-operation observations only. Neither these gates nor their receipts
/// participate in product authority or alter common finalizer accounting.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct ReplacementFixtureControl {
    pub(crate) native_await_panic: Mutex<Option<NativeAwaitPanicOnce>>,
    pub(crate) context_notification: Mutex<FixtureContextNotification>,
    pub(crate) restoration_failed: std::sync::atomic::AtomicBool,
    pub(crate) restoration_shared: std::sync::atomic::AtomicBool,
    pub(crate) preaccept_identity: Mutex<Option<String>>,
    pub(crate) preaccept_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) preaccept_entered: std::sync::atomic::AtomicBool,
    pub(crate) accepted_core_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) accepted_core_entered: std::sync::atomic::AtomicBool,
    pub(crate) native_results: Mutex<Vec<(ReplacementFixtureCut, String)>>,
    pub(crate) source_panic: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) live_receiver_drop: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) actions: Mutex<Vec<ReplacementFixtureAction>>,
    pub(crate) mock_backend: std::sync::atomic::AtomicBool,
    pub(super) identity: Mutex<Option<(Uuid, Uuid, PendingCreateBinding)>>,
    pub(crate) receipts: Mutex<Vec<ReplacementFixtureReceipt>>,
    pub(crate) baseline_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) before_release_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) submitted: std::sync::atomic::AtomicBool,
    pub(crate) received: std::sync::atomic::AtomicBool,
    pub(crate) drop_receiver: std::sync::atomic::AtomicBool,
    pub(crate) ticket_only: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum NativeAwaitPanicOnce {
    Context(oneshot::Receiver<()>),
    Seed(oneshot::Receiver<()>),
}

#[cfg(test)]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum FixtureContextNotification {
    #[default]
    Unarmed,
    Armed,
    Consumed {
        operation: Uuid,
        executor: Uuid,
        binding: PendingCreateBinding,
        error: String,
        title: String,
        message: String,
    },
}

#[cfg(test)]
pub(crate) struct ReplacementFixtureNativeInvocation {
    pub(super) control: Arc<ReplacementFixtureControl>,
    pub(super) identity: (Uuid, Uuid, PendingCreateBinding),
    pub(super) enter: ReplacementFixtureCut,
    pub(super) returned: ReplacementFixtureCut,
}

#[cfg(test)]
pub(crate) async fn await_fixture_native<F: std::future::Future>(
    native: F,
    panic: Option<oneshot::Receiver<()>>,
) -> F::Output {
    tokio::pin!(native);
    let signal = async {
        if let Some(receiver) = panic {
            if receiver.await.is_ok() {
                return;
            }
        }
        std::future::pending::<()>().await;
    };
    tokio::select! {
        biased;
        actual = &mut native => actual,
        () = signal => panic!("selected actual native-await source unwind"),
    }
}

#[cfg(test)]
impl ReplacementFixtureNativeInvocation {
    fn reached(&self, cut: ReplacementFixtureCut) {
        assert_eq!(
            *self.control.identity.lock().expect("native identity"),
            Some(self.identity)
        );
        self.control
            .receipts
            .lock()
            .expect("native receipt")
            .push(ReplacementFixtureReceipt {
                operation: self.identity.0,
                binding: Some(self.identity.2),
                cut,
                initial_version: None,
                outcome: None,
            });
        let action = {
            let mut actions = self.control.actions.lock().expect("native fixture actions");
            actions
                .iter()
                .position(|action| {
                    matches!(action,
                ReplacementFixtureAction::Pause { cut: selected, .. } |
                ReplacementFixtureAction::PanicOnce { cut: selected } if *selected == cut)
                })
                .map(|position| actions.remove(position))
        };
        match action {
            Some(ReplacementFixtureAction::Pause { resume, .. }) => {
                resume
                    .blocking_recv()
                    .expect("native fixture observer lost");
            }
            Some(ReplacementFixtureAction::PanicOnce { .. }) => {
                panic!("selected native fixture unwind at {cut:?}")
            }
            _ => {}
        }
    }

    pub(crate) fn enter(&self) {
        self.reached(self.enter);
    }

    pub(crate) fn returned(&self, actual: String) {
        self.control
            .native_results
            .lock()
            .expect("native fixture result")
            .push((self.returned, actual));
        self.reached(self.returned);
    }
}

#[cfg(test)]
impl ReplacementFixtureControl {
    pub(crate) async fn after_pty_core_accept(&self, source_identity: &str) -> bool {
        assert_eq!(
            self.preaccept_identity
                .lock()
                .expect("source identity")
                .as_deref(),
            Some(source_identity)
        );
        self.accepted_core_entered.store(true, Ordering::Release);
        Self::wait_gate(&self.accepted_core_gate).await
    }

    pub(crate) async fn before_source_accept(&self, source_identity: &str) -> bool {
        {
            let mut identity = self
                .preaccept_identity
                .lock()
                .expect("preaccept source identity");
            if let Some(expected) = identity.as_ref() {
                assert_eq!(expected, source_identity);
            } else {
                *identity = Some(source_identity.to_owned());
            }
        }
        self.preaccept_entered.store(true, Ordering::Release);
        Self::wait_gate(&self.preaccept_gate).await
    }
    fn owner(&self, coordinator: &SelectionCoordinator) -> Option<Arc<ReplacementOwner>> {
        let operation = self.identity.lock().ok()?.as_ref()?.0;
        coordinator
            .inner
            .retained_replacements
            .lock()
            .ok()?
            .entries
            .get(&operation)
            .cloned()
    }

    pub(crate) fn owner_weak(
        &self,
        coordinator: &SelectionCoordinator,
    ) -> Option<ReplacementFixtureOwnerWeak> {
        Some(ReplacementFixtureOwnerWeak(Arc::downgrade(
            &self.owner(coordinator)?,
        )))
    }

    pub(crate) fn observe_owner(
        &self,
        coordinator: &SelectionCoordinator,
    ) -> Option<ReplacementFixtureOwnerObservation> {
        let owner = self.owner(coordinator)?;
        let state = owner.state.lock().ok()?;
        let progress = state.replacement.as_ref()?;
        Some(ReplacementFixtureOwnerObservation {
            source_stop: progress.source_stop,
            source_unwound: progress.source_executor == SourceExecutorObservation::Stopped,
            source_stopped: matches!(
                progress.source_executor,
                SourceExecutorObservation::Stopped
                    | SourceExecutorObservation::Returned { failed: true }
            ),
            source_active: progress.source_executor == SourceExecutorObservation::Active,
            source_returned_failed: match progress.source_executor {
                SourceExecutorObservation::Returned { failed } => Some(failed),
                _ => None,
            },
            cleanup_disarmed: progress.cleanup == ReplacementCleanupAuthority::Disarmed,
            cleanup_claimed: matches!(progress.cleanup, ReplacementCleanupAuthority::Claimed(_)),
            rollback_completed: progress.rollback_completed,
            initial_version: progress
                .initial_b
                .as_ref()
                .map(|initial| initial.carrier_version),
            terminal: state.replacement_terminal_is_verified(),
            transferred: progress.work_transferred,
            carry_attempts: usize::from(!matches!(
                state.effects[LifecycleEffectSlot::Carry as usize],
                OwnedEffectState::NotStarted
            )),
            carry_recaptured: matches!(&state.effects[LifecycleEffectSlot::Carry as usize],
                OwnedEffectState::Completed { outcome: Err(error), .. } if error == "carry \
                    recapture: newer B preserved"),
            effects_started: state
                .effects
                .iter()
                .filter(|effect| !matches!(effect, OwnedEffectState::NotStarted))
                .count(),
            rename_failed: matches!(
                &state.effects[LifecycleEffectSlot::CarrierRename as usize],
                OwnedEffectState::Completed {
                    outcome: Err(_),
                    ..
                }
            ),
            checkpoint_confirmed: progress.first_checkpoint.is_some(),
        })
    }

    pub(crate) fn attempt_duplicate(
        &self,
        coordinator: &SelectionCoordinator,
    ) -> Result<(), String> {
        let owner = self
            .owner(coordinator)
            .ok_or_else(|| "fixture owner absent".to_string())?;
        coordinator.insert_lifecycle_entry(&owner)
    }

    pub(crate) fn poison_observed_owner(&self, coordinator: &SelectionCoordinator) {
        let owner = self.owner(coordinator).expect("actual observed owner");
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = owner.state.lock().expect("initially readable owner state");
            panic!("selected actual owner observation poison");
        }));
        assert!(panic.is_err());
        assert!(owner.state.is_poisoned());
    }
    #[cfg(test)]
    pub(crate) fn cleanup_observed_stopped<'a>(
        &'a self,
        coordinator: &'a SelectionCoordinator,
        manager: &'a SessionManager,
        pty: &'a Arc<Mutex<PtyManager>>,
    ) -> impl std::future::Future<Output = bool> + 'a {
        let owner = self
            .identity
            .lock()
            .ok()
            .and_then(|identity| *identity)
            .and_then(|(operation, nonce, _)| {
                let root = coordinator.inner.retained_replacements.lock().ok()?;
                let owner = root.entries.get(&operation)?.clone();
                Some((owner, nonce))
            });
        async move {
            let Some((owner, executor_nonce)) = owner else {
                return false;
            };
            OwnedReplacementContinuation {
                owner,
                coordinator: Arc::downgrade(&coordinator.inner),
                executor_nonce,
                source_validation: None,
                fixture: None,
            }
            .cleanup_stopped_source(manager, pty)
            .await
        }
    }
    pub(super) async fn receive<T>(
        &self,
        owned: &OwnedReplacementContinuation,
        receiver: oneshot::Receiver<T>,
    ) -> Option<T> {
        self.validate_identity(owned);
        let panic = self
            .source_panic
            .lock()
            .expect("source panic control")
            .take();
        let drop_receiver = self
            .live_receiver_drop
            .lock()
            .expect("receiver control")
            .take();
        async fn signal(receiver: Option<oneshot::Receiver<()>>) {
            if let Some(receiver) = receiver {
                if receiver.await.is_ok() {
                    return;
                }
            }
            std::future::pending::<()>().await;
        }
        tokio::select! {
            result = receiver => result.ok(),
            () = signal(panic) => panic!("selected actual source response-wait unwind"),
            () = signal(drop_receiver) => None,
        }
    }
    pub(super) fn validate_identity(&self, owned: &OwnedReplacementContinuation) {
        if owned.owner.state.is_poisoned() {
            // An unknown-proof fixture must not turn a real poisoned owner
            // into a second panic from a trace Drop. Check the immutable
            // operation/executor against the already-bound observation only;
            // record_outcome honestly leaves binding/version absent. This
            // observation grants no finalization or cleanup authority.
            let expected = self
                .identity
                .lock()
                .expect("fixture identity")
                .expect("identity was bound before owner poison");
            assert_eq!(expected.0, owned.owner.operation);
            assert_eq!(expected.1, owned.executor_nonce);
            return;
        }
        let identity = (
            owned.owner.operation,
            owned.executor_nonce,
            owned.binding().expect("fixture actual B binding"),
        );
        let mut expected = self.identity.lock().expect("fixture identity");
        match *expected {
            Some(expected) => assert_eq!(identity, expected, "fixture operation/B mismatch"),
            None => *expected = Some(identity),
        }
    }

    pub(super) fn take_action(
        &self,
        owned: &OwnedReplacementContinuation,
        select: impl Fn(&ReplacementFixtureAction) -> bool,
    ) -> Option<ReplacementFixtureAction> {
        self.validate_identity(owned);
        let mut actions = self.actions.lock().expect("fixture actions");
        let position = actions.iter().position(select)?;
        Some(actions.remove(position))
    }

    pub(super) async fn reached(
        &self,
        owned: &OwnedReplacementContinuation,
        cut: ReplacementFixtureCut,
    ) -> bool {
        let action = self.take_action(owned, |action| {
            matches!(action,
            ReplacementFixtureAction::Pause { cut: selected, .. } |
            ReplacementFixtureAction::PanicOnce { cut: selected } if *selected == cut)
        });
        self.record(owned, cut);
        match action {
            Some(ReplacementFixtureAction::Pause { resume, .. }) => resume.await.is_ok(),
            Some(ReplacementFixtureAction::PanicOnce { .. }) => {
                panic!("selected fixture unwind at {cut:?}")
            }
            _ => true,
        }
    }
    pub(crate) fn observe_retained(
        &self,
        coordinator: &SelectionCoordinator,
    ) -> Option<(bool, bool, bool, bool, usize)> {
        let operation = self.receipts.lock().ok()?.last()?.operation;
        let owner = coordinator
            .inner
            .retained_replacements
            .lock()
            .ok()?
            .entries
            .get(&operation)?
            .clone();
        let state = owner.state.lock().ok()?;
        Some((
            state.publication_observed,
            state.envelope_released,
            !matches!(
                state.effects[LifecycleEffectSlot::WorkTransfer as usize],
                OwnedEffectState::NotStarted
            ),
            state.replacement.as_ref()?.work_transferred,
            COORDINATOR_ADMISSION_CAPACITY - coordinator.inner.admission.available_permits(),
        ))
    }

    pub(super) fn record(&self, owned: &OwnedReplacementContinuation, cut: ReplacementFixtureCut) {
        self.record_outcome(owned, cut, None);
    }

    pub(super) fn record_outcome(
        &self,
        owned: &OwnedReplacementContinuation,
        cut: ReplacementFixtureCut,
        outcome: Option<ReplacementFixtureOutcome>,
    ) {
        self.validate_identity(owned);
        let receipt = ReplacementFixtureReceipt {
            operation: owned.owner.operation,
            binding: owned.binding(),
            cut,
            outcome,
            initial_version: owned.owner.state.lock().ok().and_then(|state| {
                state
                    .replacement
                    .as_ref()?
                    .initial_b
                    .as_ref()
                    .map(|initial| initial.carrier_version)
            }),
        };
        self.receipts
            .lock()
            .expect("fixture receipt lock")
            .push(receipt);
    }

    pub(super) async fn wait_gate(gate: &Mutex<Option<oneshot::Receiver<()>>>) -> bool {
        let receiver = gate.lock().expect("fixture gate lock").take();
        match receiver {
            Some(receiver) => receiver.await.is_ok(),
            None => true,
        }
    }
}
