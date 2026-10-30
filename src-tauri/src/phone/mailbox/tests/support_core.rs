use super::*;

pub(super) fn remote_activity_notice(
    kind: RemoteNoticeKind,
    base_branch: &str,
    behind_by: Option<u32>,
    blind_gap: Option<(String, String)>,
) -> Result<InternalSystemNotice, String> {
    InternalSystemNotice::for_remote_activity(
        kind,
        "mblua/AgentsCommander".to_string(),
        "feature/2083-2064-remote-alerts".to_string(),
        "abcdef1".to_string(),
        base_branch.to_string(),
        behind_by,
        "2026-09-15 23:18:43-03:00".to_string(),
        blind_gap,
    )
}

// (#885 E-3) Minimal mock PTY backend for purge-wg e2e tests. Sessions
// in `live` report `has_session: true`; `kill` removes them. Other
// methods are no-ops. This lets the purge gate exercise the F-3
// liveness predicate and the IdleDetector correlation end-to-end.
#[derive(Default)]
pub(super) struct MailboxMockPtyBackend {
    pub(super) live: std::sync::Mutex<HashSet<Uuid>>,
    pub(super) writes: std::sync::Mutex<Vec<(Uuid, Vec<u8>)>>,
    pub(super) fail_spawn: std::sync::atomic::AtomicBool,
    pub(super) fail_kill_once: std::sync::atomic::AtomicBool,
    pub(super) kill_attempts: std::sync::atomic::AtomicUsize,
    pub(super) fail_write: std::sync::atomic::AtomicBool,
    write_attempts: AtomicU32,
}

impl MailboxMockPtyBackend {
    pub(super) fn set_live(&self, id: Uuid) {
        self.live.lock().unwrap().insert(id);
    }

    pub(super) fn writes_for(&self, id: Uuid) -> Vec<Vec<u8>> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(session_id, _)| *session_id == id)
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }
}

