use super::*;

// ══════════════════════════════════════════════════════════════════════
// #2232 phase 7: Co-managed supervisor tests
// ══════════════════════════════════════════════════════════════════════

/// Test 1: idle edge with the room disabled makes zero Jev calls, writes
/// zero files, queues nothing, and `session_idle` carries `comanaged:false`.
#[tokio::test]
async fn idle_edge_with_a_disabled_room_makes_no_call_and_emits_comanaged_false() {
    let fixture = make_co_managed_fixture();
    let (endpoint, hits) = spawn_jev_listener(vec![("to-peer", 0.9), ("to-user", 0.1)]).await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, false);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let events = capture_events(&app);
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());

    let comanaged = emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    assert!(!comanaged);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "zero Jev calls");
    assert!(messaging_files(&fixture.room_root).is_empty(), "zero files");
    assert!(
        queue_files(&fixture.room_root).is_empty(),
        "zero queue entries"
    );
    let captured = events.lock().unwrap().clone();
    let idle = captured
        .iter()
        .find(|(name, _)| name == "session_idle")
        .expect("session_idle");
    assert_eq!(idle.1["comanaged"], serde_json::Value::Bool(false));
    assert!(
        !captured
            .iter()
            .any(|(name, _)| name == "session_comanaged_state"),
        "a disabled room must not emit the state event"
    );
}

/// Test 2: the enabled room writes exactly one message file before the
/// queue entry, and the pointer resolves to it.
#[tokio::test]
async fn enabled_room_writes_the_message_file_before_the_queue_entry() {
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![
        ("to-peer", 0.9),
        ("to-user", 0.1),
        ("to-root", 0.0),
        ("to-reply", 0.0),
    ])
    .await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let hooks = Arc::new(CoManagedTestHooks::default());
    let (handle, _rx) = CoManagedSupervisorHandle::with_capture_state_and_test_hooks(
        fixture.owners.capture.clone(),
        Arc::clone(&hooks),
    );

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    let files = messaging_files(&fixture.room_root);
    assert_eq!(files.len(), 1, "exactly one message file: {files:?}");
    let queue = queue_files(&fixture.room_root);
    assert_eq!(queue.len(), 1, "exactly one queue entry: {queue:?}");
    assert_eq!(
        hooks.steps(),
        vec!["messaging_file", "queue_file"],
        "the file must be created before the queue entry"
    );
    let envelope: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&queue[0]).unwrap()).unwrap();
    let pointer = envelope["body"].as_str().expect("pointer body");
    let pointer_path = pointer
        .strip_prefix(crate::phone::messaging::FILE_NOTIFICATION_PREFIX)
        .expect("canonical pointer prefix");
    assert_eq!(
        std::fs::canonicalize(pointer_path).unwrap(),
        std::fs::canonicalize(&files[0]).unwrap(),
        "the pointer must resolve to the message file"
    );
    assert_eq!(
        envelope["to"],
        serde_json::json!("proj-a:room-1-dev-team/dev-rust")
    );
    assert_eq!(
        envelope["from"],
        serde_json::json!("proj-a:room-1-dev-team/tech-lead")
    );
    assert!(envelope["token"].is_null(), "the queue carries no token");
    assert!(envelope["action"].is_null());
    assert!(envelope["command"].is_null());
}

