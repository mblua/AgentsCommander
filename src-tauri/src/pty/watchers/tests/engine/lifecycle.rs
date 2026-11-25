use super::*;

/// 9.3.24 - **a session with no agent is never registered**, so a plain shell costs this
/// engine nothing, ever: no entry, no read, no allocation.
///
/// Asserted against the real registration site, which is one helper shared with the
/// scraper, rather than against a re-implementation of its condition.
#[test]
fn a_session_with_no_agent_is_never_registered() {
    let harness = Harness::new();
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&harness.engine))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");

    let shell = Uuid::new_v4();
    crate::commands::session::register_session_samplers(app.handle(), shell, None);
    assert!(!harness.engine.is_session_registered(shell));

    let agent = Uuid::new_v4();
    crate::commands::session::register_session_samplers(
        app.handle(),
        agent,
        Some("a1".to_string()),
    );
    assert!(harness.engine.is_session_registered(agent));
}

/// 9.5.62 and 9.5.66 - the engine publishes each session's STATUS at the end of every
/// tick, so the snapshot command can be synchronous, and the `seq` in the ring is the same
/// value the event carried. That shared identity is what lets the window merge a snapshot
/// with a live stream instead of guessing.
#[tokio::test]
async fn the_engine_publishes_status_every_tick_and_the_ring_shares_the_events_seq() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            state_watcher("perm", "Permission required"),
            occurrence_watcher("read", "Read", WatcherDedupe::None, 2000),
        ],
    );

    // One tick with no match at all: the status is still published.
    harness.backend.paint(id, &["idle"]);
    harness.ticks(1).await;

    let snapshot = harness.history.snapshot(id, None);
    assert!(snapshot.warmed_up);
    assert!(snapshot.matches.is_empty());
    let counters: Vec<(&str, u64)> = snapshot
        .active_watchers
        .iter()
        .map(|counter| (counter.watcher_id.as_str(), counter.count))
        .collect();
    assert_eq!(counters, vec![("perm", 0), ("read", 0)]);

    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;

    // The engine hands the batch to the sink; the ring is the SINK's job, so mirror what
    // the production sink does and assert the identity that makes the merge possible.
    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1);
    harness.history.record(id, &emitted);
    let snapshot = harness.history.snapshot(id, None);
    assert_eq!(snapshot.matches[0].seq, emitted[0].seq);
    assert_eq!(snapshot.last_seq, emitted[0].seq);
    assert_eq!(
        snapshot
            .active_watchers
            .iter()
            .find(|counter| counter.watcher_id == "perm")
            .expect("counter")
            .count,
        1
    );
}

/// 9.5.72 - **a session whose child exits on its own KEEPS its buffer.** It is retired from
/// sampling and nothing else: an API error or a CLI crash is exactly when the evidence is
/// worth keeping, and it is the only thing left to look at.
#[tokio::test]
async fn a_session_that_dies_on_its_own_is_retired_but_keeps_its_history() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;
    harness.history.record(id, &harness.emitted());

    // Twice: a read that says `Gone` makes the engine re-resolve the backend once and ask
    // again, which is what covers a session that changed route.
    harness.backend.push(id, Painted::Gone);
    harness.backend.push(id, Painted::Gone);
    harness.ticks(1).await;

    assert!(!harness.engine.is_session_registered(id));
    let snapshot = harness.history.snapshot(id, None);
    assert_eq!(snapshot.matches.len(), 1, "the evidence must survive");
    assert!(snapshot.warmed_up);
}

/// 9.5.68 and 9.5.70 - `purge_session_side_state` retires FIRST and purges second, and the
/// entry cannot come back: only the engine creates entries, and only while holding its
/// registration lock.
#[tokio::test]
async fn purging_a_destroyed_session_retires_it_first_and_the_entry_never_returns() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;
    harness.history.record(id, &harness.emitted());
    assert!(harness.history.snapshot(id, None).warmed_up);

    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&harness.engine))
        .manage(Arc::clone(&harness.history))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");
    crate::commands::session::purge_session_side_state(app.handle(), id);

    assert!(!harness.engine.is_session_registered(id));
    assert!(!harness.history.snapshot(id, None).warmed_up);

    // Whatever else ticks, the entry stays gone: the session is no longer registered, so
    // no tick can publish it back.
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(5).await;
    assert!(!harness.history.snapshot(id, None).warmed_up);
}

