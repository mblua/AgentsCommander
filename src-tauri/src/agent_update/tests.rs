use super::*;
use crate::config::coding_agents_catalog::CodingAgentDefinition;
use crate::web::broadcast::WsBroadcaster;
use tauri::{AppHandle, Manager};

fn entry(command: &str, label: &str, update_commands: Vec<&str>) -> CodingAgentDefinition {
    CodingAgentDefinition {
        key: command.to_string(),
        label: label.to_string(),
        description: String::new(),
        color: String::new(),
        command: command.to_string(),
        instructions_filename: None,
        envs: Vec::new(),
        isolated_home: false,
        config_seed: None,
        removable: true,
        update_commands: update_commands.into_iter().map(str::to_string).collect(),
        auto_update: false,
        idle_burst: None,
        install_commands: None,
    }
}

fn cwd() -> PathBuf {
    PathBuf::from("/tmp/proj")
}

// ---------------------------------------------------------------------
// #1551 - shared helpers for the new groups below.
// ---------------------------------------------------------------------

use crate::agent_version::{CacheLookup, InstallStatus};
use crate::config::settings::{AgentConfig, AppSettings};
use crate::web::broadcast::WsOutMsg;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::task::{Context, Wake, Waker};

const POLL_STEP: Duration = Duration::from_millis(50);
const POLL_CAP: Duration = Duration::from_secs(5);
const QUIET_WINDOW: Duration = Duration::from_millis(300);

fn command_ref(command: &str, label: &str) -> AgentUpdateCommandRef {
    AgentUpdateCommandRef {
        command: command.to_string(),
        label: label.to_string(),
    }
}

fn node(command: &str, label: &str, update_commands: Vec<&str>) -> AgentUpdateNode {
    AgentUpdateNode {
        command: command.to_string(),
        label: label.to_string(),
        update_commands: update_commands.into_iter().map(str::to_string).collect(),
        install_before: None,
    }
}

fn ok_result(command: &str, label: &str) -> AgentUpdateResult {
    AgentUpdateResult {
        command: command.to_string(),
        label: label.to_string(),
        ok: true,
        outcome: AgentUpdateOutcome::Succeeded,
        error: None,
        install_before: None,
        install_after: None,
        change: AgentUpdateChange::Unknown,
        verification_error: None,
    }
}

fn build_mock_app(builder: tauri::Builder<tauri::Wry>) -> tauri::App {
    builder
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app")
}

#[test]
fn agent_update_desktop_fallback_preserves_payload() {
    use std::sync::Mutex;
    use tauri::Listener;

    let app = build_mock_app(crate::test_support::test_builder());
    assert!(app.try_state::<WsBroadcaster>().is_none());
    let desktop = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&desktop);
    let event_name = "agent_update_command_finished";
    app.listen_any(event_name, move |event| {
        let payload = serde_json::from_str::<Value>(event.payload()).expect("desktop JSON");
        observed
            .lock()
            .unwrap()
            .push((event_name.to_string(), payload));
    });
    let payload = json!({ "command": "agent-ñ", "ok": false, "error": null });

    emit_all(app.handle(), event_name, payload.clone());

    assert_eq!(
        *desktop.lock().unwrap(),
        vec![(event_name.to_string(), payload)]
    );
}

#[test]
fn agent_update_tasks_spawn_without_ambient_runtime() {
    let app = build_mock_app(crate::test_support::test_builder());
    let (tx, rx) = std::sync::mpsc::channel();
    assert!(tokio::runtime::Handle::try_current().is_err());
    AgentUpdateTasks::spawn_update_task(
        app.handle(),
        Box::pin(async move {
            tx.send(()).ok();
        }),
    );
    rx.recv_timeout(Duration::from_secs(5))
        .expect("edge task executes without an ambient runtime");
}

#[derive(Clone, Default)]
struct RecordingUpdateEvents {
    frames: Arc<std::sync::Mutex<Vec<Value>>>,
    fences: Option<(Arc<target::AsyncFence>, Arc<target::AsyncFence>)>,
}

impl ManagedBroadcastEvents for RecordingUpdateEvents {
    fn managed_event(&self, event: &str, payload: &Value) -> bool {
        if let Some((response, terminal)) = &self.fences {
            assert!(
                !terminal.is_open(),
                "terminal fence must follow event enqueue"
            );
            if event != "agent_update_command_finished" {
                assert!(
                    !response.is_open(),
                    "response fence must follow cancellation events"
                );
            }
        }
        self.frames
            .lock()
            .unwrap()
            .push(json!({ "event": event, "payload": payload }));
        true
    }
}

