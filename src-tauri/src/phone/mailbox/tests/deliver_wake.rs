use super::*;

/// M1: a successful create answers the request with a `created` result
/// sidecar carrying the session id, and deletes the request.
// clippy (1.98) `await_holding_lock`: the guard is the test-only
// cross-test serialization lock (a std `Mutex` on purpose — the sync
// C1-C5 tests in `cli::create_agent` share the same static) and must be
// held across the poll await; the tests run single-threaded per process.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn session_request_result_sidecar_written_on_success() {
    // #1163: `config_dir()` is process-wide — one shared session-requests
    // dir for every C/M artifact test. Serialize so a poll never consumes
    // another test's request file mid-test.
    let _guard = crate::cli::create_agent::SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let fixture = make_session_request_fixture();
    let app = app_handle(&fixture.app);
    let request = write_session_request_for_test(&fixture.cwd, "codex");

    MailboxPoller::default().poll_session_requests(&app).await;

    let result = read_session_request_result_json(&request.id);
    assert_eq!(result["status"], "created", "unexpected result: {}", result);
    let session_id = result["sessionId"].as_str().expect("sessionId").to_string();
    assert!(!session_id.is_empty());
    assert!(
        !session_request_file_for(&request.id).exists(),
        "request must be deleted after processing"
    );
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(
        rows.len(),
        1,
        "the session was really created; rows: {:?}",
        rows.iter()
            .map(|s| (s.id.clone(), s.name.clone(), s.working_directory.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(rows[0].id, session_id);
    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

/// M2: a failed create (PTY spawn error) answers with a `rejected` result
/// carrying the error text, and deletes the request.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn session_request_result_sidecar_written_on_rejection() {
    let _guard = crate::cli::create_agent::SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let fixture = make_session_request_fixture();
    let app = app_handle(&fixture.app);
    let request = write_session_request_for_test(&fixture.cwd, "codex");
    fixture.backend.fail_spawn.store(true, Ordering::SeqCst);

    MailboxPoller::default().poll_session_requests(&app).await;

    let result = read_session_request_result_json(&request.id);
    assert_eq!(result["status"], "rejected");
    let error = result["error"].as_str().expect("error text");
    assert!(error.contains("synthetic spawn failure"), "{error}");
    assert!(
        !session_request_file_for(&request.id).exists(),
        "request must be deleted after processing"
    );
    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

/// M3: a malformed request file gets a `rejected` result keyed by its
/// filename stem, and the request file is deleted.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn malformed_request_writes_rejected_result() {
    let _guard = crate::cli::create_agent::SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let fixture = make_session_request_fixture();
    let app = app_handle(&fixture.app);
    let id = uuid::Uuid::new_v4().to_string();
    let requests_dir = crate::config::config_dir()
        .expect("config dir")
        .join(SESSION_REQUESTS_DIR_NAME);
    std::fs::create_dir_all(&requests_dir).unwrap();
    std::fs::write(requests_dir.join(format!("{id}.json")), "{ not json !!").unwrap();

    MailboxPoller::default().poll_session_requests(&app).await;

    let result = read_session_request_result_json(&id);
    assert_eq!(result["status"], "rejected");
    let error = result["error"].as_str().expect("error text");
    assert!(error.contains("malformed session request"), "{error}");
    assert!(
        !session_request_file_for(&id).exists(),
        "malformed request must be deleted"
    );
}

/// M4: a lone result sidecar is never consumed as a request (no delete,
/// no spurious rejected result for it).
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn result_sidecars_are_not_consumed_as_requests() {
    let _guard = crate::cli::create_agent::SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let fixture = make_session_request_fixture();
    let app = app_handle(&fixture.app);
    let id = uuid::Uuid::new_v4().to_string();
    let result_path = session_request_result_file_for(&id);
    std::fs::create_dir_all(result_path.parent().unwrap()).unwrap();
    std::fs::write(
        &result_path,
        format!(r#"{{"id":"{id}","status":"created","sessionId":"sess-1"}}"#),
    )
    .unwrap();

    MailboxPoller::default().poll_session_requests(&app).await;

    assert!(
        result_path.exists(),
        "a result sidecar is an ANSWER, never a request; it must stay untouched"
    );
    assert_eq!(
        std::fs::read_to_string(&result_path).unwrap(),
        format!(r#"{{"id":"{id}","status":"created","sessionId":"sess-1"}}"#)
    );
}

/// M5: result sidecars older than 5 minutes are swept; fresh ones are
/// kept.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn stale_result_sidecars_are_swept() {
    let _guard = crate::cli::create_agent::SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let fixture = make_session_request_fixture();
    let app = app_handle(&fixture.app);
    let stale_id = uuid::Uuid::new_v4().to_string();
    let fresh_id = uuid::Uuid::new_v4().to_string();
    let stale_path = session_request_result_file_for(&stale_id);
    let fresh_path = session_request_result_file_for(&fresh_id);
    std::fs::create_dir_all(stale_path.parent().unwrap()).unwrap();
    for (path, id) in [(&stale_path, &stale_id), (&fresh_path, &fresh_id)] {
        std::fs::write(
            path,
            format!(r#"{{"id":"{id}","status":"created","sessionId":"sess-1"}}"#),
        )
        .unwrap();
    }
    // Age the stale one 6 minutes (mtime only; `filetime` is not a
    // dependency, `std::fs::File::set_times` is stable). Windows requires
    // a WRITE-capable handle to change times.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&stale_path)
        .unwrap();
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(6 * 60)),
    )
    .unwrap();

    MailboxPoller::default().poll_session_requests(&app).await;

    assert!(!stale_path.exists(), "stale result sidecar must be swept");
    assert!(fresh_path.exists(), "fresh result sidecar must be kept");
}

#[tokio::test]
async fn deliver_wake_terminal_pending_verdict_yields_err_after_inject() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let live_id =
        add_mailbox_session(&app, &fixture.target_cwd, "live", SessionStatus::Idle, None).await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(live_id, true);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    hooks
        .consumption_results
        .lock()
        .unwrap()
        .push_back(ConsumptionVerdict::Pending);
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let msg = wake_message_to_target();

    let result = poller
        .deliver_wake_with_origin(&app, &msg, WakeDeliveryOrigin::FilesystemPoller)
        .await;

    assert!(
        result.is_err(),
        "a terminal Pending verdict must convert to Err (drives redelivery), got {:?}",
        result
    );
    // The inject ran exactly once, against the live candidate.
    assert_eq!(*hooks.inject_calls.lock().unwrap(), vec![live_id]);
    assert_inject_results_consumed(&hooks);
    assert!(
        hooks.consumption_results.lock().unwrap().is_empty(),
        "the scripted verdict must be consumed by the hook arm"
    );
}

#[tokio::test]
async fn deliver_wake_observed_verdict_yields_ok_single_delivery() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let live_id =
        add_mailbox_session(&app, &fixture.target_cwd, "live", SessionStatus::Idle, None).await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(live_id, true);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    hooks
        .consumption_results
        .lock()
        .unwrap()
        .push_back(ConsumptionVerdict::Observed);
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let msg = wake_message_to_target();

    let result = poller
        .deliver_wake_with_origin(&app, &msg, WakeDeliveryOrigin::FilesystemPoller)
        .await;

    assert!(
        result.is_ok(),
        "Observed must convert to Ok, got {:?}",
        result
    );
    assert_eq!(*hooks.inject_calls.lock().unwrap(), vec![live_id]);
    assert_no_spawn_or_destroy_events(&hooks);
}

