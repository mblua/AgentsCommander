use super::*;
use crate::config::loops::{
    apply_loop_update_patch, write_loop_config, write_loop_state_atomic, LoopSessionStart,
    MissedWhileClosedPolicy,
};
use tauri::Manager;

/// A stored Loop whose `sessionStart` is `Accumulate`, so an update that
/// leaves the field alone is distinguishable from one that defaults it.
fn accumulate_config() -> LoopConfigToml {
    LoopConfigToml {
        loop_def: LoopDef {
            id: "daily-sync".to_string(),
            name: "Daily sync".to_string(),
            enabled: true,
        },
        trigger: LoopTrigger {
            kind: LoopTriggerKind::Cron,
            expr: "0 9 * * *".to_string(),
            timezone: LOOP_TIMEZONE_LOCAL.to_string(),
        },
        target: LoopTarget {
            kind: LoopTargetKind::WorkgroupCoordinator,
            workgroup: "wg-1-dev-team".to_string(),
        },
        prompt: LoopPrompt {
            body: "Send status".to_string(),
        },
        policy: LoopPolicy {
            session_start: LoopSessionStart::Accumulate,
            ..LoopPolicy::default()
        },
    }
}

/// AC-1 - a create payload from a frontend that predates this phase omits
/// the key entirely and must still deserialize, defaulting to `Fresh`.
#[test]
fn create_request_without_session_start_defaults_to_fresh() {
    let payload = include_str!("tests/fixtures/request-1.json");
    assert!(
        !payload.contains("sessionStart"),
        "the legacy payload must not carry the key, or this test passes for the wrong reason"
    );

    let request: LoopCreateRequest =
        serde_json::from_str(payload).expect("legacy create payload deserializes");
    assert_eq!(request.session_start, None);
    assert_eq!(
        policy_from_create_request(&request).session_start,
        LoopSessionStart::Fresh
    );
}

/// AC-2 - an explicit choice on create is carried into the policy.
#[test]
fn create_request_with_accumulate_persists_accumulate() {
    let payload = include_str!("tests/fixtures/request-2.json");

    let request: LoopCreateRequest =
        serde_json::from_str(payload).expect("create payload deserializes");
    assert_eq!(request.session_start, Some(LoopSessionStart::Accumulate));
    assert_eq!(
        policy_from_create_request(&request).session_start,
        LoopSessionStart::Accumulate
    );
}

/// AC-3 - an update that never mentions the field cannot rewrite it.
#[test]
fn update_request_without_session_start_leaves_the_stored_value() {
    let payload = r#"{"projectPath":"/tmp/project","id":"daily-sync","name":"Renamed sync"}"#;
    assert!(!payload.contains("sessionStart"));

    let request: LoopUpdateRequest =
        serde_json::from_str(payload).expect("legacy update payload deserializes");
    let patch = patch_from_update_request(request);
    assert_eq!(patch.session_start, None);

    let mut config = accumulate_config();
    apply_loop_update_patch(&mut config, patch).expect("patch applies");
    assert_eq!(config.policy.session_start, LoopSessionStart::Accumulate);
}

/// AC-3b - an explicit `null` is epic 5.1's normal frontend shape and must
/// behave exactly like an absent key, not like a default.
#[test]
fn update_request_with_null_session_start_leaves_the_stored_value() {
    let payload = r#"{"projectPath":"/tmp/project","id":"daily-sync","sessionStart":null}"#;

    let request: LoopUpdateRequest =
        serde_json::from_str(payload).expect("null update payload deserializes");
    let patch = patch_from_update_request(request);
    assert_eq!(patch.session_start, None);

    let mut config = accumulate_config();
    apply_loop_update_patch(&mut config, patch).expect("patch applies");
    assert_eq!(config.policy.session_start, LoopSessionStart::Accumulate);
}

/// AC-4 - an explicit choice on update changes the stored value.
#[test]
fn update_request_with_fresh_flips_a_stored_accumulate() {
    let payload = r#"{"projectPath":"/tmp/project","id":"daily-sync","sessionStart":"fresh"}"#;

    let request: LoopUpdateRequest =
        serde_json::from_str(payload).expect("update payload deserializes");
    let patch = patch_from_update_request(request);
    assert_eq!(patch.session_start, Some(LoopSessionStart::Fresh));

    let mut config = accumulate_config();
    apply_loop_update_patch(&mut config, patch).expect("patch applies");
    assert_eq!(config.policy.session_start, LoopSessionStart::Fresh);
}

