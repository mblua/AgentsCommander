use super::*;

#[tokio::test]
async fn internal_live_delivery_uses_exact_payload_guard_and_system_bookkeeping() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let coordinator_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "wg-1-dev-team/tech-lead",
        SessionStatus::Running,
        None,
    )
    .await;
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        80,
        vec![50, 75],
    )
    .unwrap();
    let calls = Arc::new(AtomicU32::new(0));
    let guard_calls = Arc::clone(&calls);
    let guard: InternalNoticeGuard = Arc::new(move || {
        guard_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let hooks = MailboxTestHooks::default();
    hooks
        .pty_presence
        .lock()
        .unwrap()
        .insert(coordinator_id, true);
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    poller
        .deliver_internal_system_notice(&app, target, notice, CancellationToken::new(), guard)
        .await
        .unwrap();

    assert_eq!(
        hooks.inject_calls.lock().unwrap().as_slice(),
        &[coordinator_id]
    );
    assert_eq!(
        hooks.internal_payloads.lock().unwrap().as_slice(),
        &[
            "\n[AC context alert] `dev-rust` in `wg-1-dev-team` reached threshold(s): 50%, 75%. \
                No action taken; you decide any follow-up.\n\r"
                .to_string()
        ]
    );
    assert_eq!(
        hooks.internal_bookkeeping.lock().unwrap().as_slice(),
        &[InternalSystemBookkeeping {
            session_id: coordinator_id,
            post_boundary: true,
            silence_touch: true,
            set_last_prompt: false,
            peer_event: false,
            response_watcher: false,
            consumption_verdict: false,
            mailbox_archive: false,
        }]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_no_spawn_or_destroy_events(&hooks);
}

#[tokio::test]
async fn internal_same_fqn_at_another_root_is_not_injected_and_exact_target_spawns() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let wrong_replica = fixture
        ._temp
        .path()
        .join("other-root")
        .join("proj-a")
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_tech-lead");
    std::fs::create_dir_all(&wrong_replica).unwrap();
    let wrong_id = add_mailbox_session(
        &app,
        &wrong_replica,
        "wrong-root",
        SessionStatus::Running,
        None,
    )
    .await;
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    // Spawn must use the target's canonical path, not a caller's possible 8.3 spelling.
    let expected_spawn_cwd = target.replica_dir().to_string_lossy().into_owned();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        50,
        vec![50],
    )
    .unwrap();
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(wrong_id, true);
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

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

    assert_eq!(hooks.spawn_calls.lock().unwrap().len(), 1);
    let spawn = hooks.spawn_calls.lock().unwrap()[0].clone();
    assert_eq!(spawn.to, CANONICAL_WAKE_FROM);
    assert_eq!(spawn.cwd, expected_spawn_cwd);
    assert_ne!(hooks.inject_calls.lock().unwrap()[0], wrong_id);
    assert_eq!(hooks.destroy_calls.lock().unwrap().len(), 0);
}

// ── #2411 internal wake of an auto-closed orchestrator ──

fn manage_i2411_clocks(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
) -> crate::config::coordinator_clocks::CoordinatorClocksState {
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(Default::default()));
    app.manage(Arc::clone(&clocks));
    clocks
}

fn i2411_target_and_notice(
    fixture: &MailboxFixture,
) -> (InternalSystemTarget, InternalSystemNotice) {
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        50,
        vec![50],
    )
    .unwrap();
    (target, notice)
}

#[test]
fn internal_wake_resumes_truth_table() {
    assert!(!internal_wake_resumes(false, false));
    assert!(internal_wake_resumes(false, true));
    assert!(internal_wake_resumes(true, false));
    assert!(internal_wake_resumes(true, true));
}