#[tokio::test]
async fn deliver_wake_without_scripted_verdict_stays_ok_unchanged() {
    // Back-compat: existing hooked tests script no consumption_results, so
    // the hook arm must default to Ok(()) (write-receipt), unchanged.
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let live_id =
        add_mailbox_session(&app, &fixture.target_cwd, "live", SessionStatus::Idle, None).await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(live_id, true);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let msg = wake_message_to_target();

    let result = poller
        .deliver_wake_with_origin(&app, &msg, WakeDeliveryOrigin::FilesystemPoller)
        .await;

    assert!(result.is_ok());
    assert_eq!(*hooks.inject_calls.lock().unwrap(), vec![live_id]);
}

#[tokio::test]
async fn deliver_wake_respawns_exited_deferred_session_with_resume_flag() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let exited_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "deferred",
        SessionStatus::Exited(0),
        None,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(exited_id, false);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-respawn");

    run_mailbox_message(&app, &message_path, hooks.clone()).await;

    assert_eq!(*hooks.destroy_calls.lock().unwrap(), vec![exited_id]);
    let spawn_calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(spawn_calls.len(), 1);
    assert!(!spawn_calls[0].skip_auto_resume);
    assert_spawn_call_matches_target(&spawn_calls[0], &fixture);

    let injected = hooks.inject_calls.lock().unwrap().clone();
    assert_eq!(injected.len(), 1);
    assert_ne!(injected[0], exited_id);
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    assert!(mgr.get_session(exited_id).await.is_none());
    drop(mgr);
    assert_spawned_session_matches_target(&app, injected[0], &fixture).await;
    assert_inject_results_consumed(&hooks);
}

