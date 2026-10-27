use super::*;

#[tokio::test]
async fn restart_after_route_loss_cleans_old_external_state_once_before_replacement() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    session_mgr
        .read()
        .await
        .set_telegram_bot_id(old_id, Some("old-bot".to_string()))
        .await;
    let telegram = app.state::<crate::telegram::manager::TelegramBridgeState>();
    telegram.lock().await.insert_test_bridge(old_id, "old-bot");
    app.state::<crate::DetachedSessionsState>()
        .lock()
        .unwrap()
        .insert(old_id);

    backend.lose_route(old_id);
    pty_mgr.lock().unwrap().remove_route_if_kind(
        old_id,
        crate::pty::backend::SessionBackendKind::LocalProcess,
    );
    let (spawn_started_tx, spawn_started_rx) = tokio::sync::oneshot::channel();
    let (spawn_release_tx, spawn_release_rx) = tokio::sync::oneshot::channel();
    backend.gate_spawn(2, spawn_started_tx, spawn_release_rx);
    let events = capture_session_lifecycle(&app);
    let restart = {
        let app_handle = app.handle().clone();
        let session_mgr = Arc::clone(&session_mgr);
        let pty_mgr = Arc::clone(&pty_mgr);
        let settings = app
            .state::<crate::config::settings::SettingsState>()
            .inner()
            .clone();
        tokio::spawn(async move {
            super::super::restart_session_inner_with_activation(
                &app_handle,
                &session_mgr,
                &pty_mgr,
                &settings,
                old_id,
                None,
                None,
                Some(false),
                true,
            )
            .await
        })
    };

    let replacement_id = spawn_started_rx.await.expect("replacement spawn starts");
    assert!(!telegram.lock().await.has_bridge(old_id));
    assert_eq!(telegram.lock().await.test_detach_count(old_id), 1);
    assert!(!app
        .state::<crate::DetachedSessionsState>()
        .lock()
        .unwrap()
        .contains(&old_id));
    let mut route_loss = {
        let sender = app
            .state::<crate::session::selection::SelectionCoordinator>()
            .container_lifecycle_sender();
        tokio::spawn(async move { sender.route_lost(old_id, 91).await })
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut route_loss)
            .await
            .is_err(),
        "late route-loss reconciliation must serialize behind restart"
    );
    spawn_release_tx.send(()).unwrap();
    let replacement = restart
        .await
        .expect("join restart")
        .expect("restart succeeds after old route loss");
    assert_eq!(replacement.id, replacement_id.to_string());
    assert_eq!(
        route_loss
            .await
            .expect("join route-loss callback")
            .expect("late route-loss callback"),
        crate::session::selection::CriticalAdmissionOutcome::Completed(())
    );
    assert_eq!(telegram.lock().await.test_detach_count(old_id), 1);
    assert!(session_mgr.read().await.get_session(old_id).await.is_none());
    assert_eq!(
        session_mgr.read().await.selection_payload().await.id(),
        Some(replacement_id)
    );
    let observed = (0..3)
        .map(|_| {
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .0
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec!["session_created", "session_destroyed", "session_switched"]
    );
    assert!(events.try_recv().is_err());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_pre_teardown_failure_preserves_old_row_route_selection_and_events() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    let events = capture_session_lifecycle(&app);
    let settings = app.state::<crate::config::settings::SettingsState>();
    settings.write().await.archived_project_paths = vec![cwd.path().to_string_lossy().to_string()];

    let error = super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(true),
        true,
    )
    .await
    .expect_err("restart teardown must fail");
    assert!(
        error.contains("Cannot start a session in archived project"),
        "{error}"
    );
    assert!(backend.has_session(old_id));
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 1);
    assert_eq!(
        session_mgr.read().await.selection_payload().await.id(),
        Some(old_id)
    );
    assert!(events.try_recv().is_err());
    let tracker = pty_mgr.lock().unwrap().response_close_state();
    let authority = match super::super::prepare_lifecycle_entry_authority(
        app.handle(),
        Uuid::new_v4(),
        Some(old_id),
        false,
        None,
    )
    .await
    .expect("real lifecycle authority")
    {
        super::super::LifecycleAuthorityAdmission::Ready(authority) => authority,
        _ => panic!("healthy Foreign fixture must acquire real authority"),
    };
    assert!(authority.has_owned_claim());
    assert!(authority
        .tracker
        .as_ref()
        .is_some_and(|actual| actual.same_state(&tracker)));
    tracker.fail_closed();
    assert!(
        !authority.release(),
        "claimed failed tracker cannot certify release"
    );
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_post_teardown_spawn_failure_destroys_old_and_publishes_one_null() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    let events = capture_session_lifecycle(&app);
    backend.fail_spawn(2);
    let settings = app.state::<crate::config::settings::SettingsState>();

    let error = super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(true),
        true,
    )
    .await
    .expect_err("replacement spawn must fail");
    assert!(
        error.contains("synthetic scripted spawn failure"),
        "{error}"
    );
    assert!(session_mgr.read().await.list_sessions().await.is_empty());
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), None);
    assert_eq!(
        selection.source(),
        crate::session::selection::SelectionSource::Restart
    );
    assert!(!backend.has_session(old_id));

    let destroyed = events
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("old destroy event");
    let switched = events
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("restart null event");
    assert_eq!(destroyed.0, "session_destroyed");
    assert_eq!(switched.0, "session_switched");
    assert!(events.try_recv().is_err());
    let selection_payload: serde_json::Value = serde_json::from_str(&switched.1).unwrap();
    assert!(selection_payload["id"].is_null());
    assert_eq!(selection_payload["source"], "restart");
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn dormant_restart_failure_retains_exact_exit_and_emits_no_lifecycle_event() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    backend.kill(old_id).unwrap();
    app.state::<crate::session::selection::SelectionCoordinator>()
        .container_lifecycle_sender()
        .route_lost(old_id, 23)
        .await
        .expect("reconcile dormant route");
    let events = capture_session_lifecycle(&app);
    backend.fail_spawn(2);
    let settings = app.state::<crate::config::settings::SettingsState>();

    super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(false),
        true,
    )
    .await
    .expect_err("dormant replacement spawn must fail");
    let retained = session_mgr
        .read()
        .await
        .get_session(old_id)
        .await
        .expect("dormant old row retained");
    assert_eq!(retained.status, SessionStatus::Exited(23));
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(old_id));
    assert_eq!(selection.status(), Some(&SessionStatus::Exited(23)));
    assert!(events.try_recv().is_err());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn create_session_inner_rolls_back_pre_created_session_on_spawn_error() {
    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let err = super::super::create_session_inner(
        &app_handle,
        &session_mgr,
        &pty_mgr,
        "missing-ac-test-command".to_string(),
        Vec::new(),
        temp.path().to_string_lossy().to_string(),
        None,
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        // #973 - headless caller: no terminal to measure, keep 120x30.
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect_err("spawn should fail");

    assert!(err.contains("synthetic spawn failure"), "{err}");
    assert!(session_mgr.read().await.list_sessions().await.is_empty());
    tokio::time::timeout(Duration::from_secs(1), async {
        while backend.killed().len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reserved rollback kills the failed spawn exactly once");
    assert_eq!(backend.killed(), backend.spawned());
    assert_eq!(backend.killed().len(), 1);
}

/// #973 - THE REGRESSION. Red against main, which hardcodes `cols: 120, rows: 30`.
///
/// AC used to open every ConPTY at 120x30 and let the frontend correct it 300-500 ms
/// later. That correction lands inside a coding agent's TUI startup, and a resize there
/// makes Codex redraw its still-empty viewport and lose the wakeup for the content that
/// becomes ready straight after: a blank terminal, alive, until any key is pressed.
///
/// Measured outside AC, on a bare ConPTY: opened at 120x30 and then resized in the
/// window, Codex comes up blank 8 times in 10. Opened at the size the view actually
/// wants, and never resized, 0 times in 10.
///
/// This pins the plumbing: the size the caller supplies is the size the PTY is opened
/// at. It does not, and cannot, prove Codex stops hanging - that needs a real child in a
/// real ConPTY inside a ~100 ms race. The harness is the proof of that.
#[tokio::test]
async fn privileged_preheld_target_gate_does_not_reenter_and_rechecks_pending_or_live() {
    let (_fixture, first_cwd, _) = strict_target_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let target =
        crate::config::teams::verify_pty_input_replica_cwd(std::path::Path::new(&first_cwd))
            .unwrap();
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let gate = state.target_gate.as_ref().unwrap().clone();
    let stripe = gate
        .acquire_target_lock(&target.canonical_fqn)
        .await
        .unwrap();
    let exact = gate.acquire_exact(&target.canonical_fqn).await;
    let ownership = gate
        .target_ownership(&target.canonical_fqn, &stripe, &exact)
        .unwrap();

    let pending = pty_mgr
        .lock()
        .unwrap()
        .mark_spawning(&first_cwd, "pending race");
    let pending_error = super::super::create_session_inner_with_pty_target_ownership(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        first_cwd.clone(),
        Some("privileged target".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::Background,
        &ownership,
    )
    .await
    .unwrap_err();
    assert_eq!(pending_error, "sessionRace");
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 0);
    drop(pending);

    let created = super::super::create_session_inner_with_pty_target_ownership(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        first_cwd.clone(),
        Some("privileged target".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::Background,
        &ownership,
    )
    .await
    .unwrap();
    let super::super::PtyTargetCreateStep::Finalized(created) = created else {
        panic!("Foreign create must actually finalize");
    };
    assert_eq!(created.working_directory, first_cwd);
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);

    let live_error = super::super::create_session_inner_with_pty_target_ownership(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        first_cwd,
        Some("stale background create".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::Background,
        &ownership,
    )
    .await
    .unwrap_err();
    assert_eq!(live_error, "sessionRace");
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ordinary_strict_create_is_fail_soft_when_pty_store_is_error_or_missing() {
    for store_state in [SessionTestStoreState::Error, SessionTestStoreState::Missing] {
        let (_fixture, first_cwd, second_cwd) = strict_target_fixture();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app_with_store(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
            store_state,
        );

        let created = create_target_for_test(
            &app,
            &session_mgr,
            &pty_mgr,
            &first_cwd,
            CreateSelectionIntent::User,
        )
        .await;

        assert!(
            created.is_ok(),
            "ordinary create must not depend on the specialized PTY store: {created:?}"
        );
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);

        let restore =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        let restored = super::super::create_session_inner_for_restore(
            &restore,
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            second_cwd,
            Some("restore target".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert!(
            restored.is_ok(),
            "startup restore must not depend on the specialized PTY store: {restored:?}"
        );
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
        close_test_coordinator(&app).await;
    }
}

#[tokio::test]
async fn sequential_user_same_target_creates_remain_compatible() {
    let (_fixture, first_cwd, _) = strict_target_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .unwrap();
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .unwrap();
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 2);
}

#[tokio::test]
async fn different_strict_targets_create_concurrently() {
    let (_fixture, first_cwd, second_cwd) = strict_target_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    backend.gate_spawn(1, started_tx, release_rx);
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let first_handle = app.handle().clone();
    let first_sessions = Arc::clone(&session_mgr);
    let first_pty = Arc::clone(&pty_mgr);
    let first_task = tokio::spawn(async move {
        super::super::create_session_inner(
            &first_handle,
            &first_sessions,
            &first_pty,
            "codex".to_string(),
            Vec::new(),
            first_cwd,
            Some("first target".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
    });
    started_rx.await.unwrap();
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        create_target_for_test(
            &app,
            &session_mgr,
            &pty_mgr,
            &second_cwd,
            CreateSelectionIntent::User,
        ),
    )
    .await
    .expect("a different target must not wait for the first exact gate")
    .unwrap();
    assert_eq!(second.working_directory, second_cwd);
    release_tx.send(()).unwrap();
    first_task.await.unwrap().unwrap();
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn concurrent_same_target_creates_serialize_on_the_exact_gate() {
    let (_fixture, first_cwd, _) = strict_target_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    // Gate the first spawn so the winning create holds the exact target gate.
    backend.gate_spawn(1, started_tx, release_rx);
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );

    // Task A wins the exact gate for the target and blocks inside its spawn.
    let a_task = {
        let handle = app.handle().clone();
        let sessions = Arc::clone(&session_mgr);
        let pty = Arc::clone(&pty_mgr);
        let cwd = first_cwd.clone();
        tokio::spawn(async move {
            super::super::create_session_inner(
                &handle,
                &sessions,
                &pty,
                "codex".to_string(),
                Vec::new(),
                cwd,
                Some("first same-target".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None,
                CreateSelectionIntent::Background,
            )
            .await
        })
    };
    started_rx.await.unwrap();

    // Task B is a concurrent background create for the SAME target. It must
    // block on the exact gate that A holds, not race into the missing-target
    // window beside A.
    let b_task = {
        let handle = app.handle().clone();
        let sessions = Arc::clone(&session_mgr);
        let pty = Arc::clone(&pty_mgr);
        let cwd = first_cwd.clone();
        tokio::spawn(async move {
            super::super::create_session_inner(
                &handle,
                &sessions,
                &pty,
                "codex".to_string(),
                Vec::new(),
                cwd,
                Some("second same-target".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None,
                CreateSelectionIntent::Background,
            )
            .await
        })
    };

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !b_task.is_finished(),
        "a second same-target create must block on the exact gate while the first holds it"
    );
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        1,
        "no second spawn may start while the exact target gate is held"
    );

    // Release A; it finalizes a live session and drops the gate. B then wins the
    // gate, rechecks live/pending state, and refuses to spawn a duplicate.
    release_tx.send(()).unwrap();
    let created = a_task.await.unwrap().unwrap();
    assert_eq!(created.working_directory, first_cwd);
    let raced = b_task.await.unwrap().unwrap_err();
    assert_eq!(
        raced, "sessionRace",
        "the serialized loser must observe the now-live target and not create beside it"
    );
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        1,
        "the serialized loser never spawned a duplicate"
    );
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 1);
}

#[tokio::test]
async fn create_session_opens_the_pty_at_the_size_the_view_supplied() {
    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let _ = super::super::create_session_inner(
        &app_handle,
        &session_mgr,
        &pty_mgr,
        "missing-ac-test-command".to_string(),
        Vec::new(),
        temp.path().to_string_lossy().to_string(),
        None,
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        Some(crate::pty::backend::PtyViewport::from_fit(74, 23)),
        CreateSelectionIntent::User,
    )
    .await;

    assert_eq!(
        backend.sizes(),
        vec![(74, 23)],
        "the PTY must be opened at the size the view fitted to, not at 120x30: \
             opening at the wrong size is what forces the startup resize that loses \
             Codex's first render (#973)"
    );
}

/// #973 - backward compatibility. Every headless caller (startup restore, the delivery
/// loop, the phone mailbox, the CLI, tests) has no terminal to measure and must keep
/// AC's historical 120x30. Those sessions are never attached to a view, so nothing
/// resizes them, and none of them ever hit this bug.
#[tokio::test]
async fn create_session_without_a_view_keeps_the_historical_120x30() {
    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let _ = super::super::create_session_inner(
        &app_handle,
        &session_mgr,
        &pty_mgr,
        "missing-ac-test-command".to_string(),
        Vec::new(),
        temp.path().to_string_lossy().to_string(),
        None,
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None, // no view
        CreateSelectionIntent::User,
    )
    .await;

    assert_eq!(
        backend.sizes(),
        vec![(120, 30)],
        "a caller with no view must behave exactly as it did before #973"
    );
}

/// #973 - opening a 0-column ConPTY would be worse than the bug we are fixing, so a
/// degenerate fitted size must fall back rather than be honoured.
///
/// It does not come from xterm: `fit()` clamps to MINIMUM_COLS = 2 / MINIMUM_ROWS = 1. It is
/// guarded because this is a `u16` boundary that anything upstream can hand a 0 to, and
/// because the cost of being wrong is a terminal with no screen at all.
#[tokio::test]
async fn a_degenerate_fitted_size_falls_back_instead_of_opening_a_zero_column_pty() {
    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let _ = super::super::create_session_inner(
        &app_handle,
        &session_mgr,
        &pty_mgr,
        "missing-ac-test-command".to_string(),
        Vec::new(),
        temp.path().to_string_lossy().to_string(),
        None,
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        Some(crate::pty::backend::PtyViewport::from_fit(0, 0)),
        CreateSelectionIntent::User,
    )
    .await;

    assert_eq!(
        backend.sizes(),
        vec![(120, 30)],
        "a 0x0 fit must fall back to the default, never open a 0-column ConPTY"
    );
}

/// #2411 T1: the coordinator-create helper seeds the badge and clears both
/// close markers. Deleting any of its three clock calls fails this test.
#[test]
fn note_coordinator_create_on_app_seeds_and_clears_both_markers() {
    let app = tauri::test::mock_builder()
        .manage(crate::app_state::AppOwners::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(Default::default()));
    app.manage(Arc::clone(&clocks));
    let handle = app.handle().clone();
    let cwd = "C:/ac-test/project/.ac/wg-2411-dev-team/__agent_tech-lead";
    let fqn = crate::config::teams::agent_fqn_from_path(cwd);

    // (c) absent fqn -> badge seeded.
    assert!(clocks.lock().unwrap().last_user_message_at(&fqn).is_none());
    crate::commands::session::note_coordinator_create_on_app(&handle, cwd)
        .expect("clocks managed")
        .succeeded();
    assert!(clocks.lock().unwrap().last_user_message_at(&fqn).is_some());

    // (a) auto-closed marker cleared.
    assert!(clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&fqn, chrono::Utc::now()));
    crate::commands::session::note_coordinator_create_on_app(&handle, cwd)
        .expect("clocks managed")
        .succeeded();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);

    // (b) manual-close marker cleared.
    assert!(clocks
        .lock()
        .unwrap()
        .mark_manually_closed(&fqn, chrono::Utc::now()));
    crate::commands::session::note_coordinator_create_on_app(&handle, cwd)
        .expect("clocks managed")
        .succeeded();
    assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), None);
}
