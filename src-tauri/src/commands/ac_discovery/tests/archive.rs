use super::*;

/// Regression for the #1080 CI failure: on a Windows runner whose `%TEMP%` is
/// an 8.3 short name (`RUNNER~1`), `tempdir()` yields short-form paths and a
/// relative `.` archive absolutises to that short form. Storing the raw
/// (short) path lets the lexical archive lookup match, while the archived
/// RUNTIME field is the decoder's canonical (long) form. Archived must NOT be
/// empty (the pre-fix bug: canonical-stored vs short-lookup did not match, the
/// project stayed active, and the archived duplicate was dropped by dedupe).
#[cfg(windows)]
#[tokio::test]
async fn archive_relative_path_matches_across_short_name_temp() {
    let _cwd_lock = cwd_lock().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-archive-relative");
    std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
    // Force the runner's condition: access the project via its 8.3 short form.
    let short = short_path(&project);
    let path = short.to_string_lossy().to_string();
    let archived_expected = canonical_display(&project);
    let settings = AppSettings {
        project_paths: vec![path.clone()],
        project_path: Some(path.clone()),
        ..AppSettings::default()
    };
    let (app, state, _rx, session_mgr, pty_mgr) = archive_command_app(settings);
    let app_handle = app.handle().clone();
    let prev = std::env::current_dir().expect("current dir");
    let _guard = CwdGuard(prev);
    std::env::set_current_dir(&short).expect("set cwd");

    archive_project_inner_with_settings_path(
        &app_handle,
        &state,
        &session_mgr,
        &pty_mgr,
        ".",
        Some(&settings_path),
    )
    .await
    .expect("archive relative project on a short-name temp");

    let archived = state.read().await.archived_project_paths.clone();
    assert_eq!(
        archived,
        vec![archived_expected],
        "archived must be the canonical path, never empty"
    );
    assert!(
        state.read().await.project_paths.is_empty(),
        "the project must be removed from the active list"
    );
}

#[test]
fn archive_blockers_names_live_pty_sessions_under_project() {
    let (_temp, project, agent, _) = archive_scope();
    let snapshot = archive_snapshot(
        vec![(
            archive_test_session("live", &agent, SessionStatus::Running, false),
            true,
        )],
        Vec::new(),
    );

    assert_eq!(archive_blockers(&snapshot, &project), vec!["live"]);
}

#[test]
fn archive_blockers_ignores_exited_session_under_project() {
    let (_temp, project, agent, _) = archive_scope();
    let snapshot = archive_snapshot(
        vec![(
            archive_test_session("done", &agent, SessionStatus::Exited(0), false),
            true,
        )],
        Vec::new(),
    );

    assert!(archive_blockers(&snapshot, &project).is_empty());
}

#[test]
fn archive_blockers_ignores_running_session_with_no_pty() {
    let (_temp, project, agent, _) = archive_scope();
    let snapshot = archive_snapshot(
        vec![(
            archive_test_session("phantom", &agent, SessionStatus::Running, false),
            false,
        )],
        Vec::new(),
    );

    assert!(archive_blockers(&snapshot, &project).is_empty());
}

#[test]
fn archive_blockers_ignores_root_agent_session() {
    let (_temp, project, agent, _) = archive_scope();
    let snapshot = archive_snapshot(
        vec![(
            archive_test_session("root", &agent, SessionStatus::Running, true),
            true,
        )],
        Vec::new(),
    );

    assert!(archive_blockers(&snapshot, &project).is_empty());
}

#[test]
fn archive_blockers_ignores_root_agent_by_dir_name_when_flag_is_false() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let root_dir = project
        .join(".ac")
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root_dir).expect("root dir");
    let snapshot = archive_snapshot(
        vec![(
            archive_test_session(
                "root",
                &root_dir.to_string_lossy(),
                SessionStatus::Running,
                false,
            ),
            true,
        )],
        Vec::new(),
    );

    assert!(archive_blockers(&snapshot, &project.to_string_lossy()).is_empty());
}

#[test]
fn archive_blockers_ignores_sessions_in_other_projects() {
    let (_temp, project, _, other) = archive_scope();
    let snapshot = archive_snapshot(
        vec![(
            archive_test_session("other", &other, SessionStatus::Running, false),
            true,
        )],
        Vec::new(),
    );

    assert!(archive_blockers(&snapshot, &project).is_empty());
}

#[test]
fn archive_blocked_message_caps_named_sessions_at_three() {
    let names = vec![
        "one".to_string(),
        "two".to_string(),
        "three".to_string(),
        "four".to_string(),
    ];

    let message = archive_blocked_message(&names);

    assert!(message.contains("one, two, three, and 1 more"), "{message}");
    assert!(!message.contains("four"), "{message}");
}

