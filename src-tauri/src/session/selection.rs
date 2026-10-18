use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Notify, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::sessions_persistence::persist_current_state_result;
use crate::pty::container_backend::ContainerTransportBackend;
use crate::pty::manager::PtyManager;
use crate::session::manager::{
    CommitDecision, CommitResult, LifecycleMutations, ManagerAggregateSnapshot,
    PendingCreateBinding, SessionManager,
};
use crate::session::session::{SessionInfo, SessionStatus};
use crate::session::warnings::{emit_session_warning, SessionWarning};
use crate::web::broadcast::WsBroadcaster;
use crate::DetachedSessionsState;

#[derive(Clone)]
pub(crate) struct ResponseCloseRegistration {
    pub tracker: crate::pty::manager::ResponseCloseState,
    pub member: crate::pty::manager::ResponseRouteIdentity,
    pub coordinator: crate::pty::manager::ResponseRouteIdentity,
    pub carrier: crate::config::sessions_persistence::PublishedCarrier,
    pub diagnostic_last_poll_outcome: Arc<std::sync::atomic::AtomicU64>,
}

impl ResponseCloseRegistration {
    fn diagnostic_poll_outcome(&self, code: u64, outcome: &'static str) {
        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
            && self
                .diagnostic_last_poll_outcome
                .swap(code, Ordering::Relaxed)
                != code
        {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown stage=poll_terminal outcome={} member={} coordinator={} site={}", outcome, self.member.session_id, self.coordinator.session_id, line!());
        }
    }
}

fn response_close_registrations() -> &'static Mutex<BTreeMap<Uuid, ResponseCloseRegistration>> {
    static REGISTRATIONS: OnceLock<Mutex<BTreeMap<Uuid, ResponseCloseRegistration>>> =
        OnceLock::new();
    REGISTRATIONS.get_or_init(Default::default)
}

pub(crate) fn remember_response_close_registration(
    registration: ResponseCloseRegistration,
) -> bool {
    let Ok(mut registrations) = response_close_registrations().lock() else {
        return false;
    };
    registrations.insert(registration.member.session_id, registration);
    true
}

pub(crate) fn response_close_registration(id: Uuid) -> Option<ResponseCloseRegistration> {
    response_close_registrations()
        .lock()
        .ok()?
        .get(&id)
        .cloned()
}

pub(crate) fn forget_response_close_registration(id: Uuid) {
    if let Ok(mut registrations) = response_close_registrations().lock() {
        registrations.remove(&id);
    }
}

fn response_registration_row<'a>(
    current: &'a crate::session::manager::CarrierSnapshot,
    identity: &crate::pty::manager::ResponseRouteIdentity,
) -> Option<&'a crate::session::session::Session> {
    current.rows.iter().find(|row| {
        row.id == identity.session_id
            && row.token == identity.token
            && row.backend_kind == identity.backend
            && current.versions.get(&row.id) == Some(&identity.carrier_version)
            && !current.pending_ids.contains(&row.id)
            && !matches!(row.status, SessionStatus::Exited(_))
    })
}

fn response_registration_route(
    pty: &Arc<Mutex<PtyManager>>,
    identity: &crate::pty::manager::ResponseRouteIdentity,
    row: &crate::session::session::Session,
) -> bool {
    let Ok(route) = PtyManager::snapshot_route_proof(pty, identity.session_id) else {
        return false;
    };
    route.backend_kind() == identity.backend
        && route.saved_cwd_matches_path(std::path::Path::new(&row.working_directory))
        && route.matches_requester_route(identity.backend, route.saved_cwd(), route.saved_replica())
}

pub(crate) async fn poll_response_close<R: Runtime>(app: &AppHandle<R>) {
    let Some(tracker) = app.try_state::<crate::pty::manager::ResponseCloseState>() else {
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=poll_health outcome=tracker_missing health=unknown", line!());
        return;
    };
    let diagnostic_status = tracker.diagnostic_poll_health();
    crate::commands::session::reconcile_response_closes(app).await;
    let Some(coordinator) = app.try_state::<SelectionCoordinator>() else {
        if diagnostic_status {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=poll_status outcome=coordinator_missing registrations=unknown", line!());
        }
        return;
    };
    let registrations = response_close_registrations()
        .lock()
        .ok()
        .map(|registrations| registrations.values().cloned().collect::<Vec<_>>());
    let Some(registrations) = registrations else {
        if diagnostic_status {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=poll_status outcome=registration_lock_error registrations=unknown", line!());
        }
        return;
    };
    if diagnostic_status {
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} outcome=observed stage=poll_status registration_count={}", line!(), registrations.len());
    }
    for registration in registrations {
        if !registration.tracker.same_state(&tracker) {
            registration.diagnostic_poll_outcome(1, "same_tracker_mismatch");
            continue;
        }
        if registration.member.backend != crate::pty::backend::SessionBackendKind::LocalProcess {
            registration.diagnostic_poll_outcome(2, "nonlocal_registration");
            continue;
        }
        let _ = coordinator.try_response_close(app, &registration).await;
    }
}

const COORDINATOR_QUEUE_CAPACITY: usize = 64;
const COORDINATOR_ADMISSION_CAPACITY: usize = 65;
const CREATE_TICKET_CAPACITY: usize = 16;
pub const SHUTDOWN_CLEANUP_BUDGET_SECS: u64 = 5;
const SHUTDOWN_FINALIZATION_RESERVE_MAX: Duration = Duration::from_millis(500);

tokio::task_local! {
    static IN_SELECTION_WORKER: ();
}

#[cfg(test)]
tokio::task_local! {
    /// #2475 - test-only run context. Read by the wrapper BEFORE the spawn, like
    /// `IN_SELECTION_WORKER`, because task locals do not propagate into it.
    pub(crate) static BENCH_CONTEXT: timing_probe::BenchContext;
}

/// #1940 - the process-wide turn serializing persisted selection operations
/// (broad profile apply, instance override, self-switch persist) together with
/// the restart settlement they trigger. Owned so a spawned task can hold it
/// across await points without borrowing its acquirer.
///
/// #2475 - a read-write lock: persisted operations take the write side and
/// stay mutually exclusive; the three read-only previews take the read side,
/// so they overlap each other but never a persisted operation. tokio's
/// `RwLock` is fair: a waiting writer blocks later readers, so an apply is
/// never starved by a stream of previews.
static SELECTION_OPERATION_TURN: OnceLock<Arc<tokio::sync::RwLock<()>>> = OnceLock::new();

fn selection_operation_turn() -> &'static Arc<tokio::sync::RwLock<()>> {
    SELECTION_OPERATION_TURN.get_or_init(|| Arc::new(tokio::sync::RwLock::new(())))
}

#[cfg(test)]
tokio::task_local! {
    /// #2475 - test-only: a private turn for this task, so a test that holds
    /// one reader while waiting for a second is not deadlocked by a writer
    /// from an unrelated test queueing on the process-wide turn. Read before
    /// any spawn, like `IN_SELECTION_WORKER`.
    pub(crate) static TEST_SELECTION_TURN: Arc<tokio::sync::RwLock<()>>;
}

/// The turn this task uses: always the process-wide one in production.
fn current_selection_turn() -> Arc<tokio::sync::RwLock<()>> {
    #[cfg(test)]
    if let Ok(turn) = TEST_SELECTION_TURN.try_with(Arc::clone) {
        return turn;
    }
    Arc::clone(selection_operation_turn())
}

/// #1940 - crate-only owned guard over the selection operation turn. Holding it
/// keeps the whole guarded operation - persisted publication through restart
/// settlement, classification and event publication - serialized against every
/// other guarded operation.
pub(crate) struct SelectionOperationGuard {
    _turn: tokio::sync::OwnedRwLockWriteGuard<()>,
}

/// #2475 - owned guard over the READ side of the selection operation turn:
/// excludes every persisted operation, admits other readers.
pub(crate) struct SelectionReadGuard {
    _turn: tokio::sync::OwnedRwLockReadGuard<()>,
}

/// #1940 - acquire the selection operation turn.
///
/// A submission from inside the selection worker task local is rejected with
/// `RecursiveSubmission` BEFORE waiting, so the worker can never block on a turn
/// held by an operation that is itself waiting for that same worker.
pub(crate) async fn acquire_selection_operation_turn(
) -> Result<SelectionOperationGuard, SelectionCoordinatorError> {
    if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
        return Err(SelectionCoordinatorError::RecursiveSubmission);
    }
    let turn = current_selection_turn().write_owned().await;
    Ok(SelectionOperationGuard { _turn: turn })
}

/// #2475 - acquire the READ side of the selection operation turn, with the same
/// `RecursiveSubmission` rejection BEFORE waiting as the write side.
pub(crate) async fn acquire_selection_read_turn(
) -> Result<SelectionReadGuard, SelectionCoordinatorError> {
    if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
        return Err(SelectionCoordinatorError::RecursiveSubmission);
    }
    let turn = current_selection_turn().read_owned().await;
    Ok(SelectionReadGuard { _turn: turn })
}

/// Which side of the selection operation turn an owned operation holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionTurnMode {
    Read,
    Write,
}

