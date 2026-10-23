use super::*;

fn activity_json(record: &crate::config::activity_log::ActivityRecord) -> serde_json::Value {
    serde_json::to_value(record).expect("an activity record serializes")
}

/// Everything the emission sites appended on this thread since the last call.
/// No sink is configured in a test process, so this observes the calls
/// without a file.
fn emitted() -> Vec<serde_json::Value> {
    crate::config::activity_log::capture::drain()
        .iter()
        .map(activity_json)
        .collect()
}

/// A live, working session: the first `create_session` takes the selection
/// and becomes `Active`, so the returned one stays `Running`.
async fn working_session(manager: &SessionManager, cwd: &str) -> Session {
    if manager.get_active().await.is_none() {
        manager
            .create_session(
                "sh".to_string(),
                Vec::new(),
                "C:\\selected".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create the selected session");
    }
    manager
        .create_session(
            "sh".to_string(),
            Vec::new(),
            cwd.to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create a working session")
}

#[tokio::test]
async fn mark_idle_on_a_working_session_yields_exactly_one_idle_record() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;
    let _ = emitted();

    manager.mark_idle(subject.id).await;

    let records = emitted();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["event"], "idle");
    assert_eq!(records[0]["reason"], "mark_idle");
    assert_eq!(records[0]["sessionId"], subject.id.to_string());
    assert_eq!(records[0]["cwd"], "C:\\subject");
    assert_eq!(records[0]["idleThresholdMs"], serde_json::json!(2_500));
}

#[tokio::test]
async fn mark_idle_on_an_already_idle_session_yields_no_record() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;
    manager.mark_idle(subject.id).await;
    let _ = emitted();

    manager.mark_idle(subject.id).await;

    assert!(
        emitted().is_empty(),
        "a call that changes nothing produces no record"
    );
}

#[tokio::test]
async fn mark_busy_on_an_idle_session_yields_exactly_one_busy_record() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;
    manager.mark_idle(subject.id).await;
    let _ = emitted();

    manager.mark_busy(subject.id).await;

    let records = emitted();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["event"], "busy");
    assert_eq!(records[0]["reason"], "mark_busy");
    assert_eq!(records[0]["sessionId"], subject.id.to_string());
    assert_eq!(
        records[0]["continuesBlock"],
        serde_json::json!(true),
        "the idle a moment ago is inside the block window"
    );
    assert!(records[0]["gapMs"].is_u64());
}

#[tokio::test]
async fn mark_busy_twice_yields_one_record() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;
    manager.mark_idle(subject.id).await;
    let _ = emitted();

    manager.mark_busy(subject.id).await;
    manager.mark_busy(subject.id).await;

    assert_eq!(emitted().len(), 1, "only the edge is recorded");
}

#[tokio::test]
async fn mark_busy_on_an_exited_session_yields_no_record() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;
    // Exiting leaves `waiting_for_input` untouched, so the session sits at
    // `(Exited, false)`: `mark_busy` changes nothing observable and
    // `is_working` stays false.
    manager.mark_exited(subject.id, 0).await;
    let stored = manager
        .get_session(subject.id)
        .await
        .expect("the session still exists");
    assert!(!stored.waiting_for_input);
    let _ = emitted();

    manager.mark_busy(subject.id).await;

    assert!(
        emitted().is_empty(),
        "a dead session must not open an interval"
    );
}

#[tokio::test]
async fn mark_idle_still_demotes_running_to_idle_when_already_waiting_for_input() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;
    // Waiting for input while still `Running`: the guard must not skip the
    // demotion just because there is no edge to record.
    manager.mark_idle(subject.id).await;
    manager.mark_busy(subject.id).await;
    manager
        .state
        .write()
        .await
        .sessions
        .get_mut(&subject.id)
        .expect("session")
        .waiting_for_input = true;
    let _ = emitted();

    manager.mark_idle(subject.id).await;

    let stored = manager.get_session(subject.id).await.expect("session");
    assert_eq!(stored.status, SessionStatus::Idle);
    assert!(stored.waiting_for_input);
    assert!(emitted().is_empty(), "no edge, so no record");
}

