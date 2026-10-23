use super::*;

// --- force ---------------------------------------------------------------

#[tokio::test]
async fn force_supersedes_the_original_waiter_with_one_exit() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    let forced = force_quit(&gate, &dyn_host, Some(1)).await;
    assert_eq!(forced, QuitOutcome::exiting(1));

    let original = quit.await.unwrap().unwrap();
    assert_eq!(original, QuitOutcome::exiting(1));
    assert_eq!(host.exits(), 1, "exit is called exactly once");
}

#[tokio::test]
async fn force_wins_while_a_gate_is_busy() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;
    gate.progress("spec-board", 1, true); // paused mid-save

    assert_eq!(force_quit(&gate, &dyn_host, Some(1)).await.epoch, 1);
    assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
    assert_eq!(host.exits(), 1);
}

#[tokio::test]
async fn force_after_abort_is_stale_and_never_exits() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
    quit.await.unwrap().unwrap();

    assert_eq!(
        force_quit(&gate, &dyn_host, Some(1)).await,
        QuitOutcome::stale(1)
    );
    assert_eq!(host.exits(), 0);
}

#[tokio::test]
async fn stale_force_cannot_clear_a_later_live_round() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let first = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
    first.await.unwrap().unwrap();

    gate.register("spec-board");
    let second = spawn_quit(&gate, &dyn_host, "attempt-2");
    settle().await;

    assert_eq!(
        force_quit(&gate, &dyn_host, Some(1)).await,
        QuitOutcome::stale(1)
    );
    assert_eq!(host.exits(), 0);
    settle().await;
    assert!(!second.is_finished(), "round 2 survives a stale Force");

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 2, true));
    assert_eq!(second.await.unwrap().unwrap(), QuitOutcome::exiting(2));
    assert_eq!(host.exits(), 1);
}

#[tokio::test]
async fn force_without_an_epoch_cannot_authorize_exit() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    assert_eq!(
        force_quit(&gate, &dyn_host, None).await,
        QuitOutcome::stale(0)
    );
    assert_eq!(host.exits(), 0);
    settle().await;
    assert!(!quit.is_finished());

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
    quit.await.unwrap().unwrap();
    assert_eq!(host.exits(), 1);
}

#[tokio::test]
async fn force_bumps_the_allocator_so_the_next_round_is_distinct() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;
    force_quit(&gate, &dyn_host, Some(1)).await;
    quit.await.unwrap().unwrap();

    apply_quit_gate_effects(host.as_ref(), &gate.unregister("spec-board"));
    let next = spawn_quit(&gate, &dyn_host, "attempt-2")
        .await
        .unwrap()
        .unwrap();
    assert!(
        next.epoch > 1,
        "epoch {} must be past the forced one",
        next.epoch
    );
    assert_eq!(host.exits(), 2, "the new round exits on its own account");
}

// --- cancellation / drop -------------------------------------------------

#[tokio::test]
async fn dropped_quit_task_releases_the_round_with_a_terminal_outcome() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    quit.abort();
    settle().await;

    // The dropped task cannot return, but the terminal result is explicit
    // and observable by main.
    assert_eq!(
        gate.terminal_snapshot_for_test(1).map(|o| o.reason),
        Some(Some(QuitAbortReason::Cancelled))
    );
    let emitted = host.outcome_to_main().unwrap();
    assert_eq!(emitted["outcome"], "Aborted");
    assert_eq!(emitted["reason"], "cancelled");
    assert_eq!(emitted["epoch"], 1);
    assert_eq!(host.exits(), 0);

    // In-flight was released: a fresh round runs with a new epoch.
    gate.register("spec-board");
    let second = spawn_quit(&gate, &dyn_host, "attempt-2");
    settle().await;
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 2, true));
    assert_eq!(second.await.unwrap().unwrap(), QuitOutcome::exiting(2));
}

// --- authority -----------------------------------------------------------

#[tokio::test]
async fn only_main_may_invoke_quit() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    for force in [false, true] {
        let err = quit_gate_run(
            Arc::clone(&gate),
            Arc::clone(&dyn_host),
            "spec-board",
            force,
            Some(1),
            Some("attempt-1".to_string()),
        )
        .await
        .unwrap_err();
        assert!(err.contains("restricted to the 'main' window"), "{err}");
    }
    assert_eq!(host.exits(), 0);
    assert!(host.events_named("app_quit_started").is_empty());
}

