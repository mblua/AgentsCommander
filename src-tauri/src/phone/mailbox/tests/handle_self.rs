use super::*;

#[tokio::test]
async fn raise_hand_process_message_bad_tokens_are_rejected_not_false() {
    for (idx, token) in [
        (0, "not-a-uuid".to_string()),
        (1, Uuid::new_v4().to_string()),
    ] {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        assert!(app.manage(crate::pty::manager::ResponseCloseState::default()));
        write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");
        let msg_id = format!("msg-rh-bad-token-{idx}");
        let rid = format!("rid-rh-bad-token-{idx}");
        let (path, _msg) =
            build_raise_hand_message(&fixture.sender_cwd, &msg_id, &rid, Some(token));
        let poller = MailboxPoller::default();
        poller
            .process_message(&app, &path, OutboxOrigin::Replica)
            .await
            .expect("bad-token raise-hand should reject cleanly");

        assert!(
            read_reject_reason(&fixture.sender_cwd, &msg_id).is_some(),
            "bad token should write a rejection reason"
        );
        assert!(
            read_response_json(&fixture.sender_cwd, &rid).is_none(),
            "bad token must not be converted into a false response"
        );
        assert!(poller.response_sources.lock().unwrap().is_empty());
    }
    // D: a real session token crosses the full validation/deferred handoff.
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let tracker = crate::pty::manager::ResponseCloseState::default();
    assert!(app.manage(tracker.clone()));
    let (_, token) =
        seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running).await;
    app.state::<SettingsState>()
        .write()
        .await
        .project_paths
        .push(fixture.sender_cwd.to_string_lossy().to_string());
    let path = write_wake_message(
        &fixture.sender_cwd,
        "valid-token-deferred",
        "2026-07-15T00:00:03+00:00",
    );
    let mut message = wake_message_to_target();
    message.id = "valid-token-deferred".into();
    message.token = Some(token.to_string());
    let content = serde_json::to_string_pretty(&message).unwrap();
    std::fs::write(&path, &content).unwrap();
    let hooks = MailboxTestHooks::default();
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let mut slot = None;
    poller
        .process_message_content(
            &app,
            &path,
            OutboxOrigin::Replica,
            &content,
            Some(&mut slot),
        )
        .await
        .unwrap();
    let validated = slot.expect("valid authenticated envelope reaches deferred slot");
    assert_eq!(
        validated.message.token.as_deref(),
        Some(token.to_string().as_str())
    );
    let work = validated
        .response
        .as_ref()
        .expect("validated carrier retains admission")
        .clone();
    assert!(work.state.same_state(&tracker));
    assert_eq!(work.target, validated.message.to);
    assert_eq!(
        work.key,
        format!("fs:{}:{}", path.to_string_lossy(), message.id)
    );
    assert_eq!(
        work.digest,
        crate::phone::types::sha256_hex(content.as_bytes())
    );
    assert!(work.delivery_context.is_none());
    {
        let sources = poller.response_sources.lock().unwrap();
        let retained = &sources[&path];
        assert!(retained.state.same_state(&work.state));
        assert_eq!(
            (&retained.target, &retained.key, &retained.digest),
            (&work.target, &work.key, &work.digest)
        );
        assert!(retained.delivery_context.is_none());
    }
    poller.live_claims.insert(wake_claim_path(&path));
    assert!(matches!(
        poller.claim_and_spawn_validated_wake(&app, &path, validated),
        WakeHandoff::LaneBusy
    ));
    let sources = poller.response_sources.lock().unwrap();
    let retained = &sources[&path];
    assert!(retained.state.same_state(&tracker));
    assert_eq!(
        (&retained.target, &retained.key, &retained.digest),
        (&work.target, &work.key, &work.digest)
    );
    assert!(retained.delivery_context.is_none());
    assert!(path.exists());
    assert!(!wake_claim_path(&path).exists());
    assert_eq!(poller.wake_workers.len(), 0);
    assert_eq!(poller.wake_lanes.len(), 0);
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn handle_self_clear_valid_token_queues() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (session_id, token) = seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
    // #626: SELF-HANDOFF.md must exist (existence gate) and SELF-FORGET.md is archived on queue.
    seed_self_handoff(&cwd);
    let forget_content = "topic to forget\nwith full archive content";
    std::fs::write(cwd.join("SELF-FORGET.md"), forget_content).unwrap();

    let (path, msg) =
        build_self_clear_message(&cwd, "msg-sc-1", "rid-sc-1", Some(token.to_string()));
    let poller = MailboxPoller::default();
    poller
        .handle_self_clear(&app, &path, &msg, false)
        .await
        .expect("handle_self_clear should succeed");

    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sc-1").as_deref(),
        Some("queued")
    );
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(pending_self_clear_len(&app).await, 1);
    // #626/#636: SELF-FORGET.md archived to exactly one self-clear/<ts>_SELF-FORGET.md, original
    // gone.
    assert!(
        !cwd.join("SELF-FORGET.md").is_file(),
        "SELF-FORGET.md must be archived away on queue"
    );
    assert_eq!(
        count_forget_archives(&cwd),
        1,
        "exactly one self-clear/<ts>_SELF-FORGET.md archive must exist after queue"
    );
    assert_eq!(read_only_forget_archive(&cwd), forget_content);
    // message moved to delivered/, original removed.
    assert!(!path.exists());
}

