use super::*;

#[derive(Default)]
struct AutoCloseBackend {
    live: Mutex<HashSet<Uuid>>,
}

impl AutoCloseBackend {
    fn set_live(&self, id: Uuid) {
        self.live.lock().unwrap().insert(id);
    }
}

impl crate::pty::backend::PtyBackend for AutoCloseBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.set_live(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.live
            .lock()
            .unwrap()
            .contains(&id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        self.write(
            &crate::pty::manager::BackendWriteAuthority::for_backend_test(),
            id,
            &[],
        )
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.has_session(id).then_some((30, 120))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: std::path::PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

fn key(id: Uuid, team: &str) -> (Uuid, String) {
    (id, team.to_string())
}

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("valid timestamp")
}

// ---- team_min_silence (folds the per-member team-clock age) ----

fn secs_of(table: &[(Uuid, u64)]) -> impl Fn(Uuid) -> Option<Duration> + '_ {
    |id| {
        table
            .iter()
            .find(|(m, _)| *m == id)
            .map(|(_, s)| Duration::from_secs(*s))
    }
}

#[test]
fn team_min_silence_takes_min_over_contributing_members() {
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
    let out = team_min_silence(&members, &secs_of(&[(a, 30), (b, 10)]));
    assert_eq!(out.get("proj:wg-1"), Some(&Duration::from_secs(10)));
}

#[test]
fn team_min_silence_absent_when_no_member_contributes() {
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
    // No eligible stamp / untracked -> None for each.
    let out = team_min_silence(&members, &|_| None);
    assert!(
        !out.contains_key("proj:wg-1"),
        "a team with no contributing member must be ABSENT (anchor frozen)"
    );
    assert!(out.is_empty());
}

#[test]
fn team_min_silence_isolates_teams() {
    // a: team 1, contributes 40; b: team 2, contributes 5; c: team 2, untracked.
    let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let members = vec![
        key(a, "proj:wg-1"),
        key(b, "proj:wg-2"),
        key(c, "proj:wg-2"),
    ];
    let out = team_min_silence(&members, &secs_of(&[(a, 40), (b, 5)]));
    assert_eq!(out.get("proj:wg-1"), Some(&Duration::from_secs(40)));
    assert_eq!(out.get("proj:wg-2"), Some(&Duration::from_secs(5)));
}

// ---- team_idle_since_secs (unified max(user, activity) anchor) ----

#[test]
fn idle_since_secs_picks_max_present_component() {
    let (t100, t200) = (ts(100).timestamp(), ts(200).timestamp());
    assert_eq!(team_idle_since_secs(Some(ts(100)), None), t100);
    assert_eq!(team_idle_since_secs(None, Some(ts(200))), t200);
    let both = team_idle_since_secs(Some(ts(100)), Some(ts(200)));
    assert_eq!(both, t200, "max of the two present components");
    let none = team_idle_since_secs(None, None);
    assert_eq!(none, i64::MIN, "both absent -> MIN (no badge)");
}

// ---- team_is_closeable (the KILL predicate; replaces the teams_to_close tests) ----

#[test]
fn closeable_when_established_and_idle_past_timeout() {
    let now = ts(10_000).timestamp();
    let timeout = 3600; // 60 min
                        // 3700s idle > 3600s timeout.
    let anchor = team_idle_since_secs(Some(ts(10_000 - 3700)), None);
    assert!(team_is_closeable(true, anchor, now, timeout));
}

#[test]
fn not_closeable_without_established_member() {
    // wake grace: no member alive past WAKE_GRACE -> not established -> never closes.
    let now = ts(10_000).timestamp();
    let anchor = team_idle_since_secs(Some(ts(10_000 - 3700)), None);
    assert!(
        !team_is_closeable(false, anchor, now, 3600),
        "no established member must protect the team (wake grace)"
    );
}

#[test]
fn not_closeable_when_user_message_keeps_anchor_fresh() {
    let now = ts(10_000).timestamp();
    // Recent user message (60s ago) keeps the anchor fresh even though activity is old.
    let anchor = team_idle_since_secs(Some(ts(10_000 - 60)), Some(ts(10_000 - 9000)));
    assert!(
        !team_is_closeable(true, anchor, now, 3600),
        "a recent last_user_message_at must keep the team open"
    );
}

