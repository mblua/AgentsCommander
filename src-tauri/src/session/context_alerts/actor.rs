use super::ActorState;
use super::AlertBatch;
use super::AlertClock;
use super::AttemptPreparation;
use super::AttemptSnapshot;
use super::ContextAlertRuntime;
use super::DeliveryCompletion;
use super::ErrorMarker;
use super::InFlightSlot;
use super::LatchState;
use super::MemberIdentityFingerprint;
use super::MemberPolicyResolution;
use super::ResolutionRecovery;
use super::SessionAlertState;
use super::CONTEXT_ALERT_MAINTENANCE_INTERVAL;
use super::FIRST_RETRY_DELAY;
use super::HOLD_RECHECK_DELAY;
use super::MAX_RETRY_DELAY;
use crate::pty::context_scrape::ContextSample;
use futures::future::BoxFuture;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(super) fn retry_delay(failure_count: u32) -> Duration {
    if failure_count == 0 {
        return Duration::ZERO;
    }
    let shift = failure_count.saturating_sub(1).min(4);
    FIRST_RETRY_DELAY
        .checked_mul(1u32 << shift)
        .unwrap_or(MAX_RETRY_DELAY)
        .min(MAX_RETRY_DELAY)
}

fn optional_sleep(clock: Arc<dyn AlertClock>, deadline: Option<Instant>) -> BoxFuture<'static, ()> {
    match deadline {
        Some(deadline) => clock.sleep_until(deadline),
        None => Box::pin(futures::future::pending()),
    }
}

fn next_actor_deadline(
    state: &ActorState,
    in_flight: Option<&InFlightSlot>,
    maintenance_deadline: Instant,
) -> Instant {
    if in_flight.is_some() {
        return maintenance_deadline;
    }
    state
        .earliest_due()
        .map(|(deadline, _)| deadline.min(maintenance_deadline))
        .unwrap_or(maintenance_deadline)
}

pub(super) async fn run_context_alert_actor(
    runtime: Arc<dyn ContextAlertRuntime>,
    clock: Arc<dyn AlertClock>,
    mut samples: mpsc::Receiver<ContextSample>,
    shutdown: CancellationToken,
) {
    let mut state = ActorState::new();
    let mut samples_open = true;
    let mut maintenance_deadline = clock.now() + CONTEXT_ALERT_MAINTENANCE_INTERVAL;
    let (completion_tx, mut completion_rx) = mpsc::channel::<DeliveryCompletion>(1);
    let mut in_flight: Option<InFlightSlot> = None;

    loop {
        let deadline = next_actor_deadline(&state, in_flight.as_ref(), maintenance_deadline);
        let sleep = optional_sleep(Arc::clone(&clock), Some(deadline));
        tokio::pin!(sleep);

        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                if let Some(slot) = in_flight.take() {
                    slot.cancellation.cancel();
                    if let Err(error) = slot.join.await {
                        log::warn!(
                            "[context-alert] delivery wrapper join failed during shutdown: {}",
                            error
                        );
                    } else if let Ok(completion) = completion_rx.try_recv() {
                        if let Err(error) = completion.result {
                            log::warn!(
                                "[context-alert] delivery generation={} ended with error \
                                 during shutdown: {}",
                                completion.generation,
                                error
                            );
                        }
                    }
                }
                state.sessions.clear();
                state.batches.clear();
                break;
            }
            completion = completion_rx.recv(), if in_flight.is_some() => {
                if let Some(completion) = completion {
                    handle_delivery_completion(
                        &mut state,
                        &mut in_flight,
                        completion,
                        clock.now(),
                    ).await;
                    if clock.now() >= maintenance_deadline {
                        run_maintenance(
                            &runtime,
                            &mut state,
                            in_flight.as_ref(),
                            &shutdown,
                        ).await;
                        maintenance_deadline = clock.now() + CONTEXT_ALERT_MAINTENANCE_INTERVAL;
                    }
                    dispatch_due_batch(
                        &runtime,
                        &clock,
                        &mut state,
                        &mut in_flight,
                        &completion_tx,
                        &shutdown,
                    ).await;
                }
            }
            _ = &mut sleep => {
                if clock.now() >= maintenance_deadline {
                    run_maintenance(
                        &runtime,
                        &mut state,
                        in_flight.as_ref(),
                        &shutdown,
                    ).await;
                    maintenance_deadline = clock.now() + CONTEXT_ALERT_MAINTENANCE_INTERVAL;
                }
                if in_flight.is_none() {
                    dispatch_due_batch(
                        &runtime,
                        &clock,
                        &mut state,
                        &mut in_flight,
                        &completion_tx,
                        &shutdown,
                    ).await;
                }
            }
            sample = samples.recv(), if samples_open => {
                match sample {
                    Some(sample) => {
                        process_sample(
                            &runtime,
                            &clock,
                            &mut state,
                            in_flight.as_ref(),
                            sample,
                            &shutdown,
                        ).await;
                        if clock.now() >= maintenance_deadline {
                            run_maintenance(
                                &runtime,
                                &mut state,
                                in_flight.as_ref(),
                                &shutdown,
                            ).await;
                            maintenance_deadline = clock.now() + CONTEXT_ALERT_MAINTENANCE_INTERVAL;
                        }
                        if in_flight.is_none() {
                            dispatch_due_batch(
                                &runtime,
                                &clock,
                                &mut state,
                                &mut in_flight,
                                &completion_tx,
                                &shutdown,
                            ).await;
                        }
                    }
                    None => {
                        samples_open = false;
                        log::warn!(
                            "[context-alert] sample channel closed; \
                             maintenance and pending retries remain active"
                        );
                    }
                }
            }
        }
    }
}