#[tokio::test]
async fn deliver_wake_respawns_exited_deferred_session_preserving_telegram_intent() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let exited_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "deferred",
        SessionStatus::Exited(0),
        Some("bot-1"),
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(exited_id, false);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-telegram");

    run_mailbox_message(&app, &message_path, hooks.clone()).await;

    assert_eq!(*hooks.destroy_calls.lock().unwrap(), vec![exited_id]);
    let spawn_calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(spawn_calls.len(), 1);
    assert!(!spawn_calls[0].skip_auto_resume);
    assert_spawn_call_matches_target(&spawn_calls[0], &fixture);
    let injected = hooks.inject_calls.lock().unwrap().clone();
    assert_eq!(injected.len(), 1);
    let new_session_id = injected[0];
    assert_ne!(new_session_id, exited_id);
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    assert!(mgr.get_session(exited_id).await.is_none());
    drop(mgr);
    assert_spawned_session_matches_target(&app, new_session_id, &fixture).await;
    assert_eq!(
        *hooks.attach_calls.lock().unwrap(),
        vec![(new_session_id, Some("bot-1".into()))]
    );
    let events = hooks.events.lock().unwrap().clone();
    let attach_pos = events
        .iter()
        .position(|event| {
            *event
                == MailboxTestEvent::Attach {
                    session_id: new_session_id,
                    bot_id: Some("bot-1".into()),
                }
        })
        .unwrap();
    let inject_pos = events
        .iter()
        .position(|event| *event == MailboxTestEvent::Inject(new_session_id))
        .unwrap();
    assert!(attach_pos < inject_pos);
    assert_inject_results_consumed(&hooks);
}

