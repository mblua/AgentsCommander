use super::*;

/// Arms the post-commit race: the hook offers `next` into `slot` inside the window the
/// production sites consume in. Returns the offered record.
fn arm_post_commit_offer(
    hooks: &Arc<CoManagedTestHooks>,
    slot: &CaptureSlot,
    next: Arc<CapturedRecord>,
) -> Arc<CapturedRecord> {
    let slot = slot.clone();
    let offered = Arc::clone(&next);
    hooks.set_after_commit(Box::new(move || {
        assert_eq!(
            slot.offer(next, 0),
            crate::capture::sink::OfferOutcome::Published
        );
    }));
    offered
}

/// Test 33a: a record offered between the secret-path commit and the
/// consume survives it and is processed by the next cycle.
#[tokio::test]
async fn a_newer_candidate_survives_the_secret_path_commit() {
    let fixture = make_co_managed_fixture();
    let (app, manager, registry) = co_managed_app(&fixture, "http://127.0.0.1:9".to_string(), true);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    let (slot, _first) = install_candidate(&registry, session_id, "leak AKIAIOSFODNN7EXAMPLE one");
    let first_seq = slot.snapshot().seq;
    let hooks = Arc::new(CoManagedTestHooks::default());
    let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));
    let next = co_managed_candidate(&session_id.to_string(), "leak AKIAIOSFODNN7EXAMPLE two");
    let offered = arm_post_commit_offer(&hooks, &slot, next);

    let trigger = CoManagedTrigger::IdleEdge(session_id);
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;

    let state = slot.snapshot();
    assert_eq!(
        state.seq,
        first_seq + 1,
        "the cycle must not clear the newer offer"
    );
    let held = state.value.record().expect("the newer record survives");
    assert!(Arc::ptr_eq(held, &offered));
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
    assert!(
        slot.snapshot().value.record().is_none(),
        "the next cycle consumed it"
    );
    assert!(messaging_files(&fixture.room_root).is_empty());
    assert!(queue_files(&fixture.room_root).is_empty());
}

/// Test 33b: a record offered between the automatic-path commit and the
/// consume survives it and is routed by the next cycle.
#[tokio::test]
async fn a_newer_candidate_survives_the_automatic_path_commit() {
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
    let (slot, _first) = install_candidate(&registry, session_id, "candidate text alpha");
    let first_seq = slot.snapshot().seq;
    let hooks = Arc::new(CoManagedTestHooks::default());
    let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));
    let next = co_managed_candidate(&session_id.to_string(), "candidate text bravo");
    let offered = arm_post_commit_offer(&hooks, &slot, next);

    let trigger = CoManagedTrigger::IdleEdge(session_id);
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
    let queued_after_first = queue_files(&fixture.room_root).len();

    let state = slot.snapshot();
    assert_eq!(
        state.seq,
        first_seq + 1,
        "the cycle must not clear the newer offer"
    );
    let held = state.value.record().expect("the newer record survives");
    assert!(Arc::ptr_eq(held, &offered));
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
    assert!(
        slot.snapshot().value.record().is_none(),
        "the next cycle consumed it"
    );
    assert!(
        queue_files(&fixture.room_root).len() > queued_after_first,
        "the second cycle routed the newer record"
    );
}

/// Test 33c: a record offered between the budget-fallback commit and the
/// consume survives it and is processed by the next cycle.
#[tokio::test]
async fn a_newer_candidate_survives_the_budget_fallback_commit() {
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![
        ("to-peer", 0.9),
        ("to-user", 0.1),
        ("to-root", 0.0),
        ("to-reply", 0.0),
    ])
    .await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
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
    let (slot, _first) = install_candidate(&registry, session_id, "candidate text alpha");
    let first_seq = slot.snapshot().seq;
    let hooks = Arc::new(CoManagedTestHooks::default());
    let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));
    let next = co_managed_candidate(&session_id.to_string(), "candidate text bravo");
    let offered = arm_post_commit_offer(&hooks, &slot, next);

    let trigger = CoManagedTrigger::IdleEdge(session_id);
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;

    let state = slot.snapshot();
    assert_eq!(
        state.seq,
        first_seq + 1,
        "the cycle must not clear the newer offer"
    );
    let held = state.value.record().expect("the newer record survives");
    assert!(Arc::ptr_eq(held, &offered));
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
    assert!(
        slot.snapshot().value.record().is_none(),
        "the next cycle consumed it"
    );
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

