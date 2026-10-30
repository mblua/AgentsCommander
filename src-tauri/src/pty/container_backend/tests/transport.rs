use super::*;

#[tokio::test]
async fn canceled_handshake_stops_installed_runtime_handle_and_removes_pending_state() {
    let id = Uuid::new_v4();
    let root_dir = tempfile::TempDir::new().unwrap();
    let root = root_dir.path().to_string_lossy().into_owned();
    let runtime = Arc::new(RecordingRuntime::default());
    let token_dir = tempfile::TempDir::new().unwrap();
    let token_manager =
        ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
    let (mut backend, _manager) = backend_with_tuning(ContainerTransportTuning {
        handshake_timeout: Duration::from_secs(30),
        ..ContainerTransportTuning::default()
    });
    backend.runtime = Some(runtime.clone());
    backend.token_manager = Some(token_manager);
    backend.runtime_settings_override = Some(api_enabled_settings());
    let backend = Arc::new(backend);
    let task_backend = Arc::clone(&backend);
    let mut spec = test_spec(id, &root, PtyOutputTarget::noop());
    spec.container_image = Some("agentscommander/test:latest".to_string());
    let task = tokio::spawn(async move { task_backend.spawn(spec).await });

    let mut installed = false;
    for _ in 0..100 {
        installed = matches!(
            backend.sessions.lock().unwrap().get(&id),
            Some(ContainerSessionState::Pending(pending))
                if pending.runtime_handle.is_some()
        );
        if installed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        installed,
        "runtime handle was not installed before handshake wait"
    );
    task.abort();
    let _ = task.await;

    for _ in 0..100 {
        if runtime.stopped().contains(&id) && !backend.sessions.lock().unwrap().contains_key(&id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("canceled handshake leaked its runtime handle or pending state");
}

/// #2413 T16: the runtime path marks the launch witness before the start job,
/// so a handshake timeout that removed the session state still reports a
/// launch; a pre-start `Err` does not.
#[tokio::test]
async fn runtime_spawn_marks_launch_witness_before_start_and_keeps_it_after_state_removal() {
    let id = Uuid::new_v4();
    let root_dir = tempfile::TempDir::new().unwrap();
    let root = root_dir.path().to_string_lossy().into_owned();
    // A permanently blocked stop would keep `spawn` awaiting its cleanup
    // forever (no `Err` to observe), so the stop here returns.
    let runtime = Arc::new(RecordingRuntime::default());
    let token_dir = tempfile::TempDir::new().unwrap();
    let token_manager =
        ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
    let (mut backend, _manager) = backend_with_tuning(ContainerTransportTuning {
        handshake_timeout: Duration::from_millis(100),
        ..ContainerTransportTuning::default()
    });
    backend.runtime = Some(runtime.clone());
    backend.token_manager = Some(token_manager);
    backend.runtime_settings_override = Some(api_enabled_settings());
    let mut spec = test_spec(id, &root, PtyOutputTarget::noop());
    spec.container_image = Some("agentscommander/test:latest".to_string());
    let witness = spec.launch_witness.clone();
    tokio::time::timeout(Duration::from_secs(10), backend.spawn(spec))
        .await
        .expect("spawn returns on handshake timeout")
        .expect_err("no bridge attaches");
    assert!(!backend.has_session(id), "session state removed");
    assert!(witness.launched(), "runtime start was queued: launched");

    // Pre-start `Err`: no API token manager -> nothing launched.
    let id = Uuid::new_v4();
    let (mut backend, _manager) = backend_with_tuning(ContainerTransportTuning::default());
    backend.runtime = Some(Arc::new(RecordingRuntime::default()));
    backend.runtime_settings_override = Some(api_enabled_settings());
    let mut spec = test_spec(id, &root, PtyOutputTarget::noop());
    spec.container_image = Some("agentscommander/test:latest".to_string());
    let witness = spec.launch_witness.clone();
    let error = backend.spawn(spec).await.expect_err("no token manager");
    assert!(
        error
            .to_string()
            .contains("token manager is not configured"),
        "{error}"
    );
    assert!(!witness.launched(), "pre-start Err: nothing launched");
}

// #930 plan section 9.a - a teardown funnel (kill -> remove_session_state ->
// cleanup_removed_resources_async) must delete the copied host credential so
// no live refresh token is left in the workspace tree.
#[test]
fn teardown_deletes_copied_credential() {
    let id = Uuid::new_v4();
    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join(".claude").join(".credentials.json");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, b"refresh-token-bytes").unwrap();
    assert!(dest.is_file(), "precondition: credential present");

    let (backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
    // No runtime/token_manager: cleanup runs remove_copied first, then the
    // runtime-stop and token-revoke branches are skipped (both None).
    let (tx, _rx) = mpsc::channel(8);
    backend.sessions.lock().unwrap().insert(
        id,
        ContainerSessionState::Active(ActiveSession {
            sender: tx,
            rows: 30,
            cols: 120,
            runtime_handle: None,
            api_client_id: None,
            credential_binding: None,
            logical_resource_slot: None,
            container_credential_path: Some(dest.clone()),
        }),
    );

    backend.kill(id).expect("kill");
    let report =
        backend.seal_and_drain_shutdown_work_blocking(Instant::now() + Duration::from_secs(1));
    assert!(report.terminal, "retained={:?}", report.retained);
    assert!(backend.retained_cleanup_sessions_for_test().is_empty());

    assert!(!dest.exists(), "teardown must delete the copied credential");
}

// #930 F1 - the teardown-during-spawn race: if the session is gone from the
// map at the post-copy_in recheck, the recheck deletes the orphaned file;
// if it is still registered, the file survives for the container. Drives the
// real `remove_credential_if_orphaned` (the extracted recheck).
#[test]
fn f1_recheck_removes_credential_when_session_gone() {
    let id = Uuid::new_v4();
    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join(".claude").join(".credentials.json");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, b"refresh-token-bytes").unwrap();
    let plan = crate::pty::container_credentials::ContainerCredentialPlan {
        source: dir.path().join("unused-source"),
        dest: dest.clone(),
        first_run: None,
    };

    let (backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
    let (tx, _rx) = mpsc::channel(8);
    backend.sessions.lock().unwrap().insert(
        id,
        ContainerSessionState::Active(ActiveSession {
            sender: tx,
            rows: 30,
            cols: 120,
            runtime_handle: None,
            api_client_id: None,
            credential_binding: None,
            logical_resource_slot: None,
            container_credential_path: Some(dest.clone()),
        }),
    );

    // Session still registered (normal spawn): recheck is a no-op, file survives.
    backend.remove_credential_if_orphaned(id, &plan);
    assert!(
        dest.is_file(),
        "file must survive while the session is registered"
    );

    // Session removed before the recheck (teardown-during-spawn): recheck deletes.
    backend.sessions.lock().unwrap().remove(&id);
    backend.remove_credential_if_orphaned(id, &plan);
    assert!(
        !dest.exists(),
        "recheck must delete the orphaned credential when the session is gone"
    );
}

#[test]
fn sanitized_child_env_removes_reserved_credentials_and_removed_keys() {
    let env = vec![
        ("CODEX_HOME".to_string(), "/workspace/.codex".to_string()),
        (
            "AGENTSCOMMANDER_TOKEN".to_string(),
            "session-token".to_string(),
        ),
        (
            "AGENTSCOMMANDER_BINARY_PATH".to_string(),
            "C:/host/ac.exe".to_string(),
        ),
        ("DROP_ME".to_string(), "x".to_string()),
    ];
    let got = sanitized_child_env(env, vec!["drop_me".to_string()]);

    assert_eq!(
        got,
        vec![("CODEX_HOME".to_string(), "/workspace/.codex".to_string())]
    );
}

fn child_env_map() -> ContainerPathMap {
    ContainerPathMap::new(r"C:\Users\maria\repo\.ac\wg-1\__agent_x", "/workspace").unwrap()
}

#[test]
fn container_child_env_translates_mappable_host_path_keys() {
    let got = container_child_env(
        vec![
            (
                CLAUDE_CONFIG_DIR_KEY.to_string(),
                r"C:\Users\maria\repo\.ac\wg-1\__agent_x\.claude".to_string(),
            ),
            ("MY_VAR".to_string(), r"C:\Users\maria\.outside".to_string()),
        ],
        Vec::new(),
        &child_env_map(),
    );

    assert_eq!(
        got.child_env,
        vec![
            (
                CLAUDE_CONFIG_DIR_KEY.to_string(),
                "/workspace/.claude".to_string()
            ),
            ("MY_VAR".to_string(), r"C:\Users\maria\.outside".to_string())
        ]
    );
    assert!(got.env_unset.is_empty());
    assert!(got.warnings.is_empty());
}

#[test]
fn container_child_env_unsets_unmappable_host_path_keys() {
    let got = container_child_env(
        vec![
            (
                CLAUDE_CONFIG_DIR_KEY.to_string(),
                r"C:\Users\maria\.claude".to_string(),
            ),
            (
                crate::pty::container_paths::CODEX_HOME_KEY.to_string(),
                "/workspace/.codex".to_string(),
            ),
        ],
        Vec::new(),
        &child_env_map(),
    );

    assert!(got.child_env.is_empty());
    assert_eq!(
        got.env_unset,
        vec![
            CLAUDE_CONFIG_DIR_KEY.to_string(),
            crate::pty::container_paths::CODEX_HOME_KEY.to_string()
        ]
    );
    assert_eq!(
        got.warnings
            .iter()
            .map(|warning| (warning.key.as_str(), warning.kind))
            .collect::<Vec<_>>(),
        vec![
            (
                CLAUDE_CONFIG_DIR_KEY,
                crate::pty::container_paths::WARNING_KIND_OUTSIDE_MOUNT
            ),
            (
                crate::pty::container_paths::CODEX_HOME_KEY,
                crate::pty::container_paths::WARNING_KIND_CONTAINER_PATH_IN_HOST_FIELD
            )
        ]
    );
}

#[test]
fn container_child_env_removal_suppresses_unset_and_warning() {
    let got = container_child_env(
        vec![(
            CLAUDE_CONFIG_DIR_KEY.to_string(),
            r"C:\Users\maria\.claude".to_string(),
        )],
        vec![CLAUDE_CONFIG_DIR_KEY.to_string()],
        &child_env_map(),
    );

    assert!(got.child_env.is_empty());
    assert!(got.env_unset.is_empty());
    assert!(got.warnings.is_empty());
}

fn token() -> ContainerApiToken {
    ContainerApiToken {
        client_id: "client".to_string(),
        credential_generation: Uuid::new_v4().to_string(),
        bound_session_id: Uuid::new_v4().to_string(),
        secret: "secret".to_string(),
        token_hash: crate::api::auth::hash_token("secret"),
    }
}

#[test]
fn build_start_request_honors_container_image_override() {
    let id = Uuid::new_v4();
    let request = build_start_request_with_settings(
        id,
        "claude",
        Vec::new(),
        "C:/repo/.ac/wg-1/__agent_dev",
        30,
        120,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Some(" agentscommander/ac-claude:latest ".to_string()),
        None,
        "ticket".to_string(),
        &token(),
        &api_enabled_settings(),
        Vec::new(),
    )
    .unwrap();

    assert_eq!(request.image, "agentscommander/ac-claude:latest");
}

#[test]
fn handshake_timeout_error_includes_container_diagnostics() {
    let diagnostics = ContainerDiagnostics {
        container_id: "container-123".to_string(),
        state: Some(crate::pty::container_runtime::ContainerStateSnapshot {
            status: Some("exited".to_string()),
            running: Some(false),
            exit_code: Some(127),
            error: None,
        }),
        inspect_error: None,
        log_tail: Some("session-bridge error: command not found: claude".to_string()),
        logs_error: None,
    };

    let err = ContainerTransportBackend::handshake_timeout_error(
        Duration::from_secs(5),
        Some(&diagnostics),
    )
    .to_string();

    assert!(
        err.contains("container bridge did not attach within 5s"),
        "{err}"
    );
    assert!(err.contains("container id container-123"), "{err}");
    assert!(err.contains("exitCode=127"), "{err}");
    assert!(err.contains("command not found: claude"), "{err}");
}

#[test]
fn handshake_timeout_error_leads_when_diagnostics_fail() {
    let diagnostics = ContainerDiagnostics {
        container_id: "container-123".to_string(),
        state: None,
        inspect_error: Some("inspect failed".to_string()),
        log_tail: None,
        logs_error: Some("logs failed".to_string()),
    };

    let err = ContainerTransportBackend::handshake_timeout_error(
        Duration::from_secs(5),
        Some(&diagnostics),
    )
    .to_string();

    assert!(
        err.starts_with("PTY error: container bridge did not attach within 5s"),
        "{err}"
    );
    assert!(err.contains("container id container-123"), "{err}");
    assert!(err.contains("logs unavailable: logs failed"), "{err}");
}

#[tokio::test]
async fn valid_ticket_plus_hello_registers_container_session() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, ticket) = pending_backend(id, root).await;

    let _rx = attach(&backend, id, root, &ticket);

    assert!(backend.has_session(id));
}

