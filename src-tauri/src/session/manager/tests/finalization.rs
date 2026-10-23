use super::*;

#[tokio::test]
async fn pending_create_is_invisible_until_atomic_live_finalization() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (pending, binding) = pending_fixture(&manager, false).await;

    assert!(manager.list_sessions().await.is_empty());
    assert!(manager.get_session(pending.id).await.is_none());
    let before = manager.aggregate_snapshot().await;
    assert!(before.sessions.is_empty());
    assert!(before.pending_ids.contains(&pending.id));
    assert!(before.order.contains(&pending.id));
    assert_eq!(before.selection.revision(), 0);

    let live = live_witness(pending.id);
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(binding, live);
    let committed = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::SessionCreated(TrustedCreateIntent::User),
            mutations,
        )
        .await
        .expect("finalize pending create");
    assert_eq!(committed.finalized_rows.len(), 1);
    let visible = manager
        .get_session(pending.id)
        .await
        .expect("finalized row visible");
    assert_eq!(visible.status, SessionStatus::Active);
    let selection = manager.selection_payload().await;
    assert_eq!(selection.id(), Some(pending.id));
    assert_eq!(selection.revision(), 1);

    let mut duplicate = LifecycleMutations::default();
    duplicate.finalize_live(binding, live);
    assert!(manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::SessionCreated(TrustedCreateIntent::User),
            duplicate,
        )
        .await
        .is_err());
    assert_eq!(manager.selection_payload().await.revision(), 1);
}

#[tokio::test]
async fn invalid_finalization_matrix_rejects_without_partial_mutation() {
    let capability = CommitCapability::for_test();

    for (has_pty, detached) in [(false, false), (true, true)] {
        let manager = SessionManager::new();
        let (pending, binding) = pending_fixture(&manager, false).await;
        let witness = LiveRuntimeWitness {
            session_id: pending.id,
            has_pty,
            detached,
        };
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, witness);
        assert!(manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Keep,
                SelectionCause::Restore,
                mutations,
            )
            .await
            .is_err());
        assert!(manager.get_pending_session(binding).await.is_some());
        assert_eq!(manager.selection_payload().await.revision(), 0);
    }

    let manager = SessionManager::new();
    let (pending, binding) = pending_fixture(&manager, false).await;
    let wrong_binding = PendingCreateBinding::new(pending.id, Uuid::new_v4());
    let mut wrong = LifecycleMutations::default();
    wrong.finalize_live(wrong_binding, live_witness(pending.id));
    assert!(manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Keep,
            SelectionCause::Restore,
            wrong,
        )
        .await
        .is_err());
    assert!(manager.get_pending_session(binding).await.is_some());

    for conflict in ["remove", "exit"] {
        let manager = SessionManager::new();
        let (pending, binding) = pending_fixture(&manager, false).await;
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(binding, live_witness(pending.id));
        if conflict == "remove" {
            mutations.remove(pending.id);
        } else {
            mutations.mark_exited(pending.id, 9);
        }
        assert!(manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Keep,
                SelectionCause::Restore,
                mutations,
            )
            .await
            .is_err());
        assert!(manager.get_pending_session(binding).await.is_some());
    }

    let manager = SessionManager::new();
    let (pending, binding) = pending_fixture(&manager, false).await;
    manager
        .rollback_pending_create(binding)
        .await
        .expect("remove pending fixture");
    let mut missing = LifecycleMutations::default();
    missing.finalize_live(binding, live_witness(pending.id));
    assert!(manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Keep,
            SelectionCause::Restore,
            missing,
        )
        .await
        .is_err());
    assert!(manager.aggregate_snapshot().await.sessions.is_empty());
}

