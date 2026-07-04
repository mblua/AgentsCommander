#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentscommander_lib::commands::pty::{get_screen_snapshot, PtyScreenSnapshotPayload};
use agentscommander_lib::commands::session::{
    create_session_inner, destroy_session_inner, restart_session_inner,
};
use agentscommander_lib::config::sessions_persistence::{
    load_sessions, persist_current_state_result,
};
use agentscommander_lib::config::settings::{
    save_settings_with_project_paths, AppSettings, SettingsState,
};
use agentscommander_lib::pty::git_watcher::GitWatcher;
use agentscommander_lib::pty::idle_detector::IdleDetector;
use agentscommander_lib::pty::manager::PtyManager;
use agentscommander_lib::resource_monitor::ResourceMonitorState;
use agentscommander_lib::session::manager::SessionManager;
use agentscommander_lib::session::session::{SessionInfo, SessionStatus};
use agentscommander_lib::shutdown::ShutdownSignal;
use agentscommander_lib::telegram::manager::{
    OutputSenderMap, TelegramBridgeManager, TelegramBridgeState,
};
use agentscommander_lib::voice::tracker::{VoiceTracker, VoiceTrackingState};
use agentscommander_lib::web::auth::WebAccessToken;
use agentscommander_lib::web::broadcast::WsBroadcaster;
use agentscommander_lib::{
    AppOutbox, DetachedSessionsState, MasterToken, RestoreInProgress, RtkStartupModeState,
    RtkSweepLockState, SpecBoardState, WebServerHandle,
};
use serde::Deserialize;
use tauri::{Listener, Manager};
use uuid::Uuid;

const OUTPUT_TIMEOUT: Duration = Duration::from_secs(10);
const PID_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(75);

type PtyCleanupEntries = Arc<Mutex<Vec<(Arc<Mutex<PtyManager>>, Uuid)>>>;

static TEST_CONFIG_ENV_LOCK: Mutex<()> = Mutex::new(());

fn lifecycle_test_config_dir() -> PathBuf {
    std::env::temp_dir()
        .join(format!(
            "agentscommander-pty-lifecycle-{}",
            std::process::id()
        ))
        .join("config")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestPtyOutputPayload {
    session_id: String,
    data: Vec<u8>,
    sequence: Option<u64>,
}

struct TestConfigEnvGuard {
    previous: Option<String>,
}

impl TestConfigEnvGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").ok();
        std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", path);
        Self { previous }
    }
}

impl Drop for TestConfigEnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.as_ref() {
            std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", previous);
        } else {
            std::env::remove_var("AGENTSCOMMANDER_TEST_CONFIG_DIR");
        }
    }
}

#[derive(Clone)]
struct LifecycleCleanup {
    entries: PtyCleanupEntries,
    pids: Arc<Mutex<Vec<u32>>>,
}

impl LifecycleCleanup {
    fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            pids: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn record_session(&self, pty_mgr: &Arc<Mutex<PtyManager>>, id: Uuid) {
        self.entries.lock().unwrap().push((Arc::clone(pty_mgr), id));
    }