#[tokio::test]
async fn handle_self_clear_pi_valid_token_queues_and_archives_once() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("pi-agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (session_id, token) = seed_self_clear_session(&app, &cwd.to_string_lossy(), "pi.cmd").await;
    seed_self_handoff(&cwd);
    std::fs::write(cwd.join("SELF-FORGET.md"), "Pi forgotten notes").unwrap();
    let (path, message) =
        build_self_clear_message(&cwd, "msg-sc-pi", "rid-sc-pi", Some(token.to_string()));
    let poller = MailboxPoller::default();

    poller
        .handle_self_clear(&app, &path, &message, false)
        .await
        .unwrap();

    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sc-pi").as_deref(),
        Some("queued")
    );
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(count_forget_archives(&cwd), 1);
    assert_eq!(read_only_forget_archive(&cwd), "Pi forgotten notes");
    assert!(!path.exists());
}

#[tokio::test]
async fn handle_self_clear_second_request_is_already_queued() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (session_id, token) = seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
    seed_self_handoff(&cwd); // existence gate
    std::fs::write(cwd.join("SELF-FORGET.md"), "topic to forget").unwrap();

    let (path1, msg1) =
        build_self_clear_message(&cwd, "msg-sc-a", "rid-sc-a", Some(token.to_string()));
    let poller = MailboxPoller::default();
    poller
        .handle_self_clear(&app, &path1, &msg1, false)
        .await
        .unwrap();
    // The first (queued) request archived SELF-FORGET.md. Re-create one to prove the second request
    // does NOT re-archive (already_queued skips the newly_inserted block).
    assert_eq!(
        count_forget_archives(&cwd),
        1,
        "first request archives SELF-FORGET.md"
    );
    assert_eq!(read_only_forget_archive(&cwd), "topic to forget");
    std::fs::write(cwd.join("SELF-FORGET.md"), "a new forget written mid-cycle").unwrap();

    let (path2, msg2) =
        build_self_clear_message(&cwd, "msg-sc-b", "rid-sc-b", Some(token.to_string()));
    poller
        .handle_self_clear(&app, &path2, &msg2, false)
        .await
        .unwrap();

    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sc-a").as_deref(),
        Some("queued")
    );
    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sc-b").as_deref(),
        Some("already_queued")
    );
    // Still exactly one pending id (no stacking).
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(pending_self_clear_len(&app).await, 1);
    // The already_queued second request did NOT re-archive: still exactly one archive, and the
    // freshly re-created SELF-FORGET.md is left in place.
    assert_eq!(
        count_forget_archives(&cwd),
        1,
        "already_queued must not re-archive"
    );
    assert!(
        cwd.join("SELF-FORGET.md").is_file(),
        "the re-created SELF-FORGET.md must survive an already_queued request"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("SELF-FORGET.md")).unwrap(),
        "a new forget written mid-cycle"
    );
    assert_eq!(
        read_only_forget_archive(&cwd),
        "topic to forget",
        "already_queued must not replace the first request's archive"
    );
}

