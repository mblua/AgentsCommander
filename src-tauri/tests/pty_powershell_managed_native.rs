//! #1271 - real-host Windows regressions for the configured default-shell
//! adapter: system `powershell.exe` managed native argv + PTY I/O, PowerShell
//! batch shims, configured cmd to `.cmd`, exit-code propagation, host shutdown,
//! and missing-host/custom-shell cleanup.
//!
//! The system-`powershell.exe` availability rule is explicit (plan Section
//! 6.2 item 7): when `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`
//! exists the full assertion set MUST run, and any mismatch, timeout, or missing
//! reporter behavior is a test FAILURE, never a skip. Only when the canonical
//! file is absent may the test print the reason and return, and that path is
//! visible in the run output.

#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentscommander_lib::commands::session::{
    create_session_inner, destroy_session_inner, CreateSelectionIntent,
};
use agentscommander_lib::config::agent_command::build_agent_spawn_command;
use agentscommander_lib::config::settings::{
    save_settings_with_project_paths, AppSettings, SettingsState,
};
use agentscommander_lib::pty::backend::ResolvedAgentHostShell;
use agentscommander_lib::pty::git_watcher::GitWatcher;
use agentscommander_lib::pty::idle_detector::IdleDetector;
use agentscommander_lib::pty::manager::PtyManager;
use agentscommander_lib::pty::spawn_diagnostics::{self, ChildLiveness};
use agentscommander_lib::resource_monitor::ResourceMonitorState;
use agentscommander_lib::session::manager::SessionManager;
use agentscommander_lib::session::selection::SelectionCoordinator;
use agentscommander_lib::session::session::SessionInfo;
use agentscommander_lib::shutdown::ShutdownSignal;
use agentscommander_lib::telegram::manager::{
    OutputSenderMap, TelegramBridgeManager, TelegramBridgeState,
};
use agentscommander_lib::voice::tracker::{VoiceTracker, VoiceTrackingState};
use agentscommander_lib::web::auth::WebAccessToken;
use agentscommander_lib::web::broadcast::WsBroadcaster;
use agentscommander_lib::{
    AppOutbox, ConfigSeedLockState, DetachedSessionsState, MasterToken, RestoreInProgress,
    SpecBoardState, WebServerHandle,
};
use serde::Deserialize;
use tauri::Listener;
use uuid::Uuid;

const OUTPUT_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(75);

/// The canonical system Windows PowerShell host (fail-not-skip rule).
fn system_powershell_path() -> Option<PathBuf> {
    let system_root = std::env::var("SystemRoot")
        .or_else(|_| std::env::var("WINDIR"))
        .unwrap_or_else(|_| "C:\\Windows".to_string());
    let path = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    path.is_file().then_some(path)
}

fn powershell_required_host() -> String {
    match system_powershell_path() {
        Some(path) => path.to_string_lossy().to_string(),
        None => {
            eprintln!(
                "[1271] SKIP-PRINT: canonical system powershell.exe is absent; \
                 the full assertion set cannot run on this machine"
            );
            std::process::exit(0);
        }
    }
}

static TEST_CONFIG_ENV_LOCK: Mutex<()> = Mutex::new(());

struct TestConfigEnvGuard {
    previous: Option<String>,
}

impl TestConfigEnvGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").ok();
        std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", path);
        Self { previous }
    }
}

