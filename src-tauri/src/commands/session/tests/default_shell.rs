use super::*;

// --- #1271 configured default-shell propagation --------------------------

/// Captures every `BackendSpawnSpec` the PTY manager hands to the backend,
/// so tests can assert the resolved-agent host-shell snapshot propagated
/// unchanged from the creation seam. Spawned ids count as live sessions so
/// the finalized-create path can display them, mirroring
/// `ScriptedSpawnBackend`.
#[derive(Default)]
pub(super) struct CapturingSpawnBackend {
    pub(super) specs: Mutex<Vec<crate::pty::backend::BackendSpawnSpec>>,
    pub(super) live: Mutex<HashSet<Uuid>>,
    /// #1873 - every id the manager asked this backend to tear down.
    pub(super) killed: Mutex<Vec<Uuid>>,
}

impl PtyBackend for CapturingSpawnBackend {
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
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Err(crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.live.lock().unwrap().remove(&id);
        self.killed.lock().unwrap().push(id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
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

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

#[tokio::test]
async fn configured_default_shell_snapshot_reaches_backend_for_resolved_agent() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let mut settings = test_settings();
    settings.default_shell =
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string();
    settings.default_shell_args = vec!["-NoProfile".to_string()];

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

    // Drive the resolved-agent path exactly as `create_session` does: the
    // resolved agent stays the command-to-run, and the configured host shell
    // (copied from the same config snapshot) travels separately.
    let spawn =
        super::super::build_configured_agent_spawn_for_cwd(&test_settings(), "claude", &cwd, None)
            .expect("resolve claude")
            .expect("claude is configured in test_settings");
    let host_shell = crate::pty::backend::ResolvedAgentHostShell {
        program: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string(),
        args: vec!["-NoProfile".to_string()],
    };
    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd.clone(),
        Some("1271 propagation".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        true,
        Some(spawn),
        Some(host_shell),
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("resolved-agent create succeeds");
    assert_eq!(created.agent_id.as_deref(), Some("claude"));

    {
        let specs = backend.specs.lock().unwrap();
        let spec = specs
            .last()
            .expect("the resolved-agent create reached the backend");
        assert_eq!(spec.cmd, "claude", "agent program stays the command-to-run");
        let host = spec
            .resolved_agent_host_shell
            .as_ref()
            .expect("host shell snapshot must reach the backend");
        assert_eq!(
            host.program,
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
        assert_eq!(host.args, vec!["-NoProfile".to_string()]);
    }
    close_test_coordinator(&app).await;
}

/// #1271 - the observable session metadata seam (`set_pending_effective_shell_args`,
/// surfaced as `effective_shell_args`) keeps the LOGICAL agent argv; the
/// configured host-shell arguments travel only in the backend spec's
/// `resolved_agent_host_shell`. The two must remain distinct: host-shell args
/// are never written into the metadata channel.
#[tokio::test]
async fn configured_default_shell_metadata_keeps_logical_agent_argv_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let mut settings = test_settings();
    settings.default_shell =
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string();
    settings.default_shell_args = vec!["-NoProfile".to_string()];

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

    let spawn =
        super::super::build_configured_agent_spawn_for_cwd(&test_settings(), "claude", &cwd, None)
            .expect("resolve claude")
            .expect("claude is configured in test_settings");
    let host_shell = crate::pty::backend::ResolvedAgentHostShell {
        program: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string(),
        args: vec!["-NoProfile".to_string()],
    };
    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd.clone(),
        Some("1271 metadata".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        true,
        Some(spawn),
        Some(host_shell),
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("resolved-agent create succeeds");

    // The claude test agent gets auto-injected logical args (`--session-id`):
    // metadata must carry exactly that logical argv, never the configured
    // host-shell args.
    let effective = created
        .effective_shell_args
        .as_deref()
        .expect("effective args are captured before spawn");
    assert!(
        effective.iter().any(|arg| arg.starts_with("--session-id")),
        "session metadata carries the logical agent argv: {effective:?}"
    );
    assert!(
        !effective.iter().any(|arg| arg == "-NoProfile"),
        "host-shell args must never leak into session metadata: {effective:?}"
    );
    let session = session_mgr
        .read()
        .await
        .get_session(Uuid::parse_str(&created.id).unwrap())
        .await
        .expect("session exists");
    let stored = session
        .effective_shell_args
        .as_deref()
        .expect("stored effective args");
    assert!(
        !stored.iter().any(|arg| arg == "-NoProfile"),
        "the pending-effective metadata channel stays logical: {stored:?}"
    );

    {
        let specs = backend.specs.lock().unwrap();
        let spec = specs.last().expect("create reached the backend");
        assert_eq!(
            spec.args, effective,
            "backend args stay the logical agent argv"
        );
        assert_eq!(
            spec.resolved_agent_host_shell
                .as_ref()
                .expect("host shell present")
                .args,
            vec!["-NoProfile".to_string()],
            "host-shell args travel only in the paired snapshot"
        );
    }
    close_test_coordinator(&app).await;
}

/// #1271 - native twin of the web invalid-input postcondition test: the
/// adapter rejection happens at the TOP of the REAL `spawn_sync`, before
/// any spawn accounting or PTY acquisition, so a rejected configured host
/// leaves no session, no pending/metadata record, no spawn record, no PTY
/// map entry, and no output task. Windows-only (the adapter is the Windows
/// host-shell branch); the backend seam proves the same postcondition with
/// a known id in `adapter_spawn_sync_tests`.
#[cfg(windows)]
#[tokio::test]
async fn configured_default_shell_invalid_input_leaves_no_session_state_native() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let mut settings = test_settings();
    // A conflicting/terminal configured option must fail before any PTY.
    settings.default_shell = "powershell.exe".to_string();
    settings.default_shell_args = vec!["-Command".to_string()];

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    // The real local backend: the invalid-input rejection lives inside its
    // spawn_sync, so a fake capturing backend would accept the spawn.
    let git_app = Box::leak(Box::new(
        crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build native invalid-input git watcher app"),
    ));
    let git_watcher = crate::pty::git_watcher::GitWatcher::new(
        Arc::clone(&session_mgr),
        git_app.handle().clone(),
    );
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let output_senders: crate::telegram::manager::OutputSenderMap =
        Arc::new(Mutex::new(HashMap::new()));
    let backend = Arc::new(crate::pty::local_backend::LocalProcessBackend::new(
        output_senders,
        idle_detector,
        git_watcher,
        None,
    ));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

    let error = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "claude".to_string(),
        Vec::new(),
        cwd.clone(),
        Some("1271 native invalid input".to_string()),
        None,
        None,
        false,
        Vec::new(),
        true,
        None,
        Some(crate::pty::backend::ResolvedAgentHostShell {
            program: "powershell.exe".to_string(),
            args: vec!["-Command".to_string()],
        }),
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect_err("invalid configured host must fail the create");
    assert!(error.contains("conflicting/terminal"), "{error}");
    assert!(
        error.contains("agent adapter owns command execution"),
        "{error}"
    );

    assert!(
        session_mgr.read().await.list_sessions().await.is_empty(),
        "no session may appear in the session manager"
    );
    // The coordinator worker rolls the pending binding back asynchronously;
    // while it is still visible, the rejected id must show no launch
    // provenance, no PTY map entry, and no output task.
    let pending_ids = session_mgr
        .read()
        .await
        .aggregate_snapshot()
        .await
        .pending_ids;
    for id in &pending_ids {
        assert!(
            crate::pty::spawn_diagnostics::record_for(*id).is_none(),
            "no launch provenance may be recorded for the rejected session"
        );
        assert!(
            !pty_mgr.lock().unwrap().has_session(*id),
            "no PTY map entry may exist for the rejected session"
        );
        assert!(
            pty_mgr.lock().unwrap().get_pty_size(*id).is_none(),
            "no output task may be attached for the rejected session"
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let pending_empty = session_mgr
            .read()
            .await
            .aggregate_snapshot()
            .await
            .pending_ids
            .is_empty();
        if pending_empty {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no pending session/metadata record may survive the rejection"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn test_resolve_blocking_menu_command() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let session =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let session_id = Uuid::parse_str(&session.id).unwrap();

    let menu_guard = app.state::<Arc<crate::pty::menu_guard::MenuGuard>>();
    let pi_entries = crate::config::settings::default_blocking_menus_for_command("pi");
    let menu_rows = vec![crate::pty::watchers::frame::LogicalRow {
        start: 0,
        end: 0,
        text: "Trust project folder?".to_string(),
    }];

    // Evaluate to trigger blocked state
    let eval = menu_guard.evaluate_logical_rows(session_id, &menu_rows, &pi_entries);
    assert!(eval.is_blocked);
    assert!(menu_guard.is_blocked(session_id));

    // Set blocked menu in session manager
    session_mgr
        .read()
        .await
        .set_blocked_menu(
            session_id,
            eval.matched_notification.unwrap(),
            chrono::Utc::now(),
        )
        .await;

    let events = capture_session_lifecycle(&app);

    // Invoke resolve_blocking_menu
    let res = super::super::resolve_blocking_menu(
        app.handle().clone(),
        app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
        app.state::<Arc<crate::pty::menu_guard::MenuGuard>>(),
        session.id.clone(),
    )
    .await;
    assert!(res.is_ok());

    // Verify menu_guard is no longer blocked for this session
    assert!(!menu_guard.is_blocked(session_id));

    // Verify session manager communication is cleared
    let stored = session_mgr
        .read()
        .await
        .get_session(session_id)
        .await
        .unwrap();
    assert!(stored.communication.is_none());

    // Verify session_communication_changed event was emitted
    let mut found_comm_event = false;
    while let Ok((event, payload)) = events.try_recv() {
        if event == "session_communication_changed" {
            assert!(payload.contains(&session.id));
            assert!(payload.contains("\"communication\":null"));
            found_comm_event = true;
            break;
        }
    }
    assert!(
        found_comm_event,
        "must emit session_communication_changed event"
    );

    close_test_coordinator(&app).await;
}
