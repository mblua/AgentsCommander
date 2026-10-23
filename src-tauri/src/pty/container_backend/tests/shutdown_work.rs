use super::*;

#[tokio::test]
async fn shutdown_barrier_rejects_start_and_stop_registration_after_empty_drain() {
    let runtime = Arc::new(RecordingRuntime::default());
    let token_dir = tempfile::TempDir::new().unwrap();
    let token_manager =
        ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
    let (mut backend, _manager) = backend_with_tuning(ContainerTransportTuning::default());
    backend.runtime = Some(runtime.clone());
    backend.token_manager = Some(token_manager);
    backend.runtime_settings_override = Some(api_enabled_settings());
    backend.seal_and_drain_shutdown_work_blocking(Instant::now() + Duration::from_secs(1));
    assert_eq!(backend.shutdown_work_state_for_test(), (true, 0, 0));

    let rejected_id = Uuid::new_v4();
    let mut spec = test_spec(
        rejected_id,
        "C:/repo/.ac/wg-1/__agent_dev",
        PtyOutputTarget::noop(),
    );
    spec.container_image = Some("agentscommander/test:latest".to_string());
    let error = backend
        .spawn(spec)
        .await
        .expect_err("sealed shutdown barrier rejects a new runtime start");
    assert!(error
        .to_string()
        .contains("container runtime start rejected during shutdown"));
    assert_eq!(runtime.start_count(), 0);
    assert!(!backend.contains_transport_state_for_test(rejected_id));
    assert_eq!(backend.shutdown_work_state_for_test(), (true, 0, 0));

    let stopped_id = Uuid::new_v4();
    let _transport_receiver =
        backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id: stopped_id,
            container_id: format!("container-{stopped_id}"),
        });
    backend
        .kill(stopped_id)
        .expect("sealed registry transfers late stop ownership");
    assert!(runtime.stopped().is_empty());
    assert_eq!(
        backend.retained_cleanup_sessions_for_test(),
        vec![stopped_id]
    );
    assert_eq!(backend.shutdown_work_state_for_test(), (true, 0, 0));

    let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
    assert!(report.terminal, "retained={:?}", report.retained);
    assert_eq!(runtime.stopped(), vec![stopped_id]);
    assert!(backend.retained_cleanup_sessions_for_test().is_empty());
    assert_eq!(backend.shutdown_work_state_for_test(), (true, 0, 0));
}

#[test]
fn shutdown_barrier_waits_for_presealed_producer_and_its_late_cleanup() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    let producer = registry
        .register_producer()
        .expect("register producer before shutdown sealing");
    let (drain_done, drain_done_rx) = std::sync::mpsc::channel();
    let drain_registry = Arc::clone(&registry);
    let drain = std::thread::spawn(move || {
        drain_registry.seal_and_drain_until(Instant::now() + Duration::from_secs(1));
        drain_done
            .send(())
            .expect("publish shutdown drain completion");
    });
    let seal_deadline = Instant::now() + Duration::from_secs(1);
    while !registry.snapshot().0 {
        assert!(
            Instant::now() < seal_deadline,
            "shutdown registry did not seal"
        );
        std::thread::yield_now();
    }
    assert_eq!(registry.snapshot(), (true, 1, 0));
    assert!(
        drain_done_rx
            .recv_timeout(Duration::from_millis(25))
            .is_err(),
        "an admitted producer must prevent an empty shutdown drain"
    );

    let (cleanup_started, cleanup_started_rx) = std::sync::mpsc::channel();
    let (cleanup_release, cleanup_release_rx) = std::sync::mpsc::channel();
    producer.spawn_owned(None, "barrier-test", move |_| {
        cleanup_started
            .send(())
            .expect("publish late cleanup start");
        cleanup_release_rx
            .recv()
            .expect("wait for late cleanup release");
    });
    drop(producer);
    cleanup_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("presealed producer registers its late cleanup");
    assert!(
        drain_done_rx
            .recv_timeout(Duration::from_millis(25))
            .is_err(),
        "shutdown drain must join cleanup registered after sealing"
    );
    cleanup_release.send(()).expect("release late cleanup");
    drain_done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("shutdown drain completes after late cleanup");
    drain.join().expect("join shutdown drain thread");
    assert_eq!(registry.snapshot(), (true, 0, 0));
    assert!(registry.register_producer().is_none());
}

