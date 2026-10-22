use super::*;

// ---------------------------------------------------------------------
// #1551 - probe scheduling
// ---------------------------------------------------------------------

#[tokio::test]
async fn update_overview_first_call_returns_checking_then_cached_state() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    bob_catalog_dir(dir.path());
    let settings = bob_settings(dir.path());
    let gate = AgentUpdateGate::new();
    gate.mark_finished(vec![]);

    let rows = update_overview_with(&handle, &settings, &gate, &cache, production_probe())
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].command, "bob-1551-missing");
    assert_eq!(rows[0].install.status, InstallStatus::Checking);
    assert_eq!(rows[0].install.seq, 0);

    let started = Instant::now();
    let committed = loop {
        let rows = update_overview_with(&handle, &settings, &gate, &cache, production_probe())
            .await
            .expect("persisted bob catalog is available");
        if rows[0].install.status == InstallStatus::Missing {
            break rows[0].install.clone();
        }
        assert!(
            started.elapsed() < POLL_CAP,
            "the probe never committed a state"
        );
        tokio::time::sleep(POLL_STEP).await;
    };
    assert_eq!(committed.seq, 1);

    let frame = next_frame(&mut frames_rx).await;
    assert_eq!(frame["event"], "agent_install_state_changed");
    assert_eq!(frame["payload"]["command"], "bob-1551-missing");
    assert_eq!(frame["payload"]["install"]["seq"], 1);
    assert_no_frame(&mut frames_rx).await;
    assert_eq!(cache.in_flight_len(), 0);
}

#[tokio::test]
async fn update_overview_schedules_nothing_until_the_pass_is_finished() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    bob_catalog_dir(dir.path());
    let settings = bob_settings(dir.path());
    let gate = AgentUpdateGate::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let probe = counting_probe(Arc::clone(&calls), None);

    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.status, InstallStatus::Checking);
    assert_no_frame(&mut frames_rx).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(cache.in_flight_len(), 0);

    gate.mark_started();
    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.status, InstallStatus::Checking);
    assert_no_frame(&mut frames_rx).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(cache.in_flight_len(), 0);

    gate.mark_finished(vec![]);
    update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    let started = Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < POLL_CAP, "the probe never ran");
        tokio::time::sleep(POLL_STEP).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let frame = next_frame(&mut frames_rx).await;
    assert_eq!(frame["event"], "agent_install_state_changed");
}

#[tokio::test]
async fn update_overview_serves_fresh_entries_without_probing_during_the_pass() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    bob_catalog_dir(dir.path());
    let settings = bob_settings(dir.path());
    let ticket = cache.try_begin("bob-1551-missing").expect("ticket");
    assert!(matches!(
        ticket.complete(InstallState::installed(
            "1.0".to_string(),
            Path::new("/bin/bob")
        )),
        Completion::Committed(_)
    ));
    let gate = AgentUpdateGate::new();
    gate.mark_started();
    let calls = Arc::new(AtomicUsize::new(0));

    let rows = update_overview_with(
        &handle,
        &settings,
        &gate,
        &cache,
        counting_probe(Arc::clone(&calls), None),
    )
    .await
    .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.status, InstallStatus::Installed);
    assert_eq!(rows[0].install.version.as_deref(), Some("1.0"));
    assert_eq!(rows[0].install.seq, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_no_frame(&mut frames_rx).await;
    assert_eq!(cache.in_flight_len(), 0);
}

#[tokio::test]
async fn update_overview_two_racing_calls_schedule_one_probe() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    bob_catalog_dir(dir.path());
    let settings = bob_settings(dir.path());
    let gate = AgentUpdateGate::new();
    gate.mark_finished(vec![]);
    let calls = Arc::new(AtomicUsize::new(0));
    let park = Arc::new(tokio::sync::Notify::new());
    let probe = counting_probe(Arc::clone(&calls), Some(Arc::clone(&park)));

    let (rows_a, rows_b) = tokio::join!(
        update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)),
        update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)),
    );
    let rows_a = rows_a.expect("persisted bob catalog is available");
    let rows_b = rows_b.expect("persisted bob catalog is available");
    assert_eq!(rows_a[0].install.status, InstallStatus::Checking);
    assert_eq!(rows_b[0].install.status, InstallStatus::Checking);

    let started = Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < POLL_CAP, "the probe never ran");
        tokio::time::sleep(POLL_STEP).await;
    }
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "single-flight per command");
    assert!(matches!(
        cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::InFlight
    ));
    assert_eq!(cache.in_flight_len(), 1);

    park.notify_one();
    let started = Instant::now();
    loop {
        if let CacheLookup::Fresh(state) =
            cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL)
        {
            assert_eq!(state.seq, 1);
            break;
        }
        assert!(started.elapsed() < POLL_CAP, "the probe never committed");
        tokio::time::sleep(POLL_STEP).await;
    }
    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.seq, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let frame = next_frame(&mut frames_rx).await;
    assert_eq!(frame["event"], "agent_install_state_changed");
    assert_no_frame(&mut frames_rx).await;
}

