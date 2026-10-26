use super::*;
use crate::config::coding_agent_profiles::SelectionWriteIntent;

fn selection_lock_web_fixture(
    locked: bool,
) -> (tempfile::TempDir, std::path::PathBuf, AppSettings) {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_codex");
    let replica = ac_root.join("room-1-team").join("__agent_codex");
    std::fs::create_dir_all(&matrix).expect("create matrix");
    std::fs::create_dir_all(&replica).expect("create replica");
    std::fs::write(matrix.join("Role.md"), "# Codex\n").expect("write Role.md");
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&json!({
            "identity": "../../_agent_codex",
            "tooling": {
                "profile": "A",
                "currentCodingAgent": "codex",
                "selectionLocked": locked,
            },
        }))
        .expect("serialize replica config"),
    )
    .expect("write replica config");
    let settings = AppSettings {
        agents: vec![test_agent("codex")],
        project_paths: vec![project.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    (temp, replica, settings)
}

#[tokio::test]
async fn issue_1937_transport_parity_removal_commands_match_inner_and_broadcast() {
    let (_temp, replica, settings) = selection_lock_web_fixture(false);
    let replica_text = replica.to_string_lossy().to_string();
    let (state, mut rx) = ws_state_for(settings);

    let preview_response = dispatch(
        &state,
        21,
        "preview_selection_lock_removal",
        &json!({ "request": { "targetReplicaPath": replica_text, "scope": "replica" } }),
    )
    .await;
    assert!(
        preview_response.get("error").is_none(),
        "{preview_response:?}"
    );
    let inner_preview = crate::commands::config::preview_selection_lock_removal_inner(
        &state.session_mgr,
        &state.settings,
        crate::commands::config::PreviewSelectionLockRemovalRequest {
            target_replica_path: replica_text.clone(),
            scope: crate::commands::config::ProfileAssignmentScope::Replica,
        },
    )
    .await
    .expect("inner preview");
    assert_eq!(
        preview_response["result"],
        serde_json::to_value(&inner_preview).expect("serialize preview"),
        "the transport must delegate the exact request"
    );
    assert!(rx.try_recv().is_err(), "a read broadcasts no event");

    let fingerprint = inner_preview.target_fingerprint.clone();
    let inner_apply = crate::commands::config::apply_selection_lock_removal_inner(
        &state.settings,
        crate::commands::config::ApplySelectionLockRemovalRequest {
            target_replica_path: replica_text.clone(),
            scope: crate::commands::config::ProfileAssignmentScope::Replica,
            confirmed_target_fingerprint: fingerprint.clone(),
        },
    )
    .await
    .expect("inner apply");
    // An already-unlocked removal is a no-op that leaves the fingerprint
    // unchanged, so both transports can run the exact same operation.
    let apply_response = dispatch(
        &state,
        22,
        "apply_selection_lock_removal",
        &json!({ "request": {
            "targetReplicaPath": replica_text,
            "scope": "replica",
            "confirmedTargetFingerprint": fingerprint,
        } }),
    )
    .await;
    assert!(apply_response.get("error").is_none(), "{apply_response:?}");
    assert_eq!(
        apply_response["result"],
        serde_json::to_value(&inner_apply.0).expect("serialize apply"),
        "both transports return the same result"
    );
    let event = match rx.try_recv().expect("broadcast event") {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
        other => panic!("expected text event, got {other:?}"),
    };
    assert_eq!(
        event["event"],
        json!("coding_agent_profile_selection_updated")
    );
    assert_eq!(event["payload"]["operation"], json!("unlock"));
    assert_eq!(event["payload"]["removedCount"], json!(0));
    assert_eq!(
        event["payload"], inner_apply.1,
        "the broadcast payload must equal the inner payload"
    );
}

#[tokio::test]
async fn issue_1937_transport_parity_default_commands_match_inner_and_broadcast() {
    let (_temp, replica, settings) = selection_lock_web_fixture(false);
    let replica_text = replica.to_string_lossy().to_string();
    let matrix = replica
        .parent()
        .and_then(|dir| dir.parent())
        .expect("matrix parent")
        .join("_agent_codex");
    let (state, mut rx) = ws_state_for(settings);

    let get_response = dispatch(
        &state,
        23,
        "get_replica_selection_default",
        &json!({ "request": { "targetReplicaPath": replica_text } }),
    )
    .await;
    assert!(get_response.get("error").is_none(), "{get_response:?}");
    let inner_get = crate::commands::config::get_replica_selection_default_inner(
        &state.settings,
        crate::commands::config::GetReplicaSelectionDefaultRequest {
            target_replica_path: replica_text.clone(),
        },
    )
    .await
    .expect("inner get");
    assert_eq!(
        get_response["result"],
        serde_json::to_value(&inner_get).expect("serialize default")
    );
    let fingerprint = inner_get.default_fingerprint.clone();

    let set_request = crate::commands::config::SetReplicaSelectionDefaultRequest {
        target_replica_path: replica_text.clone(),
        coding_agent_id: "codex".to_string(),
        requested_profile: "B".to_string(),
        selection_locked: true,
        confirmed_default_fingerprint: fingerprint.clone(),
    };
    let inner_set =
        crate::commands::config::set_replica_selection_default_inner(&state.settings, set_request)
            .await
            .expect("inner set");
    // Re-create the pre-write state (default absent) so the transport can
    // execute the exact same CAS with the original fingerprint.
    let mut value: Value =
        serde_json::from_slice(&std::fs::read(matrix.join("config.json")).expect("read"))
            .expect("parse matrix config");
    value["tooling"]
        .as_object_mut()
        .expect("tooling object")
        .remove("replicaSelectionDefault");
    std::fs::write(
        matrix.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("reset matrix default");

    let set_response = dispatch(
        &state,
        24,
        "set_replica_selection_default",
        &json!({ "request": {
            "targetReplicaPath": replica_text,
            "codingAgentId": "codex",
            "requestedProfile": "B",
            "selectionLocked": true,
            "confirmedDefaultFingerprint": fingerprint,
        } }),
    )
    .await;
    assert!(set_response.get("error").is_none(), "{set_response:?}");
    assert_eq!(
        set_response["result"],
        serde_json::to_value(&inner_set.0).expect("serialize set result")
    );
    let event = match rx.try_recv().expect("broadcast event") {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
        other => panic!("expected text event, got {other:?}"),
    };
    assert_eq!(
        event["event"],
        json!("coding_agent_profile_selection_updated")
    );
    assert_eq!(event["payload"], inner_set.1);
    assert_eq!(event["payload"]["scope"], json!("default"));
    assert_eq!(event["payload"]["operation"], json!("default"));
    assert_eq!(
        event["payload"]["agentPath"],
        json!(inner_get.matrix_path.clone())
    );
}

#[tokio::test]
async fn issue_1937_transport_parity_new_commands_reject_unknown_fields() {
    let (_temp, replica, settings) = selection_lock_web_fixture(false);
    let replica_text = replica.to_string_lossy().to_string();
    let replica_bytes = std::fs::read(replica.join("config.json")).expect("read config");
    let matrix = replica
        .parent()
        .and_then(|dir| dir.parent())
        .expect("matrix parent")
        .join("_agent_codex");
    let (state, mut rx) = ws_state_for(settings);

    let cases = [
        (
            "preview_selection_lock_removal",
            json!({ "targetReplicaPath": replica_text, "scope": "replica", "extra": 1 }),
        ),
        (
            "apply_selection_lock_removal",
            json!({
                "targetReplicaPath": replica_text,
                "scope": "replica",
                "confirmedTargetFingerprint": "whatever",
                "extra": 1,
            }),
        ),
        (
            "get_replica_selection_default",
            json!({ "targetReplicaPath": replica_text, "extra": 1 }),
        ),
        (
            "set_replica_selection_default",
            json!({
                "targetReplicaPath": replica_text,
                "codingAgentId": "codex",
                "requestedProfile": "B",
                "selectionLocked": true,
                "confirmedDefaultFingerprint": "whatever",
                "extra": 1,
            }),
        ),
    ];
    for (cmd, request) in cases {
        let response = dispatch(&state, 31, cmd, &json!({ "request": request })).await;
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(error.contains("unknown field"), "{cmd}: {error}");
        assert!(!error.contains("Unknown command"), "{cmd}: {error}");
    }

    assert_eq!(
        std::fs::read(replica.join("config.json")).expect("read config"),
        replica_bytes,
        "rejected requests have no effects"
    );
    assert!(
        !matrix.join("config.json").exists(),
        "rejected requests must not publish a Matrix default"
    );
    assert!(rx.try_recv().is_err(), "rejections broadcast nothing");
}

#[tokio::test]
async fn issue_1937_transport_parity_tauri_registration_presence() {
    let (_temp, replica, settings) = selection_lock_web_fixture(true);
    let replica_text = replica.to_string_lossy().to_string();
    let settings_state: crate::config::settings::SettingsState =
        crate::config::settings::new_settings_state(settings);
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .invoke_handler(tauri::generate_handler![
            crate::commands::config::preview_selection_lock_removal,
            crate::commands::config::apply_selection_lock_removal,
            crate::commands::config::get_replica_selection_default,
            crate::commands::config::set_replica_selection_default,
        ])
        .manage(settings_state)
        .manage(Arc::clone(&session_mgr))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build registration app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build webview");
    let call = |cmd: &str, request: Value| {
        tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: cmd.to_string(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(json!({ "request": request })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
    };

    let preview: Value = call(
        "preview_selection_lock_removal",
        json!({ "targetReplicaPath": replica_text, "scope": "replica" }),
    )
    .expect("preview command is registered")
    .deserialize()
    .expect("deserialize preview");
    assert_eq!(preview["protectedCount"], json!(1));
    let fingerprint = preview["targetFingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_string();

    let apply: Value = call(
        "apply_selection_lock_removal",
        json!({
            "targetReplicaPath": replica_text,
            "scope": "replica",
            "confirmedTargetFingerprint": fingerprint,
        }),
    )
    .expect("apply command is registered")
    .deserialize()
    .expect("deserialize apply");
    assert_eq!(apply["removedCount"], json!(1));

    let get: Value = call(
        "get_replica_selection_default",
        json!({ "targetReplicaPath": replica_text }),
    )
    .expect("default read command is registered")
    .deserialize()
    .expect("deserialize default read");
    assert_eq!(get["default"], Value::Null);
    let default_fingerprint = get["defaultFingerprint"]
        .as_str()
        .expect("default fingerprint")
        .to_string();

    let set: Value = call(
        "set_replica_selection_default",
        json!({
            "targetReplicaPath": replica_text,
            "codingAgentId": "codex",
            "requestedProfile": "B",
            "selectionLocked": true,
            "confirmedDefaultFingerprint": default_fingerprint,
        }),
    )
    .expect("default write command is registered")
    .deserialize()
    .expect("deserialize default write");
    assert_eq!(set["default"]["requestedProfile"], json!("B"));
    assert_eq!(set["default"]["selectionLocked"], json!(true));
}

#[tokio::test]
async fn issue_1937_transport_parity_event_refreshes_another_subscriber_after_waiter_drop() {
    let (_temp, replica, settings) = selection_lock_web_fixture(false);
    let replica_text = replica.to_string_lossy().to_string();
    let matrix = replica
        .parent()
        .and_then(|dir| dir.parent())
        .expect("matrix parent")
        .join("_agent_codex");
    let (state, _rx) = ws_state_for(settings);
    let mut other_subscriber = state.broadcaster.subscribe();

    let initial = crate::commands::config::get_replica_selection_default_inner(
        &state.settings,
        crate::commands::config::GetReplicaSelectionDefaultRequest {
            target_replica_path: replica_text.clone(),
        },
    )
    .await
    .expect("initial default read")
    .default_fingerprint;

    // Hold the owned operation turn so the dispatch is still queued when its
    // caller (the WebSocket waiter) is dropped.
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let holder = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || async move {
            entered_tx.send(()).expect("signal entered");
            release_rx.await.expect("release holder");
            Ok::<(), String>(())
        },
    ));
    entered_rx.await.expect("holder entered");

    let state_for_dispatch = state.clone();
    let dispatch_task = tokio::spawn(async move {
        dispatch(
            &state_for_dispatch,
            41,
            "set_replica_selection_default",
            &json!({ "request": {
                "targetReplicaPath": replica_text,
                "codingAgentId": "codex",
                "requestedProfile": "B",
                "selectionLocked": true,
                "confirmedDefaultFingerprint": initial,
            } }),
        )
        .await
    });
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !dispatch_task.is_finished(),
        "the dispatch must wait for the owned turn"
    );

    // The WebSocket waiter goes away; the owned task keeps the operation.
    dispatch_task.abort();
    let _ = dispatch_task.await;
    release_tx.send(()).expect("release holder");
    holder
        .await
        .expect("holder join")
        .expect("holder operation");

    // Another subscriber must still receive the completion event.
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), other_subscriber.recv())
        .await
        .expect("event arrives")
        .expect("subscriber channel open");
    let event = match event {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
        other => panic!("expected text event, got {other:?}"),
    };
    assert_eq!(
        event["event"],
        json!("coding_agent_profile_selection_updated")
    );
    assert_eq!(event["payload"]["operation"], json!("default"));
    assert_eq!(event["payload"]["scope"], json!("default"));
    let disk: Value =
        serde_json::from_slice(&std::fs::read(matrix.join("config.json")).expect("read"))
            .expect("parse matrix config");
    assert_eq!(
        disk["tooling"]["replicaSelectionDefault"]["requestedProfile"],
        json!("B"),
        "the detached owned task still completed the write"
    );
}

