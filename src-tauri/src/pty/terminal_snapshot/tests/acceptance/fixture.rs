use super::*;
use crate::config::instance_artifacts::SETTINGS_FILE_NAME;

pub(super) struct ConfigEnvGuard {
    pub(super) previous: Option<OsString>,
}

impl ConfigEnvGuard {
    pub(super) fn set(path: &Path) -> Self {
        let previous = std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR");
        // SAFETY: used only in isolated --test-threads=1 child processes
        unsafe {
            std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", path);
        }
        Self { previous }
    }
}

impl Drop for ConfigEnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            // SAFETY: used only in isolated --test-threads=1 child processes
            unsafe {
                std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", previous);
            }
        } else {
            // SAFETY: used only in isolated --test-threads=1 child processes
            unsafe {
                std::env::remove_var("AGENTSCOMMANDER_TEST_CONFIG_DIR");
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct BackendLookupCounts {
    pub(super) liveness: usize,
    pub(super) copies: usize,
}

pub(super) struct FixtureBackend {
    pub(super) fanout: SessionIoFanout,
    pub(super) live: Mutex<HashSet<Uuid>>,
    pub(super) lookups: Mutex<HashMap<Uuid, BackendLookupCounts>>,
    pub(super) mutations: std::sync::atomic::AtomicUsize,
}

impl FixtureBackend {
    pub(super) fn new() -> Arc<Self> {
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        Arc::new(Self {
            fanout: SessionIoFanout::new(output_senders, idle_detector, None),
            live: Mutex::new(HashSet::new()),
            lookups: Mutex::new(HashMap::new()),
            mutations: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub(super) fn install(&self, id: Uuid, output: &[u8]) {
        self.live.lock().expect("fixture live lock").insert(id);
        self.fanout
            .register_session_for_test(id, crate::session::profile::IdleTuning::DEFAULT, 4, 40)
            .expect("register fixture session");
        let token = self
            .fanout
            .registration_token_for_session(id)
            .expect("fixture token");
        self.fanout
            .handle_output(&token, &id.to_string(), output.to_vec());
    }

    pub(super) fn counts(&self, id: Uuid) -> BackendLookupCounts {
        self.lookups
            .lock()
            .expect("fixture lookup lock")
            .get(&id)
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn mutations(&self) -> usize {
        self.mutations.load(Ordering::SeqCst)
    }

    pub(super) fn record_lookup(&self, id: Uuid, update: impl FnOnce(&mut BackendLookupCounts)) {
        let mut lookups = self.lookups.lock().expect("fixture lookup lock");
        update(lookups.entry(id).or_default());
    }
}

impl PtyBackend for FixtureBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(&self, spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), AppError>> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            self.live.lock().expect("fixture live lock").insert(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), AppError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), AppError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        self.live.lock().expect("fixture live lock").remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().expect("fixture live lock").contains(&id)
    }

    fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        self.record_lookup(id, |counts| counts.liveness += 1);
        if self.has_session(id) {
            ContextSessionLiveness::Live
        } else {
            ContextSessionLiveness::SessionOver
        }
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        self.fanout.get_screen_snapshot(id)
    }

    fn copy_terminal_screen(&self, id: Uuid) -> TerminalScreenCopyRead {
        self.record_lookup(id, |counts| counts.copies += 1);
        self.fanout.copy_terminal_screen(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.fanout.get_pty_size(id)
    }

    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        match self.fanout.get_screen_rows(id) {
            Some(rows) => ScreenRowsRead::Rows(rows),
            None if self.has_session(id) => ScreenRowsRead::Unavailable,
            None => ScreenRowsRead::SessionOver,
        }
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

pub(super) struct ReplicaPaths {
    pub(super) collection: PathBuf,
    pub(super) coordinator: PathBuf,
    pub(super) worker: PathBuf,
    pub(super) live_member: PathBuf,
    pub(super) exited_member: PathBuf,
    pub(super) room_coordinator: PathBuf,
    pub(super) room_live_member: PathBuf,
    pub(super) room2_coordinator: PathBuf,
}

impl ReplicaPaths {
    pub(super) fn create(root: &Path) -> Self {
        let collection = root.join("projects");
        let ac_root = collection.join(PROJECT).join(".ac");
        let team = ac_root.join("_team_dev-team");
        let workgroup = ac_root.join(WORKGROUP);
        std::fs::create_dir_all(&team).expect("team directory");
        for name in [
            "coordinator",
            "worker",
            "member-live",
            "member-exited",
            "member-tampered",
        ] {
            std::fs::create_dir_all(ac_root.join(format!("_agent_{name}")))
                .expect("origin agent directory");
            std::fs::create_dir_all(workgroup.join(format!("__agent_{name}")))
                .expect("replica directory");
        }
        std::fs::write(
            team.join("config.json"),
            concat!(
                r#"{"agents":["../_agent_worker","../_agent_member-live","#,
                r#""../_agent_member-exited","../_agent_member-tampered","#,
                r#""../_agent_coordinator"],"coordinator":"../_agent_coordinator"}"#,
            ),
        )
        .expect("team config");
        for name in ["coordinator", "worker", "member-live", "member-exited"] {
            std::fs::write(
                workgroup.join(format!("__agent_{name}/config.json")),
                format!(r#"{{"identity":"../../_agent_{name}"}}"#),
            )
            .expect("replica config");
        }
        std::fs::write(
            workgroup.join("__agent_member-tampered/config.json"),
            r#"{"identity":"../../_agent_worker"}"#,
        )
        .expect("tampered replica config");
        // #2317: sibling Room trees beside the legacy workgroup. The same
        // project, team config and agent matrices are reused, so the existing
        // wg sessions and tests stay untouched; only the room directories and
        // their replica configs are new.
        let room = ac_root.join(ROOM);
        let room2 = ac_root.join(ROOM2);
        for (replica, name) in [
            (room.join("__agent_coordinator"), "coordinator"),
            (room.join("__agent_member-live"), "member-live"),
            (room2.join("__agent_coordinator"), "coordinator"),
        ] {
            std::fs::create_dir_all(&replica).expect("room replica directory");
            std::fs::write(
                replica.join("config.json"),
                format!(r#"{{"identity":"../../_agent_{name}"}}"#),
            )
            .expect("room replica config");
        }
        Self {
            collection,
            coordinator: workgroup.join("__agent_coordinator"),
            worker: workgroup.join("__agent_worker"),
            live_member: workgroup.join("__agent_member-live"),
            exited_member: workgroup.join("__agent_member-exited"),
            room_coordinator: room.join("__agent_coordinator"),
            room_live_member: room.join("__agent_member-live"),
            room2_coordinator: room2.join("__agent_coordinator"),
        }
    }
}

pub(super) struct AcceptanceFixture {
    pub(super) app: Option<tauri::App>,
    pub(super) snapshot_state: Arc<TerminalSnapshotState>,
    pub(super) settings: crate::config::settings::SettingsState,
    pub(super) settings_path: PathBuf,
    pub(super) registry_path: PathBuf,
    pub(super) message_store: Option<Arc<crate::api::message_store::MessageStore>>,
    pub(super) session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) pty_manager: Arc<std::sync::Mutex<PtyManager>>,
    pub(super) local_backend: Arc<FixtureBackend>,
    pub(super) paths: ReplicaPaths,
    pub(super) host_coordinator: Session,
    pub(super) host_worker: Session,
    pub(super) live_member: Session,
    pub(super) room_coordinator: Session,
    pub(super) room_live_member: Session,
    pub(super) room2_coordinator: Session,
    pub(super) api_coordinator_token: crate::pty::container_tokens::ContainerApiToken,
    pub(super) api_worker_token: crate::pty::container_tokens::ContainerApiToken,
    _container_receivers:
        Vec<tokio::sync::mpsc::Receiver<crate::pty::container_backend::HostToBridgeFrame>>,
    // Declared last so it drops after every other handle: on Windows a
    // temporary directory cannot be removed while the SQLite/WAL connection
    // of the API message store is still open.
    pub(super) _temporary: tempfile::TempDir,
}

impl Drop for AcceptanceFixture {
    fn drop(&mut self) {
        // Tear down the app and the message store before the temporary
        // directory is removed. The app's managed state and every `ApiState`
        // clone the store, and a built-but-never-run tauri test app retains
        // its managed state after drop, so the store must explicitly close
        // its SQLite/WAL handles; on Windows a directory cannot be removed
        // while that connection is open.
        self.app.take();
        if let Some(store) = self.message_store.take() {
            store.close_for_test();
        }
    }
}

struct FixtureSessions {
    host_coordinator: Session,
    host_worker: Session,
    live_member: Session,
    room_coordinator: Session,
    room_live_member: Session,
    room2_coordinator: Session,
    exited_member: Session,
    api_coordinator: Session,
    api_worker: Session,
}

struct FixtureContainer {
    backend: Arc<crate::pty::container_backend::ContainerTransportBackend>,
    registry_path: PathBuf,
    api_coordinator_token: crate::pty::container_tokens::ContainerApiToken,
    api_worker_token: crate::pty::container_tokens::ContainerApiToken,
    receivers: Vec<tokio::sync::mpsc::Receiver<crate::pty::container_backend::HostToBridgeFrame>>,
}

async fn create_fixture_sessions(
    manager: &SessionManager,
    paths: &ReplicaPaths,
) -> FixtureSessions {
    let local = SessionBackendKind::LocalProcess;
    let host_coordinator = create_session(manager, &paths.coordinator, true, local).await;
    let host_worker = create_session(manager, &paths.worker, false, local).await;
    let live_member = create_session(manager, &paths.live_member, false, local).await;
    let room_coordinator = create_session(manager, &paths.room_coordinator, true, local).await;
    let room_live_member = create_session(manager, &paths.room_live_member, false, local).await;
    let room2_coordinator = create_session(manager, &paths.room2_coordinator, true, local).await;
    let exited_member = create_session(manager, &paths.exited_member, false, local).await;
    manager.mark_exited(exited_member.id, 0).await;
    let container = SessionBackendKind::ContainerTransport;
    let api_coordinator = create_session(manager, &paths.coordinator, true, container).await;
    let api_worker = create_session(manager, &paths.worker, false, container).await;
    FixtureSessions {
        host_coordinator,
        host_worker,
        live_member,
        room_coordinator,
        room_live_member,
        room2_coordinator,
        exited_member,
        api_coordinator,
        api_worker,
    }
}

fn install_fixture_outputs(local_backend: &FixtureBackend, sessions: &FixtureSessions) {
    local_backend.install(sessions.host_coordinator.id, b"host coordinator");
    local_backend.install(sessions.host_worker.id, b"host worker");
    local_backend.install(sessions.live_member.id, &terminal_canary_output());
    local_backend.install(sessions.room_live_member.id, &terminal_canary_output());
    local_backend.install(sessions.room_coordinator.id, b"room coordinator");
    local_backend.install(sessions.room2_coordinator.id, b"room 2 coordinator");
    local_backend.install(sessions.exited_member.id, b"exited target");
}

async fn create_fixture_container(
    config: &Path,
    paths: &ReplicaPaths,
    sessions: &FixtureSessions,
) -> FixtureContainer {
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let container_backend = Arc::new(
        crate::pty::container_backend::ContainerTransportBackend::new(
            output_senders,
            idle_detector,
            None,
            None,
        ),
    );
    let registry_path = config.join(crate::api::auth::REGISTRY_FILENAME);
    let token_manager =
        crate::pty::container_tokens::ContainerApiTokenManager::new_for_path(registry_path.clone());
    let api_coordinator_token = mint_fixture_token(
        &token_manager,
        sessions.api_coordinator.id,
        paths.coordinator.to_string_lossy().as_ref(),
        "coordinator",
    )
    .await;
    let api_worker_token = mint_fixture_token(
        &token_manager,
        sessions.api_worker.id,
        paths.worker.to_string_lossy().as_ref(),
        "worker",
    )
    .await;
    let coordinator_binding = binding_for(
        &api_coordinator_token,
        &crate::path_identity::verify_directory(&paths.coordinator).expect("coordinator identity"),
    );
    let worker_binding = binding_for(
        &api_worker_token,
        &crate::path_identity::verify_directory(&paths.worker).expect("worker identity"),
    );
    let receivers = vec![
        container_backend.install_protocol_snapshot_session_for_test(
            sessions.api_coordinator.id,
            coordinator_binding,
            4,
            40,
            b"API coordinator".to_vec(),
        ),
        container_backend.install_protocol_snapshot_session_for_test(
            sessions.api_worker.id,
            worker_binding,
            4,
            40,
            b"API worker".to_vec(),
        ),
    ];
    FixtureContainer {
        backend: container_backend,
        registry_path,
        api_coordinator_token,
        api_worker_token,
        receivers,
    }
}

fn record_fixture_routes(pty: &PtyManager, sessions: &FixtureSessions, paths: &ReplicaPaths) {
    let local = SessionBackendKind::LocalProcess;
    let container = SessionBackendKind::ContainerTransport;
    record_route(pty, sessions.host_coordinator.id, local, &paths.coordinator);
    record_route(pty, sessions.host_worker.id, local, &paths.worker);
    record_route(pty, sessions.live_member.id, local, &paths.live_member);
    record_route(
        pty,
        sessions.room_coordinator.id,
        local,
        &paths.room_coordinator,
    );
    record_route(
        pty,
        sessions.room_live_member.id,
        local,
        &paths.room_live_member,
    );
    record_route(
        pty,
        sessions.room2_coordinator.id,
        local,
        &paths.room2_coordinator,
    );
    record_route(pty, sessions.exited_member.id, local, &paths.exited_member);
    record_route(
        pty,
        sessions.api_coordinator.id,
        container,
        &paths.coordinator,
    );
    record_route(pty, sessions.api_worker.id, container, &paths.worker);
}

impl AcceptanceFixture {
    pub(super) async fn new(temporary: tempfile::TempDir) -> Self {
        let paths = ReplicaPaths::create(temporary.path());
        let config = temporary.path().join("config");
        std::fs::create_dir_all(&config).expect("config directory");
        let settings_path = config.join(SETTINGS_FILE_NAME);
        write_security_settings(&settings_path, &paths.collection, true);

        let app_settings = crate::config::settings::AppSettings {
            terminal_snapshots_enabled: true,
            project_paths: vec![paths.collection.to_string_lossy().to_string()],
            ..Default::default()
        };
        let settings: crate::config::settings::SettingsState =
            crate::config::settings::new_settings_state(app_settings);

        let manager = SessionManager::new();
        let sessions = create_fixture_sessions(&manager, &paths).await;
        let session_manager = Arc::new(tokio::sync::RwLock::new(manager));

        let local_backend = FixtureBackend::new();
        install_fixture_outputs(&local_backend, &sessions);

        let container = create_fixture_container(&config, &paths, &sessions).await;

        let local_trait: Arc<dyn PtyBackend> = local_backend.clone();
        let pty = PtyManager::new_for_test_with_container_backend(local_trait, container.backend);
        record_fixture_routes(&pty, &sessions, &paths);
        let pty_manager = Arc::new(std::sync::Mutex::new(pty));

        for root in [
            &paths.coordinator,
            &paths.worker,
            &paths.room_coordinator,
            &paths.room2_coordinator,
        ] {
            create_mailbox_directories(root);
        }
        let shutdown = crate::shutdown::ShutdownSignal::new();
        let snapshot_state = TerminalSnapshotState::new(shutdown);
        let restore = Arc::new(crate::RestoreInProgress(
            std::sync::atomic::AtomicBool::new(false),
        ));
        let purge = Arc::new(crate::session::purge_guard::PurgeGuard::default());
        let message_store = Arc::new(
            crate::api::message_store::MessageStore::open(config.join("api-messages.sqlite3"))
                .expect("API message store"),
        );
        let app = crate::test_support::test_builder()
            .manage(Arc::clone(&snapshot_state))
            .manage(settings.clone())
            .manage(Arc::clone(&session_manager))
            .manage(Arc::clone(&pty_manager))
            .manage(restore)
            .manage(purge)
            .manage(crate::api::message_store::MessageStoreState::ready(
                Arc::clone(&message_store),
            ))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("terminal snapshot acceptance app");

        let FixtureSessions {
            host_coordinator,
            host_worker,
            live_member,
            room_coordinator,
            room_live_member,
            room2_coordinator,
            ..
        } = sessions;
        Self {
            app: Some(app),
            snapshot_state,
            settings,
            settings_path,
            registry_path: container.registry_path,
            message_store: Some(message_store),
            session_manager,
            pty_manager,
            local_backend,
            paths,
            host_coordinator,
            host_worker,
            live_member,
            room_coordinator,
            room_live_member,
            room2_coordinator,
            api_coordinator_token: container.api_coordinator_token,
            api_worker_token: container.api_worker_token,
            _container_receivers: container.receivers,
            _temporary: temporary,
        }
    }
}

/// Labels and wiring for one isolated acceptance child test. Every label is
/// spliced into the exact panic and surface messages the parent used inline.
pub(super) struct IsolatedChild<'a> {
    pub(super) parent: &'a str,
    pub(super) evidence_prefix: &'a str,
    pub(super) executable: &'a str,
    pub(super) failure: &'a str,
    pub(super) surface: &'a str,
    pub(super) test_name: &'a str,
    pub(super) child_env: &'a str,
    pub(super) canary_file_env: &'a str,
    pub(super) fallback_canaries: &'a [&'a str],
}

/// Runs the isolated child, then proves no canary reached its stdout/stderr.
pub(super) fn run_isolated_child(child: &IsolatedChild<'_>) {
    let parent = child.parent;
    let temporary_root = std::env::current_dir()
        .unwrap_or_else(|error| panic!("{parent} parent current directory: {error:?}"))
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root)
        .unwrap_or_else(|error| panic!("{parent} parent temporary root: {error:?}"));
    let evidence = tempfile::Builder::new()
        .prefix(child.evidence_prefix)
        .tempdir_in(temporary_root)
        .unwrap_or_else(|error| panic!("{parent} evidence directory: {error:?}"));
    let canary_file = evidence.path().join("canaries.json");
    let executable = child.executable;
    let output = std::process::Command::new(
        std::env::current_exe()
            .unwrap_or_else(|error| panic!("{executable} test executable: {error:?}")),
    )
    .args([
        "--exact",
        child.test_name,
        "--test-threads=1",
        "--nocapture",
    ])
    .env(child.child_env, "1")
    .env(child.canary_file_env, &canary_file)
    .output()
    .unwrap_or_else(|error| panic!("spawn isolated {executable} test: {error:?}"));
    let canaries: Vec<String> = std::fs::read(&canary_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| {
            child
                .fallback_canaries
                .iter()
                .copied()
                .map(str::to_string)
                .collect()
        });
    let surface = child.surface;
    assert_canaries_absent_except(&output.stdout, &canaries, &[], &format!("{surface} stdout"));
    assert_canaries_absent_except(&output.stderr, &canaries, &[], &format!("{surface} stderr"));
    if !output.status.success() {
        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
        for canary in &canaries {
            stdout = stdout.replace(canary, "<redacted-canary>");
            stderr = stderr.replace(canary, "<redacted-canary>");
        }
        let failure = child.failure;
        panic!("isolated {failure} test failed; stdout={stdout:?}; stderr={stderr:?}");
    }
}

/// Exact `expect` messages for one capacity-reuse success request.
pub(super) struct CapacityReuseMessages<'a> {
    pub(super) timeout: &'a str,
    pub(super) response: &'a str,
    pub(super) decode: &'a str,
}

/// Proves a released slot serves one more strict JSON success with its audit row.
pub(super) async fn assert_capacity_reuse_request(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    target: &str,
    messages: &CapacityReuseMessages<'_>,
    expected_copies: usize,
) {
    let success_id = Uuid::new_v4().to_string();
    let success_response = tokio::time::timeout(
        Duration::from_secs(10),
        run_direct_api_request(
            direct_api_state(fixture),
            fixture.api_coordinator_token.secret.clone(),
            direct_api_body(&success_id, target),
        ),
    )
    .await
    .expect(messages.timeout);
    let success_status = success_response.status();
    let success_bytes = axum::body::to_bytes(
        success_response.into_body(),
        terminal_snapshot_renderer::MAX_TRANSPORT_BYTES,
    )
    .await
    .expect(messages.response);
    assert_eq!(success_status, StatusCode::OK);
    let success = decode_api_success(
        &success_bytes,
        &success_id,
        target,
        TerminalSnapshotFormat::Json,
    )
    .expect(messages.decode);
    assert!(payload_has_sentinel(&success.result));
    drop(success);
    drop(success_bytes);
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 1);
    assert_api_audit_row(config, canaries, 6, "succeeded", None, Some(&success_id));
    assert_eq!(
        fixture.local_backend.counts(fixture.live_member.id).copies,
        expected_copies
    );
}
