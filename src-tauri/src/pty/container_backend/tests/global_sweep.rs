use super::*;

fn production_route_remover_fixture(
    runtime: Arc<RecordingRuntime>,
) -> (
    Arc<ContainerTransportBackend>,
    Arc<Mutex<PtyManager>>,
    Uuid,
    mpsc::Receiver<HostToBridgeFrame>,
) {
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let backend = Arc::new(ContainerTransportBackend::with_runtime(
        output_senders,
        idle_detector,
        None,
        None,
        runtime,
        None,
    ));
    let local_backend: Arc<dyn PtyBackend> = backend.clone();
    let pty_manager = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
        local_backend,
        Arc::clone(&backend),
    )));
    let session_id = Uuid::new_v4();
    let receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
        session_id,
        container_id: format!("container-{session_id}"),
    });
    pty_manager
        .lock()
        .unwrap()
        .record_route(session_id, SessionBackendKind::ContainerTransport);
    crate::install_container_route_remover(&pty_manager);
    (backend, pty_manager, session_id, receiver)
}

fn assert_exact_production_route_residue(
    backend: &ContainerTransportBackend,
    report: &ContainerShutdownReport,
    session_id: Uuid,
    error: &str,
) {
    let expected = format!(
        "owner=containerCleanup session={session_id} reason=global-shutdown-sweep program=none \
            runtimeHandle=true state=retained inFlight=false lastError=route removal failed: \
            {error}"
    );
    assert!(report.retained.iter().any(|context| {
        context.contains("owner=containerCleanup")
            && context.contains(&format!("session={session_id}"))
            && context.contains("runtimeHandle=true")
            && (context.contains("state=retained") || context.contains("state=inFlight"))
    }));
    if report.retained.iter().any(|context| context == &expected) {
        return;
    }
    let transition_deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let contexts = backend
            .cleanup_ownership
            .contexts()
            .into_iter()
            .map(|context| context.diagnostic())
            .collect::<Vec<_>>();
        if contexts.iter().any(|context| context == &expected) {
            return;
        }
        assert!(
            Instant::now() < transition_deadline,
            "expected={expected:?} report={:?} live={contexts:?}",
            report.retained
        );
        std::thread::yield_now();
    }
}

#[test]
fn production_route_remover_outer_lock_deadline_retains_real_ownership() {
    let runtime = Arc::new(RecordingRuntime::default());
    let (backend, pty_manager, session_id, _receiver) =
        production_route_remover_fixture(Arc::clone(&runtime));
    let manager_guard = pty_manager.lock().unwrap();
    let budget = Duration::from_millis(150);
    let (report_sender, report_receiver) = std::sync::mpsc::channel();
    let sweep_backend = Arc::clone(&backend);
    let sweep = std::thread::spawn(move || {
        let started_at = Instant::now();
        let report = sweep_backend.stop_all_started_containers_blocking(budget);
        report_sender
            .send((started_at.elapsed(), report))
            .expect("publish real outer-lock route-removal report");
    });

    let (elapsed, report) = report_receiver
        .recv_timeout(budget + Duration::from_millis(500))
        .expect("real production route remover returns while outer mutex remains held");
    sweep.join().expect("join bounded real-lock global sweep");

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert!(
        elapsed <= budget + Duration::from_millis(400),
        "real outer-lock route removal exceeded the absolute deadline: {elapsed:?}"
    );
    assert!(!backend.contains_transport_state_for_test(session_id));
    assert_eq!(
        backend.retained_runtime_cleanup_sessions_for_test(),
        vec![session_id]
    );
    assert!(runtime.stopped().is_empty());
    assert_eq!(
        manager_guard.backend_kind(session_id),
        Some(SessionBackendKind::ContainerTransport)
    );
    assert_exact_production_route_residue(
        &backend,
        &report,
        session_id,
        "route removal deadline reached owner=ptyManager",
    );
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    drop(manager_guard);
    let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
    assert!(terminal.terminal, "retained={:?}", terminal.retained);
    assert_eq!(runtime.stopped(), vec![session_id]);
    assert!(backend.retained_cleanup_sessions_for_test().is_empty());
    assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
}

