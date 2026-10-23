use super::*;

/// #1682 - the smallest app the stamp path resolves state from: the
/// `SessionManager` for `get_last_agent_message`, and the substantive-input
/// tracker for gate 2. Deliberately manages no `WsBroadcaster`, so
/// `broadcast_all_r` reduces to `tauri::Emitter::emit` and a `listen_any`
/// listener sees the event. `session_test_app_with_store` is not extended:
/// it is shared by more than thirty tests and manages no tracker.
fn agent_stamp_test_app(
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(session_mgr)
        .manage(crate::pty::input_activity::new_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build agent stamp test app")
}

/// #1682 - a local channel over the stamp event. Not `capture_session_lifecycle`,
/// which is shared and scoped to four other event names.
fn capture_agent_message_events(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> std::sync::mpsc::Receiver<String> {
    use tauri::Listener;

    let (sender, receiver) = std::sync::mpsc::channel();
    app.listen_any("session_agent_message", move |event| {
        let _ = sender.send(event.payload().to_string());
    });
    receiver
}

/// #1682 - the per-instance file the stamp is written into: since #2816
/// (C3) the state file, never the tracked `config.json` beside it.
fn stamp_config_path(cwd: &std::path::Path) -> PathBuf {
    cwd.join(crate::config::agent_local_dir_name())
        .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

/// #2816 (C3) - a suppressed write leaves neither the state file nor the
/// tracked `config.json` it used to create.
fn no_stamp_file(cwd: &std::path::Path) -> bool {
    let state = stamp_config_path(cwd);
    !state.exists() && !state.with_file_name("config.json").exists()
}

/// #1682 - `tooling.lastAgentMessageAt` read straight off the file, so the
/// assertion does not go through the same reader the command uses.
fn stored_stamp(cwd: &std::path::Path) -> Option<String> {
    // #2816 (C3) - the stamp must never be in the tracked file.
    let tracked = stamp_config_path(cwd).with_file_name("config.json");
    if let Ok(raw) = std::fs::read_to_string(&tracked) {
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("tracked json");
        assert!(
            parsed["tooling"].get("lastAgentMessageAt").is_none(),
            "the stamp is still tracked in {}",
            tracked.display()
        );
    }
    let raw = std::fs::read_to_string(stamp_config_path(cwd)).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parsed["tooling"]["lastAgentMessageAt"]
        .as_str()
        .map(|s| s.to_string())
}

/// #1682 - one session with the `agent_id` and `working_directory` the caller
/// needs, armed or not.
async fn stamp_test_session(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    cwd: &std::path::Path,
    agent_id: Option<String>,
    arm: bool,
) -> Uuid {
    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(
            "codex".into(),
            vec![],
            cwd.to_str().expect("utf-8 temp path").to_string(),
            agent_id,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create the stamp test session");
    if arm {
        mgr.arm_agent_turn(session.id).await;
    }
    session.id
}

/// #1682 - assert exactly one `session_agent_message` arrived for `id`
/// carrying `at`, and nothing else.
fn assert_one_event(events: &std::sync::mpsc::Receiver<String>, id: Uuid, at: &str, context: &str) {
    let payload = events
        .try_recv()
        .unwrap_or_else(|e| panic!("{context}: expected one event, got {e}"));
    assert!(
        payload.contains(&id.to_string()),
        "{context}: event must carry the session id, got {payload}"
    );
    assert!(
        payload.contains(at),
        "{context}: event must carry {at}, got {payload}"
    );
    assert!(
        events.try_recv().is_err(),
        "{context}: exactly one event, not more"
    );
}

#[tokio::test]
async fn get_last_agent_message_binds_the_stored_value_and_every_unavailable_case() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = agent_stamp_test_app(Arc::clone(&session_mgr));
    // Unknown session.
    assert_eq!(
        super::super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            Uuid::new_v4().to_string()
        )
        .await,
        Ok(None)
    );

    // Malformed uuid.
    assert!(
        super::super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            "not-a-uuid".to_string()
        )
        .await
        .is_err(),
        "a malformed uuid is an Err, not Ok(None)"
    );

    // Known session with no coding agent.
    let plain_dir = tempfile::tempdir().expect("tempdir");
    let plain = stamp_test_session(&session_mgr, plain_dir.path(), None, false).await;
    assert_eq!(
        super::super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            plain.to_string()
        )
        .await,
        Ok(None)
    );

    // Agent session whose replica carries no config.json yet.
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let agent = stamp_test_session(
        &session_mgr,
        agent_dir.path(),
        Some("codex".to_string()),
        false,
    )
    .await;
    assert_eq!(
        super::super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            agent.to_string()
        )
        .await,
        Ok(None)
    );

    // The control: without it every assertion above passes on a function
    // that always returns `Ok(None)`.
    assert_eq!(
        crate::config::agent_config::set_last_agent_message_at(
            agent_dir.path().to_str().expect("utf-8 temp path"),
            "2026-08-31T21:29:07Z",
        ),
        Ok(true)
    );
    assert_eq!(
        super::super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            agent.to_string()
        )
        .await,
        Ok(Some("2026-08-31T21:29:07Z".to_string()))
    );
}

