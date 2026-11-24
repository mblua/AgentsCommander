use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentscommander_lib::commands::session::{create_session_inner, CreateSelectionIntent};
use agentscommander_lib::config::agent_command::build_agent_spawn_command;
use agentscommander_lib::config::settings::{save_settings, AppSettings, SettingsState};
use agentscommander_lib::pty::backend::ResolvedAgentHostShell;
use agentscommander_lib::pty::git_watcher::GitWatcher;
use agentscommander_lib::pty::manager::PtyManager;
use agentscommander_lib::pty::spawn_diagnostics::{self, ChildLiveness};
use agentscommander_lib::session::manager::SessionManager;
use agentscommander_lib::session::session::SessionInfo;
use agentscommander_lib::telegram::manager::{
    OutputSenderMap, TelegramBridgeManager, TelegramBridgeState,
};
use tauri::Listener;
use uuid::Uuid;

pub(crate) use super::pty_fixture::parse_session_id;
use super::pty_fixture::{
    build_test_app, git_watcher_and_pty, wait_for_snapshot_contains as shared_wait, TestAppStates,
    TestConfigEnvGuard, TestPtyOutputDelivery, TEST_CONFIG_ENV_LOCK,
};

const OUTPUT_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const EXIT_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(75);

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

pub(crate) fn powershell_required_host() -> Option<String> {
    system_powershell_path().map(|path| path.to_string_lossy().to_string())
}

pub(crate) struct Fixture {
    pub(crate) spawn: Arc<agentscommander_lib::pty::spawn_diagnostics::SpawnRegistry>,
    pub(crate) app: tauri::App,
    pub(crate) session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(crate) pty_mgr: Arc<Mutex<PtyManager>>,
    captured_deliveries: Arc<Mutex<HashMap<String, usize>>>,
    listener_errors: Arc<Mutex<Vec<String>>>,
    tracked_sessions: Arc<Mutex<Vec<Uuid>>>,
    pub(crate) _temp: tempfile::TempDir,
    _env_guard: TestConfigEnvGuard,
    _env_lock: std::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    pub(crate) fn listener_errors(&self) -> Vec<String> {
        self.listener_errors.lock().unwrap().clone()
    }

    /// Register a session for best-effort PTY teardown at fixture drop. The
    /// kill path forgets the spawn record, so tests must assert provenance and
    /// exit codes BEFORE the fixture is dropped.
    pub(crate) fn track_session(&self, id: Uuid) {
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

fn prepare_fixture_dirs(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
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
    (repo_root, config_dir)
}

fn seed_and_verify_settings(repo_root: &Path, config_dir: &Path) {
    let root = config_dir.join(agentscommander_lib::config::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(root).expect("Root fixture creation succeeds");
    let mut settings = AppSettings::default();
    settings.default_shell = "powershell.exe".to_string();
    settings.default_shell_args = vec!["-NoLogo".to_string()];
    settings.project_paths = vec![repo_root.to_string_lossy().to_string()];
    let project_paths_path = config_dir.join("project-paths.json");
    let seed = serde_json::to_vec(&serde_json::json!({
        "projectPaths": settings.project_paths,
        "projectPathsRelativeToInstance": [null],
        "archivedProjectPaths": [],
        "archivedProjectPathsRelativeToInstance": []
    }))
    .expect("serialize canonical project fixture");
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&project_paths_path)
            .expect("create isolated project authority");
        file.write_all(&seed)
            .expect("write canonical project fixture");
    }
    let project_paths_before = std::fs::read(&project_paths_path).expect("read seeded authority");
    let saved = save_settings(&settings).expect("seed isolated settings");
    let canonical_repo_root = std::fs::canonicalize(repo_root)
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", repo_root.display()));
    let canonical_repo_root = canonical_repo_root.to_str().unwrap_or_else(|| {
        panic!(
            "canonical repo root is not UTF-8: {}",
            canonical_repo_root.display()
        )
    });
    let expected_project_path = if let Some(unc) = canonical_repo_root.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(drive_path) = canonical_repo_root.strip_prefix(r"\\?\") {
        let bytes = drive_path.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'\\'
        {
            drive_path.to_string()
        } else {
            canonical_repo_root.to_string()
        }
    } else {
        canonical_repo_root.to_string()
    };
    let expected_project_paths = vec![expected_project_path];
    assert_eq!(saved.project_paths, expected_project_paths);
    assert_eq!(saved.default_shell, "powershell.exe");
    assert_eq!(saved.default_shell_args, vec!["-NoLogo".to_string()]);
    assert_eq!(
        std::fs::read(&project_paths_path).expect("read preserved authority"),
        project_paths_before
    );
    let managed: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config_dir.join("settings.30.instance.no-git.json"))
            .expect("read managed settings"),
    )
    .expect("parse managed settings");
    for key in [
        "projectPath",
        "projectPathRelativeToInstance",
        "projectPaths",
        "projectPathsRelativeToInstance",
        "archivedProjectPaths",
        "archivedProjectPathsRelativeToInstance",
    ] {
        assert!(
            managed.get(key).is_none(),
            "managed settings retained {key}"
        );
    }
}