#[test]
fn production_route_remover_outer_poison_is_exact_structured_residue() {
    let runtime = Arc::new(RecordingRuntime::default());
    let (backend, pty_manager, session_id, _receiver) =
        production_route_remover_fixture(Arc::clone(&runtime));
    let poison_manager = Arc::clone(&pty_manager);
    let poison = std::thread::spawn(move || {
        let _manager_guard = poison_manager.lock().unwrap();
        panic!("poison the real outer PTY manager mutex");
    });
    assert!(
        poison.join().is_err(),
        "outer mutex poison fixture must panic"
    );

    let budget = Duration::from_millis(150);
    let started_at = Instant::now();
    let report = backend.stop_all_started_containers_blocking(budget);
    let elapsed = started_at.elapsed();

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert!(
        elapsed <= budget + Duration::from_millis(400),
        "outer-poison route removal exceeded the absolute deadline: {elapsed:?}"
    );
    assert!(!backend.contains_transport_state_for_test(session_id));
    assert_eq!(
        backend.retained_runtime_cleanup_sessions_for_test(),
        vec![session_id]
    );
    assert!(runtime.stopped().is_empty());
    let poisoned_guard = match pty_manager.lock() {
        Ok(_) => panic!("outer PTY manager mutex unexpectedly recovered poison"),
        Err(error) => error.into_inner(),
    };
    assert_eq!(
        poisoned_guard.backend_kind(session_id),
        Some(SessionBackendKind::ContainerTransport)
    );
    drop(poisoned_guard);
    assert_exact_production_route_residue(
        &backend,
        &report,
        session_id,
        "route removal lock poisoned owner=ptyManager",
    );
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    pty_manager.clear_poison();
    let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
    assert!(terminal.terminal, "retained={:?}", terminal.retained);
    assert_eq!(runtime.stopped(), vec![session_id]);
    assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
}

#[test]
fn production_route_remover_registry_poison_is_exact_structured_residue() {
    let runtime = Arc::new(RecordingRuntime::default());
    let (backend, pty_manager, session_id, _receiver) =
        production_route_remover_fixture(Arc::clone(&runtime));
    pty_manager.lock().unwrap().poison_route_registry_for_test();

    let budget = Duration::from_millis(150);
    let started_at = Instant::now();
    let report = backend.stop_all_started_containers_blocking(budget);
    let elapsed = started_at.elapsed();

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert!(
        elapsed <= budget + Duration::from_millis(400),
        "route-registry poison removal exceeded the absolute deadline: {elapsed:?}"
    );
    assert!(!backend.contains_transport_state_for_test(session_id));
    assert_eq!(
        backend.retained_runtime_cleanup_sessions_for_test(),
        vec![session_id]
    );
    assert!(runtime.stopped().is_empty());
    assert_eq!(
        pty_manager.lock().unwrap().backend_kind(session_id),
        Some(SessionBackendKind::ContainerTransport)
    );
    assert_exact_production_route_residue(
        &backend,
        &report,
        session_id,
        "route removal lock poisoned owner=ptyRouteRegistry",
    );
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    pty_manager
        .lock()
        .unwrap()
        .clear_route_registry_poison_for_test();
    let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
    assert!(terminal.terminal, "retained={:?}", terminal.retained);
    assert_eq!(runtime.stopped(), vec![session_id]);
    assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
}

#[test]
fn production_route_remover_healthy_path_is_terminal_once() {
    let runtime = Arc::new(RecordingRuntime::default());
    let (backend, pty_manager, session_id, _receiver) =
        production_route_remover_fixture(Arc::clone(&runtime));

    let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));

    assert!(report.terminal, "retained={:?}", report.retained);
    assert!(report.retained.is_empty());
    assert_eq!(runtime.stopped(), vec![session_id]);
    assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
    assert!(backend.retained_cleanup_sessions_for_test().is_empty());
    assert!(crate::shutdown_persistence_allowed(true, report.terminal));

    let second = backend.stop_all_started_containers_blocking(Duration::from_millis(100));
    assert!(second.terminal, "retained={:?}", second.retained);
    assert_eq!(runtime.stopped(), vec![session_id]);
}

