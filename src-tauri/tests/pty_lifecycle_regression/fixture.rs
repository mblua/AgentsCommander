use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use agentscommander_lib::commands::session::{create_session_inner, CreateSelectionIntent};
use agentscommander_lib::config::sessions_persistence::{
    load_sessions, persist_current_state_result,
};
use agentscommander_lib::config::settings::{save_settings, AppSettings, SettingsState};
use agentscommander_lib::pty::manager::PtyManager;
use agentscommander_lib::session::manager::SessionManager;
use agentscommander_lib::session::session::{SessionInfo, SessionStatus};
use agentscommander_lib::telegram::manager::{
    OutputSenderMap, TelegramBridgeManager, TelegramBridgeState,
};
use tauri::Listener;
use uuid::Uuid;

use super::process::{
    path_to_string, process_exists, run_powershell_preflight, script_args, stop_process,
    write_child_script, POLL_INTERVAL,
};
pub(crate) use super::pty_fixture::parse_session_id;
use super::pty_fixture::{
    build_test_app, git_watcher_and_pty, wait_for_snapshot_contains as shared_wait, TestAppStates,
    TestConfigEnvGuard, TestPtyOutputDelivery, TEST_CONFIG_ENV_LOCK,
};

const OUTPUT_TIMEOUT: Duration = Duration::from_secs(10);

type PtyCleanupEntries = Arc<Mutex<Vec<(Arc<Mutex<PtyManager>>, Uuid)>>>;

fn lifecycle_test_config_dir() -> PathBuf {
    std::env::temp_dir()
        .join(format!(
            "agentscommander-pty-lifecycle-{}",
            std::process::id()
        ))
        .join("config")
}

#[derive(Clone)]
pub(crate) struct LifecycleCleanup {
    entries: PtyCleanupEntries,
    pids: Arc<Mutex<Vec<u32>>>,
}

impl LifecycleCleanup {
    fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            pids: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn record_session(&self, pty_mgr: &Arc<Mutex<PtyManager>>, id: Uuid) {
        self.entries.lock().unwrap().push((Arc::clone(pty_mgr), id));
    }

