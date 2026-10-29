use super::*;
use crate::config::loops::{
    write_loop_config, LoopDef, LoopPolicy, LoopPrompt, LoopTarget, LoopTargetKind, LoopTrigger,
    LoopTriggerKind, LOOP_TIMEZONE_LOCAL,
};
use crate::config::settings::{AgentConfig, ProfileCellConfig};
use crate::pty::backend::PtyBackend;
use crate::session::session::SessionRepo;
use std::collections::{BTreeMap, HashSet};
use tauri::Manager;

#[test]
#[cfg(windows)]
fn path_compare_key_converts_verbatim_unc() {
    assert_eq!(
        path_compare_key(Path::new(r"\\?\UNC\server\share\repo")),
        "//server/share/repo"
    );
}

fn sample_config() -> LoopConfigToml {
    LoopConfigToml {
        loop_def: LoopDef {
            id: "daily-sync".to_string(),
            name: "Daily sync".to_string(),
            enabled: true,
        },
        trigger: LoopTrigger {
            kind: LoopTriggerKind::Cron,
            expr: "0 9 * * *".to_string(),
            timezone: LOOP_TIMEZONE_LOCAL.to_string(),
        },
        target: LoopTarget {
            kind: LoopTargetKind::WorkgroupCoordinator,
            workgroup: "wg-1-dev-team".to_string(),
        },
        prompt: LoopPrompt {
            body: "Send status".to_string(),
        },
        policy: LoopPolicy {
            busy_coordinator: BusyCoordinatorPolicy::WaitUntilIdle,
            ..LoopPolicy::default()
        },
    }
}

fn make_inject_test_app(session_mgr: Arc<tokio::sync::RwLock<SessionManager>>) -> tauri::App {
    let app = crate::test_support::test_builder()
        .manage(Arc::clone(&session_mgr))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build inject test app");
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let git_watcher =
        crate::pty::git_watcher::GitWatcher::new(Arc::clone(&session_mgr), app.handle().clone());
    let pty_manager = Arc::new(Mutex::new(PtyManager::new(
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        idle_detector,
        git_watcher,
        None,
        None,
    )));
    app.manage(pty_manager);
    app
}

fn loop_command_for(
    settings: &crate::config::settings::AppSettings,
    agent_id: &str,
) -> ResolvedLoopAgentCommand {
    let spawn = crate::config::agent_command::resolve_agent_spawn_command(
        settings, agent_id, None, None, false,
    )
    .expect("loop test spawn should resolve without filesystem preparation");
    ResolvedLoopAgentCommand {
        shell: spawn.shell.clone(),
        shell_args: spawn.shell_args.clone(),
        agent_id: Some(spawn.trusted_agent_id.clone()),
        agent_label: Some(spawn.trusted_agent_label.clone()),
        resolved_spawn: Some(spawn),
        resolved_agent_host_shell: None,
    }
}

/// #1873 - cold Muse is fresh, known-state Muse resumes, and every prior
/// provider (plus ad-hoc, argument-bearing and recipe-mismatched Muse) keeps
/// the historical `false` on both branches.
#[test]
fn muse_loop_cold_is_fresh_and_known_state_resumes_without_provider_drift() {
    use crate::config::settings::{AgentConfig, AppSettings};
    let agent = |id: &str, command: &str| AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    };
    let settings = AppSettings {
        agents: vec![
            agent("muse", "muse"),
            agent("muse-abs", "/opt/muse/bin/muse"),
            agent("muse-args", "muse --workspace /srv/work"),
            agent("muse-manual", "muse resume --last"),
            agent("claude", "claude"),
            agent("codex", "codex"),
            agent("agy", "agy"),
            agent("pi", "pi"),
        ],
        ..AppSettings::default()
    };

    let expected_muse_cold = cfg!(any(target_os = "macos", target_os = "linux"));
    for id in ["muse", "muse-abs"] {
        let command = loop_command_for(&settings, id);
        assert_eq!(
            loop_spawn_skip_auto_resume(false, &command),
            expected_muse_cold,
            "cold Muse creation is fresh on supported hosts, id={id}"
        );
        assert!(
            !loop_spawn_skip_auto_resume(true, &command),
            "known-state Muse wake resumes, id={id}"
        );
    }

    // Configured arguments, prior providers: unchanged `false` on both branches.
    for id in ["muse-args", "muse-manual", "claude", "codex", "agy", "pi"] {
        let command = loop_command_for(&settings, id);
        assert!(!loop_spawn_skip_auto_resume(false, &command), "id={id}");
        assert!(!loop_spawn_skip_auto_resume(true, &command), "id={id}");
    }

    // Ad-hoc Muse (no resolved spawn) and a recipe mismatch never qualify.
    let mut adhoc = loop_command_for(&settings, "muse");
    adhoc.resolved_spawn = None;
    assert!(!loop_spawn_skip_auto_resume(false, &adhoc));
    assert!(!loop_spawn_skip_auto_resume(true, &adhoc));
    let mut mismatch = loop_command_for(&settings, "muse");
    mismatch.shell_args = vec!["--workspace".to_string(), "/srv/work".to_string()];
    assert!(!loop_spawn_skip_auto_resume(false, &mismatch));
    assert!(!loop_spawn_skip_auto_resume(true, &mismatch));
}

