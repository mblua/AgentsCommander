use super::*;

// ---------------------------------------------------------------------
// #1551 - the pass end
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finish_pass_emits_before_releasing_admission() {
    let cache = Arc::new(AgentInstallCache::new());
    let ticket = cache.try_begin("bob").expect("ticket");
    assert!(matches!(
        ticket.complete(InstallState::missing("seed".to_string())),
        Completion::Committed(_)
    ));
    let gate = Arc::new(AgentUpdateGate::new());
    let results = vec![ok_result("claude", "Claude")];

    let (registered_tx, registered_rx) = tokio::sync::oneshot::channel();
    *gate
        .wait_registered
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(registered_tx);
    let waiter_returned = Arc::new(AtomicBool::new(false));
    let waiter = tokio::spawn({
        let gate = Arc::clone(&gate);
        let waiter_returned = Arc::clone(&waiter_returned);
        async move {
            gate.wait_until_done().await;
            waiter_returned.store(true, Ordering::SeqCst);
        }
    });
    registered_rx
        .await
        .expect("waiter registered its notification before the finished check");

    let (emit_entered_tx, emit_entered_rx) = std::sync::mpsc::channel();
    let (release_emit_tx, release_emit_rx) = std::sync::mpsc::channel();
    let finish = tokio::task::spawn_blocking({
        let gate = Arc::clone(&gate);
        let cache = Arc::clone(&cache);
        move || {
            finish_pass(
                &gate,
                results,
                PassEnd {
                    invalidate: || {
                        assert!(!gate.is_finished());
                        cache.invalidate_all();
                    },
                    emit: || {
                        assert!(!gate.is_finished(), "emit precedes admission release");
                        assert_eq!(gate.snapshot().results.len(), 1);
                        assert_eq!(cache.generation(), 1);
                        emit_entered_tx.send(()).expect("announce emitter entry");
                        release_emit_rx
                            .recv_timeout(Duration::from_secs(5))
                            .expect("release pass-finished emitter");
                    },
                },
            );
        }
    });
    emit_entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("pass-finished emitter reached");
    assert!(!gate.is_finished());
    assert!(
        !waiter_returned.load(Ordering::SeqCst),
        "admission waiter cannot return while the emitter is blocked"
    );
    release_emit_tx.send(()).expect("release emitter");
    finish.await.expect("finish task");
    waiter.await.expect("admission waiter");
    assert!(waiter_returned.load(Ordering::SeqCst));
    assert_eq!(gate.snapshot().results.len(), 1);
    assert!(gate.is_finished());
    assert!(matches!(
        cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::Absent
    ));
}

