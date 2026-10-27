use super::*;

/// §G9#8 / AR2-tests #21 — a session spawned by `deliver_wake` from an
/// FQN `msg.to` has a sidebar `Session.name` WITHOUT the `:` prefix.
/// §AR2-session-name handles this at mailbox.rs (spawn path) via
/// `split_project_prefix(&msg.to).1`. The logic is one line; a full
/// integration test needs a Tauri runtime.
#[test]
#[ignore = "integration: needs Tauri runtime + session manager fixture"]
fn deliver_wake_spawned_session_name_has_no_colon() {}

/// §G9#9 / AR2-tests #24 — full round-trip CLI send → mailbox route →
/// reply. Intentionally integration-level; no unit scaffolding.
#[test]
#[ignore = "integration: full CLI + mailbox + two-project fixture"]
fn resolve_to_target_round_trip_integration() {}

// ── #228 D1-a — derive_project_from_outbox_path tests ──

#[test]
fn derive_project_from_outbox_path_walks_up_wg_replica_layout() {
    let temp = tempfile::TempDir::new().unwrap();
    let project_dir = temp.path().join("proj-x");
    let outbox = project_dir
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_alice")
        .join(".agentscommander") // matches agent_local_dir_name() shape
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let file = outbox.join("msg-42.json");
    std::fs::write(&file, "{}").unwrap();

    let got = derive_project_from_outbox_path(&file)
        .unwrap()
        .expect("should derive project from WG layout");
    let expected = std::fs::canonicalize(&project_dir)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(got, expected);
}

#[test]
fn derive_project_from_outbox_path_accepts_ac_outbox() {
    let temp = tempfile::TempDir::new().unwrap();
    let project_dir = temp.path().join("proj-x");
    let outbox = project_dir
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_alice")
        .join(".agentscommander")
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let file = outbox.join("msg-42.json");
    std::fs::write(&file, "{}").unwrap();

    let got = derive_project_from_outbox_path(&file)
        .unwrap()
        .expect(".ac outbox should derive project");
    let expected = std::fs::canonicalize(&project_dir)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(got, expected);
}

#[test]
fn filter_sessions_by_fqn_accepts_ac_live_target() {
    let temp = tempfile::TempDir::new().unwrap();
    let agent = temp
        .path()
        .join("proj-x")
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_alice");
    std::fs::create_dir_all(&agent).unwrap();

    let pool = vec![make_session_info(
        "uuid-ac",
        "alice-ac",
        &path_string(&agent),
        crate::session::session::SessionStatus::Idle,
        true,
    )];

    let hits = filter_sessions_by_fqn(&pool, "proj-x:wg-1-devs/alice");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "uuid-ac");
}

#[test]
fn wg_session_fallback_returns_none_without_target_agent() {
    let temp = tempfile::TempDir::new().unwrap();
    let project_dir = temp.path().join("proj-x");
    let sibling = project_dir
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_bob");
    std::fs::create_dir_all(&sibling).unwrap();

    let dirs = vec![(Uuid::nil(), path_string(&sibling))];

    assert!(resolve_wg_path_from_session_dirs(&dirs, "proj-x:wg-1-devs/alice").is_none());
}

#[test]
fn wg_session_fallback_returns_ac_target_from_sibling() {
    let temp = tempfile::TempDir::new().unwrap();
    let project_dir = temp.path().join("proj-x");
    let target = project_dir
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_alice");
    let sibling = project_dir
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_bob");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();

    let dirs = vec![(Uuid::nil(), path_string(&sibling))];

    let got = resolve_wg_path_from_session_dirs(&dirs, "proj-x:wg-1-devs/alice")
        .expect(".ac sibling should resolve target");

    assert_eq!(
        std::fs::canonicalize(got).unwrap(),
        std::fs::canonicalize(target).unwrap()
    );
}

#[test]
fn derive_project_from_outbox_path_rejects_non_wg_layout() {
    let temp = tempfile::TempDir::new().unwrap();
    // A path with the right tail but missing the `.ac` ancestor.
    let outbox = temp
        .path()
        .join("random")
        .join(".agentscommander")
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let file = outbox.join("msg.json");
    std::fs::write(&file, "{}").unwrap();
    assert!(derive_project_from_outbox_path(&file).unwrap().is_none());
}