/// AC-5 - an unknown value is a serde error surfaced as the existing
/// command error. No new error type and no new message.
#[test]
fn unknown_session_start_value_fails_to_deserialize() {
    let create = include_str!("tests/fixtures/request-3.json");
    assert!(serde_json::from_str::<LoopCreateRequest>(create).is_err());

    let update = r#"{"projectPath":"/tmp/project","id":"daily-sync","sessionStart":"resume"}"#;
    assert!(serde_json::from_str::<LoopUpdateRequest>(update).is_err());
}

/// AC-6 (zero-effect) - the extraction is a move. For a request that never
/// mentions `sessionStart`, both functions reproduce the pre-P2 inline
/// literals field by field.
#[test]
fn extracted_mappings_reproduce_the_pre_phase_literals() {
    let create_payload = include_str!("tests/fixtures/request-4.json");
    let create: LoopCreateRequest =
        serde_json::from_str(create_payload).expect("create payload deserializes");
    let policy = policy_from_create_request(&create);
    assert_eq!(policy.busy_coordinator, BusyCoordinatorPolicy::ForceInject);
    assert_eq!(policy.missed_while_closed, MissedWhileClosedPolicy::Notify);
    // `enabled` is read by the handler, not by the policy mapping.
    assert_eq!(create.enabled, Some(false));

    let update_payload = include_str!("tests/fixtures/request-5.json");
    let update: LoopUpdateRequest =
        serde_json::from_str(update_payload).expect("update payload deserializes");
    let patch = patch_from_update_request(update);
    assert_eq!(patch.name.as_deref(), Some("Renamed sync"));
    assert_eq!(patch.expr.as_deref(), Some("30 9 * * *"));
    assert_eq!(patch.workgroup.as_deref(), Some("wg-2-dev-team"));
    assert_eq!(patch.prompt_body.as_deref(), Some("Summarize status"));
    assert_eq!(patch.busy_coordinator, Some(BusyCoordinatorPolicy::Skip));
    assert_eq!(patch.enabled, Some(true));
    assert_eq!(patch.session_start, None);
}

/// #2695 command fixture: a project whose Room resolves (the shape of
/// `make_coordinator_fixture`), one Loop with a pending run, a real
/// `LoopScheduler` managed on a test app.
struct CommandFixture {
    _tmp: tempfile::TempDir,
    project: PathBuf,
    dir: PathBuf,
    scheduler: Arc<LoopScheduler>,
    app: tauri::App,
}

fn command_fixture() -> CommandFixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let replica = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
    for dir in [&team_dir, &ac_root.join("_agent_tech-lead"), &replica] {
        std::fs::create_dir_all(dir).expect("fixture dir");
    }
    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["../_agent_tech-lead"],"coordinator":"../_agent_tech-lead"}"#,
    )
    .expect("team config");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .expect("replica config");
    let dir = write_loop_config(&ac_root, &accumulate_config()).expect("loop config");
    let state = crate::config::loops::LoopState {
        last_checked_at: Some(Utc::now() - Duration::minutes(5)),
        pending_due_at: Some(Utc::now() - Duration::minutes(5)),
        pending_run_id: Some(uuid::Uuid::new_v4()),
        ..Default::default()
    };
    write_loop_state_atomic(&dir, &state).expect("loop state");
    let scheduler = Arc::new(LoopScheduler::new());
    let app = crate::test_support::test_builder()
        .manage(Arc::clone(&scheduler))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    CommandFixture {
        _tmp: tmp,
        project,
        dir,
        scheduler,
        app,
    }
}

struct GatedScan {
    release: tokio::sync::oneshot::Sender<()>,
    scan: tokio::task::JoinHandle<Result<(), String>>,
    s0_raw: String,
}

