use super::*;

#[tokio::test]
async fn stage_e_request_close_is_idempotent_and_close_and_join_owns_the_join() {
    // Stage E (#1064) alert-shutdown ownership sentinel (plan section 8.1,
    // 10.4 item 20, acceptance item 34): request_close only cancels the token
    // (never takes or awaits the actor join) and is idempotent, while
    // close_and_join stays the sole idempotent join owner.
    let runtime = ScriptedRuntime::new();
    let clock = ManualClock::new(Instant::now());
    let shutdown = CancellationToken::new();
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        Arc::clone(&clock) as Arc<dyn AlertClock>,
        shutdown.clone(),
    );

    monitor.request_close();
    monitor.request_close();
    assert!(
        shutdown.is_cancelled(),
        "request_close must cancel the shared shutdown token"
    );

    monitor.close_and_join().await.unwrap();
    monitor.close_and_join().await.unwrap();
}

#[tokio::test]
async fn actor_uses_manual_retry_clock_and_monitor_shutdown_is_idempotent() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    runtime
        .delivery_results
        .lock()
        .unwrap()
        .extend([Err("first failure".to_string()), Ok(())]);
    let clock = ManualClock::new(Instant::now());
    let shutdown = CancellationToken::new();
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        Arc::clone(&clock) as Arc<dyn AlertClock>,
        shutdown,
    );

    monitor
        .sender()
        .send(ContextSample::Reading {
            session_id: id,
            percent: 60,
        })
        .await
        .unwrap();
    runtime.wait_for_delivery_count(1).await;
    clock
        .wait_for_live_waiter_within(Duration::from_secs(5))
        .await;

    clock.advance(Duration::from_secs(4));
    assert_eq!(runtime.delivery_count(), 1);

    clock.advance(Duration::from_secs(1));
    runtime.wait_for_delivery_count(2).await;
    {
        let deliveries = runtime.deliveries.lock().unwrap();
        assert_eq!(deliveries[0].generation, deliveries[1].generation);
        assert_eq!(deliveries[0].thresholds, vec![50]);
    }

    monitor.close_and_join().await.unwrap();
    monitor.close_and_join().await.unwrap();
}

#[tokio::test]
async fn shutdown_cancels_but_joins_the_single_in_flight_delivery() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    let release = runtime.block_next_delivery();
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        ManualClock::new(Instant::now()) as Arc<dyn AlertClock>,
        CancellationToken::new(),
    );
    monitor
        .sender()
        .send(ContextSample::Reading {
            session_id: id,
            percent: 60,
        })
        .await
        .unwrap();
    runtime.wait_for_delivery_count(1).await;

    let closing_monitor = Arc::clone(&monitor);
    let close = tokio::spawn(async move { closing_monitor.close_and_join().await });
    assert!(
        !close.is_finished(),
        "shutdown must join, not detach, delivery"
    );
    release.send(Ok(())).unwrap();
    close.await.unwrap().unwrap();
    monitor.close_and_join().await.unwrap();
}

#[tokio::test]
async fn unavailable_preserves_latches_and_impossible_reading_is_rejected() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    let runtime_dyn = Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>;
    let clock = ManualClock::new(now);
    let clock_dyn = Arc::clone(&clock) as Arc<dyn AlertClock>;
    let shutdown = CancellationToken::new();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
    let generation = *state.batches.keys().next().unwrap();

    process_sample(
        &runtime_dyn,
        &clock_dyn,
        &mut state,
        None,
        ContextSample::Unavailable { session_id: id },
        &shutdown,
    )
    .await;
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
    assert_eq!(state.batches.len(), 1);
    assert!(state.batches.contains_key(&generation));
    assert_eq!(state.sessions[&id].last_valid_percent, Some(60));

    let resolves_before = runtime.resolve_calls.load(Ordering::SeqCst);
    process_sample(
        &runtime_dyn,
        &clock_dyn,
        &mut state,
        None,
        ContextSample::Reading {
            session_id: id,
            percent: 101,
        },
        &shutdown,
    )
    .await;
    assert_eq!(
        runtime.resolve_calls.load(Ordering::SeqCst),
        resolves_before
    );
    assert!(state.batches.contains_key(&generation));

    runtime.live.store(false, Ordering::SeqCst);
    process_sample(
        &runtime_dyn,
        &clock_dyn,
        &mut state,
        None,
        ContextSample::Unavailable { session_id: id },
        &shutdown,
    )
    .await;
    assert!(!state.sessions.contains_key(&id));
    assert!(runtime.retired.lock().unwrap().contains(&id));
}