pub(super) async fn process_sample(
    runtime: &Arc<dyn ContextAlertRuntime>,
    clock: &Arc<dyn AlertClock>,
    state: &mut ActorState,
    in_flight: Option<&InFlightSlot>,
    sample: ContextSample,
    shutdown: &CancellationToken,
) {
    match sample {
        ContextSample::SessionOver { session_id } => {
            state.remove_session(session_id, in_flight);
        }
        ContextSample::Unavailable { session_id } => {
            let live = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                live = runtime.check_session_live(session_id) => live,
            };
            if !live {
                state.remove_session(session_id, in_flight);
                runtime.retire_sample_registration(session_id);
            }
        }
        ContextSample::Reading {
            session_id,
            percent,
        } => {
            if percent > 100 {
                log::error!(
                    "[context-alert] impossible producer reading session={} percent={}",
                    session_id,
                    percent
                );
                return;
            }
            let resolution = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                resolution = runtime.resolve_member_policy(session_id) => resolution,
            };
            apply_numeric_resolution(
                state,
                in_flight,
                session_id,
                percent,
                resolution,
                clock.now(),
            );
        }
    }
}

pub(super) fn apply_numeric_resolution(
    state: &mut ActorState,
    in_flight: Option<&InFlightSlot>,
    session_id: Uuid,
    percent: u8,
    resolution: MemberPolicyResolution,
    now: Instant,
) -> ResolutionRecovery {
    match resolution {
        MemberPolicyResolution::Eligible(resolved) => {
            if resolved.fingerprint.session_id != session_id {
                log::error!(
                    "[context-alert] runtime returned mismatched session identity \
                     expected={} actual={}",
                    session_id,
                    resolved.fingerprint.session_id
                );
                return ResolutionRecovery::default();
            }
            let changed_identity = state
                .sessions
                .get(&session_id)
                .and_then(|session| session.fingerprint.as_ref())
                .is_some_and(|fingerprint| fingerprint != &resolved.fingerprint);
            if changed_identity {
                state.remove_session(session_id, in_flight);
            }
            let session = state
                .sessions
                .entry(session_id)
                .or_insert_with(SessionAlertState::empty);
            session.fingerprint = Some(resolved.fingerprint.clone());
            let recovery = clear_resolution_errors(session, session_id);
            reconcile_policy(
                state,
                in_flight,
                session_id,
                &resolved.fingerprint,
                &resolved.thresholds,
                now,
            );
            if recovery.policy {
                make_session_batches_due_now(state, session_id, now);
            }
            evaluate_numeric_sample(state, session_id, percent, now);
            recovery
        }
        MemberPolicyResolution::Disabled(fingerprint) => {
            let recovery = clear_resolution_errors_for_identity(state, session_id, &fingerprint);
            state.remove_session(session_id, in_flight);
            recovery
        }
        MemberPolicyResolution::PermanentIneligible(_) => {
            state.remove_session(session_id, in_flight);
            ResolutionRecovery::default()
        }
        MemberPolicyResolution::Paused {
            fingerprint,
            class,
            message,
        } => {
            let changed_identity = state
                .sessions
                .get(&session_id)
                .and_then(|session| session.fingerprint.as_ref())
                .is_some_and(|current| current != &fingerprint);
            if changed_identity {
                state.remove_session(session_id, in_flight);
            }
            let session = state
                .sessions
                .entry(session_id)
                .or_insert_with(SessionAlertState::empty);
            session.fingerprint = Some(fingerprint);
            set_policy_error(session, session_id, class, message);
            ResolutionRecovery::default()
        }
        MemberPolicyResolution::RetryableFailure(message) => {
            let session = state
                .sessions
                .entry(session_id)
                .or_insert_with(SessionAlertState::empty);
            set_runtime_error(session, session_id, message);
            ResolutionRecovery::default()
        }
    }
}