/// Starts a real scan and returns once it is held inside the delivery.
async fn gated_command_fixture() -> (CommandFixture, GatedScan) {
    let fixture = command_fixture();
    let s0_raw = std::fs::read_to_string(fixture.dir.join(crate::config::loops::LOOP_STATE_FILE))
        .expect("s0 state");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    fixture
        .scheduler
        .install_delivery_gate(crate::loops::scheduler::LoopDeliveryGate {
            entered: Some(entered_tx),
            release: Some(release_rx),
            report: Some(crate::loops::delivery::LoopDeliveryReport {
                kind: crate::config::loops::LoopAuditKind::Delivered,
                message: "gated delivery".to_string(),
                target: None,
                session_id: None,
                error: None,
                prompt_snapshot: None,
                completed_at: Some(Utc::now()),
            }),
        });
    let scheduler = Arc::clone(&fixture.scheduler);
    let handle = fixture.app.handle().clone();
    let project = fixture.project.clone();
    let scan = tokio::spawn(async move {
        scheduler
            .scan_project_under_scan_lock(handle, project)
            .await
    });
    entered_rx.await.expect("scan entered the delivery");
    (
        fixture,
        GatedScan {
            release: release_tx,
            scan,
            s0_raw,
        },
    )
}

impl CommandFixture {
    fn project_path(&self) -> String {
        self.project.to_string_lossy().to_string()
    }

    fn state_raw(&self) -> String {
        std::fs::read_to_string(self.dir.join(crate::config::loops::LOOP_STATE_FILE))
            .expect("state")
    }

    fn assert_no_scan_audit_row(&self) {
        let audit = self.dir.join(crate::config::loops::LOOP_AUDIT_FILE);
        let rows = std::fs::read_to_string(audit).unwrap_or_default();
        assert!(rows.trim().is_empty(), "no scan audit row: {}", rows);
    }

    fn create_request(&self) -> LoopCreateRequest {
        let config = accumulate_config();
        LoopCreateRequest {
            project_path: self.project_path(),
            id: Some(config.loop_def.id),
            name: config.loop_def.name,
            expr: config.trigger.expr,
            workgroup: config.target.workgroup,
            prompt_body: config.prompt.body,
            busy_coordinator: None,
            session_start: Some(config.policy.session_start),
            enabled: Some(true),
        }
    }

    async fn toggle(&self, enabled: bool) {
        toggle_loop(
            self.app.handle().clone(),
            self.app.state::<Arc<LoopScheduler>>(),
            self.project_path(),
            "daily-sync".to_string(),
            enabled,
        )
        .await
        .expect("toggle_loop");
    }

    async fn delete(&self) {
        delete_loop(
            self.app.handle().clone(),
            self.app.state::<Arc<LoopScheduler>>(),
            self.project_path(),
            "daily-sync".to_string(),
        )
        .await
        .expect("delete_loop");
    }

    async fn create(&self) {
        create_loop(
            self.app.handle().clone(),
            self.app.state::<Arc<LoopScheduler>>(),
            self.create_request(),
        )
        .await
        .expect("create_loop");
    }
}

impl GatedScan {
    async fn finish(self) -> Result<(), String> {
        self.release.send(()).expect("release the gate");
        self.scan.await.expect("join scan")
    }
}

/// #2695 T2 - a whole `update_loop` completes while a delivery is held.
#[tokio::test]
async fn update_loop_completes_while_a_delivery_is_held() {
    let (fixture, gated) = gated_command_fixture().await;
    let request = LoopUpdateRequest {
        project_path: fixture.project_path(),
        id: "daily-sync".to_string(),
        name: None,
        expr: Some("30 9 * * *".to_string()),
        workgroup: None,
        prompt_body: None,
        busy_coordinator: None,
        session_start: None,
        enabled: None,
    };

    let started = std::time::Instant::now();
    let result = update_loop(
        fixture.app.handle().clone(),
        fixture.app.state::<Arc<LoopScheduler>>(),
        request,
    )
    .await;
    let elapsed = started.elapsed();
    eprintln!("T2: update_loop returned in {:?}", elapsed);

    result.expect("update_loop");
    assert!(elapsed < std::time::Duration::from_millis(1000));
    let config = read_loop_config(&fixture.dir).expect("config");
    assert_eq!(config.trigger.expr, "30 9 * * *");
    let baseline = fixture.state_raw();
    let state = read_loop_state(&fixture.dir).expect("state");
    assert!(
        state.pending_run_id.is_none(),
        "the command wrote a baseline"
    );

    gated.finish().await.expect("scan");
    assert_eq!(fixture.state_raw(), baseline);
    fixture.assert_no_scan_audit_row();
}

