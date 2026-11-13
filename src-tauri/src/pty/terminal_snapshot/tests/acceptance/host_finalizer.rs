use super::fixture::{run_isolated_child, IsolatedChild};
use super::*;

fn run_host_finalizer_parent() {
    run_isolated_child(&IsolatedChild {
        parent: "host finalizer",
        evidence_prefix: "host-finalizer-evidence-",
        executable: "host finalizer",
        failure: "host finalizer",
        surface: "host finalizer child",
        test_name: HOST_FINALIZER_TEST_NAME,
        child_env: HOST_FINALIZER_CHILD_ENV,
        canary_file_env: HOST_FINALIZER_CANARY_FILE_ENV,
        fallback_canaries: &[
            SCREEN_SENTINEL,
            OSC_TITLE_SENTINEL,
            OSC_HYPERLINK_SENTINEL,
            OSC_CLIPBOARD_SENTINEL,
            HOST_FINALIZER_PANIC_SENTINEL,
        ],
    });
}

async fn host_finalizer_deadline_case(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    live_target: &str,
) {
    let deadline_case = prepare_host_finalizer(fixture, live_target).await;
    let deadline_request_id = deadline_case.request_id.clone();
    let deadline_selected = deadline_case.selected_session_id;
    let liveness_before = fixture.local_backend.counts(deadline_selected).liveness;
    let deadline_control = TerminalSnapshotHostFinalizerControl::new(
        TerminalSnapshotHostFinalizerStage::RouteVerification,
        None,
    );
    let mut running = start_host_finalizer(deadline_case, Arc::clone(&deadline_control));
    running.wait_until_blocked_and_detach();
    assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
    assert!(!audit_contains_request(
        &fixture.settings_path,
        &deadline_request_id
    ));
    deadline_control.expire_deadline();
    deadline_control.release();
    let (outcome, handoffs) = running.finish().await;
    assert_eq!(
        outcome,
        HostFinalizerWorkerOutcome::Returned(Err(TerminalSnapshotReasonCode::SnapshotTimeout))
    );
    assert_eq!(
        handoffs,
        vec![HostFinalizerHandoff::Failure(
            TerminalSnapshotReasonCode::SnapshotTimeout
        )]
    );
    assert!(fixture.local_backend.counts(deadline_selected).liveness > liveness_before);
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_host_finalizer_audit(
        config,
        canaries,
        1,
        &deadline_request_id,
        "failed",
        Some(TerminalSnapshotReasonCode::SnapshotTimeout),
    );
}

async fn host_finalizer_privacy_case(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    live_target: &str,
) {
    let privacy_case = prepare_host_finalizer(fixture, live_target).await;
    let privacy_request_id = privacy_case.request_id.clone();
    let privacy_control = TerminalSnapshotHostFinalizerControl::new(
        TerminalSnapshotHostFinalizerStage::RevalidationEntry,
        None,
    );
    let mut running = start_host_finalizer(privacy_case, Arc::clone(&privacy_control));
    running.wait_until_blocked_and_detach();
    assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
    assert!(!audit_contains_request(
        &fixture.settings_path,
        &privacy_request_id
    ));
    fixture.settings.write().await.terminal_snapshots_enabled = false;
    write_security_settings(&fixture.settings_path, &fixture.paths.collection, false);
    privacy_control.release();
    let (outcome, handoffs) = running.finish().await;
    assert_eq!(
        outcome,
        HostFinalizerWorkerOutcome::Returned(Err(TerminalSnapshotReasonCode::AuthorityChanged))
    );
    assert_eq!(
        handoffs,
        vec![HostFinalizerHandoff::Failure(
            TerminalSnapshotReasonCode::AuthorityChanged
        )]
    );
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_host_finalizer_audit(
        config,
        canaries,
        2,
        &privacy_request_id,
        "failed",
        Some(TerminalSnapshotReasonCode::AuthorityChanged),
    );
    fixture.settings.write().await.terminal_snapshots_enabled = true;
    write_security_settings(&fixture.settings_path, &fixture.paths.collection, true);
}