#[test]
fn derive_project_from_outbox_path_rejects_app_outbox_layout() {
    // App-outbox lives at <config_dir>/app-outbox/<file>.json — no
    // `__agent_*` / `wg-*` ancestors. Helper must return None so app-outbox
    // messages are NOT augmented (they have their own master-token bypass).
    let temp = tempfile::TempDir::new().unwrap();
    let app_outbox = temp.path().join("config").join("app-outbox");
    std::fs::create_dir_all(&app_outbox).unwrap();
    let file = app_outbox.join("msg.json");
    std::fs::write(&file, "{}").unwrap();
    assert!(derive_project_from_outbox_path(&file).unwrap().is_none());
}

fn write_project_refresh_request_fixture(
    path: &Path,
    id: &str,
    project_path: &Path,
    changed_name: Option<&str>,
    reason: &str,
) {
    let request = crate::cli::create_agent_matrix::ProjectRefreshRequest {
        id: id.to_string(),
        project_path: project_path.to_string_lossy().to_string(),
        changed_path: changed_name.map(|_| {
            project_path
                .join(".ac")
                .join("_agent_architect")
                .to_string_lossy()
                .to_string()
        }),
        changed_name: changed_name.map(str::to_string),
        reason: reason.to_string(),
        timestamp: "2026-05-28T20:00:00Z".to_string(),
    };
    std::fs::write(path, serde_json::to_string_pretty(&request).unwrap()).unwrap();
}

#[test]
fn project_refresh_request_reader_skips_tmp_files() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests_dir = temp.path().join("project-refresh-requests");
    std::fs::create_dir_all(&requests_dir).unwrap();
    let project = temp.path().join("ProjectAlpha");
    std::fs::create_dir_all(&project).unwrap();
    let tmp_file = requests_dir.join("request.json.tmp");
    write_project_refresh_request_fixture(
        &tmp_file,
        "request-tmp",
        &project,
        Some("ProjectAlpha/architect"),
        "createAgentMatrix",
    );

    let batch = collect_project_refresh_requests(&requests_dir);

    assert!(batch.payloads.is_empty());
    assert!(batch.processed_paths.is_empty());
    assert!(
        tmp_file.is_file(),
        "tmp file should be left for a future poll"
    );
}

#[test]
fn project_refresh_request_reader_deletes_malformed_json() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests_dir = temp.path().join("project-refresh-requests");
    std::fs::create_dir_all(&requests_dir).unwrap();
    let bad_file = requests_dir.join("bad.json");
    std::fs::write(&bad_file, "{not json").unwrap();

    let batch = collect_project_refresh_requests(&requests_dir);

    assert!(batch.payloads.is_empty());
    assert!(batch.processed_paths.is_empty());
    assert!(!bad_file.exists(), "malformed request should be deleted");
}

#[test]
fn project_refresh_request_reader_coalesces_same_project() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests_dir = temp.path().join("project-refresh-requests");
    std::fs::create_dir_all(&requests_dir).unwrap();
    let project = temp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac").join("_agent_architect")).unwrap();
    write_project_refresh_request_fixture(
        &requests_dir.join("a.json"),
        "request-a",
        &project,
        Some("ProjectAlpha/architect"),
        "createAgentMatrix",
    );
    write_project_refresh_request_fixture(
        &requests_dir.join("b.json"),
        "request-b",
        &project,
        Some("ProjectAlpha/planner"),
        "workgroupCreated",
    );

    let batch = collect_project_refresh_requests(&requests_dir);

    assert_eq!(batch.payloads.len(), 1);
    assert_eq!(batch.processed_paths.len(), 2);
    let expected_project = std::fs::canonicalize(&project)
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(batch.payloads[0].project_path, expected_project);
    assert_eq!(batch.payloads[0].id, "request-a");
}

