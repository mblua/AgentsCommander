use std::sync::{Arc, OnceLock};

use super::payload::SelectionCoordinatorError;

#[cfg(test)]
use super::timing_probe;

tokio::task_local! {
    pub(super) static IN_SELECTION_WORKER: ();
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

pub(super) fn selection_operation_turn() -> &'static Arc<tokio::sync::RwLock<()>> {
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
pub(super) fn current_selection_turn() -> Arc<tokio::sync::RwLock<()>> {
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
    pub(super) _turn: tokio::sync::OwnedRwLockWriteGuard<()>,
}

/// #2475 - owned guard over the READ side of the selection operation turn:
/// excludes every persisted operation, admits other readers.
pub(crate) struct SelectionReadGuard {
    pub(super) _turn: tokio::sync::OwnedRwLockReadGuard<()>,
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
pub(super) enum SelectionTurnMode {
    Read,
    Write,
}

/// Either side of the turn, released on drop.
pub(super) enum SelectionTurnGuard {
    Read(#[allow(dead_code)] SelectionReadGuard),
    Write(#[allow(dead_code)] SelectionOperationGuard),
}

pub(super) async fn acquire_selection_turn(
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
pub(super) fn with_test_selection_turn<T: Send + 'static>(
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

pub(super) fn selection_timing_line(
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

pub(super) fn record_selection_timing(
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

pub(super) async fn run_owned_selection_operation_inner<F, Fut, T>(
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
