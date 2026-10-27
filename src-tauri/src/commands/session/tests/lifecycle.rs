use super::*;

#[tokio::test]
async fn e2992_p42_invalid_prevents_spawn() {
    use crate::config::settings::{SettingsAvailability, SettingsState};

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(test_settings(), session_mgr.clone(), pty_mgr.clone());
    let cwd = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(cwd.path()).expect("canonicalize P42 session cwd"),
    )
    .to_string_lossy()
    .to_string();
    let ready = create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd).await;
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);

    // Closed exercises the same Unavailable admission error as Invalid while
    // the earlier archive/creation gates still use StagedCompatibility reads.
    // Those gates are outside P42; no active-owner forwarding is used here.
    let settings = app.state::<SettingsState>();
    settings.drain_blocking(std::time::Duration::ZERO);
    assert_eq!(settings.availability(), SettingsAvailability::Closed);
    let error = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "test-shell".to_string(),
        Vec::new(),
        cwd,
        Some("unavailable".to_string()),
        Some("claude".to_string()),
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
    .unwrap_err();
    assert_eq!(error, "settings owner unavailable (Closed)");
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, ready.id);
    close_test_coordinator(&app).await;
}

// #1063: while a real create is paused at `before_project_gate` with its pending
// row inserted, the deletion-only pending-inclusive snapshot must observe its
// working directory even though every public read hides it. This is the exact
// property the Agent Matrix delete recheck relies on.
#[tokio::test]
async fn before_project_gate_barrier_exposes_pending_create_to_deletion_snapshot_only() {
    let cwd = tempfile::tempdir().unwrap();
    let cwd_str = cwd.path().to_string_lossy().into_owned();
    // The create normalizes its cwd; key the barriers by the same value.
    let barrier_key = crate::path_utils::normalize_windows_verbatim_path(&cwd_str);
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, false));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );

    let before_gate = super::super::seed_race_barriers::install_before_project_gate(&barrier_key);
    let after_seed = super::super::seed_race_barriers::install_after_seed_before_pty(&barrier_key);

    let create = {
        let app = app.handle().clone();
        let session_mgr = Arc::clone(&session_mgr);
        let pty_mgr = Arc::clone(&pty_mgr);
        let cwd_str = cwd_str.clone();
        tokio::spawn(async move {
            super::super::create_session_inner(
                &app,
                &session_mgr,
                &pty_mgr,
                "codex".to_string(),
                Vec::new(),
                cwd_str,
                Some("race fixture".to_string()),
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
        })
    };

    let assert_pending_visible_to_deletion_only = || {
        let session_mgr = Arc::clone(&session_mgr);
        let cwd_str = cwd_str.clone();
        async move {
            assert!(
                session_mgr.read().await.list_sessions().await.is_empty(),
                "the pending create must stay hidden from public reads"
            );
            let workdirs = tokio::task::spawn_blocking(move || {
                session_mgr
                    .blocking_read()
                    .live_working_directories_for_deletion_blocking()
            })
            .await
            .unwrap();
            assert!(
                workdirs.iter().any(|dir| dir == &cwd_str),
                "the deletion-only snapshot must include the pending create workdir; got \
                     {workdirs:?}"
            );
        }
    };

    // Paused at before_project_gate, pending row inserted.
    before_gate.reached.notified().await;
    assert_pending_visible_to_deletion_only().await;
    before_gate.release.notify_one();

    // Paused at after_seed_before_pty, still pending and unfinalized.
    after_seed.reached.notified().await;
    assert_pending_visible_to_deletion_only().await;
    after_seed.release.notify_one();

    // Release the create and drive its PTY spawn to completion.
    started_rx.await.unwrap();
    release_tx.send(()).unwrap();
    create
        .await
        .expect("join create")
        .expect("create completes");
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn manual_coordinator_cascade_barrier_publishes_only_one_final_selection() {
    use tauri::Manager;

    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    std::fs::create_dir_all(root).expect("Root fixture creation succeeds");
    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let backend = Arc::new(BarrierDestroyBackend::new(started_tx, release_rx));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let [external, member_a, member_b, coordinator_id] =
        create_manual_cascade_fixture(&app, &session_mgr, &pty_mgr, cwd.path()).await;
    app.state::<crate::session::selection::SelectionCoordinator>()
        .transition(crate::session::selection::SelectionRequest::user_switch(
            member_a,
        ))
        .await
        .unwrap();
    let events = capture_session_lifecycle(&app);
    let close = {
        let app = app.handle().clone();
        tokio::spawn(async move {
            execute_manual_coordinator_destroy(&app, coordinator_id, vec![member_a, member_b], true)
                .await
        })
    };

    assert_eq!(started_rx.await.unwrap(), member_a);
    assert_eq!(
        session_mgr.read().await.selection_payload().await.id(),
        Some(member_a),
        "selection must remain stable while the cascade is barrier-held"
    );
    assert!(events.try_recv().is_err());
    release_tx.send(()).unwrap();
    let outcome = close
        .await
        .expect("join cascade close")
        .expect("cascade succeeds");
    assert_eq!(
        outcome.destroyed_ids,
        vec![member_a, member_b, coordinator_id]
    );
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(external));
    assert_eq!(
        selection.source(),
        crate::session::selection::SelectionSource::ManualClose
    );
    let observed = (0..4)
        .map(|_| {
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .0
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            "session_destroyed",
            "session_destroyed",
            "session_destroyed",
            "session_switched",
        ]
    );
    assert!(events.try_recv().is_err());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn manual_coordinator_cascade_failed_selected_member_stays_selected_without_fallback() {
    use tauri::Manager;

    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    std::fs::create_dir_all(root).expect("Root fixture creation succeeds");
    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let backend = Arc::new(BarrierDestroyBackend::new(started_tx, release_rx));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let [_external, member_a, member_b, coordinator_id] =
        create_manual_cascade_fixture(&app, &session_mgr, &pty_mgr, cwd.path()).await;
    app.state::<crate::session::selection::SelectionCoordinator>()
        .transition(crate::session::selection::SelectionRequest::user_switch(
            member_a,
        ))
        .await
        .unwrap();
    backend.fail_kill_for(member_a);
    let revision = session_mgr
        .read()
        .await
        .selection_payload()
        .await
        .revision();
    let events = capture_session_lifecycle(&app);
    let close = {
        let app = app.handle().clone();
        tokio::spawn(async move {
            execute_manual_coordinator_destroy(&app, coordinator_id, vec![member_a, member_b], true)
                .await
        })
    };
    assert_eq!(started_rx.await.unwrap(), member_a);
    release_tx.send(()).unwrap();
    let outcome = close
        .await
        .expect("join failed-member cascade")
        .expect("coordinator still closes");

    assert!(outcome
        .failed
        .iter()
        .any(|(session_id, error)| *session_id == member_a
            && error.contains("synthetic live teardown failure")));
    assert_eq!(outcome.destroyed_ids, vec![member_b, coordinator_id]);
    assert!(session_mgr
        .read()
        .await
        .get_session(member_a)
        .await
        .is_some());
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(member_a));
    assert_eq!(selection.revision(), revision);
    let observed = (0..2)
        .map(|_| {
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .0
        })
        .collect::<Vec<_>>();
    assert_eq!(observed, vec!["session_destroyed", "session_destroyed"]);
    assert!(events.try_recv().is_err());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_success_publishes_created_destroyed_selection_in_order() {
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
    let carried = SessionCommunication {
        kind: SessionCommunicationKind::RaiseHand,
        visible: true,
        updated_at: "2026-07-16T00:00:00Z".to_string(),
        message: None,
    };
    session_mgr
        .read()
        .await
        .set_communication_for_test(old_id, carried.clone())
        .await;
    let events = capture_session_lifecycle(&app);
    let settings = app.state::<crate::config::settings::SettingsState>();

    let replacement = super::super::restart_session_inner_with_activation(
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
    .expect("restart succeeds");
    let replacement_id = Uuid::parse_str(&replacement.id).unwrap();
    assert_ne!(replacement_id, old_id);
    assert!(!backend.has_session(old_id));
    assert!(backend.has_session(replacement_id));
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, replacement.id);
    assert_eq!(replacement.communication, Some(carried));
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(replacement_id));
    assert_eq!(
        selection.source(),
        crate::session::selection::SelectionSource::Restart
    );

    let observed = (0..3)
        .map(|_| {
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["session_created", "session_destroyed", "session_switched"]
    );
    assert!(events.try_recv().is_err());
    let created_payload: serde_json::Value = serde_json::from_str(&observed[0].1).unwrap();
    assert_eq!(created_payload["communication"]["kind"], "raiseHand");
    let selection_payload: serde_json::Value = serde_json::from_str(&observed[2].1).unwrap();
    assert_eq!(selection_payload["id"], replacement.id);
    assert_eq!(selection_payload["source"], "restart");
    assert_eq!(selection_payload["userInitiated"], true);
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_of_live_wg_replica_session_does_not_sessionrace() {
    // #1101: `teardown_old_for_restart` retains the old canonical row (non-Exited)
    // until the atomic commit's `mutations.remove(uuid)`, which runs AFTER the
    // create-gate. For a live WG replica restart (Suppress intent + replica cwd) the
    // gate saw `appeared_session == true` and returned Err("sessionRace"). The gate
    // must be skipped for the restart replacement create. The existing restart tests
    // use a plain tempdir (not a WG replica), so they never entered this gate.
    let (_fixture, first_cwd, _second) = strict_target_fixture();
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

    // Seed a LIVE session at the strict WG replica cwd. User intent bypasses the
    // create-gate on create and establishes a non-Exited canonical row.
    let old = create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .expect("seed live WG replica session");
    let old_id = Uuid::parse_str(&old.id).unwrap();

    let settings = app.state::<crate::config::settings::SettingsState>();
    let replacement = super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,        // agent_id
        None,        // requested_profile
        Some(false), // skip_auto_resume
        true,        // activate_after
    )
    .await
    .unwrap_or_else(|e| panic!("restart of a live WG replica session must not fail; got Err({e})"));

    let replacement_id = Uuid::parse_str(&replacement.id).unwrap();
    assert_ne!(replacement_id, old_id, "restart must replace the old row");
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1, "old row replaced, not duplicated");
    assert_eq!(rows[0].id, replacement.id);
    close_test_coordinator(&app).await;
}

