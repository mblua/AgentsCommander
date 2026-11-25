use super::*;
use crate::session::warnings::{emit_session_warning, new_session_warning_state};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use crate::pty::context_scrape::ContextSample;
use std::time::Duration;
use tauri::Listener;
use uuid::Uuid;

fn warning() -> SessionWarning {
    SessionWarning::new(Uuid::nil(), "KEY", "kind", "message")
}

fn assert_event(receiver: &mpsc::Receiver<String>, expected: &SessionWarning) {
    let payload = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("live event");
    let actual: serde_json::Value = serde_json::from_str(&payload).expect("event JSON");
    assert_eq!(
        actual,
        serde_json::json!({
            "sessionId": expected.session_id, "key": expected.key,
            "kind": expected.kind, "message": expected.message
        })
    );
    assert!(receiver.try_recv().is_err(), "exactly one live event");
}

#[test]
fn healthy_buffer_precedes_live_event_and_releases_its_lock() {
    let state = new_session_warning_state();
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build warning app");
    let (sender, receiver) = mpsc::channel();
    let (order_sender, order_receiver) = mpsc::channel();
    let observed_state = state.clone();
    let expected = warning();
    let observed_warning = expected.clone();
    app.listen_any("session_env_warning", move |event| {
        let buffered = observed_state.try_lock().ok().map(|mut buffer| {
            let drained = buffer.drain(Some(&observed_warning.session_id));
            let matches = drained == vec![observed_warning.clone()];
            for warning in drained {
                buffer.push(warning);
            }
            matches
        });
        let _ = order_sender.send(buffered);
        let _ = sender.send(event.payload().to_string());
    });

    emit_session_warning(app.handle(), expected.clone());

    assert_eq!(
        order_receiver.try_recv().expect("synchronous callback"),
        Some(true)
    );
    assert_event(&receiver, &expected);
    let mut buffer = state.lock().expect("healthy buffer");
    assert_eq!(buffer.drain(None), vec![expected]);
    assert!(buffer.drain(None).is_empty());
}

#[test]
fn missing_state_still_emits_once_synchronously() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build warning app");
    let (sender, receiver) = mpsc::channel();
    app.listen_any("session_env_warning", move |event| {
        let _ = sender.send(event.payload().to_string());
    });
    let expected = warning();

    emit_session_warning(app.handle(), expected.clone());

    assert_event(&receiver, &expected);
}

#[test]
fn poisoned_buffer_still_emits_without_buffering_or_panicking() {
    let state = new_session_warning_state();
    let poisoned = state.clone();
    let result = std::panic::catch_unwind(move || {
        let _guard = poisoned.lock().expect("initial healthy buffer");
        panic!("poison warning buffer for live-only delivery");
    });
    assert!(result.is_err());
    assert!(state.is_poisoned());
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build warning app");
    let (sender, receiver) = mpsc::channel();
    app.listen_any("session_env_warning", move |event| {
        let _ = sender.send(event.payload().to_string());
    });
    let expected = warning();

    emit_session_warning(app.handle(), expected.clone());

    assert_event(&receiver, &expected);
    let mut buffer = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(buffer.drain(None).is_empty());
}

struct GuardRows;

impl crate::pty::context_scrape::ScreenRowsSource for GuardRows {
    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::Unavailable
    }
}

struct GuardPatterns;

impl crate::pty::context_scrape::ContextPatternSource for GuardPatterns {
    fn patterns(&self) -> BoxFuture<'_, Option<HashMap<String, String>>> {
        Box::pin(async { Some(HashMap::new()) })
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

#[tokio::test]
async fn context_alert_adapter_reads_state_managed_after_construction_and_future_creation() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let services = ContextAlertTauriServices {
        app: app.handle().clone(),
        messages: Arc::new(MessageWarnings::new()),
    };
    assert!(services.public_session(Uuid::nil()).await.is_none());
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/synthetic".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let id = session.id;
    let pending = services.public_session(id);
    assert!(app.manage(Arc::new(tokio::sync::RwLock::new(manager))));
    assert_eq!(pending.await.unwrap().id, id);
    assert!(!services.registered(id));
    services.retire_sample_registration(id);

