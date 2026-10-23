use super::*;

#[tokio::test]
async fn coding_agent_profile_commands_route_past_unknown_command() {
    // #859 regression: before web-router parity these five commands hit the
    // `Unknown command` fallback. Empty args make each inner fail arg
    // validation instead, which proves the route now exists.
    let (state, _rx) = ws_state_for(AppSettings::default());
    for cmd in [
        "resolve_coding_agent_profile",
        "preview_coding_agent_profile_selection",
        "apply_coding_agent_profile_selection",
        "set_agent_default_profile",
        "set_instance_profile_override",
        "preview_selection_lock_removal",
        "apply_selection_lock_removal",
        "get_replica_selection_default",
        "set_replica_selection_default",
    ] {
        let response = dispatch(&state, 1, cmd, &json!({})).await;
        let error = response
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert!(
            !error.contains("Unknown command"),
            "{cmd} should be routed, got error: {error:?}"
        );
    }
}

#[tokio::test]
async fn move_coding_agent_route_rejects_overlay_owned_agents_without_broadcast() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings = overlay_owned_agents_settings(temp.path());
    let overlay_order: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
    let (state, mut rx) = ws_state_for(settings);

    let response = dispatch(
        &state,
        21,
        "move_coding_agent",
        &json!({ "id": "ov-a", "neighborId": "ov-b", "direction": "up" }),
    )
    .await;

    assert_eq!(
        response["error"],
        json!(crate::commands::config::AGENT_ORDER_OVERLAY_PINNED)
    );
    let live = state.settings.read().await;
    let live_order: Vec<String> = live.agents.iter().map(|a| a.id.clone()).collect();
    assert_eq!(live_order, overlay_order);
    assert!(rx.try_recv().is_err(), "a rejected move must not broadcast");
}

#[tokio::test]
async fn move_coding_agent_native_and_browser_events_are_byte_equivalent() {
    use tauri::Listener;

    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("settings.json");
    let original = AppSettings {
        agents: vec![test_agent("a"), test_agent("b"), test_agent("c")],
        ..AppSettings::default()
    };
    let (state, mut rx) = ws_state_for(original.clone());

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build native event app");
    let (native_tx, native_rx) = std::sync::mpsc::channel();
    app.listen_any("coding_agent_settings_updated", move |event| {
        let _ = native_tx.send(event.payload().to_string());
    });

    let native_ids = crate::commands::config::move_coding_agent_command_with_saver(
        app.handle(),
        &state.settings,
        "b".to_string(),
        "a".to_string(),
        crate::config::coding_agent_mutations::AgentMoveDirection::Up,
        |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        },
    )
    .await
    .expect("native move succeeds");
    assert_eq!(native_ids, ["b", "a", "c"]);

    // Same starting state for the browser route.
    *state.settings.write().await = original;
    let browser_ids = move_coding_agent_route_with_saver(
        &state,
        "b",
        "a",
        crate::config::coding_agent_mutations::AgentMoveDirection::Up,
        |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        },
    )
    .await
    .expect("browser move succeeds");
    assert_eq!(browser_ids, json!(["b", "a", "c"]));

    let native_payload: Value = serde_json::from_str(
        &native_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("native event"),
    )
    .expect("parse native event");
    let browser_event = match rx.try_recv().expect("browser event") {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse browser event"),
        other => panic!("expected text event, got {other:?}"),
    };
    assert_eq!(
        browser_event["event"],
        json!("coding_agent_settings_updated")
    );
    assert_eq!(
        native_payload,
        crate::commands::config::move_coding_agent_event_payload("b")
    );
    assert_eq!(browser_event["payload"], native_payload);
}

#[tokio::test]
async fn move_coding_agent_tauri_registration_presence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings = overlay_owned_agents_settings(temp.path());
    let settings_state: crate::config::settings::SettingsState =
        crate::config::settings::new_settings_state(settings);
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![
            crate::commands::config::move_coding_agent
        ])
        .manage(settings_state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build registration app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build webview");

    let error = match tauri::test::get_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: "move_coding_agent".to_string(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(json!({
                "id": "ov-a",
                "neighborId": "ov-b",
                "direction": "up"
            })),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    ) {
        Ok(_) => panic!("the overlay rejection must come from the registered command"),
        Err(error) => serde_json::to_value(error).unwrap_or(Value::Null),
    };

    assert_eq!(
        error,
        json!(crate::commands::config::AGENT_ORDER_OVERLAY_PINNED),
        "unexpected registration error payload"
    );
}

#[tokio::test]
async fn reorder_coding_agent_route_rejects_overlay_owned_agents_without_broadcast() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings = overlay_owned_agents_settings(temp.path());
    let overlay_order: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
    let (state, mut rx) = ws_state_for(settings);

    let response = dispatch(
        &state,
        22,
        "reorder_coding_agent",
        &json!({ "id": "ov-a", "expectedIds": overlay_order, "targetIndex": 0 }),
    )
    .await;

    assert_eq!(
        response["error"],
        json!(crate::commands::config::AGENT_ORDER_OVERLAY_PINNED)
    );
    let live = state.settings.read().await;
    let live_order: Vec<String> = live.agents.iter().map(|a| a.id.clone()).collect();
    assert_eq!(live_order, overlay_order);
    assert!(
        rx.try_recv().is_err(),
        "a rejected reorder must not broadcast"
    );
}

