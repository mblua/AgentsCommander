use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use tauri::Manager;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::commands::entity_creation::{
    metadata_is_link_or_reparse, parse_team_from_workgroup_name, read_team_config_classified,
    TeamConfigReadError,
};
use crate::phone::mailbox::{
    InternalNoticeGuard, InternalSystemNotice, InternalSystemTarget, MailboxPoller,
};
use crate::pty::context_scrape::{ContextSample, ContextScraper};
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionStatus};

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
    pub(crate) fn start(app: tauri::AppHandle, cancellation: CancellationToken) -> Arc<Self> {
        Self::start_with_runtime(
            Arc::new(ProductionContextAlertRuntime { app }),
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

fn retry_delay(failure_count: u32) -> Duration {
    if failure_count == 0 {
        return Duration::ZERO;
    }
    let shift = failure_count.saturating_sub(1).min(4);
    FIRST_RETRY_DELAY
        .checked_mul(1u32 << shift)
        .unwrap_or(MAX_RETRY_DELAY)
        .min(MAX_RETRY_DELAY)
}

fn optional_sleep(clock: Arc<dyn AlertClock>, deadline: Option<Instant>) -> BoxFuture<'static, ()> {
    match deadline {
        Some(deadline) => clock.sleep_until(deadline),
        None => Box::pin(futures::future::pending()),
    }
}

fn next_actor_deadline(
    state: &ActorState,
    in_flight: Option<&InFlightSlot>,
    maintenance_deadline: Instant,
) -> Instant {
    if in_flight.is_some() {
        return maintenance_deadline;
    }
    state
        .earliest_due()
        .map(|(deadline, _)| deadline.min(maintenance_deadline))
        .unwrap_or(maintenance_deadline)
}

async fn run_context_alert_actor(
    runtime: Arc<dyn ContextAlertRuntime>,
    clock: Arc<dyn AlertClock>,
    mut samples: mpsc::Receiver<ContextSample>,
    shutdown: CancellationToken,
) {
    let mut state = ActorState::new();
    let mut samples_open = true;
    let mut maintenance_deadline = clock.now() + CONTEXT_ALERT_MAINTENANCE_INTERVAL;
    let (completion_tx, mut completion_rx) = mpsc::channel::<DeliveryCompletion>(1);
    let mut in_flight: Option<InFlightSlot> = None;

    loop {
        let deadline = next_actor_deadline(&state, in_flight.as_ref(), maintenance_deadline);
        let sleep = optional_sleep(Arc::clone(&clock), Some(deadline));
        tokio::pin!(sleep);

        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                if let Some(slot) = in_flight.take() {
                    slot.cancellation.cancel();
                    if let Err(error) = slot.join.await {
                        log::warn!("[context-alert] delivery wrapper join failed during shutdown: {}", error);
                    } else if let Ok(completion) = completion_rx.try_recv() {
                        if let Err(error) = completion.result {
                            log::warn!(
                                "[context-alert] delivery generation={} ended with error during shutdown: {}",
                                completion.generation,
                                error
                            );
                        }
                    }
                }
                state.sessions.clear();
                state.batches.clear();
                break;
            }
            completion = completion_rx.recv(), if in_flight.is_some() => {
                if let Some(completion) = completion {
                    handle_delivery_completion(
                        &mut state,
                        &mut in_flight,
                        completion,
                        clock.now(),
                    ).await;
                    if clock.now() >= maintenance_deadline {
                        run_maintenance(
                            &runtime,
                            &mut state,
                            in_flight.as_ref(),
                            &shutdown,
                        ).await;
                        maintenance_deadline = clock.now() + CONTEXT_ALERT_MAINTENANCE_INTERVAL;
                    }
                    dispatch_due_batch(
                        &runtime,
                        &clock,
                        &mut state,
                        &mut in_flight,
                        &completion_tx,
                        &shutdown,
                    ).await;
                }
            }
            _ = &mut sleep => {
                if clock.now() >= maintenance_deadline {
                    run_maintenance(
                        &runtime,
                        &mut state,
                        in_flight.as_ref(),
                        &shutdown,
                    ).await;
                    maintenance_deadline = clock.now() + CONTEXT_ALERT_MAINTENANCE_INTERVAL;
                }
                if in_flight.is_none() {
                    dispatch_due_batch(
                        &runtime,
                        &clock,
                        &mut state,
                        &mut in_flight,
                        &completion_tx,
                        &shutdown,
                    ).await;
                }
            }
            sample = samples.recv(), if samples_open => {
                match sample {
                    Some(sample) => {
                        process_sample(
                            &runtime,
                            &clock,
                            &mut state,
                            in_flight.as_ref(),
                            sample,
                            &shutdown,
                        ).await;
                        if clock.now() >= maintenance_deadline {
                            run_maintenance(
                                &runtime,
                                &mut state,
                                in_flight.as_ref(),
                                &shutdown,
                            ).await;
                            maintenance_deadline = clock.now() + CONTEXT_ALERT_MAINTENANCE_INTERVAL;
                        }
                        if in_flight.is_none() {
                            dispatch_due_batch(
                                &runtime,
                                &clock,
                                &mut state,
                                &mut in_flight,
                                &completion_tx,
                                &shutdown,
                            ).await;
                        }
                    }
                    None => {
                        samples_open = false;
                        log::warn!("[context-alert] sample channel closed; maintenance and pending retries remain active");
                    }
                }
            }
        }
    }
}