#[test]
fn not_closeable_when_activity_recent() {
    let now = ts(10_000).timestamp();
    // Recent activity (120s ago) keeps the anchor fresh even though the user message is old.
    let anchor = team_idle_since_secs(Some(ts(10_000 - 9000)), Some(ts(10_000 - 120)));
    assert!(
        !team_is_closeable(true, anchor, now, 3600),
        "a recent last_activity_at must keep the team open"
    );
}

#[test]
fn not_closeable_when_anchor_absent() {
    let now = ts(10_000).timestamp();
    assert!(
        !team_is_closeable(true, i64::MIN, now, 3600),
        "a team with no anchor (i64::MIN) is never closeable"
    );
}

#[test]
fn telegram_protection_is_opt_in() {
    assert!(!session_is_telegram_protected(false, true));
    assert!(!session_is_telegram_protected(false, false));
}

#[test]
fn telegram_protection_skips_only_assigned_sessions_when_enabled() {
    assert!(session_is_telegram_protected(true, true));
    assert!(!session_is_telegram_protected(true, false));
}

#[test]
fn protected_member_not_selected_when_established_and_idle_past_timeout() {
    let now = ts(10_000).timestamp();
    let timeout = 3600;
    let anchor = team_idle_since_secs(Some(ts(10_000 - 3700)), None);
    assert!(!should_close_member(true, true, anchor, now, timeout));
}

#[test]
fn unprotected_member_selected_when_established_and_idle_past_timeout() {
    let now = ts(10_000).timestamp();
    let timeout = 3600;
    let anchor = team_idle_since_secs(Some(ts(10_000 - 3700)), None);
    assert!(should_close_member(false, true, anchor, now, timeout));
}

// ---- orphan_anchor_secs (#582 orphan fallback anchor) ----

#[test]
fn orphan_anchor_uses_member_own_silence_and_becomes_closeable() {
    let now = ts(10_000).timestamp();
    let anchor = orphan_anchor_secs(Some(Duration::from_secs(3700)), now);
    assert_eq!(anchor, now - 3700);
    assert!(
        team_is_closeable(true, anchor, now, 3600),
        "an orphan silent past the timeout must become closeable"
    );
}

#[test]
fn orphan_anchor_fresh_silence_keeps_member_open() {
    let now = ts(10_000).timestamp();
    let anchor = orphan_anchor_secs(Some(Duration::from_secs(30)), now);
    assert!(
        !team_is_closeable(true, anchor, now, 3600),
        "a recently-active orphan must keep its full-timeout lease"
    );
}

#[test]
fn orphan_anchor_untracked_is_min_and_not_closeable() {
    let now = ts(10_000).timestamp();
    assert_eq!(orphan_anchor_secs(None, now), i64::MIN);
    assert!(!team_is_closeable(true, i64::MIN, now, 3600));
}

// ---- resolve_member_anchor (#582 two-case anchor; HIGH-1 regression guard) ----

#[test]
fn resolve_anchor_orphan_uses_member_silence() {
    let now = ts(10_000).timestamp();
    let snap: std::collections::HashMap<String, ClockEntry> = std::collections::HashMap::new();
    // No coordinator record -> genuine orphan -> own silence.
    let anchor = resolve_member_anchor(None, &snap, Some(Duration::from_secs(3700)), now);
    assert_eq!(anchor, now - 3700);
    assert!(team_is_closeable(true, anchor, now, 3600));
}

#[test]
fn resolve_anchor_live_coordinator_empty_snapshot_stays_min() {
    // HIGH-1 GUARD: a LIVE coordinator (record present) whose FQN is NOT in the
    // snapshot (e.g. a team restored after restart, FQN not yet in the clock
    // file) must stay i64::MIN / not-closeable -- it must NOT fall back to the
    // member's raw silence. This is the normal path, unchanged from before #582.
    let now = ts(10_000).timestamp();
    let snap: std::collections::HashMap<String, ClockEntry> = std::collections::HashMap::new();
    let coord = ("proj:wg-1/tech-lead".to_string(), "C:/x".to_string());
    let anchor = resolve_member_anchor(
        Some(&coord),
        &snap,
        Some(Duration::from_secs(99_999)), // very idle member -- MUST be ignored
        now,
    );
    assert_eq!(
        anchor,
        i64::MIN,
        "live coordinator + empty snapshot must NOT use member silence"
    );
    assert!(
        !team_is_closeable(true, anchor, now, 3600),
        "an unclocked LIVE coordinator team must stay open (normal path)"
    );
}