/// #2695 T8 - the value guard alone: an unconditional writer that bumps
/// no generation and changes no config.
#[tokio::test]
async fn scan_does_not_overwrite_a_state_changed_during_delivery() {
    let (fixture, gated) = gated_command_fixture().await;
    let mut state = read_loop_state(&fixture.dir).expect("state");
    state.last_checked_at = Some(Utc::now() + Duration::minutes(1));
    write_loop_state_atomic(&fixture.dir, &state).expect("external write");
    let written = fixture.state_raw();
    assert_ne!(written, gated.s0_raw);

    gated.finish().await.expect("scan");

    assert_eq!(fixture.state_raw(), written);
    fixture.assert_no_scan_audit_row();
}

/// #2695 T8b - the realistic ABA: toggle off then on.
#[tokio::test]
async fn scan_does_not_overwrite_a_toggle_off_then_on() {
    let (fixture, gated) = gated_command_fixture().await;
    fixture.toggle(false).await;
    fixture.toggle(true).await;
    let baseline = fixture.state_raw();

    gated.finish().await.expect("scan");

    assert_eq!(fixture.state_raw(), baseline);
    fixture.assert_no_scan_audit_row();
}

/// #2695 T9 - the identity guard alone: delete, recreate with equal
/// fields, restore the S0 bytes. Only the generation differs.
#[tokio::test]
async fn scan_does_not_write_into_a_recreated_loop() {
    let (fixture, gated) = gated_command_fixture().await;
    fixture.delete().await;
    fixture.create().await;
    std::fs::write(
        fixture.dir.join(crate::config::loops::LOOP_STATE_FILE),
        &gated.s0_raw,
    )
    .expect("restore s0 bytes");
    let s0_raw = gated.s0_raw.clone();

    gated.finish().await.expect("scan");

    assert_eq!(fixture.state_raw(), s0_raw);
    fixture.assert_no_scan_audit_row();
}

/// #2695 T10 - a Loop deleted during delivery. No log capture exists in
/// this crate, so the leg is: the scan is Ok and nothing was recreated.
#[tokio::test]
async fn scan_survives_a_delete_during_delivery() {
    let (fixture, gated) = gated_command_fixture().await;
    fixture.delete().await;

    gated.finish().await.expect("scan is Ok");

    assert!(
        !fixture.dir.exists(),
        "nothing recreated the Loop directory"
    );
}

/// #2695 T14 - every command bumps the Loop generation.
#[tokio::test]
async fn every_loop_command_bumps_the_generation() {
    let fixture = command_fixture();
    let generation = || fixture.scheduler.loop_generation(&fixture.dir);

    let before = generation();
    update_loop(
        fixture.app.handle().clone(),
        fixture.app.state::<Arc<LoopScheduler>>(),
        LoopUpdateRequest {
            project_path: fixture.project_path(),
            id: "daily-sync".to_string(),
            name: Some("Renamed".to_string()),
            expr: None,
            workgroup: None,
            prompt_body: None,
            busy_coordinator: None,
            session_start: None,
            enabled: None,
        },
    )
    .await
    .expect("update_loop");
    assert_ne!(generation(), before, "update_loop must bump");

    let before = generation();
    fixture.toggle(false).await;
    assert_ne!(generation(), before, "toggle_loop must bump");

    let before = generation();
    fixture.delete().await;
    assert_ne!(generation(), before, "delete_loop must bump");

    let before = generation();
    fixture.create().await;
    assert_ne!(generation(), before, "create_loop must bump");
}

