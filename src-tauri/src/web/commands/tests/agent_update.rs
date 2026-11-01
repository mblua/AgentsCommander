use super::*;

#[tokio::test]
async fn issue_3083_web_record_boundary_envelope_and_restore_guard() {
    let (state, _rx, _gate) = ws_state_with_agent_update(AppSettings::default(), false, true);
    let valid = json!({"record":{"version":1,
        "windowId":"fedcba98-7654-4321-8abc-012345678901", "seq":1,
        "atMs":0,"surface":"sidebar","event":"click"}});
    assert_eq!(
        dispatch(&state, 3083, "append_rail_diagnostic", &valid).await,
        json!({"id":3083,"result":null})
    );
    for args in [
        json!({}),
        json!({"record":null}),
        json!({"record":"private/path"}),
        json!({"record":[]}),
        json!({"record":{"unknown":{"secret":true}}}),
    ] {
        assert_eq!(
            dispatch(&state, 3083, "append_rail_diagnostic", &args).await,
            json!({"id":3083,"error":"invalid rail diagnostic"})
        );
    }
    let (blocked, _rx, _gate) = ws_state_with_agent_update(AppSettings::default(), true, false);
    assert_eq!(
        dispatch(&blocked, 3083, "append_rail_diagnostic", &valid).await,
        json!({"id":3083,"error":"selectionCoordinatorBusy"})
    );
}

/// Like `ws_state_for`, plus the managed state the agent-update arms read:
/// the restore flag, the startup gate (finished iff `pass_finished`) and the
/// install cache. Returns the gate so a test can seed prompts.
fn ws_state_with_agent_update(
    settings: AppSettings,
    restore_in_progress: bool,
    pass_finished: bool,
) -> (
    WsState,
    tokio::sync::mpsc::Receiver<WsOutMsg>,
    Arc<crate::agent_update::AgentUpdateGate>,
) {
    let (state, receiver, gate, _app_handle) =
        ws_state_with_agent_update_and_handle(settings, restore_in_progress, pass_finished);
    (state, receiver, gate)
}

fn ws_state_with_agent_update_and_handle(
    settings: AppSettings,
    restore_in_progress: bool,
    pass_finished: bool,
) -> (
    WsState,
    tokio::sync::mpsc::Receiver<WsOutMsg>,
    Arc<crate::agent_update::AgentUpdateGate>,
    tauri::AppHandle,
) {
    let gate = Arc::new(crate::agent_update::AgentUpdateGate::new());
    if pass_finished {
        gate.mark_finished(vec![]);
    }
    let cache = Arc::new(crate::agent_version::AgentInstallCache::new());
    let broadcaster = WsBroadcaster::new();
    let receiver = broadcaster.subscribe();
    let app = crate::test_support::test_builder()
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(crate::RestoreInProgress(
            std::sync::atomic::AtomicBool::new(restore_in_progress),
        )))
        .manage(Arc::clone(&gate))
        .manage(cache)
        // The SAME broadcaster the `WsState` carries (clones share senders),
        // so `emit_all` reaches this test's receiver.
        .manage(broadcaster.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let app_handle = app.handle().clone();
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
    let state = WsState::new(
        session_mgr,
        pty_mgr,
        crate::config::settings::new_settings_state(settings),
        broadcaster,
        app_handle.clone(),
    );
    (state, receiver, gate, app_handle)
}