/// Test 3: trigger (b) enqueues and emits `active:true`; a disabled room
/// does neither.
#[tokio::test]
async fn a_record_while_already_idle_emits_active_true_and_enqueues_only_when_enabled() {
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![
        ("to-peer", 0.9),
        ("to-user", 0.1),
        ("to-root", 0.0),
        ("to-reply", 0.0),
    ])
    .await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Idle,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let events = capture_events(&app);
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::SlotChanged(session_id),
    )
    .await;

    assert_eq!(queue_files(&fixture.room_root).len(), 1, "(b) enqueues");
    let captured = events.lock().unwrap().clone();
    assert_eq!(captured[0].0, "session_comanaged_state");
    assert_eq!(captured[0].1["active"], serde_json::Value::Bool(true));
    assert!(captured[0].1["reason"].is_null());
    assert_eq!(
        captured.last().unwrap().1["active"],
        serde_json::Value::Bool(false)
    );

    // Disabled room: no queue entry and no active:true.
    let disabled = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![("to-peer", 0.9)]).await;
    let (app2, manager2, registry2) = co_managed_app(&disabled, endpoint, false);
    let session2 = add_claude_session(
        &manager2,
        &disabled.coordinator_cwd,
        SessionStatus::Idle,
        &disabled.projects_dir,
    )
    .await;
    install_candidate(&registry2, session2, "candidate text");
    let events2 = capture_events(&app2);
    let (handle2, _rx2) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
    super::handle_co_managed_trigger(
        app2.handle(),
        &handle2,
        CoManagedTrigger::SlotChanged(session2),
    )
    .await;
    assert!(queue_files(&disabled.room_root).is_empty());
    assert!(!events2
        .lock()
        .unwrap()
        .iter()
        .any(|(_, payload)| payload["active"] == serde_json::Value::Bool(true)));
}

/// Test 4: a non-orchestrator session in an enabled room never triggers.
#[tokio::test]
async fn a_non_orchestrator_session_never_triggers() {
    let fixture = make_co_managed_fixture();
    let (endpoint, hits) = spawn_jev_listener(vec![("to-peer", 0.9)]).await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let session_id = add_claude_session(
        &manager,
        &fixture.dev_cwd,
        SessionStatus::Idle,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::SlotChanged(session_id),
    )
    .await;

    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert!(messaging_files(&fixture.room_root).is_empty());
    assert!(queue_files(&fixture.room_root).is_empty());
    assert!(!handle.armed.is_armed(&session_id.to_string()));
}

/// Test 5: `can_communicate` is applied to peer destinations, and an
/// unreachable peer is rejected instead of routed.
#[test]
fn peer_destinations_apply_can_communicate_and_reject_unreachable_peers() {
    let catalog = crate::capture::catalog::Catalog::from_json_str(
        r#"{"categories":{
            "ok":{"destination":"orchestrator","peer":"proj-a:room-1-dev-team/dev-rust",
            "question":"?"},
            "bad":{"destination":"orchestrator","peer":"proj-b:room-9-dev-team/nobody",
            "question":"?"}
        }}"#,
    );
    let from = "proj-a:room-1-dev-team/tech-lead";
    let reachable = resolve_co_managed_route(
        crate::capture::jev::ClassifyOutcome::Classified {
            category: "ok".to_string(),
            destination: crate::capture::catalog::Destination::Orchestrator,
            score: 0.9,
            runner_up: 0.1,
        },
        &catalog,
        from,
        &[],
        "body",
    );
    match reachable {
        super::CoManagedRoute::Wake { to, .. } => {
            assert_eq!(to, "proj-a:room-1-dev-team/dev-rust")
        }
        _ => panic!("a same-room peer must be reachable"),
    }
    let unreachable = resolve_co_managed_route(
        crate::capture::jev::ClassifyOutcome::Classified {
            category: "bad".to_string(),
            destination: crate::capture::catalog::Destination::Orchestrator,
            score: 0.9,
            runner_up: 0.1,
        },
        &catalog,
        from,
        &[],
        "body",
    );
    match unreachable {
        super::CoManagedRoute::User { reason, .. } => {
            assert!(reason.contains("not reachable"), "{reason}")
        }
        _ => panic!("an unreachable peer must not be routed"),
    }
}

