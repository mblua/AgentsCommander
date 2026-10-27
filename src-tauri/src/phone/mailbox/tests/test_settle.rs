use super::*;

#[test]
fn gate_accepts_peer_after_resize_settles() {
    let quiet = Duration::from_millis(3000);
    // last_resize_age = 6.1s >= resize_grace(3s) + effective_quiet(3s) = 6s.
    let sid = Uuid::new_v4();
    let peer = make_gate_peer(
        "proj:wg-1/devs/alice",
        vec![PurgeGateSession {
            session_id: sid,
            readiness: make_readiness(sid, Some(3000), true, Some(6100)),
            mirror_idle: true,
        }],
        vec!["aaa".to_string()],
    );
    let decision = evaluate_gate(&[peer], quiet);
    assert!(
        decision.passed,
        "a peer past resize settlement with all legs agreeing must pass"
    );
}

#[test]
fn no_peer_is_closed_on_a_non_purged_status() {
    // The evaluate_gate function never produces outcome "closed"; that
    // outcome is assigned by the destroy loop, not the gate. Verify.
    let quiet = Duration::from_millis(3000);
    let sid = Uuid::new_v4();
    let busy_peer = make_gate_peer(
        "proj:wg-1/devs/alice",
        vec![PurgeGateSession {
            session_id: sid,
            readiness: make_readiness(sid, Some(100), true, None),
            mirror_idle: true,
        }],
        vec!["aaa".to_string()],
    );
    let decision = evaluate_gate(&[busy_peer], quiet);
    assert!(!decision.passed);
    assert!(
        !decision.peers.iter().any(|p| p.outcome == "closed"),
        "evaluate_gate must never produce outcome 'closed'"
    );
}

// ── (#885 D-3) handle_purge_wg end-to-end tests ──