/// #626 - the existence gate REFUSES when SELF-HANDOFF.md is absent: nothing is queued, the id is
/// NOT inserted, and no SELF-FORGET archive is created (the gate runs before the insert + archive).
#[tokio::test]
async fn handle_self_clear_missing_self_handoff_is_rejected() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (session_id, token) = seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
    // No SELF-HANDOFF.md seeded. Seed a SELF-FORGET.md to prove it is NOT archived on a refuse.
    std::fs::write(cwd.join("SELF-FORGET.md"), "must not be archived").unwrap();

    let (path, msg) = build_self_clear_message(
        &cwd,
        "msg-sc-nohandoff",
        "rid-sc-nohandoff",
        Some(token.to_string()),
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_clear(&app, &path, &msg, false)
        .await
        .unwrap();

    let reason = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-sc-nohandoff.reason.txt");
    assert!(
        reason.exists(),
        "missing SELF-HANDOFF.md must be rejected with a reason file"
    );
    assert!(
        !pending_self_clear_contains(&app, session_id).await,
        "a refused request must not insert the id"
    );
    assert_eq!(pending_self_clear_len(&app).await, 0);
    // The archive runs only after the gate passes; a refuse must not touch SELF-FORGET.md.
    assert!(
        cwd.join("SELF-FORGET.md").is_file(),
        "SELF-FORGET.md must NOT be archived when the request is refused"
    );
    assert_eq!(count_forget_archives(&cwd), 0);
}

/// #626 - SELF-HANDOFF.md present but no SELF-FORGET.md: queues normally, archive is a no-op (no
/// error, no self-clear/<ts>_SELF-FORGET.md created).
#[tokio::test]
async fn handle_self_clear_no_forget_md_still_queues() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (session_id, token) = seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
    seed_self_handoff(&cwd); // no SELF-FORGET.md

    let (path, msg) = build_self_clear_message(
        &cwd,
        "msg-sc-noforget",
        "rid-sc-noforget",
        Some(token.to_string()),
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_clear(&app, &path, &msg, false)
        .await
        .unwrap();

    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sc-noforget").as_deref(),
        Some("queued")
    );
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(
        count_forget_archives(&cwd),
        0,
        "no SELF-FORGET.md means no archive (no-op), no error"
    );
}

#[tokio::test]
async fn handle_self_clear_invalid_token_is_rejected() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    // No session seeded; a random UUID token resolves to no session.
    let (path, msg) = build_self_clear_message(
        &cwd,
        "msg-sc-bad",
        "rid-sc-bad",
        Some(Uuid::new_v4().to_string()),
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_clear(&app, &path, &msg, false)
        .await
        .unwrap();

    // Rejected: reason file written, nothing queued.
    let reason = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-sc-bad.reason.txt");
    assert!(reason.exists(), "reject reason file should be written");
    assert_eq!(pending_self_clear_len(&app).await, 0);
}

#[tokio::test]
async fn handle_self_clear_non_coding_shell_is_rejected() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (_session_id, token) =
        seed_self_clear_session(&app, &cwd.to_string_lossy(), "powershell.exe").await;

    let (path, msg) = build_self_clear_message(
        &cwd,
        "msg-sc-shell",
        "rid-sc-shell",
        Some(token.to_string()),
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_clear(&app, &path, &msg, false)
        .await
        .unwrap();

    let reason = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-sc-shell.reason.txt");
    assert!(reason.exists(), "non-coding shell must be rejected");
    assert_eq!(pending_self_clear_len(&app).await, 0);
}

