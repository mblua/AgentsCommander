use super::*;
use crate::config::settings::{new_settings_state, AppSettings};
use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use crate::pty::context_scrape::ScreenRowsRead;
use crate::pty::input_route::BackendWriteAuthority;
use crate::pty::output::PtyScreenSnapshot;
use crate::web::command_port::WebCommandDispatcher;
use crate::web::commands::{dispatch, WebCommandsDispatcher, WsState};
use crate::web::event_broadcast::DesktopBroadcastEvents;
use futures::FutureExt;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct TestBackend {
    log: Arc<Mutex<Vec<String>>>,
    fail: AtomicBool,
}

struct TestOperations {
    log: Arc<Mutex<Vec<String>>>,
    pty: Arc<Mutex<PtyManager>>,
    restore: Option<Arc<crate::app_state::RestoreInProgress>>,
    purge: Option<Arc<crate::session::purge_guard::PurgeGuard>>,
    create_fails: AtomicBool,
}

impl AgentUpdateOperations for TestOperations {
    fn agent_update_status(&self) -> Result<AgentUpdateStatus, String> {
        self.log.lock().unwrap().push("status".into());
        Err("fake update gate".into())
    }

    fn agent_update_answer<'a>(
        &'a self,
        settings: &'a SettingsState,
        command: String,
        enabled: bool,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + 'a>> {
        Box::pin(async move {
            let _ = settings;
            self.log
                .lock()
                .unwrap()
                .push(format!("answer:{command}:{enabled}"));
            Ok(enabled)
        })
    }

    fn agent_update_cancel<'a>(
        &'a self,
        _command: String,
    ) -> Pin<Box<dyn Future<Output = Result<AgentUpdateCancelResponse, String>> + Send + 'a>> {
        panic!("unexpected agent_update_cancel")
    }

    fn agent_updates_cancel_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<AgentUpdateCancelAllResponse, String>> + Send + 'a>>
    {
        panic!("unexpected agent_updates_cancel_all")
    }

    fn agent_update_overview<'a>(
        &'a self,
        _settings: &'a SettingsState,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AgentUpdateOverviewRow>, String>> + Send + 'a>>
    {
        panic!("unexpected agent_update_overview")
    }

    fn install_coding_agent<'a>(
        &'a self,
        _settings: &'a SettingsState,
        _key: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        panic!("unexpected install_coding_agent")
    }
}

impl WebOperations for TestOperations {
    fn selection(&self) -> &crate::session::selection::SelectionCoordinator {
        panic!("unexpected selection")
    }

    fn warnings(&self) -> &crate::session::warnings::SessionWarningState {
        panic!("unexpected warnings")
    }

    fn purge_guard(&self) -> Option<&Arc<crate::session::purge_guard::PurgeGuard>> {
        self.log.lock().unwrap().push("purge".into());
        self.purge.as_ref()
    }

    fn response_close(&self) -> Option<&crate::pty::manager::ResponseCloseState> {
        panic!("unexpected response_close")
    }

    fn detached(&self) -> &crate::app_state::DetachedSessionsState {
        panic!("unexpected detached")
    }

    fn restore(&self) -> Option<&Arc<crate::app_state::RestoreInProgress>> {
        self.log.lock().unwrap().push("restore".into());
        self.restore.as_ref()
    }

    fn emit_agent_file_save_events(&self, _reply: &crate::config::settings::AgentFileSaveReply) {
        panic!("unexpected emit_agent_file_save_events")
    }

