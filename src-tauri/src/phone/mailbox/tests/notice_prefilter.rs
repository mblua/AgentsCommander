use super::*;

/// #1682 - both privileged planes cancel a standing control-write mark when
/// they arm a turn: (a) the injection funnel, which clears through
/// `commands::pty::clear_control_write_mark`, and (b) the exact-agent-input
/// boundary, which clears through `IdleDetector::clear_control_write` directly.
/// Each half asserts the mark IS standing first; that pre-assertion is the
/// in-test control, without which the half passes on a mark never set.
#[tokio::test]
async fn arming_a_turn_clears_the_control_write_mark_on_both_privileged_planes() {
    use crate::pty::backend::SessionBackendKind;
    use crate::session::profile::IdleTuning;

    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);

    // (a) The injection plane.
    let inject_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-live",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, inject_id);
    {
        let idle = app
            .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
            .inner()
            .clone();
        idle.register_session(inject_id, IdleTuning::DEFAULT);
        idle.set_pty_input_ready_for_test(inject_id);
        crate::commands::pty::mark_successful_pty_write_busy(&app, inject_id, 3).await;
        assert!(
            idle.control_write_age(inject_id).is_some(),
            "the control: a PTY write into the idle session must mark it"
        );

        crate::pty::inject::inject_text_into_session(&app, inject_id, "arbitrary payload")
            .await
            .unwrap();
        assert!(
            idle.control_write_age(inject_id).is_none(),
            "the injection funnel arms the turn and must cancel the mark"
        );
    }

    // (b) The exact-agent-input plane. Setup copied from
    // `pty_input_boundary_yields_exactly_one_busy_record`.
    let sender_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "coordinator",
        SessionStatus::Running,
        None,
    )
    .await;
    let target_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "member",
        SessionStatus::Idle,
        None,
    )
    .await;
    let paths = vec![fixture._temp.path().to_string_lossy().to_string()];
    let route = crate::config::teams::verify_pty_input_route(
        &fixture.sender_cwd,
        false,
        CANONICAL_WAKE_TO,
        &paths,
    )
    .expect("verified route");
    let target_cwd_identity =
        crate::path_identity::verify_directory(&fixture.target_cwd).expect("target identity");
    let sender_cwd_identity =
        crate::path_identity::verify_directory(&fixture.sender_cwd).expect("sender identity");
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            target_id,
            SessionBackendKind::LocalProcess,
            Some(target_cwd_identity),
            Some(route.target.replica_identity.clone()),
        )
        .expect("record target route");
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            sender_id,
            SessionBackendKind::LocalProcess,
            Some(sender_cwd_identity),
            Some(route.sender.replica_identity.clone()),
        )
        .expect("record sender route");
    let authority_route =
        PtyManager::authority_route_proof(&pty, sender_id).expect("sender authority proof");
    let permit = PtyManager::acquire_input_writer(&pty, target_id)
        .await
        .expect("target permit");
    let idle = app
        .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
        .inner()
        .clone();
    idle.register_session(target_id, IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(target_id);
    let sessions = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let settings = app.state::<SettingsState>().inner().clone();
    fn current_recipe(session: &crate::session::session::Session, settings: &AppSettings) -> bool {
        session_has_current_pty_submission_provenance(&SessionInfo::from(session), settings)
    }

    crate::commands::pty::mark_successful_pty_write_busy(&app, target_id, 3).await;
    assert!(
        idle.control_write_age(target_id).is_some(),
        "the control: a preceding control write into the idle target must mark it"
    );
    // That helper necessarily destroys BOTH readiness legs the boundary
    // requires (`mark_busy` clears `waiting_for_input`, and
    // `record_activity_with_bytes` stamps `activity` and drops the session from
    // `idle_set`). Restore the pre-write state; neither call touches the mark.
    sessions.mark_idle(target_id).await;
    idle.set_pty_input_ready_for_test(target_id);

    let boundary_guard = sessions
        .prepare_pty_input_boundary(
            target_id,
            &route.target,
            SessionBackendKind::LocalProcess,
            sender_id,
            &route.sender,
            SessionBackendKind::LocalProcess,
            &authority_route,
            &permit,
            &idle,
            &settings,
            current_recipe,
            || true,
        )
        .await
        .expect("the boundary must succeed");
    drop(boundary_guard);

    assert!(
        idle.control_write_age(target_id).is_none(),
        "the exact-agent-input boundary arms the turn and must cancel the mark"
    );
}