/// (#747) A dormant coordinator restored with a raised hand (Exited(0) +
/// visible communication, exactly what the startup defer arm produces)
/// receives a peer wake: the RespawnExited destroy+spawn must carry the
/// hand onto the NEW session (the wake injection is not user input) and
/// emit one `session_communication_changed` for it. The spawn hook is told
/// to create a coordinator record, mirroring production where
/// `create_session_inner` recomputes `is_coordinator` from teams discovery
/// for the same cwd.
#[tokio::test]
async fn deliver_wake_respawn_carries_restored_raise_hand_to_new_session() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let (exited_id, _token) =
        seed_raise_hand_session(&app, &fixture.target_cwd, true, SessionStatus::Exited(0)).await;
    let original_raise_time = "2026-07-01T10:00:00+00:00".to_string();
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        assert!(
            mgr.restore_communication(
                exited_id,
                SessionCommunication {
                    kind: SessionCommunicationKind::RaiseHand,
                    visible: true,
                    updated_at: original_raise_time.clone(),
                    message: None,
                },
            )
            .await,
            "seeding the dormant-restored hand must succeed"
        );
    }
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&events);
    fixture
        .app
        .listen_any("session_communication_changed", move |event| {
            captured.lock().unwrap().push(event.payload().to_string());
        });

    let hooks = MailboxTestHooks::default();
    *hooks.spawn_is_coordinator.lock().unwrap() = true;
    hooks.pty_presence.lock().unwrap().insert(exited_id, false);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-respawn-hand");

    run_mailbox_message(&app, &message_path, hooks.clone()).await;

    assert_eq!(*hooks.destroy_calls.lock().unwrap(), vec![exited_id]);
    let spawn_calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(spawn_calls.len(), 1);
    assert!(!spawn_calls[0].skip_auto_resume);
    let injected = hooks.inject_calls.lock().unwrap().clone();
    assert_eq!(injected.len(), 1);
    let new_session_id = injected[0];
    assert_ne!(new_session_id, exited_id);

    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    assert!(
        mgr.get_session(exited_id).await.is_none(),
        "the orphan record was destroyed; exactly one carrier remains"
    );
    let spawned = mgr
        .get_session(new_session_id)
        .await
        .expect("spawned session record");
    let carried = spawned
        .communication
        .expect("the respawned session must carry the restored hand");
    assert_eq!(carried.kind, SessionCommunicationKind::RaiseHand);
    assert!(carried.visible);
    assert_eq!(
        carried.updated_at, original_raise_time,
        "the carry must preserve the original raise time"
    );
    drop(mgr);

    let captured = events.lock().unwrap().clone();
    assert_eq!(
        captured.len(),
        1,
        "exactly one communication event (the carry re-apply): {captured:?}"
    );
    let event: serde_json::Value = serde_json::from_str(&captured[0]).unwrap();
    assert_eq!(event["sessionId"], new_session_id.to_string());
    assert_eq!(event["communication"]["kind"], "raiseHand");
    assert_eq!(event["communication"]["visible"], true);
    assert_inject_results_consumed(&hooks);
}

#[tokio::test]
async fn deliver_wake_promotes_resume_when_all_live_candidates_race_to_dead() {
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
        results.push_back(Err(format!(
            "PTY write failed: Session not found: {}",
            second_id
        )));
        results.push_back(Ok(()));
    }
    let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-promote");

    run_mailbox_message(&app, &message_path, hooks.clone()).await;

    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    let spawn_calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(spawn_calls.len(), 1);
    assert!(!spawn_calls[0].skip_auto_resume);
    assert_spawn_call_matches_target(&spawn_calls[0], &fixture);
    let injected = hooks.inject_calls.lock().unwrap().clone();
    assert_eq!(injected.len(), 3);
    assert_eq!(&injected[0..2], &[first_id, second_id]);
    assert_ne!(injected[2], first_id);
    assert_ne!(injected[2], second_id);
    assert_spawned_session_matches_target(&app, injected[2], &fixture).await;
    assert_inject_results_consumed(&hooks);
}

// Includes a non-BMP codepoint (😀 U+1F600 → surrogate pair D83D DE00) so the
// UTF-16 LE/BE BOM tests exercise surrogate-pair decoding, not just BMP.
const SAMPLE_JSON: &str = r#"{"id":"abc","kind":"ping","emoji":"😀"}"#;

fn write_temp(bytes: &[u8]) -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("tempfile");
    f.write_all(bytes).expect("write");
    f.flush().expect("flush");
    f
}

#[test]
fn bom_tolerant_reads_plain_utf8() {
    let f = write_temp(SAMPLE_JSON.as_bytes());
    let got = read_text_bom_tolerant(f.path()).expect("read");
    assert_eq!(got, SAMPLE_JSON);
    // Parses as JSON.
    let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
}

#[test]
fn bom_tolerant_strips_utf8_bom() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(SAMPLE_JSON.as_bytes());
    let f = write_temp(&bytes);
    let got = read_text_bom_tolerant(f.path()).expect("read");
    assert_eq!(got, SAMPLE_JSON);
    let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
}