    let temp = tempfile::tempdir().unwrap();
    let replica = temp
        .path()
        .join("project/.ac/room-1-team/__agent_coordinator");
    std::fs::create_dir_all(&replica).unwrap();
    let target = InternalSystemTarget::for_context_alert(
        "project:room-1-team/coordinator".to_string(),
        replica,
    )
    .unwrap();
    assert_eq!(
        services.validate_delivery_state(id, &target).unwrap_err(),
        "Context scraper is unavailable",
    );
    let scraper = ContextScraper::new(
        Arc::new(GuardRows),
        Arc::new(GuardPatterns),
        Arc::new(GuardEvents),
        Arc::new(GuardSamples),
        Arc::new(GuardPersist),
    );
    assert!(app.manage(Arc::clone(&scraper)));
    assert_eq!(
        services.validate_delivery_state(id, &target).unwrap_err(),
        "Sampled session is no longer registered",
    );
    scraper.register_session(id, "shell".to_string());
    assert!(services.registered(id));
    services.validate_delivery_state(id, &target).unwrap();
    services.retire_sample_registration(id);
    assert!(!services.registered(id));
    assert_eq!(
        services.validate_delivery_state(id, &target).unwrap_err(),
        "Sampled session is no longer registered",
    );
}

#[tokio::test]
async fn context_alert_adapter_observes_late_purge_and_checks_it_before_registration() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let services = ContextAlertTauriServices {
        app: app.handle().clone(),
        messages: Arc::new(MessageWarnings::new()),
    };
    let id = Uuid::new_v4();
    let scraper = ContextScraper::new(
        Arc::new(GuardRows),
        Arc::new(GuardPatterns),
        Arc::new(GuardEvents),
        Arc::new(GuardSamples),
        Arc::new(GuardPersist),
    );
    scraper.register_session(id, "shell".to_string());
    assert!(app.manage(Arc::clone(&scraper)));
    let temp = tempfile::tempdir().unwrap();
    let replica = temp
        .path()
        .join("project/.ac/room-1-team/__agent_coordinator");
    std::fs::create_dir_all(&replica).unwrap();
    let target = InternalSystemTarget::for_context_alert(
        "project:room-1-team/coordinator".to_string(),
        replica,
    )
    .unwrap();
    services.validate_delivery_state(id, &target).unwrap();
    let purge = Arc::new(PurgeGuard::default());
    assert!(app.manage(Arc::clone(&purge)));
    let lease = purge
        .acquire(HashSet::new(), HashSet::from([target.fqn().to_string()]))
        .await;
    let expected = format!("purge-room blocks '{}'", target.fqn());
    assert_eq!(
        services.validate_delivery_state(id, &target),
        Err(expected.clone())
    );
    drop(lease);
    services.validate_delivery_state(id, &target).unwrap();
    let lease = purge
        .acquire(HashSet::new(), HashSet::from([target.fqn().to_string()]))
        .await;
    services.retire_sample_registration(id);
    assert_eq!(services.validate_delivery_state(id, &target), Err(expected));
    drop(lease);
    assert_eq!(
        services.validate_delivery_state(id, &target).unwrap_err(),
        "Sampled session is no longer registered",
    );
}

#[test]
fn context_alert_start_on_without_ambient_tokio() {
    assert!(tokio::runtime::Handle::try_current().is_err());
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let cancellation = CancellationToken::new();
    assert!(tokio::runtime::Handle::try_current().is_err());
    let monitor = start_on(
        app.handle().clone(),
        cancellation.clone(),
        Arc::new(MessageWarnings::new()),
    );
    cancellation.cancel();
    tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(5), monitor.close_and_join())
            .await
            .expect("bounded actor shutdown")
            .expect("joined actor shutdown");
        monitor.close_and_join().await.expect("repeated close");
    });
}

