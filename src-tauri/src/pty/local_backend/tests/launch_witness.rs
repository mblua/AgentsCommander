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
        args: args.iter().map(std::string::ToString::to_string).collect(),
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

// #3516 tests 12-20: preflight launch preparation and the contained runner end to end.

fn preflight_request(cwd: &str) -> PreflightLaunchRequest {
    PreflightLaunchRequest {
        program: "tool".to_string(),
        args: Vec::new(),
        cwd: cwd.to_string(),
        configured_env: vec![("AC_PF_CFG".to_string(), "c".to_string())],
        env_remove_keys: Vec::new(),
        extra_env: vec![("AGENTSCOMMANDER_TOKEN".to_string(), "t".to_string())],
        resolved_agent_host_shell: None,
    }
}

fn env_value<'a>(launch: &'a PreflightLaunch, key: &str) -> Option<&'a str> {
    launch
        .env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

#[test]
fn full_env_has_configured_credential_and_term() {
    let (backend, _app) = test_backend();
    let dir = tempfile::tempdir().expect("tempdir");
    let request = preflight_request(&dir.path().to_string_lossy());
    let launch = backend.prepare_preflight_launch(request).expect("launch");
    assert_eq!(env_value(&launch, "AC_PF_CFG"), Some("c"));
    assert_eq!(env_value(&launch, "AGENTSCOMMANDER_TOKEN"), Some("t"));
    assert!(launch
        .env
        .contains(&("TERM".to_string(), "xterm-256color".to_string())));
}

#[test]
fn removed_key_is_absent() {
    let (backend, _app) = test_backend();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut request = preflight_request(&dir.path().to_string_lossy());
    request.env_remove_keys = vec!["AC_PF_ABSENT_3516".to_string()];
    let launch = backend.prepare_preflight_launch(request).expect("launch");
    assert_eq!(env_value(&launch, "AC_PF_ABSENT_3516"), None);
    assert_eq!(env_value(&launch, "AC_PF_CFG"), Some("c"));

    let mut request = preflight_request(&dir.path().to_string_lossy());
    request.env_remove_keys = vec!["PATH".to_string()];
    let launch = backend.prepare_preflight_launch(request).expect("launch");
    assert_eq!(env_value(&launch, "PATH"), None);
}

#[test]
fn cwd_is_normalized() {
    let (backend, _app) = test_backend();
    let dir = tempfile::tempdir().expect("tempdir");
    let plain = dir.path().to_string_lossy().to_string();
    let input = if cfg!(windows) {
        format!(r"\\?\{plain}")
    } else {
        plain.clone()
    };
    let launch = backend
        .prepare_preflight_launch(preflight_request(&input))
        .expect("launch");
    assert_eq!(
        launch.cwd,
        crate::path_utils::normalize_windows_verbatim_path(&input)
    );
    if !cfg!(windows) {
        assert_eq!(launch.cwd, input);
    }
}

#[test]
fn argv_matches_cli_adapter() {
    let (backend, _app) = test_backend();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut request = preflight_request(&dir.path().to_string_lossy());
    request.args = vec!["a b".to_string()];
    let launch = backend.prepare_preflight_launch(request).expect("launch");
    if cfg!(windows) {
        assert_eq!(launch.program, "cmd.exe");
        assert_eq!(launch.args[0], "/C");
        assert_eq!(launch.windows_raw_args.as_deref(), Some("/C tool \"a b\""));
    } else {
        assert_eq!(launch.program, "tool");
        assert_eq!(launch.args, vec!["a b".to_string()]);
        assert_eq!(launch.windows_raw_args, None);
    }
}