#[test]
fn bom_tolerant_decodes_utf16_le_bom() {
    let mut bytes = vec![0xFF, 0xFE];
    for u in SAMPLE_JSON.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    let f = write_temp(&bytes);
    let got = read_text_bom_tolerant(f.path()).expect("read");
    assert_eq!(got, SAMPLE_JSON);
    let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
}

#[test]
fn bom_tolerant_decodes_utf16_be_bom() {
    let mut bytes = vec![0xFE, 0xFF];
    for u in SAMPLE_JSON.encode_utf16() {
        bytes.extend_from_slice(&u.to_be_bytes());
    }
    let f = write_temp(&bytes);
    let got = read_text_bom_tolerant(f.path()).expect("read");
    assert_eq!(got, SAMPLE_JSON);
    let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
}

#[test]
fn bom_tolerant_rejects_invalid_utf8_no_bom() {
    // Lone continuation byte 0x80 is invalid UTF-8 and not a recognized BOM.
    let f = write_temp(&[0x80, 0x81, 0x82]);
    let err = read_text_bom_tolerant(f.path()).expect_err("must err");
    assert!(err.contains("Invalid UTF-8"), "unexpected error: {}", err);
}

#[test]
fn bom_tolerant_empty_file_returns_empty_string() {
    let f = write_temp(&[]);
    let got = read_text_bom_tolerant(f.path()).expect("empty ok");
    assert_eq!(got, "");
    // Serde fails with a parse error (NOT a read error) — confirms the failure
    // surfaces at the parser, which is what the callsites wrap with their own
    // context strings.
    let parse_err = serde_json::from_str::<serde_json::Value>(&got).expect_err("must err");
    assert!(
        parse_err.is_eof(),
        "expected EOF parse error, got: {}",
        parse_err
    );
}

/// §130-stuck-file regression: when the reject path receives a file whose
/// bytes are non-UTF-8 and have no BOM (e.g. PowerShell `Set-Content
/// -Encoding ANSI` from a CP1252 locale), `read_text_bom_tolerant` returns
/// `Err` every poll cycle. Before this fix, the reject branch was guarded
/// by `if let Ok(content) = ...` and dropped to `else { false }` on Err —
/// the file stayed in the source dir at `attempt_count >= MAX`, looping
/// forever. The new `Err(_) => reject_raw_file(...)` arm closes that gap.
/// This test drives the fallback directly: an unreadable file is moved to
/// `rejected/` and a reason file is written, exactly as the new branch does.
#[test]
fn reject_raw_file_moves_unreadable_outbox_file_to_rejected_dir() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let outbox = tmp.path();
    let stuck = outbox.join("stuck.json");
    // CP1252 high-byte sequence — invalid UTF-8, no BOM.
    std::fs::write(&stuck, [0x80, 0x81, 0x82]).expect("write stuck file");

    // Precondition: this is exactly the input shape that hits the new Err arm.
    let read_err = read_text_bom_tolerant(&stuck).expect_err("read must err");
    assert!(
        read_err.contains("Invalid UTF-8"),
        "unexpected error: {}",
        read_err
    );

    // Drive the new fallback path.
    MailboxPoller::reject_raw_file(
        &stuck,
        "Undeliverable after 10 attempts. Last error: Failed to read outbox file: Invalid UTF-8",
    )
    .expect("reject_raw_file ok");

    assert!(
        !stuck.exists(),
        "original file should be moved out of source dir"
    );
    let rejected_dir = outbox.join("rejected");
    assert!(rejected_dir.is_dir(), "rejected/ should be created");
    assert!(
        rejected_dir.join("stuck.json").is_file(),
        "file should be moved to rejected/stuck.json"
    );
    let reason_file = rejected_dir.join("stuck.reason.txt");
    assert!(reason_file.is_file(), "reason file should be in rejected/");
    let reason = std::fs::read_to_string(&reason_file).expect("read reason");
    assert!(
        reason.contains("Undeliverable"),
        "reason content: {}",
        reason
    );
}