#[derive(Default)]
struct DeliveryAdapterBackend {
    live: Mutex<std::collections::HashSet<Uuid>>,
    specs: Mutex<Vec<crate::pty::backend::BackendSpawnSpec>>,
}
impl crate::pty::backend::PtyBackend for DeliveryAdapterBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.live.lock().unwrap().insert(spec.id);
            self.specs.lock().unwrap().push(spec);
            Ok(())
        })
    }
    fn write(
        &self,
        _: &crate::pty::manager::BackendWriteAuthority,
        _: Uuid,
        _: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        panic!("no native input")
    }
    fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), crate::errors::AppError> {
        panic!("no resize")
    }
    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }
    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }
    fn get_screen_snapshot(&self, _: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }
    fn register_response_watcher(&self, _: Uuid, _: String, _: PathBuf) {}
    fn terminate_job_for_session(&self, _: Uuid) -> bool {
        false
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

fn delivery_adapter_app(
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    backend: Arc<DeliveryAdapterBackend>,
    settings: crate::config::settings::AppSettings,
) -> tauri::App {
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        shutdown.token().clone(),
    );
    let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
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
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
    let app = crate::test_support::test_builder()
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(Arc::new(
            crate::resource_monitor::ResourceMonitorState::new(),
        ))
        .manage(Arc::clone(&session_mgr))
        .manage(pty_mgr)
        .manage(crate::DetachedSessionsState::default())
        .manage(telegram)
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(crate::pty::menu_guard::MenuGuard::new()))
        .manage(coordinator.clone())
        .manage(target_gate_state.clone())
        .manage(shutdown)
        .manage(
            crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                Ok(message_store),
                target_gate_state.gate.clone(),
            ),
        )
        .manage(store_dir)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build restartable loop app");
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
    app
}

#[test]
fn issue_3260_delivery_adapter_required_state_panics_at_use() {
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert!(app.handle().purge_guard().is_none());
    // Constructing a borrowing future performs no required-state lookup.
    let future = app.handle().restart_fresh(Uuid::new_v4());
    drop(future);
    for getter in [
        (|app: &AppHandle| {
            let _ = app.sessions();
        }) as fn(&AppHandle),
        |app: &AppHandle| {
            let _ = app.ptys();
        },
        |app: &AppHandle| {
            let _ = app.settings_owner();
        },
    ] {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            getter(app.handle());
        }))
        .is_err());
    }
}

#[tokio::test]
async fn issue_3260_delivery_adapter_background_and_default_gate_delegation() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_string_lossy().to_string();
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(DeliveryAdapterBackend::default());
    let settings = crate::config::settings::AppSettings {
        agents: vec![delivery_adapter_agent("pi", "pi --model x")],
        ..Default::default()
    };
    let spawn = crate::config::agent_command::resolve_agent_spawn_command(
        &settings,
        "pi",
        Some(tmp.path()),
        None,
        false,
    )
    .unwrap();
    let app = delivery_adapter_app(Arc::clone(&sessions), Arc::clone(&backend), settings);
    // This cwd is outside every registered project. The cfg(test) default gate skips it.
    let settings = app.state::<crate::config::settings::SettingsState>();
    assert!(crate::config::sessions_persistence::enforce_creation_gate(
        settings.inner(),
        &cwd,
        crate::config::sessions_persistence::CreationGateEnforcement::Enforce,
    )
    .await
    .is_err());
    let selected = sessions
        .read()
        .await
        .create_session(
            "shell".into(),
            Vec::new(),
            cwd.clone(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
        .id;
    sessions
        .read()
        .await
        .set_active_only(selected)
        .await
        .unwrap();
    let info = app
        .handle()
        .create(LoopSessionCreate {
            shell: spawn.shell.clone(),
            shell_args: spawn.shell_args.clone(),
            cwd: cwd.clone(),
            session_name: Some("background loop".into()),
            agent_id: Some("pi".into()),
            agent_label: Some("pi".into()),
            skip_auto_resume: true,
            resolved_spawn: Some(spawn),
            resolved_agent_host_shell: None,
        })
        .await
        .unwrap();
    assert_eq!(sessions.read().await.get_active().await, Some(selected));
    let id = Uuid::parse_str(&info.id).unwrap();
    let restarted = app.handle().restart_fresh(id).await.unwrap();
    assert_ne!(restarted.id, info.id);
    assert_eq!(sessions.read().await.get_active().await, Some(selected));
    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        for spec in specs.iter() {
            assert_eq!((spec.cols, spec.rows), (120, 30));
            assert_eq!(spec.cmd, "pi");
            assert!(!spec.args.contains(&"--continue".to_string()));
            assert_eq!(spec.cwd, cwd);
        }
    }
    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

fn delivery_adapter_agent(id: &str, command: &str) -> crate::config::settings::AgentConfig {
    crate::config::settings::AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
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
