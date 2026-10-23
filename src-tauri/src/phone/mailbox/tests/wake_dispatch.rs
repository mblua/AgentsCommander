use super::*;

/// #1883 — both mailbox injection sites must log a menu-guard deferral at
/// debug (never into the #264 Application Error sink) and every real PTY
/// write failure at ERROR. One test, one sequential drain per phase, so the
/// process-wide sink is observed without racing another drainer.
#[tokio::test]
async fn both_injection_sites_defer_silently_and_report_real_pty_failures() {
    crate::logging::init_logger();
    // #1883 — hold the shared sink-drain lock for the whole test so no
    // other library test drainer (the sync loggate test uses
    // `blocking_lock()`) can steal these canaries between log and drain.
    let _drain_guard = crate::logging::ERROR_SINK_TEST_DRAIN_LOCK.lock().await;
    crate::logging::error_sink().drain();

    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);

    let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::new());
    assert!(fixture.app.manage(Arc::clone(&menu_guard)));

    let site_a_session = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "1883-site-a",
        "pi.cmd",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, site_a_session);
    let site_b_session = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "1883-site-b",
        "pi.cmd",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, site_b_session);

    let entries = vec![crate::config::settings::BlockingMenuEntry::Valid(
        crate::config::settings::BlockingMenuConfig {
            pattern: "trust".to_string(),
            notification: "dialog".to_string(),
            enabled: true,
            captured_against: None,
        },
    )];
    let rows = [crate::pty::watchers::frame::LogicalRow {
        text: "Do you trust the author?".to_string(),
        start: 0,
        end: 0,
    }];
    for session_id in [site_a_session, site_b_session] {
        menu_guard.evaluate_logical_rows(session_id, &rows, &entries);
        assert!(menu_guard.is_blocked(session_id));
    }

    let poller = MailboxPoller::new();

    // Leg A — site A (logical command branch) defers silently.
    let mut defer_a = logical_command_message("clear", "");
    defer_a.id = "1883-a-defer".to_string();
    let err_a = poller
        .inject_into_pty(
            &app,
            site_a_session,
            &defer_a,
            true,
            WakeDeliveryOrigin::FilesystemPoller,
            false,
            SettleReadiness::Unknown,
        )
        .await
        .expect_err("menu-blocked logical command must defer");
    assert!(
        crate::pty::menu_guard::is_menu_guard_deferred_error(&err_a),
        "site A must defer with the canonical prefix: {err_a}"
    );
    assert!(mock_pty_writes_for(&app, site_a_session).is_empty());

    // Leg B — site B (standard wake branch) defers silently.
    let mut defer_b = wake_message_to_target();
    defer_b.id = "1883-b-defer".to_string();
    let err_b = poller
        .inject_into_pty(
            &app,
            site_b_session,
            &defer_b,
            true,
            WakeDeliveryOrigin::FilesystemPoller,
            false,
            SettleReadiness::Unknown,
        )
        .await
        .expect_err("menu-blocked standard wake must defer");
    assert!(
        crate::pty::menu_guard::is_menu_guard_deferred_error(&err_b),
        "site B must defer with the canonical prefix: {err_b}"
    );
    assert!(mock_pty_writes_for(&app, site_b_session).is_empty());

    // Neither deferral may reach the Application Error sink (debug is
    // filtered out at the default info level, so nothing is captured).
    let defer_entries = crate::logging::error_sink().drain();
    for canary in ["1883-a-defer", "1883-b-defer"] {
        assert!(
            !defer_entries
                .iter()
                .any(|entry| entry.message.contains(canary)),
            "deferral {canary} reached Application Error: {defer_entries:?}"
        );
    }

    // Positive control — both sites must still raise a real PTY write
    // failure at ERROR. Clear the guard so the injector reaches the write.
    for session_id in [site_a_session, site_b_session] {
        menu_guard.evaluate_logical_rows(session_id, &[], &[]);
        assert!(!menu_guard.is_blocked(session_id));
    }
    set_mock_pty_write_failure(&app, true);

    let mut fail_a = logical_command_message("clear", "");
    fail_a.id = "1883-a-fail".to_string();
    let err_a = poller
        .inject_into_pty(
            &app,
            site_a_session,
            &fail_a,
            true,
            WakeDeliveryOrigin::FilesystemPoller,
            false,
            SettleReadiness::Unknown,
        )
        .await
        .expect_err("real PTY write failure must surface");
    assert!(
        !crate::pty::menu_guard::is_menu_guard_deferred_error(&err_a),
        "a real write failure must not be classified as deferred: {err_a}"
    );
    assert!(err_a.contains("PTY write failed"), "{err_a}");

    let mut fail_b = wake_message_to_target();
    fail_b.id = "1883-b-fail".to_string();
    let err_b = poller
        .inject_into_pty(
            &app,
            site_b_session,
            &fail_b,
            true,
            WakeDeliveryOrigin::FilesystemPoller,
            false,
            SettleReadiness::Unknown,
        )
        .await
        .expect_err("real PTY write failure must surface");
    assert!(
        !crate::pty::menu_guard::is_menu_guard_deferred_error(&err_b),
        "a real write failure must not be classified as deferred: {err_b}"
    );
    assert!(err_b.contains("PTY write failed"), "{err_b}");

    let fail_entries = crate::logging::error_sink().drain();
    for canary in ["1883-a-fail", "1883-b-fail"] {
        assert!(
            fail_entries.iter().any(|entry| {
                entry.level == "ERROR"
                    && entry.message.contains(canary)
                    && entry.message.contains("[mailbox] PTY injection FAILED")
            }),
            "site {canary} must log FAILED at ERROR; captured: {fail_entries:?}"
        );
    }
}