#[tokio::test]
async fn ticket_rejects_missing_expired_consumed_wrong_session_and_wrong_root() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, ticket) = pending_backend(id, root).await;

    assert_eq!(
        backend.consume_ticket(id, root, "wrong").unwrap_err(),
        TransportTicketError::Invalid
    );
    assert_eq!(
        backend
            .consume_ticket(Uuid::new_v4(), root, &ticket)
            .unwrap_err(),
        TransportTicketError::Invalid
    );
    assert_eq!(
        backend
            .consume_ticket(id, "C:/other/.ac/wg-1/__agent_dev", &ticket)
            .unwrap_err(),
        TransportTicketError::Invalid
    );

    backend.consume_ticket(id, root, &ticket).expect("consume");
    assert_eq!(
        backend.consume_ticket(id, root, &ticket).unwrap_err(),
        TransportTicketError::Invalid
    );

    let expired_id = Uuid::new_v4();
    let tuning = ContainerTransportTuning {
        ticket_ttl: Duration::from_millis(1),
        ..ContainerTransportTuning::default()
    };
    let (expired_backend, _mgr) = backend_with_tuning(tuning);
    expired_backend
        .spawn(test_spec(expired_id, root, PtyOutputTarget::noop()))
        .await
        .expect("spawn expired");
    let expired_ticket = expired_backend
        .last_issued_ticket_for_test(expired_id)
        .expect("expired ticket");
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        expired_backend
            .consume_ticket(expired_id, root, &expired_ticket)
            .unwrap_err(),
        TransportTicketError::Invalid
    );
}