#[test]
fn archive_blockers_names_pending_spawn_under_project() {
    let (_temp, project, agent, _) = archive_scope();
    let snapshot = archive_snapshot(
        Vec::new(),
        vec![PendingSpawn {
            cwd: agent,
            label: "spawning".to_string(),
        }],
    );

    assert_eq!(archive_blockers(&snapshot, &project), vec!["spawning"]);
}

#[test]
fn archive_blockers_ignores_pending_root_agent_by_dir_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let root_dir = project
        .join(".ac")
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root_dir).expect("root dir");
    let snapshot = archive_snapshot(
        Vec::new(),
        vec![PendingSpawn {
            cwd: root_dir.to_string_lossy().to_string(),
            label: "root".to_string(),
        }],
    );

    assert!(archive_blockers(&snapshot, &project.to_string_lossy()).is_empty());
}

#[test]
fn archive_blockers_dedupes_pending_mark_and_live_record_by_name() {
    let (_temp, project, agent, _) = archive_scope();
    let snapshot = archive_snapshot(
        vec![(
            archive_test_session("same", &agent, SessionStatus::Running, false),
            true,
        )],
        vec![PendingSpawn {
            cwd: agent,
            label: "same".to_string(),
        }],
    );

    assert_eq!(archive_blockers(&snapshot, &project), vec!["same"]);
}

#[test]
fn project_archive_changed_serializes_frontend_wire_shape() {
    let payload = serde_json::to_value(ProjectArchiveChanged {
        path: "C:/Projects/App".to_string(),
        folder_name: "App".to_string(),
        archived: false,
        reason: ArchiveChangeReason::AutoUnarchive,
        session_name: Some("Dev Session".to_string()),
    })
    .expect("serialize payload");

    assert_eq!(payload["folderName"], json!("App"));
    assert_eq!(payload["sessionName"], json!("Dev Session"));
    assert_eq!(payload["reason"], json!("autoUnarchive"));
    assert!(payload.get("folder_name").is_none());
    assert!(payload.get("session_name").is_none());
}

#[tokio::test]
async fn open_project_inner_broadcasts_only_when_archived_list_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-open");
    std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
    let path = project.to_string_lossy().to_string();
    let settings = AppSettings {
        archived_project_paths: vec![path.clone()],
        ..AppSettings::default()
    };
    crate::config::settings::save_settings_with_project_paths_to_path(&settings, &settings_path)
        .expect("seed canonical archived authority");
    let (app, state, mut rx, _, _) = archive_command_app(settings);
    let app_handle = app.handle().clone();

    open_project_inner_with_event_and_settings_path(
        &app_handle,
        &state,
        &path,
        Some(&settings_path),
    )
    .await
    .expect("open archived project");

    let event = recv_ws_event(&mut rx);
    assert_eq!(event["event"], json!("project_archive_changed"));
    assert_eq!(event["payload"]["path"], json!(path));
    assert_eq!(event["payload"]["archived"], json!(false));
    assert_eq!(event["payload"]["reason"], json!("open"));

    open_project_inner_with_event_and_settings_path(
        &app_handle,
        &state,
        &path,
        Some(&settings_path),
    )
    .await
    .expect("open already active project");

    assert!(
        rx.try_recv().is_err(),
        "opening an already-active project must not broadcast"
    );
}

#[tokio::test]
async fn new_project_inner_broadcasts_only_when_archived_list_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-new");
    let path = project.to_string_lossy().to_string();
    let settings = AppSettings {
        archived_project_paths: vec![path.clone()],
        ..AppSettings::default()
    };
    crate::config::settings::save_settings_with_project_paths_to_path(&settings, &settings_path)
        .expect("seed canonical archived authority");
    let (app, state, mut rx, _, _) = archive_command_app(settings);
    let app_handle = app.handle().clone();

    new_project_inner_with_event_and_settings_path(
        &app_handle,
        &state,
        &path,
        Some(&settings_path),
    )
    .await
    .expect("new archived project");

    let event = recv_ws_event(&mut rx);
    assert_eq!(event["event"], json!("project_archive_changed"));
    assert_eq!(event["payload"]["path"], json!(path));
    assert_eq!(event["payload"]["archived"], json!(false));
    assert_eq!(event["payload"]["reason"], json!("open"));

    new_project_inner_with_event_and_settings_path(
        &app_handle,
        &state,
        &path,
        Some(&settings_path),
    )
    .await
    .expect("new already active project");

    assert!(
        rx.try_recv().is_err(),
        "creating an already-active project must not broadcast"
    );
}