#[derive(Default)]
struct SelectionDeliveries(std::cell::RefCell<Vec<(&'static str, serde_json::Value)>>);

impl crate::session::selection::SelectionEvents for SelectionDeliveries {
    fn selection_event(&self, event: &'static str, value: serde_json::Value) {
        self.0.borrow_mut().push((event, value));
    }
}

struct CountedSelectionPayload<'a> {
    calls: &'a std::cell::Cell<usize>,
    fail: bool,
}

impl serde::Serialize for CountedSelectionPayload<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.calls.set(self.calls.get() + 1);
        if self.fail {
            return Err(serde::ser::Error::custom("issue3950-serialization-failure"));
        }
        serde_json::json!({"selection": 3950}).serialize(serializer)
    }
}

fn serialized_once(
    calls: usize,
    deliveries: &[(&str, serde_json::Value)],
) -> Result<(), &'static str> {
    if calls != 1 {
        return Err("selection payload must serialize exactly once");
    }
    if deliveries != [("issue3950-event", serde_json::json!({"selection": 3950}))] {
        return Err("selection must deliver the same JSON exactly once");
    }
    Ok(())
}

#[test]
fn issue_3950_serialize_once() {
    let calls = std::cell::Cell::new(0);
    let events = SelectionDeliveries::default();
    let payload = CountedSelectionPayload {
        calls: &calls,
        fail: false,
    };
    crate::session::selection::port_test_publish_lifecycle_event(
        &events,
        "issue3950-event",
        &payload,
    );
    assert_eq!(serialized_once(calls.get(), &events.0.borrow()), Ok(()));
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_serialize_once");

    // A double that repeats the real serializer must fail the same one-pass assertion.
    calls.set(0);
    events.0.borrow_mut().clear();
    for _ in 0..2 {
        crate::session::selection::port_test_publish_lifecycle_event(
            &events,
            "issue3950-event",
            &payload,
        );
    }
    assert_eq!(
        serialized_once(calls.get(), &events.0.borrow()),
        Err("selection payload must serialize exactly once")
    );
    println!("\nISSUE3950_MUTANT_REJECTED case=double_serialize");
}

#[test]
fn issue_3950_serialize_error() {
    let calls = std::cell::Cell::new(0);
    let events = SelectionDeliveries::default();
    crate::session::selection::port_test_publish_lifecycle_event(
        &events,
        "issue3950-event",
        &CountedSelectionPayload {
            calls: &calls,
            fail: true,
        },
    );
    assert_eq!(calls.get(), 1, "failing payload serialized exactly once");
    assert!(
        events.0.borrow().is_empty(),
        "serialization failure has zero sink calls"
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_serialize_error");
}

type SelectionDeliveryTrace = Vec<(&'static str, &'static str, serde_json::Value)>;

fn ordered_selection_delivery(trace: &SelectionDeliveryTrace) -> Result<(), &'static str> {
    let value = serde_json::json!({"selection": 3950});
    if trace
        != &[
            ("native", "issue3950-event", value.clone()),
            ("websocket", "issue3950-event", value),
        ]
    {
        return Err("native must precede WebSocket with the same event and JSON");
    }
    Ok(())
}

fn selection_delivery_trace(fail_native: bool, short_circuit: bool) -> SelectionDeliveryTrace {
    let trace = std::cell::RefCell::new(Vec::new());
    let native_failed = std::cell::Cell::new(false);
    deliver_selection_event(
        "issue3950-event",
        serde_json::json!({"selection": 3950}),
        |event, value| {
            trace.borrow_mut().push(("native", event, value));
            native_failed.set(fail_native);
            if fail_native {
                Err("issue3950-native-failure".into())
            } else {
                Ok(())
            }
        },
        |event, value| {
            // Bad sink double models short-circuiting after native failure.
            if !(short_circuit && native_failed.get()) {
                trace.borrow_mut().push(("websocket", event, value.clone()));
            }
        },
    );
    trace.into_inner()
}