#[tokio::test]
async fn output_before_hello_is_rejected() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, ticket) = pending_backend(id, root).await;
    backend
        .consume_ticket(id, root, &ticket)
        .expect("consume ticket");

    let err = backend
        .handle_bridge_output(id, b"early".to_vec())
        .expect_err("output before hello");

    assert!(matches!(err, AppError::SessionNotFound(_)));
}

#[tokio::test]
async fn binary_output_reaches_fanout_payload_shape() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let captured = Arc::new(Mutex::new(Vec::new()));
    let target = PtyOutputTarget::from_test_sink(captured.clone());
    let (backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
    backend
        .spawn(test_spec(id, root, target))
        .await
        .expect("spawn");
    let ticket = backend.last_issued_ticket_for_test(id).unwrap();
    let _rx = attach(&backend, id, root, &ticket);
    backend
        .activate_terminal_output(id, "main", true)
        .expect("attach");

    backend
        .handle_bridge_output(id, b"hello".to_vec())
        .expect("output");

    // The flush is driven by hand: the 16 ms timer is the only spawned thing on this path
    // and no assertion here depends on it.
    backend.fanout.flush_terminal_output_for_test(id);
    let got = captured.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, id.to_string());
    assert_eq!(got[0].1, b"hello".to_vec());
    assert_eq!(got[0].2, Some(1));
}