fn set_policy_error(
    session: &mut SessionAlertState,
    session_id: Uuid,
    class: &'static str,
    message: String,
) {
    let next = ErrorMarker {
        class: class.to_string(),
        message,
    };
    if session.policy_error.as_ref() != Some(&next) {
        log::warn!(
            "[context-alert] policy paused session={} class={} error={}",
            session_id,
            next.class,
            next.message
        );
        session.policy_error = Some(next);
    }
}

fn clear_policy_error(session: &mut SessionAlertState, session_id: Uuid) -> bool {
    if session.policy_error.take().is_some() {
        log::info!("[context-alert] policy recovered session={}", session_id);
        true
    } else {
        false
    }
}

fn set_runtime_error(session: &mut SessionAlertState, session_id: Uuid, message: String) {
    let next = ErrorMarker {
        class: "runtime".to_string(),
        message,
    };
    if session.runtime_error.as_ref() != Some(&next) {
        log::warn!(
            "[context-alert] resolution failed session={} error={}",
            session_id,
            next.message
        );
        session.runtime_error = Some(next);
    }
}

fn clear_runtime_error(session: &mut SessionAlertState, session_id: Uuid) -> bool {
    if session.runtime_error.take().is_some() {
        log::info!(
            "[context-alert] resolution recovered session={}",
            session_id
        );
        true
    } else {
        false
    }
}

fn clear_resolution_errors(
    session: &mut SessionAlertState,
    session_id: Uuid,
) -> ResolutionRecovery {
    ResolutionRecovery {
        runtime: clear_runtime_error(session, session_id),
        policy: clear_policy_error(session, session_id),
    }
}

fn clear_resolution_errors_for_identity(
    state: &mut ActorState,
    session_id: Uuid,
    fingerprint: &MemberIdentityFingerprint,
) -> ResolutionRecovery {
    let same_identity = fingerprint.session_id == session_id
        && state.sessions.get(&session_id).is_some_and(|session| {
            session
                .fingerprint
                .as_ref()
                .is_none_or(|current| current == fingerprint)
        });
    if !same_identity {
        return ResolutionRecovery::default();
    }
    state
        .sessions
        .get_mut(&session_id)
        .map(|session| clear_resolution_errors(session, session_id))
        .unwrap_or_default()
}

