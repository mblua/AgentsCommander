use super::*;

#[test]
fn client_broadcast_allowlist_requires_exact_typed_ui_payloads() {
    for (event, payload) in [
        ("theme_changed", json!({ "light": true })),
        ("resource_monitor_attach", json!({})),
        ("open_settings", json!({})),
        ("open_settings", json!({ "section": "integrations" })),
    ] {
        assert!(
            validate_client_broadcast(event, &payload).is_ok(),
            "valid payload rejected for {event}: {payload}"
        );
    }

    for (event, payload) in [
        ("theme_changed", json!(null)),
        ("theme_changed", json!([])),
        ("theme_changed", json!({})),
        ("theme_changed", json!({ "light": "true" })),
        ("theme_changed", json!({ "light": true, "extra": 1 })),
        ("resource_monitor_attach", json!(null)),
        ("resource_monitor_attach", json!([])),
        ("resource_monitor_attach", json!({ "extra": 1 })),
        ("open_settings", json!(null)),
        ("open_settings", json!([])),
        ("open_settings", json!({ "section": 7 })),
        ("open_settings", json!({ "section": "voice", "extra": 1 })),
    ] {
        assert!(
            validate_client_broadcast(event, &payload).is_err(),
            "invalid payload accepted for {event}: {payload}"
        );
    }
}

#[tokio::test]
async fn get_settings_route_returns_snapshot_and_omits_root_token() {
    // #1077: the browser get_settings route must reuse the shared snapshot
    // helper, so it carries the resolution report and never leaks rootToken.
    let settings = AppSettings {
        root_token: Some("WS-ROUTE-ROOT-TOKEN".to_string()),
        ..AppSettings::default()
    };
    let (state, _rx) = ws_state_for(settings);
    let value = dispatch_inner(&state, "get_settings", &json!({}))
        .await
        .expect("get_settings");
    let text = serde_json::to_string(&value).unwrap();
    assert!(
        !text.contains("rootToken"),
        "rootToken leaked over WS: {text}"
    );
    assert!(!text.contains("WS-ROUTE-ROOT-TOKEN"), "token value leaked");
    let resolution = value
        .get("projectPathResolution")
        .expect("snapshot must carry projectPathResolution");
    assert_eq!(resolution["activeRegistrationCount"], 0);
    assert_eq!(resolution["archivedRegistrationCount"], 0);
    assert!(resolution["issues"].as_array().unwrap().is_empty());
    assert!(resolution["reconciliationError"].is_null());
}

/// #2133 (P4) - the web arm is a real implementation, not the null no-op group. It goes
/// through the process config dir, so it asserts only equality with the native command.
#[tokio::test]
async fn the_web_arm_serves_the_same_agent_help_payload() {
    let (state, _rx) = ws_state_for(AppSettings::default());
    let response = dispatch(&state, 7, "get_agent_help", &json!({})).await;
    let expected = serde_json::to_value(crate::commands::config::get_agent_help()).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"], json!(null));
    assert_eq!(response["result"], expected);
}

#[tokio::test]
async fn client_cannot_forge_backend_lifecycle_broadcasts() {
    let (state, mut receiver) = ws_state_for(AppSettings::default());
    for event in [
        "session_switched",
        "session_created",
        "session_destroyed",
        "session_communication_changed",
        "session_view_requested",
    ] {
        let response = dispatch(
            &state,
            17,
            "broadcast_event",
            &json!({
                "event": event,
                "payload": {
                    "epoch": Uuid::new_v4(),
                    "revision": 999999,
                    "source": "userSwitch",
                    "userInitiated": true,
                },
            }),
        )
        .await;
        assert!(response.get("error").is_some(), "forgery accepted: {event}");
    }
    assert!(
        receiver.try_recv().is_err(),
        "rejected lifecycle forgery reached WebSocket listeners"
    );
}