/// Test 6: Root succeeds only for a verified coordinator, and
/// `can_communicate(x, Root)` is false, which is why the branch is needed.
#[tokio::test]
async fn root_destination_uses_the_verified_coordinator_validator_not_can_communicate() {
    let fixture = make_co_managed_fixture();
    let catalog = crate::capture::catalog::Catalog::from_json_str(
        r#"{"categories":{"root":{"destination":"root","question":"?"}}}"#,
    );
    let paths = vec![fixture.project.to_string_lossy().to_string()];
    let verified = "proj-a:room-1-dev-team/tech-lead";
    let outcome = || crate::capture::jev::ClassifyOutcome::Classified {
        category: "root".to_string(),
        destination: crate::capture::catalog::Destination::Root,
        score: 0.9,
        runner_up: 0.1,
    };
    match resolve_co_managed_route(outcome(), &catalog, verified, &paths, "body") {
        super::CoManagedRoute::Wake { to, .. } => {
            assert_eq!(to, crate::config::root_agent::ROOT_AGENT_SENDER)
        }
        _ => panic!("a verified coordinator may route to Root"),
    }
    match resolve_co_managed_route(
        outcome(),
        &catalog,
        "proj-a:room-1-dev-team/dev-rust",
        &paths,
        "body",
    ) {
        super::CoManagedRoute::User { reason, .. } => {
            assert!(
                reason.contains("not a verified room orchestrator"),
                "{reason}"
            )
        }
        _ => panic!("a non-verified sender may not route to Root"),
    }
    assert!(!crate::config::teams::can_communicate(
        verified,
        crate::config::root_agent::ROOT_AGENT_SENDER,
        &crate::config::teams::discover_teams(),
    ));
}

/// Test 9: a 50 KiB candidate reaches the message file byte-for-byte and
/// the user surface carries the path and a capped excerpt.
#[tokio::test]
async fn a_fifty_kib_candidate_reaches_the_file_intact() {
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![
        ("to-user", 0.9),
        ("to-peer", 0.1),
        ("to-root", 0.0),
        ("to-reply", 0.0),
    ])
    .await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    let big = "x".repeat(50 * 1024);
    install_candidate(&registry, session_id, &big);
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    let files = messaging_files(&fixture.room_root);
    assert_eq!(files.len(), 1);
    let written = std::fs::read(&files[0]).unwrap();
    assert_eq!(written.len(), big.len(), "no truncation");
    assert_eq!(written, big.as_bytes(), "byte-for-byte intact");
    let communication = session_communication(&manager, session_id)
        .await
        .expect("user communication");
    assert_eq!(
        communication.kind,
        crate::session::session::SessionCommunicationKind::CoManaged
    );
    let message = communication.message.expect("message text");
    assert!(
        message.contains(&files[0].display().to_string()),
        "{message}"
    );
    assert!(message.contains("51200 bytes"), "{message}");
}

/// Test 10: a pointer that does not fit is rejected with a visible reason;
/// nothing is truncated and no partial message is queued.
#[tokio::test]
async fn a_pointer_that_does_not_fit_is_rejected_and_nothing_is_queued() {
    // Deep, long path components push the absolute pointer over PTY_SAFE_MAX.
    //
    // The deep components are created FIRST and the root is canonicalized
    // only afterwards, so Windows keeps the `\?\` verbatim form. The
    // readiness gate repairs the replica `config.json` through the
    // pre-existing `local_config_io` publisher, whose Windows step calls
    // `ReplaceFileW`; a path beyond MAX_PATH is only reachable there when it
    // is verbatim (Windows CI otherwise fails with os error 3, and the gate
    // answers `NotAnOrchestrator` before the pointer check). The pointer
    // budget invariant asserted below is the same on every OS.
    let long = "d".repeat(200);
    let temp = tempfile::TempDir::new().unwrap();
    let mut deep = std::fs::canonicalize(temp.path()).unwrap();
    for _ in 0..5 {
        deep = deep.join(&long);
    }
    std::fs::create_dir_all(&deep).unwrap();
    let root = std::fs::canonicalize(&deep).unwrap();
    let fixture = make_co_managed_fixture_in(temp, root);
    let (endpoint, _hits) = spawn_jev_listener(vec![
        ("to-peer", 0.9),
        ("to-user", 0.1),
        ("to-root", 0.0),
        ("to-reply", 0.0),
    ])
    .await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    assert!(queue_files(&fixture.room_root).is_empty(), "nothing queued");
    assert!(
        messaging_files(&fixture.room_root).is_empty(),
        "the over-long file is removed, never delivered"
    );
    let communication = session_communication(&manager, session_id)
        .await
        .expect("visible reason");
    let message = communication.message.expect("message text");
    assert!(message.contains("PTY-safe"), "{message}");
}