/// Build a purge-wg outbox message file in the sender's outbox.
#[allow(clippy::too_many_arguments)]
fn build_purge_wg_message(
    sender_cwd: &Path,
    msg_id: &str,
    request_id: &str,
    from: &str,
    token: Option<&str>,
    dry_run: bool,
    quiet_period_ms: u64,
    wg_assertion: Option<&str>,
) -> PathBuf {
    let outbox_dir = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let message_path = outbox_dir.join(format!("{}.json", msg_id));
    let msg = OutboxMessage {
        id: msg_id.into(),
        token: token.map(String::from),
        from: from.into(),
        to: String::new(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.into()),
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-07-09T00:00:00Z".into(),
        command: None,
        action: Some(PURGE_WG_ACTION.into()),
        target: wg_assertion.map(String::from),
        force: Some(true),
        timeout_secs: Some(5),
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: Some(dry_run),
        quiet_period_ms: Some(quiet_period_ms),
        pty_input: None,
    };
    std::fs::write(&message_path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
    message_path
}

/// (#885 E-3) Helper: set up a full WG fixture with a coordinator and a
/// peer. Returns the sender CWD and peer CWD.
fn setup_purge_fixture(temp: &tempfile::TempDir, wg_suffix: &str) -> (PathBuf, PathBuf) {
    let project = temp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let team_name = format!("_team_{}", wg_suffix);
    let team_dir = ac_root.join(&team_name);
    let origin_tl = ac_root.join("_agent_tech-lead");
    let wg_dir = ac_root.join(format!("wg-1-{}", wg_suffix));
    let sender_cwd = wg_dir.join("__agent_tech-lead");
    let peer_cwd = wg_dir.join("__agent_dev-rust");
    for d in [&team_dir, &origin_tl, &sender_cwd, &peer_cwd] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
    )
    .unwrap();
    std::fs::write(
        sender_cwd.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .unwrap();
    (sender_cwd, peer_cwd)
}

/// (E-3) Seed a live, busy peer session so the gate is actually exercised.
/// Returns the session id and the mock backend handle (to set_live).
async fn seed_live_busy_peer(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    peer_cwd: &Path,
) -> Uuid {
    let session_id = add_mailbox_session(
        app,
        peer_cwd,
        "wg-1-dev-team/dev-rust",
        SessionStatus::Running,
        None,
    )
    .await;
    // Register the session as live in the mock PTY backend.
    let pty_mgr = app.state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>();
    let mgr = pty_mgr.lock().unwrap();
    mgr.record_route(
        session_id,
        crate::pty::backend::SessionBackendKind::LocalProcess,
    );
    let backend = mgr.backend_for_kind(crate::pty::backend::SessionBackendKind::LocalProcess);
    let mock = backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .expect("backend must be MailboxMockPtyBackend");
    mock.set_live(session_id);
    session_id
}

/// (D-3/F-7) A purge-wg message with no session token must be rejected,
/// even if `is_master` would be true.
#[tokio::test]
async fn process_message_purge_wg_without_token_is_rejected() {
    let temp = tempfile::TempDir::new().unwrap();
    let (sender_cwd, _peer_cwd) = setup_purge_fixture(&temp, "dev-team");

    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let hooks = MailboxTestHooks::default();
    let hooks_clone = hooks.clone();

    // Master token but NO session token: is_master=true, saw_session_token=false.
    let path = build_purge_wg_message(
        &sender_cwd,
        "msg-purge-notoken",
        "rid-purge-notoken",
        "proj-a:wg-1-dev-team/tech-lead",
        Some(MAILBOX_MASTER_TOKEN),
        false,
        3000,
        None,
    );
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let result = poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await;
    assert!(
        result.is_ok(),
        "process_message should not error on rejection"
    );

    // (#885 E-1) Assert on the REASON, not just file existence. A rejection
    // test that does not pin which guard rejected passes when the guard is
    // deleted.
    let rejected = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-purge-notoken.reason.txt");
    assert!(rejected.exists(), "tokenless purge must be rejected");
    let reason = std::fs::read_to_string(&rejected).unwrap();
    assert!(
        reason.contains("requires a session token"),
        "must be rejected by the F-7 token guard, not by WG resolution: {reason}"
    );

    // (#885 E-2) Zero destroy events (now meaningful with the hook).
    assert_no_spawn_or_destroy_events(&hooks_clone);
}

/// (D-3/F-7) A master-token message with a forged `from` naming another
/// WG's coordinator must be rejected. The forged WG is fully constructed
/// on disk so `verified_wg_coordinator_target` SUCCEEDS; the F-7 token
/// guard is the only thing between a master token and a cross-WG purge.
#[tokio::test]
async fn process_message_purge_wg_with_master_token_and_forged_from_is_rejected() {
    let temp = tempfile::TempDir::new().unwrap();
    let (sender_cwd, _peer_cwd) = setup_purge_fixture(&temp, "dev-team");

    // (#885 E-1) Construct a SECOND, fully valid workgroup so
    // `verified_wg_coordinator_target` succeeds for the forged `from`.
    // Without this, the forged WG doesn't exist and the message is
    // rejected by WG resolution, not by the F-7 guard.
    let project = temp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let other_team_dir = ac_root.join("_team_other-team");
    let other_wg_dir = ac_root.join("wg-1-other-team");
    let other_sender_cwd = other_wg_dir.join("__agent_tech-lead");
    let other_peer_cwd = other_wg_dir.join("__agent_dev-rust");
    for d in [&other_team_dir, &other_sender_cwd, &other_peer_cwd] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        other_team_dir.join("config.json"),
        r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
    )
    .unwrap();
    std::fs::write(
        other_sender_cwd.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .unwrap();

    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let hooks = MailboxTestHooks::default();
    let hooks_clone = hooks.clone();

    // Master token, `from` names the OTHER workgroup's coordinator.
    // `verified_wg_coordinator_target` will SUCCEED (the WG exists on disk).
    // The F-7 guard (`!saw_session_token`) is the only thing that rejects.
    //
    // Process this as app-outbox traffic so the repo-outbox owner check
    // does not reject first. That mirrors the master-token surface: with
    // the old `is_master || saw_session_token` guard, an app-outbox
    // message could pick any verified WG coordinator as `from`.
    let path = build_purge_wg_message(
        &sender_cwd,
        "msg-purge-forged",
        "rid-purge-forged",
        "proj-a:wg-1-other-team/tech-lead", // forged: a different WG
        Some(MAILBOX_MASTER_TOKEN),
        false,
        3000,
        None,
    );
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let result = poller
        .process_message(&app, &path, OutboxOrigin::Master)
        .await;
    assert!(result.is_ok());

    let rejected = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-purge-forged.reason.txt");
    assert!(
        rejected.exists(),
        "master-token purge with forged from must be rejected"
    );
    // (#885 E-1) Pin WHICH guard rejected.
    let reason = std::fs::read_to_string(&rejected).unwrap();
    assert!(
        reason.contains("requires a session token"),
        "must be rejected by the F-7 token guard, not by WG resolution: {reason}"
    );

    // (#885 E-2) Zero destroy events.
    assert_no_spawn_or_destroy_events(&hooks_clone);
}

/// (D-3) Guard B: a candidate with `is_root_agent: true` must abort the
/// purge with `failed_root_guard` and destroy nothing.
#[tokio::test]
async fn root_guard_aborts_purge() {
    let temp = tempfile::TempDir::new().unwrap();
    let (sender_cwd, peer_cwd) = setup_purge_fixture(&temp, "dev-team");

    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let hooks = MailboxTestHooks::default();
    let hooks_clone = hooks.clone();

    // Seed a session at the peer CWD that looks like a root agent record.
    let session_id = add_mailbox_session(
        &app,
        &peer_cwd,
        "wg-1-dev-team/dev-rust",
        SessionStatus::Idle,
        None,
    )
    .await;
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_is_root_agent(session_id, true).await;
    }

    // We need a valid session token for the sender. Seed a session at the
    // sender CWD to get a token.
    let sender_session_id = add_mailbox_session(
        &app,
        &sender_cwd,
        "wg-1-dev-team/tech-lead",
        SessionStatus::Idle,
        None,
    )
    .await;
    let token = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.get_session(sender_session_id)
            .await
            .expect("sender session must exist")
            .token
            .to_string()
    };

    let path = build_purge_wg_message(
        &sender_cwd,
        "msg-purge-rootguard",
        "rid-purge-rootguard",
        "proj-a:wg-1-dev-team/tech-lead",
        Some(&token),
        false,
        3000,
        None,
    );
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let _ = poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await;

    // (#885 E-6) Assert, don't hedge.
    let responses_dir = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("responses");
    let response_path = responses_dir.join("rid-purge-rootguard.json");
    assert!(
        response_path.exists(),
        "root guard response must be written"
    );
    let body = std::fs::read_to_string(&response_path).unwrap();
    assert!(
        body.contains("failed_root_guard"),
        "response must contain 'failed_root_guard': {}",
        body
    );

    // (#885 E-2) Zero destroy events: the root guard aborts before the
    // destroy loop.
    assert_no_spawn_or_destroy_events(&hooks_clone);
}