/// #973 - `pty_resize` is network-facing, and the old `as u16` cast SILENTLY TRUNCATED.
/// `cols: 65536` reached the PTY as **0** and `cols: 65537` as **1**: a zero-column ConPTY
/// (the very thing #973 exists to keep out) conjured from a number that looked fine on the
/// wire. Rejected at the boundary now, exactly as serde rejects it for the Tauri command.
#[test]
fn a_terminal_dimension_off_the_wire_is_never_truncated_into_a_degenerate_one() {
    let dim = |v: serde_json::Value| terminal_dimension(&json!({ "cols": v }), "cols", 120);

    assert_eq!(dim(json!(74)).expect("a plain size"), 74);
    assert_eq!(dim(json!(65535)).expect("the largest there is"), 65535);

    // 65536 -> 0 and 65537 -> 1 under `as u16`. This is the whole point.
    assert!(
        dim(json!(65536)).is_err(),
        "65536 must not become a 0-column terminal"
    );
    assert!(
        dim(json!(65537)).is_err(),
        "65537 must not become a 1-column terminal"
    );
    assert!(
        dim(json!(-1)).is_err(),
        "a negative is not a terminal dimension"
    );
    assert!(
        dim(json!("74")).is_err(),
        "a string is not a terminal dimension"
    );
    assert!(
        dim(json!(74.5)).is_err(),
        "a float is not a terminal dimension"
    );

    // absent stays absent: the caller's default, unchanged
    assert_eq!(
        terminal_dimension(&json!({}), "cols", 120).expect("absent"),
        120
    );
    assert_eq!(
        terminal_dimension(&json!({ "cols": null }), "cols", 120).expect("null"),
        120
    );

    // ...and a zero passes through to the ONE guard that refuses it, in `resize_instance`,
    // shared by both transports. Not because a zero is legitimate - xterm cannot produce one -
    // but because refusing it twice, in two places that can drift apart, is worse than
    // refusing it once where both transports meet.
    assert_eq!(
        dim(json!(0)).expect("a zero is refused downstream, not here"),
        0
    );
}

#[test]
fn browser_project_commands_route_before_unknown_fallback() {
    let expected = [
        ("open_project", BrowserProjectCommand::OpenProject),
        ("discover_project", BrowserProjectCommand::DiscoverProject),
        (
            "get_project_file_difference",
            BrowserProjectCommand::GetProjectFileDifference,
        ),
        (
            "check_project_path",
            BrowserProjectCommand::CheckProjectPath,
        ),
        ("remove_project", BrowserProjectCommand::RemoveProject),
        ("archive_project", BrowserProjectCommand::ArchiveProject),
        ("unarchive_project", BrowserProjectCommand::UnarchiveProject),
        (
            "list_archived_projects",
            BrowserProjectCommand::ListArchivedProjects,
        ),
        (
            "get_project_groups",
            BrowserProjectCommand::GetProjectGroups,
        ),
        (
            "update_project_groups",
            BrowserProjectCommand::UpdateProjectGroups,
        ),
    ];

    for (command, route) in expected {
        assert_eq!(
            route_web_command(command),
            WebCommandRoute::BrowserProject(route),
            "{command} should be routed before Unknown command"
        );
    }
}

#[tokio::test]
async fn project_file_difference_web_route_reaches_shared_helper() {
    assert_eq!(
        route_web_command("get_project_file_difference"),
        WebCommandRoute::BrowserProject(BrowserProjectCommand::GetProjectFileDifference)
    );

    // A directory without `.ac` reaches the helper and comes back unknown, not an error.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().to_str().expect("utf8 path");
    assert_eq!(
        project_file_difference_value(&json!({ "path": path }))
            .await
            .expect("helper result"),
        json!({ "count": null })
    );
    assert!(project_file_difference_value(&json!({})).await.is_err());
}

#[test]
fn deferred_and_cli_project_commands_are_not_web_commands() {
    assert_eq!(
        route_web_command("create_ac_project"),
        WebCommandRoute::Other
    );
    assert_eq!(route_web_command("new_project"), WebCommandRoute::Other);
    assert_eq!(route_web_command("open-project"), WebCommandRoute::Other);
    assert_eq!(route_web_command("new-project"), WebCommandRoute::Other);
}

#[test]
fn broadcast_all_r_sends_to_managed_websocket_broadcaster() {
    let broadcaster = WsBroadcaster::new();
    let mut receiver = broadcaster.subscribe();
    let app = crate::test_support::test_builder()
        .manage(broadcaster)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let payload = json!({ "path": "C:/project", "archived": true });

    broadcast_all_r(app.handle(), "project_archive_changed", &payload);

    let event = match receiver.try_recv().expect("broadcast event") {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
        other => panic!("expected text event, got {other:?}"),
    };
    assert_eq!(event["event"], json!("project_archive_changed"));
    assert_eq!(event["payload"], payload);
}

