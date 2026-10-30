use super::*;

#[tokio::test]
async fn credential_binding_is_embedded_through_pending_attach_and_active_states() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, ticket) = pending_backend(id, root).await;
    let binding = ContainerCredentialBinding {
        client_id: format!("container-{id}-{}", Uuid::new_v4()),
        credential_generation: Uuid::new_v4().to_string(),
        bound_session_id: id.to_string(),
        bound_root_object_id: crate::path_identity::FileObjectId {
            volume: 7,
            file: 11,
        },
        credential_token_hash: crate::api::auth::hash_token("binding-secret-sentinel"),
    };

    backend
        .install_credential_binding(id, binding.clone())
        .expect("install binding");
    assert_eq!(backend.credential_binding(id), Some(binding.clone()));

    backend
        .consume_ticket(id, root, &ticket)
        .expect("consume ticket");
    assert_eq!(backend.credential_binding(id), Some(binding.clone()));

    let (sender, _receiver) = mpsc::channel(8);
    backend.complete_hello(id, root, sender).expect("hello");
    assert_eq!(backend.credential_binding(id), Some(binding.clone()));
    assert!(!format!("{binding:?}").contains("binding-secret-sentinel"));

    let removed = backend.remove_session_state(id).expect("remove state");
    assert_eq!(
        removed.api_client_id.as_deref(),
        Some(binding.client_id.as_str())
    );
    assert!(backend.credential_binding(id).is_none());
}