#[test]
fn loop_candidate_rules_preserve_exited_and_phantom_respawn_paths() {
    assert!(loop_candidate_should_respawn(
        &SessionStatus::Exited(0),
        false
    ));
    assert!(loop_candidate_should_respawn(&SessionStatus::Idle, false));
    assert!(!loop_candidate_should_respawn(&SessionStatus::Idle, true));
    assert!(loop_candidate_is_live(&SessionStatus::Idle, true));
    assert!(!loop_candidate_is_live(&SessionStatus::Exited(0), false));
}

#[test]
fn final_revalidation_blocks_prompt_change_before_inject() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = sample_config();
    let dir = write_loop_config(tmp.path(), &config).expect("write config");
    let session_id = Uuid::new_v4();

    assert!(stale_delivery_report_if_needed(
        &dir,
        &config,
        "project:wg-1-dev-team/tech-lead",
        session_id
    )
    .is_none());

    let mut changed = config.clone();
    changed.prompt.body = "New prompt".to_string();
    write_loop_config(tmp.path(), &changed).expect("write changed config");

    let report = stale_delivery_report_if_needed(
        &dir,
        &config,
        "project:wg-1-dev-team/tech-lead",
        session_id,
    )
    .expect("stale report");

    assert_eq!(report.kind, LoopAuditKind::DeliveryFailed);
    assert_eq!(
        report.target.as_deref(),
        Some("project:wg-1-dev-team/tech-lead")
    );
    assert_eq!(report.session_id, Some(session_id));
    assert!(report.prompt_snapshot.is_none());
    assert!(report.message.contains("skipped stale delivery"));
}

#[tokio::test]
async fn pre_write_revalidation_blocks_stale_loop_after_inject_shell_lookup() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = sample_config();
    let dir = write_loop_config(tmp.path(), &config).expect("write config");

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = make_inject_test_app(session_mgr.clone());
    let session = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            tmp.path().to_string_lossy().to_string(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session")
    };
    let session_id = session.id;
    app.state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap()
        .record_route(
            session_id,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        );

    let mut changed = config.clone();
    changed.prompt.body = "New prompt".to_string();
    write_loop_config(tmp.path(), &changed).expect("write changed config");

    let result = crate::pty::inject::inject_text_into_session_with_pre_write_check(
        &app.handle().clone(),
        session_id,
        &config.prompt.body,
        || {
            stale_delivery_error_if_needed(
                &dir,
                &config,
                "project:wg-1-dev-team/tech-lead",
                session_id,
            )
        },
    )
    .await;

    let err = result.expect_err("stale loop should block before PTY write");
    assert!(err.contains("skipped stale delivery"));
}

#[test]
fn final_revalidation_blocks_policy_change_and_deleted_config_before_inject() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = sample_config();
    let dir = write_loop_config(tmp.path(), &config).expect("write config");
    let session_id = Uuid::new_v4();

    let mut changed = config.clone();
    changed.policy.busy_coordinator = BusyCoordinatorPolicy::Skip;
    write_loop_config(tmp.path(), &changed).expect("write changed config");
    assert!(stale_delivery_report_if_needed(
        &dir,
        &config,
        "project:wg-1-dev-team/tech-lead",
        session_id
    )
    .is_some());

    std::fs::remove_dir_all(&dir).expect("remove loop dir");
    assert!(stale_delivery_report_if_needed(
        &dir,
        &config,
        "project:wg-1-dev-team/tech-lead",
        session_id
    )
    .is_some());
    assert!(!dir.exists());
}