#[tokio::test]
async fn update_project_groups_web_dispatch_broadcasts_saved_config() {
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let app_handle = app.handle().clone();
    let broadcaster = WsBroadcaster::new();
    let mut receiver = broadcaster.subscribe();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), app_handle.clone());
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
        Arc::new(Mutex::new(HashMap::new())),
        idle_detector,
        git_watcher,
        Some(broadcaster.clone()),
        None,
    )));
    let settings = crate::config::settings::new_settings_state(AppSettings::default());
    let state = WsState::new(
        session_mgr,
        pty_mgr,
        settings,
        broadcaster.clone(),
        app_handle,
    );

    let project = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(project.path().join(".ac")).expect("create .ac");
    let path = project.path().to_string_lossy().to_string();
    let config = WorkgroupGroupsConfig {
        groups: vec![WorkgroupGroup {
            id: "core".to_string(),
            name: "Core".to_string(),
            regex: "^wg-14$".to_string(),
            favorite: false,
        }],
        show_all: false,
        show_ungrouped: true,
        non_stop: None,
    };

    let response = dispatch(
        &state,
        7,
        "update_project_groups",
        &json!({ "path": path, "config": config.clone() }),
    )
    .await;

    assert_eq!(response["id"], json!(7));
    assert_eq!(
        response["result"],
        serde_json::to_value(&config).expect("serialize config")
    );
    assert_eq!(
        get_project_groups_inner(&path).expect("load saved groups"),
        config
    );

    let event = match receiver.try_recv().expect("broadcast event") {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
        other => panic!("expected text event, got {other:?}"),
    };
    assert_eq!(event["event"], json!(PROJECT_GROUPS_UPDATED_EVENT));
    assert_eq!(event["payload"]["projectPath"], json!(path));
    assert_eq!(
        event["payload"]["config"],
        serde_json::to_value(&config).expect("serialize config")
    );
}

#[tokio::test]
async fn task_snapshot_web_route_matches_shared_inner_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".ac/room-1");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("TASK.md"),
        "---
title: 'USER: café'
---
日本語 🦀
",
    )
    .unwrap();
    let (state, _) = ws_state_for(AppSettings {
        project_paths: vec![dir.path().to_string_lossy().into_owned()],
        ..Default::default()
    });
    let path = root.to_string_lossy().into_owned();
    let direct = crate::commands::ac_discovery::task_snapshot_inner(
        &state.session_mgr,
        &state.settings,
        None,
        Some(&path),
    )
    .await
    .unwrap();
    let response = dispatch(
        &state,
        2841,
        "task_get_snapshot_at",
        &json!({"workgroupRoot":path}),
    )
    .await;
    assert_eq!(response, json!({"id":2841,"result":direct}));
    for args in [
        json!({}),
        json!({"workgroupRoot":42}),
        json!({"workgroupRoot":dir.path().to_string_lossy()}),
    ] {
        assert!(dispatch(&state, 1, "task_get_snapshot_at", &args)
            .await
            .get("error")
            .is_some());
    }
    for id in ["invalid".to_owned(), Uuid::new_v4().to_string()] {
        let direct = crate::commands::ac_discovery::task_snapshot_inner(
            &state.session_mgr,
            &state.settings,
            Some(&id),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(
            dispatch(&state, 2, "task_get_snapshot", &json!({"sessionId":id})).await,
            json!({"id":2,"error":direct})
        );
    }
    std::fs::write(
        root.join("TASK-status.jsonl"),
        "[]
",
    )
    .unwrap();
    assert!(dispatch(
        &state,
        3,
        "task_get_snapshot_at",
        &json!({"workgroupRoot":path})
    )
    .await
    .get("error")
    .is_some());
}

#[tokio::test]
async fn drain_session_warnings_web_dispatch_drains_buffer() {
    let (state, _rx, app_handle) = ws_state_for_with_handle(AppSettings::default());
    let session_id = Uuid::new_v4();
    {
        let warnings = app_handle.state::<crate::session::warnings::SessionWarningState>();
        let mut buffer = warnings.lock().expect("warning buffer lock");
        buffer.push(crate::session::warnings::SessionWarning::new(
            session_id,
            "CLAUDE_CONFIG_DIR",
            "outside-mount",
            "warning text",
        ));
    }

    let response = dispatch(
        &state,
        9,
        "drain_session_warnings",
        &json!({ "sessionId": session_id.to_string() }),
    )
    .await;

    assert_eq!(response["id"], json!(9));
    assert_eq!(
        response["result"][0]["sessionId"],
        json!(session_id.to_string())
    );
    assert_eq!(response["result"][0]["key"], json!("CLAUDE_CONFIG_DIR"));

    let response = dispatch(
        &state,
        10,
        "drain_session_warnings",
        &json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    assert_eq!(response["id"], json!(10));
    assert_eq!(response["result"], json!([]));
}