#[tokio::test]
async fn mark_idle_still_sets_waiting_for_input_on_every_call() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;
    for round in 0..3 {
        manager.mark_idle(subject.id).await;
        let stored = manager.get_session(subject.id).await.expect("session");
        assert!(
            stored.waiting_for_input,
            "round {round}: the mutation is unconditional"
        );
    }
}

#[tokio::test]
async fn mark_idle_on_a_pending_create_session_yields_no_record() {
    let manager = SessionManager::new();
    let (pending, _binding) = pending_fixture(&manager, false).await;
    let _ = emitted();

    manager.mark_idle(pending.id).await;
    manager.mark_busy(pending.id).await;

    assert!(
        emitted().is_empty(),
        "a session that does not exist publicly yet emits nothing"
    );
}

/// `commands/session.rs` hands the PTY spawn `resolve_launch_idle_tuning`,
/// whose `idle_threshold` is `idle_tuning_for(agent_kind)`'s (only the
/// #2124 burst subfields can differ), and the detector registers exactly
/// that. The record must therefore report the same threshold for every
/// kind, not a hardcoded constant.
#[tokio::test]
async fn idle_record_carries_the_same_threshold_the_detector_registered() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;

    for kind in [
        None,
        Some(CodingAgentKind::Claude),
        Some(CodingAgentKind::Codex),
        Some(CodingAgentKind::Antigravity),
        Some(CodingAgentKind::Pi),
    ] {
        let mut session = subject.clone();
        session.agent_kind = kind;
        let record = activity_json(&crate::config::activity_log::build_idle(
            session.id,
            &session,
            crate::config::activity_log::IdleReason::MarkIdle,
        ));
        let registered = crate::session::profile::idle_tuning_for(kind)
            .idle_threshold
            .as_millis() as u64;
        assert_eq!(
            record["idleThresholdMs"],
            serde_json::json!(registered),
            "kind={kind:?}"
        );
    }

    // And through the real mutation site, for the kind the fixture carries.
    let _ = emitted();
    manager.mark_idle(subject.id).await;
    let records = emitted();
    let registered = crate::session::profile::idle_tuning_for(subject.agent_kind)
        .idle_threshold
        .as_millis() as u64;
    assert_eq!(records[0]["idleThresholdMs"], serde_json::json!(registered));
}

/// The defect the birth edge exists to remove: without it the first
/// `mark_idle` after finalization is an orphan `idle` with no opening `busy`.
#[tokio::test]
async fn the_first_idle_after_finalization_has_a_matching_busy() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (pending, binding) = pending_fixture(&manager, false).await;
    let live = live_witness(pending.id);
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(binding, live);
    let result = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::UserSwitch,
            mutations,
        )
        .await
        .expect("finalize live");
    let opening: Vec<serde_json::Value> = result.activity.iter().map(activity_json).collect();
    let _ = emitted();

    manager.mark_idle(pending.id).await;
    let closing = emitted();

    assert_eq!(opening.len(), 1);
    assert_eq!(opening[0]["event"], "busy");
    assert_eq!(closing.len(), 1);
    assert_eq!(closing[0]["event"], "idle");
    assert_eq!(
        opening[0]["sessionId"], closing[0]["sessionId"],
        "the closing edge must pair with the opening one"
    );
}

#[tokio::test]
async fn try_snapshot_working_sessions_excludes_pending_create() {
    let manager = SessionManager::new();
    let live = working_session(&manager, "C:\\live").await;
    let (pending, _binding) = pending_fixture(&manager, false).await;

    let rows = manager
        .try_snapshot_working_sessions()
        .expect("no writer holds state");

    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    assert!(ids.contains(&live.id));
    assert!(
        !ids.contains(&pending.id),
        "a session that does not exist publicly yet must not get a synthetic close"
    );
}

#[tokio::test]
async fn try_snapshot_working_sessions_returns_none_rather_than_blocking_while_a_writer_holds_state(
) {
    let manager = SessionManager::new();
    let _live = working_session(&manager, "C:\\live").await;

    let writer = manager.state.write().await;
    assert!(
        manager.try_snapshot_working_sessions().is_none(),
        "the shutdown path must never block on the session map"
    );
    drop(writer);

    assert!(
        manager.try_snapshot_working_sessions().is_some(),
        "the snapshot succeeds again once the writer is gone"
    );
}