#[tokio::test]
async fn record_agent_turn_completed_writes_and_emits_only_for_an_armed_agent_session() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = agent_stamp_test_app(Arc::clone(&session_mgr));
    let events = capture_agent_message_events(&app);

    // Armed, with a coding agent: the stamp lands and is announced.
    let dir = tempfile::tempdir().expect("tempdir");
    let id = stamp_test_session(&session_mgr, dir.path(), Some("codex".to_string()), true).await;
    super::super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, id, None)
        .await;
    let first = stored_stamp(dir.path()).expect("the first edge must write the stamp");
    let first_parsed =
        chrono::DateTime::parse_from_rfc3339(&first).expect("the stored value is RFC3339");
    assert_one_event(&events, id, &first, "first edge");

    // A later edge advances it, and announces the new value.
    tokio::time::sleep(Duration::from_millis(2)).await;
    super::super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, id, None)
        .await;
    let second = stored_stamp(dir.path()).expect("the second edge must write the stamp");
    let second_parsed =
        chrono::DateTime::parse_from_rfc3339(&second).expect("the stored value is RFC3339");
    assert!(
        second_parsed > first_parsed,
        "the second edge must be strictly later: {first} -> {second}"
    );
    assert_one_event(&events, id, &second, "second edge");

    // Negative control 1: an agent session nothing ever armed.
    let unarmed_dir = tempfile::tempdir().expect("tempdir");
    let unarmed = stamp_test_session(
        &session_mgr,
        unarmed_dir.path(),
        Some("codex".to_string()),
        false,
    )
    .await;
    super::super::record_agent_turn_completed(
        app.handle(),
        &*session_mgr.read().await,
        unarmed,
        None,
    )
    .await;
    assert!(
        no_stamp_file(unarmed_dir.path()),
        "an unarmed session must not create a config.json"
    );
    assert!(
        events.try_recv().is_err(),
        "an unarmed session emits nothing"
    );

    // Negative control 2: armed, but a plain shell with no coding agent.
    let plain_dir = tempfile::tempdir().expect("tempdir");
    let plain = stamp_test_session(&session_mgr, plain_dir.path(), None, true).await;
    super::super::record_agent_turn_completed(
        app.handle(),
        &*session_mgr.read().await,
        plain,
        None,
    )
    .await;
    assert!(
        no_stamp_file(plain_dir.path()),
        "a session with no agent_id must not create a config.json"
    );
    assert!(
        events.try_recv().is_err(),
        "a session with no agent_id emits nothing"
    );
}

