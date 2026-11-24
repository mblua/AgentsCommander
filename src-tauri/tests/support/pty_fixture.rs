//! PTY test fixture parts shared by the `pty_lifecycle_regression` and
//! `pty_powershell_managed_native` integration-test crates.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentscommander_lib::commands::pty::get_screen_snapshot;
use agentscommander_lib::config::settings::SettingsState;
use agentscommander_lib::pty::git_watcher::GitWatcher;
use agentscommander_lib::pty::idle_detector::IdleDetector;
use agentscommander_lib::pty::manager::PtyManager;
use agentscommander_lib::resource_monitor::ResourceMonitorState;
use agentscommander_lib::session::manager::SessionManager;
use agentscommander_lib::session::selection::SelectionCoordinator;
use agentscommander_lib::session::session::SessionInfo;
use agentscommander_lib::shutdown::ShutdownSignal;
use agentscommander_lib::telegram::manager::{OutputSenderMap, TelegramBridgeState};
use agentscommander_lib::voice::tracker::{VoiceTracker, VoiceTrackingState};
use agentscommander_lib::web::auth::WebAccessToken;
use agentscommander_lib::web::broadcast::WsBroadcaster;
use agentscommander_lib::{
    AppOutbox, ConfigSeedLockState, DetachedSessionsState, MasterToken, RestoreInProgress,
    SpecBoardState, WebServerHandle,
};
use serde::Deserialize;
use tauri::Manager;
use uuid::Uuid;

pub(crate) static TEST_CONFIG_ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct TestConfigEnvGuard {
    previous: Option<String>,
}

impl TestConfigEnvGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let previous = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").ok();
        // SAFETY: the fixture holds TEST_CONFIG_ENV_LOCK
        unsafe {
            std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", path);
        }
        Self { previous }
    }
}

impl Drop for TestConfigEnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.as_ref() {
            // SAFETY: the fixture holds TEST_CONFIG_ENV_LOCK
            unsafe {
                std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", previous);
            }
        } else {
            // SAFETY: the fixture holds TEST_CONFIG_ENV_LOCK
            unsafe {
                std::env::remove_var("AGENTSCOMMANDER_TEST_CONFIG_DIR");
            }
        }
    }
}

/// Test-only wire observation of the two final `pty_output` delivery variants.
/// This is a tagged mirror of the production `TerminalOutputDelivery` union, not a
/// production DTO. Inactive-session regressions must see zero deliveries of either
/// variant; the listener records any unexpected delivery so the wait helper can fail
/// with a precise count instead of a silent parse error.
#[derive(Deserialize)]
#[serde(tag = "kind")]
pub(crate) enum TestPtyOutputDelivery {
    #[serde(rename = "data")]
    Data {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        _generation: String,
        #[serde(rename = "firstSequence")]
        _first_sequence: String,
        #[serde(rename = "sequence")]
        sequence: String,
        #[serde(rename = "data")]
        data: Vec<u8>,
    },
    #[serde(rename = "resyncRequired")]
    ResyncRequired {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        _generation: String,
        #[serde(rename = "sequence")]
        _sequence: String,
    },
}

pub(crate) fn parse_session_id(session: &SessionInfo) -> Uuid {
    Uuid::parse_str(&session.id).expect("session id is uuid")
}