pub(super) fn reconcile_policy(
    state: &mut ActorState,
    in_flight: Option<&InFlightSlot>,
    session_id: Uuid,
    fingerprint: &MemberIdentityFingerprint,
    new_policy: &[u8],
    now: Instant,
) {
    let old_policy = state
        .sessions
        .get(&session_id)
        .map(|session| session.policy.clone())
        .unwrap_or_default();
    let removed: HashSet<u8> = old_policy
        .iter()
        .copied()
        .filter(|threshold| !new_policy.contains(threshold))
        .collect();

    if !removed.is_empty() {
        let affected: Vec<u64> = state
            .batches
            .values()
            .filter(|batch| {
                batch.session_id == session_id
                    && batch
                        .thresholds
                        .iter()
                        .any(|threshold| removed.contains(threshold))
            })
            .map(|batch| batch.generation)
            .collect();
        let mut replacements = Vec::new();
        for generation in affected {
            let Some(mut batch) = state.batches.remove(&generation) else {
                continue;
            };
            let old_thresholds = batch.thresholds.clone();
            batch
                .thresholds
                .retain(|threshold| !removed.contains(threshold));
            if in_flight.is_some_and(|slot| slot.generation == generation) {
                if let Some(slot) = in_flight {
                    slot.cancellation.cancel();
                }
                if !batch.thresholds.is_empty() {
                    replacements.push((
                        batch.observed,
                        batch.thresholds.clone(),
                        batch.failure_count,
                    ));
                }
            } else if !batch.thresholds.is_empty() {
                state.batches.insert(generation, batch);
            }
            if let Some(session) = state.sessions.get_mut(&session_id) {
                for threshold in old_thresholds {
                    if session.outstanding.get(&threshold) == Some(&generation) {
                        session.outstanding.remove(&threshold);
                    }
                }
                if let Some(current) = state.batches.get(&generation) {
                    for threshold in &current.thresholds {
                        session.outstanding.insert(*threshold, generation);
                    }
                }
            }
        }
        for (observed, thresholds, failure_count) in replacements {
            create_batch(
                state,
                session_id,
                fingerprint.clone(),
                observed,
                thresholds,
                failure_count,
                now,
            );
        }
    }

    let session = state
        .sessions
        .entry(session_id)
        .or_insert_with(SessionAlertState::empty);
    session.policy = new_policy.to_vec();
    session
        .latches
        .retain(|threshold, _| new_policy.contains(threshold));
    session
        .outstanding
        .retain(|threshold, _| new_policy.contains(threshold));
    for threshold in new_policy {
        session
            .latches
            .entry(*threshold)
            .or_insert(LatchState::Armed);
    }
}

fn evaluate_numeric_sample(state: &mut ActorState, session_id: Uuid, percent: u8, now: Instant) {
    let Some(session) = state.sessions.get_mut(&session_id) else {
        return;
    };
    let Some(fingerprint) = session.fingerprint.clone() else {
        return;
    };
    let mut newly_crossed = Vec::new();
    for threshold in session.policy.clone() {
        let latch = session
            .latches
            .entry(threshold)
            .or_insert(LatchState::Armed);
        if percent < threshold {
            *latch = LatchState::Armed;
        } else if *latch == LatchState::Armed {
            *latch = LatchState::Latched;
            if !session.outstanding.contains_key(&threshold) {
                newly_crossed.push(threshold);
            }
        }
    }
    session.last_valid_percent = Some(percent);
    if newly_crossed.is_empty() {
        return;
    }
    newly_crossed.sort_unstable();
    create_batch(
        state,
        session_id,
        fingerprint,
        percent,
        newly_crossed,
        0,
        now,
    );
}

fn create_batch(
    state: &mut ActorState,
    session_id: Uuid,
    fingerprint: MemberIdentityFingerprint,
    observed: u8,
    thresholds: Vec<u8>,
    failure_count: u32,
    due_at: Instant,
) {
    let Some(generation) = state.allocate_generation() else {
        if !state.generation_exhaustion_logged {
            log::error!("[context-alert] batch generation exhausted; new crossings remain latched");
            state.generation_exhaustion_logged = true;
        }
        return;
    };
    if let Some(session) = state.sessions.get_mut(&session_id) {
        for threshold in &thresholds {
            session.outstanding.insert(*threshold, generation);
        }
    }
    state.batches.insert(
        generation,
        AlertBatch {
            generation,
            session_id,
            fingerprint,
            observed,
            thresholds,
            failure_count,
            due_at,
            in_flight: false,
        },
    );
}

