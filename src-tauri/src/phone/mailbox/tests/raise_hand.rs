use super::*;

#[test]
fn resolve_switch_targets_reports_empty_agent_list() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings = AppSettings::default();

    let err = resolve_switch_targets(
        &settings,
        temp.path(),
        Some("codex"),
        None,
        None,
        None,
        None,
    )
    .unwrap_err();

    assert!(err.contains("<none configured>"));
}

// ── #626/#629 archive_root_md unit tests (tempdir, deterministic timestamp) ──

#[test]
fn archive_root_md_forget_absent_is_noop() {
    let temp = tempfile::TempDir::new().unwrap();
    let res = archive_root_md(temp.path(), "SELF-FORGET", "20260101_000000").unwrap();
    assert!(res.is_none(), "absent SELF-FORGET.md is a no-op (Ok(None))");
    let count = std::fs::read_dir(temp.path()).unwrap().count();
    assert_eq!(
        count, 0,
        "no file or self-clear/ dir is created when SELF-FORGET.md is absent"
    );
}

#[test]
fn archive_root_md_forget_present_moves_to_self_clear_with_prefixed_name() {
    // #636 - the archive now lands in <root>/self-clear/<ts>_SELF-FORGET.md (timestamp prefixed).
    let temp = tempfile::TempDir::new().unwrap();
    let src = temp.path().join("SELF-FORGET.md");
    std::fs::write(&src, "old topic 1\nold topic 2").unwrap();
    let dst = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
        .unwrap()
        .expect("present SELF-FORGET.md must be archived");
    assert_eq!(
        dst,
        temp.path()
            .join("self-clear")
            .join("20260102_030405_SELF-FORGET.md")
    );
    assert!(
        temp.path().join("self-clear").is_dir(),
        "the self-clear/ subdir must be created on demand"
    );
    assert!(!src.exists(), "SELF-FORGET.md must be gone after the move");
    assert_eq!(
        std::fs::read_to_string(&dst).unwrap(),
        "old topic 1\nold topic 2",
        "content must be preserved across the move"
    );
}

#[test]
fn capture_self_forget_summary_is_available_before_archive_only() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("SELF-FORGET.md"),
        "- first closed\n- second closed",
    )
    .unwrap();

    let summary = capture_self_forget_summary(temp.path()).unwrap();
    assert_eq!(summary.as_str(), "first closed; second closed");

    archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
        .unwrap()
        .expect("archive");
    assert_eq!(capture_self_forget_summary(temp.path()), None);
}

#[test]
fn capture_self_forget_summary_uses_bounded_read_but_archive_keeps_full_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let full = format!(
        "{}tail that should remain in the archive",
        "a".repeat(SELF_FORGET_SUMMARY_READ_LIMIT_BYTES as usize + 1024)
    );
    std::fs::write(temp.path().join("SELF-FORGET.md"), &full).unwrap();

    let summary = capture_self_forget_summary(temp.path()).unwrap();
    assert!(summary.as_str().chars().count() <= SELF_FORGET_SUMMARY_MAX_CHARS);
    assert!(summary.as_str().ends_with("..."));

    let archived = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
        .unwrap()
        .expect("archive");
    assert_eq!(std::fs::read_to_string(archived).unwrap(), full);
}

#[test]
fn capture_self_forget_summary_truncates_split_trailing_utf8_scalar() {
    let temp = tempfile::TempDir::new().unwrap();
    let full = format!(
        "{}é",
        "a".repeat(SELF_FORGET_SUMMARY_READ_LIMIT_BYTES as usize - 1)
    );
    assert_eq!(
        full.len(),
        SELF_FORGET_SUMMARY_READ_LIMIT_BYTES as usize + 1
    );
    std::fs::write(temp.path().join("SELF-FORGET.md"), &full).unwrap();

    let summary = capture_self_forget_summary(temp.path()).unwrap();
    assert!(summary.as_str().chars().count() <= SELF_FORGET_SUMMARY_MAX_CHARS);
    assert!(summary.as_str().ends_with("..."));
    assert!(!summary.as_str().contains('é'));

    let archived = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
        .unwrap()
        .expect("archive");
    assert_eq!(std::fs::read_to_string(archived).unwrap(), full);
}