#[test]
fn project_refresh_request_reader_preserves_registration_payload() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests_dir = temp.path().join("project-refresh-requests");
    std::fs::create_dir_all(&requests_dir).unwrap();
    let project = temp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    write_project_refresh_request_fixture(
        &requests_dir.join("registration.json"),
        "request-registration",
        &project,
        None,
        "projectRegistered",
    );

    let batch = collect_project_refresh_requests(&requests_dir);

    assert_eq!(batch.payloads.len(), 1);
    let payload = &batch.payloads[0];
    assert_eq!(payload.id, "request-registration");
    assert_eq!(payload.reason, "projectRegistered");
    assert_eq!(payload.changed_path, None);
    assert_eq!(payload.changed_name, None);
}

#[test]
fn project_refresh_request_reader_accepts_legacy_agent_fields() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests_dir = temp.path().join("project-refresh-requests");
    std::fs::create_dir_all(&requests_dir).unwrap();
    let project = temp.path().join("ProjectAlpha");
    let agent_path = project.join(".ac").join("_agent_architect");
    std::fs::create_dir_all(&agent_path).unwrap();
    let request = serde_json::json!({
        "id": "legacy-request",
        "projectPath": project.to_string_lossy(),
        "agentPath": agent_path.to_string_lossy(),
        "agentName": "ProjectAlpha/architect",
        "reason": "createAgentMatrix",
        "timestamp": "2026-05-28T20:00:00Z"
    });
    std::fs::write(
        requests_dir.join("legacy.json"),
        serde_json::to_string_pretty(&request).unwrap(),
    )
    .unwrap();

    let batch = collect_project_refresh_requests(&requests_dir);

    assert_eq!(batch.payloads.len(), 1);
    let expected_agent_path = agent_path.to_string_lossy().to_string();
    assert_eq!(
        batch.payloads[0].changed_path.as_deref(),
        Some(expected_agent_path.as_str())
    );
    assert_eq!(
        batch.payloads[0].changed_name.as_deref(),
        Some("ProjectAlpha/architect")
    );
}

#[test]
fn project_refresh_request_reader_prioritizes_registration_for_same_project() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests_dir = temp.path().join("project-refresh-requests");
    std::fs::create_dir_all(&requests_dir).unwrap();
    let project = temp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac").join("_agent_architect")).unwrap();
    write_project_refresh_request_fixture(
        &requests_dir.join("a-mutation.json"),
        "request-mutation",
        &project,
        Some("ProjectAlpha/architect"),
        "createAgentMatrix",
    );
    write_project_refresh_request_fixture(
        &requests_dir.join("z-registration.json"),
        "request-registration",
        &project,
        None,
        "projectRegistered",
    );

    let batch = collect_project_refresh_requests(&requests_dir);

    assert_eq!(batch.payloads.len(), 1);
    assert_eq!(batch.processed_paths.len(), 2);
    assert_eq!(batch.payloads[0].id, "request-registration");
    assert_eq!(batch.payloads[0].reason, "projectRegistered");
    assert_eq!(batch.payloads[0].changed_path, None);
    assert_eq!(batch.payloads[0].changed_name, None);
}

// ── Issue #481 mailbox wake-routing integration tests ──

#[tokio::test]
async fn deliver_wake_routes_to_live_when_phantom_present() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let phantom_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "phantom",
        SessionStatus::Running,
        None,
    )
    .await;
    let live_id =
        add_mailbox_session(&app, &fixture.target_cwd, "live", SessionStatus::Idle, None).await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(phantom_id, false);
    hooks.pty_presence.lock().unwrap().insert(live_id, true);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let message_path = write_wake_outbox_message_with_route(
        &fixture.sender_cwd,
        "msg-live",
        LOCAL_WAKE_FROM,
        LOCAL_WAKE_TO,
    );

    run_mailbox_message(&app, &message_path, hooks.clone()).await;

    let injected = hooks.inject_calls.lock().unwrap().clone();
    assert_eq!(injected, vec![live_id]);
    assert!(!injected.contains(&phantom_id));
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert_no_spawn_or_destroy_events(&hooks);
    assert_inject_results_consumed(&hooks);
}