/// 9.5.68 and 9.5.69 (call-site guard) - **the finding revision 1 of the plan missed.**
///
/// There are three production exits from `SessionManager`, not one, and the post-commit
/// cleanup is two parallel copies. A restart is a normal flow - the invoke handler, the
/// mailbox and a bulk settings save all reach it - so a purge wired into only the destroy
/// path would orphan up to 500 ring entries on every restart, under an id already gone
/// from the session list and therefore unreachable from the UI.
///
/// This pins BOTH call sites against the source, so removing either one fails here rather
/// than leaking silently.
#[test]
fn both_post_commit_cleanups_purge_the_per_session_side_state() {
    const SOURCE: &str = include_str!("../../../../commands/session.rs");
    const RESTART: &str = include_str!("../../../../commands/session/restart/settlement.rs");
    let predicates = |source: &str, restart: &str| {
        let definitions = restart.matches("fn publish_restart_destroyed").count();
        let restart_body = restart
            .split("fn publish_restart_destroyed")
            .nth(1)
            .and_then(|body| {
                body.split(
                    "
}",
                )
                .next()
            })
            .unwrap_or_default();
        let loop_body = source
            .split("for session_id in outcome.destroyed_ids.iter().copied()")
            .nth(1)
            .and_then(|body| {
                body.split(
                    "
    }",
                )
                .next()
            })
            .unwrap_or_default();
        [
            definitions == 1 && restart_body.contains("purge_session_side_state("),
            loop_body.contains("purge_session_side_state("),
            !loop_body.contains("retained_exited_ids"),
        ]
    };
    assert_eq!(
        predicates(SOURCE, RESTART),
        [true; 3],
        "both cleanup owners must purge"
    );
    let restart = RESTART.replace("purge_session_side_state(", "removed_purge(");
    assert_eq!(predicates(SOURCE, &restart), [false, true, true]);
    let loop_start = SOURCE
        .find("for session_id in outcome.destroyed_ids.iter().copied()")
        .expect("destroyed-only loop");
    let purge = SOURCE[loop_start..]
        .find("purge_session_side_state(")
        .unwrap()
        + loop_start;
    let mut removed = SOURCE.to_string();
    removed.replace_range(
        purge..purge + "purge_session_side_state".len(),
        "removed_purge",
    );
    assert_eq!(predicates(&removed, RESTART), [true, false, true]);
    let mut retained = SOURCE.to_string();
    retained.insert_str(purge, "retained_exited_ids ");
    assert_eq!(predicates(&retained, RESTART), [true, true, false]);
    let duplicate = format!(
        "{RESTART}
fn publish_restart_destroyed() {{}}"
    );
    assert_eq!(predicates(SOURCE, &duplicate), [false, true, true]);
}

// ----- #5206 P03: engine runtime start ---------------------------------------------------

use std::sync::atomic::AtomicUsize as P03Count;
use std::time::Instant;

const P03_CHILD: &str = "AC5206_P03_ENGINE_CHILD";

/// The application error records of this (child) process, in order.
struct P03Errors(Mutex<Vec<String>>);

static P03_LOG: P03Errors = P03Errors(Mutex::new(Vec::new()));

impl log::Log for P03Errors {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Error
    }
    fn log(&self, record: &log::Record) {
        if record.level() <= log::Level::Error {
            self.0.lock().unwrap().push(format!("{}", record.args()));
        }
    }
    fn flush(&self) {}
}

fn p03_errors() -> Vec<String> {
    P03_LOG.0.lock().unwrap().clone()
}

/// The child half installs the process-wide logger and returns true. The parent half reruns
/// exactly `name` in a child of this test binary and requires its success within 60 s.
fn p03_in_child(name: &str) -> bool {
    if std::env::var(P03_CHILD).is_ok() {
        log::set_max_level(log::LevelFilter::Error);
        log::set_logger(&P03_LOG).expect("child installs the error logger");
        return true;
    }
    let test = format!("{}::{name}", module_path!().split_once("::").unwrap().1);
    let exe = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(exe)
        .args([test.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(P03_CHILD, "1")
        .stdin(std::process::Stdio::null())
        .spawn()
        .expect("spawn the child test");
    // 3000 polls of 20 ms: the same 60 s budget, then the child is killed and reaped.
    let status = (0..3000).find_map(|_| {
        std::thread::sleep(Duration::from_millis(20));
        child.try_wait().unwrap()
    });
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child {name} timed out");
    };
    assert!(status.success(), "child {name} failed: {status}");
    false
}

/// An engine with one agent session whose every tick would read the backend.
fn p03_harness() -> Harness {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["idle"]);
    harness
}