#[tokio::test]
async fn record_agent_turn_completed_gate_2_skips_only_recent_input() {
    use crate::pty::input_activity::{SubstantiveInputState, USER_WRITE_STAMP_WINDOW};

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = agent_stamp_test_app(Arc::clone(&session_mgr));
    let events = capture_agent_message_events(&app);
    let tracker = app.state::<SubstantiveInputState>().inner().clone();

    let dir = tempfile::tempdir().expect("tempdir");
    let id = stamp_test_session(&session_mgr, dir.path(), Some("codex".to_string()), true).await;

    // (a) A half-typed line that just arrived suppresses the stamp.
    tracker
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .feed(id, b"hel");
    super::super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, id, None)
        .await;
    assert!(
        no_stamp_file(dir.path()),
        "recent unsubmitted input must suppress the write"
    );
    assert!(
        events.try_recv().is_err(),
        "recent unsubmitted input emits nothing"
    );

    // (b) The age half. The flag is still set and nothing was submitted, but
    // the keystroke is now older than the window, so it no longer suppresses.
    // This is the leg that goes red if gate 2 is written unaged.
    tracker
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .backdate_pending_for_test(id, USER_WRITE_STAMP_WINDOW + Duration::from_secs(1));
    super::super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, id, None)
        .await;
    let aged = stored_stamp(dir.path()).expect("aged input must not suppress the write");
    chrono::DateTime::parse_from_rfc3339(&aged).expect("the stored value is RFC3339");
    assert_one_event(&events, id, &aged, "aged pending input");

    // (c) The control for (a): a submitted line clears `pending_nonspace`,
    // so the stamp lands. Without it, (a) stays green on a no-op function.
    let submitted_dir = tempfile::tempdir().expect("tempdir");
    let submitted = stamp_test_session(
        &session_mgr,
        submitted_dir.path(),
        Some("codex".to_string()),
        true,
    )
    .await;
    {
        let mut guard = tracker.lock().unwrap_or_else(|e| e.into_inner());
        guard.feed(submitted, b"hel");
        guard.feed(submitted, b"\r");
    }
    super::super::record_agent_turn_completed(
        app.handle(),
        &*session_mgr.read().await,
        submitted,
        None,
    )
    .await;
    let cleared = stored_stamp(submitted_dir.path()).expect("a submitted line must not suppress");
    chrono::DateTime::parse_from_rfc3339(&cleared).expect("the stored value is RFC3339");
    assert_one_event(&events, submitted, &cleared, "submitted line");
}

#[tokio::test]
async fn record_agent_turn_completed_gate_1_skips_only_a_recent_control_write() {
    use crate::pty::input_activity::USER_WRITE_STAMP_WINDOW;

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = agent_stamp_test_app(Arc::clone(&session_mgr));
    let events = capture_agent_message_events(&app);

    // No input is ever fed to the tracker, so gate 2 is open throughout and
    // `control_write_age` is the only thing under test. The age is a
    // parameter, so no detector fixture is needed.

    // Recent control write: suppressed.
    let recent_dir = tempfile::tempdir().expect("tempdir");
    let recent = stamp_test_session(
        &session_mgr,
        recent_dir.path(),
        Some("codex".to_string()),
        true,
    )
    .await;
    super::super::record_agent_turn_completed(
        app.handle(),
        &*session_mgr.read().await,
        recent,
        Some(Duration::from_millis(100)),
    )
    .await;
    assert!(
        no_stamp_file(recent_dir.path()),
        "a control write inside the window must suppress the write"
    );
    assert!(
        events.try_recv().is_err(),
        "a control write inside the window emits nothing"
    );

    // Control 1: a mark older than the window does not suppress. This is the
    // leg that goes red if any `Some(_)` suppresses.
    let aged_dir = tempfile::tempdir().expect("tempdir");
    let aged = stamp_test_session(
        &session_mgr,
        aged_dir.path(),
        Some("codex".to_string()),
        true,
    )
    .await;
    super::super::record_agent_turn_completed(
        app.handle(),
        &*session_mgr.read().await,
        aged,
        Some(USER_WRITE_STAMP_WINDOW + Duration::from_secs(1)),
    )
    .await;
    let aged_at = stored_stamp(aged_dir.path()).expect("an aged mark must not suppress");
    chrono::DateTime::parse_from_rfc3339(&aged_at).expect("the stored value is RFC3339");
    assert_one_event(&events, aged, &aged_at, "aged control write");

    // Control 2: no mark stands at all.
    let none_dir = tempfile::tempdir().expect("tempdir");
    let none = stamp_test_session(
        &session_mgr,
        none_dir.path(),
        Some("codex".to_string()),
        true,
    )
    .await;
    super::super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, none, None)
        .await;
    let none_at = stored_stamp(none_dir.path()).expect("no mark must not suppress");
    chrono::DateTime::parse_from_rfc3339(&none_at).expect("the stored value is RFC3339");
    assert_one_event(&events, none, &none_at, "no control write");
}