/// Test 11: the excerpt is capped at 500 bytes and never splits a
/// multi-byte character.
#[test]
fn the_excerpt_is_capped_at_500_bytes_without_splitting_a_character() {
    // One 300-byte line of 'a', then a multi-byte character straddling the
    // 500-byte boundary, then more text.
    let mut text = "a".repeat(300);
    text.push_str(&"\u{e9}".repeat(200));
    text.push_str("tail");
    let excerpt = utf8_safe_excerpt(&text, super::CO_MANAGED_EXCERPT_BYTES);
    assert!(excerpt.len() <= 500, "excerpt is {} bytes", excerpt.len());
    assert!(text.starts_with(&excerpt));
    assert!(text.is_char_boundary(excerpt.len()));
    assert!(!excerpt.is_empty());

    let exact = "b".repeat(500);
    assert_eq!(utf8_safe_excerpt(&exact, 500), exact, "no cut at the cap");
}

/// Test 12: flipping the room flag off during the classifier call produces
/// zero effects.
#[tokio::test]
async fn flipping_the_room_flag_during_the_classifier_call_produces_zero_effects() {
    let fixture = make_co_managed_fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let endpoint = format!("http://127.0.0.1:{port}/v1/systemone");
    let request_received = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&request_received);
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let body = jev_response_body(&[("to-peer", 0.9)]);
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            read_http_request(&mut stream).await;
            notify.notify_one();
            let _ = release_rx.await;
            write_http_json(&mut stream, &body).await;
        }
    });

    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
    let trigger_handle = app.handle().clone();
    let supervisor = handle.clone();
    let task = tokio::spawn(async move {
        super::handle_co_managed_trigger(
            &trigger_handle,
            &supervisor,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;
    });

    request_received.notified().await;
    crate::config::co_managed::set_enabled(&fixture.room_root, false).unwrap();
    let _ = release_tx.send(());
    task.await.unwrap();

    assert!(messaging_files(&fixture.room_root).is_empty());
    assert!(queue_files(&fixture.room_root).is_empty());
    let communication = session_communication(&manager, session_id).await;
    assert!(communication.is_none(), "no user-facing effect either");
}

/// Test 13: budget exhaustion abstains, sends text to the user, and
/// enqueues nothing.
#[tokio::test]
async fn budget_exhaustion_abstains_sends_text_and_enqueues_nothing() {
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![
        ("to-peer", 0.9),
        ("to-user", 0.1),
        ("to-root", 0.0),
        ("to-reply", 0.0),
    ])
    .await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    // Three automatic actions already spent since the last recharge.
    let state_path =
        crate::config::co_managed::co_managed_dir(&fixture.room_root).join("state.json");
    std::fs::write(
        &state_path,
        r#"{"spends_since_recharge":3,"recharge_stamp":1}"#,
    )
    .unwrap();
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    assert!(
        queue_files(&fixture.room_root).is_empty(),
        "nothing enqueued"
    );
    let communication = session_communication(&manager, session_id)
        .await
        .expect("text to the user");
    let message = communication.message.expect("message text");
    assert!(message.to_lowercase().contains("budget"), "{message}");
}

mod logs;
mod race;