impl Drop for TestConfigEnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.as_ref() {
            std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", previous);
        } else {
            std::env::remove_var("AGENTSCOMMANDER_TEST_CONFIG_DIR");
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestPtyOutputPayload {
    session_id: String,
    data: Vec<u8>,
    sequence: Option<u64>,
}

struct Fixture {
    app: tauri::App,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    captured_output: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    listener_errors: Arc<Mutex<Vec<String>>>,
    tracked_sessions: Arc<Mutex<Vec<Uuid>>>,
    _temp: tempfile::TempDir,
    _env_guard: TestConfigEnvGuard,
    _env_lock: std::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    fn output_text(&self, session_id: &str) -> String {
        self.captured_output
            .lock()
            .unwrap()
            .get(session_id)
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .unwrap_or_default()
    }

    fn listener_errors(&self) -> Vec<String> {
        self.listener_errors.lock().unwrap().clone()
    }

    /// Register a session for best-effort PTY teardown at fixture drop. The
    /// kill path forgets the spawn record, so tests must assert provenance and
    /// exit codes BEFORE the fixture is dropped.
    fn track_session(&self, id: Uuid) {
        self.tracked_sessions.lock().unwrap().push(id);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let ids: Vec<Uuid> = self.tracked_sessions.lock().unwrap().clone();
        for id in ids {
            let _ = self.pty_mgr.lock().unwrap().kill(id);
        }
    }
}

fn config_dir_for_test() -> PathBuf {
    std::env::temp_dir().join(format!(
        "agentscommander-pty-powershell-native-{}",
        std::process::id()
    ))
}

fn make_fixture() -> Fixture {
    let env_lock = TEST_CONFIG_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let repo_root = temp.path().join("repo-1271-native");
    let config_dir = config_dir_for_test();
    std::fs::create_dir_all(&repo_root).expect("create repo root");
    // A registered project path must resolve to a real AC project (.ac root).
    std::fs::create_dir_all(repo_root.join(".ac")).expect("create project .ac root");
    std::fs::create_dir_all(repo_root.join(".ac").join("_agent_claude"))
        .expect("create claude agent cwd");
    if config_dir.exists() {
        std::fs::remove_dir_all(&config_dir).expect("reset config dir");
    }
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(repo_root.join(".gitignore"), "*\r\n").expect("seed repo file");

    let env_guard = TestConfigEnvGuard::set(&config_dir);
    assert_eq!(
        agentscommander_lib::config::config_dir().expect("config dir override"),
        config_dir
    );
    save_settings_with_project_paths(&{
        let mut settings = AppSettings::default();
        settings.default_shell = "powershell.exe".to_string();
        settings.default_shell_args = vec!["-NoLogo".to_string()];
        settings.project_paths = vec![repo_root.to_string_lossy().to_string()];
        settings
    })
    .expect("seed isolated settings");

    let captured_output: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
    let listener_errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(Arc::clone(&output_senders)),
    ));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new({
        let mut s = AppSettings::default();
        s.default_shell = "powershell.exe".to_string();
        s.default_shell_args = vec!["-NoLogo".to_string()];
        s.project_paths = vec![repo_root.to_string_lossy().to_string()];
        s
    }));
    let git_app = Box::leak(Box::new(
        tauri::Builder::default()
            .any_thread()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build git watcher handle app"),
    ));
    let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), git_app.handle().clone());
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
        output_senders,
        idle_detector,
        Arc::clone(&git_watcher),
        None,
        None,
    )));

    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice_tracking: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));
    let spec_board_state: SpecBoardState = Arc::new(tokio::sync::RwLock::new(
        agentscommander_lib::commands::spec_board::SpecBoardManager::new(),
    ));
    let config_seed_lock: ConfigSeedLockState = Arc::new(tokio::sync::Mutex::new(()));
    let shutdown_signal = ShutdownSignal::new();
    let selection_coordinator =
        SelectionCoordinator::new(Arc::clone(&session_mgr), shutdown_signal.token().clone());

    let captured = Arc::clone(&captured_output);
    let error_capture = Arc::clone(&listener_errors);
    let app = tauri::Builder::default()
        .any_thread()
        .manage(MasterToken::new("pty-powershell-native-master-token".into()))
        .manage(AppOutbox::new(
            repo_root
                .join(".app-outbox")
                .to_string_lossy()
                .to_string(),
        ))
        .manage(settings)
        .manage(Arc::clone(&session_mgr))
        .manage(selection_coordinator.clone())
        .manage(tg_mgr)
        .manage(detached_sessions)
        .manage(voice_tracking)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .manage(shutdown_signal)
        .manage(Arc::new(WebAccessToken::new("pty-powershell-native-web-token".into())))
        .manage(WsBroadcaster::new())
        .manage(WebServerHandle::default())
        .manage(spec_board_state)
        .manage(config_seed_lock)
        .manage(git_watcher)
        .manage(Arc::new(ResourceMonitorState::new()))
        .manage(Arc::clone(&pty_mgr))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build pty powershell native test app");

    selection_coordinator
        .start(app.handle().clone())
        .expect("start selection coordinator");
    let bootstrap = selection_coordinator.clone();
    std::thread::spawn(move || {
        tauri::async_runtime::block_on(async move {
            bootstrap
                .submit_restore_first()
                .await
                .expect("open selection coordinator")
                .finish();
        });
    })
    .join()
    .expect("join selection bootstrap");

    app.listen_any("pty_output", move |event| {
        let raw = event.payload().to_string();
        match serde_json::from_str::<TestPtyOutputPayload>(&raw) {
            Ok(payload) => {
                captured
                    .lock()
                    .unwrap()
                    .entry(payload.session_id.clone())
                    .or_default()
                    .extend(payload.data);
                if payload.sequence.is_none() {
                    error_capture
                        .lock()
                        .unwrap()
                        .push(format!("pty_output payload missing sequence; raw={raw}"));
                }
            }
            Err(err) => error_capture.lock().unwrap().push(format!(
                "failed to parse pty_output payload: {err}; raw={raw}"
            )),
        }
    });

    Fixture {
        app,
        session_mgr,
        pty_mgr,
        captured_output,
        listener_errors,
        tracked_sessions: Arc::new(Mutex::new(Vec::new())),
        _temp: temp,
        _env_guard: env_guard,
        _env_lock: env_lock,
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn parse_session_id(session: &SessionInfo) -> Uuid {
    Uuid::parse_str(&session.id).expect("session id is uuid")
}

/// A bare logical program (e.g. `claude` or `ac_argv_reporter`) must resolve
/// through the spawned session's PATH, which the child inherits from this
/// process. Agent env rows cannot carry PATH (it is a reserved key), so the
/// tests prepend the fixture directory to the process PATH for the duration of
/// the create call, under a global lock so parallel tests cannot clobber each
/// other. The spawned child keeps the inherited PATH after the guard restores
/// the original value.
static PATH_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct PathEnvGuard {
    previous: String,
}

impl PathEnvGuard {
    fn prepend(dir: &Path) -> Self {
        let previous = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{};{}", path_to_string(dir), previous));
        Self { previous }
    }
}

impl Drop for PathEnvGuard {
    fn drop(&mut self) {
        std::env::set_var("PATH", &self.previous);
    }
}

fn agent_config(id: &str, command: &str) -> agentscommander_lib::config::settings::AgentConfig {
    agentscommander_lib::config::settings::AgentConfig {
        id: id.to_string(),
        label: id.to_string(),
        command: command.to_string(),
        color: "#10b981".to_string(),
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        backend: Default::default(),
    }
}

fn base_settings(host_program: &str, host_args: &[&str], project: &Path) -> AppSettings {
    let mut settings = AppSettings::default();
    settings.default_shell = host_program.to_string();
    settings.default_shell_args = host_args.iter().map(|s| s.to_string()).collect();
    settings.project_paths = vec![path_to_string(project)];
    settings
}

/// Create a session through the same native session/backend path a real agent
/// session uses, with a resolved agent spawn plus the configured host-shell
/// snapshot. Returns the session id.
async fn create_resolved_session(
    fixture: &Fixture,
    settings: &AppSettings,
    agent_id: &str,
    cwd: &str,
    name: &str,
) -> SessionInfo {
    let spawn = build_agent_spawn_command(settings, agent_id, Some(std::path::Path::new(cwd)), None)
        .unwrap_or_else(|e| panic!("resolve agent spawn for {agent_id}: {e}"));
    let host_shell = ResolvedAgentHostShell {
        program: settings.default_shell.clone(),
        args: settings.default_shell_args.clone(),
    };
    create_session_inner(
        fixture.app.handle(),
        &fixture.session_mgr,
        &fixture.pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd.to_string(),
        Some(name.to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        true, // fresh create
        Some(spawn),
        Some(host_shell),
        None, // #973 - no view in this test: 120x30
        CreateSelectionIntent::User,
    )
    .await
    .unwrap_or_else(|e| panic!("create resolved session {name} failed: {e}"))
}

/// Create a plain session (no resolved agent) with an explicit shell/argv and
/// the configured host-shell snapshot, for the cmd-host and missing-host rows.
async fn create_plain_session(
    fixture: &Fixture,
    shell: &str,
    args: &[&str],
    host_shell: Option<ResolvedAgentHostShell>,
    cwd: &str,
    name: &str,
) -> Result<SessionInfo, String> {
    create_session_inner(
        fixture.app.handle(),
        &fixture.session_mgr,
        &fixture.pty_mgr,
        shell.to_string(),
        args.iter().map(|s| s.to_string()).collect(),
        cwd.to_string(),
        Some(name.to_string()),
        None,
        None,
        false,
        Vec::new(),
        true,
        None,
        host_shell,
        None,
        CreateSelectionIntent::User,
    )
    .await
}

async fn wait_for_output(
    fixture: &Fixture,
    session_id: &str,
    marker: &str,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if fixture.output_text(session_id).contains(marker) {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!(
        "timeout waiting for output marker '{marker}' in session {session_id}; output={:?}",
        fixture.output_text(session_id)
    ))
}

/// Poll the spawn record until the awaited child exit is observable. Returns
/// the child's exit code (the host shell propagated it through WaitForExit or
/// cmd /C).
async fn wait_for_exit_code(
    fixture: &Fixture,
    session_id: Uuid,
    timeout: Duration,
) -> Result<u32, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(record) = spawn_diagnostics::record_for(session_id) {
            if let Some(ChildLiveness::Exited { code, .. }) = record.final_liveness() {
                return Ok(code);
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!(
        "timeout waiting for session {session_id} exit; listener_errors={:?}",
        fixture.listener_errors()
    ))
}

async fn wait_for_session_gone(
    fixture: &Fixture,
    session_id: Uuid,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if fixture.pty_mgr.lock().unwrap().context_session_liveness(session_id)
            == agentscommander_lib::pty::context_scrape::ContextSessionLiveness::SessionOver
        {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!("session {session_id} still live after teardown"))
}

fn process_exists(pid: u32) -> bool {
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}"
            ),
        ])
        .status();
    matches!(status, Ok(status) if status.success())
}