#[test]
fn gate_rejects_when_any_peer_busy() {
    let quiet = Duration::from_millis(3000);
    let sid_a = Uuid::new_v4();
    let peer_a = make_gate_peer(
        "proj:wg-1/devs/alice",
        vec![PurgeGateSession {
            session_id: sid_a,
            readiness: make_readiness(sid_a, Some(500), true, None),
            mirror_idle: true,
        }],
        vec!["aaa".to_string()],
    );
    let sid_b = Uuid::new_v4();
    let peer_b = make_gate_peer(
        "proj:wg-1/devs/bob",
        vec![PurgeGateSession {
            session_id: sid_b,
            readiness: make_readiness(sid_b, Some(100), true, None),
            mirror_idle: true,
        }],
        vec!["bbb".to_string()],
    );
    let decision = evaluate_gate(&[peer_a, peer_b], quiet);
    assert!(!decision.passed, "gate must reject when a peer is busy");
    assert!(
        !decision.peers.iter().any(|p| p.outcome == "closed"),
        "no peer should be 'closed' on a rejected gate"
    );
}

#[test]
fn gate_treats_no_live_session_peer_as_purgeable() {
    let quiet = Duration::from_millis(3000);
    let peer = make_gate_peer(
        "proj:wg-1/devs/alice",
        vec![], // no live sessions
        vec!["aaa".to_string()],
    );
    let decision = evaluate_gate(&[peer], quiet);
    assert!(
        decision.passed,
        "a peer with no live sessions is vacuously purgeable"
    );
    assert!(decision.peers[0].purgeable);
}

#[test]
fn gate_reports_untracked_not_busy() {
    let quiet = Duration::from_millis(3000);
    let sid = Uuid::new_v4();
    let peer = make_gate_peer(
        "proj:wg-1/devs/alice",
        vec![PurgeGateSession {
            session_id: sid,
            readiness: make_readiness(sid, None, false, None),
            mirror_idle: true,
        }],
        vec!["aaa".to_string()],
    );
    let decision = evaluate_gate(&[peer], quiet);
    assert!(!decision.passed);
    assert_eq!(
        decision.peers[0].outcome, "untracked",
        "a live record with activity_age: None must be 'untracked', not 'busy'"
    );
}

#[test]
fn gate_rejects_when_mirror_disagrees() {
    let quiet = Duration::from_millis(3000);
    let sid = Uuid::new_v4();
    let peer = make_gate_peer(
        "proj:wg-1/devs/alice",
        vec![PurgeGateSession {
            session_id: sid,
            readiness: make_readiness(sid, Some(5000), true, None),
            mirror_idle: false, // disagrees
        }],
        vec!["aaa".to_string()],
    );
    let decision = evaluate_gate(&[peer], quiet);
    assert!(!decision.passed, "mirror disagreement must reject");
}

/// (#885 F-1) The acceptance test: a peer inside resize settlement must
/// be rejected even if activity_age, watcher_idle, and mirror_idle all
/// agree "idle".
#[test]
fn gate_rejects_peer_inside_resize_settlement() {
    let quiet = Duration::from_millis(3000);
    // activity_age = 3s (>= quiet), watcher_idle = true, mirror_idle = true.
    // last_resize_age = 3.1s, resize_grace = 3s, effective_quiet = 3s.
    // resize_settled requires last_resize_age >= resize_grace + effective_quiet = 6s.
    // 3.1s < 6s => not settled => must reject.
    let sid = Uuid::new_v4();
    let peer = make_gate_peer(
        "proj:wg-1/devs/alice",
        vec![PurgeGateSession {
            session_id: sid,
            readiness: make_readiness(sid, Some(3000), true, Some(3100)),
            mirror_idle: true,
        }],
        vec!["aaa".to_string()],
    );
    let decision = evaluate_gate(&[peer], quiet);
    assert!(
        !decision.passed,
        "F-1: a peer inside resize settlement must be rejected"
    );
    assert!(
        !decision.peers[0].resize_settled,
        "resize_settled must be false"
    );
}