/// (D-3) A hand-crafted `quietPeriodMs: 0` must be clamped to >= 2500
/// daemon-side. The clamp is inside `handle_purge_wg` and unreachable
/// from a unit test on `evaluate_gate` alone, so this exercises the
/// full `process_message` path. (E-3) Seeds a live busy peer so the
/// gate is actually exercised, not vacuously passed.
#[tokio::test]
async fn quiet_period_is_clamped_to_floor() {
    let temp = tempfile::TempDir::new().unwrap();
    let (sender_cwd, peer_cwd) = setup_purge_fixture(&temp, "dev-team");

    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);

    // (#885 E-3) Seed a live, busy peer so the gate is exercised, not
    // vacuously passed with an empty pty_live set.
    let peer_sid = seed_live_busy_peer(&app, &peer_cwd).await;

    // Seed a sender session for the token.
    let sender_session_id = add_mailbox_session(
        &app,
        &sender_cwd,
        "wg-1-dev-team/tech-lead",
        SessionStatus::Idle,
        None,
    )
    .await;
    let token = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.get_session(sender_session_id)
            .await
            .expect("sender session must exist")
            .token
            .to_string()
    };

    // Dry-run with quiet_period_ms=0. The clamp must raise it to >= 2500.
    let path = build_purge_wg_message(
        &sender_cwd,
        "msg-purge-clamp",
        "rid-purge-clamp",
        "proj-a:wg-1-dev-team/tech-lead",
        Some(&token),
        true, // dry-run
        0,    // below floor
        None,
    );
    let poller = MailboxPoller::default();
    let _ = poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await;

    let responses_dir = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("responses");
    let response_path = responses_dir.join("rid-purge-clamp.json");
    assert!(response_path.exists(), "dry-run response must be written");
    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&response_path).unwrap()).unwrap();
    let qp = body
        .get("quiet_period_ms")
        .and_then(|v| v.as_u64())
        .expect("response must contain quiet_period_ms");
    assert!(
        qp >= 2500,
        "quiet_period_ms must be clamped to >= 2500, got {}",
        qp
    );
    assert_eq!(
        body.get("status").and_then(|v| v.as_str()),
        Some("dry_run_blocked"),
        "live busy peer must make the dry-run gate reject: {}",
        body
    );
    assert_eq!(
        body.get("would_purge").and_then(|v| v.as_bool()),
        Some(false),
        "dry-run must report that purge would not proceed: {}",
        body
    );
    let peer_sid_str = peer_sid.to_string();
    let peers = body
        .get("peers")
        .and_then(|v| v.as_array())
        .expect("response must contain peers");
    let peer = peers
        .iter()
        .find(|p| {
            p.get("session_ids")
                .and_then(|v| v.as_array())
                .is_some_and(|ids| {
                    ids.iter()
                        .any(|id| id.as_str() == Some(peer_sid_str.as_str()))
                })
        })
        .expect("seeded live peer session must appear in purge gate response");
    assert_eq!(
        peer.get("purgeable").and_then(|v| v.as_bool()),
        Some(false),
        "seeded live busy peer must be non-purgeable: {}",
        peer
    );
}