fn write_cmd_shim(path: &Path, body: &str) {
    std::fs::write(path, format!("@echo off\r\n{body}\r\n")).expect("write cmd shim");
}

/// `claude.cmd` shim: emits a stable marker and exits with a deterministic code.
fn write_claude_shim(dir: &Path, marker: &str, exit_code: u32) -> PathBuf {
    let path = dir.join("claude.cmd");
    write_cmd_shim(&path, &format!("echo {marker}\r\nexit /b {exit_code}"));
    path
}

/// The reporter binary is built by Cargo as part of any `cargo test` run (bin
/// targets are compiled for integration tests) and located at compile time
/// through `CARGO_BIN_EXE_ac_argv_reporter`.
fn reporter_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ac_argv_reporter"))
}

/// Expected reporter exit code: sum of UTF-8 byte lengths of the logical args,
/// modulo 256 (the reporter's deterministic derivation).
fn reporter_exit_code(args: &[&str]) -> u32 {
    (args.iter().map(|arg| arg.len()).sum::<usize>() % 256) as u32
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Section 6.2 items 1-3: bare `claude` resolving to a `claude.cmd` shim starts
/// with the configured system PowerShell as PTY parent, the shim marker arrives
/// through the normal PTY output, the shim exit code propagates through the
/// host's WaitForExit, and the spawn-record provenance names the configured
/// PowerShell (never cmd.exe).
#[test]
fn configured_powershell_launches_bare_agent_via_cmd_shim() {
    let fixture = make_fixture();
    let temp = fixture._temp.path().to_path_buf();
    let shim_dir = temp.join("shim-powershell");
    std::fs::create_dir_all(&shim_dir).expect("create shim dir");
    let shim_path = write_claude_shim(&shim_dir, "AC_SHIM_MARKER", 23);
    assert!(shim_path.is_file());
    // DIAGNOSTIC: dump PATH for this run.
    write_cmd_shim(&shim_path, "echo AC_SHIM_MARKER
echo PATHDUMP=[%PATH%]
exit /b 23");

    let powershell = powershell_required_host();
    let mut settings = base_settings(&powershell, &["-NoProfile"], &temp.join("repo-1271-native"));
    settings.agents = vec![agent_config("claude", "claude")];

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let _path_lock = PATH_ENV_LOCK.lock().await;
        let _path_guard = PathEnvGuard::prepend(&shim_dir);
        let proc_path = std::env::var("PATH").unwrap_or_default();
        assert!(
            proc_path.starts_with(&path_to_string(&shim_dir)),
            "PATH guard must prepend the shim dir: {proc_path}"
        );
        let created = create_resolved_session(
            &fixture,
            &settings,
            "claude",
            temp.join("repo-1271-native")
                .join(".ac")
                .join("_agent_claude")
                .to_string_lossy()
                .as_ref(),
            "1271 powershell shim",
        )
        .await;
        drop(_path_guard);
        drop(_path_lock);
        let id = parse_session_id(&created);
        fixture.track_session(id);

        // Provenance: the configured PowerShell path, not cmd.exe.
        let record = spawn_diagnostics::record_for(id).expect("spawn record exists");
        let argv = record.argv();
        assert_eq!(
            argv.first().map(String::as_str),
            Some(powershell.as_str()),
            "provenance must name the configured PowerShell: {argv:?}"
        );
        assert!(!argv[0].to_lowercase().ends_with("cmd.exe"), "{argv:?}");
        assert_eq!(argv.get(1).map(String::as_str), Some("-NoProfile"));
        assert_eq!(argv.get(2).map(String::as_str), Some("-Command"));
        let script = argv.get(3).expect("generated script in provenance");
        assert!(
            script.contains("GetCommand('claude', $ac_kind)"),
            "script must use the two-argument application-only lookup"
        );
        assert!(
            script.contains("$ac_start.FileName = $ac_command.Path"),
            "script must carry the managed native-application branch"
        );
        assert!(
            script.contains(
                "[System.IO.Path]::Combine([System.Environment]::SystemDirectory, 'cmd.exe')"
            ),
            "script must carry the resolved-batch managed system-cmd child"
        );
        assert!(!script.contains("--%"), "script must never emit --%");

        // Marker through the normal PTY output fanout.
        wait_for_output(&fixture, &created.id, "AC_SHIM_MARKER", OUTPUT_TIMEOUT)
            .await
            .expect("shim marker must arrive through the PTY output path");

        // The host exits with the shim's code (WaitForExit propagation).
        let code = wait_for_exit_code(&fixture, id, EXIT_TIMEOUT)
            .await
            .expect("shim exit code must propagate");
        assert_eq!(code, 23, "nested shim exit code must propagate exactly");
    });
}

