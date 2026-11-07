//! Capability compatibility and co-managed publication controls.
use super::*;
use crate::co_managed::host::CoManagedEvents;
use crate::co_managed::{
    co_managed_jev_settings, emit_co_managed_state, emit_session_idle_edge,
    surface_co_managed_user_message, CoManagedSupervisorHandle, CoManagedTrigger,
};
use crate::config::settings::new_settings_state;
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;
use tauri::{test::MockRuntime, Listener};
use uuid::Uuid;

fn app() -> tauri::App<MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

fn required_missing<T>(get: impl FnOnce() -> T, name: &str) {
    let panic = catch_unwind(AssertUnwindSafe(get))
        .err()
        .expect("required state panic");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("panic text");
    assert!(
        message.contains("state() called before manage()"),
        "{message}"
    );
    assert!(message.contains(name), "{message}");
}

#[tokio::test]
async fn issue_3950_co_missing() {
    let app = app();
    let host = app.handle();
    assert!(host.sessions_optional().is_none());
    assert!(host.input_optional().is_none());
    assert!(host.registry_optional().is_none());
    surface_co_managed_user_message(host, Uuid::nil(), "missing".into()).await;
    required_missing(|| host.sessions_required(), "SessionManager");
    required_missing(|| host.settings_required(), "SettingsOwner");
    required_missing(|| host.network_required(), "OutboundNetwork");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_missing");
}

fn settings() -> crate::config::settings::AppSettings {
    crate::config::settings::AppSettings {
        jev_api_key: "test-key".into(),
        jev_model: "test-model".into(),
        jev_endpoint: "https://example.invalid/jev".into(),
        jev_timeout_secs: 37,
        jev_threshold: 0.81,
        jev_margin: 0.23,
        ..Default::default()
    }
}