#[test]
fn capture_self_forget_summary_invalid_utf8_returns_none_and_archive_keeps_bytes() {
    let temp = tempfile::TempDir::new().unwrap();
    let bytes = vec![b'v', b'a', b'l', 0xff, b'x'];
    std::fs::write(temp.path().join("SELF-FORGET.md"), &bytes).unwrap();

    assert_eq!(capture_self_forget_summary(temp.path()), None);
    let archived = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
        .unwrap()
        .expect("archive");
    assert_eq!(std::fs::read(archived).unwrap(), bytes);
}

#[test]
fn archive_root_md_forget_target_exists_errs_without_clobber() {
    let temp = tempfile::TempDir::new().unwrap();
    let src = temp.path().join("SELF-FORGET.md");
    std::fs::write(&src, "fresh").unwrap();
    let archive_dir = temp.path().join("self-clear");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let dst = archive_dir.join("20260102_030405_SELF-FORGET.md");
    std::fs::write(&dst, "existing archive").unwrap();
    let err = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(
        src.exists(),
        "SELF-FORGET.md must stay in place when the target exists"
    );
    assert_eq!(
        std::fs::read_to_string(&dst).unwrap(),
        "existing archive",
        "the pre-existing archive must not be clobbered"
    );
}

#[test]
fn archive_root_md_self_handoff_present_moves_to_self_clear_with_prefixed_name() {
    // #629/#636 - the new consumer: SELF-HANDOFF.md is archived into self-clear/ with the same
    // helper.
    let temp = tempfile::TempDir::new().unwrap();
    let src = temp.path().join("SELF-HANDOFF.md");
    std::fs::write(&src, "resume: finish step 4\nthen run gates").unwrap();
    let dst = archive_root_md(temp.path(), "SELF-HANDOFF", "20260301_121314")
        .unwrap()
        .expect("present SELF-HANDOFF.md must be archived");
    assert_eq!(
        dst,
        temp.path()
            .join("self-clear")
            .join("20260301_121314_SELF-HANDOFF.md")
    );
    assert!(
        !src.exists(),
        "SELF-HANDOFF.md must be gone after the move (so it cannot re-trigger the gate)"
    );
    assert_eq!(
        std::fs::read_to_string(&dst).unwrap(),
        "resume: finish step 4\nthen run gates",
        "content must be preserved across the move"
    );
}

#[test]
fn archive_root_md_self_handoff_absent_is_noop() {
    // If the agent already moved or removed SELF-HANDOFF.md, the archive is a no-op.
    let temp = tempfile::TempDir::new().unwrap();
    let res = archive_root_md(temp.path(), "SELF-HANDOFF", "20260301_121314").unwrap();
    assert!(
        res.is_none(),
        "absent SELF-HANDOFF.md is a no-op (Ok(None))"
    );
    let count = std::fs::read_dir(temp.path()).unwrap().count();
    assert_eq!(
        count, 0,
        "no file or self-clear/ dir is created when SELF-HANDOFF.md is absent"
    );
}

// ── #749 archive_handoff_for_inject / restore_handoff_after_failed_inject ──

#[test]
fn archive_handoff_for_inject_moves_file_and_names_exact_relative_path() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "resume notes").unwrap();

    let (prompt_path, archived) =
        archive_handoff_for_inject(temp.path(), "20260702_181530", SELF_CLEAR_ACTION);

    assert_eq!(prompt_path, "self-clear/20260702_181530_SELF-HANDOFF.md");
    let dst = archived.expect("present SELF-HANDOFF.md must be archived");
    assert_eq!(
        dst,
        temp.path()
            .join("self-clear")
            .join("20260702_181530_SELF-HANDOFF.md")
    );
    assert!(!temp.path().join("SELF-HANDOFF.md").exists());
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "resume notes");
    // The relative path the prompt names resolves to the archived file from the root.
    assert_eq!(temp.path().join(&prompt_path), dst);
}

#[test]
fn archive_handoff_for_inject_absent_source_falls_back_to_root_name() {
    let temp = tempfile::TempDir::new().unwrap();
    let (prompt_path, archived) =
        archive_handoff_for_inject(temp.path(), "20260702_181530", SELF_CLEAR_ACTION);
    assert_eq!(prompt_path, "SELF-HANDOFF.md");
    assert!(archived.is_none());
}

#[test]
fn archive_handoff_for_inject_rename_failure_falls_back_to_root_name() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "resume notes").unwrap();
    // Force the AlreadyExists refusal: pre-create the exact dst for this timestamp.
    let archive_dir = temp.path().join("self-clear");
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(
        archive_dir.join("20260702_181530_SELF-HANDOFF.md"),
        "older archive",
    )
    .unwrap();

    let (prompt_path, archived) =
        archive_handoff_for_inject(temp.path(), "20260702_181530", SELF_CLEAR_ACTION);

    assert_eq!(
        prompt_path, "SELF-HANDOFF.md",
        "on archive failure the prompt must point at the root file, which is still there"
    );
    assert!(archived.is_none());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("SELF-HANDOFF.md")).unwrap(),
        "resume notes",
        "the source must stay in place on failure"
    );
}