struct RecordingAnswerRelease {
    events: RecordingUpdateEvents,
    saw_closure: AtomicBool,
}

impl Wake for RecordingAnswerRelease {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let expected = json!({
            "event": "agent_update_prompt_closed",
            "payload": { "command": "claude", "label": "Claude" }
        });
        self.saw_closure.store(
            self.events.frames.lock().unwrap().as_slice() == [expected],
            Ordering::SeqCst,
        );
    }
}

#[tokio::test]
async fn agent_update_answer_port_enqueues_before_delivery() {
    let events = RecordingUpdateEvents::default();
    let gate = AgentUpdateGate::new();
    let mut rx = gate.register_prompt("claude", "Claude");
    let release = Arc::new(RecordingAnswerRelease {
        events: events.clone(),
        saw_closure: AtomicBool::new(false),
    });
    let waker = Waker::from(Arc::clone(&release));
    let mut cx = Context::from_waker(&waker);
    assert!(Pin::new(&mut rx).poll(&mut cx).is_pending());

    assert!(
        answer_prompt(&events, &gate, "claude", true, || async { Ok(()) })
            .await
            .expect("answer through event capability")
    );
    assert!(release.saw_closure.load(Ordering::SeqCst));
    assert!(rx.await.expect("answer delivered"));
    assert_eq!(
        *events.frames.lock().unwrap(),
        vec![json!({
            "event": "agent_update_prompt_closed",
            "payload": { "command": "claude", "label": "Claude" }
        })]
    );
}

#[tokio::test]
async fn agent_update_cancel_port_preserves_events_and_fences() {
    let gate = Arc::new(AgentUpdateGate::new());
    seed_retained_target(&gate, "claude", "Claude");
    let _rx = gate.register_prompt("claude", "Claude");
    let (response, terminal) = {
        let state = gate.state.lock().unwrap();
        let control = state.controls.get("claude").expect("retained control");
        (
            Arc::clone(&control.response_release),
            Arc::clone(&control.terminal_publication),
        )
    };
    let events = RecordingUpdateEvents {
        fences: Some((Arc::clone(&response), Arc::clone(&terminal))),
        ..RecordingUpdateEvents::default()
    };

    let result = cancel_update(&events, &gate, "claude".to_string()).await;
    assert_eq!(result.disposition, AgentUpdateCancelDisposition::Requested);
    assert!(response.is_open());
    assert!(!terminal.is_open());
    let expected = vec![
        json!({
            "event": "agent_update_prompt_closed",
            "payload": { "command": "claude", "label": "Claude" }
        }),
        json!({
            "event": "agent_update_cancellation_changed",
            "payload": {
                "cancelRequested": [{ "command": "claude", "label": "Claude" }],
                "cancelAllRequested": false
            }
        }),
    ];
    assert_eq!(*events.frames.lock().unwrap(), expected);
    let repeated = cancel_update(&events, &gate, "claude".to_string()).await;
    assert_eq!(
        repeated.disposition,
        AgentUpdateCancelDisposition::AlreadyRequested
    );
    assert_eq!(*events.frames.lock().unwrap(), expected);

    let target = UpdateTarget {
        command: "claude".to_string(),
        label: "Claude".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: cwd(),
    };
    let terminalizer = TargetTerminalizer::new(events.clone(), Arc::clone(&gate), target.clone());
    let result = terminalizer
        .publish(cancelled_update_result(&target, None))
        .await;
    assert!(terminal.is_open());
    let frames = events.frames.lock().unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(
        frames[2],
        json!({ "event": "agent_update_command_finished", "payload": result })
    );
}

fn app_with_broadcaster() -> (tauri::App, tokio::sync::mpsc::Receiver<WsOutMsg>) {
    let broadcaster = WsBroadcaster::new();
    let rx = broadcaster.subscribe();
    let app = build_mock_app(crate::test_support::test_builder().manage(broadcaster));
    (app, rx)
}

