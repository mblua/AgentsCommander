use super::*;

const P44_CANARY: &str = "P44_SCREEN_CANARY";

const P44_TARGET: &str = "p44-project:room-01-p44-team/worker";

const P44_CHILD: &str = "AC_P44_SNAPSHOT_CHILD";

struct P44Backend {
    fanout: SessionIoFanout,
    live: Mutex<HashSet<Uuid>>,
    mutations: AtomicUsize,
    tokens: Mutex<Vec<crate::pty::output::PtyOutputRegistrationToken>>,
}

impl P44Backend {
    fn new() -> Arc<Self> {
        let senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        Arc::new(Self {
            fanout: SessionIoFanout::new(
                senders,
                crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {}),
                None,
            ),
            live: Mutex::new(HashSet::new()),
            mutations: AtomicUsize::new(0),
            tokens: Mutex::new(Vec::new()),
        })
    }

    fn install(&self, id: Uuid) {
        self.live.lock().unwrap().insert(id);
        let token = self
            .fanout
            .register_session_for_test(id, crate::session::profile::IdleTuning::DEFAULT, 4, 40)
            .unwrap();
        self.fanout
            .handle_output(&token, &id.to_string(), P44_CANARY.as_bytes().to_vec());
        self.tokens.lock().unwrap().push(token);
    }

    fn unexpected(&self) -> ! {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        panic!("unexpected mutating snapshot backend call")
    }
}

impl PtyBackend for P44Backend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(&self, _spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), AppError>> {
        self.unexpected()
    }
    fn write(&self, _: &BackendWriteAuthority, _: Uuid, _: &[u8]) -> Result<(), AppError> {
        self.unexpected()
    }
    fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), AppError> {
        self.unexpected()
    }
    fn kill(&self, _: Uuid) -> Result<(), AppError> {
        self.unexpected()
    }
    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }
    fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
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
    fn register_response_watcher(&self, _: Uuid, _: String, _: PathBuf) {
        self.unexpected()
    }
    fn terminate_job_for_session(&self, _: Uuid) -> bool {
        self.unexpected()
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        self.unexpected()
    }
}

struct P44Fixture {
    context: TerminalSnapshotServiceContext,
    state: Arc<TerminalSnapshotState>,
    requester: Session,
    requester_path: PathBuf,
    settings_path: PathBuf,
    backend: Arc<P44Backend>,
    _temporary: tempfile::TempDir,
}