// ── #2176 - Loop wake agent resolution ──

fn loop_test_agent(id: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
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

/// Two agents with codex first, plus one enabled profile cell carrying a
/// distinctive env row. As in `config/agent_command.rs`'s fixtures, codex
/// being first is what makes any surviving `settings.agents.first()`
/// behavior fail the assertions.
fn loop_test_settings_with_cell(
    agent_id: &str,
    letter: &str,
    env_key: &str,
    env_value: &str,
) -> AppSettings {
    let mut settings = AppSettings {
        agents: vec![
            loop_test_agent("codex", "codex"),
            loop_test_agent("claude", "claude"),
        ],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry(agent_id.to_string())
        .or_default()
        .insert(
            letter.to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([(env_key.to_string(), env_value.to_string())]),
                notes: String::new(),
            },
        );
    settings
}

/// A real replica directory so `path_compare_key`'s `canonicalize`
/// succeeds, laid out as `<tmp>/.ac/wg-7-dev-team/__agent_dev-rust`.
fn loop_test_replica_dir(tmp: &tempfile::TempDir) -> PathBuf {
    let replica = tmp
        .path()
        .join(".ac")
        .join("wg-7-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).expect("create replica dir");
    replica
}

fn write_replica_config(replica: &Path, tooling: &str) {
    std::fs::write(
        replica.join("config.json"),
        format!(r#"{{"tooling":{}}}"#, tooling),
    )
    .expect("write replica config.json");
}

fn session_info_for_test(
    id: &str,
    cwd: &str,
    status: SessionStatus,
    agent_id: Option<&str>,
    requested_profile: Option<&str>,
) -> SessionInfo {
    SessionInfo {
        id: id.to_string(),
        name: id.to_string(),
        shell: "claude".to_string(),
        shell_args: Vec::new(),
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: "2026-05-16T00:00:00Z".to_string(),
        working_directory: cwd.to_string(),
        status,
        waiting_for_input: false,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: agent_id.map(str::to_string),
        agent_label: agent_id.map(str::to_string),
        git_repos: Vec::new(),
        workgroup_task: None,
        is_coordinator: true,
        is_root_agent: false,
        token: "t".to_string(),
        agent_kind: None,
        requested_profile: requested_profile.map(str::to_string),
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        profile_outdated: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

/// Issue acceptance criterion 4: no `lastCodingAgent`, no
/// `currentCodingAgent`, still resolves the persisted session's agent and
/// its stored profile letter, and the profile cell is actually applied.
#[test]
fn loop_resolves_persisted_agent_and_profile_when_last_coding_agent_is_absent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let replica = loop_test_replica_dir(&tmp);
    write_replica_config(&replica, "{}");
    let settings = loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");
    let persisted = PersistedCoordinatorAgent {
        agent_id: "claude".to_string(),
        requested_profile: Some("E".to_string()),
    };

    let command = resolve_loop_agent_command_from_settings(&settings, &replica, Some(&persisted))
        .expect("persisted agent should resolve");

    assert_eq!(command.agent_id.as_deref(), Some("claude"));
    let spawn = command.resolved_spawn.expect("resolved spawn");
    assert_eq!(spawn.trusted_agent_id, "claude");
    assert_eq!(spawn.profile_resolution.requested_profile, "E");
    assert!(
        spawn
            .child_env
            .iter()
            .any(|(key, value)| key == "LOOP_TEST_PROFILE_E" && value == "applied"),
        "profile cell env row must be applied, child_env={:?}",
        spawn.child_env
    );
}

/// Rank 1 (`currentCodingAgent`) outranks rank 2 (the persisted agentId).
#[test]
fn loop_prefers_current_coding_agent_over_persisted_agent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let replica = loop_test_replica_dir(&tmp);
    write_replica_config(&replica, r#"{"currentCodingAgent":"claude"}"#);
    let settings = loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");
    let persisted = PersistedCoordinatorAgent {
        agent_id: "codex".to_string(),
        requested_profile: None,
    };

    let command = resolve_loop_agent_command_from_settings(&settings, &replica, Some(&persisted))
        .expect("currentCodingAgent should resolve");

    assert_eq!(command.agent_id.as_deref(), Some("claude"));
}

/// Rank 3 is the explicit legacy tail: no persisted record and no
/// `currentCodingAgent`, but a configured `lastCodingAgent`.
#[test]
fn loop_falls_back_to_last_coding_agent_when_no_session_record() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let replica = loop_test_replica_dir(&tmp);
    write_replica_config(&replica, r#"{"lastCodingAgent":"claude"}"#);
    let settings = loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");

    let command = resolve_loop_agent_command_from_settings(&settings, &replica, None)
        .expect("legacy lastCodingAgent tail should resolve");

    assert_eq!(command.agent_id.as_deref(), Some("claude"));
}

/// The deleted `settings.agents.first()` fallback: with no pin at all the
/// resolver must fail closed instead of waking codex just because it is
/// first in `settings.agents`.
#[test]
fn loop_resolution_fails_closed_without_any_pinned_agent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let replica = loop_test_replica_dir(&tmp);
    write_replica_config(&replica, "{}");
    let settings = loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");

    let err = resolve_loop_agent_command_from_settings(&settings, &replica, None)
        .expect_err("no pin must fail closed");

    assert!(
        err.contains("No pinned coding agent"),
        "unexpected error: {err}"
    );
}

/// Decision 2.1: a persisted record naming a deleted agent is the pin, so
/// resolution errors instead of falling back to `lastCodingAgent`.
#[test]
fn loop_fails_closed_when_persisted_agent_is_not_configured() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let replica = loop_test_replica_dir(&tmp);
    write_replica_config(&replica, r#"{"lastCodingAgent":"codex"}"#);
    let settings = loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");
    let persisted = PersistedCoordinatorAgent {
        agent_id: "ghost".to_string(),
        requested_profile: Some("E".to_string()),
    };

    let err = resolve_loop_agent_command_from_settings(&settings, &replica, Some(&persisted))
        .expect_err("an unconfigured pin must fail closed");

    assert!(err.contains("ghost"), "unexpected error: {err}");
    assert!(err.contains("not configured"), "unexpected error: {err}");
}

/// Most-live status first, then registry insertion order; an agent-less
/// record is skipped and its profile is never borrowed.
#[test]
fn persisted_agent_skips_records_without_an_agent_and_pins_the_tie_break() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let replica = loop_test_replica_dir(&tmp);
    let sibling = tmp.path().join("sibling");
    std::fs::create_dir_all(&sibling).expect("create sibling dir");
    let replica_cwd = replica.to_string_lossy().to_string();
    let sibling_cwd = sibling.to_string_lossy().to_string();

    let rows = vec![
        session_info_for_test(
            "sibling",
            &sibling_cwd,
            SessionStatus::Idle,
            Some("codex"),
            Some("Z"),
        ),
        session_info_for_test(
            "no-agent",
            &replica_cwd,
            SessionStatus::Idle,
            None,
            Some("Z"),
        ),
        session_info_for_test(
            "claude",
            &replica_cwd,
            SessionStatus::Idle,
            Some("claude"),
            Some("E"),
        ),
        session_info_for_test(
            "codex",
            &replica_cwd,
            SessionStatus::Idle,
            Some("codex"),
            Some("B"),
        ),
    ];

    assert_eq!(
        persisted_agent_for_replica(&rows, &replica),
        Some(PersistedCoordinatorAgent {
            agent_id: "claude".to_string(),
            requested_profile: Some("E".to_string()),
        })
    );
}

#[test]
fn persisted_agent_prefers_the_most_live_status() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let replica = loop_test_replica_dir(&tmp);
    let replica_cwd = replica.to_string_lossy().to_string();

    let rows = vec![
        session_info_for_test(
            "codex",
            &replica_cwd,
            SessionStatus::Exited(0),
            Some("codex"),
            Some("B"),
        ),
        session_info_for_test(
            "claude",
            &replica_cwd,
            SessionStatus::Active,
            Some("claude"),
            Some("E"),
        ),
    ];

    assert_eq!(
        persisted_agent_for_replica(&rows, &replica),
        Some(PersistedCoordinatorAgent {
            agent_id: "claude".to_string(),
            requested_profile: Some("E".to_string()),
        })
    );
    assert_eq!(persisted_agent_for_replica(&[], &replica), None);
}

mod restart;

struct RecordingDeliveryHost {
    calls: Mutex<Vec<&'static str>>,
    sessions: Arc<tokio::sync::RwLock<SessionManager>>,
    ptys: Arc<Mutex<PtyManager>>,
    purge: Arc<crate::session::purge_guard::PurgeGuard>,
    change: Option<(PathBuf, LoopConfigToml)>,
    fail: bool,
}

impl RecordingDeliveryHost {
    fn record(&self, call: &'static str) {
        self.calls.lock().unwrap().push(call);
    }
}

impl LoopPromptEvents for RecordingDeliveryHost {
    fn last_prompt(&self, _: Uuid, _: &str) {
        assert!(
            self.sessions.try_write().is_err(),
            "manager guard spans emission"
        );
        self.record("event");
    }
}

impl LoopDeliveryHost for RecordingDeliveryHost {
    fn settings_owner(&self) -> &SettingsState {
        panic!("unexpected settings lookup")
    }
    fn sessions(&self) -> &Arc<tokio::sync::RwLock<SessionManager>> {
        self.record("sessions");
        &self.sessions
    }
    fn ptys(&self) -> &Arc<Mutex<PtyManager>> {
        assert!(
            self.sessions.try_write().is_ok(),
            "session guard dropped before PTY"
        );
        self.record("ptys");
        &self.ptys
    }
    fn purge_guard(&self) -> Option<&Arc<crate::session::purge_guard::PurgeGuard>> {
        self.record("purge");
        Some(&self.purge)
    }
    fn background_destroy(
        &self,
        _: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        panic!("unexpected destroy")
    }
    fn restart_fresh(
        &self,
        _: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<SessionInfo, String>> + Send + '_>> {
        panic!("unexpected restart")
    }
    fn create(
        &self,
        _: LoopSessionCreate,
    ) -> Pin<Box<dyn Future<Output = Result<SessionInfo, String>> + Send + '_>> {
        panic!("unexpected create")
    }
    fn inject<'a>(
        &'a self,
        _: Uuid,
        _: &'a str,
        pre_write: Box<dyn FnOnce() -> Result<(), String> + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        self.record("inject-built");
        Box::pin(async move {
            self.record("inject-polled");
            if let Some((root, config)) = &self.change {
                write_loop_config(root, config).unwrap();
            }
            pre_write()?;
            self.record("write");
            if self.fail {
                Err("recording failure".into())
            } else {
                Ok(())
            }
        })
    }
    fn note_post_boundary(&self, _: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move { self.record("note") })
    }
}

