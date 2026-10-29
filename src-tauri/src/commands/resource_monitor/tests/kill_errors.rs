use super::*;

#[tokio::test]
async fn unsupported_backend_manual_kill_is_side_effect_free() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "/tmp/unsupported-resource-kill".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let pty_backend = Arc::new(ResourcePtyBackend::default());
    pty_backend.set_live(session.id);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let process_backend = Arc::new(UnsupportedKillBackend::default());
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    let before_selection = manager.read().await.selection_payload().await;

    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![kill_resource_group])
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(Arc::clone(&monitor))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build unsupported resource-kill app");
    assert!(app.try_state::<SelectionCoordinator>().is_none());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let public_response = tauri::test::get_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: "kill_resource_group".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                "request": {
                    "sessionId": session.id,
                    "reason": "user",
                }
            })),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .expect("unsupported public kill returns success")
    .deserialize::<ResourceKillResult>()
    .unwrap();
    assert_unsupported_kill_result(&public_response, session.id);

    let transaction = SelectionTransaction::for_test(app.handle().clone());
    let inner_response = execute_resource_kill_transaction(
        &transaction,
        session.id,
        TrustedResourceIntent::Watchdog,
    )
    .await
    .unwrap();
    assert_unsupported_kill_result(&inner_response, session.id);

    assert_eq!(process_backend.observe_tree_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        process_backend
            .observe_identity_calls
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        process_backend
            .terminate_verified_calls
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        process_backend
            .current_process_memory_calls
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 0);
    assert_eq!(pty_backend.kill_count.load(Ordering::SeqCst), 0);
    assert!(pty.lock().unwrap().has_session(session.id));
    assert!(!monitor.has_registered_group(session.id));
    assert_eq!(monitor.active_agent_groups(), 0);
    let after_selection = manager.read().await.selection_payload().await;
    assert_eq!(after_selection.revision(), before_selection.revision());
    assert_eq!(after_selection.id(), before_selection.id());
    assert_eq!(
        manager
            .read()
            .await
            .get_session(session.id)
            .await
            .unwrap()
            .status,
        SessionStatus::Active
    );
}

/// Build the 4-state mock app (manager, PtyManager, ResourceMonitorState,
/// DetachedSessionsState) with a Group-neutral GatedBackend (supports process
/// tree enforcement = true) but NO group registration, so `kill_group` reports
/// Terminated immediately with no coordinator/reaper barrier. Used by the S1
/// defer-arm test without any coordinator.
async fn run_kill_error_branch(
    fail_kill: bool,
    keep_pty_live: bool,
) -> (
    ResourceKillResult,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Uuid,
    Arc<ResourcePtyBackend>,
    Arc<Mutex<PtyManager>>,
) {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/kill-error-defer".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session");
    let pty_backend = Arc::new(ResourcePtyBackend::default());
    pty_backend.set_live(session.id);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let root = ProcessIdentity {
        pid: 5400,
        creation_time_100ns: 910,
    };
    let process_backend = Arc::new(GatedBackend::new(root));
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(Arc::clone(&monitor))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build kill-error app");
    pty_backend.set_fail_kill(fail_kill);
    if !keep_pty_live {
        // Simulate the PTY route already gone while `kill` still errors.
        pty_backend.live.lock().unwrap().remove(&session.id);
    }
    let transaction = SelectionTransaction::for_test(app.handle().clone());
    let result = execute_resource_kill_transaction(
        &transaction,
        session.id,
        TrustedResourceIntent::Watchdog,
    )
    .await
    .expect("kill transaction");
    (result, manager, session.id, pty_backend, pty)
}

/// Test 18 (replaces test 17): the S1 kill-error path. Round-3 finding-1(b) replaces
/// indefinite deferral with a GUARANTEED bounded teardown, so with a still-live PTY
/// whose `kill` keeps erroring the transaction escalates to a best-effort force-close
/// and finalizes within bounded time (finalized true, row Exited, route dropped). When
/// the PTY is already gone, the same `kill` error falls through and finalizes as today.
/// Both branches call `execute_resource_kill_transaction` DIRECTLY (no coordinator)
/// under a hard timeout.
#[tokio::test]
async fn resource_monitor_kill_error_defers_exited_while_pty_live() {
    let (result, manager, session_id, _pty_backend, pty) =
        tokio::time::timeout(Duration::from_secs(10), run_kill_error_branch(true, true))
            .await
            .expect("branch 1 (escalate) did not complete in time");
    assert!(
        result.finalized,
        "bounded teardown finalizes even while the PTY route stays live (escalation)"
    );
    let row = manager
        .read()
        .await
        .get_session(session_id)
        .await
        .expect("row present after finalize");
    assert!(
        matches!(row.status, SessionStatus::Exited(_)),
        "row Exited after escalation finalize"
    );
    assert!(
        !pty.lock().unwrap().has_session(session_id),
        "escalation force-close dropped the PTY route"
    );

    let (result, manager, session_id, _pty_backend, _pty) =
        tokio::time::timeout(Duration::from_secs(10), run_kill_error_branch(true, false))
            .await
            .expect("branch 2 (finalize) did not complete in time");
    assert!(
        result.finalized,
        "finalize proceeds when the PTY route is already gone"
    );
    let row = manager
        .read()
        .await
        .get_session(session_id)
        .await
        .expect("row present after finalize");
    assert!(
        matches!(row.status, SessionStatus::Exited(_)),
        "row Exited after the finalize branch"
    );
}