#[tokio::test]
async fn update_overview_serves_a_commit_that_lands_between_two_calls() {
    let (app, cache, _frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    bob_catalog_dir(dir.path());
    let settings = bob_settings(dir.path());
    let gate = AgentUpdateGate::new();
    gate.mark_finished(vec![]);
    let calls = Arc::new(AtomicUsize::new(0));
    let park = Arc::new(tokio::sync::Notify::new());
    let probe = counting_probe(Arc::clone(&calls), Some(Arc::clone(&park)));

    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.status, InstallStatus::Checking);
    let started = Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < POLL_CAP, "the probe never ran");
        tokio::time::sleep(POLL_STEP).await;
    }

    park.notify_one();
    let started = Instant::now();
    while !matches!(
        cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::Fresh(_)
    ) {
        assert!(started.elapsed() < POLL_CAP, "the probe never committed");
        tokio::time::sleep(POLL_STEP).await;
    }

    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.seq, 1);
    assert_eq!(rows[0].install.status, InstallStatus::Missing);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no second ticket, no second process"
    );
    assert_eq!(cache.in_flight_len(), 0);
}

#[tokio::test]
async fn overview_at_the_pass_boundary_never_opens_a_pre_pass_ticket() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    bob_catalog_dir(dir.path());
    let settings = bob_settings(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let probe = counting_probe(Arc::clone(&calls), None);

    let ticket = cache.try_begin("bob-1551-missing").expect("ticket");
    assert!(matches!(
        ticket.complete(InstallState::installed(
            "1.0".to_string(),
            Path::new("/bin/bob")
        )),
        Completion::Committed(_)
    ));
    let gate = AgentUpdateGate::new();
    gate.mark_started();

    // (a) gate read before mark_finished, cache read before invalidate_all.
    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.status, InstallStatus::Installed);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    cache.invalidate_all();

    // (b) gate read before mark_finished, cache read after invalidate_all.
    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.status, InstallStatus::Checking);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(cache.in_flight_len(), 0, "Deferred opens no ticket");

    gate.mark_finished(vec![]);

    // (c) gate read after mark_finished: any ticket carries the new generation.
    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted bob catalog is available");
    assert_eq!(rows[0].install.status, InstallStatus::Checking);
    let started = Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < POLL_CAP, "the probe never ran");
        tokio::time::sleep(POLL_STEP).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let frame = next_frame(&mut frames_rx).await;
    assert_eq!(frame["payload"]["install"]["seq"], 2);
    match cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL) {
        CacheLookup::Fresh(state) => assert_eq!(state.seq, 2),
        other => panic!("unexpected: {other:?}"),
    }
    assert_eq!(cache.generation(), 1);
}

#[tokio::test]
async fn update_overview_without_a_persisted_catalog_is_err_and_schedules_nothing() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    // NO catalog file is written: the persisted-only read is unavailable.
    let settings = bob_settings(dir.path());
    let gate = AgentUpdateGate::new();
    gate.mark_finished(vec![]);
    let calls = Arc::new(AtomicUsize::new(0));

    let unavailable = update_overview_with(
        &handle,
        &settings,
        &gate,
        &cache,
        forbidden_probe(Arc::clone(&calls)),
    )
    .await
    .expect_err("absent persisted catalog is unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(
        unavailable.path,
        dir.path()
            .join(".ac")
            .join("coding-agents")
            .join("agents.10.default.json")
            .display()
            .to_string()
    );
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "no probe may be scheduled for an unavailable base"
    );
    assert_eq!(cache.in_flight_len(), 0);
    assert_no_frame(&mut frames_rx).await;
}

