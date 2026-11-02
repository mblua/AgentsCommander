use super::co_managed_tests::{
    add_session_for_tests, configure_room, room_fixture, settings_with_key, RoomFixture,
};
use super::{
    raise_room_reader_demand_in, room_raise_seam, AppSettings, CodingAgentKind,
    CreateSelectionIntent, SessionBackendKind, SessionInfo, SessionManager, SettingsState,
    TrustedRestartIntent,
};
use crate::capture::key::Cut;
use crate::capture::registry::CaptureRegistry;
use crate::network::OutboundNetwork;
use crate::pty::backend::PtyBackend;
use crate::telegram::manager::{
    OutputSenderMap, ReaderConsumer, TelegramBridgeManager, TelegramBridgeState,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

pub(crate) struct Harness {
    pub(crate) app: tauri::App<tauri::test::MockRuntime>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(crate) captures: Arc<CaptureRegistry>,
    projects_dir: PathBuf,
}

pub(crate) fn harness(fixture: &RoomFixture) -> Harness {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let captures = Arc::new(CaptureRegistry::new());
    let senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let bridge_state: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::with_captures(senders, Arc::clone(&captures)),
    ));
    let projects_dir = fixture.temp_path().join("claude-projects");
    std::fs::create_dir_all(&projects_dir).expect("projects dir");
    // The production `co_managed_set_enabled` command canonicalises the
    // room root and proves it belongs to a registered project, so the
    // phase-2 fixture root is registered exactly as a project row would.
    let mut settings = settings_with_key();
    settings.project_paths = vec![fixture
        .temp_path()
        .join("project-a")
        .to_string_lossy()
        .into_owned()];
    let app = tauri::test::mock_builder()
        .manage(crate::app_state::AppOwners::new())
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(Arc::clone(&manager))
        .manage(bridge_state)
        .manage(OutboundNetwork::new_for_tests(1))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build phase 4 supervisor test app");
    Harness {
        app,
        manager,
        captures,
        projects_dir,
    }
}

impl Harness {
    pub(crate) async fn session_in(&self, cwd: &std::path::Path) -> Uuid {
        let id = add_session_for_tests(
            &self.manager,
            cwd,
            SessionBackendKind::LocalProcess,
            CodingAgentKind::Claude,
        )
        .await;
        self.manager
            .read()
            .await
            .set_resolved_claude_projects_dir(id, Some(self.projects_dir.clone()))
            .await;
        id
    }

    async fn codex_session_in(&self, cwd: &std::path::Path, home: &std::path::Path) -> Uuid {
        let id = add_session_for_tests(
            &self.manager,
            cwd,
            SessionBackendKind::LocalProcess,
            CodingAgentKind::Codex,
        )
        .await;
        self.manager
            .read()
            .await
            .set_profile_metadata(
                id,
                None,
                None,
                Vec::new(),
                false,
                None,
                None,
                Some(home.to_string_lossy().into_owned()),
                None,
            )
            .await;
        id
    }

    async fn bridge(&self) -> tauri::State<'_, TelegramBridgeState> {
        self.app.handle().state::<TelegramBridgeState>()
    }

    /// #2525 set the two global Co-managed inputs in the live settings.
    pub(crate) async fn set_global_co_managed(&self, enabled: bool, key: &str) {
        let settings = self.app.handle().state::<SettingsState>();
        let mut guard = settings.write().await;
        guard.co_managed_enabled = enabled;
        guard.jev_api_key = key.to_string();
        drop(guard);
    }
}

const PHASE4_BOT_ID: &str = "phase4-bot";

/// The heavy harness test 9 needs: the scripted-session app, extended with
/// the single capture registry, the outbound network and the phase-2 room
/// fixture paths the replacement's Room raise resolves against.
///
/// Both configured agents carry the environment row production reads for
/// the transcript memo (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), so the old
/// session and the replacement resolve their reader paths from settings
/// exactly as a real restart does.
struct RestartHarness {
    app: tauri::App<tauri::test::MockRuntime>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pty: Arc<Mutex<crate::pty::manager::PtyManager>>,
    backend: Arc<crate::commands::session::tests::ScriptedSpawnBackend>,
    captures: Arc<CaptureRegistry>,
    network: OutboundNetwork,
    settings: AppSettings,
    claude_config_dir: PathBuf,
    codex_home: PathBuf,
}

fn claude_env_row(dir: &std::path::Path) -> crate::config::settings::CodingAgentEnv {
    crate::config::settings::CodingAgentEnv {
        key: "CLAUDE_CONFIG_DIR".to_string(),
        value: dir.to_string_lossy().into_owned(),
        source: Default::default(),
        enabled: true,
    }
}