// ──────────────────────────────────────────────────────────────────────
// #1535 nested-project sessionRace confinement (S1-S2)
// ──────────────────────────────────────────────────────────────────────

/// #1535 S1/S2 fixture: `strict_target_fixture` PLUS a nested project with
/// its own full workgroup inside `__agent_dev-one`:
/// `nested-proj/.ac/{_team_team, _agent_lead2, _agent_inner2,
/// wg-2-team/{__agent_lead2, __agent_inner2}}` and the #1535 bug-shaped
/// origin dir `nested-proj/.ac/_agent_phase0`. The nested replica
/// `config.json` identity is `../../_agent_inner2`; the nested team config
/// names the nested origin matrices.
fn nested_strict_target_fixture() -> (tempfile::TempDir, String, String, String) {
    let (temp, first_cwd, _second) = strict_target_fixture();
    let outer_replica = std::path::PathBuf::from(&first_cwd);

    let nested_ac_root = outer_replica.join("nested-proj").join(".ac");
    let nested_team = nested_ac_root.join("_team_team");
    let nested_wg = nested_ac_root.join("wg-2-team");
    let nested_inner2 = nested_wg.join("__agent_inner2");
    let nested_origin = nested_ac_root.join("_agent_phase0");
    for directory in [
        &nested_team,
        &nested_ac_root.join("_agent_lead2"),
        &nested_ac_root.join("_agent_inner2"),
        &nested_wg.join("__agent_lead2"),
        &nested_inner2,
        &nested_origin,
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        nested_team.join("config.json"),
        r#"{"agents":["../_agent_lead2","../_agent_inner2"],"coordinator":"../_agent_lead2"}"#,
    )
    .unwrap();
    std::fs::write(
        nested_inner2.join("config.json"),
        r#"{"identity":"../../_agent_inner2"}"#,
    )
    .unwrap();

    (
        temp,
        first_cwd,
        nested_inner2.to_string_lossy().into_owned(),
        nested_origin.to_string_lossy().into_owned(),
    )
}

