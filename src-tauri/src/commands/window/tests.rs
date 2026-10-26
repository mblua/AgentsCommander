use super::{
    get_watchers_scope, open_watchers_window, resource_monitor_placement_for_main,
    set_main_window_placement_inner_with_saver, PhysicalWindowRect, WatchersScopeState,
    WindowDestroyAudit, RESOURCE_MONITOR_DOCK_WIDTH, WATCHERS_WINDOW_LABEL,
};
use crate::config::settings::{AppSettings, MainWindowDisplayState, SettingsState, WindowGeometry};
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::{SelectionCoordinator, SelectionMode, SelectionSource};
use crate::session::session::SessionStatus;
use crate::web::broadcast::WsBroadcaster;
use crate::DetachedSessionsState;
use futures::future::BoxFuture;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tauri::{Listener, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct GatedLivenessBackend {
    live: AtomicBool,
    calls: AtomicUsize,
    block_call: usize,
    entered: AtomicBool,
    gate: (Mutex<bool>, Condvar),
}

impl GatedLivenessBackend {
    fn new(block_call: usize) -> Self {
        Self {
            live: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            block_call,
            entered: AtomicBool::new(false),
            gate: (Mutex::new(false), Condvar::new()),
        }
    }

    fn lose_liveness_and_release(&self) {
        self.live.store(false, Ordering::SeqCst);
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}

impl PtyBackend for GatedLivenessBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(&self, _spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.live
            .load(Ordering::SeqCst)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        self.write(
            &crate::pty::manager::BackendWriteAuthority::for_backend_test(),
            id,
            &[],
        )
    }

    fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
        self.live.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn has_session(&self, _id: Uuid) -> bool {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.block_call {
            self.entered.store(true, Ordering::SeqCst);
            let mut released = self.gate.0.lock().unwrap();
            while !*released {
                released = self.gate.1.wait(released).unwrap();
            }
        }
        self.live.load(Ordering::SeqCst)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        self.live.load(Ordering::SeqCst).then_some((120, 30))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

#[test]
fn resource_monitor_stays_inside_negative_x_monitor_when_right_side_is_full() {
    let main = PhysicalWindowRect::new(-1920, 0, 1920, 1040);
    let monitor = PhysicalWindowRect::new(-1920, 0, 1920, 1040);

    let placement =
        resource_monitor_placement_for_main(main, monitor, RESOURCE_MONITOR_DOCK_WIDTH, 1040);

    assert_eq!(placement.x, -420);
    assert_eq!(placement.y, 0);
    assert_eq!(placement.width, 420);
    assert_eq!(placement.height, 1040);
    assert!(placement.x >= monitor.x);
    assert!(placement.x + placement.width as i32 <= monitor.right());
}

#[test]
fn resource_monitor_uses_right_side_when_it_fits_same_monitor() {
    let main = PhysicalWindowRect::new(80, 40, 1000, 700);
    let monitor = PhysicalWindowRect::new(0, 0, 1920, 1080);

    let placement =
        resource_monitor_placement_for_main(main, monitor, RESOURCE_MONITOR_DOCK_WIDTH, 700);

    assert_eq!(placement.x, 1080);
    assert_eq!(placement.y, 40);
    assert_eq!(placement.width, 420);
    assert_eq!(placement.height, 700);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detach_pty_loss_after_window_creation_compensates_and_reconciles_liveness() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/detach-race".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let backend = Arc::new(GatedLivenessBackend::new(2));
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let detached = DetachedSessionsState::default();
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(detached)
        .manage(WsBroadcaster::new())
        .manage(WindowDestroyAudit::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build detach race app");
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for event_name in ["session_destroyed", "session_created", "session_switched"] {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |_| {
            let _ = events_tx.send(event_name);
        });
    }

    let detach = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.detach(session.id, None, false).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while !backend.entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detach reaches post-window liveness barrier");
    let label = format!("terminal-{}", session.id.to_string().replace('-', ""));
    assert!(app.get_webview_window(&label).is_some());
    assert!(app
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap()
        .contains(&session.id));

    backend.lose_liveness_and_release();
    assert_eq!(
        detach.await.unwrap().unwrap_err(),
        "Session lost liveness during detach"
    );
    assert_eq!(app.state::<WindowDestroyAudit>().count(&label), 1);
    assert!(!app
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap()
        .contains(&session.id));
    let row = manager.read().await.get_session(session.id).await.unwrap();
    assert_eq!(row.status, SessionStatus::Exited(1));
    assert!(!row.was_detached);
    let selection = manager.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(session.id));
    assert_eq!(selection.mode(), SelectionMode::Dormant);
    assert_eq!(selection.source(), SelectionSource::LivenessReconcile);
    assert_eq!(
        (0..3)
            .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect::<Vec<_>>(),
        vec!["session_destroyed", "session_created", "session_switched"]
    );
    assert!(events_rx.try_recv().is_err());
    coordinator.close_and_join().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_post_destroy_pty_loss_clears_intent_preserves_geometry_and_publishes_exit() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/attach-race".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let geometry = WindowGeometry {
        x: 17.25,
        y: -44.5,
        width: 923.75,
        height: 611.125,
    };
    let manager_handle = manager.read().await.clone();
    manager_handle.set_was_detached(session.id, true).await;
    manager_handle
        .set_detached_geometry(session.id, geometry.clone())
        .await;
    let backend = Arc::new(GatedLivenessBackend::new(1));
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .manage(WindowDestroyAudit::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build attach race app");
    let label = format!("terminal-{}", session.id.to_string().replace('-', ""));
    WebviewWindowBuilder::new(app.handle(), &label, WebviewUrl::App("index.html".into()))
        .build()
        .unwrap();
    app.state::<DetachedSessionsState>()
        .lock()
        .unwrap()
        .insert(session.id);
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for event_name in [
        "session_destroyed",
        "session_created",
        "terminal_attached",
        "session_switched",
    ] {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |_| {
            let _ = events_tx.send(event_name);
        });
    }

    let attach = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.attach(session.id).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while !backend.entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("attach reaches post-destroy liveness barrier");
    assert_eq!(app.state::<WindowDestroyAudit>().count(&label), 1);
    assert!(!app
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap()
        .contains(&session.id));

    backend.lose_liveness_and_release();
    assert_eq!(
        attach.await.unwrap().unwrap_err(),
        "Session has no live PTY"
    );
    let row = manager_handle.get_session(session.id).await.unwrap();
    assert_eq!(row.status, SessionStatus::Exited(1));
    assert!(!row.was_detached);
    let stored_geometry = row.detached_geometry.unwrap();
    assert_eq!(stored_geometry.x.to_bits(), geometry.x.to_bits());
    assert_eq!(stored_geometry.y.to_bits(), geometry.y.to_bits());
    assert_eq!(stored_geometry.width.to_bits(), geometry.width.to_bits());
    assert_eq!(stored_geometry.height.to_bits(), geometry.height.to_bits());
    let selection = manager_handle.selection_payload().await;
    assert_eq!(selection.id(), Some(session.id));
    assert_eq!(selection.mode(), SelectionMode::Dormant);
    assert_eq!(selection.source(), SelectionSource::LivenessReconcile);
    assert_eq!(
        (0..4)
            .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect::<Vec<_>>(),
        vec![
            "session_destroyed",
            "session_created",
            "terminal_attached",
            "session_switched",
        ]
    );
    assert!(events_rx.try_recv().is_err());
    coordinator.close_and_join().await;
}

/// An app that manages the scope state, which every `open_watchers_window` call needs.
fn watchers_app(label: &str) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(WatchersScopeState::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap_or_else(|_| panic!("build {label} app"))
}

/// #1171, test 79a: a second open does not build a second window, and it does re-scope the
/// one that is already there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopening_the_watchers_window_re_scopes_it_instead_of_building_a_second_one() {
    let app = watchers_app("watchers re-scope");
    WebviewWindowBuilder::new(
        app.handle(),
        WATCHERS_WINDOW_LABEL,
        WebviewUrl::App("index.html?window=watchers".into()),
    )
    .build()
    .unwrap();

    let (payloads_tx, payloads_rx) = std::sync::mpsc::channel();
    app.listen_any("watchers_scope_request", move |event| {
        let _ = payloads_tx.send(event.payload().to_string());
    });

    let session_id = Uuid::new_v4();
    open_watchers_window(app.handle().clone(), app.state(), session_id.to_string())
        .await
        .unwrap();

    let windows = app.webview_windows();
    assert_eq!(windows.len(), 1);
    assert!(windows.contains_key(WATCHERS_WINDOW_LABEL));

    let payload: serde_json::Value =
        serde_json::from_str(&payloads_rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
    assert_eq!(payload["sessionId"], session_id.to_string());
    assert!(payloads_rx.try_recv().is_err());
}

/// #1171, test 79b: two opens leave the authoritative scope at the SECOND one, whether or
/// not any listener exists.
///
/// This is the property a Rust-side listener cannot certify. `app.listen_any` registers in
/// the Rust registry, which is always durable, so no reordering of a `listen_any` test can
/// model a JavaScript listener that has not been registered yet. Test 79a certifies that
/// the emit is issued; this one certifies that the pull recovers it when it was not heard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_last_open_wins_the_scope_with_no_listener_and_with_one() {
    let app = watchers_app("watchers scope pull");
    WebviewWindowBuilder::new(
        app.handle(),
        WATCHERS_WINDOW_LABEL,
        WebviewUrl::App("index.html?window=watchers".into()),
    )
    .build()
    .unwrap();

    assert_eq!(
        get_watchers_scope(app.state()).await.unwrap(),
        None,
        "nothing has been requested yet"
    );

    // No listener at all: the emits go nowhere, exactly as they do while the bundle loads.
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    open_watchers_window(app.handle().clone(), app.state(), first.to_string())
        .await
        .unwrap();
    open_watchers_window(app.handle().clone(), app.state(), second.to_string())
        .await
        .unwrap();
    assert_eq!(
        get_watchers_scope(app.state()).await.unwrap(),
        Some(second.to_string()),
        "the window that subscribes late still pulls the LAST order, not the first"
    );

    // And with a listener the two halves agree: the pull returns what the last emit said.
    let (payloads_tx, payloads_rx) = std::sync::mpsc::channel();
    app.listen_any("watchers_scope_request", move |event| {
        let _ = payloads_tx.send(event.payload().to_string());
    });
    let third = Uuid::new_v4();
    open_watchers_window(app.handle().clone(), app.state(), third.to_string())
        .await
        .unwrap();

    let payload: serde_json::Value =
        serde_json::from_str(&payloads_rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
    assert_eq!(payload["sessionId"], third.to_string());
    assert_eq!(
        get_watchers_scope(app.state()).await.unwrap(),
        Some(third.to_string())
    );
}

/// #1171, test 79c: two concurrent opens for two different sessions produce exactly one
/// window, two `Ok`s and no duplicate-label failure, AND the authoritative scope agrees
/// with the event the loser emitted.
///
/// Counting windows and `Ok`s is not enough. The defect this test exists for is the
/// interleaving that leaves the state at one session and the last emitted event at the
/// other, and a count-only assertion passes straight through it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_opens_build_one_window_and_agree_on_the_scope() {
    let app = watchers_app("watchers concurrent open");

    let (payloads_tx, payloads_rx) = std::sync::mpsc::channel();
    app.listen_any("watchers_scope_request", move |event| {
        let _ = payloads_tx.send(event.payload().to_string());
    });

    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let (left, right) = tokio::join!(
        open_watchers_window(app.handle().clone(), app.state(), first.to_string()),
        open_watchers_window(app.handle().clone(), app.state(), second.to_string()),
    );
    assert!(left.is_ok(), "the losing call must not fail on the label");
    assert!(right.is_ok());

    let windows = app.webview_windows();
    assert_eq!(windows.len(), 1);
    assert!(windows.contains_key(WATCHERS_WINDOW_LABEL));

    // Exactly one of the two built the window; the other found it and emitted. Whichever
    // that was, it ran second under the one guard, so it is also the last state write.
    let emitted: serde_json::Value =
        serde_json::from_str(&payloads_rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
    assert!(payloads_rx.try_recv().is_err(), "the builder emits nothing");
    assert_eq!(
        get_watchers_scope(app.state()).await.unwrap(),
        Some(emitted["sessionId"].as_str().unwrap().to_string()),
        "the authoritative scope and the last emitted event are the same call"
    );
}

/// The session id is interpolated into the window URL, so anything that is not a UUID is
/// refused before a window exists to carry it, and before the scope state is touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_watchers_window_refuses_a_session_id_that_is_not_a_uuid() {
    let app = watchers_app("watchers rejection");

    assert!(open_watchers_window(
        app.handle().clone(),
        app.state(),
        "not-a-uuid&window=main".to_string()
    )
    .await
    .is_err());
    assert!(app.webview_windows().is_empty());
    assert_eq!(get_watchers_scope(app.state()).await.unwrap(), None);
}