/// Section 6.2 item 7: the repository-owned `ac_argv_reporter` launched as a
/// bare native application through the generated `-Command`/`ProcessStartInfo`
/// branch. Empty arg, quote, apostrophe, space, terminal backslash, percent,
/// pipeline, and backslash-before-quote values are delivered exactly once;
/// a distinct marker sent through `PtyManager::write` is acknowledged through
/// the normal PTY output; the derived exit code propagates. Runs against the
/// system `powershell.exe`; fails rather than skips when that host exists.
#[test]
fn configured_powershell_managed_native_reporter_argv_and_pty_io() {
    let fixture = make_fixture();
    let temp = fixture._temp.path().to_path_buf();
    let reporter_dir = reporter_bin()
        .parent()
        .expect("reporter parent dir")
        .to_path_buf();
    let reporter_name = "ac_argv_reporter".to_string();
    let logical_args: Vec<&str> = vec![
        "",
        "a\"b",
        "with space",
        "o'clock",
        "tail\\",
        "a%z|p",
        "a\\b\"c",
        "a\\\"",
        "a\\\\\"c",
    ];

    let powershell = powershell_required_host();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        // The reporter directory is prepended to the spawned session PATH (the
        // session cwd is an agent dir, so the git-guard env carries the process
        // PATH into the child); the logical argv is passed exactly as configured.
        let _path_lock = PATH_ENV_LOCK.lock().await;
        let _path_guard = PathEnvGuard::prepend(&reporter_dir);
        let created = create_plain_session(
            &fixture,
            &reporter_name,
            &logical_args[..],
            Some(ResolvedAgentHostShell {
                program: powershell.clone(),
                args: vec!["-NoProfile".to_string()],
            }),
            temp.join("repo-1271-native")
                .join(".ac")
                .join("_agent_claude")
                .to_string_lossy()
                .as_ref(),
            "1271 managed native reporter",
        )
        .await
        .expect("reporter session create must succeed");
        drop(_path_guard);
        drop(_path_lock);
        let id = parse_session_id(&created);
        fixture.track_session(id);

        let record = spawn_diagnostics::record_for(id).expect("spawn record exists");
        let argv = record.argv();
        assert_eq!(
            argv.first().map(String::as_str),
            Some(powershell.as_str()),
            "provenance must name the configured PowerShell: {argv:?}"
        );
        assert_eq!(argv.get(1).map(String::as_str), Some("-NoProfile"));
        let script = argv.get(3).expect("generated script in provenance");
        assert!(!script.contains("--%"), "no stop-parsing token anywhere");
        assert!(script.contains("$ac_start.FileName = $ac_command.Path"));
        assert!(script.contains("$ac_process.WaitForExit()"));

        // Exact argv delivery through the managed native child: each value is
        // printed on its own length-prefixed line.
        let expected_lines: Vec<String> = logical_args
            .iter()
            .map(|arg| format!("{}:{}\r", arg.len(), arg))
            .collect();
        for line in &expected_lines {
            wait_for_output(&fixture, &created.id, line, OUTPUT_TIMEOUT)
                .await
                .unwrap_or_else(|e| {
                    panic!("{e}\nargv line missing: {line:?}\noutput={:?}", fixture.output_text(&created.id))
                });
        }

        // PTY input path: a distinct marker through PtyManager::write must be
        // echoed back by the reporter through the normal PTY output fanout.
        let permit = PtyManager::acquire_input_writer(&fixture.pty_mgr, id)
            .await
            .expect("acquire input writer");
        PtyManager::write_with_permit(&permit, b"AC_PTY_MARKER\r\n")
            .expect("write marker through the PTY input path");
        drop(permit);
        wait_for_output(&fixture, &created.id, "AC_PTY_MARKER", OUTPUT_TIMEOUT)
            .await
            .expect("reporter must echo the PTY-written marker back");

        // Exit: the reporter stops on the control line and exits with the
        // deterministic derived code; the PowerShell host propagates it.
        let permit = PtyManager::acquire_input_writer(&fixture.pty_mgr, id)
            .await
            .expect("acquire input writer for stop");
        PtyManager::write_with_permit(&permit, b"AC_1271_STOP\r\n")
            .expect("write stop control line");
        drop(permit);
        let expected_code = reporter_exit_code(&logical_args);
        let code = wait_for_exit_code(&fixture, id, EXIT_TIMEOUT)
            .await
            .expect("reporter derived exit code must propagate");
        assert_eq!(code, expected_code, "derived exit code must match");

        assert!(
            fixture.listener_errors().is_empty(),
            "pty_output listener must stay healthy: {:?}",
            fixture.listener_errors()
        );
    });
}