async fn process_sample(
    runtime: &Arc<dyn ContextAlertRuntime>,
    clock: &Arc<dyn AlertClock>,
    state: &mut ActorState,
    in_flight: Option<&InFlightSlot>,
    sample: ContextSample,
    shutdown: &CancellationToken,
) {
    match sample {
        ContextSample::SessionOver { session_id } => {
            state.remove_session(session_id, in_flight);
        }
        ContextSample::Unavailable { session_id } => {
            let live = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                live = runtime.check_session_live(session_id) => live,
            };
            if !live {
                state.remove_session(session_id, in_flight);
                runtime.retire_sample_registration(session_id);
            }
        }
        ContextSample::Reading {
            session_id,
            percent,
        } => {
            if percent > 100 {
                log::error!(
                    "[context-alert] impossible producer reading session={} percent={}",
                    session_id,
                    percent
                );
                return;
            }
            let resolution = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                resolution = runtime.resolve_member_policy(session_id) => resolution,
            };
            apply_numeric_resolution(
                state,
                in_flight,
                session_id,
                percent,
                resolution,
                clock.now(),
            );
        }
    }
}

fn apply_numeric_resolution(
    state: &mut ActorState,
    in_flight: Option<&InFlightSlot>,
    session_id: Uuid,
    percent: u8,
    resolution: MemberPolicyResolution,
    now: Instant,
) -> ResolutionRecovery {
    match resolution {
        MemberPolicyResolution::Eligible(resolved) => {
            if resolved.fingerprint.session_id != session_id {
                log::error!(
                    "[context-alert] runtime returned mismatched session identity expected={} actual={}",
                    session_id,
                    resolved.fingerprint.session_id
                );
                return ResolutionRecovery::default();
            }
            let changed_identity = state
                .sessions
                .get(&session_id)
                .and_then(|session| session.fingerprint.as_ref())
                .is_some_and(|fingerprint| fingerprint != &resolved.fingerprint);
            if changed_identity {
                state.remove_session(session_id, in_flight);
            }
            let session = state
                .sessions
                .entry(session_id)
                .or_insert_with(SessionAlertState::empty);
            session.fingerprint = Some(resolved.fingerprint.clone());
            let recovery = clear_resolution_errors(session, session_id);
            reconcile_policy(
                state,
                in_flight,
                session_id,
                &resolved.fingerprint,
                &resolved.thresholds,
                now,
            );
            if recovery.policy {
                make_session_batches_due_now(state, session_id, now);
            }
            evaluate_numeric_sample(state, session_id, percent, now);
            recovery
        }
        MemberPolicyResolution::Disabled(fingerprint) => {
            let recovery = clear_resolution_errors_for_identity(state, session_id, &fingerprint);
            state.remove_session(session_id, in_flight);
            recovery
        }
        MemberPolicyResolution::PermanentIneligible(_) => {
            state.remove_session(session_id, in_flight);
            ResolutionRecovery::default()
        }
        MemberPolicyResolution::Paused {
            fingerprint,
            class,
            message,
        } => {
            let changed_identity = state
                .sessions
                .get(&session_id)
                .and_then(|session| session.fingerprint.as_ref())
                .is_some_and(|current| current != &fingerprint);
            if changed_identity {
                state.remove_session(session_id, in_flight);
            }
            let session = state
                .sessions
                .entry(session_id)
                .or_insert_with(SessionAlertState::empty);
            session.fingerprint = Some(fingerprint);
            set_policy_error(session, session_id, class, message);
            ResolutionRecovery::default()
        }
        MemberPolicyResolution::RetryableFailure(message) => {
            let session = state
                .sessions
                .entry(session_id)
                .or_insert_with(SessionAlertState::empty);
            set_runtime_error(session, session_id, message);
            ResolutionRecovery::default()
        }
    }
}

fn set_policy_error(
    session: &mut SessionAlertState,
    session_id: Uuid,
    class: &'static str,
    message: String,
) {
    let next = ErrorMarker {
        class: class.to_string(),
        message,
    };
    if session.policy_error.as_ref() != Some(&next) {
        log::warn!(
            "[context-alert] policy paused session={} class={} error={}",
            session_id,
            next.class,
            next.message
        );
        session.policy_error = Some(next);
    }
}

fn clear_policy_error(session: &mut SessionAlertState, session_id: Uuid) -> bool {
    if session.policy_error.take().is_some() {
        log::info!("[context-alert] policy recovered session={}", session_id);
        true
    } else {
        false
    }
}

fn set_runtime_error(session: &mut SessionAlertState, session_id: Uuid, message: String) {
    let next = ErrorMarker {
        class: "runtime".to_string(),
        message,
    };
    if session.runtime_error.as_ref() != Some(&next) {
        log::warn!(
            "[context-alert] resolution failed session={} error={}",
            session_id,
            next.message
        );
        session.runtime_error = Some(next);
    }
}

fn clear_runtime_error(session: &mut SessionAlertState, session_id: Uuid) -> bool {
    if session.runtime_error.take().is_some() {
        log::info!(
            "[context-alert] resolution recovered session={}",
            session_id
        );
        true
    } else {
        false
    }
}

