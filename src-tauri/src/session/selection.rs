use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
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

pub(crate) async fn poll_response_close<R: Runtime>(app: &AppHandle<R>) {
    let Some(tracker) = app.try_state::<crate::pty::manager::ResponseCloseState>() else {
        log::debug!(
            target: "agentscommander_lib::response_close_diag",
            "response_close_diag id=unknown site={} stage=poll_health \
             outcome=tracker_missing health=unknown",
            line!()
        );
        return;
    };
    let diagnostic_status = tracker.diagnostic_poll_health();
    crate::commands::session::reconcile_response_closes(app).await;
    let Some(coordinator) = app.try_state::<SelectionCoordinator>() else {
        if diagnostic_status {
            log::debug!(
                target: "agentscommander_lib::response_close_diag",
                "response_close_diag id=unknown site={} stage=poll_status \
                 outcome=coordinator_missing registrations=unknown",
                line!()
            );
        }
        return;
    };
    let registrations = response_close_registrations()
        .lock()
        .ok()
        .map(|registrations| registrations.values().cloned().collect::<Vec<_>>());
    let Some(registrations) = registrations else {
        if diagnostic_status {
            log::debug!(
                target: "agentscommander_lib::response_close_diag",
                "response_close_diag id=unknown site={} stage=poll_status \
                 outcome=registration_lock_error registrations=unknown",
                line!()
            );
        }
        return;
    };
    if diagnostic_status {
        log::debug!(
            target: "agentscommander_lib::response_close_diag",
            "response_close_diag id=unknown site={} outcome=observed stage=poll_status \
             registration_count={}",
            line!(),
            registrations.len()
        );
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
pub(crate) mod timing_probe;

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

impl OwnedReplacementContinuation {
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
        let (admission, slot) = self.try_reserve_envelope().inspect_err(|_| {
            registration.diagnostic_poll_outcome(5, "reserve_envelope_error");
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
                let member =
                    response_registration_row(current, &registration.member).or_else(|| {
                        if log::log_enabled!(
                            target: "agentscommander_lib::response_close_diag",
                            log::Level::Debug
                        ) {
                            diagnostic_reason = (8, "current_member_rejected");
                        }
                        None
                    })?;
                let coordinator = response_registration_row(current, &registration.coordinator)
                    .or_else(|| {
                        if log::log_enabled!(
                            target: "agentscommander_lib::response_close_diag",
                            log::Level::Debug
                        ) {
                            diagnostic_reason = (9, "current_coordinator_rejected");
                        }
                        None
                    })?;
                if member.backend_kind != crate::pty::backend::SessionBackendKind::LocalProcess
                    || member.is_root_agent
                    || member.is_coordinator
                    || member.agent_kind.is_none()
                    || !coordinator.is_coordinator
                    || coordinator.is_root_agent
                {
                    if log::log_enabled!(
                        target: "agentscommander_lib::response_close_diag",
                        log::Level::Debug
                    ) {
                        diagnostic_reason = (10, "role_or_backend_rejected");
                    }
                    return None;
                }
                if !response_registration_route(&pty, &registration.member, member)
                    || !response_registration_route(&pty, &registration.coordinator, coordinator)
                {
                    if log::log_enabled!(
                        target: "agentscommander_lib::response_close_diag",
                        log::Level::Debug
                    ) {
                        diagnostic_reason = (11, "member_or_coordinator_route_rejected");
                    }
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
                        if log::log_enabled!(
                            target: "agentscommander_lib::response_close_diag",
                            log::Level::Debug
                        ) {
                            use crate::pty::manager::ResponseCloseError;
                            diagnostic_reason = match error {
                                ResponseCloseError::Unavailable => {
                                    (12, "select_ticket_unavailable")
                                }
                                ResponseCloseError::Unknown => (13, "select_ticket_unknown"),
                                ResponseCloseError::Conflict => (14, "select_ticket_conflict"),
                                ResponseCloseError::ClosingDeferred => {
                                    (15, "select_ticket_closing_deferred")
                                }
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
                "[selection] bound create ticket dropped without its reserved finalization \
                    capacity session={}",
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
            CoordinatorJob::ConditionalResponseClose { .. } => {
                hold.release();
            }
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
            hold.release();
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
                            "[selection] failed to focus detached target session={} \
                                source=userSwitch: {}",
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
    let retained_key = crate::config::sessions_persistence::prompt_key(
        &request.working_directory,
        persisted.agent_id.as_deref(),
        persisted.requested_profile.as_deref(),
        persisted.effective_profile.as_deref(),
    );
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
        last_prompt: if persisted.start_fresh_on_restore {
            None
        } else {
            persisted.last_prompt.clone()
        },
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
        effective_profile: persisted.effective_profile,
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
    crate::config::sessions_persistence::initialize_prompt_retention(&manager).await;
    let initial_prompt = session.last_prompt.clone();
    let fresh = session.start_fresh_on_restore;
    let verified_key = crate::config::sessions_persistence::prompt_key(
        &session.working_directory,
        session.agent_id.as_deref(),
        session.requested_profile.as_deref(),
        session.effective_profile.as_deref(),
    );
    let retained_key = retained_key.filter(|key| Some(key) == verified_key.as_ref());
    let carrier_permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    manager
        .insert_transaction_pending_record_with_carrier_permit(&carrier_permit, session, binding)
        .await?;
    if let Err(error) = manager
        .initialize_pending_prompt_with_carrier_permit(
            &carrier_permit,
            binding,
            retained_key,
            initial_prompt,
            true,
            fresh,
            false,
        )
        .await
    {
        manager
            .rollback_pending_create_with_carrier_permit(&carrier_permit, binding)
            .await?;
        return Err(error);
    }
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
                "[selection] container backend ownership lock unavailable during shutdown signal: \
                    {}",
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
                        "[selection] pending-create PTY cleanup failed session={} \
                            reason=pending-pty-kill state=retained error={}",
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
                        "[selection] pending-create PTY cleanup task failed session={} \
                            reason=pending-pty-kill state=retained error={}",
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
                    "[selection] pending create manager rollback failed session={} \
                        reason=pending-manager-rollback state=retained error={}",
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
                "[selection] pending-create cleanup owner failed reason=pending-cleanup-task \
                    state=retained error={}",
                error
            );
            vec!["reason=pending-cleanup-task state=retained".to_string()]
        }
        Err(_) => {
            log::error!(
                "[selection] pending-create cleanup owner retained after absolute deadline \
                    reason=pending-cleanup-await state=retained"
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
                "[selection] container shutdown-work blocking task retained after absolute \
                    deadline reason=container-drain-await state=retained"
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

mod admission;
mod response_close;
#[cfg(test)]
use admission::BlockingSeedWorkState;
pub(crate) use admission::{BlockingSeedTaskError, WatchdogKillOutcome};
pub use admission::{CriticalAdmissionKind, CriticalAdmissionOutcome};
mod payload;
mod replacement;
#[cfg(test)]
pub(crate) use replacement::FixtureRootReservation;
pub(crate) use replacement::{EntryReaderDemandReceipt, LifecycleEffectGuard, LifecycleEffectSlot};
#[cfg(test)]
mod replacement_fixture;
pub(crate) use replacement::{
    OwnedReplacementContinuation, OwnedReplacementWork, OwnedSourceStop, PreparedOwnedReplacement,
    ReplacementAdmissionStep, ResolvedReplacement, SourceExecutorGuard,
};
#[cfg(test)]
use replacement_fixture::ReplacementFixtureEnvelopeTrace;
#[cfg(test)]
pub(crate) use replacement_fixture::{
    await_fixture_native, FixtureContextNotification, NativeAwaitPanicOnce,
    ReplacementFixtureAction, ReplacementFixtureControl, ReplacementFixtureCut,
    ReplacementFixtureOutcome, ReplacementFixtureOwnerObservation, ReplacementFixtureOwnerWeak,
    ReplacementFixtureReceipt,
};
mod envelope;
use admission::{
    BlockingSeedAwaitGuard, BlockingSeedWorkTracker, BlockingSeedWorkerResult, CoordinatorPhase,
    CriticalAdmissionGuard, CriticalAdmissionKey, CriticalAdmissionReservation,
    WorkerCompletionGuard,
};
use envelope::{CoordinatorEnvelope, RestoreSubmissionPhaseGuard};
pub(super) use envelope::{FinalizedOwnedCarryCapability, FinalizedOwnedCarryOutcome};
pub(crate) use payload::{DormantRuntimeWitness, LiveRuntimeWitness, SelectionCause};
pub use payload::{
    RuntimeSelectionSnapshot, SelectionCoordinatorError, SelectionMode, SelectionSource,
    SessionSelection, TrustedCreateIntent, TrustedResourceIntent, TrustedRestartIntent,
};
use replacement::{
    EntryCreateBinding, EntryCreateStage, EntryReleaseReceipt, EntryTerminal,
    EntryTerminalDisposition, OwnedEffectState, OwnedLifecycleEntry, OwnedReplacementDescriptor,
    ReplacementOwner, ReplacementStage, ReplacementState, RetainedReplacements,
};
use replacement::{
    InitialFinalizationReceipt, ReplacementCleanupAuthority, ReplacementProgress,
    SourceExecutorObservation,
};
pub(crate) use response_close::ResponseCloseRegistration;
use response_close::{response_registration_route, response_registration_row};

mod continuation_cleanup;
mod continuation_launch;
#[cfg(test)]
mod tests;
