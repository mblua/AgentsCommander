use super::critical_key::{assert_cancelled_critical_waiter_releases_key, CriticalWaitPoint};
use super::*;

#[tokio::test]
async fn admission_and_create_ticket_budgets_are_exact_and_fail_fast() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);

    let mut create_tickets = Vec::new();
    for _ in 0..CREATE_TICKET_CAPACITY {
        create_tickets.push(
            coordinator
                .reserve_create(TrustedCreateIntent::Background)
                .await
                .expect("create ticket within budget"),
        );
    }
    assert_eq!(
        coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .expect_err("seventeenth slow create must fail"),
        SelectionCoordinatorError::Busy
    );

    let mut general = Vec::new();
    for _ in 0..(COORDINATOR_QUEUE_CAPACITY - CREATE_TICKET_CAPACITY) {
        general.push(
            coordinator
                .reserve_auto_close()
                .expect("unparked queue capacity remains available"),
        );
    }
    assert!(matches!(
        coordinator.reserve_auto_close(),
        Err(SelectionCoordinatorError::Busy)
    ));
    assert_eq!(
        COORDINATOR_ADMISSION_CAPACITY,
        COORDINATOR_QUEUE_CAPACITY + 1
    );

    drop(general);
    drop(create_tickets);
    coordinator
        .reserve_auto_close()
        .expect("dropped reservations return capacity");
}

#[tokio::test]
async fn route_loss_admission_wait_cancellation_releases_critical_key() {
    assert_cancelled_critical_waiter_releases_key(
        CriticalAdmissionKind::RouteLoss,
        CriticalWaitPoint::AdmissionPermit,
    )
    .await;
}

#[tokio::test]
async fn route_loss_queue_slot_wait_cancellation_releases_critical_key() {
    assert_cancelled_critical_waiter_releases_key(
        CriticalAdmissionKind::RouteLoss,
        CriticalWaitPoint::QueueSlot,
    )
    .await;
}

#[tokio::test]
async fn watchdog_admission_wait_cancellation_releases_critical_key() {
    assert_cancelled_critical_waiter_releases_key(
        CriticalAdmissionKind::WatchdogKill,
        CriticalWaitPoint::AdmissionPermit,
    )
    .await;
}

#[tokio::test]
async fn watchdog_queue_slot_wait_cancellation_releases_critical_key() {
    assert_cancelled_critical_waiter_releases_key(
        CriticalAdmissionKind::WatchdogKill,
        CriticalWaitPoint::QueueSlot,
    )
    .await;
}

#[tokio::test]
async fn background_cleanup_admission_wait_cancellation_releases_critical_key() {
    assert_cancelled_critical_waiter_releases_key(
        CriticalAdmissionKind::BackgroundCleanup,
        CriticalWaitPoint::AdmissionPermit,
    )
    .await;
}

#[tokio::test]
async fn background_cleanup_queue_slot_wait_cancellation_releases_critical_key() {
    assert_cancelled_critical_waiter_releases_key(
        CriticalAdmissionKind::BackgroundCleanup,
        CriticalWaitPoint::QueueSlot,
    )
    .await;
}

#[tokio::test]
async fn full_queue_critical_route_waiter_is_deduplicated_and_runs_after_capacity_returns() {
    use crate::pty::backend::SessionBackendKind;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/critical-capacity".to_string(),
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
        .expect("build critical-capacity app");
    coordinator.start(app.handle().clone()).unwrap();
    let guard = coordinator.submit_restore_first().await.unwrap();
    let mut reservations = (0..COORDINATOR_QUEUE_CAPACITY)
        .map(|_| coordinator.reserve_auto_close().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(coordinator.inner.admission.available_permits(), 0);

    backend.set_live(session.id, false);
    pty.lock()
        .unwrap()
        .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
    let mut first_waiter = {
        let sender = coordinator.container_lifecycle_sender();
        tokio::spawn(async move { sender.route_lost(session.id, 82).await })
    };
    await_state_eq(
        "critical waiter registers while the queue is full",
        Duration::from_secs(1),
        || coordinator.inner.critical_keys.lock().unwrap().len(),
        1,
        String::new,
    )
    .await;
    assert_eq!(
        coordinator
            .container_lifecycle_sender()
            .route_lost(session.id, 99)
            .await
            .unwrap(),
        CriticalAdmissionOutcome::AlreadyPending
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut first_waiter)
            .await
            .is_err(),
        "critical waiter must remain pending while all logical capacity is held"
    );

    drop(reservations.pop());
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(reservations);
    guard.finish();
    assert_eq!(
        first_waiter
            .await
            .expect("join critical waiter")
            .expect("critical route-loss result"),
        CriticalAdmissionOutcome::Completed(())
    );
    // Completed is observed before the first waiter's admission guard drops.
    await_state_eq(
        "route-loss waiter releases every critical key",
        Duration::from_secs(1),
        || coordinator.inner.critical_keys.lock().unwrap().len(),
        0,
        String::new,
    )
    .await;
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    assert_eq!(
        manager
            .read()
            .await
            .get_session(session.id)
            .await
            .unwrap()
            .status,
        SessionStatus::Exited(82)
    );
    assert_eq!(
        coordinator
            .container_lifecycle_sender()
            .route_lost(session.id, 100)
            .await
            .unwrap(),
        CriticalAdmissionOutcome::Completed(())
    );
    // Completed is observed before the follow-up probe's admission guard drops.
    await_state_eq(
        "route-loss waiter releases every critical key",
        Duration::from_secs(1),
        || coordinator.inner.critical_keys.lock().unwrap().len(),
        0,
        String::new,
    )
    .await;
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn full_queue_keeps_critical_kinds_distinct_and_deduplicates_each_kind() {
    use crate::pty::backend::SessionBackendKind;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/critical-kind-capacity".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(pty)
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build critical-kind app");
    coordinator.start(app.handle().clone()).unwrap();
    let guard = coordinator.submit_restore_first().await.unwrap();
    let reservations = (0..COORDINATOR_QUEUE_CAPACITY)
        .map(|_| coordinator.reserve_auto_close().unwrap())
        .collect::<Vec<_>>();

    let route = {
        let sender = coordinator.container_lifecycle_sender();
        tokio::spawn(async move { sender.route_lost(session.id, 31).await })
    };
    let watchdog = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
    };
    let cleanup = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.background_destroy(session.id).await })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        while coordinator.inner.critical_keys.lock().unwrap().len() != 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all critical kinds register independently at full capacity");

    assert!(matches!(
        coordinator
            .container_lifecycle_sender()
            .route_lost(session.id, 32)
            .await
            .unwrap(),
        CriticalAdmissionOutcome::AlreadyPending
    ));
    assert!(matches!(
        coordinator
            .watchdog_resource_kill(session.id)
            .await
            .unwrap(),
        WatchdogKillOutcome::AlreadyInFlight
    ));
    assert!(matches!(
        coordinator.background_destroy(session.id).await.unwrap(),
        CriticalAdmissionOutcome::AlreadyPending
    ));

    coordinator
        .close_and_join_with_budget(Duration::from_millis(25))
        .await;
    guard.finish();
    drop(reservations);
    for error in [
        route.await.unwrap().unwrap_err(),
        watchdog.await.unwrap().unwrap_err(),
        cleanup.await.unwrap().unwrap_err(),
    ] {
        assert_eq!(error, SelectionCoordinatorError::Unavailable.to_string());
    }
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    assert!(coordinator.inner.worker.lock().unwrap().is_none());
}