/// Runs `run_with_runtime` on its own thread; returns (joined in time, join was Ok).
fn p03_run<F>(engine: &Arc<WatcherEngine>, make: F, shutdown: &ShutdownSignal) -> (bool, bool)
where
    F: FnOnce() -> std::io::Result<tokio::runtime::Runtime> + std::panic::UnwindSafe + Send,
    F: 'static,
{
    let (engine, shutdown) = (Arc::clone(engine), shutdown.clone());
    let (done, wait) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        engine.run_with_runtime(make, shutdown);
        let _ = done.send(());
    });
    let in_time = wait.recv_timeout(Duration::from_secs(3)).is_ok();
    (in_time, in_time && thread.join().is_ok())
}

type ShutdownSignal = crate::shutdown::ShutdownSignal;

/// A factory error or unwind (literal, `String`, opaque payload) logs one application
/// error with the name, the cause and the frozen samples, and never reads or ticks.
#[test]
fn a_failed_engine_runtime_logs_once_and_never_ticks() {
    if !p03_in_child("a_failed_engine_runtime_logs_once_and_never_ticks") {
        return;
    }
    type Make = Box<dyn FnOnce() -> std::io::Result<tokio::runtime::Runtime> + Send>;
    let cases: [(&str, Make); 4] = [
        (
            "E_FACTORY",
            Box::new(|| Err(std::io::Error::other("E_FACTORY"))),
        ),
        ("E_LITERAL", Box::new(|| panic!("E_LITERAL"))),
        (
            "E_STRING",
            Box::new(|| std::panic::panic_any(String::from("E_STRING"))),
        ),
        (
            "non-string panic payload",
            Box::new(|| std::panic::panic_any(7u32)),
        ),
    ];
    for (cause, make) in cases {
        let harness = p03_harness();
        let entered = Arc::new(P03Count::new(0));
        let counter = Arc::clone(&entered);
        let make = std::panic::AssertUnwindSafe(make);
        let factory = move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let whole = make; // capture the wrapper, not its field
            (whole.0)()
        };
        let before = p03_errors().len();
        let shutdown = ShutdownSignal::new();
        assert_eq!(
            p03_run(&harness.engine, factory, &shutdown),
            (true, true),
            "{cause}"
        );
        assert_eq!(entered.load(Ordering::SeqCst), 1, "{cause}");
        assert_eq!(harness.backend.reads.load(Ordering::SeqCst), 0, "{cause}");
        assert_eq!(harness.engine.ticks.load(Ordering::SeqCst), 0, "{cause}");
        let logs = p03_errors()[before..].to_vec();
        assert_eq!(logs.len(), 1, "{cause}: {logs:?}");
        assert!(
            logs[0].contains("WatcherEngine") && logs[0].contains(cause),
            "{logs:?}"
        );
        assert!(
            logs[0].contains("frozen") && !logs[0].contains("PANICKED"),
            "{logs:?}"
        );
    }
}

/// The production factory is CurrentThread. Ticks start after 200 ms, run one at a time
/// at least 200 ms apart, wait out a held settings lock, and stop on cancel.
#[test]
fn the_production_runtime_ticks_on_cadence_and_stops_on_cancel() {
    use tokio::runtime::RuntimeFlavor;
    let rt = (WatcherEngine::RUNTIME_FACTORY)().expect("production runtime");
    assert_eq!(rt.handle().runtime_flavor(), RuntimeFlavor::CurrentThread);
    drop(rt);

    let harness = p03_harness();
    let gate = Arc::new(GatedPatterns(
        Arc::clone(&harness.patterns),
        Default::default(),
    ));
    let engine = WatcherEngine::new(
        Arc::clone(&harness.backends) as Arc<dyn WatcherBackendSource>,
        Arc::clone(&gate) as Arc<dyn WatcherPatternSource>,
        Arc::clone(&harness.sink) as Arc<dyn WatcherEventSink>,
        Arc::clone(&harness.history),
    );
    let shutdown = ShutdownSignal::new();
    let start = Instant::now();
    let thread = std::thread::spawn({
        let (engine, shutdown) = (Arc::clone(&engine), shutdown.clone());
        move || engine.run_with_runtime(WatcherEngine::RUNTIME_FACTORY, shutdown)
    });
    let ticks = || engine.ticks.load(Ordering::SeqCst);
    let wait_for = |n: u64| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while ticks() < n {
            assert!(Instant::now() < deadline, "tick {n} never came");
            std::thread::sleep(Duration::from_millis(2));
        }
        Instant::now()
    };
    let first = wait_for(1);
    assert!(first - start >= TICK_INTERVAL, "first tick after 200 ms");
    let second = wait_for(2);
    assert!(second - first >= TICK_INTERVAL - Duration::from_millis(10));

    let held = gate.1.blocking_write();
    let third = wait_for(3);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        ticks(),
        3,
        "no tick overlaps the one waiting on the settings lock"
    );
    drop(held);
    let fourth = wait_for(4);
    assert!(fourth - third >= Duration::from_millis(600));

    shutdown.token().cancel();
    let cancelled = Instant::now();
    thread.join().expect("engine thread");
    assert!(cancelled.elapsed() < Duration::from_secs(1));
}

