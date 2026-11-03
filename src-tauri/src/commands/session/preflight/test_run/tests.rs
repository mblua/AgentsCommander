use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::*;
use crate::commands::session::tests::support::{strict_target_fixture, test_settings};
use crate::config::settings::{new_settings_state, ConfigSeedConfig, ProfileCellConfig};
use crate::errors::AppError;
use crate::process::agent::{with_probe_settlement_hook, ProbeNativeObservation};
use crate::pty::backend::{PreflightLaunch, PreflightLaunchRequest, PtyBackend};

const ID: &str = "fake";

/// Runs every preflight request as given; records requests and CLI spawns.
#[derive(Default)]
struct RecordingBackend {
    spawns: AtomicUsize,
    requests: Mutex<Vec<PreflightLaunchRequest>>,
}

impl PtyBackend for RecordingBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        _spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), AppError> {
        Err(AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
        Ok(())
    }

    fn kill(&self, _id: Uuid) -> Result<(), AppError> {
        Ok(())
    }

    fn has_session(&self, _id: Uuid) -> bool {
        false
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(&self, _id: Uuid, _request_id: String, _dir: PathBuf) {}

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }

    fn prepare_preflight_launch(
        &self,
        request: PreflightLaunchRequest,
    ) -> Result<PreflightLaunch, AppError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(PreflightLaunch {
            program: request.program,
            args: request.args,
            windows_raw_args: None,
            cwd: request.cwd,
            env: std::env::vars().collect(),
        })
    }
}

fn text(script: &str) -> String {
    if cfg!(windows) {
        format!("cmd.exe /D /C \"{script}\"")
    } else {
        format!("sh -c \"{script}\"")
    }
}

fn long_sleep() -> String {
    text(if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    })
}

fn cell(preflight: Option<String>) -> ProfileCellConfig {
    ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        enabled: true,
        command: String::new(),
        preflight,
        preflight_timeout_seconds: None,
        env: Default::default(),
        notes: String::new(),
    }
}

/// One plain agent `fake` running `fake-cli` (never Codex or OpenCode).
fn settings(ca: Option<String>, profile: Option<String>) -> AppSettings {
    let mut cfg = test_settings();
    cfg.agents.truncate(1);
    let agent = &mut cfg.agents[0];
    agent.id = ID.to_string();
    agent.label = "Fake".to_string();
    agent.command = "fake-cli".to_string();
    agent.preflight = ca;
    if profile.is_some() {
        let cells = [("A".to_string(), cell(profile))].into_iter().collect();
        let by_agent = &mut cfg.coding_agent_profiles.profiles_by_agent;
        by_agent.insert(ID.to_string(), cells);
    }
    cfg
}

struct Fixture {
    _temp: tempfile::TempDir,
    cwd: String,
    other: String,
    cfg: AppSettings,
    backend: Arc<RecordingBackend>,
    settings: SettingsState,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<std::sync::Mutex<PtyManager>>,
    state: Arc<PreflightState>,
}

impl Fixture {
    fn new(cfg: AppSettings) -> Self {
        let (temp, cwd, other) = strict_target_fixture();
        let backend = Arc::new(RecordingBackend::default());
        let pty_mgr = Arc::new(std::sync::Mutex::new(PtyManager::new_for_test(
            backend.clone(),
        )));
        Self {
            _temp: temp,
            cwd,
            other,
            settings: new_settings_state(cfg.clone()),
            cfg,
            backend,
            session_mgr: Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
            pty_mgr,
            state: Arc::default(),
        }
    }

    fn deps(&self) -> PreflightTestDeps {
        PreflightTestDeps {
            settings: self.settings.clone(),
            session_mgr: self.session_mgr.clone(),
            pty_mgr: self.pty_mgr.clone(),
            state: self.state.clone(),
        }
    }

    fn revision(&self) -> String {
        preflight_revision(&self.cfg, ID)
    }