#[test]
fn issue_3950_native_ws_order() {
    assert_eq!(
        ordered_selection_delivery(&selection_delivery_trace(false, false)),
        Ok(())
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_native_ws_order");
}

#[test]
fn issue_3950_native_fail_ws() {
    assert_eq!(
        ordered_selection_delivery(&selection_delivery_trace(true, false)),
        Ok(())
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_native_fail_ws");
    assert_eq!(
        ordered_selection_delivery(&selection_delivery_trace(true, true)),
        Err("native must precede WebSocket with the same event and JSON")
    );
    // The short-circuit double's successful-native opposite control still delivers both.
    assert_eq!(
        ordered_selection_delivery(&selection_delivery_trace(false, true)),
        Ok(())
    );
    println!("\nISSUE3950_MUTANT_REJECTED case=native_short_circuit");
}

// ----- AutoClose missing managed state (AppHandle port and run) -----

use crate::config::coordinator_clocks::{ClockEntry, CoordinatorClocks, CoordinatorClocksState};
use crate::config::settings::SettingsState;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::auto_close::tests::{
    auto_close_session, AutoCloseBackend, Badge, RecordingServices,
};
use crate::session::auto_close::{AutoClose, AutoCloseInputs};
use crate::session::manager::SessionManager;
use crate::session::selection::SelectionCoordinator;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::Manager;

/// The AutoClose tick period, kept local so no production constant is exposed.
const TICK: Duration = Duration::from_secs(60);
const AUTOCLOSE_FQN: &str =
    "events::session::tests::run_skips_a_tick_with_missing_state_and_recovers_on_the_same_task";
const AUTOCLOSE_CHILD: &str = "AC_5206_AUTOCLOSE_CHILD";
const AUTOCLOSE_DONE: &str = "AC5206_AUTOCLOSE_DONE skip-recover";
const AUTOCLOSE_ARGS: [&str; 5] = [
    "--exact",
    "--test-threads=1",
    "--show-output",
    "--format=pretty",
    "--color=never",
];

/// Child mode needs the exact private mark and the exact argv of this one test; any
/// other mark, combination or extra argument fails before a logger is installed.
fn autoclose_child_mode() -> bool {
    let Some(mark) = std::env::var_os(AUTOCLOSE_CHILD) else {
        return false;
    };
    assert_eq!(mark, "skip-recover", "unrecognized {AUTOCLOSE_CHILD}");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut expected = vec![AUTOCLOSE_FQN.to_string()];
    expected.extend(AUTOCLOSE_ARGS.iter().map(|a| a.to_string()));
    assert_eq!(args, expected, "child argv must select only this test");
    true
}

/// The parent's handle on a child it must reap (`Child`, or a double in the probes).
trait Reap {
    fn pid(&self) -> u32;
    fn kill(&mut self) -> std::io::Result<()>;
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl Reap for std::process::Child {
    fn pid(&self) -> u32 {
        self.id()
    }
    fn kill(&mut self) -> std::io::Result<()> {
        std::process::Child::kill(self)
    }
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        std::process::Child::try_wait(self)
    }
}

/// What the bounded reap saw. Only `status: Some` confirms the child is gone.
#[derive(Debug, Default)]
struct ReapReport {
    pid: u32,
    status: Option<std::process::ExitStatus>,
    /// Kill and poll errors, oldest first, the last eight kept.
    errors: Vec<String>,
    elapsed: Duration,
}

impl ReapReport {
    fn note(&mut self, what: &str, error: std::io::Error) {
        if self.errors.len() == 8 {
            self.errors.remove(0);
        }
        self.errors.push(format!("{what}: {error}"));
    }
    fn describe(&self) -> String {
        let reaped = if self.status.is_some() { "yes" } else { "no" };
        format!("pid: {}\nreaped: {reaped}\n{self:#?}\n", self.pid)
    }
}

/// Kill, then poll until `limit`. A poll error neither confirms nor ends the wait; it
/// retries the kill. Only `Ok(Some(status))` confirms.
fn reap_bounded(child: &mut impl Reap, limit: Duration) -> ReapReport {
    let started = std::time::Instant::now();
    let pid = child.pid();
    let mut report = ReapReport {
        pid,
        ..Default::default()
    };
    let mut kill = true;
    loop {
        if std::mem::take(&mut kill) {
            if let Err(error) = child.kill() {
                report.note("kill", error);
            }
        }
        match child.try_wait() {
            Ok(status) => report.status = status,
            Err(error) => {
                report.note("poll", error);
                kill = true;
            }
        }
        if report.status.is_some() || started.elapsed() >= limit {
            report.elapsed = started.elapsed();
            return report;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One supervised child and its private root. Failure and Drop share one bounded reap;
/// the root is kept, with `diagnostics.txt`, on any failure, on unwind and whenever the
/// reap is not confirmed. A failing `sink` (stderr) never stops that retention.
struct Supervised<R: Reap> {
    child: R,
    directory: Option<tempfile::TempDir>,
    started: std::time::Instant,
    reap_limit: Duration,
    reap: Option<ReapReport>,
    sink: Box<dyn std::io::Write>,
}

impl<R: Reap> Supervised<R> {
    fn new(child: R, directory: tempfile::TempDir) -> Self {
        Self {
            child,
            directory: Some(directory),
            started: std::time::Instant::now(),
            reap_limit: Duration::from_secs(10),
            reap: None,
            sink: Box::new(std::io::stderr()),
        }
    }

    /// Waits up to `deadline` from spawn; a timeout or poll error reaps and retains.
    fn wait(&mut self, deadline: Duration) -> Result<std::process::ExitStatus, PathBuf> {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    // Status known: `kill` is a no-op and the poll confirms at once.
                    self.reap = Some(reap_bounded(&mut self.child, Duration::ZERO));
                    return Ok(status);
                }
                Ok(None) if self.started.elapsed() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => return Err(self.retain("child deadline")),
                Err(error) => return Err(self.retain(&format!("child poll failed: {error}"))),
            }
        }
    }

    fn confirmed(&mut self) -> bool {
        if self.reap.is_none() {
            self.reap = Some(reap_bounded(&mut self.child, self.reap_limit));
        }
        self.reap.as_ref().is_some_and(|r| r.status.is_some())
    }

    /// Reaps (bounded) if still needed, then keeps the root with its diagnostics.
    fn retain(&mut self, reason: &str) -> PathBuf {
        let confirmed = self.confirmed();
        let root = match self.directory.take() {
            Some(directory) => directory.keep(),
            None => return PathBuf::new(),
        };
        let report = self.reap.as_ref().map(ReapReport::describe);
        let report = report.unwrap_or_default();
        let text = format!(
            "reason: {reason}\nsince spawn: {:?}\nconfirmed: {confirmed}\n{report}",
            self.started.elapsed()
        );
        let _ = std::fs::write(root.join("diagnostics.txt"), &text);
        let _ = writeln!(self.sink, "{reason}; retained {}", root.display());
        root
    }
}

impl<R: Reap> Drop for Supervised<R> {
    fn drop(&mut self) {
        if self.directory.is_none() {
            return;
        }
        if !self.confirmed() {
            self.retain("dropped with the child reap unconfirmed");
        } else if std::thread::panicking() {
            self.retain("dropped during unwind");
        }
    }
}

/// Exactly the libtest output of one passing run of `AUTOCLOSE_FQN` with one receipt.
fn child_stdout_verdict(stdout: &str) -> Result<(), &'static str> {
    let lines: Vec<&str> = stdout.lines().collect();
    let count = |keep: &dyn Fn(&str) -> bool| lines.iter().filter(|l| keep(l)).count();
    let ok_line = format!("test {AUTOCLOSE_FQN} ... ok");
    if count(&|l| l.starts_with("running ")) != 1 || count(&|l| l == "running 1 test") != 1 {
        return Err("expected one `running 1 test` line");
    }
    if count(&|l| l.starts_with("test ") && l.contains(" ... ")) != 1
        || count(&|l| l == ok_line) != 1
    {
        return Err("expected exactly one success line, for this test");
    }
    let summary = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; ";
    if count(&|l| l.contains("test result:")) != 1 || count(&|l| l.starts_with(summary)) != 1 {
        return Err("expected exactly one summary, of one passing test");
    }
    let mark = AUTOCLOSE_DONE.split(' ').next().unwrap_or(AUTOCLOSE_DONE);
    if count(&|l| l.contains(mark)) != 1 || count(&|l| l == AUTOCLOSE_DONE) != 1 {
        return Err("expected exactly one receipt, for this scenario");
    }
    Ok(())
}

/// The only `Command` of this file: the test binary itself, private stdout/stderr
/// files, null stdin and a private config dir on the Command only.
fn spawn_test_binary(args: &[&str], mark: Option<&str>) -> Supervised<std::process::Child> {
    use std::process::{Command, Stdio};
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(directory.path().join("stdout.txt")).unwrap())
        .stderr(std::fs::File::create(directory.path().join("stderr.txt")).unwrap())
        .env_remove("RUST_TEST_NOCAPTURE")
        .env_remove(AUTOCLOSE_CHILD)
        .env("AGENTSCOMMANDER_CONFIG_DIR", &config)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &config);
    if let Some(mark) = mark {
        command.env(AUTOCLOSE_CHILD, mark);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    Supervised::new(command.spawn().unwrap(), directory)
}