#[tokio::test]
async fn dormant_finalization_and_live_transition_invariants_are_atomic() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (dormant, dormant_binding) = pending_fixture(&manager, false).await;
    let dormant_runtime = dormant_witness(dormant.id);
    let mut dormant_mutations = LifecycleMutations::default();
    dormant_mutations.finalize_dormant(dormant_binding, dormant_runtime, 37);
    manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Dormant(dormant_runtime),
            SelectionCause::Restore,
            dormant_mutations,
        )
        .await
        .expect("finalize dormant");
    assert_eq!(
        manager.get_session(dormant.id).await.unwrap().status,
        SessionStatus::Exited(37)
    );
    assert_eq!(
        manager.selection_payload().await.status(),
        Some(&SessionStatus::Exited(37))
    );

    let (first, first_binding) = pending_fixture(&manager, false).await;
    let first_live = live_witness(first.id);
    let mut first_finalize = LifecycleMutations::default();
    first_finalize.finalize_live(first_binding, first_live);
    manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(first_live),
            SelectionCause::UserSwitch,
            first_finalize,
        )
        .await
        .expect("select first live");

    let (second, second_binding) = pending_fixture(&manager, false).await;
    let second_live = live_witness(second.id);
    let mut second_finalize = LifecycleMutations::default();
    second_finalize.finalize_live(second_binding, second_live);
    manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(second_live),
            SelectionCause::UserSwitch,
            second_finalize,
        )
        .await
        .expect("select second live");
    assert_eq!(
        manager.get_session(first.id).await.unwrap().status,
        SessionStatus::Running
    );
    assert_eq!(
        manager.get_session(second.id).await.unwrap().status,
        SessionStatus::Active
    );

    let revision = manager.selection_payload().await.revision();
    for invalid in [
        LiveRuntimeWitness {
            session_id: Uuid::new_v4(),
            has_pty: true,
            detached: false,
        },
        LiveRuntimeWitness {
            session_id: first.id,
            has_pty: false,
            detached: false,
        },
        LiveRuntimeWitness {
            session_id: first.id,
            has_pty: true,
            detached: true,
        },
    ] {
        assert!(manager
            .commit_selection_transition(
                &capability,
                CommitDecision::Live(invalid),
                SelectionCause::UserSwitch,
                LifecycleMutations::default(),
            )
            .await
            .is_err());
    }
    assert_eq!(manager.selection_payload().await.revision(), revision);
}

#[tokio::test]
async fn mark_exited_preserves_first_code_and_updates_selection_and_raise_hand_once() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (session, binding) = pending_fixture(&manager, true).await;
    let live = live_witness(session.id);
    let mut finalize = LifecycleMutations::default();
    finalize.finalize_live(binding, live);
    manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::SessionCreated(TrustedCreateIntent::User),
            finalize,
        )
        .await
        .unwrap();
    assert!(manager
        .raise_hand(session.id, chrono::Utc::now())
        .await
        .is_some());

    let dormant = dormant_witness(session.id);
    let mut exit = LifecycleMutations::default();
    exit.mark_exited(session.id, 7);
    let first = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Dormant(dormant),
            SelectionCause::LivenessReconcile,
            exit,
        )
        .await
        .expect("first exit");
    assert_eq!(first.cleared_raise_hand_ids, vec![session.id]);
    assert_eq!(
        manager.get_session(session.id).await.unwrap().status,
        SessionStatus::Exited(7)
    );
    assert!(manager
        .get_session(session.id)
        .await
        .unwrap()
        .communication
        .is_none());
    let revision = manager.selection_payload().await.revision();

    let mut duplicate = LifecycleMutations::default();
    duplicate.mark_exited(session.id, 99);
    let second = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Dormant(dormant),
            SelectionCause::LivenessReconcile,
            duplicate,
        )
        .await
        .expect("duplicate exit is idempotent");
    assert!(second.selection.is_none());
    assert!(second.changed_rows.is_empty());
    assert_eq!(manager.selection_payload().await.revision(), revision);
    assert_eq!(
        manager.get_session(session.id).await.unwrap().status,
        SessionStatus::Exited(7)
    );

    let mut keep_selected = LifecycleMutations::default();
    keep_selected.mark_exited(session.id, 101);
    assert!(manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Keep,
            SelectionCause::LivenessReconcile,
            keep_selected,
        )
        .await
        .is_err());
}