impl PtyBackend for MailboxMockPtyBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        // #1163 M2 - test-only failure switch so a session-request poll can
        // exercise the rejected-result path end to end.
        if self.fail_spawn.load(Ordering::SeqCst) {
            return Box::pin(async move {
                Err(crate::errors::AppError::PtyError(
                    "synthetic spawn failure".to_string(),
                ))
            });
        }
        Box::pin(async move {
            self.live.lock().unwrap().insert(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        // #1883 — test-only failure switch so both mailbox injection sites
        // can exercise the real-error path end to end.
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_write.load(Ordering::SeqCst) {
            return Err(crate::errors::AppError::PtyError(
                "synthetic write failure".to_string(),
            ));
        }
        self.writes.lock().unwrap().push((id, data.to_vec()));
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.kill_attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_kill_once.swap(false, Ordering::SeqCst) {
            return Err(crate::errors::AppError::PtyError(
                "simulated fixture kill failure once".into(),
            ));
        }
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        Some((30, 120))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: std::path::PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().remove(&id)
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

/// (#885 E-3) Build a `PtyManager` with a mock backend whose live set
/// can be pre-populated. Sessions registered as "live" will pass the
/// F-3 `has_pty` liveness check in `handle_purge_wg`.
pub(super) fn make_test_pty_manager() -> (
    Arc<std::sync::Mutex<PtyManager>>,
    Arc<MailboxMockPtyBackend>,
) {
    let backend = Arc::new(MailboxMockPtyBackend::default());
    let mgr = Arc::new(std::sync::Mutex::new(PtyManager::new_for_test(
        backend.clone(),
    )));
    (mgr, backend)
}

// ── §224 D.3 — filter_sessions_by_fqn pure-predicate tests ──

pub(super) fn make_session_info(
    id: &str,
    name: &str,
    cwd: &str,
    status: crate::session::session::SessionStatus,
    waiting_for_input: bool,
) -> crate::session::session::SessionInfo {
    crate::session::session::SessionInfo {
        id: id.into(),
        name: name.into(),
        shell: "claude".into(),
        shell_args: vec![],
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: "2026-05-16T00:00:00Z".into(),
        working_directory: cwd.into(),
        status,
        waiting_for_input,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: None,
        agent_label: None,
        git_repos: vec![],
        workgroup_task: None,
        is_coordinator: false,
        is_root_agent: false,
        token: "t".into(),
        agent_kind: Some(crate::session::profile::CodingAgentKind::Claude),
        requested_profile: None,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        profile_outdated: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

pub(super) fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub(super) fn make_root_route_fixture(
    spoofed_coordinator_identity: bool,
) -> (tempfile::TempDir, Vec<String>) {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let origin_tech_lead = ac_root.join("_agent_tech-lead");
    let origin_dev_rust = ac_root.join("_agent_dev-rust");
    let wg_dir = ac_root.join("wg-1-dev-team");
    let tech_lead_replica = wg_dir.join("__agent_tech-lead");
    let dev_rust_replica = wg_dir.join("__agent_dev-rust");

    for dir in [
        &team_dir,
        &origin_tech_lead,
        &origin_dev_rust,
        &tech_lead_replica,
        &dev_rust_replica,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(
        team_dir.join("config.json"),
        concat!(
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"#,
            r#""coordinator":"../_agent_tech-lead"}"#,
        ),
    )
    .unwrap();
    let tech_lead_identity = if spoofed_coordinator_identity {
        "../../_agent_dev-rust"
    } else {
        "../../_agent_tech-lead"
    };
    std::fs::write(
        tech_lead_replica.join("config.json"),
        format!(r#"{{"identity":"{}"}}"#, tech_lead_identity),
    )
    .unwrap();
    std::fs::write(
        dev_rust_replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();

    let paths = vec![temp.path().to_string_lossy().to_string()];
    (temp, paths)
}

pub(super) fn root_outbox_message(body: String, command: Option<String>) -> OutboxMessage {
    OutboxMessage {
        id: "msg-root".into(),
        token: None,
        from: crate::config::root_agent::ROOT_AGENT_SENDER.into(),
        to: "proj-a:wg-1-dev-team/tech-lead".into(),
        body,
        mode: "wake".into(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: "auto".into(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-05-24T00:00:00Z".into(),
        command,
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

pub(super) fn wake_agent(id: &str, label: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.into(),
        id_hash: None,
        id_hash_debug: None,
        label: label.into(),
        command: command.into(),
        color: "#10b981".into(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

pub(super) fn wake_agents() -> Vec<AgentConfig> {
    vec![
        wake_agent("codex", "Codex", "codex --yolo"),
        wake_agent("claude", "Claude", "claude"),
    ]
}

pub(super) const MAILBOX_MASTER_TOKEN: &str = "mailbox-master-token";

pub(super) const CANONICAL_WAKE_FROM: &str = "proj-a:wg-1-dev-team/tech-lead";

pub(super) const CANONICAL_WAKE_TO: &str = "proj-a:wg-1-dev-team/dev-rust";

pub(super) const LOCAL_WAKE_FROM: &str = "wg-1-dev-team/tech-lead";

pub(super) const LOCAL_WAKE_TO: &str = "wg-1-dev-team/dev-rust";

pub(super) const WAKE_BODY: &str = "wake body";

pub(super) fn make_mailbox_app(projects_root: &Path) -> tauri::App<tauri::test::MockRuntime> {
    let owners = crate::app_state::AppOwners::new();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(
        SessionManager::with_activity_coalescer(owners.activity.clone()),
    ));

    let settings = AppSettings {
        project_paths: vec![projects_root.to_string_lossy().to_string()],
        agents: wake_agents(),
        telegram_bots: vec![crate::telegram::types::TelegramBotConfig {
            id: "bot-1".into(),
            label: "Bot 1".into(),
            token: "test-token".into(),
            chat_id: 1,
            color: "#10b981".into(),
        }],
        ..Default::default()
    };

    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let (pty_mgr, _) = make_test_pty_manager();

    tauri::test::mock_builder()
        .manage(owners)
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(Arc::new(AppLifetimeProbe))
        .manage(MasterToken::new(MAILBOX_MASTER_TOKEN.into()))
        .manage(AppOutbox::new(
            projects_root
                .join(".app-outbox")
                .to_string_lossy()
                .to_string(),
        ))
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(session_mgr.clone())
        .manage(pty_mgr) // #885 E-3: real PtyManager for pty_live probes
        .manage(Arc::new(tokio::sync::Mutex::new(
            TelegramBridgeManager::new(Arc::new(Mutex::new(HashMap::new()))),
        )))
        .manage(Arc::new(Mutex::new(HashSet::<Uuid>::new())))
        .manage(Arc::new(crate::RestoreInProgress(AtomicBool::new(false))))
        .manage(Arc::new(crate::PendingSelfClear::default()))
        .manage(idle_detector) // #885: purge_readiness
        .manage(std::sync::Arc::new(
            crate::session::purge_guard::PurgeGuard::default(),
        )) // #885
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mailbox test app")
}

pub(super) struct AppLifetimeProbe;

pub(super) fn app_handle(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> tauri::AppHandle<tauri::test::MockRuntime> {
    app.handle().clone()
}

pub(super) struct MailboxFixture {
    pub(super) _temp: tempfile::TempDir,
    pub(super) sender_cwd: PathBuf,
    pub(super) target_cwd: PathBuf,
    pub(super) app: tauri::App<tauri::test::MockRuntime>,
}

pub(super) fn make_mailbox_fixture() -> MailboxFixture {
    make_mailbox_fixture_in(tempfile::TempDir::new().unwrap())
}

pub(super) fn make_mailbox_fixture_in(temp: tempfile::TempDir) -> MailboxFixture {
    // (#1399 CI regression) Build every fixture path from the canonical
    // spelling. `dedup_outbox_dirs_by_object_id` rewrites each scanned
    // outbox dir to `verify_directory(...).canonical_path`, so the retry
    // ledger is keyed by the canonical form; on hosts whose `%TEMP%`
    // spelling differs from the on-disk canonical form (Windows 8.3 short
    // names or case variants, e.g. GitHub runners), a raw `temp.path()`
    // spelling makes every `retry_tracker.get(&source)` assertion miss.
    // Canonicalizing the root keeps fixture paths byte-identical to the
    // scan spelling on every host.
    let root = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(temp.path()).expect("canonicalize fixture temp"),
    );
    let project = root.join("proj-a");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let origin_tech_lead = ac_root.join("_agent_tech-lead");
    let origin_dev_rust = ac_root.join("_agent_dev-rust");
    let wg_dir = ac_root.join("wg-1-dev-team");
    let sender_cwd = wg_dir.join("__agent_tech-lead");
    let target_cwd = wg_dir.join("__agent_dev-rust");

    for dir in [
        &team_dir,
        &origin_tech_lead,
        &origin_dev_rust,
        &sender_cwd,
        &target_cwd,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(
        team_dir.join("config.json"),
        concat!(
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"#,
            r#""coordinator":"../_agent_tech-lead"}"#,
        ),
    )
    .unwrap();
    std::fs::write(
        sender_cwd.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .unwrap();
    std::fs::write(
        target_cwd.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();

    let app = make_mailbox_app(&root);
    MailboxFixture {
        _temp: temp,
        sender_cwd,
        target_cwd,
        app,
    }
}

pub(super) async fn add_mailbox_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &Path,
    name: &str,
    status: SessionStatus,
    telegram_bot_id: Option<&str>,
) -> Uuid {
    add_mailbox_session_with_shape(
        app,
        cwd,
        name,
        status,
        telegram_bot_id,
        "codex",
        Some("codex"),
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn add_mailbox_session_with_shape<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &Path,
    name: &str,
    status: SessionStatus,
    telegram_bot_id: Option<&str>,
    shell: &str,
    agent_id: Option<&str>,
    is_root: bool,
) -> Uuid {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(
            shell.into(),
            if shell == "codex" {
                vec!["--yolo".into()]
            } else {
                Vec::new()
            },
            cwd.to_string_lossy().to_string(),
            agent_id.map(str::to_string),
            agent_id.map(str::to_string),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    mgr.set_is_root_agent(session.id, is_root).await;
    mgr.rename_session(session.id, name.to_string())
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
    if let Some(bot_id) = telegram_bot_id {
        mgr.set_telegram_bot_id(session.id, Some(bot_id.to_string()))
            .await;
    }
    session.id
}

pub(super) async fn seed_shared_pty_engine_sessions(
    fixture: &MailboxFixture,
    target_status: SessionStatus,
) -> (crate::config::teams::VerifiedPtyInputRoute, Uuid, Uuid) {
    use crate::pty::backend::SessionBackendKind;

    let app = fixture.app.handle().clone();
    let sender_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "coordinator",
        SessionStatus::Running,
        None,
    )
    .await;
    let target_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "member",
        target_status.clone(),
        None,
    )
    .await;
    let route = crate::config::teams::verify_pty_input_route(
        &fixture.sender_cwd,
        false,
        CANONICAL_WAKE_TO,
        &[fixture._temp.path().to_string_lossy().to_string()],
    )
    .unwrap();
    let sender_cwd = crate::path_identity::verify_directory(&fixture.sender_cwd).unwrap();
    let target_cwd = crate::path_identity::verify_directory(&fixture.target_cwd).unwrap();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    {
        let manager = pty.lock().unwrap();
        manager
            .record_route_with_identities(
                sender_id,
                SessionBackendKind::LocalProcess,
                Some(sender_cwd),
                Some(route.sender.replica_identity.clone()),
            )
            .unwrap();
        manager
            .record_route_with_identities(
                target_id,
                SessionBackendKind::LocalProcess,
                Some(target_cwd),
                Some(route.target.replica_identity.clone()),
            )
            .unwrap();
        let backend = manager.backend_for_kind(SessionBackendKind::LocalProcess);
        let mock = backend
            .as_any()
            .downcast_ref::<MailboxMockPtyBackend>()
            .unwrap();
        mock.set_live(sender_id);
        mock.set_live(target_id);
    }
    if matches!(target_status, SessionStatus::Idle) {
        let idle = app
            .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
            .inner()
            .clone();
        idle.register_session(target_id, crate::session::profile::IdleTuning::DEFAULT);
        idle.set_pty_input_ready_for_test(target_id);
    }
    (route, sender_id, target_id)
}

pub(super) fn enqueue_shared_pty_engine_operation(
    store: &crate::api::message_store::MessageStore,
    route: &crate::config::teams::VerifiedPtyInputRoute,
    sender_id: Uuid,
    text: &[u8],
) -> String {
    let injection_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    store
        .enqueue_pty_input(crate::api::message_store::PtyInputEnqueueRequest {
            injection_id: injection_id.clone(),
            sender_fqn: route.sender.canonical_fqn.clone(),
            target_fqn: route.target.canonical_fqn.clone(),
            op_id: injection_id.clone(),
            nonce_sha256: crate::phone::types::sha256_hex(Uuid::new_v4().to_string().as_bytes()),
            request_fingerprint: crate::phone::types::sha256_hex(
                format!("shared-engine:{injection_id}").as_bytes(),
            ),
            confirmation_tag: Some("d".repeat(64)),
            requested_agent_id: None,
            payload: text.to_vec(),
            source_plane: crate::phone::types::PtyInputSourcePlane::HostCli,
            sender_incarnation_fingerprint: route.sender.incarnation_fingerprint.clone(),
            sender_identity_fingerprint: route.sender.authority_fingerprint.clone(),
            target_identity_fingerprint: route.target.authority_fingerprint.clone(),
            authority_session_id: sender_id.to_string(),
            authority_client_id: None,
            authority_client_generation: None,
            issued_at: crate::phone::types::canonical_pty_timestamp(now),
            expires_at: crate::phone::types::canonical_pty_timestamp(
                now + chrono::Duration::minutes(10),
            ),
        })
        .unwrap();
    injection_id
}

/// P05 preparation uses the existing mailbox fixture and the real
/// conditional-close worker. Only tracker time/idle publishers are fixture
/// stimuli; the retired carrier is minted by actual settlement.
pub(super) async fn prepare_retained_exited_respawn(
    fixture: &MailboxFixture,
) -> (
    crate::session::selection::SelectionCoordinator,
    crate::config::teams::VerifiedPtyInputRoute,
    Uuid,
    Uuid,
) {
    let app = fixture.app.handle();
    let tracker = crate::pty::manager::ResponseCloseState::default();
    let managers = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    *managers.write().await = SessionManager::with_response_close(
        tracker.clone(),
        app.state::<crate::app_state::AppOwners>().activity.clone(),
    );
    app.state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap()
        .bind_response_close(tracker.clone());
    app.manage(tracker.clone());
    app.manage(Arc::new(
        crate::resource_monitor::ResourceMonitorState::new(),
    ));
    app.manage(crate::web::broadcast::WsBroadcaster::new());
    app.manage(crate::pty::input_activity::new_state());
    app.manage(crate::pty::input_activity::new_typing_hold_state());
    app.manage(Arc::new(crate::capture::registry::CaptureRegistry::new()));
    app.manage(crate::network::OutboundNetwork::new_for_tests(1));
    app.manage(crate::session::warnings::new_session_warning_state());
    app.manage(Arc::new(crate::pty::menu_guard::MenuGuard::new()));
    let gate = crate::api::message_store::PtyInputTargetGateState::for_root(
        fixture._temp.path().to_path_buf(),
    );
    let store = Arc::new(
        crate::api::message_store::MessageStore::open(
            fixture._temp.path().join("retained-respawn.sqlite3"),
        )
        .unwrap(),
    );
    app.manage(
        crate::api::message_store::MessageStoreState::with_store_and_target_gate(
            Ok(store),
            gate.gate.clone(),
        ),
    );
    app.manage(gate);
    let coordinator =
        crate::session::selection::SelectionCoordinator::with_fixture_root_reservation(
            managers.inner().clone(),
            tokio_util::sync::CancellationToken::new(),
            crate::session::selection::FixtureRootReservation::Normal65,
        );
    app.manage(coordinator.clone());
    coordinator.start(app.clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();
    coordinator.wait_for_restore_release().await.unwrap();
    let (route, sender, previous) =
        seed_shared_pty_engine_sessions(fixture, SessionStatus::Idle).await;
    retire_retained_fixture_member(fixture, &coordinator, &route, sender, previous).await;
    (coordinator, route, sender, previous)
}

/// Owns transport for the Tauri-free Codex clear recovery tests.
pub(crate) struct CodexClearScannerFixture {
    app: tauri::App<tauri::test::MockRuntime>,
    hooks: crate::phone::mailbox::pty_claim::MailboxTestHooks,
    delivered_events: Arc<Mutex<usize>>,
}

impl CodexClearScannerFixture {
    pub(crate) fn new(outbox: &Path, backend: Arc<dyn PtyBackend>) -> Self {
        let owners = crate::app_state::AppOwners::new();
        let sessions = SessionManager::with_activity_coalescer(owners.activity.clone());
        let app = tauri::test::mock_builder()
            .manage(owners)
            .manage(crate::config::settings::new_settings_state(
                Default::default(),
            ))
            .manage(Arc::new(tokio::sync::RwLock::new(sessions)))
            .manage(Arc::new(Mutex::new(PtyManager::new_for_test(backend))))
            .manage(AppOutbox::new(outbox.to_string_lossy().into_owned()))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let delivered_events = Arc::new(Mutex::new(0));
        let recorded = delivered_events.clone();
        tauri::Listener::listen_any(&app, "message_delivered", move |_| {
            *recorded.lock().unwrap() += 1;
        });
        Self {
            app,
            hooks: Default::default(),
            delivered_events,
        }
    }

    pub(crate) fn poller(&self) -> MailboxPoller {
        MailboxPoller::new_with_test_hooks(self.hooks.clone())
    }

    pub(crate) async fn poll(&self, poller: &mut MailboxPoller) -> Result<(), String> {
        poller.poll(self.app.handle()).await
    }

    pub(crate) async fn deliver_claimed(
        &self,
        poller: &MailboxPoller,
        claim: &Path,
        origin: &Path,
        validated: &crate::phone::mailbox::poller::ValidatedWake,
    ) -> Result<(), String> {
        crate::phone::mailbox::poller::deliver_claimed_wake(
            poller,
            self.app.handle(),
            claim,
            origin,
            validated,
        )
        .await
    }

    pub(crate) async fn publish_delivered(
        &self,
        poller: &MailboxPoller,
        source: &Path,
        message: &OutboxMessage,
    ) -> Result<(), String> {
        poller
            .move_to_delivered(self.app.handle(), source, message)
            .await
    }

    pub(crate) fn install_authentication(&self, token: &str) {
        self.app.manage(MasterToken::new(token.into()));
    }

    /// Spawn, injection, hook event, follow-up and delivered event counts.
    pub(crate) fn recorded_effect_counts(&self) -> [usize; 5] {
        [
            self.hooks.spawn_calls.lock().unwrap().len(),
            self.hooks.inject_calls.lock().unwrap().len(),
            self.hooks.events.lock().unwrap().len(),
            self.hooks.internal_bookkeeping.lock().unwrap().len(),
            *self.delivered_events.lock().unwrap(),
        ]
    }
}

/// Fixture-owned barrier records the dormant adapter without activating PTY detection.
pub(crate) struct ClearObservationProbe {
    demote: bool,
    owned_children: AtomicU32,
    sources: Mutex<Vec<PathBuf>>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

impl ClearObservationProbe {
    pub(crate) fn record_owned_child(&self, child: &MailboxPoller) {
        assert!(child.codex_clear_source.is_none());
        assert!(child.clear_reservation.is_none());
        self.owned_children.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) async fn enter(&self, delivery: &MailboxPoller) {
        let source = delivery
            .codex_clear_source
            .clone()
            .expect("worker claim source");
        let child = delivery.delivery_poller();
        assert!(child.codex_clear_source.is_none());
        assert!(child.clear_reservation.is_none());
        if self.demote {
            delivery.begin_clear_observation();
            delivery.begin_clear_observation();
        }
        self.sources.lock().unwrap().push(source);
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
    }

    pub(crate) async fn wait_for(&self, count: usize) -> Vec<PathBuf> {
        loop {
            let entered = self.entered.notified();
            let sources = self.sources.lock().unwrap().clone();
            if sources.len() >= count {
                return sources;
            }
            entered.await;
        }
    }

    pub(crate) fn release(&self, count: usize) {
        self.release.add_permits(count);
    }
}

impl CodexClearScannerFixture {
    pub(crate) fn observe_deliveries(
        &self,
        poller: &mut MailboxPoller,
        demote: bool,
    ) -> Arc<ClearObservationProbe> {
        let probe = Arc::new(ClearObservationProbe {
            demote,
            owned_children: AtomicU32::new(0),
            sources: Mutex::new(Vec::new()),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        poller.clear_observation_probe = Some(probe.clone());
        probe
    }

    pub(crate) fn claim(
        &self,
        poller: &mut MailboxPoller,
        source: &Path,
        message: OutboxMessage,
    ) -> WakeHandoff {
        poller.claim_and_spawn_wake(self.app.handle(), source, message)
    }

    pub(crate) async fn seed_session(&self, cwd: &Path, exited: bool) -> Uuid {
        let id = add_mailbox_session(
            self.app.handle(),
            cwd,
            "clear-fixture",
            if exited {
                SessionStatus::Exited(0)
            } else {
                SessionStatus::Idle
            },
            None,
        )
        .await;
        self.hooks.pty_presence.lock().unwrap().insert(id, !exited);
        id
    }

    pub(crate) fn injection_error(&self, error: &str) {
        self.hooks
            .inject_results
            .lock()
            .unwrap()
            .push_back(Err(error.into()));
    }

    pub(crate) fn panic_on_inject(&self, id: Uuid) {
        self.hooks.panic_on_inject.lock().unwrap().insert(id);
    }

    pub(crate) async fn inline(
        &self,
        poller: &MailboxPoller,
        source: &Path,
        message: OutboxMessage,
        comanaged: bool,
    ) -> Result<(), String> {
        let content = serde_json::to_string(&message).unwrap();
        poller
            .deliver_outbox_tail(
                self.app.handle(),
                source,
                message,
                &content,
                (
                    if comanaged {
                        OutboxOrigin::CoManaged
                    } else {
                        OutboxOrigin::Master
                    },
                    None,
                ),
                (true, false, false, false, false),
            )
            .await
    }
}

impl CodexClearScannerFixture {
    pub(crate) async fn configure_project(&self, root: &Path) {
        let state = self.app.state::<crate::config::settings::SettingsState>();
        let mut settings = state.write().await;
        settings.project_paths = vec![root.to_string_lossy().into_owned()];
        settings.agents = wake_agents();
    }
}

impl CodexClearScannerFixture {
    pub(crate) async fn owned_clear_exclusion(temp: tempfile::TempDir) -> (bool, u32, bool, u32) {
        let fixture = make_mailbox_fixture_in(temp);
        let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
        let app = fixture.app.handle();
        let control = Arc::new(crate::session::selection::ReplacementFixtureControl::default());
        control.mock_backend.store(true, Ordering::Release);
        control.ticket_only.store(false, Ordering::Release);
        let hooks = MailboxTestHooks::default();
        *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
        let mut poller = MailboxPoller::new_with_test_hooks(hooks);
        let probe = Arc::new(ClearObservationProbe {
            demote: true,
            owned_children: AtomicU32::new(0),
            sources: Mutex::new(Vec::new()),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        poller.clear_observation_probe = Some(probe.clone());
        let mut msg = wake_message_to_target();
        msg.command = Some("clear".into());
        let source = fixture._temp.path().join(format!("{}.json", msg.id));
        std::fs::write(&source, serde_json::to_vec(&msg).unwrap()).unwrap();
        poller.codex_clear_source = Some(source.clone());
        let pty = app.state::<Arc<Mutex<PtyManager>>>();
        let backend = pty
            .lock()
            .unwrap()
            .backend_for_kind(SessionBackendKind::LocalProcess);
        backend
            .as_any()
            .downcast_ref::<MailboxMockPtyBackend>()
            .unwrap()
            .fail_write
            .store(true, Ordering::Release);
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            retained_fixture_ordinary_wake(&poller, app, &msg, &control),
        )
        .await;
        coordinator.shutdown_token().cancel();
        tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
            .await
            .unwrap();
        let pending = result.expect("bounded owned clear tail").is_none();
        let no_marker =
            super::super::clear_attempt::probe_attempt(&std::fs::read_to_string(&source).unwrap())
                .unwrap()
                .is_none();
        assert!(probe.sources.lock().unwrap().is_empty());
        assert!(!source.parent().unwrap().join("delivered").exists());
        (
            pending,
            probe.owned_children.load(Ordering::SeqCst),
            no_marker,
            backend
                .as_any()
                .downcast_ref::<MailboxMockPtyBackend>()
                .unwrap()
                .write_attempts
                .load(Ordering::SeqCst),
        )
    }
}