#[tokio::test]
async fn update_overview_schedules_one_probe_for_duplicate_commands() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    // Two entries share the `dup-1967-missing` command: only the first
    // effective entry may be scheduled, and only once.
    let catalog_dir = dir.path().join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "agents": [
                {
                    "key": "pi",
                    "label": "Pi",
                    "description": "",
                    "color": "#000000",
                    "command": "dup-1967-missing",
                    "updateCommands": ["dup up"]
                },
                {
                    "key": "pi-alt",
                    "label": "Pi (alt)",
                    "description": "",
                    "color": "#000000",
                    "command": "dup-1967-missing",
                    "updateCommands": ["dup --custom"]
                }
            ]
        }))
        .expect("manifest json"),
    )
    .expect("write catalog");
    let settings = bob_settings(dir.path());
    let gate = AgentUpdateGate::new();
    gate.mark_finished(vec![]);
    let calls = Arc::new(AtomicUsize::new(0));
    let probe = counting_probe(Arc::clone(&calls), None);

    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted catalog");
    assert_eq!(rows.len(), 1, "one row for one distinct command");
    assert_eq!(rows[0].key, "pi");
    assert_eq!(rows[0].label, "Pi");
    assert_eq!(rows[0].update_commands, vec!["dup up"]);

    let started = Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < POLL_CAP, "the probe never ran");
        tokio::time::sleep(POLL_STEP).await;
    }
    tokio::time::sleep(QUIET_WINDOW).await;

    // Repeating the overview must not open a second, hidden probe.
    let rows = update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe))
        .await
        .expect("persisted catalog");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "dedup schedules each distinct command once"
    );
    assert_eq!(cache.in_flight_len(), 0);
    let frame = next_frame(&mut frames_rx).await;
    assert_eq!(frame["payload"]["command"], "dup-1967-missing");
    assert_eq!(frame["payload"]["install"]["seq"], 1);
    assert_no_frame(&mut frames_rx).await;
}

#[tokio::test]
async fn update_overview_skips_empty_winners_without_probing() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    // The first effective entry has an empty sequence and a later duplicate
    // is non-empty: no row, no probe, no install frame.
    let catalog_dir = dir.path().join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "agents": [
                {
                    "key": "nob",
                    "label": "Nob",
                    "description": "",
                    "color": "#000000",
                    "command": "nob-1967-missing",
                    "updateCommands": []
                },
                {
                    "key": "nob-alt",
                    "label": "Nob (alt)",
                    "description": "",
                    "color": "#000000",
                    "command": "nob-1967-missing",
                    "updateCommands": ["nob up"]
                }
            ]
        }))
        .expect("manifest json"),
    )
    .expect("write catalog");
    let settings = bob_settings(dir.path());
    let gate = AgentUpdateGate::new();
    gate.mark_finished(vec![]);
    let calls = Arc::new(AtomicUsize::new(0));

    let rows = update_overview_with(
        &handle,
        &settings,
        &gate,
        &cache,
        forbidden_probe(Arc::clone(&calls)),
    )
    .await
    .expect("persisted catalog");
    assert!(rows.is_empty(), "an empty winner produces no overview row");
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(cache.in_flight_len(), 0);
    assert_no_frame(&mut frames_rx).await;
}

#[tokio::test]
async fn schedule_post_update_probes_opens_one_ticket_per_updated_command_after_finished() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    gate.mark_finished(vec![]);
    let target = UpdateTarget {
        command: "bob-1551-missing".to_string(),
        label: "Bob".to_string(),
        commands: vec!["bob up".to_string()],
        cwd: cwd(),
    };

    schedule_post_update_probes(&handle, std::slice::from_ref(&target));
    let frame = next_frame(&mut frames_rx).await;
    assert_eq!(frame["event"], "agent_install_state_changed");
    assert_eq!(frame["payload"]["command"], "bob-1551-missing");
    assert_eq!(frame["payload"]["install"]["seq"], 1);
    assert_eq!(cache.in_flight_len(), 0);

    // A second call is served from the cache: nothing is scheduled.
    schedule_post_update_probes(&handle, std::slice::from_ref(&target));
    assert_no_frame(&mut frames_rx).await;
    schedule_post_update_probes(&handle, &[]);
    assert_no_frame(&mut frames_rx).await;

    // Single-flight with a Settings-triggered probe that is still running.
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");
    bob_catalog_dir(dir.path());
    let settings = bob_settings(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let park = Arc::new(tokio::sync::Notify::new());
    update_overview_with(
        &handle,
        &settings,
        &gate,
        &cache,
        counting_probe(Arc::clone(&calls), Some(Arc::clone(&park))),
    )
    .await
    .expect("persisted bob catalog is available");
    let started = Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < POLL_CAP, "the probe never ran");
        tokio::time::sleep(POLL_STEP).await;
    }
    schedule_post_update_probes(&handle, std::slice::from_ref(&target));
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.in_flight_len(), 1, "the scheduler got InFlight");
    park.notify_one();
    let frame = next_frame(&mut frames_rx).await;
    assert_eq!(frame["event"], "agent_install_state_changed");
    drop(app);
}
