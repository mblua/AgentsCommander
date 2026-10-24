use super::*;

// Grinch HIGH (Step 7): a manual kill that races a concurrent watchdog kill must not
// mistake the transient `Terminating` for a verified success, and must settle to the
// real outcome. Proves both halves: (1) while a watchdog kill owns `Terminating`, a
// direct kill_group returns `Terminating` (NOT finalizable); (2) verify_kill_settled
// waits it out and returns the settled `Terminated` (finalizable).
#[tokio::test]
async fn force_kill_settles_past_concurrent_terminating() {
    let root = ProcessIdentity {
        pid: 4321,
        creation_time_100ns: 99,
    };
    let backend = Arc::new(GatedBackend::new(root));
    let state = ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);
    let id = Uuid::new_v4();
    register_running(&state, id, root);

    // Safety net: release the gate on scope exit so a panicking assertion below can
    // never leave the blocked watchdog thread parked on the Condvar. `release` is
    // idempotent, so the timed releaser on the happy path is unaffected.
    struct ReleaseOnDrop(Arc<GatedBackend>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let _release_guard = ReleaseOnDrop(backend.clone());

    // Arm so the next observe_tree (the watchdog kill) blocks in `Terminating`.
    backend.armed.store(true, Ordering::SeqCst);
    let watchdog = {
        let s = state.clone();
        std::thread::spawn(move || s.kill_group(id, ResourceKillReason::Watchdog))
    };
    // Wait until the watchdog kill has parked the group in `Terminating`.
    let mut waited_ms = 0;
    while !backend.entered.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
        waited_ms += 5;
        assert!(waited_ms < 2000, "watchdog kill never entered Terminating");
    }

    // (1) A direct kill now hits the `Terminating` early-return: not finalizable.
    let racing = {
        let s = state.clone();
        tokio::task::spawn_blocking(move || s.kill_group(id, ResourceKillReason::User))
            .await
            .unwrap()
            .unwrap()
    };
    assert_eq!(racing.state, ResourceGroupState::Terminating);
    assert!(!racing.finalized);
    assert!(!should_finalize_kill(racing.state));

    // (2) Release the watchdog kill shortly, then verify_kill_settled must wait out
    // `Terminating` and return the settled `Terminated`.
    let releaser = {
        let b = backend.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            b.release();
        })
    };
    let settled = verify_kill_settled(
        Arc::new(state.clone()),
        id,
        ResourceKillReason::User,
        Duration::from_millis(2000),
        Duration::from_millis(25),
    )
    .await
    .unwrap();
    assert_eq!(settled.state, ResourceGroupState::Terminated);
    assert!(should_finalize_kill(settled.state));

    watchdog.join().unwrap().unwrap();
    releaser.join().unwrap();
}