#[tokio::test]
async fn facade_write_sends_binary_input_to_bridge() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, ticket) = pending_backend(id, root).await;
    let mut rx = attach(&backend, id, root, &ticket);

    backend
        .write(
            &crate::pty::manager::BackendWriteAuthority::for_backend_test(),
            id,
            b"abc",
        )
        .expect("write");

    assert_eq!(
        rx.recv().await.expect("bridge frame"),
        HostToBridgeFrame::Binary(b"abc".to_vec())
    );
}

#[tokio::test]
async fn resize_sends_frame_and_updates_size() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, ticket) = pending_backend(id, root).await;
    let mut rx = attach(&backend, id, root, &ticket);

    backend.resize(id, 100, 40).expect("resize");

    assert_eq!(
        rx.recv().await.expect("resize frame"),
        HostToBridgeFrame::Text(HostToBridgeTextFrame::Resize {
            version: TRANSPORT_PROTOCOL_VERSION,
            cols: 100,
            rows: 40,
        })
    );
    assert_eq!(backend.get_pty_size(id), Some((40, 100)));
}

#[tokio::test]
async fn duplicate_attach_for_same_session_is_rejected() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, ticket) = pending_backend(id, root).await;
    let _rx = attach(&backend, id, root, &ticket);
    let (tx, _rx2) = mpsc::channel(8);

    assert_eq!(
        backend.complete_hello(id, root, tx).unwrap_err(),
        TransportAttachError::Invalid
    );
}