fn codex_env_row(dir: &std::path::Path) -> crate::config::settings::CodingAgentEnv {
    crate::config::settings::CodingAgentEnv {
        key: "CODEX_HOME".to_string(),
        value: dir.to_string_lossy().into_owned(),
        source: Default::default(),
        enabled: true,
    }
}

fn restart_harness(fixture: &RoomFixture) -> RestartHarness {
    let claude_config_dir = fixture.temp_path().join("claude-home");
    let codex_home = fixture.temp_path().join("codex-home");
    std::fs::create_dir_all(&claude_config_dir).expect("claude config dir");
    std::fs::create_dir_all(&codex_home).expect("codex home");

    let mut settings = super::tests::test_settings();
    settings.jev_api_key = "test-key".to_string();
    settings.co_managed_enabled = true;
    settings.agents[0].envs = vec![claude_env_row(&claude_config_dir)];
    settings.agents[1].envs = vec![codex_env_row(&codex_home)];
    settings.telegram_bots = vec![crate::telegram::types::TelegramBotConfig {
        id: PHASE4_BOT_ID.to_string(),
        label: "Phase 4 bot".to_string(),
        token: "test-token".to_string(),
        chat_id: 7,
        color: "#229ED9".to_string(),
    }];
    // The creation gate must accept the orchestrator replica cwd: the
    // phase-2 fixture lives under `<temp>/project-a`, so registering that
    // root is what a real AC project row contributes.
    settings.project_paths = vec![fixture
        .temp_path()
        .join("project-a")
        .to_string_lossy()
        .into_owned()];

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(crate::commands::session::tests::ScriptedSpawnBackend::default());
    let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = crate::commands::session::tests::session_test_app_with_store(
        settings.clone(),
        Arc::clone(&manager),
        Arc::clone(&pty),
        crate::commands::session::tests::SessionTestStoreState::Ready,
    );
    let captures = app.state::<Arc<CaptureRegistry>>().inner().clone();
    let network = app.state::<OutboundNetwork>().inner().clone();
    RestartHarness {
        app,
        manager,
        pty,
        backend,
        captures,
        network,
        settings,
        claude_config_dir,
        codex_home,
    }
}

impl RestartHarness {
    /// A live scripted session for `kind` whose transcript memo comes from
    /// the configured agent environment, exactly as production resolves it.
    async fn live_session(&self, cwd: &std::path::Path, kind: CodingAgentKind) -> Uuid {
        let info = self.live_session_info(cwd, kind).await;
        Uuid::parse_str(&info.id).expect("session id")
    }

    /// [`Self::live_session`], keeping the created `SessionInfo`: its
    /// `effective_shell_args` carry the transcript id the spawn minted
    /// (#2454), which is NOT the session UUID.
    async fn live_session_info(&self, cwd: &std::path::Path, kind: CodingAgentKind) -> SessionInfo {
        let (agent_id, label) = match kind {
            CodingAgentKind::Claude => ("claude", "Claude Code"),
            CodingAgentKind::Codex => ("codex", "Codex"),
            _ => panic!("phase 4 restart fixture supports Claude and Codex only"),
        };
        let cwd_str = cwd.to_string_lossy().into_owned();
        let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
            &self.settings,
            agent_id,
            &cwd_str,
            None,
        )
        .expect("build configured agent spawn")
        .expect("configured agent exists in test settings");
        crate::commands::session::create_session_inner(
            self.app.handle(),
            &self.manager,
            &self.pty,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            cwd_str,
            Some("phase 4 restart fixture".to_string()),
            Some(agent_id.to_string()),
            Some(label.to_string()),
            true,
            Vec::new(),
            true,
            Some(spawn),
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect("create phase 4 scripted session")
    }

