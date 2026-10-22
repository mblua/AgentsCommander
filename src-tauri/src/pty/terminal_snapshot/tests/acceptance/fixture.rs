use super::*;

pub(super) struct ConfigEnvGuard {
    pub(super) previous: Option<OsString>,
}

impl ConfigEnvGuard {
    pub(super) fn set(path: &Path) -> Self {
        let previous = std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR");
        std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", path);
        Self { previous }
    }
}

impl Drop for ConfigEnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", previous);
        } else {
            std::env::remove_var("AGENTSCOMMANDER_TEST_CONFIG_DIR");
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

impl AcceptanceFixture {
    pub(super) async fn new(temporary: tempfile::TempDir) -> Self {
        let paths = ReplicaPaths::create(temporary.path());
        let config = temporary.path().join("config");
        std::fs::create_dir_all(&config).expect("config directory");
        let settings_path = config.join("settings.30.instance.no-git.json");
        write_security_settings(&settings_path, &paths.collection, true);

        let app_settings = crate::config::settings::AppSettings {
            terminal_snapshots_enabled: true,
            project_paths: vec![paths.collection.to_string_lossy().to_string()],
            ..Default::default()
        };
        let settings: crate::config::settings::SettingsState =
            crate::config::settings::new_settings_state(app_settings);

        let manager = SessionManager::new();
        let host_coordinator = create_session(
            &manager,
            &paths.coordinator,
            true,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let host_worker = create_session(
            &manager,
            &paths.worker,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let live_member = create_session(
            &manager,
            &paths.live_member,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let room_coordinator = create_session(
            &manager,
            &paths.room_coordinator,
            true,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let room_live_member = create_session(
            &manager,
            &paths.room_live_member,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let room2_coordinator = create_session(
            &manager,
            &paths.room2_coordinator,
            true,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let exited_member = create_session(
            &manager,
            &paths.exited_member,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await;
        manager.mark_exited(exited_member.id, 0).await;
        let api_coordinator = create_session(
            &manager,
            &paths.coordinator,
            true,
            SessionBackendKind::ContainerTransport,
        )
        .await;
        let api_worker = create_session(
            &manager,
            &paths.worker,
            false,
            SessionBackendKind::ContainerTransport,
        )
        .await;
        let session_manager = Arc::new(tokio::sync::RwLock::new(manager));

        let local_backend = FixtureBackend::new();
        local_backend.install(host_coordinator.id, b"host coordinator");
        local_backend.install(host_worker.id, b"host worker");
        local_backend.install(live_member.id, &terminal_canary_output());
        local_backend.install(room_live_member.id, &terminal_canary_output());
        local_backend.install(room_coordinator.id, b"room coordinator");
        local_backend.install(room2_coordinator.id, b"room 2 coordinator");
        local_backend.install(exited_member.id, b"exited target");

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
        let token_manager = crate::pty::container_tokens::ContainerApiTokenManager::new_for_path(
            registry_path.clone(),
        );
        let api_coordinator_token = mint_fixture_token(
            &token_manager,
            api_coordinator.id,
            paths.coordinator.to_string_lossy().as_ref(),
            "coordinator",
        )
        .await;
        let api_worker_token = mint_fixture_token(
            &token_manager,
            api_worker.id,
            paths.worker.to_string_lossy().as_ref(),
            "worker",
        )
        .await;
        let coordinator_binding = binding_for(
            &api_coordinator_token,
            &crate::path_identity::verify_directory(&paths.coordinator)
                .expect("coordinator identity"),
        );
        let worker_binding = binding_for(
            &api_worker_token,
            &crate::path_identity::verify_directory(&paths.worker).expect("worker identity"),
        );
        let container_receivers = vec![
            container_backend.install_protocol_snapshot_session_for_test(
                api_coordinator.id,
                coordinator_binding,
                4,
                40,
                b"API coordinator".to_vec(),
            ),
            container_backend.install_protocol_snapshot_session_for_test(
                api_worker.id,
                worker_binding,
                4,
                40,
                b"API worker".to_vec(),
            ),
        ];

        let local_trait: Arc<dyn PtyBackend> = local_backend.clone();
        let pty = PtyManager::new_for_test_with_container_backend(local_trait, container_backend);
        record_route(
            &pty,
            host_coordinator.id,
            SessionBackendKind::LocalProcess,
            &paths.coordinator,
        );
        record_route(
            &pty,
            host_worker.id,
            SessionBackendKind::LocalProcess,
            &paths.worker,
        );
        record_route(
            &pty,
            live_member.id,
            SessionBackendKind::LocalProcess,
            &paths.live_member,
        );
        record_route(
            &pty,
            room_coordinator.id,
            SessionBackendKind::LocalProcess,
            &paths.room_coordinator,
        );
        record_route(
            &pty,
            room_live_member.id,
            SessionBackendKind::LocalProcess,
            &paths.room_live_member,
        );
        record_route(
            &pty,
            room2_coordinator.id,
            SessionBackendKind::LocalProcess,
            &paths.room2_coordinator,
        );
        record_route(
            &pty,
            exited_member.id,
            SessionBackendKind::LocalProcess,
            &paths.exited_member,
        );
        record_route(
            &pty,
            api_coordinator.id,
            SessionBackendKind::ContainerTransport,
            &paths.coordinator,
        );
        record_route(
            &pty,
            api_worker.id,
            SessionBackendKind::ContainerTransport,
            &paths.worker,
        );
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

        Self {
            app: Some(app),
            snapshot_state,
            settings,
            settings_path,
            registry_path,
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
            api_coordinator_token,
            api_worker_token,
            _container_receivers: container_receivers,
            _temporary: temporary,
        }
    }
}