#[test]
fn restore_handoff_after_failed_inject_renames_back() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "resume notes").unwrap();
    let (_, archived) =
        archive_handoff_for_inject(temp.path(), "20260702_181530", SELF_CLEAR_ACTION);
    let dst = archived.expect("archived");

    restore_handoff_after_failed_inject(temp.path(), &dst, SELF_CLEAR_ACTION);

    assert_eq!(
        std::fs::read_to_string(temp.path().join("SELF-HANDOFF.md")).unwrap(),
        "resume notes",
        "a failed inject must return the notes to the canonical root name"
    );
    assert!(!dst.exists());
}

#[test]
fn restore_handoff_after_failed_inject_refuses_to_clobber_new_root_handoff() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "old notes").unwrap();
    let (_, archived) =
        archive_handoff_for_inject(temp.path(), "20260702_181530", SELF_CLEAR_ACTION);
    let dst = archived.expect("archived");
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "new in-flight notes").unwrap();

    restore_handoff_after_failed_inject(temp.path(), &dst, SELF_CLEAR_ACTION);

    assert_eq!(
        std::fs::read_to_string(temp.path().join("SELF-HANDOFF.md")).unwrap(),
        "new in-flight notes",
        "a newer root handoff must never be clobbered by the restore"
    );
    assert_eq!(
        std::fs::read_to_string(&dst).unwrap(),
        "old notes",
        "the archived copy stays recoverable under self-clear/"
    );
}

#[test]
fn pending_self_clear_set_is_idempotent() {
    let pending = crate::PendingSelfClear::default();
    let id = Uuid::new_v4();
    {
        assert!(pending.seed_legacy(id), "first insert is newly-inserted");
        assert!(
            !pending.seed_legacy(id),
            "second insert collapses (already_queued)"
        );
        assert_eq!(pending.len(), 1);
    }
    {
        assert!(pending.remove_legacy_fixture(id));
        assert!(
            pending.seed_legacy(id),
            "re-issue after removal queues again"
        );
    }
}

fn raise_hand_delivered_path(cwd: &Path, msg_id: &str) -> PathBuf {
    cwd.join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("delivered")
        .join(format!("{}.json", msg_id))
}

#[tokio::test]
async fn raise_hand_process_message_coordinator_with_task_title_sets_state_and_event() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let (session_id, token) =
        seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running).await;
    write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&events);
    fixture
        .app
        .listen_any("session_communication_changed", move |event| {
            captured.lock().unwrap().push(event.payload().to_string());
        });

    let (path, _msg) = build_raise_hand_message(
        &fixture.sender_cwd,
        "msg-rh-1",
        "rid-rh-1",
        Some(token.to_string()),
    );
    let poller = MailboxPoller::new();
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .expect("raise-hand process_message should succeed");

    let response = read_response_json(&fixture.sender_cwd, "rid-rh-1").unwrap();
    assert_eq!(response["action"], RAISE_HAND_ACTION);
    assert_eq!(response["status"], "raised");
    assert_eq!(response["raised"], true);
    assert_eq!(response["session_id"], session_id.to_string());
    assert!(raise_hand_delivered_path(&fixture.sender_cwd, "msg-rh-1").exists());
    assert!(!path.exists());
    let mgr = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = session_mgr.read().await;
        guard.clone()
    };
    let stored = mgr.get_session(session_id).await.unwrap();
    let communication = stored.communication.expect("raise-hand state stored");
    assert_eq!(communication.kind, SessionCommunicationKind::RaiseHand);
    assert!(communication.visible);

    let captured = events.lock().unwrap().clone();
    assert_eq!(captured.len(), 1, "raise-hand should emit one event");
    let event: serde_json::Value = serde_json::from_str(&captured[0]).unwrap();
    assert_eq!(event["sessionId"], session_id.to_string());
    assert_eq!(event["communication"]["kind"], "raiseHand");
    assert_eq!(event["communication"]["visible"], true);
}

