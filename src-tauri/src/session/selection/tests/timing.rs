use super::*;

#[test]
fn selection_timing_line_separates_queue_wait_and_work() {
    assert_eq!(
        crate::session::selection::selection_timing_line(
            "preview_selection_lock_removal",
            Duration::from_micros(1_234_500),
            Duration::from_micros(5_600),
            true,
        ),
        "[selection-timing] command=preview_selection_lock_removal queue_wait_ms=1234.5 work_ms=5.6 ok=true"
    );
}

use crate::session::selection::timing_probe::{timing_field_ms, TimingSinkRegistration};

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
    use crate::session::selection::timing_probe::SelectionTimingSample;
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
    use crate::session::selection::timing_probe::{Occupancy, OccupancyGuard};
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
    use crate::session::selection::timing_probe::{BenchContext, Occupancy};
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

    crate::session::selection::BENCH_CONTEXT
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
    use crate::session::selection::timing_probe::{BenchContext, Occupancy};
    const PROBE: &str = "issue_2475_occupancy_panic_probe";
    let occupancy = Arc::new(Occupancy::default());
    let result = crate::session::selection::BENCH_CONTEXT
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