#[test]
fn issue_2113_prefilter_matches_case_differing_project_segment() {
    let cwd = "D:/0_repos/Proj/.ac/room-1-team/__agent_bob";
    let target = "proj:room-1-team/bob";
    assert_ne!(
        crate::config::teams::agent_fqn_from_path(cwd),
        target,
        "the exact compare the call sites used to make must drop the case-skewed CWD"
    );
    assert!(internal_target_fqn_prefilter_matches(cwd, target));
}

#[test]
fn issue_2113_prefilter_matches_case_differing_room_suffix_and_agent_segments() {
    assert!(internal_target_fqn_prefilter_matches(
        "D:/0_repos/proj/.ac/room-1-Team/__agent_Bob",
        "proj:room-1-team/bob"
    ));
    // Out-of-scope boundary: a case-differing `room-` prefix fails
    // `has_entity_prefix`, so `agent_fqn_from_path` returns the
    // two-segment fallback `Room-1-Team/Bob` and folding cannot align it.
    assert!(!internal_target_fqn_prefilter_matches(
        "D:/0_repos/proj/.ac/Room-1-Team/__agent_Bob",
        "proj:room-1-team/bob"
    ));
}

#[test]
fn issue_2113_prefilter_still_rejects_a_different_agent() {
    let cwd = "D:/0_repos/Proj/.ac/room-1-team/__agent_bob";
    assert!(!internal_target_fqn_prefilter_matches(
        cwd,
        "proj:room-1-team/alice"
    ));
    assert!(!internal_target_fqn_prefilter_matches(
        cwd,
        "other:room-1-team/bob"
    ));
}

#[test]
fn issue_2113_prefilter_matches_exactly_spelled_fqn() {
    assert!(internal_target_fqn_prefilter_matches(
        "D:/0_repos/proj/.ac/room-1-team/__agent_bob",
        "proj:room-1-team/bob"
    ));
}

#[test]
fn issue_2113_prefilter_folds_non_ascii_segments() {
    assert!(internal_target_fqn_prefilter_matches(
        "D:/0_repos/proj/.ac/room-1-team/__agent_ÑOÑO",
        "proj:room-1-team/ñoño"
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn issue_2113_canonical_gate_separates_two_case_distinct_linux_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).expect("canonicalize temp root");
    let room = root.join("proj").join(".ac").join("room-1-team");
    let lower = room.join("__agent_bob");
    let upper = room.join("__agent_Bob");
    std::fs::create_dir_all(&lower).unwrap();
    std::fs::create_dir_all(&upper).unwrap();
    assert_ne!(
        std::fs::canonicalize(&lower).unwrap(),
        std::fs::canonicalize(&upper).unwrap(),
        "this control requires a case-sensitive filesystem"
    );
    let replica = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(&lower).unwrap(),
    );
    let lower_cwd = lower.to_string_lossy().to_string();
    let upper_cwd = upper.to_string_lossy().to_string();
    // The pre-filter is over-wide on purpose: both spellings pass it.
    assert!(internal_target_fqn_prefilter_matches(
        &lower_cwd,
        "proj:room-1-team/bob"
    ));
    assert!(internal_target_fqn_prefilter_matches(
        &upper_cwd,
        "proj:room-1-team/bob"
    ));
    // The canonical gate is the authority: exactly the real replica passes.
    assert!(canonical_cwd_owned_by_replica(&lower_cwd, &replica).unwrap());
    assert!(!canonical_cwd_owned_by_replica(&upper_cwd, &replica).unwrap());
}