/// Section 6.2 item 8: bare `claude` resolving to batch uses the nested
/// system-cmd protocol; an explicit `.cmd` program with `%`/`"` in an argument
/// fails before PTY creation; a bare name resolving to batch with an
/// unsupported logical argument fails nonzero after lookup and cleans up.
#[test]
fn configured_powershell_batch_regression() {
    let fixture = make_fixture();
    let temp = fixture._temp.path().to_path_buf();
    let shim_dir = temp.join("shim-batch");
    std::fs::create_dir_all(&shim_dir).expect("create shim dir");
    write_claude_shim(&shim_dir, "AC_BATCH_MARKER", 17);

    let powershell = powershell_required_host();
    let mut settings = base_settings(&powershell, &["-NoProfile"], &temp.join("repo-1271-native"));
    settings.agents = vec![agent_config("claude", "claude")];

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        // Happy path: bare claude resolving to claude.cmd, nested cmd protocol.
        let _path_lock = PATH_ENV_LOCK.lock().await;
        let _path_guard = PathEnvGuard::prepend(&shim_dir);
        let created = create_resolved_session(
            &fixture,
            &settings,
            "claude",
            temp.join("repo-1271-native")
                .join(".ac")
                .join("_agent_claude")
                .to_string_lossy()
                .as_ref(),
            "1271 batch happy path",
        )
        .await;
        drop(_path_guard);
        drop(_path_lock);
        let id = parse_session_id(&created);
        fixture.track_session(id);
        wait_for_output(&fixture, &created.id, "AC_BATCH_MARKER", OUTPUT_TIMEOUT)
            .await
            .expect("batch shim marker must arrive through PTY output");
        let code = wait_for_exit_code(&fixture, id, EXIT_TIMEOUT)
            .await
            .expect("batch shim exit code must propagate");
        assert_eq!(code, 17, "nested batch shim exit code must propagate exactly");

        // Pre-PTY rejection: explicit .cmd program with '%' in an argument.
        let explicit = shim_dir.join("explicit.cmd");
        write_claude_shim(&shim_dir, "AC_EXPLICIT", 3);
        let explicit_path = path_to_string(&explicit);
        let err = create_plain_session(
            &fixture,
            &explicit_path,
            &["bad%arg"],
            Some(ResolvedAgentHostShell {
                program: powershell.clone(),
                args: vec!["-NoProfile".to_string()],
            }),
            temp.join("repo-1271-native")
                .join(".ac")
                .join("_agent_claude")
                .to_string_lossy()
                .as_ref(),
            "1271 explicit batch rejection",
        )
        .await
        .expect_err("explicit .cmd with '%' argument must fail before PTY");
        assert!(
            err.contains("unsupported explicit-batch"),
            "pre-PTY rejection must name the explicit-batch rule: {err}"
        );
        assert!(
            fixture.session_mgr.read().await.list_sessions().await.is_empty()
                || !fixture
                    .session_mgr
                    .read()
                    .await
                    .list_sessions()
                    .await
                    .iter()
                    .any(|s| s.name == "1271 explicit batch rejection"),
            "no session may survive the pre-PTY rejection"
        );

        // Post-lookup runtime rejection: bare claude + unsupported logical
        // argument resolves to batch, the script rejects, host exits nonzero.
        let mut settings2 = settings.clone();
        settings2.agents = vec![agent_config("claude", "claude \"bad%arg\"")];
        let _path_lock2 = PATH_ENV_LOCK.lock().await;
        let _path_guard2 = PathEnvGuard::prepend(&shim_dir);
        let created2 = create_resolved_session(
            &fixture,
            &settings2,
            "claude",
            temp.join("repo-1271-native")
                .join(".ac")
                .join("_agent_claude")
                .to_string_lossy()
                .as_ref(),
            "1271 batch runtime rejection",
        )
        .await;
        drop(_path_guard2);
        drop(_path_lock2);
        let id2 = parse_session_id(&created2);
        fixture.track_session(id2);
        let code2 = wait_for_exit_code(&fixture, id2, EXIT_TIMEOUT)
            .await
            .expect("runtime batch rejection must exit nonzero");
        assert_ne!(code2, 0, "bare-name batch incompatibility must be nonzero");
        // Cleanup: the session tears down without a stale PTY.
        destroy_session_inner(fixture.app.handle(), id2)
            .await
            .expect("destroy runtime-rejected session");
        wait_for_session_gone(&fixture, id2, EXIT_TIMEOUT)
            .await
            .expect("no stale PTY may survive");
    });
}