#[tokio::test]
async fn exit_and_disconnect_cleanup_remove_route_and_transport_state() {
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    for code in [7, TRANSPORT_LOST_EXIT_CODE] {
        let (backend, session_mgr) = backend_with_tuning(ContainerTransportTuning::default());
        let removed = Arc::new(AtomicUsize::new(0));
        let removed_for_cb = removed.clone();
        backend.set_route_remover(Arc::new(move |_, _| {
            removed_for_cb.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let session = session_mgr
            .read()
            .await
            .create_session(
                "container".to_string(),
                Vec::new(),
                root.to_string(),
                Some("agent".to_string()),
                None,
                Vec::new(),
                false,
                SessionBackendKind::ContainerTransport,
            )
            .await
            .expect("session");
        backend
            .spawn(test_spec(session.id, root, PtyOutputTarget::noop()))
            .await
            .expect("spawn");
        let ticket = backend.last_issued_ticket_for_test(session.id).unwrap();
        let _rx = attach(&backend, session.id, root, &ticket);

        if code == 7 {
            backend.handle_bridge_exit(session.id, code).await;
        } else {
            backend.handle_bridge_disconnect(session.id).await;
        }

        assert_eq!(removed.load(Ordering::SeqCst), 1);
        assert!(!backend.has_session(session.id));
        assert!(!backend.has_session(session.id));
    }
}

/// #1171, 9.1.4 (container half) - an id this backend has no parser for is `Gone`, not
/// `Missing`.
///
/// This backend tears down on a natural exit and `close_transport` drops the parser before
/// anyone could read it, so parser-absent IS its liveness oracle - the same reading its
/// `get_screen_rows` already gives (`:3171-3179`). The local backend answers `Missing` to
/// the identical question, which is the whole reason `ScreenRowsSince` has four variants.
#[test]
fn screen_rows_since_reports_gone_for_an_unknown_id() {
    let backend = ContainerTransportBackend::new(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        None,
        None,
    );

    assert!(matches!(
        backend.screen_rows_since(Uuid::new_v4(), None),
        ScreenRowsSince::Gone
    ));
}

#[tokio::test]
async fn synchronous_queue_full_cleanup_defers_outer_route_removal_past_pty_lock() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let output_senders = Arc::new(Mutex::new(HashMap::new()));
    let idle = IdleDetector::new(|_| {}, |_| {});
    let dummy_local = Arc::new(ContainerTransportBackend::new(
        output_senders,
        idle,
        None,
        None,
    ));
    let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        dummy_local,
    )));
    let backend = pty.lock().unwrap().container_backend();
    backend
        .spawn(test_spec(id, root, PtyOutputTarget::noop()))
        .await
        .expect("spawn pending transport");
    let ticket = backend.last_issued_ticket_for_test(id).unwrap();
    let _receiver = attach(&backend, id, root, &ticket);
    pty.lock()
        .unwrap()
        .record_route(id, SessionBackendKind::ContainerTransport);

    let weak_pty = Arc::downgrade(&pty);
    let (removed_tx, removed_rx) = std::sync::mpsc::channel();
    backend.set_route_remover(Arc::new(move |session_id, _| {
        if let Some(pty) = weak_pty.upgrade() {
            pty.lock()
                .unwrap()
                .remove_route_if_kind(session_id, SessionBackendKind::ContainerTransport);
        }
        let _ = removed_tx.send(());
        Ok(())
    }));

    let guard = pty.lock().unwrap();
    let authority = crate::pty::manager::BackendWriteAuthority::for_backend_test();
    for _ in 0..8 {
        backend
            .write(&authority, id, b"x")
            .expect("fill outbound queue");
    }
    let started = Instant::now();
    let error = backend
        .write(&authority, id, b"overflow")
        .expect_err("ninth frame must close a full queue");
    assert!(error.to_string().contains("outbound queue full"));
    assert!(started.elapsed() < Duration::from_millis(250));
    assert!(removed_rx.try_recv().is_err());
    assert_eq!(
        guard.backend_kind(id),
        Some(SessionBackendKind::ContainerTransport)
    );
    drop(guard);

    removed_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("deferred outer route removal");
    assert_eq!(pty.lock().unwrap().backend_kind(id), None);
    assert!(!backend.has_session(id));
}