#[cfg(windows)]
#[tokio::test]
async fn issue_2113_case_differing_project_segment_still_injects_live() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let project = fixture.sender_cwd.ancestors().nth(3).unwrap();
    let skewed = project
        .parent()
        .unwrap()
        .join("Proj-A")
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_tech-lead");
    assert_eq!(
        std::fs::canonicalize(&skewed).unwrap(),
        std::fs::canonicalize(&fixture.sender_cwd).unwrap(),
        "this control requires one real directory reachable by two spellings"
    );
    let id = add_mailbox_session(
        &app,
        &skewed,
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
    assert_ne!(
        crate::config::teams::agent_fqn_from_path(&skewed.to_string_lossy()),
        target.fqn(),
        "the exact compare the call sites used to make must drop this CWD"
    );
    let notice = InternalSystemNotice::for_context_alert(
        "dev-rust".to_string(),
        "wg-1-dev-team".to_string(),
        80,
        vec![50, 75],
    )
    .unwrap();
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(id, true);
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

    assert_eq!(hooks.inject_calls.lock().unwrap().as_slice(), &[id]);
    assert_no_spawn_or_destroy_events(&hooks);
}

#[cfg(windows)]
#[tokio::test]
async fn issue_2113_case_differing_project_segment_destroys_exited_orchestrator() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let project = fixture.sender_cwd.ancestors().nth(3).unwrap();
    let skewed = project
        .parent()
        .unwrap()
        .join("Proj-A")
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_tech-lead");
    assert_eq!(
        std::fs::canonicalize(&skewed).unwrap(),
        std::fs::canonicalize(&fixture.sender_cwd).unwrap(),
        "this control requires one real directory reachable by two spellings"
    );
    let exited_id = add_mailbox_session(
        &app,
        &skewed,
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
    let events = hooks.events.lock().unwrap();
    assert!(matches!(events[0], MailboxTestEvent::Destroy(id) if id == exited_id));
    assert!(matches!(events[1], MailboxTestEvent::Spawn(_)));
    assert!(matches!(events[2], MailboxTestEvent::Inject(_)));
}

/// #2129 - T5: a qualified base reaches the sentence unchanged. The value
/// is a literal here on purpose: `render_with_template` is pure token
/// substitution, so this half cannot depend on how the field was produced.
#[test]
fn issue_2129_branch_stale_line_renders_a_qualified_base() {
    let resolved = remote_activity_notice(
        RemoteNoticeKind::BranchStale,
        "main on GitHub",
        Some(4),
        None,
    )
    .expect("a branch-stale notice")
    .line();
    assert_eq!(
        resolved,
        "[AgentsCommander] mblua/AgentsCommander feature/2083-2064-remote-alerts is now 4 \
            commits behind main on GitHub as of 2026-09-15 23:18:43-03:00. If CI runs on this \
            branch, it is validating an out-of-date base."
    );

    let sentinel = remote_activity_notice(
        RemoteNoticeKind::BranchStale,
        "the default branch on GitHub",
        Some(4),
        None,
    )
    .expect("a branch-stale notice")
    .line();
    assert_eq!(
        sentinel,
        "[AgentsCommander] mblua/AgentsCommander feature/2083-2064-remote-alerts is now 4 \
            commits behind the default branch on GitHub as of 2026-09-15 23:18:43-03:00. If CI \
            runs on this branch, it is validating an out-of-date base."
    );

    // The helper hardcodes the branch, so the branch == base case is built
    // directly.
    let counterpart = InternalSystemNotice::for_remote_activity(
        RemoteNoticeKind::BranchStale,
        "mblua/AgentsCommander".to_string(),
        "main".to_string(),
        "abcdef1".to_string(),
        "its counterpart on GitHub".to_string(),
        Some(4),
        "2026-09-15 23:18:43-03:00".to_string(),
        None,
    )
    .expect("a branch-stale notice")
    .line();
    assert_eq!(
        counterpart,
        "[AgentsCommander] mblua/AgentsCommander main is now 4 commits behind its counterpart \
            on GitHub as of 2026-09-15 23:18:43-03:00. If CI runs on this branch, it is validating \
            an out-of-date base."
    );
    assert_eq!(counterpart.matches("main").count(), 1);
}

