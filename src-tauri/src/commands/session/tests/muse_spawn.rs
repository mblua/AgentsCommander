use super::*;

/// #1873 - Root agent-picker selection on a LIVE Muse record omits the restart
/// flag; `super::super::effective_restart_skip_auto_resume(None)` is fresh, and
/// `execute_restart_transaction` tears the old runtime down once and spawns
/// exactly one replacement with configured plain muse.
#[tokio::test]
async fn muse_live_root_selection_is_one_fresh_replacement() {
    use tauri::Manager;

    assert!(super::super::effective_restart_skip_auto_resume(None));
    let settings = muse_test_settings("muse");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);

    // The live record was a resume launch, so the replacement's plain argv is
    // observable as the ABSENCE of tokens the first spec carried.
    let old = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse live root".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("live Muse launch");
    let old_id = Uuid::parse_str(&old.id).unwrap();
    assert!(backend.has_session(old_id), "the record is live");
    assert_eq!(
        backend.specs.lock().unwrap()[0].args,
        expected_muse_resume_argv()
    );

    let settings_state = app.state::<crate::config::settings::SettingsState>();
    let replacement = super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings_state.inner(),
        old_id,
        None,
        None,
        None, // restart flag omitted: backend defaults fresh
        true,
    )
    .await
    .expect("one fresh replacement");
    let new_id = Uuid::parse_str(&replacement.id).unwrap();
    assert_ne!(new_id, old_id, "replacement is a new id");
    assert_eq!(replacement.agent_kind, Some(CodingAgentKind::Muse));
    assert!(replacement.shell_args.is_empty());
    assert_eq!(replacement.effective_shell_args.as_deref(), Some(&[][..]));

    assert_eq!(
        backend.killed.lock().unwrap().clone(),
        vec![old_id],
        "one teardown"
    );
    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 2, "exactly one replacement spawn");
        assert_eq!(specs[1].id, new_id);
        assert_eq!(specs[1].cmd, "muse");
        assert!(
            specs[1].args.is_empty(),
            "replacement must be configured plain muse: {:?}",
            specs[1].args
        );
        assert!(!specs[1].args.iter().any(|a| a == "resume" || a == "--last"));
    }
    assert!(!backend.has_session(old_id));
    assert!(backend.has_session(new_id));
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, replacement.id, "old row removed");
    close_test_coordinator(&app).await;
}

