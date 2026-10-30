use super::*;

#[test]
fn first_high_coalesces_sorted_thresholds_and_unchanged_high_deduplicates() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75, 90]), now);
    assert_eq!(state.batches.len(), 1);
    let batch = state.batches.values().next().unwrap();
    assert_eq!(batch.observed, 80);
    assert_eq!(batch.thresholds, vec![50, 75]);

    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75, 90]), now);
    assert_eq!(state.batches.len(), 1);
}

#[test]
fn first_below_exact_equality_and_successive_crossings_are_deterministic() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 49, eligible(id, &[50, 75]), now);
    assert!(state.batches.is_empty());

    apply_numeric_resolution(&mut state, None, id, 50, eligible(id, &[50, 75]), now);
    apply_numeric_resolution(&mut state, None, id, 75, eligible(id, &[50, 75]), now);
    let batches: Vec<&AlertBatch> = state.batches.values().collect();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].thresholds, vec![50]);
    assert_eq!(batches[0].observed, 50);
    assert_eq!(batches[1].thresholds, vec![75]);
    assert_eq!(batches[1].observed, 75);
    assert!(batches[0].generation < batches[1].generation);
}

#[test]
fn policy_removal_prunes_pending_and_readd_starts_armed() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75]), now);
    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[75, 90]), now);
    assert_eq!(state.batches.values().next().unwrap().thresholds, vec![75]);
    assert_eq!(state.sessions[&id].latches[&90], LatchState::Armed);

    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[90]), now);
    assert!(state.batches.is_empty());
    apply_numeric_resolution(&mut state, None, id, 95, eligible(id, &[90]), now);
    assert_eq!(state.batches.values().next().unwrap().thresholds, vec![90]);
}

#[test]
fn successful_notice_keeps_current_latch_and_allows_a_later_rearm() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
    let generation = *state.batches.keys().next().unwrap();
    state.remove_batch(generation);

    apply_numeric_resolution(&mut state, None, id, 40, eligible(id, &[50]), now);
    apply_numeric_resolution(&mut state, None, id, 55, eligible(id, &[50]), now);
    assert_eq!(state.batches.len(), 1);
    assert_ne!(*state.batches.keys().next().unwrap(), generation);
}

#[test]
fn partial_rearm_and_outstanding_dedup_are_threshold_scoped() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &[50, 75, 90]), now);
    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75, 90]), now);
    let session = state.sessions.get(&id).unwrap();
    assert_eq!(session.latches[&50], LatchState::Latched);
    assert_eq!(session.latches[&75], LatchState::Latched);
    assert_eq!(session.latches[&90], LatchState::Armed);

    apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &[50, 75, 90]), now);
    assert_eq!(
        state.batches.len(),
        1,
        "90 remains outstanding in first batch"
    );
}

#[test]
fn retry_schedule_caps_at_sixty_seconds_on_the_same_logical_batch() {
    let expected = [5, 10, 20, 40, 60, 60];
    for (failure_count, seconds) in expected.into_iter().enumerate() {
        assert_eq!(
            retry_delay((failure_count + 1) as u32),
            Duration::from_secs(seconds)
        );
    }
    assert_eq!(retry_delay(99), Duration::from_secs(60));

    let id = Uuid::new_v4();
    let mut now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
    let generation = *state.batches.keys().next().unwrap();
    for seconds in expected {
        fail_batch(&mut state, generation, now, "scripted failure");
        now += Duration::from_secs(seconds);
        assert_eq!(state.batches[&generation].due_at, now);
    }
    assert_eq!(state.batches[&generation].failure_count, 6);
    assert_eq!(state.sessions[&id].outstanding[&50], generation);
}