/// Cancelled before its first tick, the engine exits without ticking.
#[test]
fn the_engine_cancelled_before_its_first_tick_never_ticks() {
    let harness = p03_harness();
    let shutdown = ShutdownSignal::new();
    shutdown.token().cancel();
    let run = p03_run(&harness.engine, WatcherEngine::RUNTIME_FACTORY, &shutdown);
    assert_eq!(run, (true, true));
    assert_eq!(harness.engine.ticks.load(Ordering::SeqCst), 0);
    assert_eq!(harness.backend.reads.load(Ordering::SeqCst), 0);
}

/// A tick panic is still contained: one PANICKED error, the thread ends cleanly.
#[test]
fn a_panicking_tick_is_contained_with_the_real_runtime() {
    if !p03_in_child("a_panicking_tick_is_contained_with_the_real_runtime") {
        return;
    }
    let harness = p03_harness();
    let engine = WatcherEngine::new(
        Arc::clone(&harness.backends) as Arc<dyn WatcherBackendSource>,
        Arc::new(PanickingPatterns) as Arc<dyn WatcherPatternSource>,
        Arc::clone(&harness.sink) as Arc<dyn WatcherEventSink>,
        Arc::clone(&harness.history),
    );
    let shutdown = ShutdownSignal::new();
    assert_eq!(
        p03_run(&engine, WatcherEngine::RUNTIME_FACTORY, &shutdown),
        (true, true)
    );
    let logs = p03_errors();
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert!(logs[0].contains("PANICKED"), "{logs:?}");
}

/// Patterns behind an async lock the test can hold, like `SettingsState`'s.
struct GatedPatterns(Arc<FakePatterns>, tokio::sync::RwLock<()>);

impl WatcherPatternSource for GatedPatterns {
    fn resolve(&self) -> BoxFuture<'_, Option<HashMap<String, AgentResolution>>> {
        Box::pin(async move {
            let _read = self.1.read().await;
            self.0.resolve().await
        })
    }
}

struct PanickingPatterns;

impl WatcherPatternSource for PanickingPatterns {
    fn resolve(&self) -> BoxFuture<'_, Option<HashMap<String, AgentResolution>>> {
        Box::pin(async move { panic!("tick panic") })
    }
}

/// Linux: the real factory under `RLIMIT_NOFILE=5` fails in the driver, logs the cause
/// and the frozen samples, never reads or ticks, and leaves no descriptor or thread.
#[cfg(target_os = "linux")]
#[test]
fn the_production_runtime_fails_cleanly_when_descriptors_run_out() {
    if !p03_in_child("the_production_runtime_fails_cleanly_when_descriptors_run_out") {
        return;
    }
    let count = |dir: &str| std::fs::read_dir(dir).unwrap().count();
    let harness = p03_harness();
    let (fds, tasks) = (count("/proc/self/fd"), count("/proc/self/task"));
    assert_eq!(
        fds, 4,
        "three inherited descriptors plus the directory being read"
    );
    let mut old = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: plain getrlimit/setrlimit on this process with valid pointers.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut old) }, 0);
    let low = libc::rlimit {
        rlim_cur: 5,
        rlim_max: old.rlim_max,
    };
    // SAFETY: setrlimit on this process with a valid pointer to an initialized rlimit.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &low) }, 0);
    let shutdown = ShutdownSignal::new();
    let run = p03_run(&harness.engine, WatcherEngine::RUNTIME_FACTORY, &shutdown);
    // SAFETY: setrlimit on this process with a valid pointer to an initialized rlimit.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &old) }, 0);
    assert_eq!(run, (true, true));
    assert_eq!(
        (count("/proc/self/fd"), count("/proc/self/task")),
        (fds, tasks)
    );
    assert_eq!(harness.backend.reads.load(Ordering::SeqCst), 0);
    assert_eq!(harness.engine.ticks.load(Ordering::SeqCst), 0);
    let logs = p03_errors();
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert!(
        logs[0].contains("WatcherEngine") && logs[0].contains("frozen"),
        "{logs:?}"
    );
    assert!(logs[0].contains("Too many open files"), "{logs:?}");
}