impl P44Fixture {
    async fn new() -> Self {
        let config = PathBuf::from(std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR").unwrap());
        let temporary = tempfile::Builder::new()
            .prefix("fixture-")
            .tempdir_in(&config)
            .unwrap();
        let collection = temporary.path().join("projects");
        let ac = collection.join("p44-project/.ac");
        std::fs::create_dir_all(ac.join("_team_p44-team")).unwrap();
        std::fs::write(ac.join("_team_p44-team/config.json"),
            br#"{"agents":["../_agent_worker","../_agent_coordinator"],"coordinator":"../_agent_coordinator"}"#).unwrap();
        let mut roots = Vec::new();
        for name in ["coordinator", "worker"] {
            std::fs::create_dir_all(ac.join(format!("_agent_{name}"))).unwrap();
            let replica = ac.join(format!("room-01-p44-team/__agent_{name}"));
            std::fs::create_dir_all(&replica).unwrap();
            std::fs::write(
                replica.join("config.json"),
                format!(r#"{{"identity":"../../_agent_{name}"}}"#),
            )
            .unwrap();
            roots.push(replica);
        }
        let settings_path = config.join("settings.30.instance.no-git.json");
        let settings = AppSettings {
            terminal_snapshots_enabled: true,
            project_paths: vec![collection.to_string_lossy().into_owned()],
            ..AppSettings::default()
        };
        std::fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(
            crate::config::settings::settings_path().as_ref(),
            Some(&settings_path)
        );
        let path = settings_path.clone();
        let owner = tokio::task::spawn_blocking(move || {
            SettingsOwner::open_active(&path, Duration::from_secs(5)).unwrap()
        })
        .await
        .unwrap();
        assert_eq!(owner.availability(), SettingsAvailability::Ready);
        assert!(
            owner
                .current_read()
                .await
                .unwrap()
                .terminal_snapshots_enabled
        );
        let manager = SessionManager::new();
        let mut sessions = Vec::new();
        for (index, root) in roots.iter().enumerate() {
            sessions.push(
                manager
                    .create_session(
                        "shell".into(),
                        vec![],
                        root.to_string_lossy().into_owned(),
                        None,
                        None,
                        Vec::<SessionRepo>::new(),
                        index == 0,
                        SessionBackendKind::LocalProcess,
                    )
                    .await
                    .unwrap(),
            );
        }
        let backend = P44Backend::new();
        let pty = PtyManager::new_for_test(backend.clone());
        for (session, root) in sessions.iter().zip(&roots) {
            backend.install(session.id);
            let cwd = crate::path_identity::verify_directory(root).unwrap();
            let replica = crate::path_identity::verify_directory(root).unwrap();
            pty.record_route_with_identities(
                session.id,
                SessionBackendKind::LocalProcess,
                Some(cwd),
                Some(replica),
            )
            .unwrap();
        }
        Self {
            context: TerminalSnapshotServiceContext {
                session_manager: Arc::new(tokio::sync::RwLock::new(manager)),
                pty_manager: Arc::new(Mutex::new(pty)),
                settings: owner,
                restore: Arc::new(crate::RestoreInProgress(AtomicBool::new(false))),
                purge: Arc::new(crate::session::purge_guard::PurgeGuard::default()),
            },
            state: TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new()),
            requester: sessions.remove(0),
            requester_path: roots.remove(0),
            settings_path,
            backend,
            _temporary: temporary,
        }
    }

    async fn prepare(&self) -> Result<TerminalSnapshotPrepared, TerminalSnapshotReasonCode> {
        self.prepare_for(Duration::from_secs(30)).await
    }

    async fn prepare_for(
        &self,
        span: Duration,
    ) -> Result<TerminalSnapshotPrepared, TerminalSnapshotReasonCode> {
        let request = TerminalSnapshotServiceRequest {
            request_id: Uuid::new_v4(),
            target: P44_TARGET.into(),
            format: TerminalSnapshotFormat::Json,
            source_plane: TerminalSnapshotSourcePlane::HostCli,
            host_authorization_deadline: Some((
                Instant::now() + span,
                chrono::Utc::now() + chrono::Duration::from_std(span).unwrap(),
            )),
        };
        let admission = self
            .state
            .pre_admit_requester(
                &self.context,
                TerminalSnapshotRequesterSelector::Host {
                    token: self.requester.token,
                    expected_root: crate::path_identity::verify_directory(&self.requester_path)
                        .unwrap(),
                    claimed_from: "p44-project:room-01-p44-team/coordinator".into(),
                },
                TerminalSnapshotSourcePlane::HostCli,
                request.host_authorization_deadline,
                TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::HostCli),
            )
            .await?;
        self.state.prepare_with_admission(admission, request).await
    }

    async fn encoded(&self) -> (TerminalSnapshotFinalization, Vec<u8>) {
        let (payload, finalization) = self.prepare().await.unwrap().into_parts();
        let request_id = finalization
            .audit
            .inner
            .metadata
            .lock()
            .unwrap()
            .request_id
            .clone()
            .unwrap();
        let bytes = finalization
            .build_host_response(
                payload,
                request_id.clone(),
                "c".repeat(64),
                terminal_snapshot_renderer::canonical_timestamp(
                    chrono::Utc::now() + chrono::Duration::seconds(60),
                ),
            )
            .await
            .unwrap();
        let response = terminal_snapshot_renderer::decode_host_response(
            &bytes,
            &request_id,
            &"c".repeat(64),
            P44_TARGET,
            TerminalSnapshotFormat::Json,
        )
        .unwrap();
        let Some(TerminalSnapshotPayload::Json { snapshot }) = response.result else {
            panic!("expected JSON screen payload");
        };
        assert!(snapshot.screen.lines.iter().any(|line| {
            line.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .contains(P44_CANARY)
        }));
        (finalization, bytes)
    }

    async fn change(&self, change: &str) {
        match change {
            "invalid" => {
                assert_eq!(
                    self.context.settings.availability(),
                    SettingsAvailability::Ready
                );
                assert!(
                    self.context
                        .settings
                        .current_read()
                        .await
                        .unwrap()
                        .terminal_snapshots_enabled
                );
                assert!(crate::config::settings::read_terminal_snapshot_security_settings_strict_offloaded().await.unwrap().terminal_snapshots_enabled);
                let report = self
                    .context
                    .settings
                    .with_operation("p44-invalid", |_, guard| {
                        guard.publish(SettingsPublisher::SettingsSave, |path| {
                            let mut value: serde_json::Value =
                                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                            value["defaultShell"] = serde_json::json!(42);
                            std::fs::write(path, serde_json::to_vec(&value).unwrap())
                                .map_err(|error| error.to_string())?;
                            Ok::<(), String>(())
                        })
                    })
                    .await
                    .unwrap();
                assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
                assert_eq!(report.availability, SettingsAvailability::Invalid);
                assert_eq!(
                    self.context.settings.availability(),
                    SettingsAvailability::Invalid
                );
                assert!(matches!(
                    self.context.settings.current_read().await,
                    Err(crate::config::settings::SettingsOwnerError::Unavailable(
                        SettingsAvailability::Invalid
                    ))
                ));
                assert!(crate::config::settings::read_terminal_snapshot_security_settings_strict_offloaded().await.unwrap().terminal_snapshots_enabled);
            }
            "physical" => {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&self.settings_path).unwrap()).unwrap();
                value["terminalSnapshotsEnabled"] = serde_json::json!(false);
                std::fs::write(&self.settings_path, serde_json::to_vec(&value).unwrap()).unwrap();
                assert_eq!(
                    self.context.settings.availability(),
                    SettingsAvailability::Ready
                );
                assert!(
                    self.context
                        .settings
                        .current_read()
                        .await
                        .unwrap()
                        .terminal_snapshots_enabled
                );
                assert!(!crate::config::settings::read_terminal_snapshot_security_settings_strict_offloaded().await.unwrap().terminal_snapshots_enabled);
            }
            _ => panic!("unknown P44 change"),
        }
    }

    async fn close(self) {
        {
            let limiter = self.state.limiter.lock().unwrap();
            assert_eq!(limiter.global_in_flight, 0);
            assert_eq!(limiter.requester_in_flight.values().sum::<usize>(), 0);
            assert_eq!(limiter.target_in_flight.values().sum::<usize>(), 0);
        }
        assert_eq!(self.backend.mutations.load(Ordering::SeqCst), 0);
        let owner = self.context.settings.clone();
        tokio::task::spawn_blocking(move || {
            owner.drain_blocking(Duration::from_secs(5));
            assert_eq!(owner.availability(), SettingsAvailability::Closed);
        })
        .await
        .unwrap();
    }
}

