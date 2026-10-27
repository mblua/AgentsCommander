use super::*;
use crate::pty::backend::ResolvedAgentHostShell;
use crate::session::manager::SessionManager;
use std::sync::atomic::Ordering;

fn test_backend() -> (LocalProcessBackend, tauri::App) {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    // GitWatcher takes a plain (Wry) AppHandle; the pty_lifecycle pattern
    // builds a default-runtime mock app the same way.
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build adapter spawn-sync test app");
    let git_watcher = GitWatcher::new(session_mgr, app.handle().clone());
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    (
        LocalProcessBackend::new(output_senders, idle_detector, git_watcher, None),
        app,
    )
}

fn spawn_spec(
    cmd: &str,
    args: &[&str],
    host_shell: Option<ResolvedAgentHostShell>,
) -> BackendSpawnSpec {
    BackendSpawnSpec {
        id: Uuid::new_v4(),
        agent_id: None,
        coding_agent: None,
        cmd: cmd.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        resolved_agent_host_shell: host_shell,
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

fn host(program: &str, args: &[&str]) -> ResolvedAgentHostShell {
    ResolvedAgentHostShell {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn invalid_configured_shell_token_never_reaches_pty() {
    let (backend, _app) = test_backend();
    // A conflicting/terminal configured option for the PowerShell host.
    let spec = spawn_spec(
        "claude",
        &[],
        Some(host("powershell.exe", &["-NoProfile", "-Command"])),
    );
    let id = spec.id;

    let error = backend
        .spawn_sync(spec)
        .expect_err("must reject before PTY");
    assert!(error.to_string().contains("-Command"), "{error}");
    assert!(
        error.to_string().contains("conflicting/terminal"),
        "{error}"
    );
    assert_eq!(
        backend.pre_pty_attempts.load(Ordering::SeqCst),
        0,
        "a rejected configured-host token must never count as a spawn attempt"
    );
    assert!(
        backend.ptys.lock().unwrap().is_empty(),
        "no PTY map entry may exist for a rejected input"
    );
    assert!(
        crate::pty::spawn_diagnostics::record_for(id, backend.spawn.as_ref()).is_none(),
        "no launch provenance may be recorded for a rejected input"
    );
    assert!(
        backend.fanout.get_pty_size(id).is_none(),
        "no output task may be attached for a rejected input"
    );
}

#[test]
fn invalid_cmd_payload_token_never_reaches_pty() {
    let (backend, _app) = test_backend();
    let spec = spawn_spec(
        "claude",
        &["a b".to_string().as_str()],
        Some(host("cmd.exe", &["/D"])),
    );
    let id = spec.id;

    let error = backend
        .spawn_sync(spec)
        .expect_err("must reject before PTY");
    assert!(
        error
            .to_string()
            .contains("unsupported cmd payload character"),
        "{error}"
    );
    assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 0);
    assert!(backend.ptys.lock().unwrap().is_empty());
    assert!(crate::pty::spawn_diagnostics::record_for(id, backend.spawn.as_ref()).is_none());
}

#[test]
fn invalid_explicit_batch_argument_never_reaches_pty() {
    let (backend, _app) = test_backend();
    let spec = spawn_spec(
        r"C:\tools\claude.cmd",
        &["bad%value".to_string().as_str()],
        Some(host("powershell.exe", &["-NoProfile"])),
    );
    let id = spec.id;

    let error = backend
        .spawn_sync(spec)
        .expect_err("must reject before PTY");
    assert!(
        error.to_string().contains("unsupported explicit-batch"),
        "{error}"
    );
    assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 0);
    assert!(backend.ptys.lock().unwrap().is_empty());
    assert!(crate::pty::spawn_diagnostics::record_for(id, backend.spawn.as_ref()).is_none());
}

#[test]
fn blank_configured_program_never_reaches_pty() {
    let (backend, _app) = test_backend();
    let spec = spawn_spec("claude", &[], Some(host("   ", &[])));
    let id = spec.id;

    let error = backend
        .spawn_sync(spec)
        .expect_err("must reject before PTY");
    assert!(
        error
            .to_string()
            .contains("blank configured default-shell program"),
        "{error}"
    );
    assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 0);
    assert!(backend.ptys.lock().unwrap().is_empty());
    assert!(crate::pty::spawn_diagnostics::record_for(id, backend.spawn.as_ref()).is_none());
}

#[test]
fn non_empty_custom_shell_args_never_reaches_pty() {
    let (backend, _app) = test_backend();
    let spec = spawn_spec("claude", &[], Some(host("bash.exe", &["--norc"])));
    let id = spec.id;

    let error = backend
        .spawn_sync(spec)
        .expect_err("must reject before PTY");
    assert!(
        error.to_string().contains("conflicting/terminal"),
        "{error}"
    );
    assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 0);
    assert!(backend.ptys.lock().unwrap().is_empty());
    assert!(crate::pty::spawn_diagnostics::record_for(id, backend.spawn.as_ref()).is_none());
}

#[test]
fn detached_launcher_never_reaches_pty() {
    let (backend, _app) = test_backend();
    let specs = [
        spawn_spec("git-bash.exe", &["--login", "-i"], None),
        spawn_spec(
            "claude",
            &[],
            Some(host("git-bash.exe", &["--login", "-i"])),
        ),
    ];
    for spec in specs {
        let id = spec.id;
        let error = backend
            .spawn_sync(spec)
            .expect_err("must reject before PTY");
        assert!(error.to_string().contains("detached launcher"), "{error}");
        assert_eq!(
            backend.pre_pty_attempts.load(Ordering::SeqCst),
            0,
            "a rejected detached launcher must never count as a spawn attempt"
        );
        assert!(
            backend.ptys.lock().unwrap().is_empty(),
            "no PTY map entry may exist for a rejected launcher"
        );
        assert!(
            crate::pty::spawn_diagnostics::record_for(id, backend.spawn.as_ref()).is_none(),
            "no launch provenance may be recorded for a rejected launcher"
        );
        assert!(
            backend.fanout.get_pty_size(id).is_none(),
            "no output task may be attached for a rejected launcher"
        );
    }
}

#[test]
fn accepted_spawn_increments_observer_and_registers_session() {
    let (backend, _app) = test_backend();
    // Direct .exe configured shell: the ordinary shell-session shape, no
    // adapter. Proves the observer is not vacuously always-zero.
    let spec = spawn_spec("cmd.exe", &["/C", "exit", "0"], None);
    let id = spec.id;

    backend.spawn_sync(spec).expect("valid spawn must succeed");
    assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 1);
    assert!(backend.ptys.lock().unwrap().contains_key(&id));
    assert!(crate::pty::spawn_diagnostics::record_for(id, backend.spawn.as_ref()).is_some());
    assert!(backend.fanout.get_pty_size(id).is_some());

    backend.kill(id).expect("kill spawned child");
    assert!(!backend.ptys.lock().unwrap().contains_key(&id));
}