/// Test 33d: control. Without a race the automatic path still consumes
/// the candidate exactly once.
#[tokio::test]
async fn without_a_race_the_automatic_path_consumes_the_candidate() {
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
    let (slot, _first) = install_candidate(&registry, session_id, "candidate text alpha");
    let first_seq = slot.snapshot().seq;
    let hooks = Arc::new(CoManagedTestHooks::default());
    let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));

    let trigger = CoManagedTrigger::IdleEdge(session_id);
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;

    let state = slot.snapshot();
    assert_eq!(state.seq, first_seq + 1, "consumed exactly once");
    assert!(state.value.record().is_none(), "the candidate was consumed");
}

/// Test 14: a `provider_final == false` candidate is rendered as the
/// latest captured assistant text, and no rendering implies approval.
#[test]
fn a_non_final_candidate_is_rendered_as_latest_captured_assistant_text() {
    assert_eq!(
        co_managed_candidate_label(false),
        "latest captured assistant text at the idle edge"
    );
    assert_eq!(co_managed_candidate_label(true), "final message");
    let text = "A status update for the room.";
    let rendered = super::co_managed_user_message(
        "category 'x' routes to the user",
        Some((text, false, Some(Path::new("/tmp/msg.md")))),
    );
    assert!(rendered.contains("latest captured assistant text at the idle edge"));
    assert!(!rendered.to_lowercase().contains("final message"));
    for phrase in ["approved", "go ahead", "lgtm", "ship it"] {
        assert!(
            !rendered.to_lowercase().contains(phrase),
            "rendering must not express approval: {rendered}"
        );
    }
}

/// Test 15: the idle edge never emits a bare idle for an armed session.
/// Drive the real emission site and capture emissions in order.
#[tokio::test]
async fn the_idle_edge_never_emits_a_bare_idle_for_an_armed_session() {
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
    install_candidate(&registry, session_id, "candidate text");
    let events = capture_events(&app);
    let (handle, _rx) = CoManagedSupervisorHandle::new();
    handle.armed.arm(&session_id.to_string());

    let comanaged = emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    assert!(comanaged);
    let captured = events.lock().unwrap().clone();
    assert_eq!(captured[0].0, "session_idle");
    assert_eq!(captured[0].1["comanaged"], serde_json::Value::Bool(true));
    assert!(
        !captured.iter().any(|(name, payload)| {
            name == "session_comanaged_state" && payload["active"] == serde_json::Value::Bool(true)
        }),
        "the idle edge must not be preceded or duplicated by active:true: {captured:?}"
    );
}

