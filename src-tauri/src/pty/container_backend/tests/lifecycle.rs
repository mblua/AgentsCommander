use super::*;
use super::{
    assert_runtime_owner_retained, backend_with_runtime, insert_runtime_handle,
    production_route_remover_fixture, sweep_at_deadline, Rescue, SweepDriver, EPSILON, WATCHDOG,
};

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

/// Stop that acks `(control deadline, session, ordinal, exited)`; only the first call
/// parks, until its release is sent or dropped.
struct GatedStopRuntime {
    calls: AtomicUsize,
    acks: std::sync::mpsc::Sender<(Option<Instant>, Uuid, usize, bool)>,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl ContainerRuntime for GatedStopRuntime {
    fn start(
        &self,
        request: ContainerStartRequest,
        _control: &ContainerRuntimeControl,
    ) -> Result<ContainerRuntimeHandle, AppError> {
        Ok(ContainerRuntimeHandle {
            session_id: request.session_id,
            container_id: format!("container-{}", request.session_id),
        })
    }

    fn stop(
        &self,
        handle: &ContainerRuntimeHandle,
        _timeout: Duration,
        control: &ContainerRuntimeControl,
    ) -> Result<(), AppError> {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let ack = |exited| {
            (
                control.shutdown_deadline(),
                handle.session_id,
                ordinal,
                exited,
            )
        };
        let _ = self.acks.send(ack(false));
        let release = self.release.lock().unwrap().take();
        if let Some(release) = release {
            let _ = release.recv();
            let _ = self.acks.send(ack(true));
        }
        Ok(())
    }

    fn cleanup_labeled_orphans(
        &self,
        _live_sessions: &HashSet<Uuid>,
        _timeout: Duration,
    ) -> Result<ContainerCleanupReport, AppError> {
        Ok(ContainerCleanupReport::default())
    }
}

#[test]
fn permanently_blocked_stop_is_retained_without_exceeding_the_sweep_deadline() {
    let (acks, stop_acks) = std::sync::mpsc::channel();
    let (release, release_receiver) = std::sync::mpsc::channel::<()>();
    let runtime = Arc::new(GatedStopRuntime {
        calls: AtomicUsize::new(0),
        acks,
        release: Mutex::new(Some(release_receiver)),
    });
    let backend = Arc::new(backend_with_runtime(runtime.clone()));
    let session_id = Uuid::new_v4();
    let _receiver = insert_runtime_handle(&backend, session_id);
    let removed_routes = Arc::new(AtomicUsize::new(0));
    let route_count = Arc::clone(&removed_routes);
    backend.set_route_remover(Arc::new(move |_, _| {
        route_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let driver = SweepDriver::new(&["epochOpened", "drainPoll"]);
    let rescue = Rescue(Arc::clone(&driver));

    let report = sweep_at_deadline(&driver, &backend, |deadline| {
        let entered = stop_acks.recv_timeout(WATCHDOG).expect("stop entry ack");
        assert_eq!(entered, (Some(deadline), session_id, 1, false));
        assert_eq!(
            removed_routes.load(Ordering::SeqCst),
            1,
            "removal precedes stop"
        );
    });
    let deadline = driver.now();

    assert!(!report.terminal);
    assert!(
        report.retained.iter().any(|context| {
            context.contains("owner=containerCleanup")
                && context.contains(&format!("session={session_id}"))
                && context.contains("state=inFlight")
        }),
        "retained={:?}",
        report.retained
    );
    assert!(stop_acks.try_recv().is_err(), "stop is still closed at D");
    assert!(!backend.contains_transport_state_for_test(session_id));

    drop(release);
    let exited = stop_acks.recv_timeout(WATCHDOG).expect("stop exit ack");
    assert_eq!(exited, (Some(deadline), session_id, 1, true));
    let settle_limit = Instant::now() + WATCHDOG;
    while !(backend.retained_cleanup_sessions_for_test().is_empty()
        && backend.shutdown_work.observation().2 == 0
        && backend.shutdown_work.observation().3 == 0)
    {
        assert!(
            Instant::now() < settle_limit,
            "the released stop did not settle"
        );
        std::thread::yield_now();
    }
    assert!(backend.cleanup_ownership.contexts().is_empty());
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
    drop(rescue);
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
    backend
        .spawn(test_spec(id, root, PtyOutputTarget::noop()))
        .await
        .expect("pending");
    tokio::time::sleep(Duration::from_millis(5)).await;

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
    returned_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reaper dispatched");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !caller.is_finished() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(caller.is_finished(), "reaper dispatch did not finish");
    caller.join().expect("off-runtime reaper caller");
    assert_eq!(
        removed_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first reaper tick"),
        id
    );
    assert!(!backend.has_session(id));
    assert!(removed_rx.try_recv().is_err(), "duplicate route removal");

    shutdown.trigger();
    drop(backend);
    let deadline = Instant::now() + Duration::from_secs(2);
    while weak.upgrade().is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        weak.upgrade().is_none(),
        "cancelled reaper retained backend"
    );
}

fn assert_exact_production_route_residue(
    backend: &ContainerTransportBackend,
    report: &ContainerShutdownReport,
    session_id: Uuid,
    error: &str,
) {
    let expected = production_route_residue(session_id, error);
    assert!(report.retained.contains(&expected), "{:?}", report.retained);
    assert!(live_contexts(backend).contains(&expected));
}

fn production_route_residue(session_id: Uuid, error: &str) -> String {
    format!(
        "owner=containerCleanup session={session_id} reason=global-shutdown-sweep program=none \
            runtimeHandle=true state=retained inFlight=false lastError=route removal failed: \
            {error}"
    )
}

fn live_contexts(backend: &ContainerTransportBackend) -> Vec<String> {
    let contexts = backend.cleanup_ownership.contexts().into_iter();
    contexts.map(|context| context.diagnostic()).collect()
}

/// Reinstalls the real remover body through `PtyManager` with the driver's clock.
fn install_clocked_remover(pty_manager: &Arc<Mutex<PtyManager>>, driver: &Arc<SweepDriver>) {
    let (now, wait) = (Arc::clone(driver), Arc::clone(driver));
    PtyManager::install_container_route_remover_with_clock(
        pty_manager,
        move || now.now(),
        move |_| wait.wait("ptyManager"),
    );
}

/// One real retry with the gates open and the clock fixed before its new deadline.
fn retry_to_terminal(
    driver: &Arc<SweepDriver>,
    backend: &ContainerTransportBackend,
    pty_manager: &Arc<Mutex<PtyManager>>,
    runtime: &RecordingRuntime,
    session_id: Uuid,
) {
    driver.open();
    let budget = Duration::from_secs(60);
    let terminal = backend.stop_all_started_containers_blocking_with_clock(budget, &driver.clock());
    assert!(terminal.terminal, "retained={:?}", terminal.retained);
    assert_eq!(runtime.stopped(), vec![session_id]);
    assert!(backend.retained_cleanup_sessions_for_test().is_empty());
    assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
    assert!(crate::shutdown_persistence_allowed(true, terminal.terminal));
}

#[test]
fn production_route_remover_outer_lock_deadline_retains_real_ownership() {
    let runtime = Arc::new(RecordingRuntime::default());
    let (backend, pty_manager, session_id, _receiver) =
        production_route_remover_fixture(Arc::clone(&runtime));
    let driver = SweepDriver::new(&["ptyManager", "drainPoll"]);
    let rescue = Rescue(Arc::clone(&driver));
    install_clocked_remover(&pty_manager, &driver);
    let manager_guard = pty_manager.lock().unwrap();
    let start = driver.now();
    let (caller, report) = driver.spawn_sweep(&backend, EPSILON);

    driver.ack("ptyManager");
    driver.step("ptyManager");
    driver.ack("ptyManager");
    driver.ack("drainPoll");
    assert!(report.try_recv().is_err(), "the caller is pending before D");
    // The worker consumes D first and records Deadline(ptyManager); the caller follows.
    driver.set(start + EPSILON);
    driver.step("ptyManager");
    let error = "route removal deadline reached owner=ptyManager";
    let expected = production_route_residue(session_id, error);
    driver.settle("ptyManager", || live_contexts(&backend).contains(&expected));
    driver.step("drainPoll");
    let report = report
        .recv_timeout(WATCHDOG)
        .expect("the caller returns at D");
    caller.join().expect("join the sweep caller");

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert_runtime_owner_retained(&backend, &runtime, session_id);
    assert_eq!(
        manager_guard.backend_kind(session_id),
        Some(SessionBackendKind::ContainerTransport)
    );
    assert_exact_production_route_residue(&backend, &report, session_id, error);
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    drop(manager_guard);
    retry_to_terminal(&driver, &backend, &pty_manager, &runtime, session_id);
    drop(rescue);
}

/// Rows 10 and 11: the first epoch records the real poison and the caller reaches its
/// backoff before D; at D, with the poison intact, it cuts without a new epoch.
fn assert_poisoned_sweep_retained(
    poison: impl FnOnce(&Arc<Mutex<PtyManager>>),
    clear: impl FnOnce(&Arc<Mutex<PtyManager>>),
    error: &str,
) {
    let runtime = Arc::new(RecordingRuntime::default());
    let (backend, pty_manager, session_id, _receiver) =
        production_route_remover_fixture(Arc::clone(&runtime));
    let driver = SweepDriver::new(&["sweepBackoff"]);
    let rescue = Rescue(Arc::clone(&driver));
    install_clocked_remover(&pty_manager, &driver);
    poison(&pty_manager);
    let start = driver.now();
    let (caller, report) = driver.spawn_sweep(&backend, EPSILON);

    driver.ack("sweepBackoff");
    let expected = production_route_residue(session_id, error);
    assert!(live_contexts(&backend).contains(&expected));
    assert!(report.try_recv().is_err(), "the caller is pending before D");
    assert_eq!(driver.probes(SweepProbe::EpochStartEntered), 1);
    driver.set(start + EPSILON);
    driver.step("sweepBackoff");
    let report = report
        .recv_timeout(WATCHDOG)
        .expect("the caller returns at D");
    caller.join().expect("join the sweep caller");

    assert_eq!(
        driver.probes(SweepProbe::EpochStartEntered),
        1,
        "no epoch at D"
    );
    assert_eq!(driver.probes(SweepProbe::DrainCut), 1);
    assert!(!report.terminal, "retained={:?}", report.retained);
    assert_runtime_owner_retained(&backend, &runtime, session_id);
    let manager = pty_manager
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        manager.backend_kind(session_id),
        Some(SessionBackendKind::ContainerTransport)
    );
    drop(manager);
    assert_exact_production_route_residue(&backend, &report, session_id, error);
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    clear(&pty_manager);
    retry_to_terminal(&driver, &backend, &pty_manager, &runtime, session_id);
    drop(rescue);
}

#[test]
fn production_route_remover_outer_poison_is_exact_structured_residue() {
    assert_poisoned_sweep_retained(
        |pty_manager| {
            let poison_manager = Arc::clone(pty_manager);
            let poison = std::thread::spawn(move || {
                let _manager_guard = poison_manager.lock().unwrap();
                panic!("poison the real outer PTY manager mutex");
            });
            assert!(
                poison.join().is_err(),
                "outer mutex poison fixture must panic"
            );
        },
        |pty_manager| {
            assert!(
                pty_manager.is_poisoned(),
                "the poison stayed intact through D"
            );
            pty_manager.clear_poison();
        },
        "route removal lock poisoned owner=ptyManager",
    );
}

#[test]
fn production_route_remover_registry_poison_is_exact_structured_residue() {
    assert_poisoned_sweep_retained(
        |pty_manager| pty_manager.lock().unwrap().poison_route_registry_for_test(),
        |pty_manager| {
            pty_manager
                .lock()
                .unwrap()
                .clear_route_registry_poison_for_test();
        },
        "route removal lock poisoned owner=ptyRouteRegistry",
    );
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