#[tokio::test]
#[ignore = "#2854 WIP/technical debt: simulator-dependent C19 reclaim test"]
async fn retained_exited_respawn_c19_completed_restarts_reclaim_more_than_64_entries() {
    let settings = muse_test_settings("muse");
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings.clone(), manager.clone(), pty.clone());
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().into_owned();
    let spawn = muse_spawn_for(&settings, &cwd);
    let initial = super::super::create_session_inner(
        app.handle(),
        &manager,
        &pty,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("C19 actual successful entries".into()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        true,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .unwrap();
    let mut current = Uuid::parse_str(&initial.id).unwrap();
    let settings = app.state::<crate::config::settings::SettingsState>();
    for _ in 0..66 {
        let previous = current;
        let result = super::super::restart_session_inner_with_activation(
            app.handle(),
            &manager,
            &pty,
            settings.inner(),
            previous,
            None,
            None,
            Some(true),
            true,
        )
        .await
        .expect("real successful entry releases capacity before next admission");
        current = Uuid::parse_str(&result.id).unwrap();
        assert_ne!(current, previous);
        assert!(!backend.has_session(previous));
        assert!(backend.has_session(current));
        assert!(manager.read().await.get_session(previous).await.is_none());
    }
    assert_eq!(backend.specs.lock().unwrap().len(), 67);
    assert_eq!(backend.killed.lock().unwrap().len(), 66);
    assert_eq!(manager.read().await.list_sessions().await.len(), 1);
    let report = app
        .state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
    assert!(
        report.persistence_safe,
        "completed entries must be terminal: {:?}",
        report.retained
    );
}

/// #1873 - a spawn-time `Err` makes exactly one resume attempt, surfaces that one
/// diagnostic, publishes no created event, leaves no row, and never retries plain.
#[tokio::test]
async fn muse_spawn_error_is_not_retried_fresh() {
    let settings = muse_test_settings("muse");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let events = capture_session_lifecycle(&app);
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);

    let err = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse spawn error".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect_err("the resume attempt fails at spawn");
    assert!(err.contains("synthetic spawn failure"), "{err}");

    // Exactly one attempt, rolled back exactly once, no row, no retry.
    assert_eq!(backend.spawned().len(), 1, "one resume attempt");
    tokio::time::timeout(Duration::from_secs(1), async {
        while backend.killed().len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rollback removes the pending row exactly once");
    assert_eq!(backend.killed(), backend.spawned());
    assert!(session_mgr.read().await.list_sessions().await.is_empty());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        backend.spawned().len(),
        1,
        "no automatic plain-Muse retry follows a failed resume"
    );
    let mut created_events = 0;
    while let Ok((name, _payload)) = events.try_recv() {
        assert_ne!(
            name, "session_created",
            "a failed spawn publishes no created event"
        );
        created_events += usize::from(name == "session_created");
    }
    assert_eq!(created_events, 0);
    close_test_coordinator(&app).await;
}

/// #1873 - through the REAL local backend: a trusted resume launch of a `muse`
/// executable that journals argv and exits 41 succeeds at create (one retained
/// row, one created event, wire "muse", argv `[muse-path, "resume", "--last"]`),
/// then the monitor reports one child-initiated exit with duplicate suppression;
/// no second event, no plain fallback. The retained row still permits a
/// user-selected fresh Restart Session.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn muse_post_spawn_nonzero_exit_is_reported_once_without_fresh_fallback() {
    use std::os::unix::fs::PermissionsExt;
    use tauri::Manager;

    let bin = tempfile::tempdir().unwrap();
    let journal = bin.path().join("argv.journal");
    let muse_path = bin.path().join("muse");
    std::fs::write(
        &muse_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 41\n",
            journal.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&muse_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let muse_path_str = muse_path.to_string_lossy().to_string();
    assert_eq!(
        CodingAgentKind::detect(&muse_path_str, &[]),
        Some(CodingAgentKind::Muse)
    );

    let settings = muse_test_settings(&muse_path_str);
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    // The real local backend: the child monitor and exit attribution live in it.
    let git_app = Box::leak(Box::new(
        crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build git watcher app"),
    ));
    let git_watcher = crate::pty::git_watcher::GitWatcher::new(
        Arc::clone(&session_mgr),
        git_app.handle().clone(),
    );
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let output_senders: crate::telegram::manager::OutputSenderMap =
        Arc::new(Mutex::new(HashMap::new()));
    let backend = Arc::new(crate::pty::local_backend::LocalProcessBackend::new(
        output_senders,
        idle_detector,
        git_watcher,
        None,
    ));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let events = capture_session_lifecycle(&app);
    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);
    assert_eq!(spawn.shell, muse_path_str);
    assert!(spawn.shell_args.is_empty());

    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse exit 41".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("spawn succeeds; the nonzero exit is post-spawn");
    let id = Uuid::parse_str(&created.id).unwrap();
    assert_eq!(created.agent_kind, Some(CodingAgentKind::Muse));
    assert_eq!(
        serde_json::to_value(&created).unwrap()["agentKind"],
        serde_json::json!("muse")
    );
    assert!(created.shell_args.is_empty());
    assert_eq!(
        created.effective_shell_args.clone().unwrap_or_default(),
        muse_resume_tokens()
    );

    // Exactly one created event, for this row.
    let (name, payload) = events
        .recv_timeout(Duration::from_secs(5))
        .expect("the create publishes its one created event");
    assert_eq!(name, "session_created");
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["id"], serde_json::json!(created.id));
    assert_eq!(payload["agentKind"], serde_json::json!("muse"));

    // The monitor observes the bounded child exit and reports it once.
    let record = crate::pty::spawn_diagnostics::record_for(id)
        .expect("launch provenance is recorded for the spawned child");
    assert_eq!(
        record.argv(),
        [
            muse_path_str.clone(),
            "resume".to_string(),
            "--last".to_string()
        ]
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let liveness = loop {
        if let Some(liveness) = record.final_liveness() {
            if record.exit_reported() {
                break liveness;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the child monitor must report the exit within the bound"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(
        matches!(
            liveness,
            crate::pty::spawn_diagnostics::ChildLiveness::Exited {
                code: 41,
                success: false
            }
        ),
        "bounded nonzero exit must be observed as Exited(41,false), got {liveness:?}"
    );
    assert_eq!(
        record.exit_cause(),
        Some(crate::pty::spawn_diagnostics::ExitCause::ChildInitiated)
    );
    assert!(
        !record.log_child_exit(
            crate::pty::spawn_diagnostics::ExitCause::ChildInitiated,
            &liveness,
            "duplicate-suppression-probe"
        ),
        "a second report of the same exit is suppressed"
    );

    // One journal entry: the resume argv. No plain fallback ran.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let journaled = std::fs::read_to_string(&journal).unwrap();
    assert_eq!(journaled.lines().collect::<Vec<_>>(), vec!["resume --last"]);

    // No second created event and no destroy: the row is retained.
    while let Ok((name, _)) = events.try_recv() {
        assert!(
            name != "session_created" && name != "session_destroyed",
            "unexpected lifecycle event after the child exit: {name}"
        );
    }
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1, "the retained row survives the nonzero exit");
    assert_eq!(rows[0].id, created.id);
    assert!(rows[0].shell_args.is_empty(), "configured argv unchanged");

    // The retained row permits a deliberate user-selected fresh Restart Session:
    // one more process attempt, with configured plain argv (empty journal line).
    let settings_state = app.state::<crate::config::settings::SettingsState>();
    let restarted = super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings_state.inner(),
        id,
        None,
        None,
        None,
        true,
    )
    .await
    .expect("user-selected fresh Restart Session is available");
    assert_ne!(restarted.id, created.id);
    assert_eq!(restarted.effective_shell_args.as_deref(), Some(&[][..]));
    let restart_deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let journaled = std::fs::read_to_string(&journal).unwrap();
        let lines: Vec<&str> = journaled.lines().collect();
        if lines.len() == 2 {
            assert_eq!(lines, vec!["resume --last", ""]);
            break;
        }
        assert!(
            std::time::Instant::now() < restart_deadline,
            "the fresh restart must journal exactly one plain attempt: {lines:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    close_test_coordinator(&app).await;
}
