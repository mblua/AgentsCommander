use super::*;

#[tokio::test]
async fn clear_active_removes_active_and_demotes_status() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    assert_eq!(mgr.get_active().await, Some(session.id));
    mgr.clear_active().await;

    assert_eq!(mgr.get_active().await, None);
    let stored = mgr.get_session(session.id).await.unwrap();
    assert_eq!(stored.status, SessionStatus::Running);
}

#[tokio::test]
async fn clear_active_if_preserves_non_matching_active_session() {
    let mgr = SessionManager::new();
    let first = mgr
        .create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp\\one".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create first session");
    let second = mgr
        .create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp\\two".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create second session");

    mgr.switch_session(second.id)
        .await
        .expect("switch to second session");
    mgr.clear_active_if(first.id).await;

    assert_eq!(mgr.get_active().await, Some(second.id));
    let first_stored = mgr.get_session(first.id).await.unwrap();
    let second_stored = mgr.get_session(second.id).await.unwrap();
    assert_eq!(first_stored.status, SessionStatus::Running);
    assert_eq!(second_stored.status, SessionStatus::Active);
}

// ── Issue #248 — set_active_only (Fix A) ──

#[tokio::test]
async fn set_active_only_preserves_dormant_status() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".into(),
            vec![],
            "C:\\proj".into(),
            None,
            None,
            vec![],
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    // After create_session, the session is auto-activated (status = Active);
    // call mark_exited to put it in the dormant state under test.
    mgr.mark_exited(session.id, 0).await;
    // clear_active_if to drop the now-stale active pointer.
    mgr.clear_active_if(session.id).await;
    assert_eq!(mgr.get_active().await, None);

    // The behavior under test: select the dormant session without flipping
    // its status.
    mgr.set_active_only(session.id).await.unwrap();
    assert_eq!(mgr.get_active().await, Some(session.id));
    let s = mgr.get_session(session.id).await.unwrap();
    assert!(matches!(s.status, SessionStatus::Exited(0))); // PRESERVED, not Active
}