fn clear_resolution_errors(
    session: &mut SessionAlertState,
    session_id: Uuid,
) -> ResolutionRecovery {
    ResolutionRecovery {
        runtime: clear_runtime_error(session, session_id),
        policy: clear_policy_error(session, session_id),
    }
}

fn clear_resolution_errors_for_identity(
    state: &mut ActorState,
    session_id: Uuid,
    fingerprint: &MemberIdentityFingerprint,
) -> ResolutionRecovery {
    let same_identity = fingerprint.session_id == session_id
        && state.sessions.get(&session_id).is_some_and(|session| {
            session
                .fingerprint
                .as_ref()
                .is_none_or(|current| current == fingerprint)
        });
    if !same_identity {
        return ResolutionRecovery::default();
    }
    state
        .sessions
        .get_mut(&session_id)
        .map(|session| clear_resolution_errors(session, session_id))
        .unwrap_or_default()
}

fn reconcile_policy(
    state: &mut ActorState,
    in_flight: Option<&InFlightSlot>,
    session_id: Uuid,
    fingerprint: &MemberIdentityFingerprint,
    new_policy: &[u8],
    now: Instant,
) {
    let old_policy = state
        .sessions
        .get(&session_id)
        .map(|session| session.policy.clone())
        .unwrap_or_default();
    let removed: HashSet<u8> = old_policy
        .iter()
        .copied()
        .filter(|threshold| !new_policy.contains(threshold))
        .collect();

    if !removed.is_empty() {
        let affected: Vec<u64> = state
            .batches
            .values()
            .filter(|batch| {
                batch.session_id == session_id
                    && batch
                        .thresholds
                        .iter()
                        .any(|threshold| removed.contains(threshold))
            })
            .map(|batch| batch.generation)
            .collect();
        let mut replacements = Vec::new();
        for generation in affected {
            let Some(mut batch) = state.batches.remove(&generation) else {
                continue;
            };
            let old_thresholds = batch.thresholds.clone();
            batch
                .thresholds
                .retain(|threshold| !removed.contains(threshold));
            if in_flight.is_some_and(|slot| slot.generation == generation) {
                if let Some(slot) = in_flight {
                    slot.cancellation.cancel();
                }
                if !batch.thresholds.is_empty() {
                    replacements.push((
                        batch.observed,
                        batch.thresholds.clone(),
                        batch.failure_count,
                    ));
                }
            } else if !batch.thresholds.is_empty() {
                state.batches.insert(generation, batch);
            }
            if let Some(session) = state.sessions.get_mut(&session_id) {
                for threshold in old_thresholds {
                    if session.outstanding.get(&threshold) == Some(&generation) {
                        session.outstanding.remove(&threshold);
                    }
                }
                if let Some(current) = state.batches.get(&generation) {
                    for threshold in &current.thresholds {
                        session.outstanding.insert(*threshold, generation);
                    }
                }
            }
        }
        for (observed, thresholds, failure_count) in replacements {
            create_batch(
                state,
                session_id,
                fingerprint.clone(),
                observed,
                thresholds,
                failure_count,
                now,
            );
        }
    }

    let session = state
        .sessions
        .entry(session_id)
        .or_insert_with(SessionAlertState::empty);
    session.policy = new_policy.to_vec();
    session
        .latches
        .retain(|threshold, _| new_policy.contains(threshold));
    session
        .outstanding
        .retain(|threshold, _| new_policy.contains(threshold));
    for threshold in new_policy {
        session
            .latches
            .entry(*threshold)
            .or_insert(LatchState::Armed);
    }
}

fn evaluate_numeric_sample(state: &mut ActorState, session_id: Uuid, percent: u8, now: Instant) {
    let Some(session) = state.sessions.get_mut(&session_id) else {
        return;
    };
    let Some(fingerprint) = session.fingerprint.clone() else {
        return;
    };
    let mut newly_crossed = Vec::new();
    for threshold in session.policy.clone() {
        let latch = session
            .latches
            .entry(threshold)
            .or_insert(LatchState::Armed);
        if percent < threshold {
            *latch = LatchState::Armed;
        } else if *latch == LatchState::Armed {
            *latch = LatchState::Latched;
            if !session.outstanding.contains_key(&threshold) {
                newly_crossed.push(threshold);
            }
        }
    }
    session.last_valid_percent = Some(percent);
    if newly_crossed.is_empty() {
        return;
    }
    newly_crossed.sort_unstable();
    create_batch(
        state,
        session_id,
        fingerprint,
        percent,
        newly_crossed,
        0,
        now,
    );
}

fn create_batch(
    state: &mut ActorState,
    session_id: Uuid,
    fingerprint: MemberIdentityFingerprint,
    observed: u8,
    thresholds: Vec<u8>,
    failure_count: u32,
    due_at: Instant,
) {
    let Some(generation) = state.allocate_generation() else {
        if !state.generation_exhaustion_logged {
            log::error!("[context-alert] batch generation exhausted; new crossings remain latched");
            state.generation_exhaustion_logged = true;
        }
        return;
    };
    if let Some(session) = state.sessions.get_mut(&session_id) {
        for threshold in &thresholds {
            session.outstanding.insert(*threshold, generation);
        }
    }
    state.batches.insert(
        generation,
        AlertBatch {
            generation,
            session_id,
            fingerprint,
            observed,
            thresholds,
            failure_count,
            due_at,
            in_flight: false,
        },
    );
}

