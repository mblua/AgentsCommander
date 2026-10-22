use super::container_shutdown::{
    assert_pending_container_shutdown_waits_for_stop,
    assert_real_pending_container_start_shutdown_waits_for_stop,
};
use super::*;

#[tokio::test]
async fn shutdown_drain_runs_common_resource_rollback_for_finalize_and_drop_jobs() {
    use crate::pty::backend::{PtyBackend, SessionBackendKind};

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build shutdown-drain app");
    coordinator.start(app.handle().clone()).unwrap();
    let guard = coordinator.submit_restore_first().await.unwrap();
    let manager_handle = manager.read().await.clone();

    let mut finalizer_ticket = coordinator
        .reserve_create(TrustedCreateIntent::Background)
        .await
        .unwrap();
    let finalizer_session = manager_handle
        .create_pending_session(
            &mut finalizer_ticket,
            "shell".to_string(),
            Vec::new(),
            "C:/shutdown-finalizer".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    backend.set_live(finalizer_session.id, true);
    pty.lock()
        .unwrap()
        .record_route(finalizer_session.id, SessionBackendKind::LocalProcess);

    let mut drop_ticket = coordinator
        .reserve_create(TrustedCreateIntent::Background)
        .await
        .unwrap();
    let drop_session = manager_handle
        .create_pending_session(
            &mut drop_ticket,
            "shell".to_string(),
            Vec::new(),
            "C:/shutdown-drop".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    backend.set_live(drop_session.id, true);
    pty.lock()
        .unwrap()
        .record_route(drop_session.id, SessionBackendKind::LocalProcess);

    let finalizer = tokio::spawn(async move { finalizer_ticket.finalize(Vec::new()).await });
    drop(drop_ticket);
    tokio::task::yield_now().await;
    let close = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.close_and_join().await })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        while !coordinator.inner.shutdown.is_cancelled() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("shutdown signal becomes visible");
    guard.finish();
    close.await.expect("join close task");

    assert_eq!(
        finalizer
            .await
            .expect("join finalizer caller")
            .expect_err("queued finalizer is unavailable during shutdown"),
        SelectionCoordinatorError::Unavailable.to_string()
    );
    assert_eq!(backend.kill_count(finalizer_session.id), 1);
    assert_eq!(backend.kill_count(drop_session.id), 1);
    assert!(!backend.has_session(finalizer_session.id));
    assert!(!backend.has_session(drop_session.id));
    assert!(!pty.lock().unwrap().has_session(finalizer_session.id));
    assert!(!pty.lock().unwrap().has_session(drop_session.id));
    let aggregate = manager_handle.aggregate_snapshot().await;
    assert!(aggregate.pending_ids.is_empty());
    assert!(aggregate.sessions.is_empty());
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    assert!(coordinator.inner.worker.lock().unwrap().is_none());
}

#[tokio::test]
async fn normal_shutdown_drain_joins_pending_container_stop_exactly_once() {
    assert_pending_container_shutdown_waits_for_stop(false).await;
}

#[tokio::test]
async fn normal_shutdown_drain_joins_real_pending_container_start_and_stop_exactly_once() {
    assert_real_pending_container_start_shutdown_waits_for_stop(
        false,
        false,
        CanceledStartStopOutcome::Success,
    )
    .await;
}

