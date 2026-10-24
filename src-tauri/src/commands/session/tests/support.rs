use super::*;

pub(crate) fn test_settings() -> AppSettings {
    AppSettings {
        agents: vec![
            AgentConfig {
                preflight: None,
                preflight_timeout_seconds: None,
                id: "claude".to_string(),
                id_hash: None,
                id_hash_debug: None,
                label: "Claude Code".to_string(),
                command: "claude".to_string(),
                color: "#d97706".to_string(),
                order: None,
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                blocking_menus: None,
                backend: Default::default(),
            },
            AgentConfig {
                preflight: None,
                preflight_timeout_seconds: None,
                id: "codex".to_string(),
                id_hash: None,
                id_hash_debug: None,
                label: "Codex".to_string(),
                command: "codex".to_string(),
                color: "#10b981".to_string(),
                order: None,
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                blocking_menus: None,
                backend: Default::default(),
            },
        ],
        ..AppSettings::default()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SessionTestStoreState {
    Ready,
    Error,
    Missing,
}

pub(crate) fn session_test_app_with_store(
    settings: AppSettings,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
    store_state: SessionTestStoreState,
) -> tauri::App<tauri::test::MockRuntime> {
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        shutdown.token().clone(),
    );
    let output_senders = Arc::new(Mutex::new(HashMap::new()));
    // #2232 phase 4 test 9: the app owns the **single** capture registry and
    // an outbound network, exactly as `run()` does, so a supervisor-spawned
    // reader reaches the registry slot and a bot attach can be counted.
    let captures = Arc::new(crate::capture::registry::CaptureRegistry::new());
    let telegram: crate::telegram::manager::TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(
            crate::telegram::manager::TelegramBridgeManager::with_captures(
                output_senders,
                Arc::clone(&captures),
            ),
        ));
    let store_dir = tempfile::TempDir::new().expect("create target-gate store");
    let store_root = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(store_dir.path()).expect("canonicalize target-gate store"),
    );
    let message_store = Arc::new(
        crate::api::message_store::MessageStore::open(
            store_root.join(crate::api::message_store::DB_FILENAME),
        )
        .expect("open target-gate store"),
    );
    let target_gate_state =
        crate::api::message_store::PtyInputTargetGateState::for_root(store_root);
    let mut builder = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(Arc::new(
            crate::resource_monitor::ResourceMonitorState::new(),
        ))
        .manage(session_mgr)
        .manage(pty_mgr)
        .manage(crate::DetachedSessionsState::default())
        .manage(telegram)
        .manage(Arc::clone(&captures))
        .manage(crate::network::OutboundNetwork::new_for_tests(1))
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(crate::pty::menu_guard::MenuGuard::new()))
        .manage(coordinator.clone())
        .manage(target_gate_state.clone())
        .manage(shutdown);
    builder = match store_state {
        SessionTestStoreState::Ready => builder.manage(
            crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                Ok(message_store),
                target_gate_state.gate.clone(),
            ),
        ),
        SessionTestStoreState::Error => builder.manage(
            crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                Err("store_unavailable".to_string()),
                target_gate_state.gate.clone(),
            ),
        ),
        SessionTestStoreState::Missing => builder,
    };
    let app = builder
        .manage(store_dir)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build session test app");
    coordinator
        .start(app.handle().clone())
        .expect("start coordinator");
    let bootstrap = coordinator.clone();
    std::thread::spawn(move || {
        tauri::async_runtime::block_on(async move {
            bootstrap
                .submit_restore_first()
                .await
                .expect("open coordinator")
                .finish();
        });
    })
    .join()
    .expect("join coordinator bootstrap");
    app
}

pub(super) fn session_test_app(
    settings: AppSettings,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
) -> tauri::App<tauri::test::MockRuntime> {
    session_test_app_with_store(settings, session_mgr, pty_mgr, SessionTestStoreState::Ready)
}