async fn assert_wired_clear_and_compact_submission(clear_shell: &str, compact_shell: &str) {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let clear_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "wired-clear",
        clear_shell,
        SessionStatus::Idle,
    )
    .await;
    let compact_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "wired-compact",
        compact_shell,
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, clear_id);
    register_mock_pty_route(&app, compact_id);
    let clear_message = logical_command_message("clear", "");
    let compact_message = logical_command_message("compact", "");
    let poller = MailboxPoller::new();
    let started = std::time::Instant::now();

    let (clear_result, compact_result) = tokio::join!(
        poller.inject_into_pty(
            &app,
            clear_id,
            &clear_message,
            true,
            WakeDeliveryOrigin::FilesystemPoller,
            false,
            SettleReadiness::Unknown,
        ),
        poller.inject_into_pty(
            &app,
            compact_id,
            &compact_message,
            true,
            WakeDeliveryOrigin::FilesystemPoller,
            false,
            SettleReadiness::Unknown,
        )
    );
    clear_result.unwrap();
    compact_result.unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(2000),
        "wired submissions must await both canonical delayed Enter writes"
    );

    assert_eq!(
        mock_pty_writes_for(&app, clear_id),
        vec![b"/clear".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
    assert_eq!(
        mock_pty_writes_for(&app, compact_id),
        vec![b"/compact".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    assert!(
        manager
            .get_session(clear_id)
            .await
            .unwrap()
            .start_fresh_on_restore
    );
    assert!(
        !manager
            .get_session(compact_id)
            .await
            .unwrap()
            .start_fresh_on_restore
    );
}

#[tokio::test]
async fn remote_established_command_branches_preserve_text_and_submission() {
    tokio::join!(
        assert_wired_clear_and_compact_submission("claude.exe", "claude-wrapper.cmd"),
        assert_wired_clear_and_compact_submission("codex.exe", "codex-wrapper.cmd"),
        assert_wired_clear_and_compact_submission("agy.exe", "antigravity.exe")
    );
}

#[tokio::test]
async fn remote_cursor_command_branches_preserve_text_and_submission() {
    assert_wired_clear_and_compact_submission("agent.exe", "agent.cmd").await;
}

#[tokio::test]
async fn pi_canonical_injector_writes_arbitrary_text_then_two_enters() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-live",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, session_id);

    crate::pty::inject::inject_text_into_session(&app, session_id, "arbitrary Pi payload")
        .await
        .unwrap();

    assert_eq!(
        mock_pty_writes_for(&app, session_id),
        vec![
            b"arbitrary Pi payload".to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec()
        ]
    );
}

#[tokio::test]
async fn remote_pi_compact_existing_session_has_no_actuation() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-live",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, session_id);
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&events);
    fixture.app.listen_any("message_delivered", move |event| {
        captured.lock().unwrap().push(event.payload().to_string());
    });
    let message = logical_command_message("compact", "must not follow up");
    let poller = MailboxPoller::new();

    let error = poller
        .inject_into_pty(
            &app,
            session_id,
            &message,
            true,
            WakeDeliveryOrigin::FilesystemPoller,
            false,
            SettleReadiness::Unknown,
        )
        .await
        .unwrap_err();

    assert_eq!(
        error,
        "Cannot execute logical remote command 'compact': session shell 'pi' has no verified \
            mapping. Claude / Codex / Antigravity / Cursor agent direct shells use /clear and \
            /compact; exact Pi uses /new for clear only. cmd / pwsh outer wrappers and Pi compact \
            are unsupported."
    );
    assert!(mock_pty_writes_for(&app, session_id).is_empty());
    assert!(events.lock().unwrap().is_empty());
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    assert!(
        !manager
            .get_session(session_id)
            .await
            .unwrap()
            .start_fresh_on_restore
    );
}