#[tokio::test]
async fn cancelled_and_panicked_post_spawn_creates_roll_back_without_ghost_events() {
    use crate::pty::backend::SessionBackendKind;
    use tauri::Listener;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build cancellation rollback app");
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();
    let manager_handle = manager.read().await.clone();
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for event_name in ["session_created", "session_switched"] {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |_| {
            let _ = events_tx.send(event_name);
        });
    }

    let mut cancelled_ticket = coordinator
        .reserve_create(TrustedCreateIntent::Background)
        .await
        .unwrap();
    let cancelled_session = manager_handle
        .create_pending_session(
            &mut cancelled_ticket,
            "shell".to_string(),
            Vec::new(),
            "C:/cancelled-create".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    backend.set_live(cancelled_session.id, true);
    pty.lock()
        .unwrap()
        .record_route(cancelled_session.id, SessionBackendKind::LocalProcess);
    let (cancel_started, cancel_started_rx) = oneshot::channel();
    let cancelled_task = tokio::spawn(async move {
        let ticket = cancelled_ticket;
        let _ = cancel_started.send(());
        std::future::pending::<()>().await;
        drop(ticket);
    });
    cancel_started_rx.await.unwrap();
    cancelled_task.abort();
    assert!(cancelled_task.await.unwrap_err().is_cancelled());

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let aggregate = manager_handle.aggregate_snapshot().await;
            if !aggregate.pending_ids.contains(&cancelled_session.id)
                && !pty.lock().unwrap().has_session(cancelled_session.id)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled create rollback completes");
    assert_eq!(backend.kill_count(cancelled_session.id), 1);

    let mut panicked_ticket = coordinator
        .reserve_create(TrustedCreateIntent::Background)
        .await
        .unwrap();
    let panicked_session = manager_handle
        .create_pending_session(
            &mut panicked_ticket,
            "shell".to_string(),
            Vec::new(),
            "C:/panicked-create".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    backend.set_live(panicked_session.id, true);
    pty.lock()
        .unwrap()
        .record_route(panicked_session.id, SessionBackendKind::LocalProcess);
    let panicked_task = tokio::spawn(async move {
        let _ticket = panicked_ticket;
        panic!("synthetic create task panic");
    });
    assert!(panicked_task.await.unwrap_err().is_panic());

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let aggregate = manager_handle.aggregate_snapshot().await;
            if !aggregate.pending_ids.contains(&panicked_session.id)
                && !pty.lock().unwrap().has_session(panicked_session.id)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("panicked create rollback completes");
    assert_eq!(backend.kill_count(panicked_session.id), 1);
    assert!(manager_handle
        .aggregate_snapshot()
        .await
        .sessions
        .is_empty());
    assert!(events_rx.try_recv().is_err());
    assert_eq!(
        coordinator.inner.create_tickets.available_permits(),
        CREATE_TICKET_CAPACITY
    );
    assert_eq!(
        coordinator.inner.admission.available_permits(),
        COORDINATOR_ADMISSION_CAPACITY
    );
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn capped_shutdown_aborts_a_held_worker_then_cleans_pending_resources_and_keys() {
    use crate::pty::backend::{PtyBackend, SessionBackendKind};

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build capped shutdown app");
    coordinator.start(app.handle().clone()).unwrap();
    let guard = coordinator.submit_restore_first().await.unwrap();
    let manager_handle = manager.read().await.clone();

    let mut ticket = coordinator
        .reserve_create(TrustedCreateIntent::Background)
        .await
        .unwrap();
    let pending = manager_handle
        .create_pending_session(
            &mut ticket,
            "shell".to_string(),
            Vec::new(),
            "C:/aborted-shutdown".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    backend.set_live(pending.id, true);
    pty.lock()
        .unwrap()
        .record_route(pending.id, SessionBackendKind::LocalProcess);
    let finalizer = tokio::spawn(async move { ticket.finalize(Vec::new()).await });
    let route_waiter = {
        let sender = coordinator.container_lifecycle_sender();
        tokio::spawn(async move { sender.route_lost(pending.id, 9).await })
    };
    await_state_eq(
        "route-loss key is registered behind the held worker",
        Duration::from_secs(1),
        || coordinator.inner.critical_keys.lock().unwrap().is_empty(),
        false,
        String::new,
    )
    .await;

    let started = std::time::Instant::now();
    coordinator
        .close_and_join_with_budget(Duration::from_millis(25))
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "capped coordinator shutdown must not wait for the held transaction"
    );
    guard.finish();

    assert_eq!(
        finalizer.await.unwrap().unwrap_err(),
        SelectionCoordinatorError::Unavailable.to_string()
    );
    assert_eq!(
        route_waiter.await.unwrap().unwrap_err(),
        SelectionCoordinatorError::Unavailable.to_string()
    );
    await_state_eq(
        "capped close cleanup kills the pending session",
        Duration::from_secs(1),
        || {
            (
                backend.kill_count(pending.id),
                backend.has_session(pending.id),
                pty.lock().unwrap().has_session(pending.id),
            )
        },
        (1, false, false),
        String::new,
    )
    .await;
    assert_eq!(backend.kill_count(pending.id), 1);
    assert!(!backend.has_session(pending.id));
    assert!(!pty.lock().unwrap().has_session(pending.id));
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if manager_handle
                .aggregate_snapshot()
                .await
                .pending_ids
                .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("retained pending-create rollback reaches terminal manager state");
    let aggregate = manager_handle.aggregate_snapshot().await;
    assert!(aggregate.pending_ids.is_empty());
    assert!(aggregate.sessions.is_empty());
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    assert!(coordinator.inner.worker.lock().unwrap().is_none());
}

#[tokio::test]
async fn capped_shutdown_abort_joins_pending_container_stop_exactly_once() {
    // The restore barrier forces capped close; the later sweep owns immediate cleanup.
    // Worker counters are accounting, not proof of physical joins or deadline races.
    assert_pending_container_shutdown_waits_for_stop(true).await;
}

#[tokio::test]
async fn capped_shutdown_abort_joins_real_pending_container_start_and_stop_exactly_once() {
    assert_real_pending_container_start_shutdown_waits_for_stop(
        true,
        false,
        CanceledStartStopOutcome::Success,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_real_start_worker_spawn_failure_falls_back_without_losing_handle() {
    assert_real_pending_container_start_shutdown_waits_for_stop(
        true,
        true,
        CanceledStartStopOutcome::Success,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_real_start_stop_error_retains_and_retries_the_deterministic_handle() {
    assert_real_pending_container_start_shutdown_waits_for_stop(
        true,
        true,
        CanceledStartStopOutcome::Error,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_real_start_stop_panic_retains_and_retries_the_deterministic_handle() {
    assert_real_pending_container_start_shutdown_waits_for_stop(
        true,
        true,
        CanceledStartStopOutcome::Panic,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_handle_cleanup_worker_spawn_failure_falls_back_without_panic() {
    use crate::pty::backend::SessionBackendKind;
    use crate::pty::container_backend::ContainerTransportBackend;
    use crate::pty::container_runtime::ContainerRuntimeHandle;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (stop_started, stop_started_rx) = oneshot::channel();
    let runtime = Arc::new(GatedStopRuntime {
        stop_started: Mutex::new(Some(stop_started)),
        stop_calls: AtomicUsize::new(0),
        stop_hold: Duration::from_secs(30),
        active_stops: AtomicUsize::new(0),
        deadline_seen: AtomicBool::new(false),
    });
    let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let container_backend = Arc::new(ContainerTransportBackend::with_runtime(
        output_senders,
        idle_detector,
        None,
        None,
        runtime.clone(),
        None,
    ));
    container_backend.inject_shutdown_worker_spawn_failure_for_test();
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
        Arc::new(LifecycleTestBackend::default()),
        container_backend.clone(),
    )));
    let session_id = Uuid::new_v4();
    let _transport_receiver =
        container_backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id,
            container_id: format!("container-{session_id}"),
        });
    pty.lock()
        .unwrap_or_else(|error| error.into_inner())
        .record_route(session_id, SessionBackendKind::ContainerTransport);

    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build installed-handle admission-failure app");
    coordinator
        .start(app.handle().clone())
        .expect("start installed-handle admission-failure coordinator");
    coordinator
        .submit_restore_first()
        .await
        .expect("submit installed-handle restore")
        .finish();

    let kill_pty = Arc::clone(&pty);
    let kill = tokio::task::spawn_blocking(move || {
        match catch_unwind(AssertUnwindSafe(|| {
            kill_pty
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .kill(session_id)
        })) {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(_) => Err("cleanup admission failure panicked".to_string()),
        }
    });
    tokio::time::timeout(Duration::from_secs(1), stop_started_rx)
        .await
        .expect("installed-handle fallback stop starts")
        .expect("installed-handle fallback stop witness is delivered");
    assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 1);
    assert!(!container_backend.contains_transport_state_for_test(session_id));
    assert_eq!(
        container_backend.shutdown_work_state_for_test(),
        (false, 1, 1)
    );
    assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);

    let close_budget = Duration::from_millis(200);
    let close_started = Instant::now();
    coordinator.close_and_join_with_budget(close_budget).await;
    assert!(
        close_started.elapsed() <= close_budget + Duration::from_millis(550),
        "installed-handle close must consume the shared deadline"
    );
    tokio::time::timeout(Duration::from_secs(1), kill)
        .await
        .expect("installed-handle fallback kill finishes after shutdown cancellation")
        .expect("join installed-handle fallback kill")
        .expect("cleanup admission failure must not panic or lose the stop");

    let manager_handle = manager.read().await.clone();
    let aggregate = manager_handle.aggregate_snapshot().await;
    assert!(aggregate.sessions.is_empty());
    assert!(aggregate.pending_ids.is_empty());
    assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 0);
    assert!(runtime.deadline_seen.load(Ordering::SeqCst));
    assert!(!container_backend.contains_transport_state_for_test(session_id));
    assert!(!pty
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .has_session(session_id));
    await_state_eq(
        "installed-handle cleanup drains shutdown work",
        Duration::from_secs(1),
        || container_backend.shutdown_observation_for_test(),
        (true, 0, 0, 0),
        String::new,
    )
    .await;
    assert_eq!(
        container_backend.shutdown_work_state_for_test(),
        (true, 0, 0)
    );
    assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);
    pty.lock()
        .unwrap_or_else(|error| error.into_inner())
        .stop_all_started_containers_blocking(Duration::from_secs(1));
    assert_eq!(
        runtime.stop_calls.load(Ordering::SeqCst),
        1,
        "later global sweep must not repeat the recovered stop"
    );
}

#[tokio::test]
async fn joined_worker_retains_no_coordinator_or_extra_pty_owner() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let weak_inner = Arc::downgrade(&coordinator.inner);
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(crate::config::settings::new_settings_state(
            crate::config::settings::AppSettings::default(),
        ))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build retention test app");
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();
    // Real entry admission, worker return, source lease release and result
    // publication must reclaim capacity before the next accepted entry.
    for _ in 0..(COORDINATOR_ADMISSION_CAPACITY + 1) {
        let error = coordinator
            .restart_lifecycle(
                app.handle().clone(),
                crate::commands::session::RestartJobRequest {
                    session_id: Uuid::new_v4(),
                    agent_id: None,
                    requested_profile: None,
                    skip_auto_resume: None,
                    activate_after: false,
                    intent: TrustedRestartIntent::Background,
                    communication_override: None,
                    enforcement:
                        crate::config::sessions_persistence::default_creation_gate_enforcement(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, "Session not found");
        assert!(coordinator
            .inner
            .retained_replacements
            .lock()
            .unwrap()
            .entries
            .is_empty());
    }
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let pending = {
        let coordinator = coordinator.clone();
        let app = app.handle().clone();
        tokio::spawn(async move {
            coordinator.restart_lifecycle(app, crate::commands::session::RestartJobRequest {
                session_id: Uuid::new_v4(), agent_id: None, requested_profile: None,
                skip_auto_resume: None, activate_after: false,
                intent: TrustedRestartIntent::Background, communication_override: None,
                enforcement:
                    crate::config::sessions_persistence::default_creation_gate_enforcement(),
            }).await
        })
    };
    let owner = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let owner = coordinator
                .inner
                .retained_replacements
                .lock()
                .unwrap()
                .entries
                .values()
                .next()
                .cloned();
            if let Some(owner) = owner {
                let queued = owner.state.lock().unwrap().stage == ReplacementStage::Queued;
                if queued {
                    break owner;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("real worker reaches the current-authority U wait");
    let mut unsafe_before = Vec::new();
    coordinator.inspect_retained_entries(&mut unsafe_before);
    assert!(!unsafe_before.is_empty(), "active entry is not terminal");
    coordinator.seal_retained_entries();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    drop(permit);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if owner.state.lock().unwrap().stage == ReplacementStage::EntryTerminal {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("caller loss does not stop the accepted core");
    assert_eq!(
        coordinator
            .inner
            .retained_replacements
            .lock()
            .unwrap()
            .entries
            .len(),
        1,
        "seal prohibits removal of the now verified terminal entry"
    );
    let mut unsafe_after = Vec::new();
    coordinator.inspect_retained_entries(&mut unsafe_after);
    assert!(
        unsafe_after.is_empty(),
        "real returned/released entry is safe after seal: {unsafe_after:?}"
    );
    drop(owner);
    coordinator.close_and_join().await;
    assert!(coordinator.inner.worker.lock().unwrap().is_none());

    drop(coordinator);
    assert!(weak_inner.upgrade().is_none());
    assert_eq!(Arc::strong_count(&manager), 2);
    assert_eq!(Arc::strong_count(&pty), 2);
    assert_eq!(Arc::strong_count(&backend), 2);
    drop(app);
}