#[tokio::test]
async fn held_completion_preserves_attempts_and_another_due_batch() {
    let now = Instant::now();
    let mut state = ActorState::new();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    assert_eq!(state.earliest_due(), None);
    apply_numeric_resolution(&mut state, None, a, 60, eligible(a, &[50]), now);
    let generation = *state.batches.keys().next().unwrap();
    apply_numeric_resolution(&mut state, None, b, 60, eligible(b, &[50]), now);
    let other = *state.batches.keys().find(|g| **g != generation).unwrap();
    for _ in 0..3 {
        state.batches.get_mut(&generation).unwrap().in_flight = true;
        assert_eq!(state.earliest_due(), Some((now, other)));
        let mut slot = Some(InFlightSlot {
            generation,
            cancellation: CancellationToken::new(),
            join: tokio::spawn(async {}),
        });
        handle_delivery_completion(
            &mut state,
            &mut slot,
            DeliveryCompletion {
                generation,
                result: Err("typing_hold_deferred: held".into()),
            },
            now,
        )
        .await;
        let batch = &state.batches[&generation];
        assert!(!batch.in_flight);
        assert_eq!(batch.failure_count, 0);
        assert_eq!(batch.due_at, now + HOLD_RECHECK_DELAY);
        assert_eq!(state.earliest_due(), Some((now, other)));
    }
    state.remove_batch(other);
    assert!(state.earliest_due().unwrap().0 > now);
    fail_batch(&mut state, generation, now, "real failure");
    assert_eq!(state.batches[&generation].failure_count, 1);
    assert_eq!(state.batches[&generation].due_at, now + FIRST_RETRY_DELAY);
    assert_ne!(HOLD_RECHECK_DELAY, FIRST_RETRY_DELAY);
}

#[test]
fn partial_rearm_at_eighty_sixty_and_zero_is_exact() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    let policy = [50, 75, 90];

    apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &policy), now);
    let first = *state.batches.keys().next().unwrap();
    state.remove_batch(first);

    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &policy), now);
    assert_eq!(state.sessions[&id].latches[&50], LatchState::Latched);
    assert_eq!(state.sessions[&id].latches[&75], LatchState::Latched);
    assert_eq!(state.sessions[&id].latches[&90], LatchState::Armed);
    apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &policy), now);
    assert_eq!(state.batches.values().next().unwrap().thresholds, vec![90]);
    let second = *state.batches.keys().next().unwrap();
    state.remove_batch(second);

    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &policy), now);
    assert_eq!(state.sessions[&id].latches[&50], LatchState::Latched);
    assert_eq!(state.sessions[&id].latches[&75], LatchState::Armed);
    assert_eq!(state.sessions[&id].latches[&90], LatchState::Armed);
    apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &policy), now);
    assert_eq!(
        state.batches.values().next().unwrap().thresholds,
        vec![75, 90]
    );
    let third = *state.batches.keys().next().unwrap();
    state.remove_batch(third);

    apply_numeric_resolution(&mut state, None, id, 0, eligible(id, &policy), now);
    assert!(state.sessions[&id]
        .latches
        .values()
        .all(|latch| *latch == LatchState::Armed));
    apply_numeric_resolution(&mut state, None, id, 92, eligible(id, &policy), now);
    assert_eq!(
        state.batches.values().next().unwrap().thresholds,
        vec![50, 75, 90]
    );
}

#[test]
fn decrease_while_pending_rearms_without_replacing_historical_batch() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
    let generation = *state.batches.keys().next().unwrap();

    apply_numeric_resolution(&mut state, None, id, 40, eligible(id, &[50]), now);
    assert_eq!(state.sessions[&id].latches[&50], LatchState::Armed);
    assert_eq!(state.batches[&generation].observed, 60);
    apply_numeric_resolution(&mut state, None, id, 55, eligible(id, &[50]), now);

    assert_eq!(state.sessions[&id].latches[&50], LatchState::Latched);
    assert_eq!(state.batches.len(), 1);
    assert_eq!(state.sessions[&id].outstanding[&50], generation);
}

#[tokio::test]
async fn in_flight_policy_pruning_cancels_and_replaces_only_survivors() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 90, eligible(id, &[50, 75]), now);
    let original = *state.batches.keys().next().unwrap();
    state.batches.get_mut(&original).unwrap().failure_count = 3;
    state.batches.get_mut(&original).unwrap().in_flight = true;
    let cancellation = CancellationToken::new();
    let slot = InFlightSlot {
        generation: original,
        cancellation: cancellation.clone(),
        join: tokio::spawn(async {}),
    };

    reconcile_policy(&mut state, Some(&slot), id, &fingerprint(id), &[75], now);

    assert!(cancellation.is_cancelled());
    assert!(!state.batches.contains_key(&original));
    let replacement = state.batches.values().next().unwrap();
    assert_ne!(replacement.generation, original);
    assert_eq!(replacement.thresholds, vec![75]);
    assert_eq!(replacement.observed, 90);
    assert_eq!(replacement.failure_count, 3);
    assert_eq!(replacement.due_at, now);
    assert_eq!(state.sessions[&id].outstanding[&75], replacement.generation);
    slot.join.await.unwrap();
}

