use super::*;

#[tokio::test]
async fn bootstrapping_rejects_every_external_admission_without_allocating() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    let admission_before = coordinator.inner.admission.available_permits();
    let create_before = coordinator.inner.create_tickets.available_permits();

    assert_eq!(
        coordinator.snapshot().await.unwrap_err(),
        SelectionCoordinatorError::Busy.to_string()
    );
    assert_eq!(
        coordinator
            .transition(SelectionRequest::user_switch(Uuid::new_v4()))
            .await
            .unwrap_err(),
        SelectionCoordinatorError::Busy.to_string()
    );
    assert!(matches!(
        coordinator.reserve_auto_close(),
        Err(SelectionCoordinatorError::Busy)
    ));
    assert_eq!(
        coordinator
            .reserve_create(TrustedCreateIntent::User)
            .await
            .unwrap_err(),
        SelectionCoordinatorError::Busy
    );
    assert_eq!(
        coordinator
            .container_lifecycle_sender()
            .route_lost(Uuid::new_v4(), 1)
            .await
            .unwrap_err(),
        SelectionCoordinatorError::Busy.to_string()
    );
    assert_eq!(
        coordinator
            .watchdog_resource_kill(Uuid::new_v4())
            .await
            .unwrap_err(),
        SelectionCoordinatorError::Busy.to_string()
    );
    assert_eq!(
        coordinator.inner.admission.available_permits(),
        admission_before
    );
    assert_eq!(
        coordinator.inner.create_tickets.available_permits(),
        create_before
    );
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
}