fn app_with_cache() -> (
    tauri::App,
    Arc<AgentInstallCache>,
    tokio::sync::mpsc::Receiver<WsOutMsg>,
) {
    let broadcaster = WsBroadcaster::new();
    let rx = broadcaster.subscribe();
    let cache = Arc::new(AgentInstallCache::new());
    let app = build_mock_app(
        crate::test_support::test_builder()
            .manage(broadcaster)
            .manage(Arc::clone(&cache)),
    );
    (app, cache, rx)
}

fn decode_frame(frame: WsOutMsg) -> Value {
    match frame {
        WsOutMsg::Text(text) => serde_json::from_str(&text).expect("json frame"),
        WsOutMsg::Binary(_) => panic!("unexpected binary frame"),
    }
}

fn drain_frames(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) -> Vec<Value> {
    let mut frames = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        frames.push(decode_frame(frame));
    }
    frames
}

/// Bounded real-time poll: `tauri::async_runtime::spawn` runs on Tauri's own
/// runtime, so paused-time helpers cannot drive it.
async fn next_frame(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) -> Value {
    let started = Instant::now();
    loop {
        if let Ok(frame) = rx.try_recv() {
            return decode_frame(frame);
        }
        assert!(
            started.elapsed() < POLL_CAP,
            "no frame arrived within the bound"
        );
        tokio::time::sleep(POLL_STEP).await;
    }
}

async fn assert_no_frame(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) {
    tokio::time::sleep(QUIET_WINDOW).await;
    let leftover = drain_frames(rx);
    assert!(leftover.is_empty(), "unexpected frames: {leftover:?}");
}

/// The exact statement the prompt loop's expiry arm runs (plan 5.4 step 3).
async fn run_expiry_arm(
    app: &AppHandle,
    gate: &AgentUpdateGate,
    command: &str,
) -> Option<AgentUpdateCommandRef> {
    let _serial = gate.answer_serial.lock().await;
    let closed = gate.drop_pending(command);
    if let Some(closed) = closed.as_ref() {
        emit_all(app, "agent_update_prompt_closed", json!(closed));
    }
    closed
}

fn counting_probe(calls: Arc<AtomicUsize>, park: Option<Arc<tokio::sync::Notify>>) -> ProbeFn {
    Arc::new(move |_command| {
        let calls = Arc::clone(&calls);
        let park = park.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if let Some(park) = park {
                park.notified().await;
            }
            InstallState::missing("stub".to_string())
        })
    })
}

/// #1967 - a probe that must never run: it records the invocation and then
/// panics, so a scheduling regression is both visible (the counter) and loud.
fn forbidden_probe(calls: Arc<AtomicUsize>) -> ProbeFn {
    Arc::new(move |command: String| {
        let calls = Arc::clone(&calls);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            panic!("install probe must not run for this case ({command})");
        })
    })
}

/// `bob` (a bare token that resolves nowhere, with an update sequence) and
/// `nob` (no sequence), so no embedded vendor CLI is ever probed.
fn bob_catalog_dir(root: &Path) {
    let catalog_dir = root.join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
    let manifest = json!({
        "schemaVersion": 1,
        "agents": [
            {
                "key": "bob",
                "label": "Bob",
                "description": "",
                "color": "#000000",
                "command": "bob-1551-missing",
                "updateCommands": ["bob up"]
            },
            {
                "key": "nob",
                "label": "Nob",
                "description": "",
                "color": "#000000",
                "command": "nob-1551-missing",
                "updateCommands": []
            }
        ]
    });
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("write catalog");
}

fn bob_settings(root: &Path) -> SettingsState {
    crate::config::settings::new_settings_state(AppSettings {
        project_paths: vec![root.to_string_lossy().to_string()],
        ..AppSettings::default()
    })
}

fn installed(version: &str) -> InstallState {
    InstallState::installed(version.to_string(), Path::new("/test/agent"))
}

fn seed_retained_target(gate: &AgentUpdateGate, command: &str, label: &str) {
    gate.mark_started_with_nodes(vec![node(command, label, vec!["exit 0"])]);
}

fn set_target_phase(gate: &AgentUpdateGate, command: &str, phase: TargetPhase) {
    gate.state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .controls
        .get_mut(command)
        .expect("retained target")
        .phase = phase;
}

mod admission;
mod agent_install;
#[cfg(unix)]
mod agent_path;
mod answer_flow;
mod build_plan;
mod events;
mod gate;
mod pass_end;
mod probe_scheduling;
mod runner;
mod target_panic;
mod terminal_race;
mod update_cancellation;