/// #1941 rework ronda 1 - the real WebSocket dispatch route for the
/// assignment preview must queue behind the owned selection turn. On the
/// pre-fix arm (no `run_owned_selection_operation`) the dispatch completes
/// while the turn is held and reads the half-settled pre-publication state.
#[tokio::test]
async fn issue_1937_transport_parity_assignment_preview_queues_behind_the_turn() {
    let (_temp, replica, settings) = selection_lock_web_fixture(false);
    let replica_text = replica.to_string_lossy().to_string();
    let (state, _rx) = ws_state_for(settings);

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let settings_for_holder = state.settings.clone();
    let replica_for_holder = replica.clone();
    // The holder owns the turn while a mutation is still in flight; the
    // publication happens after the barrier, so a racing preview would
    // observe the unlocked pre-publication state.
    let holder = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_holder;
            let replica = replica_for_holder;
            async move {
                entered_tx.send(()).expect("signal entered");
                release_rx.await.expect("release holder");
                let snapshot = settings.read().await.clone();
                let expected =
                    crate::config::coding_agent_profiles::read_replica_selection_state(&replica)
                        .expectation()
                        .expect("fresh expectation");
                crate::config::coding_agent_profiles::write_replica_selection(
                    &snapshot,
                    &replica,
                    &crate::config::coding_agent_profiles::ReplicaSelectionPair {
                        coding_agent_id: "codex".to_string(),
                        requested_profile: "B".to_string(),
                    },
                    SelectionWriteIntent::IndividualAssignLock,
                    &expected,
                )
                .expect("publish the lock");
                Ok::<(), String>(())
            }
        },
    ));
    entered_rx.await.expect("holder entered");

    let state_for_dispatch = state.clone();
    let dispatch_task = tokio::spawn(async move {
        dispatch(
            &state_for_dispatch,
            51,
            "preview_coding_agent_profile_selection",
            &json!({ "request": {
                "targetReplicaPath": replica_text,
                "codingAgentId": "codex",
                "profile": "B",
                "scope": "replica",
                "restartSessions": false,
            } }),
        )
        .await
    });
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !dispatch_task.is_finished(),
        "the transport preview must queue behind the owned selection turn"
    );

    release_tx.send(()).expect("release holder");
    holder
        .await
        .expect("holder join")
        .expect("holder operation");
    let response = dispatch_task.await.expect("dispatch join");
    assert!(response.get("error").is_none(), "{response:?}");
    assert_eq!(
        response["result"]["targets"][0]["selectionState"],
        json!("locked")
    );
    assert_eq!(
        response["result"]["targets"][0]["savedPair"]["requestedProfile"],
        json!("B"),
        "the queued preview must see the settled publication"
    );
}