#[tokio::test]
async fn committed_false_liveness_retirement_is_not_skipped_by_shutdown() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let runtime = ScriptedRuntime::new();
    runtime.live.store(false, Ordering::SeqCst);
    let shutdown = CancellationToken::new();
    *runtime.cancel_during_live_check.lock().unwrap() = Some(shutdown.clone());
    let runtime_dyn = Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>;
    let clock = ManualClock::new(now) as Arc<dyn AlertClock>;
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);

    process_sample(
        &runtime_dyn,
        &clock,
        &mut state,
        None,
        ContextSample::Unavailable { session_id: id },
        &shutdown,
    )
    .await;

    assert!(shutdown.is_cancelled());
    assert!(!state.sessions.contains_key(&id));
    assert_eq!(runtime.retired.lock().unwrap().as_slice(), &[id]);
}

#[tokio::test]
async fn maintenance_committed_false_liveness_retirement_is_not_skipped_by_shutdown() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let runtime = ScriptedRuntime::new();
    runtime.live.store(false, Ordering::SeqCst);
    let shutdown = CancellationToken::new();
    *runtime.cancel_during_live_check.lock().unwrap() = Some(shutdown.clone());
    let runtime_dyn = Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>;
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);

    run_maintenance(&runtime_dyn, &mut state, None, &shutdown).await;

    assert!(shutdown.is_cancelled());
    assert!(!state.sessions.contains_key(&id));
    assert_eq!(runtime.retired.lock().unwrap().as_slice(), &[id]);
}

#[tokio::test]
async fn definite_end_has_no_tombstone_and_new_uuid_starts_fresh() {
    let old_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let now = Instant::now();
    let runtime = ScriptedRuntime::new();
    let runtime_dyn = Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>;
    let clock = ManualClock::new(now) as Arc<dyn AlertClock>;
    let shutdown = CancellationToken::new();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, old_id, 60, eligible(old_id, &[50]), now);

    process_sample(
        &runtime_dyn,
        &clock,
        &mut state,
        None,
        ContextSample::SessionOver { session_id: old_id },
        &shutdown,
    )
    .await;
    assert!(!state.sessions.contains_key(&old_id));
    assert!(state.batches.is_empty());

    apply_numeric_resolution(
        &mut state,
        None,
        old_id,
        60,
        MemberPolicyResolution::PermanentIneligible("stale queued UUID".to_string()),
        now,
    );
    assert!(!state.sessions.contains_key(&old_id));
    apply_numeric_resolution(&mut state, None, new_id, 60, eligible(new_id, &[50]), now);
    assert_eq!(state.batches.values().next().unwrap().session_id, new_id);
}

#[tokio::test]
async fn one_global_delivery_slot_keeps_samples_and_maintenance_live() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50, 75]);
    let release = runtime.block_next_delivery();
    let clock = ManualClock::new(Instant::now());
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        Arc::clone(&clock) as Arc<dyn AlertClock>,
        CancellationToken::new(),
    );

    monitor
        .sender()
        .send(ContextSample::Reading {
            session_id: id,
            percent: 60,
        })
        .await
        .unwrap();
    runtime.wait_for_delivery_count(1).await;
    monitor
        .sender()
        .send(ContextSample::Reading {
            session_id: id,
            percent: 80,
        })
        .await
        .unwrap();
    runtime.wait_for_resolve_calls(2).await;
    assert_eq!(
        runtime.delivery_count(),
        1,
        "the second due batch must wait"
    );

    clock
        .wait_for_live_waiter_within(CONTEXT_ALERT_MAINTENANCE_INTERVAL)
        .await;
    clock.advance(CONTEXT_ALERT_MAINTENANCE_INTERVAL);
    runtime.wait_for_live_checks(1).await;
    assert_eq!(runtime.delivery_count(), 1);

    release.send(Ok(())).unwrap();
    runtime.wait_for_delivery_count(2).await;
    {
        let deliveries = runtime.deliveries.lock().unwrap();
        assert_eq!(deliveries[0].thresholds, vec![50]);
        assert_eq!(deliveries[1].thresholds, vec![75]);
        assert!(deliveries[0].generation < deliveries[1].generation);
    }
    monitor.close_and_join().await.unwrap();
}