pub(super) fn strict_target_fixture() -> (tempfile::TempDir, String, String) {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let team = ac_root.join("_team_team");
    let coordinator_matrix = ac_root.join("_agent_lead");
    let first_matrix = ac_root.join("_agent_dev-one");
    let second_matrix = ac_root.join("_agent_dev-two");
    let workgroup = ac_root.join("wg-1-team");
    let coordinator = workgroup.join("__agent_lead");
    let first = workgroup.join("__agent_dev-one");
    let second = workgroup.join("__agent_dev-two");
    for directory in [
        &team,
        &coordinator_matrix,
        &first_matrix,
        &second_matrix,
        &coordinator,
        &first,
        &second,
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        team.join("config.json"),
        concat!(
            r#"{"agents":["../_agent_dev-one","../_agent_dev-two","../_agent_lead"],"#,
            r#""coordinator":"../_agent_lead"}"#
        ),
    )
    .unwrap();
    for (replica, identity) in [
        (&coordinator, "../../_agent_lead"),
        (&first, "../../_agent_dev-one"),
        (&second, "../../_agent_dev-two"),
    ] {
        std::fs::write(
            replica.join("config.json"),
            format!(r#"{{"identity":"{identity}"}}"#),
        )
        .unwrap();
    }
    (
        temp,
        first.to_string_lossy().into_owned(),
        second.to_string_lossy().into_owned(),
    )
}

/// #1175. The bytes every rotation fixture seeds into `memory/MEMORY.md`.
/// Asserted verbatim on both sides of every differential, so a green test can
/// never mean "an archive directory exists but is empty".
const ROTATION_SENTINEL: &str = "remembered bytes";

/// #1175. One replica and the origin Agent Matrix its `config.json` identity
/// points at.
pub(super) struct RotationSide {
    pub(super) replica_cwd: String,
    pub(super) matrix: std::path::PathBuf,
}

/// #1175. `strict_target_fixture` (`:5896`) already builds two symmetric
/// replicas under `<temp>/project/.ac/wg-1-team/` whose `config.json` identity
/// names `<temp>/project/.ac/_agent_dev-one` and `_agent_dev-two`. Those two
/// directories sit directly under a `.ac` workspace, so they satisfy
/// `is_canonical_agent_matrix_dir`, which is what `resolve_rotatable_matrix_root`
/// (`config/agent_memory.rs:83`) resolves a replica session to.
///
/// Seed a non-empty `memory/` in each: #1172 D2 makes an EMPTY `memory/` a
/// no-op, so the sentinel is what gives the fresh half of each differential
/// something to rotate.
pub(super) fn rotation_fixture() -> (tempfile::TempDir, RotationSide, RotationSide) {
    let (temp, first_replica, second_replica) = strict_target_fixture();
    let ac_root = temp.path().join("project").join(".ac");
    let mut sides = Vec::new();
    for (replica_cwd, agent) in [
        (first_replica, "_agent_dev-one"),
        (second_replica, "_agent_dev-two"),
    ] {
        let matrix = ac_root.join(agent);
        std::fs::create_dir_all(matrix.join("memory")).expect("create origin memory/");
        std::fs::write(matrix.join("memory").join("MEMORY.md"), ROTATION_SENTINEL)
            .expect("seed origin memory/");
        sides.push(RotationSide {
            replica_cwd,
            matrix,
        });
    }
    let second = sides.pop().expect("second side");
    let first = sides.pop().expect("first side");
    (temp, first, second)
}

/// #1175. Every nested memory rotation, sorted. Same shape as
/// `agent_memory.rs:180-189`.
fn rotated_memory_dirs(matrix: &std::path::Path) -> Vec<String> {
    let entries = match std::fs::read_dir(matrix.join("memory-archive")) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => panic!("read origin memory archive: {err}"),
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.expect("dir entry").file_name())
        .filter_map(|name| name.to_str().map(str::to_string))
        .filter(|name| name.starts_with("memory_"))
        .collect();
    names.sort();
    names
}

/// #1175. "Nothing has happened to this side." Used for the resume side AND
/// for a side whose own launch has not run yet, which is what stops the second
/// launch from laundering a side effect of the first (D3, hole 2).
pub(super) fn assert_memory_pristine(side: &RotationSide, case: &str) {
    assert!(
        std::fs::read_dir(&side.matrix)
            .expect("read matrix")
            .all(|entry| !entry
                .expect("matrix entry")
                .file_name()
                .to_string_lossy()
                .starts_with("memory_")),
        "{case}: no root archive may be created"
    );
    assert_eq!(
        std::fs::read_to_string(side.matrix.join("memory").join("MEMORY.md")).ok(),
        Some(ROTATION_SENTINEL.to_string()),
        "#1175 ({case}): the origin matrix's live memory/ must be exactly as it was"
    );
    let rotated = rotated_memory_dirs(&side.matrix);
    assert!(
        rotated.is_empty(),
        "#1175 ({case}): nothing may have rotated, and this created {rotated:?} in {}",
        side.matrix.display()
    );
    let replica = std::path::Path::new(&side.replica_cwd);
    assert!(
        rotated_memory_dirs(replica).is_empty()
            && !replica.join("memory").exists()
            && !replica.join("memory-archive").exists(),
        "#1175 ({case}): the replica itself must gain no memory* entry"
    );
}