/// Test 16: enabling the flag alone emits nothing; a cycle emits
/// `active:true` then `active:false` with a non-null reason.
#[tokio::test]
async fn enabling_the_flag_alone_emits_nothing_and_a_cycle_emits_true_then_false() {
    // Disabled room: the enable alone (config write) emits nothing.
    let disabled = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![("to-user", 0.9)]).await;
    let (app, manager, registry) = co_managed_app(&disabled, endpoint, false);
    let session_id = add_claude_session(
        &manager,
        &disabled.coordinator_cwd,
        SessionStatus::Running,
        &disabled.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let events = capture_events(&app);
    let (handle, _rx) = CoManagedSupervisorHandle::new();
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;
    assert!(!events
        .lock()
        .unwrap()
        .iter()
        .any(|(name, _)| name == "session_comanaged_state"));

    // Enabled room: one cycle emits exactly true then false(reason).
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![
        ("to-user", 0.9),
        ("to-peer", 0.1),
        ("to-root", 0.0),
        ("to-reply", 0.0),
    ])
    .await;
    let (app2, manager2, registry2) = co_managed_app(&fixture, endpoint, true);
    let session2 = add_claude_session(
        &manager2,
        &fixture.coordinator_cwd,
        SessionStatus::Idle,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry2, session2, "candidate text");
    let events2 = capture_events(&app2);
    let (handle2, _rx2) = CoManagedSupervisorHandle::new();
    super::handle_co_managed_trigger(
        app2.handle(),
        &handle2,
        CoManagedTrigger::SlotChanged(session2),
    )
    .await;
    let captured = events2.lock().unwrap().clone();
    let states: Vec<&serde_json::Value> = captured
        .iter()
        .filter(|(name, _)| name == "session_comanaged_state")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(states.len(), 2, "{captured:?}");
    assert_eq!(states[0]["active"], serde_json::Value::Bool(true));
    assert!(states[0]["reason"].is_null());
    assert_eq!(states[1]["active"], serde_json::Value::Bool(false));
    assert!(states[1]["reason"].is_string());
}

/// Test 20: `session_comanaged_state` is emitted with `emit`, so a general
/// listener receives it.
#[tokio::test]
async fn the_state_event_is_emitted_with_emit_not_emit_to() {
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
        SessionStatus::Idle,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let events = capture_events(&app);
    let (handle, _rx) = CoManagedSupervisorHandle::new();

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::SlotChanged(session_id),
    )
    .await;

    let captured = events.lock().unwrap().clone();
    let state = captured
        .iter()
        .find(|(name, _)| name == "session_comanaged_state")
        .expect("a general listener must receive the event");
    assert_eq!(state.1["id"], serde_json::json!(session_id.to_string()));
    assert!(state.1["active"].is_boolean());
    assert!(state.1.as_object().unwrap().contains_key("reason"));
}

/// Test 21: contention terminates after exactly three attempts, abstains
/// with reason "contention", keeps the flag armed and never re-triggers
/// the same sequence.
#[tokio::test]
async fn contention_terminates_after_three_attempts_and_keeps_the_flag_armed() {
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
    let (slot, _record) = install_candidate(&registry, session_id, "candidate text");
    let seq_before = slot.seq();
    let events = capture_events(&app);
    let hooks = Arc::new(CoManagedTestHooks::default());
    let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));

    // Hold the advisory lock for the whole cycle.
    let lock_path = crate::config::co_managed::lock_path(&fixture.room_root);
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    lock_file.try_lock().expect("test holds the lock");

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    assert_eq!(hooks.commit_calls(), 3, "three attempts, then abstain");
    assert!(messaging_files(&fixture.room_root).is_empty());
    assert!(queue_files(&fixture.room_root).is_empty());
    let captured = events.lock().unwrap().clone();
    let last = captured.last().expect("events");
    assert_eq!(last.0, "session_comanaged_state");
    assert_eq!(last.1["active"], serde_json::Value::Bool(false));
    assert_eq!(last.1["reason"], serde_json::json!("contention"));
    assert!(handle.armed.is_armed(&session_id.to_string()));
    assert_eq!(slot.seq(), seq_before, "the candidate is unchanged");
    assert!(slot.snapshot().value.record().is_some());

    // A trigger for the same sequence must not run a fourth attempt.
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;
    assert_eq!(
        hooks.commit_calls(),
        3,
        "no fourth attempt for the same sequence"
    );

    // A later idle callback reports the candidate as armed.
    let comanaged = emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
    assert!(comanaged, "the pending candidate stays armed");
}

