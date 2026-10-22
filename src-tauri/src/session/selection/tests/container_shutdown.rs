use super::*;

// Proves retained pending-handle ownership and exactly-once cleanup after close.
// Immediate stop does not cover near-deadline success, blocked stops, or physical joins.
pub(super) async fn assert_pending_container_shutdown_waits_for_stop(capped: bool) {
    use crate::pty::backend::SessionBackendKind;
    use crate::pty::container_backend::ContainerTransportBackend;
    use crate::pty::container_runtime::ContainerRuntimeHandle;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (stop_started, stop_started_rx) = oneshot::channel();
    let close_budget = Duration::from_millis(200);
    let runtime = Arc::new(GatedStopRuntime {
        stop_started: Mutex::new(Some(stop_started)),
        stop_calls: AtomicUsize::new(0),
        stop_hold: Duration::ZERO,
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
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
        Arc::new(LifecycleTestBackend::default()),
        container_backend.clone(),
    )));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build pending-container shutdown app");
    let mut restore_guard = None;
    let manager_handle = manager.read().await.clone();
    let setup = async {
        coordinator
            .start(app.handle().clone())
            .map_err(|error| error.to_string())?;
        restore_guard = Some(coordinator.submit_restore_first().await?);
        let mut ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .map_err(|error| error.to_string())?;
        let pending = manager_handle
            .create_pending_session(
                &mut ticket,
                "container".to_string(),
                Vec::new(),
                "C:/pending-container-shutdown".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::ContainerTransport,
            )
            .await?;
        Ok::<_, String>((ticket, pending))
    }
    .await;
    let (ticket, pending) = match setup {
        Ok(ready) => ready,
        Err(error) => {
            if let Some(guard) = restore_guard.take() {
                guard.finish();
            }
            let cleanup =
                tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join()).await;
            panic!("pending-container setup failed: {error}; coordinator cleanup={cleanup:?}; a cleanup timeout leaves worker ownership unresolved");
        }
    };
    let _transport_receiver =
        container_backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id: pending.id,
            container_id: format!("container-{}", pending.id),
        });
    pty.lock()
        .unwrap_or_else(|error| error.into_inner())
        .record_route(pending.id, SessionBackendKind::ContainerTransport);

    let mut finalizer = tokio::spawn(async move { ticket.finalize(Vec::new()).await });
    let close_started = Instant::now();
    let mut close = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move {
            if capped {
                coordinator.close_and_join_with_budget(close_budget).await
            } else {
                coordinator.close_and_join().await
            }
        })
    };
    let shutdown_signal = tokio::time::timeout(Duration::from_secs(1), async {
        while !coordinator.inner.shutdown.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    if !capped || shutdown_signal.is_err() {
        if let Some(guard) = restore_guard.take() {
            guard.finish();
        }
    }

    let close_result = tokio::time::timeout(Duration::from_secs(2), &mut close).await;
    let close_elapsed = close_started.elapsed();
    let close_bound = if capped {
        close_budget + Duration::from_millis(550)
    } else {
        Duration::from_secs(1)
    };
    if let Some(guard) = restore_guard.take() {
        guard.finish();
    }
    // Borrow task handles so timeout cannot silently detach fixture-owned work.
    let close_cleanup = if close_result.is_err() {
        let cleanup = tokio::time::timeout(Duration::from_secs(5), &mut close).await;
        if cleanup.is_err() {
            close.abort();
            let aborted = tokio::time::timeout(Duration::from_secs(5), &mut close).await;
            eprintln!("coordinator cleanup exceeded five seconds; abort join={aborted:?}; worker ownership unresolved");
        }
        Some(cleanup)
    } else {
        None
    };
    let finalizer_result = tokio::time::timeout(Duration::from_secs(5), &mut finalizer).await;
    if finalizer_result.is_err() {
        finalizer.abort();
        let aborted = tokio::time::timeout(Duration::from_secs(5), &mut finalizer).await;
        eprintln!("finalizer cleanup exceeded five seconds; abort join={aborted:?}");
    }
    let pre_stop_calls = runtime.stop_calls.load(Ordering::SeqCst);
    let pre_active_stops = runtime.active_stops.load(Ordering::SeqCst);
    let pre_transport = container_backend.contains_transport_state_for_test(pending.id);
    let pre_detached = container_backend.detached_cleanup_count_for_test();
    let pre_retained = container_backend.retained_runtime_cleanup_sessions_for_test();
    let pre_contexts = container_backend.retained_cleanup_contexts_for_test();
    let aggregate = manager_handle.aggregate_snapshot().await;
    let pre_route = pty
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .has_session(pending.id);

    // Collect both snapshots and finish the single authorized sweep before assertions.
    let global_report = pty
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .stop_all_started_containers_blocking(Duration::from_secs(1));
    let stopped_signal = tokio::time::timeout(Duration::from_secs(1), stop_started_rx).await;
    await_state_eq(
        "global sweep stops the retained handle exactly once",
        Duration::from_secs(1),
        || runtime.stop_calls.load(Ordering::SeqCst),
        1,
        String::new,
    )
    .await;
    await_state_eq(
        "retained container stop is no longer active",
        Duration::from_secs(1),
        || runtime.active_stops.load(Ordering::SeqCst),
        0,
        String::new,
    )
    .await;
    await_state_eq(
        "no shutdown worker remains after the sweep",
        Duration::from_secs(1),
        || container_backend.shutdown_observation_for_test().3,
        0,
        String::new,
    )
    .await;
    await_state_eq(
        "retained container stop observed its deadline",
        Duration::from_secs(1),
        || runtime.deadline_seen.load(Ordering::SeqCst),
        true,
        String::new,
    )
    .await;
    await_state_eq(
        "pending cleanup ownership is released after the sweep",
        Duration::from_secs(1),
        || {
            container_backend
                .retained_cleanup_sessions_for_test()
                .is_empty()
        },
        true,
        String::new,
    )
    .await;
    let stop_calls = runtime.stop_calls.load(Ordering::SeqCst);
    let active_stops = runtime.active_stops.load(Ordering::SeqCst);
    let worker_count = container_backend.shutdown_worker_count_for_test();
    let deadline_seen = runtime.deadline_seen.load(Ordering::SeqCst);
    let retained = container_backend.retained_cleanup_sessions_for_test();
    eprintln!("pending-container capped={capped} close={close_result:?} close_cleanup={close_cleanup:?} terminal={} stops={stop_calls} active={active_stops} worker_accounting={worker_count} retained={retained:?}", global_report.terminal);
    shutdown_signal.expect("pending-container shutdown signal becomes visible");
    close_result
        .expect("coordinator close obeys the shared shutdown deadline")
        .expect("join coordinator close task");
    assert!(
        close_elapsed <= close_bound,
        "coordinator close elapsed {close_elapsed:?}, bound {close_bound:?}"
    );
    assert_eq!(
        finalizer_result
            .expect("pending-container finalizer joins within five seconds")
            .expect("join pending-container finalizer")
            .expect_err("shutdown finalizer returns unavailable"),
        SelectionCoordinatorError::Unavailable.to_string()
    );
    assert_eq!(pre_stop_calls, 0);
    assert_eq!(pre_active_stops, 0);
    assert!(!pre_transport);
    assert_eq!(pre_detached, 0);
    assert_eq!(
        pre_retained,
        vec![pending.id],
        "post-seal pending cleanup must remain owned for the authorized global sweep: {:?}",
        pre_contexts
    );
    assert!(aggregate.pending_ids.is_empty());
    assert!(aggregate.sessions.is_empty());
    assert!(!pre_route);
    assert!(
        global_report.terminal,
        "retained={:?}",
        global_report.retained
    );
    stopped_signal
        .expect("global sweep invokes the retained container stop")
        .expect("container stop-start signal is delivered");
    assert_eq!(
        stop_calls, 1,
        "the single production global sweep must stop the retained handle exactly once"
    );
    assert_eq!(active_stops, 0);
    // Accounting evidence only; zero does not prove physical worker thread joins.
    assert_eq!(worker_count, 0);
    assert!(deadline_seen);
    assert!(retained.is_empty());
}

pub(super) async fn assert_real_pending_container_start_shutdown_waits_for_stop(
    capped: bool,
    fail_worker_spawn: bool,
    first_stop_outcome: CanceledStartStopOutcome,
) {
    use crate::pty::backend::SessionBackendKind;
    use crate::pty::container_backend::ContainerTransportBackend;
    use crate::pty::container_tokens::ContainerApiTokenManager;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let root_dir = tempfile::TempDir::new().expect("create container root directory");
    let root = root_dir.path().to_string_lossy().into_owned();
    let (start_started, start_started_rx) = oneshot::channel();
    let (stop_started, stop_started_rx) = oneshot::channel();
    let close_budget = Duration::from_millis(200);
    let runtime = Arc::new(GatedStartStopRuntime {
        start_started: Mutex::new(Some(start_started)),
        stop_started: Mutex::new(Some(stop_started)),
        start_calls: AtomicUsize::new(0),
        stop_calls: AtomicUsize::new(0),
        stop_hold: if capped {
            Duration::from_secs(30)
        } else {
            Duration::from_millis(10)
        },
        active_starts: AtomicUsize::new(0),
        active_stops: AtomicUsize::new(0),
        deadline_seen: AtomicBool::new(false),
        stop_outcomes: Mutex::new(std::collections::VecDeque::from([
            first_stop_outcome,
            CanceledStartStopOutcome::Success,
        ])),
    });
    let token_dir = tempfile::TempDir::new().expect("create container token directory");
    let token_manager =
        ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
    let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let runtime_settings = crate::config::settings::AppSettings {
        api_server_enabled: true,
        api_server_bind: "0.0.0.0".to_string(),
        api_server_port: 8765,
        ..crate::config::settings::AppSettings::default()
    };
    let mut container_backend = ContainerTransportBackend::with_runtime(
        output_senders,
        idle_detector,
        None,
        None,
        runtime.clone(),
        Some(token_manager),
    );
    container_backend.set_runtime_settings_for_test(runtime_settings);
    if fail_worker_spawn {
        container_backend.inject_shutdown_worker_spawn_failure_for_test();
    }
    let container_backend = Arc::new(container_backend);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
        Arc::new(LifecycleTestBackend::default()),
        container_backend.clone(),
    )));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build real pending-container shutdown app");
    coordinator
        .start(app.handle().clone())
        .expect("start real pending-container shutdown coordinator");
    let mut restore_guard = Some(
        coordinator
            .submit_restore_first()
            .await
            .expect("hold restore barrier"),
    );
    let manager_handle = manager.read().await.clone();
    let mut ticket = coordinator
        .reserve_create(TrustedCreateIntent::Background)
        .await
        .expect("reserve real pending-container finalizer");
    let pending = manager_handle
        .create_pending_session(
            &mut ticket,
            "container".to_string(),
            Vec::new(),
            root.clone(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::ContainerTransport,
        )
        .await
        .expect("create real pending-container manager row");
    let pending_id = pending.id;
    let create_shutdown = coordinator.inner.shutdown.clone();
    let create_pty = Arc::clone(&pty);
    let create_root = root.clone();
    let mut create = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build real-container create test runtime");
        runtime.block_on(async move {
            let _ticket = ticket;
            let _spawn_mark = create_pty
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .mark_spawning(&create_root, "container");
            tokio::select! {
                biased;
                _ = create_shutdown.cancelled() => {
                    Err(SelectionCoordinatorError::Unavailable.to_string())
                }
                result = PtyManager::spawn(
                    &create_pty,
                    SessionBackendKind::ContainerTransport,
                    real_container_spawn_spec(pending_id, create_root),
                ) => result.map_err(|error| error.to_string()),
            }
        })
    });

    tokio::select! {
        result = &mut create => {
            panic!("real container create ended before runtime start: {result:?}");
        }
        started = tokio::time::timeout(Duration::from_secs(5), start_started_rx) => {
            started
                .expect("runtime start reaches blocking section")
                .expect("runtime start witness is delivered");
        }
    }
    assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
    assert!(container_backend.contains_transport_state_for_test(pending_id));
    let aggregate = manager_handle.aggregate_snapshot().await;
    assert!(aggregate.pending_ids.contains(&pending_id));
    assert!(aggregate.sessions.is_empty());
    let (pending_spawns, live_routes) = pty
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .archive_liveness(&[pending_id]);
    assert_eq!(pending_spawns.len(), 1);
    assert_eq!(live_routes, vec![false]);
    assert_eq!(
        container_backend.shutdown_work_state_for_test(),
        (false, 1, 1),
        "the producer and blocking start must be shutdown-owned before runtime.start"
    );

    let close_started = Instant::now();
    let close = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move {
            if capped {
                coordinator.close_and_join_with_budget(close_budget).await
            } else {
                coordinator.close_and_join().await
            }
        })
    };
    await_state_eq(
        "real pending-container shutdown signal becomes visible",
        Duration::from_secs(1),
        || coordinator.inner.shutdown.is_cancelled(),
        true,
        String::new,
    )
    .await;
    let create_error = (&mut create)
        .await
        .expect("join canceled real container create")
        .expect_err("shutdown cancels the real container create");
    if fail_worker_spawn {
        assert!(
            create_error == SelectionCoordinatorError::Unavailable.to_string()
                || create_error == "container runtime start was canceled",
            "unexpected synchronous-fallback cancellation error: {create_error}"
        );
    } else {
        assert_eq!(
            create_error,
            SelectionCoordinatorError::Unavailable.to_string()
        );
    }
    if !capped {
        restore_guard
            .take()
            .expect("normal-drain restore guard")
            .finish();
    }
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
    .expect("pending manager row is rolled back during shutdown");
    tokio::time::timeout(Duration::from_secs(1), stop_started_rx)
        .await
        .expect("late runtime handle stop starts")
        .expect("late runtime handle stop witness is delivered");
    assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
    let close_report = tokio::time::timeout(Duration::from_secs(2), close)
        .await
        .expect("real pending-container close obeys the shared deadline")
        .expect("join real pending-container close task");
    let close_elapsed = close_started.elapsed();
    // Sampled before the bounded drain wait below, so it can still witness
    // work the close gave up on.
    let work_state_at_close = container_backend.shutdown_work_state_for_test();
    let close_bound = if capped {
        close_budget + Duration::from_millis(550)
    } else {
        Duration::from_secs(1)
    };
    assert!(
        close_elapsed <= close_bound,
        "real container close elapsed {close_elapsed:?}, bound {close_bound:?}"
    );
    if capped {
        // Only the abandoned side is asserted. A capped close may retain work
        // without ever reaching its absolute deadline: the
        // `container-shutdown-signal` and `coordinator-worker-handle` reasons are
        // raised immediately on a `try_lock` failure, `critical-key-clear` is not
        // deadline-driven at all, and `blocking-seed-transaction-await` is raised
        // at the worker deadline, one finalization reserve before the absolute
        // one. A close that returns under budget is therefore free to retain, so
        // asserting an empty retained set there would fail runs the production
        // contract permits.
        if close_elapsed >= close_budget && work_state_at_close != (true, 0, 0) {
            // The registry only drains, never refills, so work outstanding
            // here was outstanding at the close's deadline too. The predicate
            // couples the report to something observed independently of it, so
            // it does not restate `persistence_safe == retained.is_empty()`.
            assert!(
                !close_report.persistence_safe,
                "a capped close that gave up on {work_state_at_close:?} must report abandoned ownership: {close_report:?}"
            );
            assert!(
                !close_report.retained.is_empty(),
                "a capped close that gave up on {work_state_at_close:?} must name what it abandoned: {close_report:?}"
            );
        }
        // The remaining cases, a close that returned under budget and a close
        // that reached its deadline against a registry that then drained before
        // this sample, are undecidable from outside and are left unasserted.
    }
    if let Some(guard) = restore_guard.take() {
        guard.finish();
    }

    await_state_eq(
        "runtime start was called exactly once",
        Duration::from_secs(1),
        || runtime.start_calls.load(Ordering::SeqCst),
        1,
        String::new,
    )
    .await;
    await_state_eq(
        "runtime stop was called exactly once",
        Duration::from_secs(1),
        || runtime.stop_calls.load(Ordering::SeqCst),
        1,
        String::new,
    )
    .await;
    await_state_eq(
        "no runtime start is active",
        Duration::from_secs(1),
        || runtime.active_starts.load(Ordering::SeqCst),
        0,
        String::new,
    )
    .await;
    await_state_eq(
        "no runtime stop is active",
        Duration::from_secs(1),
        || runtime.active_stops.load(Ordering::SeqCst),
        0,
        String::new,
    )
    .await;
    await_state_eq(
        "runtime stop observed the shutdown deadline",
        Duration::from_secs(1),
        || runtime.deadline_seen.load(Ordering::SeqCst),
        true,
        String::new,
    )
    .await;
    assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.active_starts.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 0);
    assert!(runtime.deadline_seen.load(Ordering::SeqCst));
    assert!(!container_backend.contains_transport_state_for_test(pending_id));
    let retry_expected = first_stop_outcome != CanceledStartStopOutcome::Success;
    let retained_contexts = container_backend.retained_cleanup_contexts_for_test();
    assert_eq!(
        container_backend.retained_runtime_cleanup_sessions_for_test(),
        if retry_expected {
            vec![pending_id]
        } else {
            Vec::new()
        },
        "the deterministic canceled-start handle must remain owned after a nonterminal stop: {:?}",
        retained_contexts
    );
    assert!(
        retained_contexts.iter().all(|context| {
            context.contains("runtimeHandle=false")
                || (retry_expected
                    && context.contains(&pending_id.to_string())
                    && context.contains("runtimeHandle=true"))
        }),
        "retained cleanup must identify runtime-backed ownership separately from non-runtime residue: {retained_contexts:?}"
    );
    await_state_eq(
        "shutdown work drains",
        Duration::from_secs(5),
        || container_backend.shutdown_observation_for_test(),
        (true, 0, 0, 0),
        || {
            format!(
                "retained={:?}",
                container_backend.retained_cleanup_contexts_for_test()
            )
        },
    )
    .await;
    assert_eq!(
        container_backend.shutdown_work_state_for_test(),
        (true, 0, 0)
    );
    assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);
    let aggregate = manager_handle.aggregate_snapshot().await;
    assert!(aggregate.pending_ids.is_empty());
    assert!(aggregate.sessions.is_empty());
    let (pending_spawns, live_routes) = pty
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .archive_liveness(&[pending_id]);
    assert!(pending_spawns.is_empty());
    assert_eq!(live_routes, vec![false]);
    assert!(pty
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .backend_kind(pending_id)
        .is_none());

    if fail_worker_spawn {
        container_backend.clear_shutdown_worker_spawn_failure_for_test();
    }
    let global_report = pty
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .stop_all_started_containers_blocking(Duration::from_secs(1));
    assert!(
        global_report.terminal,
        "retained={:?}",
        global_report.retained
    );
    assert_eq!(
        runtime.stop_calls.load(Ordering::SeqCst),
        if retry_expected { 2 } else { 1 },
        "the single production global sweep must retry only a nonterminal deterministic owner"
    );
    assert!(container_backend
        .retained_cleanup_sessions_for_test()
        .is_empty());
}