    fn create_session<'a>(
        &'a self,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        request: WebSessionCreate,
    ) -> Pin<Box<dyn Future<Output = Result<SessionInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            let _ = session_mgr;
            assert!(Arc::ptr_eq(pty_mgr, &self.pty));
            assert_eq!(request.shell, "fake-shell");
            assert_eq!(request.shell_args, ["--fake"]);
            assert_eq!(request.cwd, "C:/fixture");
            assert_eq!(request.session_name.as_deref(), Some("Browser"));
            assert!(request.agent_id.is_none());
            assert!(request.agent_label.is_none());
            assert!(request.resolved_spawn.is_none());
            assert!(request.resolved_agent_host_shell.is_none());
            self.log.lock().unwrap().push("create".into());
            if self.create_fails.load(Ordering::SeqCst) {
                return Err("fake create failure".into());
            }
            serde_json::from_value(json!({
                "id": "created", "name": "Browser", "shell": request.shell,
                "shellArgs": request.shell_args, "createdAt": "now",
                "workingDirectory": request.cwd, "status": "running",
                "waitingForInput": false, "lastPrompt": null, "token": "fixture",
            }))
            .map_err(|error| error.to_string())
        })
    }

    fn destroy_session<'a>(
        &'a self,
        _uuid: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        panic!("unexpected destroy_session")
    }

    fn mark_successful_pty_write_busy<'a>(
        &'a self,
        uuid: Uuid,
        byte_count: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            assert!(PtyManager::acquire_input_writer(&self.pty, uuid)
                .now_or_never()
                .is_none());
            self.log.lock().unwrap().push(format!("busy:{byte_count}"));
        })
    }

    fn note_user_message_to_session<'a>(
        &'a self,
        uuid: Uuid,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let permit = PtyManager::acquire_input_writer(&self.pty, uuid)
                .now_or_never()
                .expect("input permit released before note")
                .unwrap();
            drop(permit);
            self.log
                .lock()
                .unwrap()
                .push(format!("note:{}", String::from_utf8_lossy(data)));
        })
    }

    fn apply_coding_agent_profile_selection<'a>(
        &'a self,
        _session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        _pty_mgr: &'a Arc<Mutex<PtyManager>>,
        _settings: &'a SettingsState,
        _request: ApplyCodingAgentProfileSelectionRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<(ApplyCodingAgentProfileSelectionResult, Value), String>>
                + Send
                + 'a,
        >,
    > {
        panic!("unexpected apply_coding_agent_profile_selection")
    }

    fn discover_project<'a>(
        &'a self,
        _session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        _path: &'a str,
        _settings: &'a SettingsState,
    ) -> Pin<Box<dyn Future<Output = Result<AcDiscoveryResult, String>> + Send + 'a>> {
        panic!("unexpected discover_project")
    }

    fn open_project<'a>(
        &'a self,
        _settings: &'a SettingsState,
        _path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ProjectRegistration, String>> + Send + 'a>> {
        panic!("unexpected open_project")
    }

    fn remove_project<'a>(
        &'a self,
        _settings: &'a SettingsState,
        _path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        panic!("unexpected remove_project")
    }

    fn archive_project<'a>(
        &'a self,
        _settings: &'a SettingsState,
        _session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        _pty_mgr: &'a Arc<Mutex<PtyManager>>,
        _path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        panic!("unexpected archive_project")
    }

    fn unarchive_project<'a>(
        &'a self,
        _settings: &'a SettingsState,
        _path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ProjectRegistration, String>> + Send + 'a>> {
        panic!("unexpected unarchive_project")
    }

    fn agency_status<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        panic!("unexpected agency_status")
    }

    fn agency_update<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        panic!("unexpected agency_update")
    }
}

impl DesktopBroadcastEvents for TestOperations {
    fn desktop_event(&self, event: &str, _payload: &Value) {
        self.log.lock().unwrap().push(format!("event:{event}"));
    }
}

impl PtyBackend for TestBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        _spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        panic!("no process spawn in web fixture")
    }

    fn write(
        &self,
        _authority: &BackendWriteAuthority,
        _id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("write:{}", String::from_utf8_lossy(data)));
        if self.fail.load(Ordering::SeqCst) {
            Err(AppError::PtyError("fake write failure".into()))
        } else {
            Ok(())
        }
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
        Ok(())
    }

    fn kill(&self, _id: Uuid) -> Result<(), AppError> {
        Ok(())
    }

    fn has_session(&self, _id: Uuid) -> bool {
        true
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
        ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: std::path::PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

fn fixture(
    restoring: Option<bool>,
    purge: Option<Arc<crate::session::purge_guard::PurgeGuard>>,
) -> (WsState, Arc<TestOperations>, Arc<TestBackend>, Uuid) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let backend = Arc::new(TestBackend {
        log: log.clone(),
        fail: AtomicBool::new(false),
    });
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    let id = Uuid::new_v4();
    pty.lock()
        .unwrap()
        .try_record_route(id, SessionBackendKind::LocalProcess)
        .unwrap();
    let operations = Arc::new(TestOperations {
        log,
        pty: pty.clone(),
        restore: restoring
            .map(|active| Arc::new(crate::app_state::RestoreInProgress(AtomicBool::new(active)))),
        purge,
        create_fails: AtomicBool::new(false),
    });
    let state = WsState {
        session_mgr: Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
        pty_mgr: pty,
        settings: new_settings_state(AppSettings::default()),
        broadcaster: crate::web::broadcast::WsBroadcaster::new(),
        operations: operations.clone(),
        desktop_events: operations.clone(),
    };
    (state, operations, backend, id)
}