async fn assert_serialized_resource_kills(watchdog_first: bool) {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/resource-serialization".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let pty_backend = Arc::new(ResourcePtyBackend::default());
    pty_backend.set_live(session.id);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);

    let order_offset = if watchdog_first { 1 } else { 0 };
    let root = ProcessIdentity {
        pid: 5100 + order_offset,
        creation_time_100ns: 700 + u64::from(order_offset),
    };
    let process_backend = Arc::new(GatedBackend::new(root));
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    register_running(monitor.as_ref(), session.id, root);
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(Arc::clone(&monitor))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build serialized resource-kill app");
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();
    let manager_handle = manager.read().await.clone();
    let before_revision = manager_handle.selection_payload().await.revision();
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for event_name in ["session_destroyed", "session_created", "session_switched"] {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |_| {
            let _ = events_tx.send(event_name);
        });
    }

    struct ReleaseOnDrop(Arc<GatedBackend>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let _release_guard = ReleaseOnDrop(process_backend.clone());
    process_backend.armed.store(true, Ordering::SeqCst);

    if watchdog_first {
        let watchdog = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while !process_backend.entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("watchdog resource kill enters the backend barrier");
        let mut user = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                coordinator
                    .resource_kill(session.id, TrustedResourceIntent::User)
                    .await
            })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut user)
                .await
                .is_err(),
            "user finalizer must queue behind the held watchdog transaction"
        );
        process_backend.release();
        let watchdog_result = watchdog.await.unwrap().unwrap();
        assert!(matches!(
            watchdog_result,
            WatchdogKillOutcome::Completed(ref result) if result.finalized
        ));
        assert!(user.await.unwrap().unwrap().finalized);
    } else {
        let user = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                coordinator
                    .resource_kill(session.id, TrustedResourceIntent::User)
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while !process_backend.entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("user resource kill enters the backend barrier");
        let mut watchdog = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut watchdog)
                .await
                .is_err(),
            "watchdog finalizer must queue behind the held user transaction"
        );
        process_backend.release();
        assert!(user.await.unwrap().unwrap().finalized);
        let watchdog_result = watchdog.await.unwrap().unwrap();
        assert!(matches!(
            watchdog_result,
            WatchdogKillOutcome::Completed(ref result) if result.finalized
        ));
    }

    let row = manager_handle.get_session(session.id).await.unwrap();
    assert_eq!(row.status, SessionStatus::Exited(0));
    let selection = manager_handle.selection_payload().await;
    assert_eq!(selection.mode(), SelectionMode::None);
    assert_eq!(selection.id(), None);
    assert_eq!(selection.source(), SelectionSource::ResourceMonitor);
    assert_eq!(selection.user_initiated(), !watchdog_first);
    assert_eq!(selection.revision(), before_revision + 1);
    assert_eq!(
        (0..3)
            .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect::<Vec<_>>(),
        vec!["session_destroyed", "session_created", "session_switched"]
    );
    assert!(events_rx.try_recv().is_err());
    assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 1);
    assert!(!pty.lock().unwrap().has_session(session.id));
    coordinator.close_and_join().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_and_watchdog_resource_finalizers_serialize_in_both_orders() {
    assert_serialized_resource_kills(false).await;
    assert_serialized_resource_kills(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_queue_watchdog_waiter_deduplicates_then_runs_one_whole_finalizer() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/watchdog-capacity".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let pty_backend = Arc::new(ResourcePtyBackend::default());
    pty_backend.set_live(session.id);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let root = ProcessIdentity {
        pid: 5300,
        creation_time_100ns: 900,
    };
    let process_backend = Arc::new(GatedBackend::new(root));
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    register_running(monitor.as_ref(), session.id, root);
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(monitor)
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build full-queue watchdog app");
    coordinator.start(app.handle().clone()).unwrap();
    let restore = coordinator.submit_restore_first().await.unwrap();
    let mut reservations = (0..64)
        .map(|_| coordinator.reserve_auto_close().unwrap())
        .collect::<Vec<_>>();
    let before_revision = manager.read().await.selection_payload().await.revision();
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for event_name in ["session_destroyed", "session_created", "session_switched"] {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |_| {
            let _ = events_tx.send(event_name);
        });
    }
    struct ReleaseOnDrop(Arc<GatedBackend>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let _release_guard = ReleaseOnDrop(process_backend.clone());
    process_backend.armed.store(true, Ordering::SeqCst);

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
    .expect("watchdog waiter registers while logical capacity is full");
    assert!(matches!(
        coordinator
            .watchdog_resource_kill(session.id)
            .await
            .unwrap(),
        WatchdogKillOutcome::AlreadyInFlight
    ));

    drop(reservations.pop());
    restore.finish();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !process_backend.entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capacity release fairly admits one whole watchdog finalizer");
    assert_eq!(
        manager.read().await.selection_payload().await.revision(),
        before_revision
    );
    assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 1);
    process_backend.release();
    let result = waiter.await.unwrap().unwrap();
    assert!(matches!(
        result,
        WatchdogKillOutcome::Completed(ref result) if result.finalized
    ));
    drop(reservations);

    let selection = manager.read().await.selection_payload().await;
    assert_eq!(selection.revision(), before_revision + 1);
    assert_eq!(selection.mode(), SelectionMode::None);
    assert!(!selection.user_initiated());
    assert_eq!(
        (0..3)
            .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect::<Vec<_>>(),
        vec!["session_destroyed", "session_created", "session_switched"]
    );
    assert!(events_rx.try_recv().is_err());
    assert!(matches!(
        coordinator
            .watchdog_resource_kill(session.id)
            .await
            .unwrap(),
        WatchdogKillOutcome::Completed(_)
    ));
    assert!(events_rx.try_recv().is_err());
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn full_coordinator_rejects_user_resource_kill_before_any_side_effect() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/resource-busy".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let pty_backend = Arc::new(ResourcePtyBackend::default());
    pty_backend.set_live(session.id);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let root = ProcessIdentity {
        pid: 5200,
        creation_time_100ns: 800,
    };
    let process_backend = Arc::new(GatedBackend::new(root));
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    register_running(monitor.as_ref(), session.id, root);
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(monitor)
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build busy resource-kill app");
    coordinator.start(app.handle().clone()).unwrap();
    let guard = coordinator.submit_restore_first().await.unwrap();
    let reservations = (0..64)
        .map(|_| coordinator.reserve_auto_close().unwrap())
        .collect::<Vec<_>>();
    let before = manager.read().await.selection_payload().await;

    assert_eq!(
        coordinator
            .resource_kill(session.id, TrustedResourceIntent::User)
            .await
            .unwrap_err(),
        "selectionCoordinatorBusy"
    );
    assert!(!process_backend.entered.load(Ordering::SeqCst));
    assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 0);
    assert_eq!(pty_backend.kill_count.load(Ordering::SeqCst), 0);
    let after = manager.read().await.selection_payload().await;
    assert_eq!(after.revision(), before.revision());
    assert_eq!(after.id(), before.id());
    assert_eq!(
        manager
            .read()
            .await
            .get_session(session.id)
            .await
            .unwrap()
            .status,
        SessionStatus::Active
    );

    drop(reservations);
    guard.finish();
    coordinator.close_and_join().await;
}