#[tokio::test]
async fn heartbeat_timeout_close_removes_session() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let tuning = ContainerTransportTuning {
        heartbeat_interval: Duration::from_millis(1),
        max_idle: Duration::from_millis(1),
        ..ContainerTransportTuning::default()
    };
    let (backend, _mgr) = backend_with_tuning(tuning);
    backend
        .spawn(test_spec(id, root, PtyOutputTarget::noop()))
        .await
        .expect("spawn");
    let ticket = backend.last_issued_ticket_for_test(id).unwrap();
    let _rx = attach(&backend, id, root, &ticket);

    tokio::time::sleep(Duration::from_millis(3)).await;
    backend.handle_bridge_disconnect(id).await;

    assert!(!backend.has_session(id));
}

#[test]
fn parses_versioned_frames_and_rejects_large_payloads() {
    let text = serde_json::json!({
        "type": "hello",
        "version": TRANSPORT_PROTOCOL_VERSION,
        "sessionId": Uuid::nil(),
        "root": "C:/root"
    })
    .to_string();
    let frame = parse_bridge_text_frame(&text).expect("parse");
    assert_eq!(frame.version(), TRANSPORT_PROTOCOL_VERSION);

    assert!(b"x".repeat(MAX_TRANSPORT_FRAME_BYTES + 1).len() > MAX_TRANSPORT_FRAME_BYTES);
}

// #992 - the posture predicate. Never assert on the spawned thread; assert on the
// decision it makes.
fn token_manager_at(dir: &tempfile::TempDir) -> ContainerApiTokenManager {
    ContainerApiTokenManager::new_for_path(dir.path().join("api-clients.json"))
}

#[test]
fn sweep_is_load_bearing_is_false_without_container_clients() {
    let dir = tempfile::TempDir::new().unwrap();
    let manager = token_manager_at(&dir);
    assert!(!ContainerTransportBackend::sweep_is_load_bearing(Some(
        &manager
    )));
}

#[test]
fn sweep_is_load_bearing_is_true_after_a_container_token_was_minted() {
    let dir = tempfile::TempDir::new().unwrap();
    let manager = token_manager_at(&dir);
    manager
        .mint_for_session(Uuid::new_v4(), "C:/project/.ac/wg-1-team/__agent_dev")
        .unwrap();
    assert!(ContainerTransportBackend::sweep_is_load_bearing(Some(
        &manager
    )));
}

#[test]
fn sweep_is_load_bearing_is_true_for_an_expired_and_revoked_client() {
    // The ex-container user's REAL steady state: the token expired after 24h AND a
    // later startup revoked it. `mint_for_session` cannot build it, because it
    // stamps expiry at +24h; the first cut of this test did exactly that and so
    // never expired anything. A predicate such as
    // `prefix && (!client.revoked || !is_expired(client))` survives every other test
    // here and answers false for this state, downgrading the sweep for precisely the
    // users whose orphans it exists to find.
    use crate::api::auth::{self, MintRequest, SCOPE_SEND};
    use chrono::{Duration as ChronoDuration, Utc};

    let dir = tempfile::TempDir::new().unwrap();
    let manager = token_manager_at(&dir);
    let client_id = format!("container-{}", Uuid::new_v4());
    auth::mint(
        manager.path(),
        MintRequest {
            client_id: client_id.clone(),
            secret: "expired-secret".to_string(),
            label: format!("container:{}", Uuid::new_v4()),
            bound_root: "C:/project/.ac/wg-1-team/__agent_dev".to_string(),
            bound_fqn: "project:wg-1-team/dev".to_string(),
            scopes: vec![SCOPE_SEND.to_string()],
            issued_at: (Utc::now() - ChronoDuration::hours(72)).to_rfc3339(),
            expires_at: Some((Utc::now() - ChronoDuration::hours(48)).to_rfc3339()),
            bound_session_id: None,
            credential_generation: None,
        },
    )
    .unwrap();
    manager.revoke(&client_id);

    // Prove the fixture is the state the test is named for, or it proves nothing.
    let client = auth::list(manager.path())
        .clients
        .into_iter()
        .find(|client| client.client_id == client_id)
        .expect("the client the test minted");
    let expiry =
        chrono::DateTime::parse_from_rfc3339(client.expires_at.as_deref().expect("expires_at"))
            .expect("rfc3339 expiry")
            .with_timezone(&Utc);
    assert!(
        expiry < Utc::now(),
        "this test must actually expire the client"
    );
    assert!(client.revoked, "this test must actually revoke the client");

    assert!(ContainerTransportBackend::sweep_is_load_bearing(Some(
        &manager
    )));
}