#[tokio::test]
async fn archive_project_inner_absolutizes_relative_path_and_emits_archive_event() {
    let _cwd_lock = cwd_lock().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-archive-relative");
    std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
    // Store the RAW path so the relative "." archive lookup (which absolutises
    // against the CWD, and on a runner whose %TEMP% is an 8.3 short name is the
    // short form) still matches lexically. The archived RUNTIME field is the
    // decoder's SELECTED canonical form (#1077 §3.4), which the assertion
    // below expects; the event still carries the raw absolutised input.
    let path = project.to_string_lossy().to_string();
    let archived_expected = canonical_display(&project);
    let settings = AppSettings {
        project_paths: vec![path.clone()],
        project_path: Some(path.clone()),
        ..AppSettings::default()
    };
    let (app, state, mut rx, session_mgr, pty_mgr) = archive_command_app(settings);
    let app_handle = app.handle().clone();
    let prev = std::env::current_dir().expect("current dir");
    let _guard = CwdGuard(prev);
    std::env::set_current_dir(&project).expect("set cwd");

    archive_project_inner_with_settings_path(
        &app_handle,
        &state,
        &session_mgr,
        &pty_mgr,
        ".",
        Some(&settings_path),
    )
    .await
    .expect("archive relative project");

    let stored = state.read().await.archived_project_paths.clone();
    assert_eq!(stored, vec![archived_expected]);
    let event = recv_ws_event(&mut rx);
    assert_eq!(event["event"], json!("project_archive_changed"));
    assert_eq!(event["payload"]["path"], json!(path));
    assert_eq!(event["payload"]["archived"], json!(true));
    assert_eq!(event["payload"]["reason"], json!("archive"));
}

#[tokio::test]
async fn archive_project_inner_archives_vanished_registered_project() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-vanished");
    let path = project.to_string_lossy().to_string();
    let settings = AppSettings {
        project_paths: vec![path.clone()],
        project_path: Some(path.clone()),
        ..AppSettings::default()
    };
    let (app, state, mut rx, session_mgr, pty_mgr) = archive_command_app(settings);
    let app_handle = app.handle().clone();

    archive_project_inner_with_settings_path(
        &app_handle,
        &state,
        &session_mgr,
        &pty_mgr,
        &path,
        Some(&settings_path),
    )
    .await
    .expect("archive vanished project");

    let settings = state.read().await;
    assert!(settings.project_paths.is_empty());
    assert_eq!(settings.archived_project_paths, vec![path.clone()]);
    drop(settings);
    let event = recv_ws_event(&mut rx);
    assert_eq!(event["payload"]["path"], json!(path));
    assert_eq!(event["payload"]["archived"], json!(true));
    assert_eq!(event["payload"]["reason"], json!("archive"));
}

#[tokio::test]
async fn archive_project_inner_allows_active_record_without_pty() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-f1");
    let agent = project.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&agent).expect("create agent dir");
    let path = canonical_display(&project);
    let settings = AppSettings {
        project_paths: vec![path.clone()],
        project_path: Some(path.clone()),
        ..AppSettings::default()
    };
    let (app, state, mut rx, session_mgr, pty_mgr) = archive_command_app(settings);
    let app_handle = app.handle().clone();
    {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "shell".to_string(),
            Vec::new(),
            agent.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create active record without PTY");
    }

    archive_project_inner_with_settings_path(
        &app_handle,
        &state,
        &session_mgr,
        &pty_mgr,
        &path,
        Some(&settings_path),
    )
    .await
    .expect("active record without PTY must not block archive");

    assert_eq!(
        state.read().await.archived_project_paths,
        vec![path.clone()]
    );
    let event = recv_ws_event(&mut rx);
    assert_eq!(event["payload"]["path"], json!(path));
    assert_eq!(event["payload"]["archived"], json!(true));
    assert_eq!(event["payload"]["reason"], json!("archive"));
}

#[tokio::test]
async fn archive_project_inner_pending_spawn_mark_blocks_precheck() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-pending");
    let agent = project.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&agent).expect("create agent dir");
    let path = project.to_string_lossy().to_string();
    let settings = AppSettings {
        project_paths: vec![path.clone()],
        project_path: Some(path.clone()),
        ..AppSettings::default()
    };
    let (app, state, mut rx, session_mgr, pty_mgr) = archive_command_app(settings);
    let app_handle = app.handle().clone();
    let _mark = {
        let pty = pty_mgr.lock().unwrap_or_else(|e| e.into_inner());
        pty.mark_spawning(&agent.to_string_lossy(), "spawning")
    };

    let err = archive_project_inner_with_settings_path(
        &app_handle,
        &state,
        &session_mgr,
        &pty_mgr,
        &path,
        Some(&settings_path),
    )
    .await
    .expect_err("pending spawn should block archive before settings write");

    assert!(err.contains("spawning"), "{err}");
    let settings = state.read().await;
    assert_eq!(settings.project_paths, vec![path]);
    assert!(settings.archived_project_paths.is_empty());
    drop(settings);
    assert!(rx.try_recv().is_err(), "precheck failure must not emit");
}