struct DeliveryBackend;
impl PtyBackend for DeliveryBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        _: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        panic!("no native spawn")
    }
    fn write(
        &self,
        _: &crate::pty::manager::BackendWriteAuthority,
        _: Uuid,
        _: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        panic!("no native input")
    }
    fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), crate::errors::AppError> {
        panic!("no resize")
    }
    fn kill(&self, _: Uuid) -> Result<(), crate::errors::AppError> {
        panic!("no kill")
    }
    fn has_session(&self, _: Uuid) -> bool {
        true
    }
    fn get_screen_snapshot(&self, _: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }
    fn register_response_watcher(&self, _: Uuid, _: String, _: PathBuf) {}
    fn terminate_job_for_session(&self, _: Uuid) -> bool {
        false
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

fn recording_delivery_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, LoopConfigToml) {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let root = project.join(".ac");
    let replica = root.join("wg-1-dev-team/__agent_lead");
    for dir in [
        root.join("_team_dev-team"),
        root.join("_agent_lead"),
        replica.clone(),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        root.join("_team_dev-team/config.json"),
        r#"{"agents":["../_agent_lead"],"coordinator":"../_agent_lead"}"#,
    )
    .unwrap();
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_lead"}"#,
    )
    .unwrap();
    let mut config = sample_config();
    config.policy.session_start = LoopSessionStart::Accumulate;
    write_loop_config(&root, &config).unwrap();
    (tmp, project, replica, config)
}