/// #1271 - full create-path harness for the web dispatcher. `ws_state_for`
/// lacks the SelectionCoordinator and the other states `create_session_inner`
/// requires, so the web-path invalid-input postcondition test builds them
/// here, mirroring the pty_lifecycle harness (plan Finding F).
#[cfg(windows)]
fn ws_state_for_1271(settings: AppSettings) -> WsState {
    let app = crate::test_support::test_builder()
        .manage(crate::session::warnings::new_session_warning_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build 1271 web test app");
    let app_handle = app.handle().clone();
    let broadcaster = WsBroadcaster::new();
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
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        shutdown.token().clone(),
    );
    let settings = crate::config::settings::new_settings_state(settings);

    let full_app = crate::test_support::test_builder()
        .manage(settings.clone())
        .manage(Arc::clone(&session_mgr))
        .manage(Arc::clone(&pty_mgr))
        .manage(coordinator.clone())
        .manage(Arc::new(
            crate::resource_monitor::ResourceMonitorState::new(),
        ))
        .manage(Arc::new(crate::RestoreInProgress(
            std::sync::atomic::AtomicBool::new(false),
        )))
        .manage(crate::MasterToken::new("1271-web-test".into()))
        .manage(crate::AppOutbox::new(
            std::env::temp_dir()
                .join("agentscommander-1271-web-outbox")
                .to_string_lossy()
                .to_string(),
        ))
        .manage(crate::DetachedSessionsState::default())
        .manage(Arc::new(crate::voice::tracker::VoiceTracker::new()))
        .manage(shutdown)
        .manage(Arc::new(crate::web::auth::WebAccessToken::new(
            "1271-web-token".into(),
        )))
        .manage(broadcaster.clone())
        .manage(crate::WebServerHandle::default())
        .manage(crate::SpecBoardState::default())
        .manage(crate::ConfigSeedLockState::default())
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(tauri::async_runtime::Mutex::new(
            crate::telegram::manager::TelegramBridgeManager::new(Arc::new(Mutex::new(
                HashMap::new(),
            ))),
        )))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build 1271 full web test app");
    let full_handle = full_app.handle().clone();
    coordinator
        .start(full_handle.clone())
        .expect("start selection coordinator");
    let bootstrap = coordinator.clone();
    std::thread::spawn(move || {
        tauri::async_runtime::block_on(async move {
            bootstrap
                .submit_restore_first()
                .await
                .expect("open selection coordinator")
                .finish();
        });
    })
    .join()
    .expect("join selection bootstrap");
    std::mem::forget(full_app);

    WsState {
        session_mgr,
        pty_mgr,
        settings,
        broadcaster,
        app_handle: full_handle,
    }
}