fn make_session_batches_due_now(state: &mut ActorState, session_id: Uuid, now: Instant) {
    for batch in state.batches.values_mut() {
        if batch.session_id == session_id && !batch.in_flight {
            batch.due_at = now;
        }
    }
}

async fn dispatch_due_batch(
    runtime: &Arc<dyn ContextAlertRuntime>,
    clock: &Arc<dyn AlertClock>,
    state: &mut ActorState,
    in_flight: &mut Option<InFlightSlot>,
    completion_tx: &mpsc::Sender<DeliveryCompletion>,
    shutdown: &CancellationToken,
) -> ResolutionRecovery {
    if in_flight.is_some() {
        return ResolutionRecovery::default();
    }
    let now = clock.now();
    let Some((due_at, generation)) = state.earliest_due() else {
        return ResolutionRecovery::default();
    };
    if due_at > now {
        return ResolutionRecovery::default();
    }
    let Some(batch) = state.batches.get(&generation).cloned() else {
        return ResolutionRecovery::default();
    };
    let snapshot = AttemptSnapshot {
        generation,
        session_id: batch.session_id,
        fingerprint: batch.fingerprint.clone(),
        observed: batch.observed,
        thresholds: batch.thresholds,
        failure_count: batch.failure_count,
    };
    let attempt_cancellation = shutdown.child_token();
    let preparation = tokio::select! {
        biased;
        _ = shutdown.cancelled() => return ResolutionRecovery::default(),
        preparation = runtime.prepare_attempt(snapshot, attempt_cancellation.clone()) => preparation,
    };

    match preparation {
        AttemptPreparation::Ready(attempt) => {
            let session_id = attempt.snapshot.session_id;
            let fingerprint_matches = state
                .sessions
                .get(&session_id)
                .and_then(|session| session.fingerprint.as_ref())
                == Some(&attempt.snapshot.fingerprint);
            if !fingerprint_matches {
                state.remove_session(session_id, None);
                return ResolutionRecovery::default();
            }
            reconcile_policy(
                state,
                None,
                session_id,
                &attempt.snapshot.fingerprint,
                &attempt.current_policy,
                now,
            );
            let still_matches = state.batches.get(&generation).is_some_and(|current| {
                // Only a context alert ever reaches this actor, so the invariant
                // is named rather than defaulted: a remote notice must never
                // silently count as "no thresholds crossed".
                let attempt_thresholds = attempt
                    .thresholds()
                    .expect("a context-alert attempt always carries thresholds");
                current.thresholds == attempt_thresholds
                    && current.fingerprint == attempt.snapshot.fingerprint
            });
            if !still_matches {
                return ResolutionRecovery::default();
            }
            if let Some(current) = state.batches.get_mut(&generation) {
                current.in_flight = true;
            }
            let recovery = state
                .sessions
                .get_mut(&session_id)
                .map(|session| clear_resolution_errors(session, session_id))
                .unwrap_or_default();
            let runtime = Arc::clone(runtime);
            let tx = completion_tx.clone();
            let join = tauri::async_runtime::spawn(async move {
                let delivery =
                    tauri::async_runtime::spawn(
                        async move { runtime.deliver_attempt(attempt).await },
                    );
                let result = match delivery.await {
                    Ok(result) => result,
                    Err(error) => Err(format!("Context alert delivery task failed: {}", error)),
                };
                let _ = tx.send(DeliveryCompletion { generation, result }).await;
            });
            *in_flight = Some(InFlightSlot {
                generation,
                cancellation: attempt_cancellation,
                join,
            });
            recovery
        }
        AttemptPreparation::Cancel {
            reason,
            current_policy,
        } => {
            log::debug!(
                "[context-alert] canceled pending generation={} reason={}",
                generation,
                reason
            );
            match current_policy {
                Some(policy) if policy.fingerprint == batch.fingerprint => {
                    let recovery = clear_resolution_errors_for_identity(
                        state,
                        batch.session_id,
                        &policy.fingerprint,
                    );
                    if policy.thresholds.is_empty() {
                        state.remove_session(batch.session_id, None);
                    } else {
                        reconcile_policy(
                            state,
                            None,
                            batch.session_id,
                            &policy.fingerprint,
                            &policy.thresholds,
                            now,
                        );
                        state.remove_batch(generation);
                    }
                    recovery
                }
                _ => {
                    state.remove_session(batch.session_id, None);
                    ResolutionRecovery::default()
                }
            }
        }
        AttemptPreparation::Paused {
            fingerprint,
            class,
            message,
        } => {
            let same_identity = state
                .sessions
                .get(&batch.session_id)
                .and_then(|session| session.fingerprint.as_ref())
                == Some(&fingerprint);
            if !same_identity {
                state.remove_session(batch.session_id, None);
                return ResolutionRecovery::default();
            }
            if let Some(session) = state.sessions.get_mut(&batch.session_id) {
                set_policy_error(session, batch.session_id, class, message.clone());
            }
            fail_batch(state, generation, now, &message);
            ResolutionRecovery::default()
        }
        AttemptPreparation::RetryableFailure(message) => {
            fail_batch(state, generation, now, &message);
            ResolutionRecovery::default()
        }
    }
}

