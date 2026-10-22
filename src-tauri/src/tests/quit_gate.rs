use super::{
    apply_quit_gate_effects, quit_gate_run, quit_gate_window_gone, QuitAbortReason, QuitGate,
    QuitGateClock, QuitGateHost, QuitGateRegistration, QuitOutcome, QuitOutcomeKind,
    QUIT_GATE_TIMEOUT,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

#[derive(Default)]
struct TestHost {
    events: Mutex<Vec<(String, String, serde_json::Value)>>,
    exits: AtomicUsize,
    /// Ordered side effects: `"close_detached"` and `"exit"`.
    ops: Mutex<Vec<String>>,
    fail_event: Mutex<Option<String>>,
    gate: Mutex<Option<Weak<QuitGate>>>,
    /// Registration attempted from inside the `app_quit_started` emit, to
    /// prove the round was already installed when the event went out.
    probe_at_start: Mutex<Option<QuitGateRegistration>>,
}

impl QuitGateHost for TestHost {
    fn emit_to(&self, label: &str, event: &str, payload: serde_json::Value) -> Result<(), String> {
        if event == "app_quit_started" {
            let probe = self
                .gate
                .lock()
                .unwrap()
                .as_ref()
                .and_then(Weak::upgrade)
                .map(|gate| gate.register("probe-at-start"));
            *self.probe_at_start.lock().unwrap() = probe;
        }
        if self.fail_event.lock().unwrap().as_deref() == Some(event) {
            return Err(format!("injected emit failure for {event}"));
        }
        self.events
            .lock()
            .unwrap()
            .push((label.to_string(), event.to_string(), payload));
        Ok(())
    }

    fn close_detached_terminals(&self) -> Vec<String> {
        self.ops.lock().unwrap().push("close_detached".to_string());
        Vec::new()
    }

    fn exit(&self, _code: i32) {
        self.ops.lock().unwrap().push("exit".to_string());
        self.exits.fetch_add(1, Ordering::SeqCst);
    }
}

impl TestHost {
    fn ops(&self) -> Vec<String> {
        self.ops.lock().unwrap().clone()
    }

    fn ops_named(&self, op: &str) -> usize {
        self.ops().iter().filter(|o| *o == op).count()
    }

    /// Both ops present, close strictly before exit.
    fn assert_closed_before_exit(&self) {
        let ops = self.ops();
        let close = ops.iter().position(|o| o == "close_detached");
        let exit = ops.iter().position(|o| o == "exit");
        assert!(close.is_some() && exit.is_some(), "ops: {ops:?}");
        assert!(close < exit, "close_detached must precede exit: {ops:?}");
    }

    fn exits(&self) -> usize {
        self.exits.load(Ordering::SeqCst)
    }

    fn events_named(&self, event: &str) -> Vec<(String, serde_json::Value)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, name, _)| name == event)
            .map(|(label, _, payload)| (label.clone(), payload.clone()))
            .collect()
    }

    fn outcome_to_main(&self) -> Option<serde_json::Value> {
        self.events_named("app_quit_outcome")
            .into_iter()
            .last()
            .map(|(_, payload)| payload)
    }
}

/// Manually stepped clock. `watch` (not `Notify`) so an `advance` between a
/// sleeper's tick read and its await cannot be lost.
struct TestClock {
    ticks: tokio::sync::watch::Sender<Duration>,
}

impl Default for TestClock {
    fn default() -> Self {
        Self {
            ticks: tokio::sync::watch::channel(Duration::ZERO).0,
        }
    }
}

impl TestClock {
    fn advance(&self, by: Duration) {
        self.ticks.send_modify(|now| *now += by);
    }
}

impl QuitGateClock for TestClock {
    fn now(&self) -> Duration {
        *self.ticks.borrow()
    }