#[tokio::test]
async fn reorder_coding_agent_native_and_browser_events_are_byte_equivalent() {
    use tauri::Listener;

    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("settings.json");
    let save = |candidate: &AppSettings| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    };
    let original = AppSettings {
        agents: vec![test_agent("a"), test_agent("b"), test_agent("c")],
        ..AppSettings::default()
    };
    let expected: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
    let (state, mut rx) = ws_state_for(original.clone());

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build native event app");
    let (native_tx, native_rx) = std::sync::mpsc::channel();
    app.listen_any("coding_agent_settings_updated", move |event| {
        let _ = native_tx.send(event.payload().to_string());
    });

    let native_ids = crate::commands::config::reorder_coding_agent_command_with_saver(
        app.handle(),
        &state.settings,
        "c".to_string(),
        expected.clone(),
        0,
        save,
    )
    .await
    .expect("native reorder succeeds");
    assert_eq!(native_ids, ["c", "a", "b"]);

    *state.settings.write().await = original;
    let browser_ids = reorder_coding_agent_route_with_saver(&state, "c", &expected, 0, save)
        .await
        .expect("browser reorder succeeds");
    assert_eq!(browser_ids, json!(["c", "a", "b"]));

    let native_payload: Value = serde_json::from_str(
        &native_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("native event"),
    )
    .expect("parse native event");
    let browser_event = match rx.try_recv().expect("browser event") {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse browser event"),
        other => panic!("expected text event, got {other:?}"),
    };
    assert_eq!(
        browser_event["event"],
        json!("coding_agent_settings_updated")
    );
    assert_eq!(
        native_payload,
        crate::commands::config::move_coding_agent_event_payload("c")
    );
    assert_eq!(browser_event["payload"], native_payload);
}

/// Proves the command's signature, camelCase argument mapping and overlay
/// rejection through the real invoke path. It builds its own handler, so it
/// does NOT prove the `lib.rs` registration; the `../lib.rs` source-text
/// test in `commands/config.rs` does.
#[tokio::test]
async fn reorder_coding_agent_tauri_registration_presence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings = overlay_owned_agents_settings(temp.path());
    let overlay_order: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
    let settings_state: crate::config::settings::SettingsState =
        crate::config::settings::new_settings_state(settings);
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![
            crate::commands::config::reorder_coding_agent
        ])
        .manage(settings_state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build registration app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build webview");

    let error = match tauri::test::get_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: "reorder_coding_agent".to_string(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(json!({
                "id": "ov-a",
                "expectedIds": overlay_order,
                "targetIndex": 0
            })),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    ) {
        Ok(_) => panic!("the overlay rejection must come from the invoked command"),
        Err(error) => serde_json::to_value(error).unwrap_or(Value::Null),
    };

    assert_eq!(
        error,
        json!(crate::commands::config::AGENT_ORDER_OVERLAY_PINNED),
        "unexpected invoke error payload"
    );
}

#[tokio::test]
async fn resolve_coding_agent_profile_web_dispatch_returns_resolution() {
    let settings = AppSettings {
        agents: vec![test_agent("codex")],
        ..AppSettings::default()
    };
    let (state, _rx) = ws_state_for(settings);

    let response = dispatch(
        &state,
        2,
        "resolve_coding_agent_profile",
        &json!({ "agentId": "codex", "requestedProfile": "A" }),
    )
    .await;

    assert_eq!(response["id"], json!(2));
    assert!(
        response.get("error").is_none(),
        "resolve should succeed, got {response:?}"
    );
    assert!(
        response["result"]["effectiveProfile"].is_string(),
        "expected a resolved profile, got {response:?}"
    );
}

#[tokio::test]
async fn apply_coding_agent_profile_selection_web_dispatch_broadcasts() {
    // Real WG replica layout so enumeration + config write succeed under the
    // replica scope (no confirmation fingerprint required).
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_codex");
    let replica = ac_root.join("wg-1-team").join("__agent_codex");
    std::fs::create_dir_all(&matrix).expect("create matrix");
    std::fs::create_dir_all(&replica).expect("create replica");
    std::fs::write(matrix.join("Role.md"), "# Codex\n").expect("write Role.md");
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_string(&json!({ "identity": "../../_agent_codex" }))
            .expect("serialize replica config"),
    )
    .expect("write replica config");

    let settings = AppSettings {
        agents: vec![test_agent("codex")],
        project_paths: vec![project.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, mut rx) = ws_state_for(settings);

    let request = json!({
        "targetReplicaPath": replica.to_string_lossy(),
        "codingAgentId": "codex",
        "profile": "B",
        "scope": "replica",
        "restartSessions": false,
        "confirmedTargetFingerprint": null,
        "typedConfirmation": null,
    });

    let response = dispatch(
        &state,
        3,
        "apply_coding_agent_profile_selection",
        &json!({ "request": request }),
    )
    .await;

    assert_eq!(response["id"], json!(3));
    assert!(
        response.get("error").is_none(),
        "apply should succeed, got {response:?}"
    );
    assert_eq!(response["result"]["updatedCount"], json!(1));
    assert_eq!(response["result"]["scope"], json!("replica"));

    // The web dispatcher must broadcast the selection-updated event so
    // browser clients refresh, mirroring the save_settings_draft pattern.
    let event = match rx.try_recv().expect("broadcast event") {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
        other => panic!("expected text event, got {other:?}"),
    };
    assert_eq!(
        event["event"],
        json!("coding_agent_profile_selection_updated")
    );
    assert_eq!(event["payload"]["scope"], json!("replica"));
    assert_eq!(event["payload"]["codingAgentId"], json!("codex"));
    assert_eq!(event["payload"]["profile"], json!("B"));
    assert_eq!(event["payload"]["updatedCount"], json!(1));
}

// ── #1941 scoped selection-lock transport parity ────────────────────