/// #1175. The RESUME assertion: pristine, PLUS a witness that the launch
/// actually entered the context block.
///
/// The witness is not decoration. `:2041` is an `if let`, and a launch that
/// skips it still reaches the spawn, so `spawn_count` cannot see the
/// difference; without this, "nothing rotated" is satisfiable by a resume that
/// never reached the gate at all.
/// `materialize_agent_context_file_with_filename_activated`
/// (`config/session_context.rs:2215`) writes `cwd.join(target_filename)`, and
/// the fixture seeds only `config.json` into a replica, so this file existing
/// means the block ran. Measured in probe 14 and in 2.7.
pub(super) fn assert_resume_left_memory_alone(side: &RotationSide, case: &str) {
    assert!(
        std::path::Path::new(&side.replica_cwd)
            .join("AGENTS.md")
            .is_file(),
        "#1175 ({case}): the resume must have ENTERED the context block; without \
             this witness the no-rotation assertion below can pass vacuously"
    );
    assert_memory_pristine(side, case);
}

/// #1175. The FRESH assertion, which is also the POSITIVE CONTROL: it fails if
/// the launch never reached the rotation chokepoint, which is what makes a green
/// `assert_resume_left_memory_alone` on the resume side attributable to the gate
/// rather than to an unreached call site.
pub(super) fn assert_memory_rotated_once(side: &RotationSide, case: &str) {
    assert!(
        std::fs::read_dir(&side.matrix)
            .expect("read matrix")
            .all(|entry| !entry
                .expect("matrix entry")
                .file_name()
                .to_string_lossy()
                .starts_with("memory_")),
        "{case}: no root archive may be created"
    );
    let rotated = rotated_memory_dirs(&side.matrix);
    assert_eq!(
        rotated.len(),
        1,
        "#1175 ({case}): a fresh launch must rotate exactly once, found {rotated:?}"
    );
    assert_eq!(
        std::fs::read_to_string(
            side.matrix
                .join("memory-archive")
                .join(&rotated[0])
                .join("MEMORY.md")
        )
        .ok(),
        Some(ROTATION_SENTINEL.to_string()),
        "#1175 ({case}): the archive must carry the previous session's bytes"
    );
    let live = side.matrix.join("memory");
    assert!(
        live.is_dir(),
        "#1175 ({case}): the fresh session must get a live memory/ back"
    );
    assert_eq!(
        std::fs::read_dir(&live).expect("read live memory/").count(),
        0,
        "#1175 ({case}): the recreated memory/ starts empty"
    );
    let replica = std::path::Path::new(&side.replica_cwd);
    assert!(
        rotated_memory_dirs(replica).is_empty()
            && !replica.join("memory").exists()
            && !replica.join("memory-archive").exists(),
        "#1175 ({case}): the replica itself must gain no memory* entry"
    );
}

pub(super) async fn create_target_for_test(
    app: &tauri::App<tauri::test::MockRuntime>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
    cwd: &str,
    intent: CreateSelectionIntent,
) -> Result<SessionInfo, String> {
    super::super::create_session_inner(
        app.handle(),
        session_mgr,
        pty_mgr,
        "codex".to_string(),
        Vec::new(),
        cwd.to_string(),
        Some("target gate fixture".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        intent,
    )
    .await
}

pub(crate) async fn create_scripted_session(
    app: &tauri::App<tauri::test::MockRuntime>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
    cwd: &str,
) -> SessionInfo {
    super::super::create_session_inner(
        app.handle(),
        session_mgr,
        pty_mgr,
        "test-shell".to_string(),
        Vec::new(),
        cwd.to_string(),
        Some("restart fixture".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("create scripted session")
}

pub(super) fn capture_session_lifecycle(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> std::sync::mpsc::Receiver<(String, String)> {
    use tauri::Listener;

    let (sender, receiver) = std::sync::mpsc::channel();
    for event_name in [
        "session_created",
        "session_destroyed",
        "session_switched",
        "session_communication_changed",
    ] {
        let sender = sender.clone();
        app.listen_any(event_name, move |event| {
            let _ = sender.send((event_name.to_string(), event.payload().to_string()));
        });
    }
    receiver
}

pub(crate) async fn close_test_coordinator(app: &tauri::App<tauri::test::MockRuntime>) {
    use tauri::Manager;

    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

pub(super) async fn create_manual_cascade_fixture(
    app: &tauri::App<tauri::test::MockRuntime>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
    root: &std::path::Path,
) -> [Uuid; 4] {
    let mut ids = Vec::new();
    for name in ["external", "member-a", "member-b", "coordinator"] {
        let cwd = root.join(name);
        std::fs::create_dir_all(&cwd).unwrap();
        let info = create_scripted_session(app, session_mgr, pty_mgr, &cwd.to_string_lossy()).await;
        ids.push(Uuid::parse_str(&info.id).unwrap());
    }
    ids.try_into().expect("four cascade fixture ids")
}
