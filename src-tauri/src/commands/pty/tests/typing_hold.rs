use super::*;

/// #2336 - `get_typing_hold` / `toggle_typing_hold`: UUID validation, absent
/// session error, per-session isolation, and the flip/snapshot contract.
#[tokio::test]
async fn typing_hold_commands_report_and_reject_sessions() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let first = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            "C:/ac-test/project/.ac/wg-2336-dev-team/__agent_one".to_string(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create first session")
    };
    let second = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            "C:/ac-test/project/.ac/wg-2336-dev-team/__agent_two".to_string(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create second session")
    };
    let app = crate::test_support::test_builder()
        .manage(crate::config::settings::new_settings_state(
            crate::config::settings::AppSettings::default(),
        ))
        .manage(session_mgr)
        .manage(crate::pty::input_activity::new_typing_hold_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build typing hold command test app");

    assert!(
        get_typing_hold(app.handle().clone(), "not-a-uuid".to_string())
            .await
            .is_err(),
        "a malformed session id must error"
    );
    let err = get_typing_hold(app.handle().clone(), Uuid::new_v4().to_string())
        .await
        .unwrap_err();
    assert!(err.contains("Session not found"), "{err}");

    let open = get_typing_hold(app.handle().clone(), first.id.to_string())
        .await
        .unwrap();
    assert!(!open.closed);
    assert_eq!(open.held_count, 0);

    let held = toggle_typing_hold(app.handle().clone(), first.id.to_string())
        .await
        .unwrap();
    assert!(held.closed);
    let other = get_typing_hold(app.handle().clone(), second.id.to_string())
        .await
        .unwrap();
    assert!(!other.closed, "toggle must be per-session");

    let released = toggle_typing_hold(app.handle().clone(), first.id.to_string())
        .await
        .unwrap();
    assert!(!released.closed);
}

/// #2336 - the desktop `pty_write` path is the only recorder for the typing
/// hold, and only qualifying human chunks arm it.
#[test]
fn typing_hold_recorder_arms_on_human_keys_and_ignores_terminal_sequences() {
    let app = tauri::test::mock_builder()
        .manage(crate::pty::input_activity::new_typing_hold_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build typing hold test app");
    let id = Uuid::new_v4();
    let window = Duration::from_secs(30);
    let state = app.state::<crate::pty::input_activity::TypingHoldState>();

    record_typing_hold_keystroke(app.handle(), id, b"\x1b[A");
    assert!(!state.lock().unwrap().is_hold_active(id, window));
    record_typing_hold_keystroke(app.handle(), id, b"");
    assert!(!state.lock().unwrap().is_hold_active(id, window));

    record_typing_hold_keystroke(app.handle(), id, b"hi");
    assert!(state.lock().unwrap().is_hold_active(id, window));
}

/// #2336 - the user-message bookkeeping is not the recording choke point:
/// the web transport's `UserInputSource::Web` never arms the hold.
#[tokio::test]
async fn typing_hold_user_message_sources_do_not_arm_hold() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let app = tauri::test::mock_builder()
        .manage(session_mgr.clone())
        .manage(clocks)
        .manage(crate::pty::input_activity::new_state())
        .manage(crate::pty::input_activity::new_typing_hold_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build typing hold user input test app");
    let session = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            "C:/ac-test/project/.ac/wg-2336-dev-team/__agent_rust".to_string(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session")
    };
    let id = session.id;
    let window = Duration::from_secs(30);
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();

    note_user_message_to_session(app.handle(), id, UserInputSource::Web(b"hello")).await;
    assert!(!hold.lock().unwrap().is_hold_active(id, window));

    note_user_message_to_session(app.handle(), id, UserInputSource::Terminal(b"hello")).await;
    assert!(!hold.lock().unwrap().is_hold_active(id, window));
}

/// #2336 - the destroy/restart side-state purge clears the typing hold with
/// the substantive tracker it already reset, so no manual padlock or counted
/// ID survives a session teardown.
#[test]
fn typing_hold_is_cleared_by_session_side_state_purge() {
    let app = tauri::test::mock_builder()
        .manage(crate::pty::input_activity::new_typing_hold_state())
        .manage(crate::pty::input_activity::new_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build teardown test app");
    let id = Uuid::new_v4();
    let window = Duration::from_secs(30);
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();

    record_typing_hold_keystroke(app.handle(), id, b"hi");
    hold.lock().unwrap().record_held_message(id, "held-msg");
    assert!(hold.lock().unwrap().is_hold_active(id, window));
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

    crate::commands::session::purge_session_side_state(app.handle(), id);

    assert!(!hold.lock().unwrap().is_hold_active(id, window));
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
}

/// #2336 - deterministic boundary race: a qualifying desktop key lands while
/// a logical-clear writer permit is in flight (before the stamp, the order
/// the plan specifies), then the shared fresh-boundary stamp runs. The stamp
/// resets ONLY the substantive tracker, so the hold stays armed, counted held
/// IDs survive, and the next peer wake is deferred. The pre-set fresh intent
/// keeps the record half a no-op, so the test writes no instance state.
#[tokio::test]
async fn typing_hold_survives_fresh_boundary_stamp_and_defers_next_wake() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let app = tauri::test::mock_builder()
        .manage(session_mgr.clone())
        .manage(clocks)
        .manage(crate::pty::input_activity::new_state())
        .manage(crate::pty::input_activity::new_typing_hold_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build boundary race test app");
    let session = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            "C:/ac-test/project/.ac/wg-2336-dev-team/__agent_rust".to_string(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session")
    };
    let id = session.id;
    {
        let mgr = session_mgr.read().await;
        mgr.set_start_fresh_on_restore(id, true).await;
    }
    let window = Duration::from_secs(30);

    let substantive = app.state::<crate::pty::input_activity::SubstantiveInputState>();
    substantive.lock().unwrap().feed(id, b"half-typed");
    assert!(substantive
        .lock()
        .unwrap()
        .pending_within(id, crate::pty::input_activity::USER_WRITE_STAMP_WINDOW));
    record_typing_hold_keystroke(app.handle(), id, b"half-typed");
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
    hold.lock().unwrap().record_held_message(id, "held-msg");

    stamp_fresh_boundary_to_session(app.handle(), id).await;

    assert!(hold.lock().unwrap().is_hold_active(id, window));
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);
    assert!(typing_hold_defers_injection(app.handle(), id).await);
    // The stamp still did its own job on the substantive tracker.
    assert!(!substantive
        .lock()
        .unwrap()
        .pending_within(id, crate::pty::input_activity::USER_WRITE_STAMP_WINDOW));
}