// ── #2348 - the narrow main-window placement command ──────────────────

fn placement_geometry(x: f64, y: f64) -> WindowGeometry {
    WindowGeometry {
        x,
        y,
        width: 800.0,
        height: 600.0,
    }
}

fn placement_state(settings: AppSettings) -> SettingsState {
    crate::config::settings::new_settings_state(settings)
}

fn write_placement_base(path: &std::path::Path, placement: serde_json::Value) {
    let mut base = serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "agents": [],
        "rootToken": "fixed-token",
    });
    for (key, value) in placement.as_object().unwrap() {
        base[key] = value.clone();
    }
    std::fs::write(path, serde_json::to_string_pretty(&base).unwrap()).unwrap();
}

#[tokio::test]
async fn issue_2348_placement_command_publishes_the_written_pair() {
    let original = AppSettings {
        main_geometry: None,
        main_window_display_state: MainWindowDisplayState::Normal,
        ..AppSettings::default()
    };
    let state = placement_state(original);

    set_main_window_placement_inner_with_saver(
        &state,
        placement_geometry(5.0, 6.0),
        MainWindowDisplayState::Maximized,
        |current, geometry, display_state| {
            let mut written = current.clone();
            written.main_geometry = Some(geometry.clone());
            written.main_window_display_state = display_state;
            Ok(written)
        },
    )
    .await
    .unwrap();

    let live = state.read().await;
    assert_eq!(
        live.main_window_display_state,
        MainWindowDisplayState::Maximized
    );
    let geometry = live.main_geometry.as_ref().expect("geometry published");
    assert_eq!((geometry.x, geometry.y), (5.0, 6.0));
}

