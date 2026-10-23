use super::*;

#[test]
fn snapshot_production_panic_boundaries_are_payload_free() {
    if std::env::var_os(PANIC_CHILD_ENV).is_none() {
        let temporary_root = std::env::current_dir()
            .expect("panic parent current directory")
            .join("target")
            .join("terminal-snapshot-acceptance-temp");
        std::fs::create_dir_all(&temporary_root).expect("panic parent temporary root");
        let evidence = tempfile::Builder::new()
            .prefix("panic-evidence-")
            .tempdir_in(temporary_root)
            .expect("panic evidence directory");
        let canary_file = evidence.path().join("canaries.json");
        let output = std::process::Command::new(
            std::env::current_exe().expect("terminal snapshot panic test executable"),
        )
        .args([
            "--exact",
            PANIC_TEST_NAME,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(PANIC_CHILD_ENV, "1")
        .env(PANIC_CANARY_FILE_ENV, &canary_file)
        .output()
        .expect("spawn isolated terminal snapshot panic test");
        let canaries: Vec<String> = std::fs::read(&canary_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| {
                [
                    SCREEN_SENTINEL,
                    OSC_TITLE_SENTINEL,
                    OSC_HYPERLINK_SENTINEL,
                    OSC_CLIPBOARD_SENTINEL,
                    CALLER_PATH_SENTINEL,
                    API_PANIC_SENTINEL,
                    BLOCKING_PANIC_SENTINEL,
                    HOST_PANIC_SENTINEL,
                    PNG_PANIC_SENTINEL,
                    BASE64_PANIC_SENTINEL,
                ]
                .into_iter()
                .map(str::to_string)
                .collect()
            });
        assert_canaries_absent_except(&output.stdout, &canaries, &[], "panic child stdout");
        assert_canaries_absent_except(&output.stderr, &canaries, &[], "panic child stderr");
        if !output.status.success() {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            for canary in &canaries {
                stdout = stdout.replace(canary, "<redacted-canary>");
                stderr = stderr.replace(canary, "<redacted-canary>");
            }
            panic!("isolated panic test failed; stdout={stdout:?}; stderr={stderr:?}");
        }
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("panic current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("panic temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("daemon-panic-")
        .tempdir_in(temporary_root)
        .expect("panic temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("panic config directory");
    let _env = ConfigEnvGuard::set(&config);
    std::env::set_var("RUST_LOG", "vt100=trace,agentscommander=trace");
    crate::logging::init_logger();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("panic runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
        let live_target = format!("{PROJECT}:{WORKGROUP}/member-live");
        let coordinator_fqn = format!("{PROJECT}:{WORKGROUP}/coordinator");
        let host_panic_request = retag_host_request(
            host_request(&fixture.host_coordinator, &coordinator_fqn, &live_target),
            HOST_PANIC_NONCE,
        );
        let mut canaries = vec![
            SCREEN_SENTINEL.to_string(),
            OSC_TITLE_SENTINEL.to_string(),
            OSC_HYPERLINK_SENTINEL.to_string(),
            OSC_CLIPBOARD_SENTINEL.to_string(),
            CALLER_PATH_SENTINEL.to_string(),
            API_PANIC_SENTINEL.to_string(),
            BLOCKING_PANIC_SENTINEL.to_string(),
            HOST_PANIC_SENTINEL.to_string(),
            PNG_PANIC_SENTINEL.to_string(),
            BASE64_PANIC_SENTINEL.to_string(),
            HOST_PANIC_NONCE.to_string(),
            fixture.host_coordinator.token.to_string(),
            fixture.api_coordinator_token.secret.clone(),
            host_panic_request.confirmation_tag.clone(),
        ];
        canaries.sort();
        canaries.dedup();
        let canary_file =
            PathBuf::from(std::env::var_os(PANIC_CANARY_FILE_ENV).expect("panic canary file path"));
        std::fs::write(
            canary_file,
            serde_json::to_vec(&canaries).expect("panic canary manifest"),
        )
        .expect("write panic canary manifest");
        log::trace!(target: "vt100", "{OSC_TITLE_SENTINEL}");
        log::trace!(target: "vt100::parser", "{OSC_CLIPBOARD_SENTINEL}");

        let api_shutdown = tokio_util::sync::CancellationToken::new();
        let start = crate::api::start_server_on_listener(
            reserved_loopback_listener(),
            fixture
                .app
                .as_ref()
                .expect("fixture app is alive")
                .handle()
                .clone(),
            Arc::clone(&fixture.session_manager),
            Arc::clone(&fixture.pty_manager),
            api_shutdown.clone(),
        );
        let address = crate::api::wait_for_startup_ready(start.readiness)
            .await
            .expect("panic API listener");
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("panic API client");
        let mut scanner = crate::phone::terminal_snapshot::SnapshotMailboxScanner::default();

        let api_panic_payload = format!(
            "{API_PANIC_SENTINEL}|{PNG_PANIC_SENTINEL}|{BASE64_PANIC_SENTINEL}|\
                {}|{CALLER_PATH_SENTINEL}|{SCREEN_SENTINEL}",
            fixture.api_coordinator_token.secret
        );
        fixture
            .snapshot_state
            .install_api_final_handoff_hook(move || std::panic::panic_any(api_panic_payload));
        let (_, api_status, api_headers, api_bytes) = post_api_snapshot(
            &client,
            address,
            &fixture.api_coordinator_token.secret,
            &live_target,
        )
        .await;
        assert_eq!(api_status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(api_headers[reqwest::header::CACHE_CONTROL], "no-store");
        assert_eq!(api_headers[reqwest::header::PRAGMA], "no-cache");
        assert_canaries_absent_except(&api_bytes, &canaries, &[], "API panic error envelope");
        assert_eq!(
            decode_api_error(&api_bytes, api_status.as_u16())
                .expect("API panic response")
                .error,
            TerminalSnapshotReasonCode::Internal
        );

        let blocking_audit =
            TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
        let blocking_permit = fixture
            .snapshot_state
            .admit_requester("panic-boundary-requester".to_string())
            .expect("panic boundary permit");
        let blocking_panic_payload = format!(
            "{BLOCKING_PANIC_SENTINEL}|{PNG_PANIC_SENTINEL}|{BASE64_PANIC_SENTINEL}|\
                {}|{CALLER_PATH_SENTINEL}|{OSC_TITLE_SENTINEL}",
            fixture.host_coordinator.token
        );
        let blocking_result: Result<(), TerminalSnapshotReasonCode> = run_blocking_with_deadline(
            &fixture.snapshot_state,
            TerminalSnapshotBlockingStage::TestResourceRetention,
            std::time::Instant::now() + Duration::from_secs(5),
            &blocking_permit,
            &blocking_audit,
            move || std::panic::panic_any(blocking_panic_payload),
        )
        .await;
        assert_eq!(blocking_result, Err(TerminalSnapshotReasonCode::Internal));
        blocking_audit.finalize_failure(TerminalSnapshotReasonCode::Internal);
        drop(blocking_permit);

        let host_panic_payload = format!(
            "{HOST_PANIC_SENTINEL}|{PNG_PANIC_SENTINEL}|{BASE64_PANIC_SENTINEL}|\
                {}|{CALLER_PATH_SENTINEL}|{SCREEN_SENTINEL}",
            fixture.host_coordinator.token
        );
        fixture
            .snapshot_state
            .install_host_final_handoff_hook(move || std::panic::panic_any(host_panic_payload));
        submit_host_request_expect_no_response(
            &fixture,
            &mut scanner,
            &fixture.paths.coordinator,
            &host_panic_request,
        )
        .await;

        assert_eq!(fixture.local_backend.mutations(), 0);
        assert_panic_audit_inventory(&config, &canaries);
        api_shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), start.join_handle)
            .await
            .expect("panic API shutdown deadline")
            .expect("panic API server task");
        log::logger().flush();
        let app_log = std::fs::read(config.join("app.log")).expect("panic application log");
        assert_canaries_absent_except(&app_log, &canaries, &[], "panic application log");
        for diagnostic in [
            "[terminal-snapshot] stage=api_task code=internal",
            "[terminal-snapshot] stage=blocking_task code=internal",
            "[terminal-snapshot] stage=host_finalizer_task code=internal",
        ] {
            assert!(contains_raw(&app_log, diagnostic.as_bytes()));
        }
        assert_cleanup_and_secondary_surfaces(&fixture, &config, &canaries);
    });
}