#[test]
fn resolve_anchor_live_coordinator_uses_team_clock() {
    let now = ts(10_000).timestamp();
    let fqn = "proj:wg-1/tech-lead".to_string();
    let coord = (fqn.clone(), "C:/x".to_string());
    let mut snap: std::collections::HashMap<String, ClockEntry> = std::collections::HashMap::new();
    snap.insert(
        fqn,
        ClockEntry {
            last_user_message_at: Some(ts(10_000 - 3700)),
            last_activity_at: None,
            auto_closed_at: None,
            // #588 added this required field to ClockEntry; the orphan-anchor
            // path (#582) ignores it, so None keeps this test's intent intact.
            manually_closed_at: None,
            // (#756) same reasoning: the anchor path ignores the fresh intent.
            start_fresh_at: None,
        },
    );
    // Normal path: anchor from the persisted team clock; member silence ignored.
    let anchor = resolve_member_anchor(Some(&coord), &snap, Some(Duration::from_secs(1)), now);
    assert_eq!(anchor, ts(10_000 - 3700).timestamp());
    assert!(team_is_closeable(true, anchor, now, 3600));
}

// ---- destroyed_is_team_coordinator (#589 gated AUTO-CLOSED mark) ----

#[test]
fn mark_gate_true_only_when_destroyed_is_the_coordinator() {
    let coord = Uuid::new_v4();
    let member = Uuid::new_v4();

    // The destroyed session IS the team's coordinator -> the row may be marked.
    assert!(
        destroyed_is_team_coordinator(coord, Some(coord)),
        "destroying the coordinator's own session marks the row"
    );

    // A sibling member was reaped while the coordinator (coord) survives ->
    // the live coordinator row must NOT be stamped. This is the #589 bug.
    assert!(
        !destroyed_is_team_coordinator(member, Some(coord)),
        "a surviving coordinator must NOT be stamped when only a member is reaped"
    );

    // No coordinator record for the team -> never a coordinator close.
    assert!(
        !destroyed_is_team_coordinator(member, None),
        "absent coordinator id must not mark the row"
    );
}

struct AutoCloseFixture {
    _temp: tempfile::TempDir,
    app: tauri::App<tauri::test::MockRuntime>,
    backend: Arc<AutoCloseBackend>,
    coordinator_id: Uuid,
    member_id: Uuid,
    cwd: String,
    anchor: DateTime<Utc>,
    services: RecordingServices,
}

async fn auto_close_session(
    manager: &tokio::sync::RwLock<SessionManager>,
    cwd: &std::path::Path,
    coding_agent: Option<String>,
    is_coordinator: bool,
) -> Uuid {
    manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            cwd.to_string_lossy().into_owned(),
            coding_agent,
            None,
            Vec::new(),
            is_coordinator,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
        .id
}

