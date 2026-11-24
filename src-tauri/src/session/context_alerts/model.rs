//! Data model of the context-alert monitor: retry timing constants, member identity and
//! policy types, attempt snapshots, the `ContextAlertServices`/`ContextAlertRuntime`/
//! `AlertClock` traits and the actor's per-session and batch state. It imports nothing from
//! its parent, so `actor.rs` and `runtime.rs` depend on it without a cycle.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::phone::mailbox::{InternalNoticeGuard, InternalSystemNotice, InternalSystemTarget};
use crate::session::session::Session;
pub(crate) const CONTEXT_ALERT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);

pub(super) const FIRST_RETRY_DELAY: Duration = Duration::from_secs(5);
pub(super) const HOLD_RECHECK_DELAY: Duration = Duration::from_secs(2);
pub(super) const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MemberIdentityFingerprint {
    pub(super) session_id: Uuid,
    pub(super) agent_id: String,
    pub(super) working_directory: String,
    pub(super) project_dir: PathBuf,
    pub(super) ac_root: PathBuf,
    pub(super) workgroup_dir: PathBuf,
    pub(super) replica_dir: PathBuf,
    pub(super) matrix_dir: PathBuf,
    pub(super) project: String,
    pub(super) team: String,
    pub(super) workgroup: String,
    pub(super) member: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedPolicy {
    pub(super) fingerprint: MemberIdentityFingerprint,
    pub(super) thresholds: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MemberPolicyResolution {
    Eligible(ResolvedPolicy),
    Disabled(MemberIdentityFingerprint),
    PermanentIneligible(String),
    Paused {
        fingerprint: MemberIdentityFingerprint,
        class: &'static str,
        message: String,
    },
    RetryableFailure(String),
}

#[derive(Debug, Clone)]
pub(super) struct AttemptSnapshot {
    pub(super) generation: u64,
    pub(super) session_id: Uuid,
    pub(super) fingerprint: MemberIdentityFingerprint,
    pub(super) observed: u8,
    pub(super) thresholds: Vec<u8>,
    pub(super) failure_count: u32,
}

pub(super) struct ReadyAttempt {
    pub(super) snapshot: AttemptSnapshot,
    pub(super) current_policy: Vec<u8>,
    pub(super) target: InternalSystemTarget,
    pub(super) notice: InternalSystemNotice,
    pub(super) guard: InternalNoticeGuard,
    pub(super) cancellation: CancellationToken,
}

impl ReadyAttempt {
    /// A one-line re-export, so the `Option` the notice answers with is the
    /// `Option` every reader here sees; this is why the change is a chain and not
    /// a list of construction sites.
    pub(super) fn thresholds(&self) -> Option<&[u8]> {
        self.notice.thresholds()
    }
}

/// The ready arm carries the notice, which is the largest value this actor
/// builds, and it is also the arm that is routinely constructed: boxing it would
/// add an allocation to every attempt to buy nothing measurable.
#[allow(
    clippy::large_enum_variant,
    reason = "Ready holds a full attempt; short-lived local enum, boxing adds noise"
)]
pub(super) enum AttemptPreparation {
    Ready(ReadyAttempt),
    Cancel {
        reason: String,
        current_policy: Option<ResolvedPolicy>,
    },
    Paused {
        fingerprint: MemberIdentityFingerprint,
        class: &'static str,
        message: String,
    },
    RetryableFailure(String),
}

pub(crate) trait ContextAlertServices: Send + Sync {
    fn public_session(&self, id: Uuid) -> BoxFuture<'static, Option<Session>>;
    fn registered(&self, id: Uuid) -> bool;
    fn retire_sample_registration(&self, id: Uuid);
    fn validate_delivery_state(
        &self,
        id: Uuid,
        target: &InternalSystemTarget,
    ) -> Result<(), String>;
    fn deliver(
        &self,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        cancellation: CancellationToken,
        guard: InternalNoticeGuard,
    ) -> BoxFuture<'static, Result<(), String>>;
}