#[tokio::test]
async fn issue_2348_placement_command_rejection_leaves_live_settings_unchanged() {
    let state = placement_state(AppSettings {
        main_geometry: None,
        main_window_display_state: MainWindowDisplayState::Normal,
        ..AppSettings::default()
    });

    let err = set_main_window_placement_inner_with_saver(
        &state,
        placement_geometry(5.0, 6.0),
        MainWindowDisplayState::Maximized,
        |_, _, _| Err("main_window_placement_overlay_pinned".to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "main_window_placement_overlay_pinned");

    let live = state.read().await;
    assert!(live.main_geometry.is_none());
    assert_eq!(
        live.main_window_display_state,
        MainWindowDisplayState::Normal
    );
}

#[test]
fn issue_2348_placement_owner_writes_both_keys_as_one_explicit_pair() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    write_placement_base(
        &path,
        serde_json::json!({
            "mainGeometry": { "x": 1.0, "y": 2.0, "width": 300.0, "height": 400.0 },
            "mainWindowDisplayState": "normal",
        }),
    );
    let current = crate::config::settings::load_settings_from_path(&path);

    let written = crate::config::settings::set_main_window_placement_at_path(
        &current,
        &path,
        &placement_geometry(50.0, 60.0),
        MainWindowDisplayState::Maximized,
    )
    .unwrap();

    let disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(disk["mainGeometry"]["x"], serde_json::json!(50.0));
    assert_eq!(disk["mainGeometry"]["y"], serde_json::json!(60.0));
    assert_eq!(disk["mainGeometry"]["width"], serde_json::json!(800.0));
    assert_eq!(
        disk["mainWindowDisplayState"],
        serde_json::json!("maximized")
    );
    assert_eq!(
        written.main_window_display_state,
        MainWindowDisplayState::Maximized
    );
    assert_eq!(written.main_geometry.as_ref().map(|g| g.x), Some(50.0));
}