fn build_settings_git_watcher_and_pty(
    owners: &agentscommander_lib::AppOwners,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    output_senders: OutputSenderMap,
    repo_root: &Path,
) -> (SettingsState, Arc<GitWatcher>, Arc<Mutex<PtyManager>>) {
    let settings: SettingsState = agentscommander_lib::config::settings::new_settings_state({
        let mut s = AppSettings::default();
        s.default_shell = "powershell.exe".to_string();
        s.default_shell_args = vec!["-NoLogo".to_string()];
        s.project_paths = vec![repo_root.to_string_lossy().to_string()];
        s
    });
    let (git_watcher, pty_mgr) = git_watcher_and_pty(owners, session_mgr, output_senders);
    (settings, git_watcher, pty_mgr)
}

fn listen_pty_output(
    app: &tauri::App,
    deliveries: Arc<Mutex<HashMap<String, usize>>>,
    error_capture: Arc<Mutex<Vec<String>>>,
) {
    app.listen_any("pty_output", move |event| {
        let raw = event.payload().to_string();
        match serde_json::from_str::<TestPtyOutputDelivery>(&raw) {
            Ok(TestPtyOutputDelivery::Data { session_id, .. })
            | Ok(TestPtyOutputDelivery::ResyncRequired { session_id, .. }) => {
                deliveries
                    .lock()
                    .unwrap()
                    .entry(session_id)
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
            }
            Err(err) => error_capture.lock().unwrap().push(format!(
                "failed to parse pty_output payload: {err}; raw={raw}"
            )),
        }
    });
}

pub(crate) fn make_fixture() -> Fixture {
    let env_lock = TEST_CONFIG_ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let (repo_root, config_dir) = prepare_fixture_dirs(&temp);

    let env_guard = TestConfigEnvGuard::set(&config_dir);
    assert_eq!(
        agentscommander_lib::config::config_dir().expect("config dir override"),
        config_dir
    );
    seed_and_verify_settings(&repo_root, &config_dir);

    let captured_deliveries: Arc<Mutex<HashMap<String, usize>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let listener_errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let owners = agentscommander_lib::AppOwners::new();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(
        SessionManager::with_activity_coalescer(owners.activity.clone()),
    ));
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(Arc::clone(&output_senders)),
    ));
    let (settings, git_watcher, pty_mgr) =
        build_settings_git_watcher_and_pty(&owners, &session_mgr, output_senders, &repo_root);

    let deliveries = Arc::clone(&captured_deliveries);
    let error_capture = Arc::clone(&listener_errors);
    let states = TestAppStates {
        settings,
        session_mgr: Arc::clone(&session_mgr),
        tg_mgr,
        git_watcher,
        pty_mgr: Arc::clone(&pty_mgr),
    };
    let app = build_test_app(&owners, &repo_root, states, "pty-powershell-native");

    listen_pty_output(&app, deliveries, error_capture);

    Fixture {
        spawn: owners.spawn.clone(),
        app,
        session_mgr,
        pty_mgr,
        captured_deliveries,
        listener_errors,
        tracked_sessions: Arc::new(Mutex::new(Vec::new())),
        _temp: temp,
        _env_guard: env_guard,
        _env_lock: env_lock,
    }
}