#[tokio::test]
async fn test_wake_settle_defers_and_does_not_burn_attempts() {
    let temp = tempfile::TempDir::new().unwrap();
    let msg_path = temp.path().join("test_msg.json");
    std::fs::write(&msg_path, "{}").unwrap();

    let mut poller = MailboxPoller::default();
    let deferred_err = format!(
        "{}: session 123 is blocked by interactive menu",
        crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED
    );

    // Record outcome with deferred error
    poller
        .record_message_outcome(msg_path.clone(), Err(deferred_err.clone()))
        .await;

    // Attempt tracker must NOT have recorded an attempt or burned retry count
    assert!(!poller.retry_tracker.contains_key(&msg_path));
    assert!(msg_path.exists());
    assert!(!temp.path().join("rejected").exists());
}

/// #2336 - a typing-held wake gets the menu guard's exemption: the filesystem
/// poller never counts an attempt against it and never rejects it, even past
/// the attempt ceiling. The file stays at its original path for the next poll.
#[tokio::test]
async fn test_typing_hold_defers_without_burning_attempts() {
    let temp = tempfile::TempDir::new().unwrap();
    let msg_path = temp.path().join("test_msg.json");
    std::fs::write(&msg_path, "{}").unwrap();

    let mut poller = MailboxPoller::default();
    let deferred_err = format!(
        "{}: session 123 is holding peer wake injection while the user is typing",
        crate::pty::menu_guard::ERR_TYPING_HOLD_DEFERRED
    );

    for _ in 0..(MAX_DELIVERY_ATTEMPTS + 5) {
        poller
            .record_message_outcome(msg_path.clone(), Err(deferred_err.clone()))
            .await;
    }

    assert!(!poller.retry_tracker.contains_key(&msg_path));
    assert!(msg_path.exists());
    assert!(!temp.path().join("rejected").exists());
}

#[test]
fn typing_hold_marker_classifies_as_deferred() {
    let err = format!(
        "{}: session 123 is holding peer wake injection while the user is typing",
        crate::pty::menu_guard::ERR_TYPING_HOLD_DEFERRED
    );
    assert_eq!(
        classify_injection_error(&err),
        (log::Level::Debug, "deferred")
    );
}

