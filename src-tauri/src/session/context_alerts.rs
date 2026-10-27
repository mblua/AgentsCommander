use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::phone::mailbox::{InternalNoticeGuard, InternalSystemNotice, InternalSystemTarget};
use crate::pty::context_scrape::ContextSample;

#[cfg(test)]
use crate::commands::entity_creation::parse_team_from_workgroup_name;
#[cfg(test)]
use crate::pty::context_scrape::ContextScraper;
#[cfg(test)]
use crate::session::manager::SessionManager;
#[cfg(test)]
use crate::session::session::{Session, SessionStatus};
#[cfg(test)]
use std::collections::HashSet;

mod actor;
mod runtime;

use actor::run_context_alert_actor;
#[cfg(test)]
use actor::{
    apply_numeric_resolution, dispatch_due_batch, fail_batch, handle_delivery_completion,
    process_sample, reconcile_policy, retry_delay, run_maintenance,
};
use runtime::ProductionContextAlertRuntime;
#[cfg(test)]
use runtime::{
    prepare_internal_route_blocking, resolve_member_policy_blocking,
    session_matches_policy_snapshot, validate_attempt_guard, validated_project_fqn_component,
};

pub(crate) const CONTEXT_SAMPLE_QUEUE_CAPACITY: usize = 1024;
pub(crate) const CONTEXT_ALERT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);

const FIRST_RETRY_DELAY: Duration = Duration::from_secs(5);
const HOLD_RECHECK_DELAY: Duration = Duration::from_secs(2);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemberIdentityFingerprint {
    session_id: Uuid,
    agent_id: String,
    working_directory: String,
    project_dir: PathBuf,
    ac_root: PathBuf,
    workgroup_dir: PathBuf,
    replica_dir: PathBuf,
    matrix_dir: PathBuf,
    project: String,
    team: String,
    workgroup: String,
    member: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPolicy {
    fingerprint: MemberIdentityFingerprint,
    thresholds: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MemberPolicyResolution {
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
struct AttemptSnapshot {
    generation: u64,
    session_id: Uuid,
    fingerprint: MemberIdentityFingerprint,
    observed: u8,
    thresholds: Vec<u8>,
    failure_count: u32,
}

struct ReadyAttempt {
    snapshot: AttemptSnapshot,
    current_policy: Vec<u8>,
    target: InternalSystemTarget,
    notice: InternalSystemNotice,
    guard: InternalNoticeGuard,
    cancellation: CancellationToken,
}

impl ReadyAttempt {
    /// A one-line re-export, so the `Option` the notice answers with is the
    /// `Option` every reader here sees; this is why the change is a chain and not
    /// a list of construction sites.
    fn thresholds(&self) -> Option<&[u8]> {
        self.notice.thresholds()
    }
}

/// The ready arm carries the notice, which is the largest value this actor
/// builds, and it is also the arm that is routinely constructed: boxing it would
/// add an allocation to every attempt to buy nothing measurable.
#[allow(clippy::large_enum_variant)]
enum AttemptPreparation {
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

trait ContextAlertRuntime: Send + Sync {
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

trait AlertClock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'static, ()>;
}

#[derive(Default)]
struct ProductionAlertClock;

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

pub(crate) struct ContextAlertMonitor {
    sender: mpsc::Sender<ContextSample>,
    cancellation: CancellationToken,
    join: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl ContextAlertMonitor {
    pub(crate) fn start(
        app: tauri::AppHandle,
        cancellation: CancellationToken,
        messages: Arc<crate::config::injected_messages::MessageWarnings>,
    ) -> Arc<Self> {
        Self::start_with_runtime(
            Arc::new(ProductionContextAlertRuntime { app, messages }),
            Arc::new(ProductionAlertClock),
            cancellation,
        )
    }

    fn start_with_runtime(
        runtime: Arc<dyn ContextAlertRuntime>,
        clock: Arc<dyn AlertClock>,
        cancellation: CancellationToken,
    ) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(CONTEXT_SAMPLE_QUEUE_CAPACITY);
        let actor_cancellation = cancellation.clone();
        let join = tauri::async_runtime::spawn(async move {
            run_context_alert_actor(runtime, clock, receiver, actor_cancellation).await;
        });
        Arc::new(Self {
            sender,
            cancellation,
            join: Mutex::new(Some(join)),
        })
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<ContextSample> {
        self.sender.clone()
    }

    pub(crate) fn request_close(&self) {
        self.cancellation.cancel();
    }

    pub(crate) async fn close_and_join(&self) -> Result<(), String> {
        self.request_close();
        let handle = self
            .join
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let Some(handle) = handle else {
            return Ok(());
        };
        handle
            .await
            .map_err(|error| format!("Context alert actor join failed: {}", error))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LatchState {
    Armed,
    Latched,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ErrorMarker {
    class: String,
    message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ResolutionRecovery {
    policy: bool,
    runtime: bool,
}

struct SessionAlertState {
    fingerprint: Option<MemberIdentityFingerprint>,
    policy: Vec<u8>,
    last_valid_percent: Option<u8>,
    latches: BTreeMap<u8, LatchState>,
    outstanding: BTreeMap<u8, u64>,
    policy_error: Option<ErrorMarker>,
    runtime_error: Option<ErrorMarker>,
}

impl SessionAlertState {
    fn empty() -> Self {
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
struct AlertBatch {
    generation: u64,
    session_id: Uuid,
    fingerprint: MemberIdentityFingerprint,
    observed: u8,
    thresholds: Vec<u8>,
    failure_count: u32,
    due_at: Instant,
    in_flight: bool,
}

struct ActorState {
    sessions: HashMap<Uuid, SessionAlertState>,
    batches: BTreeMap<u64, AlertBatch>,
    next_generation: Option<u64>,
    generation_exhaustion_logged: bool,
}

impl ActorState {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            batches: BTreeMap::new(),
            next_generation: Some(1),
            generation_exhaustion_logged: false,
        }
    }

    fn allocate_generation(&mut self) -> Option<u64> {
        let generation = self.next_generation?;
        self.next_generation = generation.checked_add(1);
        Some(generation)
    }

    fn remove_batch(&mut self, generation: u64) -> Option<AlertBatch> {
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

    fn remove_session(&mut self, session_id: Uuid, in_flight: Option<&InFlightSlot>) {
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

    fn earliest_due(&self) -> Option<(Instant, u64)> {
        self.batches
            .values()
            .filter(|batch| !batch.in_flight)
            .map(|batch| (batch.due_at, batch.generation))
            .min()
    }
}

struct InFlightSlot {
    generation: u64,
    cancellation: CancellationToken,
    join: tauri::async_runtime::JoinHandle<()>,
}

struct DeliveryCompletion {
    generation: u64,
    result: Result<(), String>,
}

#[cfg(test)]
mod tests;