    async fn bridge(&self) -> tauri::State<'_, TelegramBridgeState> {
        self.app.handle().state::<TelegramBridgeState>()
    }

    /// Drive the production restart entry point (the innermost of the
    /// three, so all three inherit it) and return its error, if any.
    async fn restart(&self, id: Uuid) -> Result<SessionInfo, String> {
        self.restart_with(id, Some(false)).await
    }

    /// The restart button's default: `skip_auto_resume` unset resolves to
    /// FRESH, so the replacement mints a new transcript id (#2454).
    async fn restart_fresh(&self, id: Uuid) -> Result<SessionInfo, String> {
        self.restart_with(id, None).await
    }

    async fn restart_with(
        &self,
        id: Uuid,
        skip_auto_resume: Option<bool>,
    ) -> Result<SessionInfo, String> {
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        crate::commands::session::restart_session_inner_with_intent(
            self.app.handle(),
            &self.manager,
            &self.pty,
            settings.inner(),
            id,
            None,
            None,
            skip_auto_resume,
            true,
            TrustedRestartIntent::User,
            None,
            crate::config::sessions_persistence::default_creation_gate_enforcement(),
        )
        .await
    }

    async fn snapshot(
        &self,
        id: Uuid,
    ) -> (Option<u64>, std::collections::BTreeSet<ReaderConsumer>) {
        let tg = self.bridge().await;
        let tg = tg.lock().await;
        (tg.reader_id(id), tg.reader_demands(id))
    }

    async fn release_all(&self, id: Uuid) {
        let shutdown = {
            let tg = self.bridge().await;
            let mut tg = tg.lock().await;
            tg.reader_release_all(id)
        };
        if let Some(shutdown) = shutdown {
            shutdown.abort_now();
        }
    }

    /// Point the configured Claude agent at `dir`: the next restart builds
    /// its replacement spawn from the live settings, so the replacement's
    /// transcript memo moves with it (a "changed path" restart).
    async fn set_claude_config_dir(&self, dir: &std::path::Path) {
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        settings.write().await.agents[0].envs = vec![claude_env_row(dir)];
    }

    /// Configure the Claude agent's command line, in the harness copy that
    /// creates sessions AND in the live settings a restart re-resolves
    /// argv from.
    async fn set_claude_command(&mut self, command: &str) {
        self.settings.agents[0].command = command.to_string();
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        settings.write().await.agents[0].command = command.to_string();
    }

    async fn set_codex_home(&self, dir: &std::path::Path) {
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        settings.write().await.agents[1].envs = vec![codex_env_row(dir)];
    }

    /// The Claude transcript directory the configured memo resolves to.
    #[expect(
        clippy::unused_self,
        reason = "Método de fixture existente: conservar interfaz de construcción de rutas y \
            sus callers; sin refactor de fixtures."
    )]
    fn claude_projects_dir(&self, config_dir: &std::path::Path, cwd: &std::path::Path) -> PathBuf {
        config_dir
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(
                &cwd.to_string_lossy(),
            ))
    }

    fn message_permits(&self) -> usize {
        self.network
            .acquired_labels_for_tests()
            .iter()
            .filter(|label| **label == "telegram.send_message")
            .count()
    }

    async fn attach_bot(&self, id: Uuid) -> crate::telegram::types::BridgeInfo {
        crate::commands::telegram::attach_telegram_bot_by_id(self.app.handle(), id, PHASE4_BOT_ID)
            .await
            .expect("attach the configured bot")
    }

    async fn close(self) {
        crate::commands::session::tests::close_test_coordinator(&self.app).await;
    }
}

/// The transcript id a spawn minted (#2454), read back from its effective
/// argv; `None` when no `--session-id` was injected.
fn minted_transcript_id(info: &SessionInfo) -> Option<String> {
    crate::commands::session::transcript_id_from_args(
        info.effective_shell_args.as_deref().unwrap_or(&[]),
    )
}

/// Move a file's mtime `secs` into the past, so it is never the newest.
fn backdate(path: &std::path::Path, secs: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open for backdate")
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(secs))
        .expect("backdate mtime");
}

/// One Claude assistant line with an explicit timestamp.
fn claude_line(body: &str, ts: chrono::DateTime<chrono::Utc>) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "message": {"content": [{"type": "text", "text": body}]}
    })
    .to_string()
}

fn append_claude(path: &std::path::Path, body: &str) {
    use std::io::Write as _;
    let line = claude_line(body, chrono::Utc::now() - chrono::Duration::seconds(1));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append transcript");
    writeln!(f, "{line}").expect("write transcript line");
    f.sync_all().expect("sync transcript");
}

/// A >64 KiB Claude transcript whose only assistant record is at the very
/// start, far above the 64 KiB tail window (test 9's backfill probe).
/// Returns the byte length written.
fn large_claude_transcript(path: &std::path::Path, body: &str) -> u64 {
    use std::io::Write as _;
    let first = claude_line(body, chrono::Utc::now() - chrono::Duration::seconds(1));
    let mut f = std::fs::File::create(path).expect("create transcript");
    writeln!(f, "{first}").expect("first line");
    let filler = r#"{"type":"user","message":{"content":[{"type":"text","text":"filler"}]}}"#;
    let mut written = first.len() as u64 + 1;
    while written < 70 * 1024 {
        writeln!(f, "{filler}").expect("filler line");
        written += filler.len() as u64 + 1;
    }
    f.sync_all().expect("sync transcript");
    written
}