#[tokio::test]
async fn agent_update_routes_bypass_restore_guard() {
    // A project with no persisted catalog keeps the catalog route's result
    // deterministic; the point is that the route is REACHABLE while restore
    // is parked (not the restore guard's `selectionCoordinatorBusy`).
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, _rx, _gate) = ws_state_with_agent_update(settings, true, false);

    let busy = dispatch_inner(&state, "list_sessions", &json!({})).await;
    assert_eq!(busy, Err("selectionCoordinatorBusy".to_string()));

    let status = dispatch_inner(&state, "get_agent_update_status", &json!({}))
        .await
        .expect("status route");
    assert_eq!(status["inProgress"], false);
    assert_eq!(status["running"], json!([]));
    assert_eq!(status["results"], json!([]));
    assert_eq!(status["answered"], json!({}));
    assert_eq!(status["nodes"], json!([]));

    let catalog_error = dispatch_inner(&state, "get_coding_agent_catalog", &json!({}))
        .await
        .expect_err("no persisted catalog in the test project");
    assert!(catalog_error.contains("baseUnavailable"), "{catalog_error}");
    assert_ne!(catalog_error, "selectionCoordinatorBusy");
}

#[tokio::test]
async fn agent_update_answer_route_rejects_unprompted_command() {
    let (state, _rx, _gate) = ws_state_with_agent_update(AppSettings::default(), false, false);
    let error = dispatch_inner(
        &state,
        "agent_update_answer",
        &json!({ "command": "claude", "enabled": true }),
    )
    .await
    .expect_err("an unprompted command must be rejected");
    assert!(error.contains("was not prompted"), "unexpected: {error}");
}

#[tokio::test]
async fn agent_update_answer_route_rejects_missing_args() {
    let (state, _rx, _gate) = ws_state_with_agent_update(AppSettings::default(), false, false);
    assert!(dispatch_inner(&state, "agent_update_answer", &json!({}))
        .await
        .is_err());
}

#[tokio::test]
async fn agent_update_answer_route_returns_false_for_an_already_answered_prompt() {
    let (state, _rx, gate) = ws_state_with_agent_update(AppSettings::default(), false, false);
    let _rx_prompt = gate.register_prompt("claude", "Claude");
    // The claim is dropped: the closure emission is not under test here.
    let _claim = gate.claim_answer("claude", true);

    let applied = dispatch_inner(
        &state,
        "agent_update_answer",
        &json!({ "command": "claude", "enabled": false }),
    )
    .await
    .expect("answer route");
    assert_eq!(applied, json!(false));

    let status = dispatch_inner(&state, "get_agent_update_status", &json!({}))
        .await
        .expect("status route");
    assert_eq!(status["answered"], json!({ "claude": true }));
    assert_eq!(status["prompt"], Value::Null);
}

fn welcome_status_state_2736() -> (tempfile::TempDir, WsState, tauri::AppHandle) {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog_dir = dir.path().join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "agents": [
                {
                    "key": "claude",
                    "label": "Claude",
                    "description": "d",
                    "color": "#000000",
                    "command": "claude-2736-absent",
                    "envs": [],
                    "isolatedHome": false,
                    "removable": true,
                    "installCommands": { "default": "npm i -g claude" }
                },
                {
                    "key": "mine-2736",
                    "label": "Mine",
                    "description": "d",
                    "color": "#111111",
                    "command": "mine-2736-absent",
                    "envs": [],
                    "isolatedHome": false,
                    "removable": true
                }
            ]
        }))
        .expect("manifest json"),
    )
    .expect("write catalog");
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, _rx, _gate, app_handle) =
        ws_state_with_agent_update_and_handle(settings, false, false);
    (dir, state, app_handle)
}

#[tokio::test]
async fn get_coding_agent_welcome_status_route_matches_the_shared_inner() {
    let (_dir, state, _app_handle) = welcome_status_state_2736();
    let inner = crate::commands::config::coding_agent_welcome_status_inner(&state.settings)
        .await
        .expect("shared inner");
    assert_eq!(
        inner.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
        vec!["claude", "mine-2736"]
    );
    let routed = dispatch_inner(&state, "get_coding_agent_welcome_status", &json!({}))
        .await
        .expect("welcome status route");
    assert_eq!(routed, serde_json::to_value(&inner).expect("rows json"));
    assert_eq!(routed[0]["testedLevel"], json!("high"));
    assert_eq!(routed[0]["installCommand"], json!("npm i -g claude"));
    assert_eq!(routed[1]["testedLevel"], Value::Null);
}