#[tokio::test]
async fn raise_hand_process_message_second_request_reports_already_visible() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let (_session_id, token) =
        seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running).await;
    write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&events);
    fixture
        .app
        .listen_any("session_communication_changed", move |event| {
            captured.lock().unwrap().push(event.payload().to_string());
        });

    let (first_path, _first_msg) = build_raise_hand_message(
        &fixture.sender_cwd,
        "msg-rh-2a",
        "rid-rh-2a",
        Some(token.to_string()),
    );
    let poller = MailboxPoller::new();
    poller
        .process_message(&app, &first_path, OutboxOrigin::Replica)
        .await
        .expect("first raise-hand should succeed");
    let (second_path, _second_msg) = build_raise_hand_message(
        &fixture.sender_cwd,
        "msg-rh-2b",
        "rid-rh-2b",
        Some(token.to_string()),
    );
    poller
        .process_message(&app, &second_path, OutboxOrigin::Replica)
        .await
        .expect("second raise-hand should succeed");

    let response = read_response_json(&fixture.sender_cwd, "rid-rh-2b").unwrap();
    assert_eq!(response["raised"], true);
    assert_eq!(response["status"], "already_visible");
    assert_eq!(
        events.lock().unwrap().len(),
        1,
        "idempotent raise-hand should not emit a duplicate change event"
    );
}

#[tokio::test]
async fn raise_hand_process_message_without_visible_title_slot_returns_false() {
    for (idx, task_contents) in [
        (0, None),
        (1, Some("body only\n")),
        (2, Some("---\ntitle: \n---\n\nbody")),
    ] {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let (session_id, token) =
            seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running).await;
        if let Some(contents) = task_contents {
            let task_path = crate::session::session::find_workgroup_task_path_for_cwd(
                &fixture.sender_cwd.to_string_lossy(),
            )
            .unwrap();
            std::fs::write(task_path, contents).unwrap();
        }
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&events);
        fixture
            .app
            .listen_any("session_communication_changed", move |event| {
                captured.lock().unwrap().push(event.payload().to_string());
            });

        let msg_id = format!("msg-rh-noslot-{idx}");
        let rid = format!("rid-rh-noslot-{idx}");
        let (path, _msg) =
            build_raise_hand_message(&fixture.sender_cwd, &msg_id, &rid, Some(token.to_string()));
        let poller = MailboxPoller::new();
        poller
            .process_message(&app, &path, OutboxOrigin::Replica)
            .await
            .expect("raise-hand no-slot message should be processed");

        let response = read_response_json(&fixture.sender_cwd, &rid).unwrap();
        assert_eq!(response["raised"], false);
        assert_eq!(response["status"], "not_visible");
        assert!(events.lock().unwrap().is_empty());
        let mgr = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = session_mgr.read().await;
            guard.clone()
        };
        assert!(mgr
            .get_session(session_id)
            .await
            .unwrap()
            .communication
            .is_none());
    }
}

#[tokio::test]
async fn raise_hand_process_message_non_coordinator_returns_false() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let (session_id, token) =
        seed_raise_hand_session(&app, &fixture.sender_cwd, false, SessionStatus::Running).await;
    write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");

    let (path, _msg) = build_raise_hand_message(
        &fixture.sender_cwd,
        "msg-rh-noncoord",
        "rid-rh-noncoord",
        Some(token.to_string()),
    );
    let poller = MailboxPoller::new();
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .expect("non-coordinator raise-hand should be processed");

    let response = read_response_json(&fixture.sender_cwd, "rid-rh-noncoord").unwrap();
    assert_eq!(response["raised"], false);
    assert_eq!(response["status"], "not_visible");
    let mgr = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = session_mgr.read().await;
        guard.clone()
    };
    assert!(mgr
        .get_session(session_id)
        .await
        .unwrap()
        .communication
        .is_none());
}

#[tokio::test]
async fn raise_hand_process_message_exited_coordinator_returns_false() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let (session_id, token) =
        seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Exited(0)).await;
    write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");

    let (path, _msg) = build_raise_hand_message(
        &fixture.sender_cwd,
        "msg-rh-exited",
        "rid-rh-exited",
        Some(token.to_string()),
    );
    let poller = MailboxPoller::new();
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .expect("exited coordinator raise-hand should be processed");

    let response = read_response_json(&fixture.sender_cwd, "rid-rh-exited").unwrap();
    assert_eq!(response["raised"], false);
    assert_eq!(response["status"], "not_visible");
    let mgr = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = session_mgr.read().await;
        guard.clone()
    };
    assert!(mgr
        .get_session(session_id)
        .await
        .unwrap()
        .communication
        .is_none());
}