#[tokio::test]
async fn attempt_id_is_required_for_a_new_round() {
    let (gate, host, dyn_host, _clock) = new_gate();
    for attempt in [None, Some(String::new())] {
        let err = quit_gate_run(
            Arc::clone(&gate),
            Arc::clone(&dyn_host),
            "main",
            false,
            None,
            attempt,
        )
        .await
        .unwrap_err();
        assert!(err.contains("nonempty attemptId"), "{err}");
    }
    assert_eq!(host.exits(), 0);
}

// --- B1 regression: lost resume wakeup ------------------------------------

/// Documents the primitive the waiter loop depends on.
///
/// `Notified` snapshots the notify-waiters counter when it is CREATED, so
/// a `notify_waiters()` landing before that snapshot is lost forever while
/// one landing after it is observed even before the first poll. That is
/// exactly why `quit_gate_run` creates and enables its `Notified` ahead of
/// reading the round state instead of after it.
#[tokio::test]
async fn notified_snapshot_must_be_taken_before_the_state_read() {
    let notify = Arc::new(tokio::sync::Notify::new());

    // Snapshot taken too late: the notification is already history.
    notify.notify_waiters();
    let late = notify.notified();
    tokio::pin!(late);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut late)
            .await
            .is_err(),
        "a Notified created after notify_waiters must miss it"
    );

    // Snapshot taken first: observed, even though it had not been polled.
    let early = notify.notified();
    tokio::pin!(early);
    early.as_mut().enable();
    notify.notify_waiters();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut early)
            .await
            .is_ok(),
        "a Notified created before notify_waiters must observe it"
    );
}

/// B1 REGRESSION BARRIER.
///
/// The lost-wakeup window is a handful of instructions inside one
/// synchronous stretch of `quit_gate_run`: between the round-state read and
/// the `notified()` counter snapshot. It is not reachable from a black-box
/// test — I swept a resume across that window from a second thread for
/// thousands of rounds against the buggy ordering and never hit it — so the
/// ordering is pinned structurally instead, the way the single-exit site is.
///
/// If someone moves the enrolment back below the state read, this fails.
#[test]
fn waiter_enrols_before_reading_round_state() {
    let production = include_str!("../../quit_gate.rs");

    let loops: Vec<&str> = production.split("let outcome = loop {").skip(1).collect();
    assert_eq!(loops.len(), 1, "expected exactly one quit waiter loop");
    let head = loops[0]
        .split("tokio::select! {")
        .next()
        .expect("waiter loop selects");

    let enrol = head
        .find("gate.notify.notified()")
        .expect("waiter loop enrols on the gate notify");
    let terminal = head
        .find("terminal_for(new_epoch)")
        .expect("waiter loop reads the terminal slot");
    let deadline = head
        .find("deadline_for(new_epoch)")
        .expect("waiter loop reads the deadline");

    assert!(
        enrol < terminal && enrol < deadline,
        "the Notified snapshot must be taken BEFORE the round-state read, \
         otherwise a notify_waiters() in between is lost and the waiter \
         parks forever with a stale deadline"
    );
    assert!(
        head.contains(".enable()"),
        "the Notified must be enabled up front, not only on first poll"
    );
}

/// Companion smoke test: pause/resume driven from real threads must always
/// leave a round that can still time out. This does NOT reach the B1
/// window (see `waiter_enrols_before_reading_round_state`); it guards the
/// ordinary multi-threaded pause/resume path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_resume_from_another_thread_still_times_out() {
    for attempt in 0..200 {
        let (gate, _host, dyn_host, clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");

        // Wait for the atomic install rather than assuming a yield count.
        while gate.deadline_for(1).is_none() {
            tokio::task::yield_now().await;
        }

        gate.progress("spec-board", 1, true);
        tokio::time::sleep(Duration::from_micros(200)).await;

        let hammer = {
            let gate = Arc::clone(&gate);
            let delay = Duration::from_nanos((attempt % 250) * 100);
            tokio::task::spawn_blocking(move || {
                gate.progress("spec-board", 1, true);
                let spin_until = std::time::Instant::now() + delay;
                while std::time::Instant::now() < spin_until {
                    std::hint::spin_loop();
                }
                gate.progress("spec-board", 1, false);
            })
        };
        hammer.await.unwrap();

        // The gate is idle and never answers: only the timeout can end it.
        clock.advance(QUIT_GATE_TIMEOUT * 2);
        let outcome = tokio::time::timeout(Duration::from_secs(5), quit)
            .await
            .unwrap_or_else(|_| panic!("attempt {attempt}: waiter never timed out"))
            .unwrap()
            .unwrap();
        assert_eq!(outcome.reason, Some(QuitAbortReason::Timeout));
        assert_eq!(outcome.epoch, 1);
    }
}