    async fn describe_at(&self, path: &str, letter: Option<&str>) -> PreflightTargetDescription {
        let letter = letter.map(String::from);
        describe_preflight_target_inner(&self.deps(), ID.to_string(), letter, path.to_string())
            .await
            .expect("describe")
    }

    async fn describe(&self) -> PreflightTargetDescription {
        self.describe_at(&self.cwd, None).await
    }

    async fn start_at(&self, path: &str, revision: String) -> Result<String, String> {
        let (id, path) = (ID.to_string(), path.to_string());
        start_preflight_test_inner(self.deps(), id, None, path, revision).await
    }

    async fn start(&self) -> Result<String, String> {
        self.start_at(&self.cwd, self.revision()).await
    }

    async fn prepare(&self) -> (String, PreflightTestTask) {
        let (id, path) = (ID.to_string(), self.cwd.clone());
        prepare_preflight_test(self.deps(), id, None, path, self.revision())
            .await
            .expect("prepare")
    }

    async fn live_session(&self, cwd: &str) -> crate::session::session::Session {
        self.session_mgr
            .read()
            .await
            .create_session(
                "sh".to_string(),
                Vec::new(),
                cwd.to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("live session")
    }

    fn get(&self, run_id: &str) -> PreflightTestSnapshot {
        get_preflight_test_inner(&self.state, run_id).expect("known run")
    }

    async fn wait_until(&self, run_id: &str, done: impl Fn(&PreflightTestSnapshot) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done(&self.get(run_id)) {
            assert!(Instant::now() < deadline, "{:?}", self.get(run_id));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn finished(&self, run_id: &str) -> PreflightTestSnapshot {
        self.wait_until(run_id, |s| s.phase == PreflightTestPhase::Finished)
            .await;
        self.get(run_id)
    }

    /// The hold is dropped right after the final snapshot write.
    async fn wait_free(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.state.is_free(&self.cwd) {
            assert!(Instant::now() < deadline, "folder still held");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

fn geometry() -> crate::config::settings::WindowGeometry {
    crate::config::settings::WindowGeometry {
        x: 1.0,
        y: 2.0,
        width: 3.0,
        height: 4.0,
    }
}

fn pass() -> AppSettings {
    settings(Some(text("exit 0")), Some(text("exit 0")))
}

#[tokio::test]
async fn describe_lists_saved_stages_and_revision() {
    let fx = Fixture::new(pass());
    let description = fx.describe().await;
    assert_eq!(description.unavailable_reason, None);
    let sources: Vec<_> = description.stages.iter().map(|s| s.source).collect();
    let expected = [
        PreflightStageSource::CodingAgent,
        PreflightStageSource::Profile,
    ];
    assert_eq!(sources, expected);
    assert_eq!(description.stages[0].command_text, text("exit 0"));
    assert!(description.stopped);
    assert_eq!(
        description.settings_revision,
        preflight_revision(&fx.cfg, ID)
    );
    assert_eq!(description.active_run_id, None);
    assert_eq!(description.effective_profile, "A");
}

#[tokio::test]
async fn describe_empty_chain_has_no_stages() {
    let fx = Fixture::new(settings(None, None));
    let description = fx.describe().await;
    assert!(description.stages.is_empty());
    assert_eq!(description.unavailable_reason, None);
}

#[tokio::test]
async fn describe_rejects_non_agent_folder() {
    let fx = Fixture::new(pass());
    let plain = tempfile::tempdir().unwrap();
    let description = fx.describe_at(&plain.path().to_string_lossy(), None).await;
    let reason = description.unavailable_reason.as_deref();
    assert_eq!(reason, Some("This folder is not an AgentsCommander agent."));
    assert!(description.stages.is_empty());
    assert_eq!(description.settings_revision, fx.revision());
}

#[tokio::test]
async fn describe_rejects_container_agent() {
    let mut cfg = pass();
    cfg.agents[0].backend.kind = SessionBackendKind::ContainerTransport;
    let description = Fixture::new(cfg).describe().await;
    let reason = description.unavailable_reason.as_deref();
    assert_eq!(reason, Some("Preflights cannot run for container agents."));
    assert!(description.stages.is_empty());
}

#[tokio::test]
async fn describe_rejects_profile_mismatch() {
    let fx = Fixture::new(pass());
    let description = fx.describe_at(&fx.cwd, Some("B")).await;
    let reason = description.unavailable_reason.as_deref();
    assert_eq!(reason, Some("This agent uses Profile A, not Profile B."));
    assert!(description.stages.is_empty());
    let matching = fx.describe_at(&fx.cwd, Some("A")).await;
    assert_eq!(matching.unavailable_reason, None);
}

#[tokio::test]
async fn describe_rejects_invalid_preflight() {
    let fx = Fixture::new(settings(Some("pre \"unclosed".to_string()), None));
    let description = fx.describe().await;
    let reason = description.unavailable_reason.expect("unavailable");
    assert!(
        reason.starts_with("Invalid Coding Agent preflight command: "),
        "{reason}"
    );
    assert!(description.stages.is_empty());
}

#[test]
fn revision_scope() {
    let base = pass();
    let revision = preflight_revision(&base, ID);
    let keeps: [fn(&mut AppSettings); 5] = [
        |c| c.theme_light = !c.theme_light,
        |c| c.sounds_enabled = !c.sounds_enabled,
        |c| c.rail_favorites_collapsed = !c.rail_favorites_collapsed,
        |c| c.watchers_geometry = Some(geometry()),
        |c| c.root_token = Some("changed".to_string()),
    ];
    for (index, edit) in keeps.iter().enumerate() {
        let mut cfg = base.clone();
        edit(&mut cfg);
        assert_eq!(preflight_revision(&cfg, ID), revision, "keep {index}");
    }
    let moves: [fn(&mut AppSettings); 5] = [
        |c| c.default_shell.push_str("-other"),
        |c| c.agents[0].preflight_timeout_seconds = Some(9),
        |c| {
            let cells = c
                .coding_agent_profiles
                .profiles_by_agent
                .get_mut(ID)
                .unwrap();
            cells.get_mut("A").unwrap().preflight = Some("other".to_string());
        },
        |c| c.default_shell_args.push("-x".to_string()),
        |c| {
            c.agents[0].config_seed = Some(ConfigSeedConfig {
                enabled: true,
                dest: ".cfg".to_string(),
            })
        },
    ];
    for (index, edit) in moves.iter().enumerate() {
        let mut cfg = base.clone();
        edit(&mut cfg);
        assert_ne!(preflight_revision(&cfg, ID), revision, "move {index}");
    }
    let mut other = base.clone();
    let mut second = other.agents[0].clone();
    second.id = "second".to_string();
    other.agents.push(second);
    assert_eq!(preflight_revision(&other, ID), revision);
    other.agents[1].preflight = Some("x".to_string());
    assert_eq!(preflight_revision(&other, ID), revision);
}

#[tokio::test]
async fn start_rejects_stale_revision() {
    let fx = Fixture::new(pass());
    let error = fx.start_at(&fx.cwd, "x".to_string()).await.unwrap_err();
    assert_eq!(error, "Save your changes before testing preflights.");
    assert!(fx.state.runs().runs.is_empty());
}

#[tokio::test]
async fn start_rejects_live_and_pending_session() {
    let fx = Fixture::new(pass());
    let session = fx.live_session(&fx.cwd).await;
    assert_eq!(fx.start().await.unwrap_err(), STOP_FIRST);
    drop(session);

    let fx = Fixture::new(pass());
    let manager = fx.session_mgr.read().await;
    manager
        .insert_pending_session_for_test(fx.cwd.clone())
        .await;
    drop(manager);
    assert!(!fx.describe().await.stopped);
    assert_eq!(fx.start().await.unwrap_err(), STOP_FIRST);

    let fx = Fixture::new(pass());
    let manager = fx.session_mgr.read().await;
    manager
        .insert_pending_session_for_test(fx.other.clone())
        .await;
    drop(manager);
    let run_id = fx.start().await.expect("another folder's pending session");
    fx.finished(&run_id).await;
}

#[tokio::test]
async fn start_refused_while_launch_shares() {
    let fx = Fixture::new(pass());
    let share = fx.state.try_share(&fx.cwd).expect("share");
    let error = fx.start().await.unwrap_err();
    assert_eq!(
        error,
        "This agent is starting. Try again when it is stopped."
    );
    drop(share);
    assert!(fx.state.runs().runs.is_empty());
}

#[tokio::test]
async fn run_passes_and_never_creates_session() {
    let fx = Fixture::new(pass());
    let run_id = fx.start().await.expect("start");
    let snapshot = fx.finished(&run_id).await;
    assert_eq!(snapshot.outcome, Some(PreflightTestOutcome::Passed));
    let message = "Test passed. Preflights will run again when you launch.";
    assert_eq!(snapshot.message.as_deref(), Some(message));
    assert_eq!(snapshot.stages.len(), 2);
    assert!(fx.session_mgr.read().await.list_sessions().await.is_empty());
    assert_eq!(fx.backend.spawns.load(Ordering::SeqCst), 0);
    assert_eq!(fx.backend.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn run_failure_marks_not_run() {
    let fx = Fixture::new(settings(Some(text("exit 2")), Some(text("exit 0"))));
    let run_id = fx.start().await.expect("start");
    let snapshot = fx.finished(&run_id).await;
    assert_eq!(snapshot.outcome, Some(PreflightTestOutcome::Failed));
    let message = "Test failed. Launch would be blocked.";
    assert_eq!(snapshot.message.as_deref(), Some(message));
    assert_eq!(snapshot.stages[0].status, PreflightStageStatus::Failed);
    assert_eq!(snapshot.stages[0].exit_code, Some(2));
    assert_eq!(snapshot.stages[1].status, PreflightStageStatus::NotRun);
}

#[tokio::test]
async fn cancel_yields_cancelled() {
    let fx = Fixture::new(settings(Some(long_sleep()), None));
    let run_id = fx.start().await.expect("start");
    fx.wait_until(&run_id, |s| s.phase == PreflightTestPhase::Running)
        .await;
    let cancelling = cancel_preflight_test_inner(&fx.state, &run_id).expect("cancel");
    assert_ne!(cancelling.outcome, Some(PreflightTestOutcome::Passed));
    let snapshot = fx.finished(&run_id).await;
    assert_eq!(snapshot.outcome, Some(PreflightTestOutcome::Cancelled));
    let message = "Test cancelled. No passing result.";
    assert_eq!(snapshot.message.as_deref(), Some(message));
    let again = cancel_preflight_test_inner(&fx.state, &run_id).expect("finished run");
    assert_eq!(again, snapshot);
}

#[tokio::test]
async fn progress_is_live() {
    let fx = Fixture::new(settings(Some(long_sleep()), Some(text("exit 0"))));
    let run_id = fx.start().await.expect("start");
    fx.wait_until(&run_id, |s| {
        s.phase == PreflightTestPhase::Running
            && s.stages.first().map(|r| r.status) == Some(PreflightStageStatus::Running)
    })
    .await;
    let live = fx.get(&run_id);
    assert_eq!(live.stages[1].status, PreflightStageStatus::Pending);
    cancel_preflight_test_inner(&fx.state, &run_id).expect("cancel");
    fx.finished(&run_id).await;
}

#[tokio::test]
async fn test_excludes_launch() {
    let fx = Fixture::new(settings(Some(long_sleep()), None));
    let run_id = fx.start().await.expect("start");
    assert_eq!(fx.state.try_share(&fx.cwd).err(), Some(HoldRefusal::Test));
    let description = fx.describe().await;
    assert_eq!(description.active_run_id, Some(run_id.clone()));
    assert!(!description.stopped);
    let second = fx.start().await.unwrap_err();
    assert_eq!(second, "A preflight test is already running on this agent.");
    cancel_preflight_test_inner(&fx.state, &run_id).expect("cancel");
    fx.finished(&run_id).await;
    fx.wait_free().await;
    assert!(fx.state.try_share(&fx.cwd).is_ok());
    assert_eq!(fx.describe().await.active_run_id, None);
}

#[tokio::test]
async fn pending_cleanup_keeps_folder_unavailable() {
    let fx = Fixture::new(settings(Some(text("exit 0")), None));
    let (run_id, task) = fx.prepare().await;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let hook = crate::process::agent::ProbeSettlementHook {
        before_native_proof: Some(ready_tx),
        release_native_proof: Some(release_rx),
        settlement_window: None,
        native_observations: [ProbeNativeObservation::Error("x")].into(),
        panic_after_native_proof_release: false,
        panic_after_native_query_defect: false,
        stdout_reader: None,
    };
    let checker = async {
        ready_rx.await.expect("cleanup reached");
        assert_ne!(fx.get(&run_id).phase, PreflightTestPhase::Finished);
        assert_eq!(fx.state.try_share(&fx.cwd).err(), Some(HoldRefusal::Test));
        let description = fx.describe().await;
        assert!(!description.stopped);
        assert_eq!(description.active_run_id, Some(run_id.clone()));
        release_tx.send(()).expect("release");
    };
    with_probe_settlement_hook(hook, async { tokio::join!(task, checker) }).await;
    let snapshot = fx.get(&run_id);
    assert_eq!(snapshot.phase, PreflightTestPhase::Finished);
    assert_eq!(snapshot.outcome, Some(PreflightTestOutcome::Failed));
    let defect = snapshot.stages[0].cleanup_defect.clone().expect("defect");
    assert!(defect.contains('x'), "{defect}");
    assert!(fx.state.try_share(&fx.cwd).is_ok());
}

#[tokio::test]
async fn test_token_matches_no_session() {
    let fx = Fixture::new(settings(Some(text("exit 0")), None));
    let session = fx.live_session(&fx.other).await;
    let run_id = fx.start().await.expect("start");
    fx.finished(&run_id).await;
    let requests = fx.backend.requests.lock().unwrap();
    let token = requests[0]
        .extra_env
        .iter()
        .find(|(key, _)| key == "AGENTSCOMMANDER_TOKEN")
        .map(|(_, value)| value.clone())
        .expect("token env");
    let parsed = Uuid::parse_str(&token).expect("uuid token");
    assert_ne!(parsed, session.token);
}

#[test]
fn retention_is_bounded() {
    let state = PreflightState::default();
    let description = PreflightTargetDescription {
        agent_id: ID.to_string(),
        replica_path: String::new(),
        cwd: String::new(),
        requested_profile: String::new(),
        effective_profile: String::new(),
        fallback_applied: false,
        stopped: true,
        stages: Vec::new(),
        settings_revision: String::new(),
        active_run_id: None,
        unavailable_reason: None,
    };
    let unfinished = pending_snapshot("unfinished", &description);
    let (cancel, _) = tokio::sync::watch::channel(false);
    let run = StoredRun {
        snapshot: unfinished,
        cancel,
        key: String::new(),
    };
    state.runs().insert("unfinished".to_string(), run);
    let ids: Vec<String> = (0..20).map(|n| format!("run-{n}")).collect();
    for id in &ids {
        let mut snapshot = pending_snapshot(id, &description);
        finish(&mut snapshot, PreflightTestOutcome::Passed);
        let (cancel, _) = tokio::sync::watch::channel(false);
        let key = String::new();
        state.runs().insert(
            id.clone(),
            StoredRun {
                snapshot,
                cancel,
                key,
            },
        );
    }
    assert_eq!(state.runs().runs.len(), MAX_STORED_RUNS);
    assert_eq!(state.runs().order.len(), MAX_STORED_RUNS);
    let kept = get_preflight_test_inner(&state, "unfinished").expect("unfinished run kept");
    assert_eq!(kept.phase, PreflightTestPhase::Preparing);
    assert_eq!(
        get_preflight_test_inner(&state, &ids[4]),
        Err(UNKNOWN_RUN.to_string())
    );
    assert!(get_preflight_test_inner(&state, &ids[5]).is_ok());
    assert_eq!(
        get_preflight_test_inner(&state, &ids[0]),
        Err(UNKNOWN_RUN.to_string())
    );
    assert!(get_preflight_test_inner(&state, &ids[19]).is_ok());
    let unknown = get_preflight_test_inner(&state, "nope").unwrap_err();
    assert_eq!(unknown, "Unknown preflight test run.");
    let unknown = cancel_preflight_test_inner(&state, "nope").unwrap_err();
    assert_eq!(unknown, "Unknown preflight test run.");
}

#[tokio::test]
async fn test_never_seeds() {
    let mut cfg = settings(Some(text("exit 0")), None);
    cfg.agents[0].config_seed = Some(ConfigSeedConfig {
        enabled: true,
        dest: ".fake-config".to_string(),
    });
    let fx = Fixture::new(cfg);
    let spawn = build_configured_agent_spawn_for_cwd(&fx.cfg, ID, &fx.cwd, None)
        .expect("resolve")
        .expect("configured");
    let seed = spawn.seed.expect("active seed");
    let source = &seed.candidates[0].path;
    std::fs::create_dir_all(source).unwrap();
    // CI's TEMP is an 8.3 short name; the resolver spells the long one. Compare canonical forms.
    let canonical = std::fs::canonicalize(source).unwrap();
    let root = std::fs::canonicalize(fx._temp.path()).unwrap();
    assert!(
        canonical.starts_with(&root),
        "{canonical:?} not under {root:?}"
    );
    std::fs::write(source.join("seeded.txt"), "seed").unwrap();
    std::fs::create_dir_all(&seed.dest).unwrap();
    std::fs::write(seed.dest.join("kept.txt"), "kept bytes").unwrap();

    let run_id = fx.start().await.expect("start");
    let snapshot = fx.finished(&run_id).await;
    assert_eq!(snapshot.outcome, Some(PreflightTestOutcome::Passed));
    assert!(!seed.dest.join("seeded.txt").exists());
    assert_eq!(
        std::fs::read(seed.dest.join("kept.txt")).unwrap(),
        b"kept bytes"
    );

    let mut control = seed.clone();
    control.dest = PathBuf::from(&fx.cwd).join(".fake-config-control");
    let report = crate::config::config_seed::perform_config_seed(&control, "p04");
    assert!(control.dest.join("seeded.txt").exists(), "{report:?}");
}

#[test]
fn test_hold_rules() {
    let state = Arc::new(PreflightState::default());
    let cwd = "C:\\A\\b";
    let first = state.try_share(cwd).expect("share");
    let second = state.try_share(cwd).expect("share");
    assert_eq!(state.try_test(cwd).err(), Some(HoldRefusal::Launching));
    drop(first);
    assert_eq!(state.try_test(cwd).err(), Some(HoldRefusal::Launching));
    drop(second);
    let hold = state.try_test(cwd).expect("test hold");
    assert!(!state.is_free(cwd));
    assert_eq!(state.try_share(cwd).err(), Some(HoldRefusal::Test));
    assert_eq!(state.try_test(cwd).err(), Some(HoldRefusal::Test));
    drop(hold);
    assert!(state.is_free(cwd));
    if cfg!(windows) {
        let hold = state.try_test("C:\\A\\b").expect("test hold");
        assert_eq!(state.try_share("c:/a/B/").err(), Some(HoldRefusal::Test));
        assert!(!state.is_free("c:/a/B/"));
        drop(hold);
    }
}