async fn auto_close_fixture() -> AutoCloseFixture {
    use crate::config::settings::{AppSettings, SettingsState};
    use crate::pty::backend::SessionBackendKind;
    use crate::resource_monitor::ResourceMonitorState;
    use crate::session::selection::SelectionCoordinator;
    use crate::telegram::manager::{TelegramBridgeManager, TelegramBridgeState};

    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).expect("Root fixture creation succeeds");
    let temp = tempfile::tempdir().unwrap();
    let team = temp.path().join("myproj/.ac/wg-1-team");
    let coordinator_cwd = team.join("__agent_lead");
    let member_cwd = team.join("__agent_rust");
    for cwd in [&coordinator_cwd, &member_cwd] {
        std::fs::create_dir_all(cwd).unwrap();
        assert!(cwd.is_dir());
        assert_ne!(cwd.canonicalize().unwrap(), root.canonicalize().unwrap());
    }

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator_id = auto_close_session(&manager, &coordinator_cwd, None, true).await;
    let member_id =
        auto_close_session(&manager, &member_cwd, Some("codex".to_string()), false).await;

    let backend = Arc::new(AutoCloseBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    for id in [coordinator_id, member_id] {
        backend.set_live(id);
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
    }
    let idle = IdleDetector::new(|_| {}, |_| {});
    for id in [coordinator_id, member_id] {
        idle.set_auto_close_ages_for_test(id, Duration::from_secs(120), Duration::from_secs(120));
    }

    let settings = AppSettings {
        coordinator_auto_close_enabled: true,
        coordinator_auto_close_minutes: 1,
        coordinator_auto_close_skip_telegram_assigned: false,
        ..AppSettings::default()
    };
    let settings: SettingsState = crate::config::settings::new_settings_state(settings);
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let anchor = Utc::now() - chrono::Duration::seconds(120);
    clocks
        .lock()
        .unwrap()
        .note_user_message("myproj:wg-1-team/lead", anchor);
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), shutdown.token().clone());
    let output_senders = Arc::new(Mutex::new(HashMap::new()));
    let telegram: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(output_senders),
    ));
    let services = RecordingServices {
        settings: settings.clone(),
        manager: manager.clone(),
        pty: pty.clone(),
        idle: idle.clone(),
        coordinator: coordinator.clone(),
        clocks: clocks.clone(),
        notices: Mutex::new(Vec::new()),
    };
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(settings)
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(Arc::clone(&idle))
        .manage(clocks)
        .manage(crate::DetachedSessionsState::default())
        .manage(telegram)
        .manage(Arc::new(ResourceMonitorState::new()))
        .manage(coordinator.clone())
        .manage(shutdown)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build auto-close test app");
    coordinator
        .start(app.handle().clone())
        .expect("start selection coordinator");
    coordinator.submit_restore_first().await.unwrap().finish();

    AutoCloseFixture {
        _temp: temp,
        app,
        backend,
        coordinator_id,
        member_id,
        cwd: coordinator_cwd.to_string_lossy().into_owned(),
        anchor,
        services,
    }
}

#[tokio::test]
async fn selected_idle_team_closes_as_one_batch_and_clears_selection_once() {
    use crate::pty::backend::PtyBackend;
    use crate::session::selection::{SelectionMode, SelectionSource};
    use tauri::Listener;
    let fixture = auto_close_fixture().await;
    let app = &fixture.app;
    let manager = &fixture.services.manager;
    let backend = &fixture.backend;
    let coordinator = &fixture.services.coordinator;
    let (badges_tx, badges_rx) = std::sync::mpsc::channel();
    for name in [
        "coordinator_clock_updated",
        "coordinator_auto_close_changed",
    ] {
        let tx = badges_tx.clone();
        app.listen_any(name, move |event| {
            tx.send((
                name,
                serde_json::from_str::<serde_json::Value>(event.payload()).unwrap(),
            ))
            .unwrap();
        });
    }
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for event_name in ["session_destroyed", "session_switched"] {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |event| {
            let _ = events_tx.send((event_name, event.payload().to_string()));
        });
    }

    tick(app.handle(), &mut HashMap::new()).await;

    assert!(manager.read().await.list_sessions().await.is_empty());
    assert!(!backend.has_session(fixture.coordinator_id));
    assert!(!backend.has_session(fixture.member_id));
    let selection = manager.read().await.selection_payload().await;
    assert_eq!(selection.mode(), SelectionMode::None);
    assert_eq!(selection.source(), SelectionSource::AutoClose);
    let observed = (0..3)
        .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        observed.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        vec!["session_destroyed", "session_destroyed", "session_switched"]
    );
    let selection_payload: serde_json::Value = serde_json::from_str(&observed[2].1).unwrap();
    assert!(selection_payload["id"].is_null());
    assert_eq!(selection_payload["source"], "autoClose");
    assert!(events_rx.try_recv().is_err());
    let idle = badges_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        idle,
        (
            "coordinator_clock_updated",
            serde_json::json!({
                "replicaPath": fixture.cwd,
                "lastUserMessageAt": DateTime::<Utc>::from_timestamp(fixture.anchor.timestamp(), 0)
                    .unwrap().to_rfc3339(),
            })
        )
    );
    let marker = fixture.services.clocks.lock().unwrap().snapshot()["myproj:wg-1-team/lead"]
        .auto_closed_at
        .unwrap();
    let closed = badges_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        closed,
        (
            "coordinator_auto_close_changed",
            serde_json::json!({
                "replicaPath": fixture.cwd, "autoClosedAt": marker.to_rfc3339(),
            })
        )
    );
    assert!(badges_rx.try_recv().is_err());
    coordinator.close_and_join().await;
}