    pub(crate) fn record_pid(&self, pid: u32) {
        let mut pids = self.pids.lock().unwrap();
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
}

impl Drop for LifecycleCleanup {
    fn drop(&mut self) {
        for (pty_mgr, id) in self.entries.lock().unwrap().iter() {
            let _ = pty_mgr.lock().unwrap().kill(*id);
        }
        for pid in self.pids.lock().unwrap().iter().copied() {
            if process_exists(pid) {
                let _ = stop_process(pid);
            }
        }
    }
}

pub(crate) struct LifecycleFixture {
    pub(crate) cleanup: LifecycleCleanup,
    pub(crate) repo_root: PathBuf,
    config_dir: PathBuf,
    session_cwd: PathBuf,
    script_path: PathBuf,
    pid_files: Vec<PathBuf>,
    pub(crate) captured_output: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pub(crate) captured_sequences: Arc<Mutex<HashMap<String, Vec<u64>>>>,
    /// Per-session count of tagged `pty_output` deliveries (data or resync marker).
    /// Inactive-session regressions require this to stay at zero for each session.
    pub(crate) captured_deliveries: Arc<Mutex<HashMap<String, usize>>>,
    pub(crate) listener_errors: Arc<Mutex<Vec<String>>>,
    pub(crate) app: tauri::App,
    pub(crate) session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(crate) pty_mgr: Arc<Mutex<PtyManager>>,
    _temp: tempfile::TempDir,
    _env_guard: TestConfigEnvGuard,
    _env_lock: std::sync::MutexGuard<'static, ()>,
}

fn seed_project_authority(config_dir: &Path, repo_root: &Path) -> (AppSettings, PathBuf, Vec<u8>) {
    // Seed canonical project authority before the unrelated Preserve save.
    let mut settings = AppSettings::default();
    settings.default_shell = "powershell.exe".to_string();
    settings.default_shell_args = vec!["-NoLogo".to_string()];
    settings.project_paths = vec![path_to_string(repo_root)];
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
    (settings, project_paths_path, project_paths_before)
}

fn expected_canonical_project_path(repo_root: &Path) -> String {
    let canonical_repo_root = std::fs::canonicalize(repo_root)
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", repo_root.display()));
    let canonical_repo_root = canonical_repo_root.to_str().unwrap_or_else(|| {
        panic!(
            "canonical repo root is not UTF-8: {}",
            canonical_repo_root.display()
        )
    });
    if let Some(unc) = canonical_repo_root.strip_prefix(r"\\?\UNC\") {
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
    }
}

fn assert_managed_settings_omit_project_paths(config_dir: &Path) {
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

fn install_pty_output_listener(
    app: &tauri::App,
    captured: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    captured_sequences: Arc<Mutex<HashMap<String, Vec<u64>>>>,
    captured_deliveries: Arc<Mutex<HashMap<String, usize>>>,
    listener_errors: Arc<Mutex<Vec<String>>>,
) {
    app.listen_any("pty_output", move |event| {
        let raw = event.payload().to_string();
        match serde_json::from_str::<TestPtyOutputDelivery>(&raw) {
            Ok(TestPtyOutputDelivery::Data {
                session_id,
                sequence,
                data,
                ..
            }) => {
                captured
                    .lock()
                    .unwrap()
                    .entry(session_id.clone())
                    .or_default()
                    .extend(data);
                captured_deliveries
                    .lock()
                    .unwrap()
                    .entry(session_id.clone())
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
                match sequence.parse::<u64>() {
                    Ok(sequence) => captured_sequences
                        .lock()
                        .unwrap()
                        .entry(session_id)
                        .or_default()
                        .push(sequence),
                    Err(err) => listener_errors.lock().unwrap().push(format!(
                        "pty_output data sequence is not canonical decimal: {err}; raw={raw}"
                    )),
                }
            }
            Ok(TestPtyOutputDelivery::ResyncRequired { session_id, .. }) => {
                captured_deliveries
                    .lock()
                    .unwrap()
                    .entry(session_id)
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
            }
            Err(err) => listener_errors.lock().unwrap().push(format!(
                "failed to parse pty_output payload: {err}; raw={raw}"
            )),
        }
    });
}

pub(crate) fn make_lifecycle_fixture() -> LifecycleFixture {
    let env_lock = TEST_CONFIG_ENV_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let repo_root = temp.path().join("repo-pty-lifecycle");
    let config_dir = lifecycle_test_config_dir();
    let session_cwd = repo_root.clone();
    let script_path = temp.path().join("pty-child.ps1");

    std::fs::create_dir_all(&repo_root).expect("create repo root");
    // A registered project path must resolve to a real AC project (a
    // directory containing a `.ac` Project Root) for the six-field decoder to
    // select it into the runtime projectPaths. Without this `.ac`, the CLI loader
    // (`load_settings_for_cli`) validates repo_root as AcRootOrCollectionMissing,
    // drops it from projectPaths, and session-retention purges the persisted
    // session — failing the Phase D persistence assertion.
    std::fs::create_dir_all(repo_root.join(".ac")).expect("create project .ac root");
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
    let root = config_dir.join(agentscommander_lib::config::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(root).expect("Root fixture creation succeeds");
    let (settings, project_paths_path, project_paths_before) =
        seed_project_authority(&config_dir, &repo_root);
    let saved = save_settings(&settings).expect("seed isolated settings");
    let expected_project_path = expected_canonical_project_path(&repo_root);
    let expected_project_paths = vec![expected_project_path];
    assert_eq!(saved.project_paths, expected_project_paths);
    assert_eq!(saved.default_shell, "powershell.exe");
    assert_eq!(saved.default_shell_args, vec!["-NoLogo".to_string()]);
    assert_eq!(
        std::fs::read(&project_paths_path).expect("read preserved authority"),
        project_paths_before
    );
    assert_managed_settings_omit_project_paths(&config_dir);

    write_child_script(&script_path);
    run_powershell_preflight();

    let captured_output = Arc::new(Mutex::new(HashMap::new()));
    let captured_sequences = Arc::new(Mutex::new(HashMap::new()));
    let captured_deliveries = Arc::new(Mutex::new(HashMap::new()));
    let listener_errors = Arc::new(Mutex::new(Vec::new()));
    let cleanup = LifecycleCleanup::new();
    let (app, session_mgr, pty_mgr) = make_test_app(
        &repo_root,
        &captured_output,
        &captured_sequences,
        &captured_deliveries,
        &listener_errors,
        &cleanup,
        None,
        agentscommander_lib::AppOwners::new(),
    );

    LifecycleFixture {
        cleanup,
        repo_root,
        config_dir,
        session_cwd,
        script_path,
        pid_files: Vec::new(),
        captured_output,
        captured_sequences,
        captured_deliveries,
        listener_errors,
        app,
        session_mgr,
        pty_mgr,
        _temp: temp,
        _env_guard: env_guard,
        _env_lock: env_lock,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "test app wires each capture map, cleanup, override and owners"
)]
pub(crate) fn make_test_app(
    repo_root: &Path,
    captured_output: &Arc<Mutex<HashMap<String, Vec<u8>>>>,
    captured_sequences: &Arc<Mutex<HashMap<String, Vec<u64>>>>,
    captured_deliveries: &Arc<Mutex<HashMap<String, usize>>>,
    listener_errors: &Arc<Mutex<Vec<String>>>,
    _cleanup: &LifecycleCleanup,
    session_mgr_override: Option<Arc<tokio::sync::RwLock<SessionManager>>>,
    owners: agentscommander_lib::AppOwners,
) -> (
    tauri::App,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Arc<Mutex<PtyManager>>,
) {
    let session_mgr = session_mgr_override.unwrap_or_else(|| {
        Arc::new(tokio::sync::RwLock::new(
            SessionManager::with_activity_coalescer(owners.activity.clone()),
        ))
    });
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(Arc::clone(&output_senders)),
    ));
    let settings: SettingsState = agentscommander_lib::config::settings::new_settings_state({
        let mut s = AppSettings::default();
        s.default_shell = "powershell.exe".to_string();
        s.default_shell_args = vec!["-NoLogo".to_string()];
        s.project_paths = vec![path_to_string(repo_root)];
        s
    });
    let (git_watcher, pty_mgr) = git_watcher_and_pty(&owners, &session_mgr, output_senders);

    let captured = Arc::clone(captured_output);
    let captured_sequences = Arc::clone(captured_sequences);
    let captured_deliveries = Arc::clone(captured_deliveries);
    let listener_errors = Arc::clone(listener_errors);
    let states = TestAppStates {
        settings,
        session_mgr: Arc::clone(&session_mgr),
        tg_mgr,
        git_watcher,
        pty_mgr: Arc::clone(&pty_mgr),
    };
    let app = build_test_app(&owners, repo_root, states, "pty-lifecycle");

    install_pty_output_listener(
        &app,
        captured,
        captured_sequences,
        captured_deliveries,
        listener_errors,
    );

    (app, session_mgr, pty_mgr)
}

impl LifecycleFixture {
    pub(crate) fn pid_path(&mut self, label: &str) -> PathBuf {
        let path = self.config_dir.join(format!("{label}.pid"));
        self.pid_files.push(path.clone());
        path
    }

