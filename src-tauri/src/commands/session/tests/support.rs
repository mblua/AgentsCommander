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

pub(super) type SessionSources = BTreeMap<String, String>;
type SourcePredicate = fn(&SessionSources) -> bool;
const RESTART_REFUSAL: &str = "crate::config::archive_gate::probe_spawn_refusal(app,&cwd).await?;";
const SESSION_SOURCE: &str = "session.rs";
const CORE_SOURCE: &str = "session/create/core.rs";
const CONTEXT_SOURCE: &str = "session/create/core/context.rs";
const BACKEND_SOURCE: &str = "session/create/core/backend.rs";
const PUBLICATION_SOURCE: &str = "session/create/core/publication.rs";
const RESTART_SOURCE: &str = "session/restart/execution.rs";
const CREATE_ENTRY: &str = "create_session_inner_core";
const RESTART_ENTRY: &str = "execute_restart_transaction_in_context";
const SPAWN: &str =
    "letspawn_result=PtyManager::spawn(pty_mgr,session.backend_kind,spawn_spec).await;";
const FENCE: &str = "t.session_may_exist=true;";
const UNFENCE: &str = "if!launch_witness.launched(){";
const MARK: &str = "letspawn_mark=";
const DROP_MARK: &str = "drop(spawn_mark);";
const PENDING: &str =
    "letpending_result=mgr.create_pending_session_with_binding_and_carrier_permit(";
const ROTATION: &str = "rotate_origin_memory_at_spawn";
const ROTATION_GATE: &str = "ifstart_fresh&&context_result.is_ok(){";
const FRESH: &str = "letstart_fresh=skip_auto_resume;";
const ARCHIVE_GATE: &str = concat!(
    "enforce_owned_unarchived_for_spawn(app,&cwd,&session_label,&create_owner,",
    "&create_manager,lifecycle_domain,inline_transaction,)"
);

pub(super) fn session_sources() -> SessionSources {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/commands");
    let mut sources = BTreeMap::new();
    for relative in [
        SESSION_SOURCE,
        "session/create.rs",
        "session/create/contracts.rs",
        "session/create/publication.rs",
        "session/create/admission.rs",
        "session/create/context_prep.rs",
        "session/create/effects.rs",
        "session/create/launch.rs",
        CORE_SOURCE,
        CONTEXT_SOURCE,
        BACKEND_SOURCE,
        PUBLICATION_SOURCE,
        "session/restart.rs",
        "session/restart/context.rs",
        "session/restart/settlement.rs",
        RESTART_SOURCE,
    ] {
        let path = root.join(relative);
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                sources.insert(relative.to_string(), production_source(&source));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("read {}: {error}", path.display()),
        }
    }
    sources
}

fn normalized(source: &str) -> String {
    source.split_whitespace().collect()
}

