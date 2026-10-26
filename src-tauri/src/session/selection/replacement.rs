use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::AtomicBool;

use std::sync::{Arc, Mutex, Weak};

use tokio::sync::Notify;
use uuid::Uuid;

use crate::session::manager::{CommitResult, PendingCreateBinding};
use crate::session::session::SessionInfo;
use crate::session::warnings::SessionWarning;

#[cfg(test)]
use super::replacement_fixture::ReplacementFixtureControl;
use super::{CoordinatorInner, COORDINATOR_ADMISSION_CAPACITY};

#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "C20 retains complete owned recovery work; preserve both variants and separate deep \
        copies"
)]
pub(crate) enum OwnedReplacementWork {
    OrdinaryOwned {
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        message_id: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        body: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        payload: serde_json::Value,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        recipient: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        request: serde_json::Value,
    },
    PtyDurableReference {
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        version: u32,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        injection_id: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        op_id: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        sender: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        target: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        source_plane: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        payload_sha256: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        payload_bytes: u64,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        issued_at: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        expires_at: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        queued_at: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        lease_owner: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        attempt: i64,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        authority_session_id: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        authority_client_id: Option<String>,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        authority_client_generation: Option<String>,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        sender_identity_fingerprint: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        target_identity_fingerprint: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no \
                replay authority"
        )]
        requested_agent_id: Option<String>,
    },
}

#[derive(Clone)]
pub(crate) struct ResolvedReplacement {
    pub(crate) target: String,
    pub(crate) directory: std::path::PathBuf,
    pub(crate) previous: crate::session::session::Session,
    pub(crate) carrier: crate::config::sessions_persistence::PublishedCarrier,
    pub(crate) requested_agent_id: Option<String>,
    #[expect(
        dead_code,
        reason = "C20 requires complete inert retained source data; this field grants no replay \
            authority"
    )]
    pub(crate) requested_profile: Option<String>,
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) launch_recipe: serde_json::Value,
    pub(crate) cwd: String,
    pub(crate) backend: crate::pty::backend::SessionBackendKind,
    pub(crate) work: OwnedReplacementWork,
    pub(crate) cwd_identity: crate::session::manager::CarrierPhysicalIdentity,
    pub(crate) replica_identity: Option<crate::session::manager::CarrierPhysicalIdentity>,
}