#[tokio::test]
async fn internal_no_session_auto_closed_recipient_spawns_with_resume() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let clocks = manage_i2411_clocks(&app);
    let (target, notice) = i2411_target_and_notice(&fixture);
    let spawn_cwd = target.replica_dir().to_string_lossy().into_owned();
    let fqn = crate::config::teams::agent_fqn_from_path(&spawn_cwd);
    assert!(clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&fqn, chrono::Utc::now()));
    let hooks = MailboxTestHooks::default();
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

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

    let spawns = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(spawns.len(), 1);
    assert_eq!(spawns[0].cwd, spawn_cwd);
    assert!(
        !spawns[0].skip_auto_resume,
        "auto-closed recipient must resume"
    );
    let injected = hooks.inject_calls.lock().unwrap().clone();
    assert_eq!(injected.len(), 1);
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let spawned = session_mgr
        .read()
        .await
        .get_session(injected[0])
        .await
        .expect("notice injected into the spawned session");
    assert_eq!(spawned.working_directory, spawn_cwd);
    assert_eq!(hooks.destroy_calls.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn internal_no_session_without_marker_spawns_fresh() {
    for other_fqn_marker in [false, true] {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let clocks = manage_i2411_clocks(&app);
        if other_fqn_marker {
            assert!(clocks
                .lock()
                .unwrap()
                .mark_auto_closed("proj-a:wg-9-other/someone", chrono::Utc::now()));
        }
        let (target, notice) = i2411_target_and_notice(&fixture);
        let hooks = MailboxTestHooks::default();
        let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

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

        let spawns = hooks.spawn_calls.lock().unwrap().clone();
        assert_eq!(spawns.len(), 1);
        assert!(
            spawns[0].skip_auto_resume,
            "no marker for this fqn (other_fqn_marker={other_fqn_marker}) must stay fresh"
        );
    }
}

#[tokio::test]
async fn internal_auto_closed_spawn_failure_restores_marker_then_retry_resumes() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let clocks = manage_i2411_clocks(&app);
    let (target, notice) = i2411_target_and_notice(&fixture);
    let fqn = crate::config::teams::agent_fqn_from_path(&target.replica_dir().to_string_lossy());
    let t = chrono::Utc::now() - chrono::Duration::minutes(30);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    let hooks = MailboxTestHooks::default();
    *hooks.spawn_is_coordinator.lock().unwrap() = true;
    *hooks.internal_spawn_error.lock().unwrap() = Some("admission denied".to_string());
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    let first = poller
        .deliver_internal_system_notice(
            &app,
            target.clone(),
            notice.clone(),
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await;
    assert!(first.is_err(), "failed spawn must surface the error");
    assert_eq!(
        clocks.lock().unwrap().auto_closed_at(&fqn),
        Some(t),
        "failed wake spawn must restore the original auto-closed timestamp"
    );
    {
        let spawns = hooks.spawn_calls.lock().unwrap();
        assert_eq!(spawns.len(), 1);
        assert!(!spawns[0].skip_auto_resume);
    }

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
    let spawns = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(spawns.len(), 2);
    assert!(!spawns[1].skip_auto_resume, "retry must resume");
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
}

#[tokio::test]
async fn internal_cancellation_during_background_spawn_drops_the_response_waiter() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        50,
        vec![50],
    )
    .unwrap();
    let hooks = MailboxTestHooks::default();
    let (spawn_release, spawn_gate) = tokio::sync::oneshot::channel();
    *hooks.internal_spawn_gate.lock().unwrap() = Some(spawn_gate);
    let cancellation = CancellationToken::new();
    let cancel_during_spawn = cancellation.clone();
    let cancel_hooks = hooks.clone();
    let canceller = tokio::spawn(async move {
        cancel_hooks.internal_spawn_started.notified().await;
        cancel_during_spawn.cancel();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        MailboxPoller::new_with_test_hooks(hooks.clone()).deliver_internal_system_notice(
            &app,
            target,
            notice,
            cancellation,
            Arc::new(|| Ok(())),
        ),
    )
    .await
    .expect("spawn cancellation must return promptly");
    canceller.await.unwrap();
    drop(spawn_release);

    assert!(result
        .unwrap_err()
        .contains("canceled during background spawn"));
    assert_eq!(hooks.spawn_calls.lock().unwrap().len(), 1);
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
    assert!(hooks.internal_bookkeeping.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_exited_recipient_is_destroyed_then_resumed_without_selection_spawn() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let exited_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "exited-coordinator",
        SessionStatus::Exited(0),
        None,
    )
    .await;
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        50,
        vec![50],
    )
    .unwrap();
    let hooks = MailboxTestHooks::default();
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

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

    assert_eq!(hooks.destroy_calls.lock().unwrap().as_slice(), &[exited_id]);
    assert_eq!(hooks.spawn_calls.lock().unwrap().len(), 1);
    assert!(!hooks.spawn_calls.lock().unwrap()[0].skip_auto_resume);
    let events = hooks.events.lock().unwrap();
    assert!(matches!(events[0], MailboxTestEvent::Destroy(id) if id == exited_id));
    assert!(matches!(events[1], MailboxTestEvent::Spawn(_)));
    assert!(matches!(events[2], MailboxTestEvent::Inject(_)));
}