#[test]
fn bounded_shutdown_executor_limits_multiple_slow_cleanups() {
    const WORK_ITEMS: usize = 12;

    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    for _ in 0..WORK_ITEMS {
        let producer = registry
            .register_producer()
            .expect("register bounded-executor producer");
        let active = Arc::clone(&active);
        let max_active = Arc::clone(&max_active);
        let completed = Arc::clone(&completed);
        producer.spawn_owned(None, "bounded-cleanup-test", move |control| {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_active.fetch_max(current, Ordering::SeqCst);
            let _ = control.wait_for_shutdown();
            active.fetch_sub(1, Ordering::SeqCst);
            completed.fetch_add(1, Ordering::SeqCst);
        });
    }

    let start_deadline = Instant::now() + Duration::from_secs(1);
    while active.load(Ordering::SeqCst) < CONTAINER_SHUTDOWN_WORKER_CAPACITY {
        assert!(
            Instant::now() < start_deadline,
            "bounded shutdown workers did not start"
        );
        std::thread::yield_now();
    }
    assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);
    assert_eq!(registry.snapshot(), (false, 0, WORK_ITEMS));
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        CONTAINER_SHUTDOWN_WORKER_CAPACITY
    );

    registry.seal_and_drain_until(Instant::now() + Duration::from_secs(1));
    assert_eq!(completed.load(Ordering::SeqCst), WORK_ITEMS);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(max_active.load(Ordering::SeqCst) <= CONTAINER_SHUTDOWN_WORKER_CAPACITY);
    assert_eq!(registry.snapshot(), (true, 0, 0));
    assert_eq!(registry.worker_count(), 0);
}

#[test]
fn worker_spawn_failure_serializes_multiple_slow_cleanups() {
    const WORK_ITEMS: usize = 4;

    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    registry.inject_worker_spawn_failure();
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut callers = Vec::new();
    for index in 0..WORK_ITEMS {
        let producer = registry
            .register_producer()
            .expect("register synchronous-fallback producer");
        let active = Arc::clone(&active);
        let max_active = Arc::clone(&max_active);
        let completed = Arc::clone(&completed);
        callers.push(
            std::thread::Builder::new()
                .name(format!("shutdown-fallback-caller-{index}"))
                .spawn(move || {
                    producer.spawn_owned(None, "fallback-bound-test", move |control| {
                        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                        max_active.fetch_max(current, Ordering::SeqCst);
                        let _ = control.wait_for_shutdown();
                        active.fetch_sub(1, Ordering::SeqCst);
                        completed.fetch_add(1, Ordering::SeqCst);
                    });
                })
                .expect("spawn synchronous-fallback caller"),
        );
    }

    let start_deadline = Instant::now() + Duration::from_secs(1);
    while max_active.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < start_deadline,
            "synchronous fallback did not start"
        );
        std::thread::yield_now();
    }
    assert_eq!(max_active.load(Ordering::SeqCst), 1);
    assert_eq!(registry.worker_count(), 0);
    assert_eq!(registry.snapshot(), (false, WORK_ITEMS, 1));

    registry.seal_and_drain_until(Instant::now() + Duration::from_secs(1));
    for caller in callers {
        caller.join().expect("join synchronous-fallback caller");
    }
    assert_eq!(completed.load(Ordering::SeqCst), WORK_ITEMS);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(max_active.load(Ordering::SeqCst), 1);
    assert_eq!(registry.snapshot(), (true, 0, 0));
}