#[tokio::test]
async fn set_active_only_demotes_previously_active() {
    let mgr = SessionManager::new();
    let live = mgr
        .create_session(
            "c".into(),
            vec![],
            "C:\\a".into(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    // First session auto-activates → status = Active, active_session = live.id
    assert_eq!(mgr.get_active().await, Some(live.id));
    let live_state = mgr.get_session(live.id).await.unwrap();
    assert_eq!(live_state.status, SessionStatus::Active);

    // Create + mark-exited a second session for the dormant-select scenario.
    let dormant = mgr
        .create_session(
            "c".into(),
            vec![],
            "C:\\b".into(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    mgr.mark_exited(dormant.id, 0).await;

    mgr.set_active_only(dormant.id).await.unwrap();

    // Active pointer moved.
    assert_eq!(mgr.get_active().await, Some(dormant.id));
    // Previously-active demoted: Active → Running.
    let live_after = mgr.get_session(live.id).await.unwrap();
    assert_eq!(live_after.status, SessionStatus::Running);
    // New active preserved as Exited.
    let dormant_after = mgr.get_session(dormant.id).await.unwrap();
    assert!(matches!(dormant_after.status, SessionStatus::Exited(0)));
}

#[tokio::test]
async fn set_active_only_returns_session_not_found_for_unknown_id() {
    let mgr = SessionManager::new();
    let bogus = uuid::Uuid::new_v4();
    let err = mgr.set_active_only(bogus).await.unwrap_err();
    assert!(matches!(err, AppError::SessionNotFound(_)));
    // Active pointer untouched.
    assert_eq!(mgr.get_active().await, None);
}

// ── Issue #248 / Grinch Z9 — defer + set_active_only + list_sessions chain ──

#[tokio::test]
async fn issue_248_defer_set_active_only_list_sessions_chain() {
    let mgr = SessionManager::new();
    // Simulate the defer arm of lib.rs §3.4: create a session, mark_exited,
    // clear_active_if.
    let session = mgr
        .create_session(
            "claude".into(),
            vec![],
            "C:\\proj\\.ac\\_agent_architect".into(),
            Some("aid".into()),
            Some("Architect".into()),
            vec![],
            true, // is_coordinator
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    mgr.mark_exited(session.id, 0).await;
    mgr.clear_active_if(session.id).await;

    // Simulate the post-loop active-switch (§3.7) with the dormant branch.
    mgr.set_active_only(session.id).await.unwrap();

    // The wire payload (what list_sessions IPC returns to the frontend).
    let infos = mgr.list_sessions().await;
    assert_eq!(infos.len(), 1);
    let json = serde_json::to_value(&infos[0]).unwrap();

    // The critical assertion — Round-2 Z1 blocker.
    // Before Fix A, this would be `"status":"active"` and the FE would
    // render the live dot, taking the wrong click path. With Fix A, status
    // round-trips as the object form for SessionStatus::Exited.
    assert_eq!(json["status"], serde_json::json!({ "exited": 0 }));

    // Active pointer correctly reflects the selection.
    assert_eq!(mgr.get_active().await, Some(session.id));
}

/// #260 G1 — pins `mark_idle`'s contract: the terminal mutation the
/// idle-detector `on_idle` callback performs. NOTE: `create_session`
/// auto-activates the first session (status `Active`), and `mark_idle`
/// only transitions `Running → Idle` — so demote via `clear_active` first.
/// Without that step the status assertion below would be vacuous.
#[tokio::test]
async fn mark_idle_sets_waiting_for_input_and_running_to_idle() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "codex".into(),
            vec![],
            "C:\\proj".into(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    mgr.clear_active().await; // Active → Running
    let before = mgr.get_session(session.id).await.unwrap();
    assert_eq!(before.status, SessionStatus::Running);
    assert!(!before.waiting_for_input);

    mgr.mark_idle(session.id).await;

    let after = mgr.get_session(session.id).await.unwrap();
    assert!(
        after.waiting_for_input,
        "mark_idle must set waiting_for_input = true"
    );
    assert_eq!(
        after.status,
        SessionStatus::Idle,
        "mark_idle must transition Running → Idle"
    );
}

// ── #1088: set_context_percent mutator ──

#[tokio::test]
async fn set_context_percent_round_trips_including_zero_and_clear() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "codex".into(),
            vec![],
            "C:\\proj".into(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();

    // Fresh session has no reading.
    assert_eq!(
        mgr.get_session(session.id).await.unwrap().context_percent,
        None
    );

    mgr.set_context_percent(session.id, Some(42)).await;
    assert_eq!(
        mgr.get_session(session.id).await.unwrap().context_percent,
        Some(42)
    );

    // `0` is a valid reading, stored as Some(0), never coerced to None.
    mgr.set_context_percent(session.id, Some(0)).await;
    assert_eq!(
        mgr.get_session(session.id).await.unwrap().context_percent,
        Some(0)
    );

    // None clears it.
    mgr.set_context_percent(session.id, None).await;
    assert_eq!(
        mgr.get_session(session.id).await.unwrap().context_percent,
        None
    );
}

#[tokio::test]
async fn set_context_percent_noops_on_unknown_id() {
    let mgr = SessionManager::new();
    // Must not panic; the id is not present, so the write is a silent no-op.
    mgr.set_context_percent(Uuid::new_v4(), Some(5)).await;
}

#[tokio::test]
async fn set_context_percent_noops_on_pending_create_id() {
    let mgr = SessionManager::new();
    let (pending, _binding) = pending_fixture(&mgr, false).await;
    // The pending guard suppresses the write; the row stays invisible and
    // nothing panics (mirrors mark_idle's pending_create guard).
    mgr.set_context_percent(pending.id, Some(5)).await;
    assert!(mgr.get_session(pending.id).await.is_none());
}