#[tokio::test]
async fn install_coding_agent_route_matches_the_shared_inner_for_a_refusal() {
    let (_dir, state, app_handle) = welcome_status_state_2736();
    let inner = crate::commands::config::install_coding_agent_inner(
        &app_handle,
        &state.settings,
        "nope-2736".to_string(),
    )
    .await;
    assert_eq!(inner, Err("unknown coding agent 'nope-2736'".to_string()));
    let routed = dispatch_inner(
        &state,
        "install_coding_agent",
        &json!({ "key": "nope-2736" }),
    )
    .await;
    assert_eq!(routed, inner.map(|()| Value::Null));
}

#[tokio::test]
async fn install_coding_agent_route_rejects_missing_key() {
    let (_dir, state, _app_handle) = welcome_status_state_2736();
    let routed = dispatch_inner(&state, "install_coding_agent", &json!({})).await;
    assert_eq!(routed, Err("Missing required field: key".to_string()));
}

#[tokio::test]
async fn get_coding_agent_welcome_status_route_ignores_extra_args() {
    let (_dir, state, _app_handle) = welcome_status_state_2736();
    let plain = dispatch_inner(&state, "get_coding_agent_welcome_status", &json!({}))
        .await
        .expect("plain route");
    let extra = dispatch_inner(
        &state,
        "get_coding_agent_welcome_status",
        &json!({ "unexpected": true }),
    )
    .await
    .expect("extra args must not be rejected");
    assert_eq!(extra, plain);
    assert_eq!(extra.as_array().expect("rows").len(), 2);
}

#[tokio::test]
async fn get_coding_agent_catalog_route_serves_the_persisted_catalog_without_a_donor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog_dir = dir.path().join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "agents": [
                {
                    "key": "legacy-1967",
                    "label": "Legacy",
                    "description": "d",
                    "color": "#000000",
                    "command": "legacy-1967",
                    "envs": [],
                    "isolatedHome": false,
                    "removable": true
                },
                {
                    "key": "fresh-1967",
                    "label": "Fresh",
                    "description": "d",
                    "color": "#111111",
                    "command": "fresh-1967",
                    "envs": [],
                    "isolatedHome": false,
                    "removable": true,
                    "updateCommands": ["fresh update"]
                }
            ]
        }))
        .expect("manifest json"),
    )
    .expect("write catalog");
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, _rx, _gate) = ws_state_with_agent_update(settings, false, false);

    let inner = crate::commands::config::coding_agent_catalog_inner(&state.settings)
        .await
        .expect("shared inner");
    assert_eq!(
        inner.iter().map(|def| def.key.as_str()).collect::<Vec<_>>(),
        vec!["legacy-1967", "fresh-1967"]
    );

    // The array selection is IDENTICAL to the report's catalog: no in-memory
    // backfill, and the legacy entry keeps its empty sequence. Its warning is
    // carried structurally by the report route.
    let report = crate::commands::config::coding_agent_catalog_report_inner(&state.settings).await;
    assert_eq!(report.catalog, inner);
    assert!(report.unavailable.is_none());
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].code, "migrationPending");

    let routed = dispatch_inner(&state, "get_coding_agent_catalog", &json!({}))
        .await
        .expect("catalog route");
    assert_eq!(routed, serde_json::to_value(&inner).expect("catalog json"));
    let rows = routed.as_array().expect("catalog array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["key"], "legacy-1967");
    assert_eq!(rows[0]["updateCommands"], json!([]));
    assert_eq!(rows[1]["updateCommands"], json!(["fresh update"]));

    let reseedable = dispatch_inner(&state, "list_reseedable_agent_commands", &json!({}))
        .await
        .expect("reseedable route");
    assert_eq!(
        reseedable,
        serde_json::to_value(crate::commands::config::list_reseedable_agent_commands())
            .expect("reseedable json")
    );
}