fn take_log(host: &TestOperations) -> Vec<String> {
    std::mem::take(&mut *host.log.lock().unwrap())
}

#[tokio::test]
async fn recognized_updates_including_malformed_args_bypass_restore_lookup() {
    let (state, host, _, _) = fixture(Some(true), None);
    let malformed = dispatch(&state, 7, "agent_update_answer", &json!({})).await;
    assert!(malformed["error"].as_str().unwrap().contains("command"));
    assert!(take_log(&host).is_empty());
    let status = dispatch(&state, 8, "get_agent_update_status", &json!({})).await;
    assert_eq!(status, json!({"id": 8, "error": "fake update gate"}));
    assert_eq!(take_log(&host), ["status"]);
    let answer = dispatch(
        &state,
        9,
        "agent_update_answer",
        &json!({"command": "codex", "enabled": true}),
    )
    .await;
    assert_eq!(answer, json!({"id": 9, "result": true}));
    assert_eq!(take_log(&host), ["answer:codex:true"]);
    let guarded = dispatch(&state, 10, "destroy_session", &json!({})).await;
    assert_eq!(
        guarded,
        json!({"id": 10, "error": "selectionCoordinatorBusy"})
    );
    assert_eq!(take_log(&host), ["restore"]);
}

#[tokio::test]
async fn malformed_browser_inputs_stop_before_lazy_dependencies() {
    let (state, host, _, _) = fixture(None, None);
    for (command, args) in [
        ("discover_project", json!({})),
        ("destroy_session", json!({"id": "not-a-uuid"})),
        ("update_settings", json!({"newSettings": 3})),
        ("save_settings_draft", json!({"draft": 3})),
        ("set_terminal_snapshots_enabled", json!({"expected": true})),
    ] {
        assert!(dispatch(&state, 1, command, &args)
            .await
            .get("error")
            .is_some());
        assert_eq!(take_log(&host), ["restore"], "{command}");
    }
}

#[tokio::test]
async fn binary_success_failure_and_invalid_frames_preserve_permit_boundaries() {
    let (state, host, backend, id) = fixture(None, None);
    let dispatcher = WebCommandsDispatcher::new(state);
    for data in [vec![0; 35], vec![255; 36], vec![b'x'; 36]] {
        dispatcher.binary(&data).await;
        assert!(take_log(&host).is_empty());
    }
    let frame = format!("{id}hello").into_bytes();
    dispatcher.binary(&frame).await;
    assert_eq!(
        take_log(&host),
        ["purge", "write:hello", "busy:5", "note:hello"]
    );
    backend.fail.store(true, Ordering::SeqCst);
    dispatcher.binary(&frame).await;
    assert_eq!(take_log(&host), ["purge", "write:hello"]);
    let missing = format!("{}hello", Uuid::new_v4());
    dispatcher.binary(missing.as_bytes()).await;
    assert!(
        take_log(&host).is_empty(),
        "no purge lookup without a writer"
    );
}

#[tokio::test]
async fn purge_rejects_binary_and_json_only_after_writer_acquisition() {
    let purge = Arc::new(crate::session::purge_guard::PurgeGuard::default());
    let (state, host, _, id) = fixture(None, Some(purge.clone()));
    let _lease = purge
        .acquire([id].into_iter().collect(), Default::default())
        .await;
    let dispatcher = WebCommandsDispatcher::new(state.clone());
    dispatcher.binary(format!("{id}hello").as_bytes()).await;
    assert_eq!(take_log(&host), ["purge"]);
    let reply = dispatch(
        &state,
        2,
        "pty_write",
        &json!({"sessionId": id, "data": [65]}),
    )
    .await;
    assert_eq!(
        reply,
        json!({"id": 2,
        "error": "purge-room in progress for this session; input rejected"})
    );
    assert_eq!(take_log(&host), ["restore", "purge"]);
    assert!(PtyManager::acquire_input_writer(&state.pty_mgr, id)
        .now_or_never()
        .expect("rejected write released permit")
        .is_ok());
}