/// Section 6.2 item 5: configured cmd launches a `.cmd` shim with flag-style
/// values, `!` (literal under /V:OFF), and internal backslashes as one literal
/// argument each; the shim exit code propagates. Out-of-domain payload forms
/// fail the create before any PTY is opened.
#[test]
fn configured_cmd_host_shim_argv_and_pre_pty_rejections() {
    let fixture = make_fixture();
    let temp = fixture._temp.path().to_path_buf();
    let shim_dir = temp.join("shim-cmd");
    std::fs::create_dir_all(&shim_dir).expect("create shim dir");
    let shim_path = shim_dir.join("cmdprobe.cmd");
    write_cmd_shim(&shim_path, "echo CMD_PROBE_MARKER\r\necho [%*]\r\nexit /b 7");

    let cmd_host = std::env::var("ComSpec").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_string());
    let host_shell = ResolvedAgentHostShell {
        program: cmd_host.clone(),
        args: vec!["/D".to_string()],
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let cwd = temp.join("repo-1271-native").to_string_lossy().to_string();
        let created = create_plain_session(
            &fixture,
            &path_to_string(&shim_path),
            &["--flag", "!bang!", r"a\b"],
            Some(host_shell.clone()),
            &cwd,
            "1271 cmd shim",
        )
        .await
        .expect("cmd-host shim launch must succeed");
        let id = parse_session_id(&created);
        fixture.track_session(id);

        let record = spawn_diagnostics::record_for(id).expect("spawn record exists");
        let argv = record.argv();
        assert_eq!(
            argv.first().map(String::as_str),
            Some(cmd_host.as_str()),
            "provenance must name the configured cmd: {argv:?}"
        );
        assert_eq!(argv.get(1).map(String::as_str), Some("/D"));
        assert_eq!(
            argv.get(2..5),
            Some(&["/V:OFF".to_string(), "/S".to_string(), "/C".to_string()][..])
        );

        wait_for_output(&fixture, &created.id, "CMD_PROBE_MARKER", OUTPUT_TIMEOUT)
            .await
            .expect("cmd shim marker must arrive through PTY output");
        // One literal argument each: the shim echoes [%*].
        wait_for_output(&fixture, &created.id, "[--flag !bang! a\\b]", OUTPUT_TIMEOUT)
            .await
            .expect("flag-style, bang, and internal-backslash values must be one literal argument each");
        let code = wait_for_exit_code(&fixture, id, EXIT_TIMEOUT)
            .await
            .expect("cmd shim exit code must propagate");
        assert_eq!(code, 7, "cmd shim exit code must propagate exactly");

        // Pre-PTY rejection for out-of-domain payload forms, through the same
        // native creation path.
        for (args, fragment) in [
            (vec!["with space"], "unsupported cmd payload character"),
            (vec!["a&b"], "unsupported cmd payload character"),
            (vec!["a%b"], "unsupported cmd payload character"),
            (vec!["a=b"], "unsupported cmd payload character"),
            (vec![r"tail\"], "unsupported cmd payload character"),
            (vec![""], "unsupported cmd payload character"),
        ] {
            let err = create_plain_session(
                &fixture,
                &path_to_string(&shim_path),
                &args,
                Some(host_shell.clone()),
                &cwd,
                "1271 cmd rejection",
            )
            .await
            .expect_err("out-of-domain cmd payload must fail before PTY");
            assert!(err.contains(fragment), "expected '{fragment}' in: {err}");
        }
        assert!(
            fixture
                .session_mgr
                .read()
                .await
                .list_sessions()
                .await
                .iter()
                .all(|s| s.name != "1271 cmd rejection"),
            "no rejected cmd session may appear"
        );
    });
}