pub(crate) fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// A bare logical program (e.g. `claude` or the test binary's own file stem
/// used by `ac_1271_reporter_mode`) must resolve through the spawned session's
/// PATH. portable-pty rebuilds the child PATH
/// from the registry, so the fixture directory is delivered through the
/// pre-existing git-guard env path: the session cwd is an agent dir, which
/// makes `spawn_sync` call `build_git_guard_env` and set the child PATH to
/// `guard_dir;process_PATH`. Agent env rows cannot carry PATH (reserved key),
/// so the tests prepend the fixture directory to the process PATH for the
/// duration of the create call, under a global lock so parallel tests cannot
/// clobber each other. The spawned child keeps the inherited PATH after the
/// guard restores the original value. See the module docs for the git
/// dependency boundary.
pub(crate) static PATH_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) struct PathEnvGuard {
    previous: String,
}

impl PathEnvGuard {
    pub(crate) fn prepend(dir: &Path) -> Self {
        let previous = std::env::var("PATH").unwrap_or_default();
        // SAFETY: callers hold PATH_ENV_LOCK
        unsafe {
            std::env::set_var("PATH", format!("{};{}", path_to_string(dir), previous));
        }
        Self { previous }
    }
}

impl Drop for PathEnvGuard {
    fn drop(&mut self) {
        // SAFETY: callers hold PATH_ENV_LOCK
        unsafe {
            std::env::set_var("PATH", &self.previous);
        }
    }
}