// ── #2232 phase 7: the Co-managed origin, end to end ──────────────────

fn comanaged_origin_message(
    id: &str,
    action: Option<&str>,
    command: Option<&str>,
) -> OutboxMessage {
    OutboxMessage {
        id: id.to_string(),
        token: None,
        from: CANONICAL_WAKE_FROM.to_string(),
        to: CANONICAL_WAKE_TO.to_string(),
        body: WAKE_BODY.to_string(),
        mode: "wake".to_string(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: "auto".to_string(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: "2026-09-21T00:00:00Z".to_string(),
        command: command.map(str::to_string),
        action: action.map(str::to_string),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

/// Test 7: the same body and the same `from` written into an agent's own
/// replica outbox is delivered with **no** suffix; only the Co-managed
/// origin composes one. This is the impersonation property, measured on
/// the bytes that actually reach the recipient's PTY.
#[tokio::test]
async fn replica_delivery_carries_no_comanaged_suffix() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let target = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "replica-target",
        "codex",
        SessionStatus::Running,
    )
    .await;
    register_mock_pty_route(&app, target);
    let hooks = MailboxTestHooks::default();
    hooks.real_inject_sessions.lock().unwrap().insert(target);
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    let path = write_wake_outbox_message(&fixture.sender_cwd, "impersonation-check");
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .expect("replica message processes");

    let writes = mock_pty_writes_for(&app, target);
    assert!(!writes.is_empty(), "the replica wake must reach the PTY");
    let payload = String::from_utf8(writes[0].clone()).expect("utf-8 payload");
    assert!(
        payload.contains(&format!("[Message from {CANONICAL_WAKE_FROM}]")),
        "the exact FQN is displayed as-is for a replica origin: {payload:?}"
    );
    assert!(
        !payload.contains(crate::phone::messaging::CO_MANAGED_SENDER_SUFFIX),
        "a replica must never carry the Co-managed suffix: {payload:?}"
    );
}

/// Test 18: the Co-managed origin rejects `action` and `command`; the same
/// document from an agent replica outbox keeps today's behaviour. Two
/// assertions, so the rejection is proven origin-scoped.
#[tokio::test]
async fn comanaged_origin_rejects_action_and_command_while_replica_is_unchanged() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    // (a) The Co-managed queue: `action` is rejected before dispatch.
    let queue_dir = fixture
        .sender_cwd
        .parent()
        .unwrap()
        .join(".co-managed")
        .join("queue");
    std::fs::create_dir_all(&queue_dir).unwrap();
    let action_msg = comanaged_origin_message("co-managed-action", Some("close-session"), None);
    let queue_file = queue_dir.join("co-managed-action.json");
    std::fs::write(
        &queue_file,
        serde_json::to_string_pretty(&action_msg).unwrap(),
    )
    .unwrap();
    poller
        .process_message(&app, &queue_file, OutboxOrigin::CoManaged)
        .await
        .expect("the rejection is a settled outcome");
    let reason = std::fs::read_to_string(
        queue_dir
            .join("rejected")
            .join("co-managed-action.reason.txt"),
    )
    .expect("the rejected reason file");
    assert!(
        reason.contains("Co-managed origin may carry only a wake"),
        "{reason}"
    );
    assert!(
        hooks.inject_calls.lock().unwrap().is_empty(),
        "a rejected Co-managed action is never delivered"
    );

    // (a2) `command` is rejected the same way.
    let command_msg = comanaged_origin_message("co-managed-command", None, Some("clear"));
    let command_file = queue_dir.join("co-managed-command.json");
    std::fs::write(
        &command_file,
        serde_json::to_string_pretty(&command_msg).unwrap(),
    )
    .unwrap();
    poller
        .process_message(&app, &command_file, OutboxOrigin::CoManaged)
        .await
        .expect("the rejection is a settled outcome");
    let reason = std::fs::read_to_string(
        queue_dir
            .join("rejected")
            .join("co-managed-command.reason.txt"),
    )
    .expect("the rejected reason file");
    assert!(
        reason.contains("Co-managed origin may carry only a wake"),
        "{reason}"
    );

    // (a3) A privileged PTY-input envelope is not a wake either. Drive the
    // PRODUCTION sweep (`poll`), not the test-only `process_message`
    // mirror, so deleting the CoManaged arm in the sweep fails this test.
    // The production sweep discovers queues from `room-*`-shaped roots
    // (`room_root_for_path`), so the file goes in a `room-` room next to
    // the legacy fixture, with a live session that makes `poll` scan it.
    let sweep_room = fixture
        .sender_cwd
        .parent()
        .and_then(|wg_dir| wg_dir.parent())
        .unwrap()
        .join("room-1-dev-team");
    let sweep_cwd = sweep_room.join("__agent_tech-lead");
    std::fs::create_dir_all(&sweep_cwd).unwrap();
    let sweep_queue = crate::config::co_managed::queue_dir(&sweep_room);
    std::fs::create_dir_all(&sweep_queue).unwrap();
    let privileged = serde_json::json!({
        "id": "co-managed-pty",
        "token": null,
        "from": CANONICAL_WAKE_FROM,
        "to": CANONICAL_WAKE_TO,
        "body": WAKE_BODY,
        "mode": "wake",
        "ptyInput": {}
    });
    let privileged_file = sweep_queue.join("co-managed-pty.json");
    std::fs::write(
        &privileged_file,
        serde_json::to_string_pretty(&privileged).unwrap(),
    )
    .unwrap();
    add_mailbox_session_with_shell(
        &app,
        &sweep_cwd,
        "queue-room-session",
        "codex",
        SessionStatus::Running,
    )
    .await;
    poller.poll(&app).await.expect("a production sweep cycle");
    let reason = std::fs::read_to_string(
        sweep_queue
            .join("rejected")
            .join("co-managed-pty.reason.txt"),
    )
    .expect("the rejected reason file");
    assert!(
        reason.contains("privileged PTY input is rejected"),
        "{reason}"
    );
    assert!(
        !privileged_file.exists(),
        "the sweep settles the file out of the queue"
    );
    assert!(hooks.inject_calls.lock().unwrap().is_empty());

    // (b) The same document from the sender's own replica outbox goes
    // through today's chain: the Co-managed rejection text is absent.
    let replica_path = write_wake_outbox_message_with_route(
        &fixture.sender_cwd,
        "replica-action",
        CANONICAL_WAKE_FROM,
        CANONICAL_WAKE_TO,
    );
    let mut replica_msg: OutboxMessage =
        serde_json::from_str(&std::fs::read_to_string(&replica_path).unwrap()).unwrap();
    replica_msg.action = Some("close-session".to_string());
    std::fs::write(
        &replica_path,
        serde_json::to_string_pretty(&replica_msg).unwrap(),
    )
    .unwrap();
    let replica_result = poller
        .process_message(&app, &replica_path, OutboxOrigin::Replica)
        .await;
    if let Err(error) = &replica_result {
        assert!(
            !error.contains("Co-managed origin"),
            "the rejection must be origin-scoped, got: {error}"
        );
    }
    let replica_reason = std::fs::read_to_string(
        fixture
            .sender_cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("rejected")
            .join("replica-action.reason.txt"),
    )
    .unwrap_or_default();
    assert!(
        !replica_reason.contains("Co-managed origin"),
        "the rejection must be origin-scoped, got: {replica_reason}"
    );
}

// #2716 (B3, option C) - rows E35, E36 and E38 of the phase plan.

fn b3_unreadable(path: &Path) -> crate::config::settings::AgentsLayerState {
    crate::config::settings::AgentsLayerState::Unreadable {
        path: path.to_path_buf(),
        reason: "test".to_string(),
    }
}

/// E35: a wake for a configured agent under the mark answers the refusal,
/// which is the value the reply is built from, instead of a bare debug log.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_telegram_wake_reports_the_refusal() {
    let _guard = crate::cli::create_agent::SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let fixture = make_session_request_fixture();
    let app = app_handle(&fixture.app);
    let sidecar = fixture.cwd.join("agents.30.instance.no-git.json");
    app.state::<SettingsState>().write().await.agents_layer = b3_unreadable(&sidecar);
    let mut msg = root_outbox_message("wake".to_string(), None);
    msg.sender_agent = Some(wake_agents()[0].id.clone());

    let result = MailboxPoller::new().resolve_agent_command(&app, &msg).await;

    assert_eq!(
        result.err(),
        Some(crate::config::settings::agents_unreadable_session_error(
            &sidecar
        ))
    );
    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

/// E36: the daemon's response file carries the refusal, so the waiting CLI
/// verb prints it, and no session is created.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn the_daemon_response_file_carries_the_refusal() {
    let _guard = crate::cli::create_agent::SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let fixture = make_session_request_fixture();
    let app = app_handle(&fixture.app);
    let sidecar = fixture.cwd.join("agents.30.instance.no-git.json");
    app.state::<SettingsState>().write().await.agents_layer = b3_unreadable(&sidecar);
    let request = write_session_request_for_test(&fixture.cwd, "codex");

    MailboxPoller::new().poll_session_requests(&app).await;

    let result = read_session_request_result_json(&request.id);
    assert_eq!(result["status"], "rejected", "unexpected result: {result}");
    let error = result["error"].as_str().expect("error text");
    assert!(
        error.contains(&crate::config::settings::agents_unreadable_session_error(
            &sidecar
        )),
        "{error}"
    );
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    assert!(session_mgr.read().await.list_sessions().await.is_empty());
    assert!(fixture.backend.live.lock().unwrap().is_empty());
    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

/// E38: the provenance read is a comparison, not a spawn, so the mark does
/// not turn it into a failure.
#[test]
fn the_provenance_read_is_exempt_from_the_refusal() {
    let cwd = tempfile::tempdir().unwrap();
    let mut settings = AppSettings {
        agents: vec![wake_agent(
            "codex",
            "Codex wrapper",
            "codex-shell.exe --yolo",
        )],
        ..Default::default()
    };
    let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
        &settings,
        "codex",
        &cwd.path().to_string_lossy(),
        None,
    )
    .unwrap()
    .unwrap();
    let mut session = make_session_info(
        "wrapper",
        "member",
        &cwd.path().to_string_lossy(),
        SessionStatus::Idle,
        true,
    );
    session.shell = spawn.shell.clone();
    session.shell_args = spawn.shell_args.clone();
    session.agent_id = Some(spawn.trusted_agent_id.clone());
    session.agent_kind = Some(crate::session::profile::CodingAgentKind::Codex);
    session.profile_content_hash = Some(spawn.profile_content_hash.clone());
    session.trusted_configured_spawn = true;
    settings.agents_layer = b3_unreadable(&cwd.path().join("agents.30.instance.no-git.json"));

    assert!(
        session_has_current_pty_submission_provenance(&session, &settings),
        "the provenance read failed under the mark"
    );
}

