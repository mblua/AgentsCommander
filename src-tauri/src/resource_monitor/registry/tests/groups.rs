use super::*;

// #1151 (R9) - group_counts_toward_admission is the admission predicate itself, not
// an inference, so the watchdog can report the fact without claiming authorship.
// Compared against the active_count FILTER (the delta this group contributes), not
// against active_agent_groups() directly, which also counts pending permits.
#[test]
fn group_counts_toward_admission_matches_the_active_count_filter() {
    let (state, backend) = state_with_fake();
    let absent = Uuid::new_v4();
    assert_eq!(state.active_agent_groups(), 0);
    assert!(!state.group_counts_toward_admission(absent));

    let root = identity(96, 96);
    let child = identity(97, 97);
    backend.add_tree(
        root,
        vec![observed(96, 96, None, 0), observed(97, 97, Some(96), 1)],
    );
    backend.mark_stubborn(child);
    let permit = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
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

    // Running: included by the filter, so the figure rises by exactly this group.
    assert!(state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 1);

    // Quarantined: still included. This is the whole reason the permit leaks.
    state
        .kill_group(id, ResourceKillReason::SessionDestroy)
        .unwrap();
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
    assert!(state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 1);

    // Terminated with the permit released: excluded, back to the empty figure.
    backend.mark_gone(child);
    state.retry_orphaned_quarantine(id, root).unwrap();
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Terminated));
    assert!(!state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 0);

    // Absent: never included.
    assert!(!state.group_counts_toward_admission(absent));
}

// #1151 (R10) - the restart shape at registry level. A restart replacement carries the
// same workgroup/agent/project under a NEW uuid, so register_group's relaunch dedup
// WOULD have collected the old row and does not, because it is Quarantined rather
// than Terminated. That is the accumulator; the retry is what drains it.
#[test]
fn restart_shaped_orphan_retry_unblocks_relaunch_dedup() {
    let (state, backend) = state_with_fake();
    let triple = ("wg-1", "dev-rust", "ac");
    let wg = || Some(triple.0.to_string());
    let agent = || Some(triple.1.to_string());
    let project = || Some(triple.2.to_string());

    let id_a = Uuid::new_v4();
    let (root_a, child_a) = quarantined_group(&state, &backend, id_a, 100, 2, Some(triple));

    // The restart replacement: a NEW uuid with the SAME identity triple.
    let id_b = Uuid::new_v4();
    let root_b = identity(110, 110);
    backend.add_tree(root_b, vec![observed(110, 110, None, 0)]);
    let permit_b = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
    state
        .register_group(
            permit_b,
            id_b,
            "agent".into(),
            None,
            None,
            wg(),
            agent(),
            project(),
            root_b,
        )
        .unwrap();

    // A's row survives the dedup because it is Quarantined, and both rows count.
    assert_eq!(
        state.test_state(id_a),
        Some(ResourceGroupState::Quarantined)
    );
    assert_eq!(state.active_agent_groups(), 2);

    // The blocker clears and the orphan retry drains A.
    backend.mark_gone(child_a);
    assert!(matches!(
        state.retry_orphaned_quarantine(id_a, root_a).unwrap(),
        QuarantineRetryOutcome::Completed(_)
    ));
    assert_eq!(state.test_state(id_a), Some(ResourceGroupState::Terminated));
    assert_eq!(state.active_agent_groups(), 1);

    // A third launch of the same triple now reclaims A's row instead of stacking.
    let id_c = Uuid::new_v4();
    let root_c = identity(120, 120);
    backend.add_tree(root_c, vec![observed(120, 120, None, 0)]);
    let permit_c = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
    state
        .register_group(
            permit_c,
            id_c,
            "agent".into(),
            None,
            None,
            wg(),
            agent(),
            project(),
            root_c,
        )
        .unwrap();
    assert_eq!(state.test_state(id_a), None);
    assert_eq!(state.active_agent_groups(), 2);
}