#[derive(Debug, PartialEq)]
enum Badge {
    Idle(String, DateTime<Utc>),
    Closed(String, DateTime<Utc>),
}

struct RecordingServices {
    settings: SettingsState,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pty: Arc<Mutex<PtyManager>>,
    idle: Arc<IdleDetector>,
    coordinator: SelectionCoordinator,
    clocks: CoordinatorClocksState,
    notices: Mutex<Vec<Badge>>,
}

impl AutoCloseServices for RecordingServices {
    fn settings(&self) -> SettingsState {
        self.settings.clone()
    }
    fn session_manager(&self) -> Arc<tokio::sync::RwLock<SessionManager>> {
        self.manager.clone()
    }
    fn pty_manager(&self) -> Arc<Mutex<PtyManager>> {
        self.pty.clone()
    }
    fn idle_detector(&self) -> Arc<IdleDetector> {
        self.idle.clone()
    }
    fn selection_coordinator(&self) -> SelectionCoordinator {
        self.coordinator.clone()
    }
    fn coordinator_clocks(&self) -> Option<CoordinatorClocksState> {
        Some(self.clocks.clone())
    }
    fn idle_badge(&self, cwd: &str, at: DateTime<Utc>) {
        self.notices
            .lock()
            .unwrap()
            .push(Badge::Idle(cwd.into(), at));
    }
    fn auto_closed_badge(&self, cwd: &str, at: DateTime<Utc>) {
        self.notices
            .lock()
            .unwrap()
            .push(Badge::Closed(cwd.into(), at));
    }
}

#[tokio::test]
async fn recording_tick_deduplicates_and_advances_idle_badges() {
    let fixture = auto_close_fixture().await;
    let services = &fixture.services;
    services
        .settings
        .write()
        .await
        .coordinator_auto_close_enabled = false;
    let mut emitted = HashMap::new();
    tick(services, &mut emitted).await;
    tick(services, &mut emitted).await;
    let first = DateTime::from_timestamp(fixture.anchor.timestamp(), 0).unwrap();
    assert_eq!(
        *services.notices.lock().unwrap(),
        vec![Badge::Idle(fixture.cwd.clone(), first)]
    );
    let next = first + chrono::Duration::seconds(11);
    services
        .clocks
        .lock()
        .unwrap()
        .note_user_message("myproj:wg-1-team/lead", next);
    tick(services, &mut emitted).await;
    assert_eq!(
        *services.notices.lock().unwrap(),
        vec![
            Badge::Idle(fixture.cwd.clone(), first),
            Badge::Idle(fixture.cwd.clone(), next),
        ]
    );
    services.coordinator.close_and_join().await;
}

#[tokio::test]
async fn recording_tick_purges_removed_teams_and_reemits_same_anchor() {
    let mut fixture = auto_close_fixture().await;
    let services = &mut fixture.services;
    services
        .settings
        .write()
        .await
        .coordinator_auto_close_enabled = false;
    let mut emitted = HashMap::new();
    tick(services, &mut emitted).await;
    let removed = std::mem::replace(
        &mut services.manager,
        Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
    );
    tick(services, &mut emitted).await;
    assert!(emitted.is_empty());
    services.manager = removed;
    tick(services, &mut emitted).await;
    let anchor = DateTime::from_timestamp(fixture.anchor.timestamp(), 0).unwrap();
    assert_eq!(
        *services.notices.lock().unwrap(),
        vec![
            Badge::Idle(fixture.cwd.clone(), anchor),
            Badge::Idle(fixture.cwd.clone(), anchor),
        ]
    );
    services.coordinator.close_and_join().await;
}

#[tokio::test]
async fn recording_tick_closes_only_coordinator_badges_once() {
    use crate::pty::backend::PtyBackend;
    use crate::session::selection::{SelectionMode, SelectionSource};
    let fixture = auto_close_fixture().await;
    let services = &fixture.services;
    let mut emitted = HashMap::new();
    tick(services, &mut emitted).await;
    tick(services, &mut emitted).await;
    assert!(services
        .manager
        .read()
        .await
        .list_sessions()
        .await
        .is_empty());
    assert!(!fixture.backend.has_session(fixture.coordinator_id));
    assert!(!fixture.backend.has_session(fixture.member_id));
    let selection = services.manager.read().await.selection_payload().await;
    assert_eq!(selection.mode(), SelectionMode::None);
    assert_eq!(selection.source(), SelectionSource::AutoClose);
    let marker = services.clocks.lock().unwrap().snapshot()["myproj:wg-1-team/lead"]
        .auto_closed_at
        .unwrap();
    assert_eq!(
        *services.notices.lock().unwrap(),
        vec![
            Badge::Idle(
                fixture.cwd.clone(),
                DateTime::from_timestamp(fixture.anchor.timestamp(), 0).unwrap()
            ),
            Badge::Closed(fixture.cwd.clone(), marker),
        ]
    );
    services.coordinator.close_and_join().await;
}