#[tokio::test]
async fn pass_supervisor_cleanup_precedes_finish() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let ticket = cache.try_begin("bob").expect("ticket");
    assert!(matches!(
        ticket.complete(InstallState::missing("seed".to_string())),
        Completion::Committed(_)
    ));
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let slow_step = if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    };
    let target = UpdateTarget {
        command: "supervisor-missing".to_string(),
        label: "Supervisor".to_string(),
        commands: vec![slow_step.to_string()],
        cwd: dir.path().to_path_buf(),
    };
    gate.mark_started_with_nodes(vec![node(&target.command, &target.label, vec![slow_step])]);
    let task = tokio::spawn(run_update_target(
        handle.clone(),
        Arc::clone(&gate),
        target.clone(),
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !gate
        .snapshot()
        .running
        .iter()
        .any(|entry| entry.command == target.command)
    {
        assert!(Instant::now() < deadline, "target owner did not start");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let mut runtime = PassRuntime {
        started: true,
        known_targets: vec![target.clone()],
        tasks: vec![SupervisedTargetTask {
            target,
            handle: Some(task),
        }],
        ..PassRuntime::default()
    };
    let supervisor = PassSupervisor {
        app: handle.clone(),
        gate: Arc::clone(&gate),
    };
    supervisor.settle_pass_panic(&mut runtime).await;
    assert!(
        !gate.is_finished(),
        "native owner cleanup and terminal event precede gate release"
    );
    supervisor.finalize(runtime).await;
    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["event"], "agent_update_command_started");
    assert_eq!(frames[1]["event"], "agent_update_command_finished");
    assert_eq!(frames[1]["payload"]["outcome"], "failed");
    assert_eq!(
        frames[1]["payload"]["error"],
        "Update supervisor stopped unexpectedly."
    );
    assert_eq!(frames[2]["event"], "agent_updates_finished");
    assert_eq!(
        frames[2]["payload"]["results"]
            .as_array()
            .expect("results")
            .len(),
        1
    );
    assert!(gate.is_finished());
    assert_eq!(cache.generation(), 1);
    assert!(matches!(
        cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::Absent
    ));

    // A quiet boot: the invalidation still runs, the emit does not.
    let (quiet_app, quiet_cache, mut quiet_rx) = app_with_cache();
    let quiet_gate = Arc::new(AgentUpdateGate::new());
    PassSupervisor {
        app: quiet_app.handle().clone(),
        gate: Arc::clone(&quiet_gate),
    }
    .finalize(PassRuntime::default())
    .await;
    assert_no_frame(&mut quiet_rx).await;
    assert!(quiet_gate.is_finished());
    assert_eq!(quiet_cache.generation(), 1);
}

#[tokio::test]
async fn pass_panic_preserves_live_cancellation_response_release() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    seed_retained_target(&gate, "panic-row", "Panic row");
    set_target_phase(&gate, "panic-row", TargetPhase::Running(0, 1));
    let (response_ready_tx, response_ready_rx) = tokio::sync::oneshot::channel();
    let (release_response_tx, release_response_rx) = tokio::sync::oneshot::channel();
    gate.state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .controls
        .get_mut("panic-row")
        .expect("panic row control")
        .response_release_hook = Some(ResponseReleaseTestHook {
        ready: Some(response_ready_tx),
        release: Some(release_response_rx),
    });

    let cancellation = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        async move { cancel_update(&handle, &gate, "panic-row".to_string()).await }
    });
    let changed = next_frame(&mut frames).await;
    assert_eq!(changed["event"], "agent_update_cancellation_changed");

    let panic_settlement = tokio::spawn({
        let supervisor = PassSupervisor {
            app: handle.clone(),
            gate: Arc::clone(&gate),
        };
        async move {
            let mut runtime = PassRuntime {
                started: true,
                known_targets: vec![UpdateTarget {
                    command: "panic-row".to_string(),
                    label: "Panic row".to_string(),
                    commands: vec!["exit 0".to_string()],
                    cwd: cwd(),
                }],
                ..PassRuntime::default()
            };
            supervisor.settle_pass_panic(&mut runtime).await;
            (supervisor, runtime)
        }
    });

    response_ready_rx
        .await
        .expect("cancellation reached its response-release tail");
    assert!(
        !panic_settlement.is_finished(),
        "supervisor terminalization must retain the live caller fence"
    );
    assert!(gate.snapshot().results.is_empty());
    assert!(
        frames.try_recv().is_err(),
        "no terminal event before response release"
    );

    release_response_tx
        .send(())
        .expect("release cancellation response");
    let response = cancellation.await.expect("cancellation task");
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    let (supervisor, runtime) = panic_settlement.await.expect("panic settlement task");
    supervisor.finalize(runtime).await;
    let remaining = drain_frames(&mut frames);
    assert_eq!(remaining[0]["event"], "agent_update_command_finished");
    assert_eq!(
        remaining[0]["payload"]["error"],
        "Update supervisor stopped unexpectedly."
    );
    assert_eq!(remaining[1]["event"], "agent_updates_finished");
}

#[tokio::test]
async fn dropped_completion_receiver_does_not_abort_panicking_supervisor() {
    let app = build_mock_app(crate::test_support::test_builder());
    let gate = Arc::new(AgentUpdateGate::new());
    let completion = PassSupervisor::spawn(app.handle().clone(), Arc::clone(&gate));
    drop(completion);
    tokio::time::timeout(Duration::from_secs(2), gate.wait_until_done())
        .await
        .expect("private supervisor survives its dropped completion receiver");
    assert!(gate.is_finished());
}

// ---------------------------------------------------------------------
// #1551 - end to end
// ---------------------------------------------------------------------