#[tokio::test]
async fn exhausted_generation_fails_closed_for_in_flight_replacement() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 90, eligible(id, &[50, 75]), now);
    let generation = *state.batches.keys().next().unwrap();
    state.batches.get_mut(&generation).unwrap().in_flight = true;
    state.next_generation = None;
    let cancellation = CancellationToken::new();
    let slot = InFlightSlot {
        generation,
        cancellation: cancellation.clone(),
        join: tokio::spawn(async {}),
    };

    reconcile_policy(&mut state, Some(&slot), id, &fingerprint(id), &[75], now);

    assert!(cancellation.is_cancelled());
    assert!(state.batches.is_empty());
    assert!(state.sessions[&id].outstanding.is_empty());
    slot.join.await.unwrap();
}

#[tokio::test]
async fn stale_matching_completion_clears_slot_without_mutating_replacement() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 90, eligible(id, &[50, 75]), now);
    let original = *state.batches.keys().next().unwrap();
    state.batches.get_mut(&original).unwrap().in_flight = true;
    let cancellation = CancellationToken::new();
    let mut in_flight = Some(InFlightSlot {
        generation: original,
        cancellation: cancellation.clone(),
        join: tokio::spawn(async {}),
    });
    reconcile_policy(
        &mut state,
        in_flight.as_ref(),
        id,
        &fingerprint(id),
        &[75],
        now,
    );
    let replacement = *state.batches.keys().next().unwrap();
    assert_ne!(replacement, original);

    handle_delivery_completion(
        &mut state,
        &mut in_flight,
        DeliveryCompletion {
            generation: original,
            result: Ok(()),
        },
        now,
    )
    .await;

    assert!(in_flight.is_none());
    assert!(cancellation.is_cancelled());
    assert!(state.batches.contains_key(&replacement));
    assert_eq!(state.batches[&replacement].thresholds, vec![75]);
}

#[tokio::test]
async fn explicit_policy_disable_cancels_in_flight_and_removes_all_state() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
    let generation = *state.batches.keys().next().unwrap();
    state.batches.get_mut(&generation).unwrap().in_flight = true;
    let cancellation = CancellationToken::new();
    let slot = InFlightSlot {
        generation,
        cancellation: cancellation.clone(),
        join: tokio::spawn(async {}),
    };

    apply_numeric_resolution(
        &mut state,
        Some(&slot),
        id,
        60,
        MemberPolicyResolution::Disabled(fingerprint(id)),
        now,
    );

    assert!(cancellation.is_cancelled());
    assert!(!state.sessions.contains_key(&id));
    assert!(state.batches.is_empty());
    slot.join.await.unwrap();
}

#[test]
fn disabled_numeric_resolution_logs_policy_recovery_once_before_removing_state() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();

    let first = apply_numeric_resolution(
        &mut state,
        None,
        id,
        80,
        MemberPolicyResolution::Paused {
            fingerprint: fingerprint(id),
            class: "malformed",
            message: "partial JSON".to_string(),
        },
        now,
    );
    assert_eq!(first, ResolutionRecovery::default());
    assert!(state.sessions[&id].policy_error.is_some());

    let recovered = apply_numeric_resolution(
        &mut state,
        None,
        id,
        80,
        MemberPolicyResolution::Disabled(fingerprint(id)),
        now,
    );
    assert_eq!(
        recovered,
        ResolutionRecovery {
            policy: true,
            runtime: false,
        },
        "the true policy transition is the single info recovery log"
    );
    assert!(!state.sessions.contains_key(&id));

    let repeated = apply_numeric_resolution(
        &mut state,
        None,
        id,
        80,
        MemberPolicyResolution::Disabled(fingerprint(id)),
        now,
    );
    assert_eq!(
        repeated,
        ResolutionRecovery::default(),
        "a later disabled sample has no stale marker to log again"
    );
}