/// Either side of the turn, released on drop.
enum SelectionTurnGuard {
    Read(#[allow(dead_code)] SelectionReadGuard),
    Write(#[allow(dead_code)] SelectionOperationGuard),
}

async fn acquire_selection_turn(
    mode: SelectionTurnMode,
) -> Result<SelectionTurnGuard, SelectionCoordinatorError> {
    match mode {
        SelectionTurnMode::Read => acquire_selection_read_turn()
            .await
            .map(SelectionTurnGuard::Read),
        SelectionTurnMode::Write => acquire_selection_operation_turn()
            .await
            .map(SelectionTurnGuard::Write),
    }
}

/// Test-only: carry the submitter's private turn into the owned task, since
/// task locals do not propagate into a spawn.
#[cfg(test)]
fn with_test_selection_turn<T: Send + 'static>(
    turn: Option<Arc<tokio::sync::RwLock<()>>>,
    task: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send>> {
    match turn {
        Some(turn) => Box::pin(TEST_SELECTION_TURN.scope(turn, task)),
        None => Box::pin(task),
    }
}

/// #1940 - run a persisted selection operation on its own owned task.
///
/// The submitting task is rejected with `RecursiveSubmission` BEFORE the spawn
/// (task locals do not propagate into the spawned task, so checking there would
/// be ineffective). The owned task acquires the turn, awaits the supplied
/// operation and releases the turn only when that operation finishes; dropping
/// the caller detaches the owned task instead of aborting it, so a restart that
/// was already accepted still settles under the turn. A task join failure is
/// reported as the explicit `selectionOperationTurnJoinFailed` error.
pub(crate) async fn run_owned_selection_operation<F, Fut, T>(operation: F) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send,
    T: Send + 'static,
{
    run_owned_selection_operation_inner(SelectionTurnMode::Write, None, operation).await
}

/// #2475 - `run_owned_selection_operation` plus one timing log line that
/// separates the wait for the turn (`queue_wait`) from the operation itself
/// (`work`). Instrumentation only: the turn and the result are unchanged.
/// #2475 - test-only since the three timed reads moved to
/// `run_owned_selection_read_timed`; persisted operations are untimed.
#[cfg(test)]
pub(crate) async fn run_owned_selection_operation_timed<F, Fut, T>(
    command: &'static str,
    operation: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send,
    T: Send + 'static,
{
    run_owned_selection_operation_inner(SelectionTurnMode::Write, Some(command), operation).await
}

/// #2475 - a READ-ONLY operation on its own owned task, holding the read side
/// of the turn, plus the same timing line as `run_owned_selection_operation_timed`.
/// Only for operations proven not to write (see the #2485 read-only audit).
/// Same rejection, owned task and join-failure error as the write path.
pub(crate) async fn run_owned_selection_read_timed<F, Fut, T>(
    command: &'static str,
    operation: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send,
    T: Send + 'static,
{
    run_owned_selection_operation_inner(SelectionTurnMode::Read, Some(command), operation).await
}

fn selection_timing_line(
    command: &str,
    queue_wait: std::time::Duration,
    work: std::time::Duration,
    ok: bool,
) -> String {
    format!(
        "[selection-timing] command={} queue_wait_ms={:.1} work_ms={:.1} ok={}",
        command,
        queue_wait.as_secs_f64() * 1000.0,
        work.as_secs_f64() * 1000.0,
        ok
    )
}

fn record_selection_timing(
    command: &str,
    line: String,
    #[cfg(test)] sample: timing_probe::SelectionTimingSample,
) {
    log::info!("{}", line);
    #[cfg(test)]
    timing_probe::deliver(command, &sample, &line);
    #[cfg(not(test))]
    let _ = command;
}

/// #2475 - test-only timing instrumentation: recording sinks with their own
/// registration id, per-invocation correlation and per-run occupancy.
#[cfg(test)]
pub(crate) mod timing_probe {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread::ThreadId;
    use std::time::{Duration, Instant};

    /// One recorded timed operation. `submitted` is the wrapper's own stamp.
    #[derive(Debug, Clone)]
    pub(crate) struct SelectionTimingSample {
        pub(crate) invocation: Option<u64>,
        pub(crate) command: &'static str,
        pub(crate) submitted: Instant,
        pub(crate) queue_wait: Duration,
        pub(crate) work: Duration,
        pub(crate) ok: bool,
        pub(crate) body_thread: ThreadId,
    }

    pub(crate) type SelectionTimingSink = Arc<dyn Fn(&SelectionTimingSample, &str) + Send + Sync>;

    static SELECTION_TIMING_SINKS: Mutex<Vec<(u64, &'static str, SelectionTimingSink)>> =
        Mutex::new(Vec::new());
    static NEXT_SINK_ID: AtomicU64 = AtomicU64::new(1);
    static NEXT_INVOCATION: AtomicU64 = AtomicU64::new(1);

    /// Deliver to EVERY matching sink. The `Arc`s are cloned under the mutex
    /// and invoked after it is released, so a blocking sink never holds it.
    pub(super) fn deliver(command: &str, sample: &SelectionTimingSample, line: &str) {
        let sinks: Vec<SelectionTimingSink> = SELECTION_TIMING_SINKS
            .lock()
            .map(|sinks| {
                sinks
                    .iter()
                    .filter(|(_, name, _)| *name == command)
                    .map(|(_, _, sink)| Arc::clone(sink))
                    .collect()
            })
            .unwrap_or_default();
        for sink in sinks {
            sink(sample, line);
        }
    }

    /// Registers a recording sink for one command; only this registration is
    /// removed on drop.
    pub(crate) struct TimingSinkRegistration(u64);

    impl TimingSinkRegistration {
        /// A sink that only sees the production log line.
        pub(crate) fn new(command: &'static str, sink: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
            Self::new_sample(command, Arc::new(move |_sample, line| sink(line)))
        }

        pub(crate) fn new_sample(command: &'static str, sink: SelectionTimingSink) -> Self {
            let id = NEXT_SINK_ID.fetch_add(1, Ordering::Relaxed);
            SELECTION_TIMING_SINKS
                .lock()
                .expect("sinks")
                .push((id, command, sink));
            Self(id)
        }
    }

    impl Drop for TimingSinkRegistration {
        fn drop(&mut self) {
            if let Ok(mut sinks) = SELECTION_TIMING_SINKS.lock() {
                sinks.retain(|(id, _, _)| *id != self.0);
            }
        }
    }

    /// How many sinks are registered for `command` right now.
    pub(crate) fn registered_sinks(command: &str) -> usize {
        SELECTION_TIMING_SINKS
            .lock()
            .map(|sinks| sinks.iter().filter(|(_, name, _)| *name == command).count())
            .unwrap_or(0)
    }

    pub(crate) fn timing_field_ms(line: &str, name: &str) -> f64 {
        line.split(' ')
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{name} missing in {line}"))
    }

    /// Serializes whole bench runs against each other.
    pub(crate) fn bench_serialization_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// Simultaneous occupancy owned by one run: only bodies that inherited
    /// this `Arc` contribute, and there is no reset.
    #[derive(Debug, Default)]
    pub(crate) struct Occupancy {
        in_flight: AtomicUsize,
        max: AtomicUsize,
    }

    impl Occupancy {
        pub(crate) fn in_flight(&self) -> usize {
            self.in_flight.load(Ordering::SeqCst)
        }

        pub(crate) fn max(&self) -> usize {
            self.max.load(Ordering::SeqCst)
        }
    }

    /// Counts one holder until dropped (success, error or unwind).
    pub(crate) struct OccupancyGuard(Arc<Occupancy>);

    impl OccupancyGuard {
        pub(crate) fn enter(occupancy: &Arc<Occupancy>) -> Self {
            let now = occupancy.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            occupancy.max.fetch_max(now, Ordering::SeqCst);
            Self(Arc::clone(occupancy))
        }
    }

    impl Drop for OccupancyGuard {
        fn drop(&mut self) {
            self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[derive(Clone)]
    pub(crate) struct BenchContext {
        pub(crate) invocation: u64,
        pub(crate) occupancy: Arc<Occupancy>,
    }

    impl BenchContext {
        pub(crate) fn new(occupancy: &Arc<Occupancy>) -> Self {
            Self {
                invocation: NEXT_INVOCATION.fetch_add(1, Ordering::Relaxed),
                occupancy: Arc::clone(occupancy),
            }
        }
    }
}

async fn run_owned_selection_operation_inner<F, Fut, T>(
    mode: SelectionTurnMode,
    command: Option<&'static str>,
    operation: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send,
    T: Send + 'static,
{
    if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
        return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
    }
    let submitted = std::time::Instant::now();
    #[cfg(test)]
    let bench = BENCH_CONTEXT.try_with(Clone::clone).ok();
    #[cfg(test)]
    let test_turn = TEST_SELECTION_TURN.try_with(Arc::clone).ok();
    let task = async move {
        let turn = match acquire_selection_turn(mode).await {
            Ok(turn) => turn,
            Err(error) => return Err(error.to_string()),
        };
        let Some(command) = command else {
            let _turn = turn;
            return operation().await;
        };
        let queue_wait = submitted.elapsed();
        #[cfg(test)]
        let occupancy = bench
            .as_ref()
            .map(|bench| timing_probe::OccupancyGuard::enter(&bench.occupancy));
        let started = std::time::Instant::now();
        let result = operation().await;
        let work = started.elapsed();
        // Leave before the turn is released, so the next body cannot enter
        // while this one still counts.
        #[cfg(test)]
        drop(occupancy);
        // #2475 - release the turn BEFORE logging: the logger writes (and may
        // rotate) app.log synchronously, and that must never extend the turn.
        drop(turn);
        record_selection_timing(
            command,
            selection_timing_line(command, queue_wait, work, result.is_ok()),
            #[cfg(test)]
            timing_probe::SelectionTimingSample {
                invocation: bench.as_ref().map(|bench| bench.invocation),
                command,
                submitted,
                queue_wait,
                work,
                ok: result.is_ok(),
                body_thread: std::thread::current().id(),
            },
        );
        result
    };
    #[cfg(test)]
    let task = with_test_selection_turn(test_turn, task);
    let handle = tauri::async_runtime::spawn(task);
    match handle.await {
        Ok(result) => result,
        Err(_) => Err("selectionOperationTurnJoinFailed".to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionSource {
    InitialHydration,
    SessionCreated,
    UserSwitch,
    ManualClose,
    AutoClose,
    Restart,
    Restore,
    Detach,
    Attach,
    SpawnRollback,
    ResourceMonitor,
    BackgroundCleanup,
    LivenessReconcile,
}

impl fmt::Display for SelectionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::InitialHydration => "initialHydration",
            Self::SessionCreated => "sessionCreated",
            Self::UserSwitch => "userSwitch",
            Self::ManualClose => "manualClose",
            Self::AutoClose => "autoClose",
            Self::Restart => "restart",
            Self::Restore => "restore",
            Self::Detach => "detach",
            Self::Attach => "attach",
            Self::SpawnRollback => "spawnRollback",
            Self::ResourceMonitor => "resourceMonitor",
            Self::BackgroundCleanup => "backgroundCleanup",
            Self::LivenessReconcile => "livenessReconcile",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionMode {
    None,
    Live,
    Dormant,
}

impl fmt::Display for SelectionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Live => f.write_str("live"),
            Self::Dormant => f.write_str("dormant"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSelection {
    epoch: String,
    source: SelectionSource,
    user_initiated: bool,
    revision: u64,
    mode: SelectionMode,
    id: Option<String>,
    status: Option<SessionStatus>,
    has_pty: bool,
    detached: bool,
    displayable: bool,
}

impl SessionSelection {
    pub(super) fn initial(epoch: Uuid) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: SelectionSource::InitialHydration,
            user_initiated: false,
            revision: 0,
            mode: SelectionMode::None,
            id: None,
            status: None,
            has_pty: false,
            detached: false,
            displayable: false,
        }
    }

    pub(super) fn none(epoch: Uuid, revision: u64, cause: SelectionCause) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::None,
            id: None,
            status: None,
            has_pty: false,
            detached: false,
            displayable: false,
        }
    }

    pub(super) fn live(epoch: Uuid, revision: u64, cause: SelectionCause, id: Uuid) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::Live,
            id: Some(id.to_string()),
            status: Some(SessionStatus::Active),
            has_pty: true,
            detached: false,
            displayable: true,
        }
    }

    pub(super) fn dormant(
        epoch: Uuid,
        revision: u64,
        cause: SelectionCause,
        id: Uuid,
        exit_code: i32,
        has_pty: bool,
    ) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::Dormant,
            id: Some(id.to_string()),
            status: Some(SessionStatus::Exited(exit_code)),
            has_pty,
            detached: false,
            displayable: false,
        }
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    pub fn source(&self) -> SelectionSource {
        self.source
    }

    pub fn user_initiated(&self) -> bool {
        self.user_initiated
    }

    pub fn id(&self) -> Option<Uuid> {
        self.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
    }

    pub fn status(&self) -> Option<&SessionStatus> {
        self.status.as_ref()
    }

    pub fn has_pty(&self) -> bool {
        self.has_pty
    }

    pub fn detached(&self) -> bool {
        self.detached
    }

    pub fn displayable(&self) -> bool {
        self.displayable
    }

    // #1842: compiled on exactly the targets that compile `screenshot::native`,
    // whose `live_capture_resolution_...` test is the only caller in the crate.
    // Keep this predicate equal to `screenshot/mod.rs`'s `native` predicate. A
    // plain `#[cfg(test)]` would leave these caller-less on targets outside the
    // three, where `native` is not compiled, and `dead_code` under
    // `clippy -D warnings` would fail those CI legs.
    #[cfg(all(
        test,
        any(target_os = "windows", target_os = "linux", target_os = "macos")
    ))]
    pub(crate) fn live_for_test(id: Uuid) -> Self {
        Self::live(Uuid::new_v4(), 1, SelectionCause::UserSwitch, id)
    }

    #[cfg(all(
        test,
        any(target_os = "windows", target_os = "linux", target_os = "macos")
    ))]
    pub(crate) fn dormant_for_test(id: Uuid, exit_code: i32) -> Self {
        Self::dormant(
            Uuid::new_v4(),
            1,
            SelectionCause::Restore,
            id,
            exit_code,
            false,
        )
    }

    #[cfg(all(
        test,
        any(target_os = "windows", target_os = "linux", target_os = "macos")
    ))]
    pub(crate) fn none_for_test() -> Self {
        Self::none(Uuid::new_v4(), 1, SelectionCause::AutoClose)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedCreateIntent {
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedRestartIntent {
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedResourceIntent {
    User,
    Watchdog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionCause {
    SessionCreated(TrustedCreateIntent),
    UserSwitch,
    ManualClose,
    AutoClose,
    Restart(TrustedRestartIntent),
    Restore,
    Detach,
    Attach,
    SpawnRollback,
    ResourceMonitor(TrustedResourceIntent),
    BackgroundCleanup,
    LivenessReconcile,
}

impl SelectionCause {
    pub(super) fn source(self) -> SelectionSource {
        match self {
            Self::SessionCreated(_) => SelectionSource::SessionCreated,
            Self::UserSwitch => SelectionSource::UserSwitch,
            Self::ManualClose => SelectionSource::ManualClose,
            Self::AutoClose => SelectionSource::AutoClose,
            Self::Restart(_) => SelectionSource::Restart,
            Self::Restore => SelectionSource::Restore,
            Self::Detach => SelectionSource::Detach,
            Self::Attach => SelectionSource::Attach,
            Self::SpawnRollback => SelectionSource::SpawnRollback,
            Self::ResourceMonitor(_) => SelectionSource::ResourceMonitor,
            Self::BackgroundCleanup => SelectionSource::BackgroundCleanup,
            Self::LivenessReconcile => SelectionSource::LivenessReconcile,
        }
    }

    pub(super) fn user_initiated(self) -> bool {
        match self {
            Self::SessionCreated(TrustedCreateIntent::User)
            | Self::Restart(TrustedRestartIntent::User)
            | Self::ResourceMonitor(TrustedResourceIntent::User)
            | Self::UserSwitch
            | Self::ManualClose
            | Self::Detach
            | Self::Attach => true,
            Self::SessionCreated(TrustedCreateIntent::Background)
            | Self::Restart(TrustedRestartIntent::Background)
            | Self::ResourceMonitor(TrustedResourceIntent::Watchdog)
            | Self::AutoClose
            | Self::Restore
            | Self::SpawnRollback
            | Self::BackgroundCleanup
            | Self::LivenessReconcile => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSelectionSnapshot {
    pub session_id: Uuid,
    pub has_pty: bool,
    pub detached: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveRuntimeWitness {
    pub(super) session_id: Uuid,
    pub(super) has_pty: bool,
    pub(super) detached: bool,
}

impl LiveRuntimeWitness {
    fn from_snapshot(snapshot: RuntimeSelectionSnapshot) -> Option<Self> {
        (snapshot.has_pty && !snapshot.detached).then_some(Self {
            session_id: snapshot.session_id,
            has_pty: true,
            detached: false,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DormantRuntimeWitness {
    pub(super) session_id: Uuid,
    pub(super) has_pty: bool,
    pub(super) detached: bool,
}

impl DormantRuntimeWitness {
    fn from_snapshot(snapshot: RuntimeSelectionSnapshot) -> Option<Self> {
        (!snapshot.detached).then_some(Self {
            session_id: snapshot.session_id,
            has_pty: snapshot.has_pty,
            detached: false,
        })
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SelectionCoordinatorError {
    #[error("selectionCoordinatorBusy")]
    Busy,
    #[error("selectionCoordinatorUnavailable")]
    Unavailable,
    #[error("selectionCoordinatorRecursiveSubmission")]
    RecursiveSubmission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CriticalAdmissionKind {
    RouteLoss,
    WatchdogKill,
    BackgroundCleanup,
    ResponseClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CriticalAdmissionKey {
    session_id: Uuid,
    kind: CriticalAdmissionKind,
}

struct CriticalAdmissionGuard {
    inner: Weak<CoordinatorInner>,
    key: CriticalAdmissionKey,
}

impl CriticalAdmissionGuard {
    fn new(inner: &Arc<CoordinatorInner>, key: CriticalAdmissionKey) -> Self {
        Self {
            inner: Arc::downgrade(inner),
            key,
        }
    }
}

impl Drop for CriticalAdmissionGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            remove_critical_key(&inner, self.key);
        }
    }
}

struct CriticalAdmissionReservation {
    admission: OwnedSemaphorePermit,
    slot: mpsc::OwnedPermit<CoordinatorEnvelope>,
    guard: CriticalAdmissionGuard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriticalAdmissionOutcome<T> {
    Completed(T),
    AlreadyPending,
}

/// #1151 - `watchdog_resource_kill` must distinguish two conditions that the shared
/// `CriticalAdmissionOutcome::AlreadyPending` conflates: a genuinely in-flight critical
/// admission for the same UUID, and a session that is no longer public or pending. Only
/// the second may fall back to a registry-only quarantine retry.
#[derive(Debug, Clone)]
pub(crate) enum WatchdogKillOutcome {
    Completed(crate::resource_monitor::ResourceKillResult),
    AlreadyInFlight,
    NoPublicSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinatorPhase {
    Bootstrapping = 0,
    Restoring = 1,
    Running = 2,
    Closing = 3,
}

impl CoordinatorPhase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Restoring,
            2 => Self::Running,
            3 => Self::Closing,
            _ => Self::Bootstrapping,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum BlockingSeedTaskError {
    #[error("selectionCoordinatorUnavailable")]
    Unavailable,
    #[error("blocking seed task canceled before start")]
    CanceledBeforeStart,
    #[error("blocking seed task join failed: {0}")]
    Join(String),
}

enum BlockingSeedWorkState {
    Queued {
        abort: Option<tokio::task::AbortHandle>,
    },
    Started,
}

struct BlockingSeedTrackerState {
    accepting: bool,
    next_id: u64,
    work: BTreeMap<u64, BlockingSeedWorkState>,
}

struct BlockingSeedWorkTracker {
    state: Mutex<BlockingSeedTrackerState>,
    notify: Notify,
}

impl BlockingSeedWorkTracker {
    fn new() -> Self {
        Self {
            state: Mutex::new(BlockingSeedTrackerState {
                accepting: true,
                next_id: 1,
                work: BTreeMap::new(),
            }),
            notify: Notify::new(),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, BlockingSeedTrackerState> {
        self.state.lock().unwrap_or_else(|error| {
            log::error!("[selection] blocking seed tracker mutex poisoned; recovering");
            error.into_inner()
        })
    }

    fn register(&self, phase: &AtomicU8) -> Result<u64, BlockingSeedTaskError> {
        let mut state = self.lock_state();
        if !state.accepting
            || CoordinatorPhase::from_u8(phase.load(Ordering::Acquire)) != CoordinatorPhase::Running
        {
            return Err(BlockingSeedTaskError::Unavailable);
        }
        let id = state.next_id;
        let Some(next_id) = id.checked_add(1) else {
            state.accepting = false;
            log::error!("[selection] blocking seed tracker id space exhausted");
            return Err(BlockingSeedTaskError::Unavailable);
        };
        state.next_id = next_id;
        state
            .work
            .insert(id, BlockingSeedWorkState::Queued { abort: None });
        Ok(id)
    }

    fn attach_abort(&self, id: u64, abort: tokio::task::AbortHandle) {
        let abort_now = {
            let mut state = self.lock_state();
            match state.work.get_mut(&id) {
                Some(BlockingSeedWorkState::Queued { abort: slot }) => {
                    *slot = Some(abort.clone());
                    false
                }
                Some(BlockingSeedWorkState::Started) => false,
                None => true,
            }
        };
        if abort_now {
            abort.abort();
        }
    }

    fn begin(&self, id: u64) -> bool {
        let mut state = self.lock_state();
        match state.work.get_mut(&id) {
            Some(slot @ BlockingSeedWorkState::Queued { .. }) => {
                *slot = BlockingSeedWorkState::Started;
                true
            }
            Some(BlockingSeedWorkState::Started) | None => false,
        }
    }

    fn cancel_waiter(&self, id: u64) -> Option<tokio::task::AbortHandle> {
        let (abort, empty) = {
            let mut state = self.lock_state();
            let abort = match state.work.get(&id) {
                Some(BlockingSeedWorkState::Queued { .. }) => match state.work.remove(&id) {
                    Some(BlockingSeedWorkState::Queued { abort }) => abort,
                    _ => None,
                },
                Some(BlockingSeedWorkState::Started) | None => None,
            };
            (abort, state.work.is_empty())
        };
        if empty {
            self.notify.notify_waiters();
        }
        abort
    }

    fn complete_started(&self, id: u64) {
        let empty = {
            let mut state = self.lock_state();
            match state.work.remove(&id) {
                Some(BlockingSeedWorkState::Started) => {}
                Some(BlockingSeedWorkState::Queued { .. }) => {
                    log::error!("[selection] blocking seed tracker completed queued work id={id}");
                }
                None => {
                    log::error!("[selection] blocking seed tracker lost started work id={id}");
                }
            }
            state.work.is_empty()
        };
        if empty {
            self.notify.notify_waiters();
        }
    }

    fn seal(&self) -> Vec<tokio::task::AbortHandle> {
        let (aborts, empty) = {
            let mut state = self.lock_state();
            state.accepting = false;
            let queued_ids = state
                .work
                .iter()
                .filter_map(|(id, work)| {
                    matches!(work, BlockingSeedWorkState::Queued { .. }).then_some(*id)
                })
                .collect::<Vec<_>>();
            let mut aborts = Vec::new();
            for id in queued_ids {
                if let Some(BlockingSeedWorkState::Queued { abort: Some(abort) }) =
                    state.work.remove(&id)
                {
                    aborts.push(abort);
                }
            }
            (aborts, state.work.is_empty())
        };
        if empty {
            self.notify.notify_waiters();
        }
        aborts
    }

    async fn wait_quiescent_until(&self, deadline: Instant) -> bool {
        loop {
            let notified = self.notify.notified();
            if self.lock_state().work.is_empty() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || tokio::time::timeout(remaining, notified).await.is_err() {
                return self.lock_state().work.is_empty();
            }
        }
    }
}

struct BlockingSeedAwaitGuard {
    tracker: Arc<BlockingSeedWorkTracker>,
    id: u64,
    armed: bool,
}

impl BlockingSeedAwaitGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BlockingSeedAwaitGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(abort) = self.tracker.cancel_waiter(self.id) {
            abort.abort();
        }
    }
}

struct WorkerCompletionGuard {
    tracker: Arc<BlockingSeedWorkTracker>,
    id: u64,
}

impl Drop for WorkerCompletionGuard {
    fn drop(&mut self) {
        self.tracker.complete_started(self.id);
    }
}

enum BlockingSeedWorkerResult<T> {
    Completed(T),
    CanceledBeforeStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionRequest {
    UserSwitch { session_id: Uuid },
    Restore { persisted_target: Option<Uuid> },
}

#[derive(Debug, Clone)]
pub(crate) struct DormantRestoreRequest {
    pub persisted: crate::config::sessions_persistence::PersistedSession,
    pub working_directory: String,
    pub is_coordinator: bool,
    pub is_root_agent: bool,
}

impl SelectionRequest {
    pub fn user_switch(session_id: Uuid) -> Self {
        Self::UserSwitch { session_id }
    }

    pub(crate) fn restore(persisted_target: Option<Uuid>) -> Self {
        Self::Restore { persisted_target }
    }
}

#[derive(Debug)]
struct FinalizeCreateRequest {
    owned_replacement: Option<OwnedReplacementContinuation>,
    binding: PendingCreateBinding,
    cause: SelectionCause,
    auto_select_precondition: Option<(String, u64)>,
    warnings: Vec<SessionWarning>,
}

struct ConditionalResponseClose {
    ticket: crate::pty::manager::ResponseCloseTicket,
}

impl fmt::Debug for ConditionalResponseClose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConditionalResponseClose")
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum CoordinatorJob {
    #[cfg(test)]
    InlineReplacementFixture {
        input: crate::commands::session::FixtureReplacementInput,
        control: Arc<ReplacementFixtureControl>,
        response: oneshot::Sender<crate::commands::session::FixtureReplacementResult>,
    },
    Transition {
        request: SelectionRequest,
        response: oneshot::Sender<Result<Option<SessionSelection>, String>>,
    },
    Snapshot {
        response: oneshot::Sender<Result<SessionSelection, String>>,
    },
    FinalizeCreate {
        request: FinalizeCreateRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    RollbackCreate {
        binding: PendingCreateBinding,
    },
    RouteLoss {
        session_id: Uuid,
        exit_code: i32,
        response: oneshot::Sender<Result<(), String>>,
    },
    ConditionalResponseClose {
        request: ConditionalResponseClose,
    },
    Destroy {
        request: crate::commands::session::DestroyRequest,
        response: oneshot::Sender<Result<crate::commands::session::DestroyOutcome, String>>,
    },
    RestartLifecycle {
        entry: OwnedLifecycleEntry,
        request: crate::commands::session::RestartJobRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    RootLifecycle {
        entry: OwnedLifecycleEntry,
        request: crate::commands::session::RootJobRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    ResourceKill {
        session_id: Uuid,
        intent: TrustedResourceIntent,
        response: oneshot::Sender<Result<crate::resource_monitor::ResourceKillResult, String>>,
    },
    Detach {
        session_id: Uuid,
        geometry: Option<crate::config::settings::WindowGeometry>,
        suppress_selection: bool,
        response: oneshot::Sender<Result<String, String>>,
    },
    Attach {
        session_id: Uuid,
        response: oneshot::Sender<Result<(), String>>,
    },
    RestoreBarrier {
        nonce: Uuid,
        started: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    },
}

struct CoordinatorEnvelope {
    // #1580: admission and critical key are declared before `job` so an envelope
    // dropped without execution frees both before `job` drops the caller's sender.
    _admission: OwnedSemaphorePermit,
    _critical_admission: Option<CriticalAdmissionGuard>,
    job: CoordinatorJob,
    _create_ticket: Option<OwnedSemaphorePermit>,
}

/// #1580: the guards an executing envelope still owns after its job moved out.
struct EnvelopeHold {
    admission: Option<OwnedSemaphorePermit>,
    critical_admission: Option<CriticalAdmissionGuard>,
    _create_ticket: Option<OwnedSemaphorePermit>,
}

impl CoordinatorEnvelope {
    fn into_parts(self) -> (CoordinatorJob, EnvelopeHold) {
        let CoordinatorEnvelope {
            _admission: admission,
            _critical_admission: critical_admission,
            job,
            _create_ticket: create_ticket,
        } = self;
        (
            job,
            EnvelopeHold {
                admission: Some(admission),
                critical_admission,
                _create_ticket: create_ticket,
            },
        )
    }
}

impl EnvelopeHold {
    /// Capacity first, then the dedup key. Runs before every completion send.
    /// The create ticket is not released here; it drops with the hold.
    fn release(&mut self) {
        drop(self.admission.take());
        drop(self.critical_admission.take());
    }

    fn is_released(&self) -> bool {
        self.admission.is_none() && self.critical_admission.is_none()
    }

    /// Releases, then sends. Use only for a completion send.
    fn respond<T>(&mut self, response: oneshot::Sender<T>, value: T) -> Result<(), T> {
        self.release();
        response.send(value)
    }
}

/// A canceled admission must not strand startup in Restoring. Never overwrite
/// a concurrent Closing transition when returning the unaccepted admission.
struct RestoreSubmissionPhaseGuard<'a> {
    phase: &'a AtomicU8,
}

impl Drop for RestoreSubmissionPhaseGuard<'_> {
    fn drop(&mut self) {
        let _ = self.phase.compare_exchange(
            CoordinatorPhase::Restoring as u8,
            CoordinatorPhase::Bootstrapping as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Constructed only by the owned continuation after its initial B receipt and
/// the current A/Preparing/launch relation have been checked under the same U.
/// This capability never lives in an owner record or a serialized value.
pub(super) struct FinalizedOwnedCarryCapability {
    owner: Arc<ReplacementOwner>,
    operation: Uuid,
    preparing_nonce: Uuid,
    previous: Uuid,
    binding: PendingCreateBinding,
    token: Uuid,
    expected_version: u64,
    telegram_bot_id: Option<String>,
    communication: Option<crate::session::session::SessionCommunication>,
    start_fresh_on_restore: bool,
}

impl FinalizedOwnedCarryCapability {
    pub(super) fn bound_operation(&self) -> bool {
        self.operation == self.owner.operation
            && self.preparing_nonce == self.operation
            && self.previous != self.binding.session_id()
    }
    pub(super) fn session_id(&self) -> Uuid {
        self.binding.session_id()
    }
    pub(super) fn token(&self) -> Uuid {
        self.token
    }
    pub(super) fn expected_version(&self) -> u64 {
        self.expected_version
    }
    pub(super) fn telegram_bot_id(&self) -> &Option<String> {
        &self.telegram_bot_id
    }
    pub(super) fn communication(&self) -> &Option<crate::session::session::SessionCommunication> {
        &self.communication
    }
    pub(super) fn start_fresh_on_restore(&self) -> bool {
        self.start_fresh_on_restore
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FinalizedOwnedCarryOutcome {
    Applied,
    Recapture,
}

#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "C20 retains complete owned recovery work; preserve both variants and separate deep copies"
)]
pub(crate) enum OwnedReplacementWork {
    OrdinaryOwned {
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        message_id: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        body: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        payload: serde_json::Value,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        recipient: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        request: serde_json::Value,
    },
    PtyDurableReference {
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        version: u32,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        injection_id: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        op_id: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        sender: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        target: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        source_plane: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        payload_sha256: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        payload_bytes: u64,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        issued_at: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        expires_at: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        queued_at: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        lease_owner: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        attempt: i64,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        authority_session_id: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        authority_client_id: Option<String>,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        authority_client_generation: Option<String>,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        sender_identity_fingerprint: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
        )]
        target_identity_fingerprint: String,
        #[expect(
            dead_code,
            reason = "C20 requires complete inert retained source data; this field grants no replay authority"
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
        reason = "C20 requires complete inert retained source data; this field grants no replay authority"
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
struct OwnedLaunchReceipt {
    operation: Uuid,
    preparing_nonce: Uuid,
    binding: PendingCreateBinding,
    token: Uuid,
    generation: u64,
    backend: crate::pty::backend::SessionBackendKind,
    info: SessionInfo,
    cwd_identity: crate::session::manager::CarrierPhysicalIdentity,
    replica_identity: Option<crate::session::manager::CarrierPhysicalIdentity>,
}

struct InitialFinalizationReceipt {
    operation: Uuid,
    preparing_nonce: Uuid,
    binding: PendingCreateBinding,
    token: Uuid,
    carrier_version: u64,
    launch_generation: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplacementCleanupAuthority {
    Executor(Uuid),
    Claimed(Uuid),
    Disarmed,
    Unknown,
}

struct ReplacementProgress {
    source_executor: SourceExecutorObservation,
    current_previous: crate::session::session::Session,
    launch_start_fresh: Option<bool>,
    previous_carrier: crate::config::sessions_persistence::PublishedCarrier,
    binding: PendingCreateBinding,
    preparing_nonce: Uuid,
    committed: Option<CommitResult>,
    warnings: Vec<SessionWarning>,
    activity_started: bool,
    activity_completed: bool,
    initial_b: Option<InitialFinalizationReceipt>,
    launch: Option<OwnedLaunchReceipt>,
    current_checkpoint: Option<crate::config::sessions_persistence::PublishedCarrier>,
    first_checkpoint: Option<crate::config::sessions_persistence::PublishedCarrier>,
    pending_carrier: Option<crate::config::sessions_persistence::PublishedCarrier>,
    retirement_result: Option<CommitResult>,
    retirement_cleanup_completed: bool,
    preparing_released: bool,
    create_failed: bool,
    rollback_completed: bool,
    create_reservation: Option<crate::pty::manager::CreateOwnerReservation>,
    work_reservation: Option<crate::pty::manager::WorkDeliveryReservation>,
    tracker: Option<crate::pty::manager::ResponseCloseState>,
    #[expect(
        dead_code,
        reason = "C20 requires complete inert retained source data; this field grants no replay authority"
    )]
    accepted_work: OwnedReplacementWork,
    cleanup: ReplacementCleanupAuthority,
    preparing_started: bool,
    retired: bool,
    work_transferred: bool,
    reservations_released: bool,
    terminal_verified: bool,
    source_stop: Option<OwnedSourceStop>,
    context_prerequisite: Option<OwnedEffectState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceExecutorObservation {
    NotStarted,
    Active,
    Stopped,
    Returned { failed: bool },
}

/// Lexical observation of the accepted producer, never of its outer waiter.
/// This handle is transient; the retained owner stores only the observation.
pub(crate) struct SourceExecutorGuard {
    bound: Option<(Arc<ReplacementOwner>, Uuid)>,
    returned: bool,
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

enum PendingCleanupEvidence<'a> {
    ActualSetupFailure(&'a str),
    ObservedSourceStoppedOrReturnedFailure,
}

/// A transient handle to the same data-only owner retained by CoordinatorInner.
/// No app, backend, guard, task or future returns from that root.
#[derive(Clone)]
pub(crate) struct OwnedReplacementContinuation {
    owner: Arc<ReplacementOwner>,
    coordinator: Weak<CoordinatorInner>,
    executor_nonce: Uuid,
    // Transient executor/request only. ReplacementOwner never stores this
    // callback, its app captures, or the future it creates.
    source_validation: Option<OwnedSourceValidation>,
    #[cfg(test)]
    fixture: Option<Arc<ReplacementFixtureControl>>,
}

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
struct ReplacementFixtureEnvelopeTrace<'a> {
    hold: &'a mut EnvelopeHold,
    control: &'a ReplacementFixtureControl,
    owned: &'a OwnedReplacementContinuation,
    released: bool,
    outcome: Option<ReplacementFixtureOutcome>,
}

#[cfg(test)]
impl ReplacementFixtureEnvelopeTrace<'_> {
    fn release(&mut self) {
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
    identity: Mutex<Option<(Uuid, Uuid, PendingCreateBinding)>>,
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
    control: Arc<ReplacementFixtureControl>,
    identity: (Uuid, Uuid, PendingCreateBinding),
    enter: ReplacementFixtureCut,
    returned: ReplacementFixtureCut,
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
                OwnedEffectState::Completed { outcome: Err(error), .. } if error == "carry recapture: newer B preserved"),
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
    async fn receive<T>(
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
    fn validate_identity(&self, owned: &OwnedReplacementContinuation) {
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

    fn take_action(
        &self,
        owned: &OwnedReplacementContinuation,
        select: impl Fn(&ReplacementFixtureAction) -> bool,
    ) -> Option<ReplacementFixtureAction> {
        self.validate_identity(owned);
        let mut actions = self.actions.lock().expect("fixture actions");
        let position = actions.iter().position(select)?;
        Some(actions.remove(position))
    }

    async fn reached(
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

    fn record(&self, owned: &OwnedReplacementContinuation, cut: ReplacementFixtureCut) {
        self.record_outcome(owned, cut, None);
    }

    fn record_outcome(
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

    async fn wait_gate(gate: &Mutex<Option<oneshot::Receiver<()>>>) -> bool {
        let receiver = gate.lock().expect("fixture gate lock").take();
        match receiver {
            Some(receiver) => receiver.await.is_ok(),
            None => true,
        }
    }
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
    owner: Arc<ReplacementOwner>,
    executor_nonce: Uuid,
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

impl OwnedReplacementContinuation {
    #[cfg(test)]
    pub(crate) fn fixture_take_native_await_panic(
        &self,
        seed: bool,
    ) -> Option<oneshot::Receiver<()>> {
        let control = self.fixture.as_ref()?;
        control.validate_identity(self);
        let mut selected = control
            .native_await_panic
            .lock()
            .expect("native await panic");
        if matches!(
            (&*selected, seed),
            (Some(NativeAwaitPanicOnce::Context(_)), false)
                | (Some(NativeAwaitPanicOnce::Seed(_)), true)
        ) {
            selected.take().map(|selected| match selected {
                NativeAwaitPanicOnce::Context(receiver) | NativeAwaitPanicOnce::Seed(receiver) => {
                    receiver
                }
            })
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture_observe_context_failure(
        &self,
        error: &str,
        title: &str,
        message: &str,
    ) -> bool {
        let Some(control) = self.fixture.as_ref() else {
            return false;
        };
        {
            let observation = control
                .context_notification
                .lock()
                .expect("context notification");
            match &*observation {
                FixtureContextNotification::Unarmed => return false,
                FixtureContextNotification::Consumed { .. } => {
                    drop(observation);
                    panic!("duplicate context failure notification");
                }
                FixtureContextNotification::Armed => {}
            }
        }
        control.validate_identity(self);
        let binding = {
            let state = self.owner.state.lock().expect("context failure owner");
            assert_eq!(state.executor_nonce, self.executor_nonce);
            assert!(
                matches!(&state.effects[LifecycleEffectSlot::ContextPreparation as usize],
                OwnedEffectState::Completed { outcome: Err(actual), .. } if actual == error)
            );
            state
                .replacement
                .as_ref()
                .expect("context replacement")
                .binding
        };
        let mut observation = control
            .context_notification
            .lock()
            .expect("context notification");
        if !matches!(*observation, FixtureContextNotification::Armed) {
            drop(observation);
            panic!("context failure notification changed before consumption");
        }
        *observation = FixtureContextNotification::Consumed {
            operation: self.owner.operation,
            executor: self.executor_nonce,
            binding,
            error: error.to_owned(),
            title: title.to_owned(),
            message: message.to_owned(),
        };
        true
    }

    #[cfg(test)]
    pub(crate) fn fixture_assert_context_failure_observer(
        &self,
        error: &str,
        title: &str,
        message: &str,
    ) {
        let Some(original) = self.fixture.as_ref() else {
            return;
        };
        if !matches!(
            *original
                .context_notification
                .lock()
                .expect("context notification"),
            FixtureContextNotification::Armed
        ) {
            return;
        }
        // Reuse this SAME completed native failure for primitive assertions;
        // leave the real command's armed control untouched.
        let temporary = Arc::new(ReplacementFixtureControl::default());
        let mut probe = self.clone();
        probe.fixture = Some(temporary.clone());
        assert!(!probe.fixture_observe_context_failure("not the actual error", title, message));
        assert!(temporary.identity.lock().unwrap().is_none());
        assert!(matches!(
            *temporary.context_notification.lock().unwrap(),
            FixtureContextNotification::Unarmed
        ));
        *temporary.context_notification.lock().unwrap() = FixtureContextNotification::Armed;
        assert!(probe.fixture_observe_context_failure(error, title, message));
        let first = temporary.context_notification.lock().unwrap().clone();
        assert!(matches!(&first, FixtureContextNotification::Consumed {
            operation, executor, binding, error: actual, title: actual_title, message: actual_message,
        } if *operation == self.owner.operation && *executor == self.executor_nonce
            && Some(*binding) == self.binding() && actual == error
            && actual_title == title && actual_message == message));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            probe.fixture_observe_context_failure(error, title, message)
        }))
        .is_err());
        assert_eq!(*temporary.context_notification.lock().unwrap(), first);
        assert!(matches!(
            *original.context_notification.lock().unwrap(),
            FixtureContextNotification::Armed
        ));
    }

    #[cfg(test)]
    pub(crate) fn fixture_inserted(&self) {
        if let Some(control) = self.fixture.as_ref() {
            control.record(self, ReplacementFixtureCut::Inserted);
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture_native(&self, seed: bool) -> Option<ReplacementFixtureNativeInvocation> {
        let control = self.fixture.as_ref()?;
        control.validate_identity(self);
        Some(ReplacementFixtureNativeInvocation {
            control: control.clone(),
            identity: (
                self.owner.operation,
                self.executor_nonce,
                self.binding().expect("actual native B"),
            ),
            enter: if seed {
                ReplacementFixtureCut::SeedNativeEnter
            } else {
                ReplacementFixtureCut::ContextNativeEnter
            },
            returned: if seed {
                ReplacementFixtureCut::SeedNativeReturned
            } else {
                ReplacementFixtureCut::ContextNativeReturned
            },
        })
    }
    #[cfg(test)]
    pub(crate) async fn fixture_reached(&self, cut: ReplacementFixtureCut) -> bool {
        match &self.fixture {
            Some(control) => control.reached(self, cut).await,
            None => true,
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture_bind_twice(&self) -> bool {
        self.fixture
            .as_ref()
            .and_then(|control| {
                control.take_action(self, |action| {
                    matches!(action, ReplacementFixtureAction::BindPreparedTwice)
                })
            })
            .is_some()
    }
    #[cfg(test)]
    pub(crate) fn with_fixture_control(mut self, control: Arc<ReplacementFixtureControl>) -> Self {
        self.fixture = Some(control);
        self
    }
    fn reservation_release_is_observed(&self, pty: &Arc<Mutex<PtyManager>>) -> bool {
        let tracker = self
            .owner
            .state
            .lock()
            .ok()
            .and_then(|state| state.replacement.as_ref()?.tracker.clone());
        let Some(tracker) = tracker else {
            return false;
        };
        pty.try_lock().ok().is_some_and(|pty| {
            pty.response_close_state().same_state(&tracker) && tracker.publishers_bound()
        })
    }

    pub(crate) async fn validate_pre_effect_region(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        if !self.validate_source().await {
            return false;
        }
        let tracker = match pty.try_lock() {
            Ok(pty) => pty.response_close_state(),
            Err(_) => return false,
        };
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                self.validate_create_region(&permit, current, &tracker, pty)
            })
            .await
    }

    pub(crate) fn note_retained_work(&self, reason: String) {
        if let Ok(mut state) = self.owner.state.lock() {
            state.retained_reason = Some(reason);
        }
    }
    pub(crate) fn preserve_context_prerequisite(&self) -> bool {
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != self.executor_nonce
            || state.stage != ReplacementStage::Admitted
            || !matches!(
                state.effects[LifecycleEffectSlot::ContextPreparation as usize],
                OwnedEffectState::Completed { .. }
            )
            || state
                .replacement
                .as_ref()
                .is_none_or(|progress| progress.context_prerequisite.is_some())
        {
            return false;
        }
        let receipt = std::mem::replace(
            &mut state.effects[LifecycleEffectSlot::ContextPreparation as usize],
            OwnedEffectState::NotStarted,
        );
        if let Some(progress) = state.replacement.as_mut() {
            progress.context_prerequisite = Some(receipt);
        }
        true
    }
    pub(crate) fn with_source_validation(mut self, validation: OwnedSourceValidation) -> Self {
        self.source_validation = Some(validation);
        self
    }

    pub(crate) fn source_stop(&self) -> Option<OwnedSourceStop> {
        self.owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .source_stop
    }

    pub(crate) async fn validate_source(&self) -> bool {
        if self.source_stop().is_some() {
            return false;
        }
        let result = match self.source_validation.as_ref() {
            Some(validate) => validate().await,
            None => Err(OwnedSourceStop::StoreTransient),
        };
        if let Err(reason) = result {
            if let Ok(mut state) = self.owner.state.lock() {
                if let Some(progress) = state.replacement.as_mut() {
                    progress.source_stop.get_or_insert(reason);
                }
                state.retained_reason = Some(format!("replacement source stopped: {reason:?}"));
            }
            return false;
        }
        self.source_stop().is_none()
    }
    pub(crate) fn operation(&self) -> Uuid {
        self.owner.operation
    }
    pub(crate) fn binding(&self) -> Option<PendingCreateBinding> {
        let state = self.owner.state.lock().ok()?;
        (state.executor_nonce == self.executor_nonce).then_some(state.replacement.as_ref()?.binding)
    }
    pub(crate) fn previous_for_launch(
        &self,
        _permit: &crate::session::manager::CarrierMutationPermit,
    ) -> Option<crate::session::session::Session> {
        let mut state = self.owner.state.lock().ok()?;
        if state.executor_nonce != self.executor_nonce
            || state.stage != ReplacementStage::PendingCreate
        {
            return None;
        }
        let progress = state.replacement.as_mut()?;
        if progress.launch_start_fresh.is_some() {
            return None;
        }
        progress.launch_start_fresh = Some(progress.current_previous.start_fresh_on_restore);
        Some(progress.current_previous.clone())
    }
    pub(crate) fn launch_token(&self) -> Option<Uuid> {
        let state = self.owner.state.lock().ok()?;
        (state.executor_nonce == self.executor_nonce).then(|| {
            state
                .replacement
                .as_ref()?
                .launch
                .as_ref()
                .map(|launch| launch.token)
        })?
    }

    pub(crate) fn matches_captured_launch_route(
        &self,
        proof: &crate::pty::manager::PtySnapshotRouteProof,
    ) -> bool {
        let launch = self
            .owner
            .state
            .lock()
            .ok()
            .and_then(|state| state.replacement.as_ref()?.launch.clone());
        launch.is_some_and(|launch| {
            proof.backend_kind() == launch.backend
                && proof.matches_requester_route(
                    launch.backend,
                    proof.saved_cwd(),
                    proof.saved_replica(),
                )
                && launch.cwd_identity.matches_route_observations(
                    Some(proof.saved_cwd()),
                    proof.saved_replica(),
                    launch.replica_identity.as_ref(),
                )
        })
    }

    pub(crate) fn current_completion_region(
        &self,
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        let captured = self.owner.state.lock().ok().and_then(|state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            if !progress.preparing_released
                || !progress.retired
                || !progress.retirement_cleanup_completed
            {
                return None;
            }
            Some((
                progress.launch.clone()?,
                progress.current_checkpoint.clone()?,
            ))
        });
        let Some((launch, carrier)) = captured else {
            return false;
        };
        Self::launch_is_current(&launch, current, pty)
            && crate::config::sessions_persistence::current_protected_carrier(
                permit, &carrier, current,
            )
            .is_some()
    }

    pub(crate) async fn refresh_completion_info(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> Option<SessionInfo> {
        if !self.validate_source().await {
            return None;
        }
        let captured = self.owner.state.lock().ok().and_then(|state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            if !progress.preparing_released
                || !progress.retired
                || !progress.retirement_cleanup_completed
            {
                return None;
            }
            Some((
                progress.launch.clone()?,
                progress.current_checkpoint.clone()?,
            ))
        })?;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let observed = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !Self::launch_is_current(&captured.0, current, pty) {
                    return None;
                }
                let key = crate::config::sessions_persistence::current_protected_carrier(
                    &permit,
                    &captured.1,
                    current,
                )?;
                let row = current
                    .rows
                    .iter()
                    .find(|row| row.id == key.session_id && row.token == key.token)?;
                Some((SessionInfo::from(row), key))
            })
            .await?;
        {
            let mut state = self.owner.state.lock().ok()?;
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            // A new actual strict checkpoint is recovery evidence. The initial
            // post-finalization baseline stays immutable and is never rebased.
            state.replacement.as_mut()?.current_checkpoint = Some(observed.1);
        }
        drop(permit);
        Some(observed.0)
    }

    pub(crate) fn pre_write_work_matches(
        &self,
        current: &crate::session::manager::CarrierSnapshot,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        let captured = self.owner.state.lock().ok().and_then(|state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            if !progress.preparing_released
                || !progress.retired
                || progress.source_stop.is_some()
                || !progress.retirement_cleanup_completed
            {
                return None;
            }
            Some((
                progress.launch.clone()?,
                progress.current_checkpoint.clone()?,
            ))
        });
        let Some((launch, carrier)) = captured else {
            return false;
        };
        Self::launch_is_current(&launch, current, pty)
            && carrier.matches_owned_work_snapshot(current, launch.cwd_identity.canonical_path())
    }
    pub(crate) fn validate_create_region(
        &self,
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        tracker: &crate::pty::manager::ResponseCloseState,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseRouteIdentity};
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return false;
        };
        let captured = self.owner.state.lock().ok().and_then(|mut state| {
            if state.executor_nonce != self.executor_nonce
                || !matches!(
                    state.stage,
                    ReplacementStage::Admitted | ReplacementStage::PendingCreate
                )
            {
                return None;
            }
            let progress = state.replacement.as_mut()?;
            if !progress.preparing_started
                || progress.cleanup != ReplacementCleanupAuthority::Executor(self.executor_nonce)
                || !progress
                    .tracker
                    .as_ref()
                    .is_some_and(|saved| saved.same_state(tracker))
            {
                return None;
            }
            Some((
                progress.previous_carrier.clone(),
                progress.binding,
                progress.create_reservation.take(),
            ))
        });
        let Some((carrier, binding, reservation)) = captured else {
            return false;
        };
        // Only this operation's old reservation is released, under continuous U.
        drop(reservation);
        let Some(actual) = crate::config::sessions_persistence::renew_carrier_replacement(
            permit,
            &carrier,
            self.owner.operation,
            descriptor.previous.id,
            binding.session_id(),
            current,
        ) else {
            return false;
        };
        if !descriptor
            .replica_identity
            .as_ref()
            .is_some_and(|physical| actual.retired_local_replica_matches(physical))
        {
            return false;
        }
        let Some(generation) = actual.retired_local_generation() else {
            return false;
        };
        let Some(previous) = current
            .rows
            .iter()
            .find(|row| row.id == actual.session_id && row.token == actual.token)
            .cloned()
        else {
            return false;
        };
        if !pty.try_lock().ok().is_some_and(|pty| {
            pty.backend_kind(actual.session_id).is_none()
                && tracker.generation(actual.session_id).is_none()
        }) {
            return false;
        }
        let epoch = match crate::config::sessions_persistence::classify_create_carrier_scope(
            permit,
            &descriptor.target,
            std::path::Path::new(&descriptor.cwd),
        ) {
            crate::config::sessions_persistence::CreateCarrierScope::Protected {
                epoch,
                preparing_owner: Some(owner),
                ..
            } if owner == self.owner.operation => epoch,
            _ => return false,
        };
        let reservation = match tracker.reserve_create_owner(
            &descriptor.target,
            self.owner.operation,
            CreateClassification::Preparing {
                owner_nonce: self.owner.operation,
                identity: ResponseRouteIdentity {
                    target: descriptor.target.clone(),
                    session_id: actual.session_id,
                    token: actual.token,
                    generation,
                    backend: crate::pty::backend::SessionBackendKind::LocalProcess,
                    carrier_version: actual.version,
                },
                publication_epoch: epoch,
            },
        ) {
            Ok(CreateAdmission::Reserved(reservation)) => reservation,
            _ => return false,
        };
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        let Some(progress) = state.replacement.as_mut() else {
            return false;
        };
        if progress
            .launch_start_fresh
            .is_some_and(|fresh| fresh != previous.start_fresh_on_restore)
        {
            state.retained_reason =
                Some("replacement launch fresh decision changed; current A preserved".into());
            return false;
        }
        progress.current_previous = previous;
        progress.previous_carrier = actual;
        progress.create_reservation = Some(reservation);
        true
    }

    pub(crate) fn mark_pending(&self, binding: PendingCreateBinding) -> bool {
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != self.executor_nonce || state.stage != ReplacementStage::Admitted
        {
            return false;
        }
        if !state
            .replacement
            .as_ref()
            .is_some_and(|progress| progress.binding == binding && progress.preparing_started)
        {
            return false;
        }
        state.stage = ReplacementStage::PendingCreate;
        true
    }

    /// Bind the actual observed spawn once; no launch witness/query invents a
    /// backend result, and a later route is never adopted as the initial launch.
    pub(crate) fn record_launch(
        &self,
        session: &crate::session::session::Session,
        generation: u64,
    ) -> bool {
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return false;
        };
        if session.shell != descriptor.shell
            || session.backend_kind != descriptor.backend
            || session.working_directory != descriptor.cwd
            || session.agent_id != descriptor.requested_agent_id
        {
            return false;
        }
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != self.executor_nonce
            || state.stage != ReplacementStage::PendingCreate
        {
            return false;
        }
        let Some(progress) = state.replacement.as_mut() else {
            return false;
        };
        if progress.binding.session_id() != session.id
            || progress.launch.is_some()
            || progress.cleanup != ReplacementCleanupAuthority::Executor(self.executor_nonce)
        {
            return false;
        }
        progress.launch = Some(OwnedLaunchReceipt {
            operation: self.owner.operation,
            preparing_nonce: progress.preparing_nonce,
            binding: progress.binding,
            token: session.token,
            generation,
            backend: session.backend_kind,
            info: SessionInfo::from(session),
            cwd_identity: descriptor.cwd_identity.clone(),
            replica_identity: descriptor.replica_identity.clone(),
        });
        true
    }

    fn launch_is_current(
        launch: &OwnedLaunchReceipt,
        current: &crate::session::manager::CarrierSnapshot,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        let Some(row) = current.rows.iter().find(|row| {
            row.id == launch.binding.session_id()
                && row.token == launch.token
                && row.backend_kind == launch.backend
                && !row.is_root_agent
                && !row.is_coordinator
                && !row.was_detached
                && !matches!(row.status, SessionStatus::Exited(_))
                && row.working_directory == launch.info.working_directory
                && row.shell == launch.info.shell
                && row.shell_args == launch.info.shell_args
                && row.effective_shell_args == launch.info.effective_shell_args
                && row.agent_id == launch.info.agent_id
                && row.agent_label == launch.info.agent_label
                && row.agent_kind == launch.info.agent_kind
                && row.requested_profile == launch.info.requested_profile
                && row.effective_profile == launch.info.effective_profile
                && row.profile_fallback_chain == launch.info.profile_fallback_chain
                && row.profile_fallback_applied == launch.info.profile_fallback_applied
                && row.profile_content_hash == launch.info.profile_content_hash
                && row.trusted_configured_spawn == launch.info.trusted_configured_spawn
                && row.effective_codex_home == launch.info.effective_codex_home
        }) else {
            return false;
        };
        pty.try_lock().ok().is_some_and(|pty| {
            let Some((Some(cwd), replica, generation)) = pty.route_identities(row.id) else {
                return false;
            };
            generation == launch.generation
                && pty.backend_kind(row.id) == Some(launch.backend)
                && pty.has_session(row.id)
                && launch.cwd_identity.matches_route_observations(
                    Some(&cwd),
                    replica.as_ref(),
                    launch.replica_identity.as_ref(),
                )
        })
    }

    fn retained_commit_copy(&self) -> Option<CommitResult> {
        let state = self.owner.state.lock().ok()?;
        let committed = state.replacement.as_ref()?.committed.as_ref()?;
        Some(CommitResult {
            removed_rows: committed.removed_rows.clone(),
            missing_ids: committed.missing_ids.clone(),
            changed_rows: committed.changed_rows.clone(),
            finalized_rows: committed.finalized_rows.clone(),
            cleared_raise_hand_ids: committed.cleared_raise_hand_ids.clone(),
            selection: committed.selection.clone(),
            activity: committed.activity.clone(),
        })
    }

    #[cfg(test)]
    fn fixture_stop_exact_b(
        &self,
        pty: &Arc<Mutex<PtyManager>>,
        launch: &OwnedLaunchReceipt,
        _permit: &crate::session::manager::CarrierMutationPermit,
    ) {
        let stop = self
            .fixture
            .as_ref()
            .and_then(|control| {
                control.take_action(self, |action| {
                    matches!(action, ReplacementFixtureAction::FixtureStopExactB)
                })
            })
            .is_some();
        if stop {
            assert!(
                self.fixture
                    .as_ref()
                    .is_some_and(|control| control.mock_backend.load(Ordering::Acquire)),
                "stop-exact-B requires fixture mock backend"
            );
            let pty = pty.lock().expect("fixture mock backend lock");
            assert_eq!(
                pty.backend_kind(launch.binding.session_id()),
                Some(launch.backend)
            );
            assert_eq!(
                pty.response_close_state()
                    .generation(launch.binding.session_id()),
                Some(launch.generation)
            );
            pty.kill_for_kind(launch.binding.session_id(), launch.backend)
                .expect("fixture stop exact B");
        }
    }

    /// None is an internal retained outcome. It is never translated into a
    /// successful create or ordinary pending-create error/rollback.
    async fn commit_finalized<R: Runtime>(
        &self,
        transaction: &SelectionTransaction<R>,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Option<Result<CommitResult, String>> {
        if !self.validate_source().await {
            return None;
        }
        let manager = transaction.manager().await;
        let pty = transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .inner()
            .clone();
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforeCommit)
            .await
        {
            return None;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let launch = self.owner.state.lock().ok().and_then(|state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::PendingCreate
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            (progress.cleanup == ReplacementCleanupAuthority::Executor(self.executor_nonce))
                .then(|| progress.launch.clone())
                .flatten()
        })?;
        let authorized = manager
            .with_current_carrier_snapshot(&permit, |current| {
                let tracker = pty.try_lock().ok().map(|pty| pty.response_close_state());
                tracker.is_some_and(|tracker| {
                    self.validate_create_region(&permit, current, &tracker, &pty)
                }) && Self::launch_is_current(&launch, current, &pty)
            })
            .await;
        if !authorized {
            return None;
        }
        #[cfg(test)]
        let panic_before_disarm = self
            .fixture
            .as_ref()
            .and_then(|control| {
                control.take_action(self, |action| {
                    matches!(
                        action,
                        ReplacementFixtureAction::CommitBeforeDisarmPanicOnce
                    )
                })
            })
            .is_some();
        let committed = match manager
            .commit_selection_transition_with_carrier_permit(
                &permit,
                &transaction.capability,
                decision,
                cause,
                mutations,
            )
            .await
        {
            Ok(committed) => committed,
            Err(error) => return Some(Err(error)),
        };
        // Actual commit -> synchronous disarm. No append, allocation, await or
        // fallible row lookup intervenes in this ownership transition.
        {
            #[cfg(test)]
            if panic_before_disarm {
                panic!("selected Replacement commit-before-disarm unwind");
            }
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.cleanup = ReplacementCleanupAuthority::Disarmed;
            progress.committed = Some(committed);
            state.stage = ReplacementStage::FinalizedCarry;
        }
        #[cfg(test)]
        {
            if !self
                .fixture_reached(ReplacementFixtureCut::AfterDisarm)
                .await
            {
                return None;
            }
            self.fixture_stop_exact_b(&pty, &launch, &permit);
        }
        let baseline = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !Self::launch_is_current(&launch, current, &pty)
                    || current.pending_ids.contains(&launch.binding.session_id())
                {
                    return false;
                }
                let Some(version) = current
                    .versions
                    .get(&launch.binding.session_id())
                    .copied()
                    .filter(|version| *version < u64::MAX)
                else {
                    return false;
                };
                let Ok(mut state) = self.owner.state.lock() else {
                    return false;
                };
                let Some(progress) = state.replacement.as_mut() else {
                    return false;
                };
                if progress.initial_b.is_some() {
                    return false;
                }
                progress.initial_b = Some(InitialFinalizationReceipt {
                    operation: self.owner.operation,
                    preparing_nonce: progress.preparing_nonce,
                    binding: launch.binding,
                    token: launch.token,
                    carrier_version: version,
                    launch_generation: launch.generation,
                });
                true
            })
            .await;
        drop(permit);
        let committed = self.retained_commit_copy()?;
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            if progress.activity_started {
                return None;
            }
            progress.activity_started = true;
        }
        crate::config::activity_log::append_batch(&committed.activity);
        if let Ok(mut state) = self.owner.state.lock() {
            if let Some(progress) = state.replacement.as_mut() {
                progress.activity_completed = true;
            }
            if !baseline {
                state.stage = ReplacementStage::Ambiguous;
                state.retained_reason =
                    Some("initial B receipt unavailable before releasing finalization U".into());
            }
        }
        #[cfg(test)]
        if baseline {
            if let Some(control) = &self.fixture {
                control.record(self, ReplacementFixtureCut::Baseline);
                if !ReplacementFixtureControl::wait_gate(&control.baseline_gate).await {
                    self.note_retained_work("fixture baseline observer lost".into());
                    return None;
                }
            }
        }
        baseline.then_some(Ok(committed))
    }

    async fn apply_finalized_carry(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        if !self.validate_source().await {
            return false;
        }
        let Some(effect) = self.start_effect(LifecycleEffectSlot::Carry) else {
            return false;
        };
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforeCarry)
            .await
        {
            return false;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let admission = manager
            .with_current_carrier_snapshot(&permit, |current| {
                self.finalized_carry_capability(&permit, current, pty)
            })
            .await;
        let capability = match admission {
            Some(Some(capability)) => capability,
            Some(None) => {
                // Initial version remains immutable. A newer B is reconciled
                // as it stands and never overwritten by A's old carry.
                effect.complete(Err("carry recapture: newer B preserved".into()));
                if let Ok(mut state) = self.owner.state.lock() {
                    state.stage = ReplacementStage::FinalizedPreRename;
                }
                return true;
            }
            None => return false,
        };
        let outcome = manager
            .apply_finalized_owned_replacement_carry(&permit, capability)
            .await;
        match outcome {
            FinalizedOwnedCarryOutcome::Applied => {
                // Consume Carry synchronously before any further await.
                if let Ok(mut state) = self.owner.state.lock() {
                    state.stage = ReplacementStage::FinalizedPreRename;
                } else {
                    return false;
                }
                effect.complete(Ok(None));
            }
            FinalizedOwnedCarryOutcome::Recapture => {
                effect.complete(Err("carry recapture: finalized binding changed".into()));
                return false;
            }
        }
        // Read the real version after CarrierStateWrite::drop, still under U.
        let launch = self.owner.state.lock().ok().and_then(|state| {
            state
                .replacement
                .as_ref()
                .and_then(|progress| progress.launch.clone())
        });
        let Some(launch) = launch else {
            return false;
        };
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                Self::launch_is_current(&launch, current, pty).then_some(())?;
                current
                    .versions
                    .get(&launch.binding.session_id())
                    .filter(|version| **version < u64::MAX)?;
                Some(())
            })
            .await
            .is_some()
    }

    /// Some(None) is the explicit newer-B path, never a replacement baseline.
    fn finalized_carry_capability(
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

    fn publication_region_current(
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

    async fn publish_checkpoint(
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
                        let confirmed = manager.with_current_carrier_snapshot(&permit, |current| {
                            if !self.publication_region_current(current, pty) { return None; }
                            crate::config::sessions_persistence::reconcile_owned_replacement_readback(
                                &permit, &carrier, self.owner.operation, descriptor.previous.id, current,
                            )
                        }).await;
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

    async fn retire_previous<R: Runtime>(
        &self,
        transaction: &SelectionTransaction<R>,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> Option<bool> {
        if !self.validate_source().await {
            return None;
        }
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return None;
        };
        let effect = self.start_effect(LifecycleEffectSlot::Retirement)?;
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforeRetirement)
            .await
        {
            return None;
        }
        let carrier = self
            .owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .current_checkpoint
            .clone()?;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let decision = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !self.publication_region_current(current, pty)
                    || crate::config::sessions_persistence::renew_carrier_replacement(
                        &permit,
                        &carrier,
                        self.owner.operation,
                        descriptor.previous.id,
                        carrier.session_id,
                        current,
                    )
                    .is_none()
                {
                    return None;
                }
                Some(if current.selection.id() == Some(descriptor.previous.id) {
                    CommitDecision::Clear
                } else {
                    CommitDecision::Keep
                })
            })
            .await?;
        let mut mutations = LifecycleMutations::default();
        mutations.remove(descriptor.previous.id);
        let retired = match manager
            .commit_selection_transition_with_carrier_permit(
                &permit,
                &transaction.capability,
                decision,
                SelectionCause::BackgroundCleanup,
                mutations,
            )
            .await
        {
            Ok(retired) => retired,
            Err(error) => {
                effect.complete(Err(error));
                return None;
            }
        };
        // Record the actual removal before any cleanup await. A missing A is
        // admissible on recovery only through this same-operation receipt.
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.retired = retired
                .removed_rows
                .iter()
                .any(|row| row.id == descriptor.previous.id.to_string());
            progress.retirement_result = Some(retired);
            if !progress.retired {
                return None;
            }
        }
        // The version is observed after manager Drop under the SAME retirement U.
        let reusable = manager
            .with_current_carrier_snapshot(&permit, |current| {
                crate::config::sessions_persistence::renew_carrier_replacement(
                    &permit,
                    &carrier,
                    self.owner.operation,
                    descriptor.previous.id,
                    carrier.session_id,
                    current,
                )
            })
            .await;
        drop(permit);
        let activity = self
            .owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .retirement_result
            .as_ref()?
            .activity
            .clone();
        crate::config::activity_log::append_batch(&activity);
        crate::commands::session::complete_owned_replacement_retirement(
            transaction,
            descriptor.previous.id,
        )
        .await;
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.retirement_cleanup_completed = true;
            if let Some(current) = reusable.as_ref() {
                progress.current_checkpoint = Some(current.clone());
            }
        }
        effect.complete(Ok(Some(descriptor.previous.id.to_string())));
        Some(reusable.is_some())
    }

    async fn complete_finalized_create<R: Runtime>(
        &self,
        transaction: &SelectionTransaction<R>,
    ) -> Option<(SessionInfo, SessionSelection)> {
        let manager = transaction.manager().await;
        let pty = transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .inner()
            .clone();
        let settings = transaction
            .app()
            .state::<crate::config::settings::SettingsState>()
            .read()
            .await
            .clone();
        let roots =
            crate::config::sessions_persistence::session_retention_project_paths(&settings).ok()?;
        if !self.apply_finalized_carry(&manager, &pty).await {
            return None;
        }
        let carrier = self
            .owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .previous_carrier
            .clone();
        if !self
            .publish_checkpoint(&manager, &pty, &roots, carrier)
            .await
        {
            return None;
        }
        {
            let mut state = self.owner.state.lock().ok()?;
            state.stage = ReplacementStage::FinalizedRetire;
        }
        let reusable = self.retire_previous(transaction, &manager, &pty).await?;
        if !reusable {
            let carrier = {
                let mut state = self.owner.state.lock().ok()?;
                state.stage = ReplacementStage::FinalizedRecheckpoint;
                state.replacement.as_ref()?.current_checkpoint.clone()?
            };
            if !self
                .publish_checkpoint(&manager, &pty, &roots, carrier)
                .await
            {
                return None;
            }
        }
        let carrier = self
            .owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .current_checkpoint
            .clone()?;
        let previous = match &self.owner.descriptor {
            OwnedReplacementDescriptor::Replacement(descriptor) => descriptor.previous.id,
            _ => return None,
        };
        if !self.validate_source().await {
            return None;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let current = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !self.publication_region_current(current, &pty)
                    || !crate::config::sessions_persistence::finish_carrier_replacement(
                        &permit,
                        &carrier,
                        self.owner.operation,
                        previous,
                        current,
                    )
                {
                    return None;
                }
                let key = crate::config::sessions_persistence::current_protected_carrier(
                    &permit, &carrier, current,
                )?;
                let row = current
                    .rows
                    .iter()
                    .find(|row| row.id == key.session_id && row.token == key.token)?;
                Some((SessionInfo::from(row), current.selection.clone(), key))
            })
            .await?;
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.preparing_released = true;
            progress.current_checkpoint = Some(current.2);
            state.stage = ReplacementStage::FinalizedPublish;
        }
        drop(permit);
        Some((current.0, current.1))
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
                || !progress.work_transferred || !progress.preparing_released || !progress.reservations_released
                || !progress.retired || !progress.retirement_cleanup_completed
                || !progress.activity_completed || progress.initial_b.is_none()
                || !matches!(progress.context_prerequisite,
                    None | Some(OwnedEffectState::Completed { .. }))
                || progress.create_reservation.is_some() || progress.work_reservation.is_some()
                || !matches!(state.completion.as_ref(), Some(Ok(info)) if info.id == launch.binding.session_id().to_string())
                || !state.effects.iter().all(|effect| matches!(effect,
                    OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }))
            { return None; }
            let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else { return None; };
            Some((progress.current_checkpoint.clone()?, launch.clone(), descriptor.previous.id))
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

    async fn cleanup_stopped_source(
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

    fn pending_cleanup_runtime_matches(
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
    fn claim_pending_cleanup_state(
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

    async fn claim_and_cleanup_pending(
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

impl fmt::Debug for OwnedReplacementContinuation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedReplacementContinuation")
            .field("operation", &self.owner.operation)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
enum OwnedReplacementDescriptor {
    Replacement(Box<ResolvedReplacement>),
    EntryRestart(crate::commands::session::RestartJobRequest),
    EntryRoot(crate::commands::session::RootJobRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplacementStage {
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
enum EntryTerminalDisposition {
    Success,
    Failure,
    RolledBack,
}

struct EntryReleaseReceipt {
    operation: Uuid,
    executor_nonce: Uuid,
    envelope_never_sent: bool,
    worker_returned_and_sidecar_released: bool,
    reached_effects: [Option<Uuid>; 11],
    prerequisite_attempt: Option<Uuid>,
    binding: Option<(PendingCreateBinding, EntryCreateStage)>,
    reader_demand: Option<EntryReaderDemandReceipt>,
}

struct EntryTerminal {
    disposition: EntryTerminalDisposition,
    actual_result: Result<SessionInfo, String>,
    release_receipt: EntryReleaseReceipt,
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
enum OwnedEffectState {
    NotStarted,
    Running(Uuid),
    Completed {
        attempt: Uuid,
        outcome: Result<Option<String>, String>,
    },
    Ambiguous(Uuid),
}

pub(crate) struct LifecycleEffectGuard {
    owner: Arc<ReplacementOwner>,
    slot: LifecycleEffectSlot,
    attempt: Uuid,
    completed: bool,
}

impl LifecycleEffectGuard {
    /// The caller observed that this particular effect was not reached.
    fn skip(mut self) {
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
enum EntryCreateStage {
    Reserved,
    Pending,
    Finalized,
    CleanupClaimed(Uuid),
    RolledBack,
    Unknown,
}

struct EntryCreateBinding {
    binding: PendingCreateBinding,
    stage: EntryCreateStage,
    preparation_receipt: Option<OwnedEffectState>,
    reader_demand: Option<EntryReaderDemandReceipt>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryReaderDemandReceipt {
    LegacyTaskAccepted,
    Returned,
}

struct ReplacementState {
    replacement: Option<ReplacementProgress>,
    effects: [OwnedEffectState; 11],
    entry_terminal: Option<EntryTerminal>,
    stage: ReplacementStage,
    executor_nonce: Uuid,
    completion: Option<Result<SessionInfo, String>>,
    publication_observed: bool,
    envelope_released: bool,
    sidecar_released: bool,
    claim_reached: bool,
    claim_released: bool,
    pre_teardown_failure: bool,
    reuse_live_result: Option<Result<SessionInfo, String>>,
    body_result: Option<Result<SessionInfo, String>>,
    entry_create: Option<EntryCreateBinding>,
    retained_reason: Option<String>,
}

impl ReplacementState {
    fn retained_effect_diagnostic(&self) -> String {
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

    fn replacement_terminal_is_verified(&self) -> bool {
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

    fn entry_terminal_is_verified(&self, operation: Uuid) -> bool {
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

    fn prerequisite_attempt(&self) -> Result<Option<Uuid>, ()> {
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
    fn entry_body_terminal(&self, actual: &Result<SessionInfo, String>) -> bool {
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

struct ReplacementOwner {
    changed: Notify,
    operation: Uuid,
    descriptor: OwnedReplacementDescriptor,
    state: Mutex<ReplacementState>,
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
struct OwnedLifecycleEntry {
    owner: Arc<ReplacementOwner>,
    executor_nonce: Uuid,
    authority: Option<crate::commands::session::LifecycleEntryAuthority>,
}

impl OwnedLifecycleEntry {
    async fn release_lease(&mut self) {
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let current = self.owner.state.lock().ok().is_some_and(|state| {
            state.executor_nonce == self.executor_nonce && state.stage == ReplacementStage::Returned
        });
        let authority = self.authority.take();
        drop(permit);
        let released = authority.is_some_and(|authority| authority.release());
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
    fn observe_authority_interruption(&self) {
        if let Ok(mut state) = self.owner.state.lock() {
            if state.executor_nonce == self.executor_nonce {
                state.stage = ReplacementStage::Ambiguous;
                state.retained_reason = Some("lifecycle authority changed before teardown".into());
            }
        }
    }

    fn observe_return(&self, result: &Result<SessionInfo, String>) {
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
struct RetainedReplacements {
    restore_active: Arc<AtomicBool>,
    restore_nonce: Option<Uuid>,
    sealed: bool,
    unavailable: bool,
    entries: HashMap<Uuid, Arc<ReplacementOwner>>,
}

impl RetainedReplacements {
    fn new() -> Self {
        Self::reserve(COORDINATOR_ADMISSION_CAPACITY)
    }

    fn reserve(demand: usize) -> Self {
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

struct CoordinatorInner {
    retained_replacements: Mutex<RetainedReplacements>,
    sender: mpsc::Sender<CoordinatorEnvelope>,
    receiver: Mutex<Option<mpsc::Receiver<CoordinatorEnvelope>>>,
    admission: Arc<Semaphore>,
    create_tickets: Arc<Semaphore>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    phase: AtomicU8,
    blocking_seed_work: Arc<BlockingSeedWorkTracker>,
    critical_keys: Mutex<HashSet<CriticalAdmissionKey>>,
    shutdown: CancellationToken,
    worker: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    retained_workers: Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
    retained_cleanup_tasks:
        Mutex<Vec<tokio::task::JoinHandle<crate::pty::container_backend::ContainerShutdownReport>>>,
    retained_rollback_tasks: Mutex<Vec<tokio::task::JoinHandle<Vec<String>>>>,
    cleanup_pty: Mutex<Option<Weak<Mutex<PtyManager>>>>,
    cleanup_container_backend: Mutex<Option<Weak<ContainerTransportBackend>>>,
}

#[derive(Clone)]
pub struct SelectionCoordinator {
    inner: Arc<CoordinatorInner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionShutdownReport {
    pub persistence_safe: bool,
    pub retained: Vec<String>,
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

    fn with_retained_root(
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

    pub fn start<R: Runtime>(&self, app: AppHandle<R>) -> Result<(), SelectionCoordinatorError> {
        if let Some(pty) = app.try_state::<Arc<Mutex<PtyManager>>>() {
            let container_backend = pty
                .inner()
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?
                .container_backend();
            let mut cleanup_pty = self
                .inner
                .cleanup_pty
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?;
            *cleanup_pty = Some(Arc::downgrade(pty.inner()));
            let mut cleanup_container_backend = self
                .inner
                .cleanup_container_backend
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?;
            *cleanup_container_backend = Some(Arc::downgrade(&container_backend));
        }
        let receiver = self
            .inner
            .receiver
            .lock()
            .map_err(|_| SelectionCoordinatorError::Unavailable)?
            .take()
            .ok_or(SelectionCoordinatorError::Unavailable)?;
        let inner = Arc::clone(&self.inner);
        let handle = tauri::async_runtime::spawn(IN_SELECTION_WORKER.scope((), async move {
            worker_loop(app, inner, receiver).await;
        }));
        let mut worker = self
            .inner
            .worker
            .lock()
            .map_err(|_| SelectionCoordinatorError::Unavailable)?;
        *worker = Some(handle);
        Ok(())
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

    pub(crate) async fn run_blocking_seed_work<F, T>(
        &self,
        work: F,
    ) -> Result<T, BlockingSeedTaskError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let tracker = Arc::clone(&self.inner.blocking_seed_work);
        let id = tracker.register(&self.inner.phase)?;
        let worker_tracker = Arc::clone(&tracker);
        let handle = tokio::task::spawn_blocking(move || {
            if !worker_tracker.begin(id) {
                return BlockingSeedWorkerResult::CanceledBeforeStart;
            }
            let _completion = WorkerCompletionGuard {
                tracker: worker_tracker,
                id,
            };
            BlockingSeedWorkerResult::Completed(work())
        });
        tracker.attach_abort(id, handle.abort_handle());
        let mut waiter = BlockingSeedAwaitGuard {
            tracker,
            id,
            armed: true,
        };
        let joined = handle.await;
        waiter.disarm();
        match joined {
            Ok(BlockingSeedWorkerResult::Completed(result)) => Ok(result),
            Ok(BlockingSeedWorkerResult::CanceledBeforeStart) => {
                Err(BlockingSeedTaskError::CanceledBeforeStart)
            }
            Err(error) if error.is_cancelled() => Err(BlockingSeedTaskError::CanceledBeforeStart),
            Err(error) => Err(BlockingSeedTaskError::Join(error.to_string())),
        }
    }

    fn check_external_submission(&self) -> Result<(), SelectionCoordinatorError> {
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

    fn try_reserve_envelope(
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

    async fn try_response_close<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        registration: &ResponseCloseRegistration,
    ) -> Result<(), SelectionCoordinatorError> {
        let key = CriticalAdmissionKey {
            session_id: registration.member.session_id,
            kind: CriticalAdmissionKind::ResponseClose,
        };
        let critical = {
            let mut keys = self.inner.critical_keys.lock().map_err(|_| {
                registration.diagnostic_poll_outcome(3, "critical_key_lock_error");
                SelectionCoordinatorError::Unavailable
            })?;
            if !keys.insert(key) {
                registration.diagnostic_poll_outcome(4, "duplicate_critical_key");
                return Ok(());
            }
            CriticalAdmissionGuard::new(&self.inner, key)
        };
        let (admission, slot) = self.try_reserve_envelope().map_err(|error| {
            registration.diagnostic_poll_outcome(5, "reserve_envelope_error");
            error
        })?;
        let tracker = app
            .state::<crate::pty::manager::ResponseCloseState>()
            .inner()
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let manager = self.inner.manager.read().await.clone();
        let settings = app.state::<crate::config::settings::SettingsState>();
        let settings = settings.read().await;
        let carrier_permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        if self.restore_is_active() {
            registration.diagnostic_poll_outcome(6, "restore_active");
            return Ok(());
        }
        let mut diagnostic_reason = (7, "ticket_absent");
        let ticket = manager
            .with_current_carrier_snapshot(&carrier_permit, |current| {
                let member = response_registration_row(current, &registration.member).or_else(|| { if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { diagnostic_reason = (8, "current_member_rejected"); } None })?;
                let coordinator = response_registration_row(current, &registration.coordinator).or_else(|| { if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { diagnostic_reason = (9, "current_coordinator_rejected"); } None })?;
                if member.backend_kind != crate::pty::backend::SessionBackendKind::LocalProcess
                    || member.is_root_agent
                    || member.is_coordinator
                    || member.agent_kind.is_none()
                    || !coordinator.is_coordinator
                    || coordinator.is_root_agent
                {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { diagnostic_reason = (10, "role_or_backend_rejected"); }
                    return None;
                }
                if !response_registration_route(&pty, &registration.member, member)
                    || !response_registration_route(&pty, &registration.coordinator, coordinator)
                {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { diagnostic_reason = (11, "member_or_coordinator_route_rejected"); }
                    return None;
                }
                let now = Instant::now();
                tracker.apply_settings(
                    settings.response_close_enabled,
                    settings.response_close_idle_seconds,
                    now,
                );
                tracker.publish_manager_facts(
                    registration.member.clone(),
                    true,
                    crate::session::session::is_working(member),
                    now,
                );
                tracker.publish_manager_facts(
                    registration.coordinator.clone(),
                    false,
                    crate::session::session::is_working(coordinator),
                    now,
                );
                match tracker.select_ticket(member.id, now) {
                    Ok(ticket) => Some(ticket),
                    Err(error) => {
                        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) {
                            diagnostic_reason = match error {
                                crate::pty::manager::ResponseCloseError::Unavailable => (12, "select_ticket_unavailable"),
                                crate::pty::manager::ResponseCloseError::Unknown => (13, "select_ticket_unknown"),
                                crate::pty::manager::ResponseCloseError::Conflict => (14, "select_ticket_conflict"),
                                crate::pty::manager::ResponseCloseError::ClosingDeferred => (15, "select_ticket_closing_deferred"),
                            };
                        }
                        None
                    }
                }
            })
            .await;
        drop(carrier_permit);
        let Some(ticket) = ticket else {
            registration.diagnostic_poll_outcome(diagnostic_reason.0, diagnostic_reason.1);
            return Ok(());
        };
        if ticket.committed_binding().member != registration.member {
            registration.diagnostic_poll_outcome(16, "committed_binding_mismatch");
            return Ok(());
        }
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::ConditionalResponseClose {
                request: ConditionalResponseClose { ticket },
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(critical),
        }));
        registration.diagnostic_poll_outcome(17, "envelope_submitted");
        Ok(())
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

    pub(crate) async fn restart_lifecycle<R: Runtime>(
        &self,
        app: AppHandle<R>,
        request: crate::commands::session::RestartJobRequest,
    ) -> Result<SessionInfo, String> {
        self.submit_lifecycle_entry(app, OwnedReplacementDescriptor::EntryRestart(request))
            .await
    }

    pub(crate) async fn root_lifecycle<R: Runtime>(
        &self,
        app: AppHandle<R>,
        request: crate::commands::session::RootJobRequest,
    ) -> Result<SessionInfo, String> {
        self.submit_lifecycle_entry(app, OwnedReplacementDescriptor::EntryRoot(request))
            .await
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

    fn insert_lifecycle_entry(&self, owner: &Arc<ReplacementOwner>) -> Result<(), String> {
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

    fn prepare_lifecycle_owner(
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

    async fn submit_lifecycle_entry<R: Runtime>(
        &self,
        app: AppHandle<R>,
        descriptor: OwnedReplacementDescriptor,
    ) -> Result<SessionInfo, String> {
        // Original real capacity probe precedes acceptance and validation.
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (owner, executor_nonce) = Self::prepare_lifecycle_owner(descriptor);
        let (publication, result) = oneshot::channel();
        self.insert_lifecycle_entry(&owner)?;
        drop(slot);
        drop(admission);
        let coordinator = self.clone();
        crate::config::sessions_persistence::run_owned_persist_core(async move {
            let execution = coordinator
                .execute_accepted_entry(&app, owner.clone(), executor_nonce)
                .await;
            coordinator
                .finish_lifecycle_entry(&owner, executor_nonce, execution, publication)
                .await;
        })
        .await;
        result
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    async fn execute_accepted_entry<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        owner: Arc<ReplacementOwner>,
        executor_nonce: Uuid,
    ) -> (Result<SessionInfo, String>, bool) {
        if let Err(error) = self.wait_for_restore_release().await {
            return (Err(error), true);
        }
        if matches!(&owner.descriptor, OwnedReplacementDescriptor::EntryRoot(_)) {
            let effect = {
                owner.state.lock().ok().and_then(|mut state| {
                    if state.executor_nonce != executor_nonce
                        || state.stage != ReplacementStage::EntryWaiting
                    {
                        return None;
                    }
                    let attempt = Uuid::new_v4();
                    state.effects[LifecycleEffectSlot::RootDirectory as usize] =
                        OwnedEffectState::Running(attempt);
                    Some(LifecycleEffectGuard {
                        owner: owner.clone(),
                        slot: LifecycleEffectSlot::RootDirectory,
                        attempt,
                        completed: false,
                    })
                })
            };
            let Some(effect) = effect else {
                return std::future::pending().await;
            };
            if let Err(error) = crate::commands::session::ensure_owned_root_directory(effect) {
                return (Err(error), true);
            }
        }
        let mut root_directory = owner.state.lock().ok().and_then(|state| {
            match &state.effects[LifecycleEffectSlot::RootDirectory as usize] {
                OwnedEffectState::Completed {
                    outcome: Ok(Some(path)),
                    ..
                } => Some(path.clone()),
                _ => None,
            }
        });
        let (requested, root_request) = match &owner.descriptor {
            OwnedReplacementDescriptor::EntryRestart(request) => (Some(request.session_id), false),
            OwnedReplacementDescriptor::EntryRoot(_) => (None, true),
            OwnedReplacementDescriptor::Replacement(_) => return std::future::pending().await,
        };
        let authority = loop {
            if let Err(error) = self.wait_for_restore_release().await {
                return (Err(error), true);
            }
            if self.inner.shutdown.is_cancelled() {
                return (
                    Err(SelectionCoordinatorError::Unavailable.to_string()),
                    true,
                );
            }
            let admission = tokio::select! {
                biased;
                _ = self.inner.shutdown.cancelled() => {
                    return (Err(SelectionCoordinatorError::Unavailable.to_string()), true);
                }
                admission = crate::commands::session::prepare_lifecycle_entry_authority(
                    app, owner.operation, requested, root_request, root_directory.as_deref(),
                ) => admission,
            };
            match admission {
                Ok(crate::commands::session::LifecycleAuthorityAdmission::Ready(authority)) => {
                    break authority
                }
                Ok(crate::commands::session::LifecycleAuthorityAdmission::Waiting) => {
                    // Every failed probe has dropped its own stripe/exact/claim.
                    tokio::task::yield_now().await;
                }
                Ok(crate::commands::session::LifecycleAuthorityAdmission::NeedsRootDirectory) => {
                    let effect = owner.state.lock().ok().and_then(|mut state| {
                        if state.executor_nonce != executor_nonce
                            || !matches!(
                                state.effects[LifecycleEffectSlot::RootDirectory as usize],
                                OwnedEffectState::NotStarted
                            )
                        {
                            return None;
                        }
                        let attempt = Uuid::new_v4();
                        state.effects[LifecycleEffectSlot::RootDirectory as usize] =
                            OwnedEffectState::Running(attempt);
                        Some(LifecycleEffectGuard {
                            owner: owner.clone(),
                            slot: LifecycleEffectSlot::RootDirectory,
                            attempt,
                            completed: false,
                        })
                    });
                    let Some(effect) = effect else {
                        return std::future::pending().await;
                    };
                    match crate::commands::session::ensure_owned_root_directory(effect) {
                        Ok(path) => root_directory = Some(path),
                        Err(error) => return (Err(error), true),
                    }
                }
                Err(error) => return (Err(error), true),
            }
        };
        if let Ok(mut state) = owner.state.lock() {
            state.claim_reached = authority.has_owned_claim();
        }
        let (admission, slot) = match self.try_reserve_envelope() {
            Ok(capacity) => capacity,
            Err(error) => {
                let released = authority.release();
                if let Ok(mut state) = owner.state.lock() {
                    state.sidecar_released = released;
                }
                return (Err(error.to_string()), true);
            }
        };
        let (response, receiver) = oneshot::channel();
        let entry = OwnedLifecycleEntry {
            owner: owner.clone(),
            executor_nonce,
            authority: Some(authority),
        };
        if let Ok(mut state) = owner.state.lock() {
            state.stage = ReplacementStage::Queued;
        }
        let job = match &owner.descriptor {
            OwnedReplacementDescriptor::EntryRestart(request) => CoordinatorJob::RestartLifecycle {
                request: request.clone(),
                entry,
                response,
            },
            OwnedReplacementDescriptor::EntryRoot(request) => CoordinatorJob::RootLifecycle {
                request: request.clone(),
                entry,
                response,
            },
            OwnedReplacementDescriptor::Replacement(_) => return std::future::pending().await,
        };
        drop(slot.send(CoordinatorEnvelope {
            job,
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        match receiver.await {
            Ok(result) => (result, false),
            Err(_) => {
                // Lost internal outcome has no public fabricated result or retry.
                if let Ok(mut state) = owner.state.lock() {
                    state.stage = ReplacementStage::Ambiguous;
                    state
                        .retained_reason
                        .get_or_insert_with(|| "lifecycle internal result lost".into());
                }
                std::future::pending().await
            }
        }
    }

    async fn finish_lifecycle_entry(
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

    fn seal_retained_entries(&self) {
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

    fn inspect_retained_entries(&self, retained: &mut Vec<String>) {
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

    fn remove_terminal_entry(&self, owner: &Arc<ReplacementOwner>) {
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
    pub(crate) async fn submit_inline_replacement_fixture(
        &self,
        input: crate::commands::session::FixtureReplacementInput,
        control: Arc<ReplacementFixtureControl>,
    ) -> Option<crate::commands::session::FixtureReplacementResult> {
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
                let outcome = match &result.outcome {
                    Ok(crate::commands::session::OwnedReplacementCreateStep::Finalized(_)) => {
                        ReplacementFixtureOutcome::Finalized
                    }
                    Ok(crate::commands::session::OwnedReplacementCreateStep::Retained) => {
                        ReplacementFixtureOutcome::Retained
                    }
                    Err(_) => ReplacementFixtureOutcome::Error,
                };
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

    async fn reserve_create_internal(
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
            .map_err(|_| SelectionCoordinatorError::Busy.to_string())?;
        let _phase_guard = RestoreSubmissionPhaseGuard {
            phase: &self.inner.phase,
        };
        let carrier_permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        match crate::config::sessions_persistence::restore_carrier_scopes_are_empty(&carrier_permit)
        {
            Some(true) => {}
            Some(false) => return Err(SelectionCoordinatorError::Busy.to_string()),
            None => return Err(SelectionCoordinatorError::Unavailable.to_string()),
        }
        let admission = match Arc::clone(&self.inner.admission).try_acquire_owned() {
            Ok(admission) => admission,
            Err(_) => {
                return Err(SelectionCoordinatorError::Busy.to_string());
            }
        };
        let slot = match self.inner.sender.clone().try_reserve_owned() {
            Ok(slot) => slot,
            Err(_) => {
                return Err(SelectionCoordinatorError::Busy.to_string());
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

    async fn reserve_critical_admission(
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

    async fn submit_route_loss(
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
    ) -> Result<CriticalAdmissionOutcome<crate::commands::session::DestroyOutcome>, String> {
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
                request: crate::commands::session::DestroyRequest {
                    ids: vec![session_id],
                    source: crate::commands::session::DestructionSource::BackgroundCleanup,
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
    async fn critical_probe_for_test(
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
        ContainerLifecycleSender {
            inner: Arc::downgrade(&self.inner),
        }
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

    async fn close_and_join_with_budget(&self, budget: Duration) -> SelectionShutdownReport {
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
                        "[selection] coordinator worker phase exhausted before shared deadline budget={:?} finalizationReserve={:?} outstandingCriticalKeys={}",
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
                                "[selection] aborted coordinator worker retained after absolute deadline state=retained"
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

pub struct RestoreBarrierGuard {
    coordinator: Weak<CoordinatorInner>,
    nonce: Uuid,
    release: Option<oneshot::Sender<()>>,
}

pub(crate) struct AutoCloseBatchTicket {
    slot: Option<mpsc::OwnedPermit<CoordinatorEnvelope>>,
    admission: Option<OwnedSemaphorePermit>,
}

impl AutoCloseBatchTicket {
    pub async fn finalize(
        mut self,
        ids: Vec<Uuid>,
    ) -> Result<crate::commands::session::DestroyOutcome, String> {
        let slot = self
            .slot
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let admission = self
            .admission
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Destroy {
                request: crate::commands::session::DestroyRequest {
                    ids,
                    source: crate::commands::session::DestructionSource::AutoClose,
                    force_destroy_root: false,
                },
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
}

impl RestoreBarrierGuard {
    pub(crate) fn transaction<R: Runtime>(&self, app: AppHandle<R>) -> SelectionTransaction<R> {
        let mut transaction = SelectionTransaction::new(app, self.coordinator.clone());
        transaction.restore_nonce = Some(self.nonce);
        transaction
    }

    fn release_worker(&mut self) {
        if let Some(release) = self.release.take() {
            if release.send(()).is_err() {
                log::warn!("[selection] restore barrier worker ended before completion signal");
            }
        }
    }

    pub fn finish(mut self) {
        self.release_worker();
    }
}

impl Drop for RestoreBarrierGuard {
    fn drop(&mut self) {
        if self.release.is_some() {
            log::warn!("[selection] restore barrier released by dropped owner");
            self.release_worker();
        }
    }
}

pub struct CreateFinalizationTicket {
    owned_replacement: Option<OwnedReplacementContinuation>,
    slot: Option<mpsc::OwnedPermit<CoordinatorEnvelope>>,
    admission: Option<OwnedSemaphorePermit>,
    create_ticket: Option<OwnedSemaphorePermit>,
    binding: Option<PendingCreateBinding>,
    cause: SelectionCause,
    auto_select_precondition: Option<(String, u64)>,
    completed: bool,
}

impl fmt::Debug for CreateFinalizationTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateFinalizationTicket")
            .field("binding", &self.binding)
            .field("auto_select_precondition", &self.auto_select_precondition)
            .field("completed", &self.completed)
            .finish_non_exhaustive()
    }
}

impl CreateFinalizationTicket {
    pub(crate) fn retain_owned_replacement(
        &mut self,
        continuation: OwnedReplacementContinuation,
    ) -> Result<(), String> {
        self.check_unbound()?;
        if self.owned_replacement.is_some() {
            return Err("create finalization ticket was already bound".to_string());
        }
        self.auto_select_precondition = None;
        self.owned_replacement = Some(continuation);
        Ok(())
    }
    /// Keep the original selection precondition and binding metadata, while a
    /// blocked accepted create releases scarce finalizer capacity.
    pub(crate) fn release_capacity_for_wait(&mut self) -> bool {
        drop(self.slot.take());
        drop(self.admission.take());
        drop(self.create_ticket.take());
        !self.completed && self.binding.is_none()
    }

    pub(crate) fn resume_unbound_capacity(
        &mut self,
        coordinator: &SelectionCoordinator,
    ) -> Result<(), SelectionCoordinatorError> {
        if self.slot.is_some() && self.admission.is_some() && self.create_ticket.is_some() {
            return Ok(());
        }
        if self.completed
            || self.binding.is_some()
            || self.slot.is_some()
            || self.admission.is_some()
            || self.create_ticket.is_some()
        {
            return Err(SelectionCoordinatorError::Unavailable);
        }
        coordinator.check_external_submission()?;
        let create_ticket = coordinator
            .inner
            .create_tickets
            .clone()
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let admission = coordinator
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let slot = coordinator
            .inner
            .sender
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Closed(_) => SelectionCoordinatorError::Unavailable,
                mpsc::error::TrySendError::Full(_) => SelectionCoordinatorError::Busy,
            })?;
        self.slot = Some(slot);
        self.admission = Some(admission);
        self.create_ticket = Some(create_ticket);
        Ok(())
    }

    fn check_unbound(&self) -> Result<(), String> {
        if self.completed || self.binding.is_some() {
            return Err("create finalization ticket was already bound".to_string());
        }
        Ok(())
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(super) fn bind(&mut self, session_id: Uuid) -> Result<PendingCreateBinding, String> {
        self.check_unbound()?;
        self.bind_prepared(PendingCreateBinding::new(session_id, Uuid::new_v4()))
    }

    pub(crate) fn bind_prepared(
        &mut self,
        binding: PendingCreateBinding,
    ) -> Result<PendingCreateBinding, String> {
        self.check_unbound()?;
        self.binding = Some(binding);
        Ok(binding)
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) fn binding(&self) -> Option<PendingCreateBinding> {
        self.binding
    }

    pub async fn finalize(self, warnings: Vec<SessionWarning>) -> Result<SessionInfo, String> {
        match self.finalize_with_owned_outcome(warnings).await {
            Some(result) => result,
            None => std::future::pending().await,
        }
    }

    pub(crate) async fn finalize_with_owned_outcome(
        mut self,
        warnings: Vec<SessionWarning>,
    ) -> Option<Result<SessionInfo, String>> {
        let owned = self.owned_replacement.is_some();
        #[cfg(test)]
        let fixture_owned = self.owned_replacement.clone();
        let queued = (|| {
            let slot = self
                .slot
                .take()
                .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
            let admission = self
                .admission
                .take()
                .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
            let create_ticket = self
                .create_ticket
                .take()
                .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
            let binding = self
                .binding
                .take()
                .ok_or_else(|| "create finalization ticket was not bound".to_string())?;
            let (response, receiver) = oneshot::channel();
            drop(slot.send(CoordinatorEnvelope {
                job: CoordinatorJob::FinalizeCreate {
                    request: FinalizeCreateRequest {
                        owned_replacement: self.owned_replacement.take(),
                        binding,
                        cause: self.cause,
                        auto_select_precondition: self.auto_select_precondition.take(),
                        warnings,
                    },
                    response,
                },
                _admission: admission,
                _create_ticket: Some(create_ticket),
                _critical_admission: None,
            }));
            self.completed = true;
            Ok::<_, String>(receiver)
        })();
        let receiver = match queued {
            Ok(receiver) => receiver,
            Err(error) => return Some(Err(error)),
        };
        #[cfg(test)]
        if let Some(continuation) = fixture_owned.as_ref() {
            if let Some(control) = continuation.fixture.as_ref() {
                control.record(continuation, ReplacementFixtureCut::TicketSubmitted);
                return control.receive(continuation, receiver).await;
            }
        }
        match receiver.await {
            Ok(result) => Some(result),
            Err(_) if owned => None,
            Err(_) => Some(Err(SelectionCoordinatorError::Unavailable.to_string())),
        }
    }
}

impl Drop for CreateFinalizationTicket {
    fn drop(&mut self) {
        // A dropped waiter/task is never explicit stopped-executor authority.
        // The retained owner, including finalized B, survives this ticket.
        if self.owned_replacement.is_some() {
            return;
        }

        if self.completed {
            return;
        }
        let Some(binding) = self.binding.take() else {
            return;
        };
        let (Some(slot), Some(admission), Some(create_ticket)) = (
            self.slot.take(),
            self.admission.take(),
            self.create_ticket.take(),
        ) else {
            log::error!(
                "[selection] bound create ticket dropped without its reserved finalization capacity session={}",
                binding.session_id()
            );
            return;
        };
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RollbackCreate { binding },
            _admission: admission,
            _create_ticket: Some(create_ticket),
            _critical_admission: None,
        }));
        self.completed = true;
    }
}

#[derive(Clone)]
pub struct ContainerLifecycleSender {
    inner: Weak<CoordinatorInner>,
}

impl ContainerLifecycleSender {
    pub async fn route_lost(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Result<CriticalAdmissionOutcome<()>, String> {
        let inner = self
            .inner
            .upgrade()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        SelectionCoordinator { inner }
            .submit_route_loss(session_id, exit_code)
            .await
    }
}

pub(crate) struct SelectionTransaction<R: Runtime> {
    app: AppHandle<R>,
    capability: CommitCapability,
    lifecycle_entry: Option<(Arc<ReplacementOwner>, Uuid)>,
    coordinator: Weak<CoordinatorInner>,
    restore_nonce: Option<Uuid>,
}

impl<R: Runtime> Clone for SelectionTransaction<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            capability: self.capability.clone(),
            lifecycle_entry: self.lifecycle_entry.clone(),
            coordinator: self.coordinator.clone(),
            restore_nonce: self.restore_nonce,
        }
    }
}

#[derive(Clone)]
pub(super) struct CommitCapability {
    _private: (),
}

#[cfg(test)]
impl CommitCapability {
    pub(super) fn for_test() -> Self {
        Self { _private: () }
    }
}

impl<R: Runtime> SelectionTransaction<R> {
    fn new(app: AppHandle<R>, coordinator: Weak<CoordinatorInner>) -> Self {
        Self {
            app,
            capability: CommitCapability { _private: () },
            lifecycle_entry: None,
            coordinator,
            restore_nonce: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(app: AppHandle<R>) -> Self {
        Self {
            app,
            capability: CommitCapability::for_test(),
            lifecycle_entry: None,
            coordinator: Weak::new(),
            restore_nonce: None,
        }
    }

    fn for_lifecycle_entry(&self, entry: &OwnedLifecycleEntry) -> Self {
        let mut transaction = self.clone();
        transaction.lifecycle_entry = Some((entry.owner.clone(), entry.executor_nonce));
        if let Ok(mut state) = entry.owner.state.lock() {
            if state.executor_nonce == entry.executor_nonce
                && state.stage == ReplacementStage::Queued
            {
                state.stage = ReplacementStage::Executing;
            }
        }
        transaction
    }

    /// Only the original exact-UUID lookup failure calls this before any
    /// teardown/create/native prerequisite. No error string implies this fact.
    pub(crate) fn observe_lifecycle_missing_source(&self) {
        let Some((owner, nonce)) = &self.lifecycle_entry else {
            return;
        };
        if let Ok(mut state) = owner.state.lock() {
            if state.executor_nonce == *nonce
                && state.stage == ReplacementStage::Executing
                && state
                    .effects
                    .iter()
                    .all(|effect| matches!(effect, OwnedEffectState::NotStarted))
            {
                state.pre_teardown_failure = true;
            }
        }
    }

    /// The original resolver/probe/native preparation returned its observed
    /// error before retirement or pending-runtime creation. Native residues stay.
    pub(crate) fn observe_lifecycle_preparation_failure(&self) {
        let Some((owner, nonce)) = &self.lifecycle_entry else {
            return;
        };
        if let Ok(mut state) = owner.state.lock() {
            let context_failed = matches!(
                &state.effects[LifecycleEffectSlot::ContextPreparation as usize],
                OwnedEffectState::Completed {
                    outcome: Err(_),
                    ..
                }
            );
            let no_later_effect = state.effects.iter().enumerate().all(|(index, effect)| {
                if index == LifecycleEffectSlot::RootDirectory as usize
                    || index == LifecycleEffectSlot::ContextPreparation as usize
                {
                    matches!(
                        effect,
                        OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }
                    )
                } else {
                    matches!(effect, OwnedEffectState::NotStarted)
                }
            });
            if state.executor_nonce == *nonce
                && state.stage == ReplacementStage::Executing
                && context_failed
                && no_later_effect
            {
                state.pre_teardown_failure = true;
            }
        }
    }

    /// Only the actual ReuseLive return can certify this path. Restart/create
    /// paths never call this and still require their own reached-effect proof.
    pub(crate) fn observe_reuse_live_result(&self, result: &Result<SessionInfo, String>) {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return;
        };
        let Ok(mut state) = owner.state.lock() else {
            return;
        };
        if state.executor_nonce == *nonce
            && state.stage == ReplacementStage::Executing
            && matches!(&owner.descriptor, OwnedReplacementDescriptor::EntryRoot(_))
            && state.reuse_live_result.is_none()
            && state.entry_create.is_none()
            && state.effects.iter().enumerate().all(|(index, effect)| {
                if index == LifecycleEffectSlot::RootDirectory as usize
                    || index == LifecycleEffectSlot::Publication as usize
                {
                    matches!(effect, OwnedEffectState::Completed { .. })
                } else {
                    matches!(effect, OwnedEffectState::NotStarted)
                }
            })
        {
            state.reuse_live_result = Some(result.clone());
        }
    }

    pub(crate) fn retain_entry_create_binding(&self, binding: PendingCreateBinding) -> bool {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return true;
        };
        let Ok(mut state) = owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != *nonce || state.stage != ReplacementStage::Executing {
            return false;
        }
        if state.entry_create.is_some() {
            return false;
        }
        let preparation_receipt =
            match &state.effects[LifecycleEffectSlot::ContextPreparation as usize] {
                OwnedEffectState::NotStarted => None,
                receipt @ OwnedEffectState::Completed { .. } => Some(receipt.clone()),
                _ => return false,
            };
        state.effects[LifecycleEffectSlot::ContextPreparation as usize] =
            OwnedEffectState::NotStarted;
        state.entry_create = Some(EntryCreateBinding {
            binding,
            stage: EntryCreateStage::Reserved,
            preparation_receipt,
            reader_demand: None,
        });
        true
    }

    pub(crate) fn observe_entry_reader_demand(
        &self,
        binding: PendingCreateBinding,
        receipt: EntryReaderDemandReceipt,
    ) -> bool {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return true;
        };
        let Ok(mut state) = owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != *nonce || state.stage != ReplacementStage::Executing {
            return false;
        }
        let Some(create) = state.entry_create.as_mut() else {
            return false;
        };
        if create.binding != binding
            || create.stage != EntryCreateStage::Finalized
            || create.reader_demand.is_some()
        {
            return false;
        }
        create.reader_demand = Some(receipt);
        true
    }

    pub(crate) fn observe_entry_pending_binding(&self, binding: PendingCreateBinding) -> bool {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return true;
        };
        let Ok(mut state) = owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != *nonce || state.stage != ReplacementStage::Executing {
            return false;
        }
        let Some(create) = state.entry_create.as_mut() else {
            return false;
        };
        if create.binding != binding || create.stage != EntryCreateStage::Reserved {
            return false;
        }
        create.stage = EntryCreateStage::Pending;
        true
    }

    /// The actual commit and synchronous disarm share U. A lost result after
    /// this transition never turns finalized B into pending cleanup authority.
    pub(crate) async fn commit_entry_create(
        &self,
        binding: PendingCreateBinding,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        let Some((owner, nonce)) = self.lifecycle_entry.as_ref() else {
            return self.commit(decision, cause, mutations).await;
        };
        let manager = self.manager().await;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let authorized = owner.state.lock().ok().is_some_and(|state| {
            state.executor_nonce == *nonce
                && state.stage == ReplacementStage::Executing
                && state.entry_create.as_ref().is_some_and(|create| {
                    create.binding == binding && create.stage == EntryCreateStage::Pending
                })
        });
        if !authorized {
            return Err(SelectionCoordinatorError::Unavailable.to_string());
        }
        let committed = manager
            .commit_selection_transition_with_carrier_permit(
                &permit,
                &self.capability,
                decision,
                cause,
                mutations,
            )
            .await?;
        // No await/fallible exit between real manager commit and disarm.
        let disarmed = match owner.state.lock() {
            Ok(mut state) => {
                let same_executor = state.executor_nonce == *nonce;
                match state.entry_create.as_mut() {
                    Some(create)
                        if same_executor
                            && create.binding == binding
                            && create.stage == EntryCreateStage::Pending =>
                    {
                        create.stage = EntryCreateStage::Finalized;
                        true
                    }
                    Some(create) => {
                        create.stage = EntryCreateStage::Unknown;
                        state.stage = ReplacementStage::Ambiguous;
                        false
                    }
                    None => {
                        state.stage = ReplacementStage::Ambiguous;
                        false
                    }
                }
            }
            Err(_) => false,
        };
        drop(permit);
        if !disarmed {
            // Actual B committed: never turn missing owner bookkeeping into an
            // ordinary Err which could trigger pending-runtime cleanup.
            return std::future::pending().await;
        }
        crate::config::activity_log::append_batch(&committed.activity);
        Ok(committed)
    }

    pub(crate) fn has_lifecycle_entry(&self) -> bool {
        self.lifecycle_entry.is_some()
    }

    pub(crate) fn start_lifecycle_effect(
        &self,
        slot: LifecycleEffectSlot,
    ) -> Option<LifecycleEffectGuard> {
        let (owner, nonce) = self.lifecycle_entry.as_ref()?;
        let mut state = owner.state.lock().ok()?;
        if state.executor_nonce != *nonce
            || state.stage != ReplacementStage::Executing
            || !matches!(state.effects[slot as usize], OwnedEffectState::NotStarted)
        {
            state.stage = ReplacementStage::Ambiguous;
            return None;
        }
        let attempt = Uuid::new_v4();
        state.effects[slot as usize] = OwnedEffectState::Running(attempt);
        Some(LifecycleEffectGuard {
            owner: owner.clone(),
            slot,
            attempt,
            completed: false,
        })
    }

    pub(crate) fn lifecycle_root_directory_outcome(&self) -> Option<Result<String, String>> {
        let (owner, nonce) = self.lifecycle_entry.as_ref()?;
        let state = owner.state.lock().ok()?;
        if state.executor_nonce != *nonce {
            return None;
        }
        match &state.effects[LifecycleEffectSlot::RootDirectory as usize] {
            OwnedEffectState::Completed {
                outcome: Ok(Some(path)),
                ..
            } => Some(Ok(path.clone())),
            OwnedEffectState::Completed {
                outcome: Err(error),
                ..
            } => Some(Err(error.clone())),
            _ => None,
        }
    }

    pub(crate) fn has_accepted_lifecycle_core(&self) -> bool {
        self.lifecycle_entry.as_ref().is_some_and(|(owner, nonce)| {
            owner.state.lock().ok().is_some_and(|state| {
                state.executor_nonce == *nonce && state.stage == ReplacementStage::Executing
            })
        })
    }

    pub(crate) fn current_restore_nonce(&self) -> Option<Uuid> {
        self.has_active_restore_capability()
            .then_some(self.restore_nonce)
            .flatten()
    }

    pub(crate) fn has_restore_context(&self) -> bool {
        self.restore_nonce.is_some()
    }

    /// Contradictory mid-effect authority invalidates only this real restore
    /// capability. Keep its actual barrier active until the existing owner
    /// releases it, and latch persistence unsafe rather than resetting history.
    pub(crate) fn invalidate_restore_ownership(&self) {
        let (Some(nonce), Some(inner)) = (self.restore_nonce, self.coordinator.upgrade()) else {
            return;
        };
        if let Ok(mut root) = inner.retained_replacements.lock() {
            if root.restore_nonce == Some(nonce) && root.restore_active.load(Ordering::Acquire) {
                root.unavailable = true;
            }
        };
    }

    pub(crate) fn has_active_restore_capability(&self) -> bool {
        let Some(nonce) = self.restore_nonce else {
            return false;
        };
        let Some(inner) = self.coordinator.upgrade() else {
            return false;
        };
        let Ok(root) = inner.retained_replacements.lock() else {
            return false;
        };
        !root.unavailable
            && root.restore_nonce == Some(nonce)
            && root.restore_active.load(Ordering::Acquire)
            && CoordinatorPhase::from_u8(inner.phase.load(Ordering::Acquire))
                == CoordinatorPhase::Running
            && !inner.shutdown.is_cancelled()
    }

    fn begin_restore(&self, nonce: Uuid) -> bool {
        let Some(inner) = self.coordinator.upgrade() else {
            return false;
        };
        let Ok(mut root) = inner.retained_replacements.lock() else {
            return false;
        };
        if !root.restore_active.load(Ordering::Acquire)
            || root.restore_nonce.is_some()
            || CoordinatorPhase::from_u8(inner.phase.load(Ordering::Acquire))
                == CoordinatorPhase::Closing
        {
            return false;
        }
        root.restore_nonce = Some(nonce);
        true
    }

    fn release_restore(&self, nonce: Uuid) {
        let Some(inner) = self.coordinator.upgrade() else {
            return;
        };
        let Ok(mut root) = inner.retained_replacements.lock() else {
            return;
        };
        if root.restore_nonce == Some(nonce) {
            root.restore_nonce = None;
            root.restore_active.store(false, Ordering::Release);
        }
    }

    pub(crate) fn app(&self) -> &AppHandle<R> {
        &self.app
    }

    pub(crate) async fn manager(&self) -> SessionManager {
        self.app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone()
    }

    /// Uses the caller's preallocated binding and the current selection turn.
    /// This reserves no queue slot and performs no manager access.
    pub(crate) fn prepare_inline_create_binding(
        &self,
        binding: PendingCreateBinding,
        reserved: &mut Option<PendingCreateBinding>,
    ) -> Result<PendingCreateBinding, String> {
        if reserved.is_some() {
            return Err("inline create binding was already prepared".to_string());
        }
        *reserved = Some(binding);
        Ok(binding)
    }

    #[allow(clippy::too_many_arguments)]
    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn create_pending_session(
        &self,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<crate::session::session::SessionRepo>,
        is_coordinator: bool,
        backend_kind: crate::pty::backend::SessionBackendKind,
    ) -> Result<(crate::session::session::Session, PendingCreateBinding), String> {
        self.manager()
            .await
            .create_transaction_pending_session(
                shell,
                shell_args,
                working_directory,
                agent_id,
                agent_label,
                git_repos,
                is_coordinator,
                backend_kind,
            )
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn finalize_inline_create(
        &self,
        binding: PendingCreateBinding,
        cause: SelectionCause,
        warnings: Vec<SessionWarning>,
        owned_replacement: Option<OwnedReplacementContinuation>,
    ) -> Option<Result<SessionInfo, String>> {
        let request = FinalizeCreateRequest {
            owned_replacement,
            binding,
            cause,
            auto_select_precondition: None,
            warnings,
        };
        if let Some(owned) = request.owned_replacement.clone() {
            let result = execute_owned_finalize_create(self, request).await;
            if let Some(result) = result.as_ref() {
                if let Ok(mut state) = owned.owner.state.lock() {
                    state.completion = Some(result.clone());
                    state.publication_observed = true;
                    state.envelope_released = true;
                }
                owned.owner.changed.notify_waiters();
            }
            result
        } else {
            Some(execute_finalize_create(self, request).await)
        }
    }

    pub(crate) async fn rollback_inline_create(&self, binding: PendingCreateBinding) {
        execute_rollback_create(self, binding).await;
    }

    pub(crate) async fn reconcile_route_loss_inline(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Result<(), String> {
        execute_route_loss(self, session_id, exit_code).await
    }

    pub(crate) async fn restore_dormant_inline(
        &self,
        request: DormantRestoreRequest,
    ) -> Result<SessionInfo, String> {
        execute_restore_dormant(self, request).await
    }

    pub(crate) async fn restore_selection_inline(
        &self,
        persisted_target: Option<Uuid>,
    ) -> Result<Option<SessionSelection>, String> {
        execute_transition(self, SelectionRequest::restore(persisted_target)).await
    }

    pub(crate) async fn aggregate_snapshot(&self) -> ManagerAggregateSnapshot {
        self.manager().await.aggregate_snapshot().await
    }

    pub(crate) fn runtime_snapshot(&self, session_id: Uuid) -> RuntimeSelectionSnapshot {
        let detached = self
            .app
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&session_id);
        let has_pty = self
            .app
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(session_id);
        RuntimeSelectionSnapshot {
            session_id,
            has_pty,
            detached,
        }
    }

    pub(crate) fn live_decision(&self, session_id: Uuid) -> Option<CommitDecision> {
        LiveRuntimeWitness::from_snapshot(self.runtime_snapshot(session_id))
            .map(CommitDecision::Live)
    }

    pub(crate) fn dormant_decision(&self, session_id: Uuid) -> Option<CommitDecision> {
        DormantRuntimeWitness::from_snapshot(self.runtime_snapshot(session_id))
            .map(CommitDecision::Dormant)
    }

    pub(crate) async fn commit(
        &self,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        // #1149 - the single funnel that returns a `CommitResult` outward, so the
        // only place the birth-edge records can be appended.
        //
        // The `let` binding is load-bearing. As a tail expression the temporary
        // `SessionManager` would live across the append. It is harmless today
        // only because `manager()` returns by value, cloning out of the outer
        // lock and releasing that guard before it returns, so `append_batch`
        // holds neither the outer nor the inner lock. That stops being true if
        // `manager()` is ever changed to return a guard.
        //
        // The `?` means an `Err` returns before the append and drops `result`
        // whole, so no record survives for a session that was never finalized.
        let result = self
            .manager()
            .await
            .commit_selection_transition(&self.capability, decision, cause, mutations)
            .await?;
        crate::config::activity_log::append_batch(&result.activity);
        Ok(result)
    }

    pub(crate) async fn commit_response_close_retained(
        &self,
        id: Uuid,
    ) -> Result<CommitResult, String> {
        let manager = self.manager().await;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let decision = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if current.selection.id() == Some(id) {
                    CommitDecision::Clear
                } else {
                    CommitDecision::Keep
                }
            })
            .await;
        let mut mutations = LifecycleMutations::default();
        mutations.set_detached_intent(id, false);
        mutations.mark_exited(id, 0);
        let result = manager
            .commit_selection_transition_with_carrier_permit(
                &permit,
                &self.capability,
                decision,
                SelectionCause::BackgroundCleanup,
                mutations,
            )
            .await?;
        drop(permit);
        crate::config::activity_log::append_batch(&result.activity);
        Ok(result)
    }

    pub(crate) async fn persist(&self, source: SelectionSource, session_id: Option<Uuid>) {
        let manager = self.manager().await;
        if let Err(error) = persist_current_state_result(&manager).await {
            log::warn!(
                "[selection] persistence failed source={} session={:?}: {}",
                source,
                session_id,
                error
            );
        }
    }

    pub(crate) fn publish_selection(&self, payload: &SessionSelection) {
        publish_selection(&self.app, payload);
    }

    pub(crate) fn publish_created(&self, info: &SessionInfo) {
        publish_lifecycle_event(&self.app, "session_created", info);
    }

    pub(crate) fn publish_destroyed(&self, session_id: Uuid) {
        publish_lifecycle_event(
            &self.app,
            "session_destroyed",
            &serde_json::json!({ "id": session_id.to_string() }),
        );
    }

    pub(crate) fn publish_view_requested(&self, session_id: Uuid) {
        publish_lifecycle_event(
            &self.app,
            "session_view_requested",
            &serde_json::json!({ "id": session_id.to_string() }),
        );
    }

    pub(crate) fn publish_communication_cleared(&self, session_id: Uuid) {
        publish_lifecycle_event(
            &self.app,
            "session_communication_changed",
            &serde_json::json!({
                "sessionId": session_id.to_string(),
                "communication": null,
            }),
        );
    }
}

async fn worker_loop<R: Runtime>(
    app: AppHandle<R>,
    inner: Arc<CoordinatorInner>,
    mut receiver: mpsc::Receiver<CoordinatorEnvelope>,
) {
    let transaction = SelectionTransaction::new(app, Arc::downgrade(&inner));
    loop {
        let envelope = tokio::select! {
            biased;
            _ = inner.shutdown.cancelled() => {
                receiver.close();
                drain_after_shutdown(&transaction, &mut receiver).await;
                break;
            }
            envelope = receiver.recv() => envelope,
        };
        let Some(envelope) = envelope else {
            break;
        };
        execute_envelope(&transaction, envelope).await;
    }
}

async fn drain_after_shutdown<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    receiver: &mut mpsc::Receiver<CoordinatorEnvelope>,
) {
    while let Some(envelope) = receiver.recv().await {
        let (job, mut hold) = envelope.into_parts();
        match job {
            #[cfg(test)]
            CoordinatorJob::InlineReplacementFixture {
                input,
                control,
                response,
            } => {
                control.record(&input.continuation, ReplacementFixtureCut::Drained);
                hold.release();
                control.record(
                    &input.continuation,
                    ReplacementFixtureCut::AdmissionReleased,
                );
                drop(input);
                drop(response);
            }
            CoordinatorJob::FinalizeCreate { request, response } => {
                if let Some(owned) = request.owned_replacement {
                    // Shutdown is not stopped-executor cleanup authority. The
                    // accepted caller observes Retained when this channel ends;
                    // its original pending/finalized binding remains accounted.
                    hold.release();
                    drop(hold._create_ticket.take());
                    if let Ok(mut state) = owned.owner.state.lock() {
                        state.envelope_released = true;
                        state.retained_reason =
                            Some("owned finalizer drained during shutdown".into());
                    }
                    #[cfg(test)]
                    if let Some(control) = owned.fixture.as_ref() {
                        control.record(&owned, ReplacementFixtureCut::TicketDrained);
                    }
                    drop(response);
                    continue;
                }
                execute_rollback_create(transaction, request.binding).await;
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!(
                        "[selection] shutdown finalizer caller dropped session={}",
                        request.binding.session_id()
                    );
                }
            }
            CoordinatorJob::RollbackCreate { binding } => {
                execute_rollback_create(transaction, binding).await;
                hold.release();
            }
            CoordinatorJob::Transition { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued transition caller dropped during shutdown");
                }
            }
            CoordinatorJob::Snapshot { response } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued snapshot caller dropped during shutdown");
                }
            }
            CoordinatorJob::RouteLoss { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued route-loss caller dropped during shutdown");
                }
            }
            CoordinatorJob::ConditionalResponseClose { .. } => {}
            CoordinatorJob::Destroy { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued destroy caller dropped during shutdown");
                }
            }
            CoordinatorJob::RestartLifecycle { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued restart caller dropped during shutdown");
                }
            }
            CoordinatorJob::RootLifecycle { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued Root caller dropped during shutdown");
                }
            }
            CoordinatorJob::ResourceKill { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued resource-kill caller dropped during shutdown");
                }
            }
            CoordinatorJob::Detach { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued detach caller dropped during shutdown");
                }
            }
            CoordinatorJob::Attach { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued attach caller dropped during shutdown");
                }
            }
            CoordinatorJob::RestoreBarrier { started, .. } => {
                if started.send(()).is_err() {
                    log::debug!("[selection] restore submitter dropped during shutdown");
                }
                hold.release();
            }
        }
        debug_assert!(
            hold.is_released(),
            "#1580: a coordinator job arm finished without releasing admission and critical key"
        );
    }
}

async fn execute_envelope<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    envelope: CoordinatorEnvelope,
) {
    let (job, mut hold) = envelope.into_parts();
    match job {
        #[cfg(test)]
        CoordinatorJob::InlineReplacementFixture {
            input,
            control,
            response,
        } => {
            let owned = input.continuation.clone();
            control.record(&owned, ReplacementFixtureCut::WorkerEntered);
            let mut trace = ReplacementFixtureEnvelopeTrace {
                hold: &mut hold,
                control: &control,
                owned: &owned,
                released: false,
                outcome: None,
            };
            let result =
                crate::commands::session::execute_inline_replacement_fixture(transaction, input)
                    .await;
            let outcome = match &result.outcome {
                Ok(crate::commands::session::OwnedReplacementCreateStep::Finalized(_)) => {
                    ReplacementFixtureOutcome::Finalized
                }
                Ok(crate::commands::session::OwnedReplacementCreateStep::Retained) => {
                    ReplacementFixtureOutcome::Retained
                }
                Err(_) => ReplacementFixtureOutcome::Error,
            };
            control.record_outcome(
                &owned,
                ReplacementFixtureCut::AdapterReturned,
                Some(outcome),
            );
            let resumed = ReplacementFixtureControl::wait_gate(&control.before_release_gate).await;
            trace.outcome = Some(outcome);
            trace.release();
            let after_release = control
                .reached(&owned, ReplacementFixtureCut::AfterReleaseBeforeSend)
                .await;
            if resumed && after_release {
                if response.send(result).is_err() {
                    control.record(&owned, ReplacementFixtureCut::ReceiverLost);
                }
            } else {
                control.record(&owned, ReplacementFixtureCut::GateLost);
                drop(result);
                drop(response);
            }
        }
        CoordinatorJob::Transition { request, response } => {
            let result = execute_transition(transaction, request).await;
            if hold.respond(response, result).is_err() {
                log::debug!("[selection] transition caller dropped before result delivery");
            }
        }
        CoordinatorJob::Snapshot { response } => {
            let snapshot = transaction.manager().await.selection_payload().await;
            if hold.respond(response, Ok(snapshot)).is_err() {
                log::debug!("[selection] snapshot caller dropped before result delivery");
            }
        }
        CoordinatorJob::FinalizeCreate { request, response } => {
            let session_id = request.binding.session_id();
            let owned = request.owned_replacement.clone();
            let result = if owned.is_some() {
                execute_owned_finalize_create(transaction, request).await
            } else {
                Some(execute_finalize_create(transaction, request).await)
            };
            match result {
                Some(result) => {
                    let observed = result.clone();
                    hold.release();
                    if owned.is_some() {
                        drop(hold._create_ticket.take());
                    }
                    if let Some(owned) = owned {
                        if let Ok(mut state) = owned.owner.state.lock() {
                            state.completion = Some(observed);
                            state.publication_observed = true;
                            state.envelope_released = true;
                        }
                        owned.owner.changed.notify_waiters();
                    }
                    // No await separates recording the attempt from sending.
                    // Only receipt permits the producer to certify work.
                    let sent = response.send(result);
                    if sent.is_err() {
                        log::debug!(
                            "[selection] create finalizer caller dropped session={}",
                            session_id
                        );
                    }
                }
                None => {
                    hold.release();
                    drop(response);
                }
            }
        }
        CoordinatorJob::RollbackCreate { binding } => {
            execute_rollback_create(transaction, binding).await;
            hold.release();
        }
        CoordinatorJob::RouteLoss {
            session_id,
            exit_code,
            response,
        } => {
            let result = execute_route_loss(transaction, session_id, exit_code).await;
            if hold.respond(response, result).is_err() {
                log::debug!(
                    "[selection] route-loss caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::ConditionalResponseClose { request } => {
            if let Err(error) = crate::commands::session::execute_response_close_transaction(
                transaction,
                request.ticket,
            )
            .await
            {
                log::warn!("[response-close] conditional operation failed: {}", error);
            }
        }
        CoordinatorJob::Destroy { request, response } => {
            let result =
                crate::commands::session::execute_destroy_transaction(transaction, request).await;
            if hold.respond(response, result).is_err() {
                log::debug!("[selection] destroy caller dropped before result delivery");
            }
        }
        CoordinatorJob::RestartLifecycle {
            request,
            mut entry,
            response,
        } => {
            let Some(authority) = entry.authority.as_mut() else {
                hold.release();
                drop(response);
                return;
            };
            if !crate::commands::session::validate_lifecycle_entry_authority(
                transaction.app(),
                authority,
            )
            .await
            {
                hold.release();
                drop(response);
                return;
            }
            let transaction = transaction.for_lifecycle_entry(&entry);
            if !transaction.has_accepted_lifecycle_core() {
                hold.release();
                drop(response);
                return;
            }
            let Some(authority) = entry.authority.as_ref() else {
                hold.release();
                drop(response);
                return;
            };
            let result = crate::commands::session::execute_external_restart_transaction(
                &transaction,
                request,
                authority,
            )
            .await;
            let result = match result {
                crate::commands::session::LifecycleEntryOutcome::Returned(result) => result,
                crate::commands::session::LifecycleEntryOutcome::Interrupted => {
                    entry.observe_authority_interruption();
                    drop(transaction);
                    drop(entry);
                    hold.release();
                    drop(response);
                    return;
                }
            };
            entry.observe_return(&result);
            entry.release_lease().await;
            drop(transaction);
            drop(entry);
            if hold.respond(response, result).is_err() {
                log::debug!("[selection] restart caller dropped before result delivery");
            }
        }
        CoordinatorJob::RootLifecycle {
            request,
            mut entry,
            response,
        } => {
            let Some(authority) = entry.authority.as_mut() else {
                hold.release();
                drop(response);
                return;
            };
            if !crate::commands::session::validate_lifecycle_entry_authority(
                transaction.app(),
                authority,
            )
            .await
            {
                hold.release();
                drop(response);
                return;
            }
            let transaction = transaction.for_lifecycle_entry(&entry);
            if !transaction.has_accepted_lifecycle_core() {
                hold.release();
                drop(response);
                return;
            }
            let Some(authority) = entry.authority.as_ref() else {
                hold.release();
                drop(response);
                return;
            };
            let result = crate::commands::session::execute_external_root_transaction(
                &transaction,
                request,
                authority,
            )
            .await;
            let result = match result {
                crate::commands::session::LifecycleEntryOutcome::Returned(result) => result,
                crate::commands::session::LifecycleEntryOutcome::Interrupted => {
                    entry.observe_authority_interruption();
                    drop(transaction);
                    drop(entry);
                    hold.release();
                    drop(response);
                    return;
                }
            };
            entry.observe_return(&result);
            entry.release_lease().await;
            drop(transaction);
            drop(entry);
            if hold.respond(response, result).is_err() {
                log::debug!("[selection] Root caller dropped before result delivery");
            }
        }
        CoordinatorJob::ResourceKill {
            session_id,
            intent,
            response,
        } => {
            let result = crate::commands::resource_monitor::execute_resource_kill_transaction(
                transaction,
                session_id,
                intent,
            )
            .await;
            if hold.respond(response, result).is_err() {
                log::debug!(
                    "[selection] resource-kill caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::Detach {
            session_id,
            geometry,
            suppress_selection,
            response,
        } => {
            let result = crate::commands::window::execute_detach_transaction(
                transaction,
                session_id,
                geometry,
                suppress_selection,
            )
            .await;
            if hold.respond(response, result).is_err() {
                log::debug!(
                    "[selection] detach caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::Attach {
            session_id,
            response,
        } => {
            let result =
                crate::commands::window::execute_attach_transaction(transaction, session_id).await;
            if hold.respond(response, result).is_err() {
                log::debug!(
                    "[selection] attach caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::RestoreBarrier {
            nonce,
            started,
            release,
        } => {
            if !transaction.begin_restore(nonce) {
                drop(started);
                hold.release();
                return;
            }
            if started.send(()).is_err() {
                log::debug!("[selection] restore submitter dropped before barrier start");
            }
            if release.await.is_err() {
                log::warn!("[selection] restore barrier released by dropped owner");
            }
            transaction.release_restore(nonce);
            hold.release();
        }
    }
    debug_assert!(
        hold.is_released(),
        "#1580: a coordinator job arm finished without releasing admission and critical key"
    );
}

async fn execute_transition<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: SelectionRequest,
) -> Result<Option<SessionSelection>, String> {
    match request {
        SelectionRequest::UserSwitch { session_id } => {
            let aggregate = transaction.aggregate_snapshot().await;
            let target = aggregate
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
                .ok_or_else(|| format!("Session not found: {session_id}"))?;
            let runtime = transaction.runtime_snapshot(session_id);
            if runtime.detached {
                let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
                if let Some(window) = transaction.app().get_webview_window(&label) {
                    if let Err(error) = window.set_focus() {
                        log::warn!(
                            "[selection] failed to focus detached target session={} source=userSwitch: {}",
                            session_id,
                            error
                        );
                    }
                }
                return Ok(None);
            }

            let decision = match target.status {
                SessionStatus::Exited(_) => CommitDecision::Dormant(
                    DormantRuntimeWitness::from_snapshot(runtime)
                        .ok_or_else(|| "Session is detached".to_string())?,
                ),
                _ if runtime.has_pty => CommitDecision::Live(
                    LiveRuntimeWitness::from_snapshot(runtime)
                        .ok_or_else(|| "Session has no live PTY".to_string())?,
                ),
                _ => {
                    if aggregate.selection.id() == Some(session_id) {
                        let repair = transaction
                            .commit(
                                CommitDecision::Clear,
                                SelectionCause::LivenessReconcile,
                                LifecycleMutations::default(),
                            )
                            .await?;
                        if let Some(payload) = repair.selection.as_ref() {
                            transaction
                                .persist(SelectionSource::LivenessReconcile, Some(session_id))
                                .await;
                            transaction.publish_selection(payload);
                        }
                    }
                    return Err("Session has no live PTY".to_string());
                }
            };
            let committed = transaction
                .commit(
                    decision,
                    SelectionCause::UserSwitch,
                    LifecycleMutations::default(),
                )
                .await?;
            transaction.publish_view_requested(session_id);
            if let Some(payload) = committed.selection.as_ref() {
                transaction
                    .persist(SelectionSource::UserSwitch, Some(session_id))
                    .await;
                transaction.publish_selection(payload);
            }
            Ok(committed.selection)
        }
        SelectionRequest::Restore { persisted_target } => {
            let aggregate = transaction.aggregate_snapshot().await;
            let exact = persisted_target.and_then(|target_id| {
                let record = aggregate
                    .sessions
                    .iter()
                    .find(|record| record.id == target_id)?;
                let runtime = transaction.runtime_snapshot(target_id);
                if runtime.detached {
                    return None;
                }
                match record.status {
                    SessionStatus::Exited(_) => transaction.dormant_decision(target_id),
                    _ if runtime.has_pty => transaction.live_decision(target_id),
                    _ => None,
                }
            });
            let decision = exact
                .or_else(|| {
                    aggregate.sessions.iter().find_map(|record| {
                        if matches!(record.status, SessionStatus::Exited(_)) {
                            return None;
                        }
                        transaction.live_decision(record.id)
                    })
                })
                .unwrap_or(CommitDecision::Clear);
            let committed = transaction
                .commit(
                    decision,
                    SelectionCause::Restore,
                    LifecycleMutations::default(),
                )
                .await?;
            if let Some(payload) = committed.selection.as_ref() {
                transaction
                    .persist(SelectionSource::Restore, payload.id())
                    .await;
                transaction.publish_selection(payload);
            }
            Ok(committed.selection)
        }
    }
}

async fn execute_owned_finalize_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: FinalizeCreateRequest,
) -> Option<Result<SessionInfo, String>> {
    let continuation = request.owned_replacement?;
    #[cfg(test)]
    if !continuation
        .fixture_reached(ReplacementFixtureCut::BeforeFinalizer)
        .await
    {
        return None;
    }
    let manager = transaction.manager().await;
    if manager.get_pending_session(request.binding).await.is_none() {
        return Some(Err("created session pending record is unavailable".into()));
    }
    let runtime = transaction.runtime_snapshot(request.binding.session_id());
    let live = match LiveRuntimeWitness::from_snapshot(runtime) {
        Some(live) => live,
        None => return Some(Err("created session is not displayable".into())),
    };
    {
        let mut state = continuation.owner.state.lock().ok()?;
        state.replacement.as_mut()?.warnings = request.warnings;
    }
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(request.binding, live);
    #[cfg(test)]
    if continuation
        .fixture
        .as_ref()
        .and_then(|control| {
            control.take_action(&continuation, |action| {
                matches!(action, ReplacementFixtureAction::SimulatedPrecommitFailure)
            })
        })
        .is_some()
    {
        return Some(Err("simulated fixture precommit setup failure".into()));
    }
    // Owned replacement never auto-selects B. The later R5 commit alone
    // removes A with current Clear/Keep; generic None stays literal below.
    match continuation
        .commit_finalized(transaction, CommitDecision::Keep, request.cause, mutations)
        .await?
    {
        Ok(_) => {}
        Err(error) => return Some(Err(error)),
    }
    let (info, selection) = continuation.complete_finalized_create(transaction).await?;
    let publication = continuation.start_effect(LifecycleEffectSlot::Publication)?;
    let (warnings, previous) = {
        let state = continuation.owner.state.lock().ok()?;
        let progress = state.replacement.as_ref()?;
        let OwnedReplacementDescriptor::Replacement(descriptor) = &continuation.owner.descriptor
        else {
            return None;
        };
        (progress.warnings.clone(), descriptor.previous.id)
    };
    // Owned checkpoint already persisted current B. No generic stale save.
    transaction.publish_created(&info);
    transaction.publish_destroyed(previous);
    transaction.publish_selection(&selection);
    for warning in warnings {
        emit_session_warning(transaction.app(), warning);
    }
    publication.complete(Ok(Some(info.id.clone())));
    Some(Ok(info))
}

async fn execute_finalize_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: FinalizeCreateRequest,
) -> Result<SessionInfo, String> {
    let session_id = request.binding.session_id();
    if transaction
        .manager()
        .await
        .get_pending_session(request.binding)
        .await
        .is_none()
    {
        execute_rollback_create(transaction, request.binding).await;
        return Err("created session pending record is unavailable".to_string());
    }
    let runtime = transaction.runtime_snapshot(session_id);
    let live = LiveRuntimeWitness::from_snapshot(runtime)
        .ok_or_else(|| "created session is not displayable".to_string());
    let live = match live {
        Ok(live) => live,
        Err(error) => {
            execute_rollback_create(transaction, request.binding).await;
            return Err(error);
        }
    };
    let current = transaction.manager().await.selection_payload().await;
    let should_select =
        request
            .auto_select_precondition
            .as_ref()
            .is_some_and(|(epoch, revision)| {
                current.mode() == SelectionMode::None
                    && current.epoch() == epoch
                    && current.revision() == *revision
            });
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(request.binding, live);
    let decision = if should_select {
        CommitDecision::Live(live)
    } else {
        CommitDecision::Keep
    };
    let committed = match transaction
        .commit_entry_create(request.binding, decision, request.cause, mutations)
        .await
    {
        Ok(committed) => committed,
        Err(error) => {
            log::warn!(
                "[selection] create finalization commit rejected session={}: {}",
                session_id,
                error
            );
            execute_rollback_create(transaction, request.binding).await;
            return Err(error);
        }
    };
    let info = committed
        .finalized_rows
        .iter()
        .find(|row| row.id == session_id.to_string())
        .cloned()
        .ok_or_else(|| "create finalization did not publish a row".to_string())?;
    let publication = transaction.start_lifecycle_effect(LifecycleEffectSlot::Publication);
    if transaction.has_lifecycle_entry() && publication.is_none() {
        return Err(SelectionCoordinatorError::Unavailable.to_string());
    }
    transaction
        .persist(SelectionSource::SessionCreated, Some(session_id))
        .await;
    transaction.publish_created(&info);
    if let Some(payload) = committed.selection.as_ref() {
        transaction.publish_selection(payload);
    }
    for warning in request.warnings {
        emit_session_warning(transaction.app(), warning);
    }
    if let Some(publication) = publication {
        publication.complete(Ok(Some(info.id.clone())));
    }
    Ok(info)
}

async fn execute_restore_dormant<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: DormantRestoreRequest,
) -> Result<SessionInfo, String> {
    let id = Uuid::new_v4();
    let binding = PendingCreateBinding::new(id, Uuid::new_v4());
    let persisted = request.persisted;
    let coding_agent_binding = persisted.binding_metadata()?;
    let exit_code = match persisted.status {
        Some(SessionStatus::Exited(code)) => code,
        _ => 0,
    };
    let session = crate::session::session::Session {
        id,
        name: persisted.name,
        shell: persisted.shell,
        shell_args: persisted.shell_args,
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: chrono::Utc::now(),
        working_directory: request.working_directory,
        status: SessionStatus::Running,
        waiting_for_input: false,
        communication: persisted.communication,
        pending_review: false,
        last_prompt: persisted.last_prompt,
        agent_id: persisted.agent_id,
        agent_label: persisted.agent_label,
        git_repos: persisted.git_repos,
        is_coordinator: request.is_coordinator,
        is_root_agent: request.is_root_agent,
        git_repos_gen: 0,
        agent_turn_armed: false,
        token: Uuid::new_v4(),
        agent_kind: None,
        requested_profile: persisted.requested_profile,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        resolved_claude_projects_dir: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        telegram_bot_id: persisted.telegram_bot_id,
        was_detached: persisted.was_detached,
        detached_geometry: persisted.detached_geometry,
        start_fresh_on_restore: persisted.start_fresh_on_restore,
        // #1088 ignored on restore: the scraper re-reads and re-commits on its
        // first post-restart tick (mirrors id/waiting_for_input/created_at above).
        context_percent: None,
    };
    let manager = transaction.manager().await;
    let carrier_permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    manager
        .insert_transaction_pending_record_with_carrier_permit(&carrier_permit, session, binding)
        .await?;
    if let Err(error) = manager
        .install_pending_coding_agent_binding_with_carrier_permit(
            &carrier_permit,
            binding,
            coding_agent_binding,
        )
        .await
    {
        manager
            .rollback_pending_create_with_carrier_permit(&carrier_permit, binding)
            .await?;
        return Err(error);
    }
    drop(carrier_permit);
    let witness = DormantRuntimeWitness::from_snapshot(transaction.runtime_snapshot(id))
        .ok_or_else(|| "restored dormant record is runtime-detached".to_string())?;
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_dormant(binding, witness, exit_code);
    let committed = transaction
        .commit(CommitDecision::Keep, SelectionCause::Restore, mutations)
        .await?;
    let info = committed
        .finalized_rows
        .into_iter()
        .find(|row| row.id == id.to_string())
        .ok_or_else(|| "dormant restore finalization did not publish a row".to_string())?;
    transaction
        .persist(SelectionSource::Restore, Some(id))
        .await;
    transaction.publish_created(&info);
    Ok(info)
}

async fn execute_rollback_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    binding: PendingCreateBinding,
) {
    let manager = transaction.manager().await;
    let pty = transaction
        .app()
        .try_state::<Arc<Mutex<PtyManager>>>()
        .map(|state| state.inner().clone());
    if transaction.has_lifecycle_entry() {
        execute_entry_rollback_create(transaction, manager, pty, binding).await;
    } else {
        if let Some(inner) = transaction.coordinator.upgrade() {
            if !matches!(retained_entry_owns_binding(&inner, binding), Ok(None)) {
                // An owned or unreadable retained binding is never a generic
                // rollback capability, including a stale queued cleanup job.
                return;
            }
        }
        execute_rollback_create_parts(manager, pty, binding).await;
    }
}

/// An entry's own stopped setup body calls this on its real error exit. Neither
/// a missing pending row nor an Err grants authority to kill a finalized B.
async fn execute_entry_rollback_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    manager: SessionManager,
    pty: Option<Arc<Mutex<PtyManager>>>,
    binding: PendingCreateBinding,
) {
    let Some((owner, executor)) = transaction.lifecycle_entry.as_ref() else {
        return;
    };
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let pending = manager.get_pending_session(binding).await;
    let cleanup_nonce = Uuid::new_v4();
    let claimed = owner.state.lock().ok().is_some_and(|mut state| {
        if state.executor_nonce != *executor
            || state.stage != ReplacementStage::Executing
            || state.effects.iter().any(|effect| {
                matches!(
                    effect,
                    OwnedEffectState::Running(_) | OwnedEffectState::Ambiguous(_)
                )
            })
        {
            return false;
        }
        let Some(create) = state.entry_create.as_mut() else {
            return false;
        };
        if create.binding != binding
            || create.stage != EntryCreateStage::Pending
            || pending.is_none()
        {
            return false;
        }
        create.stage = EntryCreateStage::CleanupClaimed(cleanup_nonce);
        true
    });
    drop(permit);
    if !claimed {
        return;
    }
    let effect = transaction.start_lifecycle_effect(LifecycleEffectSlot::PendingRollback);
    let Some(effect) = effect else {
        return;
    };
    let Some(pending) = pending else {
        return;
    };
    let kill = match pty.as_ref().and_then(|pty| pty.lock().ok()) {
        Some(pty) => pty
            .kill_for_kind(binding.session_id(), pending.backend_kind)
            .map_err(|error| error.to_string()),
        None => Err(SelectionCoordinatorError::Unavailable.to_string()),
    };
    if let Err(error) = kill {
        effect.complete(Err(error));
        if let Ok(mut state) = owner.state.lock() {
            state.stage = ReplacementStage::Ambiguous;
            state.retained_reason = Some("entry pending backend rollback failed".into());
        }
        return;
    }
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let still_claimed = owner.state.lock().ok().is_some_and(|state| {
        state.executor_nonce == *executor
            && state.entry_create.as_ref().is_some_and(|create| {
                create.binding == binding
                    && create.stage == EntryCreateStage::CleanupClaimed(cleanup_nonce)
            })
    });
    if !still_claimed {
        return;
    }
    let result = manager
        .rollback_pending_create_with_carrier_permit(&permit, binding)
        .await;
    if let Ok(mut state) = owner.state.lock() {
        if let Some(create) = state.entry_create.as_mut() {
            if create.binding == binding
                && create.stage == EntryCreateStage::CleanupClaimed(cleanup_nonce)
            {
                create.stage = if result.is_ok() {
                    EntryCreateStage::RolledBack
                } else {
                    EntryCreateStage::Unknown
                };
            }
        }
        if result.is_err() {
            state.stage = ReplacementStage::Ambiguous;
            state.retained_reason = Some("entry pending manager rollback failed".into());
        }
    }
    effect.complete(result.map(|_| None));
    drop(permit);
}

async fn execute_rollback_create_parts(
    manager: SessionManager,
    pty: Option<Arc<Mutex<PtyManager>>>,
    binding: PendingCreateBinding,
) {
    let session_id = binding.session_id();
    let backend_kind = manager
        .get_pending_session(binding)
        .await
        .map(|session| session.backend_kind);
    if let Some(pty) = pty {
        let pty = pty.lock().unwrap_or_else(|error| error.into_inner());
        let kill_result = match backend_kind {
            Some(kind) => Some(pty.kill_for_kind(session_id, kind)),
            None if pty.has_session(session_id) => Some(pty.kill(session_id)),
            None => None,
        };
        if let Some(Err(error)) = kill_result {
            log::debug!(
                "[selection] pending create PTY cleanup session={} result={}",
                session_id,
                error
            );
        }
    }
    if backend_kind.is_none() {
        return;
    }
    if let Err(error) = manager.rollback_pending_create(binding).await {
        log::warn!(
            "[selection] pending create manager rollback failed session={}: {}",
            session_id,
            error
        );
    }
}

fn begin_container_shutdown(inner: &Arc<CoordinatorInner>, deadline: Instant) -> bool {
    let container_backend = match inner.cleanup_container_backend.try_lock() {
        Ok(backend) => backend.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] container backend ownership lock unavailable during shutdown signal: {}",
                error
            );
            return false;
        }
    };
    if let Some(container_backend) = container_backend {
        return container_backend.begin_shutdown(deadline);
    }
    true
}

/// Clone one retained owner at a time; no root lock spans owner inspection,
/// U acquisition, native cleanup, or resource destruction.
fn retained_entry_owns_binding(
    inner: &CoordinatorInner,
    binding: PendingCreateBinding,
) -> Result<Option<Arc<ReplacementOwner>>, ()> {
    let mut cursor = None;
    loop {
        let next = {
            let root = inner.retained_replacements.try_lock().map_err(|_| ())?;
            if root.unavailable {
                return Err(());
            }
            root.entries
                .iter()
                .filter(|(operation, _)| cursor.is_none_or(|last| **operation > last))
                .min_by_key(|(operation, _)| **operation)
                .map(|(operation, owner)| (*operation, owner.clone()))
        };
        let Some((operation, owner)) = next else {
            return Ok(None);
        };
        cursor = Some(operation);
        let state = owner.state.try_lock().map_err(|_| ())?;
        if let Some(progress) = state.replacement.as_ref() {
            if progress.binding.session_id() == binding.session_id() {
                return if progress.binding == binding {
                    Ok(Some(owner.clone()))
                } else {
                    Err(())
                };
            }
        }
        if let Some(create) = state.entry_create.as_ref() {
            if create.binding.session_id() == binding.session_id() {
                return if create.binding == binding {
                    Ok(Some(owner.clone()))
                } else {
                    Err(())
                };
            }
        }
    }
}

async fn cleanup_pending_creates_after_join(
    inner: &Arc<CoordinatorInner>,
    deadline: Instant,
) -> Vec<String> {
    let pty = match inner.cleanup_pty.try_lock() {
        Ok(pty) => pty.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] pending-create PTY ownership lock unavailable: {}",
                error
            );
            return vec!["reason=pending-pty-owner state=retained".to_string()];
        }
    };
    let Some(pty) = pty else {
        log::error!(
            "[selection] pending-create PTY owner unavailable after worker join state=retained"
        );
        return vec!["reason=pending-pty-owner state=retained".to_string()];
    };
    let manager = Arc::clone(&inner.manager);
    let cleanup_inner = Arc::clone(inner);
    let mut cleanup = tokio::spawn(async move {
        let manager = manager.read().await.clone();
        let bindings = manager.pending_create_bindings().await;
        if !bindings.is_empty() {
            log::warn!(
                "[selection] cleaning {} pending create(s) after worker join",
                bindings.len()
            );
        }
        let mut retained = Vec::new();
        for binding in bindings {
            let session_id = binding.session_id();
            match retained_entry_owns_binding(&cleanup_inner, binding) {
                Ok(None) => {}
                Ok(Some(owner)) => {
                    let nonce =
                        owner.state.lock().ok().and_then(|state| {
                            state.replacement.as_ref().map(|_| state.executor_nonce)
                        });
                    let cleaned = if let Some(executor_nonce) = nonce {
                        OwnedReplacementContinuation {
                            owner,
                            coordinator: Arc::downgrade(&cleanup_inner),
                            executor_nonce,
                            source_validation: None,
                            #[cfg(test)]
                            fixture: None,
                        }
                        .cleanup_stopped_source(&manager, &pty)
                        .await
                    } else {
                        false
                    };
                    if cleaned {
                        continue;
                    }
                    retained.push(format!(
                        "session={session_id} reason=owned-entry-cleanup-authority state=retained"
                    ));
                    continue;
                }
                Err(()) => {
                    retained.push(format!(
                        "session={session_id} reason=owned-entry-unreadable state=retained"
                    ));
                    continue;
                }
            }
            let Some(backend_kind) = manager
                .get_pending_session(binding)
                .await
                .map(|session| session.backend_kind)
            else {
                continue;
            };
            let kill_pty = Arc::clone(&pty);
            let kill = tokio::task::spawn_blocking(move || {
                kill_pty
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .kill_for_kind(session_id, backend_kind)
            })
            .await;
            match kill {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    log::error!(
                        "[selection] pending-create PTY cleanup failed session={} reason=pending-pty-kill state=retained error={}",
                        session_id,
                        error
                    );
                    retained.push(format!(
                        "session={session_id} reason=pending-pty-kill state=retained"
                    ));
                    continue;
                }
                Err(error) => {
                    log::error!(
                        "[selection] pending-create PTY cleanup task failed session={} reason=pending-pty-kill state=retained error={}",
                        session_id,
                        error
                    );
                    retained.push(format!(
                        "session={session_id} reason=pending-pty-kill state=retained"
                    ));
                    continue;
                }
            }
            if let Err(error) = manager.rollback_pending_create(binding).await {
                log::warn!(
                    "[selection] pending create manager rollback failed session={} reason=pending-manager-rollback state=retained error={}",
                    session_id,
                    error
                );
                retained.push(format!(
                    "session={session_id} reason=pending-manager-rollback state=retained"
                ));
            }
        }
        retained
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    match tokio::time::timeout(remaining, &mut cleanup).await {
        Ok(Ok(retained)) => retained,
        Ok(Err(error)) => {
            log::error!(
                "[selection] pending-create cleanup owner failed reason=pending-cleanup-task state=retained error={}",
                error
            );
            vec!["reason=pending-cleanup-task state=retained".to_string()]
        }
        Err(_) => {
            log::error!(
                "[selection] pending-create cleanup owner retained after absolute deadline reason=pending-cleanup-await state=retained"
            );
            inner
                .retained_rollback_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(cleanup);
            vec!["reason=pending-cleanup-await state=retained".to_string()]
        }
    }
}

async fn seal_and_drain_container_shutdown_work_after_join(
    inner: &Arc<CoordinatorInner>,
    deadline: Instant,
) -> Vec<String> {
    let mut retained = Vec::new();
    let container_backend = match inner.cleanup_container_backend.try_lock() {
        Ok(backend) => backend.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] container backend ownership lock unavailable before bounded drain: {}",
                error
            );
            retained.push("reason=container-drain-owner state=retained".to_string());
            return retained;
        }
    };
    let Some(container_backend) = container_backend else {
        return retained;
    };
    let mut task = tokio::task::spawn_blocking(move || {
        container_backend.seal_and_drain_shutdown_work_blocking(deadline)
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    match tokio::time::timeout(remaining, &mut task).await {
        Ok(Ok(report)) => retained.extend(report.retained),
        Ok(Err(error)) => {
            log::error!(
                "[selection] container shutdown-work drain failed after coordinator join: {}",
                error
            );
            retained.push("reason=container-drain-task state=retained".to_string());
        }
        Err(_) => {
            log::error!(
                "[selection] container shutdown-work blocking task retained after absolute deadline reason=container-drain-await state=retained"
            );
            retained.push("reason=container-drain-await state=retained".to_string());
            inner
                .retained_cleanup_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(task);
        }
    }
    retained
}

async fn execute_route_loss<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    exit_code: i32,
) -> Result<(), String> {
    let aggregate = transaction.aggregate_snapshot().await;
    if aggregate.pending_ids.contains(&session_id) {
        return Ok(());
    }
    let Some(record) = aggregate
        .sessions
        .iter()
        .find(|record| record.id == session_id)
    else {
        return Ok(());
    };
    if matches!(record.status, SessionStatus::Exited(_)) {
        return Ok(());
    }
    let mut runtime = transaction.runtime_snapshot(session_id);
    if runtime.detached {
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
        if let Some(window) = transaction.app().get_webview_window(&label) {
            if let Err(error) = window.destroy() {
                log::warn!(
                    "[selection] route-loss detached window close failed session={}: {}",
                    session_id,
                    error
                );
            }
        }
        runtime = transaction.runtime_snapshot(session_id);
    }
    let decision = if aggregate.selection.id() == Some(session_id) {
        CommitDecision::Dormant(
            DormantRuntimeWitness::from_snapshot(runtime)
                .ok_or_else(|| "route-loss target remained detached".to_string())?,
        )
    } else {
        CommitDecision::Keep
    };
    let mut mutations = LifecycleMutations::default();
    mutations.mark_exited(session_id, exit_code);
    let committed = transaction
        .commit(decision, SelectionCause::LivenessReconcile, mutations)
        .await?;
    if committed.changed_rows.is_empty() {
        return Ok(());
    }
    transaction
        .persist(SelectionSource::LivenessReconcile, Some(session_id))
        .await;
    transaction.publish_destroyed(session_id);
    for row in &committed.changed_rows {
        transaction.publish_created(row);
    }
    for cleared in &committed.cleared_raise_hand_ids {
        transaction.publish_communication_cleared(*cleared);
    }
    if let Some(payload) = committed.selection.as_ref() {
        transaction.publish_selection(payload);
    }
    Ok(())
}

fn remove_critical_key(inner: &Arc<CoordinatorInner>, key: CriticalAdmissionKey) {
    match inner.critical_keys.lock() {
        Ok(mut keys) => {
            keys.remove(&key);
        }
        Err(error) => {
            log::error!(
                "[selection] critical-key lock poisoned session={} kind={:?}: {}",
                key.session_id,
                key.kind,
                error
            );
        }
    }
}

fn publish_selection<R: Runtime>(app: &AppHandle<R>, payload: &SessionSelection) {
    publish_lifecycle_event(app, "session_switched", payload);
}

fn publish_lifecycle_event<R: Runtime, T: Serialize>(
    app: &AppHandle<R>,
    event: &'static str,
    payload: &T,
) {
    let value = match serde_json::to_value(payload) {
        Ok(value) => value,
        Err(error) => {
            log::error!(
                "[selection] failed to serialize event={} payload: {}",
                event,
                error
            );
            return;
        }
    };
    if let Err(error) = app.emit(event, value.clone()) {
        log::error!(
            "[selection] Tauri publication failed event={}: {}",
            event,
            error
        );
    }
    if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
        broadcaster.broadcast_event(event, &value);
    } else {
        log::debug!(
            "[selection] WebSocket broadcaster unavailable event={}",
            event
        );
    }
}

pub(crate) fn publish_session_communication<R: Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    communication: Option<&crate::session::session::SessionCommunication>,
) {
    publish_lifecycle_event(
        app,
        "session_communication_changed",
        &serde_json::json!({
            "sessionId": session_id.to_string(),
            "communication": communication,
        }),
    );
}

#[cfg(test)]
mod tests {
    fn response_close_unbound_ticket() -> super::CreateFinalizationTicket {
        super::CreateFinalizationTicket {
            owned_replacement: None,
            slot: None,
            admission: None,
            create_ticket: None,
            binding: None,
            cause: super::SelectionCause::SessionCreated(super::TrustedCreateIntent::Background),
            auto_select_precondition: None,
            completed: false,
        }
    }

    #[tokio::test]
    async fn response_close_prepared_ticket_rejects_both_duplicate_bind_orders() {
        let mut prepared_first = response_close_unbound_ticket();
        let binding = super::PendingCreateBinding::new(uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        assert_eq!(prepared_first.bind_prepared(binding).unwrap(), binding);
        assert!(prepared_first.bind(uuid::Uuid::new_v4()).is_err());
        assert!(prepared_first.bind_prepared(binding).is_err());
        assert_eq!(prepared_first.binding(), Some(binding));
        // No manager row was created by this pure preparation fixture.
        prepared_first.completed = true;

        let mut default_first = response_close_unbound_ticket();
        let first = default_first.bind(uuid::Uuid::new_v4()).unwrap();
        assert!(default_first.bind_prepared(binding).is_err());
        assert_eq!(default_first.binding(), Some(first));
        default_first.completed = true;
        assert!(default_first.bind_prepared(binding).is_err());

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager.clone(), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(manager)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();
        coordinator.snapshot().await.unwrap();
        let mut waiting = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let original_selection = waiting.auto_select_precondition.clone();
        assert!(original_selection.is_some());
        assert!(waiting.release_capacity_for_wait());
        let mut occupied = Vec::new();
        for _ in 0..CREATE_TICKET_CAPACITY {
            occupied.push(coordinator.reserve_suppressed_create().await.unwrap());
        }
        assert!(
            matches!(
                waiting.resume_unbound_capacity(&coordinator),
                Err(SelectionCoordinatorError::Busy)
            ),
            "a real second probe reports actual saturation; it does not await capacity"
        );
        assert_eq!(waiting.auto_select_precondition, original_selection);
        drop(occupied);
        waiting.resume_unbound_capacity(&coordinator).unwrap();
        assert_eq!(
            waiting.auto_select_precondition, original_selection,
            "reacquisition preserves option1's original None-selection condition"
        );
        drop(waiting);
        coordinator.close_and_join().await;
    }

    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    async fn await_state_eq<T, F, D>(
        label: &str,
        budget: Duration,
        mut probe: F,
        expected: T,
        diagnose: D,
    ) -> T
    where
        T: PartialEq + std::fmt::Debug,
        F: FnMut() -> T,
        D: FnOnce() -> String,
    {
        let mut last = probe();
        if tokio::time::timeout(budget, async {
            while last != expected {
                tokio::time::sleep(Duration::from_millis(5)).await;
                last = probe();
            }
        })
        .await
        .is_err()
        {
            panic!(
                "{label}: expected {expected:?} within {budget:?}, last observed {last:?}; {}",
                diagnose()
            );
        }
        last
    }

    #[tokio::test]
    #[should_panic(expected = "AC2-DIAGNOSE-MARKER")]
    async fn await_state_eq_fails_when_the_state_never_arrives() {
        await_state_eq(
            "await_state_eq positive control",
            Duration::from_millis(50),
            || 0usize,
            1usize,
            || "AC2-DIAGNOSE-MARKER".to_string(),
        )
        .await;
    }

    #[tokio::test]
    async fn await_state_eq_does_not_evaluate_diagnose_when_the_state_arrives() {
        let diagnose_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&diagnose_calls);
        await_state_eq(
            "await_state_eq lazy diagnose control",
            Duration::from_millis(50),
            || 7usize,
            7usize,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                String::new()
            },
        )
        .await;
        assert_eq!(diagnose_calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn response_registration_physical_cwd_boundary() {
        use crate::pty::backend::SessionBackendKind;
        use crate::pty::manager::ResponseRouteIdentity;
        use std::path::PathBuf;
        let configured = PathBuf::from(
            std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("isolated test config"),
        );
        assert!(configured.is_absolute());
        if let Some(public) = std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR") {
            assert_eq!(
                PathBuf::from(public),
                configured,
                "public override must match before config access"
            );
        }
        assert_eq!(crate::config::config_dir().unwrap(), configured);
        for name in ["TEMP", "TMP", "TMPDIR"] {
            assert!(
                PathBuf::from(std::env::var_os(name).expect("isolated absolute temp"))
                    .is_absolute()
            );
        }
        for (canonical_member, canonical_coordinator) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            assert_eq!(crate::config::config_dir().unwrap(), configured);
            let temp = tempfile::TempDir::new().unwrap();
            let member_path = temp.path().join("member");
            let coordinator_path = temp.path().join("coordinator");
            std::fs::create_dir(&member_path).unwrap();
            std::fs::create_dir(&coordinator_path).unwrap();
            let manager = SessionManager::new();
            let member = manager
                .create_session(
                    "shell".into(),
                    Vec::new(),
                    member_path.to_string_lossy().into_owned(),
                    None,
                    None,
                    Vec::new(),
                    false,
                    SessionBackendKind::LocalProcess,
                )
                .await
                .unwrap();
            let coordinator = manager
                .create_session(
                    "shell".into(),
                    Vec::new(),
                    coordinator_path.to_string_lossy().into_owned(),
                    None,
                    None,
                    Vec::new(),
                    false,
                    SessionBackendKind::LocalProcess,
                )
                .await
                .unwrap();
            manager.rename_session(member.id, "member".to_string()).await.unwrap();
            manager.rename_session(coordinator.id, "coordinator".to_string()).await.unwrap();
            let backend = Arc::new(LifecycleTestBackend::default());
            let injected: Arc<dyn crate::pty::backend::PtyBackend> = backend.clone();
            let pty = Arc::new(Mutex::new(PtyManager::new_for_test(injected)));
            let mut spellings = Vec::new();
            for row in [&member, &coordinator] {
                let ordinary_path = crate::path_utils::normalize_windows_verbatim_path_buf(
                    &std::fs::canonicalize(&row.working_directory).unwrap(),
                );
                let ordinary_spec = crate::pty::backend::BackendSpawnSpec {
                    id: row.id,
                    agent_id: None,
                    coding_agent: None,
                    cmd: "shell".to_string(),
                    args: Vec::new(),
                    resolved_agent_host_shell: None,
                    cwd: ordinary_path.to_string_lossy().into_owned(),
                    selected_cwd: None,
                    cols: 120,
                    rows: 30,
                    container_image: None,
                    configured_env: Vec::new(),
                    env_remove_keys: Vec::new(),
                    env_unset: Vec::new(),
                    extra_env: Vec::new(),
                    idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
                    output_target: crate::pty::output::PtyOutputTarget::noop(),
                    resource_registration: None,
                    logical_resource_slot: None,
                    container_credential: None,
                    container_repo_mounts: Vec::new(),
                    launch_witness: Default::default(),
                };
                PtyManager::spawn(&pty, SessionBackendKind::LocalProcess, ordinary_spec)
                    .await
                    .unwrap();
                let proof = PtyManager::snapshot_route_proof(&pty, row.id).unwrap();
                assert!(proof.saved_replica().is_none());
                let ordinary_id = proof.saved_cwd().object_id;
                let canonical_path = proof.saved_cwd().canonical_path.clone();
                assert_ne!(ordinary_path, canonical_path);
                let first_generation = pty
                    .lock()
                    .unwrap()
                    .response_close_state()
                    .generation(row.id)
                    .expect("ordinary route registered");
                drop(proof);
                pty.lock()
                    .unwrap()
                    .remove_route_if_kind(row.id, SessionBackendKind::LocalProcess);
                assert!(pty.lock().unwrap().backend_kind(row.id).is_none());
                assert!(pty
                    .lock()
                    .unwrap()
                    .response_close_state()
                    .generation(row.id)
                    .is_none());
                backend.set_live(row.id, false);
                let canonical_spec = crate::pty::backend::BackendSpawnSpec {
                    id: row.id,
                    agent_id: None,
                    coding_agent: None,
                    cmd: "shell".to_string(),
                    args: Vec::new(),
                    resolved_agent_host_shell: None,
                    cwd: canonical_path.to_string_lossy().into_owned(),
                    selected_cwd: None,
                    cols: 120,
                    rows: 30,
                    container_image: None,
                    configured_env: Vec::new(),
                    env_remove_keys: Vec::new(),
                    env_unset: Vec::new(),
                    extra_env: Vec::new(),
                    idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
                    output_target: crate::pty::output::PtyOutputTarget::noop(),
                    resource_registration: None,
                    logical_resource_slot: None,
                    container_credential: None,
                    container_repo_mounts: Vec::new(),
                    launch_witness: Default::default(),
                };
                PtyManager::spawn(&pty, SessionBackendKind::LocalProcess, canonical_spec)
                    .await
                    .unwrap();
                let proof = PtyManager::snapshot_route_proof(&pty, row.id).unwrap();
                assert!(proof.saved_replica().is_none());
                let canonical_id = proof.saved_cwd().object_id;
                assert_eq!(ordinary_id, canonical_id, "independent spawn verifications");
                let final_generation = pty
                    .lock()
                    .unwrap()
                    .response_close_state()
                    .generation(row.id)
                    .expect("canonical route registered");
                assert!(final_generation > first_generation);
                drop(proof);
                spellings.push((ordinary_path, canonical_path, final_generation));
            }
            let other = temp.path().join("other");
            std::fs::create_dir(&other).unwrap();
            let file = temp.path().join("file");
            std::fs::write(&file, b"not a directory").unwrap();
            let invalid_paths = [
                other.clone(),
                other.join("missing"),
                file,
                other.join("..").join("other"),
            ];
            let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
            manager
                .with_current_carrier_snapshot(&permit, |current| {
                    let mut snapshot = current.clone();
                    for (index, (row, canonical)) in [
                        (&member, canonical_member),
                        (&coordinator, canonical_coordinator),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let (ordinary_path, canonical_path, final_generation) = &spellings[index];
                        let identity = ResponseRouteIdentity {
                            target: String::new(),
                            session_id: row.id,
                            token: row.token,
                            generation: *final_generation,
                            carrier_version: current.versions[&row.id],
                            backend: row.backend_kind,
                        };
                        snapshot
                            .rows
                            .iter_mut()
                            .find(|actual| actual.id == row.id)
                            .unwrap()
                            .working_directory = if canonical {
                            canonical_path
                        } else {
                            ordinary_path
                        }
                        .to_string_lossy()
                        .into_owned();
                        assert_eq!(
                            pty.lock()
                                .unwrap()
                                .response_close_state()
                                .generation(row.id),
                            Some(*final_generation)
                        );
                        let accepted = response_registration_row(&snapshot, &identity).unwrap();
                        assert!(response_registration_route(&pty, &identity, accepted));
                        for invalid in &invalid_paths {
                            let mut changed = snapshot.clone();
                            changed
                                .rows
                                .iter_mut()
                                .find(|actual| actual.id == row.id)
                                .unwrap()
                                .working_directory = invalid.to_string_lossy().into_owned();
                            let accepted = response_registration_row(&changed, &identity).unwrap();
                            assert!(!response_registration_route(&pty, &identity, accepted));
                        }
                        let mut container = snapshot.clone();
                        container
                            .rows
                            .iter_mut()
                            .find(|actual| actual.id == row.id)
                            .unwrap()
                            .backend_kind = SessionBackendKind::ContainerTransport;
                        let mut container_identity = identity.clone();
                        container_identity.backend = SessionBackendKind::ContainerTransport;
                        let accepted =
                            response_registration_row(&container, &container_identity).unwrap();
                        assert!(!response_registration_route(
                            &pty,
                            &container_identity,
                            accepted
                        ));
                        let mut wrong_token = identity.clone();
                        wrong_token.token = if row.id == member.id {
                            coordinator.token
                        } else {
                            member.token
                        };
                        assert!(response_registration_row(&snapshot, &wrong_token).is_none());
                        let mut wrong_version = identity.clone();
                        wrong_version.carrier_version += 1;
                        assert!(response_registration_row(&snapshot, &wrong_version).is_none());
                        let mut pending = snapshot.clone();
                        pending.pending_ids.insert(row.id);
                        assert!(response_registration_row(&pending, &identity).is_none());
                        let mut exited = snapshot.clone();
                        exited
                            .rows
                            .iter_mut()
                            .find(|actual| actual.id == row.id)
                            .unwrap()
                            .status = SessionStatus::Exited(0);
                        assert!(response_registration_row(&exited, &identity).is_none());
                    }
                })
                .await;
            drop(permit);
            for row in [&member, &coordinator] {
                pty.lock()
                    .unwrap()
                    .remove_route_if_kind(row.id, SessionBackendKind::LocalProcess);
                assert!(pty.lock().unwrap().backend_kind(row.id).is_none());
                assert!(pty
                    .lock()
                    .unwrap()
                    .response_close_state()
                    .generation(row.id)
                    .is_none());
                backend.set_live(row.id, false);
            }
        }
    }

    #[derive(Default)]
    struct LifecycleTestBackend {
        live: Mutex<HashSet<Uuid>>,
        kills: Mutex<std::collections::HashMap<Uuid, usize>>,
    }

    impl LifecycleTestBackend {
        fn set_live(&self, session_id: Uuid, live: bool) {
            let mut sessions = self.live.lock().unwrap();
            if live {
                sessions.insert(session_id);
            } else {
                sessions.remove(&session_id);
            }
        }

        fn kill_count(&self, session_id: Uuid) -> usize {
            self.kills
                .lock()
                .unwrap()
                .get(&session_id)
                .copied()
                .unwrap_or_default()
        }
    }

    struct GatedStopRuntime {
        stop_started: Mutex<Option<oneshot::Sender<()>>>,
        stop_calls: AtomicUsize,
        stop_hold: Duration,
        active_stops: AtomicUsize,
        deadline_seen: AtomicBool,
    }

    struct GatedStartStopRuntime {
        start_started: Mutex<Option<oneshot::Sender<()>>>,
        stop_started: Mutex<Option<oneshot::Sender<()>>>,
        start_calls: AtomicUsize,
        stop_calls: AtomicUsize,
        stop_hold: Duration,
        active_starts: AtomicUsize,
        active_stops: AtomicUsize,
        deadline_seen: AtomicBool,
        stop_outcomes: Mutex<std::collections::VecDeque<CanceledStartStopOutcome>>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CanceledStartStopOutcome {
        Success,
        Error,
        Panic,
    }

    struct AtomicActivityGuard<'a> {
        active: &'a AtomicUsize,
    }

    impl<'a> AtomicActivityGuard<'a> {
        fn enter(active: &'a AtomicUsize) -> Self {
            active.fetch_add(1, Ordering::SeqCst);
            Self { active }
        }
    }

    impl Drop for AtomicActivityGuard<'_> {
        fn drop(&mut self) {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn hold_runtime_stop_until_budget(
        control: &crate::pty::container_runtime::ContainerRuntimeControl,
        requested_hold: Duration,
    ) {
        if requested_hold.is_zero() {
            return;
        }
        let _ = control.wait_for_shutdown();
        let remaining = control.remaining().unwrap_or(requested_hold);
        let hold = requested_hold.min(remaining.saturating_sub(Duration::from_millis(20)));
        if !hold.is_zero() {
            std::thread::sleep(hold);
        }
    }

    impl crate::pty::container_runtime::ContainerRuntime for GatedStopRuntime {
        fn start(
            &self,
            request: crate::pty::container_runtime::ContainerStartRequest,
            _control: &crate::pty::container_runtime::ContainerRuntimeControl,
        ) -> Result<crate::pty::container_runtime::ContainerRuntimeHandle, crate::errors::AppError>
        {
            Ok(crate::pty::container_runtime::ContainerRuntimeHandle {
                session_id: request.session_id,
                container_id: format!("container-{}", request.session_id),
            })
        }

        fn stop(
            &self,
            _handle: &crate::pty::container_runtime::ContainerRuntimeHandle,
            _timeout: Duration,
            control: &crate::pty::container_runtime::ContainerRuntimeControl,
        ) -> Result<(), crate::errors::AppError> {
            let _active = AtomicActivityGuard::enter(&self.active_stops);
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self
                .stop_started
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = started.send(());
            }
            hold_runtime_stop_until_budget(control, self.stop_hold);
            self.deadline_seen
                .store(control.shutdown_deadline().is_some(), Ordering::SeqCst);
            Ok(())
        }

        fn cleanup_labeled_orphans(
            &self,
            _live_sessions: &HashSet<Uuid>,
            _timeout: Duration,
        ) -> Result<crate::pty::container_runtime::ContainerCleanupReport, crate::errors::AppError>
        {
            Ok(crate::pty::container_runtime::ContainerCleanupReport::default())
        }
    }

    impl crate::pty::container_runtime::ContainerRuntime for GatedStartStopRuntime {
        fn start(
            &self,
            request: crate::pty::container_runtime::ContainerStartRequest,
            control: &crate::pty::container_runtime::ContainerRuntimeControl,
        ) -> Result<crate::pty::container_runtime::ContainerRuntimeHandle, crate::errors::AppError>
        {
            let _active = AtomicActivityGuard::enter(&self.active_starts);
            self.start_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self
                .start_started
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = started.send(());
            }
            let _ = control.wait_for_shutdown();
            Ok(crate::pty::container_runtime::ContainerRuntimeHandle {
                session_id: request.session_id,
                container_id: format!("container-{}", request.session_id),
            })
        }

        fn stop(
            &self,
            _handle: &crate::pty::container_runtime::ContainerRuntimeHandle,
            _timeout: Duration,
            control: &crate::pty::container_runtime::ContainerRuntimeControl,
        ) -> Result<(), crate::errors::AppError> {
            let _active = AtomicActivityGuard::enter(&self.active_stops);
            let call = self.stop_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(started) = self
                .stop_started
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = started.send(());
            }
            if call == 1 {
                hold_runtime_stop_until_budget(control, self.stop_hold);
            }
            self.deadline_seen
                .store(control.shutdown_deadline().is_some(), Ordering::SeqCst);
            match self
                .stop_outcomes
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pop_front()
                .unwrap_or(CanceledStartStopOutcome::Success)
            {
                CanceledStartStopOutcome::Success => Ok(()),
                CanceledStartStopOutcome::Error => Err(crate::errors::AppError::Other(
                    "injected canceled-start stop failure".to_string(),
                )),
                CanceledStartStopOutcome::Panic => {
                    panic!("injected canceled-start stop panic")
                }
            }
        }

        fn cleanup_labeled_orphans(
            &self,
            _live_sessions: &HashSet<Uuid>,
            _timeout: Duration,
        ) -> Result<crate::pty::container_runtime::ContainerCleanupReport, crate::errors::AppError>
        {
            Ok(crate::pty::container_runtime::ContainerCleanupReport::default())
        }
    }

    fn real_container_spawn_spec(
        session_id: Uuid,
        cwd: String,
    ) -> crate::pty::backend::BackendSpawnSpec {
        crate::pty::backend::BackendSpawnSpec {
            id: session_id,
            agent_id: None,
            coding_agent: None,
            cmd: "container".to_string(),
            args: Vec::new(),
            resolved_agent_host_shell: None,
            cwd,
            selected_cwd: None,
            cols: 120,
            rows: 30,
            container_image: Some("agentscommander/test:latest".to_string()),
            configured_env: Vec::new(),
            env_remove_keys: Vec::new(),
            env_unset: Vec::new(),
            extra_env: Vec::new(),
            idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
            output_target: crate::pty::output::PtyOutputTarget::noop(),
            resource_registration: None,
            logical_resource_slot: None,
            container_credential: None,
            container_repo_mounts: Vec::new(),
            launch_witness: Default::default(),
        }
    }

    impl crate::pty::backend::PtyBackend for LifecycleTestBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async move {
                self.set_live(spec.id, true);
                Ok(())
            })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            id: Uuid,
            _data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            self.live
                .lock()
                .unwrap()
                .contains(&id)
                .then_some(())
                .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            self.write(
                &crate::pty::manager::BackendWriteAuthority::for_backend_test(),
                id,
                &[],
            )
        }

        fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
            *self.kills.lock().unwrap().entry(id).or_default() += 1;
            self.set_live(id, false);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
            self.has_session(id).then_some((30, 120))
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    #[derive(Clone, Copy)]
    enum CriticalWaitPoint {
        AdmissionPermit,
        QueueSlot,
    }

    async fn assert_cancelled_critical_waiter_releases_key(
        kind: CriticalAdmissionKind,
        wait_point: CriticalWaitPoint,
    ) {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/critical-cancellation".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create critical-cancellation fixture");
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(manager)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build critical-cancellation app");
        coordinator
            .start(app.handle().clone())
            .expect("start critical-cancellation coordinator");
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);

        let reservations = (0..COORDINATOR_QUEUE_CAPACITY)
            .map(|_| {
                coordinator
                    .reserve_auto_close()
                    .expect("reserve physical queue slot")
            })
            .collect::<Vec<_>>();
        let held_admission = match wait_point {
            CriticalWaitPoint::AdmissionPermit => Some(
                Arc::clone(&coordinator.inner.admission)
                    .try_acquire_owned()
                    .expect("hold final logical admission permit"),
            ),
            CriticalWaitPoint::QueueSlot => None,
        };

        let waiter = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.critical_probe_for_test(session.id, kind).await })
        };
        await_state_eq(
            "critical waiter reaches requested cancellation point",
            Duration::from_secs(1),
            || {
                let registered = coordinator.critical_key_registered_for_test(session.id, kind);
                match wait_point {
                    CriticalWaitPoint::AdmissionPermit => registered,
                    CriticalWaitPoint::QueueSlot => {
                        registered && coordinator.inner.admission.available_permits() == 0
                    }
                }
            },
            true,
            String::new,
        )
        .await;

        waiter.abort();
        assert!(waiter
            .await
            .expect_err("waiter must be aborted")
            .is_cancelled());
        await_state_eq(
            "aborted waiter removes its critical admission key",
            Duration::from_secs(1),
            || coordinator.critical_key_registered_for_test(session.id, kind),
            false,
            String::new,
        )
        .await;

        drop(held_admission);
        drop(reservations);
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(1),
                coordinator.critical_probe_for_test(session.id, kind),
            )
            .await
            .expect("fresh same-kind submission completes")
            .expect("fresh same-kind submission succeeds"),
            CriticalAdmissionOutcome::Completed(())
        );
        // #1580 releases the key before Completed; this bounded wait is kept unchanged.
        await_state_eq(
            "fresh probe releases its critical admission key",
            Duration::from_secs(1),
            || coordinator.critical_key_registered_for_test(session.id, kind),
            false,
            String::new,
        )
        .await;
        assert!(!coordinator.critical_key_registered_for_test(session.id, kind));
        coordinator.close_and_join().await;
    }

    // #1580 deterministic ordering detector. The worker needs `critical_keys` to free
    // the key; the test holds it. "All admission permits returned" happens after the
    // send before the fix, and before the (blocked) send after it.
    async fn assert_critical_completion_observed_after_release(shutdown_first: bool) {
        use crate::pty::backend::SessionBackendKind;
        use std::future::Future;
        use std::task::{Context, Waker};

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/critical-release-order".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create critical-release-order fixture");
        let token = CancellationToken::new();
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), token.clone());
        let app = tauri::test::mock_builder()
            .manage(manager)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build critical-release-order app");
        coordinator
            .start(app.handle().clone())
            .expect("start critical-release-order coordinator");
        let barrier = coordinator
            .submit_restore_first()
            .await
            .expect("restore barrier holds the worker");

        let kind = CriticalAdmissionKind::RouteLoss;
        let mut probe = std::pin::pin!(coordinator.critical_probe_for_test(session.id, kind));
        let mut cx = Context::from_waker(Waker::noop());
        // Queue capacity drops only in the poll that also sends the envelope.
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                assert!(
                    probe.as_mut().poll(&mut cx).is_pending(),
                    "probe cannot complete while the barrier holds the worker"
                );
                if coordinator.inner.sender.capacity() == COORDINATOR_QUEUE_CAPACITY - 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("probe queues its envelope behind the barrier");
        assert!(coordinator.critical_key_registered_for_test(session.id, kind));
        assert_eq!(
            coordinator.inner.admission.available_permits(),
            COORDINATOR_ADMISSION_CAPACITY - 2
        );
        if shutdown_first {
            token.cancel();
        }

        // Lexical scope, no `.await` inside: clippy::await_holding_lock, and the
        // worker needs this lock. Do not replace the scope with `drop(keys)`.
        {
            let keys = coordinator.inner.critical_keys.lock().unwrap();
            assert!(keys.contains(&CriticalAdmissionKey {
                session_id: session.id,
                kind,
            }));
            barrier.finish();
            let anchor = Instant::now();
            while coordinator.inner.admission.available_permits() != COORDINATOR_ADMISSION_CAPACITY
            {
                assert!(
                    anchor.elapsed() < Duration::from_secs(10),
                    "#1580: worker did not return admission before releasing the critical key"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                probe.as_mut().poll(&mut cx).is_pending(),
                "#1580: completion was observable while the critical key was still registered"
            );
        }

        let result = tokio::time::timeout(Duration::from_secs(5), probe.as_mut())
            .await
            .expect("probe completes once the key lock is free");
        assert!(!coordinator.critical_key_registered_for_test(session.id, kind));
        assert_eq!(
            coordinator.inner.admission.available_permits(),
            COORDINATOR_ADMISSION_CAPACITY
        );
        if shutdown_first {
            assert_eq!(
                result,
                Err(SelectionCoordinatorError::Unavailable.to_string())
            );
        } else {
            assert_eq!(result, Ok(CriticalAdmissionOutcome::Completed(())));
        }
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn critical_success_is_observed_only_after_admission_and_key_release() {
        assert_critical_completion_observed_after_release(false).await;
    }

    #[tokio::test]
    async fn shutdown_drained_critical_failure_is_observed_only_after_admission_and_key_release() {
        assert_critical_completion_observed_after_release(true).await;
    }

    #[tokio::test]
    async fn disposed_envelope_releases_admission_and_critical_key_before_closing_response() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        let key = CriticalAdmissionKey {
            session_id: Uuid::new_v4(),
            kind: CriticalAdmissionKind::BackgroundCleanup,
        };
        assert!(coordinator.inner.critical_keys.lock().unwrap().insert(key));
        let admission = Arc::clone(&coordinator.inner.admission)
            .try_acquire_owned()
            .expect("acquire admission for disposal fixture");
        let (response, mut receiver) = oneshot::channel();
        let envelope = CoordinatorEnvelope {
            _admission: admission,
            _critical_admission: Some(CriticalAdmissionGuard::new(&coordinator.inner, key)),
            job: CoordinatorJob::Snapshot { response },
            _create_ticket: None,
        };

        // Lexical scope; the disposer thread blocks on this lock at the key.
        let disposer =
            {
                let keys = coordinator.inner.critical_keys.lock().unwrap();
                let disposer = std::thread::spawn(move || drop(envelope));
                let anchor = Instant::now();
                while coordinator.inner.admission.available_permits()
                    != COORDINATOR_ADMISSION_CAPACITY
                {
                    assert!(
                        anchor.elapsed() < Duration::from_secs(10),
                        "#1580: disposed envelope did not return admission first"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert!(
                matches!(receiver.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
                "#1580: disposal closed the caller's response before releasing the critical key"
            );
                assert!(keys.contains(&key));
                disposer
            };
        disposer.join().expect("join envelope disposer");
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
        assert!(!coordinator.critical_key_registered_for_test(key.session_id, key.kind));
    }

    #[tokio::test]
    async fn restore_barrier_start_acknowledgement_keeps_admission_until_release() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(manager)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build restore admission test app");
        coordinator
            .start(app.handle().clone())
            .expect("start selection coordinator");

        let guard = coordinator
            .submit_restore_first()
            .await
            .expect("submit first restore job");
        assert_eq!(
            coordinator.inner.admission.available_permits(),
            COORDINATOR_ADMISSION_CAPACITY - 1,
            "RestoreBarrier start acknowledgement is not completion; admission stays held"
        );
        guard.finish();
        tokio::time::timeout(Duration::from_secs(1), async {
            while coordinator.inner.admission.available_permits() != COORDINATOR_ADMISSION_CAPACITY
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("restore barrier returns admission after release");
        coordinator.close_and_join().await;
    }

    // Proves retained pending-handle ownership and exactly-once cleanup after close.
    // Immediate stop does not cover near-deadline success, blocked stops, or physical joins.
    async fn assert_pending_container_shutdown_waits_for_stop(capped: bool) {
        use crate::pty::backend::SessionBackendKind;
        use crate::pty::container_backend::ContainerTransportBackend;
        use crate::pty::container_runtime::ContainerRuntimeHandle;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (stop_started, stop_started_rx) = oneshot::channel();
        let close_budget = Duration::from_millis(200);
        let runtime = Arc::new(GatedStopRuntime {
            stop_started: Mutex::new(Some(stop_started)),
            stop_calls: AtomicUsize::new(0),
            stop_hold: Duration::ZERO,
            active_stops: AtomicUsize::new(0),
            deadline_seen: AtomicBool::new(false),
        });
        let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let container_backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        ));
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
            Arc::new(LifecycleTestBackend::default()),
            container_backend.clone(),
        )));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build pending-container shutdown app");
        let mut restore_guard = None;
        let manager_handle = manager.read().await.clone();
        let setup = async {
            coordinator
                .start(app.handle().clone())
                .map_err(|error| error.to_string())?;
            restore_guard = Some(coordinator.submit_restore_first().await?);
            let mut ticket = coordinator
                .reserve_create(TrustedCreateIntent::Background)
                .await
                .map_err(|error| error.to_string())?;
            let pending = manager_handle
                .create_pending_session(
                    &mut ticket,
                    "container".to_string(),
                    Vec::new(),
                    "C:/pending-container-shutdown".to_string(),
                    None,
                    None,
                    Vec::new(),
                    false,
                    SessionBackendKind::ContainerTransport,
                )
                .await?;
            Ok::<_, String>((ticket, pending))
        }
        .await;
        let (ticket, pending) = match setup {
            Ok(ready) => ready,
            Err(error) => {
                if let Some(guard) = restore_guard.take() {
                    guard.finish();
                }
                let cleanup =
                    tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
                        .await;
                panic!("pending-container setup failed: {error}; coordinator cleanup={cleanup:?}; a cleanup timeout leaves worker ownership unresolved");
            }
        };
        let _transport_receiver =
            container_backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
                session_id: pending.id,
                container_id: format!("container-{}", pending.id),
            });
        pty.lock()
            .unwrap_or_else(|error| error.into_inner())
            .record_route(pending.id, SessionBackendKind::ContainerTransport);

        let mut finalizer = tokio::spawn(async move { ticket.finalize(Vec::new()).await });
        let close_started = Instant::now();
        let mut close = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                if capped {
                    coordinator.close_and_join_with_budget(close_budget).await
                } else {
                    coordinator.close_and_join().await
                }
            })
        };
        let shutdown_signal = tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator.inner.shutdown.is_cancelled() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        if !capped || shutdown_signal.is_err() {
            if let Some(guard) = restore_guard.take() {
                guard.finish();
            }
        }

        let close_result = tokio::time::timeout(Duration::from_secs(2), &mut close).await;
        let close_elapsed = close_started.elapsed();
        let close_bound = if capped {
            close_budget + Duration::from_millis(550)
        } else {
            Duration::from_secs(1)
        };
        if let Some(guard) = restore_guard.take() {
            guard.finish();
        }
        // Borrow task handles so timeout cannot silently detach fixture-owned work.
        let close_cleanup = if close_result.is_err() {
            let cleanup = tokio::time::timeout(Duration::from_secs(5), &mut close).await;
            if cleanup.is_err() {
                close.abort();
                let aborted = tokio::time::timeout(Duration::from_secs(5), &mut close).await;
                eprintln!("coordinator cleanup exceeded five seconds; abort join={aborted:?}; worker ownership unresolved");
            }
            Some(cleanup)
        } else {
            None
        };
        let finalizer_result = tokio::time::timeout(Duration::from_secs(5), &mut finalizer).await;
        if finalizer_result.is_err() {
            finalizer.abort();
            let aborted = tokio::time::timeout(Duration::from_secs(5), &mut finalizer).await;
            eprintln!("finalizer cleanup exceeded five seconds; abort join={aborted:?}");
        }
        let pre_stop_calls = runtime.stop_calls.load(Ordering::SeqCst);
        let pre_active_stops = runtime.active_stops.load(Ordering::SeqCst);
        let pre_transport = container_backend.contains_transport_state_for_test(pending.id);
        let pre_detached = container_backend.detached_cleanup_count_for_test();
        let pre_retained = container_backend.retained_runtime_cleanup_sessions_for_test();
        let pre_contexts = container_backend.retained_cleanup_contexts_for_test();
        let aggregate = manager_handle.aggregate_snapshot().await;
        let pre_route = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(pending.id);

        // Collect both snapshots and finish the single authorized sweep before assertions.
        let global_report = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_all_started_containers_blocking(Duration::from_secs(1));
        let stopped_signal = tokio::time::timeout(Duration::from_secs(1), stop_started_rx).await;
        await_state_eq(
            "global sweep stops the retained handle exactly once",
            Duration::from_secs(1),
            || runtime.stop_calls.load(Ordering::SeqCst),
            1,
            String::new,
        )
        .await;
        await_state_eq(
            "retained container stop is no longer active",
            Duration::from_secs(1),
            || runtime.active_stops.load(Ordering::SeqCst),
            0,
            String::new,
        )
        .await;
        await_state_eq(
            "no shutdown worker remains after the sweep",
            Duration::from_secs(1),
            || container_backend.shutdown_observation_for_test().3,
            0,
            String::new,
        )
        .await;
        await_state_eq(
            "retained container stop observed its deadline",
            Duration::from_secs(1),
            || runtime.deadline_seen.load(Ordering::SeqCst),
            true,
            String::new,
        )
        .await;
        await_state_eq(
            "pending cleanup ownership is released after the sweep",
            Duration::from_secs(1),
            || {
                container_backend
                    .retained_cleanup_sessions_for_test()
                    .is_empty()
            },
            true,
            String::new,
        )
        .await;
        let stop_calls = runtime.stop_calls.load(Ordering::SeqCst);
        let active_stops = runtime.active_stops.load(Ordering::SeqCst);
        let worker_count = container_backend.shutdown_worker_count_for_test();
        let deadline_seen = runtime.deadline_seen.load(Ordering::SeqCst);
        let retained = container_backend.retained_cleanup_sessions_for_test();
        eprintln!("pending-container capped={capped} close={close_result:?} close_cleanup={close_cleanup:?} terminal={} stops={stop_calls} active={active_stops} worker_accounting={worker_count} retained={retained:?}", global_report.terminal);
        shutdown_signal.expect("pending-container shutdown signal becomes visible");
        close_result
            .expect("coordinator close obeys the shared shutdown deadline")
            .expect("join coordinator close task");
        assert!(
            close_elapsed <= close_bound,
            "coordinator close elapsed {close_elapsed:?}, bound {close_bound:?}"
        );
        assert_eq!(
            finalizer_result
                .expect("pending-container finalizer joins within five seconds")
                .expect("join pending-container finalizer")
                .expect_err("shutdown finalizer returns unavailable"),
            SelectionCoordinatorError::Unavailable.to_string()
        );
        assert_eq!(pre_stop_calls, 0);
        assert_eq!(pre_active_stops, 0);
        assert!(!pre_transport);
        assert_eq!(pre_detached, 0);
        assert_eq!(
            pre_retained,
            vec![pending.id],
            "post-seal pending cleanup must remain owned for the authorized global sweep: {:?}",
            pre_contexts
        );
        assert!(aggregate.pending_ids.is_empty());
        assert!(aggregate.sessions.is_empty());
        assert!(!pre_route);
        assert!(
            global_report.terminal,
            "retained={:?}",
            global_report.retained
        );
        stopped_signal
            .expect("global sweep invokes the retained container stop")
            .expect("container stop-start signal is delivered");
        assert_eq!(
            stop_calls, 1,
            "the single production global sweep must stop the retained handle exactly once"
        );
        assert_eq!(active_stops, 0);
        // Accounting evidence only; zero does not prove physical worker thread joins.
        assert_eq!(worker_count, 0);
        assert!(deadline_seen);
        assert!(retained.is_empty());
    }

    async fn assert_real_pending_container_start_shutdown_waits_for_stop(
        capped: bool,
        fail_worker_spawn: bool,
        first_stop_outcome: CanceledStartStopOutcome,
    ) {
        use crate::pty::backend::SessionBackendKind;
        use crate::pty::container_backend::ContainerTransportBackend;
        use crate::pty::container_tokens::ContainerApiTokenManager;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let root_dir = tempfile::TempDir::new().expect("create container root directory");
        let root = root_dir.path().to_string_lossy().into_owned();
        let (start_started, start_started_rx) = oneshot::channel();
        let (stop_started, stop_started_rx) = oneshot::channel();
        let close_budget = Duration::from_millis(200);
        let runtime = Arc::new(GatedStartStopRuntime {
            start_started: Mutex::new(Some(start_started)),
            stop_started: Mutex::new(Some(stop_started)),
            start_calls: AtomicUsize::new(0),
            stop_calls: AtomicUsize::new(0),
            stop_hold: if capped {
                Duration::from_secs(30)
            } else {
                Duration::from_millis(10)
            },
            active_starts: AtomicUsize::new(0),
            active_stops: AtomicUsize::new(0),
            deadline_seen: AtomicBool::new(false),
            stop_outcomes: Mutex::new(std::collections::VecDeque::from([
                first_stop_outcome,
                CanceledStartStopOutcome::Success,
            ])),
        });
        let token_dir = tempfile::TempDir::new().expect("create container token directory");
        let token_manager =
            ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
        let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let runtime_settings = crate::config::settings::AppSettings {
            api_server_enabled: true,
            api_server_bind: "0.0.0.0".to_string(),
            api_server_port: 8765,
            ..crate::config::settings::AppSettings::default()
        };
        let mut container_backend = ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            Some(token_manager),
        );
        container_backend.set_runtime_settings_for_test(runtime_settings);
        if fail_worker_spawn {
            container_backend.inject_shutdown_worker_spawn_failure_for_test();
        }
        let container_backend = Arc::new(container_backend);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
            Arc::new(LifecycleTestBackend::default()),
            container_backend.clone(),
        )));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build real pending-container shutdown app");
        coordinator
            .start(app.handle().clone())
            .expect("start real pending-container shutdown coordinator");
        let mut restore_guard = Some(
            coordinator
                .submit_restore_first()
                .await
                .expect("hold restore barrier"),
        );
        let manager_handle = manager.read().await.clone();
        let mut ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .expect("reserve real pending-container finalizer");
        let pending = manager_handle
            .create_pending_session(
                &mut ticket,
                "container".to_string(),
                Vec::new(),
                root.clone(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::ContainerTransport,
            )
            .await
            .expect("create real pending-container manager row");
        let pending_id = pending.id;
        let create_shutdown = coordinator.inner.shutdown.clone();
        let create_pty = Arc::clone(&pty);
        let create_root = root.clone();
        let mut create = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build real-container create test runtime");
            runtime.block_on(async move {
                let _ticket = ticket;
                let _spawn_mark = create_pty
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .mark_spawning(&create_root, "container");
                tokio::select! {
                    biased;
                    _ = create_shutdown.cancelled() => {
                        Err(SelectionCoordinatorError::Unavailable.to_string())
                    }
                    result = PtyManager::spawn(
                        &create_pty,
                        SessionBackendKind::ContainerTransport,
                        real_container_spawn_spec(pending_id, create_root),
                    ) => result.map_err(|error| error.to_string()),
                }
            })
        });

        tokio::select! {
            result = &mut create => {
                panic!("real container create ended before runtime start: {result:?}");
            }
            started = tokio::time::timeout(Duration::from_secs(5), start_started_rx) => {
                started
                    .expect("runtime start reaches blocking section")
                    .expect("runtime start witness is delivered");
            }
        }
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
        assert!(container_backend.contains_transport_state_for_test(pending_id));
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.contains(&pending_id));
        assert!(aggregate.sessions.is_empty());
        let (pending_spawns, live_routes) = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .archive_liveness(&[pending_id]);
        assert_eq!(pending_spawns.len(), 1);
        assert_eq!(live_routes, vec![false]);
        assert_eq!(
            container_backend.shutdown_work_state_for_test(),
            (false, 1, 1),
            "the producer and blocking start must be shutdown-owned before runtime.start"
        );

        let close_started = Instant::now();
        let close = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                if capped {
                    coordinator.close_and_join_with_budget(close_budget).await
                } else {
                    coordinator.close_and_join().await
                }
            })
        };
        await_state_eq(
            "real pending-container shutdown signal becomes visible",
            Duration::from_secs(1),
            || coordinator.inner.shutdown.is_cancelled(),
            true,
            String::new,
        )
        .await;
        let create_error = (&mut create)
            .await
            .expect("join canceled real container create")
            .expect_err("shutdown cancels the real container create");
        if fail_worker_spawn {
            assert!(
                create_error == SelectionCoordinatorError::Unavailable.to_string()
                    || create_error == "container runtime start was canceled",
                "unexpected synchronous-fallback cancellation error: {create_error}"
            );
        } else {
            assert_eq!(
                create_error,
                SelectionCoordinatorError::Unavailable.to_string()
            );
        }
        if !capped {
            restore_guard
                .take()
                .expect("normal-drain restore guard")
                .finish();
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if manager_handle
                    .aggregate_snapshot()
                    .await
                    .pending_ids
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("pending manager row is rolled back during shutdown");
        tokio::time::timeout(Duration::from_secs(1), stop_started_rx)
            .await
            .expect("late runtime handle stop starts")
            .expect("late runtime handle stop witness is delivered");
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        let close_report = tokio::time::timeout(Duration::from_secs(2), close)
            .await
            .expect("real pending-container close obeys the shared deadline")
            .expect("join real pending-container close task");
        let close_elapsed = close_started.elapsed();
        // Sampled before the bounded drain wait below, so it can still witness
        // work the close gave up on.
        let work_state_at_close = container_backend.shutdown_work_state_for_test();
        let close_bound = if capped {
            close_budget + Duration::from_millis(550)
        } else {
            Duration::from_secs(1)
        };
        assert!(
            close_elapsed <= close_bound,
            "real container close elapsed {close_elapsed:?}, bound {close_bound:?}"
        );
        if capped {
            // Only the abandoned side is asserted. A capped close may retain work
            // without ever reaching its absolute deadline: the
            // `container-shutdown-signal` and `coordinator-worker-handle` reasons are
            // raised immediately on a `try_lock` failure, `critical-key-clear` is not
            // deadline-driven at all, and `blocking-seed-transaction-await` is raised
            // at the worker deadline, one finalization reserve before the absolute
            // one. A close that returns under budget is therefore free to retain, so
            // asserting an empty retained set there would fail runs the production
            // contract permits.
            if close_elapsed >= close_budget && work_state_at_close != (true, 0, 0) {
                // The registry only drains, never refills, so work outstanding
                // here was outstanding at the close's deadline too. The predicate
                // couples the report to something observed independently of it, so
                // it does not restate `persistence_safe == retained.is_empty()`.
                assert!(
                    !close_report.persistence_safe,
                    "a capped close that gave up on {work_state_at_close:?} must report abandoned ownership: {close_report:?}"
                );
                assert!(
                    !close_report.retained.is_empty(),
                    "a capped close that gave up on {work_state_at_close:?} must name what it abandoned: {close_report:?}"
                );
            }
            // The remaining cases, a close that returned under budget and a close
            // that reached its deadline against a registry that then drained before
            // this sample, are undecidable from outside and are left unasserted.
        }
        if let Some(guard) = restore_guard.take() {
            guard.finish();
        }

        await_state_eq(
            "runtime start was called exactly once",
            Duration::from_secs(1),
            || runtime.start_calls.load(Ordering::SeqCst),
            1,
            String::new,
        )
        .await;
        await_state_eq(
            "runtime stop was called exactly once",
            Duration::from_secs(1),
            || runtime.stop_calls.load(Ordering::SeqCst),
            1,
            String::new,
        )
        .await;
        await_state_eq(
            "no runtime start is active",
            Duration::from_secs(1),
            || runtime.active_starts.load(Ordering::SeqCst),
            0,
            String::new,
        )
        .await;
        await_state_eq(
            "no runtime stop is active",
            Duration::from_secs(1),
            || runtime.active_stops.load(Ordering::SeqCst),
            0,
            String::new,
        )
        .await;
        await_state_eq(
            "runtime stop observed the shutdown deadline",
            Duration::from_secs(1),
            || runtime.deadline_seen.load(Ordering::SeqCst),
            true,
            String::new,
        )
        .await;
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.active_starts.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 0);
        assert!(runtime.deadline_seen.load(Ordering::SeqCst));
        assert!(!container_backend.contains_transport_state_for_test(pending_id));
        let retry_expected = first_stop_outcome != CanceledStartStopOutcome::Success;
        let retained_contexts = container_backend.retained_cleanup_contexts_for_test();
        assert_eq!(
            container_backend.retained_runtime_cleanup_sessions_for_test(),
            if retry_expected {
                vec![pending_id]
            } else {
                Vec::new()
            },
            "the deterministic canceled-start handle must remain owned after a nonterminal stop: {:?}",
            retained_contexts
        );
        assert!(
            retained_contexts.iter().all(|context| {
                context.contains("runtimeHandle=false")
                    || (retry_expected
                        && context.contains(&pending_id.to_string())
                        && context.contains("runtimeHandle=true"))
            }),
            "retained cleanup must identify runtime-backed ownership separately from non-runtime residue: {retained_contexts:?}"
        );
        await_state_eq(
            "shutdown work drains",
            Duration::from_secs(5),
            || container_backend.shutdown_observation_for_test(),
            (true, 0, 0, 0),
            || {
                format!(
                    "retained={:?}",
                    container_backend.retained_cleanup_contexts_for_test()
                )
            },
        )
        .await;
        assert_eq!(
            container_backend.shutdown_work_state_for_test(),
            (true, 0, 0)
        );
        assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.is_empty());
        assert!(aggregate.sessions.is_empty());
        let (pending_spawns, live_routes) = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .archive_liveness(&[pending_id]);
        assert!(pending_spawns.is_empty());
        assert_eq!(live_routes, vec![false]);
        assert!(pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .backend_kind(pending_id)
            .is_none());

        if fail_worker_spawn {
            container_backend.clear_shutdown_worker_spawn_failure_for_test();
        }
        let global_report = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(
            global_report.terminal,
            "retained={:?}",
            global_report.retained
        );
        assert_eq!(
            runtime.stop_calls.load(Ordering::SeqCst),
            if retry_expected { 2 } else { 1 },
            "the single production global sweep must retry only a nonterminal deterministic owner"
        );
        assert!(container_backend
            .retained_cleanup_sessions_for_test()
            .is_empty());
    }

    fn production_prefix(source: &str) -> &str {
        [
            "\n#[cfg(test)]\nmod tests",
            "\n#[cfg(test)]\nimpl SessionManager",
        ]
        .into_iter()
        .filter_map(|marker| source.find(marker))
        .min()
        .map_or(source, |index| &source[..index])
    }

    fn function_name(line: &str) -> Option<String> {
        let start = line.find("fn ")? + 3;
        let name = line[start..]
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect::<String>();
        (!name.is_empty()).then_some(name)
    }

    /// #1149 - the only functions allowed to write `Session::status` in
    /// production. Each one either records the working-state edge it causes or is
    /// covered by the heartbeat backstop; an assignment anywhere else silently
    /// reintroduces an unrecorded edge.
    const STATUS_WRITER_OWNERS: [&str; 5] = [
        "mark_idle_with_carrier_permit",
        "mark_busy_with_carrier_permit",
        "prepare_pty_input_boundary_with_carrier_permit",
        "commit_selection_transition_with_carrier_permit",
        "remove_exited_sessions_with_carrier_permit",
    ];

    fn ownership_violations(files: &[(String, String)]) -> Vec<String> {
        let mut violations = Vec::new();
        for (path, source) in files {
            let mut function = String::new();
            let normalized = source.replace("\r\n", "\n");
            for (index, line) in production_prefix(&normalized).lines().enumerate() {
                if let Some(name) = function_name(line) {
                    function = name;
                }
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                // #1149 - all four variants, not only `Active`. The birth-edge
                // writer, `record.status = SessionStatus::Running` in
                // `commit_selection_transition`, was unguarded, so a future
                // assignment of exactly that shape would have passed CI and
                // silently reintroduced the unrecorded-birth defect. The guard's
                // reach ends where `production_prefix` cuts, so it protects the
                // writer table only above the `#[cfg(test)] impl SessionManager`
                // marker, which is exactly what A12 complements.
                for variant in ["Active", "Running", "Idle", "Exited"] {
                    if line.contains(&format!("= SessionStatus::{variant}"))
                        && !(path.ends_with("session/manager.rs")
                            && STATUS_WRITER_OWNERS.contains(&function.as_str()))
                    {
                        violations.push(format!(
                            "{path}:{} {variant} assignment in {function}",
                            index + 1
                        ));
                    }
                }

                for event in [
                    "session_switched",
                    "session_created",
                    "session_destroyed",
                    "session_communication_changed",
                ] {
                    if !line.contains(&format!("\"{event}\"")) {
                        continue;
                    }
                    let allowed = path.ends_with("session/selection.rs")
                        && matches!(
                            function.as_str(),
                            "publish_selection"
                                | "publish_created"
                                | "publish_destroyed"
                                | "publish_communication_cleared"
                                | "publish_session_communication"
                        );
                    if !allowed {
                        violations
                            .push(format!("{path}:{} {event} owner is {function}", index + 1));
                    }
                }

                if line.contains("pub async fn mark_exited(&self")
                    || line.contains("pub async fn get_active(&self")
                    || line.contains("pub async fn switch_session(&self")
                    || line.contains("pub async fn set_active_only(&self")
                    || line.contains("pub async fn clear_active(&self")
                    || line.contains("pub async fn clear_active_if(&self")
                {
                    violations.push(format!(
                        "{path}:{} removed production manager mutator {function}",
                        index + 1
                    ));
                }
            }
        }
        violations
    }

    fn collect_rust_sources(dir: &Path, files: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read Rust source directory") {
            let entry = entry.expect("read Rust source entry");
            let path = entry.path();
            if path.is_dir() {
                collect_rust_sources(&path, files);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push((
                    path.to_string_lossy().replace('\\', "/"),
                    std::fs::read_to_string(&path).expect("read Rust source file"),
                ));
            }
        }
    }

    fn coordinator_job_declaration(source: &str) -> &str {
        let start = source
            .find("enum CoordinatorJob")
            .expect("CoordinatorJob declaration");
        let declaration = &source[start..];
        let open = declaration.find('{').expect("CoordinatorJob opening brace");
        let mut depth = 0usize;
        for (offset, character) in declaration[open..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &declaration[..open + offset + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("CoordinatorJob closing brace");
    }

    fn coordinator_job_handle_violations(source: &str) -> Vec<&'static str> {
        let declaration = coordinator_job_declaration(source);
        [
            "SessionManager",
            "PtyManager",
            "AppHandle",
            "SelectionTransaction",
            "BoxFuture",
            "dyn Future",
            "FnOnce",
        ]
        .into_iter()
        .filter(|forbidden| declaration.contains(forbidden))
        .collect()
    }

    #[test]
    fn production_selection_and_lifecycle_sources_have_one_owner() {
        let mut files = Vec::new();
        collect_rust_sources(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let violations = ownership_violations(&files);
        assert!(
            violations.is_empty(),
            "production lifecycle ownership violations:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn source_ownership_sentinel_rejects_each_one_line_mutation() {
        let mutations = [
            "fn rogue() { row.status = SessionStatus::Active; }",
            "fn rogue() { app.emit(\"session_switched\", payload); }",
            "fn rogue() { app.emit(\"session_created\", payload); }",
            "fn rogue() { app.emit(\"session_destroyed\", payload); }",
            "fn rogue() { app.emit(\"session_communication_changed\", payload); }",
            "pub async fn mark_exited(&self) {}",
            "pub async fn get_active(&self) {}",
            "pub async fn switch_session(&self) {}",
            "pub async fn set_active_only(&self) {}",
            "pub async fn clear_active(&self) {}",
            "pub async fn clear_active_if(&self) {}",
        ];
        for mutation in mutations {
            let files = vec![("src/commands/session.rs".to_string(), mutation.to_string())];
            assert!(
                !ownership_violations(&files).is_empty(),
                "sentinel accepted mutation: {mutation}"
            );
        }
        let web_forgery = vec![(
            "src/web/commands.rs".to_string(),
            "fn validate_client_broadcast() { allow(\"session_switched\"); }".to_string(),
        )];
        assert!(
            !ownership_violations(&web_forgery).is_empty(),
            "sentinel accepted a lifecycle name in the client allowlist"
        );
    }

    #[test]
    fn the_status_writer_guard_rejects_a_rogue_running_assignment() {
        for variant in ["Active", "Running", "Idle", "Exited(0)"] {
            let rogue = format!("fn rogue() {{ row.status = SessionStatus::{variant}; }}");
            for path in ["src/commands/session.rs", "src/session/manager.rs"] {
                let files = vec![(path.to_string(), rogue.clone())];
                assert!(
                    !ownership_violations(&files).is_empty(),
                    "sentinel accepted {rogue} in {path}"
                );
            }
        }
    }

    #[test]
    fn the_status_writer_guard_accepts_the_four_allowlisted_functions() {
        let owned = "\
fn mark_idle_with_carrier_permit() { s.status = SessionStatus::Idle; }
fn mark_busy_with_carrier_permit() { s.status = SessionStatus::Running; }
fn prepare_pty_input_boundary_with_carrier_permit() { session.status = SessionStatus::Running; }
fn commit_selection_transition_with_carrier_permit() {
    record.status = SessionStatus::Running;
    record.status = SessionStatus::Exited(exit_code);
    if previous.status == SessionStatus::Active { previous.status = SessionStatus::Running; }
    target.status = SessionStatus::Active;
}
";
        let owning = vec![("src/session/manager.rs".to_string(), owned.to_string())];
        assert_eq!(
            ownership_violations(&owning),
            Vec::<String>::new(),
            "the four allowlisted writers must pass"
        );

        // The same bodies anywhere else are violations, one per assignment plus
        // the tolerated `==` comparison the substring check also matches.
        let elsewhere = vec![("src/commands/session.rs".to_string(), owned.to_string())];
        assert_eq!(ownership_violations(&elsewhere).len(), 8);
    }

    #[test]
    fn coordinator_jobs_are_typed_data_without_managed_handles_or_futures() {
        let source = include_str!("selection.rs");
        assert!(
            coordinator_job_handle_violations(source).is_empty(),
            "CoordinatorJob contains a managed handle or arbitrary executable field: {:?}",
            coordinator_job_handle_violations(source)
        );
        let commands = include_str!("../commands/session.rs");
        let payload = &commands[commands
            .find("pub(crate) struct FixturePtyCutLease")
            .unwrap()
            ..commands
                .find("impl std::fmt::Debug for FixtureReplacementInput")
                .unwrap()];
        for forbidden in [
            "SessionManager",
            "PtyManager",
            "AppHandle",
            "SelectionTransaction",
            "BoxFuture",
            "dyn Future",
            "FnOnce",
        ] {
            assert!(
                !payload.contains(forbidden),
                "fixture payload hides forbidden {forbidden}"
            );
            let mutated = format!("enum CoordinatorJob {{ Rogue {{ value: {forbidden} }} }}");
            assert_eq!(
                coordinator_job_handle_violations(&mutated),
                vec![forbidden],
                "sentinel accepted forbidden CoordinatorJob field {forbidden}"
            );
        }
    }

    #[test]
    fn initial_payload_matches_contract() {
        let epoch = Uuid::new_v4();
        let payload = SessionSelection::initial(epoch);
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(value["epoch"], epoch.to_string());
        assert_eq!(value["source"], "initialHydration");
        assert_eq!(value["userInitiated"], false);
        assert_eq!(value["revision"], 0);
        assert_eq!(value["mode"], "none");
        assert!(value["id"].is_null());
        assert!(value["status"].is_null());
        assert_eq!(value["hasPty"], false);
        assert_eq!(value["detached"], false);
        assert_eq!(value["displayable"], false);
    }

    #[test]
    fn payload_mode_invariants_and_epoch_are_stable() {
        let epoch = Uuid::new_v4();
        let id = Uuid::new_v4();
        let none = SessionSelection::none(epoch, 1, SelectionCause::AutoClose);
        let live = SessionSelection::live(epoch, 2, SelectionCause::UserSwitch, id);
        let dormant =
            SessionSelection::dormant(epoch, 3, SelectionCause::LivenessReconcile, id, 19, false);

        assert_eq!(none.epoch(), live.epoch());
        assert_eq!(live.epoch(), dormant.epoch());
        assert_eq!(none.mode(), SelectionMode::None);
        assert_eq!(none.id(), None);
        assert_eq!(none.status(), None);
        assert!(!none.has_pty() && !none.detached() && !none.displayable());
        assert_eq!(live.mode(), SelectionMode::Live);
        assert_eq!(live.id(), Some(id));
        assert_eq!(live.status(), Some(&SessionStatus::Active));
        assert!(live.has_pty() && !live.detached() && live.displayable());
        assert_eq!(dormant.mode(), SelectionMode::Dormant);
        assert_eq!(dormant.id(), Some(id));
        assert_eq!(dormant.status(), Some(&SessionStatus::Exited(19)));
        assert!(!dormant.has_pty() && !dormant.detached() && !dormant.displayable());

        let serialized = serde_json::to_value(dormant).expect("serialize dormant payload");
        assert_eq!(serialized["source"], "livenessReconcile");
        assert_eq!(serialized["userInitiated"], false);
        assert_eq!(serialized["mode"], "dormant");
    }

    #[test]
    fn authoritative_publisher_sends_byte_equivalent_tauri_and_websocket_payloads() {
        use crate::web::broadcast::WsOutMsg;
        use tauri::Listener;

        let broadcaster = WsBroadcaster::new();
        let mut web = broadcaster.subscribe();
        let app = tauri::test::mock_builder()
            .manage(broadcaster)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build publisher test app");
        let (native_sender, native_receiver) = std::sync::mpsc::channel();
        app.listen_any("session_switched", move |event| {
            let _ = native_sender.send(event.payload().to_string());
        });

        let payload = SessionSelection::live(
            Uuid::new_v4(),
            41,
            SelectionCause::UserSwitch,
            Uuid::new_v4(),
        );
        publish_selection(app.handle(), &payload);
        let native: serde_json::Value = serde_json::from_str(
            &native_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("native selection event"),
        )
        .expect("parse native selection");
        let websocket = match web.try_recv().expect("websocket selection event") {
            WsOutMsg::Text(text) => {
                serde_json::from_str::<serde_json::Value>(&text).expect("parse websocket selection")
            }
            other => panic!("expected websocket text event, got {other:?}"),
        };
        assert_eq!(websocket["event"], "session_switched");
        assert_eq!(websocket["payload"], native);
        assert_eq!(native, serde_json::to_value(payload).unwrap());
    }

    #[test]
    fn sealed_causes_pin_source_and_user_intent() {
        let cases = [
            (
                SelectionCause::UserSwitch,
                SelectionSource::UserSwitch,
                true,
            ),
            (
                SelectionCause::ManualClose,
                SelectionSource::ManualClose,
                true,
            ),
            (SelectionCause::AutoClose, SelectionSource::AutoClose, false),
            (SelectionCause::Restore, SelectionSource::Restore, false),
            (SelectionCause::Detach, SelectionSource::Detach, true),
            (SelectionCause::Attach, SelectionSource::Attach, true),
            (
                SelectionCause::SpawnRollback,
                SelectionSource::SpawnRollback,
                false,
            ),
            (
                SelectionCause::BackgroundCleanup,
                SelectionSource::BackgroundCleanup,
                false,
            ),
            (
                SelectionCause::LivenessReconcile,
                SelectionSource::LivenessReconcile,
                false,
            ),
        ];
        for (cause, source, user_initiated) in cases {
            assert_eq!(cause.source(), source);
            assert_eq!(cause.user_initiated(), user_initiated);
        }
        for (cause, source, user_initiated) in [
            (
                SelectionCause::SessionCreated(TrustedCreateIntent::User),
                SelectionSource::SessionCreated,
                true,
            ),
            (
                SelectionCause::SessionCreated(TrustedCreateIntent::Background),
                SelectionSource::SessionCreated,
                false,
            ),
            (
                SelectionCause::Restart(TrustedRestartIntent::User),
                SelectionSource::Restart,
                true,
            ),
            (
                SelectionCause::Restart(TrustedRestartIntent::Background),
                SelectionSource::Restart,
                false,
            ),
            (
                SelectionCause::ResourceMonitor(TrustedResourceIntent::User),
                SelectionSource::ResourceMonitor,
                true,
            ),
            (
                SelectionCause::ResourceMonitor(TrustedResourceIntent::Watchdog),
                SelectionSource::ResourceMonitor,
                false,
            ),
        ] {
            assert_eq!(cause.source(), source);
            assert_eq!(cause.user_initiated(), user_initiated);
        }
    }

    #[tokio::test]
    async fn bootstrapping_rejects_every_external_admission_without_allocating() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        let admission_before = coordinator.inner.admission.available_permits();
        let create_before = coordinator.inner.create_tickets.available_permits();

        assert_eq!(
            coordinator.snapshot().await.unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert_eq!(
            coordinator
                .transition(SelectionRequest::user_switch(Uuid::new_v4()))
                .await
                .unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert!(matches!(
            coordinator.reserve_auto_close(),
            Err(SelectionCoordinatorError::Busy)
        ));
        assert_eq!(
            coordinator
                .reserve_create(TrustedCreateIntent::User)
                .await
                .unwrap_err(),
            SelectionCoordinatorError::Busy
        );
        assert_eq!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(Uuid::new_v4(), 1)
                .await
                .unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert_eq!(
            coordinator
                .watchdog_resource_kill(Uuid::new_v4())
                .await
                .unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert_eq!(
            coordinator.inner.admission.available_permits(),
            admission_before
        );
        assert_eq!(
            coordinator.inner.create_tickets.available_permits(),
            create_before
        );
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn restore_is_fifo_first_and_queues_external_work_until_release() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(manager)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build restore coordinator test app");
        coordinator
            .start(app.handle().clone())
            .expect("start selection coordinator");

        let publication = crate::session::manager::acquire_carrier_mutation_permit().await;
        let canceled = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.submit_restore_first().await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while CoordinatorPhase::from_u8(coordinator.inner.phase.load(Ordering::Acquire))
                != CoordinatorPhase::Restoring
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("restore admission reaches its U wait");
        canceled.abort();
        assert!(matches!(canceled.await, Err(error) if error.is_cancelled()));
        assert_eq!(
            CoordinatorPhase::from_u8(coordinator.inner.phase.load(Ordering::Acquire)),
            CoordinatorPhase::Bootstrapping
        );
        assert!(coordinator
            .inner
            .retained_replacements
            .lock()
            .unwrap()
            .restore_nonce
            .is_none());
        drop(publication);
        let guard = coordinator
            .submit_restore_first()
            .await
            .expect("submit first restore job");
        assert_eq!(
            CoordinatorPhase::from_u8(coordinator.inner.phase.load(Ordering::Acquire)),
            CoordinatorPhase::Running
        );
        assert!(matches!(
            coordinator.submit_restore_first().await,
            Err(error) if error == SelectionCoordinatorError::Busy.to_string()
        ));
        assert!(
            coordinator.restore_is_active(),
            "Running is not restore release"
        );
        let restore_transaction = guard.transaction(app.handle().clone());
        assert!(restore_transaction.has_active_restore_capability());
        let mut queued_snapshot = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.snapshot().await })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut queued_snapshot)
                .await
                .is_err(),
            "hydration admitted after restore must remain queued behind it"
        );
        restore_transaction.invalidate_restore_ownership();
        assert!(!restore_transaction.has_active_restore_capability());
        assert!(
            coordinator.restore_is_active(),
            "lost authority does not fabricate barrier release"
        );
        assert!(
            coordinator
                .inner
                .retained_replacements
                .lock()
                .unwrap()
                .unavailable,
            "unknown restore ownership stays persistence unsafe"
        );
        guard.finish();
        assert_eq!(
            queued_snapshot
                .await
                .expect("join queued hydration")
                .expect("queued hydration succeeds")
                .revision(),
            0
        );
        assert!(!coordinator.restore_is_active());
        assert!(
            !restore_transaction.has_active_restore_capability(),
            "released nonce cannot authorize inline work"
        );
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn restore_held_route_loss_waits_then_reconciles_the_public_row() {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/restore-route-loss".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build restore-held route-loss app");
        coordinator
            .start(app.handle().clone())
            .expect("start selection coordinator");
        let guard = coordinator.submit_restore_first().await.unwrap();

        backend.set_live(session.id, false);
        pty.lock()
            .unwrap()
            .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
        let mut route_loss = {
            let sender = coordinator.container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(session.id, 71).await })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut route_loss)
                .await
                .is_err(),
            "route loss must queue behind the held restore job"
        );
        assert!(!matches!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Exited(_)
        ));

        guard.finish();
        assert_eq!(
            route_loss
                .await
                .expect("join route-loss waiter")
                .expect("route-loss reconciliation"),
            CriticalAdmissionOutcome::Completed(())
        );
        let row = manager.read().await.get_session(session.id).await.unwrap();
        assert_eq!(row.status, SessionStatus::Exited(71));
        let selection = manager.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(session.id));
        assert_eq!(selection.mode(), SelectionMode::Dormant);
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn derived_agent_id_binding_deferred_and_root_restore_install_before_publish() {
        use crate::config::sessions_persistence::{
            snapshot_sessions, CodingAgentBindingSeed, PersistedSession,
        };
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager.clone(), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(manager.clone())
            .manage(DetachedSessionsState::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        assert!(app.manage(pty));
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let transaction = guard.transaction(app.handle().clone());
        for root in [false, true] {
            let binding = Uuid::new_v4().to_string();
            let old_runtime = Uuid::new_v4().to_string();
            let seed = CodingAgentBindingSeed {
                identity_revision: format!("r1:0:{}", "a".repeat(64)),
                original_agent_id: "original".into(),
            };
            let origin = serde_json::json!({"sessionsPath":"C:/sessions.json","rawFileSHA256":"original-bytes","originalOrdinal":if root { 1 } else { 0 }});
            let cwd = if root { "C:/root" } else { "C:/deferred" };
            let info = transaction
                .restore_dormant_inline(DormantRestoreRequest {
                    persisted: PersistedSession {
                        name: cwd.into(),
                        shell: "shell".into(),
                        working_directory: cwd.into(),
                        agent_id: Some("original".into()),
                        id: Some(old_runtime.clone()),
                        created_at: Some("old-date".into()),
                        coding_agent_binding_id: Some(binding.clone()),
                        coding_agent_binding_seed: Some(seed.clone()),
                        coding_agent_binding_origin: Some(origin.clone()),
                        status: Some(SessionStatus::Exited(9)),
                        ..Default::default()
                    },
                    working_directory: cwd.into(),
                    is_coordinator: root,
                    is_root_agent: root,
                })
                .await
                .unwrap();
            assert_ne!(info.id, old_runtime);
            assert_ne!(info.created_at, "old-date");
            let handle = manager.read().await.clone();
            let snapshot = snapshot_sessions(&handle).await;
            let row = snapshot
                .iter()
                .find(|row| row.id.as_deref() == Some(&info.id))
                .unwrap();
            assert_eq!(row.coding_agent_binding_id.as_ref(), Some(&binding));
            assert_eq!(row.coding_agent_binding_seed.as_ref(), Some(&seed));
            assert_eq!(row.coding_agent_binding_origin.as_ref(), Some(&origin));
            assert_eq!(row.is_root_agent, root);
        }
        drop(transaction);
        drop(guard);
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn restore_transaction_publishes_rows_before_one_final_selection() {
        use crate::config::sessions_persistence::PersistedSession;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let broadcaster = WsBroadcaster::new();
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(DetachedSessionsState::default())
            .manage(broadcaster.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build restore transaction app");
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        assert!(app.manage(pty));
        coordinator
            .start(app.handle().clone())
            .expect("start restore coordinator");
        let guard = coordinator.submit_restore_first().await.unwrap();
        let transaction = guard.transaction(app.handle().clone());
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |event| {
                let _ = events_tx.send((event_name, event.payload().to_string()));
            });
        }

        let first = transaction
            .restore_dormant_inline(DormantRestoreRequest {
                persisted: PersistedSession {
                    name: "first".to_string(),
                    shell: "shell".to_string(),
                    shell_args: Vec::new(),
                    working_directory: "C:/first".to_string(),
                    status: Some(SessionStatus::Exited(7)),
                    ..PersistedSession::default()
                },
                working_directory: "C:/first".to_string(),
                is_coordinator: false,
                is_root_agent: false,
            })
            .await
            .unwrap();
        transaction
            .restore_dormant_inline(DormantRestoreRequest {
                persisted: PersistedSession {
                    name: "second".to_string(),
                    shell: "shell".to_string(),
                    shell_args: Vec::new(),
                    working_directory: "C:/second".to_string(),
                    status: Some(SessionStatus::Exited(8)),
                    ..PersistedSession::default()
                },
                working_directory: "C:/second".to_string(),
                is_coordinator: false,
                is_root_agent: false,
            })
            .await
            .unwrap();
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_created"
        );
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_created"
        );
        assert!(events_rx.try_recv().is_err());

        let target = Uuid::parse_str(&first.id).unwrap();
        let selection = transaction
            .restore_selection_inline(Some(target))
            .await
            .unwrap()
            .expect("restore selection changes");
        assert_eq!(selection.id(), Some(target));
        assert_eq!(selection.mode(), SelectionMode::Dormant);
        assert_eq!(selection.status(), Some(&SessionStatus::Exited(7)));
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_switched"
        );
        assert!(events_rx.try_recv().is_err());
        guard.finish();
        coordinator.close_and_join().await;
    }

    // #2451 - a restored dormant session was matched by nothing, and the tier
    // is session-lifetime state that `PersistedSession` does not carry.
    #[tokio::test]
    async fn restored_dormant_session_has_no_match_tier() {
        use crate::config::sessions_persistence::PersistedSession;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build restore transaction app");
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        assert!(app.manage(pty));
        coordinator
            .start(app.handle().clone())
            .expect("start restore coordinator");
        let guard = coordinator.submit_restore_first().await.unwrap();
        let transaction = guard.transaction(app.handle().clone());

        let restored = transaction
            .restore_dormant_inline(DormantRestoreRequest {
                persisted: PersistedSession {
                    name: "dormant".to_string(),
                    shell: "shell".to_string(),
                    shell_args: Vec::new(),
                    working_directory: "C:/dormant".to_string(),
                    status: Some(SessionStatus::Exited(0)),
                    effective_profile: Some("A".to_string()),
                    ..PersistedSession::default()
                },
                working_directory: "C:/dormant".to_string(),
                is_coordinator: false,
                is_root_agent: false,
            })
            .await
            .unwrap();

        assert_eq!(restored.match_tier, None);
        assert_eq!(restored.original_profile_letter, None);
        let id = Uuid::parse_str(&restored.id).unwrap();
        let stored = manager.read().await.get_session(id).await.unwrap();
        assert_eq!(stored.match_tier, None);
        assert_eq!(stored.original_profile_letter, None);
        guard.finish();
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn user_switch_to_already_selected_session_publishes_view_requested() {
        use crate::pty::backend::SessionBackendKind;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        // The filler absorbs the manager's automatic first-session selection, so
        // the first user switch below is a real selection change.
        manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/filler".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/already-selected".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build already-selected app");
        coordinator
            .start(app.handle().clone())
            .expect("start already-selected coordinator");
        coordinator.submit_restore_first().await.unwrap().finish();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_switched", "session_view_requested"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |event| {
                let _ = events_tx.send((event_name, event.payload().to_string()));
            });
        }

        coordinator
            .transition(SelectionRequest::user_switch(session.id))
            .await
            .unwrap()
            .expect("the first switch changes the selection");
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_view_requested"
        );
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_switched"
        );
        let revision = manager.read().await.selection_payload().await.revision();

        let committed = coordinator
            .transition(SelectionRequest::user_switch(session.id))
            .await
            .unwrap();
        assert!(
            committed.is_none(),
            "an already-selected target commits no selection payload"
        );
        let (event_name, payload) = events_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("an already-selected switch publishes the view request");
        assert_eq!(event_name, "session_view_requested");
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["id"], session.id.to_string());
        assert!(
            events_rx.try_recv().is_err(),
            "an unchanged selection publishes no session_switched"
        );
        assert_eq!(
            manager.read().await.selection_payload().await.revision(),
            revision
        );
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn user_switch_to_detached_target_publishes_no_view_requested() {
        use crate::pty::backend::SessionBackendKind;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let detached_session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/detached-target".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let live_session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/live-detached-control".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(detached_session.id, true);
        backend.set_live(live_session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(detached_session.id, SessionBackendKind::LocalProcess);
        pty.lock()
            .unwrap()
            .record_route(live_session.id, SessionBackendKind::LocalProcess);
        let detached = DetachedSessionsState::default();
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(Arc::clone(&detached))
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build detached-target app");
        coordinator
            .start(app.handle().clone())
            .expect("start detached-target coordinator");
        coordinator.submit_restore_first().await.unwrap().finish();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_switched", "session_view_requested"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |event| {
                let _ = events_tx.send((event_name, event.payload().to_string()));
            });
        }

        assert_eq!(
            manager.read().await.selection_payload().await.id(),
            Some(detached_session.id),
            "the created session is the current selection"
        );
        detached.lock().unwrap().insert(detached_session.id);

        assert!(
            coordinator
                .transition(SelectionRequest::user_switch(detached_session.id))
                .await
                .unwrap()
                .is_none(),
            "a detached target exits before commit"
        );
        coordinator
            .transition(SelectionRequest::user_switch(live_session.id))
            .await
            .unwrap()
            .expect("the control switch reaches the live session");
        // The channel was never drained, so the first event arriving after both
        // switches proves the detector was live and the detached switch emitted nothing.
        let (event_name, payload) = events_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the control switch keeps the detector live");
        assert_eq!(event_name, "session_view_requested");
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["id"], live_session.id.to_string());
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_switched"
        );
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn user_switch_to_session_without_live_pty_publishes_no_view_requested() {
        use crate::pty::backend::SessionBackendKind;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let dead_session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/no-live-pty".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let live_session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/live-after-repair".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(dead_session.id, true);
        backend.set_live(live_session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(dead_session.id, SessionBackendKind::LocalProcess);
        pty.lock()
            .unwrap()
            .record_route(live_session.id, SessionBackendKind::LocalProcess);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build no-live-pty app");
        coordinator
            .start(app.handle().clone())
            .expect("start no-live-pty coordinator");
        coordinator.submit_restore_first().await.unwrap().finish();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_switched", "session_view_requested"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |event| {
                let _ = events_tx.send((event_name, event.payload().to_string()));
            });
        }

        // Precondition: the session that will lose its PTY is the current selection.
        assert_eq!(
            manager.read().await.selection_payload().await.id(),
            Some(dead_session.id),
            "the created session is the current selection"
        );
        backend.set_live(dead_session.id, false);
        pty.lock()
            .unwrap()
            .remove_route_if_kind(dead_session.id, SessionBackendKind::LocalProcess);

        assert_eq!(
            coordinator
                .transition(SelectionRequest::user_switch(dead_session.id))
                .await
                .unwrap_err(),
            "Session has no live PTY"
        );
        coordinator
            .transition(SelectionRequest::user_switch(live_session.id))
            .await
            .unwrap()
            .expect("the control switch reaches the live session");

        // The channel was never drained: the repair event is the in-test positive
        // control that proves the dead switch emitted no view request.
        let (repair_name, repair_payload) = events_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the liveness repair publishes a clearing selection event");
        assert_eq!(repair_name, "session_switched");
        let repair_payload: serde_json::Value = serde_json::from_str(&repair_payload).unwrap();
        assert!(repair_payload["id"].is_null());
        assert_eq!(repair_payload["source"], "livenessReconcile");

        let (view_name, view_payload) = events_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(view_name, "session_view_requested");
        let view_payload: serde_json::Value = serde_json::from_str(&view_payload).unwrap();
        assert_eq!(view_payload["id"], live_session.id.to_string());

        let (switched_name, switched_payload) =
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(switched_name, "session_switched");
        let switched_payload: serde_json::Value = serde_json::from_str(&switched_payload).unwrap();
        assert_eq!(switched_payload["id"], live_session.id.to_string());

        assert!(events_rx.try_recv().is_err());
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn route_loss_publishes_destroyed_exited_row_and_one_dormant_selection() {
        use crate::pty::backend::SessionBackendKind;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/work".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let broadcaster = WsBroadcaster::new();
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(broadcaster)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build route-loss app");
        coordinator
            .start(app.handle().clone())
            .expect("start route-loss coordinator");
        coordinator.submit_restore_first().await.unwrap().finish();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_destroyed", "session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |event| {
                let _ = events_tx.send((event_name, event.payload().to_string()));
            });
        }

        backend.set_live(session.id, false);
        pty.lock()
            .unwrap()
            .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
        let sender = coordinator.container_lifecycle_sender();
        assert_eq!(
            sender.route_lost(session.id, 44).await.unwrap(),
            CriticalAdmissionOutcome::Completed(())
        );
        let row = manager.read().await.get_session(session.id).await.unwrap();
        assert_eq!(row.status, SessionStatus::Exited(44));
        let selection = manager.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(session.id));
        assert_eq!(selection.mode(), SelectionMode::Dormant);
        assert_eq!(selection.status(), Some(&SessionStatus::Exited(44)));
        assert_eq!(selection.source(), SelectionSource::LivenessReconcile);
        let observed = (0..3)
            .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0)
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec!["session_destroyed", "session_created", "session_switched"]
        );
        let revision = selection.revision();
        assert_eq!(
            sender.route_lost(session.id, 99).await.unwrap(),
            CriticalAdmissionOutcome::Completed(())
        );
        assert_eq!(
            manager.read().await.selection_payload().await.revision(),
            revision
        );
        assert!(events_rx.try_recv().is_err());
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn admission_and_create_ticket_budgets_are_exact_and_fail_fast() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);

        let mut create_tickets = Vec::new();
        for _ in 0..CREATE_TICKET_CAPACITY {
            create_tickets.push(
                coordinator
                    .reserve_create(TrustedCreateIntent::Background)
                    .await
                    .expect("create ticket within budget"),
            );
        }
        assert_eq!(
            coordinator
                .reserve_create(TrustedCreateIntent::Background)
                .await
                .expect_err("seventeenth slow create must fail"),
            SelectionCoordinatorError::Busy
        );

        let mut general = Vec::new();
        for _ in 0..(COORDINATOR_QUEUE_CAPACITY - CREATE_TICKET_CAPACITY) {
            general.push(
                coordinator
                    .reserve_auto_close()
                    .expect("unparked queue capacity remains available"),
            );
        }
        assert!(matches!(
            coordinator.reserve_auto_close(),
            Err(SelectionCoordinatorError::Busy)
        ));
        assert_eq!(
            COORDINATOR_ADMISSION_CAPACITY,
            COORDINATOR_QUEUE_CAPACITY + 1
        );

        drop(general);
        drop(create_tickets);
        coordinator
            .reserve_auto_close()
            .expect("dropped reservations return capacity");
    }

    #[tokio::test]
    async fn route_loss_admission_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::RouteLoss,
            CriticalWaitPoint::AdmissionPermit,
        )
        .await;
    }

    #[tokio::test]
    async fn route_loss_queue_slot_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::RouteLoss,
            CriticalWaitPoint::QueueSlot,
        )
        .await;
    }

    #[tokio::test]
    async fn watchdog_admission_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::WatchdogKill,
            CriticalWaitPoint::AdmissionPermit,
        )
        .await;
    }

    #[tokio::test]
    async fn watchdog_queue_slot_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::WatchdogKill,
            CriticalWaitPoint::QueueSlot,
        )
        .await;
    }

    #[tokio::test]
    async fn background_cleanup_admission_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::BackgroundCleanup,
            CriticalWaitPoint::AdmissionPermit,
        )
        .await;
    }

    #[tokio::test]
    async fn background_cleanup_queue_slot_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::BackgroundCleanup,
            CriticalWaitPoint::QueueSlot,
        )
        .await;
    }

    #[tokio::test]
    async fn full_queue_critical_route_waiter_is_deduplicated_and_runs_after_capacity_returns() {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/critical-capacity".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build critical-capacity app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let mut reservations = (0..COORDINATOR_QUEUE_CAPACITY)
            .map(|_| coordinator.reserve_auto_close().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(coordinator.inner.admission.available_permits(), 0);

        backend.set_live(session.id, false);
        pty.lock()
            .unwrap()
            .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
        let mut first_waiter = {
            let sender = coordinator.container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(session.id, 82).await })
        };
        await_state_eq(
            "critical waiter registers while the queue is full",
            Duration::from_secs(1),
            || coordinator.inner.critical_keys.lock().unwrap().len(),
            1,
            String::new,
        )
        .await;
        assert_eq!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(session.id, 99)
                .await
                .unwrap(),
            CriticalAdmissionOutcome::AlreadyPending
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut first_waiter)
                .await
                .is_err(),
            "critical waiter must remain pending while all logical capacity is held"
        );

        drop(reservations.pop());
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(reservations);
        guard.finish();
        assert_eq!(
            first_waiter
                .await
                .expect("join critical waiter")
                .expect("critical route-loss result"),
            CriticalAdmissionOutcome::Completed(())
        );
        // Completed is observed before the first waiter's admission guard drops.
        await_state_eq(
            "route-loss waiter releases every critical key",
            Duration::from_secs(1),
            || coordinator.inner.critical_keys.lock().unwrap().len(),
            0,
            String::new,
        )
        .await;
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        assert_eq!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Exited(82)
        );
        assert_eq!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(session.id, 100)
                .await
                .unwrap(),
            CriticalAdmissionOutcome::Completed(())
        );
        // Completed is observed before the follow-up probe's admission guard drops.
        await_state_eq(
            "route-loss waiter releases every critical key",
            Duration::from_secs(1),
            || coordinator.inner.critical_keys.lock().unwrap().len(),
            0,
            String::new,
        )
        .await;
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn full_queue_keeps_critical_kinds_distinct_and_deduplicates_each_kind() {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/critical-kind-capacity".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(pty)
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build critical-kind app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let reservations = (0..COORDINATOR_QUEUE_CAPACITY)
            .map(|_| coordinator.reserve_auto_close().unwrap())
            .collect::<Vec<_>>();

        let route = {
            let sender = coordinator.container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(session.id, 31).await })
        };
        let watchdog = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
        };
        let cleanup = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.background_destroy(session.id).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while coordinator.inner.critical_keys.lock().unwrap().len() != 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all critical kinds register independently at full capacity");

        assert!(matches!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(session.id, 32)
                .await
                .unwrap(),
            CriticalAdmissionOutcome::AlreadyPending
        ));
        assert!(matches!(
            coordinator
                .watchdog_resource_kill(session.id)
                .await
                .unwrap(),
            WatchdogKillOutcome::AlreadyInFlight
        ));
        assert!(matches!(
            coordinator.background_destroy(session.id).await.unwrap(),
            CriticalAdmissionOutcome::AlreadyPending
        ));

        coordinator
            .close_and_join_with_budget(Duration::from_millis(25))
            .await;
        guard.finish();
        drop(reservations);
        for error in [
            route.await.unwrap().unwrap_err(),
            watchdog.await.unwrap().unwrap_err(),
            cleanup.await.unwrap().unwrap_err(),
        ] {
            assert_eq!(error, SelectionCoordinatorError::Unavailable.to_string());
        }
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        assert!(coordinator.inner.worker.lock().unwrap().is_none());
    }

    // #1151 (S1) - an absent session UUID reports NoPublicSession, distinctly from the
    // in-flight case, and does so before any admission is reserved or job enqueued.
    #[tokio::test]
    async fn watchdog_kill_reports_no_public_session_for_absent_uuid() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(pty)
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build no-public-session app");
        coordinator.start(app.handle().clone()).unwrap();
        let restore = coordinator.submit_restore_first().await.unwrap();

        // Every envelope slot is taken and the restore barrier is still held, so ANY
        // attempt to reserve a critical admission would park indefinitely. Returning
        // promptly is therefore proof that the absent-session check runs before the
        // reservation and before a CoordinatorJob::ResourceKill can be enqueued.
        let mut reservations = Vec::new();
        while let Ok(reservation) = coordinator.reserve_auto_close() {
            reservations.push(reservation);
        }
        assert!(!reservations.is_empty());

        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            coordinator.watchdog_resource_kill(Uuid::new_v4()),
        )
        .await
        .expect("absent-session watchdog kill returns without reserving admission")
        .unwrap();
        assert!(matches!(outcome, WatchdogKillOutcome::NoPublicSession));
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());

        restore.finish();
        coordinator.close_and_join().await;
        drop(reservations);
    }

    // #1151 (S2) - with the WatchdogKill critical key held for a PUBLIC uuid, the second
    // call reports AlreadyInFlight, never NoPublicSession. The orphan fallback keys off
    // the second variant only, so conflating them would re-enter a live cleanup.
    #[tokio::test]
    async fn watchdog_kill_reports_already_in_flight_when_key_held() {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/watchdog-in-flight".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(pty)
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build already-in-flight app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let reservations = (0..COORDINATOR_QUEUE_CAPACITY)
            .map(|_| coordinator.reserve_auto_close().unwrap())
            .collect::<Vec<_>>();

        let waiter = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator
                .critical_key_registered_for_test(session.id, CriticalAdmissionKind::WatchdogKill)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first watchdog kill registers its critical key");

        assert!(matches!(
            coordinator
                .watchdog_resource_kill(session.id)
                .await
                .unwrap(),
            WatchdogKillOutcome::AlreadyInFlight
        ));

        coordinator
            .close_and_join_with_budget(Duration::from_millis(25))
            .await;
        guard.finish();
        drop(reservations);
        assert_eq!(
            waiter.await.unwrap().unwrap_err(),
            SelectionCoordinatorError::Unavailable.to_string()
        );
    }

    // #1151 (S3) - the absent-session outcomes of the two SIBLING entry points are
    // deliberately inconsistent with each other and are left exactly as they are. A
    // dedicated WatchdogKillOutcome exists so nobody normalizes all three at once.
    #[tokio::test]
    async fn absent_session_outcomes_for_route_loss_and_background_destroy_are_unchanged() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(pty)
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build absent-session sibling app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();

        assert!(matches!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(Uuid::new_v4(), 7)
                .await
                .unwrap(),
            CriticalAdmissionOutcome::Completed(())
        ));
        assert!(matches!(
            coordinator
                .background_destroy(Uuid::new_v4())
                .await
                .unwrap(),
            CriticalAdmissionOutcome::AlreadyPending
        ));
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn shutdown_drain_runs_common_resource_rollback_for_finalize_and_drop_jobs() {
        use crate::pty::backend::{PtyBackend, SessionBackendKind};

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LifecycleTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build shutdown-drain app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let manager_handle = manager.read().await.clone();

        let mut finalizer_ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let finalizer_session = manager_handle
            .create_pending_session(
                &mut finalizer_ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/shutdown-finalizer".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(finalizer_session.id, true);
        pty.lock()
            .unwrap()
            .record_route(finalizer_session.id, SessionBackendKind::LocalProcess);

        let mut drop_ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let drop_session = manager_handle
            .create_pending_session(
                &mut drop_ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/shutdown-drop".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(drop_session.id, true);
        pty.lock()
            .unwrap()
            .record_route(drop_session.id, SessionBackendKind::LocalProcess);

        let finalizer = tokio::spawn(async move { finalizer_ticket.finalize(Vec::new()).await });
        drop(drop_ticket);
        tokio::task::yield_now().await;
        let close = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.close_and_join().await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator.inner.shutdown.is_cancelled() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown signal becomes visible");
        guard.finish();
        close.await.expect("join close task");

        assert_eq!(
            finalizer
                .await
                .expect("join finalizer caller")
                .expect_err("queued finalizer is unavailable during shutdown"),
            SelectionCoordinatorError::Unavailable.to_string()
        );
        assert_eq!(backend.kill_count(finalizer_session.id), 1);
        assert_eq!(backend.kill_count(drop_session.id), 1);
        assert!(!backend.has_session(finalizer_session.id));
        assert!(!backend.has_session(drop_session.id));
        assert!(!pty.lock().unwrap().has_session(finalizer_session.id));
        assert!(!pty.lock().unwrap().has_session(drop_session.id));
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.is_empty());
        assert!(aggregate.sessions.is_empty());
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        assert!(coordinator.inner.worker.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn normal_shutdown_drain_joins_pending_container_stop_exactly_once() {
        assert_pending_container_shutdown_waits_for_stop(false).await;
    }

    #[tokio::test]
    async fn normal_shutdown_drain_joins_real_pending_container_start_and_stop_exactly_once() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            false,
            false,
            CanceledStartStopOutcome::Success,
        )
        .await;
    }

    #[tokio::test]
    async fn cancelled_and_panicked_post_spawn_creates_roll_back_without_ghost_events() {
        use crate::pty::backend::SessionBackendKind;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LifecycleTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build cancellation rollback app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();
        let manager_handle = manager.read().await.clone();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |_| {
                let _ = events_tx.send(event_name);
            });
        }

        let mut cancelled_ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let cancelled_session = manager_handle
            .create_pending_session(
                &mut cancelled_ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/cancelled-create".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(cancelled_session.id, true);
        pty.lock()
            .unwrap()
            .record_route(cancelled_session.id, SessionBackendKind::LocalProcess);
        let (cancel_started, cancel_started_rx) = oneshot::channel();
        let cancelled_task = tokio::spawn(async move {
            let ticket = cancelled_ticket;
            let _ = cancel_started.send(());
            std::future::pending::<()>().await;
            drop(ticket);
        });
        cancel_started_rx.await.unwrap();
        cancelled_task.abort();
        assert!(cancelled_task.await.unwrap_err().is_cancelled());

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let aggregate = manager_handle.aggregate_snapshot().await;
                if !aggregate.pending_ids.contains(&cancelled_session.id)
                    && !pty.lock().unwrap().has_session(cancelled_session.id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled create rollback completes");
        assert_eq!(backend.kill_count(cancelled_session.id), 1);

        let mut panicked_ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let panicked_session = manager_handle
            .create_pending_session(
                &mut panicked_ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/panicked-create".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(panicked_session.id, true);
        pty.lock()
            .unwrap()
            .record_route(panicked_session.id, SessionBackendKind::LocalProcess);
        let panicked_task = tokio::spawn(async move {
            let _ticket = panicked_ticket;
            panic!("synthetic create task panic");
        });
        assert!(panicked_task.await.unwrap_err().is_panic());

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let aggregate = manager_handle.aggregate_snapshot().await;
                if !aggregate.pending_ids.contains(&panicked_session.id)
                    && !pty.lock().unwrap().has_session(panicked_session.id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("panicked create rollback completes");
        assert_eq!(backend.kill_count(panicked_session.id), 1);
        assert!(manager_handle
            .aggregate_snapshot()
            .await
            .sessions
            .is_empty());
        assert!(events_rx.try_recv().is_err());
        assert_eq!(
            coordinator.inner.create_tickets.available_permits(),
            CREATE_TICKET_CAPACITY
        );
        assert_eq!(
            coordinator.inner.admission.available_permits(),
            COORDINATOR_ADMISSION_CAPACITY
        );
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn capped_shutdown_aborts_a_held_worker_then_cleans_pending_resources_and_keys() {
        use crate::pty::backend::{PtyBackend, SessionBackendKind};

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LifecycleTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build capped shutdown app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let manager_handle = manager.read().await.clone();

        let mut ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let pending = manager_handle
            .create_pending_session(
                &mut ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/aborted-shutdown".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(pending.id, true);
        pty.lock()
            .unwrap()
            .record_route(pending.id, SessionBackendKind::LocalProcess);
        let finalizer = tokio::spawn(async move { ticket.finalize(Vec::new()).await });
        let route_waiter = {
            let sender = coordinator.container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(pending.id, 9).await })
        };
        await_state_eq(
            "route-loss key is registered behind the held worker",
            Duration::from_secs(1),
            || coordinator.inner.critical_keys.lock().unwrap().is_empty(),
            false,
            String::new,
        )
        .await;

        let started = std::time::Instant::now();
        coordinator
            .close_and_join_with_budget(Duration::from_millis(25))
            .await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "capped coordinator shutdown must not wait for the held transaction"
        );
        guard.finish();

        assert_eq!(
            finalizer.await.unwrap().unwrap_err(),
            SelectionCoordinatorError::Unavailable.to_string()
        );
        assert_eq!(
            route_waiter.await.unwrap().unwrap_err(),
            SelectionCoordinatorError::Unavailable.to_string()
        );
        await_state_eq(
            "capped close cleanup kills the pending session",
            Duration::from_secs(1),
            || {
                (
                    backend.kill_count(pending.id),
                    backend.has_session(pending.id),
                    pty.lock().unwrap().has_session(pending.id),
                )
            },
            (1, false, false),
            String::new,
        )
        .await;
        assert_eq!(backend.kill_count(pending.id), 1);
        assert!(!backend.has_session(pending.id));
        assert!(!pty.lock().unwrap().has_session(pending.id));
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if manager_handle
                    .aggregate_snapshot()
                    .await
                    .pending_ids
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("retained pending-create rollback reaches terminal manager state");
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.is_empty());
        assert!(aggregate.sessions.is_empty());
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        assert!(coordinator.inner.worker.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn capped_shutdown_abort_joins_pending_container_stop_exactly_once() {
        // The restore barrier forces capped close; the later sweep owns immediate cleanup.
        // Worker counters are accounting, not proof of physical joins or deadline races.
        assert_pending_container_shutdown_waits_for_stop(true).await;
    }

    #[tokio::test]
    async fn capped_shutdown_abort_joins_real_pending_container_start_and_stop_exactly_once() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            true,
            false,
            CanceledStartStopOutcome::Success,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_real_start_worker_spawn_failure_falls_back_without_losing_handle() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            true,
            true,
            CanceledStartStopOutcome::Success,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_real_start_stop_error_retains_and_retries_the_deterministic_handle() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            true,
            true,
            CanceledStartStopOutcome::Error,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_real_start_stop_panic_retains_and_retries_the_deterministic_handle() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            true,
            true,
            CanceledStartStopOutcome::Panic,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installed_handle_cleanup_worker_spawn_failure_falls_back_without_panic() {
        use crate::pty::backend::SessionBackendKind;
        use crate::pty::container_backend::ContainerTransportBackend;
        use crate::pty::container_runtime::ContainerRuntimeHandle;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (stop_started, stop_started_rx) = oneshot::channel();
        let runtime = Arc::new(GatedStopRuntime {
            stop_started: Mutex::new(Some(stop_started)),
            stop_calls: AtomicUsize::new(0),
            stop_hold: Duration::from_secs(30),
            active_stops: AtomicUsize::new(0),
            deadline_seen: AtomicBool::new(false),
        });
        let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let container_backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        ));
        container_backend.inject_shutdown_worker_spawn_failure_for_test();
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
            Arc::new(LifecycleTestBackend::default()),
            container_backend.clone(),
        )));
        let session_id = Uuid::new_v4();
        let _transport_receiver =
            container_backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
                session_id,
                container_id: format!("container-{session_id}"),
            });
        pty.lock()
            .unwrap_or_else(|error| error.into_inner())
            .record_route(session_id, SessionBackendKind::ContainerTransport);

        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build installed-handle admission-failure app");
        coordinator
            .start(app.handle().clone())
            .expect("start installed-handle admission-failure coordinator");
        coordinator
            .submit_restore_first()
            .await
            .expect("submit installed-handle restore")
            .finish();

        let kill_pty = Arc::clone(&pty);
        let kill = tokio::task::spawn_blocking(move || {
            match catch_unwind(AssertUnwindSafe(|| {
                kill_pty
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .kill(session_id)
            })) {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(_) => Err("cleanup admission failure panicked".to_string()),
            }
        });
        tokio::time::timeout(Duration::from_secs(1), stop_started_rx)
            .await
            .expect("installed-handle fallback stop starts")
            .expect("installed-handle fallback stop witness is delivered");
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 1);
        assert!(!container_backend.contains_transport_state_for_test(session_id));
        assert_eq!(
            container_backend.shutdown_work_state_for_test(),
            (false, 1, 1)
        );
        assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);

        let close_budget = Duration::from_millis(200);
        let close_started = Instant::now();
        coordinator.close_and_join_with_budget(close_budget).await;
        assert!(
            close_started.elapsed() <= close_budget + Duration::from_millis(550),
            "installed-handle close must consume the shared deadline"
        );
        tokio::time::timeout(Duration::from_secs(1), kill)
            .await
            .expect("installed-handle fallback kill finishes after shutdown cancellation")
            .expect("join installed-handle fallback kill")
            .expect("cleanup admission failure must not panic or lose the stop");

        let manager_handle = manager.read().await.clone();
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.sessions.is_empty());
        assert!(aggregate.pending_ids.is_empty());
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 0);
        assert!(runtime.deadline_seen.load(Ordering::SeqCst));
        assert!(!container_backend.contains_transport_state_for_test(session_id));
        assert!(!pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(session_id));
        await_state_eq(
            "installed-handle cleanup drains shutdown work",
            Duration::from_secs(1),
            || container_backend.shutdown_observation_for_test(),
            (true, 0, 0, 0),
            String::new,
        )
        .await;
        assert_eq!(
            container_backend.shutdown_work_state_for_test(),
            (true, 0, 0)
        );
        assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);
        pty.lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_all_started_containers_blocking(Duration::from_secs(1));
        assert_eq!(
            runtime.stop_calls.load(Ordering::SeqCst),
            1,
            "later global sweep must not repeat the recovered stop"
        );
    }

    #[tokio::test]
    async fn joined_worker_retains_no_coordinator_or_extra_pty_owner() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LifecycleTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let weak_inner = Arc::downgrade(&coordinator.inner);
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(Arc::new(tokio::sync::RwLock::new(
                crate::config::settings::AppSettings::default(),
            )))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build retention test app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();
        // Real entry admission, worker return, source lease release and result
        // publication must reclaim capacity before the next accepted entry.
        for _ in 0..(COORDINATOR_ADMISSION_CAPACITY + 1) {
            let error = coordinator.restart_lifecycle(
                app.handle().clone(),
                crate::commands::session::RestartJobRequest {
                    session_id: Uuid::new_v4(),
                    agent_id: None,
                    requested_profile: None,
                    skip_auto_resume: None,
                    activate_after: false,
                    intent: TrustedRestartIntent::Background,
                    communication_override: None,
                    enforcement: crate::config::sessions_persistence::default_creation_gate_enforcement(),
                },
            ).await.unwrap_err();
            assert_eq!(error, "Session not found");
            assert!(coordinator
                .inner
                .retained_replacements
                .lock()
                .unwrap()
                .entries
                .is_empty());
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let pending = {
            let coordinator = coordinator.clone();
            let app = app.handle().clone();
            tokio::spawn(async move {
                coordinator.restart_lifecycle(app, crate::commands::session::RestartJobRequest {
                    session_id: Uuid::new_v4(), agent_id: None, requested_profile: None,
                    skip_auto_resume: None, activate_after: false,
                    intent: TrustedRestartIntent::Background, communication_override: None,
                    enforcement: crate::config::sessions_persistence::default_creation_gate_enforcement(),
                }).await
            })
        };
        let owner = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let owner = coordinator
                    .inner
                    .retained_replacements
                    .lock()
                    .unwrap()
                    .entries
                    .values()
                    .next()
                    .cloned();
                if let Some(owner) = owner {
                    let queued = owner.state.lock().unwrap().stage == ReplacementStage::Queued;
                    if queued {
                        break owner;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("real worker reaches the current-authority U wait");
        let mut unsafe_before = Vec::new();
        coordinator.inspect_retained_entries(&mut unsafe_before);
        assert!(!unsafe_before.is_empty(), "active entry is not terminal");
        coordinator.seal_retained_entries();
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        drop(permit);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if owner.state.lock().unwrap().stage == ReplacementStage::EntryTerminal {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("caller loss does not stop the accepted core");
        assert_eq!(
            coordinator
                .inner
                .retained_replacements
                .lock()
                .unwrap()
                .entries
                .len(),
            1,
            "seal prohibits removal of the now verified terminal entry"
        );
        let mut unsafe_after = Vec::new();
        coordinator.inspect_retained_entries(&mut unsafe_after);
        assert!(
            unsafe_after.is_empty(),
            "real returned/released entry is safe after seal: {unsafe_after:?}"
        );
        drop(owner);
        coordinator.close_and_join().await;
        assert!(coordinator.inner.worker.lock().unwrap().is_none());

        drop(coordinator);
        assert!(weak_inner.upgrade().is_none());
        assert_eq!(Arc::strong_count(&manager), 2);
        assert_eq!(Arc::strong_count(&pty), 2);
        assert_eq!(Arc::strong_count(&backend), 2);
        drop(app);
    }

    #[tokio::test]
    async fn create_auto_selection_precondition_exists_only_for_starting_none() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        let none_ticket = coordinator
            .reserve_create(TrustedCreateIntent::User)
            .await
            .expect("reserve under none");
        assert!(none_ticket.auto_select_precondition.is_some());
        drop(none_ticket);

        manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/work".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("test fixture live session");
        let live_ticket = coordinator
            .reserve_create(TrustedCreateIntent::User)
            .await
            .expect("reserve under live");
        assert!(live_ticket.auto_select_precondition.is_none());
    }

    #[tokio::test]
    async fn worker_task_local_rejects_recursive_external_submission() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        let error = IN_SELECTION_WORKER
            .scope((), async {
                coordinator
                    .snapshot()
                    .await
                    .expect_err("recursive snapshot must fail")
            })
            .await;
        assert_eq!(
            error,
            SelectionCoordinatorError::RecursiveSubmission.to_string()
        );

        let reserve_error = IN_SELECTION_WORKER
            .scope((), async {
                coordinator
                    .reserve_create(TrustedCreateIntent::User)
                    .await
                    .unwrap_err()
            })
            .await;
        assert_eq!(
            reserve_error,
            SelectionCoordinatorError::RecursiveSubmission
        );
        let auto_close_error = IN_SELECTION_WORKER
            .scope((), async {
                match coordinator.reserve_auto_close() {
                    Err(error) => error,
                    Ok(_) => panic!("recursive auto-close reservation must fail"),
                }
            })
            .await;
        assert_eq!(
            auto_close_error,
            SelectionCoordinatorError::RecursiveSubmission
        );
        let route_error = IN_SELECTION_WORKER
            .scope((), async {
                coordinator
                    .container_lifecycle_sender()
                    .route_lost(Uuid::new_v4(), 1)
                    .await
                    .unwrap_err()
            })
            .await;
        assert_eq!(
            route_error,
            SelectionCoordinatorError::RecursiveSubmission.to_string()
        );
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    }

    /// #1940 - the owned operation turn serializes the whole persisted
    /// publication -> restart settlement span: a queued guarded mutation cannot
    /// acknowledge until the first operation releases it, and the release lands
    /// after the settlement step, never between publication and settlement.
    #[tokio::test]
    async fn issue_1937_operation_turn_serializes() {
        let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let (a_entered_tx, a_entered_rx) = oneshot::channel::<()>();
        let (release_a_tx, release_a_rx) = oneshot::channel::<()>();

        let events_a = Arc::clone(&events);
        let first = tokio::spawn(run_owned_selection_operation(move || {
            let events = Arc::clone(&events_a);
            async move {
                events.lock().unwrap().push("a:publish");
                assert!(
                    selection_operation_turn().try_write().is_err(),
                    "the publication phase must run under the owned turn"
                );
                a_entered_tx.send(()).expect("signal first enter");
                release_a_rx.await.expect("release first operation");
                assert!(
                    selection_operation_turn().try_write().is_err(),
                    "the restart settlement span must stay under the owned turn"
                );
                events.lock().unwrap().push("a:settle");
                Ok::<(), String>(())
            }
        }));
        a_entered_rx.await.expect("first operation entered");

        let events_b = Arc::clone(&events);
        let (b_entered_tx, mut b_entered_rx) = oneshot::channel::<()>();
        let second = tokio::spawn(run_owned_selection_operation(move || {
            let events = Arc::clone(&events_b);
            async move {
                events.lock().unwrap().push("b:publish");
                b_entered_tx.send(()).expect("signal second enter");
                Ok::<(), String>(())
            }
        }));
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(
            !second.is_finished(),
            "the queued guarded mutation must not acknowledge while the first holds the turn"
        );
        assert!(
            b_entered_rx.try_recv().is_err(),
            "the queued guarded mutation must not enter before the turn is released"
        );
        assert_eq!(&*events.lock().unwrap(), &["a:publish"]);

        release_a_tx.send(()).expect("release the first operation");
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert_eq!(
            &*events.lock().unwrap(),
            &["a:publish", "a:settle", "b:publish"]
        );
    }

    /// #1940 - a submission from inside the selection worker must be rejected
    /// before waiting, for both the owned helper and the raw acquisition, and
    /// rejection must not consume the turn.
    #[tokio::test]
    async fn issue_1937_operation_turn_rejects_worker() {
        let executed = Arc::new(AtomicBool::new(false));
        let executed_probe = Arc::clone(&executed);
        let error = IN_SELECTION_WORKER
            .scope((), async move {
                run_owned_selection_operation(move || async move {
                    executed_probe.store(true, Ordering::SeqCst);
                    Ok::<(), String>(())
                })
                .await
            })
            .await
            .unwrap_err();
        assert_eq!(
            error,
            SelectionCoordinatorError::RecursiveSubmission.to_string()
        );
        assert!(
            !executed.load(Ordering::SeqCst),
            "the rejected operation must never run"
        );

        let acquire_error = IN_SELECTION_WORKER
            .scope((), async {
                acquire_selection_operation_turn()
                    .await
                    .err()
                    .expect("worker-local acquisition must be rejected")
            })
            .await;
        assert_eq!(
            acquire_error,
            SelectionCoordinatorError::RecursiveSubmission
        );

        let guard = acquire_selection_operation_turn()
            .await
            .expect("normal acquisition after a rejection");
        drop(guard);
    }

    /// #1940 - dropping the outer caller detaches the owned task: the turn
    /// stays held until the accepted operation finishes, and the next guarded
    /// operation observes the final state instead of racing it.
    #[tokio::test]
    async fn issue_1937_operation_turn_client_drop() {
        let final_state = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = oneshot::channel::<()>();
        let (release_tx, release_rx) = oneshot::channel::<()>();

        let state_for_owner = Arc::clone(&final_state);
        let owner = tokio::spawn(run_owned_selection_operation(move || async move {
            entered_tx.send(()).expect("signal owner enter");
            release_rx.await.expect("release owner operation");
            state_for_owner.store(7, Ordering::SeqCst);
            Ok::<(), String>(())
        }));
        entered_rx.await.expect("owner operation entered");

        owner.abort();
        assert!(owner.await.unwrap_err().is_cancelled());
        assert_eq!(final_state.load(Ordering::SeqCst), 0);

        let (observed_tx, observed_rx) = oneshot::channel::<usize>();
        let state_for_observer = Arc::clone(&final_state);
        let (done_tx, mut done_rx) = oneshot::channel::<()>();
        let observer = tokio::spawn(run_owned_selection_operation(move || async move {
            observed_tx
                .send(state_for_observer.load(Ordering::SeqCst))
                .expect("report observed state");
            done_tx.send(()).expect("signal observer done");
            Ok::<(), String>(())
        }));
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(
            !observer.is_finished(),
            "dropping the outer caller must not release the owned turn"
        );
        assert!(done_rx.try_recv().is_err());

        release_tx.send(()).expect("release the detached owner");
        let observed = observed_rx.await.expect("observer acquired the turn");
        assert_eq!(
            observed, 7,
            "the subsequent operation must observe the owner's final state"
        );
        observer.await.unwrap().unwrap();
    }

    /// #1940 - coordinator-side restart failures (closed, admission-busy, dead
    /// worker channel) and a failed owned task all surface as explicit errors;
    /// the persisted subset stays, nothing late is queued, and the turn is
    /// released only after the failure classification.
    #[tokio::test]
    async fn issue_1937_operation_turn_restart_failure() {
        fn restart_request(session_id: Uuid) -> crate::commands::session::RestartJobRequest {
            crate::commands::session::RestartJobRequest {
                session_id,
                agent_id: None,
                requested_profile: None,
                skip_auto_resume: None,
                activate_after: false,
                intent: TrustedRestartIntent::Background,
                communication_override: None,
                enforcement: crate::config::sessions_persistence::default_creation_gate_enforcement(
                ),
            }
        }

        async fn panicking_operation() -> Result<(), String> {
            panic!("simulated operation worker failure");
        }

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        // (a) Coordinator closing: the phase gate rejects before any admission.
        let closing = SelectionCoordinator::new(
            Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
            CancellationToken::new(),
        );
        closing
            .inner
            .phase
            .store(CoordinatorPhase::Closing as u8, Ordering::Release);
        let persisted = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let persisted_a = Arc::clone(&persisted);
        let closing_for_operation = closing.clone();
        let restart_app = app.handle().clone();
        let error = run_owned_selection_operation(move || {
            let persisted = Arc::clone(&persisted_a);
            async move {
                persisted.lock().unwrap().push("a:publish");
                let failure = closing_for_operation
                    .restart_lifecycle(restart_app.clone(), restart_request(Uuid::new_v4()))
                    .await
                    .map(|_| ())
                    .unwrap_err();
                assert!(
                    selection_operation_turn().try_write().is_err(),
                    "the failure classification must stay under the owned turn"
                );
                persisted.lock().unwrap().push("a:classified");
                Err::<(), String>(failure)
            }
        })
        .await
        .unwrap_err();
        assert_eq!(error, SelectionCoordinatorError::Unavailable.to_string());
        assert_eq!(
            &*persisted.lock().unwrap(),
            &["a:publish", "a:classified"],
            "a failed restart must retain the persisted subset"
        );
        assert_eq!(
            closing.inner.sender.capacity(),
            COORDINATOR_QUEUE_CAPACITY,
            "a rejected restart must not leave a late queued job"
        );

        // (b) Admission busy: every permit is taken, so no envelope is sent.
        let busy = SelectionCoordinator::new(
            Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
            CancellationToken::new(),
        );
        busy.inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        let mut held_permits = Vec::new();
        while let Ok(permit) = busy.inner.admission.clone().try_acquire_owned() {
            held_permits.push(permit);
        }
        assert_eq!(held_permits.len(), COORDINATOR_ADMISSION_CAPACITY);
        let busy_for_operation = busy.clone();
        let restart_app = app.handle().clone();
        let error = run_owned_selection_operation(move || async move {
            busy_for_operation
                .restart_lifecycle(restart_app.clone(), restart_request(Uuid::new_v4()))
                .await
                .map(|_| ())
        })
        .await
        .unwrap_err();
        assert_eq!(error, SelectionCoordinatorError::Busy.to_string());
        assert_eq!(
            busy.inner.sender.capacity(),
            COORDINATOR_QUEUE_CAPACITY,
            "a busy admission must not consume a queue slot"
        );
        drop(held_permits);

        // (c) Worker gone: phase still says Running, but the worker channel is
        // closed, so the accepted-job channel reports Unavailable.
        let orphaned = SelectionCoordinator::new(
            Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
            CancellationToken::new(),
        );
        orphaned
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        drop(orphaned.inner.receiver.lock().unwrap().take());
        let orphaned_for_operation = orphaned.clone();
        let restart_app = app.handle().clone();
        let error = run_owned_selection_operation(move || async move {
            orphaned_for_operation
                .restart_lifecycle(restart_app.clone(), restart_request(Uuid::new_v4()))
                .await
                .map(|_| ())
        })
        .await
        .unwrap_err();
        assert_eq!(error, SelectionCoordinatorError::Unavailable.to_string());
        assert_eq!(
            orphaned.inner.sender.capacity(),
            COORDINATOR_QUEUE_CAPACITY,
            "a dead worker channel must not accept a late job"
        );

        // A real accepted entry releases its provisional capacity while the
        // existing restore barrier is active. Saturation then belongs to the
        // actual second probe, rather than an invented admission receipt.
        let second_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let second_probe =
            SelectionCoordinator::new(second_manager.clone(), CancellationToken::new());
        let second_app = tauri::test::mock_builder()
            .manage(second_manager)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        second_probe.start(second_app.handle().clone()).unwrap();
        let restore = second_probe.submit_restore_first().await.unwrap();
        let accepted = {
            let coordinator = second_probe.clone();
            let app = second_app.handle().clone();
            tokio::spawn(async move {
                coordinator
                    .restart_lifecycle(app, restart_request(Uuid::new_v4()))
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let waiting = second_probe
                    .inner
                    .retained_replacements
                    .lock()
                    .unwrap()
                    .entries
                    .len()
                    == 1;
                if waiting
                    && second_probe.inner.admission.available_permits()
                        == COORDINATOR_ADMISSION_CAPACITY - 1
                {
                    // The restore envelope itself holds the remaining permit.
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("entry accepted and provisional admission released");
        let mut saturation = Vec::new();
        while let Ok(permit) = second_probe.inner.admission.clone().try_acquire_owned() {
            saturation.push(permit);
        }
        // The restore envelope will release one admission permit. Occupy the
        // real queue slots too, so the second probe still observes Busy without
        // racing that release or submitting any synthetic lifecycle job.
        let mut occupied_slots = Vec::new();
        while let Ok(slot) = second_probe.inner.sender.clone().try_reserve_owned() {
            occupied_slots.push(slot);
        }
        assert_eq!(occupied_slots.len(), COORDINATOR_QUEUE_CAPACITY);
        restore.finish();
        let outcome = tokio::time::timeout(Duration::from_secs(2), accepted)
            .await
            .expect("second probe returns without queueing")
            .unwrap();
        assert_eq!(
            outcome.unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert!(second_probe
            .inner
            .retained_replacements
            .lock()
            .unwrap()
            .entries
            .is_empty());
        drop(occupied_slots);
        drop(saturation);
        second_probe.close_and_join().await;

        // (d) Owned-task failure: a panicking operation maps to the explicit
        // join error, and the turn is not poisoned.
        let error = run_owned_selection_operation(panicking_operation)
            .await
            .unwrap_err();
        assert_eq!(error, "selectionOperationTurnJoinFailed");
        let guard = acquire_selection_operation_turn()
            .await
            .expect("a failed owned task must still release the turn");
        drop(guard);
    }

    #[tokio::test]
    async fn blocking_seed_tracker_completes_and_removes_started_work() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);

        assert_eq!(coordinator.run_blocking_seed_work(|| 42_u8).await, Ok(42));
        assert!(coordinator
            .inner
            .blocking_seed_work
            .lock_state()
            .work
            .is_empty());
    }

    #[tokio::test]
    async fn blocking_seed_tracker_is_panic_safe() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);

        let error = coordinator
            .run_blocking_seed_work(|| -> () { panic!("injected tracker panic") })
            .await
            .expect_err("panic must surface through the response join");
        assert!(matches!(error, BlockingSeedTaskError::Join(_)), "{error:?}");
        assert!(coordinator
            .inner
            .blocking_seed_work
            .lock_state()
            .work
            .is_empty());
    }

    #[test]
    fn blocking_seed_tracker_seal_suppresses_queued_user_work() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let (blocker_started_tx, blocker_started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                blocker_started_tx.send(()).expect("signal blocker");
                release_rx.recv().expect("release blocker");
            });
            blocker_started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("blocking pool occupied");

            let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
            let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
            coordinator
                .inner
                .phase
                .store(CoordinatorPhase::Running as u8, Ordering::Release);
            let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let waiter = {
                let coordinator = coordinator.clone();
                let ran = Arc::clone(&ran);
                tokio::spawn(async move {
                    coordinator
                        .run_blocking_seed_work(move || ran.store(true, Ordering::Release))
                        .await
                })
            };
            loop {
                if !coordinator
                    .inner
                    .blocking_seed_work
                    .lock_state()
                    .work
                    .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            coordinator
                .inner
                .phase
                .store(CoordinatorPhase::Closing as u8, Ordering::Release);
            for abort in coordinator.inner.blocking_seed_work.seal() {
                abort.abort();
            }
            release_tx.send(()).expect("release blocker");
            blocker.await.expect("join blocker");
            assert!(matches!(
                waiter.await.expect("join waiter"),
                Err(BlockingSeedTaskError::CanceledBeforeStart)
            ));
            assert!(!ran.load(Ordering::Acquire));
        });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_seed_tracker_keeps_detached_started_work_accounted() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let waiter = {
            let coordinator = coordinator.clone();
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                coordinator
                    .run_blocking_seed_work(move || {
                        started.store(true, Ordering::Release);
                        release_rx.recv().expect("release started work");
                    })
                    .await
            })
        };
        while !started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        waiter.abort();
        let _ = waiter.await;
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Closing as u8, Ordering::Release);
        assert!(coordinator.inner.blocking_seed_work.seal().is_empty());
        assert!(
            !coordinator
                .inner
                .blocking_seed_work
                .wait_quiescent_until(Instant::now())
                .await
        );

        release_tx.send(()).expect("release worker");
        assert!(
            coordinator
                .inner
                .blocking_seed_work
                .wait_quiescent_until(Instant::now() + Duration::from_secs(1))
                .await
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_reports_the_exact_retained_blocking_seed_reason() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let waiter = {
            let coordinator = coordinator.clone();
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                coordinator
                    .run_blocking_seed_work(move || {
                        started.store(true, Ordering::Release);
                        release_rx.recv().expect("release retained work");
                    })
                    .await
            })
        };
        while !started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }

        let report = coordinator
            .close_and_join_with_budget(Duration::from_millis(20))
            .await;
        waiter.abort();
        let _ = waiter.await;
        release_tx.send(()).expect("release worker");
        assert!(
            coordinator
                .inner
                .blocking_seed_work
                .wait_quiescent_until(Instant::now() + Duration::from_secs(1))
                .await
        );
        assert!(!report.persistence_safe);
        assert_eq!(
            report
                .retained
                .iter()
                .filter(|reason| {
                    reason.as_str() == "reason=blocking-seed-transaction-await state=retained"
                })
                .count(),
            1
        );
    }

    // Stage E (#1064) tracker mutation proofs (plan section 10.4 item 16): the
    // poison-recovery and id-overflow paths were previously unexercised.
    #[test]
    fn blocking_seed_tracker_recovers_from_mutex_poison_and_preserves_work() {
        use std::sync::atomic::AtomicU8;
        let tracker = Arc::new(BlockingSeedWorkTracker::new());
        let phase = AtomicU8::new(CoordinatorPhase::Running as u8);
        let id = tracker.register(&phase).expect("register");

        // Poison the state mutex by panicking while holding it.
        let poisoner = Arc::clone(&tracker);
        let joined = std::thread::spawn(move || {
            let _guard = poisoner.state.lock().expect("lock to poison");
            panic!("intentional poison");
        })
        .join();
        assert!(joined.is_err(), "the poisoning thread must have panicked");

        // lock_state recovers via into_inner and preserves the tracked work map.
        assert!(
            tracker.lock_state().work.contains_key(&id),
            "poison recovery must preserve the tracked work map"
        );
        assert!(
            tracker.begin(id),
            "begin must still work after poison recovery"
        );
        tracker.complete_started(id);
        assert!(tracker.lock_state().work.is_empty());
    }

    #[test]
    fn blocking_seed_tracker_id_overflow_seals_admission_without_reuse() {
        use std::sync::atomic::AtomicU8;
        let tracker = BlockingSeedWorkTracker::new();
        let phase = AtomicU8::new(CoordinatorPhase::Running as u8);
        let first = tracker.register(&phase).expect("first register");

        // Force the id space to its last value; the next register overflows.
        tracker.lock_state().next_id = u64::MAX;
        assert_eq!(
            tracker.register(&phase),
            Err(BlockingSeedTaskError::Unavailable),
            "an exhausted id space must fail closed"
        );

        {
            let state = tracker.lock_state();
            assert!(!state.accepting, "overflow seals admission");
            assert!(state.work.contains_key(&first), "prior ids remain tracked");
            assert!(
                !state.work.contains_key(&u64::MAX),
                "the overflowing id is never inserted or reused"
            );
        }
        assert_eq!(
            tracker.register(&phase),
            Err(BlockingSeedTaskError::Unavailable),
            "after sealing, no new id is admitted"
        );
    }

    #[test]
    fn blocking_seed_tracker_late_attach_after_completion_is_harmless() {
        use std::sync::atomic::AtomicU8;
        let tracker = BlockingSeedWorkTracker::new();
        let phase = AtomicU8::new(CoordinatorPhase::Running as u8);
        let id = tracker.register(&phase).expect("register");
        assert!(tracker.begin(id));
        tracker.complete_started(id);
        assert!(tracker.lock_state().work.is_empty());

        // Attaching an abort handle for an id that already completed aborts the
        // handle immediately outside the mutex and never re-inserts or panics.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let abort = runtime
            .spawn(async {
                std::future::pending::<()>().await;
            })
            .abort_handle();
        tracker.attach_abort(id, abort);
        assert!(
            tracker.lock_state().work.is_empty(),
            "late attach must not re-insert"
        );
    }

    #[test]
    fn exact_coordinator_error_strings_are_stable() {
        assert_eq!(
            SelectionCoordinatorError::Busy.to_string(),
            "selectionCoordinatorBusy"
        );
        assert_eq!(
            SelectionCoordinatorError::Unavailable.to_string(),
            "selectionCoordinatorUnavailable"
        );
        assert_eq!(
            SelectionCoordinatorError::RecursiveSubmission.to_string(),
            "selectionCoordinatorRecursiveSubmission"
        );
    }

    #[test]
    fn selection_timing_line_separates_queue_wait_and_work() {
        assert_eq!(
            super::selection_timing_line(
                "preview_selection_lock_removal",
                Duration::from_micros(1_234_500),
                Duration::from_micros(5_600),
                true,
            ),
            "[selection-timing] command=preview_selection_lock_removal queue_wait_ms=1234.5 work_ms=5.6 ok=true"
        );
    }

    use super::timing_probe::{timing_field_ms, TimingSinkRegistration};

    /// #2475 - queue_wait and work are bounded by synchronized events, not by
    /// sleep lengths: the submission provably happens before the release, and
    /// the operation's entry and finish are explicit handshakes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_selection_operation_records_queue_wait_and_work() {
        use std::time::Instant;
        const PROBE: &str = "issue_2475_timing_probe";
        let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
        let line_tx = Mutex::new(line_tx);
        let _sink = TimingSinkRegistration::new(
            PROBE,
            Arc::new(move |line: &str| {
                let _ = line_tx.lock().expect("line tx").send(line.to_string());
            }),
        );
        let (entered_tx, mut entered_rx) = tokio::sync::oneshot::channel::<Instant>();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();

        let held = acquire_selection_operation_turn()
            .await
            .expect("hold the turn");
        let before_submit = Instant::now();
        let mut queued = Box::pin(run_owned_selection_operation_timed(
            PROBE,
            move || async move {
                let _ = entered_tx.send(Instant::now());
                let _ = finish_rx.await;
                Err::<(), String>("probe failure".to_string())
            },
        ));
        // One poll runs the submission: `submitted` is stamped and the owned
        // task is spawned before this returns.
        assert!(futures::poll!(queued.as_mut()).is_pending());
        let after_submit = Instant::now();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            entered_rx.try_recv().is_err(),
            "the operation must not enter while the turn is held"
        );

        let released = Instant::now();
        drop(held);
        let queued = tokio::spawn(queued);
        let entered = (&mut entered_rx).await.expect("operation entered");
        tokio::time::sleep(Duration::from_millis(5)).await;
        let finish_sent = Instant::now();
        finish_tx.send(()).expect("operation still waiting");
        let result = queued.await.expect("join");
        assert_eq!(result, Err("probe failure".to_string()), "result unchanged");

        let line = line_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("timing line recorded");
        assert!(line.starts_with(&format!("[selection-timing] command={PROBE} ")));
        assert!(line.ends_with(" ok=false"), "failure is recorded: {line}");
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        // The line rounds to 0.1 ms.
        let tolerance = 0.05;
        let queue_wait = timing_field_ms(&line, "queue_wait_ms");
        let work = timing_field_ms(&line, "work_ms");
        assert!(
            queue_wait + tolerance >= ms(released - after_submit),
            "queue wait covers the held turn: {line}"
        );
        assert!(
            queue_wait - tolerance <= ms(entered - before_submit),
            "queue wait ends at admission: {line}"
        );
        assert!(
            work + tolerance >= ms(finish_sent - entered),
            "work covers the operation: {line}"
        );
    }

    /// #2475 - recording happens after the turn is released: a recording sink
    /// that blocks must not keep the next operation out of the turn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn timing_record_runs_after_the_turn_is_released() {
        const PROBE: &str = "issue_2475_blocked_sink_probe";
        let (in_sink_tx, in_sink_rx) = std::sync::mpsc::channel::<()>();
        let (unblock_tx, unblock_rx) = std::sync::mpsc::channel::<()>();
        let sink_channels = Mutex::new((in_sink_tx, unblock_rx));
        let _sink = TimingSinkRegistration::new(
            PROBE,
            Arc::new(move |_line: &str| {
                let channels = sink_channels.lock().expect("sink channels");
                let _ = channels.0.send(());
                let _ = channels.1.recv_timeout(Duration::from_secs(10));
            }),
        );

        let first = tokio::spawn(run_owned_selection_operation_timed(PROBE, || async {
            Ok::<(), String>(())
        }));
        tokio::task::spawn_blocking(move || in_sink_rx.recv_timeout(Duration::from_secs(10)))
            .await
            .expect("join")
            .expect("recording started");

        // The sink is now blocked inside the recording of `first`.
        let next =
            tokio::time::timeout(Duration::from_secs(5), acquire_selection_operation_turn()).await;
        let _ = unblock_tx.send(());
        let next = next.expect("the turn must be free while the timing line is recorded");
        drop(next.expect("turn"));
        assert_eq!(first.await.expect("join"), Ok(()));
    }

    /// #2475 - every matching registration receives the sample, and dropping
    /// one registration removes only that one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_two_concurrent_sinks_for_one_command_both_receive_the_sample() {
        use super::timing_probe::SelectionTimingSample;
        const PROBE: &str = "issue_2475_two_sinks_probe";
        let first_seen = Arc::new(Mutex::new(Vec::<Option<u64>>::new()));
        let second_seen = Arc::new(Mutex::new(Vec::<Option<u64>>::new()));
        let recorder = |seen: &Arc<Mutex<Vec<Option<u64>>>>| {
            let seen = Arc::clone(seen);
            Arc::new(move |sample: &SelectionTimingSample, _line: &str| {
                seen.lock().expect("seen").push(sample.invocation);
            })
        };
        let first = TimingSinkRegistration::new_sample(PROBE, recorder(&first_seen));
        let _second = TimingSinkRegistration::new_sample(PROBE, recorder(&second_seen));

        run_owned_selection_operation_timed(PROBE, || async { Ok::<(), String>(()) })
            .await
            .expect("first run");
        assert_eq!(first_seen.lock().expect("seen").len(), 1, "first sink");
        assert_eq!(second_seen.lock().expect("seen").len(), 1, "second sink");

        drop(first);
        run_owned_selection_operation_timed(PROBE, || async { Ok::<(), String>(()) })
            .await
            .expect("second run");
        assert_eq!(
            first_seen.lock().expect("seen").len(),
            1,
            "a dropped registration receives nothing"
        );
        assert_eq!(
            second_seen.lock().expect("seen").len(),
            2,
            "dropping one registration keeps the other"
        );
    }

    /// #2475 - the mechanism itself: two guards held together count 2, two
    /// taken in sequence count 1 (which a set of thread ids cannot tell apart).
    #[test]
    fn issue_2475_occupancy_guard_counts_simultaneous_holders() {
        use super::timing_probe::{Occupancy, OccupancyGuard};
        let together = Arc::new(Occupancy::default());
        let a = OccupancyGuard::enter(&together);
        let b = OccupancyGuard::enter(&together);
        assert_eq!(together.in_flight(), 2);
        drop(a);
        drop(b);
        assert_eq!(together.max(), 2);
        assert_eq!(together.in_flight(), 0);

        let sequence = Arc::new(Occupancy::default());
        drop(OccupancyGuard::enter(&sequence));
        drop(OccupancyGuard::enter(&sequence));
        assert_eq!(sequence.max(), 1);
        assert_eq!(sequence.in_flight(), 0);
    }

    /// #2475 - a timed body launched WITHOUT the run context never touches the
    /// run's occupancy; one launched with it does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_occupancy_ignores_non_participating_bodies() {
        use super::timing_probe::{BenchContext, Occupancy};
        const PROBE: &str = "issue_2475_occupancy_isolation_probe";
        let occupancy = Arc::new(Occupancy::default());

        run_owned_selection_operation_timed(PROBE, || async { Ok::<(), String>(()) })
            .await
            .expect("non-participating run");
        assert_eq!(
            occupancy.max(),
            0,
            "a body without the context is not counted"
        );

        super::BENCH_CONTEXT
            .scope(
                BenchContext::new(&occupancy),
                run_owned_selection_operation_timed(PROBE, || async { Ok::<(), String>(()) }),
            )
            .await
            .expect("participating run");
        assert_eq!(occupancy.max(), 1, "a body with the context is counted");
        assert_eq!(occupancy.in_flight(), 0);
    }

    /// #2475 - the guard's Drop also covers unwind: a panicking body leaves
    /// nothing in flight.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_occupancy_guard_releases_on_panic() {
        use super::timing_probe::{BenchContext, Occupancy};
        const PROBE: &str = "issue_2475_occupancy_panic_probe";
        let occupancy = Arc::new(Occupancy::default());
        let result = super::BENCH_CONTEXT
            .scope(
                BenchContext::new(&occupancy),
                run_owned_selection_operation_timed(PROBE, || async {
                    if true {
                        panic!("issue_2475 probe panic");
                    }
                    Ok::<(), String>(())
                }),
            )
            .await;
        assert_eq!(result, Err("selectionOperationTurnJoinFailed".to_string()));
        assert_eq!(occupancy.max(), 1, "the body entered");
        assert_eq!(occupancy.in_flight(), 0, "unwind released the guard");
    }

    // ── #2475 p3: read side of the selection operation turn ─────────────
    //
    // Every body announces its ENTRY as its first statement. Assertions are on
    // the presence, absence and order of those announcements, never on task
    // completion. Absence is a bounded timeout on the channel, and each test
    // then shows the same channel does receive once the blocker is released,
    // so the timeout is not vacuous.

    const P3_ABSENCE: Duration = Duration::from_millis(250);
    const P3_PRESENCE: Duration = Duration::from_secs(10);

    /// Runs a p3 test on its own turn (same `RwLock` type and code path as the
    /// process-wide one), isolated from writers of unrelated tests.
    async fn p3_private_turn(test: impl std::future::Future<Output = ()>) {
        TEST_SELECTION_TURN
            .scope(Arc::new(tokio::sync::RwLock::new(())), test)
            .await;
    }

    /// `tokio::spawn` that keeps this task's private turn in the new task.
    fn p3_spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let turn = TEST_SELECTION_TURN.with(Arc::clone);
        tokio::spawn(TEST_SELECTION_TURN.scope(turn, future))
    }

    /// A read operation that announces entry, then waits for `release`.
    fn p3_held_read(
        command: &'static str,
    ) -> (
        tokio::task::JoinHandle<Result<(), String>>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let task = p3_spawn(run_owned_selection_read_timed(
            command,
            move || async move {
                let _ = entered_tx.send(());
                let _ = release_rx.await;
                Ok::<(), String>(())
            },
        ));
        (task, entered_rx, release_tx)
    }

    /// p3 test 1: two reads are inside their bodies at the same time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_two_reads_enter_the_body_concurrently() {
        p3_private_turn(issue_2475_two_reads_enter_the_body_concurrently_body()).await;
    }

    async fn issue_2475_two_reads_enter_the_body_concurrently_body() {
        const PROBE: &str = "issue_2475_p3_concurrent_reads_probe";
        let (first, first_entered, first_release) = p3_held_read(PROBE);
        tokio::time::timeout(P3_PRESENCE, first_entered)
            .await
            .expect("the first read enters")
            .expect("entry sent");
        let (second, second_entered, second_release) = p3_held_read(PROBE);
        // The first read is still inside: the second must enter anyway.
        let second_entered = tokio::time::timeout(P3_PRESENCE, second_entered).await;
        let _ = first_release.send(());
        let _ = second_release.send(());
        assert_eq!(first.await.expect("join"), Ok(()));
        assert_eq!(second.await.expect("join"), Ok(()));
        second_entered
            .expect("the second read entered while the first was still inside")
            .expect("entry sent");
    }

    /// p3 test 2: a read body is not entered while a persisted operation
    /// holds the write side, and is entered right after its release.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_read_body_is_not_entered_while_a_write_holds_the_turn() {
        p3_private_turn(issue_2475_read_body_is_not_entered_while_a_write_holds_the_turn_body())
            .await;
    }

    async fn issue_2475_read_body_is_not_entered_while_a_write_holds_the_turn_body() {
        const PROBE: &str = "issue_2475_p3_read_behind_write_probe";
        let writer = acquire_selection_operation_turn()
            .await
            .expect("hold the write side");
        let (read, mut entered, release) = p3_held_read(PROBE);
        let while_held = tokio::time::timeout(P3_ABSENCE, &mut entered).await;
        drop(writer);
        // A oneshot must not be polled again once it has completed.
        let after_release = match while_held {
            Ok(_) => None,
            Err(_) => Some(tokio::time::timeout(P3_PRESENCE, &mut entered).await),
        };
        let _ = release.send(());
        assert_eq!(read.await.expect("join"), Ok(()));
        assert!(
            while_held.is_err(),
            "the read entered while the write side was held"
        );
        after_release
            .expect("checked above")
            .expect("the read enters once the write side is released")
            .expect("entry sent");
    }

    /// p3 test 3: a persisted operation does not enter while a read is inside.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_write_body_is_not_entered_while_a_read_holds_the_turn() {
        p3_private_turn(issue_2475_write_body_is_not_entered_while_a_read_holds_the_turn_body())
            .await;
    }

    async fn issue_2475_write_body_is_not_entered_while_a_read_holds_the_turn_body() {
        const PROBE: &str = "issue_2475_p3_write_behind_read_probe";
        let (read, read_entered, read_release) = p3_held_read(PROBE);
        tokio::time::timeout(P3_PRESENCE, read_entered)
            .await
            .expect("the read enters")
            .expect("entry sent");
        let (write_entered_tx, mut write_entered) = tokio::sync::oneshot::channel::<()>();
        let write = p3_spawn(run_owned_selection_operation(move || async move {
            let _ = write_entered_tx.send(());
            Ok::<(), String>(())
        }));
        let while_read = tokio::time::timeout(P3_ABSENCE, &mut write_entered).await;
        let _ = read_release.send(());
        // A oneshot must not be polled again once it has completed.
        let after_release = match while_read {
            Ok(_) => None,
            Err(_) => Some(tokio::time::timeout(P3_PRESENCE, &mut write_entered).await),
        };
        assert_eq!(read.await.expect("join"), Ok(()));
        assert_eq!(write.await.expect("join"), Ok(()));
        assert!(
            while_read.is_err(),
            "the write entered while a read was inside"
        );
        after_release
            .expect("checked above")
            .expect("the write enters once the read is released")
            .expect("entry sent");
    }

    /// Counts wakes; a registered waiter is observed by polling, not by time.
    struct P3CountingWaker(AtomicUsize);

    impl std::task::Wake for P3CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// p3 test 4: a writer that is already QUEUED (observed by a manual poll
    /// that returned Pending) enters before a reader submitted after it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_waiting_writer_blocks_a_later_reader() {
        p3_private_turn(issue_2475_waiting_writer_blocks_a_later_reader_body()).await;
    }

    async fn issue_2475_waiting_writer_blocks_a_later_reader_body() {
        const PROBE: &str = "issue_2475_p3_writer_priority_probe";
        let order = Arc::new(AtomicUsize::new(0));
        // 1. A first reader is inside.
        let (first, first_entered, first_release) = p3_held_read(PROBE);
        tokio::time::timeout(P3_PRESENCE, first_entered)
            .await
            .expect("the first read enters")
            .expect("entry sent");
        // 2. Register the writer in the lock's queue with an observed poll.
        let mut write_acquire = Box::pin(acquire_selection_operation_turn());
        let wakes = Arc::new(P3CountingWaker(AtomicUsize::new(0)));
        let waker = std::task::Waker::from(Arc::clone(&wakes));
        let mut cx = std::task::Context::from_waker(&waker);
        let first_poll = std::future::Future::poll(write_acquire.as_mut(), &mut cx);
        let polled_pending = first_poll.is_pending();
        // A future must not be polled again once it returned Ready.
        let ready_guard = match first_poll {
            std::task::Poll::Ready(guard) => Some(guard.expect("writer turn")),
            std::task::Poll::Pending => None,
        };
        // 3. Only now submit the second reader.
        let (second_entered_tx, mut second_entered) = tokio::sync::oneshot::channel::<usize>();
        let second = p3_spawn({
            let order = Arc::clone(&order);
            run_owned_selection_read_timed(PROBE, move || async move {
                let _ = second_entered_tx.send(order.fetch_add(1, Ordering::SeqCst));
                Ok::<(), String>(())
            })
        });
        // 4. The second reader stays out while the writer waits (a failure bound).
        let while_writer_waits = tokio::time::timeout(P3_ABSENCE, &mut second_entered).await;
        // 5. Release the first reader, finish the pinned writer in its own task.
        let _ = first_release.send(());
        let writer = tokio::spawn({
            let order = Arc::clone(&order);
            async move {
                let guard = match ready_guard {
                    Some(guard) => guard,
                    None => write_acquire.await.expect("writer turn"),
                };
                let at = order.fetch_add(1, Ordering::SeqCst);
                drop(guard);
                at
            }
        });
        let writer_at = tokio::time::timeout(P3_PRESENCE, writer)
            .await
            .expect("the writer enters")
            .expect("join");
        let second_at = match &while_writer_waits {
            Ok(Ok(at)) => *at,
            Ok(Err(_)) => panic!("second entry channel closed"),
            Err(_) => tokio::time::timeout(P3_PRESENCE, &mut second_entered)
                .await
                .expect("the second read enters after the writer")
                .expect("entry sent"),
        };
        assert_eq!(first.await.expect("join"), Ok(()));
        assert_eq!(second.await.expect("join"), Ok(()));
        assert!(
            polled_pending,
            "the writer must be queued behind the first reader"
        );
        assert!(
            while_writer_waits.is_err(),
            "a later reader entered past a queued writer"
        );
        assert!(
            writer_at < second_at,
            "writer entry ({writer_at}) must precede the later reader ({second_at})"
        );
    }

    /// p3 test 5: from inside the selection worker, the read side is rejected
    /// BEFORE waiting (a held write side would otherwise block forever).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_read_from_the_selection_worker_is_rejected_before_waiting() {
        p3_private_turn(
            issue_2475_read_from_the_selection_worker_is_rejected_before_waiting_body(),
        )
        .await;
    }

    async fn issue_2475_read_from_the_selection_worker_is_rejected_before_waiting_body() {
        let writer = acquire_selection_operation_turn()
            .await
            .expect("hold the write side");
        let (direct, wrapped) = IN_SELECTION_WORKER
            .scope((), async {
                let direct = tokio::time::timeout(P3_PRESENCE, acquire_selection_read_turn()).await;
                let wrapped = tokio::time::timeout(
                    P3_PRESENCE,
                    run_owned_selection_read_timed("issue_2475_p3_recursive_probe", || async {
                        Ok::<(), String>(())
                    }),
                )
                .await;
                (direct, wrapped)
            })
            .await;
        drop(writer);
        assert!(
            matches!(
                direct.expect("rejected without waiting"),
                Err(SelectionCoordinatorError::RecursiveSubmission)
            ),
            "the read side rejects a recursive submission"
        );
        assert_eq!(
            wrapped.expect("rejected without waiting"),
            Err(SelectionCoordinatorError::RecursiveSubmission.to_string())
        );
    }

    /// p3 test 6: dropping the caller detaches the owned read task; its body
    /// still reaches its end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_read_turn_survives_a_dropped_caller() {
        p3_private_turn(issue_2475_read_turn_survives_a_dropped_caller_body()).await;
    }

    async fn issue_2475_read_turn_survives_a_dropped_caller_body() {
        const PROBE: &str = "issue_2475_p3_dropped_caller_probe";
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel::<()>();
        let caller = p3_spawn(run_owned_selection_read_timed(PROBE, move || async move {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
            let _ = finished_tx.send(());
            Ok::<(), String>(())
        }));
        tokio::time::timeout(P3_PRESENCE, entered_rx)
            .await
            .expect("the read enters")
            .expect("entry sent");
        caller.abort();
        assert!(caller.await.expect_err("caller aborted").is_cancelled());
        let _ = release_tx.send(());
        tokio::time::timeout(P3_PRESENCE, finished_rx)
            .await
            .expect("the owned read reaches its end after the caller is gone")
            .expect("finish sent");
    }

    /// p3 test 7: the read mode still records one timing line with both
    /// segments and the real command name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn issue_2475_read_turn_records_queue_wait_and_work() {
        p3_private_turn(issue_2475_read_turn_records_queue_wait_and_work_body()).await;
    }

    async fn issue_2475_read_turn_records_queue_wait_and_work_body() {
        const PROBE: &str = "issue_2475_p3_read_timing_probe";
        let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
        let line_tx = Mutex::new(line_tx);
        let _sink = TimingSinkRegistration::new(
            PROBE,
            Arc::new(move |line: &str| {
                let _ = line_tx.lock().expect("line tx").send(line.to_string());
            }),
        );
        let writer = acquire_selection_operation_turn()
            .await
            .expect("hold the write side");
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let mut queued = Box::pin(run_owned_selection_read_timed(PROBE, move || async move {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
            Ok::<(), String>(())
        }));
        // One poll runs the submission: `submitted` is stamped before this returns.
        assert!(futures::poll!(queued.as_mut()).is_pending());
        let after_submit = Instant::now();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let released = Instant::now();
        drop(writer);
        let queued = tokio::spawn(queued);
        tokio::time::timeout(P3_PRESENCE, entered_rx)
            .await
            .expect("the read enters")
            .expect("entry sent");
        let entered_seen = Instant::now();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let finish_sent = Instant::now();
        let _ = release_tx.send(());
        assert_eq!(queued.await.expect("join"), Ok(()));
        let line = line_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("timing line recorded");
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        // The line rounds to 0.1 ms.
        let tolerance = 0.05;
        assert!(line.starts_with(&format!("[selection-timing] command={PROBE} ")));
        assert!(line.ends_with(" ok=true"), "{line}");
        assert!(
            timing_field_ms(&line, "queue_wait_ms") + tolerance >= ms(released - after_submit),
            "queue wait covers the held write side: {line}"
        );
        assert!(
            timing_field_ms(&line, "work_ms") + tolerance >= ms(finish_sent - entered_seen),
            "work covers the body: {line}"
        );
    }
}