#[test]
fn gate_reports_busy_when_same_peer_has_busy_and_resize_unsettled_sessions() {
    let quiet = Duration::from_millis(3000);
    let busy_sid = Uuid::new_v4();
    let resize_sid = Uuid::new_v4();
    let peer = make_gate_peer(
        "proj:wg-1/devs/alice",
        vec![
            PurgeGateSession {
                session_id: busy_sid,
                readiness: make_readiness(busy_sid, Some(100), true, None),
                mirror_idle: true,
            },
            PurgeGateSession {
                session_id: resize_sid,
                readiness: make_readiness(resize_sid, Some(3000), true, Some(3100)),
                mirror_idle: true,
            },
        ],
        vec![busy_sid.to_string(), resize_sid.to_string()],
    );

    let decision = evaluate_gate(&[peer], quiet);

    assert!(!decision.passed);
    assert_eq!(
        decision.peers[0].outcome, "busy",
        "resize_unsettled must not mask a busy live session on the same peer"
    );
}

#[cfg(not(windows))]
fn rejection_devices() -> Vec<String> {
    let mut ids: Vec<_> = "CON|PRN|AUX|CONIN$|CONOUT$|cOn.txt|COM1 .txt"
        .split('|')
        .map(str::to_owned)
        .collect();
    for prefix in ["COM", "LPT"] {
        for digit in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"] {
            ids.push(format!("{prefix}{digit}"));
        }
    }
    ids
}
fn rejection_inventory(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, dir: &Path, rows: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().components().collect();
            if path.is_dir() {
                rows.push((relative, Vec::new()));
                visit(root, &path, rows);
            } else {
                rows.push((relative, std::fs::read(path).unwrap()));
            }
        }
    }
    let mut rows = Vec::new();
    visit(root, root, &mut rows);
    rows.sort();
    rows
}
fn rejection_case(id: &str, raw: bool, failure: u8) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let inbox = root.join("inbox");
    let rejected = inbox.join("rejected");
    std::fs::create_dir_all(&rejected).unwrap();
    std::fs::write(root.join("sentinel"), b"unchanged").unwrap();
    let ads_file = root.join("ads-file");
    std::fs::write(&ads_file, b"regular-file").unwrap();
    let id = match id {
        "@absolute" => root.join("absolute").to_string_lossy().into_owned(),
        "@drive-absolute" => {
            root.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_owned()
                + r"\drive-absolute"
        }
        "@verbatim" => root.to_string_lossy().into_owned() + r"\verbatim",
        "@ads" => ads_file.to_string_lossy().into_owned() + ":stream",
        other => other.to_owned(),
    };
    #[cfg(windows)]
    if id.contains(":stream") {
        for suffix in [".json", ".reason.txt"] {
            std::fs::write(format!("{id}{suffix}"), b"stream-sentinel").unwrap();
        }
    }
    let source = inbox.join("actual-source.json");
    let mut msg = wake_message_to_target();
    msg.id = id.clone();
    msg.token = Some("retained-secret".into());
    let bytes = serde_json::to_vec(&msg).unwrap();
    if failure != 1 {
        std::fs::write(&source, &bytes).unwrap();
    }
    if failure == 2 {
        std::fs::create_dir(rejected.join("actual-source.reason.txt")).unwrap();
    }
    let mut expected = rejection_inventory(&root);
    let poller = MailboxPoller::delivery_only_for_tests();
    let state = crate::pty::manager::ResponseCloseState::default();
    let mut admission = state.reserve_work(&msg.to, "rejection-work").unwrap();
    admission.accept_durable("digest").unwrap();
    drop(admission);
    poller.response_sources.lock().unwrap().insert(
        source.clone(),
        ResponseWakeWork {
            diagnostic_message_id: None,
            state,
            target: msg.to.clone(),
            key: "rejection-work".into(),
            digest: "digest".into(),
            attempt: Arc::default(),
            return_context: None,
            return_routes: None,
            delivery_context: None,
        },
    );
    let result = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(poller.reject_message(&source, &msg, "unchanged reason"));
    if failure == 1 {
        let missing = inbox.join("other-missing.json");
        assert_eq!(
            result,
            MailboxPoller::reject_raw_file(&missing, "unchanged reason")
        );
    } else {
        expected.retain(|(path, _)| path != Path::new("inbox/actual-source.json"));
        let archive = if raw { "actual-source" } else { &id };
        let archived_bytes = if raw {
            bytes
        } else {
            msg.token = None;
            serde_json::to_string_pretty(&msg).unwrap().into_bytes()
        };
        expected.push((
            PathBuf::from(format!("inbox/rejected/{archive}.json")),
            archived_bytes,
        ));
        if failure == 2 {
            assert_eq!(result, Err("Failed to write reason file".into()));
        } else {
            assert_eq!(result, Ok(()));
            expected.push((
                PathBuf::from(format!("inbox/rejected/{archive}.reason.txt")),
                b"unchanged reason".to_vec(),
            ));
        }
    }
    for (path, _) in &mut expected {
        *path = path.components().collect();
    }
    expected.sort();
    assert_eq!(rejection_inventory(&root), expected, "id={id:?}");
    let sources = poller.response_sources.lock().unwrap();
    let unsettled = sources.contains_key(&source);
    assert_eq!(unsettled, failure != 0);
    #[cfg(windows)]
    if id.contains(":stream") {
        for suffix in [".json", ".reason.txt"] {
            assert_eq!(
                std::fs::read(format!("{id}{suffix}")).unwrap(),
                b"stream-sentinel"
            );
        }
    }
}
fn rejection_table(ids: Vec<String>, raw: bool) {
    let mut failures = Vec::new();
    for id in ids {
        match std::panic::catch_unwind(|| rejection_case(&id, raw, 0)) {
            Ok(()) => println!("PASS id={id:?}"),
            Err(_) => failures.push(id),
        }
    }
    assert!(
        failures.is_empty(),
        "fallback assertions failed for ids: {failures:?}"
    );
}
#[test]
fn reject_message_hostile_ids_use_source_basename() {
    let mut ids: Vec<_> = "|.|..|../escape|nested/slash|nested\\backslash|@absolute|NUL"
        .split('|')
        .map(str::to_owned)
        .collect();
    ids.push(String::from(char::from(0)));
    ids.extend(
        ['<', '>', '"', '|', '?', '*']
            .into_iter()
            .chain('\u{1}'..='\u{1f}')
            .map(|c| format!("a{c}b")),
    );
    ids.extend(["a".repeat(245), "a".repeat(251), "雪".repeat(81) + "ab"]);
    #[cfg(not(windows))]
    {
        ids.extend(
            [
                "C:relative",
                "C:/absolute",
                r"C:\absolute",
                r"\\server\share\escape",
                r"\\?\C:\escape",
                r"\\.\COM1",
                "a:b",
            ]
            .map(str::to_owned),
        );
        ids.extend(rejection_devices());
    }
    #[cfg(windows)]
    ids.extend(["@drive-absolute", "@verbatim", "@ads"].map(str::to_owned));
    rejection_table(ids, true);
}
#[test]
fn reject_message_valid_id_keeps_stripped_archive_and_reason() {
    rejection_case("8f9d3c54-51c2-4f64-9c54-59afdbde150e", false, 0);
}
#[test]
fn reject_message_plain_non_uuid_id_keeps_existing_behavior() {
    rejection_table(
        "msg-sr-origin-switch|embedded.dot|雪|with space|tail.|tail "
            .split('|')
            .map(str::to_owned)
            .chain([
                "\u{7f}".into(),
                "\u{85}".into(),
                "a".repeat(244),
                "雪".repeat(81) + "a",
            ])
            .collect(),
        false,
    );
}
#[test]
fn reject_message_invalid_id_fallback_error_preserves_error() {
    for failure in [1, 2] {
        rejection_case("../escape", true, failure);
    }
}