#[tokio::test]
async fn try_snapshot_working_sessions_uses_the_same_predicate_as_is_working() {
    let manager = SessionManager::new();
    let subject = working_session(&manager, "C:\\subject").await;

    // Every state the two production writers and the exit path can leave a
    // session in, checked against the shared predicate rather than a second
    // definition over `SessionInfo`.
    for step in ["born", "idle", "busy", "exited"] {
        match step {
            "born" => {}
            "idle" => manager.mark_idle(subject.id).await,
            "busy" => manager.mark_busy(subject.id).await,
            _ => {
                manager.mark_exited(subject.id, 0).await;
            }
        }
        let stored = manager.get_session(subject.id).await.expect("session");
        let listed = manager
            .try_snapshot_working_sessions()
            .expect("no writer holds state")
            .iter()
            .any(|row| row.id == subject.id);
        assert_eq!(
            listed,
            crate::session::session::is_working(&stored),
            "step {step}: status={:?} waiting_for_input={}",
            stored.status,
            stored.waiting_for_input
        );
    }
}

#[tokio::test]
async fn finalize_live_yields_exactly_one_session_start_busy_record() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (pending, binding) = pending_fixture(&manager, false).await;
    let live = live_witness(pending.id);
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(binding, live);

    let result = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::UserSwitch,
            mutations,
        )
        .await
        .expect("finalize live");

    assert_eq!(
        result.activity.len(),
        1,
        "session birth is one opening edge, no more and no less"
    );
    let record = activity_json(&result.activity[0]);
    assert_eq!(record["event"], "busy");
    assert_eq!(record["reason"], "session_start");
    assert_eq!(record["sessionId"], pending.id.to_string());
    assert_eq!(record["cwd"], "C:/work");
    assert_eq!(record["continuesBlock"], serde_json::json!(false));
    assert!(record.get("gapMs").is_none());
}

#[tokio::test]
async fn finalize_dormant_yields_no_activity_record() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (pending, binding) = pending_fixture(&manager, false).await;
    let dormant = dormant_witness(pending.id);
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_dormant(binding, dormant, 0);

    let result = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Dormant(dormant),
            SelectionCause::UserSwitch,
            mutations,
        )
        .await
        .expect("finalize dormant");

    assert!(
        result.activity.is_empty(),
        "a dormant finalization never opens an interval"
    );
}

// The conflict validation rejects remove+finalize outright, so the deferred
// re-read is defense in depth rather than the active mechanism here. Either
// way no record reaches the caller, which is the required behavior.
#[tokio::test]
async fn a_session_finalized_and_removed_in_the_same_commit_yields_no_activity_record() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (pending, binding) = pending_fixture(&manager, false).await;
    let live = live_witness(pending.id);
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(binding, live);
    mutations.remove(pending.id);

    let error = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::UserSwitch,
            mutations,
        )
        .await
        .expect_err("remove+finalize is rejected");

    assert!(
        error.contains("remove+finalize conflict"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn a_session_finalized_and_marked_exited_in_the_same_commit_yields_no_activity_record() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (pending, binding) = pending_fixture(&manager, false).await;
    let live = live_witness(pending.id);
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(binding, live);
    mutations.mark_exited(pending.id, 0);

    let error = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::UserSwitch,
            mutations,
        )
        .await
        .expect_err("markExited+finalize is rejected");

    assert!(
        error.contains("markExited+finalize conflict"),
        "unexpected error: {error}"
    );
}

/// The records are built after every fallible exit, so an `Err` returns
/// before any `CommitResult` reaches the caller and nothing can be appended
/// for a session this commit did not finalize.
#[tokio::test]
async fn a_failed_commit_yields_no_activity_records() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (session, binding) = pending_fixture(&manager, false).await;
    manager.state.write().await.revision = u64::MAX;
    let live = live_witness(session.id);
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(binding, live);

    let outcome = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::UserSwitch,
            mutations,
        )
        .await;

    assert!(
        outcome.is_err(),
        "no CommitResult, therefore no activity records, may reach the caller"
    );
}

// ──────────────────────────────────────────────────────────────────────
// §1295 — remove_exited_sessions
// ──────────────────────────────────────────────────────────────────────