/// Builds the 4-state mock app for the round-3 bounded-deferred-teardown tests
/// (tests 19-20): a GatedBackend with no group registration so `kill_group` reports
/// Terminated immediately, and a backend-setup closure so each test configures how the
/// PTY `kill` fails. Returns the result, manager, session id, backend, and pty manager.
async fn run_bounded_teardown_branch(
    backend_setup: impl FnOnce(Arc<ResourcePtyBackend>),
) -> (
    ResourceKillResult,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Uuid,
    Arc<ResourcePtyBackend>,
    Arc<Mutex<PtyManager>>,
    usize,
) {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/bounded-teardown".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session");
    let pty_backend = Arc::new(ResourcePtyBackend::default());
    pty_backend.set_live(session.id);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let process_backend = Arc::new(GatedBackend::new(ProcessIdentity {
        pid: 5600,
        creation_time_100ns: 1200,
    }));
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    backend_setup(Arc::clone(&pty_backend));
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(Arc::clone(&monitor))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build bounded teardown app");
    let transaction = SelectionTransaction::for_test(app.handle().clone());
    let retry_failures = AtomicUsize::new(0);
    let result = execute_resource_kill_transaction_inner(
        &transaction,
        session.id,
        TrustedResourceIntent::Watchdog,
        &retry_failures,
    )
    .await
    .expect("kill transaction");
    (
        result,
        manager,
        session.id,
        pty_backend,
        pty,
        retry_failures.load(Ordering::SeqCst),
    )
}

/// Test 19 (round-3): the bounded deferred teardown retries the still-live PTY route
/// and finalizes once the route clears on the last attempt. The mock `kill` fails the
/// initial verified-kill call and the first N-1 retry attempts (so exactly N-1 teardown
/// WARNs), then succeeds on the Nth attempt: the route clears, the row flips to Exited,
/// and `result.finalized` is true. Deterministic and bounded.
#[tokio::test]
async fn s1_defer_retries_pty_teardown_then_finalizes_when_route_clears() {
    let (result, manager, session_id, pty_backend, pty, retry_failures) = tokio::time::timeout(
        Duration::from_secs(10),
        run_bounded_teardown_branch(|backend| {
            backend.set_fail_kill(false);
            backend.set_fail_kill_remaining(PTY_ROUTE_RETRY_ATTEMPTS as usize);
        }),
    )
    .await
    .expect("bounded teardown (route clears) did not complete in time");
    assert!(
        result.finalized,
        "finalized after the route clears on the last retry attempt"
    );
    let row = manager
        .read()
        .await
        .get_session(session_id)
        .await
        .expect("row present after finalize");
    assert!(
        matches!(row.status, SessionStatus::Exited(_)),
        "row Exited after the bounded teardown finalize"
    );
    assert!(
        !pty.lock().unwrap().has_session(session_id),
        "PTY route cleared on the final retry attempt"
    );
    assert_eq!(
        retry_failures,
        (PTY_ROUTE_RETRY_ATTEMPTS - 1) as usize,
        "exactly N-1 teardown WARNs emitted before the route cleared"
    );
    assert_eq!(
        pty_backend.kill_fail_count.load(Ordering::SeqCst),
        PTY_ROUTE_RETRY_ATTEMPTS as usize,
        "the initial kill plus N-1 retries failed before the route cleared"
    );
}

/// Test 20 (round-3): when the PTY route never clears, the bounded deferred teardown
/// escalates after exactly N failed attempts to a best-effort force-close of the route
/// and still runs the finalize, so the tile is released (row Exited, finalized true)
/// instead of stranding in Running forever. No infinite retry.
#[tokio::test]
async fn s1_defer_escalates_force_close_after_n_failures() {
    let (result, manager, session_id, _pty_backend, pty, retry_failures) = tokio::time::timeout(
        Duration::from_secs(10),
        run_bounded_teardown_branch(|backend| {
            backend.set_fail_kill(true);
        }),
    )
    .await
    .expect("bounded teardown (escalate) did not complete in time");
    assert!(
        result.finalized,
        "escalating force-close releases the tile via the Exited flip"
    );
    let row = manager
        .read()
        .await
        .get_session(session_id)
        .await
        .expect("row present after escalation");
    assert!(
        matches!(row.status, SessionStatus::Exited(_)),
        "row Exited after escalation force-close"
    );
    assert_eq!(
        retry_failures,
        PTY_ROUTE_RETRY_ATTEMPTS as usize,
        "exactly N failed retry attempts before escalation"
    );
    assert!(
        !pty.lock().unwrap().has_session(session_id),
        "best-effort force-close dropped the PTY route on escalation"
    );
}

#[test]
fn finalizes_only_on_verified_terminated() {
    // Verified dead -> tear down + mark Exited.
    assert!(should_finalize_kill(ResourceGroupState::Terminated));
    // Quarantined (possibly alive) -> keep instance/job, never mark Exited.
    assert!(!should_finalize_kill(ResourceGroupState::Quarantined));
    // A concurrent kill still in flight (early return) -> let it own the outcome,
    // do NOT finalize here (the bug grinch HIGH-1 would otherwise reintroduce).
    assert!(!should_finalize_kill(ResourceGroupState::Terminating));
    assert!(!should_finalize_kill(ResourceGroupState::Running));
}