#[tokio::test]
async fn restore_is_fifo_first_and_queues_external_work_until_release() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build restore coordinator test app");
    coordinator
        .start(app.handle().clone())
        .expect("start selection coordinator");

    let publication = crate::session::manager::acquire_carrier_mutation_permit().await;
    let canceled = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.submit_restore_first().await })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        while CoordinatorPhase::from_u8(coordinator.inner.phase.load(Ordering::Acquire))
            != CoordinatorPhase::Restoring
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("restore admission reaches its U wait");
    canceled.abort();
    assert!(matches!(canceled.await, Err(error) if error.is_cancelled()));
    assert_eq!(
        CoordinatorPhase::from_u8(coordinator.inner.phase.load(Ordering::Acquire)),
        CoordinatorPhase::Bootstrapping
    );
    assert!(coordinator
        .inner
        .retained_replacements
        .lock()
        .unwrap()
        .restore_nonce
        .is_none());
    drop(publication);
    let guard = coordinator
        .submit_restore_first()
        .await
        .expect("submit first restore job");
    assert_eq!(
        CoordinatorPhase::from_u8(coordinator.inner.phase.load(Ordering::Acquire)),
        CoordinatorPhase::Running
    );
    assert!(matches!(
        coordinator.submit_restore_first().await,
        Err(error) if error == SelectionCoordinatorError::Busy.to_string()
    ));
    assert!(
        coordinator.restore_is_active(),
        "Running is not restore release"
    );
    let restore_transaction = guard.transaction(app.handle().clone());
    assert!(restore_transaction.has_active_restore_capability());
    let mut queued_snapshot = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.snapshot().await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut queued_snapshot)
            .await
            .is_err(),
        "hydration admitted after restore must remain queued behind it"
    );
    restore_transaction.invalidate_restore_ownership();
    assert!(!restore_transaction.has_active_restore_capability());
    assert!(
        coordinator.restore_is_active(),
        "lost authority does not fabricate barrier release"
    );
    assert!(
        coordinator
            .inner
            .retained_replacements
            .lock()
            .unwrap()
            .unavailable,
        "unknown restore ownership stays persistence unsafe"
    );
    guard.finish();
    assert_eq!(
        queued_snapshot
            .await
            .expect("join queued hydration")
            .expect("queued hydration succeeds")
            .revision(),
        0
    );
    assert!(!coordinator.restore_is_active());
    assert!(
        !restore_transaction.has_active_restore_capability(),
        "released nonce cannot authorize inline work"
    );
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn restore_held_route_loss_waits_then_reconciles_the_public_row() {
    use crate::pty::backend::SessionBackendKind;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/restore-route-loss".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let backend = Arc::new(LifecycleTestBackend::default());
    backend.set_live(session.id, true);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build restore-held route-loss app");
    coordinator
        .start(app.handle().clone())
        .expect("start selection coordinator");
    let guard = coordinator.submit_restore_first().await.unwrap();

    backend.set_live(session.id, false);
    pty.lock()
        .unwrap()
        .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
    let mut route_loss = {
        let sender = coordinator.container_lifecycle_sender();
        tokio::spawn(async move { sender.route_lost(session.id, 71).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut route_loss)
            .await
            .is_err(),
        "route loss must queue behind the held restore job"
    );
    assert!(!matches!(
        manager
            .read()
            .await
            .get_session(session.id)
            .await
            .unwrap()
            .status,
        SessionStatus::Exited(_)
    ));

    guard.finish();
    assert_eq!(
        route_loss
            .await
            .expect("join route-loss waiter")
            .expect("route-loss reconciliation"),
        CriticalAdmissionOutcome::Completed(())
    );
    let row = manager.read().await.get_session(session.id).await.unwrap();
    assert_eq!(row.status, SessionStatus::Exited(71));
    let selection = manager.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(session.id));
    assert_eq!(selection.mode(), SelectionMode::Dormant);
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn derived_agent_id_binding_deferred_and_root_restore_install_before_publish() {
    use crate::config::sessions_persistence::{
        snapshot_sessions, CodingAgentBindingSeed, PersistedSession,
    };
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager.clone(), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(manager.clone())
        .manage(DetachedSessionsState::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    assert!(app.manage(pty));
    coordinator.start(app.handle().clone()).unwrap();
    let guard = coordinator.submit_restore_first().await.unwrap();
    let transaction = guard.transaction(app.handle().clone());
    for root in [false, true] {
        let binding = Uuid::new_v4().to_string();
        let old_runtime = Uuid::new_v4().to_string();
        let seed = CodingAgentBindingSeed {
            identity_revision: format!("r1:0:{}", "a".repeat(64)),
            original_agent_id: "original".into(),
            configuration_ref: None,
        };
        let origin = serde_json::json!({
            "sessionsPath": "C:/sessions.json",
            "rawFileSHA256": "original-bytes",
            "originalOrdinal": if root { 1 } else { 0 }
        });
        let cwd = if root { "C:/root" } else { "C:/deferred" };
        let info = transaction
            .restore_dormant_inline(DormantRestoreRequest {
                persisted: PersistedSession {
                    name: cwd.into(),
                    shell: "shell".into(),
                    working_directory: cwd.into(),
                    agent_id: Some("original".into()),
                    id: Some(old_runtime.clone()),
                    created_at: Some("old-date".into()),
                    coding_agent_binding_id: Some(binding.clone()),
                    coding_agent_binding_seed: Some(seed.clone()),
                    coding_agent_binding_origin: Some(origin.clone()),
                    status: Some(SessionStatus::Exited(9)),
                    ..Default::default()
                },
                working_directory: cwd.into(),
                is_coordinator: root,
                is_root_agent: root,
            })
            .await
            .unwrap();
        assert_ne!(info.id, old_runtime);
        assert_ne!(info.created_at, "old-date");
        let handle = manager.read().await.clone();
        let snapshot = snapshot_sessions(&handle).await;
        let row = snapshot
            .iter()
            .find(|row| row.id.as_deref() == Some(&info.id))
            .unwrap();
        assert_eq!(row.coding_agent_binding_id.as_ref(), Some(&binding));
        assert_eq!(row.coding_agent_binding_seed.as_ref(), Some(&seed));
        assert_eq!(row.coding_agent_binding_origin.as_ref(), Some(&origin));
        assert_eq!(row.is_root_agent, root);
    }
    drop(transaction);
    drop(guard);
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn restore_transaction_publishes_rows_before_one_final_selection() {
    use crate::config::sessions_persistence::PersistedSession;
    use tauri::Listener;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let broadcaster = WsBroadcaster::new();
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(DetachedSessionsState::default())
        .manage(broadcaster.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build restore transaction app");
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    assert!(app.manage(pty));
    coordinator
        .start(app.handle().clone())
        .expect("start restore coordinator");
    let guard = coordinator.submit_restore_first().await.unwrap();
    let transaction = guard.transaction(app.handle().clone());
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for event_name in ["session_created", "session_switched"] {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |event| {
            let _ = events_tx.send((event_name, event.payload().to_string()));
        });
    }

    let first = transaction
        .restore_dormant_inline(DormantRestoreRequest {
            persisted: PersistedSession {
                name: "first".to_string(),
                shell: "shell".to_string(),
                shell_args: Vec::new(),
                working_directory: "C:/first".to_string(),
                status: Some(SessionStatus::Exited(7)),
                ..PersistedSession::default()
            },
            working_directory: "C:/first".to_string(),
            is_coordinator: false,
            is_root_agent: false,
        })
        .await
        .unwrap();
    transaction
        .restore_dormant_inline(DormantRestoreRequest {
            persisted: PersistedSession {
                name: "second".to_string(),
                shell: "shell".to_string(),
                shell_args: Vec::new(),
                working_directory: "C:/second".to_string(),
                status: Some(SessionStatus::Exited(8)),
                ..PersistedSession::default()
            },
            working_directory: "C:/second".to_string(),
            is_coordinator: false,
            is_root_agent: false,
        })
        .await
        .unwrap();
    assert_eq!(
        events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
        "session_created"
    );
    assert_eq!(
        events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
        "session_created"
    );
    assert!(events_rx.try_recv().is_err());

    let target = Uuid::parse_str(&first.id).unwrap();
    let selection = transaction
        .restore_selection_inline(Some(target))
        .await
        .unwrap()
        .expect("restore selection changes");
    assert_eq!(selection.id(), Some(target));
    assert_eq!(selection.mode(), SelectionMode::Dormant);
    assert_eq!(selection.status(), Some(&SessionStatus::Exited(7)));
    assert_eq!(
        events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
        "session_switched"
    );
    assert!(events_rx.try_recv().is_err());
    guard.finish();
    coordinator.close_and_join().await;
}

// #2451 - a restored dormant session was matched by nothing, and the tier
// is session-lifetime state that `PersistedSession` does not carry.
#[tokio::test]
async fn restored_dormant_session_has_no_match_tier() {
    use crate::config::sessions_persistence::PersistedSession;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build restore transaction app");
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    assert!(app.manage(pty));
    coordinator
        .start(app.handle().clone())
        .expect("start restore coordinator");
    let guard = coordinator.submit_restore_first().await.unwrap();
    let transaction = guard.transaction(app.handle().clone());

    let restored = transaction
        .restore_dormant_inline(DormantRestoreRequest {
            persisted: PersistedSession {
                name: "dormant".to_string(),
                shell: "shell".to_string(),
                shell_args: Vec::new(),
                working_directory: "C:/dormant".to_string(),
                status: Some(SessionStatus::Exited(0)),
                effective_profile: Some("A".to_string()),
                ..PersistedSession::default()
            },
            working_directory: "C:/dormant".to_string(),
            is_coordinator: false,
            is_root_agent: false,
        })
        .await
        .unwrap();

    assert_eq!(restored.match_tier, None);
    assert_eq!(restored.original_profile_letter, None);
    assert_eq!(restored.effective_profile.as_deref(), Some("A"));
    let id = Uuid::parse_str(&restored.id).unwrap();
    let stored = manager.read().await.get_session(id).await.unwrap();
    assert_eq!(stored.match_tier, None);
    assert_eq!(stored.original_profile_letter, None);
    assert_eq!(stored.effective_profile.as_deref(), Some("A"));
    guard.finish();
    coordinator.close_and_join().await;
}