/// Test 9: of a mixed set (Running, Exited, pending) only the Exited
/// non-pending row is removed; the selection is repaired to the next
/// non-pending row with `SelectionCause::BackgroundCleanup`; count is
/// correct.
#[tokio::test]
async fn remove_exited_sessions_removes_only_exited_non_pending() {
    let mgr = SessionManager::new();
    let a = mgr
        .create_session(
            "running".into(),
            vec![],
            "C:/x/a".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create running");
    let b = mgr
        .create_session(
            "exited".into(),
            vec![],
            "C:/x/b".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create exited");
    let c_pending = mgr.insert_pending_session_for_test("C:/x/c".into()).await;
    mgr.mark_exited(b.id, 0).await;
    // Make the soon-to-remove Exited row the current selection.
    mgr.set_active_only(b.id).await.expect("select b");
    assert_eq!(mgr.selection_payload().await.id(), Some(b.id));

    let removed = mgr.remove_exited_sessions(&[a.id, b.id, c_pending]).await;
    assert_eq!(removed, 1, "only the Exited non-pending row is removed");

    // b gone; a (Running) and c (pending) remain.
    assert_eq!(mgr.get_session(b.id).await.map(|s| s.name), None);
    assert!(mgr.get_session(a.id).await.is_some());
    // Pending rows are hidden from the public read path; assert the pending
    // create is still present instead.
    assert!(mgr.contains_public_or_pending(c_pending).await);

    // Selection repaired to the next non-pending row (a) with BackgroundCleanup.
    let selection = mgr.selection_payload().await;
    assert_eq!(selection.id(), Some(a.id));
    assert_eq!(
        selection.source(),
        crate::session::selection::SelectionSource::BackgroundCleanup
    );
}

/// Test 10: rows that are live (Running) or pending are never removed.
#[tokio::test]
async fn remove_exited_sessions_never_removes_live_or_pending() {
    let mgr = SessionManager::new();
    let a = mgr
        .create_session(
            "running-a".into(),
            vec![],
            "C:/x/a".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create a");
    let b = mgr
        .create_session(
            "running-b".into(),
            vec![],
            "C:/x/b".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create b");
    let c_pending = mgr.insert_pending_session_for_test("C:/x/c".into()).await;

    let removed = mgr.remove_exited_sessions(&[a.id, b.id, c_pending]).await;
    assert_eq!(removed, 0, "no live/pending row is ever removed");
    assert!(mgr.get_session(a.id).await.is_some());
    assert!(mgr.get_session(b.id).await.is_some());
    // Pending rows are hidden from the public read path (by design), so assert
    // the pending create is still present rather than via get_session.
    assert!(mgr.contains_public_or_pending(c_pending).await);
}

#[tokio::test]
async fn an_armed_session_yields_the_same_target_on_every_edge() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "codex".into(),
            vec![],
            "C:/x/armed".into(),
            Some("codex".to_string()),
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create the armed session");

    // Control, and the whole assertion for the unarmed gate.
    assert_eq!(mgr.agent_stamp_target(session.id).await, None);

    mgr.arm_agent_turn(session.id).await;
    let expected = Some((session.working_directory.clone(), "codex".to_string()));
    // The latch is never consumed, so every later busy->idle edge sees the
    // same target rather than only the first.
    assert_eq!(mgr.agent_stamp_target(session.id).await, expected);
    assert_eq!(mgr.agent_stamp_target(session.id).await, expected);
    assert_eq!(mgr.agent_stamp_target(session.id).await, expected);
}

#[tokio::test]
async fn a_session_without_an_agent_id_never_yields_a_target() {
    let mgr = SessionManager::new();
    let plain = mgr
        .create_session(
            "pwsh".into(),
            vec![],
            "C:/x/plain".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create the agentless session");
    mgr.arm_agent_turn(plain.id).await;
    assert_eq!(mgr.agent_stamp_target(plain.id).await, None);
    assert_eq!(mgr.agent_stamp_target(plain.id).await, None);

    // In-test control: without it this passes against an
    // `agent_stamp_target` that always returns `None`.
    let agent = mgr
        .create_session(
            "codex".into(),
            vec![],
            "C:/x/agent".into(),
            Some("codex".to_string()),
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create the agent session");
    mgr.arm_agent_turn(agent.id).await;
    assert_eq!(
        mgr.agent_stamp_target(agent.id).await,
        Some((agent.working_directory.clone(), "codex".to_string()))
    );
}