fn defer_batch(state: &mut ActorState, generation: u64, now: Instant) {
    if let Some(batch) = state.batches.get_mut(&generation) {
        batch.in_flight = false;
        batch.due_at = now + HOLD_RECHECK_DELAY;
    }
}

fn fail_batch(state: &mut ActorState, generation: u64, now: Instant, message: &str) {
    let Some(batch) = state.batches.get_mut(&generation) else {
        return;
    };
    batch.in_flight = false;
    batch.failure_count = batch.failure_count.saturating_add(1);
    batch.due_at = now + retry_delay(batch.failure_count);
    log::warn!(
        "[context-alert] generation={} attempt={} failed; retryInMs={} error={}",
        generation,
        batch.failure_count,
        retry_delay(batch.failure_count).as_millis(),
        message
    );
}

async fn handle_delivery_completion(
    state: &mut ActorState,
    in_flight: &mut Option<InFlightSlot>,
    completion: DeliveryCompletion,
    now: Instant,
) {
    let Some(slot) = in_flight.take() else {
        log::debug!(
            "[context-alert] ignored completion generation={} without a global slot",
            completion.generation
        );
        return;
    };
    let slot_generation = slot.generation;
    if let Err(error) = slot.join.await {
        log::warn!(
            "[context-alert] delivery wrapper generation={} join failed: {}",
            slot_generation,
            error
        );
    }
    if slot_generation != completion.generation {
        log::debug!(
            "[context-alert] ignored mismatched completion slot={} result={}",
            slot_generation,
            completion.generation
        );
        return;
    }
    let matching = state
        .batches
        .get(&completion.generation)
        .is_some_and(|batch| batch.in_flight);
    if !matching {
        log::debug!(
            "[context-alert] ignored stale completion generation={}",
            completion.generation
        );
        return;
    }
    match completion.result {
        Ok(()) => {
            state.remove_batch(completion.generation);
        }
        Err(message) if crate::phone::mailbox::is_deferred_internal_delivery_error(&message) => {
            defer_batch(state, completion.generation, now);
        }
        Err(message) => fail_batch(state, completion.generation, now, &message),
    }
}

async fn run_maintenance(
    runtime: &Arc<dyn ContextAlertRuntime>,
    state: &mut ActorState,
    in_flight: Option<&InFlightSlot>,
    shutdown: &CancellationToken,
) {
    let ids: Vec<Uuid> = state.sessions.keys().copied().collect();
    for session_id in ids {
        let live = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            live = runtime.check_session_live(session_id) => live,
        };
        if !live {
            state.remove_session(session_id, in_flight);
            runtime.retire_sample_registration(session_id);
        }
    }
}

#[derive(Clone)]
struct ProductionContextAlertRuntime {
    app: tauri::AppHandle,
}

impl ContextAlertRuntime for ProductionContextAlertRuntime {
    fn resolve_member_policy(
        &self,
        session_id: Uuid,
    ) -> BoxFuture<'static, MemberPolicyResolution> {
        let runtime = self.clone();
        Box::pin(async move { runtime.resolve_member_policy_inner(session_id).await })
    }

    fn check_session_live(&self, session_id: Uuid) -> BoxFuture<'static, bool> {
        let runtime = self.clone();
        Box::pin(async move { runtime.session_and_registration_live(session_id).await })
    }

    fn retire_sample_registration(&self, session_id: Uuid) {
        if let Some(scraper) = self.app.try_state::<Arc<ContextScraper>>() {
            scraper.retire_session(session_id);
        }
    }

    fn prepare_attempt(
        &self,
        snapshot: AttemptSnapshot,
        cancellation: CancellationToken,
    ) -> BoxFuture<'static, AttemptPreparation> {
        let runtime = self.clone();
        Box::pin(async move { runtime.prepare_attempt_inner(snapshot, cancellation).await })
    }

    fn deliver_attempt(&self, attempt: ReadyAttempt) -> BoxFuture<'static, Result<(), String>> {
        let app = self.app.clone();
        Box::pin(async move {
            MailboxPoller::new()
                .deliver_internal_system_notice(
                    &app,
                    attempt.target,
                    attempt.notice,
                    attempt.cancellation,
                    attempt.guard,
                )
                .await
        })
    }
}

impl ProductionContextAlertRuntime {
    async fn public_session(&self, session_id: Uuid) -> Option<Session> {
        let state = self
            .app
            .try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()?;
        let manager = state.read().await.clone();
        manager.get_session(session_id).await
    }

    fn registered(&self, session_id: Uuid) -> bool {
        self.app
            .try_state::<Arc<ContextScraper>>()
            .is_some_and(|scraper| scraper.is_session_registered(session_id))
    }

    async fn session_and_registration_live(&self, session_id: Uuid) -> bool {
        self.public_session(session_id)
            .await
            .is_some_and(|session| !matches!(session.status, SessionStatus::Exited(_)))
            && self.registered(session_id)
    }