    fn record_pid(&self, pid: u32) {
        let mut pids = self.pids.lock().unwrap();
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
}

impl Drop for LifecycleCleanup {
    fn drop(&mut self) {
        for (pty_mgr, id) in self.entries.lock().unwrap().iter() {
            let _ = pty_mgr.lock().unwrap().kill(*id);
        }
        for pid in self.pids.lock().unwrap().iter().copied() {
            if process_exists(pid) {
                let _ = stop_process(pid);
            }
        }
    }
}

struct LifecycleFixture {
    cleanup: LifecycleCleanup,
    repo_root: PathBuf,
    config_dir: PathBuf,
    session_cwd: PathBuf,
    script_path: PathBuf,
    pid_files: Vec<PathBuf>,
    captured_output: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    captured_sequences: Arc<Mutex<HashMap<String, Vec<u64>>>>,
    listener_errors: Arc<Mutex<Vec<String>>>,
    app: tauri::App,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    _temp: tempfile::TempDir,
    _env_guard: TestConfigEnvGuard,
    _env_lock: std::sync::MutexGuard<'static, ()>,
}

fn make_lifecycle_fixture() -> LifecycleFixture {
    let env_lock = TEST_CONFIG_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let repo_root = temp.path().join("repo-pty-lifecycle");
    let config_dir = lifecycle_test_config_dir();
    let session_cwd = repo_root.clone();
    let script_path = temp.path().join("pty-child.ps1");

    std::fs::create_dir_all(&repo_root).expect("create repo root");
    if config_dir.exists() {
        std::fs::remove_dir_all(&config_dir).expect("reset config dir");
    }
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(repo_root.join(".gitignore"), "*\r\n").expect("seed repo file");

    let env_guard = TestConfigEnvGuard::set(&config_dir);
    assert_eq!(
        agentscommander_lib::config::config_dir().expect("config dir override"),
        config_dir
    );
    // #778: seeding project_paths to a fresh settings.json is a deliberate list
    // write, so it must use the explicit verbatim writer. The default
    // `save_settings` is now preserve-disk (it would read the not-yet-existent
    // file as empty and write project_paths: []), which is exactly the fail-safe
    // behavior #778 introduced.
    save_settings_with_project_paths(&AppSettings {
        default_shell: "powershell.exe".to_string(),
        default_shell_args: vec!["-NoLogo".to_string()],
        project_paths: vec![path_to_string(&repo_root)],
        ..AppSettings::default()
    })
    .expect("seed isolated settings");

    write_child_script(&script_path);
    run_powershell_preflight();

    let captured_output = Arc::new(Mutex::new(HashMap::new()));
    let captured_sequences = Arc::new(Mutex::new(HashMap::new()));
    let listener_errors = Arc::new(Mutex::new(Vec::new()));
    let cleanup = LifecycleCleanup::new();
    let (app, session_mgr, pty_mgr) = make_test_app(
        &repo_root,
        &captured_output,
        &captured_sequences,
        &listener_errors,
        &cleanup,
        None,
        None,
    );

    LifecycleFixture {
        cleanup,
        repo_root,
        config_dir,
        session_cwd,
        script_path,
        pid_files: Vec::new(),
        captured_output,
        captured_sequences,
        listener_errors,
        app,
        session_mgr,
        pty_mgr,
        _temp: temp,
        _env_guard: env_guard,
        _env_lock: env_lock,
    }
}

fn make_test_app(
    repo_root: &Path,
    captured_output: &Arc<Mutex<HashMap<String, Vec<u8>>>>,
    captured_sequences: &Arc<Mutex<HashMap<String, Vec<u64>>>>,
    listener_errors: &Arc<Mutex<Vec<String>>>,
    _cleanup: &LifecycleCleanup,
    session_mgr_override: Option<Arc<tokio::sync::RwLock<SessionManager>>>,
    _pty_mgr_override: Option<Arc<Mutex<PtyManager>>>,
) -> (
    tauri::App,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Arc<Mutex<PtyManager>>,
) {
    let session_mgr = session_mgr_override
        .unwrap_or_else(|| Arc::new(tokio::sync::RwLock::new(SessionManager::new())));
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(Arc::clone(&output_senders)),
    ));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(AppSettings {
        default_shell: "powershell.exe".to_string(),
        default_shell_args: vec!["-NoLogo".to_string()],
        project_paths: vec![path_to_string(repo_root)],
        ..AppSettings::default()
    }));
    let git_app = Box::leak(Box::new(
        tauri::Builder::default()
            .any_thread()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build git watcher handle app"),
    ));
    let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), git_app.handle().clone());
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
        output_senders,
        idle_detector,
        Arc::clone(&git_watcher),
        None,
    )));

    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice_tracking: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));
    let spec_board_state: SpecBoardState = Arc::new(tokio::sync::RwLock::new(
        agentscommander_lib::commands::spec_board::SpecBoardManager::new(),
    ));
    let rtk_sweep_lock: RtkSweepLockState = Arc::new(tokio::sync::Mutex::new(()));
    let rtk_startup_mode: RtkStartupModeState = Arc::new(std::sync::OnceLock::new());
    let shutdown_signal = ShutdownSignal::new();

    let captured = Arc::clone(captured_output);
    let captured_sequences = Arc::clone(captured_sequences);
    let listener_errors = Arc::clone(listener_errors);
    let app = tauri::Builder::default()
        .any_thread()
        .manage(MasterToken::new("pty-lifecycle-master-token".into()))
        .manage(AppOutbox::new(
            repo_root.join(".app-outbox").to_string_lossy().to_string(),
        ))
        .manage(settings)
        .manage(Arc::clone(&session_mgr))
        .manage(tg_mgr)
        .manage(detached_sessions)
        .manage(voice_tracking)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .manage(shutdown_signal)
        .manage(Arc::new(WebAccessToken::new(
            "pty-lifecycle-web-token".into(),
        )))
        .manage(WsBroadcaster::new())
        .manage(WebServerHandle::default())
        .manage(spec_board_state)
        .manage(rtk_sweep_lock)
        .manage(rtk_startup_mode)
        .manage(git_watcher)
        .manage(Arc::new(ResourceMonitorState::new()))
        .manage(Arc::clone(&pty_mgr))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build pty lifecycle test app");

    app.listen_any("pty_output", move |event| {
        let raw = event.payload().to_string();
        match serde_json::from_str::<TestPtyOutputPayload>(&raw) {
            Ok(payload) => {
                captured
                    .lock()
                    .unwrap()
                    .entry(payload.session_id.clone())
                    .or_default()
                    .extend(payload.data);

                match payload.sequence {
                    Some(sequence) => captured_sequences
                        .lock()
                        .unwrap()
                        .entry(payload.session_id)
                        .or_default()
                        .push(sequence),
                    None => listener_errors
                        .lock()
                        .unwrap()
                        .push(format!("pty_output payload missing sequence; raw={raw}")),
                }
            }
            Err(err) => listener_errors.lock().unwrap().push(format!(
                "failed to parse pty_output payload: {err}; raw={raw}"
            )),
        }
    });

    (app, session_mgr, pty_mgr)
}

