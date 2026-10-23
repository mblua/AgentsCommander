use super::*;

#[test]
fn unsupported_backend_never_consumes_agent_capacity() {
    let backend = Arc::new(CapabilityTestBackend::new(false));
    let state = ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);
    let test_limits = limits(1);

    for _ in 0..33 {
        assert!(state.try_reserve_agent_slot(test_limits).unwrap().is_none());
        assert_eq!(state.active_agent_groups(), 0);
        assert_disabled_snapshot(&state.snapshot(test_limits), test_limits);
    }

    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend.current_process_memory_calls.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn supported_backend_still_enforces_agent_capacity() {
    let (state, _backend) = state_with_fake();
    let first = state
        .try_reserve_agent_slot(limits(1))
        .unwrap()
        .expect("default-supported backend reserves the first slot");

    let error = state.try_reserve_agent_slot(limits(1)).unwrap_err();
    assert!(error.contains("1/1"), "unexpected cap error: {error}");

    state.release_unregistered_permit(first);
    let next = state
        .try_reserve_agent_slot(limits(1))
        .unwrap()
        .expect("released default-supported permit restores capacity");
    state.release_unregistered_permit(next);
}

#[test]
fn unsupported_backend_releases_preissued_permits_before_registration() {
    let backend = Arc::new(CapabilityTestBackend::new(true));
    let state = ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);
    let test_limits = limits(1);

    let logical_permit = state.try_reserve_agent_slot(test_limits).unwrap().unwrap();
    backend.set_supported(false);
    assert!(state.hold_logical_agent_slot(logical_permit).is_none());
    backend.set_supported(true);
    let replacement = state
        .try_reserve_agent_slot(test_limits)
        .unwrap()
        .expect("logical-slot rejection releases the preissued permit");
    state.release_unregistered_permit(replacement);

    let registration_permit = state.try_reserve_agent_slot(test_limits).unwrap().unwrap();
    let session_id = Uuid::new_v4();
    let mut registration = ResourceLaunchRegistration::new(
        state.clone(),
        registration_permit,
        ResourceLaunchMetadata {
            session_id,
            name: "agent".to_string(),
            agent_id: None,
            agent_label: None,
            workgroup: None,
            agent: None,
            project: None,
        },
    );
    backend.set_supported(false);
    assert_eq!(registration.register_root_pid(77).unwrap(), None);
    assert!(!state.has_registered_group(session_id));
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);

    backend.set_supported(true);
    let replacement = state
        .try_reserve_agent_slot(test_limits)
        .unwrap()
        .expect("registration rejection releases the preissued permit");
    state.release_unregistered_permit(replacement);
}

#[test]
fn cap_rejects_when_active_plus_pending_reaches_limit() {
    let (state, _backend) = state_with_fake();
    let _permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let err = state.try_reserve_agent_slot(limits(1)).unwrap_err();
    assert!(err.contains("cap reached"));
}

