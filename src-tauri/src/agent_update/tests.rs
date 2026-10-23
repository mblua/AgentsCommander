use super::*;
use crate::config::coding_agents_catalog::CodingAgentDefinition;

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