    fn sleep_until(&self, at: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let mut rx = self.ticks.subscribe();
        Box::pin(async move {
            loop {
                if *rx.borrow_and_update() >= at {
                    return;
                }
                if rx.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        })
    }
}

fn new_gate() -> (
    Arc<QuitGate>,
    Arc<TestHost>,
    Arc<dyn QuitGateHost>,
    Arc<TestClock>,
) {
    let clock = Arc::new(TestClock::default());
    let gate = Arc::new(QuitGate::with_clock(clock.clone()));
    let host = Arc::new(TestHost::default());
    *host.gate.lock().unwrap() = Some(Arc::downgrade(&gate));
    let dyn_host: Arc<dyn QuitGateHost> = host.clone();
    (gate, host, dyn_host, clock)
}

/// Lets the spawned quit task reach its next await point. The clock only
/// moves when a test calls `TestClock::advance`, so yielding is safe.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

fn spawn_quit(
    gate: &Arc<QuitGate>,
    host: &Arc<dyn QuitGateHost>,
    attempt: &str,
) -> tokio::task::JoinHandle<Result<QuitOutcome, String>> {
    let gate = Arc::clone(gate);
    let host = Arc::clone(host);
    let attempt = attempt.to_string();
    tokio::spawn(async move { quit_gate_run(gate, host, "main", false, None, Some(attempt)).await })
}

async fn force_quit(
    gate: &Arc<QuitGate>,
    host: &Arc<dyn QuitGateHost>,
    epoch: Option<u64>,
) -> QuitOutcome {
    quit_gate_run(
        Arc::clone(gate),
        Arc::clone(host),
        "main",
        true,
        epoch,
        None,
    )
    .await
    .expect("force quit from main is never an Err")
}

// --- consent, refusal, timeout -------------------------------------------

#[tokio::test]
async fn all_consent_exits_once() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    gate.register("watchers");

    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 1, true));

    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(outcome, QuitOutcome::exiting(1));
    assert_eq!(host.exits(), 1);
    // Terminal Exiting is not an abort: no app_quit_outcome, no cancels.
    assert!(host.events_named("app_quit_outcome").is_empty());
    assert!(host.events_named("app_quit_cancelled").is_empty());
}

#[tokio::test]
async fn no_registered_gates_exits_immediately() {
    let (gate, host, dyn_host, _clock) = new_gate();
    let outcome = spawn_quit(&gate, &dyn_host, "attempt-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome, QuitOutcome::exiting(1));
    assert_eq!(host.exits(), 1);
}

#[tokio::test]
async fn all_consent_writes_terminal_before_cleanup() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
    // Cleanup already ran (a new round may be allocated), yet the terminal
    // slot still reads Exiting — cleanup cannot replace it with Aborted.
    assert_eq!(
        gate.terminal_snapshot_for_test(1),
        Some(QuitOutcome::exiting(1))
    );
    assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
    assert_eq!(host.exits(), 1);
}

#[tokio::test]
async fn refusal_aborts_immediately_despite_busy_peer() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    gate.register("watchers");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    gate.progress("watchers", 1, true); // peer is mid-save
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));

    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(outcome.outcome, QuitOutcomeKind::Aborted);
    assert_eq!(outcome.epoch, 1);
    assert_eq!(outcome.reason, Some(QuitAbortReason::Refused));
    assert_eq!(
        outcome.refusing_labels.as_deref(),
        Some(["spec-board".to_string()].as_slice())
    );
    assert_eq!(host.exits(), 0);
    let emitted = host.outcome_to_main().unwrap();
    assert_eq!(emitted["outcome"], "Aborted");
    assert_eq!(emitted["epoch"], 1);
    assert_eq!(emitted["reason"], "refused");
    assert_eq!(emitted["refusingLabels"][0], "spec-board");
}