fn recording_delivery_host() -> RecordingDeliveryHost {
    RecordingDeliveryHost {
        calls: Mutex::new(Vec::new()),
        sessions: Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
        ptys: Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            DeliveryBackend,
        )))),
        purge: Arc::new(crate::session::purge_guard::PurgeGuard::default()),
        change: None,
        fail: false,
    }
}

#[tokio::test]
async fn issue_3260_delivery_target_and_purge_exit_before_sessions() {
    let (_tmp, project, _replica, config) = recording_delivery_fixture();
    let host = recording_delivery_host();
    let report = deliver_loop_prompt(
        &host,
        Path::new("missing"),
        &config,
        Uuid::new_v4(),
        Utc::now(),
    )
    .await;
    assert!(matches!(report.kind, LoopAuditKind::TargetUnresolved));
    assert!(host.calls.lock().unwrap().is_empty());
    let target = resolve_loop_target(&project, &config).unwrap();
    let _lease = host
        .purge
        .acquire(HashSet::new(), HashSet::from([target.target_fqn]))
        .await;
    let report = deliver_loop_prompt(&host, &project, &config, Uuid::new_v4(), Utc::now()).await;
    assert!(matches!(report.kind, LoopAuditKind::SkippedBusy));
    assert_eq!(*host.calls.lock().unwrap(), ["purge"]);
}