/// Builds the git watcher (on its own leaked handle app) and the PTY manager.
pub(crate) fn git_watcher_and_pty(
    owners: &agentscommander_lib::AppOwners,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    output_senders: OutputSenderMap,
) -> (Arc<GitWatcher>, Arc<Mutex<PtyManager>>) {
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let git_app = Box::leak(Box::new(
        agentscommander_lib::test_support::test_builder_with_owners(owners.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build git watcher handle app"),
    ));
    let git_watcher = GitWatcher::with_state(
        Arc::clone(session_mgr),
        git_app.handle().clone(),
        owners.git.clone(),
    );
    let pty_mgr = Arc::new(Mutex::new(PtyManager::with_owners(
        output_senders,
        idle_detector,
        Arc::clone(&git_watcher),
        None,
        None,
        #[cfg(windows)]
        owners.git_guard.clone(),
        owners.spawn.clone(),
    )));
    (git_watcher, pty_mgr)
}

/// The states a PTY test app manages beyond the fixed defaults.
pub(crate) struct TestAppStates {
    pub(crate) settings: SettingsState,
    pub(crate) session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(crate) tg_mgr: TelegramBridgeState,
    pub(crate) git_watcher: Arc<GitWatcher>,
    pub(crate) pty_mgr: Arc<Mutex<PtyManager>>,
}

/// Builds the managed test app and opens its selection coordinator. `label` names the
/// tokens (`<label>-master-token`, `<label>-web-token`) and the build expectation.
pub(crate) fn build_test_app(
    owners: &agentscommander_lib::AppOwners,
    repo_root: &Path,
    states: TestAppStates,
    label: &str,
) -> tauri::App {
    let TestAppStates {
        settings,
        session_mgr,
        tg_mgr,
        git_watcher,
        pty_mgr,
    } = states;
    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice_tracking: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));
    let spec_board_state: SpecBoardState = Arc::new(tokio::sync::RwLock::new(
        agentscommander_lib::commands::spec_board::SpecBoardManager::new(),
    ));
    let config_seed_lock: ConfigSeedLockState = Arc::new(tokio::sync::Mutex::new(()));
    let shutdown_signal = ShutdownSignal::new();
    let selection_coordinator =
        SelectionCoordinator::new(Arc::clone(&session_mgr), shutdown_signal.token().clone());
    let app = agentscommander_lib::test_support::test_builder_with_owners(owners.clone())
        .manage(MasterToken::new(format!("{label}-master-token")))
        .manage(AppOutbox::new(
            repo_root.join(".app-outbox").to_string_lossy().to_string(),
        ))
        .manage(settings)
        .manage(session_mgr)
        .manage(selection_coordinator.clone())
        .manage(tg_mgr)
        .manage(detached_sessions)
        .manage(voice_tracking)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .manage(shutdown_signal)
        .manage(Arc::new(WebAccessToken::new(format!("{label}-web-token"))))
        .manage(WsBroadcaster::new())
        .manage(WebServerHandle::default())
        .manage(spec_board_state)
        .manage(config_seed_lock)
        .manage(git_watcher)
        .manage(Arc::new(ResourceMonitorState::with_spawn_registry(
            owners.spawn.clone(),
        )))
        .manage(pty_mgr)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap_or_else(|e| panic!("build {} test app: {e:?}", label.replace('-', " ")));
    bootstrap_selection(&selection_coordinator, &app);
    app
}

fn bootstrap_selection(selection_coordinator: &SelectionCoordinator, app: &tauri::App) {
    selection_coordinator
        .start(app.handle().clone())
        .expect("start selection coordinator");
    let bootstrap = selection_coordinator.clone();
    std::thread::spawn(move || {
        tauri::async_runtime::block_on(async move {
            bootstrap
                .submit_restore_first()
                .await
                .expect("open selection coordinator")
                .finish();
        });
    })
    .join()
    .expect("join selection bootstrap");
}

/// Polls the unchanged public `commands::pty::get_screen_snapshot` route until the native
/// snapshot contains the expected bytes (bounded by `timeout`). Before returning success it
/// inspects the per-session tagged listener recorder and asserts zero `pty_output` data or
/// resync-marker deliveries: these are inactive-session regressions, so native
/// parser/snapshot progress must occur without any UI observer delivery.
pub(crate) async fn wait_for_snapshot_contains(
    app: &tauri::App,
    captured_deliveries: &Mutex<HashMap<String, usize>>,
    (session_id, expected_bytes, phase): (&Uuid, &str, &str),
    (timeout, poll_interval): (Duration, Duration),
) -> Result<(), String> {
    let start = Instant::now();
    let mut last_snapshot: Option<serde_json::Value> = None;
    while start.elapsed() < timeout {
        let snapshot = get_screen_snapshot(
            app.state::<Arc<Mutex<PtyManager>>>(),
            session_id.to_string(),
        )
        .map_err(|e| format!("{phase}: get_screen_snapshot failed for {session_id}: {e}"))?;
        if let Some(snapshot) = snapshot {
            last_snapshot =
                Some(serde_json::to_value(&snapshot).expect("serialize legacy snapshot payload"));
            let text = String::from_utf8_lossy(&snapshot.data);
            if text.contains(expected_bytes) {
                let deliveries = captured_deliveries
                    .lock()
                    .unwrap()
                    .get(&session_id.to_string())
                    .copied()
                    .unwrap_or(0);
                if deliveries != 0 {
                    return Err(format!(
                        "{phase}: session {session_id} recorded {deliveries} tagged pty_output \
                         deliveries while inactive; expected zero"
                    ));
                }
                return Ok(());
            }
        }
        tokio::time::sleep(poll_interval).await;
    }
    Err(format!(
        "{phase}: timeout waiting for snapshot marker '{expected_bytes}' in session \
         {session_id}; last_snapshot={last_snapshot:?}"
    ))
}