#[tokio::test]
async fn cancellation_targets_every_gate_of_the_epoch() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    gate.register("watchers");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 1, false));
    quit.await.unwrap().unwrap();

    let mut targets: Vec<String> = host
        .events_named("app_quit_cancelled")
        .into_iter()
        .map(|(label, payload)| {
            assert_eq!(payload["epoch"], 1);
            assert_eq!(payload["label"], label);
            label
        })
        .collect();
    targets.sort();
    assert_eq!(targets, vec!["spec-board", "watchers"]);
}

#[tokio::test]
async fn no_timeout_at_29999ms_aborts_at_30s() {
    let (gate, host, dyn_host, clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    clock.advance(Duration::from_millis(29_999));
    settle().await;
    assert!(!quit.is_finished(), "must outlast main's 10s Force offer");
    assert_eq!(gate.terminal_snapshot_for_test(1), None);

    clock.advance(Duration::from_millis(1));
    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(outcome.outcome, QuitOutcomeKind::Aborted);
    assert_eq!(outcome.reason, Some(QuitAbortReason::Timeout));
    assert_eq!(
        outcome.unanswered_labels.as_deref(),
        Some(["spec-board".to_string()].as_slice())
    );
    assert_eq!(host.exits(), 0);
    assert_eq!(QUIT_GATE_TIMEOUT, Duration::from_secs(30));
}

#[tokio::test]
async fn timeout_labels_are_sorted() {
    let (gate, _host, dyn_host, clock) = new_gate();
    gate.register("zeta");
    gate.register("alpha");
    gate.register("mid");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    clock.advance(QUIT_GATE_TIMEOUT);
    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(
        outcome.unanswered_labels.as_deref(),
        Some(["alpha".to_string(), "mid".to_string(), "zeta".to_string()].as_slice())
    );
}

// --- pause / resume ------------------------------------------------------

#[tokio::test]
async fn busy_pause_preserves_remaining_budget() {
    let (gate, host, dyn_host, clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    clock.advance(Duration::from_secs(10));
    settle().await;
    gate.progress("spec-board", 1, true); // 20s left, clock stops

    clock.advance(Duration::from_secs(300)); // slow save
    settle().await;
    assert!(!quit.is_finished(), "a busy gate must not time out");

    gate.progress("spec-board", 1, false); // 20s budget restored
    clock.advance(Duration::from_millis(19_999));
    settle().await;
    assert!(!quit.is_finished());

    clock.advance(Duration::from_millis(1));
    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(outcome.reason, Some(QuitAbortReason::Timeout));
    assert_eq!(host.exits(), 0);
}

#[tokio::test]
async fn near_deadline_busy_wins_over_timeout() {
    let (gate, _host, dyn_host, clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    clock.advance(Duration::from_millis(29_999));
    settle().await;
    gate.progress("spec-board", 1, true);
    clock.advance(Duration::from_secs(600));
    settle().await;
    assert!(!quit.is_finished());

    gate.progress("spec-board", 1, false);
    clock.advance(Duration::from_millis(1));
    assert_eq!(
        quit.await.unwrap().unwrap().reason,
        Some(QuitAbortReason::Timeout)
    );
}

#[tokio::test]
async fn busy_progress_cannot_affect_a_later_epoch() {
    let (gate, host, dyn_host, clock) = new_gate();
    gate.register("spec-board");
    let first = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
    first.await.unwrap().unwrap();

    gate.register("spec-board");
    let second = spawn_quit(&gate, &dyn_host, "attempt-2");
    settle().await;
    gate.progress("spec-board", 1, true); // stale epoch: ignored

    clock.advance(QUIT_GATE_TIMEOUT);
    let outcome = second.await.unwrap().unwrap();
    assert_eq!(outcome.epoch, 2);
    assert_eq!(outcome.reason, Some(QuitAbortReason::Timeout));
}

// --- resolve validation --------------------------------------------------

#[tokio::test]
async fn stale_double_and_unknown_resolve_do_nothing() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    assert!(gate
        .resolve("spec-board", 99, true)
        .outcome_to_main
        .is_none());
    assert!(gate.resolve("ghost", 1, false).outcome_to_main.is_none());
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
    // Duplicate answer, now with a refusal, must not reopen the decision.
    assert!(gate
        .resolve("spec-board", 1, false)
        .outcome_to_main
        .is_none());

    assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
    assert_eq!(host.exits(), 1);
}

// --- registration lifecycle ----------------------------------------------

#[tokio::test]
async fn registration_is_rejected_during_a_round_and_retried_after() {
    let (gate, host, dyn_host, clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    assert_eq!(
        gate.register("watchers"),
        QuitGateRegistration::InFlight { epoch: 1 }
    );
    // The late registrant is excluded from the atomic snapshot, so it is
    // not an unanswered gate either.
    clock.advance(QUIT_GATE_TIMEOUT);
    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(
        outcome.unanswered_labels.as_deref(),
        Some(["spec-board".to_string()].as_slice())
    );

    // Retry after the epoch terminates now succeeds.
    assert_eq!(gate.register("watchers"), QuitGateRegistration::Registered);
    let second = spawn_quit(&gate, &dyn_host, "attempt-2");
    settle().await;
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 2, true));
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 2, true));
    assert_eq!(second.await.unwrap().unwrap(), QuitOutcome::exiting(2));
}