/// Writes the base file plus an overlay that owns the given placement key,
/// then loads through the real merge so ownership is genuine.
fn seed_overlay_placement(
    key: &str,
    value: serde_json::Value,
) -> (tempfile::TempDir, std::path::PathBuf, AppSettings) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    write_placement_base(&path, serde_json::json!({}));
    std::fs::write(
        temp.path().join("settings.50.personal.no-git.json"),
        serde_json::to_string_pretty(&serde_json::json!({ key: value })).unwrap(),
    )
    .unwrap();
    let current = crate::config::settings::load_settings_from_path(&path);
    (temp, path, current)
}

#[test]
fn issue_2348_overlay_owned_geometry_rejects_without_touching_the_base_file() {
    let (temp, path, current) = seed_overlay_placement(
        "mainGeometry",
        serde_json::json!({ "x": 1.0, "y": 2.0, "width": 300.0, "height": 400.0 }),
    );
    assert!(current.local_overlay_state.owns_top_level("mainGeometry"));
    let before = std::fs::read_to_string(&path).unwrap();

    let err = crate::config::settings::set_main_window_placement_at_path(
        &current,
        &path,
        &placement_geometry(50.0, 60.0),
        MainWindowDisplayState::Maximized,
    )
    .unwrap_err();

    assert_eq!(err, "main_window_placement_overlay_pinned");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    drop(temp);
}

#[test]
fn issue_2348_overlay_owned_display_state_rejects_without_touching_the_base_file() {
    let (temp, path, current) =
        seed_overlay_placement("mainWindowDisplayState", serde_json::json!("maximized"));
    assert!(current
        .local_overlay_state
        .owns_top_level("mainWindowDisplayState"));
    let before = std::fs::read_to_string(&path).unwrap();

    let err = crate::config::settings::set_main_window_placement_at_path(
        &current,
        &path,
        &placement_geometry(50.0, 60.0),
        MainWindowDisplayState::Normal,
    )
    .unwrap_err();

    assert_eq!(err, "main_window_placement_overlay_pinned");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    drop(temp);
}