    async fn resolve_member_policy_inner(&self, session_id: Uuid) -> MemberPolicyResolution {
        let Some(session) = self.public_session(session_id).await else {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled session is no longer present".to_string(),
            );
        };
        if session.is_root_agent
            || matches!(session.status, SessionStatus::Exited(_))
            || session.agent_id.is_none()
            || !self.registered(session_id)
        {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled session is no longer an eligible registered member".to_string(),
            );
        }
        let agent_id = session.agent_id.clone().unwrap_or_default();
        let working_directory = session.working_directory.clone();
        let blocking_session = session.clone();
        let blocking =
            tokio::task::spawn_blocking(move || resolve_member_policy_blocking(&blocking_session))
                .await;
        let resolution = match blocking {
            Ok(resolution) => resolution,
            Err(error) => {
                return MemberPolicyResolution::RetryableFailure(format!(
                    "Member policy blocking task failed for session {}: {}",
                    session_id, error
                ));
            }
        };

        if matches!(
            resolution,
            MemberPolicyResolution::Eligible(_)
                | MemberPolicyResolution::Disabled(_)
                | MemberPolicyResolution::Paused { .. }
        ) {
            let current = self.public_session(session_id).await;
            if !session_matches_policy_snapshot(
                current.as_ref(),
                session_id,
                &agent_id,
                &working_directory,
                self.registered(session_id),
            ) {
                return MemberPolicyResolution::PermanentIneligible(
                    "sampled session identity changed during policy resolution".to_string(),
                );
            }
        }
        resolution
    }

    async fn prepare_attempt_inner(
        &self,
        snapshot: AttemptSnapshot,
        cancellation: CancellationToken,
    ) -> AttemptPreparation {
        log::debug!(
            "[context-alert] preparing generation={} priorFailures={}",
            snapshot.generation,
            snapshot.failure_count
        );
        let resolution = self.resolve_member_policy_inner(snapshot.session_id).await;
        let policy = match resolution {
            MemberPolicyResolution::Eligible(policy) => policy,
            MemberPolicyResolution::Disabled(fingerprint) => {
                return AttemptPreparation::Cancel {
                    reason: "context alert policy is disabled".to_string(),
                    current_policy: Some(ResolvedPolicy {
                        fingerprint,
                        thresholds: Vec::new(),
                    }),
                }
            }
            MemberPolicyResolution::PermanentIneligible(reason) => {
                return AttemptPreparation::Cancel {
                    reason,
                    current_policy: None,
                }
            }
            MemberPolicyResolution::Paused {
                fingerprint,
                class,
                message,
            } => {
                return AttemptPreparation::Paused {
                    fingerprint,
                    class,
                    message,
                }
            }
            MemberPolicyResolution::RetryableFailure(message) => {
                return AttemptPreparation::RetryableFailure(message)
            }
        };
        if policy.fingerprint != snapshot.fingerprint {
            return AttemptPreparation::Cancel {
                reason: "sampled member identity changed before delivery".to_string(),
                current_policy: None,
            };
        }
        let thresholds: Vec<u8> = snapshot
            .thresholds
            .iter()
            .copied()
            .filter(|threshold| policy.thresholds.contains(threshold))
            .collect();
        if thresholds.is_empty() {
            return AttemptPreparation::Cancel {
                reason: "all pending context alert thresholds were removed".to_string(),
                current_policy: Some(policy),
            };
        }
        let fingerprint = snapshot.fingerprint.clone();
        let observed = snapshot.observed;
        let route = tokio::task::spawn_blocking(move || {
            prepare_internal_route_blocking(&fingerprint, observed, &thresholds)
        })
        .await;
        let (target, notice) = match route {
            Ok(Ok(route)) => route,
            Ok(Err(error)) => return AttemptPreparation::RetryableFailure(error),
            Err(error) => {
                return AttemptPreparation::RetryableFailure(format!(
                    "Orchestrator route blocking task failed: {}",
                    error
                ))
            }
        };
        let guard_app = self.app.clone();
        let guard_fingerprint = snapshot.fingerprint.clone();
        let guard_target = target.clone();
        let guard_thresholds = notice
            .thresholds()
            .expect("a context-alert notice always carries thresholds")
            .to_vec();
        let guard_cancellation = cancellation.clone();
        let guard: InternalNoticeGuard = Arc::new(move || {
            validate_attempt_guard(
                &guard_app,
                &guard_fingerprint,
                &guard_target,
                &guard_thresholds,
                &guard_cancellation,
            )
        });
        AttemptPreparation::Ready(ReadyAttempt {
            snapshot,
            current_policy: policy.thresholds,
            target,
            notice,
            guard,
            cancellation,
        })
    }
}

fn session_matches_policy_snapshot(
    current: Option<&Session>,
    session_id: Uuid,
    agent_id: &str,
    working_directory: &str,
    registered: bool,
) -> bool {
    current.is_some_and(|current| {
        current.id == session_id
            && current.agent_id.as_deref() == Some(agent_id)
            && current.working_directory == working_directory
            && !current.is_root_agent
            && !matches!(current.status, SessionStatus::Exited(_))
            && registered
    })
}

fn canonical_real_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("{} '{}' is not readable: {}", label, path.display(), error))?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        return Err(format!(
            "{} '{}' must be a real non-link directory",
            label,
            path.display()
        ));
    }
    std::fs::canonicalize(path)
        .map(|canonical| crate::path_utils::normalize_windows_verbatim_path_buf(&canonical))
        .map_err(|error| {
            format!(
                "{} '{}' cannot be canonicalized: {}",
                label,
                path.display(),
                error
            )
        })
}

