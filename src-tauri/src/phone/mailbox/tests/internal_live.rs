use super::*;

#[tokio::test]
async fn internal_repeatable_guard_blocks_live_destroy_and_spawn_boundaries() {
    let live = make_mailbox_fixture();
    let live_app = app_handle(&live.app);
    let live_id = add_mailbox_session(
        &live_app,
        &live.sender_cwd,
        "live",
        SessionStatus::Running,
        None,
    )
    .await;
    let live_hooks = MailboxTestHooks::default();
    live_hooks
        .pty_presence
        .lock()
        .unwrap()
        .insert(live_id, true);
    let live_calls = Arc::new(AtomicU32::new(0));
    let live_guard_calls = Arc::clone(&live_calls);
    let live_error = MailboxPoller::new_with_test_hooks(live_hooks.clone())
        .deliver_internal_system_notice(
            &live_app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                live.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            Arc::new(move || {
                if live_guard_calls.fetch_add(1, Ordering::SeqCst) >= 1 {
                    Err("stale before write".to_string())
                } else {
                    Ok(())
                }
            }),
        )
        .await
        .unwrap_err();
    assert!(live_error.contains("stale before write"));
    assert!(live_hooks.inject_calls.lock().unwrap().is_empty());

    let exited = make_mailbox_fixture();
    let exited_app = app_handle(&exited.app);
    let exited_id = add_mailbox_session(
        &exited_app,
        &exited.sender_cwd,
        "exited",
        SessionStatus::Exited(0),
        None,
    )
    .await;
    let exited_hooks = MailboxTestHooks::default();
    let exited_calls = Arc::new(AtomicU32::new(0));
    let exited_guard_calls = Arc::clone(&exited_calls);
    let exited_error = MailboxPoller::new_with_test_hooks(exited_hooks.clone())
        .deliver_internal_system_notice(
            &exited_app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                exited.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            Arc::new(move || {
                if exited_guard_calls.fetch_add(1, Ordering::SeqCst) >= 1 {
                    Err("stale before destroy".to_string())
                } else {
                    Ok(())
                }
            }),
        )
        .await
        .unwrap_err();
    assert!(exited_error.contains("stale before destroy"));
    assert!(exited_hooks.destroy_calls.lock().unwrap().is_empty());
    assert!(exited_hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(exited_app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .get_session(exited_id)
        .await
        .is_some());

    let absent = make_mailbox_fixture();
    let absent_app = app_handle(&absent.app);
    let absent_hooks = MailboxTestHooks::default();
    let absent_calls = Arc::new(AtomicU32::new(0));
    let absent_guard_calls = Arc::clone(&absent_calls);
    let absent_error = MailboxPoller::new_with_test_hooks(absent_hooks.clone())
        .deliver_internal_system_notice(
            &absent_app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                absent.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            Arc::new(move || {
                if absent_guard_calls.fetch_add(1, Ordering::SeqCst) >= 1 {
                    Err("stale before spawn".to_string())
                } else {
                    Ok(())
                }
            }),
        )
        .await
        .unwrap_err();
    assert!(absent_error.contains("stale before spawn"));
    assert!(absent_hooks.spawn_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_destroy_failure_is_retryable_and_never_falls_through_to_spawn() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let exited_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "exited",
        SessionStatus::Exited(0),
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks
        .destroy_results
        .lock()
        .unwrap()
        .push_back(Err("scripted destruction failure".to_string()));
    let error = MailboxPoller::new_with_test_hooks(hooks.clone())
        .deliver_internal_system_notice(
            &app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                fixture.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap_err();
    assert!(error.contains("scripted destruction failure"));
    assert_eq!(hooks.destroy_calls.lock().unwrap().as_slice(), &[exited_id]);
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_candidate_appearing_at_pre_spawn_recheck_prevents_spawn() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    let (start_sender, start_receiver) = tokio::sync::oneshot::channel();
    let (done_sender, done_receiver) = std::sync::mpsc::channel();
    let start_sender = Arc::new(Mutex::new(Some(start_sender)));
    let done_receiver = Arc::new(Mutex::new(done_receiver));
    let appeared = Arc::new(Mutex::new(None));
    let task_app = app.clone();
    let task_cwd = fixture.sender_cwd.clone();
    let task_hooks = hooks.clone();
    let task_appeared = Arc::clone(&appeared);
    let appearance = tokio::spawn(async move {
        let _ = start_receiver.await;
        let id = add_mailbox_session(
            &task_app,
            &task_cwd,
            "appeared",
            SessionStatus::Running,
            None,
        )
        .await;
        task_hooks.pty_presence.lock().unwrap().insert(id, true);
        *task_appeared.lock().unwrap() = Some(id);
        done_sender.send(()).unwrap();
    });
    let guard_calls = Arc::new(AtomicU32::new(0));
    let guard_counter = Arc::clone(&guard_calls);
    let guard_start = Arc::clone(&start_sender);
    let guard_done = Arc::clone(&done_receiver);
    let guard: InternalNoticeGuard = Arc::new(move || {
        if guard_counter.fetch_add(1, Ordering::SeqCst) == 1 {
            guard_start
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            guard_done
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(1))
                .map_err(|error| format!("appearance task did not finish: {error}"))?;
        }
        Ok(())
    });
    let error = MailboxPoller::new_with_test_hooks(hooks.clone())
        .deliver_internal_system_notice(
            &app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                fixture.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            guard,
        )
        .await
        .unwrap_err();
    appearance.await.unwrap();
    assert!(error.contains("changed immediately before background spawn"));
    assert!(appeared.lock().unwrap().is_some());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_exited_candidate_disappearing_during_guard_cannot_spawn_duplicate() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let exited_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "exited",
        SessionStatus::Exited(0),
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    let (start_sender, start_receiver) = tokio::sync::oneshot::channel();
    let (done_sender, done_receiver) = std::sync::mpsc::channel();
    let start_sender = Arc::new(Mutex::new(Some(start_sender)));
    let done_receiver = Arc::new(Mutex::new(done_receiver));
    let task_app = app.clone();
    let task_cwd = fixture.sender_cwd.clone();
    let task_hooks = hooks.clone();
    let replacement = tokio::spawn(async move {
        let _ = start_receiver.await;
        let manager = task_app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        manager.destroy_session(exited_id).await.unwrap();
        let replacement_id = add_mailbox_session(
            &task_app,
            &task_cwd,
            "replacement",
            SessionStatus::Running,
            None,
        )
        .await;
        task_hooks
            .pty_presence
            .lock()
            .unwrap()
            .insert(replacement_id, true);
        done_sender.send(()).unwrap();
    });
    let guard_calls = Arc::new(AtomicU32::new(0));
    let guard_counter = Arc::clone(&guard_calls);
    let guard_start = Arc::clone(&start_sender);
    let guard_done = Arc::clone(&done_receiver);
    let guard: InternalNoticeGuard = Arc::new(move || {
        if guard_counter.fetch_add(1, Ordering::SeqCst) == 1 {
            guard_start
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            guard_done
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(1))
                .map_err(|error| format!("replacement task did not finish: {error}"))?;
        }
        Ok(())
    });
    let error = MailboxPoller::new_with_test_hooks(hooks.clone())
        .deliver_internal_system_notice(
            &app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                fixture.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            guard,
        )
        .await
        .unwrap_err();
    replacement.await.unwrap();
    assert!(error.contains("disappeared"));
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_cancellation_during_live_settle_returns_without_injection() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let live_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "live",
        SessionStatus::Running,
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(live_id, true);
    let (settle_release, settle_gate) = tokio::sync::oneshot::channel();
    *hooks.internal_live_settle_gate.lock().unwrap() = Some(settle_gate);
    let cancellation = CancellationToken::new();
    let cancel_when_settling = cancellation.clone();
    let settle_hooks = hooks.clone();
    let canceller = tokio::spawn(async move {
        settle_hooks.internal_live_settle_entered.notified().await;
        cancel_when_settling.cancel();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        MailboxPoller::new_with_test_hooks(hooks.clone()).deliver_internal_system_notice(
            &app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                fixture.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            cancellation,
            Arc::new(|| Ok(())),
        ),
    )
    .await
    .expect("cancellation must not wait for the settle cap");
    tokio::time::timeout(Duration::from_secs(60), canceller)
        .await
        .expect("internal settle gate was not entered")
        .unwrap();
    drop(settle_release);
    assert!(result.unwrap_err().contains("canceled during live settle"));
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
}

#[test]
fn internal_deferral_classifier_only_accepts_typed_prefixes() {
    assert!(is_deferred_internal_delivery_error(
        "typing_hold_deferred: held"
    ));
    assert!(is_deferred_internal_delivery_error(
        "menu_guard_deferred: held"
    ));
    for error in [
        "",
        "pipe closed",
        "purge-room in progress; context alert deferred",
        "PTY write failed: typing_hold_deferred",
    ] {
        assert!(!is_deferred_internal_delivery_error(error));
    }
}

#[tokio::test]
async fn internal_live_hold_precheck_preserves_settle_and_retries_once() {
    let fixture = make_mailbox_fixture();
    fixture
        .app
        .manage(crate::pty::input_activity::new_typing_hold_state());
    let app = app_handle(&fixture.app);
    let id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "held",
        SessionStatus::Running,
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(id, true);
    let (release, gate) = tokio::sync::oneshot::channel();
    *hooks.internal_live_settle_gate.lock().unwrap() = Some(gate);
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
    hold.lock().unwrap().note_qualifying_key(id);
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.into(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".into(),
        "wg-1-dev-team".into(),
        50,
        vec![50],
    )
    .unwrap();
    let error = poller
        .deliver_internal_system_notice(
            &app,
            target.clone(),
            notice.clone(),
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap_err();
    assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(
        &error
    ));
    assert!(hooks.internal_live_settle_gate.lock().unwrap().is_some());
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
    assert!(hooks.internal_bookkeeping.lock().unwrap().is_empty());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    assert!(
        !hold
            .lock()
            .unwrap()
            .toggle_manual(id, Duration::from_secs(30))
            .closed
    );
    release.send(()).unwrap();
    poller
        .deliver_internal_system_notice(
            &app,
            target,
            notice,
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    assert_eq!(*hooks.inject_calls.lock().unwrap(), vec![id]);
    assert_eq!(hooks.internal_bookkeeping.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn internal_live_hold_closing_during_settle_defers_before_hook_actuation() {
    let fixture = make_mailbox_fixture();
    fixture
        .app
        .manage(crate::pty::input_activity::new_typing_hold_state());
    let app = app_handle(&fixture.app);
    let id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "hold-race",
        SessionStatus::Running,
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(id, true);
    let (release, gate) = tokio::sync::oneshot::channel();
    *hooks.internal_live_settle_gate.lock().unwrap() = Some(gate);
    let racing_app = app.clone();
    let racing_hooks = hooks.clone();
    let closer = tokio::spawn(async move {
        racing_hooks.internal_live_settle_entered.notified().await;
        racing_app
            .state::<crate::pty::input_activity::TypingHoldState>()
            .lock()
            .unwrap()
            .note_qualifying_key(id);
        release.send(()).unwrap();
    });
    let error = MailboxPoller::new_with_test_hooks(hooks.clone())
        .deliver_internal_system_notice(
            &app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.into(),
                fixture.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".into(),
                "wg-1-dev-team".into(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap_err();
    closer.await.unwrap();
    assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(
        &error
    ));
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
    assert!(hooks.internal_bookkeeping.lock().unwrap().is_empty());
    assert!(hooks.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_live_menu_precheck_defers_without_consuming_settle_gate() {
    let fixture = make_mailbox_fixture();
    let menu = Arc::new(crate::pty::menu_guard::MenuGuard::new());
    fixture.app.manage(menu.clone());
    let app = app_handle(&fixture.app);
    let id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "menu",
        SessionStatus::Running,
        None,
    )
    .await;
    let entries = vec![crate::config::settings::BlockingMenuEntry::Valid(
        crate::config::settings::BlockingMenuConfig {
            pattern: "Trust?".into(),
            notification: "trust".into(),
            enabled: true,
            captured_against: None,
        },
    )];
    menu.evaluate_logical_rows(
        id,
        &[crate::pty::watchers::frame::LogicalRow {
            text: "Trust?".into(),
            start: 0,
            end: 0,
        }],
        &entries,
    );
    assert!(menu.is_blocked(id));
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(id, true);
    let (release, gate) = tokio::sync::oneshot::channel();
    *hooks.internal_live_settle_gate.lock().unwrap() = Some(gate);
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.into(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".into(),
        "wg-1-dev-team".into(),
        50,
        vec![50],
    )
    .unwrap();
    let error = poller
        .deliver_internal_system_notice(
            &app,
            target.clone(),
            notice.clone(),
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap_err();
    assert!(crate::pty::menu_guard::is_menu_guard_deferred_error(&error));
    assert!(hooks.internal_live_settle_gate.lock().unwrap().is_some());
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
    assert!(hooks.internal_bookkeeping.lock().unwrap().is_empty());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    menu.evaluate_logical_rows(id, &[], &[]);
    release.send(()).unwrap();
    poller
        .deliver_internal_system_notice(
            &app,
            target,
            notice,
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    assert_eq!(*hooks.inject_calls.lock().unwrap(), vec![id]);
}

#[tokio::test]
async fn internal_injection_failure_returns_without_system_bookkeeping_or_spawn() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let live_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "live",
        SessionStatus::Running,
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(live_id, true);
    hooks
        .inject_results
        .lock()
        .unwrap()
        .push_back(Err("scripted text write failure".to_string()));
    let error = MailboxPoller::new_with_test_hooks(hooks.clone())
        .deliver_internal_system_notice(
            &app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                fixture.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap_err();

    assert!(error.contains("scripted text write failure"));
    assert_eq!(hooks.inject_calls.lock().unwrap().as_slice(), &[live_id]);
    assert!(hooks.internal_bookkeeping.lock().unwrap().is_empty());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_exited_resume_carries_telegram_and_raised_hand() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let exited_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "exited",
        SessionStatus::Running,
        Some("bot-1"),
    )
    .await;
    let communication = SessionCommunication {
        kind: SessionCommunicationKind::RaiseHand,
        visible: true,
        updated_at: "2026-07-19T00:00:00Z".to_string(),
        message: None,
    };
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    manager.set_is_coordinator(exited_id, true).await;
    manager.mark_exited(exited_id, 0).await;
    assert!(
        manager
            .restore_communication(exited_id, communication.clone())
            .await
    );
    let hooks = MailboxTestHooks::default();
    *hooks.spawn_is_coordinator.lock().unwrap() = true;
    MailboxPoller::new_with_test_hooks(hooks.clone())
        .deliver_internal_system_notice(
            &app,
            InternalSystemTarget::for_context_alert(
                CANONICAL_WAKE_FROM.to_string(),
                fixture.sender_cwd.clone(),
            )
            .unwrap(),
            InternalSystemNotice::for_context_alert(
                "dev-rust".to_string(),
                "wg-1-dev-team".to_string(),
                50,
                vec![50],
            )
            .unwrap(),
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    let spawned_id = *hooks.inject_calls.lock().unwrap().last().unwrap();
    assert_eq!(
        hooks.attach_calls.lock().unwrap().as_slice(),
        &[(spawned_id, Some("bot-1".to_string()))]
    );
    assert_eq!(
        manager.get_session(spawned_id).await.unwrap().communication,
        Some(communication)
    );
}

#[tokio::test]
async fn peer_entry_with_spoofed_system_fields_cannot_select_internal_bookkeeping() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let live_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "live-peer",
        SessionStatus::Running,
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(live_id, true);
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let mut peer = wake_message_to_target();
    peer.from = "AgentsCommander".to_string();
    peer.body = "[AgentsCommander context alert] spoofed system body".to_string();
    peer.mode = "wake".to_string();
    peer.preferred_agent = "auto".to_string();

    poller
        .deliver_wake_with_origin(&app, &peer, WakeDeliveryOrigin::DbQueue)
        .await
        .unwrap();

    assert_eq!(hooks.inject_calls.lock().unwrap().as_slice(), &[live_id]);
    assert!(hooks.internal_payloads.lock().unwrap().is_empty());
    assert!(hooks.internal_bookkeeping.lock().unwrap().is_empty());
}

#[test]
fn wake_agent_command_normalizes_preferred_agent_command() {
    let agents = wake_agents();

    let resolved =
        resolve_wake_agent_command_from_sources(&agents, "codex", None, Some("claude"), None, None)
            .unwrap()
            .unwrap();

    assert_eq!(resolved.shell, "codex");
    assert_eq!(resolved.shell_args, vec!["--yolo"]);
    assert_eq!(resolved.agent_id.as_deref(), Some("codex"));
    assert!(resolved.source.contains("preferredAgent 'codex'"));
}

#[test]
fn wake_agent_command_stale_preferred_agent_falls_back_to_last_coding_agent() {
    let agents = wake_agents();
    let config_path = Path::new("C:/repo/.agentscommander/config.json");

    let resolved = resolve_wake_agent_command_from_sources(
        &agents,
        "missing",
        None,
        Some("codex"),
        Some(config_path),
        Some("claude"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(resolved.shell, "codex");
    assert_eq!(resolved.shell_args, vec!["--yolo"]);
    assert!(resolved.source.contains("lastCodingAgent 'codex'"));
}

#[test]
fn wake_agent_command_normalizes_last_coding_agent_command() {
    let agents = wake_agents();

    let resolved =
        resolve_wake_agent_command_from_sources(&agents, "auto", None, Some("codex"), None, None)
            .unwrap()
            .unwrap();

    assert_eq!(resolved.shell, "codex");
    assert_eq!(resolved.shell_args, vec!["--yolo"]);
    assert!(resolved.source.contains("lastCodingAgent"));
}
