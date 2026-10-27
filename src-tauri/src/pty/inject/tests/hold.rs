use super::*;

#[tokio::test]
async fn missing_session_record_with_lingering_route_writes_nothing() {
    let id = Uuid::new_v4();
    let backend = Arc::new(RecordingBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(id, SessionBackendKind::LocalProcess);
    let app = tauri::test::mock_builder()
        .manage(Arc::new(tokio::sync::RwLock::new(SessionManager::new())))
        .manage(pty)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();

    let err = inject_text_into_session(app.handle(), id, "/new")
        .await
        .unwrap_err();

    assert_eq!(err, format!("Session not found: {id}"));
    assert!(backend.writes.lock().unwrap().is_empty());
}

async fn recorded_injection_writes_for_shell(shell: &str) -> Vec<Vec<u8>> {
    let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = session_manager
        .read()
        .await
        .create_session(
            shell.to_string(),
            Vec::new(),
            "C:\\test".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let id = session.id;
    let backend = Arc::new(RecordingBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(id, SessionBackendKind::LocalProcess);
    let app = tauri::test::mock_builder()
        .manage(session_manager)
        .manage(pty)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();

    inject_text_into_session(app.handle(), id, "arbitrary payload")
        .await
        .unwrap();

    let all_writes = backend.writes.lock().unwrap().clone();
    all_writes
        .into_iter()
        .filter(|(write_id, _)| *write_id == id)
        .map(|(_, bytes)| bytes)
        .collect()
}

#[tokio::test]
async fn explicit_submit_stems_write_text_then_two_enters() {
    let (hermes, opencode, grok) = tokio::join!(
        recorded_injection_writes_for_shell("hermes"),
        recorded_injection_writes_for_shell("opencode"),
        recorded_injection_writes_for_shell("grok"),
    );
    for (shell, writes) in [("hermes", hermes), ("opencode", opencode), ("grok", grok)] {
        assert_eq!(
            writes,
            vec![
                b"arbitrary payload".to_vec(),
                b"\r".to_vec(),
                b"\r".to_vec()
            ],
            "shell={shell}"
        );
    }
}

#[tokio::test]
async fn unsupported_stem_writes_text_without_enter() {
    let writes = recorded_injection_writes_for_shell("muse").await;
    assert_eq!(writes, vec![b"arbitrary payload".to_vec()]);
}

#[tokio::test]
async fn test_injection_blocked_when_menu_guard_active() {
    let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = session_manager
        .read()
        .await
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\test".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let id = session.id;

    let backend = Arc::new(RecordingBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(id, SessionBackendKind::LocalProcess);

    let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::new());
    let entries = vec![crate::config::settings::BlockingMenuEntry::Valid(
        crate::config::settings::BlockingMenuConfig {
            pattern: "Do you trust".to_string(),
            notification: "trust dialog".to_string(),
            enabled: true,
            captured_against: None,
        },
    )];
    let eval = menu_guard.evaluate_logical_rows(
        id,
        &[crate::pty::watchers::frame::LogicalRow {
            text: "Do you trust the authors of this file?".to_string(),
            start: 0,
            end: 0,
        }],
        &entries,
    );
    assert!(eval.is_blocked);
    assert!(menu_guard.is_blocked(id));

    let app = tauri::test::mock_builder()
        .manage(session_manager)
        .manage(pty)
        .manage(menu_guard)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();

    let err = inject_text_into_session(app.handle(), id, "echo hello")
        .await
        .unwrap_err();

    assert!(crate::pty::menu_guard::is_menu_guard_deferred_error(&err));
    assert!(err.contains(&id.to_string()));
    assert!(backend.writes.lock().unwrap().is_empty());
}

async fn supported_agent_typing_hold_app(
    shell: &str,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    Uuid,
    Arc<RecordingBackend>,
) {
    let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = session_manager
        .read()
        .await
        .create_session(
            shell.to_string(),
            Vec::new(),
            "C:\\test".to_string(),
            Some(shell.to_string()),
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let id = session.id;
    let backend = Arc::new(RecordingBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(id, SessionBackendKind::LocalProcess);
    let settings_state: SettingsState = new_settings_state(AppSettings::default());
    let app = tauri::test::mock_builder()
        .manage(session_manager)
        .manage(pty)
        .manage(crate::pty::input_activity::new_typing_hold_state())
        .manage(settings_state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    (app, id, backend)
}

/// #2336 - a peer wake is deferred with zero bytes written while the typing
/// hold is active; a repeated poll does not grow the unique count; a manual
/// release lets the very next attempt deliver and clears the id.
#[tokio::test]
async fn test_peer_wake_injection_blocked_while_typing_hold_active() {
    let (app, id, backend) = typing_hold_app("claude").await;
    let window = std::time::Duration::from_secs(30);
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
    hold.lock().unwrap().note_qualifying_key(id);

    let err = inject_peer_wake_text_into_session(
        app.handle(),
        id,
        "echo hello",
        "msg-1",
        SettleReadiness::Unknown,
    )
    .await
    .unwrap_err();
    assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(&err));
    assert!(err.contains(&id.to_string()));
    assert!(
        backend.writes.lock().unwrap().is_empty(),
        "payload and Enter bytes must not be written while held"
    );
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

    // A repeated poll of the same message never increases the unique count.
    let _ = inject_peer_wake_text_into_session(
        app.handle(),
        id,
        "echo hello",
        "msg-1",
        SettleReadiness::Unknown,
    )
    .await;
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

    // The closed-click release suppresses the window, so the next attempt
    // delivers and the observed id drops out of the count.
    let released = hold.lock().unwrap().toggle_manual(id, window);
    assert!(!released.closed);
    inject_peer_wake_text_into_session(
        app.handle(),
        id,
        "echo hello",
        "msg-1",
        SettleReadiness::Unknown,
    )
    .await
    .unwrap();
    let writes: Vec<Vec<u8>> = backend
        .writes
        .lock()
        .unwrap()
        .iter()
        .filter(|(write_id, _)| *write_id == id)
        .map(|(_, bytes)| bytes.clone())
        .collect();
    assert_eq!(
        writes,
        vec![b"echo hello".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
}

#[tokio::test]
async fn response_close_peer_callback_respects_hold_and_success_cleanup() {
    let (app, id, backend) = typing_hold_app("bash").await;
    let window = std::time::Duration::from_secs(30);
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
    hold.lock().unwrap().note_qualifying_key(id);
    let calls = std::cell::Cell::new(0);
    let error = inject_peer_wake_text_into_session_with_pre_write_check(
        app.handle(),
        id,
        "payload",
        "callback-held",
        SettleReadiness::Unknown,
        |_| {
            calls.set(calls.get() + 1);
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(
        &error
    ));
    assert_eq!(calls.get(), 0);
    assert!(recorded_writes(&backend, id).is_empty());
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);
    assert!(!hold.lock().unwrap().toggle_manual(id, window).closed);
    inject_peer_wake_text_into_session_with_pre_write_check(
        app.handle(),
        id,
        "payload",
        "callback-held",
        SettleReadiness::Unknown,
        |session| {
            assert_eq!(session.id, id);
            assert!(recorded_writes(&backend, id).is_empty());
            calls.set(calls.get() + 1);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(recorded_writes(&backend, id), vec![b"payload".to_vec()]);
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
}

#[tokio::test]
async fn response_close_peer_callback_waits_for_writer_and_rejects_before_bytes() {
    use std::future::Future;
    let (app, id, backend) = typing_hold_app("bash").await;
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let writer = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
    let calls = std::cell::Cell::new(0);
    let mut delivery = std::pin::pin!(inject_peer_wake_text_into_session_with_pre_write_check(
        app.handle(),
        id,
        "payload",
        "callback-rejected",
        SettleReadiness::Unknown,
        |session| {
            assert_eq!(session.id, id);
            assert!(recorded_writes(&backend, id).is_empty());
            calls.set(calls.get() + 1);
            Err("peer authorization changed".to_string())
        },
    ));
    {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(delivery.as_mut().poll(&mut context).is_pending());
    }
    assert_eq!(calls.get(), 0);
    assert!(recorded_writes(&backend, id).is_empty());
    drop(writer);
    assert_eq!(delivery.await, Err("peer authorization changed".into()));
    assert_eq!(calls.get(), 1);
    assert!(recorded_writes(&backend, id).is_empty());
    inject_peer_wake_text_into_session(
        app.handle(),
        id,
        "payload",
        "legacy-noop",
        SettleReadiness::Unknown,
    )
    .await
    .unwrap();
    assert_eq!(recorded_writes(&backend, id), vec![b"payload".to_vec()]);
}

/// Plain maintenance/Telegram writes remain exempt from typing hold.
#[tokio::test]
async fn test_plain_injection_bypasses_typing_hold() {
    let (app, id, backend) = typing_hold_app("claude").await;
    app.state::<crate::pty::input_activity::TypingHoldState>()
        .lock()
        .unwrap()
        .note_qualifying_key(id);

    inject_text_into_session(app.handle(), id, "internal notice")
        .await
        .unwrap();

    let writes: Vec<Vec<u8>> = backend
        .writes
        .lock()
        .unwrap()
        .iter()
        .filter(|(write_id, _)| *write_id == id)
        .map(|(_, bytes)| bytes.clone())
        .collect();
    assert_eq!(writes.len(), 3);
    assert_eq!(writes[0], b"internal notice".to_vec());
}

#[tokio::test]
async fn internal_notice_hold_writes_nothing_and_retries_without_held_id() {
    for manual in [false, true] {
        let (app, id, backend) = supported_agent_typing_hold_app("claude").await;
        let window = std::time::Duration::from_secs(30);
        let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
        if manual {
            assert!(hold.lock().unwrap().toggle_manual(id, window).closed);
        } else {
            hold.lock().unwrap().note_qualifying_key(id);
        }
        let error =
            inject_internal_notice_into_supported_agent_session(app.handle(), id, "notice", |_| {
                Ok(())
            })
            .await
            .unwrap_err();
        assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(
            &error
        ));
        assert!(recorded_writes(&backend, id).is_empty());
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
        if manual {
            assert!(!hold.lock().unwrap().toggle_manual(id, window).closed);
        } else {
            hold.lock()
                .unwrap()
                .backdate_last_key_for_test(id, window + std::time::Duration::from_secs(1));
        }
        inject_internal_notice_into_supported_agent_session(app.handle(), id, "notice", |_| Ok(()))
            .await
            .unwrap();
        assert_eq!(
            recorded_writes(&backend, id),
            vec![b"notice".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
    }
}

#[tokio::test]
async fn supported_restart_resume_wrapper_remains_exempt_under_hold() {
    let (app, id, backend) = supported_agent_typing_hold_app("claude").await;
    app.state::<crate::pty::input_activity::TypingHoldState>()
        .lock()
        .unwrap()
        .note_qualifying_key(id);
    inject_text_into_supported_agent_session_with_pre_write_check(
        app.handle(),
        id,
        "resume",
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(
        recorded_writes(&backend, id),
        vec![b"resume".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
}

#[tokio::test]
async fn issue_3286_queued_notice_exited_writes_nothing() {
    use std::future::Future;
    let (app, id, backend) = supported_agent_typing_hold_app("claude").await;
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    manager.mark_idle(id).await;
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let writer = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
    let calls = std::cell::Cell::new(0);
    let mut delivery = std::pin::pin!(inject_internal_notice_into_supported_agent_session(
        app.handle(),
        id,
        "notice",
        |_| {
            calls.set(calls.get() + 1);
            Ok(())
        },
    ));
    {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(delivery.as_mut().poll(&mut context).is_pending());
    }
    assert_eq!(calls.get(), 0);
    assert!(recorded_writes(&backend, id).is_empty());
    manager.mark_exited(id, 0).await;
    assert!(matches!(
        manager.get_session(id).await.unwrap().status,
        SessionStatus::Exited(0)
    ));
    drop(writer);
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), delivery)
        .await
        .unwrap();
    assert!(recorded_writes(&backend, id).is_empty());
    assert_eq!(calls.get(), 0);
    assert_eq!(result, Err(format!("Session {id} exited before injection")));
}

#[tokio::test]
async fn issue_3286_queued_notice_root_writes_nothing() {
    use std::future::Future;
    let (app, id, backend) = supported_agent_typing_hold_app("claude").await;
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    manager.mark_idle(id).await;
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let writer = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
    let calls = std::cell::Cell::new(0);
    let mut delivery = std::pin::pin!(inject_internal_notice_into_supported_agent_session(
        app.handle(),
        id,
        "notice",
        |_| {
            calls.set(calls.get() + 1);
            Ok(())
        },
    ));
    {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(delivery.as_mut().poll(&mut context).is_pending());
    }
    assert_eq!(calls.get(), 0);
    assert!(recorded_writes(&backend, id).is_empty());
    manager.set_is_root_agent(id, true).await;
    assert!(manager.get_session(id).await.unwrap().is_root_agent);
    drop(writer);
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), delivery)
        .await
        .unwrap();
    assert!(recorded_writes(&backend, id).is_empty());
    assert_eq!(calls.get(), 0);
    assert_eq!(
        result,
        Err(format!(
            "Session {id} is a root session, not a supported orchestrator agent"
        ))
    );
}

#[tokio::test]
async fn issue_3286_queued_notice_admissible_keeps_bytes() {
    use std::future::Future;
    for becomes_busy in [false, true] {
        let (app, id, backend) = supported_agent_typing_hold_app("claude").await;
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        manager.mark_idle(id).await;
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let writer = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
        let calls = std::cell::Cell::new(0);
        let mut delivery = std::pin::pin!(inject_internal_notice_into_supported_agent_session(
            app.handle(),
            id,
            "notice",
            |_| {
                calls.set(calls.get() + 1);
                Ok(())
            },
        ));
        {
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(delivery.as_mut().poll(&mut context).is_pending());
        }
        assert_eq!(calls.get(), 0);
        assert!(recorded_writes(&backend, id).is_empty());
        if becomes_busy {
            manager.mark_busy(id).await;
        }
        assert_eq!(
            manager.get_session(id).await.unwrap().waiting_for_input,
            !becomes_busy
        );
        drop(writer);
        let result = tokio::time::timeout(std::time::Duration::from_secs(15), delivery)
            .await
            .unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(calls.get(), 1);
        assert_eq!(
            recorded_writes(&backend, id),
            vec![b"notice".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );
    }
}

#[tokio::test]
async fn internal_notice_queued_behind_writer_rechecks_hold() {
    let (app, id, backend) = supported_agent_typing_hold_app("claude").await;
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let permit = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
    let app_clone = app.handle().clone();
    let queued = tokio::spawn(async move {
        inject_internal_notice_into_supported_agent_session(&app_clone, id, "notice", |_| Ok(()))
            .await
    });
    app.state::<crate::pty::input_activity::TypingHoldState>()
        .lock()
        .unwrap()
        .note_qualifying_key(id);
    drop(permit);
    assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(
        &queued.await.unwrap().unwrap_err()
    ));
    assert!(recorded_writes(&backend, id).is_empty());
}

/// #2336 - the serialized-write-boundary race: a peer wake queued behind the
/// held per-session writer permit re-checks the hold when it finally acquires
/// the permit, so a qualifying desktop key recorded while the permit was held
/// defers it with the typed marker and zero payload/Enter bytes. After
/// release the same message delivers exactly once and leaves the count.
#[tokio::test]
async fn typing_hold_peer_wake_queued_behind_writer_permit_rechecks_and_defers() {
    let (app, id, backend) = typing_hold_app("claude").await;
    let window = std::time::Duration::from_secs(30);
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();

    // Hold the writer permit, then queue a peer wake behind it.
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let permit = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
    let app_clone = app.handle().clone();
    let queued = tokio::spawn(async move {
        inject_peer_wake_text_into_session(
            &app_clone,
            id,
            "echo hello",
            "msg-race",
            SettleReadiness::Unknown,
        )
        .await
    });
    // Let the queued wake reach the permit wait.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // A qualifying desktop key lands while the permit is still held.
    hold.lock().unwrap().note_qualifying_key(id);
    drop(permit);

    let err = queued.await.unwrap().unwrap_err();
    assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(&err));
    assert!(
        recorded_writes(&backend, id).is_empty(),
        "a deferred queued wake must write no payload or Enter byte"
    );
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

    // Release: the next attempt delivers exactly once and clears the count.
    assert!(!hold.lock().unwrap().toggle_manual(id, window).closed);
    inject_peer_wake_text_into_session(
        app.handle(),
        id,
        "echo hello",
        "msg-race",
        SettleReadiness::Unknown,
    )
    .await
    .unwrap();
    assert_eq!(
        recorded_writes(&backend, id),
        vec![b"echo hello".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
}

/// #2336 - natural expiry with no manual action: once the configured window
/// has passed since the last qualifying key, the next attempt delivers.
#[tokio::test]
async fn typing_hold_peer_wake_delivers_after_natural_window_expiry() {
    let (app, id, backend) = typing_hold_app("claude").await;
    let window = std::time::Duration::from_secs(30);
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
    hold.lock().unwrap().note_qualifying_key(id);

    let err = inject_peer_wake_text_into_session(
        app.handle(),
        id,
        "echo hello",
        "msg-expiry",
        SettleReadiness::Unknown,
    )
    .await
    .unwrap_err();
    assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(&err));
    assert!(recorded_writes(&backend, id).is_empty());
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

    // Age the key past the configured window with no release of any kind.
    hold.lock()
        .unwrap()
        .backdate_last_key_for_test(id, window + std::time::Duration::from_secs(1));

    inject_peer_wake_text_into_session(
        app.handle(),
        id,
        "echo hello",
        "msg-expiry",
        SettleReadiness::Unknown,
    )
    .await
    .unwrap();
    assert_eq!(
        recorded_writes(&backend, id),
        vec![b"echo hello".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
    assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
}