/// #2786 (C1) - the PTY-input state read keeps the config.json guard.
#[test]
fn guarded_agent_state_read_rejects_unsafe_input() {
    use crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME;
    use crate::phone::types::PtyInputReasonCode as C;

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    assert_eq!(
        read_guarded_agent_state(root),
        Ok(None),
        "absent is absence"
    );

    let state = root.join(CONFIG_STATE_TARGET_NAME);
    std::fs::write(&state, r#"{"tooling":{"lastCodingAgent":"claude"}}"#).expect("seed");
    assert_eq!(
        read_guarded_agent_state(root),
        Ok(Some(
            serde_json::json!({"tooling":{"lastCodingAgent":"claude"}})
        ))
    );

    std::fs::write(
        &state,
        r#"{"tooling":{"lastCodingAgent":"a","lastCodingAgent":"b"}}"#,
    )
    .expect("seed duplicate");
    assert_eq!(read_guarded_agent_state(root), Err(C::UnsupportedProfile));

    std::fs::write(&state, vec![b' '; 1024 * 1024 + 1]).expect("seed oversized");
    assert_eq!(read_guarded_agent_state(root), Err(C::UnsafePath));

    // A hard-linked state file is not a single-owner regular file.
    let outside = root.join("outside.json");
    std::fs::write(&outside, r#"{"tooling":{}}"#).expect("seed outside");
    std::fs::remove_file(&state).expect("remove");
    std::fs::hard_link(&outside, &state).expect("hard link");
    assert_eq!(read_guarded_agent_state(root), Err(C::UnsafePath));

    // A symlinked state file is refused (only where this host can make one).
    std::fs::remove_file(&state).expect("remove");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&outside, &state).is_ok();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, &state).is_ok();
    if linked {
        assert_eq!(read_guarded_agent_state(root), Err(C::UnsafePath));
        std::fs::remove_file(&state).expect("remove link");
    }

    std::fs::create_dir(&state).expect("a directory is not a regular file");
    assert_eq!(read_guarded_agent_state(root), Err(C::UnsafePath));

    // Legs 4 and 5 run the launch path's own composition, step for step as
    // in `spawn_pty_input_target`: guarded reads, then the shared overlay,
    // then the typed parse. No test reaches that method itself.
    let launch_config = |root: &Path| {
        let (bytes, _) =
            crate::path_identity::read_bounded_regular(&root.join("config.json"), 1024 * 1024)
                .map_err(|_| C::UnsafePath)?;
        let config = crate::path_identity::parse_json_no_duplicates(&bytes)
            .map_err(|_| C::UnsupportedProfile)?;
        let state = read_guarded_agent_state(root)?;
        serde_json::from_value::<crate::config::agent_config::AgentLocalConfig>(
            crate::config::agent_config::overlay_agent_local_state(config, state),
        )
        .map_err(|_| C::UnsupportedProfile)
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::write(
        root.join("config.json"),
        r#"{"tooling":{"lastCodingAgent":"tracked"}}"#,
    )
    .expect("seed config");
    let state = root.join(CONFIG_STATE_TARGET_NAME);

    // Leg 4, fail-closed on purpose (section 5): a regular, small,
    // duplicate-free state object that fails the typed parse REFUSES the
    // launch, while both wrappers ignore the same file.
    std::fs::write(&state, r#"{"tooling":{"codingAgents":null}}"#).expect("seed null");
    assert_eq!(launch_config(root).map(|_| ()), Err(C::UnsupportedProfile));
    let wrapper = crate::config::agent_config::read_agent_local_config(root).expect("typed");
    assert_eq!(
        wrapper.tooling.last_coding_agent.as_deref(),
        Some("tracked")
    );
    let raw = crate::config::agent_config::read_agent_local_config_json(root)
        .expect("raw")
        .expect("some");
    assert_eq!(
        raw["tooling"],
        serde_json::json!({"lastCodingAgent": "tracked"})
    );

    // Leg 5, the control: a well-formed state file launches and its value
    // reaches the decision.
    std::fs::write(&state, r#"{"tooling":{"lastCodingAgent":"state"}}"#).expect("seed ok");
    let launched = launch_config(root).expect("a well-formed state file launches");
    assert_eq!(launched.tooling.last_coding_agent.as_deref(), Some("state"));
}