/// F1 (step 9): a contended candidate whose session loses readiness must
/// clear the armed flag and publish the readiness reason, without ever
/// re-running the candidate. Before the fix the early contended return kept
/// `comanaged: true` on every later idle edge for the same `seq`.
#[tokio::test]
async fn losing_readiness_clears_the_armed_flag_for_a_contended_candidate() {
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
    let (slot, _record) = install_candidate(&registry, session_id, "candidate text");
    let seq_before = slot.seq();
    let events = capture_events(&app);
    let hooks = Arc::new(CoManagedTestHooks::default());
    let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));

    // Hold the advisory lock so the first trigger abstains on contention.
    let lock_path = crate::config::co_managed::lock_path(&fixture.room_root);
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    lock_file.try_lock().expect("test holds the lock");
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;
    assert_eq!(hooks.commit_calls(), 3, "the first trigger contends");
    assert!(handle.armed.is_armed(&session_id.to_string()));

    // The room flag goes off while the same candidate is still pending.
    write_co_managed_room_config(&fixture.room_root, false, Some("catalog.json"));
    emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    assert_eq!(
        hooks.commit_calls(),
        3,
        "readiness loss must not re-run the contended candidate"
    );
    assert!(
        !handle.armed.is_armed(&session_id.to_string()),
        "readiness loss must clear the armed flag"
    );
    let captured = events.lock().unwrap().clone();
    let last = captured.last().expect("events");
    assert_eq!(last.0, "session_comanaged_state", "{captured:?}");
    assert_eq!(last.1["active"], serde_json::Value::Bool(false));
    assert_eq!(last.1["reason"], serde_json::json!("RoomFlagOff"));
    assert_eq!(slot.seq(), seq_before, "the candidate is unchanged");
    assert!(slot.snapshot().value.record().is_some());

    // The next idle edge reports waiting, not red.
    let comanaged = emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
    assert!(!comanaged, "the cleared flag must reach the idle payload");
}

/// F4 (step 9): the Root branch must use the room-derived project even when
/// settings omit it, exactly as the CLI and the mailbox's Co-managed branch
/// do. Without the derived path a verified coordinator degrades to a user
/// message instead of the route the mailbox would accept.
#[tokio::test]
async fn root_destination_derives_the_project_from_the_room() {
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(vec![
        ("to-root", 0.9),
        ("to-user", 0.1),
        ("to-peer", 0.0),
        ("to-reply", 0.0),
    ])
    .await;
    // Settings do NOT contain the project the room lives in.
    let (app, manager, registry) =
        co_managed_app_with_project_paths(&fixture, endpoint, true, Vec::new());
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let (handle, _rx) = CoManagedSupervisorHandle::new();

    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    assert!(
        !queue_files(&fixture.room_root).is_empty(),
        "the verified coordinator must reach Root through the room-derived project"
    );
    assert_eq!(
        messaging_files(&fixture.room_root).len(),
        1,
        "the routed wake writes exactly one message file"
    );
}

