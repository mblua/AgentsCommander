//! P04 selection dispatch tests for the lifecycle execution port.

use super::*;
use std::sync::atomic::AtomicU8;

#[tokio::test]
async fn missing_binding_start_preserves_cleanup_receiver_and_capacity_for_retry() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    let coordinator = SelectionCoordinator::new(manager.clone(), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(manager)
        .manage(pty.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert_eq!(
        coordinator.start(app.handle().clone()),
        Err(SelectionCoordinatorError::Unavailable)
    );
    assert!(coordinator.inner.cleanup_pty.lock().unwrap().is_none());
    assert!(coordinator
        .inner
        .cleanup_container_backend
        .lock()
        .unwrap()
        .is_none());
    assert!(coordinator.inner.receiver.lock().unwrap().is_some());
    assert!(coordinator.inner.worker.lock().unwrap().is_none());
    assert_eq!(
        coordinator.inner.sender.capacity(),
        COORDINATOR_QUEUE_CAPACITY
    );
    assert_eq!(
        coordinator.inner.admission.available_permits(),
        COORDINATOR_ADMISSION_CAPACITY
    );
    assert_eq!(
        coordinator.inner.create_tickets.available_permits(),
        CREATE_TICKET_CAPACITY
    );
    assert!(
        app.manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
    );
    coordinator.start(app.handle().clone()).unwrap();
    assert!(coordinator.inner.receiver.lock().unwrap().is_none());
    assert!(Arc::ptr_eq(
        &coordinator
            .inner
            .cleanup_pty
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap(),
        &pty
    ));
    coordinator.submit_restore_first().await.unwrap().finish();
    coordinator.snapshot().await.unwrap();
    coordinator.close_and_join().await;
}

#[test]
fn duplicate_binding_keeps_original_executor() {
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let transaction = SelectionTransaction::new(app.handle().clone(), Weak::new());
    let original = transaction.lifecycle_executor().unwrap();
    assert!(
        !app.manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
    );
    assert!(Arc::ptr_eq(
        &original,
        &transaction.lifecycle_executor().unwrap()
    ));
}

#[tokio::test]
async fn wrong_payload_is_rejected_without_native_state_or_teardown() {
    // No native dependencies are installed: reaching a native body would panic.
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let transaction = SelectionTransaction::new(app.handle().clone(), Weak::new());
    let executor = transaction.lifecycle_executor().unwrap();
    let receipt = Arc::new(AtomicU8::new(0));
    let mut lease = LifecycleLease::new(Box::new(()), receipt.clone());
    assert!(!executor.has_claim(&lease));
    assert!(!executor.validate(&transaction, &mut lease).await);
    let restart = RestartJobRequest {
        session_id: Uuid::new_v4(),
        agent_id: None,
        requested_profile: None,
        skip_auto_resume: None,
        activate_after: false,
        intent: TrustedRestartIntent::User,
        communication_override: None,
        enforcement: crate::config::sessions_persistence::CreationGateEnforcement::Enforce,
    };
    assert!(matches!(
        executor.restart(&transaction, restart, &lease).await,
        LifecycleEntryOutcome::Interrupted
    ));
    let root = RootJobRequest {
        requested_agent_id: None,
        requested_profile: None,
        skip_auto_resume_for_new_session: false,
        intent: TrustedCreateIntent::User,
        select_after: false,
    };
    assert!(matches!(
        executor.root(&transaction, root, &lease).await,
        LifecycleEntryOutcome::Interrupted
    ));
    assert!(!executor.release(lease));
    assert_eq!(receipt.load(Ordering::Acquire), 0);
    assert!(transaction.lifecycle_entry.is_none());
    assert!(transaction.restore_nonce.is_none());
}

#[tokio::test]
async fn real_authority_explicit_and_fallback_release_free_claim_once() {
    let cwd = tempfile::tempdir().unwrap();
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let source = manager
        .read()
        .await
        .create_session(
            "shell".into(),
            Vec::new(),
            cwd.path().to_string_lossy().into_owned(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let source = source.id;
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    let tracker = pty.lock().unwrap().response_close_state();
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(manager.clone())
        .manage(pty)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let transaction = SelectionTransaction::new(app.handle().clone(), Weak::new());
    let executor = transaction.lifecycle_executor().unwrap();
    for explicit in [true, false] {
        let LifecycleAdmission::Ready(lease) = executor
            .prepare(&transaction, Uuid::new_v4(), Some(source), false, None)
            .await
            .unwrap()
        else {
            panic!("native prepare must mint a real authority and claim");
        };
        assert!(executor.has_claim(&lease));
        let receipt = lease.release_receipt.clone();
        assert_eq!(receipt.load(Ordering::Acquire), 0);
        if explicit {
            assert!(executor.release(lease));
        } else {
            drop(lease);
        }
        assert_eq!(receipt.load(Ordering::Acquire), 1);
        assert!(tracker.is_healthy());
        // A second real claim on the same target proves the first was released.
        let LifecycleAdmission::Ready(next) = executor
            .prepare(&transaction, Uuid::new_v4(), Some(source), false, None)
            .await
            .unwrap()
        else {
            panic!("released target must accept a fresh authority");
        };
        assert!(executor.has_claim(&next));
        drop(next);
        assert_eq!(receipt.load(Ordering::Acquire), 1);
        assert_eq!(backend.kill_count(source), 0);
        assert!(manager.read().await.get_session(source).await.is_some());
    }
}