/// Parent verdict on one child: `Ok(stdout)` only for status 0 and the exact oracle.
fn check_child(
    child: &mut Supervised<std::process::Child>,
    deadline: Duration,
) -> Result<String, PathBuf> {
    let status = child.wait(deadline)?;
    let path = child
        .directory
        .as_ref()
        .map(|d| d.path().join("stdout.txt"));
    let stdout = path.and_then(|p| std::fs::read_to_string(p).ok());
    let stdout = stdout.unwrap_or_default();
    if !status.success() {
        return Err(child.retain(&format!("child status {status}")));
    }
    child_stdout_verdict(&stdout).map_err(|why| child.retain(why))?;
    Ok(stdout)
}

/// Parent side of the AutoClose run test.
fn run_autoclose_child() {
    let args = [&[AUTOCLOSE_FQN][..], &AUTOCLOSE_ARGS].concat();
    let mut child = spawn_test_binary(&args, Some("skip-recover"));
    let stdout = check_child(&mut child, Duration::from_secs(45));
    let stdout = stdout.unwrap_or_else(|root| panic!("AutoClose child retained {root:?}"));
    for line in stdout.lines() {
        println!("child| {line}");
    }
}

const LEAD: &str = "myproj:wg-1-team/lead";

struct Handles {
    settings: SettingsState,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pty: Arc<Mutex<PtyManager>>,
    idle: Arc<IdleDetector>,
    coordinator: SelectionCoordinator,
}