#[tokio::test]
async fn deliver_wake_terminal_commands_preflight_before_lifecycle() {
    // Live Pi compact rejects before settle or injection.
    let live_fixture = make_mailbox_fixture();
    let live_app = app_handle(&live_fixture.app);
    let live_id = add_mailbox_session_with_shell(
        &live_app,
        &live_fixture.target_cwd,
        "pi-live",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    let live_hooks = MailboxTestHooks::default();
    live_hooks
        .pty_presence
        .lock()
        .unwrap()
        .insert(live_id, true);
    let live_poller = MailboxPoller::new_with_test_hooks(live_hooks.clone());
    let live_error = live_poller
        .deliver_wake_with_origin(
            &live_app,
            &logical_command_message("compact", "follow-up"),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap_err();
    assert!(live_error.starts_with(ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND));
    assert!(live_hooks.settle_calls.lock().unwrap().is_empty());
    assert!(live_hooks.events.lock().unwrap().is_empty());

    // No-session Pi compact rejects before spawn.
    let cold_fixture = make_mailbox_fixture();
    let cold_app = app_handle(&cold_fixture.app);
    {
        let settings = cold_app.state::<SettingsState>();
        settings.write().await.agents = vec![wake_agent("pi", "Pi", "pi")];
    }
    let cold_hooks = MailboxTestHooks::default();
    let cold_poller = MailboxPoller::new_with_test_hooks(cold_hooks.clone());
    let mut cold_message = logical_command_message("compact", "follow-up");
    cold_message.preferred_agent = "pi".to_string();
    let cold_error = cold_poller
        .deliver_wake_with_origin(
            &cold_app,
            &cold_message,
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap_err();
    assert!(cold_error.starts_with(ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND));
    assert!(cold_hooks.events.lock().unwrap().is_empty());

    // An Exited Pi record is left intact when the carried spawn shell has no mapping.
    let exited_fixture = make_mailbox_fixture();
    let exited_app = app_handle(&exited_fixture.app);
    {
        let settings = exited_app.state::<SettingsState>();
        settings.write().await.agents = vec![wake_agent("pi", "Pi", "pi")];
    }
    let exited_id = add_mailbox_session_with_shell(
        &exited_app,
        &exited_fixture.target_cwd,
        "pi-exited",
        "pi",
        SessionStatus::Exited(0),
    )
    .await;
    let exited_hooks = MailboxTestHooks::default();
    exited_hooks
        .pty_presence
        .lock()
        .unwrap()
        .insert(exited_id, false);
    let exited_poller = MailboxPoller::new_with_test_hooks(exited_hooks.clone());
    let mut exited_message = logical_command_message("compact", "follow-up");
    exited_message.preferred_agent = "pi".to_string();
    let exited_error = exited_poller
        .deliver_wake_with_origin(
            &exited_app,
            &exited_message,
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap_err();
    assert!(exited_error.starts_with(ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND));
    assert!(exited_hooks.events.lock().unwrap().is_empty());
    let exited_manager = {
        let state = exited_app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    assert!(exited_manager.get_session(exited_id).await.is_some());

    // Unknown wire input is parsed before candidate settling.
    let unknown_fixture = make_mailbox_fixture();
    let unknown_app = app_handle(&unknown_fixture.app);
    let unknown_id = add_mailbox_session_with_shell(
        &unknown_app,
        &unknown_fixture.target_cwd,
        "pi-live",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    let unknown_hooks = MailboxTestHooks::default();
    unknown_hooks
        .pty_presence
        .lock()
        .unwrap()
        .insert(unknown_id, true);
    let unknown_poller = MailboxPoller::new_with_test_hooks(unknown_hooks.clone());
    let unknown_error = unknown_poller
        .deliver_wake_with_origin(
            &unknown_app,
            &logical_command_message("Clear", ""),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap_err();
    assert_eq!(
        unknown_error,
        "Unsupported logical remote command 'Clear'. Allowed values: clear, compact"
    );
    assert!(unknown_hooks.settle_calls.lock().unwrap().is_empty());
    assert!(unknown_hooks.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn deliver_wake_logical_command_session_race_continues_to_next_candidate() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let vanished_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-vanishes-after-preflight",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    let surviving_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-survives",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, vanished_id);
    register_mock_pty_route(&app, surviving_id);

    let hooks = MailboxTestHooks::default();
    {
        let mut presence = hooks.pty_presence.lock().unwrap();
        presence.insert(vanished_id, true);
        presence.insert(surviving_id, true);
    }
    hooks
        .remove_session_on_settle
        .lock()
        .unwrap()
        .insert(vanished_id);
    {
        let mut real = hooks.real_inject_sessions.lock().unwrap();
        real.insert(vanished_id);
        real.insert(surviving_id);
    }
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    poller
        .deliver_wake_with_origin(
            &app,
            &logical_command_message("clear", ""),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();

    assert_eq!(
        *hooks.settle_calls.lock().unwrap(),
        vec![vanished_id, surviving_id],
        "the missing-record error must continue the same delivery attempt"
    );
    assert_eq!(
        *hooks.inject_calls.lock().unwrap(),
        vec![vanished_id, surviving_id]
    );
    assert!(mock_pty_writes_for(&app, vanished_id).is_empty());
    assert_eq!(
        mock_pty_writes_for(&app, surviving_id),
        vec![b"/new".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    assert!(manager.get_session(vanished_id).await.is_none());
    assert!(
        manager
            .get_session(surviving_id)
            .await
            .unwrap()
            .start_fresh_on_restore
    );
}

#[tokio::test]
async fn deliver_wake_supported_established_command_keeps_spawn_path() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let message = logical_command_message("clear", "");

    poller
        .deliver_wake_with_origin(&app, &message, WakeDeliveryOrigin::FilesystemPoller)
        .await
        .unwrap();

    assert_eq!(hooks.spawn_calls.lock().unwrap().len(), 1);
    assert_eq!(hooks.inject_calls.lock().unwrap().len(), 1);
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
}

// ── #1635-2 wake dispatch honoring: agent+profile at spawn, receipt, no-write ──

/// Install profile cells for the fixture's agents (codex/claude). Cells map
/// agent id -> (letter, enabled); the cell command is `codex --<lower>` so
/// the composed spawn command (`<base> <cell>`) visibly carries the
/// effective cell letter.
async fn configure_profile_cells(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    cells: &[(&str, &[(&str, bool)])],
) {
    let settings = app.state::<SettingsState>();
    let mut guard = settings.write().await;
    guard.coding_agent_profiles.profile_slots = std::collections::BTreeMap::from([
        (
            "A".to_string(),
            crate::config::settings::ProfileSlotConfig {
                label: String::new(),
            },
        ),
        (
            "B".to_string(),
            crate::config::settings::ProfileSlotConfig {
                label: String::new(),
            },
        ),
        (
            "C".to_string(),
            crate::config::settings::ProfileSlotConfig {
                label: String::new(),
            },
        ),
    ]);
    guard.coding_agent_profiles.profiles_by_agent = cells
        .iter()
        .map(|(agent_id, letters)| {
            (
                (*agent_id).to_string(),
                letters
                    .iter()
                    .map(|(letter, enabled)| {
                        (
                            (*letter).to_string(),
                            crate::config::settings::ProfileCellConfig {
                                id_hash: None,
                                id_hash_debug: None,
                                preflight: None,
                                preflight_timeout_seconds: None,
                                enabled: *enabled,
                                command: format!("codex --{}", letter.to_ascii_lowercase()),
                                env: std::collections::BTreeMap::new(),
                                notes: String::new(),
                            },
                        )
                    })
                    .collect(),
            )
        })
        .collect();
}

fn assert_effective_agent_and_profile(delivered: &OutboxMessage, agent: &str, profile: &str) {
    assert_eq!(delivered.effective_agent_id.as_deref(), Some(agent));
    assert_eq!(delivered.effective_profile.as_deref(), Some(profile));
    assert!(!delivered.profile_fallback_applied);
    assert!(delivered.dispatch_not_applied.is_none());
}

#[tokio::test]
async fn wake_dispatch_agent_and_profile_honored_on_cold_spawn() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    configure_profile_cells(&app, &[("codex", &[("A", true), ("B", true), ("C", true)])]).await;
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    let delivered = poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("codex", Some("C")),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();

    let calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.to, CANONICAL_WAKE_TO);
    assert_eq!(call.shell, "codex");
    assert!(call.shell_args.contains(&"--c".to_string()));
    assert!(call.skip_tooling_save);
    assert_eq!(hooks.inject_calls.lock().unwrap().len(), 1);
    assert_effective_agent_and_profile(&delivered, "codex", "C");
}

#[tokio::test]
async fn wake_dispatch_profile_falls_back_to_nearest_enabled_cell() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    // C is disabled: the walk must fall back to the nearest enabled cell (B).
    configure_profile_cells(
        &app,
        &[("codex", &[("A", true), ("B", true), ("C", false)])],
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    let delivered = poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("codex", Some("C")),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();

    let calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].shell_args.contains(&"--b".to_string()));
    assert_eq!(delivered.effective_agent_id.as_deref(), Some("codex"));
    assert_eq!(delivered.effective_profile.as_deref(), Some("B"));
    assert!(delivered.profile_fallback_applied);
    assert!(delivered.dispatch_not_applied.is_none());
}

#[tokio::test]
async fn wake_dispatch_profile_without_agent_applies_to_auto_resolution() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    configure_profile_cells(&app, &[("codex", &[("A", true), ("B", true), ("C", true)])]).await;
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    // auto resolution: no currentCodingAgent/lastCodingAgent on the replica,
    // so the sender agent (codex) picks the row; the requested letter C
    // applies to that row.
    let mut message = dispatch_wake_message("auto", Some("C"));
    message.sender_agent = Some("codex".to_string());

    let delivered = poller
        .deliver_wake_with_origin(&app, &message, WakeDeliveryOrigin::FilesystemPoller)
        .await
        .unwrap();

    let calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].shell, "codex");
    assert!(calls[0].shell_args.contains(&"--c".to_string()));
    assert!(calls[0].skip_tooling_save);
    assert_effective_agent_and_profile(&delivered, "codex", "C");
}

#[tokio::test]
async fn wake_dispatch_outranks_replica_pin() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    // Replica pinned to tooling.profile=B.
    std::fs::write(
        fixture.target_cwd.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","tooling":{"profile":"B"}}"#,
    )
    .unwrap();
    configure_profile_cells(&app, &[("codex", &[("A", true), ("B", true), ("C", true)])]).await;
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    let delivered = poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("codex", Some("C")),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();

    assert_effective_agent_and_profile(&delivered, "codex", "C");
    let calls = hooks.spawn_calls.lock().unwrap().clone();
    assert!(calls[0].shell_args.contains(&"--c".to_string()));
    // The pin is untouched on disk (the hook never writes, and the flagged
    // spawn suppresses tooling writes anyway).
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.target_cwd.join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["tooling"]["profile"], "B");
}

#[tokio::test]
async fn wake_dispatch_respawn_of_exited_session_honors_flags() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    configure_profile_cells(&app, &[("codex", &[("A", true), ("B", true), ("C", true)])]).await;
    let exited_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "exited",
        SessionStatus::Exited(0),
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    let delivered = poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("codex", Some("C")),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();

    assert_eq!(hooks.destroy_calls.lock().unwrap().as_slice(), &[exited_id]);
    let calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    // Respawn path: auto-resume stays enabled (spawn_with_resume true).
    assert!(!calls[0].skip_auto_resume);
    assert!(calls[0].shell_args.contains(&"--c".to_string()));
    assert!(calls[0].skip_tooling_save);
    assert_effective_agent_and_profile(&delivered, "codex", "C");
}