/// #1535 S1: a create in a nested project (origin dir under an ancestor
/// workgroup replica, the exact bug shape) must NOT sessionRace against
/// the ANCESTOR replica's live session (pre-fix: `Err("sessionRace")`).
#[tokio::test]
async fn nested_project_create_does_not_sessionrace_against_ancestor_live_session() {
    let (temp, first_cwd, _nested_replica, nested_origin) = nested_strict_target_fixture();
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

    // Seed a LIVE session at the OUTER replica; the ancestor's live session
    // must not gate a nested-project create.
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .expect("seed live ancestor WG replica session");

    let created = create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &nested_origin,
        CreateSelectionIntent::Background,
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "nested-project create must not sessionRace against the ancestor session; got Err({e})"
        )
    });
    assert_eq!(created.working_directory, nested_origin);
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 2, "ancestor + nested sessions both exist");
    assert!(
        rows.iter()
            .any(|row| row.working_directory == nested_origin),
        "the nested-project session row must exist"
    );
    let _ = temp;
    close_test_coordinator(&app).await;
}

/// #1535 S2: a nested project with its OWN replica (a) does not race
/// against the ancestor's live session, and (b) still gates against its
/// own replica's duplicate.
#[tokio::test]
async fn nested_project_create_gates_against_its_own_replica_only() {
    let (temp, first_cwd, nested_replica, nested_origin) = nested_strict_target_fixture();
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

    // (a) Live ancestor session + nested replica create -> Ok (the nested
    // replica's gate scopes to the nested project, never the ancestor).
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .expect("seed live ancestor WG replica session");
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &nested_replica,
        CreateSelectionIntent::Background,
    )
    .await
    .unwrap_or_else(|e| {
        panic!("nested replica create must not race the ancestor session; got Err({e})")
    });

    // (b) A live session on the nested replica itself + another Background
    // create at the same nested replica -> the nested project's OWN
    // duplicate still sessionRaces.
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &nested_replica,
        CreateSelectionIntent::User,
    )
    .await
    .expect("seed live nested replica session");
    let err = create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &nested_replica,
        CreateSelectionIntent::Background,
    )
    .await
    .expect_err("the nested replica's own duplicate must still gate");
    assert_eq!(err, "sessionRace");
    let _ = (temp, nested_origin);
    close_test_coordinator(&app).await;
}