#[tokio::test]
async fn large_clock_jump_coalesces_missed_maintenance_intervals() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    let clock = ManualClock::new(Instant::now());
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        Arc::clone(&clock) as Arc<dyn AlertClock>,
        CancellationToken::new(),
    );
    monitor
        .sender()
        .send(ContextSample::Reading {
            session_id: id,
            percent: 40,
        })
        .await
        .unwrap();
    runtime.wait_for_resolve_calls(1).await;
    clock
        .wait_for_live_waiter_within(CONTEXT_ALERT_MAINTENANCE_INTERVAL)
        .await;

    clock.advance(CONTEXT_ALERT_MAINTENANCE_INTERVAL * 10);
    runtime.wait_for_live_checks(1).await;
    assert_eq!(runtime.live_checks.load(Ordering::SeqCst), 1);
    clock
        .wait_for_live_waiter_within(CONTEXT_ALERT_MAINTENANCE_INTERVAL)
        .await;
    monitor.close_and_join().await.unwrap();
}

#[tokio::test]
async fn delivery_task_panic_is_one_retryable_failure_for_same_generation() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    runtime.panic_next_delivery.store(true, Ordering::SeqCst);
    let clock = ManualClock::new(Instant::now());
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        Arc::clone(&clock) as Arc<dyn AlertClock>,
        CancellationToken::new(),
    );
    monitor
        .sender()
        .send(ContextSample::Reading {
            session_id: id,
            percent: 60,
        })
        .await
        .unwrap();
    runtime.wait_for_delivery_count(1).await;
    clock
        .wait_for_live_waiter_within(Duration::from_secs(5))
        .await;
    clock.advance(Duration::from_secs(5));
    runtime.wait_for_delivery_count(2).await;
    {
        let deliveries = runtime.deliveries.lock().unwrap();
        assert_eq!(deliveries[0].generation, deliveries[1].generation);
    }
    monitor.close_and_join().await.unwrap();
}

#[tokio::test]
async fn closed_sample_receiver_prepares_due_retry_before_next_backlog_sample() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    runtime
        .delivery_results
        .lock()
        .unwrap()
        .extend([Err("retry".to_string()), Ok(())]);
    let clock = ManualClock::new(Instant::now());
    let shutdown = CancellationToken::new();
    let release_live_check = runtime.block_next_live_check();
    let (sender, receiver) = mpsc::channel(2_048);
    let actor = tauri::async_runtime::spawn(run_context_alert_actor(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        Arc::clone(&clock) as Arc<dyn AlertClock>,
        receiver,
        shutdown.clone(),
    ));
    sender
        .send(ContextSample::Reading {
            session_id: id,
            percent: 60,
        })
        .await
        .unwrap();
    runtime.wait_for_delivery_count(1).await;
    clock
        .wait_for_live_waiter_within(Duration::from_secs(5))
        .await;
    for _ in 0..1_000 {
        sender
            .try_send(ContextSample::Unavailable { session_id: id })
            .unwrap();
    }
    drop(sender);
    runtime.wait_for_live_checks(1).await;
    clock.advance(Duration::from_secs(5));
    release_live_check
        .send(())
        .expect("release the first queued liveness check");
    runtime.wait_for_delivery_count(2).await;
    // The ordering evidence is a saved count, so let the actor finish the whole
    // closed-receiver backlog before any observer reads the recorded state.
    runtime.wait_for_live_checks(1_000).await;
    // The saved liveness count in the retry preparation record proves the due retry
    // was prepared before the actor consumed a second queued sample, while the
    // eventual second delivery proves the dispatch completed. It does not prove
    // which redundant dispatch route ran (sample postprocessing or the deadline
    // arm), how the delivery task was scheduled, or any wall-clock latency, and it
    // says nothing about when recv() first returned None; the sender-closed backlog
    // integration is what the drain above still covers.
    let preparations = runtime.prepare_live_checks.lock().unwrap().clone();
    let deliveries = runtime.deliveries.lock().unwrap().clone();
    shutdown.cancel();
    actor.await.unwrap();

    assert_eq!(
        preparations.len(),
        2,
        "the first delivery and its due retry must each prepare exactly once"
    );
    assert_eq!(
        preparations[0].0, preparations[1].0,
        "preparations must stay within one generation"
    );
    assert_eq!(
        preparations[0].1, 0,
        "the initial preparation carries no failures"
    );
    assert_eq!(
        preparations[1].1, 1,
        "the retry preparation follows the first failure"
    );
    assert_eq!(preparations[0].2, 0);
    assert_eq!(deliveries.len(), 2);
    assert_eq!(deliveries[0].session_id, id);
    assert_eq!(deliveries[1].session_id, id);
    assert_eq!(deliveries[0].generation, deliveries[1].generation);
    assert_eq!(deliveries[0].failure_count, 0);
    assert_eq!(deliveries[1].failure_count, 1);
    assert_eq!(runtime.live_checks.load(Ordering::SeqCst), 1_000);
    assert_eq!(
        preparations[1].2, 1,
        "due retry preparation must precede the next backlog sample"
    );
}