#[tokio::test]
async fn detached_intent_is_atomic_and_preserves_geometry() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (session, binding) = pending_fixture(&manager, false).await;
    let live = live_witness(session.id);
    let mut finalize = LifecycleMutations::default();
    finalize.finalize_live(binding, live);
    manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Keep,
            SelectionCause::Restore,
            finalize,
        )
        .await
        .unwrap();
    let geometry = WindowGeometry {
        x: 12.0,
        y: 34.0,
        width: 800.0,
        height: 600.0,
    };
    manager
        .set_detached_geometry(session.id, geometry.clone())
        .await;

    let mut detach = LifecycleMutations::default();
    detach.set_detached_intent(session.id, true);
    let changed = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Keep,
            SelectionCause::Detach,
            detach,
        )
        .await
        .unwrap();
    assert_eq!(changed.changed_rows.len(), 1);
    let stored = manager.get_session(session.id).await.unwrap();
    assert!(stored.was_detached);
    let stored_geometry = stored.detached_geometry.expect("stored geometry");
    assert_eq!(stored_geometry.x, geometry.x);
    assert_eq!(stored_geometry.y, geometry.y);
    assert_eq!(stored_geometry.width, geometry.width);
    assert_eq!(stored_geometry.height, geometry.height);

    let revision = manager.selection_payload().await.revision();
    let mut repeat = LifecycleMutations::default();
    repeat.set_detached_intent(session.id, true);
    let repeated = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Keep,
            SelectionCause::Detach,
            repeat,
        )
        .await
        .unwrap();
    assert!(repeated.changed_rows.is_empty());
    assert_eq!(manager.selection_payload().await.revision(), revision);
}

#[tokio::test]
async fn revision_overflow_rejects_before_any_mutation() {
    let manager = SessionManager::new();
    let capability = CommitCapability::for_test();
    let (session, binding) = pending_fixture(&manager, false).await;
    manager.state.write().await.revision = u64::MAX;
    let live = live_witness(session.id);
    let mut finalize = LifecycleMutations::default();
    finalize.finalize_live(binding, live);
    let error = manager
        .commit_selection_transition(
            &capability,
            CommitDecision::Live(live),
            SelectionCause::UserSwitch,
            finalize,
        )
        .await
        .expect_err("revision must not wrap");
    assert_eq!(error, "selection revision overflow");
    assert!(manager.get_pending_session(binding).await.is_some());
    assert!(manager.list_sessions().await.is_empty());
}

/// The unstated invariant that makes `commit_selection_transition`'s
/// `-> Active` promotion neutral for the activity signal: a session is never
/// `Idle` while `waiting_for_input` is false. If it ever were, focusing that
/// session would flip `is_working` false to true with no record behind it.
#[tokio::test]
async fn idle_status_implies_waiting_for_input() {
    let manager = SessionManager::new();
    // The first session takes the selection and becomes Active; the second
    // stays Running, which is the only status `mark_idle` demotes.
    let _selected = manager
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
    let subject = manager
        .create_session(
            "sh".to_string(),
            Vec::new(),
            "C:\\subject".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create the subject session");

    assert_eq!(subject.status, SessionStatus::Running);
    assert!(
        !subject.waiting_for_input,
        "a session is born (Running, false)"
    );

    let mut saw_idle = false;
    // Both writers, repeated, so the no-op paths are covered too.
    for action in ["idle", "idle", "busy", "busy", "idle"] {
        match action {
            "idle" => manager.mark_idle(subject.id).await,
            _ => manager.mark_busy(subject.id).await,
        }
        let observed = manager
            .get_session(subject.id)
            .await
            .expect("the subject session survives");
        if matches!(observed.status, SessionStatus::Idle) {
            saw_idle = true;
            assert!(
                observed.waiting_for_input,
                concat!(
                    "after {action}: Idle with waiting_for_input false ",
                    "manufactures an unrecorded ON edge"
                ),
                action = action
            );
        }
    }
    assert!(
        saw_idle,
        "the sequence must actually reach Idle or it proves nothing"
    );
}