async fn host_finalizer_route_case(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    live_target: &str,
) {
    let route_case = prepare_host_finalizer(fixture, live_target).await;
    let route_request_id = route_case.request_id.clone();
    let route_selected = route_case.selected_session_id;
    let route_control = TerminalSnapshotHostFinalizerControl::new(
        TerminalSnapshotHostFinalizerStage::RouteVerification,
        None,
    );
    let mut running = start_host_finalizer(route_case, Arc::clone(&route_control));
    running.wait_until_blocked_and_detach();
    assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
    assert!(!audit_contains_request(
        &fixture.settings_path,
        &route_request_id
    ));
    fixture
        .pty_manager
        .lock()
        .expect("host finalizer PTY manager")
        .remove_route_if_kind(route_selected, SessionBackendKind::LocalProcess);
    route_control.release();
    let (outcome, handoffs) = running.finish().await;
    assert_eq!(
        outcome,
        HostFinalizerWorkerOutcome::Returned(Err(TerminalSnapshotReasonCode::AuthorityChanged))
    );
    assert_eq!(
        handoffs,
        vec![HostFinalizerHandoff::Failure(
            TerminalSnapshotReasonCode::AuthorityChanged
        )]
    );
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_host_finalizer_audit(
        config,
        canaries,
        3,
        &route_request_id,
        "failed",
        Some(TerminalSnapshotReasonCode::AuthorityChanged),
    );
    {
        let manager = fixture
            .pty_manager
            .lock()
            .expect("restore host finalizer PTY route");
        record_route(
            &manager,
            route_selected,
            SessionBackendKind::LocalProcess,
            &fixture.paths.live_member,
        );
    }
}

async fn host_finalizer_session_case(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    live_target: &str,
) -> (SessionManager, Session) {
    let session_case = prepare_host_finalizer(fixture, live_target).await;
    let session_request_id = session_case.request_id.clone();
    let session_selected = session_case.selected_session_id;
    let session_control = TerminalSnapshotHostFinalizerControl::new(
        TerminalSnapshotHostFinalizerStage::RouteVerification,
        None,
    );
    let mut running = start_host_finalizer(session_case, Arc::clone(&session_control));
    running.wait_until_blocked_and_detach();
    assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
    assert!(!audit_contains_request(
        &fixture.settings_path,
        &session_request_id
    ));
    let manager = { fixture.session_manager.read().await.clone() };
    manager.mark_exited(session_selected, 0).await;
    session_control.release();
    let (outcome, handoffs) = running.finish().await;
    assert_eq!(
        outcome,
        HostFinalizerWorkerOutcome::Returned(Err(TerminalSnapshotReasonCode::AuthorityChanged))
    );
    assert_eq!(
        handoffs,
        vec![HostFinalizerHandoff::Failure(
            TerminalSnapshotReasonCode::AuthorityChanged
        )]
    );
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_host_finalizer_audit(
        config,
        canaries,
        4,
        &session_request_id,
        "failed",
        Some(TerminalSnapshotReasonCode::AuthorityChanged),
    );
    let replacement = create_session(
        &manager,
        &fixture.paths.live_member,
        false,
        SessionBackendKind::LocalProcess,
    )
    .await;
    fixture
        .local_backend
        .install(replacement.id, &terminal_canary_output());
    {
        let pty = fixture
            .pty_manager
            .lock()
            .expect("replacement host finalizer PTY route");
        record_route(
            &pty,
            replacement.id,
            SessionBackendKind::LocalProcess,
            &fixture.paths.live_member,
        );
    }
    (manager, replacement)
}

async fn host_finalizer_panic_case(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    live_target: &str,
) {
    let panic_case = prepare_host_finalizer(fixture, live_target).await;
    let panic_request_id = panic_case.request_id.clone();
    let panic_payload = format!(
        "{HOST_FINALIZER_PANIC_SENTINEL}|{SCREEN_SENTINEL}|{}",
        fixture.host_coordinator.token
    );
    let panic_control = TerminalSnapshotHostFinalizerControl::new(
        TerminalSnapshotHostFinalizerStage::RouteVerification,
        Some(panic_payload),
    );
    let mut running = start_host_finalizer(panic_case, Arc::clone(&panic_control));
    running.wait_until_blocked_and_detach();
    assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
    assert!(!audit_contains_request(
        &fixture.settings_path,
        &panic_request_id
    ));
    panic_control.release();
    let (outcome, handoffs) = running.finish().await;
    assert_eq!(outcome, HostFinalizerWorkerOutcome::Panicked);
    assert!(
        handoffs.is_empty(),
        "a panicked finalizer must not hand off a response"
    );
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_host_finalizer_audit(
        config,
        canaries,
        5,
        &panic_request_id,
        "failed",
        Some(TerminalSnapshotReasonCode::Internal),
    );
}

