use super::*;
use crate::commands::session::preflight::{
    refusal_message, resolution_failure_message, seed_gate, HoldRefusal, PreflightState,
    CONTAINER_REFUSAL,
};

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
    /// #3516 - when true, preflight requests run as given with the inherited env.
    pub(super) preflight_runs: bool,
    pub(super) preflight_requests: Mutex<Vec<crate::pty::backend::PreflightLaunchRequest>>,
    /// #3516 - the hold state and cwd whose launch count `spawn` records.
    pub(super) share_probe: Mutex<Option<(Arc<PreflightState>, String)>>,
    pub(super) share_counts: Mutex<Vec<usize>>,
    /// #3516 - `try_test` on the probed folder, taken inside `spawn`.
    pub(super) test_probes: Mutex<Vec<Result<(), HoldRefusal>>>,
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
            if let Some((state, cwd)) = self.share_probe.lock().unwrap().as_ref() {
                self.share_counts
                    .lock()
                    .unwrap()
                    .push(state.launch_count(cwd));
                let probe = state.try_test(cwd).map(drop);
                self.test_probes.lock().unwrap().push(probe);
            }
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

    fn prepare_preflight_launch(
        &self,
        request: crate::pty::backend::PreflightLaunchRequest,
    ) -> Result<crate::pty::backend::PreflightLaunch, crate::errors::AppError> {
        if !self.preflight_runs {
            return Err(crate::errors::AppError::Other("no preflight".to_string()));
        }
        self.preflight_requests
            .lock()
            .unwrap()
            .push(request.clone());
        Ok(crate::pty::backend::PreflightLaunch {
            program: request.program,
            args: request.args,
            windows_raw_args: None,
            cwd: request.cwd,
            env: std::env::vars().collect(),
        })
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
    let owners = crate::AppOwners::new();
    let git_app = Box::leak(Box::new(
        crate::test_support::test_builder_with_owners(owners.clone())
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
    let backend = Arc::new(crate::pty::local_backend::LocalProcessBackend::with_owners(
        output_senders,
        idle_detector,
        git_watcher,
        None,
        #[cfg(windows)]
        owners.git_guard.clone(),
        owners.spawn.clone(),
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
            crate::pty::spawn_diagnostics::record_for(*id, owners.spawn.as_ref()).is_none(),
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
    assert_rejected_pending_sessions_drained(&session_mgr).await;
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

// --- #3516 launch gate on the create path ---------------------------------

fn preflight_text(code: u8) -> String {
    if cfg!(windows) {
        format!("cmd.exe /D /C \"exit {code}\"")
    } else {
        format!("sh -c \"exit {code}\"")
    }
}

struct GateRun {
    result: Result<crate::session::session::SessionInfo, String>,
    backend: Arc<CapturingSpawnBackend>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    state: Option<Arc<PreflightState>>,
    cwd: String,
    _temp: tempfile::TempDir,
}

/// Mirrors `configured_default_shell_snapshot_reaches_backend_for_resolved_agent`
/// with a configured Coding Agent preflight and, when `manage`, the hold state.
async fn gate_create(
    preflight: Option<String>,
    container: bool,
    manage: bool,
    before: impl FnOnce(&Arc<PreflightState>, &str),
) -> GateRun {
    gate_create_with(preflight, container, manage, before, |_, _| {}).await
}

async fn gate_create_with(
    preflight: Option<String>,
    container: bool,
    manage: bool,
    before: impl FnOnce(&Arc<PreflightState>, &str),
    edit_spawn: impl FnOnce(&mut crate::config::agent_command::AgentSpawnCommand, &std::path::Path),
) -> GateRun {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let mut settings = test_settings();
    settings.agents[0].preflight = preflight;
    if container {
        settings.agents[0].backend = crate::config::settings::AgentBackendConfig {
            kind: SessionBackendKind::ContainerTransport,
            image: Some("ghcr.io/example/claude:test".to_string()),
        };
    }
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let state = manage.then(|| Arc::new(PreflightState::default()));
    let backend = Arc::new(CapturingSpawnBackend {
        preflight_runs: true,
        share_probe: Mutex::new(state.as_ref().map(|s| (Arc::clone(s), cwd.clone()))),
        ..Default::default()
    });
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let seed_lock: crate::app_state::ConfigSeedLockState = Default::default();
    app.manage(seed_lock);
    if let Some(state) = &state {
        app.manage(Arc::clone(state));
        before(state, &cwd);
    }
    let mut spawn =
        super::super::build_configured_agent_spawn_for_cwd(&settings, "claude", &cwd, None)
            .expect("resolve claude")
            .expect("claude is configured");
    edit_spawn(&mut spawn, temp.path());
    let result = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd.clone(),
        Some("3516 gate".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        true,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await;
    close_test_coordinator(&app).await;
    GateRun {
        result,
        backend,
        session_mgr,
        state,
        cwd,
        _temp: temp,
    }
}

const GATE_TEXT: &str = "The CLI will not start because the gate failed.";

#[tokio::test]
async fn failing_preflight_blocks_backend_spawn() {
    let run = gate_create(Some(preflight_text(1)), false, true, |_, _| {}).await;
    let error = run
        .result
        .expect_err("a failing preflight blocks the launch");
    assert!(error.contains("failed with exit code 1"), "{error}");
    assert!(error.ends_with(GATE_TEXT), "{error}");
    assert!(run.backend.specs.lock().unwrap().is_empty());
    assert_eq!(run.backend.preflight_requests.lock().unwrap().len(), 1);
    assert!(run
        .session_mgr
        .read()
        .await
        .list_sessions()
        .await
        .is_empty());
}

#[tokio::test]
async fn passing_preflight_reaches_backend_with_cli_env() {
    let run = gate_create(Some(preflight_text(0)), false, true, |_, _| {}).await;
    run.result.expect("a passing preflight launches");
    let specs = run.backend.specs.lock().unwrap();
    assert_eq!(specs.len(), 1);
    let requests = run.backend.preflight_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (spec, request) = (&specs[0], &requests[0]);
    assert_eq!(request.cwd, spec.cwd);
    assert_eq!(request.configured_env, spec.configured_env);
    assert_eq!(request.env_remove_keys, spec.env_remove_keys);
    assert_eq!(request.extra_env, spec.extra_env);
}

#[tokio::test]
async fn empty_chain_unmanaged_is_unchanged() {
    let run = gate_create(None, false, false, |_, _| {}).await;
    run.result
        .expect("an empty chain launches without the hold state");
    assert_eq!(run.backend.specs.lock().unwrap().len(), 1);
    assert!(run.backend.preflight_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn container_agent_with_chain_is_refused() {
    let run = gate_create(Some(preflight_text(0)), true, true, |_, _| {}).await;
    assert_eq!(
        run.result.expect_err("container refused"),
        CONTAINER_REFUSAL
    );
    assert!(run.backend.specs.lock().unwrap().is_empty());
    assert!(run.backend.preflight_requests.lock().unwrap().is_empty());

    let run = gate_create(None, true, true, |_, _| {}).await;
    if let Err(error) = run.result {
        assert_ne!(error, CONTAINER_REFUSAL);
    }
}

#[tokio::test]
async fn share_released_after_failed_launch() {
    let run = gate_create(Some(preflight_text(1)), false, true, |_, _| {}).await;
    run.result.expect_err("failing preflight");
    let state = run.state.expect("managed");
    assert_eq!(state.launch_count(&run.cwd), 0);

    let run = gate_create(Some(preflight_text(0)), false, true, |_, _| {}).await;
    run.result.expect("passing preflight");
    assert_eq!(*run.backend.share_counts.lock().unwrap(), vec![1]);
}

#[tokio::test]
async fn concurrent_launches_share() {
    let held = Mutex::new(None);
    let run = gate_create(Some(preflight_text(0)), false, true, |state, cwd| {
        *held.lock().unwrap() = Some(state.try_share(cwd).expect("held share"));
    })
    .await;
    run.result.expect("a held share does not block a launch");
    assert_eq!(*run.backend.share_counts.lock().unwrap(), vec![2]);
    drop(held);
}

#[tokio::test]
async fn share_spans_pty_spawn() {
    for preflight in [None, Some(preflight_text(0))] {
        let run = gate_create(preflight, false, true, |_, _| {}).await;
        run.result.expect("launch");
        assert_eq!(*run.backend.share_counts.lock().unwrap(), vec![1]);
        assert_eq!(run.state.expect("managed").launch_count(&run.cwd), 0);
    }
}

#[tokio::test]
async fn invalid_preflight_fails_only_the_launch() {
    let run = gate_create(Some("pre \"unclosed".to_string()), false, true, |_, _| {}).await;
    let error = run.result.expect_err("invalid preflight");
    assert!(
        error.starts_with("Invalid Coding Agent preflight command: "),
        "{error}"
    );
    let cause = error
        .strip_suffix(&format!(". {GATE_TEXT}"))
        .expect("gate suffix");
    assert_eq!(error, resolution_failure_message(cause));
    assert!(run.backend.specs.lock().unwrap().is_empty());
    assert!(run.backend.preflight_requests.lock().unwrap().is_empty());
}

/// A real seed resolved for a temp replica, then pointed at a missing replica
/// root, so the seed reports `StaleReplica`.
fn stale_seed(spawn: &mut crate::config::agent_command::AgentSpawnCommand, temp: &std::path::Path) {
    let replica = temp
        .join(".ac")
        .join("wg-7-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();
    let mut settings = test_settings();
    settings.agents[0].config_seed = Some(crate::config::settings::ConfigSeedConfig {
        enabled: true,
        dest: ".claude".to_string(),
    });
    let replica = replica.to_string_lossy().to_string();
    let mut seed =
        super::super::build_configured_agent_spawn_for_cwd(&settings, "claude", &replica, None)
            .expect("resolve claude")
            .expect("claude is configured")
            .seed
            .expect("a replica resolves a seed");
    let missing = temp.join("missing-replica");
    seed.dest = missing.join(".claude");
    seed.context.replica_root = missing;
    spawn.seed = Some(seed);
}

#[tokio::test]
async fn stale_seed_without_chain_still_launches() {
    let run = gate_create_with(None, false, true, |_, _| {}, stale_seed).await;
    run.result
        .expect("an empty chain keeps the seed best-effort");
    assert_eq!(run.backend.specs.lock().unwrap().len(), 1);
    assert!(run.backend.preflight_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stale_seed_with_chain_blocks_launch() {
    let run = gate_create_with(Some(preflight_text(0)), false, true, |_, _| {}, stale_seed).await;
    let stale = crate::config::config_seed::ConfigSeedReport::Skipped(
        crate::config::config_seed::ConfigSeedSkipReason::StaleReplica {
            replica_root: Default::default(),
            reason: String::new(),
        },
    );
    let expected = seed_gate(Some(&stale)).expect_err("a stale seed fails the gate");
    assert_eq!(run.result.expect_err("strict seed"), expected);
    assert!(run.backend.specs.lock().unwrap().is_empty());
    assert!(run.backend.preflight_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_hold_blocks_any_launch() {
    for preflight in [Some(preflight_text(0)), None] {
        let held = Mutex::new(None);
        let run = gate_create(preflight.clone(), false, true, |state, cwd| {
            *held.lock().unwrap() = Some(state.try_test(cwd).expect("test hold"));
        })
        .await;
        let error = run.result.expect_err("a test hold blocks the launch");
        assert_eq!(error, refusal_message(HoldRefusal::Test));
        assert!(run.backend.specs.lock().unwrap().is_empty());
        drop(held);

        let run = gate_create(preflight, false, true, |state, cwd| {
            drop(state.try_test(cwd).expect("test hold"));
        })
        .await;
        run.result.expect("a dropped test hold no longer blocks");
        assert_eq!(run.backend.specs.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn test_refused_inside_launch_spawn() {
    let run = gate_create(Some(preflight_text(0)), false, true, |_, _| {}).await;
    run.result.expect("launch");
    assert_eq!(
        *run.backend.test_probes.lock().unwrap(),
        vec![Err(HoldRefusal::Launching)]
    );
    let state = run.state.expect("managed");
    assert!(state.try_test(&run.cwd).is_ok());
}

#[cfg(windows)]
async fn assert_rejected_pending_sessions_drained(
    session_mgr: &tokio::sync::RwLock<SessionManager>,
) {
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
}