#[test]
fn blocked_producer_fallback_is_reported_at_the_absolute_deadline() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    registry.inject_worker_spawn_failure();
    let producer = registry
        .register_producer()
        .expect("register blocked-fallback producer");
    let (started, started_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let caller = std::thread::spawn(move || {
        producer.spawn_owned(None, "blocked-fallback-test", move |_| {
            started.send(()).expect("publish blocked fallback start");
            release_rx.recv().expect("release blocked fallback");
        });
    });
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("blocked fallback starts");

    let budget = Duration::from_millis(100);
    let started_at = Instant::now();
    let report = registry.seal_and_drain_until(started_at + budget);
    assert!(!report.terminal);
    assert!(
        started_at.elapsed() <= budget + Duration::from_millis(250),
        "blocked fallback drain exceeded its absolute deadline: {:?}",
        started_at.elapsed()
    );
    assert_eq!(
        report
            .retained
            .iter()
            .filter(|context| context.contains("reason=blocked-fallback-test"))
            .count(),
        1,
        "retained={:?}",
        report.retained
    );

    release.send(()).expect("release blocked fallback owner");
    caller.join().expect("join blocked fallback caller");
    let terminal = registry.seal_and_drain_until(Instant::now() + Duration::from_secs(1));
    assert!(terminal.terminal, "retained={:?}", terminal.retained);
    assert_eq!(registry.snapshot(), (true, 0, 0));
}

fn wait_for_shutdown_work_drain(registry: &ContainerShutdownWorkRegistry) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while registry.snapshot().2 != 0 {
        assert!(Instant::now() < deadline, "shutdown work did not drain");
        std::thread::sleep(CONTAINER_SHUTDOWN_POLL);
    }
}

fn poll_shutdown_workers_until_zero(registry: &ContainerShutdownWorkRegistry, context: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while registry.worker_count() != 0 {
        assert!(
            Instant::now() < deadline,
            "shutdown workers stayed alive after {context}: count={}",
            registry.worker_count()
        );
        std::thread::sleep(CONTAINER_SHUTDOWN_POLL);
    }
}