/// Test 22: `commit_effect` runs on the blocking pool, so a current-thread
/// runtime keeps making progress while the advisory lock is held.
#[tokio::test(flavor = "current_thread")]
async fn the_blocking_commit_does_not_park_a_current_thread_worker() {
    let fixture = make_co_managed_fixture();
    write_co_managed_room_config(&fixture.room_root, true, Some("catalog.json"));
    let registry = Arc::new(CaptureRegistry::new());
    let session_id = uuid::Uuid::new_v4();
    let (slot, record) = install_candidate(&registry, session_id, "candidate text");
    let expected_seq = slot.seq();
    let expected_key = crate::capture::state::consumption_key(&fixture.room_root, record.as_ref());
    // The holder signals the acquired lock before the commit starts, so the
    // commit cannot win the race and find it free, and it holds the lock
    // until the async worker proves progress instead of for a fixed sleep,
    // so a descheduled test thread cannot make the commit run uncontended.
    let lock_path = crate::config::co_managed::lock_path(&fixture.room_root);
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let lock_path_for_thread = lock_path.clone();
    let (lock_ready_tx, lock_ready_rx) = std::sync::mpsc::channel();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let lock_holder = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path_for_thread)
            .unwrap();
        file.try_lock().expect("lock holder");
        lock_ready_tx.send(()).expect("announce the held lock");
        // The bound is a liveness guard only: a runtime-parking mutant must
        // fail the progress assertion instead of hanging the suite. The
        // healthy path is released by the tick, never by the timeout.
        let _ = progress_rx.recv_timeout(Duration::from_secs(10));
        drop(file);
    });
    lock_ready_rx
        .recv()
        .expect("the lock holder must hold the lock before the commit starts");

    let ticks = Arc::new(AtomicUsize::new(0));
    let ticker = {
        let ticks = Arc::clone(&ticks);
        tokio::spawn(async move {
            // This first tick runs only once the commit has yielded the
            // runtime: it is both the property under test and the holder's
            // release signal, so a direct in-task call never reaches it.
            tokio::task::yield_now().await;
            ticks.fetch_add(1, Ordering::SeqCst);
            let _ = progress_tx.send(());
            for _ in 0..2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                ticks.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    // Built as late as possible: the test must not spend the commit's
    // staleness window on its own synchronization.
    let pre = crate::capture::state::EffectPreconditions {
        session_alive: true,
        session_id: session_id.to_string(),
        unique_live_session_for_cwd: true,
        no_pending_user_input: true,
        effective_ready: true,
        observed_at: Instant::now(),
        kind: crate::capture::state::EffectKind::Automatic,
    };
    let commit = super::commit_co_managed_effect(
        fixture.room_root.clone(),
        slot.clone(),
        expected_seq,
        expected_key,
        pre,
    )
    .await;
    let ticks_when_commit_returned = ticks.load(Ordering::SeqCst);
    ticker.await.unwrap();
    lock_holder.join().unwrap();

    // Checked before `commit`: an in-task mutant can return `LockBusy`
    // before the ticker ever runs, and the missing progress is the finding.
    assert!(
        ticks_when_commit_returned >= 1,
        "the async worker must make real progress WHILE the blocking commit waits for the lock; \
         a direct in-task call parks it and observes zero ticks here (F2)"
    );
    assert!(
        commit.is_ok(),
        "the lock is released and the commit applies"
    );
    assert_eq!(
        ticks.load(Ordering::SeqCst),
        3,
        "the async worker kept making progress while the blocking commit waited"
    );
}

/// Test 23: the late-poll residual is real. A record delivered after the
/// idle edge paints waiting (`comanaged:false`) and then red (`active:true`).
#[tokio::test]
async fn the_late_poll_residual_paints_waiting_then_red() {
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
        SessionStatus::Idle,
        &fixture.projects_dir,
    )
    .await;
    let events = capture_events(&app);
    let (handle, _rx) = CoManagedSupervisorHandle::new();

    // The idle edge fires with no candidate: false.
    emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
    // The record arrives afterwards, while the session is already idle.
    install_candidate(&registry, session_id, "candidate text");
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::SlotChanged(session_id),
    )
    .await;

    let captured = events.lock().unwrap().clone();
    assert_eq!(captured[0].0, "session_idle", "{captured:?}");
    assert_eq!(captured[0].1["comanaged"], serde_json::Value::Bool(false));
    let active = captured
        .iter()
        .find(|(name, payload)| {
            name == "session_comanaged_state" && payload["active"] == serde_json::Value::Bool(true)
        })
        .expect("the late record arms red after the waiting dot");
    assert_eq!(captured[0].0, "session_idle");
    assert_eq!(active.1["id"], serde_json::json!(session_id.to_string()));
}

/// The two supervisor helpers the contention/abstention reasons must stay
/// stable for phase 8 and the log readers.
#[test]
fn supervisor_reason_strings_are_stable() {
    assert_eq!(
        abstain_reason_label(&crate::capture::state::AbstainReason::LockBusy),
        "LockBusy"
    );
    assert_eq!(co_managed_state_reason(&Ok(CoManagedState::Ready)), "Ready");
    assert_eq!(
        co_managed_state_reason(&Ok(CoManagedState::Off {
            reason: crate::config::co_managed::OffReason::RoomFlagOff
        })),
        "RoomFlagOff"
    );
    assert_eq!(
        co_managed_state_reason(&Ok(CoManagedState::Off {
            reason: crate::config::co_managed::OffReason::GlobalSwitchOff
        })),
        "GlobalSwitchOff"
    );
}

