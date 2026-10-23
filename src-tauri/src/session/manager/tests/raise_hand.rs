use super::*;

#[tokio::test]
async fn raise_hand_coordinator_running_session_stores_communication() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    let now = chrono::DateTime::parse_from_rfc3339("2026-06-28T17:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    let raised = mgr.raise_hand(session.id, now).await;

    let Some((changed, communication)) = raised else {
        panic!("coordinator should accept raise-hand");
    };
    assert!(changed);
    assert_eq!(communication.kind, SessionCommunicationKind::RaiseHand);
    assert!(communication.visible);
    assert_eq!(communication.updated_at, now.to_rfc3339());
    let stored = mgr.get_session(session.id).await.unwrap();
    assert_eq!(stored.communication, Some(communication.clone()));
    let infos = mgr.list_sessions().await;
    assert_eq!(infos[0].communication, Some(communication));
}

#[tokio::test]
async fn raise_hand_second_request_returns_existing_without_mutation() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    let first = chrono::DateTime::parse_from_rfc3339("2026-06-28T17:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let second = chrono::DateTime::parse_from_rfc3339("2026-06-28T18:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    let (_, first_comm) = mgr.raise_hand(session.id, first).await.unwrap();
    let second_result = mgr.raise_hand(session.id, second).await;

    assert_eq!(second_result, Some((false, first_comm.clone())));
    let stored = mgr.get_session(session.id).await.unwrap();
    assert_eq!(stored.communication, Some(first_comm));
}

#[tokio::test]
async fn raise_hand_non_coordinator_returns_none() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
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

    assert!(mgr
        .raise_hand(session.id, chrono::Utc::now())
        .await
        .is_none());
    assert!(mgr
        .get_session(session.id)
        .await
        .unwrap()
        .communication
        .is_none());
}

#[tokio::test]
async fn raise_hand_exited_coordinator_returns_none() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    assert!(!mgr.mark_exited(session.id, 0).await);

    assert!(mgr
        .raise_hand(session.id, chrono::Utc::now())
        .await
        .is_none());
    assert!(mgr
        .get_session(session.id)
        .await
        .unwrap()
        .communication
        .is_none());
}

#[tokio::test]
async fn raise_hand_clear_removes_visible_raise_hand_once() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    mgr.raise_hand(session.id, chrono::Utc::now())
        .await
        .unwrap();

    assert!(
        mgr.clear_communication_if_kind(session.id, SessionCommunicationKind::RaiseHand)
            .await
    );
    assert!(mgr
        .get_session(session.id)
        .await
        .unwrap()
        .communication
        .is_none());
    assert!(
        !mgr.clear_communication_if_kind(session.id, SessionCommunicationKind::RaiseHand)
            .await
    );
    assert!(
        !mgr.clear_communication_if_kind(Uuid::new_v4(), SessionCommunicationKind::RaiseHand)
            .await
    );
}

#[tokio::test]
async fn raise_hand_mark_exited_clears_visible_state() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    mgr.raise_hand(session.id, chrono::Utc::now())
        .await
        .unwrap();

    assert!(mgr.mark_exited(session.id, 7).await);
    let stored = mgr.get_session(session.id).await.unwrap();
    assert!(matches!(stored.status, SessionStatus::Exited(7)));
    assert!(stored.communication.is_none());
    let infos = mgr.list_sessions().await;
    assert!(infos[0].communication.is_none());
}

/// #2232 phase 7: the Co-managed communication follows the same shape and
/// dedup rule as `set_blocked_menu`, and its kind serializes as `coManaged`.
#[tokio::test]
async fn set_co_managed_round_trips_and_dedups_identical_messages() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    let now = chrono::Utc::now();

    let (changed, communication) = mgr
        .set_co_managed(session.id, "Co-managed: routed".to_string(), now)
        .await
        .expect("the session exists and is live");
    assert!(changed);
    assert_eq!(communication.kind, SessionCommunicationKind::CoManaged);
    assert!(communication.visible);
    assert_eq!(communication.message.as_deref(), Some("Co-managed: routed"));

    // Re-setting the identical visible message reports unchanged.
    let (changed, again) = mgr
        .set_co_managed(
            session.id,
            "Co-managed: routed".to_string(),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert!(!changed);
    assert_eq!(again.updated_at, communication.updated_at);

    // The wire form of the new variant is camelCase, like its siblings.
    assert_eq!(
        serde_json::to_string(&SessionCommunicationKind::CoManaged).unwrap(),
        "\"coManaged\""
    );

    assert!(mgr.clear_co_managed(session.id).await);
    assert!(mgr
        .get_session(session.id)
        .await
        .unwrap()
        .communication
        .is_none());
}

#[tokio::test]
async fn raise_hand_mark_exited_returns_false_without_visible_state() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    assert!(!mgr.mark_exited(session.id, 0).await);
    assert!(mgr
        .get_session(session.id)
        .await
        .unwrap()
        .communication
        .is_none());
}

/// (#747) The restore seam accepts an Exited coordinator record (the defer
/// arm calls it right after `mark_exited`) and keeps the original raise
/// time so the indicator's age stays truthful.
#[tokio::test]
async fn restore_communication_applies_visible_raise_hand_to_dormant_coordinator() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    mgr.mark_exited(session.id, 0).await;

    let original_raise_time = "2026-06-30T11:00:00+00:00".to_string();
    let communication = SessionCommunication {
        kind: SessionCommunicationKind::RaiseHand,
        visible: true,
        updated_at: original_raise_time.clone(),
        message: None,
    };

    assert!(
        mgr.restore_communication(session.id, communication.clone())
            .await,
        "dormant coordinator must accept the restored hand"
    );
    let stored = mgr.get_session(session.id).await.unwrap();
    assert!(matches!(stored.status, SessionStatus::Exited(0)));
    let restored = stored.communication.expect("restored hand stored");
    assert_eq!(restored.kind, SessionCommunicationKind::RaiseHand);
    assert!(restored.visible);
    assert_eq!(
        restored.updated_at, original_raise_time,
        "restore must preserve the ORIGINAL raise time, not re-stamp it"
    );
}