#[tokio::test]
async fn closed_badges_ignore_existing_marker_sibling_and_missing_reference() {
    let fixture = auto_close_fixture().await;
    let services = &fixture.services;
    let team = "myproj:wg-1-team".to_string();
    let refs = HashMap::from([(
        team.clone(),
        ("myproj:wg-1-team/lead".into(), fixture.cwd.clone()),
    )]);
    let closed = HashSet::from([team.clone()]);
    mark_closed_badges(&services.clocks, &closed, &refs, |cwd, at| {
        services.auto_closed_badge(cwd, at);
    });
    let marker = services.clocks.lock().unwrap().snapshot()["myproj:wg-1-team/lead"]
        .auto_closed_at
        .unwrap();
    mark_closed_badges(&services.clocks, &closed, &refs, |cwd, at| {
        services.auto_closed_badge(cwd, at);
    });
    mark_closed_badges(&services.clocks, &closed, &HashMap::new(), |cwd, at| {
        services.auto_closed_badge(cwd, at);
    });
    let sibling_closed =
        destroyed_is_team_coordinator(fixture.member_id, Some(fixture.coordinator_id));
    assert!(!sibling_closed);
    let sibling_teams = if sibling_closed {
        closed
    } else {
        HashSet::new()
    };
    mark_closed_badges(&services.clocks, &sibling_teams, &refs, |cwd, at| {
        services.auto_closed_badge(cwd, at);
    });
    assert_eq!(
        *services.notices.lock().unwrap(),
        vec![Badge::Closed(fixture.cwd, marker)]
    );
    services.coordinator.close_and_join().await;
}

#[test]
fn tauri_badge_adapter_preserves_event_names_and_complete_payloads() {
    use tauri::Listener;
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    for name in [
        "coordinator_clock_updated",
        "coordinator_auto_close_changed",
    ] {
        let tx = tx.clone();
        app.listen_any(name, move |event| {
            tx.send((
                name,
                serde_json::from_str::<serde_json::Value>(event.payload()).unwrap(),
            ))
            .unwrap();
        });
    }
    app.handle().idle_badge("C:/team/lead", ts(123));
    app.handle().auto_closed_badge("C:/team/lead", ts(456));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (
            "coordinator_clock_updated",
            serde_json::json!({
                "replicaPath": "C:/team/lead", "lastUserMessageAt": ts(123).to_rfc3339(),
            })
        )
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (
            "coordinator_auto_close_changed",
            serde_json::json!({
                "replicaPath": "C:/team/lead", "autoClosedAt": ts(456).to_rfc3339(),
            })
        )
    );
    assert!(rx.try_recv().is_err());
}

fn assert_state_panic<T: Send + Sync + 'static>(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    getter: impl FnOnce(),
) {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use tauri::Manager;
    let expected = format!(
        "state() called before manage() for {}",
        std::any::type_name::<T>()
    );
    let real = catch_unwind(AssertUnwindSafe(|| {
        app.state::<T>();
    }));
    let adapted = catch_unwind(AssertUnwindSafe(getter));
    for result in [real, adapted] {
        let payload = result.expect_err("missing required state must panic");
        assert_eq!(
            *payload.downcast::<String>().expect("String panic payload"),
            expected
        );
    }
}

#[test]
fn tauri_state_adapter_preserves_required_panics_and_optional_clock_identity() {
    use tauri::Manager;
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let handle = app.handle();
    assert_state_panic::<SettingsState>(handle, || {
        handle.settings();
    });
    assert_state_panic::<Arc<tokio::sync::RwLock<SessionManager>>>(handle, || {
        handle.session_manager();
    });
    assert_state_panic::<Arc<Mutex<PtyManager>>>(handle, || {
        handle.pty_manager();
    });
    assert_state_panic::<Arc<IdleDetector>>(handle, || {
        handle.idle_detector();
    });
    assert_state_panic::<SelectionCoordinator>(handle, || {
        handle.selection_coordinator();
    });
    assert!(handle.coordinator_clocks().is_none());
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    app.manage(clocks.clone());
    assert!(Arc::ptr_eq(&clocks, &handle.coordinator_clocks().unwrap()));
}