fn make_session_batches_due_now(state: &mut ActorState, session_id: Uuid, now: Instant) {
    for batch in state.batches.values_mut() {
        if batch.session_id == session_id && !batch.in_flight {
            batch.due_at = now;
        }
    }
}

pub(super) async fn dispatch_due_batch(
    runtime: &Arc<dyn ContextAlertRuntime>,
    clock: &Arc<dyn AlertClock>,
    state: &mut ActorState,
    in_flight: &mut Option<InFlightSlot>,
    completion_tx: &mpsc::Sender<DeliveryCompletion>,
    shutdown: &CancellationToken,
) -> ResolutionRecovery {
    if in_flight.is_some() {
        return ResolutionRecovery::default();
    }
    let now = clock.now();
    let Some((due_at, generation)) = state.earliest_due() else {
        return ResolutionRecovery::default();
    };
    if due_at > now {
        return ResolutionRecovery::default();
    }
    let Some(batch) = state.batches.get(&generation).cloned() else {
        return ResolutionRecovery::default();
    };
    let snapshot = AttemptSnapshot {
        generation,
        session_id: batch.session_id,
        fingerprint: batch.fingerprint.clone(),
        observed: batch.observed,
        thresholds: batch.thresholds,
        failure_count: batch.failure_count,
    };
    let attempt_cancellation = shutdown.child_token();
    let preparation = tokio::select! {
        biased;
        _ = shutdown.cancelled() => return ResolutionRecovery::default(),
        preparation = runtime.prepare_attempt(snapshot, attempt_cancellation.clone()) =>
            preparation,
    };

    match preparation {
        AttemptPreparation::Ready(attempt) => {
            let session_id = attempt.snapshot.session_id;
            let fingerprint_matches = state
                .sessions
                .get(&session_id)
                .and_then(|session| session.fingerprint.as_ref())
                == Some(&attempt.snapshot.fingerprint);
            if !fingerprint_matches {
                state.remove_session(session_id, None);
                return ResolutionRecovery::default();
            }
            reconcile_policy(
                state,
                None,
                session_id,
                &attempt.snapshot.fingerprint,
                &attempt.current_policy,
                now,
            );
            let still_matches = state.batches.get(&generation).is_some_and(|current| {
                // Only a context alert ever reaches this actor, so the invariant
                // is named rather than defaulted: a remote notice must never
                // silently count as "no thresholds crossed".
                let attempt_thresholds = attempt
                    .thresholds()
                    .expect("a context-alert attempt always carries thresholds");
                current.thresholds == attempt_thresholds
                    && current.fingerprint == attempt.snapshot.fingerprint
            });
            if !still_matches {
                return ResolutionRecovery::default();
            }
            if let Some(current) = state.batches.get_mut(&generation) {
                current.in_flight = true;
            }
            let recovery = state
                .sessions
                .get_mut(&session_id)
                .map(|session| clear_resolution_errors(session, session_id))
                .unwrap_or_default();
            let runtime = Arc::clone(runtime);
            let tx = completion_tx.clone();
            let join = tokio::spawn(async move {
                let delivery = tokio::spawn(async move { runtime.deliver_attempt(attempt).await });
                let result = match delivery.await {
                    Ok(result) => result,
                    Err(error) => Err(format!("Context alert delivery task failed: {}", error)),
                };
                let _ = tx.send(DeliveryCompletion { generation, result }).await;
            });
            *in_flight = Some(InFlightSlot {
                generation,
                cancellation: attempt_cancellation,
                join,
            });
            recovery
        }
        AttemptPreparation::Cancel {
            reason,
            current_policy,
        } => {
            log::debug!(
                "[context-alert] canceled pending generation={} reason={}",
                generation,
                reason
            );
            match current_policy {
                Some(policy) if policy.fingerprint == batch.fingerprint => {
                    let recovery = clear_resolution_errors_for_identity(
                        state,
                        batch.session_id,
                        &policy.fingerprint,
                    );
                    if policy.thresholds.is_empty() {
                        state.remove_session(batch.session_id, None);
                    } else {
                        reconcile_policy(
                            state,
                            None,
                            batch.session_id,
                            &policy.fingerprint,
                            &policy.thresholds,
                            now,
                        );
                        state.remove_batch(generation);
                    }
                    recovery
                }
                _ => {
                    state.remove_session(batch.session_id, None);
                    ResolutionRecovery::default()
                }
            }
        }
        AttemptPreparation::Paused {
            fingerprint,
            class,
            message,
        } => {
            let same_identity = state
                .sessions
                .get(&batch.session_id)
                .and_then(|session| session.fingerprint.as_ref())
                == Some(&fingerprint);
            if !same_identity {
                state.remove_session(batch.session_id, None);
                return ResolutionRecovery::default();
            }
            if let Some(session) = state.sessions.get_mut(&batch.session_id) {
                set_policy_error(session, batch.session_id, class, message.clone());
            }
            fail_batch(state, generation, now, &message);
            ResolutionRecovery::default()
        }
        AttemptPreparation::RetryableFailure(message) => {
            fail_batch(state, generation, now, &message);
            ResolutionRecovery::default()
        }
    }
}

