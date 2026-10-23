use super::*;

#[test]
fn natural_exit_reaps_after_two_strikes_and_confirm() {
    let (state, backend) = state_with_fake();
    let root = identity(60, 60);
    backend.add_tree(root, vec![observed(60, 60, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    // Live root: no strike.
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
    assert_eq!(state.active_agent_groups(), 1);

    // Root exits: tree reports it missing, observe_identity confirms gone.
    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    backend.mark_gone(root);

    // First strike: still Running (the two-strike pre-filter).
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
    assert_eq!(state.test_strikes(id), Some(1));
    assert_eq!(state.active_agent_groups(), 1);

    // Second strike + confirm gone: reaped, slot reclaimed.
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Terminated));
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

// (a2) H1 - a live sample between two missing-root samples resets the strike count.
#[test]
fn missing_root_strike_resets_on_live_sample() {
    let (state, backend) = state_with_fake();
    let root = identity(62, 62);
    backend.add_tree(root, vec![observed(62, 62, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    // One missing-root strike.
    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    state.snapshot(limits(1));
    assert_eq!(state.test_strikes(id), Some(1));

    // Live again: strike resets.
    backend.replace_tree(root, vec![observed(62, 62, None, 0)], Vec::new());
    state.snapshot(limits(1));
    assert_eq!(state.test_strikes(id), Some(0));

    // A single fresh missing-root sample only reaches one strike again: no reap.
    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    state.snapshot(limits(1));
    assert_eq!(state.test_strikes(id), Some(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
    assert_eq!(state.active_agent_groups(), 1);
}

// (a3 = T1, MANDATORY) H1 - observe_tree Err must never reap or touch the strikes.
#[test]
fn never_reaps_when_observe_tree_errors() {
    let (state, backend) = state_with_fake();
    let root = identity(63, 63);
    backend.add_tree(root, vec![observed(63, 63, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    // A failed enumeration routes through set_group_error (the Err arm), which never
    // reaches merge_sample. Assert no reap and no strike change across two ticks.
    backend.mark_fail_observe(root);
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
    assert_eq!(state.test_strikes(id), Some(0));
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
    assert_eq!(state.test_strikes(id), Some(0));
    assert_eq!(state.active_agent_groups(), 1);
}

// (a4) H1 - a recycled root (PID reuse) reaps after two strikes + a mismatch confirm.
#[test]
fn recycled_root_reaps_after_two_strikes() {
    let (state, backend) = state_with_fake();
    let root = identity(64, 64);
    backend.add_tree(root, vec![observed(64, 64, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    // Tree reports missing-root (the creation-time-mismatch path) and the confirm
    // returns a bumped identity (foreign process recycled the pid).
    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    backend.mark_stale(root);

    state.snapshot(limits(1));
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Terminated));
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

// (a5) H1 - a spurious missing-root signal on two ticks is overridden by the confirm
// when the root is actually still alive: no reap, strikes reset.
#[test]
fn confirm_overrides_spurious_missing_root_signal() {
    let (state, backend) = state_with_fake();
    let root = identity(65, 65);
    backend.add_tree(root, vec![observed(65, 65, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    // Root stays in the identities map, so observe_identity returns Ok(Some(root)).
    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    state.snapshot(limits(1));
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
    assert_eq!(state.test_strikes(id), Some(0));
    assert_eq!(state.active_agent_groups(), 1);
}

// (a6) H1 - an unverifiable confirm (observe_identity Err) aborts the reap.
#[test]
fn unverifiable_confirm_aborts_reap() {
    let (state, backend) = state_with_fake();
    let root = identity(66, 66);
    backend.add_tree(root, vec![observed(66, 66, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    // Missing-root signal fires, but observe_identity returns Err (pid still exists,
    // momentarily unreadable): conservative abort.
    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    backend.mark_unverifiable(root.pid);
    state.snapshot(limits(1));
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
    assert_eq!(state.test_strikes(id), Some(0));
    assert_eq!(state.active_agent_groups(), 1);
}

// (b) H2 - re-invoking kill_group on a quarantine after the blocker clears frees it.
#[test]
fn quarantine_recleanup_frees_slot() {
    let (state, backend) = state_with_fake();
    let root = identity(67, 67);
    let child = identity(68, 68);
    backend.add_tree(
        root,
        vec![observed(67, 67, None, 0), observed(68, 68, Some(67), 1)],
    );
    backend.mark_stubborn(child);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    let result = state.kill_group(id, ResourceKillReason::User).unwrap();
    assert!(result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Quarantined);
    assert_eq!(state.active_agent_groups(), 1);

    // The stubborn child finally exits; the watchdog-driven retry now cleans up.
    backend.mark_gone(child);
    let retry = state.kill_group(id, ResourceKillReason::Watchdog).unwrap();
    assert!(!retry.quarantined);
    assert_eq!(retry.state, ResourceGroupState::Terminated);
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

// #1151 (R1) - the fix itself at registry level: once the blocker is verifiably gone,
// the registry-only retry releases the permit and the next launch is admitted.
#[test]
fn orphan_quarantine_retry_reclaims_slot() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, child) = quarantined_group(&state, &backend, id, 80, 1, None);
    assert_eq!(state.active_agent_groups(), 1);

    backend.mark_gone(child);
    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    let QuarantineRetryOutcome::Completed(result) = outcome else {
        panic!("an eligible orphan retry runs the cleanup: {outcome:?}");
    };
    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert!(!state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

// #1151 (R2) - the safety half of R1: an owned descendant that is still unverifiable
// keeps blocking capacity. The retry decides only WHETHER kill_group runs.
#[test]
fn orphan_retry_keeps_live_descendant_quarantined() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, _child) = quarantined_group(&state, &backend, id, 82, 1, None);

    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    let QuarantineRetryOutcome::Completed(result) = outcome else {
        panic!("an eligible orphan retry runs the cleanup: {outcome:?}");
    };
    assert!(result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Quarantined);
    assert!(state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 1);
    assert!(state.try_reserve_agent_slot(limits(1)).is_err());
}

// #1151 (R3) - a stale sample bound to a different root identity cannot act.
#[test]
fn orphan_retry_rejects_root_mismatch() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (_root, _child) = quarantined_group(&state, &backend, id, 84, 1, None);

    let before = backend.call_counts();
    let outcome = state
        .retry_orphaned_quarantine(id, identity(999, 999))
        .unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::RootMismatch)
    ));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R4) - a Running group is never taken by the orphan path. That case belongs
// to the missing-root strike reaper (#1040), not to this fix.
#[test]
fn orphan_retry_rejects_non_quarantined_state() {
    let (state, backend) = state_with_fake();
    let root = identity(86, 86);
    backend.add_tree(root, vec![observed(86, 86, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    let before = backend.call_counts();
    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::NotQuarantined(
            ResourceGroupState::Running
        ))
    ));
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R5) - defensive arm only. A Quarantined row can never be removed from the
// group map, so this is unreachable through real sampling and must be driven with a
// fabricated UUID.
#[test]
fn orphan_retry_rejects_unregistered() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, _child) = quarantined_group(&state, &backend, id, 88, 1, None);

    let before = backend.call_counts();
    let outcome = state
        .retry_orphaned_quarantine(Uuid::new_v4(), root)
        .unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::NotRegistered)
    ));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R6) - deterministic stand-in for the concurrent-kill case: another caller
// already owns the transition, so the orphan retry must not touch it.
#[test]
fn orphan_retry_rejects_terminating_state() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, _child) = quarantined_group(&state, &backend, id, 90, 1, None);
    state.test_force_state(id, ResourceGroupState::Terminating);

    let before = backend.call_counts();
    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::NotQuarantined(
            ResourceGroupState::Terminating
        ))
    ));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Terminating));
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R7) - a second retry after a successful one is a no-op. The watchdog
// re-samples every tick, so this runs in production whenever a tick overlaps a
// just-completed reclaim.
#[test]
fn orphan_retry_is_idempotent_after_success() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, child) = quarantined_group(&state, &backend, id, 92, 1, None);

    backend.mark_gone(child);
    assert!(matches!(
        state.retry_orphaned_quarantine(id, root).unwrap(),
        QuarantineRetryOutcome::Completed(_)
    ));
    assert_eq!(state.active_agent_groups(), 0);

    let before = backend.call_counts();
    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::NotQuarantined(
            ResourceGroupState::Terminated
        ))
    ));
    assert_eq!(state.active_agent_groups(), 0);
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R8) - defense in depth. The watchdog is already gated on capability, so this
// check is unreachable on Windows; it must still cost nothing where it is reachable.
#[test]
fn unsupported_backend_orphan_retry_is_noop() {
    let backend = Arc::new(CapabilityTestBackend::new(false));
    let state = ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);

    let outcome = state
        .retry_orphaned_quarantine(Uuid::new_v4(), identity(94, 94))
        .unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::Unsupported)
    ));
    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend.current_process_memory_calls.load(Ordering::SeqCst),
        0
    );
}