// #2158 team activity clock: real `advance_team_activity`, injected time.
use crate::config::coordinator_clocks::CoordinatorClocks;
use crate::session::profile::IdleTuning;
const U0: i64 = 1_700_000_000;

/// Detector, base instant, team clocks and the "t1"/"t2" coordinator refs.
struct Fx(IdleDetector, Instant, CoordinatorClocksState);

impl Fx {
    fn new(anchors: &[(&str, i64)]) -> Self {
        let mut clocks = CoordinatorClocks::default();
        for (team, s) in anchors {
            clocks.note_activity(team, DateTime::from_timestamp(U0 + s, 0).unwrap());
        }
        let idle = Arc::into_inner(IdleDetector::new(|_| {}, |_| {})).unwrap();
        Fx(idle, Instant::now(), Arc::new(Mutex::new(clocks)))
    }
    fn at(&self, s: f64) -> Instant {
        self.1 + Duration::from_secs_f64(s)
    }
    /// Registered at the base; first non-empty chunk classified at +1 s.
    fn session(&self, tuning: IdleTuning) -> Uuid {
        let id = Uuid::new_v4();
        self.0.register_session_at(id, tuning, Some(self.1));
        self.out(id, 1.0);
        id
    }
    fn out(&self, id: Uuid, s: f64) {
        let at = self.at(s);
        self.0
            .record_output_classified_at(id, 2048, true, at, Some(at));
    }
    /// One tick with a coherent monotonic/UTC pair at `base + s`; returns both anchors.
    fn tick(&self, members: &[(Uuid, String)], s: f64) -> [Option<i64>; 2] {
        let utc = DateTime::from_timestamp_nanos(U0 * 1_000_000_000 + (s * 1e9) as i64);
        advance_team_activity(&self.2, members, &self.0, &refs(), self.at(s), utc);
        let g = self.2.lock().unwrap();
        ["t1", "t2"].map(|t| g.last_activity_at(t).map(|a| a.timestamp() - U0))
    }
}

fn refs() -> HashMap<String, (String, String)> {
    ["t1", "t2"]
        .map(|t| (t.into(), (t.into(), t.into())))
        .into()
}

#[test]
fn issue_2158_focus_resize_keeps_team_anchor() {
    for tuning in [IdleTuning::AGENT_DEFAULT, IdleTuning::DEFAULT] {
        let fx = Fx::new(&[("t1", 15)]);
        let id = fx.session(tuning);
        let members = vec![key(id, "t1")];
        fx.0.record_resize_at(id, Some(fx.at(30.0)));
        let before = fx.0.silence_age(id).unwrap();
        fx.out(id, 31.0);
        assert!(fx.0.silence_age(id).unwrap() <= before, "#2124 counts it");
        assert_eq!(fx.tick(&members, 40.0)[0], Some(15), "focus repaint");
        fx.out(id, 34.0);
        assert_eq!(fx.tick(&members, 41.0)[0], Some(34), "work after the grace");
    }
}