fn defer_batch(state: &mut ActorState, generation: u64, now: Instant) {
    if let Some(batch) = state.batches.get_mut(&generation) {
        batch.in_flight = false;
        batch.due_at = now + HOLD_RECHECK_DELAY;
    }
}

pub(super) fn fail_batch(state: &mut ActorState, generation: u64, now: Instant, message: &str) {
    let Some(batch) = state.batches.get_mut(&generation) else {
        return;
    };
    batch.in_flight = false;
    batch.failure_count = batch.failure_count.saturating_add(1);
    batch.due_at = now + retry_delay(batch.failure_count);
    log::warn!(
        "[context-alert] generation={} attempt={} failed; retryInMs={} error={}",
        generation,
        batch.failure_count,
        retry_delay(batch.failure_count).as_millis(),
        message
    );
}

pub(super) async fn handle_delivery_completion(
    state: &mut ActorState,
    in_flight: &mut Option<InFlightSlot>,
    completion: DeliveryCompletion,
    now: Instant,
) {
    let Some(slot) = in_flight.take() else {
        log::debug!(
            "[context-alert] ignored completion generation={} without a global slot",
            completion.generation
        );
        return;
    };
    let slot_generation = slot.generation;
    if let Err(error) = slot.join.await {
        log::warn!(
            "[context-alert] delivery wrapper generation={} join failed: {}",
            slot_generation,
            error
        );
    }
    if slot_generation != completion.generation {
        log::debug!(
            "[context-alert] ignored mismatched completion slot={} result={}",
            slot_generation,
            completion.generation
        );
        return;
    }
    let matching = state
        .batches
        .get(&completion.generation)
        .is_some_and(|batch| batch.in_flight);
    if !matching {
        log::debug!(
            "[context-alert] ignored stale completion generation={}",
            completion.generation
        );
        return;
    }
    match completion.result {
        Ok(()) => {
            state.remove_batch(completion.generation);
        }
        Err(message) if crate::phone::mailbox::is_deferred_internal_delivery_error(&message) => {
            defer_batch(state, completion.generation, now);
        }
        Err(message) => fail_batch(state, completion.generation, now, &message),
    }
}

pub(super) async fn run_maintenance(
    runtime: &Arc<dyn ContextAlertRuntime>,
    state: &mut ActorState,
    in_flight: Option<&InFlightSlot>,
    shutdown: &CancellationToken,
) {
    let ids: Vec<Uuid> = state.sessions.keys().copied().collect();
    for session_id in ids {
        let live = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            live = runtime.check_session_live(session_id) => live,
        };
        if !live {
            state.remove_session(session_id, in_flight);
            runtime.retire_sample_registration(session_id);
        }
    }
}