#[tokio::test]
async fn run_startup_updates_emits_started_nodes_command_events_finished_post_probe() {
    let dir = tempfile::tempdir().expect("tempdir");
    bob_catalog_dir(dir.path());
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "agent-0".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Bob".to_string(),
            command: "bob-1551-missing".to_string(),
            color: "#000000".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        agent_auto_update_by_command: BTreeMap::from([("bob-1551-missing".to_string(), true)]),
        ..AppSettings::default()
    };
    let settings_state: SettingsState = crate::config::settings::new_settings_state(settings);
    let broadcaster = WsBroadcaster::new();
    let mut frames_rx = broadcaster.subscribe();
    let cache = Arc::new(AgentInstallCache::new());
    let app = build_mock_app(
        crate::test_support::test_builder()
            .manage(settings_state)
            .manage(broadcaster)
            .manage(Arc::clone(&cache)),
    );
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());

    run_startup_updates(handle.clone(), Arc::clone(&gate)).await;

    let started = next_frame(&mut frames_rx).await;
    assert_eq!(started["event"], "agent_updates_started");
    assert_eq!(
        started["payload"]["nodes"],
        json!([{
            "command": "bob-1551-missing",
            "label": "Bob",
            "updateCommands": ["bob up"]
        }])
    );

    let command_started = next_frame(&mut frames_rx).await;
    assert_eq!(command_started["event"], "agent_update_command_started");
    assert_eq!(command_started["payload"]["command"], "bob-1551-missing");
    assert_eq!(
        command_started["payload"]["installBefore"]["status"],
        "missing"
    );
    assert_eq!(command_started["payload"]["installBefore"]["seq"], 0);

    let command_finished = next_frame(&mut frames_rx).await;
    assert_eq!(command_finished["event"], "agent_update_command_finished");
    assert_eq!(command_finished["payload"]["command"], "bob-1551-missing");
    assert_eq!(command_finished["payload"]["ok"], false);

    let finished = next_frame(&mut frames_rx).await;
    assert_eq!(finished["event"], "agent_updates_finished");
    let results = finished["payload"]["results"]
        .as_array()
        .expect("results")
        .clone();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["ok"], false);

    let install = next_frame(&mut frames_rx).await;
    assert_eq!(install["event"], "agent_install_state_changed");
    assert_eq!(install["payload"]["command"], "bob-1551-missing");
    assert_eq!(install["payload"]["install"]["status"], "missing");
    assert_eq!(install["payload"]["install"]["seq"], 1);
    assert_no_frame(&mut frames_rx).await;

    assert!(gate.is_finished());
    let snapshot = gate.snapshot();
    assert!(!snapshot.in_progress);
    assert!(snapshot.running.is_empty());
    assert_eq!(snapshot.results.len(), 1);
    assert_eq!(snapshot.nodes.len(), 1);
    assert_eq!(
        snapshot.nodes[0]
            .install_before
            .as_ref()
            .map(|state| state.status),
        Some(InstallStatus::Missing)
    );
    assert_eq!(cache.generation(), 1);
    assert_eq!(cache.in_flight_len(), 0);
    match cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL) {
        CacheLookup::Fresh(state) => assert_eq!(state.seq, 1),
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn run_startup_updates_unavailable_catalog_spawns_nothing_and_finishes_the_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    // NO catalog is written: the persisted-only read is unavailable, so the
    // pass must not start, prompt, execute a vendor command or probe.
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "agent-0".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Bob".to_string(),
            command: "bob-1551-missing".to_string(),
            color: "#000000".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        agent_auto_update_by_command: BTreeMap::from([("bob-1551-missing".to_string(), true)]),
        ..AppSettings::default()
    };
    let settings_state: SettingsState = crate::config::settings::new_settings_state(settings);
    let broadcaster = WsBroadcaster::new();
    let mut frames_rx = broadcaster.subscribe();
    let cache = Arc::new(AgentInstallCache::new());
    let app = build_mock_app(
        crate::test_support::test_builder()
            .manage(settings_state)
            .manage(broadcaster)
            .manage(Arc::clone(&cache)),
    );
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());

    run_startup_updates(handle, Arc::clone(&gate)).await;

    // No `agent_updates_started`, no command events, no install probes.
    assert_no_frame(&mut frames_rx).await;
    assert!(
        gate.is_finished(),
        "the supervisor still completes the gate"
    );
    let snapshot = gate.snapshot();
    assert!(!snapshot.in_progress);
    assert!(snapshot.running.is_empty());
    assert!(snapshot.results.is_empty());
    assert!(snapshot.nodes.is_empty());
    assert_eq!(
        cache.generation(),
        1,
        "finalize still invalidates the cache"
    );
    assert_eq!(cache.in_flight_len(), 0);
}
