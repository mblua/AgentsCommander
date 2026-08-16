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
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemberIdentityFingerprint {
    session_id: Uuid,
    agent_id: String,
    working_directory: String,
    project_dir: PathBuf,
    workspace_dir: PathBuf,
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
    fn thresholds(&self) -> &[u8] {
        self.notice.thresholds()
    }
}

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
                current.thresholds == attempt.thresholds()
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
                    "Coordinator route blocking task failed: {}",
                    error
                ))
            }
        };
        let guard_app = self.app.clone();
        let guard_fingerprint = snapshot.fingerprint.clone();
        let guard_target = target.clone();
        let guard_thresholds = notice.thresholds().to_vec();
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
            "sampled CWD is not inside a lexical workgroup member replica".to_string(),
        );
    };
    let Some(lexical_workgroup) = lexical_replica.parent() else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled lexical replica has no workgroup parent".to_string(),
        );
    };
    let Some(lexical_workspace) = lexical_workgroup.parent() else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled lexical workgroup has no Project AC Root parent".to_string(),
        );
    };
    for (path, label) in [
        (lexical_replica, "replica"),
        (lexical_workgroup, "workgroup"),
        (lexical_workspace, "Project AC Root"),
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
            "sampled CWD is not inside a workgroup member replica".to_string(),
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
    let layout = match crate::config::workspace::wg_replica_layout_from_agent_dir(&replica_dir) {
        Ok(Some(layout)) => layout,
        Ok(None) => {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled replica does not have the canonical workgroup layout".to_string(),
            )
        }
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let workgroup_dir = match canonical_real_directory(&layout.wg_dir, "Workgroup") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let workspace_dir = match canonical_real_directory(&layout.workspace_dir, "Project AC Root") {
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
    let identity_workspace =
        match canonical_real_directory(&identity.workspace_dir, "Identity workspace") {
            Ok(path) => path,
            Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
        };
    let matrix_dir = match canonical_real_directory(&identity.matrix_dir, "Agent Matrix") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    if identity.agent_name != layout.agent_name || identity_workspace != workspace_dir {
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
        workspace_dir: workspace_dir.clone(),
        workgroup_dir,
        replica_dir,
        matrix_dir,
        project,
        team: team.clone(),
        workgroup: layout.wg_name,
        member: layout.agent_name,
    };

    match read_team_config_classified(&workspace_dir, &team) {
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
    let config = read_team_config_classified(&fingerprint.workspace_dir, &fingerprint.team)
        .map_err(|error| error.to_string())?;
    let coordinator =
        crate::config::replica_identity::agent_bare_name_from_ref(&config.coordinator)?;
    let expected = fingerprint
        .workgroup_dir
        .join(format!("__agent_{}", coordinator));
    let canonical_expected = canonical_real_directory(&expected, "Coordinator replica")?;
    if canonical_expected.parent() != Some(fingerprint.workgroup_dir.as_path())
        || canonical_expected
            .file_name()
            .and_then(|name| name.to_str())
            != Some(format!("__agent_{}", coordinator).as_str())
    {
        return Err("Coordinator replica escapes the sampled workgroup".to_string());
    }
    let resolved = crate::config::teams::resolve_wg_coordinator_replica(
        &fingerprint.workspace_dir,
        &fingerprint.workgroup_dir,
    )
    .ok_or_else(|| "Current coordinator replica is unavailable".to_string())?;
    let canonical_resolved =
        canonical_real_directory(&resolved.replica_dir, "Resolved coordinator replica")?;
    if resolved.project != fingerprint.project
        || resolved.team != fingerprint.team
        || resolved.wg_name != fingerprint.workgroup
        || resolved.agent_name != coordinator
        || canonical_resolved != canonical_expected
    {
        return Err(
            "Resolved coordinator identity does not match the exact sampled workgroup".to_string(),
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
        return Err(format!("purge-wg blocks '{}'", target.fqn()));
    }
    let scraper = app
        .try_state::<Arc<ContextScraper>>()
        .ok_or_else(|| "Context scraper is unavailable".to_string())?;
    if !scraper.is_session_registered(fingerprint.session_id) {
        return Err("Sampled session is no longer registered".to_string());
    }
    let replica = canonical_real_directory(&fingerprint.replica_dir, "Member replica")?;
    let workgroup = canonical_real_directory(&fingerprint.workgroup_dir, "Workgroup")?;
    let workspace = canonical_real_directory(&fingerprint.workspace_dir, "Project AC Root")?;
    let project = canonical_real_directory(&fingerprint.project_dir, "Project")?;
    if replica != fingerprint.replica_dir
        || workgroup != fingerprint.workgroup_dir
        || workspace != fingerprint.workspace_dir
        || project != fingerprint.project_dir
        || replica.parent() != Some(workgroup.as_path())
        || workgroup.parent() != Some(workspace.as_path())
        || workspace.parent() != Some(project.as_path())
        || replica.file_name().and_then(|name| name.to_str())
            != Some(format!("__agent_{}", fingerprint.member).as_str())
        || parse_team_from_workgroup_name(&fingerprint.workgroup)? != fingerprint.team
    {
        return Err("Sampled canonical identity paths changed".to_string());
    }
    let (_, identity) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&replica)?;
    let identity_workspace =
        canonical_real_directory(&identity.workspace_dir, "Identity workspace")?;
    let identity_matrix = canonical_real_directory(&identity.matrix_dir, "Agent Matrix")?;
    if identity.agent_name != fingerprint.member
        || identity_workspace != fingerprint.workspace_dir
        || identity_matrix != fingerprint.matrix_dir
    {
        return Err("Sampled member identity changed".to_string());
    }
    let config = read_team_config_classified(&workspace, &fingerprint.team)
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
        return Err("Coordinator target changed before notice delivery".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct GuardRows;

    impl crate::pty::context_scrape::ScreenRowsSource for GuardRows {
        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::Unavailable
        }
    }

    struct GuardPatterns;

    impl crate::pty::context_scrape::ContextPatternSource for GuardPatterns {
        fn patterns(&self) -> BoxFuture<'_, HashMap<String, String>> {
            Box::pin(async { HashMap::new() })
        }
    }

    struct GuardEvents;

    impl crate::pty::context_scrape::ContextEventSink for GuardEvents {
        fn emit(&self, _payload: crate::pty::context_scrape::ContextUsagePayload) {}
    }

    struct GuardSamples;

    impl crate::pty::context_scrape::ContextSampleSink for GuardSamples {
        fn observe(&self, _sample: ContextSample) {}
    }

    struct GuardPersist;

    impl crate::pty::context_scrape::ContextPersistSink for GuardPersist {
        fn commit(&self, _changed: Vec<(Uuid, Option<u8>)>) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    struct ManualClock {
        now: Mutex<Instant>,
        waiters: Mutex<Vec<(Instant, tokio::sync::oneshot::Sender<()>)>>,
        waiters_changed: tokio::sync::Notify,
    }

    impl ManualClock {
        fn new(now: Instant) -> Arc<Self> {
            Arc::new(Self {
                now: Mutex::new(now),
                waiters: Mutex::new(Vec::new()),
                waiters_changed: tokio::sync::Notify::new(),
            })
        }

        fn advance(&self, duration: Duration) {
            let now = {
                let mut now = self.now.lock().unwrap();
                *now += duration;
                *now
            };
            let mut waiters = self.waiters.lock().unwrap();
            let mut pending = Vec::new();
            for (deadline, sender) in waiters.drain(..) {
                if deadline <= now {
                    let _ = sender.send(());
                } else {
                    pending.push((deadline, sender));
                }
            }
            *waiters = pending;
        }

        fn has_live_waiter_within(&self, duration: Duration) -> bool {
            let latest = *self.now.lock().unwrap() + duration;
            self.waiters
                .lock()
                .unwrap()
                .iter()
                .any(|(deadline, sender)| !sender.is_closed() && *deadline <= latest)
        }

        async fn wait_for_live_waiter_within(&self, duration: Duration) {
            wait_until_notified(
                &self.waiters_changed,
                "manual clock waiter",
                || self.has_live_waiter_within(duration),
                || {
                    let live_waiters = self
                        .waiters
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|(_, sender)| !sender.is_closed())
                        .count();
                    format!("live_waiters={live_waiters} window={duration:?}")
                },
            )
            .await;
        }
    }

    impl AlertClock for ManualClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }

        fn sleep_until(&self, deadline: Instant) -> BoxFuture<'static, ()> {
            let now = *self.now.lock().unwrap();
            if deadline <= now {
                return Box::pin(async {});
            }
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.waiters.lock().unwrap().push((deadline, sender));
            self.waiters_changed.notify_one();
            Box::pin(async move {
                let _ = receiver.await;
            })
        }
    }

    struct ScriptedRuntime {
        _temp: tempfile::TempDir,
        target: InternalSystemTarget,
        policies: Mutex<HashMap<Uuid, ResolvedPolicy>>,
        live: AtomicBool,
        live_checks: AtomicUsize,
        resolve_calls: AtomicUsize,
        prepare_calls: Mutex<Vec<AttemptSnapshot>>,
        deliveries: Mutex<Vec<AttemptSnapshot>>,
        delivery_results: Mutex<VecDeque<Result<(), String>>>,
        blocked_delivery: Mutex<Option<tokio::sync::oneshot::Receiver<Result<(), String>>>>,
        blocked_resolution: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        blocked_live_check: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        cancel_during_live_check: Mutex<Option<CancellationToken>>,
        panic_next_delivery: AtomicBool,
        retired: Mutex<Vec<Uuid>>,
        changed: tokio::sync::Notify,
    }

    impl ScriptedRuntime {
        fn new() -> Arc<Self> {
            let temp = tempfile::tempdir().unwrap();
            let replica = temp
                .path()
                .join("project")
                .join(".ac")
                .join("wg-1-team")
                .join("__agent_coordinator");
            std::fs::create_dir_all(&replica).unwrap();
            let target = InternalSystemTarget::for_context_alert(
                "project:wg-1-team/coordinator".to_string(),
                replica,
            )
            .unwrap();
            Arc::new(Self {
                _temp: temp,
                target,
                policies: Mutex::new(HashMap::new()),
                live: AtomicBool::new(true),
                live_checks: AtomicUsize::new(0),
                resolve_calls: AtomicUsize::new(0),
                prepare_calls: Mutex::new(Vec::new()),
                deliveries: Mutex::new(Vec::new()),
                delivery_results: Mutex::new(VecDeque::new()),
                blocked_delivery: Mutex::new(None),
                blocked_resolution: Mutex::new(None),
                blocked_live_check: Mutex::new(None),
                cancel_during_live_check: Mutex::new(None),
                panic_next_delivery: AtomicBool::new(false),
                retired: Mutex::new(Vec::new()),
                changed: tokio::sync::Notify::new(),
            })
        }

        fn set_policy(&self, id: Uuid, thresholds: &[u8]) {
            self.policies.lock().unwrap().insert(
                id,
                ResolvedPolicy {
                    fingerprint: fingerprint(id),
                    thresholds: thresholds.to_vec(),
                },
            );
        }

        fn delivery_count(&self) -> usize {
            self.deliveries.lock().unwrap().len()
        }

        async fn wait_for_delivery_count(&self, expected: usize) {
            wait_until_notified(
                &self.changed,
                "scripted delivery count",
                || self.delivery_count() >= expected,
                || {
                    format!(
                        "expected_at_least={expected} actual={}",
                        self.delivery_count()
                    )
                },
            )
            .await;
        }

        async fn wait_for_resolve_calls(&self, expected: usize) {
            wait_until_notified(
                &self.changed,
                "scripted policy resolutions",
                || self.resolve_calls.load(Ordering::SeqCst) >= expected,
                || {
                    format!(
                        "expected_at_least={expected} actual={}",
                        self.resolve_calls.load(Ordering::SeqCst)
                    )
                },
            )
            .await;
        }

        async fn wait_for_live_checks(&self, expected: usize) {
            wait_until_notified(
                &self.changed,
                "scripted liveness checks",
                || self.live_checks.load(Ordering::SeqCst) >= expected,
                || {
                    format!(
                        "expected_at_least={expected} actual={}",
                        self.live_checks.load(Ordering::SeqCst)
                    )
                },
            )
            .await;
        }

        async fn wait_for_retirement(&self, session_id: Uuid) {
            wait_until_notified(
                &self.changed,
                "scripted sample retirement",
                || self.retired.lock().unwrap().contains(&session_id),
                || {
                    format!(
                        "session_id={session_id} retired={:?}",
                        self.retired.lock().unwrap().as_slice()
                    )
                },
            )
            .await;
        }

        fn block_next_delivery(&self) -> tokio::sync::oneshot::Sender<Result<(), String>> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            *self.blocked_delivery.lock().unwrap() = Some(receiver);
            sender
        }

        fn block_next_resolution(&self) -> tokio::sync::oneshot::Sender<()> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            *self.blocked_resolution.lock().unwrap() = Some(receiver);
            sender
        }

        fn block_next_live_check(&self) -> tokio::sync::oneshot::Sender<()> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            *self.blocked_live_check.lock().unwrap() = Some(receiver);
            sender
        }
    }

    impl ContextAlertRuntime for ScriptedRuntime {
        fn resolve_member_policy(
            &self,
            session_id: Uuid,
        ) -> BoxFuture<'static, MemberPolicyResolution> {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            self.changed.notify_one();
            let resolution = self
                .policies
                .lock()
                .unwrap()
                .get(&session_id)
                .cloned()
                .map(MemberPolicyResolution::Eligible)
                .unwrap_or_else(|| {
                    MemberPolicyResolution::PermanentIneligible("missing policy".to_string())
                });
            let blocked = self.blocked_resolution.lock().unwrap().take();
            Box::pin(async move {
                if let Some(blocked) = blocked {
                    let _ = blocked.await;
                }
                resolution
            })
        }

        fn check_session_live(&self, _session_id: Uuid) -> BoxFuture<'static, bool> {
            self.live_checks.fetch_add(1, Ordering::SeqCst);
            self.changed.notify_one();
            let live = self.live.load(Ordering::SeqCst);
            let blocked = self.blocked_live_check.lock().unwrap().take();
            let cancel = self.cancel_during_live_check.lock().unwrap().take();
            Box::pin(async move {
                if let Some(blocked) = blocked {
                    let _ = blocked.await;
                }
                if let Some(cancel) = cancel {
                    cancel.cancel();
                }
                live
            })
        }

        fn retire_sample_registration(&self, session_id: Uuid) {
            self.retired.lock().unwrap().push(session_id);
            self.changed.notify_one();
        }

        fn prepare_attempt(
            &self,
            snapshot: AttemptSnapshot,
            cancellation: CancellationToken,
        ) -> BoxFuture<'static, AttemptPreparation> {
            self.prepare_calls.lock().unwrap().push(snapshot.clone());
            let policy = self
                .policies
                .lock()
                .unwrap()
                .get(&snapshot.session_id)
                .cloned();
            let target = self.target.clone();
            Box::pin(async move {
                let Some(policy) = policy else {
                    return AttemptPreparation::Cancel {
                        reason: "missing policy".to_string(),
                        current_policy: None,
                    };
                };
                let thresholds: Vec<u8> = snapshot
                    .thresholds
                    .iter()
                    .copied()
                    .filter(|threshold| policy.thresholds.contains(threshold))
                    .collect();
                if thresholds.is_empty() {
                    return AttemptPreparation::Cancel {
                        reason: "removed".to_string(),
                        current_policy: Some(policy),
                    };
                }
                let notice = InternalSystemNotice::for_context_alert(
                    snapshot.fingerprint.member.clone(),
                    snapshot.fingerprint.workgroup.clone(),
                    snapshot.observed,
                    thresholds,
                )
                .unwrap();
                AttemptPreparation::Ready(ReadyAttempt {
                    snapshot,
                    current_policy: policy.thresholds,
                    target,
                    notice,
                    guard: Arc::new(|| Ok(())),
                    cancellation,
                })
            })
        }

        fn deliver_attempt(&self, attempt: ReadyAttempt) -> BoxFuture<'static, Result<(), String>> {
            self.deliveries
                .lock()
                .unwrap()
                .push(attempt.snapshot.clone());
            self.changed.notify_one();
            if self.panic_next_delivery.swap(false, Ordering::SeqCst) {
                return Box::pin(async move {
                    panic!("scripted delivery panic");
                });
            }
            if let Some(receiver) = self.blocked_delivery.lock().unwrap().take() {
                return Box::pin(async move {
                    receiver
                        .await
                        .unwrap_or_else(|_| Err("blocked delivery sender dropped".to_string()))
                });
            }
            let result = self
                .delivery_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()));
            Box::pin(async move { result })
        }
    }

    async fn wait_until_notified(
        notify: &tokio::sync::Notify,
        label: &str,
        mut predicate: impl FnMut() -> bool,
        diagnostics: impl Fn() -> String,
    ) {
        let wait = async {
            loop {
                let notified = notify.notified();
                if predicate() {
                    return;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(60), wait)
            .await
            .is_err()
        {
            panic!("timed out waiting for {label}: {}", diagnostics());
        }
    }

    struct IdentityFixture {
        _temp: tempfile::TempDir,
        session: Session,
        team_config: PathBuf,
        workspace: PathBuf,
        workgroup: PathBuf,
        replica: PathBuf,
        member_matrix: PathBuf,
    }

    async fn identity_fixture() -> IdentityFixture {
        identity_fixture_named("project-a", "wg-2-dev-team").await
    }

    async fn identity_fixture_named(project: &str, workgroup_name: &str) -> IdentityFixture {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join(project).join(".ac");
        let workgroup = workspace.join(workgroup_name);
        let replica = workgroup.join("__agent_member");
        let coordinator_replica = workgroup.join("__agent_coordinator");
        let nested = replica.join("repo-one").join("src");
        let member_matrix = workspace.join("_agent_member");
        let coordinator_matrix = workspace.join("_agent_coordinator");
        let team = parse_team_from_workgroup_name(workgroup_name).unwrap();
        let team_dir = workspace.join(format!("_team_{}", team));
        for path in [
            &nested,
            &coordinator_replica,
            &member_matrix,
            &coordinator_matrix,
            &team_dir,
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_member","context":[],"repos":[]}"#,
        )
        .unwrap();
        std::fs::write(
            coordinator_replica.join("config.json"),
            r#"{"identity":"../../_agent_coordinator","context":[],"repos":[]}"#,
        )
        .unwrap();
        let team_config = team_dir.join("config.json");
        std::fs::write(
            &team_config,
            r#"{"agents":["_agent_member","_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[50,75]}"#,
        )
        .unwrap();
        let manager = SessionManager::new();
        let session = manager
            .create_session(
                "claude".to_string(),
                Vec::new(),
                nested.to_string_lossy().to_string(),
                Some("claude-profile".to_string()),
                Some("Claude".to_string()),
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        IdentityFixture {
            _temp: temp,
            session,
            team_config,
            workspace,
            workgroup,
            replica,
            member_matrix,
        }
    }

    fn fingerprint(id: Uuid) -> MemberIdentityFingerprint {
        MemberIdentityFingerprint {
            session_id: id,
            agent_id: "claude".to_string(),
            working_directory: "cwd".to_string(),
            project_dir: PathBuf::from("project"),
            workspace_dir: PathBuf::from("workspace"),
            workgroup_dir: PathBuf::from("workgroup"),
            replica_dir: PathBuf::from("replica"),
            matrix_dir: PathBuf::from("matrix"),
            project: "project".to_string(),
            team: "team".to_string(),
            workgroup: "wg-1-team".to_string(),
            member: "member".to_string(),
        }
    }

    fn eligible(id: Uuid, thresholds: &[u8]) -> MemberPolicyResolution {
        MemberPolicyResolution::Eligible(ResolvedPolicy {
            fingerprint: fingerprint(id),
            thresholds: thresholds.to_vec(),
        })
    }

    #[test]
    fn first_high_coalesces_sorted_thresholds_and_unchanged_high_deduplicates() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75, 90]), now);
        assert_eq!(state.batches.len(), 1);
        let batch = state.batches.values().next().unwrap();
        assert_eq!(batch.observed, 80);
        assert_eq!(batch.thresholds, vec![50, 75]);

        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75, 90]), now);
        assert_eq!(state.batches.len(), 1);
    }

    #[test]
    fn first_below_exact_equality_and_successive_crossings_are_deterministic() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 49, eligible(id, &[50, 75]), now);
        assert!(state.batches.is_empty());

        apply_numeric_resolution(&mut state, None, id, 50, eligible(id, &[50, 75]), now);
        apply_numeric_resolution(&mut state, None, id, 75, eligible(id, &[50, 75]), now);
        let batches: Vec<&AlertBatch> = state.batches.values().collect();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].thresholds, vec![50]);
        assert_eq!(batches[0].observed, 50);
        assert_eq!(batches[1].thresholds, vec![75]);
        assert_eq!(batches[1].observed, 75);
        assert!(batches[0].generation < batches[1].generation);
    }

    #[test]
    fn policy_removal_prunes_pending_and_readd_starts_armed() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75]), now);
        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[75, 90]), now);
        assert_eq!(state.batches.values().next().unwrap().thresholds, vec![75]);
        assert_eq!(state.sessions[&id].latches[&90], LatchState::Armed);

        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[90]), now);
        assert!(state.batches.is_empty());
        apply_numeric_resolution(&mut state, None, id, 95, eligible(id, &[90]), now);
        assert_eq!(state.batches.values().next().unwrap().thresholds, vec![90]);
    }

    #[test]
    fn successful_notice_keeps_current_latch_and_allows_a_later_rearm() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
        let generation = *state.batches.keys().next().unwrap();
        state.remove_batch(generation);

        apply_numeric_resolution(&mut state, None, id, 40, eligible(id, &[50]), now);
        apply_numeric_resolution(&mut state, None, id, 55, eligible(id, &[50]), now);
        assert_eq!(state.batches.len(), 1);
        assert_ne!(*state.batches.keys().next().unwrap(), generation);
    }

    #[test]
    fn partial_rearm_and_outstanding_dedup_are_threshold_scoped() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &[50, 75, 90]), now);
        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75, 90]), now);
        let session = state.sessions.get(&id).unwrap();
        assert_eq!(session.latches[&50], LatchState::Latched);
        assert_eq!(session.latches[&75], LatchState::Latched);
        assert_eq!(session.latches[&90], LatchState::Armed);

        apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &[50, 75, 90]), now);
        assert_eq!(
            state.batches.len(),
            1,
            "90 remains outstanding in first batch"
        );
    }

    #[test]
    fn retry_schedule_caps_at_sixty_seconds_on_the_same_logical_batch() {
        let expected = [5, 10, 20, 40, 60, 60];
        for (failure_count, seconds) in expected.into_iter().enumerate() {
            assert_eq!(
                retry_delay((failure_count + 1) as u32),
                Duration::from_secs(seconds)
            );
        }
        assert_eq!(retry_delay(99), Duration::from_secs(60));

        let id = Uuid::new_v4();
        let mut now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
        let generation = *state.batches.keys().next().unwrap();
        for seconds in expected {
            fail_batch(&mut state, generation, now, "scripted failure");
            now += Duration::from_secs(seconds);
            assert_eq!(state.batches[&generation].due_at, now);
        }
        assert_eq!(state.batches[&generation].failure_count, 6);
        assert_eq!(state.sessions[&id].outstanding[&50], generation);
    }

    #[test]
    fn partial_rearm_at_eighty_sixty_and_zero_is_exact() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        let policy = [50, 75, 90];

        apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &policy), now);
        let first = *state.batches.keys().next().unwrap();
        state.remove_batch(first);

        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &policy), now);
        assert_eq!(state.sessions[&id].latches[&50], LatchState::Latched);
        assert_eq!(state.sessions[&id].latches[&75], LatchState::Latched);
        assert_eq!(state.sessions[&id].latches[&90], LatchState::Armed);
        apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &policy), now);
        assert_eq!(state.batches.values().next().unwrap().thresholds, vec![90]);
        let second = *state.batches.keys().next().unwrap();
        state.remove_batch(second);

        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &policy), now);
        assert_eq!(state.sessions[&id].latches[&50], LatchState::Latched);
        assert_eq!(state.sessions[&id].latches[&75], LatchState::Armed);
        assert_eq!(state.sessions[&id].latches[&90], LatchState::Armed);
        apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &policy), now);
        assert_eq!(
            state.batches.values().next().unwrap().thresholds,
            vec![75, 90]
        );
        let third = *state.batches.keys().next().unwrap();
        state.remove_batch(third);

        apply_numeric_resolution(&mut state, None, id, 0, eligible(id, &policy), now);
        assert!(state.sessions[&id]
            .latches
            .values()
            .all(|latch| *latch == LatchState::Armed));
        apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &policy), now);
        assert_eq!(
            state.batches.values().next().unwrap().thresholds,
            vec![50, 75, 90]
        );
    }

    #[test]
    fn decrease_while_pending_rearms_without_replacing_historical_batch() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
        let generation = *state.batches.keys().next().unwrap();

        apply_numeric_resolution(&mut state, None, id, 40, eligible(id, &[50]), now);
        assert_eq!(state.sessions[&id].latches[&50], LatchState::Armed);
        assert_eq!(state.batches[&generation].observed, 60);
        apply_numeric_resolution(&mut state, None, id, 55, eligible(id, &[50]), now);

        assert_eq!(state.sessions[&id].latches[&50], LatchState::Latched);
        assert_eq!(state.batches.len(), 1);
        assert_eq!(state.sessions[&id].outstanding[&50], generation);
    }

    #[tokio::test]
    async fn in_flight_policy_pruning_cancels_and_replaces_only_survivors() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 90, eligible(id, &[50, 75]), now);
        let original = *state.batches.keys().next().unwrap();
        state.batches.get_mut(&original).unwrap().failure_count = 3;
        state.batches.get_mut(&original).unwrap().in_flight = true;
        let cancellation = CancellationToken::new();
        let slot = InFlightSlot {
            generation: original,
            cancellation: cancellation.clone(),
            join: tauri::async_runtime::spawn(async {}),
        };

        reconcile_policy(&mut state, Some(&slot), id, &fingerprint(id), &[75], now);

        assert!(cancellation.is_cancelled());
        assert!(!state.batches.contains_key(&original));
        let replacement = state.batches.values().next().unwrap();
        assert_ne!(replacement.generation, original);
        assert_eq!(replacement.thresholds, vec![75]);
        assert_eq!(replacement.observed, 90);
        assert_eq!(replacement.failure_count, 3);
        assert_eq!(replacement.due_at, now);
        assert_eq!(state.sessions[&id].outstanding[&75], replacement.generation);
        slot.join.await.unwrap();
    }

    #[tokio::test]
    async fn exhausted_generation_fails_closed_for_in_flight_replacement() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 90, eligible(id, &[50, 75]), now);
        let generation = *state.batches.keys().next().unwrap();
        state.batches.get_mut(&generation).unwrap().in_flight = true;
        state.next_generation = None;
        let cancellation = CancellationToken::new();
        let slot = InFlightSlot {
            generation,
            cancellation: cancellation.clone(),
            join: tauri::async_runtime::spawn(async {}),
        };

        reconcile_policy(&mut state, Some(&slot), id, &fingerprint(id), &[75], now);

        assert!(cancellation.is_cancelled());
        assert!(state.batches.is_empty());
        assert!(state.sessions[&id].outstanding.is_empty());
        slot.join.await.unwrap();
    }

    #[tokio::test]
    async fn stale_matching_completion_clears_slot_without_mutating_replacement() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 90, eligible(id, &[50, 75]), now);
        let original = *state.batches.keys().next().unwrap();
        state.batches.get_mut(&original).unwrap().in_flight = true;
        let cancellation = CancellationToken::new();
        let mut in_flight = Some(InFlightSlot {
            generation: original,
            cancellation: cancellation.clone(),
            join: tauri::async_runtime::spawn(async {}),
        });
        reconcile_policy(
            &mut state,
            in_flight.as_ref(),
            id,
            &fingerprint(id),
            &[75],
            now,
        );
        let replacement = *state.batches.keys().next().unwrap();
        assert_ne!(replacement, original);

        handle_delivery_completion(
            &mut state,
            &mut in_flight,
            DeliveryCompletion {
                generation: original,
                result: Ok(()),
            },
            now,
        )
        .await;

        assert!(in_flight.is_none());
        assert!(cancellation.is_cancelled());
        assert!(state.batches.contains_key(&replacement));
        assert_eq!(state.batches[&replacement].thresholds, vec![75]);
    }

    #[tokio::test]
    async fn explicit_policy_disable_cancels_in_flight_and_removes_all_state() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
        let generation = *state.batches.keys().next().unwrap();
        state.batches.get_mut(&generation).unwrap().in_flight = true;
        let cancellation = CancellationToken::new();
        let slot = InFlightSlot {
            generation,
            cancellation: cancellation.clone(),
            join: tauri::async_runtime::spawn(async {}),
        };

        apply_numeric_resolution(
            &mut state,
            Some(&slot),
            id,
            60,
            MemberPolicyResolution::Disabled(fingerprint(id)),
            now,
        );

        assert!(cancellation.is_cancelled());
        assert!(!state.sessions.contains_key(&id));
        assert!(state.batches.is_empty());
        slot.join.await.unwrap();
    }

    #[test]
    fn disabled_numeric_resolution_logs_policy_recovery_once_before_removing_state() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();

        let first = apply_numeric_resolution(
            &mut state,
            None,
            id,
            80,
            MemberPolicyResolution::Paused {
                fingerprint: fingerprint(id),
                class: "malformed",
                message: "partial JSON".to_string(),
            },
            now,
        );
        assert_eq!(first, ResolutionRecovery::default());
        assert!(state.sessions[&id].policy_error.is_some());

        let recovered = apply_numeric_resolution(
            &mut state,
            None,
            id,
            80,
            MemberPolicyResolution::Disabled(fingerprint(id)),
            now,
        );
        assert_eq!(
            recovered,
            ResolutionRecovery {
                policy: true,
                runtime: false,
            },
            "the true policy transition is the single info recovery log"
        );
        assert!(!state.sessions.contains_key(&id));

        let repeated = apply_numeric_resolution(
            &mut state,
            None,
            id,
            80,
            MemberPolicyResolution::Disabled(fingerprint(id)),
            now,
        );
        assert_eq!(
            repeated,
            ResolutionRecovery::default(),
            "a later disabled sample has no stale marker to log again"
        );
    }

    #[tokio::test]
    async fn disabled_retry_cancellation_logs_policy_recovery_once_and_drops_state() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50]), now);
        apply_numeric_resolution(
            &mut state,
            None,
            id,
            80,
            MemberPolicyResolution::Paused {
                fingerprint: fingerprint(id),
                class: "unreadable",
                message: "config read failed".to_string(),
            },
            now,
        );
        assert!(state.sessions[&id].policy_error.is_some());
        assert_eq!(state.batches.len(), 1);

        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[]);
        let runtime_trait: Arc<dyn ContextAlertRuntime> = runtime.clone();
        let clock: Arc<dyn AlertClock> = ManualClock::new(now);
        let (completion_tx, _completion_rx) = mpsc::channel(1);
        let shutdown = CancellationToken::new();
        let mut in_flight = None;

        let recovered = dispatch_due_batch(
            &runtime_trait,
            &clock,
            &mut state,
            &mut in_flight,
            &completion_tx,
            &shutdown,
        )
        .await;
        assert_eq!(
            recovered,
            ResolutionRecovery {
                policy: true,
                runtime: false,
            },
            "disabled retry preparation clears and logs the policy marker once"
        );
        assert!(!state.sessions.contains_key(&id));
        assert!(state.batches.is_empty());
        assert_eq!(runtime.prepare_calls.lock().unwrap().len(), 1);

        let repeated = dispatch_due_batch(
            &runtime_trait,
            &clock,
            &mut state,
            &mut in_flight,
            &completion_tx,
            &shutdown,
        )
        .await;
        assert_eq!(repeated, ResolutionRecovery::default());
        assert_eq!(
            runtime.prepare_calls.lock().unwrap().len(),
            1,
            "removed disabled state cannot emit a duplicate recovery log"
        );
    }

    #[test]
    fn retry_preparation_policy_addition_arms_but_does_not_evaluate_cached_usage() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[90]), now);
        assert!(state.batches.is_empty());
        assert_eq!(state.sessions[&id].last_valid_percent, Some(80));

        reconcile_policy(&mut state, None, id, &fingerprint(id), &[50, 90], now);
        assert!(state.batches.is_empty());
        assert_eq!(state.sessions[&id].latches[&50], LatchState::Armed);

        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 90]), now);
        assert_eq!(state.batches.values().next().unwrap().thresholds, vec![50]);
    }

    #[test]
    fn policy_pause_and_runtime_error_transitions_preserve_only_matching_identity() {
        let id = Uuid::new_v4();
        let first_error_id = Uuid::new_v4();
        let first_pause_id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
        let generation = *state.batches.keys().next().unwrap();

        let paused = MemberPolicyResolution::Paused {
            fingerprint: fingerprint(id),
            class: "malformed",
            message: "partial JSON".to_string(),
        };
        apply_numeric_resolution(&mut state, None, id, 70, paused.clone(), now);
        let marker = state.sessions[&id].policy_error.clone();
        apply_numeric_resolution(&mut state, None, id, 70, paused, now);
        assert_eq!(state.sessions[&id].policy_error, marker);
        assert!(state.batches.contains_key(&generation));
        assert_eq!(state.sessions[&id].last_valid_percent, Some(60));

        apply_numeric_resolution(&mut state, None, id, 70, eligible(id, &[50]), now);
        assert!(state.sessions[&id].policy_error.is_none());
        assert_eq!(state.batches.len(), 1, "repair must not duplicate a latch");

        apply_numeric_resolution(
            &mut state,
            None,
            first_pause_id,
            90,
            MemberPolicyResolution::Paused {
                fingerprint: fingerprint(first_pause_id),
                class: "malformed",
                message: "first partial read".to_string(),
            },
            now,
        );
        assert!(state.sessions[&first_pause_id].last_valid_percent.is_none());
        assert!(state
            .batches
            .values()
            .all(|batch| batch.session_id != first_pause_id));
        apply_numeric_resolution(
            &mut state,
            None,
            first_pause_id,
            90,
            eligible(first_pause_id, &[50]),
            now,
        );
        assert!(state
            .batches
            .values()
            .any(|batch| batch.session_id == first_pause_id));

        apply_numeric_resolution(
            &mut state,
            None,
            first_error_id,
            90,
            MemberPolicyResolution::RetryableFailure("blocking join failed".to_string()),
            now,
        );
        assert!(state.sessions[&first_error_id].fingerprint.is_none());
        assert!(state.sessions[&first_error_id].last_valid_percent.is_none());
        assert!(state
            .batches
            .values()
            .all(|batch| batch.session_id != first_error_id));
        apply_numeric_resolution(
            &mut state,
            None,
            first_error_id,
            90,
            eligible(first_error_id, &[50]),
            now,
        );
        assert!(state.sessions[&first_error_id].runtime_error.is_none());
        assert!(state
            .batches
            .values()
            .any(|batch| batch.session_id == first_error_id));

        let mut changed = fingerprint(id);
        changed.member = "replacement".to_string();
        apply_numeric_resolution(
            &mut state,
            None,
            id,
            70,
            MemberPolicyResolution::Paused {
                fingerprint: changed.clone(),
                class: "invalid",
                message: "changed identity".to_string(),
            },
            now,
        );
        assert_eq!(state.sessions[&id].fingerprint.as_ref(), Some(&changed));
        assert!(state.batches.values().all(|batch| batch.session_id != id));
        assert!(state.sessions[&id].last_valid_percent.is_none());
    }

    #[test]
    fn earliest_deadline_and_generation_tie_break_prevent_failed_batch_starvation() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50, 75]), now);
        apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75]), now);
        let generations: Vec<u64> = state.batches.keys().copied().collect();
        assert_eq!(state.earliest_due(), Some((now, generations[0])));

        fail_batch(&mut state, generations[0], now, "permanent first failure");
        assert_eq!(state.earliest_due(), Some((now, generations[1])));
        assert_eq!(state.batches[&generations[0]].failure_count, 1);
        assert_eq!(state.batches[&generations[1]].failure_count, 0);
    }

    #[test]
    fn exhausted_new_crossing_never_wraps_or_corrupts_existing_batch() {
        let existing_id = Uuid::new_v4();
        let exhausted_id = Uuid::new_v4();
        let now = Instant::now();
        let mut state = ActorState::new();
        apply_numeric_resolution(
            &mut state,
            None,
            existing_id,
            60,
            eligible(existing_id, &[50]),
            now,
        );
        let existing_generation = *state.batches.keys().next().unwrap();
        state.next_generation = None;
        apply_numeric_resolution(
            &mut state,
            None,
            exhausted_id,
            60,
            eligible(exhausted_id, &[50]),
            now,
        );

        assert_eq!(state.batches.len(), 1);
        assert!(state.batches.contains_key(&existing_generation));
        assert_eq!(
            state.sessions[&exhausted_id].latches[&50],
            LatchState::Latched
        );
        assert!(state.sessions[&exhausted_id].outstanding.is_empty());
    }

    #[tokio::test]
    async fn blocking_identity_resolution_classifies_valid_malformed_and_missing_policy() {
        let fixture = identity_fixture().await;
        let policy = match resolve_member_policy_blocking(&fixture.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected eligible resolution, got {other:?}"),
        };
        assert_eq!(policy.thresholds, vec![50, 75]);
        assert_eq!(policy.fingerprint.project, "project-a");
        assert_eq!(policy.fingerprint.workgroup, "wg-2-dev-team");
        assert_eq!(policy.fingerprint.member, "member");
        assert!(policy.fingerprint.replica_dir.is_absolute());

        let (target, notice) =
            prepare_internal_route_blocking(&policy.fingerprint, 80, &[50, 75]).unwrap();
        assert_eq!(target.fqn(), "project-a:wg-2-dev-team/coordinator");
        assert_eq!(notice.thresholds(), &[50, 75]);

        std::fs::write(&fixture.team_config, b"{partial").unwrap();
        match resolve_member_policy_blocking(&fixture.session) {
            MemberPolicyResolution::Paused { class, .. } => assert_eq!(class, "malformed"),
            other => panic!("expected paused malformed resolution, got {other:?}"),
        }

        std::fs::remove_file(&fixture.team_config).unwrap();
        assert!(matches!(
            resolve_member_policy_blocking(&fixture.session),
            MemberPolicyResolution::PermanentIneligible(_)
        ));
    }

    #[tokio::test]
    async fn production_attempt_guard_rechecks_cancellation_purge_registration_policy_and_route() {
        let fixture = identity_fixture().await;
        let policy = match resolve_member_policy_blocking(&fixture.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected eligible resolution, got {other:?}"),
        };
        let (target, _) =
            prepare_internal_route_blocking(&policy.fingerprint, 80, &[50, 75]).unwrap();
        let scraper = ContextScraper::new(
            Arc::new(GuardRows),
            Arc::new(GuardPatterns),
            Arc::new(GuardEvents),
            Arc::new(GuardSamples),
            Arc::new(GuardPersist),
        );
        scraper.register_session(fixture.session.id, "claude".to_string());
        let purge = Arc::new(crate::session::purge_guard::PurgeGuard::default());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&scraper))
            .manage(Arc::clone(&purge))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let app_handle = app.handle().clone();
        let cancellation = CancellationToken::new();

        validate_attempt_guard(
            &app_handle,
            &policy.fingerprint,
            &target,
            &[50, 75],
            &cancellation,
        )
        .unwrap();

        let canceled = CancellationToken::new();
        canceled.cancel();
        assert!(validate_attempt_guard(
            &app_handle,
            &policy.fingerprint,
            &target,
            &[50, 75],
            &canceled,
        )
        .unwrap_err()
        .contains("canceled"));

        let purge_lease = purge
            .acquire(HashSet::new(), HashSet::from([target.fqn().to_string()]))
            .await;
        assert!(validate_attempt_guard(
            &app_handle,
            &policy.fingerprint,
            &target,
            &[50, 75],
            &cancellation,
        )
        .unwrap_err()
        .contains("purge-wg"));
        drop(purge_lease);

        std::fs::write(
            &fixture.team_config,
            r#"{"agents":["_agent_member","_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[75]}"#,
        )
        .unwrap();
        assert!(validate_attempt_guard(
            &app_handle,
            &policy.fingerprint,
            &target,
            &[50, 75],
            &cancellation,
        )
        .unwrap_err()
        .contains("policy changed"));

        std::fs::write(
            &fixture.team_config,
            r#"{"agents":["_agent_member","_agent_coordinator"],"coordinator":"_agent_member","repos":[],"contextAlertPercentages":[50,75]}"#,
        )
        .unwrap();
        assert!(validate_attempt_guard(
            &app_handle,
            &policy.fingerprint,
            &target,
            &[50, 75],
            &cancellation,
        )
        .unwrap_err()
        .contains("Coordinator target changed"));

        std::fs::write(
            &fixture.team_config,
            r#"{"agents":["_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[50,75]}"#,
        )
        .unwrap();
        assert!(validate_attempt_guard(
            &app_handle,
            &policy.fingerprint,
            &target,
            &[50, 75],
            &cancellation,
        )
        .unwrap_err()
        .contains("roster"));

        scraper.retire_session(fixture.session.id);
        assert!(validate_attempt_guard(
            &app_handle,
            &policy.fingerprint,
            &target,
            &[50, 75],
            &cancellation,
        )
        .unwrap_err()
        .contains("no longer registered"));
    }

    #[tokio::test]
    async fn identity_resolution_rejects_invalid_roster_matrix_and_cwd_shapes() {
        let roster = identity_fixture().await;
        std::fs::write(
            &roster.team_config,
            r#"{"agents":["_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[50]}"#,
        )
        .unwrap();
        assert!(matches!(
            resolve_member_policy_blocking(&roster.session),
            MemberPolicyResolution::PermanentIneligible(_)
        ));

        let missing_matrix = identity_fixture().await;
        std::fs::remove_dir_all(&missing_matrix.member_matrix).unwrap();
        assert!(matches!(
            resolve_member_policy_blocking(&missing_matrix.session),
            MemberPolicyResolution::PermanentIneligible(_)
        ));

        let ad_hoc = identity_fixture().await;
        let mut ad_hoc_session = ad_hoc.session.clone();
        ad_hoc_session.working_directory = ad_hoc.workspace.to_string_lossy().to_string();
        assert!(matches!(
            resolve_member_policy_blocking(&ad_hoc_session),
            MemberPolicyResolution::PermanentIneligible(_)
        ));

        let origin = identity_fixture().await;
        let mut origin_session = origin.session.clone();
        origin_session.working_directory = origin.member_matrix.to_string_lossy().to_string();
        assert!(matches!(
            resolve_member_policy_blocking(&origin_session),
            MemberPolicyResolution::PermanentIneligible(_)
        ));

        let lookalike = identity_fixture().await;
        let nested_lookalike = lookalike
            .replica
            .join("repo-one")
            .join("__agent_impostor")
            .join("src");
        std::fs::create_dir_all(&nested_lookalike).unwrap();
        let mut lookalike_session = lookalike.session.clone();
        lookalike_session.working_directory = nested_lookalike.to_string_lossy().to_string();
        assert!(matches!(
            resolve_member_policy_blocking(&lookalike_session),
            MemberPolicyResolution::PermanentIneligible(_)
        ));
    }

    #[tokio::test]
    async fn identity_resolution_distinguishes_unreadable_malformed_invalid_and_missing_config() {
        let unreadable = identity_fixture().await;
        std::fs::remove_file(&unreadable.team_config).unwrap();
        std::fs::create_dir(&unreadable.team_config).unwrap();
        match resolve_member_policy_blocking(&unreadable.session) {
            MemberPolicyResolution::Paused { class, .. } => assert_eq!(class, "unreadable"),
            other => panic!("expected unreadable pause, got {other:?}"),
        }

        let malformed = identity_fixture().await;
        std::fs::write(&malformed.team_config, b"{partial").unwrap();
        match resolve_member_policy_blocking(&malformed.session) {
            MemberPolicyResolution::Paused { class, .. } => assert_eq!(class, "malformed"),
            other => panic!("expected malformed pause, got {other:?}"),
        }

        let invalid = identity_fixture().await;
        std::fs::write(
            &invalid.team_config,
            r#"{"agents":["_agent_member","_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[0]}"#,
        )
        .unwrap();
        match resolve_member_policy_blocking(&invalid.session) {
            MemberPolicyResolution::Paused { class, .. } => assert_eq!(class, "invalid"),
            other => panic!("expected invalid pause, got {other:?}"),
        }

        let missing = identity_fixture().await;
        std::fs::remove_file(&missing.team_config).unwrap();
        assert!(matches!(
            resolve_member_policy_blocking(&missing.session),
            MemberPolicyResolution::PermanentIneligible(_)
        ));
    }

    #[tokio::test]
    async fn exact_workgroups_and_same_spelled_filesystem_roots_never_share_target_capability() {
        let first_workgroup = identity_fixture_named("same-project", "wg-1-dev-team").await;
        let second_workgroup = identity_fixture_named("same-project", "wg-2-dev-team").await;
        let first_policy = match resolve_member_policy_blocking(&first_workgroup.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected first policy, got {other:?}"),
        };
        let second_policy = match resolve_member_policy_blocking(&second_workgroup.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected second policy, got {other:?}"),
        };
        let (first_target, _) =
            prepare_internal_route_blocking(&first_policy.fingerprint, 80, &[50]).unwrap();
        let (second_target, _) =
            prepare_internal_route_blocking(&second_policy.fingerprint, 80, &[50]).unwrap();
        assert_eq!(first_target.fqn(), "same-project:wg-1-dev-team/coordinator");
        assert_eq!(
            second_target.fqn(),
            "same-project:wg-2-dev-team/coordinator"
        );
        assert_ne!(first_target, second_target);

        let other_project = identity_fixture_named("other-project", "wg-1-dev-team").await;
        let other_policy = match resolve_member_policy_blocking(&other_project.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected other-project policy, got {other:?}"),
        };
        let (other_target, _) =
            prepare_internal_route_blocking(&other_policy.fingerprint, 80, &[50]).unwrap();
        assert_eq!(
            other_target.fqn(),
            "other-project:wg-1-dev-team/coordinator"
        );
        assert_ne!(first_target, other_target);

        let same_spelling_a = identity_fixture_named("same-project", "wg-3-dev-team").await;
        let same_spelling_b = identity_fixture_named("same-project", "wg-3-dev-team").await;
        let policy_a = match resolve_member_policy_blocking(&same_spelling_a.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected same-spelling policy A, got {other:?}"),
        };
        let policy_b = match resolve_member_policy_blocking(&same_spelling_b.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected same-spelling policy B, got {other:?}"),
        };
        let (target_a, _) =
            prepare_internal_route_blocking(&policy_a.fingerprint, 80, &[50]).unwrap();
        let (target_b, _) =
            prepare_internal_route_blocking(&policy_b.fingerprint, 80, &[50]).unwrap();
        assert_eq!(target_a.fqn(), target_b.fqn());
        assert_ne!(target_a.replica_dir(), target_b.replica_dir());
        assert_ne!(target_a, target_b);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_route_rejects_symlink_before_repair_capable_resolver() {
        use std::os::unix::fs::symlink;

        let fixture = identity_fixture().await;
        let policy = match resolve_member_policy_blocking(&fixture.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected eligible policy, got {other:?}"),
        };
        let expected = fixture.workgroup.join("__agent_coordinator");
        let real = fixture.workgroup.join("coordinator-real");
        std::fs::rename(&expected, &real).unwrap();
        symlink(&real, &expected).unwrap();

        let error = prepare_internal_route_blocking(&policy.fingerprint, 80, &[50]).unwrap_err();
        assert!(error.contains("real non-link directory"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn coordinator_route_rejects_reparse_before_repair_capable_resolver() {
        let fixture = identity_fixture().await;
        let policy = match resolve_member_policy_blocking(&fixture.session) {
            MemberPolicyResolution::Eligible(policy) => policy,
            other => panic!("expected eligible policy, got {other:?}"),
        };
        let expected = fixture.workgroup.join("__agent_coordinator");
        let real = fixture.workgroup.join("coordinator-real");
        std::fs::rename(&expected, &real).unwrap();
        let output = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                expected.to_str().unwrap(),
                real.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        if !output.status.success() {
            println!(
                "skipping coordinator reparse check: stdout: {} stderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let error = prepare_internal_route_blocking(&policy.fingerprint, 80, &[50]).unwrap_err();
        assert!(error.contains("real non-link directory"));
        let _ = std::fs::remove_dir(&expected);
    }

    #[tokio::test]
    async fn post_filesystem_snapshot_recheck_rejects_end_removal_and_identity_changes() {
        let fixture = identity_fixture().await;
        let original = fixture.session.clone();
        let agent = original.agent_id.as_deref().unwrap();
        assert!(session_matches_policy_snapshot(
            Some(&original),
            original.id,
            agent,
            &original.working_directory,
            true,
        ));

        let mut normal_transition = original.clone();
        normal_transition.status = SessionStatus::Idle;
        assert!(session_matches_policy_snapshot(
            Some(&normal_transition),
            original.id,
            agent,
            &original.working_directory,
            true,
        ));

        let mut active_transition = original.clone();
        active_transition.status = SessionStatus::Active;
        assert!(session_matches_policy_snapshot(
            Some(&active_transition),
            original.id,
            agent,
            &original.working_directory,
            true,
        ));

        let mut exited = original.clone();
        exited.status = SessionStatus::Exited(0);
        assert!(!session_matches_policy_snapshot(
            Some(&exited),
            original.id,
            agent,
            &original.working_directory,
            true,
        ));
        let mut root = original.clone();
        root.is_root_agent = true;
        assert!(!session_matches_policy_snapshot(
            Some(&root),
            original.id,
            agent,
            &original.working_directory,
            true,
        ));
        let mut agentless = original.clone();
        agentless.agent_id = None;
        assert!(!session_matches_policy_snapshot(
            Some(&agentless),
            original.id,
            agent,
            &original.working_directory,
            true,
        ));
        assert!(!session_matches_policy_snapshot(
            None,
            original.id,
            agent,
            &original.working_directory,
            true,
        ));
        assert!(!session_matches_policy_snapshot(
            Some(&original),
            original.id,
            agent,
            &original.working_directory,
            false,
        ));

        let mut changed_agent = original.clone();
        changed_agent.agent_id = Some("other-agent".to_string());
        assert!(!session_matches_policy_snapshot(
            Some(&changed_agent),
            original.id,
            agent,
            &original.working_directory,
            true,
        ));
        let mut changed_cwd = original.clone();
        changed_cwd.working_directory = fixture.workgroup.to_string_lossy().to_string();
        assert!(!session_matches_policy_snapshot(
            Some(&changed_cwd),
            original.id,
            agent,
            &original.working_directory,
            true,
        ));
    }

    #[test]
    fn project_fqn_component_rejects_empty_colon_control_and_missing_utf8_name() {
        assert_eq!(
            validated_project_fqn_component(Some("project-a")).unwrap(),
            "project-a"
        );
        for invalid in [None, Some(""), Some("project:a"), Some("project\nname")] {
            assert!(validated_project_fqn_component(invalid).is_err());
        }
    }

    #[test]
    fn generation_exhaustion_never_wraps() {
        let mut state = ActorState::new();
        state.next_generation = Some(u64::MAX);
        assert_eq!(state.allocate_generation(), Some(u64::MAX));
        assert_eq!(state.allocate_generation(), None);
    }

    #[tokio::test]
    async fn stage_e_request_close_is_idempotent_and_close_and_join_owns_the_join() {
        // Stage E (#1064) alert-shutdown ownership sentinel (plan section 8.1,
        // 10.4 item 20, acceptance item 34): request_close only cancels the token
        // (never takes or awaits the actor join) and is idempotent, while
        // close_and_join stays the sole idempotent join owner.
        let runtime = ScriptedRuntime::new();
        let clock = ManualClock::new(Instant::now());
        let shutdown = CancellationToken::new();
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            Arc::clone(&clock) as Arc<dyn AlertClock>,
            shutdown.clone(),
        );

        monitor.request_close();
        monitor.request_close();
        assert!(
            shutdown.is_cancelled(),
            "request_close must cancel the shared shutdown token"
        );

        monitor.close_and_join().await.unwrap();
        monitor.close_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn actor_uses_manual_retry_clock_and_monitor_shutdown_is_idempotent() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        runtime
            .delivery_results
            .lock()
            .unwrap()
            .extend([Err("first failure".to_string()), Ok(())]);
        let clock = ManualClock::new(Instant::now());
        let shutdown = CancellationToken::new();
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            Arc::clone(&clock) as Arc<dyn AlertClock>,
            shutdown,
        );

        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 60,
            })
            .await
            .unwrap();
        runtime.wait_for_delivery_count(1).await;
        clock
            .wait_for_live_waiter_within(Duration::from_secs(5))
            .await;

        clock.advance(Duration::from_secs(4));
        assert_eq!(runtime.delivery_count(), 1);

        clock.advance(Duration::from_secs(1));
        runtime.wait_for_delivery_count(2).await;
        {
            let deliveries = runtime.deliveries.lock().unwrap();
            assert_eq!(deliveries[0].generation, deliveries[1].generation);
            assert_eq!(deliveries[0].thresholds, vec![50]);
        }

        monitor.close_and_join().await.unwrap();
        monitor.close_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_cancels_but_joins_the_single_in_flight_delivery() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        let release = runtime.block_next_delivery();
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            ManualClock::new(Instant::now()) as Arc<dyn AlertClock>,
            CancellationToken::new(),
        );
        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 60,
            })
            .await
            .unwrap();
        runtime.wait_for_delivery_count(1).await;

        let closing_monitor = Arc::clone(&monitor);
        let close = tokio::spawn(async move { closing_monitor.close_and_join().await });
        assert!(
            !close.is_finished(),
            "shutdown must join, not detach, delivery"
        );
        release.send(Ok(())).unwrap();
        close.await.unwrap().unwrap();
        monitor.close_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn unavailable_preserves_latches_and_impossible_reading_is_rejected() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        let runtime_dyn = Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>;
        let clock = ManualClock::new(now);
        let clock_dyn = Arc::clone(&clock) as Arc<dyn AlertClock>;
        let shutdown = CancellationToken::new();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
        let generation = *state.batches.keys().next().unwrap();

        process_sample(
            &runtime_dyn,
            &clock_dyn,
            &mut state,
            None,
            ContextSample::Unavailable { session_id: id },
            &shutdown,
        )
        .await;
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
        assert_eq!(state.batches.len(), 1);
        assert!(state.batches.contains_key(&generation));
        assert_eq!(state.sessions[&id].last_valid_percent, Some(60));

        let resolves_before = runtime.resolve_calls.load(Ordering::SeqCst);
        process_sample(
            &runtime_dyn,
            &clock_dyn,
            &mut state,
            None,
            ContextSample::Reading {
                session_id: id,
                percent: 101,
            },
            &shutdown,
        )
        .await;
        assert_eq!(
            runtime.resolve_calls.load(Ordering::SeqCst),
            resolves_before
        );
        assert!(state.batches.contains_key(&generation));

        runtime.live.store(false, Ordering::SeqCst);
        process_sample(
            &runtime_dyn,
            &clock_dyn,
            &mut state,
            None,
            ContextSample::Unavailable { session_id: id },
            &shutdown,
        )
        .await;
        assert!(!state.sessions.contains_key(&id));
        assert!(runtime.retired.lock().unwrap().contains(&id));
    }

    #[tokio::test]
    async fn committed_false_liveness_retirement_is_not_skipped_by_shutdown() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let runtime = ScriptedRuntime::new();
        runtime.live.store(false, Ordering::SeqCst);
        let shutdown = CancellationToken::new();
        *runtime.cancel_during_live_check.lock().unwrap() = Some(shutdown.clone());
        let runtime_dyn = Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>;
        let clock = ManualClock::new(now) as Arc<dyn AlertClock>;
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);

        process_sample(
            &runtime_dyn,
            &clock,
            &mut state,
            None,
            ContextSample::Unavailable { session_id: id },
            &shutdown,
        )
        .await;

        assert!(shutdown.is_cancelled());
        assert!(!state.sessions.contains_key(&id));
        assert_eq!(runtime.retired.lock().unwrap().as_slice(), &[id]);
    }

    #[tokio::test]
    async fn maintenance_committed_false_liveness_retirement_is_not_skipped_by_shutdown() {
        let id = Uuid::new_v4();
        let now = Instant::now();
        let runtime = ScriptedRuntime::new();
        runtime.live.store(false, Ordering::SeqCst);
        let shutdown = CancellationToken::new();
        *runtime.cancel_during_live_check.lock().unwrap() = Some(shutdown.clone());
        let runtime_dyn = Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>;
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);

        run_maintenance(&runtime_dyn, &mut state, None, &shutdown).await;

        assert!(shutdown.is_cancelled());
        assert!(!state.sessions.contains_key(&id));
        assert_eq!(runtime.retired.lock().unwrap().as_slice(), &[id]);
    }

    #[tokio::test]
    async fn definite_end_has_no_tombstone_and_new_uuid_starts_fresh() {
        let old_id = Uuid::new_v4();
        let new_id = Uuid::new_v4();
        let now = Instant::now();
        let runtime = ScriptedRuntime::new();
        let runtime_dyn = Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>;
        let clock = ManualClock::new(now) as Arc<dyn AlertClock>;
        let shutdown = CancellationToken::new();
        let mut state = ActorState::new();
        apply_numeric_resolution(&mut state, None, old_id, 60, eligible(old_id, &[50]), now);

        process_sample(
            &runtime_dyn,
            &clock,
            &mut state,
            None,
            ContextSample::SessionOver { session_id: old_id },
            &shutdown,
        )
        .await;
        assert!(!state.sessions.contains_key(&old_id));
        assert!(state.batches.is_empty());

        apply_numeric_resolution(
            &mut state,
            None,
            old_id,
            60,
            MemberPolicyResolution::PermanentIneligible("stale queued UUID".to_string()),
            now,
        );
        assert!(!state.sessions.contains_key(&old_id));
        apply_numeric_resolution(&mut state, None, new_id, 60, eligible(new_id, &[50]), now);
        assert_eq!(state.batches.values().next().unwrap().session_id, new_id);
    }

    #[tokio::test]
    async fn one_global_delivery_slot_keeps_samples_and_maintenance_live() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50, 75]);
        let release = runtime.block_next_delivery();
        let clock = ManualClock::new(Instant::now());
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            Arc::clone(&clock) as Arc<dyn AlertClock>,
            CancellationToken::new(),
        );

        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 60,
            })
            .await
            .unwrap();
        runtime.wait_for_delivery_count(1).await;
        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 80,
            })
            .await
            .unwrap();
        runtime.wait_for_resolve_calls(2).await;
        assert_eq!(
            runtime.delivery_count(),
            1,
            "the second due batch must wait"
        );

        clock
            .wait_for_live_waiter_within(CONTEXT_ALERT_MAINTENANCE_INTERVAL)
            .await;
        clock.advance(CONTEXT_ALERT_MAINTENANCE_INTERVAL);
        runtime.wait_for_live_checks(1).await;
        assert_eq!(runtime.delivery_count(), 1);

        release.send(Ok(())).unwrap();
        runtime.wait_for_delivery_count(2).await;
        {
            let deliveries = runtime.deliveries.lock().unwrap();
            assert_eq!(deliveries[0].thresholds, vec![50]);
            assert_eq!(deliveries[1].thresholds, vec![75]);
            assert!(deliveries[0].generation < deliveries[1].generation);
        }
        monitor.close_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn large_clock_jump_coalesces_missed_maintenance_intervals() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        let clock = ManualClock::new(Instant::now());
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            Arc::clone(&clock) as Arc<dyn AlertClock>,
            CancellationToken::new(),
        );
        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 40,
            })
            .await
            .unwrap();
        runtime.wait_for_resolve_calls(1).await;
        clock
            .wait_for_live_waiter_within(CONTEXT_ALERT_MAINTENANCE_INTERVAL)
            .await;

        clock.advance(CONTEXT_ALERT_MAINTENANCE_INTERVAL * 10);
        runtime.wait_for_live_checks(1).await;
        assert_eq!(runtime.live_checks.load(Ordering::SeqCst), 1);
        clock
            .wait_for_live_waiter_within(CONTEXT_ALERT_MAINTENANCE_INTERVAL)
            .await;
        monitor.close_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn delivery_task_panic_is_one_retryable_failure_for_same_generation() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        runtime.panic_next_delivery.store(true, Ordering::SeqCst);
        let clock = ManualClock::new(Instant::now());
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            Arc::clone(&clock) as Arc<dyn AlertClock>,
            CancellationToken::new(),
        );
        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 60,
            })
            .await
            .unwrap();
        runtime.wait_for_delivery_count(1).await;
        clock
            .wait_for_live_waiter_within(Duration::from_secs(5))
            .await;
        clock.advance(Duration::from_secs(5));
        runtime.wait_for_delivery_count(2).await;
        {
            let deliveries = runtime.deliveries.lock().unwrap();
            assert_eq!(deliveries[0].generation, deliveries[1].generation);
        }
        monitor.close_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn closed_sample_receiver_keeps_due_retry_and_biased_deadline_live() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        runtime
            .delivery_results
            .lock()
            .unwrap()
            .extend([Err("retry".to_string()), Ok(())]);
        let clock = ManualClock::new(Instant::now());
        let shutdown = CancellationToken::new();
        let release_live_check = runtime.block_next_live_check();
        let (sender, receiver) = mpsc::channel(2_048);
        let actor = tauri::async_runtime::spawn(run_context_alert_actor(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            Arc::clone(&clock) as Arc<dyn AlertClock>,
            receiver,
            shutdown.clone(),
        ));
        sender
            .send(ContextSample::Reading {
                session_id: id,
                percent: 60,
            })
            .await
            .unwrap();
        runtime.wait_for_delivery_count(1).await;
        clock
            .wait_for_live_waiter_within(Duration::from_secs(5))
            .await;
        for _ in 0..1_000 {
            sender
                .try_send(ContextSample::Unavailable { session_id: id })
                .unwrap();
        }
        drop(sender);
        runtime.wait_for_live_checks(1).await;
        clock.advance(Duration::from_secs(5));
        release_live_check
            .send(())
            .expect("release the first queued liveness check");
        runtime.wait_for_delivery_count(2).await;
        assert!(
            runtime.live_checks.load(Ordering::SeqCst) < 1_000,
            "the biased due deadline must run before draining a ready sample backlog"
        );

        shutdown.cancel();
        actor.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_interrupts_blocked_resolution_without_detaching_actor() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        let blocked = runtime.block_next_resolution();
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            ManualClock::new(Instant::now()) as Arc<dyn AlertClock>,
            CancellationToken::new(),
        );
        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 60,
            })
            .await
            .unwrap();
        runtime.wait_for_resolve_calls(1).await;

        tokio::time::timeout(Duration::from_secs(1), monitor.close_and_join())
            .await
            .expect("shutdown must cancel a blocked resolution")
            .unwrap();
        drop(blocked);
        assert_eq!(runtime.delivery_count(), 0);
    }

    #[tokio::test]
    async fn new_monitor_instance_models_restart_and_first_high_alerts_again() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        for expected in 1..=2 {
            let monitor = ContextAlertMonitor::start_with_runtime(
                Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
                ManualClock::new(Instant::now()) as Arc<dyn AlertClock>,
                CancellationToken::new(),
            );
            monitor
                .sender()
                .send(ContextSample::Reading {
                    session_id: id,
                    percent: 60,
                })
                .await
                .unwrap();
            runtime.wait_for_delivery_count(expected).await;
            monitor.close_and_join().await.unwrap();
        }
        assert_ne!(
            runtime.deliveries.lock().unwrap()[0].generation,
            0,
            "each process-local actor begins at a checked nonzero generation"
        );
        assert_eq!(runtime.deliveries.lock().unwrap()[0].generation, 1);
        assert_eq!(runtime.deliveries.lock().unwrap()[1].generation, 1);
    }

    #[tokio::test]
    async fn maintenance_retires_state_when_a_dropped_end_is_witnessed_as_not_live() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.set_policy(id, &[50]);
        let clock = ManualClock::new(Instant::now());
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            Arc::clone(&clock) as Arc<dyn AlertClock>,
            CancellationToken::new(),
        );

        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 40,
            })
            .await
            .unwrap();
        runtime.wait_for_resolve_calls(1).await;
        clock
            .wait_for_live_waiter_within(CONTEXT_ALERT_MAINTENANCE_INTERVAL)
            .await;
        runtime.live.store(false, Ordering::SeqCst);
        clock.advance(CONTEXT_ALERT_MAINTENANCE_INTERVAL);
        runtime.wait_for_retirement(id).await;

        monitor.close_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn unavailable_retires_even_when_actor_has_no_alert_state() {
        let id = Uuid::new_v4();
        let runtime = ScriptedRuntime::new();
        runtime.live.store(false, Ordering::SeqCst);
        let clock = ManualClock::new(Instant::now());
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            clock as Arc<dyn AlertClock>,
            CancellationToken::new(),
        );

        monitor
            .sender()
            .send(ContextSample::Unavailable { session_id: id })
            .await
            .unwrap();
        runtime.wait_for_retirement(id).await;

        monitor.close_and_join().await.unwrap();
    }
}