#[tokio::test]
async fn internal_precanceled_attempt_performs_no_wake_actuation() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        50,
        vec![50],
    )
    .unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let hooks = MailboxTestHooks::default();
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    assert!(poller
        .deliver_internal_system_notice(&app, target, notice, cancellation, Arc::new(|| Ok(())),)
        .await
        .is_err());
    assert!(hooks.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_unsupported_root_agentless_plain_shell_and_escaped_records_are_untouched() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let root_id = add_mailbox_session_with_shape(
        &app,
        &fixture.sender_cwd,
        "root",
        SessionStatus::Running,
        None,
        "codex",
        Some("codex"),
        true,
    )
    .await;
    let agentless_id = add_mailbox_session_with_shape(
        &app,
        &fixture.sender_cwd,
        "agentless",
        SessionStatus::Running,
        None,
        "codex",
        None,
        false,
    )
    .await;
    let shell_id = add_mailbox_session_with_shape(
        &app,
        &fixture.sender_cwd,
        "plain-shell",
        SessionStatus::Running,
        None,
        "pwsh",
        Some("codex"),
        false,
    )
    .await;
    let escape_subdir = fixture.sender_cwd.join("subdir");
    std::fs::create_dir_all(&escape_subdir).unwrap();
    let escaped_cwd = escape_subdir.join("..").join("..").join("__agent_dev-rust");
    let escaped_id = add_mailbox_session_with_shape(
        &app,
        &escaped_cwd,
        "escaped",
        SessionStatus::Running,
        None,
        "codex",
        Some("codex"),
        false,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    for id in [root_id, agentless_id, shell_id, escaped_id] {
        hooks.pty_presence.lock().unwrap().insert(id, true);
    }
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        fixture.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        50,
        vec![50],
    )
    .unwrap();

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

    assert_eq!(hooks.spawn_calls.lock().unwrap().len(), 1);
    let injected = hooks.inject_calls.lock().unwrap()[0];
    assert!(![root_id, agentless_id, shell_id, escaped_id].contains(&injected));
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    for id in [root_id, agentless_id, shell_id, escaped_id] {
        assert!(manager.get_session(id).await.is_some());
    }
}

#[tokio::test]
async fn internal_missing_command_and_unsupported_resolved_shell_never_spawn() {
    let missing = make_mailbox_fixture();
    let missing_app = app_handle(&missing.app);
    missing_app
        .state::<SettingsState>()
        .write()
        .await
        .agents
        .clear();
    let missing_hooks = MailboxTestHooks::default();
    let missing_poller = MailboxPoller::new_with_test_hooks(missing_hooks.clone());
    let missing_target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        missing.sender_cwd.clone(),
    )
    .unwrap();
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        50,
        vec![50],
    )
    .unwrap();
    let error = missing_poller
        .deliver_internal_system_notice(
            &missing_app,
            missing_target,
            notice.clone(),
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap_err();
    assert!(error.contains("No supported coding-agent command"));
    assert!(missing_hooks.spawn_calls.lock().unwrap().is_empty());

    let unsupported = make_mailbox_fixture();
    let unsupported_app = app_handle(&unsupported.app);
    unsupported_app
        .state::<SettingsState>()
        .write()
        .await
        .agents = vec![wake_agent("codex", "Codex", "pwsh")];
    std::fs::write(
        unsupported.sender_cwd.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","tooling":{"currentCodingAgent":"codex"}}"#,
    )
    .unwrap();
    let unsupported_hooks = MailboxTestHooks::default();
    let unsupported_poller = MailboxPoller::new_with_test_hooks(unsupported_hooks.clone());
    let unsupported_target = InternalSystemTarget::for_context_alert(
        CANONICAL_WAKE_FROM.to_string(),
        unsupported.sender_cwd.clone(),
    )
    .unwrap();
    let error = unsupported_poller
        .deliver_internal_system_notice(
            &unsupported_app,
            unsupported_target,
            notice,
            CancellationToken::new(),
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap_err();
    assert!(error.contains("not a supported coding-agent CLI"));
    assert!(unsupported_hooks.spawn_calls.lock().unwrap().is_empty());
}