#[test]
fn pty_screen_snapshot_payload_serializes_camel_case_bytes_and_optional_size() {
    let value = serde_json::to_value(PtyScreenSnapshotPayload {
        session_id: "session-1".to_string(),
        data: vec![27, 91, 72],
        rows: Some(30),
        cols: Some(120),
        sequence: 7,
    })
    .expect("serialize snapshot payload");

    assert_eq!(
        value,
        serde_json::json!({
            "sessionId": "session-1",
            "data": [27, 91, 72],
            "rows": 30,
            "cols": 120,
            "sequence": 7,
        })
    );
    assert!(value.get("session_id").is_none());
}

#[test]
fn real_pty_session_lifecycle_create_io_resize_restart_persist_restore_cleanup() {
    let mut fixture = make_lifecycle_fixture();
    assert!(get_screen_snapshot(
        fixture.app.state::<Arc<Mutex<PtyManager>>>(),
        "not-a-uuid".to_string(),
    )
    .is_err());

    let missing = get_screen_snapshot(
        fixture.app.state::<Arc<Mutex<PtyManager>>>(),
        Uuid::nil().to_string(),
    )
    .expect("missing session lookup should not fail");
    assert!(missing.is_none());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    runtime.block_on(async {
        let pid_a = fixture.pid_path("a");
        let session_a = fixture
            .create_session("PTY lifecycle A", &pid_a)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase A create failed: {e}\n{}",
                    fixture.dump("Phase A", &[], std::slice::from_ref(&pid_a))
                )
            });
        let id_a = parse_session_id(&session_a);
        fixture.cleanup.record_session(&fixture.pty_mgr, id_a);
        assert_has_pty(&fixture.pty_mgr, id_a, "Phase A", &fixture);
        wait_for_output(&fixture, &session_a.id, "AC_READY", OUTPUT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump("Phase A", &[id_a], std::slice::from_ref(&pid_a))
                )
            });
        {
            let last_event_sequence =
                last_strictly_increasing_sequence(&fixture, &session_a.id, "Phase A");
            let snapshot = get_screen_snapshot(
                fixture.app.state::<Arc<Mutex<PtyManager>>>(),
                session_a.id.clone(),
            )
            .expect("snapshot command succeeds")
            .expect("screen snapshot exists after PTY output");
            let snapshot_text = String::from_utf8_lossy(&snapshot.data);
            assert_eq!(snapshot.session_id, session_a.id);
            assert!(snapshot.rows.is_some(), "Phase A snapshot rows missing");
            assert!(snapshot.cols.is_some(), "Phase A snapshot cols missing");
            assert!(
                snapshot.sequence >= last_event_sequence,
                "Phase A snapshot sequence {} should cover last event sequence {}",
                snapshot.sequence,
                last_event_sequence
            );
            assert!(
                snapshot_text.contains("AC_READY"),
                "Phase A snapshot missing AC_READY; snapshot={snapshot_text:?}\n{}",
                fixture.dump("Phase A snapshot", &[id_a], std::slice::from_ref(&pid_a))
            );
        }
        let pid_a_value = wait_for_pid_file(&pid_a, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump("Phase A", &[id_a], std::slice::from_ref(&pid_a))
                )
            });
        fixture.cleanup.record_pid(pid_a_value);
        assert!(
            process_exists(pid_a_value),
            "Phase A process not alive\n{}",
            fixture.dump("Phase A", &[id_a], std::slice::from_ref(&pid_a))
        );

        {
            fixture
                .pty_mgr
                .lock()
                .unwrap()
                .write(id_a, b"AC_ECHO hello-from-pty-lifecycle\r\n")
                .expect("write to pty");
        }
        wait_for_output(
            &fixture,
            &session_a.id,
            "AC_ECHOED hello-from-pty-lifecycle",
            OUTPUT_TIMEOUT,
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{e}\n{}",
                fixture.dump("Phase B", &[id_a], std::slice::from_ref(&pid_a))
            )
        });
        assert_has_pty(&fixture.pty_mgr, id_a, "Phase B", &fixture);
        assert!(process_exists(pid_a_value));

        {
            fixture
                .pty_mgr
                .lock()
                .unwrap()
                .resize(id_a, 80, 24)
                .expect("resize pty");
        }
        let size = { fixture.pty_mgr.lock().unwrap().get_pty_size(id_a) };
        assert_eq!(
            size,
            Some((24, 80)),
            "Phase C resize mismatch\n{}",
            fixture.dump("Phase C", &[id_a], std::slice::from_ref(&pid_a))
        );
        assert_has_pty(&fixture.pty_mgr, id_a, "Phase C", &fixture);
        assert!(process_exists(pid_a_value));

        persist_sessions(&fixture.session_mgr).await;
        assert_persisted_contains(&fixture, "PTY lifecycle A", "Phase D");

        destroy_session_inner(fixture.app.handle(), id_a)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase E destroy failed: {e}\n{}",
                    fixture.dump("Phase E", &[id_a], std::slice::from_ref(&pid_a))
                )
            });
        wait_for_process_exit(pid_a_value, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump("Phase E", &[id_a], std::slice::from_ref(&pid_a))
                )
            });
        assert_no_pty(&fixture.pty_mgr, id_a, "Phase E", &fixture);
        assert_session_missing(&fixture.session_mgr, id_a).await;
        assert_persisted_missing(&fixture, "PTY lifecycle A", "Phase E");

        let pid_restart = fixture.pid_path("restart");
        let restart_session = fixture
            .create_session("PTY lifecycle restart", &pid_restart)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase F create failed: {e}\n{}",
                    fixture.dump("Phase F", &[], std::slice::from_ref(&pid_restart))
                )
            });
        let restart_old_id = parse_session_id(&restart_session);
        fixture
            .cleanup
            .record_session(&fixture.pty_mgr, restart_old_id);
        wait_for_output(&fixture, &restart_session.id, "AC_READY", OUTPUT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_old_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        let restart_old_pid = wait_for_pid_file(&pid_restart, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_old_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        fixture.cleanup.record_pid(restart_old_pid);

        let restarted = restart_session_inner(
            fixture.app.handle(),
            &fixture.session_mgr,
            &fixture.pty_mgr,
            fixture.app.state::<SettingsState>().inner(),
            restart_old_id,
            None,
            None,
            Some(true),
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Phase F restart failed: {e}\n{}",
                fixture.dump(
                    "Phase F",
                    &[restart_old_id],
                    std::slice::from_ref(&pid_restart)
                )
            )
        });
        let restart_new_id = parse_session_id(&restarted);
        fixture
            .cleanup
            .record_session(&fixture.pty_mgr, restart_new_id);
        assert_ne!(restart_old_id, restart_new_id);
        wait_for_process_exit(restart_old_pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_old_id, restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        let restart_new_pid = wait_for_pid_change(&pid_restart, restart_old_pid, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_old_id, restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        fixture.cleanup.record_pid(restart_new_pid);
        assert!(process_exists(restart_new_pid));
        wait_for_output(&fixture, &restarted.id, "AC_READY", OUTPUT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_old_id, restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        {
            let last_event_sequence =
                last_strictly_increasing_sequence(&fixture, &restarted.id, "Phase F");
            let snapshot = get_screen_snapshot(
                fixture.app.state::<Arc<Mutex<PtyManager>>>(),
                restarted.id.clone(),
            )
            .expect("restarted snapshot command succeeds")
            .expect("restarted screen snapshot exists after PTY output");
            let snapshot_text = String::from_utf8_lossy(&snapshot.data);
            assert_eq!(snapshot.session_id, restarted.id);
            assert!(snapshot.rows.is_some(), "Phase F snapshot rows missing");
            assert!(snapshot.cols.is_some(), "Phase F snapshot cols missing");
            assert!(
                snapshot.sequence >= last_event_sequence,
                "Phase F snapshot sequence {} should cover last event sequence {}",
                snapshot.sequence,
                last_event_sequence
            );
            assert!(
                snapshot_text.contains("AC_READY"),
                "Phase F snapshot missing AC_READY; snapshot={snapshot_text:?}\n{}",
                fixture.dump(
                    "Phase F snapshot",
                    &[restart_new_id],
                    std::slice::from_ref(&pid_restart)
                )
            );
        }
        assert_no_pty(&fixture.pty_mgr, restart_old_id, "Phase F", &fixture);
        assert_has_pty(&fixture.pty_mgr, restart_new_id, "Phase F", &fixture);
        assert_active_session(&fixture.session_mgr, restart_new_id).await;

        destroy_session_inner(fixture.app.handle(), restart_new_id)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase F destroy failed: {e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        wait_for_process_exit(restart_new_pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });

        let pid_persisted = fixture.pid_path("persisted");
        let persisted_session = fixture
            .create_session("PTY lifecycle persisted", &pid_persisted)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase G create failed: {e}\n{}",
                    fixture.dump("Phase G", &[], std::slice::from_ref(&pid_persisted))
                )
            });
        let persisted_id = parse_session_id(&persisted_session);
        fixture
            .cleanup
            .record_session(&fixture.pty_mgr, persisted_id);
        wait_for_output(&fixture, &persisted_session.id, "AC_READY", OUTPUT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        let persisted_old_pid = wait_for_pid_file(&pid_persisted, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        fixture.cleanup.record_pid(persisted_old_pid);
        persist_sessions(&fixture.session_mgr).await;
        assert_persisted_contains(&fixture, "PTY lifecycle persisted", "Phase G");

        {
            fixture
                .pty_mgr
                .lock()
                .unwrap()
                .kill(persisted_id)
                .expect("kill persisted pty");
        }
        wait_for_process_exit(persisted_old_pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        assert_persisted_contains(&fixture, "PTY lifecycle persisted", "Phase G");

        let persisted_row = load_sessions()
            .into_iter()
            .find(|row| row.name == "PTY lifecycle persisted")
            .expect("persisted recipe row");
        let second_session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let second_captured = Arc::clone(&fixture.captured_output);
        let second_sequences = Arc::clone(&fixture.captured_sequences);
        let second_errors = Arc::clone(&fixture.listener_errors);
        let (second_app, second_mgr, second_pty_mgr) = make_test_app(
            &fixture.repo_root,
            &second_captured,
            &second_sequences,
            &second_errors,
            &fixture.cleanup,
            Some(second_session_mgr),
            None,
        );

        let restored = create_session_inner(
            second_app.handle(),
            &second_mgr,
            &second_pty_mgr,
            persisted_row.shell,
            persisted_row.shell_args,
            persisted_row.working_directory,
            Some(persisted_row.name),
            persisted_row.agent_id,
            persisted_row.agent_label,
            true,
            persisted_row.git_repos,
            false,
            None,
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Phase G restore failed: {e}\n{}",
                fixture.dump(
                    "Phase G",
                    &[persisted_id],
                    std::slice::from_ref(&pid_persisted)
                )
            )
        });
        let restored_id = parse_session_id(&restored);
        fixture.cleanup.record_session(&second_pty_mgr, restored_id);
        wait_for_output(&fixture, &restored.id, "AC_READY", OUTPUT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id, restored_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        let restored_pid = wait_for_pid_change(&pid_persisted, persisted_old_pid, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id, restored_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        fixture.cleanup.record_pid(restored_pid);
        assert!(process_exists(restored_pid));
        assert_has_pty(&second_pty_mgr, restored_id, "Phase G", &fixture);

        destroy_session_inner(second_app.handle(), restored_id)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase G restored destroy failed: {e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[restored_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        wait_for_process_exit(restored_pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[restored_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
    });
}

#[test]
fn claude_launch_materializes_context_without_prompt_file_arg() {
    let mut fixture = make_lifecycle_fixture();
    let agent_cwd = fixture._temp.path().join(".ac").join("_agent_claude");
    std::fs::create_dir_all(&agent_cwd).expect("create claude agent cwd");
    let script_path = agent_cwd.join("claude.ps1");
    write_child_script(&script_path);
    let pid_file = fixture.pid_path("claude_no_prompt_arg");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let info = create_session_inner(
            fixture.app.handle(),
            &fixture.session_mgr,
            &fixture.pty_mgr,
            "powershell.exe".to_string(),
            script_args(&script_path, &pid_file),
            path_to_string(&agent_cwd),
            Some("Claude context materialization".to_string()),
            None,
            None,
            false,
            Vec::new(),
            true,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("Claude launch failed: {e}"));

        let session_id = parse_session_id(&info);
        fixture.cleanup.record_session(&fixture.pty_mgr, session_id);
        wait_for_output(&fixture, &info.id, "AC_READY", OUTPUT_TIMEOUT)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(
            info.agent_kind,
            Some(agentscommander_lib::session::profile::CodingAgentKind::Claude)
        );
        assert!(
            agent_cwd.join("CLAUDE.md").is_file(),
            "Claude managed instructions file should still be materialized"
        );
        let effective_args = info
            .effective_shell_args
            .as_ref()
            .expect("effective args are captured before spawn");
        assert!(
            !effective_args
                .iter()
                .any(|arg| arg.to_lowercase().contains("--append-system-prompt-file")),
            "Claude launch args must not include --append-system-prompt-file: {effective_args:?}"
        );

        let pid = wait_for_pid_file(&pid_file, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        fixture.cleanup.record_pid(pid);
        destroy_session_inner(fixture.app.handle(), session_id)
            .await
            .unwrap_or_else(|e| panic!("destroy Claude materialization session failed: {e}"));
        wait_for_process_exit(pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
}

impl LifecycleFixture {
    fn pid_path(&mut self, label: &str) -> PathBuf {
        let path = self.config_dir.join(format!("{label}.pid"));
        self.pid_files.push(path.clone());
        path
    }

    async fn create_session(&self, name: &str, pid_file: &Path) -> Result<SessionInfo, String> {
        create_session_inner(
            self.app.handle(),
            &self.session_mgr,
            &self.pty_mgr,
            "powershell.exe".to_string(),
            script_args(&self.script_path, pid_file),
            path_to_string(&self.session_cwd),
            Some(name.to_string()),
            None,
            None,
            false,
            Vec::new(),
            true,
            None,
        )
        .await
    }

    fn dump(&self, phase: &str, session_ids: &[Uuid], pid_files: &[PathBuf]) -> String {
        let sessions_json = std::fs::read_to_string(self.config_dir.join("sessions.json"))
            .unwrap_or_else(|e| format!("<unreadable: {e}>"));
        let captured = self
            .captured_output
            .lock()
            .unwrap()
            .iter()
            .map(|(id, bytes)| format!("{id}: {}", String::from_utf8_lossy(bytes)))
            .collect::<Vec<_>>()
            .join("\n");
        let captured_sequences = self
            .captured_sequences
            .lock()
            .unwrap()
            .iter()
            .map(|(id, sequences)| format!("{id}: {sequences:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        let listener_errors = self.listener_errors.lock().unwrap().join("\n");
        let pid_info = pid_files
            .iter()
            .map(|path| {
                let content =
                    std::fs::read_to_string(path).unwrap_or_else(|e| format!("<missing: {e}>"));
                let parsed = content.trim().parse::<u32>().ok();
                let exists = parsed.map(process_exists);
                format!(
                    "{} => {:?}, exists={exists:?}",
                    path.display(),
                    content.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let pty_info = session_ids
            .iter()
            .map(|id| {
                let pty = self.pty_mgr.lock().unwrap();
                format!(
                    "{id}: has_session={}, size={:?}",
                    pty.has_session(*id),
                    pty.get_pty_size(*id)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "phase={phase}\nconfig_dir={}\nrepo_root={}\nsession_cwd={}\nsessions_json={sessions_json}\npids=\n{pid_info}\npty=\n{pty_info}\nlistener_errors=\n{listener_errors}\ncaptured_sequences=\n{captured_sequences}\ncaptured=\n{captured}",
            self.config_dir.display(),
            self.repo_root.display(),
            self.session_cwd.display()
        )
    }
}

fn write_child_script(path: &Path) {
    let script = r#"param([string]$PidFile)
$ErrorActionPreference = 'Stop'
$pid | Set-Content -LiteralPath $PidFile -Encoding ASCII
[Console]::Out.WriteLine('AC_READY')
[Console]::Out.Flush()
while ($true) {
    $line = [Console]::In.ReadLine()
    if ($null -eq $line) { break }
    if ($line -eq 'AC_EXIT') {
        [Console]::Out.WriteLine('AC_EXITING')
        [Console]::Out.Flush()
        exit 0
    }
    if ($line.StartsWith('AC_ECHO ')) {
        [Console]::Out.WriteLine('AC_ECHOED ' + $line.Substring(8))
        [Console]::Out.Flush()
    }
}
"#;
    std::fs::write(path, script).expect("write pty child script");
}

fn script_args(script_path: &Path, pid_file: &Path) -> Vec<String> {
    vec![
        "-NoLogo".to_string(),
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        path_to_string(script_path),
        "-PidFile".to_string(),
        path_to_string(pid_file),
    ]
}

fn run_powershell_preflight() {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Write-Output AC_PS_OK",
        ])
        .output()
        .expect("run PowerShell preflight");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("AC_PS_OK"),
        "PowerShell preflight failed: status={:?}\nstdout={stdout}\nstderr={stderr}",
        output.status.code()
    );
}

async fn wait_for_output(
    fixture: &LifecycleFixture,
    session_id: &str,
    marker: &str,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let text = {
            let captured = fixture.captured_output.lock().unwrap();
            captured
                .get(session_id)
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .unwrap_or_default()
        };
        if text.contains(marker) {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!(
        "timeout waiting for output marker '{marker}' in session {session_id}"
    ))
}

fn last_strictly_increasing_sequence(
    fixture: &LifecycleFixture,
    session_id: &str,
    phase: &str,
) -> u64 {
    let sequences = fixture
        .captured_sequences
        .lock()
        .unwrap()
        .get(session_id)
        .cloned()
        .unwrap_or_default();
    assert!(
        !sequences.is_empty(),
        "{phase}: expected at least one pty_output sequence for {session_id}"
    );
    for pair in sequences.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{phase}: sequences must be strictly increasing: {sequences:?}"
        );
    }
    *sequences.last().unwrap()
}

async fn wait_for_pid_file(path: &Path, timeout: Duration) -> Result<u32, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                return Ok(pid);
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!("timeout waiting for pid file {}", path.display()))
}

async fn wait_for_pid_change(path: &Path, old_pid: u32, timeout: Duration) -> Result<u32, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                if pid != old_pid {
                    return Ok(pid);
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!(
        "timeout waiting for pid file {} to change from {old_pid}",
        path.display()
    ))
}

async fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !process_exists(pid) {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!("process {pid} still alive after {timeout:?}"))
}

fn process_exists(pid: u32) -> bool {
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}"
            ),
        ])
        .status();
    matches!(status, Ok(status) if status.success())
}