#[test]
fn blocked_global_sweep_route_remover_returns_at_deadline_with_retained_owner() {
    let runtime = Arc::new(RecordingRuntime::default());
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let backend = Arc::new(ContainerTransportBackend::with_runtime(
        output_senders,
        idle_detector,
        None,
        None,
        runtime.clone(),
        None,
    ));
    let session_id = Uuid::new_v4();
    let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
        session_id,
        container_id: format!("container-{session_id}"),
    });

    let route_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&route_calls);
    let (route_entered, route_entered_rx) = std::sync::mpsc::channel();
    let (route_release, route_release_rx) = std::sync::mpsc::channel();
    let route_release_rx = Arc::new(Mutex::new(route_release_rx));
    let release = Arc::clone(&route_release_rx);
    backend.set_route_remover(Arc::new(move |removed_session, _| {
        assert_eq!(removed_session, session_id);
        calls.fetch_add(1, Ordering::SeqCst);
        route_entered
            .send(())
            .expect("publish blocked route-remover entry");
        release
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recv()
            .expect("release blocked route remover");
        Ok(())
    }));

    let budget = Duration::from_millis(150);
    let (report_sender, report_receiver) = std::sync::mpsc::channel();
    let sweep_backend = Arc::clone(&backend);
    let sweep = std::thread::spawn(move || {
        let started_at = Instant::now();
        let report = sweep_backend.stop_all_started_containers_blocking(budget);
        report_sender
            .send((started_at.elapsed(), report))
            .expect("publish blocked route-remover report");
    });

    route_entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("global sweep enters the deterministic blocked route remover");
    let (elapsed, report) = report_receiver
        .recv_timeout(budget + Duration::from_millis(500))
        .expect("global sweep returns while route remover remains blocked");
    sweep.join().expect("join bounded global sweep caller");

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert!(
        elapsed <= budget + Duration::from_millis(400),
        "blocked route remover exceeded the absolute deadline: {elapsed:?}"
    );
    assert!(!backend.contains_transport_state_for_test(session_id));
    assert_eq!(
        backend.retained_runtime_cleanup_sessions_for_test(),
        vec![session_id]
    );
    assert!(runtime.stopped().is_empty());
    assert_eq!(route_calls.load(Ordering::SeqCst), 1);
    assert!(
        report.retained.iter().any(|context| {
            context.contains("owner=containerCleanup")
                && context.contains(&format!("session={session_id}"))
                && context.contains("runtimeHandle=true")
                && context.contains("state=inFlight")
                && context.contains("inFlight=true")
        }),
        "retained={:?}",
        report.retained
    );
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    route_release.send(()).expect("release route remover owner");
    let cleanup_deadline = Instant::now() + Duration::from_secs(1);
    while runtime.stopped() != vec![session_id] {
        assert!(
            Instant::now() < cleanup_deadline,
            "retained cleanup did not finish after route-remover release"
        );
        std::thread::yield_now();
    }
    while !backend
        .retained_runtime_cleanup_sessions_for_test()
        .is_empty()
    {
        assert!(
            Instant::now() < cleanup_deadline,
            "retained ownership did not become terminal after release"
        );
        std::thread::yield_now();
    }
    assert_eq!(route_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn poisoned_global_sweep_route_removal_is_structured_residue() {
    let runtime = Arc::new(RecordingRuntime::default());
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let backend = ContainerTransportBackend::with_runtime(
        output_senders,
        idle_detector,
        None,
        None,
        runtime.clone(),
        None,
    );
    let session_id = Uuid::new_v4();
    let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
        session_id,
        container_id: format!("container-{session_id}"),
    });
    backend.set_route_remover(Arc::new(|_, _| {
        Err(RouteRemovalError::LockPoisoned("ptyManager"))
    }));

    let report = backend.stop_all_started_containers_blocking(Duration::from_millis(50));

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert!(!backend.contains_transport_state_for_test(session_id));
    assert_eq!(
        backend.retained_runtime_cleanup_sessions_for_test(),
        vec![session_id]
    );
    assert!(runtime.stopped().is_empty());
    assert!(
        report.retained.iter().any(|context| {
            context.contains("owner=containerCleanup")
                && context.contains(&format!("session={session_id}"))
                && context.contains("runtimeHandle=true")
                && context.contains("route removal lock poisoned owner=ptyManager")
        }),
        "retained={:?}",
        report.retained
    );
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));
}