#[tokio::test]
async fn deliver_wake_falls_back_when_first_inject_race_kills_pty() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let first_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "first",
        SessionStatus::Running,
        None,
    )
    .await;
    let second_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "second",
        SessionStatus::Running,
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(first_id, true);
    hooks.pty_presence.lock().unwrap().insert(second_id, true);
    {
        let mut results = hooks.inject_results.lock().unwrap();
        results.push_back(Err(format!(
            "PTY write failed: Session not found: {}",
            first_id
        )));
        results.push_back(Ok(()));
    }
    let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-race");

    run_mailbox_message(&app, &message_path, hooks.clone()).await;

    assert_eq!(
        *hooks.inject_calls.lock().unwrap(),
        vec![first_id, second_id]
    );
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert_no_spawn_or_destroy_events(&hooks);
    assert_inject_results_consumed(&hooks);
}

#[tokio::test]
async fn remote_pi_clear_command_branch_writes_new_emits_logical_event_and_stamps_boundary() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-live",
        "pi.cmd",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, session_id);
    {
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
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&events);
    fixture.app.listen_any("message_delivered", move |event| {
        captured.lock().unwrap().push(event.payload().to_string());
    });
    let poller = MailboxPoller::default();
    let message = logical_command_message("clear", "");

    poller
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
        .unwrap();

    assert_eq!(
        mock_pty_writes_for(&app, session_id),
        vec![b"/new".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    assert!(
        manager
            .get_session(session_id)
            .await
            .unwrap()
            .start_fresh_on_restore
    );
    let delivered = events.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1);
    let payload: serde_json::Value = serde_json::from_str(&delivered[0]).unwrap();
    assert_eq!(payload["command"], "clear");
    assert_eq!(payload["id"], message.id);
}

/// #2336 - the post-clear follow-up body is exempt from the typing hold. The
/// logical command half is delivered and receipted FIRST; a hold then arms
/// during the detached idle wait (and stays armed), and the plain injector
/// must still write the body exactly once. The follow-up is not counted as a
/// held wake and no second delivered receipt is emitted.
#[tokio::test]
async fn typing_hold_armed_during_post_clear_wait_does_not_drop_followup_body() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-followup",
        "pi.cmd",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, session_id);
    assert!(fixture
        .app
        .manage(crate::pty::input_activity::new_typing_hold_state()));

    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&events);
    fixture.app.listen_any("message_delivered", move |event| {
        captured.lock().unwrap().push(event.payload().to_string());
    });

    let poller = MailboxPoller::default();
    let message = logical_command_message("clear", "follow-up body");
    poller
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
        .unwrap();

    // The command half is delivered and receipted; the detached follow-up is
    // now in its idle wait, so arm the hold during that wait.
    assert_eq!(events.lock().unwrap().len(), 1);
    let window = std::time::Duration::from_secs(30);
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
    hold.lock().unwrap().note_qualifying_key(session_id);
    assert!(hold.lock().unwrap().is_hold_active(session_id, window));

    // The agent becomes idle again; the follow-up must be written anyway.
    {
        let manager = {
            let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = state.read().await;
            guard.clone()
        };
        manager.mark_idle(session_id).await;
    }

    // #2586 D1 - a `send_enter` shell is written the wrap without its final `\n\r`.
    let wrapped = crate::phone::messaging::format_pty_wrap(&message.from, "follow-up body");
    let expected_body = wrapped.strip_suffix("\n\r").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        // The follow-up writes the body, then sleeps 1500ms and 500ms before
        // its two Enters, so settle on the full six-write sequence.
        if mock_pty_writes_for(&app, session_id).len() == 6 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the held-window follow-up did not complete: {:?}",
            mock_pty_writes_for(&app, session_id)
                .iter()
                .map(|write| String::from_utf8_lossy(write).to_string())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Command text + two Enters, then the body + two Enters, exactly once.
    let writes = mock_pty_writes_for(&app, session_id);
    assert_eq!(
        writes,
        vec![
            b"/new".to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec(),
            expected_body.as_bytes().to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec(),
        ],
        "writes={:?}",
        writes
            .iter()
            .map(|write| String::from_utf8_lossy(write).to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        writes
            .iter()
            .filter(|write| write.as_slice() == expected_body.as_bytes())
            .count(),
        1,
        "the follow-up body must be written exactly once"
    );
    assert_eq!(
        events.lock().unwrap().len(),
        1,
        "no second delivered receipt may follow the body"
    );
    assert_eq!(
        hold.lock().unwrap().snapshot(session_id, window).held_count,
        0,
        "the follow-up body is not a held wake"
    );
}