#[tokio::test]
async fn create_preserves_typed_request_and_success_or_failure_envelope() {
    let (state, host, _, _) = fixture(None, None);
    let args = json!({"cwd": "C:/fixture", "shell": "fake-shell",
        "shellArgs": ["--fake"], "sessionName": "Browser"});
    let reply = dispatch(&state, 41, "create_session", &args).await;
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(reply["id"], 41);
    assert_eq!(reply["result"]["id"], "created");
    assert_eq!(take_log(&host), ["restore", "create"]);
    host.create_fails.store(true, Ordering::SeqCst);
    assert_eq!(
        dispatch(&state, 42, "create_session", &args).await,
        json!({"id": 42, "error": "fake create failure"})
    );
    assert_eq!(take_log(&host), ["restore", "create"]);
}

#[tokio::test]
async fn constructor_is_lazy_optional_absence_and_required_state_at_use() {
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let (fixture, _, _, _) = fixture(None, None);
    let state = WsState::new(
        fixture.session_mgr,
        fixture.pty_mgr,
        fixture.settings,
        fixture.broadcaster,
        app.handle().clone(),
    );
    assert!(state.operations.restore().is_none());
    assert!(state.operations.purge_guard().is_none());
    assert!(state.operations.response_close().is_none());
    assert_eq!(
        dispatch(&state, 3, "list_sessions", &json!({})).await,
        json!({"id": 3, "result": []})
    );
    for command in [
        "get_active_session",
        "drain_session_warnings",
        "list_detached_sessions",
    ] {
        let failure = std::panic::AssertUnwindSafe(dispatch(&state, 4, command, &json!({})))
            .catch_unwind()
            .await;
        assert!(
            failure.is_err(),
            "required state must still panic at use: {command}"
        );
    }
    let warnings = crate::session::warnings::new_session_warning_state();
    app.manage(warnings.clone());
    assert!(Arc::ptr_eq(state.operations.warnings(), &warnings));
    assert_eq!(
        dispatch(&state, 5, "drain_session_warnings", &json!({})).await,
        json!({"id": 5, "result": []})
    );
    let restore = Arc::new(crate::app_state::RestoreInProgress(AtomicBool::new(true)));
    app.manage(restore.clone());
    assert!(Arc::ptr_eq(state.operations.restore().unwrap(), &restore));
    assert_eq!(
        dispatch(&state, 6, "list_sessions", &json!({})).await,
        json!({"id": 6, "error": "selectionCoordinatorBusy"})
    );
    restore.0.store(false, Ordering::SeqCst);
    let tracker = crate::pty::manager::ResponseCloseState::default();
    app.manage(tracker);
    assert!(std::ptr::eq(
        state.operations.response_close().unwrap(),
        app.state::<crate::pty::manager::ResponseCloseState>()
            .inner()
    ));
}

#[test]
fn publication_order_isolated_process() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "commands::web::tests::publication_order_child",
            "--test-threads=1",
        ])
        .env("AC_P07_PUBLICATION_FIXTURE", &dir)
        .env("AGENTSCOMMANDER_CONFIG_DIR", &dir)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &dir)
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "publication child: {status}");
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("publication child timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[tokio::test]
async fn publication_order_child() {
    use tauri::Listener;
    let Some(path) = std::env::var_os("AC_P07_PUBLICATION_FIXTURE") else {
        return;
    };
    let path = std::path::PathBuf::from(path).canonicalize().unwrap();
    assert_eq!(
        crate::config::config_dir().unwrap().canonicalize().unwrap(),
        path
    );
    let app = crate::test_support::test_builder()
        .manage(crate::pty::manager::ResponseCloseState::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let (fixture, _, _, _) = fixture(None, None);
    let state = WsState::new(
        fixture.session_mgr,
        fixture.pty_mgr,
        fixture.settings,
        fixture.broadcaster,
        app.handle().clone(),
    );
    let initial = state.settings.read().await.clone();
    crate::config::settings::save_settings(&initial).unwrap();
    let mut next = initial;
    next.room_idle_confirmation_seconds = if next.room_idle_confirmation_seconds == 30 {
        60
    } else {
        30
    };
    let expected = next.room_idle_confirmation_seconds;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let callbacks = observed.clone();
    let settings = state.settings.clone();
    let file = path.join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
    app.listen_any("room_idle_confirmation_updated", move |_| {
        let published = settings
            .try_read()
            .expect("settings published before event");
        assert_eq!(published.room_idle_confirmation_seconds, expected);
        let disk: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(disk["roomIdleConfirmationSeconds"], expected);
        callbacks.lock().unwrap().push(expected);
    });
    let reply = dispatch(&state, 51, "update_settings", &json!({"newSettings": next})).await;
    assert_eq!(reply, json!({"id": 51, "result": null}));
    assert_eq!(*observed.lock().unwrap(), [expected]);
}