fn handles() -> Handles {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(AutoCloseBackend::default());
    let shutdown = crate::shutdown::ShutdownSignal::new();
    Handles {
        settings: crate::config::settings::new_settings_state(Default::default()),
        coordinator: SelectionCoordinator::new(Arc::clone(&manager), shutdown.token().clone()),
        manager,
        pty: Arc::new(Mutex::new(PtyManager::new_for_test(backend))),
        idle: IdleDetector::new(|_| {}, |_| {}),
    }
}

/// A mock app managing every required handle except the one at `skip`.
fn app_without(h: &Handles, skip: usize) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    manage_index(&app, h, (0..5).filter(|i| *i != skip));
    app
}

fn manage_index(
    app: &impl Manager<tauri::test::MockRuntime>,
    h: &Handles,
    which: impl Iterator<Item = usize>,
) {
    for i in which {
        match i {
            0 => app.manage(h.settings.clone()),
            1 => app.manage(Arc::clone(&h.manager)),
            2 => app.manage(Arc::clone(&h.pty)),
            3 => app.manage(Arc::clone(&h.idle)),
            _ => app.manage(h.coordinator.clone()),
        };
    }
}

/// All five returned handles are the managed ones. The coordinator has no identity
/// accessor, so its shared shutdown token stands in: cancelling the returned
/// coordinator's token is seen by the managed one and not by an unrelated decoy.
fn assert_same_handles(inputs: AutoCloseInputs, h: &Handles) {
    assert!(Arc::ptr_eq(&inputs.settings, &h.settings));
    assert!(Arc::ptr_eq(&inputs.session_manager, &h.manager));
    assert!(Arc::ptr_eq(&inputs.pty_manager, &h.pty));
    assert!(Arc::ptr_eq(&inputs.idle_detector, &h.idle));
    let decoy = handles().coordinator;
    assert!(!h.coordinator.shutdown_token().is_cancelled());
    inputs.selection_coordinator.shutdown_token().cancel();
    assert!(h.coordinator.shutdown_token().is_cancelled());
    assert!(!decoy.shutdown_token().is_cancelled());
}