fn syntax_mask(source: &str) -> String {
    let tokens = regex::Regex::new(r#"(?s)//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*""#).unwrap();
    tokens
        .replace_all(source, |m: &regex::Captures<'_>| " ".repeat(m[0].len()))
        .into_owned()
}

fn block_end(source: &str, start: usize) -> Option<usize> {
    let mask = syntax_mask(source);
    let opening = mask[start..].find('{')? + start;
    let mut depth = 0;
    for (offset, byte) in mask.as_bytes()[opening..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(opening + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn production_source(source: &str) -> String {
    let mut source = source.to_string();
    let attribute = regex::Regex::new(r"(?m)^[ \t]*#\[cfg\(test\)\]").unwrap();
    while let Some(found) = attribute.find(&syntax_mask(&source)) {
        let (start, after) = (found.start(), found.end());
        let mask = syntax_mask(&source);
        let rest = &mask[after..];
        let semicolon = rest.find(';');
        let brace = rest.find('{');
        let end = match (semicolon, brace) {
            (Some(semi), Some(open)) if semi < open => Some(after + semi + 1),
            (Some(semi), None) => Some(after + semi + 1),
            (_, Some(_)) => block_end(&source, after),
            _ => None,
        }
        .expect("complete cfg(test) item or statement");
        source.replace_range(start..end, "");
    }
    source
}

fn function(source: &str, name: &str) -> Option<String> {
    let start = source.find(&format!("fn {name}"))?;
    let end = block_end(source, start)?;
    Some(normalized(&source[start..end]))
}

fn definition_count(sources: &SessionSources, name: &str) -> usize {
    let entry = regex::Regex::new(&format!("fn {name}[<(]")).unwrap();
    sources.values().map(|s| entry.find_iter(s).count()).sum()
}

fn occurrences(sources: &SessionSources, needle: &str) -> usize {
    sources
        .values()
        .map(|s| normalized(s).matches(needle).count())
        .sum()
}

fn ordered(source: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .try_fold(0, |after, needle| {
            source[after..]
                .find(needle)
                .map(|at| after + at + needle.len())
        })
        .is_some()
}

fn create_stages(sources: &SessionSources) -> Option<[String; 4]> {
    let split = [
        CORE_SOURCE,
        CONTEXT_SOURCE,
        BACKEND_SOURCE,
        PUBLICATION_SOURCE,
    ];
    if split.iter().any(|path| sources.contains_key(*path)) {
        Some([
            function(sources.get(CORE_SOURCE)?, CREATE_ENTRY)?,
            function(sources.get(CONTEXT_SOURCE)?, "prepare")?,
            function(sources.get(BACKEND_SOURCE)?, "spawn")?,
            function(sources.get(PUBLICATION_SOURCE)?, "complete")?,
        ])
    } else {
        let whole = function(sources.get(SESSION_SOURCE)?, CREATE_ENTRY)?;
        Some([whole.clone(), whole.clone(), whole.clone(), whole])
    }
}

pub(super) fn source_map_complete(sources: &SessionSources) -> bool {
    let restart = if sources.contains_key(RESTART_SOURCE) {
        RESTART_SOURCE
    } else {
        SESSION_SOURCE
    };
    create_stages(sources).is_some()
        && definition_count(sources, CREATE_ENTRY) == 1
        && definition_count(sources, RESTART_ENTRY) == 1
        && sources
            .get(restart)
            .is_some_and(|s| function(s, RESTART_ENTRY).is_some())
}

pub(super) fn source_chain(sources: &SessionSources) -> bool {
    let Some([core, context, backend, _]) = create_stages(sources) else {
        return false;
    };
    if sources.contains_key(CORE_SOURCE) {
        [
            (&core, "context::prepare("),
            (&context, "backend::spawn("),
            (&backend, "publication::complete("),
        ]
        .iter()
        .all(|(body, call)| body.matches(call).count() == 1 && body.contains(".await"))
            && ["prepare", "spawn", "complete"]
                .iter()
                .all(|name| definition_count(sources, name) == 1)
    } else {
        source_map_complete(sources)
    }
}

fn first_effect(sources: &SessionSources) -> &str {
    if sources.contains_key(CORE_SOURCE) {
        "context::prepare("
    } else {
        SPAWN
    }
}

pub(super) fn source_pending(sources: &SessionSources) -> bool {
    let Some([core, _, _, _]) = create_stages(sources) else {
        return false;
    };
    occurrences(sources, PENDING) == 1 && ordered(&core, &[PENDING, first_effect(sources)])
}

pub(super) fn source_fence(sources: &SessionSources) -> bool {
    let Some([_, _, backend, _]) = create_stages(sources) else {
        return false;
    };
    [FENCE, SPAWN, UNFENCE]
        .iter()
        .all(|needle| occurrences(sources, needle) == 1)
        && occurrences(sources, "has_backend_session") == 0
        && ordered(&backend, &[FENCE, SPAWN, UNFENCE])
}

pub(super) fn source_mark(sources: &SessionSources) -> bool {
    let Some([core, _, backend, _]) = create_stages(sources) else {
        return false;
    };
    occurrences(sources, MARK) == 1
        && occurrences(sources, DROP_MARK) == 1
        && ordered(&core, &[MARK, PENDING, first_effect(sources)])
        && ordered(&backend, &[SPAWN, DROP_MARK, "ifletErr(e)=spawn_result{"])
        && source_archive(sources)
}

pub(super) fn source_effect(sources: &SessionSources) -> bool {
    let Some([_, _, backend, _]) = create_stages(sources) else {
        return false;
    };
    ordered(
        &backend,
        &[
            SPAWN,
            "ifletSome(effect)=backend_effect{effect.complete(",
            DROP_MARK,
            "ifletErr(e)=spawn_result{",
        ],
    )
}

pub(super) fn source_rotation(sources: &SessionSources) -> bool {
    let Some([_, context, _, _]) = create_stages(sources) else {
        return false;
    };
    let call = format!("{ROTATION_GATE}crate::config::agent_memory::{ROTATION}(&cwd);}}");
    context.matches(&call).count() == 1
}

pub(super) fn source_fresh(sources: &SessionSources) -> bool {
    occurrences(sources, "letstart_fresh=") == 1 && occurrences(sources, FRESH) == 1
}

pub(super) fn source_archive(sources: &SessionSources) -> bool {
    let Some([core, _, _, _]) = create_stages(sources) else {
        return false;
    };
    occurrences(sources, ARCHIVE_GATE) == 2 && ordered(&core, &[ARCHIVE_GATE, MARK, PENDING])
}

pub(super) fn source_budgets(sources: &SessionSources) -> bool {
    occurrences(sources, "note_coordinator_create_on_app(app,&cwd)") == 1
        && occurrences(sources, ROTATION) == 2
        && occurrences(sources, "`rotate_origin_memory_at_spawn`returns`()`") == 1
}

pub(super) fn source_restart(sources: &SessionSources) -> bool {
    let path = if sources.contains_key(RESTART_SOURCE) {
        RESTART_SOURCE
    } else {
        SESSION_SOURCE
    };
    let Some(body) = sources.get(path).and_then(|s| function(s, RESTART_ENTRY)) else {
        return false;
    };
    ordered(&body, &[
        RESTART_REFUSAL,
        "crate::config::sessions_persistence::enforce_creation_gate(app,&cwd,enforcement).await?;",
        "letretirement=", "retire_lifecycle_entry_protection(app,authority).await",
        "teardown_old_for_restart(transaction,uuid,old_was_dormant).await",
    ])
}

fn all_source_predicates(sources: &SessionSources) -> bool {
    source_map_complete(sources)
        && source_chain(sources)
        && source_pending(sources)
        && source_fence(sources)
        && source_mark(sources)
        && source_effect(sources)
        && source_rotation(sources)
        && source_fresh(sources)
        && source_archive(sources)
        && source_budgets(sources)
        && source_restart(sources)
}

fn source_range(source: &str, needle: &str, index: usize) -> Option<std::ops::Range<usize>> {
    let compact = normalized(source);
    let at = compact.match_indices(needle).nth(index)?.0;
    let offsets: Vec<usize> = source
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace())
        .flat_map(|(i, c)| i..i + c.len_utf8())
        .collect();
    Some(offsets[at]..offsets[at + needle.len() - 1] + 1)
}

fn replace_source(sources: &mut SessionSources, from: &str, to: &str) {
    let hits: usize = sources
        .values()
        .map(|s| normalized(s).matches(from).count())
        .sum();
    assert_eq!(hits, 1, "unique mutation anchor: {from}");
    for source in sources.values_mut() {
        if let Some(range) = source_range(source, from, 0) {
            source.replace_range(range, to);
        }
    }
}

fn move_before(sources: &mut SessionSources, statement: &str, anchor: &str) {
    replace_source(sources, statement, "");
    replace_source(sources, anchor, &format!("{statement}{anchor}"));
}

fn split_source_map(original: &SessionSources) -> SessionSources {
    if original.contains_key(CORE_SOURCE) {
        return original.clone();
    }
    let mut map = original.clone();
    let session = map.get_mut(SESSION_SOURCE).unwrap();
    let start = session.find("async fn create_session_inner_core").unwrap();
    let end = block_end(session, start).unwrap();
    let whole = session[start..end].to_string();
    session.replace_range(start..end, "");
    let context = whole
        .find("        // #930 - resolve the host-credential")
        .unwrap();
    let backend = whole
        .find("        // Capture the effective arg vector BEFORE spawn")
        .unwrap();
    let publication = whole
        .find(concat!(
            "        if let Some(continuation) = owned_replacement.as_ref() {\n",
            "            let actual = mgr.get_pending_session"
        ))
        .unwrap();
    let finish = whole
        .find("\n    };\n    // This accepted core owns every launched effect")
        .unwrap();
    let core = format!(
        "{}context::prepare(ctx, input).await{}",
        &whole[..context],
        &whole[finish..]
    );
    map.insert(CORE_SOURCE.into(), core);
    for (path, signature, body, next) in [
        (
            CONTEXT_SOURCE,
            "prepare",
            &whole[context..backend],
            "backend::spawn(ctx, input).await",
        ),
        (
            BACKEND_SOURCE,
            "spawn",
            &whole[backend..publication],
            "publication::complete(ctx, input).await",
        ),
        (
            PUBLICATION_SOURCE,
            "complete",
            &whole[publication..finish],
            "",
        ),
    ] {
        map.insert(
            path.into(),
            format!("async fn {signature}() {{\n{body}\n{next}\n}}"),
        );
    }
    map
}

fn rejected(mode: &str, name: &str, passes: bool) {
    assert!(!passes, "{mode}: {name}");
    println!("{mode}: {name}: rejected");
}

#[test]
fn source_guards_round4_controls() {
    let real = session_sources();
    let split = split_source_map(&real);
    for (mode, original) in [("original", real), ("split", split)] {
        assert!(all_source_predicates(&original), "positive {mode}");
        let probes: &[(&str, &str, &str, SourcePredicate)] = &[
            ("remove fence", FENCE, "", source_fence),
            ("remove mark", MARK, "letother_mark=", source_mark),
            (
                "rotation conjunction",
                ROTATION_GATE,
                "ifstart_fresh{",
                source_rotation,
            ),
            (
                "second fresh binding",
                FRESH,
                "letstart_fresh=skip_auto_resume;letstart_fresh=true;",
                source_fresh,
            ),
            ("fresh RHS", FRESH, "letstart_fresh=true;", source_fresh),
            (
                "remove note",
                "note_coordinator_create_on_app(app,&cwd)",
                "None",
                source_budgets,
            ),
            (
                "extra rotation reference",
                FRESH,
                "letstart_fresh=skip_auto_resume;/*rotate_origin_memory_at_spawn*/",
                source_budgets,
            ),
            (
                "remove restart refusal",
                RESTART_REFUSAL,
                "",
                source_restart,
            ),
        ];
        for (name, from, to, predicate) in probes {
            let mut probe = original.clone();
            replace_source(&mut probe, from, to);
            rejected(mode, name, predicate(&probe));
        }
        for index in 0..2 {
            let mut probe = original.clone();
            let source = probe
                .values_mut()
                .find(|s| normalized(s).contains(ARCHIVE_GATE))
                .unwrap();
            let range = source_range(source, ARCHIVE_GATE, index).unwrap();
            source.replace_range(range, "");
            rejected(
                mode,
                &format!("remove activation gate {index}"),
                source_archive(&probe),
            );
        }
        for (name, statement, anchor, predicate) in [
            (
                "late fence",
                FENCE,
                UNFENCE,
                source_fence as SourcePredicate,
            ),
            ("early mark drop", DROP_MARK, SPAWN, source_mark),
            (
                "late restart refusal",
                RESTART_REFUSAL,
                "teardown_old_for_restart(transaction,uuid,old_was_dormant).await",
                source_restart,
            ),
        ] {
            let mut probe = original.clone();
            move_before(&mut probe, statement, anchor);
            rejected(mode, name, predicate(&probe));
        }
        let mut probe = original.clone();
        move_before(&mut probe, DROP_MARK, "ifletSome(effect)=backend_effect{");
        rejected(mode, "late effect completion", source_effect(&probe));
        let mut probe = original.clone();
        let path = if probe.contains_key(CORE_SOURCE) {
            CORE_SOURCE
        } else {
            SESSION_SOURCE
        };
        probe
            .get_mut(path)
            .unwrap()
            .push_str("\nasync fn create_session_inner_core() {}\n");
        rejected(mode, "duplicate entry", source_map_complete(&probe));
        if original.contains_key(CORE_SOURCE) {
            for path in [
                CORE_SOURCE,
                CONTEXT_SOURCE,
                BACKEND_SOURCE,
                PUBLICATION_SOURCE,
            ] {
                let mut probe = original.clone();
                probe.remove(path);
                rejected(
                    mode,
                    &format!("missing {path}"),
                    source_map_complete(&probe),
                );
            }
            for call in [
                "context::prepare(",
                "backend::spawn(",
                "publication::complete(",
            ] {
                for replacement in ["", "other::redirect("] {
                    let mut probe = original.clone();
                    replace_source(&mut probe, call, replacement);
                    assert!(!source_chain(&probe), "{mode}: remove/redirect {call}");
                    println!("{mode}: {call} -> {replacement}: rejected");
                }
            }
            let mut probe = original.clone();
            let core = probe.get_mut(CORE_SOURCE).unwrap();
            let pending = source_range(core, PENDING, 0).unwrap().start;
            let tail = ".map_err(|error|error.to_string());";
            let range = source_range(&core[pending..], tail, 0).unwrap();
            let end = pending + range.end;
            let statement = core[pending..end].to_string();
            core.replace_range(pending..end, "");
            let after = source_range(core, "context::prepare(ctx,input).await", 0)
                .unwrap()
                .end;
            core.insert_str(after, &statement);
            rejected(mode, "late pending insertion", source_pending(&probe));
        }
    }
}