fn resolve_member_policy_blocking(session: &Session) -> MemberPolicyResolution {
    let lexical_cwd = Path::new(&session.working_directory);
    let Some(lexical_replica) = lexical_cwd.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("__agent_"))
    }) else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled CWD is not inside a lexical room member replica".to_string(),
        );
    };
    let Some(lexical_workgroup) = lexical_replica.parent() else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled lexical replica has no room parent".to_string(),
        );
    };
    let Some(lexical_ac_root) = lexical_workgroup.parent() else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled lexical room has no Project AC Root parent".to_string(),
        );
    };
    for (path, label) in [
        (lexical_replica, "replica"),
        (lexical_workgroup, "room"),
        (lexical_ac_root, "Project AC Root"),
    ] {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return MemberPolicyResolution::PermanentIneligible(format!(
                    "sampled lexical {} '{}' is not readable: {}",
                    label,
                    path.display(),
                    error
                ))
            }
        };
        if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
            return MemberPolicyResolution::PermanentIneligible(format!(
                "sampled lexical {} '{}' must be a real non-link directory",
                label,
                path.display()
            ));
        }
    }
    let canonical_lexical_replica = match std::fs::canonicalize(lexical_replica)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
    {
        Ok(path) => path,
        Err(error) => {
            return MemberPolicyResolution::PermanentIneligible(format!(
                "sampled lexical replica '{}' cannot be canonicalized: {}",
                lexical_replica.display(),
                error
            ))
        }
    };

    let canonical_cwd = match std::fs::canonicalize(&session.working_directory)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
    {
        Ok(path) => path,
        Err(error) => {
            return MemberPolicyResolution::PermanentIneligible(format!(
                "sampled CWD '{}' cannot be canonicalized: {}",
                session.working_directory, error
            ))
        }
    };
    let Some(replica_candidate) = canonical_cwd.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("__agent_"))
    }) else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled CWD is not inside a room member replica".to_string(),
        );
    };
    let replica_dir = match canonical_real_directory(replica_candidate, "Member replica") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    if canonical_lexical_replica != replica_dir {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled CWD reaches a different replica through a link or path escape".to_string(),
        );
    }
    if canonical_cwd != replica_dir && !canonical_cwd.starts_with(&replica_dir) {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled CWD escapes its member replica".to_string(),
        );
    }
    let layout = match crate::config::ac_root::wg_replica_layout_from_agent_dir(&replica_dir) {
        Ok(Some(layout)) => layout,
        Ok(None) => {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled replica does not have the canonical room layout".to_string(),
            )
        }
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let workgroup_dir = match canonical_real_directory(&layout.wg_dir, "Room") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let ac_root = match canonical_real_directory(&layout.ac_root, "Project AC Root") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let team = match parse_team_from_workgroup_name(&layout.wg_name) {
        Ok(team) => team,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let (_, identity) =
        match crate::config::replica_identity::read_wg_replica_config_read_only(&replica_dir) {
            Ok(result) => result,
            Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
        };
    let identity_ac_root = match canonical_real_directory(&identity.ac_root, "Identity workspace") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let matrix_dir = match canonical_real_directory(&identity.matrix_dir, "Agent Matrix") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    if identity.agent_name != layout.agent_name || identity_ac_root != ac_root {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled replica identity does not match its canonical layout".to_string(),
        );
    }
    let project_dir = match canonical_real_directory(&layout.project_dir, "Project") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let project = match validated_project_fqn_component(
        project_dir.file_name().and_then(|name| name.to_str()),
    ) {
        Ok(project) => project,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let agent_id = match session.agent_id.clone() {
        Some(agent_id) if !agent_id.is_empty() => agent_id,
        _ => {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled session has no coding-agent id".to_string(),
            )
        }
    };
    let fingerprint = MemberIdentityFingerprint {
        session_id: session.id,
        agent_id,
        working_directory: session.working_directory.clone(),
        project_dir,
        ac_root: ac_root.clone(),
        workgroup_dir,
        replica_dir,
        matrix_dir,
        project,
        team: team.clone(),
        workgroup: layout.wg_name,
        member: layout.agent_name,
    };

    match read_team_config_classified(&ac_root, &team) {
        Ok(config) => {
            let member_ref = format!("_agent_{}", fingerprint.member);
            if !config.agents.contains(&member_ref) {
                return MemberPolicyResolution::PermanentIneligible(
                    "sampled member is no longer in the canonical team roster".to_string(),
                );
            }
            if config.context_alert_percentages.is_empty() {
                MemberPolicyResolution::Disabled(fingerprint)
            } else {
                MemberPolicyResolution::Eligible(ResolvedPolicy {
                    fingerprint,
                    thresholds: config.context_alert_percentages,
                })
            }
        }
        Err(error @ TeamConfigReadError::NotFound { .. }) => {
            MemberPolicyResolution::PermanentIneligible(error.to_string())
        }
        Err(error) => MemberPolicyResolution::Paused {
            fingerprint,
            class: error.class(),
            message: error.to_string(),
        },
    }
}