#[test]
fn tauri_inputs_name_each_missing_type_and_return_the_managed_handles() {
    let names = [
        std::any::type_name::<SettingsState>(),
        std::any::type_name::<Arc<tokio::sync::RwLock<SessionManager>>>(),
        std::any::type_name::<Arc<Mutex<PtyManager>>>(),
        std::any::type_name::<Arc<IdleDetector>>(),
        std::any::type_name::<SelectionCoordinator>(),
    ];
    for (skip, name) in names.iter().enumerate() {
        for clock_present in [false, true] {
            let h = handles();
            let app = app_without(&h, skip);
            let handle = app.handle();
            let clocks: CoordinatorClocksState = Arc::new(Mutex::new(CoordinatorClocks::default()));
            if clock_present {
                app.manage(clocks.clone());
            }
            assert_eq!(AutoCloseServices::inputs(handle).err(), Some(*name));
            manage_index(&app, &h, std::iter::once(skip));
            let inputs = AutoCloseServices::inputs(handle).expect("all five managed");
            match handle.coordinator_clocks() {
                Some(managed) => assert!(clock_present && Arc::ptr_eq(&clocks, &managed)),
                None => assert!(!clock_present),
            }
            assert_same_handles(inputs, &h);
        }
    }
}

static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct AutoCloseLog;

impl log::Log for AutoCloseLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target() == "auto-close"
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) && record.level() == log::Level::Warn {
            WARNINGS.lock().unwrap().push(record.args().to_string());
        }
    }
    fn flush(&self) {}
}

/// Delegates the five handles to a real mock `AppHandle`; badges and saves go to the
/// shared `RecordingServices` spy, and clock queries are counted.
struct Delegated {
    app: tauri::AppHandle<tauri::test::MockRuntime>,
    rec: RecordingServices,
    clock_queries: AtomicUsize,
}

impl AutoCloseServices for Arc<Delegated> {
    fn inputs(&self) -> Result<AutoCloseInputs, &'static str> {
        AutoCloseServices::inputs(&self.app)
    }
    fn coordinator_clocks(&self) -> Option<CoordinatorClocksState> {
        self.clock_queries.fetch_add(1, Ordering::SeqCst);
        self.rec.coordinator_clocks()
    }
    fn idle_badge(&self, cwd: &str, at: DateTime<Utc>) {
        self.rec.idle_badge(cwd, at);
    }
    fn auto_closed_badge(&self, cwd: &str, at: DateTime<Utc>) {
        self.rec.auto_closed_badge(cwd, at);
    }
    fn save_clocks(&self, map: &HashMap<String, ClockEntry>) -> Result<(), String> {
        self.rec.save_clocks(map)
    }
}

impl Delegated {
    fn counts(&self) -> (usize, usize, usize, usize) {
        (
            WARNINGS.lock().unwrap().len(),
            self.clock_queries.load(Ordering::SeqCst),
            self.rec.notices.lock().unwrap().len(),
            self.rec.saved.lock().unwrap().len(),
        )
    }
}