#[test]
fn sweep_is_load_bearing_fails_load_bearing_without_a_token_manager() {
    // An unknown answer must never downgrade the sweep.
    assert!(ContainerTransportBackend::sweep_is_load_bearing(None));
}

#[test]
fn sweep_is_load_bearing_fails_load_bearing_on_a_malformed_registry() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("api-clients.json");
    std::fs::write(&path, "{").unwrap();
    let manager = ContainerApiTokenManager::new_for_path(path);
    assert!(ContainerTransportBackend::sweep_is_load_bearing(Some(
        &manager
    )));
}

struct OffRuntimeLifecycle {
    events: std::sync::mpsc::Sender<(Uuid, Option<i32>)>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl crate::pty::container_lifecycle::ContainerLifecycle for OffRuntimeLifecycle {
    fn route_lost(
        &self,
        id: Uuid,
        exit_code: i32,
    ) -> BoxFuture<'_, Result<crate::pty::container_lifecycle::CriticalAdmissionOutcome<()>, String>>
    {
        let release = self
            .release
            .lock()
            .unwrap()
            .take()
            .expect("one reconciliation");
        Box::pin(async move {
            self.events
                .send((id, Some(exit_code)))
                .expect("record route loss");
            let _ = release.await;
            Ok(crate::pty::container_lifecycle::CriticalAdmissionOutcome::Completed(()))
        })
    }
}

#[tokio::test]
async fn off_runtime_full_and_closed_queues_remove_then_reconcile_without_waiting() {
    for full in [true, false] {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let (mut backend, ticket) = pending_backend(id, root).await;
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        backend.lifecycle_sender = Some(ContainerLifecycleSender::new(Arc::new(
            OffRuntimeLifecycle {
                events: events_tx.clone(),
                release: Mutex::new(Some(release_rx)),
            },
        )));
        backend.set_route_remover(Arc::new(move |removed_id, _| {
            events_tx.send((removed_id, None)).expect("record removal");
            Ok(())
        }));
        let receiver = attach(&backend, id, root, &ticket);
        let receiver = if full { Some(receiver) } else { None };
        let backend = Arc::new(backend);
        let caller_backend = Arc::clone(&backend);
        let (returned_tx, returned_rx) = std::sync::mpsc::channel();
        let caller = std::thread::spawn(move || {
            assert!(tokio::runtime::Handle::try_current().is_err());
            let authority = crate::pty::manager::BackendWriteAuthority::for_backend_test();
            if full {
                for _ in 0..8 {
                    caller_backend
                        .write(&authority, id, b"x")
                        .expect("fill queue");
                }
            }
            let error = caller_backend
                .write(&authority, id, b"x")
                .expect_err("queue failure");
            if full {
                assert!(matches!(error, AppError::PtyError(ref text)
                    if text.contains("outbound queue full")));
            } else {
                assert!(matches!(error, AppError::SessionNotFound(ref text)
                    if text == &id.to_string()));
            }
            returned_tx.send(()).expect("record caller return");
        });

        // Reconciliation cannot complete until release, but the caller must return.
        returned_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("caller returned");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !caller.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(caller.is_finished(), "off-runtime caller did not finish");
        caller.join().expect("off-runtime caller");
        assert_eq!(
            events_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("route removal"),
            (id, None)
        );
        assert_eq!(
            events_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("route loss"),
            (id, Some(1))
        );
        assert!(!backend.has_session(id));
        assert!(events_rx.try_recv().is_err(), "duplicate callback");
        release_tx.send(()).expect("release reconciliation");
        drop(receiver);
    }
}