#[tokio::test]
async fn expired_pending_reaper_removes_route_and_transport_state() {
    let id_root = "C:/repo/.ac/wg-1/__agent_dev";
    let tuning = ContainerTransportTuning {
        ticket_ttl: Duration::from_millis(1),
        ..ContainerTransportTuning::default()
    };
    let (backend, session_mgr) = backend_with_tuning(tuning);
    let removed = Arc::new(AtomicUsize::new(0));
    let removed_for_cb = removed.clone();
    backend.set_route_remover(Arc::new(move |_, _| {
        removed_for_cb.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let session = session_mgr
        .read()
        .await
        .create_session(
            "container".to_string(),
            Vec::new(),
            id_root.to_string(),
            Some("agent".to_string()),
            None,
            Vec::new(),
            false,
            SessionBackendKind::ContainerTransport,
        )
        .await
        .expect("session");
    backend
        .spawn(test_spec(session.id, id_root, PtyOutputTarget::noop()))
        .await
        .expect("spawn pending");

    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(backend.reap_expired_pending_sessions().await, 1);

    assert_eq!(removed.load(Ordering::SeqCst), 1);
    assert!(!backend.has_session(session.id));
}

#[test]
fn kill_revokes_container_token_and_stops_runtime_handle() {
    let id = Uuid::new_v4();
    let runtime = Arc::new(RecordingRuntime::default());
    let dir = tempfile::TempDir::new().unwrap();
    let token_manager = ContainerApiTokenManager::new_for_path(dir.path().join("api-clients.json"));
    let token = token_manager
        .mint_for_session(id, "C:/repo/.ac/wg-1/__agent_dev")
        .expect("token");
    let (mut backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
    backend.runtime = Some(runtime.clone());
    backend.token_manager = Some(token_manager.clone());
    let (tx, _rx) = mpsc::channel(8);
    backend.sessions.lock().unwrap().insert(
        id,
        ContainerSessionState::Active(ActiveSession {
            sender: tx,
            rows: 30,
            cols: 120,
            runtime_handle: Some(ContainerRuntimeHandle {
                session_id: id,
                container_id: "container-id".to_string(),
            }),
            api_client_id: Some(token.client_id.clone()),
            credential_binding: None,
            logical_resource_slot: None,
            container_credential_path: None,
        }),
    );

    backend.kill(id).expect("kill");
    let report =
        backend.seal_and_drain_shutdown_work_blocking(Instant::now() + Duration::from_secs(1));
    assert!(report.terminal, "retained={:?}", report.retained);
    assert!(backend.retained_cleanup_sessions_for_test().is_empty());

    let registry = crate::api::auth::list(token_manager.path());
    assert!(
        registry
            .clients
            .iter()
            .find(|client| client.client_id == token.client_id)
            .expect("client")
            .revoked
    );
    assert_eq!(runtime.stopped(), vec![id]);
}

#[tokio::test]
async fn canceled_blocking_runtime_start_stops_late_handle_and_removes_pending_state() {
    let id = Uuid::new_v4();
    let root_dir = tempfile::TempDir::new().unwrap();
    let root = root_dir.path().to_string_lossy().into_owned();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let stopped = Arc::new(Mutex::new(Vec::new()));
    let runtime = Arc::new(GatedStartRuntime {
        started: Mutex::new(Some(started_tx)),
        release: Mutex::new(release_rx),
        stopped: Arc::clone(&stopped),
    });
    let token_dir = tempfile::TempDir::new().unwrap();
    let token_manager =
        ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
    let (mut backend, _manager) = backend_with_tuning(ContainerTransportTuning::default());
    backend.runtime = Some(runtime);
    backend.token_manager = Some(token_manager);
    backend.runtime_settings_override = Some(api_enabled_settings());
    let backend = Arc::new(backend);
    let task_backend = Arc::clone(&backend);
    let mut spec = test_spec(id, &root, PtyOutputTarget::noop());
    spec.container_image = Some("agentscommander/test:latest".to_string());
    let mut task = tokio::spawn(async move { task_backend.spawn(spec).await });

    tokio::select! {
        result = &mut task => panic!("container spawn ended before runtime start: {result:?}"),
        started = tokio::time::timeout(Duration::from_secs(5), started_rx) => {
            started
                .expect("runtime start reached blocking section before timeout")
                .expect("runtime start witness sender");
        }
    }
    task.abort();
    let _ = task.await;
    release_tx.send(()).expect("release runtime start");

    for _ in 0..100 {
        if stopped.lock().unwrap().contains(&id) && !backend.has_session(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("canceled container start leaked its pending state or late runtime handle");
}

#[test]
fn global_sweep_uses_a_fresh_deadline_after_coordinator_expiry() {
    let runtime = Arc::new(SequencedStopRuntime {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        stop_calls: AtomicUsize::new(0),
        active_deadline_seen: AtomicBool::new(false),
    });
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
    let removed_routes = Arc::new(AtomicUsize::new(0));
    let route_count = Arc::clone(&removed_routes);
    backend.set_route_remover(Arc::new(move |_, _| {
        route_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));

    assert!(backend.begin_shutdown(Instant::now()));
    let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));

    assert!(report.terminal, "retained={:?}", report.retained);
    assert!(runtime.active_deadline_seen.load(Ordering::SeqCst));
    assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
    assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
    assert!(!backend.contains_transport_state_for_test(session_id));

    let second = backend.stop_all_started_containers_blocking(Duration::from_millis(100));
    assert!(second.terminal, "retained={:?}", second.retained);
    assert_eq!(
        runtime.stop_calls.load(Ordering::SeqCst),
        1,
        "terminal global cleanup must not be repeated"
    );
    assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
}

#[test]
fn single_production_global_sweep_retries_a_failed_handle_to_terminal() {
    let runtime = Arc::new(SequencedStopRuntime {
        outcomes: Mutex::new(VecDeque::from([Err("injected stop failure"), Ok(())])),
        stop_calls: AtomicUsize::new(0),
        active_deadline_seen: AtomicBool::new(false),
    });
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
    let removed_routes = Arc::new(AtomicUsize::new(0));
    let route_count = Arc::clone(&removed_routes);
    backend.set_route_remover(Arc::new(move |_, _| {
        route_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));

    let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
    assert!(report.terminal, "retained={:?}", report.retained);
    assert!(!backend.contains_transport_state_for_test(session_id));
    assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 2);
    assert!(backend.retained_cleanup_sessions_for_test().is_empty());
}

#[test]
fn permanently_blocked_stop_is_retained_without_exceeding_the_sweep_deadline() {
    let runtime = Arc::new(PermanentlyBlockedStopRuntime {
        stop_calls: AtomicUsize::new(0),
        active_deadline_seen: AtomicBool::new(false),
    });
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
    let removed_routes = Arc::new(AtomicUsize::new(0));
    let route_count = Arc::clone(&removed_routes);
    backend.set_route_remover(Arc::new(move |_, _| {
        route_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));

    let budget = Duration::from_millis(150);
    let started_at = Instant::now();
    let report = backend.stop_all_started_containers_blocking(budget);
    let elapsed = started_at.elapsed();

    assert!(!report.terminal);
    assert!(
        elapsed <= budget + Duration::from_millis(250),
        "permanently blocked stop exceeded the sweep deadline: {elapsed:?}"
    );
    assert!(runtime.active_deadline_seen.load(Ordering::SeqCst));
    assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
    assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
    assert!(!backend.contains_transport_state_for_test(session_id));
    assert!(
        report
            .retained
            .iter()
            .any(|context| context.contains(&session_id.to_string())),
        "retained={:?}",
        report.retained
    );
}

struct PendingReaperCancellation(crate::shutdown::ShutdownSignal);

impl Drop for PendingReaperCancellation {
    fn drop(&mut self) {
        self.0.trigger();
    }
}

#[tokio::test]
async fn off_runtime_pending_reaper_ticks_immediately_and_releases_backend_on_cancellation() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, _manager) = backend_with_tuning(ContainerTransportTuning {
        ticket_ttl: Duration::from_millis(1),
        ..ContainerTransportTuning::default()
    });
    let (removed_tx, removed_rx) = std::sync::mpsc::channel();
    backend.set_route_remover(Arc::new(move |removed_id, _| {
        removed_tx.send(removed_id).expect("record expired route");
        Ok(())
    }));
    backend.spawn(test_spec(id, root, PtyOutputTarget::noop())).await.expect("pending");
    std::thread::sleep(Duration::from_millis(5));

    let backend = Arc::new(backend);
    let weak = Arc::downgrade(&backend);
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let _cancellation = PendingReaperCancellation(shutdown.clone());
    let caller_backend = Arc::clone(&backend);
    let caller_shutdown = shutdown.clone();
    let (returned_tx, returned_rx) = std::sync::mpsc::channel();
    let caller = std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        caller_backend.start_pending_reaper(caller_shutdown);
        returned_tx.send(()).expect("record reaper dispatch");
    });
    returned_rx.recv_timeout(Duration::from_secs(2)).expect("reaper dispatched");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !caller.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(caller.is_finished(), "reaper dispatch did not finish");
    caller.join().expect("off-runtime reaper caller");
    assert_eq!(
        removed_rx.recv_timeout(Duration::from_secs(2)).expect("first reaper tick"),
        id
    );
    assert!(!backend.has_session(id));
    assert!(removed_rx.try_recv().is_err(), "duplicate route removal");

    shutdown.trigger();
    drop(backend);
    let deadline = Instant::now() + Duration::from_secs(2);
    while weak.upgrade().is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(weak.upgrade().is_none(), "cancelled reaper retained backend");
}