fn append_codex(path: &std::path::Path, body: &str) {
    use std::io::Write as _;
    let line = serde_json::json!({
        "timestamp": (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "assistant",
            "phase": "final_answer",
            "content": [{"type": "output_text", "text": body}]
        }
    })
    .to_string();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append rollout");
    writeln!(f, "{line}").expect("write rollout line");
    f.sync_all().expect("sync rollout");
}

/// Poll the slot until it holds a record the predicate accepts.
async fn wait_for_slot_record(
    slot: &crate::capture::sink::CaptureSlot,
    timeout: std::time::Duration,
    mut accept: impl FnMut(&Arc<crate::capture::record::CapturedRecord>) -> bool,
) -> (u64, Arc<crate::capture::record::CapturedRecord>) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let state = slot.snapshot();
        if let crate::capture::sink::SlotValue::Valid(record) = state.value {
            if accept(&record) {
                return (state.seq, record);
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no matching slot record within the budget"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Write one Codex rollout with a `session_meta` line and a single
/// `final_answer` record carrying `body` at `ts`.
fn write_codex_rollout(
    home: &std::path::Path,
    cwd: &str,
    name: &str,
    body: &str,
    ts: chrono::DateTime<chrono::Utc>,
) -> std::path::PathBuf {
    use std::io::Write as _;
    let now = chrono::Utc::now();
    let day = home
        .join("sessions")
        .join(format!("{:04}", now.format("%Y")))
        .join(format!("{:02}", now.format("%m")))
        .join(format!("{:02}", now.format("%d")));
    std::fs::create_dir_all(&day).expect("codex day dir");
    let path = day.join(name);
    let meta = serde_json::json!({
        "timestamp": now.to_rfc3339(),
        "type": "session_meta",
        "payload": {"id": name, "cwd": cwd, "timestamp": now.to_rfc3339()}
    });
    let record = serde_json::json!({
        "timestamp": ts.to_rfc3339(),
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "assistant",
            "phase": "final_answer",
            "content": [{"type": "output_text", "text": body}]
        }
    });
    let mut f = std::fs::File::create(&path).expect("rollout");
    writeln!(f, "{meta}").expect("meta line");
    writeln!(f, "{record}").expect("record line");
    f.sync_all().expect("sync rollout");
    path
}

async fn wait_for_message_permits(h: &RestartHarness, at_least: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while h.message_permits() < at_least {
        assert!(
            std::time::Instant::now() < deadline,
            "no telegram.send_message attempt within the budget"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

// ── #2516: the raise/disable race on the Room demand ─────────────────

/// Spawn the Room raise for `id`, wait until it parks at the armed seam
/// (`reached`), run `interfere`, resume it (`release`) and return what the
/// raise answered.
pub(crate) async fn race_room_raise<Fut>(
    h: &Harness,
    room: &std::path::Path,
    id: Uuid,
    reached: &tokio::sync::Notify,
    release: &tokio::sync::Notify,
    interfere: Fut,
) -> bool
where
    Fut: std::future::Future<Output = ()>,
{
    let app = h.app.handle().clone();
    let room = room.to_path_buf();
    let raise = tokio::spawn(async move { raise_room_reader_demand_in(&app, &room, id).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), reached.notified())
        .await
        .expect("the Room raise reaches the armed seam");
    interfere.await;
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(10), raise)
        .await
        .expect("the Room raise resumes")
        .expect("the Room raise task joins")
}

/// The production disable toggle for the fixture's room.
async fn disable_room(h: &Harness, fixture: &RoomFixture) {
    let config = crate::commands::co_managed::co_managed_set_enabled(
        h.app.handle().clone(),
        fixture.room_path().to_string_lossy().into_owned(),
        false,
    )
    .await
    .expect("disabling the flag succeeds");
    assert!(!config.enabled);
}

/// No Room demand, no reader and no capture slot for `id`.
pub(crate) async fn assert_no_room_reader_state_for(h: &Harness, id: Uuid) {
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(
            tg.reader_demands(id).is_empty(),
            "no Room demand may be installed after the release"
        );
        assert!(!tg.reader_is_running(id), "no reader may be spawned");
    }
    assert!(
        !h.captures.is_open(&id.to_string()),
        "no capture slot may be opened"
    );
}

/// T1's assertions: the aborted raise left no demand, no reader, no slot.
pub(crate) async fn assert_no_room_reader_state(h: &Harness, raised: bool, id: Uuid) {
    assert!(!raised, "a raise overtaken by a release must answer false");
    assert_no_room_reader_state_for(h, id).await;
}

#[cfg(test)]
mod tests;