#[test]
fn snapshot_merges_descendants() {
    let (state, backend) = state_with_fake();
    let root = identity(10, 10);
    backend.add_tree(
        root,
        vec![observed(10, 10, None, 0), observed(11, 11, Some(10), 1)],
    );
    let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
    state
        .register_group(
            permit,
            Uuid::new_v4(),
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();
    let snapshot = state.snapshot(limits(3));
    assert_eq!(snapshot.groups[0].process_count, 2);
    assert!(snapshot.groups[0].descendants_observed);
    assert_eq!(snapshot.overall_state, ResourceOverallState::Ok);
}

#[test]
fn kill_uses_deepest_first_order() {
    let (state, backend) = state_with_fake();
    let root = identity(20, 20);
    backend.add_tree(
        root,
        vec![
            observed(20, 20, None, 0),
            observed(21, 21, Some(20), 1),
            observed(22, 22, Some(21), 2),
        ],
    );
    let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
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
    assert!(!result.quarantined);
    let killed = backend.terminated();
    assert_eq!(killed[0].pid, 22);
    assert_eq!(killed[1].pid, 21);
    assert_eq!(killed[2].pid, 20);
    assert_eq!(state.active_agent_groups(), 0);
}

#[test]
fn stale_identity_is_not_killed_and_releases_permit() {
    let (state, backend) = state_with_fake();
    let root = identity(30, 30);
    let child = identity(31, 31);
    backend.add_tree(
        root,
        vec![observed(30, 30, None, 0), observed(31, 31, Some(30), 1)],
    );
    backend.mark_stale(child);
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
    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert!(!backend.terminated().contains(&child));
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

#[test]
fn stubborn_identity_quarantines_and_keeps_permit() {
    let (state, backend) = state_with_fake();
    let root = identity(32, 32);
    let child = identity(33, 33);
    backend.add_tree(
        root,
        vec![observed(32, 32, None, 0), observed(33, 33, Some(32), 1)],
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
    assert!(state.try_reserve_agent_slot(limits(1)).is_err());
}

#[test]
fn vanished_root_and_already_gone_identities_release_permit() {
    let (state, backend) = state_with_fake();
    let root = identity(34, 34);
    backend.add_tree(root, vec![observed(34, 34, None, 0)]);
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

    backend.mark_gone(root);
    backend.replace_tree(
        root,
        Vec::new(),
        vec![format!("root pid {} was not in process snapshot", root.pid)],
    );

    let result = state
        .kill_group(id, ResourceKillReason::SessionDestroy)
        .unwrap();
    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert!(result.killed_processes.is_empty());
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

#[test]
fn observed_unverifiable_process_quarantines_and_keeps_permit() {
    let (state, backend) = state_with_fake();
    let root = identity(35, 35);
    let unverifiable_child_pid = 36;
    let unverifiable_child = ProcessIdentity {
        pid: unverifiable_child_pid,
        creation_time_100ns: 0,
    };
    backend.add_tree(root, vec![observed(35, 35, None, 0)]);
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

    backend.replace_tree(
        root,
        vec![
            observed(35, 35, None, 0),
            observed_unverifiable(unverifiable_child_pid, Some(root.pid), 1),
        ],
        vec![format!(
            "identity unavailable for pid {unverifiable_child_pid}"
        )],
    );
    backend.mark_present(unverifiable_child);
    backend.mark_unverifiable(unverifiable_child.pid);

    let result = state.kill_group(id, ResourceKillReason::User).unwrap();
    assert!(result.quarantined);
    assert!(result.message.contains("identity unavailable"));
    assert_eq!(state.active_agent_groups(), 1);
    assert!(state.try_reserve_agent_slot(limits(1)).is_err());
}

#[test]
fn child_identity_error_clears_if_parent_termination_removes_child() {
    let (state, backend) = state_with_fake();
    let root = identity(41, 41);
    let child = identity(42, 42);
    backend.add_tree(
        root,
        vec![observed(41, 41, None, 0), observed(42, 42, Some(41), 1)],
    );
    backend.mark_unverifiable(child.pid);
    backend.mark_remove_on_terminate(root, vec![child]);
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

    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert_eq!(result.killed_processes, vec![root]);
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

#[test]
fn cleanup_sample_child_identity_error_clears_if_parent_termination_removes_child() {
    let (state, backend) = state_with_fake();
    let root = identity(43, 43);
    let unverifiable_child_pid = 44;
    let unverifiable_child = ProcessIdentity {
        pid: unverifiable_child_pid,
        creation_time_100ns: 0,
    };
    backend.add_tree(root, vec![observed(43, 43, None, 0)]);
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

    backend.replace_tree(
        root,
        vec![
            observed(43, 43, None, 0),
            observed_unverifiable(unverifiable_child_pid, Some(root.pid), 1),
        ],
        vec![format!(
            "identity unavailable for pid {unverifiable_child_pid}"
        )],
    );
    backend.mark_present(unverifiable_child);
    backend.mark_unverifiable(unverifiable_child.pid);
    backend.mark_remove_on_terminate(root, vec![unverifiable_child]);

    let result = state.kill_group(id, ResourceKillReason::User).unwrap();

    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert_eq!(result.killed_processes, vec![root]);
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

#[test]
fn cleanup_sample_identity_error_for_prior_target_clears_if_kill_removes_all_pids() {
    let (state, backend) = state_with_fake();
    let root = identity(49, 49);
    let node = identity(50, 50);
    let codex = identity(51, 51);
    backend.add_tree(
        root,
        vec![
            observed(49, 49, None, 0),
            observed(50, 50, Some(root.pid), 1),
            observed(51, 51, Some(node.pid), 2),
        ],
    );
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

    backend.replace_tree(
        root,
        vec![observed(49, 49, None, 0)],
        vec![format!("identity unavailable for pid {}", node.pid)],
    );

    let result = state.kill_group(id, ResourceKillReason::User).unwrap();

    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert_eq!(backend.terminated(), vec![codex, node, root]);
    assert!(state.observe_identity(root.pid).unwrap().is_none());
    assert!(state.observe_identity(node.pid).unwrap().is_none());
    assert!(state.observe_identity(codex.pid).unwrap().is_none());
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

#[test]
fn cleanup_sample_placeholder_identity_error_quarantines_when_child_still_alive() {
    let (state, backend) = state_with_fake();
    let root = identity(45, 45);
    let child_pid = 46;
    let concrete_child = identity(child_pid, 46);
    backend.add_tree(root, vec![observed(45, 45, None, 0)]);
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

    backend.replace_tree(
        root,
        vec![
            observed(45, 45, None, 0),
            observed_unverifiable(child_pid, Some(root.pid), 1),
        ],
        vec![format!("identity unavailable for pid {child_pid}")],
    );
    backend.mark_present(concrete_child);

    let result = state.kill_group(id, ResourceKillReason::User).unwrap();

    assert!(result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Quarantined);
    assert!(result.message.contains("identity unavailable"));
    assert_eq!(backend.terminated(), vec![root]);
    assert_eq!(state.active_agent_groups(), 1);
    assert!(state.try_reserve_agent_slot(limits(1)).is_err());
}

#[test]
fn stale_unverifiable_placeholder_gone_before_cleanup_releases_permit() {
    let (state, backend) = state_with_fake();
    let root = identity(36, 36);
    let unverifiable_child_pid = 360;
    backend.add_tree(root, vec![observed(36, 36, None, 0)]);
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

    backend.replace_tree(
        root,
        vec![
            observed(36, 36, None, 0),
            observed_unverifiable(unverifiable_child_pid, Some(root.pid), 1),
        ],
        vec![format!(
            "identity unavailable for pid {unverifiable_child_pid}"
        )],
    );
    let snapshot = state.snapshot(limits(1));
    assert_eq!(snapshot.groups[0].process_count, 2);

    backend.replace_tree(root, vec![observed(36, 36, None, 0)], Vec::new());

    let result = state
        .kill_group(id, ResourceKillReason::SessionDestroy)
        .unwrap();
    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert_eq!(backend.terminated(), vec![root]);
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

#[test]
fn stale_unverifiable_placeholder_still_alive_quarantines() {
    let (state, backend) = state_with_fake();
    let root = identity(47, 47);
    let child_pid = 48;
    let concrete_child = identity(child_pid, 48);
    backend.add_tree(root, vec![observed(47, 47, None, 0)]);
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

    backend.replace_tree(
        root,
        vec![
            observed(47, 47, None, 0),
            observed_unverifiable(child_pid, Some(root.pid), 1),
        ],
        vec![format!("identity unavailable for pid {child_pid}")],
    );
    let snapshot = state.snapshot(limits(1));
    assert_eq!(snapshot.groups[0].process_count, 2);

    backend.replace_tree(root, vec![observed(47, 47, None, 0)], Vec::new());
    backend.mark_present(concrete_child);

    let result = state
        .kill_group(id, ResourceKillReason::SessionDestroy)
        .unwrap();

    assert!(result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Quarantined);
    assert!(result.message.contains("identity unavailable"));
    assert_eq!(backend.terminated(), vec![root]);
    assert_eq!(state.active_agent_groups(), 1);
    assert!(state.try_reserve_agent_slot(limits(1)).is_err());
}

#[test]
fn observe_identity_error_quarantines_and_keeps_permit() {
    let (state, backend) = state_with_fake();
    let root = identity(37, 37);
    backend.add_tree(root, vec![observed(37, 37, None, 0)]);
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

    backend.mark_unverifiable(root.pid);
    backend.replace_tree(
        root,
        Vec::new(),
        vec![format!("root pid {} was not in process snapshot", root.pid)],
    );

    let result = state.kill_group(id, ResourceKillReason::User).unwrap();
    assert!(result.quarantined);
    assert!(result.message.contains("identity unavailable"));
    assert_eq!(state.active_agent_groups(), 1);
    assert!(state.try_reserve_agent_slot(limits(1)).is_err());
}

#[test]
fn exited_during_termination_releases_permit() {
    let (state, backend) = state_with_fake();
    let root = identity(38, 38);
    backend.add_tree(root, vec![observed(38, 38, None, 0)]);
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

    backend.mark_exit_during_terminate(root);

    let result = state.kill_group(id, ResourceKillReason::User).unwrap();
    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert!(result.killed_processes.is_empty());
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

#[test]
fn double_kill_is_idempotent_after_termination() {
    let (state, backend) = state_with_fake();
    let root = identity(40, 40);
    backend.add_tree(root, vec![observed(40, 40, None, 0)]);
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
    let first = state.kill_group(id, ResourceKillReason::User).unwrap();
    let second = state.kill_group(id, ResourceKillReason::Watchdog).unwrap();
    assert_eq!(first.state, ResourceGroupState::Terminated);
    assert_eq!(second.state, ResourceGroupState::Terminated);
    assert_eq!(backend.terminated().len(), 1);
}

#[test]
fn disabled_monitor_does_not_reserve_new_slot() {
    let (state, _backend) = state_with_fake();
    let mut disabled = limits(1);
    disabled.monitor_enabled = false;
    assert!(state.try_reserve_agent_slot(disabled).unwrap().is_none());
    assert_eq!(state.active_agent_groups(), 0);
}

#[test]
fn dynamic_cap_shrink_rejects_new_launches() {
    let (state, _backend) = state_with_fake();
    let _a = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
    let _b = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
    assert!(state.try_reserve_agent_slot(limits(1)).is_err());
}