#[tokio::test]
async fn unanswered_unregister_aborts_but_consented_unregister_does_not() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    gate.register("watchers");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    // Consented gate leaves: its consent is final, the round survives.
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 1, true));
    apply_quit_gate_effects(host.as_ref(), &gate.unregister("watchers"));
    settle().await;
    assert!(!quit.is_finished());

    // Unanswered gate leaves: abort with `unregistered`.
    apply_quit_gate_effects(host.as_ref(), &gate.unregister("spec-board"));
    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(outcome.reason, Some(QuitAbortReason::Unregistered));
    assert_eq!(
        outcome.unanswered_labels.as_deref(),
        Some(["spec-board".to_string()].as_slice())
    );
    assert_eq!(host.exits(), 0);
}

#[tokio::test]
async fn unanswered_destruction_aborts_and_consented_destruction_does_not() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    gate.register("watchers");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 1, true));
    quit_gate_window_gone(&gate, host.as_ref(), "watchers");
    settle().await;
    assert!(!quit.is_finished());

    // A dirty board destroyed without answering is NOT consent.
    quit_gate_window_gone(&gate, host.as_ref(), "spec-board");
    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(outcome.reason, Some(QuitAbortReason::Destroyed));
    assert_eq!(
        outcome.unanswered_labels.as_deref(),
        Some(["spec-board".to_string()].as_slice())
    );
    assert_eq!(host.exits(), 0);
}

#[tokio::test]
async fn main_destruction_retains_the_launched_round() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    quit_gate_window_gone(&gate, host.as_ref(), "main");
    settle().await;
    assert!(!quit.is_finished(), "main is not a gate; the round stands");

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
    assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
    assert_eq!(host.exits(), 1);
}

#[tokio::test]
async fn destroyed_gate_is_dropped_from_the_next_round() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    quit_gate_window_gone(&gate, host.as_ref(), "spec-board");

    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
    assert_eq!(host.exits(), 1);
}

// --- start event ---------------------------------------------------------

#[tokio::test]
async fn start_event_follows_installation_and_precedes_a_slow_gate() {
    let (gate, host, dyn_host, clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-xyz");
    settle().await;

    // Emitted while the round was already installed: the probe registration
    // made from inside the emit was rejected as InFlight.
    assert_eq!(
        *host.probe_at_start.lock().unwrap(),
        Some(QuitGateRegistration::InFlight { epoch: 1 })
    );
    let started = host.events_named("app_quit_started");
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].0, "main");
    assert_eq!(started[0].1["epoch"], 1);
    assert_eq!(started[0].1["attemptId"], "attempt-xyz");

    // ...and before the slow gate answers.
    let requested = host.events_named("app_quit_requested");
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].0, "spec-board");
    gate.progress("spec-board", 1, true);
    clock.advance(Duration::from_secs(120));
    settle().await;
    assert!(!quit.is_finished());

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
    assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
}