fn validated_project_fqn_component(component: Option<&str>) -> Result<String, String> {
    match component {
        Some(project)
            if !project.is_empty()
                && !project.contains(':')
                && !project
                    .chars()
                    .any(|character| character == '\0' || character.is_ascii_control()) =>
        {
            Ok(project.to_string())
        }
        _ => Err("project directory has an invalid qualified-name component".to_string()),
    }
}

fn prepare_internal_route_blocking(
    fingerprint: &MemberIdentityFingerprint,
    observed: u8,
    thresholds: &[u8],
) -> Result<(InternalSystemTarget, InternalSystemNotice), String> {
    let config = read_team_config_classified(&fingerprint.ac_root, &fingerprint.team)
        .map_err(|error| error.to_string())?;
    let coordinator =
        crate::config::replica_identity::agent_bare_name_from_ref(&config.coordinator)?;
    let expected = fingerprint
        .workgroup_dir
        .join(format!("__agent_{}", coordinator));
    let canonical_expected = canonical_real_directory(&expected, "Orchestrator replica")?;
    if canonical_expected.parent() != Some(fingerprint.workgroup_dir.as_path())
        || canonical_expected
            .file_name()
            .and_then(|name| name.to_str())
            != Some(format!("__agent_{}", coordinator).as_str())
    {
        return Err("Orchestrator replica escapes the sampled room".to_string());
    }
    let resolved = crate::config::teams::resolve_wg_coordinator_replica(
        &fingerprint.ac_root,
        &fingerprint.workgroup_dir,
    )
    .ok_or_else(|| "Current orchestrator replica is unavailable".to_string())?;
    let canonical_resolved =
        canonical_real_directory(&resolved.replica_dir, "Resolved orchestrator replica")?;
    if resolved.project != fingerprint.project
        || resolved.team != fingerprint.team
        || resolved.wg_name != fingerprint.workgroup
        || resolved.agent_name != coordinator
        || canonical_resolved != canonical_expected
    {
        return Err(
            "Resolved orchestrator identity does not match the exact sampled room".to_string(),
        );
    }
    let fqn = format!(
        "{}:{}/{}",
        resolved.project, resolved.wg_name, resolved.agent_name
    );
    let target = InternalSystemTarget::for_context_alert(fqn, canonical_resolved)?;
    let notice = InternalSystemNotice::for_context_alert(
        fingerprint.member.clone(),
        fingerprint.workgroup.clone(),
        observed,
        thresholds.to_vec(),
    )?;
    Ok((target, notice))
}

fn validate_attempt_guard<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    fingerprint: &MemberIdentityFingerprint,
    target: &InternalSystemTarget,
    thresholds: &[u8],
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("Context alert attempt was canceled".to_string());
    }
    if app
        .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
        .is_some_and(|guard| guard.blocks_agent(target.fqn()))
    {
        return Err(format!("purge-room blocks '{}'", target.fqn()));
    }
    let scraper = app
        .try_state::<Arc<ContextScraper>>()
        .ok_or_else(|| "Context scraper is unavailable".to_string())?;
    if !scraper.is_session_registered(fingerprint.session_id) {
        return Err("Sampled session is no longer registered".to_string());
    }
    let replica = canonical_real_directory(&fingerprint.replica_dir, "Member replica")?;
    let workgroup = canonical_real_directory(&fingerprint.workgroup_dir, "Room")?;
    let ac_root = canonical_real_directory(&fingerprint.ac_root, "Project AC Root")?;
    let project = canonical_real_directory(&fingerprint.project_dir, "Project")?;
    if replica != fingerprint.replica_dir
        || workgroup != fingerprint.workgroup_dir
        || ac_root != fingerprint.ac_root
        || project != fingerprint.project_dir
        || replica.parent() != Some(workgroup.as_path())
        || workgroup.parent() != Some(ac_root.as_path())
        || ac_root.parent() != Some(project.as_path())
        || replica.file_name().and_then(|name| name.to_str())
            != Some(format!("__agent_{}", fingerprint.member).as_str())
        || parse_team_from_workgroup_name(&fingerprint.workgroup)? != fingerprint.team
    {
        return Err("Sampled canonical identity paths changed".to_string());
    }
    let (_, identity) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&replica)?;
    let identity_ac_root = canonical_real_directory(&identity.ac_root, "Identity workspace")?;
    let identity_matrix = canonical_real_directory(&identity.matrix_dir, "Agent Matrix")?;
    if identity.agent_name != fingerprint.member
        || identity_ac_root != fingerprint.ac_root
        || identity_matrix != fingerprint.matrix_dir
    {
        return Err("Sampled member identity changed".to_string());
    }
    let config = read_team_config_classified(&ac_root, &fingerprint.team)
        .map_err(|error| error.to_string())?;
    if !config
        .agents
        .contains(&format!("_agent_{}", fingerprint.member))
        || thresholds
            .iter()
            .any(|threshold| !config.context_alert_percentages.contains(threshold))
    {
        return Err("Sampled member roster or context alert policy changed".to_string());
    }
    let (fresh_target, _) = prepare_internal_route_blocking(fingerprint, 100, thresholds)?;
    if fresh_target != *target {
        return Err("Orchestrator target changed before notice delivery".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