pub(super) trait ContextAlertRuntime: Send + Sync {
    fn resolve_member_policy(&self, session_id: Uuid)
        -> BoxFuture<'static, MemberPolicyResolution>;
    fn check_session_live(&self, session_id: Uuid) -> BoxFuture<'static, bool>;
    fn retire_sample_registration(&self, session_id: Uuid);
    fn prepare_attempt(
        &self,
        snapshot: AttemptSnapshot,
        cancellation: CancellationToken,
    ) -> BoxFuture<'static, AttemptPreparation>;
    fn deliver_attempt(&self, attempt: ReadyAttempt) -> BoxFuture<'static, Result<(), String>>;
}

pub(super) trait AlertClock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'static, ()>;
}

#[derive(Default)]
pub(super) struct ProductionAlertClock;

impl AlertClock for ProductionAlertClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
            deadline,
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LatchState {
    Armed,
    Latched,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ErrorMarker {
    pub(super) class: String,
    pub(super) message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ResolutionRecovery {
    pub(super) policy: bool,
    pub(super) runtime: bool,
}

pub(super) struct SessionAlertState {
    pub(super) fingerprint: Option<MemberIdentityFingerprint>,
    pub(super) policy: Vec<u8>,
    pub(super) last_valid_percent: Option<u8>,
    pub(super) latches: BTreeMap<u8, LatchState>,
    pub(super) outstanding: BTreeMap<u8, u64>,
    pub(super) policy_error: Option<ErrorMarker>,
    pub(super) runtime_error: Option<ErrorMarker>,
}

impl SessionAlertState {
    pub(super) fn empty() -> Self {
        Self {
            fingerprint: None,
            policy: Vec::new(),
            last_valid_percent: None,
            latches: BTreeMap::new(),
            outstanding: BTreeMap::new(),
            policy_error: None,
            runtime_error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct AlertBatch {
    pub(super) generation: u64,
    pub(super) session_id: Uuid,
    pub(super) fingerprint: MemberIdentityFingerprint,
    pub(super) observed: u8,
    pub(super) thresholds: Vec<u8>,
    pub(super) failure_count: u32,
    pub(super) due_at: Instant,
    pub(super) in_flight: bool,
}

pub(super) struct ActorState {
    pub(super) sessions: HashMap<Uuid, SessionAlertState>,
    pub(super) batches: BTreeMap<u64, AlertBatch>,
    pub(super) next_generation: Option<u64>,
    pub(super) generation_exhaustion_logged: bool,
}

impl ActorState {
    pub(super) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            batches: BTreeMap::new(),
            next_generation: Some(1),
            generation_exhaustion_logged: false,
        }
    }

    pub(super) fn allocate_generation(&mut self) -> Option<u64> {
        let generation = self.next_generation?;
        self.next_generation = generation.checked_add(1);
        Some(generation)
    }

    pub(super) fn remove_batch(&mut self, generation: u64) -> Option<AlertBatch> {
        let batch = self.batches.remove(&generation)?;
        if let Some(session) = self.sessions.get_mut(&batch.session_id) {
            for threshold in &batch.thresholds {
                if session.outstanding.get(threshold) == Some(&generation) {
                    session.outstanding.remove(threshold);
                }
            }
        }
        Some(batch)
    }

    pub(super) fn remove_session(&mut self, session_id: Uuid, in_flight: Option<&InFlightSlot>) {
        if let Some(slot) = in_flight {
            if self
                .batches
                .get(&slot.generation)
                .is_some_and(|batch| batch.session_id == session_id)
            {
                slot.cancellation.cancel();
            }
        }
        self.sessions.remove(&session_id);
        self.batches
            .retain(|_, batch| batch.session_id != session_id);
    }

    pub(super) fn earliest_due(&self) -> Option<(Instant, u64)> {
        self.batches
            .values()
            .filter(|batch| !batch.in_flight)
            .map(|batch| (batch.due_at, batch.generation))
            .min()
    }
}

pub(super) struct InFlightSlot {
    pub(super) generation: u64,
    pub(super) cancellation: CancellationToken,
    pub(super) join: tokio::task::JoinHandle<()>,
}

pub(super) struct DeliveryCompletion {
    pub(super) generation: u64,
    pub(super) result: Result<(), String>,
}