/// Section 6.2 item 6: a nonexistent logical agent program yields a nonzero
/// host completion; a stale or absent LASTEXITCODE cannot be reported as
/// success (the script resets it and exits 1 on lookup failure).
#[test]
fn configured_powershell_nonexistent_agent_fails_nonzero() {
    let fixture = make_fixture();
    let temp = fixture._temp.path().to_path_buf();
    let powershell = powershell_required_host();
    // A bare name that resolves nowhere on any PATH: no such executable or shim
    // exists on this machine, so the application-only lookup must fail and the
    // script must exit 1 (never a stale-LASTEXITCODE success).
    let missing_name = "ac_1271_missing_agent_xyz";

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let created = create_plain_session(
            &fixture,
            missing_name,
            &[],
            Some(ResolvedAgentHostShell {
                program: powershell.clone(),
                args: vec!["-NoProfile".to_string()],
            }),
            temp.join("repo-1271-native")
                .join(".ac")
                .join("_agent_claude")
                .to_string_lossy()
                .as_ref(),
            "1271 nonexistent agent",
        )
        .await
        .expect("create with missing agent must succeed at the PTY level");
        let id = parse_session_id(&created);
        fixture.track_session(id);
        let code = wait_for_exit_code(&fixture, id, EXIT_TIMEOUT)
            .await
            .expect("lookup failure must produce a host exit");
        assert_ne!(code, 0, "a missing agent must never report success");
    });
}