#[tokio::test]
async fn test_settle_live_holds_during_menu_guard_block() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let sid = add_mailbox_session(
        &app,
        temp.path(),
        "wg-1-dev-team/dev",
        SessionStatus::Idle,
        None,
    )
    .await;

    let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::new());
    let entries = vec![crate::config::settings::BlockingMenuEntry::Valid(
        crate::config::settings::BlockingMenuConfig {
            pattern: "trust".to_string(),
            notification: "dialog".to_string(),
            enabled: true,
            captured_against: None,
        },
    )];
    menu_guard.evaluate_logical_rows(
        sid,
        &[crate::pty::watchers::frame::LogicalRow {
            text: "Do you trust the author?".to_string(),
            start: 0,
            end: 0,
        }],
        &entries,
    );
    assert!(menu_guard.is_blocked(sid));

    let app_with_guard = tauri::test::mock_builder()
        .manage(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                .inner()
                .clone(),
        )
        .manage(menu_guard)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();

    let poller = MailboxPoller::default();
    let res = poller
        .settle_until_ready(
            app_with_guard.handle(),
            sid,
            Duration::from_millis(50),
            Duration::from_millis(10),
            Duration::from_millis(10),
            None,
        )
        .await;

    let err = res.unwrap_err();
    assert!(crate::pty::menu_guard::is_menu_guard_deferred_error(&err));
}

/// #2586 T10 - an idle session settles `Ready` well before the cap. The
/// negative controls redden a constant `TimedOut` mapping and a hang.
#[tokio::test]
async fn test_settle_until_ready_reports_ready_before_the_cap() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let sid = add_mailbox_session(
        &app,
        temp.path(),
        "wg-1-dev-team/dev",
        SessionStatus::Idle,
        None,
    )
    .await;
    let max_wait = Duration::from_millis(500);
    let started = std::time::Instant::now();

    let res = MailboxPoller::default()
        .settle_until_ready(
            &app,
            sid,
            max_wait,
            Duration::from_millis(20),
            Duration::from_millis(10),
            None,
        )
        .await;

    assert!(started.elapsed() < max_wait, "{:?}", started.elapsed());
    assert_ne!(res, Ok(SettleReadiness::TimedOut));
    assert_eq!(res, Ok(SettleReadiness::Ready));
}

/// #2586 T11 - a never-ready session hits the cap: still `Ok` (bias to
/// deliver) and `TimedOut`. T10 and T11 are each other's control.
#[tokio::test]
async fn test_settle_until_ready_reports_timed_out_at_the_cap() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let sid = add_mailbox_session(
        &app,
        temp.path(),
        "wg-1-dev-team/dev",
        SessionStatus::Running,
        None,
    )
    .await;

    let res = MailboxPoller::default()
        .settle_until_ready(
            &app,
            sid,
            Duration::from_millis(80),
            Duration::from_millis(20),
            Duration::from_millis(10),
            None,
        )
        .await;

    assert!(res.is_ok(), "{res:?}");
    assert_ne!(res, Ok(SettleReadiness::Ready));
    assert_eq!(res, Ok(SettleReadiness::TimedOut));
}

/// #2586 T12 - the readiness survives the real cold-spawn caller. No test
/// hooks, so the production 2000 ms settle runs (about 2.1 s wall).
#[tokio::test]
async fn test_wait_for_spawned_wake_idle_propagates_ready() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let sid = add_mailbox_session(
        &app,
        temp.path(),
        "wg-1-dev-team/dev",
        SessionStatus::Idle,
        None,
    )
    .await;
    let poller = MailboxPoller::default();
    assert!(poller.test_hooks.is_none());

    let res = poller.wait_for_spawned_wake_idle(&app, sid).await;

    assert_eq!(res, Ok(SettleReadiness::Ready));
}

/// Single source of the TTL-bound fixture pair: `expires_at` is derived from the same
/// instant as `issued_at`, so the canonical millisecond truncation can never split them.
fn pty_ttl_fixture_timestamps(now: chrono::DateTime<chrono::Utc>) -> (String, String) {
    (
        crate::phone::types::canonical_pty_timestamp(now),
        crate::phone::types::canonical_pty_timestamp(
            now + chrono::Duration::seconds(crate::phone::types::PTY_INPUT_TTL_SECS),
        ),
    )
}