#[tokio::test]
async fn issue_2158_delayed_restore_keeps_team_anchors() {
    let fx = Fx::new(&[("t1", 3), ("t2", 7)]);
    let ids = [(); 3].map(|()| Uuid::new_v4());
    for (id, registered) in ids.into_iter().zip([5.0, 5.0, 6.0]) {
        fx.0.register_session_at(id, IdleTuning::AGENT_DEFAULT, Some(fx.at(registered)));
        let _ = [20.0, 22.0].map(|s| fx.out(id, s));
    }
    let members = vec![key(ids[0], "t1"), key(ids[1], "t1"), key(ids[2], "t2")];
    assert_eq!(fx.tick(&members, 35.0), [Some(3), Some(7)]);
    // Injected time: t1 is established at exactly 30 s, not 1 ns before; t2 (29 s) is not.
    let (at, ns) = (fx.at(35.0), Duration::from_nanos(1));
    assert!(established_teams_at(&members, &fx.0, at - ns).is_empty());
    let t1 = HashSet::from(["t1".to_string()]);
    assert_eq!(established_teams_at(&members, &fx.0, at), t1);
    // Stale anchor: restore and resize-repaint emissions veto the close, a silent control closes.
    let (old, now) = (Duration::from_secs(300), Instant::now());
    let [repaint, control] = [(); 2].map(|()| Uuid::new_v4());
    let _ = [repaint, control].map(|id| fx.0.set_auto_close_ages_for_test(id, old, old));
    fx.0.record_resize_at(repaint, Some(now));
    let quiet = |id| fx.0.silence_age(id).unwrap() >= REPAINT_GRACE;
    assert!(quiet(repaint));
    fx.0.record_output_classified_at(repaint, 2048, true, now, Some(now));
    assert!(!quiet(repaint) && fx.0.team_silence_age_at(repaint, now).is_none());
    let to_close = vec![ids[0], repaint, control];
    let team: HashMap<_, _> = to_close.iter().map(|id| (*id, "t1".to_string())).collect();
    let policy = auto_close_policy(Some((true, 1, false)), U0 + 200).unwrap();
    let (mgr, refs) = (tokio::sync::RwLock::new(SessionManager::new()), refs());
    let confirm = confirm_closeable_members(to_close, &fx.2, &team, &refs, &fx.0, &mgr, policy);
    assert_eq!(confirm.await, vec![control]);
}

#[test]
fn issue_2158_work_keeps_advancing_team_anchor() {
    let fx = Fx::new(&[("t2", 5)]);
    let [worker, quiet, other] = [(); 3].map(|()| fx.session(IdleTuning::AGENT_DEFAULT));
    let _ = [20.0, 40.0, 60.0].map(|s| fx.out(worker, s));
    let members = vec![key(worker, "t1"), key(quiet, "t1"), key(other, "t2")];
    assert_eq!(fx.tick(&members, 61.0), [Some(60), Some(5)]);
}

#[test]
fn issue_2158_suppressed_repaint_keeps_anchor_across_ticks() {
    let fx = Fx::new(&[]);
    let id = fx.session(IdleTuning::DEFAULT);
    let members = vec![key(id, "t1")];
    fx.out(id, 90.5);
    assert_eq!(fx.tick(&members, 100.75)[0], Some(90));
    fx.0.record_resize_at(id, Some(fx.at(120.0)));
    fx.out(id, 121.0);
    assert_eq!(fx.tick(&members, 160.25)[0], Some(90), "rounded once");
    fx.out(id, 150.0);
    assert_eq!(fx.tick(&members, 170.0)[0], Some(150));
}

#[test]
fn issue_2158_tick_protects_silent_coordinator() {
    for seeded in [true, false] {
        let fx = Fx::new(&[]);
        let ids @ [coord, member, worker] = [(); 3].map(|()| Uuid::new_v4());
        let (age, s) = (Duration::from_secs(300), Duration::from_secs);
        let _ = ids.map(|id| fx.0.set_auto_close_ages_for_test(id, age, age));
        let (now, utc) = (Instant::now(), Utc::now());
        let stamps = [now - s(20), now + Duration::from_millis(1)];
        for stamp in stamps.into_iter().filter(|_| seeded) {
            fx.0.record_output_classified_at(worker, 2048, true, stamp, Some(stamp));
        }
        let members = vec![key(coord, "t1"), key(member, "t1"), key(worker, "t1")];
        let stale = utc - chrono::Duration::seconds(600);
        fx.2.lock().unwrap().note_user_message("t1", stale);
        advance_team_activity(&fx.2, &members, &fx.0, &refs(), now, utc);
        let snap = fx.2.lock().unwrap().snapshot();
        let policy = auto_close_policy(Some((true, 1, false)), utc.timestamp()).unwrap();
        let teams = established_teams(&members, &fx.0);
        let none = HashSet::new();
        let closing = closeable_members(&members, &refs(), &snap, &fx.0, &teams, &none, policy);
        let anchor = snap["t1"].last_activity_at.map(|t| t.timestamp());
        assert_eq!(anchor, seeded.then_some(utc.timestamp()));
        assert_eq!(closing.is_empty(), seeded);
        assert!(seeded || closing.contains(&coord) && closing.contains(&member));
    }
}