/// Section 6.1 host-shutdown row: stopping a long-running PowerShell-hosted
/// agent through the existing session control kills/reaps the tracked host.
#[test]
fn configured_powershell_host_shutdown_reaps_agent() {
    let fixture = make_fixture();
    let temp = fixture._temp.path().to_path_buf();
    let shim_dir = temp.join("shim-long");
    std::fs::create_dir_all(&shim_dir).expect("create shim dir");
    let shim_path = shim_dir.join("claude.cmd");
    write_cmd_shim(&shim_path, "echo AC_LONG_MARKER\r\nping -n 60 127.0.0.1 > nul");

    let powershell = powershell_required_host();
    let mut settings = base_settings(&powershell, &["-NoProfile"], &temp.join("repo-1271-native"));
    settings.agents = vec![agent_config("claude", "claude")];

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let _path_lock = PATH_ENV_LOCK.lock().await;
        let _path_guard = PathEnvGuard::prepend(&shim_dir);
        let created = create_resolved_session(
            &fixture,
            &settings,
            "claude",
            temp.join("repo-1271-native")
                .join(".ac")
                .join("_agent_claude")
                .to_string_lossy()
                .as_ref(),
            "1271 long-running host",
        )
        .await;
        drop(_path_guard);
        drop(_path_lock);
        let id = parse_session_id(&created);
        wait_for_output(&fixture, &created.id, "AC_LONG_MARKER", OUTPUT_TIMEOUT)
            .await
            .expect("long-running shim must start");

        let record = spawn_diagnostics::record_for(id).expect("spawn record exists");
        let host_pid = record
            .pid()
            .expect("the tracked host pid is available");
        assert!(process_exists(host_pid), "PowerShell host must be running");

        destroy_session_inner(fixture.app.handle(), id)
            .await
            .expect("destroy long-running host session");
        wait_for_session_gone(&fixture, id, EXIT_TIMEOUT)
            .await
            .expect("no stale PTY may survive the destroy");

        let deadline = Instant::now() + EXIT_TIMEOUT;
        while Instant::now() < deadline {
            if !process_exists(host_pid) {
                return;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        panic!("tracked PowerShell host {host_pid} still alive after destroy");
    });
}

/// Section 6.2 item 9: a missing configured host executable fails after the PTY
/// pair exists and leaves no session state; an incompatible custom `-c` host is
/// characterized (the host receives `-c` + script) and exits nonzero.
#[test]
fn configured_missing_host_and_incompatible_custom_shell_cleanup() {
    let fixture = make_fixture();
    let temp = fixture._temp.path().to_path_buf();
    let cwd = temp.join("repo-1271-native").to_string_lossy().to_string();
    let missing_host = temp.join("missing").join("pwsh.exe");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        // Missing host: spawn fails, no session survives, no stale PTY.
        let err = create_plain_session(
            &fixture,
            "claude",
            &[],
            Some(ResolvedAgentHostShell {
                program: path_to_string(&missing_host),
                args: vec!["-NoProfile".to_string()],
            }),
            &cwd,
            "1271 missing host",
        )
        .await
        .expect_err("missing host executable must fail the create");
        assert!(err.contains("PTY error") || err.contains("pty"), "{err}");
        assert!(
            fixture
                .session_mgr
                .read()
                .await
                .list_sessions()
                .await
                .iter()
                .all(|s| s.name != "1271 missing host"),
            "no session may survive a missing host"
        );

        // Incompatible custom shell: the ac_argv_reporter does not honor -c, so
        // the spawn succeeds but the agent never runs; the host receives
        // `-c <script>` as its argv and exits with its own derived code (nonzero).
        let reporter_dir = reporter_bin()
            .parent()
            .expect("reporter parent dir")
            .to_path_buf();
        let reporter_path = path_to_string(&reporter_bin());
        let created = create_plain_session(
            &fixture,
            "claude",
            &[],
            Some(ResolvedAgentHostShell {
                program: reporter_path.clone(),
                args: Vec::new(),
            }),
            &cwd,
            "1271 custom shell",
        )
        .await
        .expect("custom host spawn itself must succeed");
        let id = parse_session_id(&created);
        fixture.track_session(id);
        // The reporter prints its argv: `-c` and the exec script.
        wait_for_output(&fixture, &created.id, "-c", OUTPUT_TIMEOUT)
            .await
            .expect("custom host must receive -c");
        wait_for_output(&fixture, &created.id, "exec 'claude'", OUTPUT_TIMEOUT)
            .await
            .expect("custom host must receive the exec script");
        // The reporter host blocks echoing stdin until the control line; send it
        // so the host exits with its own derived (nonzero) code.
        let permit = PtyManager::acquire_input_writer(&fixture.pty_mgr, id)
            .await
            .expect("acquire input writer for custom host");
        PtyManager::write_with_permit(&permit, b"AC_1271_STOP\r\n").expect("stop custom host");
        drop(permit);
        let code = wait_for_exit_code(&fixture, id, EXIT_TIMEOUT)
            .await
            .expect("custom host must exit");
        assert_ne!(code, 0, "an incompatible -c host must exit nonzero");

        let _ = reporter_dir;
        let _ = code;
        destroy_session_inner(fixture.app.handle(), id)
            .await
            .expect("destroy custom-shell session");
        wait_for_session_gone(&fixture, id, EXIT_TIMEOUT)
            .await
            .expect("no stale PTY may survive the custom-shell session");
    });
}
