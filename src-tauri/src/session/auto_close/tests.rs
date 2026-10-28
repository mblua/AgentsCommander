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

// ---- member_post_repaint_silence (the E1 repaint-exclusion fix) ----

#[test]
fn post_repaint_none_when_untracked() {
    let g = Duration::from_secs(10);
    assert_eq!(
        member_post_repaint_silence(None, Some(Duration::from_secs(5)), g),
        None,
        "missing alive_age (spawn race) -> excluded"
    );
    assert_eq!(
        member_post_repaint_silence(Some(Duration::from_secs(60)), None, g),
        None,
        "missing silence_age (spawn race) -> excluded"
    );
}

#[test]
fn post_repaint_none_when_inside_wake_window() {
    let g = Duration::from_secs(10);
    // alive < grace -> checked_sub None -> excluded.
    assert_eq!(
        member_post_repaint_silence(
            Some(Duration::from_secs(5)),
            Some(Duration::from_secs(0)),
            g
        ),
        None
    );
    // alive == grace -> window 0 -> silence(0) < 0 is false -> excluded.
    assert_eq!(
        member_post_repaint_silence(
            Some(Duration::from_secs(10)),
            Some(Duration::from_secs(0)),
            g
        ),
        None
    );
}

#[test]
fn post_repaint_none_when_last_output_was_repaint_replay() {
    // Case A (abandoned): alive 60, repaint burst ended at t=2 -> silence 58.
    // window = 50; 58 >= 50 -> excluded, the persisted anchor stays frozen.
    let g = Duration::from_secs(10);
    assert_eq!(
        member_post_repaint_silence(
            Some(Duration::from_secs(60)),
            Some(Duration::from_secs(58)),
            g
        ),
        None,
        "scrollback-only output must NOT advance the anchor"
    );
}

#[test]
fn post_repaint_some_when_genuine_post_repaint_output() {
    // Case B (genuine resume): alive 60, real work at ~t=25 -> silence 35.
    // window = 50; 35 < 50 -> contributes Some(35).
    let g = Duration::from_secs(10);
    assert_eq!(
        member_post_repaint_silence(
            Some(Duration::from_secs(60)),
            Some(Duration::from_secs(35)),
            g
        ),
        Some(Duration::from_secs(35)),
        "genuine post-repaint work must advance the anchor"
    );
}

#[test]
fn post_repaint_boundary_is_excluded() {
    // silence == alive - grace -> NOT strictly less -> excluded (strict <).
    let g = Duration::from_secs(10);
    assert_eq!(
        member_post_repaint_silence(
            Some(Duration::from_secs(60)),
            Some(Duration::from_secs(50)),
            g
        ),
        None,
        "boundary silence == alive - grace is excluded"
    );
}

// ---- team_min_silence (folds the per-member post-repaint filter) ----

#[test]
fn team_min_silence_takes_min_over_contributing_members() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
    let out = team_min_silence(&members, &|id| {
        if id == a {
            Some(Duration::from_secs(30))
        } else {
            Some(Duration::from_secs(10))
        }
    });
    assert_eq!(out.get("proj:wg-1"), Some(&Duration::from_secs(10)));
}

#[test]
fn team_min_silence_absent_when_no_member_contributes() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
    // All members repaint-only / sub-grace / untracked -> None for each.
    let out = team_min_silence(&members, &|_| None);
    assert!(
        !out.contains_key("proj:wg-1"),
        "a team with no contributing member must be ABSENT (anchor frozen)"
    );
    assert!(out.is_empty());
}

#[test]
fn team_min_silence_isolates_teams() {
    let a = Uuid::new_v4(); // team 1, contributes 40
    let b = Uuid::new_v4(); // team 2, contributes 5
    let c = Uuid::new_v4(); // team 2, untracked -> excluded
    let members = vec![
        key(a, "proj:wg-1"),
        key(b, "proj:wg-2"),
        key(c, "proj:wg-2"),
    ];
    let out = team_min_silence(&members, &|id| {
        if id == a {
            Some(Duration::from_secs(40))
        } else if id == b {
            Some(Duration::from_secs(5))
        } else {
            None
        }
    });
    assert_eq!(out.get("proj:wg-1"), Some(&Duration::from_secs(40)));
    assert_eq!(out.get("proj:wg-2"), Some(&Duration::from_secs(5)));
}

// ---- team_idle_since_secs (unified max(user, activity) anchor) ----

#[test]
fn idle_since_secs_picks_max_present_component() {
    assert_eq!(
        team_idle_since_secs(Some(ts(100)), None),
        ts(100).timestamp()
    );
    assert_eq!(
        team_idle_since_secs(None, Some(ts(200))),
        ts(200).timestamp()
    );
    assert_eq!(
        team_idle_since_secs(Some(ts(100)), Some(ts(200))),
        ts(200).timestamp(),
        "max of the two present components"
    );
    assert_eq!(
        team_idle_since_secs(None, None),
        i64::MIN,
        "both absent -> MIN (no badge)"
    );
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

#[tokio::test]
async fn selected_idle_team_closes_as_one_batch_and_clears_selection_once() {
    use crate::config::coordinator_clocks::CoordinatorClocks;
    use crate::config::settings::{AppSettings, SettingsState};
    use crate::pty::backend::{PtyBackend, SessionBackendKind};
    use crate::resource_monitor::ResourceMonitorState;
    use crate::session::selection::{SelectionCoordinator, SelectionMode, SelectionSource};
    use crate::telegram::manager::{TelegramBridgeManager, TelegramBridgeState};
    use tauri::Listener;

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
    let coordinator_session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            coordinator_cwd.to_string_lossy().into_owned(),
            None,
            None,
            Vec::new(),
            true,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let member_session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            member_cwd.to_string_lossy().into_owned(),
            Some("codex".to_string()),
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();

    let backend = Arc::new(AutoCloseBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    for id in [coordinator_session.id, member_session.id] {
        backend.set_live(id);
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
    }
    let idle = IdleDetector::new(|_| {}, |_| {});
    for id in [coordinator_session.id, member_session.id] {
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
    clocks.lock().unwrap().note_user_message(
        "myproj:wg-1-team/lead",
        Utc::now() - chrono::Duration::seconds(120),
    );
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), shutdown.token().clone());
    let output_senders = Arc::new(Mutex::new(HashMap::new()));
    let telegram: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(output_senders),
    ));
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

    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for event_name in ["session_destroyed", "session_switched"] {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |event| {
            let _ = events_tx.send((event_name, event.payload().to_string()));
        });
    }

    tick(app.handle(), &mut HashMap::new()).await;

    assert!(manager.read().await.list_sessions().await.is_empty());
    assert!(!backend.has_session(coordinator_session.id));
    assert!(!backend.has_session(member_session.id));
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
    coordinator.close_and_join().await;
}