#[tokio::test]
async fn archive_project_inner_rolls_back_with_unarchive_event_when_liveness_appears_after_write() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-rollback");
    let agent = project.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&agent).expect("create agent dir");
    let path = canonical_display(&project);
    let settings = AppSettings {
        project_paths: vec![path.clone()],
        project_path: Some(path.clone()),
        ..AppSettings::default()
    };
    let (app, state, mut rx, session_mgr, _) = archive_command_app(settings);
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        FlippingLiveBackend::default(),
    ))));
    let app_handle = app.handle().clone();
    let session_id = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "shell".to_string(),
            Vec::new(),
            agent.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session")
        .id
    };
    pty_mgr
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .record_route(
            session_id,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        );

    let err = archive_project_inner_with_settings_path(
        &app_handle,
        &state,
        &session_mgr,
        &pty_mgr,
        &path,
        Some(&settings_path),
    )
    .await
    .expect_err("late spawn should roll back archive");

    assert!(err.contains("open session"), "{err}");
    let settings = state.read().await;
    assert_eq!(settings.project_paths, vec![path.clone()]);
    assert!(settings.archived_project_paths.is_empty());
    drop(settings);
    let event = recv_ws_event(&mut rx);
    assert_eq!(event["event"], json!("project_archive_changed"));
    assert_eq!(event["payload"]["path"], json!(path));
    assert_eq!(event["payload"]["archived"], json!(false));
    assert_eq!(event["payload"]["reason"], json!("unarchive"));
}

#[tokio::test]
async fn archive_project_inner_rolls_back_vanished_project_without_validation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("proj-vanished-rollback");
    let agent = project.join(".ac").join("wg-1").join("__agent_dev");
    let path = project.to_string_lossy().to_string();
    let settings = AppSettings {
        project_paths: vec![path.clone()],
        project_path: Some(path.clone()),
        ..AppSettings::default()
    };
    let (app, state, mut rx, session_mgr, _) = archive_command_app(settings);
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        FlippingLiveBackend::default(),
    ))));
    let app_handle = app.handle().clone();
    let session_id = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "shell".to_string(),
            Vec::new(),
            agent.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session")
        .id
    };
    pty_mgr
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .record_route(
            session_id,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        );

    let err = archive_project_inner_with_settings_path(
        &app_handle,
        &state,
        &session_mgr,
        &pty_mgr,
        &path,
        Some(&settings_path),
    )
    .await
    .expect_err("late liveness should roll back vanished project");

    assert!(err.contains("open session"), "{err}");
    // #1077: the rollback is disk-authoritative. The vanished project is
    // active again on disk, but excluded from the SELECTED active runtime
    // (session restoration must not restore an unvalidated directory) and it
    // is not archived, so both in-memory lists are empty.
    let settings = state.read().await;
    assert!(settings.project_paths.is_empty());
    assert!(settings.archived_project_paths.is_empty());
    drop(settings);
    let disk: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(settings_path.with_file_name("project-paths.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(disk["projectPaths"], json!([path]));
    assert!(disk["archivedProjectPaths"].as_array().unwrap().is_empty());
    let event = recv_ws_event(&mut rx);
    assert_eq!(event["payload"]["path"], json!(path));
    assert_eq!(event["payload"]["archived"], json!(false));
    assert_eq!(event["payload"]["reason"], json!("unarchive"));
}

#[tokio::test]
async fn poll_tasks_filters_archived_session_workgroups() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("proj-task-filter");
    let wg_root = project.join(".ac").join("wg-1");
    let agent = wg_root.join("__agent_dev");
    std::fs::create_dir_all(&agent).expect("create agent dir");
    std::fs::write(wg_root.join("TASK.md"), "Task: should stay hidden").expect("write task");
    let path = project.to_string_lossy().to_string();
    let settings = AppSettings {
        archived_project_paths: vec![path],
        ..AppSettings::default()
    };
    let (app, _, _, session_mgr, _) = archive_command_app(settings);
    let app_handle = app.handle().clone();
    {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "shell".to_string(),
            Vec::new(),
            agent.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create archived session");
    }
    let watcher = DiscoveryBranchWatcher::new(app_handle, Arc::clone(&session_mgr));

    watcher.poll_tasks(&[]).await.expect("poll tasks");

    assert!(
        watcher.task_cache.lock().unwrap().is_empty(),
        "archived session workgroups must not be checked for TASK.md"
    );
}