#[test]
fn pty_quoted_matches_portable_pty() {
    let table: &[(&str, &str)] = &[
        ("abc", "abc"),
        ("", "\"\""),
        ("a b", "\"a b\""),
        ("a\"b", "\"a\\\"b\""),
        ("a\\b", "a\\b"),
        ("a b\\", "\"a b\\\\\""),
        ("a\\\"b", "\"a\\\\\\\"b\""),
        ("a\tb", "\"a\tb\""),
        ("a\u{0b}b", "\"a\u{0b}b\""),
    ];
    for (input, expected) in table {
        assert_eq!(
            crate::pty::local_backend::preflight::pty_quoted(input),
            *expected,
            "{input:?}"
        );
    }
}

#[cfg(windows)]
fn script_request(
    root: &std::path::Path,
    dir_name: &str,
    host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
) -> PreflightLaunchRequest {
    let dir = root.join(dir_name);
    std::fs::create_dir(&dir).expect("create script dir");
    let script = dir.join("pre.cmd");
    std::fs::write(&script, "@echo pf-out & exit /b 7\r\n").expect("write script");
    let mut request = preflight_request(&root.to_string_lossy());
    request.program = script.to_string_lossy().to_string();
    request.resolved_agent_host_shell = host_shell;
    request
}

#[cfg(windows)]
async fn run_spaced_script(host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>) {
    run_script_in("a b", host_shell).await;
}

#[cfg(windows)]
async fn run_script_in(
    dir_name: &str,
    host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
) {
    let (backend, _app) = test_backend();
    let root = tempfile::tempdir().expect("tempdir");
    let request = script_request(root.path(), dir_name, host_shell);
    let launch = backend.prepare_preflight_launch(request).expect("launch");
    let raw = launch.windows_raw_args.clone();
    let spec = crate::process::agent::ContainedRunSpec {
        program: launch.program,
        args: launch.args,
        windows_raw_args: launch.windows_raw_args,
        cwd: launch.cwd,
        env: launch.env,
        timeout: std::time::Duration::from_secs(30),
    };
    let (_tx, mut cancel) = tokio::sync::watch::channel(false);
    let run = crate::process::agent::run_contained(spec, &mut cancel).await;
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert_eq!(
        run.end,
        crate::process::agent::ContainedRunEnd::Exited(7),
        "line {raw:?} stdout {stdout:?}"
    );
    assert!(stdout.contains("pf-out"), "line {raw:?} stdout {stdout:?}");
}

#[cfg(windows)]
#[tokio::test]
async fn windows_cmd_default_runs_spaced_script() {
    run_spaced_script(None).await;
}

#[cfg(windows)]
#[tokio::test]
async fn windows_powershell_adapter_runs_spaced_script() {
    run_spaced_script(Some(crate::pty::backend::ResolvedAgentHostShell {
        program: "powershell.exe".to_string(),
        args: vec!["-NoProfile".to_string()],
    }))
    .await;
}

#[cfg(windows)]
#[tokio::test]
async fn windows_cmd_adapter_matches_cli() {
    let cmd = || {
        Some(crate::pty::backend::ResolvedAgentHostShell {
            program: "cmd.exe".to_string(),
            args: Vec::new(),
        })
    };
    let (backend, _app) = test_backend();
    let root = tempfile::tempdir().expect("tempdir");
    let error = backend
        .prepare_preflight_launch(script_request(root.path(), "a b", cmd()))
        .expect_err("cmd adapter refuses a spaced program, as the CLI does");
    let text = error.to_string();
    assert!(text.contains("cannot host a resolved agent"), "{text}");
    assert!(text.contains("whitespace splits tokens in cmd"), "{text}");
    run_script_in("nospace", cmd()).await;
}

#[test]
fn default_trait_method_refuses() {
    let (backend, _app) = test_backend();
    let dir = tempfile::tempdir().expect("tempdir");
    let request = preflight_request(&dir.path().to_string_lossy());
    let error = crate::pty::manager::PtyManager::new_for_test(Arc::new(backend))
        .backend_for_kind(crate::pty::backend::SessionBackendKind::ContainerTransport)
        .prepare_preflight_launch(request)
        .expect_err("container backend keeps the default refusal");
    assert!(
        error
            .to_string()
            .contains("Preflights run only for local agents."),
        "{error}"
    );
}
