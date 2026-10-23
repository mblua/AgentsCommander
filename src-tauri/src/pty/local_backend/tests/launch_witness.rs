use super::*;
use crate::session::manager::SessionManager;

fn test_backend() -> (LocalProcessBackend, tauri::App) {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build launch-witness test app");
    let git_watcher = GitWatcher::new(session_mgr, app.handle().clone());
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    (
        LocalProcessBackend::new(output_senders, idle_detector, git_watcher, None),
        app,
    )
}

fn spawn_spec(cmd: &str, args: &[&str]) -> BackendSpawnSpec {
    BackendSpawnSpec {
        id: Uuid::new_v4(),
        agent_id: None,
        coding_agent: None,
        cmd: cmd.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        resolved_agent_host_shell: None,
        cwd: std::env::temp_dir().to_string_lossy().to_string(),
        selected_cwd: None,
        cols: 80,
        rows: 24,
        container_image: None,
        configured_env: Vec::new(),
        env_remove_keys: Vec::new(),
        env_unset: Vec::new(),
        extra_env: Vec::new(),
        idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
        output_target: crate::pty::output::PtyOutputTarget::noop(),
        resource_registration: None,
        logical_resource_slot: None,
        container_credential: None,
        container_repo_mounts: Vec::new(),
        launch_witness: Default::default(),
    }
}

#[test]
fn local_spawn_witness_unset_when_spawn_command_fails() {
    let (backend, _app) = test_backend();

    // A `.exe` name is launched directly on every OS. Without it, Windows
    // wraps the command in `cmd.exe /C`, which does start (and so is
    // correctly marked launched).
    let spec = spawn_spec("ac-2413-no-such-program-xyz.exe", &[]);
    let witness = spec.launch_witness.clone();
    let error = backend
        .spawn_sync(spec)
        .expect_err("a missing program must fail to spawn");
    assert!(matches!(error, AppError::PtyError(_)), "{error}");
    assert!(!witness.launched(), "no child exists: witness stays unset");

    #[cfg(windows)]
    let spec = spawn_spec("cmd.exe", &["/C", "exit", "0"]);
    #[cfg(not(windows))]
    let spec = spawn_spec("/bin/sh", &["-c", "exit 0"]);
    let id = spec.id;
    let witness = spec.launch_witness.clone();
    backend.spawn_sync(spec).expect("valid spawn must succeed");
    assert!(witness.launched(), "a child exists: witness marked");
    backend.kill(id).expect("kill spawned child");
}