impl fmt::Debug for ResolvedReplacement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedReplacement")
            .field("target", &self.target)
            .field("previous", &self.previous.id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub(super) struct OwnedLaunchReceipt {
    pub(super) operation: Uuid,
    pub(super) preparing_nonce: Uuid,
    pub(super) binding: PendingCreateBinding,
    pub(super) token: Uuid,
    pub(super) generation: u64,
    pub(super) backend: crate::pty::backend::SessionBackendKind,
    pub(super) info: SessionInfo,
    pub(super) cwd_identity: crate::session::manager::CarrierPhysicalIdentity,
    pub(super) replica_identity: Option<crate::session::manager::CarrierPhysicalIdentity>,
}

pub(super) struct InitialFinalizationReceipt {
    pub(super) operation: Uuid,
    pub(super) preparing_nonce: Uuid,
    pub(super) binding: PendingCreateBinding,
    pub(super) token: Uuid,
    pub(super) carrier_version: u64,
    pub(super) launch_generation: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplacementCleanupAuthority {
    Executor(Uuid),
    Claimed(Uuid),
    Disarmed,
    Unknown,
}

pub(super) struct ReplacementProgress {
    pub(super) source_executor: SourceExecutorObservation,
    pub(super) current_previous: crate::session::session::Session,
    pub(super) launch_start_fresh: Option<bool>,
    pub(super) previous_carrier: crate::config::sessions_persistence::PublishedCarrier,
    pub(super) binding: PendingCreateBinding,
    pub(super) preparing_nonce: Uuid,
    pub(super) committed: Option<CommitResult>,
    pub(super) warnings: Vec<SessionWarning>,
    pub(super) activity_started: bool,
    pub(super) activity_completed: bool,
    pub(super) initial_b: Option<InitialFinalizationReceipt>,
    pub(super) launch: Option<OwnedLaunchReceipt>,
    pub(super) current_checkpoint: Option<crate::config::sessions_persistence::PublishedCarrier>,
    pub(super) first_checkpoint: Option<crate::config::sessions_persistence::PublishedCarrier>,
    pub(super) pending_carrier: Option<crate::config::sessions_persistence::PublishedCarrier>,
    pub(super) retirement_result: Option<CommitResult>,
    pub(super) retirement_cleanup_completed: bool,
    pub(super) preparing_released: bool,
    pub(super) create_failed: bool,
    pub(super) rollback_completed: bool,
    pub(super) create_reservation: Option<crate::pty::manager::CreateOwnerReservation>,
    pub(super) work_reservation: Option<crate::pty::manager::WorkDeliveryReservation>,
    pub(super) tracker: Option<crate::pty::manager::ResponseCloseState>,
    #[expect(
        dead_code,
        reason = "C20 requires complete inert retained source data; this field grants no replay \
            authority"
    )]
    pub(super) accepted_work: OwnedReplacementWork,
    pub(super) cleanup: ReplacementCleanupAuthority,
    pub(super) preparing_started: bool,
    pub(super) retired: bool,
    pub(super) work_transferred: bool,
    pub(super) reservations_released: bool,
    pub(super) terminal_verified: bool,
    pub(super) source_stop: Option<OwnedSourceStop>,
    pub(super) context_prerequisite: Option<OwnedEffectState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceExecutorObservation {
    NotStarted,
    Active,
    Stopped,
    Returned { failed: bool },
}

/// Lexical observation of the accepted producer, never of its outer waiter.
/// This handle is transient; the retained owner stores only the observation.
pub(crate) struct SourceExecutorGuard {
    pub(super) bound: Option<(Arc<ReplacementOwner>, Uuid)>,
    pub(super) returned: bool,
}

impl SourceExecutorGuard {
    pub(crate) fn unbound() -> Self {
        Self {
            bound: None,
            returned: false,
        }
    }

    pub(crate) fn bind(&mut self, continuation: &OwnedReplacementContinuation) -> bool {
        if let Some((owner, nonce)) = &self.bound {
            return Arc::ptr_eq(owner, &continuation.owner)
                && *nonce == continuation.executor_nonce;
        }
        let Ok(mut state) = continuation.owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != continuation.executor_nonce {
            return false;
        }
        let Some(progress) = state.replacement.as_mut() else {
            return false;
        };
        if progress.source_executor != SourceExecutorObservation::NotStarted {
            return false;
        }
        progress.source_executor = SourceExecutorObservation::Active;
        self.bound = Some((continuation.owner.clone(), continuation.executor_nonce));
        true
    }

    pub(crate) fn returned(&mut self, failed: bool) {
        let Some((owner, nonce)) = &self.bound else {
            return;
        };
        let Ok(mut state) = owner.state.lock() else {
            return;
        };
        if state.executor_nonce != *nonce {
            return;
        }
        let Some(progress) = state.replacement.as_mut() else {
            return;
        };
        if progress.source_executor == SourceExecutorObservation::Active {
            progress.source_executor = SourceExecutorObservation::Returned { failed };
            self.returned = true;
        }
    }
}

impl Drop for SourceExecutorGuard {
    fn drop(&mut self) {
        if self.returned {
            return;
        }
        let Some((owner, nonce)) = &self.bound else {
            return;
        };
        if let Ok(mut state) = owner.state.lock() {
            if state.executor_nonce == *nonce {
                if let Some(progress) = state.replacement.as_mut() {
                    if progress.source_executor == SourceExecutorObservation::Active {
                        progress.source_executor = SourceExecutorObservation::Stopped;
                    }
                }
            }
        }
    }
}

pub(super) enum PendingCleanupEvidence<'a> {
    ActualSetupFailure(&'a str),
    ObservedSourceStoppedOrReturnedFailure,
}

/// A transient handle to the same data-only owner retained by CoordinatorInner.
/// No app, backend, guard, task or future returns from that root.
#[derive(Clone)]
pub(crate) struct OwnedReplacementContinuation {
    pub(super) owner: Arc<ReplacementOwner>,
    pub(super) coordinator: Weak<CoordinatorInner>,
    pub(super) executor_nonce: Uuid,
    // Transient executor/request only. ReplacementOwner never stores this
    // callback, its app captures, or the future it creates.
    pub(super) source_validation: Option<OwnedSourceValidation>,
    #[cfg(test)]
    pub(super) fixture: Option<Arc<ReplacementFixtureControl>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnedSourceStop {
    Terminal,
    AuthorityChanged,
    StoreTransient,
    Expired,
    LeaseLost,
}

pub(crate) type OwnedSourceValidation = Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), OwnedSourceStop>> + Send>,
        > + Send
        + Sync,
>;

pub(crate) struct PreparedOwnedReplacement {
    pub(super) owner: Arc<ReplacementOwner>,
    pub(super) executor_nonce: Uuid,
}

pub(crate) enum ReplacementAdmissionStep {
    Waiting(PreparedOwnedReplacement),
    Accepted(OwnedReplacementContinuation),
}

impl PreparedOwnedReplacement {
    pub(crate) fn matches_launch(
        &self,
        shell: &str,
        args: &[String],
        cwd: &str,
        agent: &Option<String>,
        recipe: &serde_json::Value,
    ) -> bool {
        matches!(&self.owner.descriptor, OwnedReplacementDescriptor::Replacement(descriptor)
            if descriptor.shell == shell && descriptor.shell_args == args
                && descriptor.cwd == cwd && descriptor.requested_agent_id == *agent
                && descriptor.launch_recipe == *recipe
                && descriptor.backend == crate::pty::backend::SessionBackendKind::LocalProcess)
    }
}

impl fmt::Debug for OwnedReplacementContinuation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedReplacementContinuation")
            .field("operation", &self.owner.operation)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(super) enum OwnedReplacementDescriptor {
    Replacement(Box<ResolvedReplacement>),
    EntryRestart(super::RestartJobRequest),
    EntryRoot(super::RootJobRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplacementStage {
    Admitted,
    PendingCreate,
    FinalizedCarry,
    FinalizedPreRename,
    FinalizedPendingReadback,
    FinalizedRetire,
    FinalizedRecheckpoint,
    FinalizedPublish,
    Completed,
    Refused,
    RolledBack,
    EntryWaiting,
    Queued,
    Executing,
    Returned,
    Ambiguous,
    EntryTerminal,
}

#[derive(Clone, Copy)]
pub(super) enum EntryTerminalDisposition {
    Success,
    Failure,
    RolledBack,
}

pub(super) struct EntryReleaseReceipt {
    pub(super) operation: Uuid,
    pub(super) executor_nonce: Uuid,
    pub(super) envelope_never_sent: bool,
    pub(super) worker_returned_and_sidecar_released: bool,
    pub(super) reached_effects: [Option<Uuid>; 11],
    pub(super) prerequisite_attempt: Option<Uuid>,
    pub(super) binding: Option<(PendingCreateBinding, EntryCreateStage)>,
    pub(super) reader_demand: Option<EntryReaderDemandReceipt>,
}

pub(super) struct EntryTerminal {
    pub(super) disposition: EntryTerminalDisposition,
    pub(super) actual_result: Result<SessionInfo, String>,
    pub(super) release_receipt: EntryReleaseReceipt,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleEffectSlot {
    RootDirectory,
    ContextPreparation,
    ConfigSeed,
    BackendSpawn,
    Carry,
    CarrierRename,
    Readback,
    Retirement,
    PendingRollback,
    Publication,
    WorkTransfer,
}

#[derive(Clone)]
pub(super) enum OwnedEffectState {
    NotStarted,
    Running(Uuid),
    Completed {
        attempt: Uuid,
        outcome: Result<Option<String>, String>,
    },
    Ambiguous(Uuid),
}

pub(crate) struct LifecycleEffectGuard {
    pub(super) owner: Arc<ReplacementOwner>,
    pub(super) slot: LifecycleEffectSlot,
    pub(super) attempt: Uuid,
    pub(super) completed: bool,
}

impl LifecycleEffectGuard {
    /// The caller observed that this particular effect was not reached.
    pub(super) fn skip(mut self) {
        if let Ok(mut state) = self.owner.state.lock() {
            let effect = &mut state.effects[self.slot as usize];
            if matches!(effect, OwnedEffectState::Running(attempt) if *attempt == self.attempt) {
                *effect = OwnedEffectState::NotStarted;
                self.completed = true;
            }
        }
    }
    pub(crate) fn complete(mut self, outcome: Result<Option<String>, String>) {
        if let Ok(mut state) = self.owner.state.lock() {
            let effect = &mut state.effects[self.slot as usize];
            if matches!(effect, OwnedEffectState::Running(attempt) if *attempt == self.attempt) {
                *effect = OwnedEffectState::Completed {
                    attempt: self.attempt,
                    outcome,
                };
                self.completed = true;
            }
        }
    }
}

impl Drop for LifecycleEffectGuard {
    fn drop(&mut self) {
        if !self.completed {
            if let Ok(mut state) = self.owner.state.lock() {
                if matches!(state.effects[self.slot as usize],
                    OwnedEffectState::Running(attempt) if attempt == self.attempt)
                {
                    state.effects[self.slot as usize] = OwnedEffectState::Ambiguous(self.attempt);
                    if let Some(progress) = state.replacement.as_mut() {
                        progress.cleanup = ReplacementCleanupAuthority::Unknown;
                    }
                    state.stage = ReplacementStage::Ambiguous;
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryCreateStage {
    Reserved,
    Pending,
    Finalized,
    CleanupClaimed(Uuid),
    RolledBack,
    Unknown,
}

pub(super) struct EntryCreateBinding {
    pub(super) binding: PendingCreateBinding,
    pub(super) stage: EntryCreateStage,
    pub(super) preparation_receipt: Option<OwnedEffectState>,
    pub(super) reader_demand: Option<EntryReaderDemandReceipt>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryReaderDemandReceipt {
    LegacyTaskAccepted,
    Returned,
}

pub(super) struct ReplacementState {
    pub(super) replacement: Option<ReplacementProgress>,
    pub(super) effects: [OwnedEffectState; 11],
    pub(super) entry_terminal: Option<EntryTerminal>,
    pub(super) stage: ReplacementStage,
    pub(super) executor_nonce: Uuid,
    pub(super) completion: Option<Result<SessionInfo, String>>,
    pub(super) publication_observed: bool,
    pub(super) envelope_released: bool,
    pub(super) sidecar_released: bool,
    pub(super) claim_reached: bool,
    pub(super) claim_released: bool,
    pub(super) pre_teardown_failure: bool,
    pub(super) reuse_live_result: Option<Result<SessionInfo, String>>,
    pub(super) body_result: Option<Result<SessionInfo, String>>,
    pub(super) entry_create: Option<EntryCreateBinding>,
    pub(super) retained_reason: Option<String>,
}

impl ReplacementState {
    pub(super) fn retained_effect_diagnostic(&self) -> String {
        let pending = self
            .effects
            .iter()
            .enumerate()
            .find_map(|(slot, effect)| match effect {
                OwnedEffectState::Running(attempt) => Some((slot, "running", attempt)),
                OwnedEffectState::Ambiguous(attempt) => Some((slot, "ambiguous", attempt)),
                _ => None,
            });
        match pending {
            Some((slot, outcome, attempt)) => format!(
                "stage={:?} effect_slot={} effect_state={} attempt={}",
                self.stage, slot, outcome, attempt
            ),
            None => format!(
                "stage={:?} detail={}",
                self.stage,
                self.retained_reason
                    .as_deref()
                    .unwrap_or("terminal-proof-incomplete")
            ),
        }
    }

    fn effects_are_terminal(&self) -> bool {
        self.effects.iter().all(|effect| {
            matches!(
                effect,
                OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }
            )
        })
    }

    pub(super) fn replacement_terminal_is_verified(&self) -> bool {
        if !self.publication_observed || !self.envelope_released || !self.effects_are_terminal() {
            return false;
        }
        let Some(progress) = self.replacement.as_ref() else {
            return false;
        };
        let released = progress.terminal_verified
            && progress.work_transferred
            && progress.reservations_released
            && progress.preparing_released
            && progress.create_reservation.is_none()
            && progress.work_reservation.is_none()
            && matches!(
                progress.context_prerequisite,
                None | Some(OwnedEffectState::Completed { .. })
            );
        if !released {
            return false;
        }
        match self.stage {
            ReplacementStage::Completed => {
                progress.retired && progress.retirement_cleanup_completed
            }
            ReplacementStage::RolledBack => {
                progress.rollback_completed
                    && progress.committed.is_none()
                    && matches!(self.completion, Some(Err(_)))
            }
            _ => false,
        }
    }

    pub(super) fn entry_terminal_is_verified(&self, operation: Uuid) -> bool {
        self.stage == ReplacementStage::EntryTerminal
            && self.publication_observed
            && self.envelope_released
            && self.claim_released
            && self.entry_terminal.as_ref().is_some_and(|terminal| {
                matches!(
                    (&terminal.disposition, &terminal.actual_result),
                    (EntryTerminalDisposition::Success, Ok(_))
                        | (EntryTerminalDisposition::Failure, Err(_))
                        | (EntryTerminalDisposition::RolledBack, Err(_))
                ) && terminal.release_receipt.operation == operation
                    && self.prerequisite_attempt().ok()
                        == Some(terminal.release_receipt.prerequisite_attempt)
                    && self
                        .entry_create
                        .as_ref()
                        .map(|create| (create.binding, create.stage))
                        == terminal.release_receipt.binding
                    && self
                        .entry_create
                        .as_ref()
                        .and_then(|create| create.reader_demand)
                        == terminal.release_receipt.reader_demand
                    && terminal.release_receipt.executor_nonce == self.executor_nonce
                    && (terminal.release_receipt.envelope_never_sent
                        || terminal
                            .release_receipt
                            .worker_returned_and_sidecar_released)
                    && self
                        .effects
                        .iter()
                        .enumerate()
                        .all(|(index, effect)| match effect {
                            OwnedEffectState::NotStarted => {
                                terminal.release_receipt.reached_effects[index].is_none()
                            }
                            OwnedEffectState::Completed { attempt, .. } => {
                                terminal.release_receipt.reached_effects[index] == Some(*attempt)
                            }
                            _ => false,
                        })
            })
    }

    pub(super) fn prerequisite_attempt(&self) -> Result<Option<Uuid>, ()> {
        match self
            .entry_create
            .as_ref()
            .and_then(|create| create.preparation_receipt.as_ref())
        {
            None => Ok(None),
            Some(OwnedEffectState::Completed { attempt, .. }) => Ok(Some(*attempt)),
            _ => Err(()),
        }
    }
    /// Actual body return plus terminal binding/effects; an Err by itself is
    /// never a pending rollback or finalized-row cleanup certificate.
    pub(super) fn entry_body_terminal(&self, actual: &Result<SessionInfo, String>) -> bool {
        let result_matches = match (self.body_result.as_ref(), actual) {
            (Some(Ok(recorded)), Ok(actual)) => serde_json::to_value(recorded)
                .ok()
                .zip(serde_json::to_value(actual).ok())
                .is_some_and(|(a, b)| a == b),
            (Some(Err(recorded)), Err(actual)) => recorded == actual,
            _ => false,
        };
        if !result_matches {
            return false;
        }
        let published = matches!(
            &self.effects[LifecycleEffectSlot::Publication as usize],
            OwnedEffectState::Completed { .. }
        );
        match (
            actual,
            self.entry_create.as_ref().map(|create| create.stage),
        ) {
            (Ok(info), Some(EntryCreateStage::Finalized)) => {
                published
                    && self.entry_create.as_ref().is_some_and(|create| {
                        create.reader_demand.is_some()
                            && info.id == create.binding.session_id().to_string()
                    })
            }
            (Err(_), Some(EntryCreateStage::RolledBack)) => true,
            (Err(_), Some(EntryCreateStage::Finalized)) => published,
            (Err(_), Some(EntryCreateStage::Reserved)) => true,
            (Err(_), None) => {
                self.pre_teardown_failure
                    || published
                    || matches!(&self.effects[LifecycleEffectSlot::Retirement as usize],
                        OwnedEffectState::Completed { outcome: Err(error), .. }
                            if error.starts_with("old_lost=false:"))
                    || matches!(&self.effects[LifecycleEffectSlot::Retirement as usize],
                        OwnedEffectState::Completed { outcome: Ok(Some(outcome)), .. }
                            if outcome == "old_lost=false")
            }
            _ => false,
        }
    }
}

pub(super) struct ReplacementOwner {
    pub(super) changed: Notify,
    pub(super) operation: Uuid,
    pub(super) descriptor: OwnedReplacementDescriptor,
    pub(super) state: Mutex<ReplacementState>,
}

impl fmt::Debug for ReplacementOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplacementOwner")
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

/// Non-Clone transient executor sidecar. No executor/guard/app backreference is
/// stored in the coordinator's data-only root.
#[derive(Debug)]
pub(super) struct OwnedLifecycleEntry {
    pub(super) owner: Arc<ReplacementOwner>,
    pub(super) executor_nonce: Uuid,
    pub(super) authority: Option<super::LifecycleLease>,
}

impl OwnedLifecycleEntry {
    pub(super) async fn release_lease<T>(&mut self, executor: &dyn super::LifecycleExecutor<T>) {
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let current = self.owner.state.lock().ok().is_some_and(|state| {
            state.executor_nonce == self.executor_nonce && state.stage == ReplacementStage::Returned
        });
        let authority = self.authority.take();
        drop(permit);
        let released = authority.is_some_and(|authority| executor.release(authority));
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        if let Ok(mut state) = self.owner.state.lock() {
            if current
                && state.executor_nonce == self.executor_nonce
                && state.stage == ReplacementStage::Returned
            {
                state.sidecar_released = released;
            }
        }
        drop(permit);
    }
    pub(super) fn observe_authority_interruption(&self) {
        if let Ok(mut state) = self.owner.state.lock() {
            if state.executor_nonce == self.executor_nonce {
                state.stage = ReplacementStage::Ambiguous;
                state.retained_reason = Some("lifecycle authority changed before teardown".into());
            }
        }
    }

    pub(super) fn observe_return(&self, result: &Result<SessionInfo, String>) {
        if let Ok(mut state) = self.owner.state.lock() {
            if state.executor_nonce == self.executor_nonce
                && state.stage == ReplacementStage::Executing
            {
                state.stage = ReplacementStage::Returned;
                state.body_result = Some(result.clone());
                state.completion = Some(result.clone());
            }
        }
    }
}

impl Drop for OwnedLifecycleEntry {
    fn drop(&mut self) {
        // Release real target guards and tracker claim BEFORE observing release.
        let released = self.authority.take().map(|authority| authority.release());
        if let Ok(mut state) = self.owner.state.lock() {
            if state.executor_nonce == self.executor_nonce
                && state.stage == ReplacementStage::Returned
            {
                if let Some(released) = released {
                    state.sidecar_released = released;
                }
            }
            if state.executor_nonce == self.executor_nonce
                && matches!(
                    state.stage,
                    ReplacementStage::Queued | ReplacementStage::Executing
                )
            {
                state.stage = ReplacementStage::Ambiguous;
                state.retained_reason =
                    Some("lifecycle executor ended without observed settlement".into());
            }
        }
    }
}

// The existing coordinator owns the single retained-root state. Replacement
// entries join this same state; restore metadata is never a separate registry.
pub(super) struct RetainedReplacements {
    pub(super) restore_active: Arc<AtomicBool>,
    pub(super) restore_nonce: Option<Uuid>,
    pub(super) sealed: bool,
    pub(super) unavailable: bool,
    pub(super) entries: HashMap<Uuid, Arc<ReplacementOwner>>,
}

impl RetainedReplacements {
    pub(super) fn new() -> Self {
        Self::reserve(COORDINATOR_ADMISSION_CAPACITY)
    }

    pub(super) fn reserve(demand: usize) -> Self {
        let mut entries = HashMap::new();
        let unavailable = entries.try_reserve(demand).is_err();
        Self {
            restore_active: Arc::new(AtomicBool::new(true)),
            restore_nonce: None,
            sealed: false,
            unavailable,
            entries,
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum FixtureRootReservation {
    Normal65,
    RejectCapacity,
}