// #1151 (S1) - an absent session UUID reports NoPublicSession, distinctly from the
// in-flight case, and does so before any admission is reserved or job enqueued.
#[tokio::test]
async fn watchdog_kill_reports_no_public_session_for_absent_uuid() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(pty)
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build no-public-session app");
    coordinator.start(app.handle().clone()).unwrap();
    let restore = coordinator.submit_restore_first().await.unwrap();

    // Every envelope slot is taken and the restore barrier is still held, so ANY
    // attempt to reserve a critical admission would park indefinitely. Returning
    // promptly is therefore proof that the absent-session check runs before the
    // reservation and before a CoordinatorJob::ResourceKill can be enqueued.
    let mut reservations = Vec::new();
    while let Ok(reservation) = coordinator.reserve_auto_close() {
        reservations.push(reservation);
    }
    assert!(!reservations.is_empty());

    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        coordinator.watchdog_resource_kill(Uuid::new_v4()),
    )
    .await
    .expect("absent-session watchdog kill returns without reserving admission")
    .unwrap();
    assert!(matches!(outcome, WatchdogKillOutcome::NoPublicSession));
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());

    restore.finish();
    coordinator.close_and_join().await;
    drop(reservations);
}

// #1151 (S2) - with the WatchdogKill critical key held for a PUBLIC uuid, the second
// call reports AlreadyInFlight, never NoPublicSession. The orphan fallback keys off
// the second variant only, so conflating them would re-enter a live cleanup.
#[tokio::test]
async fn watchdog_kill_reports_already_in_flight_when_key_held() {
    use crate::pty::backend::SessionBackendKind;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/watchdog-in-flight".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(pty)
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build already-in-flight app");
    coordinator.start(app.handle().clone()).unwrap();
    let guard = coordinator.submit_restore_first().await.unwrap();
    let reservations = (0..COORDINATOR_QUEUE_CAPACITY)
        .map(|_| coordinator.reserve_auto_close().unwrap())
        .collect::<Vec<_>>();

    let waiter = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        while !coordinator
            .critical_key_registered_for_test(session.id, CriticalAdmissionKind::WatchdogKill)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first watchdog kill registers its critical key");

    assert!(matches!(
        coordinator
            .watchdog_resource_kill(session.id)
            .await
            .unwrap(),
        WatchdogKillOutcome::AlreadyInFlight
    ));

    coordinator
        .close_and_join_with_budget(Duration::from_millis(25))
        .await;
    guard.finish();
    drop(reservations);
    assert_eq!(
        waiter.await.unwrap().unwrap_err(),
        SelectionCoordinatorError::Unavailable.to_string()
    );
}

// #1151 (S3) - the absent-session outcomes of the two SIBLING entry points are
// deliberately inconsistent with each other and are left exactly as they are. A
// dedicated WatchdogKillOutcome exists so nobody normalizes all three at once.
#[tokio::test]
async fn absent_session_outcomes_for_route_loss_and_background_destroy_are_unchanged() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(pty)
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build absent-session sibling app");
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();

    assert!(matches!(
        coordinator
            .container_lifecycle_sender()
            .route_lost(Uuid::new_v4(), 7)
            .await
            .unwrap(),
        CriticalAdmissionOutcome::Completed(())
    ));
    assert!(matches!(
        coordinator
            .background_destroy(Uuid::new_v4())
            .await
            .unwrap(),
        CriticalAdmissionOutcome::AlreadyPending
    ));
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    coordinator.close_and_join().await;
}