#[tokio::test]
async fn get_coding_agent_catalog_route_reports_unavailability_with_code_path_and_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, _rx, _gate) = ws_state_with_agent_update(settings, false, false);

    let report = crate::commands::config::coding_agent_catalog_report_inner(&state.settings).await;
    let unavailable = report.unavailable.as_ref().expect("unavailable");
    assert!(report.catalog.is_empty());

    let routed = dispatch_inner(&state, "get_coding_agent_catalog", &json!({}))
        .await
        .expect_err("unavailable catalog route");
    assert!(routed.contains(&unavailable.code), "{routed}");
    assert!(routed.contains(&unavailable.path), "{routed}");
    assert!(routed.contains(&unavailable.reason), "{routed}");
}

// ---------------------------------------------------------------------
// #1963 P1 - persisted-catalog report transport parity
// ---------------------------------------------------------------------

#[tokio::test]
async fn get_coding_agent_catalog_report_route_matches_shared_inner_for_warnings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog_dir = dir.path().join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "agents": [{
                "key": "legacy-1963",
                "label": "Legacy",
                "description": "d",
                "color": "#000000",
                "command": "legacy-1963",
                "envs": [],
                "isolatedHome": false,
                "removable": true
            }]
        }))
        .expect("manifest json"),
    )
    .expect("write catalog");
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, _rx, _gate) = ws_state_with_agent_update(settings, false, false);

    let inner = crate::commands::config::coding_agent_catalog_report_inner(&state.settings).await;
    assert_eq!(inner.warnings.len(), 1);
    assert_eq!(inner.warnings[0].code, "migrationPending");
    assert_eq!(inner.catalog.len(), 1);
    let inner_json = serde_json::to_value(&inner).expect("report json");

    let routed = dispatch_inner(&state, "get_coding_agent_catalog_report", &json!({}))
        .await
        .expect("catalog report route");
    // Complete serialized report parity between the shared inner and WS.
    assert_eq!(inner_json, routed);
    assert_eq!(routed["unavailable"], Value::Null);
    assert_eq!(
        routed["catalog"][0]["updateCommands"],
        json!([]),
        "no embedded donor on the persisted-only path"
    );
}

#[tokio::test]
async fn get_coding_agent_catalog_report_route_matches_shared_inner_for_unavailable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, _rx, _gate) = ws_state_with_agent_update(settings, false, false);

    let inner = crate::commands::config::coding_agent_catalog_report_inner(&state.settings).await;
    assert!(inner.catalog.is_empty());
    assert!(inner.unavailable.is_some());
    let inner_json = serde_json::to_value(&inner).expect("report json");

    let routed = dispatch_inner(&state, "get_coding_agent_catalog_report", &json!({}))
        .await
        .expect("catalog report route");
    assert_eq!(inner_json, routed);
    assert_eq!(routed["unavailable"]["code"], "baseUnavailable");
    assert_eq!(routed["catalog"], json!([]));
}

