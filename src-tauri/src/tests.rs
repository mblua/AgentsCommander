use super::*;

use super::{
    apply_main_display_state, centered_default_main_geometry, effective_main_display_state,
    is_visible_on_monitors, normalize_persisted_active_flags, physical_to_logical,
    prepare_app_outbox, resolve_is_coord_for_restore, restore_session_should_become_active,
    restore_session_should_wake, should_auto_create_root_agent_on_first_restore,
    should_wake_on_restore, should_wake_root_agent_on_restore,
    should_wake_working_agent_on_restore, skip_auto_resume_for_restore, ApiServerHandle,
    ApiServerTask, ContextPatternSource, ContextSample, ContextSampleSink,
    PersistedActiveFlagNormalization, RestoreObserverStartBarrier, ScraperPatterns, ScraperSamples,
    SettingsState, StartupError, StartupErrorKind, WebServerHandle, WebServerLifecycle,
    WebServerLifecycleSnapshot, WebServerStopWaiter, WEB_SERVER_START_CANCELLED,
};
use crate::config::sessions_persistence::PersistedSession;
use crate::config::settings::{AgentConfig, AppSettings, MainWindowDisplayState, WindowGeometry};
use crate::testability::window_placement::TestWindowPlacement;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

mod co_managed;
mod lifecycle;
mod quota_sources;
mod restore;
mod startup;
mod watcher_sink;
mod web_server;
mod window;

/// #2756 U2a: capture without key or catalog, and recovery of the retained
/// candidate once the room is `Ready`.
///
/// Controlled window (plan 9.1), declared once for every test that counts
/// cycles: no supervisor loop runs, no `IdleEdge` and no new offer happen
/// between the not-ready discard and the measured pass except the ones the
/// test itself makes, no trigger is processed between the mount and the
/// pass, and every pass or notification is closed by
/// `u2a_drain_and_handle` before cycles are counted. Test 10 is the single
/// declared exception: it calls `watch_capture_slots` and handles the
/// discovery `SlotChanged` itself, and it mounts no mark.
mod u2a_tests;

/// #2296 quit-gate unit tests.
///
/// Time is injected through `TestClock`, so every deadline assertion is exact
/// and the suite never sleeps on wall-clock time. Events and process exit are
/// injected through `TestHost`.
mod quit_gate;

/// #2456 (epic #2442 p5): the Room reader-demand re-raise tick.
///
/// Every test calls `reraise_room_reader_demands` directly, N times: the
/// supervisor's own timer runs on `tauri::async_runtime`, which defeats a
/// paused clock. Log evidence comes from snapshots of the process-global tee,
/// never a drain, filtered by this test's own session ids.
mod reader_reraise;

use super::{
    abstain_reason_label, co_managed_candidate_label, co_managed_state_reason,
    emit_session_idle_edge, resolve_co_managed_route, route_co_managed_wake, utf8_safe_excerpt,
    CoManagedSupervisorHandle, CoManagedTestHooks, CoManagedTrigger,
};
use crate::capture::record::{CaptureProvider, CapturedRecord, RecordOrigin};
use crate::capture::registry::CaptureRegistry;
use crate::capture::sink::CaptureSlot;
use crate::config::co_managed::CoManagedState;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::session::SessionStatus;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use tauri::Listener;

struct CoManagedFixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    room_root: PathBuf,
    coordinator_cwd: PathBuf,
    dev_cwd: PathBuf,
    projects_dir: PathBuf,
}

fn make_co_managed_fixture() -> CoManagedFixture {
    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(temp.path()).expect("canonicalize fixture temp"),
    );
    make_co_managed_fixture_in(temp, root)
}