// The child receives its config directory before any settings initialization.
fn p44_isolated(name: &str, body: impl std::future::Future<Output = ()>) {
    if std::env::var_os(P44_CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(body);
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/2992-evidence/P44");
    std::fs::create_dir_all(&root).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("snapshot-child-")
        .tempdir_in(root)
        .unwrap();
    let stdout = std::fs::File::create(dir.path().join("stdout.log")).unwrap();
    let stderr = std::fs::File::create(dir.path().join("stderr.log")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("pty::terminal_snapshot::tests::{name}"),
            "--test-threads=1",
            "--nocapture",
        ])
        .env(P44_CHILD, "1")
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let limit = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= limit {
            child.kill().unwrap();
            child.wait().unwrap();
            let retained = dir.keep();
            panic!("P44 child timed out; evidence: {}", retained.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = std::fs::read(dir.path().join("stdout.log")).unwrap();
    let err = std::fs::read(dir.path().join("stderr.log")).unwrap();
    if !status.success()
        || out
            .windows(P44_CANARY.len())
            .any(|w| w == P44_CANARY.as_bytes())
        || err
            .windows(P44_CANARY.len())
            .any(|w| w == P44_CANARY.as_bytes())
    {
        let retained = dir.keep();
        panic!(
            "P44 child failed or leaked canary ({status}); evidence: {}",
            retained.display()
        );
    }
}

async fn p44_denial(change: &str, prepare_denial: bool) {
    if prepare_denial {
        let fixture = P44Fixture::new().await;
        fixture.change(change).await;
        assert_eq!(
            fixture.prepare().await.err(),
            Some(TerminalSnapshotReasonCode::TerminalSnapshotsDisabled)
        );
        fixture.close().await;
    }
    let fixture = P44Fixture::new().await;
    let (finalization, _bytes) = fixture.encoded().await;
    fixture.change(change).await;
    assert_eq!(
        final_revalidate_async(&finalization).await,
        Err(TerminalSnapshotReasonCode::AuthorityChanged)
    );
    assert!(!finalization.audit.inner.finalized.load(Ordering::SeqCst));
    drop(finalization);
    fixture.close().await;
    let fixture = P44Fixture::new().await;
    let (finalization, bytes) = fixture.encoded().await;
    fixture.change(change).await;
    let audit = finalization.audit.clone();
    let result = tokio::task::spawn_blocking(move || {
        finalization.finalize_host(bytes, |handoff| {
            assert!(matches!(
                handoff,
                Err(TerminalSnapshotReasonCode::AuthorityChanged)
            ));
            Ok(())
        })
    })
    .await
    .unwrap();
    assert_eq!(result, Err(TerminalSnapshotReasonCode::AuthorityChanged));
    assert!(audit.inner.finalized.load(Ordering::SeqCst));
    {
        let metadata = audit.inner.metadata.lock().unwrap();
        assert_eq!(metadata.status, "failed");
        assert_eq!(
            metadata.reason_code.as_deref(),
            Some(TerminalSnapshotReasonCode::AuthorityChanged.as_str())
        );
        assert!(metadata.selected_session_id.is_some());
        assert!(metadata.payload_bytes.is_some());
    }
    drop(audit);
    fixture.close().await;
}

#[test]
fn e2992_p44_stage_specific_denial_owner_invalid() {
    p44_isolated(
        "e2992_p44_stage_specific_denial_owner_invalid",
        p44_denial("invalid", true),
    );
}

#[test]
fn e2992_p44_stage_specific_denial_physical_only() {
    p44_isolated(
        "e2992_p44_stage_specific_denial_physical_only",
        p44_denial("physical", false),
    );
}

#[test]
fn e2992_p44_stage_specific_denial_ready_canary() {
    p44_isolated("e2992_p44_stage_specific_denial_ready_canary", async {
        let fixture = P44Fixture::new().await;
        let (finalization, _) = fixture.encoded().await;
        assert_eq!(final_revalidate_async(&finalization).await, Ok(()));
        drop(finalization);
        fixture.close().await;
        let fixture = P44Fixture::new().await;
        let (finalization, bytes) = fixture.encoded().await;
        let expected = bytes.clone();
        let audit = finalization.audit.clone();
        tokio::task::spawn_blocking(move || {
            finalization.finalize_host(bytes, |handoff| {
                assert_eq!(handoff.unwrap(), expected);
                Ok(())
            })
        })
        .await
        .unwrap()
        .unwrap();
        assert!(audit.inner.finalized.load(Ordering::SeqCst));
        assert_eq!(audit.inner.metadata.lock().unwrap().status, "succeeded");
        drop(audit);
        fixture.close().await;
    });
}

#[test]
fn service_request_selector_and_prepared_payload_debug_are_structural_only() {
    const CELL_CANARY: &str = "CELL_1173_SVC_F4R6";
    const PNG_CANARY: &str = "PNG_1173_SVC_F4R6";
    const AUTH_CANARY: &str = "AUTH_1173_SVC_F4R6";
    const PATH_CANARY: &str = r"C:\PATH_1173_SVC_F4R6\snapshot.png";
    let request_id = Uuid::parse_str("11730000-0000-4000-8000-00000000f406").unwrap();
    let request = TerminalSnapshotServiceRequest {
        request_id,
        target: PATH_CANARY.to_string(),
        format: TerminalSnapshotFormat::Json,
        source_plane: TerminalSnapshotSourcePlane::HostCli,
        host_authorization_deadline: Some((Instant::now(), chrono::Utc::now())),
    };
    let temporary = tempfile::TempDir::new().unwrap();
    let expected_root = crate::path_identity::verify_directory(temporary.path()).unwrap();
    let selector = TerminalSnapshotRequesterSelector::Host {
        token: request_id,
        expected_root,
        claimed_from: AUTH_CANARY.to_string(),
    };
    let api_selector = TerminalSnapshotRequesterSelector::ApiSession(request_id);

    let mut model: TerminalScreenModel = terminal_snapshot_renderer::decode_bounded(
        include_bytes!(
            "../../../../../crates/terminal-snapshot-renderer/tests/fixtures/blank-cursor-model.json"
        ),
        terminal_snapshot_renderer::MAX_JSON_BYTES,
    )
    .unwrap();
    model.screen.lines[0].cells[0].text = CELL_CANARY.to_string();
    let json = PreparedSnapshotPayload::Json {
        request_id: AUTH_CANARY.to_string(),
        requester: AUTH_CANARY.to_string(),
        target: PATH_CANARY.to_string(),
        model: Arc::new(model),
    };
    let png_model: TerminalScreenModel = terminal_snapshot_renderer::decode_bounded(
        include_bytes!(
            "../../../../../crates/terminal-snapshot-renderer/tests/fixtures/blank-cursor-model.json"
        ),
        terminal_snapshot_renderer::MAX_JSON_BYTES,
    )
    .unwrap();
    let rendered = render_png(&png_model).unwrap();
    let png = PreparedSnapshotPayload::Png(Box::new(TerminalSnapshotPayload::Png {
        metadata: rendered.metadata(
            AUTH_CANARY.to_string(),
            AUTH_CANARY.to_string(),
            PATH_CANARY.to_string(),
            &png_model,
        ),
        png: PNG_CANARY.as_bytes().to_vec(),
    }));

    let diagnostic = format!("{request:?}\n{selector:?}\n{api_selector:?}\n{json:?}\n{png:?}");
    let request_id_text = request_id.to_string();
    let temporary_path = temporary.path().to_string_lossy().into_owned();
    for forbidden in [
        CELL_CANARY,
        PNG_CANARY,
        AUTH_CANARY,
        PATH_CANARY,
        request_id_text.as_str(),
        temporary_path.as_str(),
    ] {
        assert!(!diagnostic.contains(forbidden));
    }
    for structural in [
        "source_plane: HostCli",
        "has_host_authorization_deadline: true",
        "TerminalSnapshotRequesterSelector::Host",
        "TerminalSnapshotRequesterSelector::ApiSession",
        "PreparedSnapshotPayload::Json",
        "PreparedSnapshotPayload::Png",
        "decoded_bytes",
    ] {
        assert!(diagnostic.contains(structural));
    }
}