#[tokio::test]
async fn handle_self_clear_root_agent_is_allowed() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (session_id, token) = seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
    seed_self_handoff(&cwd); // #626 existence gate
                             // #617 follow-up: the Root Agent exclusion was removed. A
                             // token-authorized self-clear from the Root must now queue like any
                             // other coding-agent session. Identity is still resolved solely by
                             // find_by_token, so this can only ever clear the session that owns the
                             // presented token.
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_is_root_agent(session_id, true).await;
    }

    let (path, msg) =
        build_self_clear_message(&cwd, "msg-sc-root", "rid-sc-root", Some(token.to_string()));
    let poller = MailboxPoller::default();
    poller
        .handle_self_clear(&app, &path, &msg, false)
        .await
        .unwrap();

    // Queued, not rejected: the response is "queued", exactly one id is pending,
    // and no reject reason file was written for the Root session.
    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sc-root").as_deref(),
        Some("queued")
    );
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(pending_self_clear_len(&app).await, 1);
    let reason = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-sc-root.reason.txt");
    assert!(
        !reason.exists(),
        "root self-clear must NOT be rejected anymore"
    );
}

#[tokio::test]
async fn handle_self_switch_valid_token_queues_with_resolved_targets() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    let (session_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "codex",
        Some("codex"),
        Some("A"),
        Some("B"),
    )
    .await;
    seed_self_handoff(&fixture.replica);
    let forget_content = "topic to forget\nwith full switch archive content";
    std::fs::write(fixture.replica.join("SELF-FORGET.md"), forget_content).unwrap();

    let (path, msg) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-1",
        "rid-ss-1",
        Some(token.to_string()),
        Some("claude"),
        Some("c"),
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_handoff_switch(&app, &path, &msg, false)
        .await
        .unwrap();

    let response = read_response_json(&fixture.replica, "rid-ss-1").unwrap();
    assert_eq!(response["action"], SELF_SWITCH_ACTION);
    assert_eq!(response["status"], "queued");
    assert_eq!(response["target_coding_agent"], "claude");
    assert_eq!(response["target_profile"], "C");
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(pending_self_clear_len(&app).await, 1);
    assert_eq!(count_forget_archives(&fixture.replica), 1);
    assert_eq!(read_only_forget_archive(&fixture.replica), forget_content);
    assert!(!fixture.replica.join("SELF-FORGET.md").is_file());
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.replica.join("config.json")).unwrap(),
    )
    .unwrap();
    assert!(
        saved["tooling"]["currentCodingAgent"].is_null(),
        "target selection is persisted by Phase 1, not queue time"
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn handle_self_switch_unsupported_source_rejected_before_other_work() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    let (session_id, token) =
        seed_self_switch_session(&app, &fixture.replica, "pwsh", None, None, None).await;
    std::fs::write(fixture.replica.join("SELF-FORGET.md"), "must remain").unwrap();
    let (path, message) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-unsupported-source",
        "rid-ss-unsupported-source",
        Some(token.to_string()),
        Some("deliberately-invalid-target"),
        Some("not-a-profile"),
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::default();

    poller
        .handle_self_handoff_switch(&app, &path, &message, false)
        .await
        .unwrap();

    let reason = read_reject_reason(&fixture.replica, "msg-ss-unsupported-source").unwrap();
    assert!(reason.contains("not a supported source shell"), "{reason}");
    assert!(!reason.contains("deliberately-invalid-target"), "{reason}");
    assert!(!reason.contains("SELF-HANDOFF"), "{reason}");
    assert!(!pending_self_clear_contains(&app, session_id).await);
    assert_eq!(count_forget_archives(&fixture.replica), 0);
    assert_eq!(
        std::fs::read_to_string(fixture.replica.join("SELF-FORGET.md")).unwrap(),
        "must remain"
    );
}