/// A room under an arbitrarily deep `root`, for the pointer-budget test.
fn make_co_managed_fixture_in(temp: tempfile::TempDir, root: PathBuf) -> CoManagedFixture {
    let project = root.join("proj-a");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let origin_tech_lead = ac_root.join("_agent_tech-lead");
    let origin_dev_rust = ac_root.join("_agent_dev-rust");
    let room_root = ac_root.join("room-1-dev-team");
    let coordinator_cwd = room_root.join("__agent_tech-lead");
    let dev_cwd = room_root.join("__agent_dev-rust");
    for dir in [
        &team_dir,
        &origin_tech_lead,
        &origin_dev_rust,
        &coordinator_cwd,
        &dev_cwd,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],
            "coordinator":"../_agent_tech-lead"}"#,
    )
    .unwrap();
    std::fs::write(
        coordinator_cwd.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .unwrap();
    std::fs::write(
        dev_cwd.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();
    let projects_dir = root.join("claude-projects");
    std::fs::create_dir_all(&projects_dir).unwrap();
    CoManagedFixture {
        _temp: temp,
        project,
        room_root,
        coordinator_cwd,
        dev_cwd,
        projects_dir,
    }
}

fn write_co_managed_room_config(room_root: &Path, enabled: bool, catalog: Option<&str>) {
    let dir = crate::config::co_managed::co_managed_dir(room_root);
    std::fs::create_dir_all(&dir).unwrap();
    let catalog_value = match catalog {
        Some(name) => serde_json::Value::String(name.to_string()),
        None => serde_json::Value::Null,
    };
    let json = serde_json::json!({ "enabled": enabled, "catalogPath": catalog_value });
    std::fs::write(
        crate::config::co_managed::config_path(room_root),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

fn write_co_managed_catalog(room_root: &Path) {
    let catalog = r#"{"categories":{
        "to-peer":{"destination":"orchestrator","peer":"proj-a:room-1-dev-team/dev-rust",
            "question":"route to the peer?"},
        "to-user":{"destination":"user","question":"route to the user?"},
        "to-root":{"destination":"root","question":"route to the root?"},
        "to-reply":{"destination":"default_reply","reply":"Acknowledged by the Co-managed room.",
            "question":"send the default reply?"}
    }}"#;
    std::fs::write(room_root.join("catalog.json"), catalog).unwrap();
}

fn co_managed_app(
    fixture: &CoManagedFixture,
    endpoint: String,
    enabled: bool,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Arc<CaptureRegistry>,
) {
    co_managed_app_with_project_paths(
        fixture,
        endpoint,
        enabled,
        vec![fixture.project.to_string_lossy().to_string()],
    )
}

/// F4: the settings slice is a parameter so a test can omit the project the
/// room lives in; the supervisor must still derive it from the room root.
fn co_managed_app_with_project_paths(
    fixture: &CoManagedFixture,
    endpoint: String,
    enabled: bool,
    project_paths: Vec<String>,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Arc<CaptureRegistry>,
) {
    write_co_managed_room_config(&fixture.room_root, enabled, Some("catalog.json"));
    write_co_managed_catalog(&fixture.room_root);
    let settings = AppSettings {
        project_paths,
        jev_api_key: "test-key".to_string(),
        jev_model: "jev-1.13.0".to_string(),
        jev_endpoint: endpoint,
        jev_timeout_secs: 5,
        jev_threshold: 0.70,
        jev_margin: 0.15,
        co_managed_enabled: true,
        ..AppSettings::default()
    };
    let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let registry = Arc::new(CaptureRegistry::new());
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let app = tauri::test::mock_builder()
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(session_manager.clone())
        .manage(crate::network::OutboundNetwork::new().expect("shared outbound network"))
        .manage(registry.clone())
        .manage(crate::pty::input_activity::new_state())
        .manage(idle_detector)
        .manage(Arc::new(crate::session::purge_guard::PurgeGuard::default()))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build Co-managed test app");
    (app, session_manager, registry)
}

async fn add_claude_session(
    session_manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    cwd: &Path,
    status: SessionStatus,
    projects_dir: &Path,
) -> uuid::Uuid {
    let guard = session_manager.read().await;
    let session = guard
        .create_session(
            "claude".to_string(),
            Vec::new(),
            cwd.to_string_lossy().to_string(),
            Some("claude".to_string()),
            Some("claude".to_string()),
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    guard
        .set_agent_kind(session.id, Some(CodingAgentKind::Claude))
        .await;
    guard
        .set_resolved_claude_projects_dir(session.id, Some(projects_dir.to_path_buf()))
        .await;
    if matches!(status, SessionStatus::Idle) {
        guard.mark_idle(session.id).await;
    }
    session.id
}

fn co_managed_candidate(session_id: &str, text: &str) -> Arc<CapturedRecord> {
    let text_sha256: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()).into();
    let path = PathBuf::from(format!("/tmp/#2270/{session_id}.jsonl"));
    Arc::new(CapturedRecord {
        session_id: session_id.to_string(),
        text: text.to_string(),
        file: path.clone(),
        epoch: 0,
        record_start: Some(0),
        reader_seq: 0,
        text_sha256,
        turn_id: None,
        provider: CaptureProvider::Claude,
        provider_final: false,
        turn_identified: false,
        origin: RecordOrigin::Live,
        observed_path: path,
        observed_len: text.len() as u64,
        observed_prefix: Vec::new(),
    })
}

fn install_candidate(
    registry: &Arc<CaptureRegistry>,
    session_id: uuid::Uuid,
    text: &str,
) -> (CaptureSlot, Arc<CapturedRecord>) {
    let (capture, _rx) = registry.open(&session_id.to_string());
    let record = co_managed_candidate(&session_id.to_string(), text);
    capture.slot.offer(record.clone(), 0);
    (capture.slot, record)
}

fn capture_events(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> Arc<Mutex<Vec<(String, serde_json::Value)>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    for name in ["session_idle", "session_comanaged_state"] {
        let events = Arc::clone(&events);
        app.listen_any(name, move |event| {
            let payload = serde_json::from_str::<serde_json::Value>(event.payload())
                .unwrap_or(serde_json::Value::Null);
            events.lock().unwrap().push((name.to_string(), payload));
        });
    }
    events
}

fn queue_files(room_root: &Path) -> Vec<PathBuf> {
    let dir = crate::config::co_managed::queue_dir(room_root);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn messaging_files(room_root: &Path) -> Vec<PathBuf> {
    let dir = room_root.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

async fn session_communication(
    session_manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    session_id: uuid::Uuid,
) -> Option<crate::session::session::SessionCommunication> {
    let guard = session_manager.read().await;
    guard.get_session(session_id).await?.communication
}

/// Build the raw HTTP response body Jev expects: every requested category
/// with its score. `decide` requires all ids present and no unknowns.
fn jev_response_body(scores: &[(&str, f32)]) -> String {
    let results: Vec<serde_json::Value> = scores
        .iter()
        .map(|(id, noul)| serde_json::json!({ "id": id, "noul": noul }))
        .collect();
    serde_json::json!({ "results": results }).to_string()
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(header_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
            let content_length = header
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buf.len() >= header_end + 4 + content_length {
                return;
            }
        }
    }
}

async fn write_http_json(stream: &mut tokio::net::TcpStream, body: &str) {
    use tokio::io::AsyncWriteExt;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: \
            close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn spawn_jev_listener(scores: Vec<(&'static str, f32)>) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            read_http_request(&mut stream).await;
            let body = jev_response_body(&scores);
            write_http_json(&mut stream, &body).await;
        }
    });
    (format!("http://127.0.0.1:{port}/v1/systemone"), hits)
}

/// Test 1: idle edge with the room disabled makes zero Jev calls, writes

fn settings_with_agent() -> AppSettings {
    AppSettings {
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "codex".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Codex".to_string(),
            command: "codex".to_string(),
            color: "#10b981".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        ..AppSettings::default()
    }
}

fn api_test_addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}
