use super::*;
use crate::session::selection::timing_probe::{timing_field_ms, TimingSinkRegistration};

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
    p3_private_turn(issue_2475_read_body_is_not_entered_while_a_write_holds_the_turn_body()).await;
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
    p3_private_turn(issue_2475_write_body_is_not_entered_while_a_read_holds_the_turn_body()).await;
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
    p3_private_turn(issue_2475_read_from_the_selection_worker_is_rejected_before_waiting_body())
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