#[tokio::test]
async fn disabled_retry_cancellation_logs_policy_recovery_once_and_drops_state() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50]), now);
    apply_numeric_resolution(
        &mut state,
        None,
        id,
        80,
        MemberPolicyResolution::Paused {
            fingerprint: fingerprint(id),
            class: "unreadable",
            message: "config read failed".to_string(),
        },
        now,
    );
    assert!(state.sessions[&id].policy_error.is_some());
    assert_eq!(state.batches.len(), 1);

    let runtime = ScriptedRuntime::new();
    runtime.set_policy(id, &[]);
    let runtime_trait: Arc<dyn ContextAlertRuntime> = runtime.clone();
    let clock: Arc<dyn AlertClock> = ManualClock::new(now);
    let (completion_tx, _completion_rx) = mpsc::channel(1);
    let shutdown = CancellationToken::new();
    let mut in_flight = None;

    let recovered = dispatch_due_batch(
        &runtime_trait,
        &clock,
        &mut state,
        &mut in_flight,
        &completion_tx,
        &shutdown,
    )
    .await;
    assert_eq!(
        recovered,
        ResolutionRecovery {
            policy: true,
            runtime: false,
        },
        "disabled retry preparation clears and logs the policy marker once"
    );
    assert!(!state.sessions.contains_key(&id));
    assert!(state.batches.is_empty());
    assert_eq!(runtime.prepare_calls.lock().unwrap().len(), 1);

    let repeated = dispatch_due_batch(
        &runtime_trait,
        &clock,
        &mut state,
        &mut in_flight,
        &completion_tx,
        &shutdown,
    )
    .await;
    assert_eq!(repeated, ResolutionRecovery::default());
    assert_eq!(
        runtime.prepare_calls.lock().unwrap().len(),
        1,
        "removed disabled state cannot emit a duplicate recovery log"
    );
}

#[test]
fn retry_preparation_policy_addition_arms_but_does_not_evaluate_cached_usage() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[90]), now);
    assert!(state.batches.is_empty());
    assert_eq!(state.sessions[&id].last_valid_percent, Some(80));

    reconcile_policy(&mut state, None, id, &fingerprint(id), &[50, 90], now);
    assert!(state.batches.is_empty());
    assert_eq!(state.sessions[&id].latches[&50], LatchState::Armed);

    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 90]), now);
    assert_eq!(state.batches.values().next().unwrap().thresholds, vec![50]);
}

#[test]
fn policy_pause_and_runtime_error_transitions_preserve_only_matching_identity() {
    let id = Uuid::new_v4();
    let first_error_id = Uuid::new_v4();
    let first_pause_id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50]), now);
    let generation = *state.batches.keys().next().unwrap();

    let paused = MemberPolicyResolution::Paused {
        fingerprint: fingerprint(id),
        class: "malformed",
        message: "partial JSON".to_string(),
    };
    apply_numeric_resolution(&mut state, None, id, 70, paused.clone(), now);
    let marker = state.sessions[&id].policy_error.clone();
    apply_numeric_resolution(&mut state, None, id, 70, paused, now);
    assert_eq!(state.sessions[&id].policy_error, marker);
    assert!(state.batches.contains_key(&generation));
    assert_eq!(state.sessions[&id].last_valid_percent, Some(60));

    apply_numeric_resolution(&mut state, None, id, 70, eligible(id, &[50]), now);
    assert!(state.sessions[&id].policy_error.is_none());
    assert_eq!(state.batches.len(), 1, "repair must not duplicate a latch");

    apply_numeric_resolution(
        &mut state,
        None,
        first_pause_id,
        90,
        MemberPolicyResolution::Paused {
            fingerprint: fingerprint(first_pause_id),
            class: "malformed",
            message: "first partial read".to_string(),
        },
        now,
    );
    assert!(state.sessions[&first_pause_id].last_valid_percent.is_none());
    assert!(state
        .batches
        .values()
        .all(|batch| batch.session_id != first_pause_id));
    apply_numeric_resolution(
        &mut state,
        None,
        first_pause_id,
        90,
        eligible(first_pause_id, &[50]),
        now,
    );
    assert!(state
        .batches
        .values()
        .any(|batch| batch.session_id == first_pause_id));

    apply_numeric_resolution(
        &mut state,
        None,
        first_error_id,
        90,
        MemberPolicyResolution::RetryableFailure("blocking join failed".to_string()),
        now,
    );
    assert!(state.sessions[&first_error_id].fingerprint.is_none());
    assert!(state.sessions[&first_error_id].last_valid_percent.is_none());
    assert!(state
        .batches
        .values()
        .all(|batch| batch.session_id != first_error_id));
    apply_numeric_resolution(
        &mut state,
        None,
        first_error_id,
        90,
        eligible(first_error_id, &[50]),
        now,
    );
    assert!(state.sessions[&first_error_id].runtime_error.is_none());
    assert!(state
        .batches
        .values()
        .any(|batch| batch.session_id == first_error_id));

    let mut changed = fingerprint(id);
    changed.member = "replacement".to_string();
    apply_numeric_resolution(
        &mut state,
        None,
        id,
        70,
        MemberPolicyResolution::Paused {
            fingerprint: changed.clone(),
            class: "invalid",
            message: "changed identity".to_string(),
        },
        now,
    );
    assert_eq!(state.sessions[&id].fingerprint.as_ref(), Some(&changed));
    assert!(state.batches.values().all(|batch| batch.session_id != id));
    assert!(state.sessions[&id].last_valid_percent.is_none());
}