#[tokio::test]
async fn failed_start_emit_aborts_the_installed_round() {
    let (gate, host, dyn_host, _clock) = new_gate();
    *host.fail_event.lock().unwrap() = Some("app_quit_started".to_string());
    gate.register("spec-board");

    let outcome = spawn_quit(&gate, &dyn_host, "attempt-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.outcome, QuitOutcomeKind::Aborted);
    assert_eq!(outcome.epoch, 1);
    assert_eq!(outcome.reason, Some(QuitAbortReason::Cancelled));
    assert_eq!(host.exits(), 0);
    // The round is released, so a later attempt can run.
    assert_eq!(gate.register("watchers"), QuitGateRegistration::Registered);
}

// --- concurrency and epochs ----------------------------------------------

#[tokio::test]
async fn concurrent_non_force_returns_in_flight_matching_the_terminal_outcome() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let first = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    let second = quit_gate_run(
        Arc::clone(&gate),
        Arc::clone(&dyn_host),
        "main",
        false,
        None,
        Some("attempt-2".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(second, QuitOutcome::in_flight(1));
    // A rejected invoke emits no second start event.
    assert_eq!(host.events_named("app_quit_started").len(), 1);

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
    let terminal = first.await.unwrap().unwrap();
    assert_eq!(terminal.epoch, second.epoch);
    assert_eq!(host.outcome_to_main().unwrap()["epoch"], 1);
}

#[tokio::test]
async fn successive_rounds_get_distinct_increasing_epochs() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");

    let first = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
    assert_eq!(first.await.unwrap().unwrap().epoch, 1);

    gate.register("spec-board");
    let second = spawn_quit(&gate, &dyn_host, "attempt-2");
    settle().await;

    // Late prior-round traffic must not touch the live round.
    gate.progress("spec-board", 1, true);
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
    apply_quit_gate_effects(host.as_ref(), &gate.unregister("gone-with-round-1"));
    settle().await;
    assert!(!second.is_finished());

    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 2, true));
    assert_eq!(second.await.unwrap().unwrap(), QuitOutcome::exiting(2));
    assert_eq!(host.exits(), 1);
}

// --- #2563 detached windows close before exit ----------------------------

#[tokio::test]
async fn exiting_round_closes_detached_before_exit() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));

    assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
    host.assert_closed_before_exit();
    assert_eq!(host.ops_named("close_detached"), 1);
    assert_eq!(host.ops_named("exit"), 1);
}

#[tokio::test]
async fn force_round_closes_detached_before_exit_once() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;

    assert_eq!(
        force_quit(&gate, &dyn_host, Some(1)).await,
        QuitOutcome::exiting(1)
    );
    assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
    host.assert_closed_before_exit();
    assert_eq!(host.ops(), vec!["close_detached", "exit"]);
}

#[tokio::test]
async fn refused_round_never_closes_detached() {
    let (gate, host, dyn_host, _clock) = new_gate();
    gate.register("spec-board");
    let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
    settle().await;
    apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));

    let outcome = quit.await.unwrap().unwrap();
    assert_eq!(outcome.reason, Some(QuitAbortReason::Refused));
    assert_eq!(host.ops_named("close_detached"), 0);
    assert_eq!(host.ops_named("exit"), 0);
}

#[tokio::test]
async fn stale_force_never_closes_detached() {
    let (gate, host, dyn_host, _clock) = new_gate();
    assert_eq!(
        force_quit(&gate, &dyn_host, Some(1)).await,
        QuitOutcome::stale(1)
    );
    assert_eq!(host.ops_named("close_detached"), 0);
    assert_eq!(host.ops_named("exit"), 0);
}

mod force;