async fn settle_until(spy: &Delegated, done: impl Fn((usize, usize, usize, usize)) -> bool) {
    for _ in 0..10_000 {
        if done(spy.counts()) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("no observation: {:?}", spy.counts());
}

async fn yields(n: usize) {
    for _ in 0..n {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn run_skips_a_tick_with_missing_state_and_recovers_on_the_same_task() {
    use crate::pty::backend::{PtyBackend, SessionBackendKind};
    // One process-wide logger: the body runs in a child of this exact test.
    if !autoclose_child_mode() {
        return run_autoclose_child();
    }
    log::set_logger(&AutoCloseLog).expect("child installs the auto-close logger");
    log::set_max_level(log::LevelFilter::Warn);

    // The child's fixtures live in the parent's private root, retained with its logs.
    let config = std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("child config dir");
    let case_root = Path::new(&config)
        .parent()
        .expect("private root")
        .to_path_buf();
    // A plain directory, not a TempDir: a child panic must not delete it on unwind; the
    // parent removes the whole root only after accepting the receipt.
    let fixture = case_root.join("fixture");
    std::fs::create_dir(&fixture).unwrap();
    let team = fixture.join("myproj/.ac/wg-1-team");
    let (lead_cwd, member_cwd) = (team.join("__agent_lead"), team.join("__agent_rust"));
    let h = handles();
    let lead = auto_close_session(&h.manager, &lead_cwd, None, true).await;
    let member = auto_close_session(&h.manager, &member_cwd, Some("codex".into()), false).await;
    let backend = Arc::new(AutoCloseBackend::default());
    *h.pty.lock().unwrap() = PtyManager::new_for_test(backend.clone());
    for id in [lead, member] {
        backend.set_live(id);
        h.pty
            .lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
    }
    // Disabled: badges only. Members are not tracked by the detector, so no activity
    // advance moves the anchor between ticks.
    h.settings.write().await.coordinator_auto_close_enabled = false;
    let anchor = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let mut seeded = CoordinatorClocks::default();
    assert!(seeded.note_user_message(LEAD, anchor));
    let before = seeded.snapshot();
    let app = app_without(&h, 4);
    let spy = Arc::new(Delegated {
        app: app.handle().clone(),
        rec: RecordingServices::new(
            h.settings.clone(),
            (
                Arc::clone(&h.manager),
                Arc::clone(&h.pty),
                Arc::clone(&h.idle),
            ),
            h.coordinator.clone(),
            Arc::new(Mutex::new(seeded)),
        ),
        clock_queries: AtomicUsize::new(0),
    });
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let task = tokio::spawn(AutoClose::run(Arc::clone(&spy), shutdown.clone()));
    yields(50).await;

    tokio::time::advance(TICK).await;
    settle_until(&spy, |(warnings, ..)| warnings == 1).await;
    yields(50).await;
    assert_eq!(spy.counts(), (1, 0, 0, 0));
    let warning = WARNINGS.lock().unwrap()[0].clone();
    assert!(
        warning.contains(std::any::type_name::<SelectionCoordinator>()),
        "{warning}"
    );
    assert!(warning.contains("skipping tick"), "{warning}");
    assert_eq!(spy.rec.clocks.lock().unwrap().snapshot(), before);
    assert_eq!(h.manager.read().await.list_sessions().await.len(), 2);
    assert!(backend.has_session(lead) && backend.has_session(member));

    manage_index(&spy.app, &h, std::iter::once(4));
    tokio::time::advance(TICK).await;
    settle_until(&spy, |(.., saves)| saves == 1).await;
    yields(50).await;
    let cwd = lead_cwd.to_string_lossy().into_owned();
    assert_eq!(
        *spy.rec.notices.lock().unwrap(),
        vec![Badge::Idle(cwd, anchor)]
    );
    assert_eq!(spy.rec.saved.lock().unwrap()[0], before);
    assert_eq!(WARNINGS.lock().unwrap().len(), 1);

    let queries = spy.counts().1;
    tokio::time::advance(TICK).await;
    settle_until(&spy, |(_, q, ..)| q > queries).await;
    yields(50).await;
    assert_eq!(spy.counts().2, 1, "same anchor: no repeated badge");
    assert_eq!(spy.counts().3, 1, "clean map: no second save");

    shutdown.token().cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("run stops on cancellation")
        .unwrap();
    let after = spy.counts();
    tokio::time::advance(TICK * 3).await;
    yields(50).await;
    assert_eq!(spy.counts(), after);
    assert_eq!(h.manager.read().await.list_sessions().await.len(), 2);
    println!("{AUTOCLOSE_DONE}");
}