/// (#747) Restore gates: non-coordinator records, hidden payloads, and
/// unknown ids are all rejected without mutating state.
#[tokio::test]
async fn restore_communication_rejects_non_coordinator_hidden_payload_and_unknown_id() {
    let mgr = SessionManager::new();
    let visible_hand = SessionCommunication {
        kind: SessionCommunicationKind::RaiseHand,
        visible: true,
        updated_at: "2026-06-30T11:00:00+00:00".to_string(),
        message: None,
    };

    // Non-coordinator record: rejected, communication stays None.
    let member = mgr
        .create_session(
            "claude".to_string(),
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
    assert!(
        !mgr.restore_communication(member.id, visible_hand.clone())
            .await
    );
    assert!(mgr
        .get_session(member.id)
        .await
        .unwrap()
        .communication
        .is_none());

    // Hidden payload on a coordinator: rejected.
    let coordinator = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp2".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    let hidden_hand = SessionCommunication {
        visible: false,
        ..visible_hand.clone()
    };
    assert!(!mgr.restore_communication(coordinator.id, hidden_hand).await);
    assert!(mgr
        .get_session(coordinator.id)
        .await
        .unwrap()
        .communication
        .is_none());

    // Unknown id: rejected.
    assert!(
        !mgr.restore_communication(Uuid::new_v4(), visible_hand)
            .await
    );
}

/// #698 MEDIUM fix: `clear_user_input_transitions` lowers a visible raise-hand
/// and, when gated, re-arms the fresh intent in ONE critical section,
/// reporting which fields transitioned. A second call is a no-op, and an
/// unknown id is a no-op.
#[tokio::test]
async fn clear_user_input_transitions_clears_both_fields_atomically() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    mgr.set_start_fresh_on_restore(session.id, true).await;
    mgr.raise_hand(session.id, chrono::Utc::now())
        .await
        .expect("raise_hand should succeed");

    // Both transitions happen together and are reported.
    let (cleared_start_fresh, cleared_raise_hand) =
        mgr.clear_user_input_transitions(session.id, true).await;
    assert!(cleared_start_fresh);
    assert!(cleared_raise_hand);

    let stored = mgr
        .get_session(session.id)
        .await
        .expect("session should still exist");
    assert!(!stored.start_fresh_on_restore);
    assert!(stored.communication.is_none());

    // Idempotent: a second call transitions nothing.
    assert_eq!(
        mgr.clear_user_input_transitions(session.id, true).await,
        (false, false)
    );

    // Unknown id is a no-op.
    assert_eq!(
        mgr.clear_user_input_transitions(Uuid::new_v4(), true).await,
        (false, false)
    );
}

/// #698: the two transitions are independent. `clear_user_input_transitions`
/// reports each field's own true/false, so a session with only one set re-arms
/// only that one.
#[tokio::test]
async fn clear_user_input_transitions_reports_each_field_independently() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    // Only the fresh intent is set -> (true, false).
    mgr.set_start_fresh_on_restore(session.id, true).await;
    assert_eq!(
        mgr.clear_user_input_transitions(session.id, true).await,
        (true, false)
    );

    // Only a raised hand is set -> (false, true).
    mgr.raise_hand(session.id, chrono::Utc::now())
        .await
        .expect("raise_hand should succeed");
    assert_eq!(
        mgr.clear_user_input_transitions(session.id, true).await,
        (false, true)
    );
}

#[tokio::test]
async fn clear_user_input_transitions_preserves_fresh_when_gate_is_false() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    mgr.set_start_fresh_on_restore(session.id, true).await;
    mgr.raise_hand(session.id, chrono::Utc::now())
        .await
        .expect("raise_hand should succeed");

    assert_eq!(
        mgr.clear_user_input_transitions(session.id, false).await,
        (false, true)
    );

    let stored = mgr
        .get_session(session.id)
        .await
        .expect("session should still exist");
    assert!(stored.start_fresh_on_restore);
    assert!(stored.communication.is_none());
}

#[tokio::test]
async fn set_telegram_bot_id_writes_and_clears_field() {
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

    assert!(!mgr.session_has_telegram_bot(session.id).await);

    mgr.set_telegram_bot_id(session.id, Some("bot-1".to_string()))
        .await;
    assert!(mgr.session_has_telegram_bot(session.id).await);
    assert_eq!(
        mgr.get_session(session.id)
            .await
            .unwrap()
            .telegram_bot_id
            .as_deref(),
        Some("bot-1")
    );

    mgr.set_telegram_bot_id(session.id, None).await;
    assert!(!mgr.session_has_telegram_bot(session.id).await);
    assert!(mgr
        .get_session(session.id)
        .await
        .unwrap()
        .telegram_bot_id
        .is_none());
}

#[tokio::test]
async fn set_effective_shell_args_overwrites_on_recall() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude-mb".to_string(),
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

    mgr.set_effective_shell_args(session.id, vec!["--continue".to_string()])
        .await;
    mgr.set_effective_shell_args(
        session.id,
        vec!["--continue".to_string(), "--debug".to_string()],
    )
    .await;

    let stored = mgr.get_session(session.id).await.unwrap();
    assert_eq!(
        stored.effective_shell_args,
        Some(vec!["--continue".to_string(), "--debug".to_string()])
    );
}