#[tokio::test]
async fn get_agent_update_overview_route_is_instant_and_probes_in_background() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog_dir = dir.path().join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "agents": [
                {
                    "key": "bob",
                    "label": "Bob",
                    "description": "",
                    "color": "#000000",
                    "command": "bob-1551-missing",
                    "updateCommands": ["bob up"]
                },
                {
                    "key": "nob",
                    "label": "Nob",
                    "description": "",
                    "color": "#000000",
                    "command": "nob-1551-missing",
                    "updateCommands": []
                }
            ]
        }))
        .expect("manifest json"),
    )
    .expect("write catalog");
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, mut rx, _gate) = ws_state_with_agent_update(settings, false, true);

    let rows = dispatch_inner(&state, "get_agent_update_overview", &json!({}))
        .await
        .expect("overview route");
    let rows = rows.as_array().expect("rows").clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["command"], "bob-1551-missing");
    assert_eq!(rows[0]["install"]["status"], "checking");
    assert_eq!(rows[0]["install"]["seq"], 0);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let rows = dispatch_inner(&state, "get_agent_update_overview", &json!({}))
            .await
            .expect("overview route");
        let rows = rows.as_array().expect("rows").clone();
        if rows[0]["install"]["status"] == "missing" {
            assert_eq!(rows[0]["install"]["seq"], 1);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the background probe never committed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let install_frame = loop {
        if let Ok(WsOutMsg::Text(text)) = rx.try_recv() {
            let frame: Value = serde_json::from_str(&text).expect("json frame");
            if frame["event"] == "agent_install_state_changed" {
                break frame;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no agent_install_state_changed frame arrived"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(install_frame["payload"]["command"], "bob-1551-missing");
}

#[tokio::test]
async fn get_agent_update_overview_route_reports_unavailable_catalog_as_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = AppSettings {
        project_paths: vec![dir.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let (state, _rx, _gate) = ws_state_with_agent_update(settings, false, true);

    let error = dispatch_inner(&state, "get_agent_update_overview", &json!({}))
        .await
        .expect_err("unavailable catalog must not hide behind Ok([])");
    assert!(error.contains("baseUnavailable"), "{error}");
}

#[tokio::test]
async fn agent_update_cancel_transport_parity() {
    async fn next_text(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) -> String {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event frame");
        match frame {
            WsOutMsg::Text(text) => text,
            WsOutMsg::Binary(_) => panic!("unexpected binary event"),
        }
    }

    fn decode(text: &str) -> Value {
        serde_json::from_str(text).expect("event json")
    }

    fn retain(gate: &crate::agent_update::AgentUpdateGate, commands: &[(&str, &str)]) {
        gate.mark_started_with_nodes(
            commands
                .iter()
                .map(|(command, label)| crate::agent_update::AgentUpdateNode {
                    command: (*command).to_string(),
                    label: (*label).to_string(),
                    update_commands: vec!["exit 0".to_string()],
                    install_before: None,
                })
                .collect(),
        );
    }

    let (_desktop, mut desktop_events, desktop_gate, desktop_handle) =
        ws_state_with_agent_update_and_handle(AppSettings::default(), false, false);
    let (web, mut web_events, web_gate) =
        ws_state_with_agent_update(AppSettings::default(), false, false);
    retain(&desktop_gate, &[("claude", "Claude")]);
    retain(&web_gate, &[("claude", "Claude")]);

    let desktop_response =
        crate::commands::config::agent_update_cancel(desktop_handle.clone(), "claude".to_string())
            .await
            .expect("desktop cancellation");
    let web_response = dispatch_inner(&web, "agent_update_cancel", &json!({ "command": "claude" }))
        .await
        .expect("web cancellation");
    let desktop_bytes = serde_json::to_vec(&desktop_response).expect("desktop bytes");
    let web_bytes = serde_json::to_vec(&web_response).expect("web bytes");
    assert_eq!(desktop_bytes, web_bytes);
    assert_eq!(
        std::str::from_utf8(&desktop_bytes).expect("desktop response utf8"),
        r#"{"command":"claude","disposition":"requested"}"#
    );
    assert_eq!(
        web_response,
        json!({ "command": "claude", "disposition": "requested" })
    );
    let desktop_event_text = next_text(&mut desktop_events).await;
    let web_event_text = next_text(&mut web_events).await;
    assert_eq!(desktop_event_text, web_event_text);
    let desktop_event = decode(&desktop_event_text);
    assert_eq!(desktop_event["event"], "agent_update_cancellation_changed");
    assert_eq!(
        desktop_event["payload"],
        json!({
            "cancelRequested": [{ "command": "claude", "label": "Claude" }],
            "cancelAllRequested": false
        })
    );

    let (_desktop, mut desktop_events, desktop_gate, desktop_handle) =
        ws_state_with_agent_update_and_handle(AppSettings::default(), false, false);
    let (web, mut web_events, web_gate, web_handle) =
        ws_state_with_agent_update_and_handle(AppSettings::default(), false, false);
    retain(&desktop_gate, &[("claude", "Claude"), ("codex", "Codex")]);
    retain(&web_gate, &[("claude", "Claude"), ("codex", "Codex")]);
    let desktop_response =
        crate::commands::config::agent_updates_cancel_all(desktop_handle.clone())
            .await
            .expect("desktop batch cancellation");
    let web_response = dispatch_inner(&web, "agent_updates_cancel_all", &json!({}))
        .await
        .expect("web batch cancellation");
    let desktop_bytes =
        serde_json::to_vec(&desktop_response).expect("desktop batch response bytes");
    let web_bytes = serde_json::to_vec(&web_response).expect("web batch response bytes");
    assert_eq!(desktop_bytes, web_bytes);
    assert_eq!(
        std::str::from_utf8(&desktop_bytes).expect("desktop batch response utf8"),
        concat!(
            r#"{"alreadyRequested":[],"alreadyTerminal":[],"requested":[{"command":"claude","#,
            r#""label":"Claude"},{"command":"codex","label":"Codex"}]}"#,
        )
    );
    assert_eq!(web_response["requested"][0]["command"], "claude");
    assert_eq!(web_response["requested"][1]["command"], "codex");
    let desktop_event_text = next_text(&mut desktop_events).await;
    let web_event_text = next_text(&mut web_events).await;
    assert_eq!(desktop_event_text, web_event_text);
    let desktop_event = decode(&desktop_event_text);
    assert_eq!(desktop_event["payload"]["cancelAllRequested"], true);
    assert_eq!(
        desktop_event["payload"]["cancelRequested"],
        json!([
            { "command": "claude", "label": "Claude" },
            { "command": "codex", "label": "Codex" }
        ])
    );

    crate::agent_update::emit_agent_update_transport_contract_for_test(&desktop_handle);
    crate::agent_update::emit_agent_update_transport_contract_for_test(&web_handle);
    let desktop_verifying = next_text(&mut desktop_events).await;
    let web_verifying = next_text(&mut web_events).await;
    assert_eq!(desktop_verifying, web_verifying);
    let verifying = decode(&desktop_verifying);
    assert_eq!(verifying["event"], "agent_update_command_verifying");
    assert_eq!(
        verifying["payload"],
        json!({ "command": "claude", "label": "Claude" })
    );

    let desktop_terminal = next_text(&mut desktop_events).await;
    let web_terminal = next_text(&mut web_events).await;
    assert_eq!(desktop_terminal, web_terminal);
    let terminal = decode(&desktop_terminal);
    assert_eq!(terminal["event"], "agent_update_command_finished");
    assert_eq!(
        terminal["payload"],
        json!({
            "change": "unknown",
            "command": "claude",
            "installAfter": null,
            "installBefore": {
                "seq": 0,
                "status": "installed",
                "version": "1.2.3"
            },
            "label": "Claude",
            "ok": false,
            "outcome": "cancelled"
        })
    );
}

#[tokio::test]
async fn e2432_p02_browser_routes_share_physical_source_flow() {
    use crate::config::instance_artifacts::{AGENTS_INSTANCE_FILE_NAME, SETTINGS_FILE_NAME};
    use crate::config::settings::SettingsOwner;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    std::fs::write(&path, serde_json::to_vec(&AppSettings::default()).unwrap()).unwrap();
    std::fs::write(
        temp.path().join(AGENTS_INSTANCE_FILE_NAME),
        br#"{"agents":[{"id":"own","label":"Own","color":"red","baseCommand":"tool"}]}"#,
    )
    .unwrap();
    let (mut state, _rx) = ws_state_for(AppSettings::default());
    state.settings = SettingsOwner::open_active(&path, std::time::Duration::from_secs(1)).unwrap();
    let listed = dispatch_inner(&state, "list_agent_file_sources", &json!({}))
        .await
        .unwrap();
    let source = listed["sources"][0]["source"].clone();
    assert!(listed["sources"][0].get("tickets").is_none());
    let snapshot = dispatch_inner(&state, "read_agent_file_source", &json!({"source":source}))
        .await
        .unwrap();
    assert!(snapshot["tickets"]["own"]["default"]["ref"]["requestedProfile"].is_null());
    let reply = dispatch_inner(
        &state,
        "save_agent_file_source",
        &json!({
            "source": source,
            "expectedRevision": snapshot["revision"],
            "contextRevision": snapshot["contextRevision"],
            "edits": {
                "rows": [{"entryHint": "own", "action": "upsert", "patch": {"label": "Edited"}}],
                "profileSets": []
            }
        }),
    )
    .await
    .unwrap();
    assert_eq!(reply["outcome"], json!("committed"));
    assert_eq!(
        reply["snapshot"]["rows"][0]["raw"]["label"],
        json!("Edited")
    );
    let remembered = dispatch_inner(
        &state,
        "remember_agent_file_source",
        &json!({"menu":"settings","source":source}),
    )
    .await
    .unwrap();
    assert_eq!(remembered["settings"], source);
    assert!(remembered.get("picker").is_none());
}

#[tokio::test]
async fn e2432_p02_browser_catalog_events_silent_registered_events_live() {
    use crate::config::instance_artifacts::{AGENTS_INSTANCE_FILE_NAME, SETTINGS_FILE_NAME};
    use crate::config::settings::{SettingsOwner, SourceKind};
    use std::time::Duration;
    use tauri::Listener;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let initial = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    std::fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
    std::fs::write(
        temp.path().join(AGENTS_INSTANCE_FILE_NAME),
        concat!(
            r#"{"agents":[{"id":"registered","label":"Registered","color":"red","#,
            r#""baseCommand":"tool"}]}"#,
        ),
    )
    .unwrap();
    let catalog = crate::config::coding_agents_catalog::independent_catalog_path(
        &project.join(".ac"),
        SourceKind::CatalogPersonal,
    )
    .unwrap();
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    std::fs::write(
        &catalog,
        concat!(
            r#"{"schemaVersion":1,"agents":[{"key":"own","label":"Original","color":"blue","#,
            r#""baseCommand":"catalog-tool"}]}"#,
        ),
    )
    .unwrap();
    let (mut state, mut ws, app_handle) = ws_state_for_with_handle(initial);
    state.settings = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    assert!(app_handle.manage(state.broadcaster.clone()));
    let (tx, events) = std::sync::mpsc::channel();
    for event in [
        "coding_agent_settings_updated",
        "coding_agent_profiles_updated",
    ] {
        let tx = tx.clone();
        app_handle.listen_any(event, move |_| {
            tx.send(event).unwrap();
        });
    }
    let list = dispatch_inner(&state, "list_agent_file_sources", &json!({}))
        .await
        .unwrap();
    let sources = list["sources"].as_array().unwrap();
    let source = sources
        .iter()
        .find(|s| {
            s["source"]["scope"] == json!("project")
                && s["source"]["kind"] == json!("catalogPersonal")
        })
        .unwrap()["source"]
        .clone();
    let before = dispatch_inner(&state, "read_agent_file_source", &json!({"source":source}))
        .await
        .unwrap();
    let epoch = state.settings.identity().epoch;
    let ram = serde_json::to_value(state.settings.current_snapshot().await.unwrap()).unwrap();
    let reply = dispatch_inner(
        &state,
        "save_agent_file_source",
        &json!({
            "source": source,
            "expectedRevision": before["revision"],
            "contextRevision": before["contextRevision"],
            "edits": {
                "rows": [{"entryHint": "own", "action": "upsert", "patch": {"label": "Edited"}}],
                "profileSets": []
            }
        }),
    )
    .await
    .unwrap();
    assert_eq!(reply["outcome"], json!("committed"));
    assert!(reply["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == json!("effectiveSettingsUnchanged")));
    assert_eq!(reply["effectiveSettings"], ram);
    let actual: Value = serde_json::from_slice(&std::fs::read(&catalog).unwrap()).unwrap();
    assert_eq!(actual["agents"][0]["label"], json!("Edited"));
    let fresh = dispatch_inner(&state, "read_agent_file_source", &json!({"source":source}))
        .await
        .unwrap();
    assert_eq!(fresh["rows"][0]["raw"]["label"], json!("Edited"));
    assert_eq!(fresh["revision"], reply["snapshot"]["revision"]);
    assert_ne!(fresh["revision"], before["revision"]);
    assert!(events.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(matches!(
        ws.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let stale = dispatch_inner(
        &state,
        "save_agent_file_source",
        &json!({
            "source": source,
            "expectedRevision": before["revision"],
            "contextRevision": before["contextRevision"],
            "edits": {"rows": [], "profileSets": []}
        }),
    )
    .await
    .unwrap();
    assert_eq!(stale["outcome"], json!("rejected"));
    assert!(events.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(matches!(
        ws.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let bytes = std::fs::read(&catalog).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&catalog)
            .unwrap();
        let uncertain = dispatch_inner(
            &state,
            "save_agent_file_source",
            &json!({
                "source": source,
                "expectedRevision": fresh["revision"],
                "contextRevision": fresh["contextRevision"],
                "edits": {
                    "rows": [{
                        "entryHint": "own",
                        "action": "upsert",
                        "patch": {"label": "Uncertain"}
                    }],
                    "profileSets": []
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(uncertain["outcome"], json!("partialOrUnknown"));
        assert_eq!(uncertain["snapshot"]["revision"], fresh["revision"]);
        assert_eq!(std::fs::read(&catalog).unwrap(), bytes);
        assert!(events.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(matches!(
            ws.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        drop(held);
    }
    assert_eq!(state.settings.identity().epoch, epoch);
    assert_eq!(
        serde_json::to_value(state.settings.current_snapshot().await.unwrap()).unwrap(),
        ram
    );
    let registered = sources
        .iter()
        .find(|s| s["source"]["kind"] == json!("registeredInstance"))
        .unwrap()["source"]
        .clone();
    let snapshot = dispatch_inner(
        &state,
        "read_agent_file_source",
        &json!({"source":registered}),
    )
    .await
    .unwrap();
    let adopted = dispatch_inner(
        &state,
        "save_agent_file_source",
        &json!({
            "source": registered,
            "expectedRevision": snapshot["revision"],
            "contextRevision": snapshot["contextRevision"],
            "edits": {
                "rows": [{
                    "entryHint": "registered",
                    "action": "upsert",
                    "patch": {"label": "Adopted"}
                }],
                "profileSets": []
            }
        }),
    )
    .await
    .unwrap();
    assert_eq!(adopted["outcome"], json!("committed"));
    assert!(state.settings.identity().epoch > epoch);
    for expected in [
        "coding_agent_settings_updated",
        "coding_agent_profiles_updated",
    ] {
        assert_eq!(
            events.recv_timeout(Duration::from_secs(1)).unwrap(),
            expected
        );
        match ws.try_recv().expect("managed WS sink must be live") {
            crate::web::broadcast::WsOutMsg::Text(text) => assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap()["event"],
                json!(expected)
            ),
            other @ WsOutMsg::Binary(_) => panic!("unexpected WS message {other:?}"),
        }
    }
    assert!(events.try_recv().is_err());
    assert!(matches!(
        ws.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}
