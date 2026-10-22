use super::*;

async fn row(manager: &SessionManager, name: &str, coordinator: bool) -> Session {
    manager
        .create_session(
            "shell".into(),
            vec![],
            name.into(),
            None,
            None,
            vec![],
            coordinator,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn response_close_f1_excludes_protected_unknown_and_pending_before_foreign_fallback() {
    let manager = SessionManager::new();
    let protected = row(&manager, "protected", false).await;
    let unknown = row(&manager, "unknown", false).await;
    let pending = manager
        .insert_pending_session_for_test("pending".into())
        .await;
    let foreign = row(&manager, "foreign", false).await;
    let selected = row(&manager, "selected", false).await;
    manager.mark_exited(protected.id, 0).await;
    manager.mark_exited(selected.id, 0).await;
    manager.set_active_only(selected.id).await.unwrap();
    let permit = acquire_carrier_mutation_permit().await;
    let mut classified = HashSet::new();
    let removed = manager
        .remove_exited_sessions_with_carrier_permit(
            &[selected.id, protected.id],
            &permit,
            |row, _| {
                classified.insert(row.id);
                if row.id == protected.id {
                    CarrierDisposition::Protected
                } else if row.id == unknown.id {
                    CarrierDisposition::Unknown
                } else {
                    CarrierDisposition::Foreign
                }
            },
        )
        .await;
    assert_eq!(removed, 1);
    assert_eq!(classified.len(), 5);
    assert!(classified.contains(&pending));
    assert_eq!(manager.selection_payload().await.id(), Some(foreign.id));
    assert!(matches!(
        manager.get_session(protected.id).await.unwrap().status,
        SessionStatus::Exited(0)
    ));
}

#[tokio::test]
async fn response_close_f1_clear_surviving_protected_target_and_no_foreign_fallback() {
    let manager = SessionManager::new();
    let protected = row(&manager, "protected", false).await;
    let other = row(&manager, "unknown", false).await;
    let permit = acquire_carrier_mutation_permit().await;
    let removed = manager
        .remove_exited_sessions_with_carrier_permit(&[protected.id], &permit, |row, _| {
            if row.id == protected.id {
                CarrierDisposition::Protected
            } else {
                CarrierDisposition::Unknown
            }
        })
        .await;
    assert_eq!(removed, 0);
    assert_eq!(
        manager.selection_payload().await.mode(),
        SelectionMode::None
    );
    assert!(manager.get_session(protected.id).await.is_some());
    assert!(manager.get_session(other.id).await.is_some());
    assert!(manager
        .state
        .read()
        .await
        .carrier_versions
        .contains_key(&protected.id));
}

#[tokio::test]
async fn response_close_f1_without_protection_preserves_legacy_exited_fallback_order() {
    let manager = SessionManager::new();
    let first = row(&manager, "first", false).await;
    let second = row(&manager, "second", false).await;
    let selected = row(&manager, "selected", false).await;
    manager.mark_exited(first.id, 0).await;
    manager.mark_exited(selected.id, 0).await;
    manager.set_active_only(selected.id).await.unwrap();
    assert_eq!(manager.remove_exited_sessions(&[selected.id]).await, 1);
    assert_eq!(manager.selection_payload().await.id(), Some(first.id));
    assert_eq!(
        manager.get_session(first.id).await.unwrap().status,
        SessionStatus::Active
    );
    assert!(manager.get_session(second.id).await.is_some());
}

#[tokio::test]
async fn response_close_binder_waits_for_publication_before_exposing_identity_change() {
    use std::future::Future;
    let manager = SessionManager::new();
    let session = row(&manager, "original", false).await;
    let permit = acquire_carrier_mutation_permit().await;
    let mut mutation = std::pin::pin!(manager.rename_session(session.id, "updated".into()));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(mutation.as_mut().poll(&mut context).is_pending());
    manager
        .with_current_carrier_snapshot(&permit, |snapshot| {
            assert_eq!(
                snapshot
                    .rows
                    .iter()
                    .find(|r| r.id == session.id)
                    .unwrap()
                    .name,
                session.name
            );
        })
        .await;
    drop(permit);
    mutation.await.unwrap();
    assert_eq!(
        manager.get_session(session.id).await.unwrap().name,
        "updated"
    );
}
#[tokio::test]
async fn response_close_carrier_versions_ignore_observations_noops_and_fail_closed_on_overflow() {
    let manager = SessionManager::new();
    let session = row(&manager, "row", false).await;
    assert_eq!(
        manager
            .state
            .read()
            .await
            .carrier_versions
            .get(&session.id)
            .copied(),
        Some(1)
    );
    assert_eq!(manager.get_active().await, Some(session.id));
    let second = row(&manager, "second", false).await;
    assert_eq!(
        manager
            .state
            .read()
            .await
            .carrier_versions
            .get(&second.id)
            .copied(),
        Some(1)
    );
    assert_eq!(
        manager
            .state
            .read()
            .await
            .carrier_versions
            .get(&session.id)
            .copied(),
        Some(1)
    );
    assert_eq!(manager.get_active().await, Some(session.id));
    manager.mark_idle(session.id).await;
    manager.mark_idle(second.id).await;
    assert_eq!(
        manager
            .state
            .read()
            .await
            .carrier_versions
            .get(&session.id)
            .copied(),
        Some(1)
    );
    assert_eq!(
        manager
            .state
            .read()
            .await
            .carrier_versions
            .get(&second.id)
            .copied(),
        Some(1)
    );
    manager
        .set_git_repos(
            session.id,
            vec![SessionRepo {
                label: "repo".into(),
                source_path: "path".into(),
                branch: None,
                dirty: None,
            }],
        )
        .await;
    let before = manager.state.read().await.carrier_versions[&session.id];
    manager
        .set_git_repos(
            session.id,
            vec![SessionRepo {
                label: "repo".into(),
                source_path: "path".into(),
                branch: Some("branch".into()),
                dirty: Some(true),
            }],
        )
        .await;
    manager.set_context_percent(session.id, Some(20)).await;
    manager.set_last_prompt(session.id, "prompt".into()).await;
    manager
        .rename_session(session.id, session.name.clone())
        .await
        .unwrap();
    assert_eq!(
        manager.state.read().await.carrier_versions[&session.id],
        before
    );
    manager.set_is_coordinator(session.id, true).await;
    assert_eq!(
        manager.state.read().await.carrier_versions[&session.id],
        before + 1
    );
    manager
        .state
        .write()
        .await
        .carrier_versions
        .insert(session.id, u64::MAX);
    manager
        .rename_session(session.id, "overflow".into())
        .await
        .unwrap();
    assert_eq!(
        manager.state.read().await.carrier_versions[&session.id],
        u64::MAX
    );
    assert!(manager.response_close.generation(session.id).is_none());
}

#[tokio::test]
async fn response_close_raise_rollback_cannot_clear_successor_communication() {
    let manager = SessionManager::new();
    let session = row(&manager, "coordinator", true).await;
    let permit = acquire_carrier_mutation_permit().await;
    let RaiseCarrierOutcome::Changed(_, receipt) = manager
        .raise_hand_with_carrier_permit(&permit, session.id, chrono::Utc::now())
        .await
    else {
        panic!("expected raise");
    };
    manager
        .clear_communication_if_kind_with_carrier_permit(
            &permit,
            session.id,
            SessionCommunicationKind::RaiseHand,
        )
        .await;
    let RaiseCarrierOutcome::Changed(_, successor) = manager
        .raise_hand_with_carrier_permit(&permit, session.id, chrono::Utc::now())
        .await
    else {
        panic!("expected successor");
    };
    assert_eq!(
        manager
            .rollback_raise_with_carrier_permit(&permit, receipt)
            .await,
        RaiseRollbackOutcome::Superseded
    );
    assert!(
        manager
            .get_session(session.id)
            .await
            .unwrap()
            .communication
            .unwrap()
            .visible
    );
    assert_eq!(
        manager
            .rollback_raise_with_carrier_permit(&permit, successor)
            .await,
        RaiseRollbackOutcome::Reverted
    );
    assert!(manager
        .get_session(session.id)
        .await
        .unwrap()
        .communication
        .is_none());
}

#[tokio::test]
async fn response_close_prepared_create_binding_and_borrowed_pending_aliases_keep_exact_uuid_nonce()
{
    let manager = SessionManager::new();
    let binding = manager.reserve_carrier_create_binding();
    let permit = acquire_carrier_mutation_permit().await;
    let session = manager
        .create_pending_session_with_binding_and_carrier_permit(
            binding,
            &permit,
            "shell".into(),
            vec![],
            "cwd".into(),
            None,
            None,
            vec![],
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    assert_eq!(session.id, binding.session_id());
    manager
        .rename_pending_session_with_carrier_permit(&permit, binding, "pending".into())
        .await
        .unwrap();
    assert!(manager
        .rename_pending_session_with_carrier_permit(
            &permit,
            PendingCreateBinding::new(binding.session_id, Uuid::new_v4()),
            "wrong".into()
        )
        .await
        .is_err());
    assert_eq!(
        manager.get_pending_session(binding).await.unwrap().name,
        "pending"
    );
    manager
        .rollback_pending_create_with_carrier_permit(&permit, binding)
        .await
        .unwrap();
    assert!(!manager.contains_public_or_pending(session.id).await);
}