    pub(crate) async fn create_session(
        &self,
        name: &str,
        pid_file: &Path,
    ) -> Result<SessionInfo, String> {
        create_session_inner(
            self.app.handle(),
            &self.session_mgr,
            &self.pty_mgr,
            "powershell.exe".to_string(),
            script_args(&self.script_path, pid_file),
            path_to_string(&self.session_cwd),
            Some(name.to_string()),
            None,
            None,
            false,
            Vec::new(),
            true,
            None,
            None,
            None, // #973 - no view in this test: 120x30
            CreateSelectionIntent::User,
        )
        .await
    }

    pub(crate) fn dump(&self, phase: &str, session_ids: &[Uuid], pid_files: &[PathBuf]) -> String {
        let sessions_json = std::fs::read_to_string(self.config_dir.join("sessions.json"))
            .unwrap_or_else(|e| format!("<unreadable: {e}>"));
        let captured = self
            .captured_output
            .lock()
            .unwrap()
            .iter()
            .map(|(id, bytes)| format!("{id}: {}", String::from_utf8_lossy(bytes)))
            .collect::<Vec<_>>()
            .join("\n");
        let captured_sequences = self
            .captured_sequences
            .lock()
            .unwrap()
            .iter()
            .map(|(id, sequences)| format!("{id}: {sequences:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        let listener_errors = self.listener_errors.lock().unwrap().join("\n");
        let pid_info = pid_files
            .iter()
            .map(|path| {
                let content =
                    std::fs::read_to_string(path).unwrap_or_else(|e| format!("<missing: {e}>"));
                let parsed = content.trim().parse::<u32>().ok();
                let exists = parsed.map(process_exists);
                format!(
                    "{} => {:?}, exists={exists:?}",
                    path.display(),
                    content.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let pty_info = session_ids
            .iter()
            .map(|id| {
                let pty = self.pty_mgr.lock().unwrap();
                format!(
                    "{id}: has_session={}, size={:?}",
                    pty.has_session(*id),
                    pty.get_pty_size(*id)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "phase={phase}\n\
             config_dir={}\n\
             repo_root={}\n\
             session_cwd={}\n\
             sessions_json={sessions_json}\n\
             pids=\n\
             {pid_info}\n\
             pty=\n\
             {pty_info}\n\
             listener_errors=\n\
             {listener_errors}\n\
             captured_sequences=\n\
             {captured_sequences}\n\
             captured=\n\
             {captured}",
            self.config_dir.display(),
            self.repo_root.display(),
            self.session_cwd.display()
        )
    }
}

/// Lifecycle wrapper of the shared snapshot wait, bounded by `OUTPUT_TIMEOUT`.
pub(crate) async fn wait_for_snapshot_contains(
    fixture: &LifecycleFixture,
    app: &tauri::App,
    session_id: &Uuid,
    expected_bytes: &str,
    phase: &str,
) -> Result<(), String> {
    let marker = (session_id, expected_bytes, phase);
    let bounds = (OUTPUT_TIMEOUT, POLL_INTERVAL);
    shared_wait(app, &fixture.captured_deliveries, marker, bounds).await
}

/// The last UI event sequence for a session. Inactive-session regressions deliver zero
/// `pty_output` events, so the empty baseline (0) is the correct anchor and the snapshot
/// sequence comparison remains meaningful; the strict-increasing check still guards any
/// future path that does record sequences.
pub(crate) fn last_strictly_increasing_sequence(
    fixture: &LifecycleFixture,
    session_id: &str,
    phase: &str,
) -> u64 {
    let sequences = fixture
        .captured_sequences
        .lock()
        .unwrap()
        .get(session_id)
        .cloned()
        .unwrap_or_default();
    for pair in sequences.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{phase}: sequences must be strictly increasing: {sequences:?}"
        );
    }
    sequences.last().copied().unwrap_or(0)
}

pub(crate) async fn persist_sessions(session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>) {
    let mgr = session_mgr.read().await;
    persist_current_state_result(&mgr)
        .await
        .expect("persist current state");
}

pub(crate) fn assert_persisted_contains(fixture: &LifecycleFixture, name: &str, phase: &str) {
    let sessions_json = std::fs::read_to_string(fixture.config_dir.join("sessions.json"))
        .expect("sessions.json exists");
    assert!(
        sessions_json.contains(name)
            && sessions_json.contains("powershell.exe")
            && sessions_json.contains("shellArgs")
            && sessions_json.contains("workingDirectory")
            && sessions_json.contains("status"),
        "{phase} persisted content missing expected fields\n{}",
        fixture.dump(phase, &[], &fixture.pid_files)
    );
    assert!(
        load_sessions().iter().any(|row| row.name == name),
        "{phase} load_sessions missing {name}\n{}",
        fixture.dump(phase, &[], &fixture.pid_files)
    );
}

pub(crate) fn assert_persisted_missing(fixture: &LifecycleFixture, name: &str, phase: &str) {
    assert!(
        !load_sessions().iter().any(|row| row.name == name),
        "{phase} load_sessions still contains {name}\n{}",
        fixture.dump(phase, &[], &fixture.pid_files)
    );
}

pub(crate) fn assert_has_pty(
    pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    phase: &str,
    fixture: &LifecycleFixture,
) {
    assert!(
        pty_mgr.lock().unwrap().has_session(id),
        "{phase} expected PTY session {id}\n{}",
        fixture.dump(phase, &[id], &fixture.pid_files)
    );
}

pub(crate) fn assert_no_pty(
    pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    phase: &str,
    fixture: &LifecycleFixture,
) {
    assert!(
        !pty_mgr.lock().unwrap().has_session(id),
        "{phase} expected no PTY session {id}\n{}",
        fixture.dump(phase, &[id], &fixture.pid_files)
    );
}

pub(crate) async fn assert_session_missing(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    id: Uuid,
) {
    let mgr = session_mgr.read().await;
    assert!(mgr.get_session(id).await.is_none());
}

pub(crate) async fn assert_active_session(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    id: Uuid,
) {
    let mgr = session_mgr.read().await;
    let session = mgr.get_session(id).await.expect("active session exists");
    assert_eq!(session.status, SessionStatus::Active);
}