#[test]
fn docker_command_owner_stays_visible_during_real_global_sweep_retry() {
    let runtime = Arc::new(crate::pty::docker_runtime::DockerRuntime::new());
    let readers = runtime.retain_blocked_command_readers_for_test(
        "global-sweep-reader",
        "docker-reader-fixture",
        "reader cleanup failed OPENAI_API_KEY=sk-proj-global-sweep-secret\nnested failure",
    );
    assert_eq!(runtime.active_reader_count(), 2);
    let (retry_entered, retry_release) = runtime.install_command_retry_gate_for_test();
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let backend = Arc::new(ContainerTransportBackend::with_runtime(
        output_senders,
        idle_detector,
        None,
        None,
        runtime.clone(),
        None,
    ));
    let budget = Duration::from_millis(150);
    let (report_sender, report_receiver) = std::sync::mpsc::channel();
    let sweep_backend = Arc::clone(&backend);
    let sweep = std::thread::spawn(move || {
        let started_at = Instant::now();
        let report = sweep_backend.stop_all_started_containers_blocking(budget);
        report_sender
            .send((started_at.elapsed(), report))
            .expect("publish blocked Docker-command global-sweep report");
    });

    retry_entered
        .recv_timeout(Duration::from_secs(1))
        .expect("production runtime retry publishes its in-flight transition");
    let (elapsed, report) = report_receiver
        .recv_timeout(budget + Duration::from_millis(500))
        .expect("global sweep returns while the Docker command retry remains active");
    sweep
        .join()
        .expect("join bounded Docker-command sweep caller");

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert!(
        elapsed <= budget + Duration::from_millis(400),
        "in-flight Docker command exceeded the global-sweep deadline: {elapsed:?}"
    );
    let expected = "owner=dockerCommand session=none reason=global-sweep-reader \
        program=docker-reader-fixture runtimeHandle=none state=inFlight inFlight=true \
        lastError=reader cleanup failed OPENAI_API_KEY=[REDACTED]\\nnested failure";
    assert_eq!(
        report
            .retained
            .iter()
            .filter(|context| context.as_str() == expected)
            .count(),
        1,
        "retained={:?}",
        report.retained
    );
    assert!(report.retained.iter().all(|context| {
        !context.contains("sk-proj-global-sweep-secret")
            && !context.contains("00000000-0000-0000-0000-000000000000")
    }));
    let live_contexts = runtime.retained_cleanup_contexts();
    assert_eq!(live_contexts.len(), 1);
    assert_eq!(live_contexts[0].owner, "dockerCommand");
    assert_eq!(live_contexts[0].session_id, None);
    assert_eq!(live_contexts[0].reason, "global-sweep-reader");
    assert_eq!(
        live_contexts[0].program.as_deref(),
        Some("docker-reader-fixture")
    );
    assert_eq!(live_contexts[0].state, "inFlight");
    assert!(live_contexts[0].in_flight);
    assert_eq!(runtime.active_reader_count(), 2);
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    let mut selection_retained = (0..300)
        .map(|index| {
            format!(
                "owner=selectionFixture session=none reason=fixture-{index:03} program=none \
                    runtimeHandle=none state=retained inFlight=false"
            )
        })
        .collect::<Vec<_>>();
    selection_retained.push(expected.to_string());
    let mut uncapped = selection_retained
        .iter()
        .map(|context| {
            crate::pty::container_runtime::normalize_retained_owner_diagnostic("selection", context)
        })
        .chain(report.retained.iter().map(|context| {
            crate::pty::container_runtime::normalize_retained_owner_diagnostic(
                "containerShutdown",
                context,
            )
        }))
        .collect::<Vec<_>>();
    uncapped.sort();
    uncapped.dedup();
    let combined =
        crate::combined_shutdown_retained_diagnostics(selection_retained, report.retained.clone());
    let expected_omitted = uncapped.len() - (RETAINED_OWNER_REPORT_CAPACITY - 1);
    assert_eq!(combined.len(), RETAINED_OWNER_REPORT_CAPACITY);
    assert_eq!(
        combined
            .iter()
            .filter(|context| context.as_str() == expected)
            .count(),
        1,
        "combined={combined:?}"
    );
    assert_eq!(
        combined.last().expect("global omitted-count sentinel"),
        &format!(
            "owner=diagnosticSummary session=none reason=retained-owner-report-truncated \
                program=none runtimeHandle=none state=retained inFlight=false \
                omittedCount={expected_omitted}"
        )
    );

    retry_release
        .send(())
        .expect("release the deterministic in-flight retry gate");
    let transition_deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let contexts = runtime.retained_cleanup_contexts();
        if contexts
            .first()
            .is_some_and(|context| context.state == "retained" && !context.in_flight)
        {
            break;
        }
        assert!(
            Instant::now() < transition_deadline,
            "Docker command owner did not return to retained state after retry release: \
                {contexts:?}"
        );
        std::thread::yield_now();
    }
    assert_eq!(runtime.active_reader_count(), 2);
    readers.release();
    let retry_control = ContainerRuntimeControl::default();
    retry_control.request_shutdown(Instant::now() + Duration::from_secs(1));
    runtime.retry_retained_cleanups(&retry_control);
    assert!(runtime.retained_cleanup_contexts().is_empty());
    assert_eq!(runtime.active_reader_count(), 0);

    let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
    assert!(terminal.terminal, "retained={:?}", terminal.retained);
    assert!(terminal.retained.is_empty());
}