pub(crate) fn agent_config(
    id: &str,
    command: &str,
) -> agentscommander_lib::config::settings::AgentConfig {
    agentscommander_lib::config::settings::AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#10b981".to_string(),
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

pub(crate) fn base_settings(host_program: &str, host_args: &[&str], project: &Path) -> AppSettings {
    let mut settings = AppSettings::default();
    settings.default_shell = host_program.to_string();
    settings.default_shell_args = host_args
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    settings.project_paths = vec![path_to_string(project)];
    settings
}

/// Create a session through the same native session/backend path a real agent
/// session uses, with a resolved agent spawn plus the configured host-shell
/// snapshot. Returns the session id.
pub(crate) async fn create_resolved_session(
    fixture: &Fixture,
    settings: &AppSettings,
    agent_id: &str,
    cwd: &str,
    name: &str,
) -> SessionInfo {
    let spawn =
        build_agent_spawn_command(settings, agent_id, Some(std::path::Path::new(cwd)), None)
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
pub(crate) async fn create_plain_session(
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
        args.iter().map(std::string::ToString::to_string).collect(),
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

/// Native-PowerShell wrapper of the shared snapshot wait, bounded by `OUTPUT_TIMEOUT`.
pub(crate) async fn wait_for_snapshot_contains(
    fixture: &Fixture,
    session_id: &Uuid,
    expected_bytes: &str,
    phase: &str,
) -> Result<(), String> {
    let marker = (session_id, expected_bytes, phase);
    let bounds = (OUTPUT_TIMEOUT, POLL_INTERVAL);
    shared_wait(&fixture.app, &fixture.captured_deliveries, marker, bounds).await
}

/// Poll the spawn record until the awaited child exit is observable. Returns
/// the child's exit code (the host shell propagated it through `WaitForExit` or
/// cmd /C).
pub(crate) async fn wait_for_exit_code(
    fixture: &Fixture,
    session_id: Uuid,
    timeout: Duration,
) -> Result<u32, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(record) = spawn_diagnostics::record_for(session_id, fixture.spawn.as_ref()) {
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

pub(crate) async fn wait_for_session_gone(
    fixture: &Fixture,
    session_id: Uuid,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if fixture
            .pty_mgr
            .lock()
            .unwrap()
            .context_session_liveness(session_id)
            == agentscommander_lib::pty::context_scrape::ContextSessionLiveness::SessionOver
        {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!("session {session_id} still live after teardown"))
}

pub(crate) fn process_exists(pid: u32) -> bool {
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                concat!(
                    "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit ",
                    "0 }} else {{ exit 1 }}"
                ),
                pid = pid
            ),
        ])
        .status();
    matches!(status, Ok(status) if status.success())
}

pub(crate) fn write_cmd_shim(path: &Path, body: &str) {
    std::fs::write(path, format!("@echo off\r\n{body}\r\n")).expect("write cmd shim");
}

/// `claude.cmd` shim: emits a stable marker and exits with a deterministic code.
pub(crate) fn write_claude_shim(dir: &Path, marker: &str, exit_code: u32) -> PathBuf {
    let path = dir.join("claude.cmd");
    write_cmd_shim(&path, &format!("echo {marker}\r\nexit /b {exit_code}"));
    path
}

/// The managed native child is the integration test binary itself (architect
/// option 1, F8): the repository-owned helper is the `ac_1271_reporter_mode`
/// test, selected in the child by the fixed harness prefix `--exact
/// ac_1271_reporter_mode` plus the `AC_1271_ARGV_REPORTER=1` env flag. This
/// keeps the package single-bin, so tauri's `mainBinaryName` rename can no
/// longer capture an auto-discovered helper.
pub(crate) fn test_binary_stem() -> String {
    std::env::current_exe()
        .expect("test binary path")
        .file_stem()
        .expect("test binary file stem")
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn test_binary_dir() -> PathBuf {
    std::env::current_exe()
        .expect("test binary path")
        .parent()
        .expect("test binary parent dir")
        .to_path_buf()
}

/// Fixed harness prefix for the self-exec child: libtest accepts `--exact
/// <name>` plus extra positional filters, so the logical argv follows at a
/// deterministic index and the child runs only the single mode test.
pub(crate) const REPORTER_MODE_PREFIX: [&str; 2] = ["--exact", "ac_1271_reporter_mode"];

/// Expected reporter exit code: sum of UTF-8 byte lengths of the logical args,
/// modulo 256 (the mode test's deterministic derivation).
pub(crate) fn reporter_exit_code(args: &[&str]) -> u32 {
    (args.iter().map(|arg| arg.len()).sum::<usize>() % 256) as u32
}

/// Scoped guard that sets `AC_1271_ARGV_REPORTER=1` for the duration of the
/// create call so the spawned child (this same test binary) enters reporter
/// mode. Restores the previous value on drop.
pub(crate) struct ReporterEnvGuard {
    previous: Option<String>,
}

impl ReporterEnvGuard {
    pub(crate) fn set() -> Self {
        let previous = std::env::var("AC_1271_ARGV_REPORTER").ok();
        // SAFETY: callers hold PATH_ENV_LOCK
        unsafe {
            std::env::set_var("AC_1271_ARGV_REPORTER", "1");
        }
        Self { previous }
    }
}

impl Drop for ReporterEnvGuard {
    fn drop(&mut self) {
        // SAFETY: callers hold PATH_ENV_LOCK
        unsafe {
            match self.previous.as_ref() {
                Some(previous) => std::env::set_var("AC_1271_ARGV_REPORTER", previous),
                None => std::env::remove_var("AC_1271_ARGV_REPORTER"),
            }
        }
    }
}