fn stop_process(pid: u32) -> std::io::Result<()> {
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("Stop-Process -Id {pid} -Force -ErrorAction SilentlyContinue"),
        ])
        .status()
        .map(|_| ())
}

async fn persist_sessions(session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>) {
    let mgr = session_mgr.read().await;
    persist_current_state_result(&mgr)
        .await
        .expect("persist current state");
}

fn assert_persisted_contains(fixture: &LifecycleFixture, name: &str, phase: &str) {
    let sessions_json = std::fs::read_to_string(fixture.config_dir.join("sessions.json"))
        .expect("sessions.json exists");
    assert!(
        sessions_json.contains(name)
            && sessions_json.contains("powershell.exe")
            && sessions_json.contains("shellArgs")
            && sessions_json.contains("workingDirectory")
            && sessions_json.contains("status"),
        "{phase} persisted content missing expected fields\n{}",
        fixture.dump(phase, &[], &fixture.pid_files)
    );
    assert!(
        load_sessions().iter().any(|row| row.name == name),
        "{phase} load_sessions missing {name}\n{}",
        fixture.dump(phase, &[], &fixture.pid_files)
    );
}

fn assert_persisted_missing(fixture: &LifecycleFixture, name: &str, phase: &str) {
    assert!(
        !load_sessions().iter().any(|row| row.name == name),
        "{phase} load_sessions still contains {name}\n{}",
        fixture.dump(phase, &[], &fixture.pid_files)
    );
}

fn assert_has_pty(
    pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    phase: &str,
    fixture: &LifecycleFixture,
) {
    assert!(
        pty_mgr.lock().unwrap().has_session(id),
        "{phase} expected PTY session {id}\n{}",
        fixture.dump(phase, &[id], &fixture.pid_files)
    );
}

fn assert_no_pty(
    pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    phase: &str,
    fixture: &LifecycleFixture,
) {
    assert!(
        !pty_mgr.lock().unwrap().has_session(id),
        "{phase} expected no PTY session {id}\n{}",
        fixture.dump(phase, &[id], &fixture.pid_files)
    );
}

async fn assert_session_missing(session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>, id: Uuid) {
    let mgr = session_mgr.read().await;
    assert!(mgr.get_session(id).await.is_none());
}

async fn assert_active_session(session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>, id: Uuid) {
    let mgr = session_mgr.read().await;
    let session = mgr.get_session(id).await.expect("active session exists");
    assert_eq!(session.status, SessionStatus::Active);
}

fn parse_session_id(session: &SessionInfo) -> Uuid {
    Uuid::parse_str(&session.id).expect("session id is uuid")
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