#[tokio::test]
async fn issue_3950_co_six_fields() {
    let app = app();
    assert!(app.manage(new_settings_state(settings())));
    let actual = co_managed_jev_settings(app.handle()).await.unwrap();
    assert_eq!(
        actual,
        crate::capture::jev::JevSettings {
            api_key: "test-key".into(),
            model: "test-model".into(),
            endpoint: "https://example.invalid/jev".into(),
            timeout_secs: 37,
            threshold: 0.81,
            margin: 0.23,
        }
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_six_fields");
}

#[tokio::test]
async fn issue_3950_co_fresh_settings() {
    let app = app();
    let state = new_settings_state(settings());
    assert!(app.manage(state.clone()));
    let before = co_managed_jev_settings(app.handle()).await.unwrap();
    {
        let mut guard = state.write().await;
        guard.jev_api_key = "next-key".into();
        guard.jev_model = "next-model".into();
        guard.jev_endpoint = "https://example.invalid/next".into();
        guard.jev_timeout_secs = 51;
        guard.jev_threshold = 0.93;
        guard.jev_margin = 0.34;
    }
    let after = co_managed_jev_settings(app.handle()).await.unwrap();
    assert_ne!(before, after);
    assert_eq!(
        after,
        crate::capture::jev::JevSettings {
            api_key: "next-key".into(),
            model: "next-model".into(),
            endpoint: "https://example.invalid/next".into(),
            timeout_secs: 51,
            threshold: 0.93,
            margin: 0.34,
        }
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_fresh_settings");
}

#[derive(Debug, PartialEq)]
enum IdlePublication {
    Native(&'static str, Value),
    Notify(CoManagedTrigger),
    WebSocket,
}

struct IdleSink {
    receiver: Mutex<tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>>,
    calls: Mutex<Vec<IdlePublication>>,
    fail: bool,
}

impl CoManagedEvents for IdleSink {
    fn co_managed_event(&self, event: &'static str, value: Value) {
        self.record_notifications();
        let _ = self.native(event, value);
    }
}

impl IdleSink {
    fn record_notifications(&self) {
        let mut receiver = self.receiver.lock().unwrap();
        while let Ok(trigger) = receiver.try_recv() {
            self.calls
                .lock()
                .unwrap()
                .push(IdlePublication::Notify(trigger));
        }
    }

    fn native(&self, event: &'static str, value: Value) -> Result<(), &'static str> {
        self.calls
            .lock()
            .unwrap()
            .push(IdlePublication::Native(event, value));
        if self.fail {
            Err("native failure")
        } else {
            Ok(())
        }
    }

    fn recording(&self) -> Vec<IdlePublication> {
        self.record_notifications();
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

fn ordered_idle_publication(
    recording: &[IdlePublication],
    id: Uuid,
    armed: bool,
) -> Result<(), &'static str> {
    let expected = [
        IdlePublication::Native(
            "session_idle",
            json!({"id": id.to_string(), "comanaged": armed}),
        ),
        IdlePublication::Notify(CoManagedTrigger::IdleEdge(id)),
    ];
    if recording != expected {
        return Err("native idle event must precede one notification without WebSocket");
    }
    Ok(())
}

fn idle_control(armed: bool, fail: bool) {
    let (supervisor, receiver) = CoManagedSupervisorHandle::new();
    let id = Uuid::new_v4();
    if armed {
        supervisor.armed.arm(&id.to_string());
    }
    let sink = IdleSink {
        receiver: Mutex::new(receiver),
        calls: Mutex::new(Vec::new()),
        fail,
    };
    assert_eq!(
        emit_session_idle_edge(&sink, &supervisor.armed, Some(&supervisor), id),
        armed
    );
    assert_eq!(
        ordered_idle_publication(&sink.recording(), id, armed),
        Ok(())
    );
}

fn idle_double(id: Uuid, deliver: impl FnOnce(&IdleSink, &dyn Fn())) -> Vec<IdlePublication> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let sink = IdleSink {
        receiver: Mutex::new(receiver),
        calls: Mutex::new(Vec::new()),
        fail: false,
    };
    let notify = || sender.send(CoManagedTrigger::IdleEdge(id)).unwrap();
    deliver(&sink, &notify);
    sink.recording()
}

#[test]
fn issue_3950_co_idle_order() {
    idle_control(false, false);
    idle_control(true, false);
    let id = Uuid::new_v4();
    let native = |sink: &IdleSink| {
        sink.co_managed_event(
            "session_idle",
            json!({"id": id.to_string(), "comanaged": false}),
        );
    };
    let positive = idle_double(id, |sink, notify| {
        native(sink);
        notify();
    });
    assert_eq!(ordered_idle_publication(&positive, id, false), Ok(()));
    let notify_first = idle_double(id, |sink, notify| {
        notify();
        native(sink);
    });
    let violation = Err("native idle event must precede one notification without WebSocket");
    assert_eq!(
        ordered_idle_publication(&notify_first, id, false),
        violation
    );
    println!(
        "
ISSUE3950_MUTANT_REJECTED case=notify_first"
    );
    let add_ws = idle_double(id, |sink, notify| {
        native(sink);
        sink.calls.lock().unwrap().push(IdlePublication::WebSocket);
        notify();
    });
    assert_eq!(ordered_idle_publication(&add_ws, id, false), violation);
    println!(
        "
ISSUE3950_MUTANT_REJECTED case=add_ws"
    );
    println!(
        "
ISSUE3950_CONTROL_OK case=issue_3950_co_idle_order"
    );
}

#[test]
fn issue_3950_co_idle_error() {
    idle_control(false, true);
    idle_control(true, true);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_idle_error");
}

#[test]
fn issue_3950_co_state_native_only() {
    let app = app();
    let broadcaster = crate::web::broadcast::WsBroadcaster::new();
    let mut ws = broadcaster.subscribe();
    assert!(app.manage(broadcaster));
    let first = tauri::WebviewWindowBuilder::new(
        app.handle(),
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .unwrap();
    let second = tauri::WebviewWindowBuilder::new(
        app.handle(),
        "detached",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    for (label, window) in [("main", first), ("detached", second)] {
        let received = received.clone();
        window.listen("session_comanaged_state", move |event| {
            received.lock().unwrap().push((
                label,
                serde_json::from_str::<Value>(event.payload()).unwrap(),
            ));
        });
    }
    let id = Uuid::new_v4();
    emit_co_managed_state(app.handle(), id, true, Some("Ready"));
    let mut actual = received.lock().unwrap().clone();
    actual.sort_by_key(|(label, _)| *label);
    let expected = json!({"id": id.to_string(), "active": true, "reason": "Ready"});
    assert_eq!(
        actual,
        vec![("detached", expected.clone()), ("main", expected)]
    );
    assert!(ws.try_recv().is_err(), "co-managed event stays native only");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_state_native_only");
}

#[tokio::test]
async fn issue_3950_co_communication() {
    let app = app();
    let manager = Arc::new(RwLock::new(SessionManager::new()));
    let id = manager
        .read()
        .await
        .create_session(
            "sh".into(),
            Vec::new(),
            "C:/work".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
        .id;
    assert!(app.manage(manager));
    let received = Arc::new(Mutex::new(Vec::new()));
    let recording = received.clone();
    app.listen_any("session_communication_changed", move |event| {
        recording
            .lock()
            .unwrap()
            .push(serde_json::from_str::<Value>(event.payload()).unwrap());
    });
    surface_co_managed_user_message(app.handle(), id, "first".into()).await;
    assert_eq!(received.lock().unwrap().len(), 1);
    let initial = received.lock().unwrap()[0].clone();
    assert_eq!(initial["sessionId"], id.to_string());
    assert_eq!(initial["communication"]["message"], "first");
    surface_co_managed_user_message(app.handle(), id, "first".into()).await;
    assert_eq!(*received.lock().unwrap(), vec![initial]);
    surface_co_managed_user_message(app.handle(), id, "second".into()).await;
    assert_eq!(received.lock().unwrap().len(), 2);
    assert_eq!(
        received.lock().unwrap()[1]["communication"]["message"],
        "second"
    );
    surface_co_managed_user_message(app.handle(), Uuid::new_v4(), "unknown".into()).await;
    assert_eq!(received.lock().unwrap().len(), 2);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_communication");
}

use crate::co_managed::route::{
    co_managed_cycle, commit_co_managed_with_retries, gather_co_managed_preconditions,
    spawn_co_managed_supervisor, CoManagedCommit, CoManagedOutcome,
};
use crate::reader_demand::ReaderDemandHost;
use crate::session::selection::task::SelectionTask;
use crate::session::selection::SelectionEvents;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const PRESENCE: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct RouteHost(Arc<RouteInputs>);

struct RouteInputs {
    sessions: Option<Arc<RwLock<SessionManager>>>,
    input: Option<SubstantiveInputState>,
    settings: SettingsState,
    network: OutboundNetwork,
    observations: Mutex<Vec<EffectPreconditions>>,
    outcomes: Mutex<VecDeque<Result<(), AbstainReason>>>,
    calls: Mutex<Vec<&'static str>>,
    ready: Mutex<CoManagedState>,
    mutate_on_retry: bool,
    tasks: RouteTasks,
    work: AtomicUsize,
    work_signal: Mutex<Option<oneshot::Sender<()>>>,
}

struct RouteTasks {
    tokio: bool,
    finished: Mutex<Option<oneshot::Sender<tokio::runtime::Id>>>,
}

struct TokioTask(tokio::task::JoinHandle<()>);

impl std::future::Future for TokioTask {
    type Output = Result<(), String>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0)
            .poll(cx)
            .map(|r| r.map_err(|e| e.to_string()))
    }
}

impl SelectionTask for TokioTask {
    fn abort(&self) {
        self.0.abort();
    }
}

impl SelectionTasks for RouteTasks {
    fn spawn(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SelectionTask>> {
        let finished = self.finished.lock().unwrap().take().unwrap();
        let future = Box::pin(async move {
            let runtime = tokio::runtime::Handle::current().id();
            future.await;
            let _ = finished.send(runtime);
        });
        if self.tokio {
            Box::pin(TokioTask(tokio::spawn(future)))
        } else {
            NATIVE_SELECTION_TASKS.spawn(future)
        }
    }
}

impl CoManagedEvents for RouteHost {
    fn co_managed_event(&self, _: &'static str, _: Value) {
        self.0.calls.lock().unwrap().push("native");
    }
}

impl SelectionEvents for RouteHost {
    fn selection_event(&self, _: &'static str, _: Value) {
        self.0.calls.lock().unwrap().push("surface");
    }
}

impl CoManagedHost for RouteHost {
    fn sessions_optional(&self) -> Option<&Arc<RwLock<SessionManager>>> {
        self.0.calls.lock().unwrap().push("sessions");
        self.0.sessions.as_ref()
    }
    fn sessions_required(&self) -> &Arc<RwLock<SessionManager>> {
        self.0.sessions.as_ref().unwrap()
    }
    fn settings_required(&self) -> &SettingsState {
        &self.0.settings
    }
    fn input_optional(&self) -> Option<&SubstantiveInputState> {
        self.0.calls.lock().unwrap().push("input");
        self.0.input.as_ref()
    }
    fn registry_optional(&self) -> Option<&Arc<CaptureRegistry>> {
        None
    }
    fn network_required(&self) -> &OutboundNetwork {
        self.0.calls.lock().unwrap().push("network");
        &self.0.network
    }
    fn tasks(&self) -> &dyn SelectionTasks {
        &self.0.tasks
    }
    fn effective_state<'a>(
        &'a self,
        _: &'a Path,
        _: &'a str,
    ) -> BoxFuture<'a, Result<CoManagedState, String>> {
        Box::pin(async move {
            self.0.calls.lock().unwrap().push("effective");
            Ok(self.0.ready.lock().unwrap().clone())
        })
    }
    fn commit_effect(
        &self,
        _: PathBuf,
        _: CaptureSlot,
        _: u64,
        key: ConsumptionKey,
        pre: EffectPreconditions,
        _: Arc<CaptureState>,
    ) -> BoxFuture<'static, Result<CommittedEffect, AbstainReason>> {
        self.0.calls.lock().unwrap().push("commit");
        self.0.observations.lock().unwrap().push(pre.clone());
        let outcome = self.0.outcomes.lock().unwrap().pop_front().unwrap();
        if self.0.mutate_on_retry && outcome.is_err() {
            *self.0.ready.lock().unwrap() = CoManagedState::Off {
                reason: crate::config::co_managed::OffReason::RoomFlagOff,
            };
        }
        Box::pin(async move {
            outcome.map(|()| CommittedEffect {
                session_id: pre.session_id,
                key,
                budget_remaining: 2,
                kind: pre.kind,
                routable: true,
            })
        })
    }
}

impl ReaderDemandHost for RouteHost {
    fn reader_settings(&self) -> Option<&SettingsState> {
        None
    }
    fn reader_sessions(&self) -> Option<&Arc<RwLock<SessionManager>>> {
        None
    }
    fn reader_sessions_required(&self) -> &Arc<RwLock<SessionManager>> {
        self.sessions_required()
    }
    fn reader_registry(&self) -> Option<&Arc<CaptureRegistry>> {
        self.0.work.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = self.0.work_signal.lock().unwrap().take() {
            let _ = sender.send(());
        }
        None
    }
    async fn holds_room_reader_demand(&self, _: Uuid) -> bool {
        false
    }
    async fn raise_room_reader_demand(&self, _: Uuid) -> bool {
        false
    }
    async fn release_room_reader_demand(&self, _: Uuid) {}
    async fn co_managed_capturable_for_session(&self, _: &Path, _: &str) -> Result<bool, String> {
        Ok(false)
    }
    async fn co_managed_effective_state_for_session(
        &self,
        room: &Path,
        id: &str,
    ) -> Result<CoManagedState, String> {
        self.effective_state(room, id).await
    }
}

fn route_host(
    sessions: bool,
    input: bool,
    outcomes: Vec<Result<(), AbstainReason>>,
    mutate: bool,
    tokio: bool,
) -> (RouteHost, oneshot::Receiver<tokio::runtime::Id>) {
    let (finished, receiver) = oneshot::channel();
    let host = RouteHost(Arc::new(RouteInputs {
        sessions: sessions.then(|| Arc::new(RwLock::new(SessionManager::new()))),
        input: input.then(crate::pty::input_activity::new_state),
        settings: new_settings_state(Default::default()),
        network: OutboundNetwork::new_for_tests(1),
        observations: Mutex::new(Vec::new()),
        outcomes: Mutex::new(outcomes.into()),
        calls: Mutex::new(Vec::new()),
        ready: Mutex::new(CoManagedState::Ready),
        mutate_on_retry: mutate,
        tasks: RouteTasks {
            tokio,
            finished: Mutex::new(Some(finished)),
        },
        work: AtomicUsize::new(0),
        work_signal: Mutex::new(None),
    }));
    (host, receiver)
}

async fn route_session(host: &RouteHost) -> Uuid {
    host.sessions_required()
        .read()
        .await
        .create_session(
            "sh".into(),
            Vec::new(),
            "C:/work".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
        .id
}

fn route_candidate(id: Uuid, text: &str) -> Arc<crate::capture::record::CapturedRecord> {
    use crate::capture::record::{CaptureProvider, CapturedRecord, RecordOrigin};
    Arc::new(CapturedRecord {
        session_id: id.to_string(),
        text: text.into(),
        file: "candidate.jsonl".into(),
        epoch: 0,
        record_start: Some(0),
        reader_seq: 1,
        text_sha256: [7; 32],
        turn_id: None,
        provider: CaptureProvider::Claude,
        provider_final: false,
        turn_identified: false,
        origin: RecordOrigin::Live,
        observed_path: "candidate.jsonl".into(),
        observed_len: 1,
        observed_prefix: vec![1],
    })
}

async fn route_retry(host: &RouteHost, id: Uuid) -> CoManagedCommit {
    let (handle, _receiver) = CoManagedSupervisorHandle::new();
    let slot = CaptureSlot::new();
    let key = ConsumptionKey::from_record(&route_candidate(id, "candidate"), 0);
    commit_co_managed_with_retries(
        host,
        &handle,
        id,
        Path::new("unused-room"),
        &slot,
        0,
        &key,
        crate::capture::state::EffectKind::Automatic,
    )
    .await
}

#[tokio::test]
async fn issue_3950_co_retry_fresh() {
    let (host, _) = route_host(
        true,
        false,
        vec![Err(AbstainReason::LockBusy), Ok(())],
        true,
        false,
    );
    let id = route_session(&host).await;
    assert!(matches!(
        route_retry(&host, id).await,
        CoManagedCommit::Committed(_)
    ));
    let observations = host.0.observations.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(observations[0].effective_ready);
    assert!(
        !observations[1].effective_ready,
        "retry must observe mutated readiness"
    );
    assert!(observations[1].observed_at > observations[0].observed_at);
    drop(observations);
    let (control, _) = route_host(true, false, vec![Ok(())], false, false);
    assert!(matches!(
        route_retry(&control, id).await,
        CoManagedCommit::Committed(_)
    ));
    assert_eq!(control.0.observations.lock().unwrap().len(), 1);
    assert!(control.0.observations.lock().unwrap()[0].effective_ready);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_retry_fresh");
}

#[tokio::test]
async fn issue_3950_co_early_missing() {
    let (host, _) = route_host(false, true, vec![], false, false);
    let id = Uuid::new_v4();
    let pre = gather_co_managed_preconditions(
        &host,
        id,
        Path::new("unused"),
        crate::capture::state::EffectKind::Automatic,
    )
    .await;
    assert!(!pre.session_alive && !pre.unique_live_session_for_cwd);
    assert!(!pre.no_pending_user_input && !pre.effective_ready);
    assert_eq!(pre.session_id, id.to_string());
    assert_eq!(*host.0.calls.lock().unwrap(), ["sessions"]);
    let (control, _) = route_host(true, true, vec![], false, false);
    gather_co_managed_preconditions(
        &control,
        id,
        Path::new("unused"),
        crate::capture::state::EffectKind::Automatic,
    )
    .await;
    assert_eq!(
        *control.0.calls.lock().unwrap(),
        ["sessions", "input", "effective"]
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_early_missing");
}

#[tokio::test]
async fn issue_3950_co_input_none() {
    for input in [false, true] {
        let (host, _) = route_host(true, input, vec![], false, false);
        let id = route_session(&host).await;
        if let Some(input) = host.input_optional() {
            input.lock().unwrap().feed(id, b"pending text");
        }
        let pre = gather_co_managed_preconditions(
            &host,
            id,
            Path::new("unused"),
            crate::capture::state::EffectKind::Automatic,
        )
        .await;
        assert_eq!(pre.no_pending_user_input, !input);
        assert!(pre.session_alive && pre.unique_live_session_for_cwd && pre.effective_ready);
    }
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_input_none");
}

#[tokio::test]
async fn issue_3950_co_retry_errors() {
    for retryable in [AbstainReason::LockBusy, AbstainReason::PreconditionsStale] {
        let (host, _) = route_host(true, false, vec![Err(retryable); 3], false, false);
        assert!(matches!(
            route_retry(&host, Uuid::nil()).await,
            CoManagedCommit::Contention
        ));
        assert_eq!(host.0.observations.lock().unwrap().len(), 3);
    }
    let (host, _) = route_host(
        true,
        false,
        vec![Err(AbstainReason::PreconditionsRejected), Ok(())],
        false,
        false,
    );
    assert!(matches!(
        route_retry(&host, Uuid::nil()).await,
        CoManagedCommit::Rejected(AbstainReason::PreconditionsRejected)
    ));
    assert_eq!(host.0.observations.lock().unwrap().len(), 1);
    let (control, _) = route_host(true, false, vec![Ok(())], false, false);
    assert!(matches!(
        route_retry(&control, Uuid::nil()).await,
        CoManagedCommit::Committed(_)
    ));
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_retry_errors");
}

fn join_mapping_is_exact(result: Result<u8, AbstainReason>) -> Result<(), &'static str> {
    if result
        != Err(AbstainReason::LockUnavailable(
            "commit task join failed".into(),
        ))
    {
        return Err("join failure must abstain with exact text");
    }
    Ok(())
}

#[tokio::test]
async fn issue_3950_co_join_error() {
    let joined = tauri::async_runtime::spawn_blocking(|| -> Result<u8, AbstainReason> {
        panic!("issue3950 native commit join sentinel")
    })
    .await;
    assert!(
        joined.is_err(),
        "real native panic must produce a join error"
    );
    assert_eq!(join_mapping_is_exact(commit_join_result(joined)), Ok(()));
    let good = tauri::async_runtime::spawn_blocking(|| Ok::<_, AbstainReason>(17)).await;
    assert_eq!(commit_join_result(good), Ok(17));
    let inner =
        tauri::async_runtime::spawn_blocking(|| Err::<u8, _>(AbstainReason::SlotChanged)).await;
    assert_eq!(commit_join_result(inner), Err(AbstainReason::SlotChanged));
    assert_eq!(
        join_mapping_is_exact(Ok(0)),
        Err("join failure must abstain with exact text")
    );
    println!("\nISSUE3950_MUTANT_REJECTED case=accept_join_error");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_join_error");
}

async fn cycle_control(text: &str, result: Result<(), AbstainReason>) -> (RouteHost, PathBuf) {
    let (host, _) = route_host(true, false, vec![result], false, false);
    let id = route_session(&host).await;
    let room = tempfile::tempdir().unwrap();
    let room_root = room.path().join("project/.ac/room-1-dev-team");
    std::fs::create_dir_all(&room_root).unwrap();
    let candidate = route_candidate(id, text);
    let slot = CaptureSlot::new();
    slot.offer(candidate.clone(), 0);
    let (handle, _receiver) = CoManagedSupervisorHandle::new();
    let outcome = co_managed_cycle(
        &host,
        &handle,
        id,
        &room_root,
        &slot,
        slot.seq(),
        &candidate,
    )
    .await;
    assert!(matches!(outcome, CoManagedOutcome::Done(_)));
    let messaging = room_root.join("messaging");
    // Preserve only the observable existence; tempfile owns all cleanup.
    let observed = if messaging.exists() {
        messaging
    } else {
        PathBuf::new()
    };
    (host, observed)
}

#[tokio::test]
async fn issue_3950_co_secret_first() {
    let (secret, written) = cycle_control("-----BEGIN PRIVATE KEY-----", Ok(())).await;
    let calls = secret.0.calls.lock().unwrap();
    assert!(
        !calls.contains(&"network"),
        "secret must bypass classification network access"
    );
    assert_eq!(calls.last(), Some(&"surface"));
    assert!(
        written.as_os_str().is_empty(),
        "secret must not write messaging file"
    );
    drop(calls);
    let (plain, written) = cycle_control("ordinary candidate text", Ok(())).await;
    assert!(plain.0.calls.lock().unwrap().contains(&"network"));
    assert!(
        !written.as_os_str().is_empty(),
        "ordinary candidate writes a message"
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_secret_first");
}

#[tokio::test]
async fn issue_3950_co_reservation() {
    let (accepted, written) = cycle_control("ordinary candidate text", Ok(())).await;
    let calls = accepted.0.calls.lock().unwrap();
    let commit = calls.iter().position(|x| *x == "commit").unwrap();
    let surface = calls.iter().position(|x| *x == "surface").unwrap();
    assert!(commit < surface, "reservation must precede route surface");
    assert!(!written.as_os_str().is_empty());
    drop(calls);
    let (rejected, written) = cycle_control(
        "ordinary candidate text",
        Err(AbstainReason::PreconditionsRejected),
    )
    .await;
    assert!(!rejected.0.calls.lock().unwrap().contains(&"surface"));
    assert!(
        written.as_os_str().is_empty(),
        "rejected reservation must not write a file"
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_reservation");
}

#[tokio::test]
async fn issue_3950_co_cancel_bias() {
    let (cancelled, done) = route_host(false, false, vec![], false, false);
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let (handle, _old) = CoManagedSupervisorHandle::new();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender
        .send(CoManagedTrigger::IdleEdge(Uuid::nil()))
        .unwrap();
    spawn_co_managed_supervisor(cancelled.clone(), handle, receiver, shutdown);
    timeout(PRESENCE, done).await.unwrap().unwrap();
    assert_eq!(cancelled.0.work.load(Ordering::SeqCst), 0);
    let (control, done) = route_host(false, false, vec![], false, false);
    let (work, worked) = oneshot::channel();
    *control.0.work_signal.lock().unwrap() = Some(work);
    let shutdown = CancellationToken::new();
    let (handle, _old) = CoManagedSupervisorHandle::new();
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    spawn_co_managed_supervisor(control.clone(), handle, receiver, shutdown.clone());
    timeout(PRESENCE, worked).await.unwrap().unwrap();
    shutdown.cancel();
    timeout(PRESENCE, done).await.unwrap().unwrap();
    assert!(control.0.work.load(Ordering::SeqCst) > 0);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_cancel_bias");
}

async fn supervisor_runtime(tokio: bool) -> tokio::runtime::Id {
    let (host, done) = route_host(false, false, vec![], false, tokio);
    let (work, worked) = oneshot::channel();
    *host.0.work_signal.lock().unwrap() = Some(work);
    let (handle, _old) = CoManagedSupervisorHandle::new();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    spawn_co_managed_supervisor(host, handle, receiver, CancellationToken::new());
    timeout(PRESENCE, worked).await.unwrap().unwrap();
    // The native task handle and caller host have already been dropped.
    drop(sender);
    timeout(PRESENCE, done).await.unwrap().unwrap()
}

fn native_runtime_is_exact(
    actual: tokio::runtime::Id,
    native: tokio::runtime::Id,
) -> Result<(), &'static str> {
    if actual == native {
        Ok(())
    } else {
        Err("supervisor must use native runtime")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn issue_3950_co_native_detach() {
    let native = tauri::async_runtime::spawn(async { tokio::runtime::Handle::current().id() })
        .await
        .unwrap();
    assert_ne!(native, tokio::runtime::Handle::current().id());
    assert_eq!(
        native_runtime_is_exact(supervisor_runtime(false).await, native),
        Ok(())
    );
    assert_eq!(
        native_runtime_is_exact(supervisor_runtime(true).await, native),
        Err("supervisor must use native runtime")
    );
    println!("\nISSUE3950_MUTANT_REJECTED case=tokio_spawn");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_native_detach");
}