// #1271 - the adapter's deterministic rejections are Windows-only (the
// host-shell adapter lives in the Windows spawn branch), so the full
// invalid-input postcondition rows are verified on Windows at both the
// session layer (here and the native twin) and the backend seam
// (adapter_spawn_sync_tests).
#[cfg(windows)]
#[tokio::test]
async fn configured_default_shell_invalid_input_leaves_no_session_state_via_web() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    // A conflicting/terminal configured option must fail before any PTY.
    let settings = AppSettings {
        default_shell: "powershell.exe".to_string(),
        default_shell_args: vec!["-Command".to_string()],
        agents: vec![test_agent("claude")],
        ..AppSettings::default()
    };
    let state = ws_state_for_1271(settings);

    let response = dispatch(
        &state,
        7,
        "create_session",
        &json!({ "cwd": cwd, "agentId": "claude" }),
    )
    .await;
    let error = response["error"]
        .as_str()
        .expect("invalid configured host must fail the create");
    assert!(error.contains("conflicting/terminal"), "{error}");
    assert!(
        error.contains("agent adapter owns command execution"),
        "{error}"
    );

    // Full no-side-effect postcondition through the web creation path: no
    // session row and no pending/metadata record survive the rejection.
    assert!(
        state
            .session_mgr
            .read()
            .await
            .list_sessions()
            .await
            .is_empty(),
        "no session may appear in the session manager"
    );
    // The adapter rejection happens at the TOP of spawn_sync, before any
    // spawn accounting or PTY acquisition, so while the coordinator worker
    // still holds the pending binding (the rollback is async) the rejected
    // id must already show no launch provenance, no PTY map entry, and no
    // output task.
    let pending_ids = state
        .session_mgr
        .read()
        .await
        .aggregate_snapshot()
        .await
        .pending_ids;
    for id in &pending_ids {
        assert!(
            crate::pty::spawn_diagnostics::record_for(*id).is_none(),
            "no launch provenance may be recorded for the rejected session"
        );
        assert!(
            !state.pty_mgr.lock().unwrap().has_session(*id),
            "no PTY map entry may exist for the rejected session"
        );
        assert!(
            state.pty_mgr.lock().unwrap().get_pty_size(*id).is_none(),
            "no output task may be attached for the rejected session"
        );
    }
    // The coordinator worker rolls the pending binding back asynchronously
    // (CreateFinalizationTicket::drop -> RollbackCreate), so poll for it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let pending_empty = state
            .session_mgr
            .read()
            .await
            .aggregate_snapshot()
            .await
            .pending_ids
            .is_empty();
        if pending_empty {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no pending session/metadata record may survive the rejection"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[cfg(windows)]
#[tokio::test]
async fn configured_default_shell_invalid_payload_leaves_no_session_state_via_web() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    // The claude agent's logical argv carries a space token, which the cmd
    // payload domain rejects before PTY creation.
    let mut agent = test_agent("claude");
    agent.command = "claude \"a b\"".to_string();
    let settings = AppSettings {
        default_shell: "cmd.exe".to_string(),
        default_shell_args: vec!["/D".to_string()],
        agents: vec![agent],
        ..AppSettings::default()
    };
    let state = ws_state_for_1271(settings);

    let response = dispatch(
        &state,
        7,
        "create_session",
        &json!({ "cwd": cwd, "agentId": "claude" }),
    )
    .await;
    let error = response["error"]
        .as_str()
        .expect("invalid cmd payload must fail the create");
    assert!(
        error.contains("unsupported cmd payload character"),
        "{error}"
    );
    assert!(state
        .session_mgr
        .read()
        .await
        .list_sessions()
        .await
        .is_empty());
    let pending_ids = state
        .session_mgr
        .read()
        .await
        .aggregate_snapshot()
        .await
        .pending_ids;
    for id in &pending_ids {
        assert!(crate::pty::spawn_diagnostics::record_for(*id).is_none());
        assert!(!state.pty_mgr.lock().unwrap().has_session(*id));
        assert!(state.pty_mgr.lock().unwrap().get_pty_size(*id).is_none());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let pending_empty = state
            .session_mgr
            .read()
            .await
            .aggregate_snapshot()
            .await
            .pending_ids
            .is_empty();
        if pending_empty {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no pending session/metadata record may survive the rejection"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------
// #1551 - agent-update route parity
// ---------------------------------------------------------------------