async fn host_finalizer_reuse_case(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    live_target: &str,
) {
    let reuse_case = prepare_host_finalizer(fixture, live_target).await;
    let reuse_request_id = reuse_case.request_id.clone();
    let reuse_control = TerminalSnapshotHostFinalizerControl::new(
        TerminalSnapshotHostFinalizerStage::FinalDeadline,
        None,
    );
    let mut running = start_host_finalizer(reuse_case, Arc::clone(&reuse_control));
    running.wait_until_blocked_and_detach();
    assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
    assert!(!audit_contains_request(
        &fixture.settings_path,
        &reuse_request_id
    ));
    reuse_control.release();
    let (outcome, handoffs) = running.finish().await;
    assert_eq!(outcome, HostFinalizerWorkerOutcome::Returned(Ok(())));
    assert_eq!(handoffs, vec![HostFinalizerHandoff::Success]);
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_host_finalizer_audit(config, canaries, 6, &reuse_request_id, "succeeded", None);
}

#[test]
fn synchronous_host_finalizer_retains_authority_through_late_completion() {
    if std::env::var_os(HOST_FINALIZER_CHILD_ENV).is_none() {
        run_host_finalizer_parent();
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("host finalizer current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("host finalizer temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("host-finalizer-")
        .tempdir_in(temporary_root)
        .expect("host finalizer temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("host finalizer config directory");
    let _env = ConfigEnvGuard::set(&config);
    // SAFETY: runs only in the isolated --test-threads=1 child process
    unsafe {
        std::env::set_var("RUST_LOG", "vt100=trace,agentscommander=trace");
    }
    crate::logging::init_logger();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("host finalizer runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
        let live_target = format!("{PROJECT}:{WORKGROUP}/member-live");
        let mut canaries = vec![
            SCREEN_SENTINEL.to_string(),
            OSC_TITLE_SENTINEL.to_string(),
            OSC_HYPERLINK_SENTINEL.to_string(),
            OSC_CLIPBOARD_SENTINEL.to_string(),
            HOST_FINALIZER_PANIC_SENTINEL.to_string(),
            fixture.host_coordinator.token.to_string(),
        ];
        canaries.sort();
        canaries.dedup();
        let canary_file = PathBuf::from(
            std::env::var_os(HOST_FINALIZER_CANARY_FILE_ENV)
                .expect("host finalizer canary file path"),
        );
        std::fs::write(
            canary_file,
            serde_json::to_vec(&canaries).expect("host finalizer canary manifest"),
        )
        .expect("write host finalizer canary manifest");

        host_finalizer_deadline_case(&fixture, &config, &canaries, &live_target).await;
        host_finalizer_privacy_case(&fixture, &config, &canaries, &live_target).await;
        host_finalizer_route_case(&fixture, &config, &canaries, &live_target).await;
        let (_manager, _replacement) =
            host_finalizer_session_case(&fixture, &config, &canaries, &live_target).await;
        host_finalizer_panic_case(&fixture, &config, &canaries, &live_target).await;
        host_finalizer_reuse_case(&fixture, &config, &canaries, &live_target).await;

        assert_eq!(fixture.local_backend.mutations(), 0);
        assert!(!fixture.snapshot_state.has_blocking_controls());
        log::logger().flush();
        let app_log = std::fs::read(config.join("app.log")).expect("host finalizer app log");
        assert_canaries_absent_except(&app_log, &canaries, &[], "host finalizer app log");
        assert!(contains_raw(
            &app_log,
            b"[terminal-snapshot] stage=host_finalizer_task code=internal"
        ));
        assert_cleanup_and_secondary_surfaces(&fixture, &config, &canaries);
    });
}