/// `route_co_managed_wake` is the in-process writer: it must own the file
/// authority check and leave a resolvable pointer.
#[test]
fn route_co_managed_wake_requires_an_authoritative_room() {
    let temp = tempfile::TempDir::new().unwrap();
    let not_a_room = temp.path().join("not-a-room");
    std::fs::create_dir_all(&not_a_room).unwrap();
    let (handle, _rx) = CoManagedSupervisorHandle::new();
    let error = route_co_managed_wake(
        &handle,
        &not_a_room,
        "proj-a:room-1-dev-team/tech-lead",
        "proj-a:room-1-dev-team/dev-rust",
        "body",
    )
    .expect_err("a non-authoritative root must be refused before any write");
    assert!(!error.is_empty());
    assert!(
        !not_a_room
            .join(crate::phone::messaging::MESSAGING_DIR_NAME)
            .exists(),
        "nothing may be created under a refused root"
    );
}

#[test]
fn issue_3081_co_managed_post_flush_snapshot_and_queue_failures() {
    for blocked in [false, true] {
        let fixture = make_co_managed_fixture();
        if blocked {
            std::fs::write(
                fixture.room_root.parent().unwrap().join("message-archive"),
                b"blocked",
            )
            .unwrap();
        }
        let (handle, _rx) = CoManagedSupervisorHandle::new();
        let warning = route_co_managed_wake(
            &handle,
            &fixture.room_root,
            "proj-a:room-1-dev-team/tech-lead",
            "proj-a:room-1-dev-team/dev-rust",
            "\u{feff}body\r\n",
        )
        .unwrap();
        assert_eq!(warning.is_some(), blocked);
        assert_eq!(queue_files(&fixture.room_root).len(), 1);
        let source = messaging_files(&fixture.room_root).pop().unwrap();
        assert_eq!(
            std::fs::read(&source).unwrap(),
            "\u{feff}body\r\n".as_bytes()
        );
        if !blocked {
            let name = source.file_name().unwrap().to_str().unwrap();
            let dir = fixture
                .room_root
                .parent()
                .unwrap()
                .join("message-archive")
                .join(&name[..8]);
            assert_eq!(
                std::fs::read(dir.join(source.file_name().unwrap())).unwrap(),
                std::fs::read(source).unwrap()
            );
        }
    }
    let fixture = make_co_managed_fixture();
    std::fs::write(
        fixture.room_root.parent().unwrap().join("message-archive"),
        b"blocked",
    )
    .unwrap();
    let queue = crate::config::co_managed::queue_dir(&fixture.room_root);
    std::fs::create_dir_all(queue.parent().unwrap()).unwrap();
    std::fs::write(queue, b"blocked queue").unwrap();
    let (handle, _rx) = CoManagedSupervisorHandle::new();
    let error = route_co_managed_wake(
        &handle,
        &fixture.room_root,
        "proj-a:room-1-dev-team/tech-lead",
        "proj-a:room-1-dev-team/dev-rust",
        "body",
    )
    .unwrap_err();
    assert!(error.contains("queue dir create failed"));
    assert!(error.contains("Message archive warning"));
}

#[tokio::test]
async fn issue_3081_co_managed_archive_failure_surfaces_warning_and_keeps_outcome() {
    let fixture = make_co_managed_fixture();
    std::fs::write(
        fixture.room_root.parent().unwrap().join("message-archive"),
        b"blocked",
    )
    .unwrap();
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
    let (handle, _rx) = CoManagedSupervisorHandle::new();
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;
    assert_eq!(queue_files(&fixture.room_root).len(), 1);
    let communication = session_communication(&manager, session_id).await.unwrap();
    let message = communication.message.unwrap();
    assert!(message.contains("Send queued"), "{message}");
    assert!(message.contains("archive warning"), "{message}");
    assert!(!message.contains("routing failed"), "{message}");
}