#[test]
fn earliest_deadline_and_generation_tie_break_prevent_failed_batch_starvation() {
    let id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(&mut state, None, id, 60, eligible(id, &[50, 75]), now);
    apply_numeric_resolution(&mut state, None, id, 80, eligible(id, &[50, 75]), now);
    let generations: Vec<u64> = state.batches.keys().copied().collect();
    assert_eq!(state.earliest_due(), Some((now, generations[0])));

    fail_batch(&mut state, generations[0], now, "permanent first failure");
    assert_eq!(state.earliest_due(), Some((now, generations[1])));
    assert_eq!(state.batches[&generations[0]].failure_count, 1);
    assert_eq!(state.batches[&generations[1]].failure_count, 0);
}

#[test]
fn exhausted_new_crossing_never_wraps_or_corrupts_existing_batch() {
    let existing_id = Uuid::new_v4();
    let exhausted_id = Uuid::new_v4();
    let now = Instant::now();
    let mut state = ActorState::new();
    apply_numeric_resolution(
        &mut state,
        None,
        existing_id,
        60,
        eligible(existing_id, &[50]),
        now,
    );
    let existing_generation = *state.batches.keys().next().unwrap();
    state.next_generation = None;
    apply_numeric_resolution(
        &mut state,
        None,
        exhausted_id,
        60,
        eligible(exhausted_id, &[50]),
        now,
    );

    assert_eq!(state.batches.len(), 1);
    assert!(state.batches.contains_key(&existing_generation));
    assert_eq!(
        state.sessions[&exhausted_id].latches[&50],
        LatchState::Latched
    );
    assert!(state.sessions[&exhausted_id].outstanding.is_empty());
}

#[tokio::test]
async fn ready_attempt_thresholds_propagates_the_option() {
    let fixture = identity_fixture().await;
    let policy = match resolve_member_policy_blocking(&fixture.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected eligible resolution, got {other:?}"),
    };
    let (target, notice) =
        prepare_internal_route_blocking(&policy.fingerprint, 80, &[50, 75]).unwrap();
    let snapshot = AttemptSnapshot {
        generation: 1,
        session_id: policy.fingerprint.session_id,
        fingerprint: policy.fingerprint.clone(),
        observed: 80,
        thresholds: vec![50, 75],
        failure_count: 0,
    };

    let context_attempt = ReadyAttempt {
        snapshot: snapshot.clone(),
        current_policy: vec![50, 75],
        target: target.clone(),
        notice,
        guard: Arc::new(|| Ok(())),
        cancellation: CancellationToken::new(),
    };
    assert_eq!(context_attempt.thresholds(), Some(&[50u8, 75][..]));

    let remote = InternalSystemNotice::for_remote_activity(
        crate::phone::mailbox::RemoteNoticeKind::CiStarted,
        "mblua/AgentsCommander".to_string(),
        "main".to_string(),
        "abcdef1".to_string(),
        String::new(),
        None,
        "2026-09-15 23:18:43-03:00".to_string(),
        None,
    )
    .expect("valid remote notice");
    let remote_attempt = ReadyAttempt {
        snapshot,
        current_policy: vec![50, 75],
        target,
        notice: remote,
        guard: Arc::new(|| Ok(())),
        cancellation: CancellationToken::new(),
    };
    // The re-export must forward the `Option` rather than flatten it.
    assert_eq!(remote_attempt.thresholds(), None);
}