#[test]
fn drain_overrun_reclaims_workers_after_busy_item_is_released() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    let producer = registry
        .register_producer()
        .expect("register overrun producer");
    let (started, started_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    producer.spawn_owned(None, "t1-busy-item", move |_| {
        started.send(()).expect("publish busy item start");
        release_rx.recv().expect("wait for busy item release");
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("busy item starts");
    assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);

    let report = registry.seal_and_drain_until(Instant::now() + Duration::from_millis(100));
    assert!(!report.terminal, "retained={:?}", report.retained);
    release.send(()).expect("release busy item");
    drop(producer);
    poll_shutdown_workers_until_zero(&registry, "producers dropped after overrun");
    assert_eq!(registry.snapshot(), (true, 0, 0));
}

#[test]
fn drain_overrun_keeps_workers_while_late_producer_is_alive() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    let producer = registry
        .register_producer()
        .expect("register late-producer owner");
    let (started, started_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    producer.spawn_owned(None, "t1b-busy-item", move |_| {
        started.send(()).expect("publish busy item start");
        release_rx.recv().expect("wait for busy item release");
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("busy item starts");
    assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);

    let report = registry.seal_and_drain_until(Instant::now() + Duration::from_millis(100));
    assert!(!report.terminal, "retained={:?}", report.retained);
    release.send(()).expect("release busy item");
    let sample_deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < sample_deadline {
        assert_eq!(
            registry.worker_count(),
            CONTAINER_SHUTDOWN_WORKER_CAPACITY,
            "workers exited while the pre-seal producer was alive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let late_ran = Arc::new(AtomicBool::new(false));
    let late_ran_work = Arc::clone(&late_ran);
    producer.spawn_owned(None, "t1b-late-item", move |_| {
        late_ran_work.store(true, Ordering::SeqCst);
    });
    let late_deadline = Instant::now() + Duration::from_secs(5);
    while !late_ran.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < late_deadline,
            "late producer work never ran after the overrun"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(producer);
    poll_shutdown_workers_until_zero(&registry, "late producer dropped");
}

#[test]
fn expired_control_deadline_with_unavailable_state_lock_still_seals_and_exits() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    let producer = registry
        .register_producer()
        .expect("register quick-item producer");
    let (done, done_rx) = std::sync::mpsc::channel();
    producer.spawn_owned(None, "t2-quick-item", move |_| {
        done.send(()).expect("publish quick item completion");
    });
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("quick item runs");
    drop(producer);
    wait_for_shutdown_work_drain(&registry);
    assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);

    let state_guard = registry
        .shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let drain_registry = Arc::clone(&registry);
    let drain = std::thread::spawn(move || {
        drain_registry.seal_and_drain_until(Instant::now() + Duration::from_millis(50))
    });
    let report = drain.join().expect("join state-lock-blocked drain");
    assert!(!report.terminal, "retained={:?}", report.retained);
    drop(state_guard);

    poll_shutdown_workers_until_zero(&registry, "failed state-lock drain");
    assert!(
        registry.register_producer().is_none(),
        "Accepting admission hole stayed open after the failed begin_shutdown"
    );
    assert!(
        registry.snapshot().0,
        "failed begin_shutdown was never sealed"
    );
}

#[test]
fn sweep_epoch_overrun_reclaims_workers() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    assert!(
        registry.begin_shutdown(Instant::now() + Duration::from_secs(1)),
        "sweep epoch pre-seal must succeed"
    );
    let sweep_deadline = Instant::now() + Duration::from_millis(100);
    assert!(
        registry.start_global_sweep_epoch(sweep_deadline),
        "global sweep epoch must start"
    );
    let control = registry.shared.control.clone();
    let (started, started_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    assert!(
        registry.spawn_global_sweep_owned_with_control(
            None,
            "t3-sweep-item",
            control,
            sweep_deadline,
            move |_| {
                started.send(()).expect("publish sweep item start");
                release_rx.recv().expect("wait for sweep item release");
            },
        ),
        "sweep item must be admitted"
    );
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("sweep item starts");
    assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);

    let report = registry.seal_and_drain_until(sweep_deadline);
    assert!(!report.terminal, "retained={:?}", report.retained);
    release.send(()).expect("release sweep item");
    poll_shutdown_workers_until_zero(&registry, "sweep epoch overrun");
}

#[test]
fn sweep_epoch_reopen_after_overrun_keeps_fresh_workers() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    let producer = registry
        .register_producer()
        .expect("register overrun producer");
    let (started, started_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    producer.spawn_owned(None, "t4-busy-item", move |_| {
        started.send(()).expect("publish busy item start");
        release_rx.recv().expect("wait for busy item release");
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("busy item starts");
    assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);
    let report = registry.seal_and_drain_until(Instant::now() + Duration::from_millis(100));
    assert!(!report.terminal, "retained={:?}", report.retained);
    release.send(()).expect("release busy item");
    drop(producer);
    poll_shutdown_workers_until_zero(&registry, "control deadline overrun before reopen");

    let epoch_deadline = Instant::now() + Duration::from_secs(1);
    assert!(
        registry.start_global_sweep_epoch(epoch_deadline),
        "reopened sweep epoch must start"
    );
    let sample_deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < sample_deadline {
        assert_eq!(
            registry.worker_count(),
            CONTAINER_SHUTDOWN_WORKER_CAPACITY,
            "reopened epoch workers used the expired control deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let sweep_ran = Arc::new(AtomicBool::new(false));
    let sweep_ran_work = Arc::clone(&sweep_ran);
    let control = registry.shared.control.clone();
    assert!(
        registry.spawn_global_sweep_owned_with_control(
            None,
            "t4-sweep-item",
            control,
            epoch_deadline,
            move |_| {
                sweep_ran_work.store(true, Ordering::SeqCst);
            },
        ),
        "reopened sweep item must be admitted"
    );
    let report = registry.seal_and_drain_until(epoch_deadline);
    assert!(report.terminal, "retained={:?}", report.retained);
    assert!(sweep_ran.load(Ordering::SeqCst), "sweep item never ran");
    assert_eq!(registry.worker_count(), 0);
}

#[test]
fn registry_drop_helper_retains_only_busy_workers() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    let producer = registry
        .register_producer()
        .expect("register busy-item producer");
    let (started, started_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    producer.spawn_owned(None, "t5-busy-item", move |_| {
        started.send(()).expect("publish busy item start");
        release_rx.recv().expect("wait for busy item release");
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("busy item starts");
    assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);
    {
        let mut state = registry
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.terminating = true;
        registry.shared.work_available.notify_all();
        registry.shared.state_changed.notify_all();
    }

    let remaining = registry.join_idle_workers_until(Instant::now() + Duration::from_secs(5));
    assert_eq!(
        remaining.len(),
        1,
        "the drop helper must retain only the busy worker"
    );
    release.send(()).expect("release busy item");
    for worker in remaining {
        worker.join().expect("join retained busy worker");
    }
    drop(producer);
}

#[test]
fn sweep_epoch_reopen_right_after_worker_exit_still_runs_sweep_work() {
    // Probabilistic: a reopen racing the previous epoch's worker exit must
    // still spawn live workers and admit sweep work. The 50 iterations
    // exercise the race window; any single failure is a regression.
    for _ in 0..50 {
        let registry = Arc::new(ContainerShutdownWorkRegistry::default());
        let producer = registry
            .register_producer()
            .expect("register overrun producer");
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        producer.spawn_owned(None, "t6-busy-item", move |_| {
            started.send(()).expect("publish busy item start");
            release_rx.recv().expect("wait for busy item release");
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("busy item starts");
        let report = registry.seal_and_drain_until(Instant::now() + Duration::from_millis(100));
        assert!(!report.terminal, "retained={:?}", report.retained);
        release.send(()).expect("release busy item");
        drop(producer);
        poll_shutdown_workers_until_zero(&registry, "overrun before reopen");

        let epoch_deadline = Instant::now() + Duration::from_secs(1);
        assert!(
            registry.start_global_sweep_epoch(epoch_deadline),
            "reopened sweep epoch must start"
        );
        let sweep_ran = Arc::new(AtomicBool::new(false));
        let sweep_ran_work = Arc::clone(&sweep_ran);
        let control = registry.shared.control.clone();
        assert!(
            registry.spawn_global_sweep_owned_with_control(
                None,
                "t6-sweep-item",
                control,
                epoch_deadline,
                move |_| {
                    sweep_ran_work.store(true, Ordering::SeqCst);
                },
            ),
            "reopened sweep item must be admitted"
        );
        let report = registry.seal_and_drain_until(epoch_deadline);
        assert!(report.terminal, "retained={:?}", report.retained);
        assert!(sweep_ran.load(Ordering::SeqCst), "sweep item never ran");
        assert_eq!(registry.worker_count(), 0);
    }
}

#[test]
fn ensure_workers_does_not_spawn_or_spin_while_terminating() {
    let registry = Arc::new(ContainerShutdownWorkRegistry::default());
    let producer = registry
        .register_producer()
        .expect("register quick-item producer");
    let (done, done_rx) = std::sync::mpsc::channel();
    producer.spawn_owned(None, "t7-quick-item", move |_| {
        done.send(()).expect("publish quick item completion");
    });
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("quick item runs");
    drop(producer);
    assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);
    {
        let mut state = registry
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.terminating = true;
        registry.shared.work_available.notify_all();
        registry.shared.state_changed.notify_all();
    }
    poll_shutdown_workers_until_zero(&registry, "terminating registry");

    let (returned, returned_rx) = std::sync::mpsc::channel();
    let ensure_registry = Arc::clone(&registry);
    std::thread::spawn(move || {
        let context = ContainerShutdownWorkContext {
            session_id: None,
            reason: "t7-ensure-workers",
            provenance: ContainerShutdownWorkProvenance::GlobalSweep,
        };
        returned
            .send(ensure_registry.ensure_workers(&context))
            .expect("publish ensure_workers result");
    });
    assert_eq!(
        returned_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("ensure_workers must return while terminating"),
        0
    );
    assert_eq!(
        registry
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        0
    );
    assert_eq!(registry.worker_count(), 0);
}