#[test]
fn high_cardinality_retained_scheduling_obeys_the_advertised_deadline() {
    const INSTALLED_ENTRIES: usize = 180;
    const RUNTIME_ENTRIES: usize = 180;

    let runtime_contexts = (0..RUNTIME_ENTRIES)
        .map(|index| {
            if index % 2 == 0 {
                RetainedContainerOwnerContext {
                    owner: "ambiguousStartCleanup",
                    session_id: Some(Uuid::from_u128(10_000 + index as u128)),
                    reason: "ambiguous-start".to_string(),
                    program: None,
                    runtime_handle: Some(true),
                    state: "retained",
                    in_flight: false,
                    last_error: None,
                }
            } else {
                RetainedContainerOwnerContext {
                    owner: "dockerCommand",
                    session_id: None,
                    reason: "docker-command".to_string(),
                    program: Some(format!("docker-reader-{index}")),
                    runtime_handle: None,
                    state: "retained",
                    in_flight: false,
                    last_error: None,
                }
            }
        })
        .collect();
    let runtime = Arc::new(RetainedContextRuntime {
        contexts: runtime_contexts,
    });
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let backend = ContainerTransportBackend::with_runtime(
        output_senders,
        idle_detector,
        None,
        None,
        runtime,
        None,
    );
    for index in 0..INSTALLED_ENTRIES {
        let session_id = Uuid::from_u128(20_000 + index as u128);
        let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id,
            container_id: format!("container-{session_id}"),
        });
    }

    let budget = Duration::ZERO;
    let started_at = Instant::now();
    let report = backend.stop_all_started_containers_blocking(budget);
    let elapsed = started_at.elapsed();

    assert!(!report.terminal);
    assert!(
        elapsed <= Duration::from_millis(250),
        "high-cardinality scheduling exceeded its advertised deadline: {elapsed:?}"
    );
    assert_eq!(
        report
            .retained
            .iter()
            .filter(|context| context.contains("owner=installedContainer "))
            .count(),
        INSTALLED_ENTRIES
    );
    assert_eq!(
        report
            .retained
            .iter()
            .filter(|context| {
                context.contains("owner=ambiguousStartCleanup ")
                    || context.contains("owner=dockerCommand ")
            })
            .count(),
        RUNTIME_ENTRIES
    );

    let selection_retained = vec![
        "reason=selection-coordinator state=retained".to_string(),
        "owner=pendingCreate session=none reason=create state=retained".to_string(),
    ];
    let mut uncapped = selection_retained
        .iter()
        .map(|context| {
            crate::pty::container_runtime::normalize_retained_owner_diagnostic("selection", context)
        })
        .chain(report.retained.iter().map(|context| {
            crate::pty::container_runtime::normalize_retained_owner_diagnostic(
                "containerShutdown",
                context,
            )
        }))
        .collect::<Vec<_>>();
    uncapped.sort();
    uncapped.dedup();
    assert!(uncapped
        .iter()
        .any(|context| context.starts_with("owner=selection ")));
    assert!(uncapped
        .iter()
        .any(|context| context.starts_with("owner=installedContainer ")));
    assert!(uncapped
        .iter()
        .any(|context| context.starts_with("owner=dockerCommand ")));
    assert!(uncapped
        .iter()
        .any(|context| context.starts_with("owner=ambiguousStartCleanup ")));

    let retained =
        crate::combined_shutdown_retained_diagnostics(selection_retained, report.retained);
    let expected_omitted = uncapped.len() - (RETAINED_OWNER_REPORT_CAPACITY - 1);
    assert_eq!(retained.len(), RETAINED_OWNER_REPORT_CAPACITY);
    assert_eq!(
        retained.last().expect("omitted-count sentinel"),
        &format!(
            "owner=diagnosticSummary session=none reason=retained-owner-report-truncated \
                program=none runtimeHandle=none state=retained inFlight=false \
                omittedCount={expected_omitted}"
        )
    );
}