#[test]
fn test_pty_ttl_fixture_single_instant_survives_millisecond_boundary() {
    // Adversarial base: 900 microseconds past a millisecond boundary, so a second clock
    // sample taken 100 microseconds later truncates to the next canonical millisecond.
    let base = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00.000900Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    let (issued_at, expires_at) = pty_ttl_fixture_timestamps(base);
    assert_eq!(issued_at, "2026-01-01T00:00:00.000Z");
    assert_eq!(expires_at, "2026-01-01T00:10:00.000Z");

    let issued = crate::phone::types::parse_canonical_pty_timestamp(&issued_at).unwrap();
    let expires = crate::phone::types::parse_canonical_pty_timestamp(&expires_at).unwrap();
    assert_eq!(
        expires - issued,
        chrono::Duration::milliseconds(600_000),
        "the TTL fixture helper must yield exactly the span enqueue_pty_input accepts"
    );
    assert_eq!(
        expires - issued,
        chrono::Duration::seconds(crate::phone::types::PTY_INPUT_TTL_SECS)
    );

    // The rejected two-sample form, pinned at the same base: a 100-microsecond-later
    // second sample lands one canonical millisecond ahead, i.e. 600001 ms.
    let second_sample = base + chrono::Duration::microseconds(100);
    let two_sample_expires = crate::phone::types::canonical_pty_timestamp(
        second_sample + chrono::Duration::seconds(crate::phone::types::PTY_INPUT_TTL_SECS),
    );
    assert_eq!(two_sample_expires, "2026-01-01T00:10:00.001Z");
    assert_ne!(
        two_sample_expires, expires_at,
        "the fixture must not be rebuilt from a second clock sample"
    );
    let two_sample =
        crate::phone::types::parse_canonical_pty_timestamp(&two_sample_expires).unwrap();
    assert_eq!(two_sample - issued, chrono::Duration::milliseconds(600_001));
}

#[tokio::test]
async fn test_pty_input_operation_retried_on_menu_guard() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("messages.db");
    let store = crate::api::message_store::MessageStore::open(path).unwrap();

    let op_id = Uuid::new_v4().to_string();
    let (issued_at, expires_at) = pty_ttl_fixture_timestamps(chrono::Utc::now());
    store
        .enqueue_pty_input(crate::api::message_store::PtyInputEnqueueRequest {
            injection_id: op_id.clone(),
            sender_fqn: "proj:wg-1-team/lead".into(),
            target_fqn: "proj:wg-1-team/dev".into(),
            op_id: op_id.clone(),
            nonce_sha256: "a".repeat(64),
            request_fingerprint: "b".repeat(64),
            confirmation_tag: None,
            requested_agent_id: None,
            payload: b"hello".to_vec(),
            source_plane: crate::phone::types::PtyInputSourcePlane::ContainerApi,
            sender_incarnation_fingerprint: "c".repeat(64),
            sender_identity_fingerprint: "d".repeat(64),
            target_identity_fingerprint: "e".repeat(64),
            authority_session_id: Uuid::new_v4().to_string(),
            authority_client_id: Some("client".into()),
            authority_client_generation: Some(Uuid::new_v4().to_string()),
            issued_at,
            expires_at,
        })
        .unwrap();

    store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&op_id),
            "lease-test",
            chrono::Utc::now(),
        )
        .unwrap();

    let mut heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
        Arc::new(store.clone()),
        op_id.clone(),
        "lease-test".to_string(),
        chrono::Utc::now() + chrono::Duration::seconds(30),
    );

    assert!(
        finish_pty_input_before_boundary(
            &store,
            &mut heartbeat,
            &op_id,
            "lease-test",
            crate::phone::types::PtyInputReasonCode::MenuGuardBlocked,
        )
        .await,
        "actual conditional retry certifies source handoff"
    );

    let op = store
        .query_pty_input_by_injection(&op_id)
        .unwrap()
        .expect("operation exists");
    assert_eq!(op.status, crate::phone::types::PtyInputPublicStatus::Queued);
    assert_eq!(
        op.reason.map(|r| r.code),
        Some(crate::phone::types::PtyInputReasonCode::MenuGuardBlocked)
    );
}