// --- N2 regression: terminal precedes sender drop -------------------------

/// The waiter must be able to read a terminal result the moment its
/// receiver closes, for BOTH teardown paths.
#[tokio::test]
async fn teardown_writes_the_terminal_before_dropping_the_sender() {
    for (reason, teardown) in [
        (QuitAbortReason::Unregistered, false),
        (QuitAbortReason::Destroyed, true),
    ] {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        let effects = if teardown {
            gate.window_gone("spec-board")
        } else {
            gate.unregister("spec-board")
        };
        // Observed straight after the call returns, before the waiter has
        // run at all: the decision is already durable.
        assert_eq!(
            gate.terminal_snapshot_for_test(1).map(|o| o.reason),
            Some(Some(reason))
        );
        apply_quit_gate_effects(host.as_ref(), &effects);

        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(outcome.reason, Some(reason));
        assert_eq!(host.exits(), 0);
    }
}

#[test]
fn outcome_serialization_omits_absent_fields() {
    let json = serde_json::to_value(QuitOutcome::in_flight(7)).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "outcome": "InFlight", "epoch": 7 })
    );
    let json = serde_json::to_value(QuitOutcome::aborted(
        2,
        QuitAbortReason::Timeout,
        vec!["b".into(), "a".into()],
        vec!["c".into()],
    ))
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "outcome": "Aborted",
            "epoch": 2,
            "reason": "timeout",
            "refusingLabels": ["b", "a"],
            "unansweredLabels": ["c"],
        })
    );
    assert_eq!(
        serde_json::to_value(QuitGateRegistration::Registered).unwrap(),
        serde_json::json!({ "status": "Registered" })
    );
}

/// Production sources of the crate root and its private root modules.
const PRODUCTION_ROOT_SOURCES: [&str; 14] = [
    include_str!("../../lib.rs"),
    include_str!("../../app_run.rs"),
    include_str!("../../app_run/exit.rs"),
    include_str!("../../app_run/setup.rs"),
    include_str!("../../co_managed.rs"),
    include_str!("../../co_managed/route.rs"),
    include_str!("../../pty_wiring.rs"),
    include_str!("../../quit_gate.rs"),
    include_str!("../../reader_demand.rs"),
    include_str!("../../restore.rs"),
    include_str!("../../restore/native.rs"),
    include_str!("../../startup.rs"),
    include_str!("../../web_server.rs"),
    include_str!("../../window.rs"),
];

/// The gate owns the only production `AppHandle::exit` call. Guarding it
/// here keeps a second exit path from being added silently.
#[test]
fn exactly_one_production_exit_site() {
    let production = PRODUCTION_ROOT_SOURCES.concat();
    let production = production.as_str();

    // Collect the RECEIVER of every `.exit(` call in production code, so a
    // second exit smuggled in as `app_handle.exit(0)` or `handle.exit(0)`
    // fails here instead of slipping past a literal match.
    let receivers: Vec<String> = production
        .match_indices(".exit(")
        .map(|(idx, _)| {
            production[..idx]
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<Vec<char>>()
                .into_iter()
                .rev()
                .collect()
        })
        .collect();

    // `app` is the one real `AppHandle::exit`; `host` is the injected seam
    // that routes every Exiting decision to it.
    let unexpected: Vec<&String> = receivers
        .iter()
        .filter(|recv| recv.as_str() != "app" && recv.as_str() != "host")
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected exit receivers: {unexpected:?}"
    );
    assert_eq!(
        receivers
            .iter()
            .filter(|recv| recv.as_str() == "app")
            .count(),
        1,
        "quit gate must keep exactly one AppHandle::exit site"
    );
}