#[tokio::test]
async fn issue_3260_delivery_callback_and_success_only_prompt_effects() {
    for outcome in ["success", "stale", "failure", "busy", "fresh-busy"] {
        let (_tmp, project, replica, mut config) = recording_delivery_fixture();
        let mut host = recording_delivery_host();
        let id = host
            .sessions
            .read()
            .await
            .create_session(
                "codex".into(),
                Vec::new(),
                replica.to_string_lossy().into(),
                None,
                None,
                Vec::<SessionRepo>::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap()
            .id;
        host.ptys
            .lock()
            .unwrap()
            .record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess);
        if outcome != "busy" && outcome != "fresh-busy" {
            host.sessions.read().await.mark_idle(id).await;
        }
        if outcome == "fresh-busy" {
            config.policy.session_start = LoopSessionStart::Fresh;
            write_loop_config(&project.join(".ac"), &config).unwrap();
        }
        if outcome == "stale" {
            let mut changed = config.clone();
            changed.prompt.body = "changed after injection starts".into();
            host.change = Some((project.join(".ac"), changed));
        }
        host.fail = outcome == "failure";
        let report =
            deliver_loop_prompt(&host, &project, &config, Uuid::new_v4(), Utc::now()).await;
        let calls = host.calls.lock().unwrap().clone();
        let mut expected = vec!["purge", "sessions", "ptys", "sessions"];
        if outcome == "busy" || outcome == "fresh-busy" {
            assert!(matches!(report.kind, LoopAuditKind::PendingBusy));
        } else {
            expected.extend(["inject-built", "inject-polled"]);
            if outcome != "stale" {
                expected.push("write");
            }
            if outcome == "success" {
                expected.extend(["note", "sessions", "event"]);
                assert!(matches!(report.kind, LoopAuditKind::Delivered));
            } else {
                assert!(matches!(report.kind, LoopAuditKind::DeliveryFailed));
            }
        }
        assert_eq!(calls, expected, "{outcome}");
        let sessions = host.sessions.read().await.list_sessions().await;
        let prompt = sessions
            .iter()
            .find(|s| s.id == id.to_string())
            .unwrap()
            .last_prompt
            .clone();
        assert_eq!(
            prompt,
            (outcome == "success").then(|| config.prompt.body.clone())
        );
    }
}

#[tokio::test]
async fn issue_3260_delivery_exited_candidate_never_looks_up_pty() {
    let (_tmp, _project, replica, _config) = recording_delivery_fixture();
    let host = recording_delivery_host();
    let mgr = host.sessions.read().await;
    let session = mgr
        .create_session(
            "codex".into(),
            Vec::new(),
            replica.to_string_lossy().into(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    mgr.mark_exited(session.id, 0).await;
    drop(mgr);
    let lookup = find_coordinator_session(&host, &replica).await.unwrap();
    assert!(lookup.live.is_none());
    assert_eq!(lookup.stale_session_ids, [session.id]);
    assert_eq!(*host.calls.lock().unwrap(), ["sessions"]);
}