#[tokio::test]
async fn shutdown_interrupts_blocked_resolution_without_detaching_actor() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    let blocked = runtime.block_next_resolution();
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        ManualClock::new(Instant::now()) as Arc<dyn AlertClock>,
        CancellationToken::new(),
    );
    monitor
        .sender()
        .send(ContextSample::Reading {
            session_id: id,
            percent: 60,
        })
        .await
        .unwrap();
    runtime.wait_for_resolve_calls(1).await;

    tokio::time::timeout(Duration::from_secs(1), monitor.close_and_join())
        .await
        .expect("shutdown must cancel a blocked resolution")
        .unwrap();
    drop(blocked);
    assert_eq!(runtime.delivery_count(), 0);
}

#[tokio::test]
async fn new_monitor_instance_models_restart_and_first_high_alerts_again() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    for expected in 1..=2 {
        let monitor = ContextAlertMonitor::start_with_runtime(
            Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
            ManualClock::new(Instant::now()) as Arc<dyn AlertClock>,
            CancellationToken::new(),
        );
        monitor
            .sender()
            .send(ContextSample::Reading {
                session_id: id,
                percent: 60,
            })
            .await
            .unwrap();
        runtime.wait_for_delivery_count(expected).await;
        monitor.close_and_join().await.unwrap();
    }
    assert_ne!(
        runtime.deliveries.lock().unwrap()[0].generation,
        0,
        "each process-local actor begins at a checked nonzero generation"
    );
    assert_eq!(runtime.deliveries.lock().unwrap()[0].generation, 1);
    assert_eq!(runtime.deliveries.lock().unwrap()[1].generation, 1);
}

#[tokio::test]
async fn maintenance_retires_state_when_a_dropped_end_is_witnessed_as_not_live() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[50]);
    let clock = ManualClock::new(Instant::now());
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        Arc::clone(&clock) as Arc<dyn AlertClock>,
        CancellationToken::new(),
    );

    monitor
        .sender()
        .send(ContextSample::Reading {
            session_id: id,
            percent: 40,
        })
        .await
        .unwrap();
    runtime.wait_for_resolve_calls(1).await;
    clock
        .wait_for_live_waiter_within(CONTEXT_ALERT_MAINTENANCE_INTERVAL)
        .await;
    runtime.live.store(false, Ordering::SeqCst);
    clock.advance(CONTEXT_ALERT_MAINTENANCE_INTERVAL);
    runtime.wait_for_retirement(id).await;

    monitor.close_and_join().await.unwrap();
}

#[tokio::test]
async fn unavailable_retires_even_when_actor_has_no_alert_state() {
    let id = Uuid::new_v4();
    let runtime = ScriptedRuntime::new();
    runtime.live.store(false, Ordering::SeqCst);
    let clock = ManualClock::new(Instant::now());
    let monitor = ContextAlertMonitor::start_with_runtime(
        Arc::clone(&runtime) as Arc<dyn ContextAlertRuntime>,
        clock as Arc<dyn AlertClock>,
        CancellationToken::new(),
    );

    monitor
        .sender()
        .send(ContextSample::Unavailable { session_id: id })
        .await
        .unwrap();
    runtime.wait_for_retirement(id).await;

    monitor.close_and_join().await.unwrap();
}