// (c) H3 - registering the same wg/role drops the prior Terminated duplicate row.
#[test]
fn register_dedups_prior_terminated_row_for_same_identity() {
    let (state, backend) = state_with_fake();
    let root_a = identity(70, 70);
    backend.add_tree(root_a, vec![observed(70, 70, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
    let id_a = Uuid::new_v4();
    state
        .register_group(
            permit,
            id_a,
            "agent".into(),
            None,
            None,
            Some("wg-1".into()),
            Some("tech-lead".into()),
            None,
            root_a,
        )
        .unwrap();
    let killed = state.kill_group(id_a, ResourceKillReason::User).unwrap();
    assert_eq!(killed.state, ResourceGroupState::Terminated);

    // Relaunch the same wg/role with a fresh pid: the prior Terminated row is dropped.
    let root_b = identity(71, 71);
    backend.add_tree(root_b, vec![observed(71, 71, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
    let id_b = Uuid::new_v4();
    state
        .register_group(
            permit,
            id_b,
            "agent".into(),
            None,
            None,
            Some("wg-1".into()),
            Some("tech-lead".into()),
            None,
            root_b,
        )
        .unwrap();

    let snap = state.snapshot(limits(3));
    assert_eq!(snap.groups.len(), 1);
    assert_eq!(snap.groups[0].session_id, id_b.to_string());
    assert_eq!(snap.groups[0].state, ResourceGroupState::Running);
}

// (c2) #566 - two different projects running the same wg/role must NOT dedup
// against each other's Terminated row (the dedup predicate gained `project`).
// Regression guard: reverting the `g.project == project` clause makes this
// assert len == 1 instead of 2.
#[test]
fn cross_project_same_role_terminated_rows_are_not_deduped() {
    let (state, backend) = state_with_fake();

    // Project A: register the (wg-1, tech-lead) group, then kill it so its
    // row is Terminated.
    let root_a = identity(80, 80);
    backend.add_tree(root_a, vec![observed(80, 80, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
    let id_a = Uuid::new_v4();
    state
        .register_group(
            permit,
            id_a,
            "agent".into(),
            None,
            None,
            Some("wg-1".into()),
            Some("tech-lead".into()),
            Some("proj-a".into()),
            root_a,
        )
        .unwrap();
    let killed = state.kill_group(id_a, ResourceKillReason::User).unwrap();
    assert_eq!(killed.state, ResourceGroupState::Terminated);

    // Project B: same wg/role, different project. The predicate's
    // `g.project == project` clause means A's Terminated row is NOT dropped.
    let root_b = identity(81, 81);
    backend.add_tree(root_b, vec![observed(81, 81, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
    let id_b = Uuid::new_v4();
    state
        .register_group(
            permit,
            id_b,
            "agent".into(),
            None,
            None,
            Some("wg-1".into()),
            Some("tech-lead".into()),
            Some("proj-b".into()),
            root_b,
        )
        .unwrap();

    // Both rows survive: A (Terminated, proj-a) + B (Running, proj-b).
    let snap = state.snapshot(limits(3));
    assert_eq!(snap.groups.len(), 2);
}

// (T2) H1 - the reaping sample stamps the friendly message and suppresses the raw
// warning, while the non-reaping strike still surfaces it.
#[test]
fn reaping_sample_sets_friendly_message_and_suppresses_warning() {
    let (state, backend) = state_with_fake();
    let root = identity(72, 72);
    backend.add_tree(root, vec![observed(72, 72, None, 0)]);
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

    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    backend.mark_gone(root);

    // First strike (non-reaping): the raw warning IS surfaced.
    let first = state.snapshot(limits(1));
    assert!(first
        .warnings
        .iter()
        .any(|w| w.contains("was not in process snapshot")));

    // Second strike reaps: friendly last_error, raw warning suppressed.
    let reaping = state.snapshot(limits(1));
    assert!(reaping
        .warnings
        .iter()
        .all(|w| !w.contains("was not in process snapshot")));
    let row = reaping
        .groups
        .iter()
        .find(|g| g.session_id == id.to_string())
        .expect("reaped group present in snapshot");
    assert_eq!(row.state, ResourceGroupState::Terminated);
    assert_eq!(
        row.last_error.as_deref(),
        Some("agent process exited; resource slot reclaimed")
    );
}

// (T3) H1 never races H2: a Quarantined group IS sampled (#647 C) but merge_sample
// only strike-counts/nominates Running groups, so it is still never reaped.
#[test]
fn reap_never_touches_quarantined_group() {
    let (state, backend) = state_with_fake();
    let root = identity(73, 73);
    let child = identity(74, 74);
    backend.add_tree(
        root,
        vec![observed(73, 73, None, 0), observed(74, 74, Some(73), 1)],
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

    // Even with a missing-root signal + confirmed-gone root, merge_sample only
    // strike-counts/nominates Running groups, so H1 never reaps Quarantined to
    // Terminated (even though #647 C now re-samples it).
    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    backend.mark_gone(root);
    state.snapshot(limits(1));
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
    assert_eq!(state.active_agent_groups(), 1);
}

// #647 C: a Quarantined group IS included in sample_running_groups so its process
// list is re-observed each snapshot (unfreezing the display). Pairs with
// reap_never_touches_quarantined_group, which proves sampling it stays reap-safe.
#[test]
fn quarantined_group_is_resampled() {
    let (state, backend) = state_with_fake();
    let root = identity(81, 81);
    let child = identity(82, 82);
    backend.add_tree(
        root,
        vec![observed(81, 81, None, 0), observed(82, 82, Some(81), 1)],
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

    let sampled = state.sample_running_groups();
    assert!(
        sampled.iter().any(|(sid, _)| *sid == id),
        "Quarantined group must be re-sampled (#647 C)"
    );
}

// #647 D / E1: the ACCESS_DENIED signature must match the EXACT win32 code 5, so
// unrelated codes that merely start with the digit 5 (50/53/567) never trip it.
#[test]
fn access_denied_signature_matches_exact_code_5_only() {
    assert!(has_access_denied_signature(&[
        "pid 1: TerminateProcess failed: win32 error 5".to_string()
    ]));
    assert!(!has_access_denied_signature(&[
        "pid 1: OpenProcess(1) failed: win32 error 50".to_string()
    ]));
    assert!(!has_access_denied_signature(&[
        "pid 1: win32 error 53".to_string()
    ]));
    assert!(!has_access_denied_signature(&[
        "pid 1: win32 error 567".to_string()
    ]));
    assert!(!has_access_denied_signature(&[
        "pid 1: timed out waiting for pid 1 to exit".to_string()
    ]));
    // `any` policy (E2): a mixed set with one error-5 still flags. HIGH-2 keeps the
    // per-PID detail in `message`, so this only ADDS the AV hint, never hides a failure.
    assert!(has_access_denied_signature(&[
        "pid 1: win32 error 5".to_string(),
        "pid 2: timed out".to_string(),
    ]));
}

// #647 D: blocked_by_security serializes as camelCase and, being serde(default),
// a legacy payload without the field deserializes to false.
#[test]
fn kill_result_blocked_by_security_round_trips_and_defaults() {
    let result = ResourceKillResult {
        session_id: "s".to_string(),
        state: ResourceGroupState::Quarantined,
        killed_processes: Vec::new(),
        quarantined: true,
        message: "pid 1: win32 error 5".to_string(),
        blocked_by_security: false,
        finalized: false,
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"blockedBySecurity\":false"));
    assert!(json.contains("\"finalized\":false"));

    // Strip the field to simulate a pre-#647 payload; serde(default) -> false.
    let legacy = json.replace(",\"blockedBySecurity\":false", "");
    assert!(!legacy.contains("blockedBySecurity"));
    let parsed: ResourceKillResult = serde_json::from_str(&legacy).unwrap();
    assert!(!parsed.blocked_by_security);
}

// (T4) H3 - retained Terminated rows are bounded by MAX_TERMINATED_RETAINED.
#[test]
fn terminated_rows_are_bounded_by_retention_cap() {
    let (state, backend) = state_with_fake();
    let total = MAX_TERMINATED_RETAINED + 4;
    for i in 0..total {
        let pid = 100 + i as u32;
        let root = identity(pid, pid as u64);
        backend.add_tree(root, vec![observed(pid, pid as u64, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(100)).unwrap().unwrap();
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
        let killed = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert_eq!(killed.state, ResourceGroupState::Terminated);
    }

    let snap = state.snapshot(limits(100));
    assert_eq!(snap.groups.len(), MAX_TERMINATED_RETAINED);
    assert_eq!(state.active_agent_groups(), 0);
}

// #632 B2a - the app-shutdown bulk path (kill_all_owned_groups -> fresh_targets_only)
// targets only the fresh live tree, NOT the accumulated observed_processes. A child
// that left the live tree but is still alive must therefore NOT be killed here.
#[test]
fn kill_all_owned_groups_uses_fresh_tree_only() {
    let (state, backend) = state_with_fake();
    let root = identity(90, 90);
    let child = identity(91, 91);
    backend.add_tree(
        root,
        vec![observed(90, 90, None, 0), observed(91, 91, Some(90), 1)],
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

    // register seeded observed_processes with {root, child}. The child now leaves
    // the live tree (fresh observe_tree is root-only) but stays alive.
    backend.replace_tree(root, vec![observed(90, 90, None, 0)], Vec::new());
    backend.mark_present(child);

    let results = state.kill_all_owned_groups(ResourceKillReason::AppShutdown);
    assert_eq!(results.len(), 1);
    assert!(!results[0].quarantined);
    assert!(backend.terminated().contains(&root));
    assert!(
        !backend.terminated().contains(&child),
        "fresh-only shutdown kill must not target the accumulated child"
    );
}

// #632 B2a contrast - the per-session union path (kill_group, fresh_targets_only =
// false) DOES target the accumulated child. Same setup as the fresh-only test;
// proves the flag is the only difference.
#[test]
fn kill_group_union_still_targets_accumulated_child() {
    let (state, backend) = state_with_fake();
    let root = identity(92, 92);
    let child = identity(93, 93);
    backend.add_tree(
        root,
        vec![observed(92, 92, None, 0), observed(93, 93, Some(92), 1)],
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

    backend.replace_tree(root, vec![observed(92, 92, None, 0)], Vec::new());
    backend.mark_present(child);

    let result = state
        .kill_group(id, ResourceKillReason::SessionDestroy)
        .unwrap();
    assert!(!result.quarantined);
    assert!(backend.terminated().contains(&root));
    assert!(
        backend.terminated().contains(&child),
        "union kill must still target the accumulated child"
    );
}

// #632 B2b - a CLEAN sample (root alive, no errors) prunes a descendant that has
// exited, so observed_processes tracks the live set instead of accumulating.
#[test]
fn clean_sample_prunes_exited_descendant() {
    let (state, backend) = state_with_fake();
    let root = identity(94, 94);
    let child = identity(95, 95);
    backend.add_tree(
        root,
        vec![observed(94, 94, None, 0), observed(95, 95, Some(94), 1)],
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
    let snap = state.snapshot(limits(1));
    assert_eq!(snap.groups[0].process_count, 2);

    // Child exits; a clean root-only sample prunes it.
    backend.replace_tree(root, vec![observed(94, 94, None, 0)], Vec::new());
    backend.mark_gone(child);
    let snap = state.snapshot(limits(1));
    assert_eq!(snap.groups[0].process_count, 1);
}

// #632 B2b guard - a missing-root sample (empty tree + missing-root error) must NOT
// prune; the last-known set is owned by the #559 strike/reap path.
#[test]
fn missing_root_sample_retains_last_known_set() {
    let (state, backend) = state_with_fake();
    let root = identity(96, 96);
    backend.add_tree(
        root,
        vec![observed(96, 96, None, 0), observed(97, 97, Some(96), 1)],
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
    let snap = state.snapshot(limits(1));
    assert_eq!(snap.groups[0].process_count, 2);

    backend.replace_tree(
        root,
        Vec::new(),
        vec![format!("root pid {} was not in process snapshot", root.pid)],
    );
    backend.mark_gone(root);
    let snap = state.snapshot(limits(1));
    assert_eq!(
        snap.groups[0].process_count, 2,
        "missing-root sample must retain the last-known set"
    );
}

// #632 B2b guard (MED-3) - a root-alive but ERRORED sample (e.g. a transient
// "identity unavailable" for a child) must NOT prune, so a racy/partial snapshot
// can never drop a still-live descendant.
#[test]
fn errored_sample_does_not_prune() {
    let (state, backend) = state_with_fake();
    let root = identity(98, 98);
    let child = identity(99, 99);
    backend.add_tree(
        root,
        vec![observed(98, 98, None, 0), observed(99, 99, Some(98), 1)],
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
    let snap = state.snapshot(limits(1));
    assert_eq!(snap.groups[0].process_count, 2);

    // Root-only fresh tree, but the sample carries an enumeration error: root_alive
    // is true yet sample_clean is false, so MED-3 keeps the live child.
    backend.replace_tree(
        root,
        vec![observed(98, 98, None, 0)],
        vec![format!("identity unavailable for pid {}", child.pid)],
    );
    backend.mark_present(child);
    let snap = state.snapshot(limits(1));
    assert_eq!(
        snap.groups[0].process_count, 2,
        "errored sample must not prune the live child"
    );
}

// #2581 - per-agent memory warnings are structured (session id + level +
// bytes + limit) and never duplicated as free text in `warnings`.
fn snapshot_with_group_at(bytes: u32) -> (Uuid, ResourceSnapshot) {
    let (state, backend) = state_with_fake();
    let root = identity(bytes, bytes as u64);
    backend.add_tree(root, vec![observed(bytes, bytes as u64, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "Claude Code".into(),
            None,
            None,
            Some("room-1-team".into()),
            Some("dev-rust".into()),
            None,
            root,
        )
        .unwrap();
    (id, state.snapshot(limits(3)))
}

#[test]
fn group_over_warn_limit_emits_structured_group_warning() {
    let (id, snap) = snapshot_with_group_at(150);
    assert_eq!(snap.group_warnings.len(), 1);
    let warning = &snap.group_warnings[0];
    assert_eq!(warning.session_id, id.to_string());
    assert_eq!(warning.level, ResourceGroupWarningLevel::Warn);
    assert_eq!(warning.private_bytes, 150);
    assert_eq!(warning.limit_bytes, 100);
    assert!(snap.warnings.iter().all(|w| !w.contains("exceeds group")));
    assert_eq!(snap.overall_state, ResourceOverallState::Warn);
}

#[test]
fn group_over_kill_limit_emits_kill_group_warning() {
    let (id, snap) = snapshot_with_group_at(250);
    assert_eq!(snap.group_warnings.len(), 1);
    let warning = &snap.group_warnings[0];
    assert_eq!(warning.session_id, id.to_string());
    assert_eq!(warning.level, ResourceGroupWarningLevel::Kill);
    assert_eq!(warning.private_bytes, 250);
    assert_eq!(warning.limit_bytes, 200);
    assert!(snap.warnings.iter().all(|w| !w.contains("exceeds group")));
    assert_eq!(snap.overall_state, ResourceOverallState::Critical);
}

#[test]
fn group_under_warn_limit_emits_no_group_warning() {
    let (_id, snap) = snapshot_with_group_at(50);
    assert!(snap.group_warnings.is_empty());
    assert_eq!(snap.overall_state, ResourceOverallState::Ok);
}

#[test]
fn disabled_snapshot_has_no_group_warnings() {
    assert!(disabled_snapshot(limits(1)).group_warnings.is_empty());
}

#[test]
fn group_warnings_serialize_in_camel_case() {
    let (id, snap) = snapshot_with_group_at(150);
    let value = serde_json::to_value(&snap).unwrap();
    assert_eq!(
        value["groupWarnings"],
        serde_json::json!([{
            "sessionId": id.to_string(),
            "level": "warn",
            "privateBytes": 150,
            "limitBytes": 100,
        }])
    );
}

#[test]
fn snapshot_without_group_warnings_key_deserializes_to_empty() {
    let mut value = serde_json::to_value(disabled_snapshot(limits(1))).unwrap();
    value.as_object_mut().unwrap().remove("groupWarnings");
    let snap: ResourceSnapshot = serde_json::from_value(value).unwrap();
    assert!(snap.group_warnings.is_empty());
}
