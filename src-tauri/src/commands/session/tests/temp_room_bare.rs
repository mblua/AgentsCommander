//! A temp-room session gets its limited context and credentials.
use super::*;
use crate::config::sessions_persistence::{enforce_creation_gate, CreationGateEnforcement};
use crate::config::temp_rooms::temp_room_dir_name;
use crate::pty::credentials::CREDENTIAL_ENV_KEYS;

/// A `config_dir()/temp-room-trNN` folder, removed on drop.
struct TempRoomDir(PathBuf);

impl TempRoomDir {
    fn create(n: u32) -> Self {
        let dir = Self::missing(n);
        std::fs::create_dir_all(&dir.0).expect("create temp room");
        dir
    }

    fn missing(n: u32) -> Self {
        let base = crate::config::config_dir().expect("config dir");
        let path = base.join(temp_room_dir_name(n));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }

    fn cwd(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempRoomDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TempRoomRun {
    app: tauri::App<tauri::test::MockRuntime>,
    result: Result<crate::session::session::SessionInfo, String>,
    backend: Arc<CapturingSpawnBackend>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
}

/// `gate_create_with` (default_shell.rs) with the cwd as a parameter; the
/// coordinator stays open so the caller can restart before closing it.
async fn launch_in(cwd: &str, container: bool) -> TempRoomRun {
    let mut settings = test_settings();
    if container {
        settings.agents[0].backend = crate::config::settings::AgentBackendConfig {
            kind: SessionBackendKind::ContainerTransport,
            image: Some("ghcr.io/example/claude:test".to_string()),
        };
    }
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend {
        preflight_runs: true,
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
    let spawn = super::super::build_configured_agent_spawn_for_cwd(&settings, "claude", cwd, None)
        .expect("resolve claude")
        .expect("claude is configured");
    let result = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd.to_string(),
        Some("4779 temp room".to_string()),
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
    TempRoomRun {
        app,
        result,
        backend,
        session_mgr,
        pty_mgr,
    }
}

fn credential_keys_in(spec: &crate::pty::backend::BackendSpawnSpec) -> Vec<String> {
    spec.extra_env
        .iter()
        .filter(|(key, _)| CREDENTIAL_ENV_KEYS.contains(&key.as_str()))
        .map(|(key, _)| key.clone())
        .collect()
}

fn entries_in(cwd: &str) -> usize {
    std::fs::read_dir(cwd).expect("read temp room").count()
}

/// All five credentials, with `AGENTSCOMMANDER_ROOT` = cwd.
fn assert_room_credentials(spec: &crate::pty::backend::BackendSpawnSpec, cwd: &str) {
    let mut keys = credential_keys_in(spec);
    keys.sort();
    let mut expected: Vec<String> = CREDENTIAL_ENV_KEYS.iter().map(|k| k.to_string()).collect();
    expected.sort();
    assert_eq!(keys, expected);
    let root = spec
        .extra_env
        .iter()
        .find(|(key, _)| key == "AGENTSCOMMANDER_ROOT")
        .map(|(_, value)| value.as_str());
    assert_eq!(root, Some(cwd));
}

/// The cwd holds exactly one entry, the managed limited context file.
fn assert_only_room_context(cwd: &str) {
    assert_eq!(
        entries_in(cwd),
        1,
        "temp room should hold only the context file"
    );
    let entry = std::fs::read_dir(cwd).unwrap().next().unwrap().unwrap();
    let content = std::fs::read_to_string(entry.path()).expect("read context file");
    assert!(content.contains("Temporary Room Access Limit"), "{content}");
    assert!(content.contains("master-token.txt"), "{content}");
}

#[tokio::test]
async fn temp_room_launch_and_restart_get_room_context() {
    let room = TempRoomDir::create(97);
    let cwd = room.cwd();
    let run = launch_in(&cwd, false).await;
    let info = run.result.as_ref().expect("temp-room launch succeeds");
    {
        let specs = run.backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 1);
        assert_room_credentials(&specs[0], &cwd);
    }
    assert_only_room_context(&cwd);

    let id = Uuid::parse_str(&info.id).unwrap();
    let settings = run.app.state::<crate::config::settings::SettingsState>();
    super::super::restart_session_inner_with_intent(
        run.app.handle(),
        &run.session_mgr,
        &run.pty_mgr,
        settings.inner(),
        id,
        None,
        None,
        Some(true),
        true,
        crate::session::selection::TrustedRestartIntent::User,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await
    .expect("temp-room restart succeeds");
    {
        let specs = run.backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        assert_room_credentials(&specs[1], &cwd);
    }
    assert_only_room_context(&cwd);
    close_test_coordinator(&run.app).await;
}

/// Removes the context-cache file a temp room wrote, on drop.
struct CacheFile(PathBuf);

impl Drop for CacheFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn temp_room_context_keeps_manual_clear_omits_auto_section() {
    let room = TempRoomDir::create(82);
    let _cache = CacheFile(
        crate::config::config_dir()
            .expect("config dir")
            .join(crate::config::instance_artifacts::CONTEXT_CACHE_DIR_NAME)
            .join("ac-context-temp-room-tr82.md"),
    );
    let path = crate::config::session_context::materialize_agent_context_file_with_filename(
        &room.cwd(),
        "CLAUDE.md",
        &[],
        false,
        true,
        None,
    );
    let Ok(Some(path)) = path else {
        panic!("temp room should get a context file: {path:?}");
    };
    let content = std::fs::read_to_string(&path).expect("read context file");
    assert!(content.contains("## Manual context clear"), "{content}");
    assert!(!content.contains("## Self-Maintenance (auto"), "{content}");
}

#[tokio::test]
async fn temp_room_control_plain_cwd_gets_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().into_owned();
    let run = launch_in(&cwd, false).await;
    run.result.as_ref().expect("plain launch succeeds");
    {
        let specs = run.backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 1);
        let mut keys = credential_keys_in(&specs[0]);
        keys.sort();
        let mut expected: Vec<String> = CREDENTIAL_ENV_KEYS.iter().map(|k| k.to_string()).collect();
        expected.sort();
        assert_eq!(keys, expected);
    }
    close_test_coordinator(&run.app).await;
}

#[tokio::test]
async fn temp_room_refuses_container_transport() {
    let room = TempRoomDir::create(98);
    let run = launch_in(&room.cwd(), true).await;
    let error = run
        .result
        .as_ref()
        .expect_err("container launch is refused");
    assert!(
        error.contains("temp rooms cannot use container transport"),
        "{error}"
    );
    assert!(run.backend.specs.lock().unwrap().is_empty());
    close_test_coordinator(&run.app).await;
}

#[tokio::test]
async fn temp_room_creation_gate_requires_existing_folder() {
    let existing = TempRoomDir::create(99);
    let missing = TempRoomDir::missing(96);
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(AppSettings::default(), session_mgr, pty_mgr);
    let settings = app.state::<crate::config::settings::SettingsState>();
    enforce_creation_gate(
        settings.inner(),
        &existing.cwd(),
        CreationGateEnforcement::Enforce,
    )
    .await
    .expect("an existing temp room passes the gate");
    let error = enforce_creation_gate(
        settings.inner(),
        &missing.cwd(),
        CreationGateEnforcement::Enforce,
    )
    .await
    .expect_err("a missing temp room is refused");
    assert!(error.contains("does not exist on disk"), "{error}");
    close_test_coordinator(&app).await;
}

/// `build_configured_agent_spawn_for_cwd` for a temp room with `command`.
fn spawn_in_temp_room(cwd: &str, command: &str) -> Result<Vec<String>, String> {
    let mut settings = test_settings();
    settings.agents[0].command = command.to_string();
    super::super::build_configured_agent_spawn_for_cwd(&settings, "claude", cwd, None)
        .map(|spawn| spawn.expect("claude is configured").shell_args)
}

#[test]
fn temp_room_expands_replica_root_in_command() {
    let room = TempRoomDir::create(94);
    let args = spawn_in_temp_room(&room.cwd(), "claude --add-dir %AC_REPLICA_ROOT%")
        .expect("replica root expands in a temp room");
    let canonical = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(&room.0).expect("canonicalize temp room"),
    );
    let canonical = canonical.to_string_lossy().into_owned();
    assert!(
        args.iter().any(|arg| arg.contains(&canonical)),
        "{args:?} should contain {canonical}"
    );
}

#[test]
fn temp_room_matrix_placeholder_fails_with_clear_error() {
    let room = TempRoomDir::create(93);
    let error = spawn_in_temp_room(&room.cwd(), "claude --add-dir %AC_MATRIX_ROOT%")
        .expect_err("matrix root is refused in a temp room");
    assert!(
        error.contains("does not apply to Temporary Rooms"),
        "{error}"
    );
}
