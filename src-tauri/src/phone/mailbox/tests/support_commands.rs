use super::*;

pub(super) async fn seed_raise_hand_session(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    cwd: &Path,
    is_coordinator: bool,
    status: SessionStatus,
) -> (Uuid, Uuid) {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(
            "codex".into(),
            vec![],
            cwd.to_string_lossy().to_string(),
            Some("codex".into()),
            Some("Codex".into()),
            Vec::new(),
            is_coordinator,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    match status {
        SessionStatus::Active => {
            mgr.switch_session(session.id).await.unwrap();
        }
        SessionStatus::Running => {}
        SessionStatus::Idle => {
            mgr.mark_idle(session.id).await;
        }
        SessionStatus::Exited(code) => {
            mgr.mark_exited(session.id, code).await;
        }
    }
    (session.id, session.token)
}

pub(super) fn write_raise_hand_task_title(cwd: &Path, title: &str) {
    let task_path =
        crate::session::session::find_workgroup_task_path_for_cwd(&cwd.to_string_lossy())
            .expect("raise-hand test cwd should be inside a workgroup");
    std::fs::write(task_path, format!("---\ntitle: {}\n---\n\nbody", title)).unwrap();
}

pub(super) fn build_raise_hand_message(
    cwd: &Path,
    msg_id: &str,
    request_id: &str,
    token: Option<String>,
) -> (PathBuf, OutboxMessage) {
    let outbox_dir = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let path = outbox_dir.join(format!("{}.json", msg_id));
    let msg = OutboxMessage {
        id: msg_id.into(),
        token,
        from: sender_name_for_session_cwd(&cwd.to_string_lossy()),
        to: String::new(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.into()),
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-06-28T00:00:00Z".into(),
        command: None,
        action: Some(RAISE_HAND_ACTION.into()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };
    std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
    (path, msg)
}

pub(super) fn read_self_clear_response_status(cwd: &Path, request_id: &str) -> Option<String> {
    let v = read_response_json(cwd, request_id)?;
    v.get("status").and_then(|s| s.as_str()).map(String::from)
}

pub(super) async fn pending_self_clear_len(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
) -> usize {
    let pending = app.state::<Arc<crate::PendingSelfClear>>();
    let g = pending.0.lock().unwrap_or_else(|e| e.into_inner());
    g.len()
}

pub(super) async fn pending_self_clear_contains(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    id: Uuid,
) -> bool {
    let pending = app.state::<Arc<crate::PendingSelfClear>>();
    let g = pending.0.lock().unwrap_or_else(|e| e.into_inner());
    g.contains(&id)
}

/// Build a self-clear outbox message with an explicit `from`, written under
/// `<cwd>/<local-dir>/outbox/`. The canonical OutboxMessage literal for the
/// self-clear tests lives here; `build_self_clear_message` delegates with the
/// fixed dev-rust sender, and the Root e2e tests pass either the corrected
/// sender (ROOT_AGENT_SENDER) or the buggy path-derived value. Returns the
/// on-disk path and the message; the `process_message` driver reads the message
/// back from disk and ignores the returned struct.
pub(super) fn build_self_clear_message_with_from(
    cwd: &Path,
    msg_id: &str,
    request_id: &str,
    token: Option<String>,
    from: &str,
) -> (PathBuf, OutboxMessage) {
    let outbox_dir = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let path = outbox_dir.join(format!("{}.json", msg_id));
    let msg = OutboxMessage {
        id: msg_id.into(),
        token,
        from: from.into(),
        to: String::new(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.into()),
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-06-24T00:00:00Z".into(),
        command: None,
        action: Some(SELF_CLEAR_ACTION.into()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };
    std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
    (path, msg)
}

pub(super) struct SelfSwitchFixture {
    pub(super) _temp: tempfile::TempDir,
    pub(super) replica: PathBuf,
    pub(super) _origin: PathBuf,
    pub(super) app: tauri::App<tauri::test::MockRuntime>,
}

pub(super) fn make_self_switch_fixture() -> SelfSwitchFixture {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let origin = ac_root.join("_agent_dev-rust");
    let wg_dir = ac_root.join("wg-1-dev-team");
    let replica = wg_dir.join("__agent_dev-rust");

    std::fs::create_dir_all(&origin).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();

    let app = make_mailbox_app(temp.path());
    SelfSwitchFixture {
        _temp: temp,
        replica,
        _origin: origin,
        app,
    }
}

pub(super) async fn seed_self_switch_session(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    cwd: &Path,
    shell: &str,
    agent_id: Option<&str>,
    requested_profile: Option<&str>,
    effective_profile: Option<&str>,
) -> (Uuid, Uuid) {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(
            shell.into(),
            vec![],
            cwd.to_string_lossy().to_string(),
            agent_id.map(str::to_string),
            agent_id.map(|id| format!("Label for {}", id)),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    mgr.set_profile_metadata(
        session.id,
        requested_profile.map(str::to_string),
        effective_profile.map(str::to_string),
        Vec::new(),
        false,
        None,
        None,
        None,
        None,
    )
    .await;
    (session.id, session.token)
}

pub(super) fn build_self_switch_message(
    cwd: &Path,
    msg_id: &str,
    request_id: &str,
    token: Option<String>,
    coding_agent: Option<&str>,
    profile: Option<&str>,
    from: &str,
) -> (PathBuf, OutboxMessage) {
    let outbox_dir = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let path = outbox_dir.join(format!("{}.json", msg_id));
    let msg = OutboxMessage {
        id: msg_id.into(),
        token,
        from: from.into(),
        to: String::new(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.into()),
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-06-28T00:00:00Z".into(),
        command: None,
        action: Some(SELF_SWITCH_ACTION.into()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: coding_agent.map(str::to_string),
        switch_profile: profile.map(str::to_string),
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };
    std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
    (path, msg)
}

pub(super) fn read_reject_reason(cwd: &Path, msg_id: &str) -> Option<String> {
    std::fs::read_to_string(
        cwd.join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("rejected")
            .join(format!("{}.reason.txt", msg_id)),
    )
    .ok()
}

/// #1632 - sibling of `build_self_clear_message_with_from`. The landed helper hardcodes
/// `action: Some(SELF_CLEAR_ACTION)`, so it cannot be reused and must not be edited.
pub(super) fn build_self_restart_message_with_from(
    cwd: &Path,
    msg_id: &str,
    request_id: &str,
    token: Option<String>,
    from: &str,
) -> (PathBuf, OutboxMessage) {
    let outbox_dir = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let path = outbox_dir.join(format!("{}.json", msg_id));
    let msg = OutboxMessage {
        id: msg_id.into(),
        token,
        from: from.into(),
        to: String::new(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.into()),
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-08-31T00:00:00Z".into(),
        command: None,
        action: Some(SELF_RESTART_ACTION.into()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };
    std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
    (path, msg)
}

pub(super) const SELF_RESTART_TEST_FROM: &str = "proj-a:wg-1-dev-team/dev-rust";

// ── (#1001 PR1) consumption-verdict wiring: AC3 deterministic hooked tests ──
//
// These prove the shared `verdict_to_result` (G6) is what
// `inject_wake_into_pty` runs, so the conversion A (PR3) changes has real,
// deterministic coverage instead of a hook-local copy. A live Idle candidate
// takes the WakeAction::Inject arm; the inject itself is scripted Ok, and the
// scripted ConsumptionVerdict drives the returned Result.

/// Build a well-formed wake OutboxMessage targeting the fixture's dev-rust
/// replica. `deliver_wake_with_origin` does not re-validate token/routing
/// (that is `process_message`'s job), so a direct call is the tightest way to
/// assert the Ok/Err the verdict produces.
pub(super) fn wake_message_to_target() -> OutboxMessage {
    OutboxMessage {
        id: "consume-verdict".into(),
        token: None,
        from: CANONICAL_WAKE_FROM.into(),
        to: CANONICAL_WAKE_TO.into(),
        body: WAKE_BODY.into(),
        mode: "wake".into(),
        get_output: false,
        request_id: None,
        sender_agent: Some("codex".into()),
        preferred_agent: "codex".into(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-07-15T00:00:00Z".into(),
        command: None,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

pub(super) fn logical_command_message(command: &str, body: &str) -> OutboxMessage {
    let mut message = wake_message_to_target();
    message.id = format!("logical-{command}");
    message.command = Some(command.to_string());
    message.body = body.to_string();
    message
}

pub(super) fn write_logical_command_message(
    sender_cwd: &Path,
    msg_id: &str,
    command: &str,
    body: &str,
) -> PathBuf {
    let outbox_dir = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let path = outbox_dir.join(format!("{msg_id}.json"));
    let mut message = logical_command_message(command, body);
    message.id = msg_id.to_string();
    message.token = Some(MAILBOX_MASTER_TOKEN.to_string());
    std::fs::write(&path, serde_json::to_string_pretty(&message).unwrap()).unwrap();
    path
}

/// (#1399) A plain wake message (no logical command) written into the
/// sender's outbox with the master token, as `poll()` finds it.
pub(super) fn write_wake_message(sender_cwd: &Path, msg_id: &str, timestamp: &str) -> PathBuf {
    let outbox_dir = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let path = outbox_dir.join(format!("{msg_id}.json"));
    let mut message = wake_message_to_target();
    message.id = msg_id.to_string();
    message.token = Some(MAILBOX_MASTER_TOKEN.to_string());
    message.timestamp = timestamp.to_string();
    std::fs::write(&path, serde_json::to_string_pretty(&message).unwrap()).unwrap();
    path
}

/// A dispatch-flagged wake targeting the fixture's dev-rust replica: a
/// named agent plus an optional requested profile letter.
pub(super) fn dispatch_wake_message(agent: &str, profile: Option<&str>) -> OutboxMessage {
    let mut message = wake_message_to_target();
    message.id = format!("dispatch-{}", Uuid::new_v4());
    message.preferred_agent = agent.to_string();
    message.requested_profile = profile.map(str::to_string);
    message
}

// ──────────────────────────────────────────────────────────────────────
// #1163 session-request result sidecars (M1-M5)
// ──────────────────────────────────────────────────────────────────────

pub(super) struct SessionRequestFixture {
    pub(super) _temp: tempfile::TempDir,
    pub(super) cwd: PathBuf,
    pub(super) app: tauri::App<tauri::test::MockRuntime>,
    pub(super) backend: Arc<MailboxMockPtyBackend>,
}

/// Build an app that can run the FULL session-request create path
/// (`poll_session_requests` → `create_session_inner`): the mailbox states
/// plus the selection coordinator, resource monitor, target-gate store and
/// related states the create pipeline requires. The cwd is a plain temp
/// dir (not a WG replica), so no target-gate / sessionRace machinery
/// participates and the create either succeeds or fails purely on the PTY
/// backend.
pub(super) fn make_session_request_fixture() -> SessionRequestFixture {
    let temp = tempfile::TempDir::new().unwrap();
    let cwd = temp.path().join("session-cwd");
    std::fs::create_dir_all(&cwd).unwrap();

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (pty_mgr, backend) = make_test_pty_manager();
    let settings = AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        agents: wake_agents(),
        ..Default::default()
    };
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        shutdown.token().clone(),
    );
    let output_senders = Arc::new(Mutex::new(HashMap::new()));
    let telegram: crate::telegram::manager::TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(
            crate::telegram::manager::TelegramBridgeManager::new(output_senders),
        ));
    let store_dir = tempfile::TempDir::new().expect("create target-gate store");
    let message_store = Arc::new(
        crate::api::message_store::MessageStore::open(
            store_dir
                .path()
                .join(crate::api::message_store::DB_FILENAME),
        )
        .expect("open target-gate store"),
    );
    let target_gate_state = crate::api::message_store::PtyInputTargetGateState::for_root(
        store_dir.path().to_path_buf(),
    );
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let app = tauri::test::mock_builder()
        .manage(MasterToken::new(MAILBOX_MASTER_TOKEN.into()))
        .manage(AppOutbox::new(
            temp.path()
                .join(".app-outbox")
                .to_string_lossy()
                .to_string(),
        ))
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(session_mgr.clone())
        .manage(pty_mgr)
        .manage(telegram)
        .manage(Arc::new(crate::RestoreInProgress(AtomicBool::new(false))))
        .manage(Arc::new(crate::PendingSelfClear::default()))
        .manage(idle_detector)
        .manage(std::sync::Arc::new(
            crate::session::purge_guard::PurgeGuard::default(),
        ))
        .manage(Arc::new(
            crate::resource_monitor::ResourceMonitorState::new(),
        ))
        .manage(coordinator.clone())
        .manage(target_gate_state.clone())
        .manage(shutdown)
        .manage(crate::DetachedSessionsState::default())
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(
            crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                Ok(message_store),
                target_gate_state.gate.clone(),
            ),
        )
        .manage(store_dir)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build session request test app");
    coordinator
        .start(app.handle().clone())
        .expect("start coordinator");
    let bootstrap = coordinator.clone();
    std::thread::spawn(move || {
        tauri::async_runtime::block_on(async move {
            bootstrap
                .submit_restore_first()
                .await
                .expect("open coordinator")
                .finish();
        });
    })
    .join()
    .expect("join coordinator bootstrap");

    SessionRequestFixture {
        _temp: temp,
        cwd,
        app,
        backend,
    }
}

pub(super) fn write_session_request_for_test(
    cwd: &Path,
    agent_id: &str,
) -> crate::cli::create_agent::SessionRequest {
    let request = crate::cli::create_agent::SessionRequest {
        id: uuid::Uuid::new_v4().to_string(),
        cwd: cwd.to_string_lossy().to_string(),
        session_name: "session-request-m-test".to_string(),
        agent_id: agent_id.to_string(),
        shell: "codex".to_string(),
        shell_args: vec!["--yolo".to_string()],
        requested_profile: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    crate::cli::create_agent::write_session_request(&request).expect("write request");
    request
}

pub(super) fn session_request_file_for(id: &str) -> PathBuf {
    crate::config::config_dir()
        .expect("config dir")
        .join(SESSION_REQUESTS_DIR_NAME)
        .join(format!("{id}.json"))
}

pub(super) fn session_request_result_file_for(id: &str) -> PathBuf {
    crate::config::config_dir()
        .expect("config dir")
        .join(SESSION_REQUESTS_DIR_NAME)
        .join(format!("{id}.result.json"))
}

pub(super) fn read_session_request_result_json(id: &str) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(session_request_result_file_for(id)).expect("read result"),
    )
    .expect("result json")
}

// ── (#885) evaluate_gate pure function tests ──

pub(super) fn make_readiness(
    session_id: Uuid,
    activity_age_ms: Option<u64>,
    watcher_idle: bool,
    last_resize_age_ms: Option<u64>,
) -> crate::pty::idle_detector::PurgeReadiness {
    crate::pty::idle_detector::PurgeReadiness {
        session_id,
        activity_age: activity_age_ms.map(Duration::from_millis),
        watcher_idle,
        last_resize_age: last_resize_age_ms.map(Duration::from_millis),
        resize_grace: Duration::from_millis(3000),
        idle_threshold: Duration::from_millis(2500),
        silence_age: None,
    }
}

pub(super) fn make_gate_peer(
    fqn: &str,
    live: Vec<PurgeGateSession>,
    all_session_ids: Vec<String>,
) -> PurgeGatePeer {
    PurgeGatePeer {
        fqn: fqn.to_string(),
        all_session_ids,
        live,
    }
}
