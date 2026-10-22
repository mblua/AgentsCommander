use super::*;

#[test]
fn real_host_and_api_daemon_paths_enforce_secondary_leakage_confinement() {
    if std::env::var_os(LEAKAGE_CHILD_ENV).is_none() {
        let temporary_root = std::env::current_dir()
            .expect("leakage parent current directory")
            .join("target")
            .join("terminal-snapshot-acceptance-temp");
        std::fs::create_dir_all(&temporary_root).expect("leakage parent temporary root");
        let evidence = tempfile::Builder::new()
            .prefix("leakage-evidence-")
            .tempdir_in(temporary_root)
            .expect("leakage evidence directory");
        let canary_file = evidence.path().join("canaries.json");
        let output = std::process::Command::new(
            std::env::current_exe().expect("terminal snapshot leakage test executable"),
        )
        .args([
            "--exact",
            LEAKAGE_TEST_NAME,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(LEAKAGE_CHILD_ENV, "1")
        .env(LEAKAGE_CANARY_FILE_ENV, &canary_file)
        .output()
        .expect("spawn isolated terminal snapshot leakage test");
        let canaries: Vec<String> = std::fs::read(&canary_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| {
                [
                    SCREEN_SENTINEL,
                    OSC_TITLE_SENTINEL,
                    OSC_HYPERLINK_SENTINEL,
                    OSC_CLIPBOARD_SENTINEL,
                    MALFORMED_BODY_SENTINEL,
                    MALFORMED_TARGET_SENTINEL,
                    CALLER_PATH_SENTINEL,
                ]
                .into_iter()
                .map(str::to_string)
                .collect()
            });
        assert_canaries_absent_except(&output.stdout, &canaries, &[], "child stdout");
        assert_canaries_absent_except(&output.stderr, &canaries, &[], "child stderr");
        if !output.status.success() {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            for canary in &canaries {
                stdout = stdout.replace(canary, "<redacted-canary>");
                stderr = stderr.replace(canary, "<redacted-canary>");
            }
            panic!("isolated leakage test failed; stdout={stdout:?}; stderr={stderr:?}");
        }
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("leakage current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("leakage temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("daemon-leakage-")
        .tempdir_in(temporary_root)
        .expect("leakage temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("leakage config directory");
    let _env = ConfigEnvGuard::set(&config);
    std::env::set_var("RUST_LOG", "vt100=trace,agentscommander=trace");
    crate::logging::init_logger();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("leakage runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
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
            .expect("leakage API listener");
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("leakage API client");
        let mut scanner = crate::phone::terminal_snapshot::SnapshotMailboxScanner::default();
        let live_target = format!("{PROJECT}:{WORKGROUP}/member-live");
        let coordinator_fqn = format!("{PROJECT}:{WORKGROUP}/coordinator");
        let worker_fqn = format!("{PROJECT}:{WORKGROUP}/worker");

        let host_denial_request = retag_host_request(
            host_request(&fixture.host_worker, &worker_fqn, &live_target),
            HOST_DENIAL_NONCE,
        );
        let host_success_request = retag_host_request(
            host_request(&fixture.host_coordinator, &coordinator_fqn, &live_target),
            HOST_SUCCESS_NONCE,
        );
        let host_uncorrelated_request = retag_host_request(
            host_request(&fixture.host_coordinator, &coordinator_fqn, &live_target),
            HOST_UNCORRELATED_NONCE,
        );
        let host_final_request = retag_host_request(
            host_request(&fixture.host_coordinator, &coordinator_fqn, &live_target),
            HOST_FINAL_NONCE,
        );

        let mut canaries = vec![
            SCREEN_SENTINEL.to_string(),
            OSC_TITLE_SENTINEL.to_string(),
            OSC_HYPERLINK_SENTINEL.to_string(),
            OSC_CLIPBOARD_SENTINEL.to_string(),
            MALFORMED_BODY_SENTINEL.to_string(),
            MALFORMED_TARGET_SENTINEL.to_string(),
            CALLER_PATH_SENTINEL.to_string(),
            HOST_DENIAL_NONCE.to_string(),
            HOST_SUCCESS_NONCE.to_string(),
            HOST_UNCORRELATED_NONCE.to_string(),
            HOST_FINAL_NONCE.to_string(),
            fixture.host_worker.token.to_string(),
            fixture.host_coordinator.token.to_string(),
            fixture.api_worker_token.secret.clone(),
            fixture.api_coordinator_token.secret.clone(),
            host_denial_request.confirmation_tag.clone(),
            host_success_request.confirmation_tag.clone(),
            host_uncorrelated_request.confirmation_tag.clone(),
            host_final_request.confirmation_tag.clone(),
        ];
        canaries.sort();
        canaries.dedup();
        let canary_file = PathBuf::from(
            std::env::var_os(LEAKAGE_CANARY_FILE_ENV).expect("leakage canary file path"),
        );
        std::fs::write(
            canary_file,
            serde_json::to_vec(&canaries).expect("leakage canary manifest"),
        )
        .expect("write leakage canary manifest");

        let host_denial_wire =
            serde_json::to_vec(&host_denial_request).expect("host denial request wire");
        assert!(contains_raw(
            &host_denial_wire,
            host_denial_request.token.as_bytes()
        ));
        assert!(contains_raw(
            &host_denial_wire,
            host_denial_request.nonce.as_bytes()
        ));
        assert!(contains_raw(
            &host_denial_wire,
            host_denial_request.confirmation_tag.as_bytes()
        ));
        let host_denial_bytes = submit_host_request(
            &fixture,
            &mut scanner,
            &fixture.paths.worker,
            &host_denial_request,
        )
        .await;
        assert_canaries_absent_except(
            &host_denial_bytes,
            &canaries,
            &[host_denial_request.confirmation_tag.as_str()],
            "host authorization denial envelope",
        );
        let host_denial = decode_host_response(
            &host_denial_bytes,
            &host_denial_request.request_id,
            &host_denial_request.confirmation_tag,
            &live_target,
            TerminalSnapshotFormat::Json,
        )
        .expect("host authorization denial response");
        assert_eq!(
            host_denial.error,
            Some(TerminalSnapshotReasonCode::NotAuthorized)
        );
        assert!(host_denial.result.is_none());

        let (_, api_denial_status, api_denial_headers, api_denial_bytes) = post_api_snapshot(
            &client,
            address,
            &fixture.api_worker_token.secret,
            &live_target,
        )
        .await;
        assert_eq!(api_denial_status, StatusCode::FORBIDDEN);
        assert_eq!(
            api_denial_headers[reqwest::header::CACHE_CONTROL],
            "no-store"
        );
        assert_eq!(api_denial_headers[reqwest::header::PRAGMA], "no-cache");
        assert_canaries_absent_except(
            &api_denial_bytes,
            &canaries,
            &[],
            "API authorization denial envelope",
        );
        assert_eq!(
            decode_api_error(&api_denial_bytes, api_denial_status.as_u16())
                .expect("API denial response")
                .error,
            TerminalSnapshotReasonCode::NotAuthorized
        );

        let host_success_wire =
            serde_json::to_vec(&host_success_request).expect("host success request wire");
        assert!(contains_raw(
            &host_success_wire,
            host_success_request.token.as_bytes()
        ));
        assert!(contains_raw(
            &host_success_wire,
            host_success_request.confirmation_tag.as_bytes()
        ));
        let host_success_bytes = submit_host_request(
            &fixture,
            &mut scanner,
            &fixture.paths.coordinator,
            &host_success_request,
        )
        .await;
        assert_canaries_absent_except(
            &host_success_bytes,
            &canaries,
            &[
                SCREEN_SENTINEL,
                host_success_request.confirmation_tag.as_str(),
            ],
            "authorized host response artifact",
        );
        let host_success = decode_host_response(
            &host_success_bytes,
            &host_success_request.request_id,
            &host_success_request.confirmation_tag,
            &live_target,
            TerminalSnapshotFormat::Json,
        )
        .expect("authorized host response");
        assert!(payload_has_sentinel(
            host_success.result.as_ref().expect("host success result")
        ));

        let (api_success_request_id, api_success_status, api_success_headers, api_success_bytes) =
            post_api_snapshot(
                &client,
                address,
                &fixture.api_coordinator_token.secret,
                &live_target,
            )
            .await;
        assert_eq!(api_success_status, StatusCode::OK);
        assert_eq!(
            api_success_headers[reqwest::header::CACHE_CONTROL],
            "no-store"
        );
        assert_eq!(api_success_headers[reqwest::header::PRAGMA], "no-cache");
        assert_canaries_absent_except(
            &api_success_bytes,
            &canaries,
            &[SCREEN_SENTINEL],
            "authorized API response body",
        );
        let api_success = decode_api_success(
            &api_success_bytes,
            &api_success_request_id,
            &live_target,
            TerminalSnapshotFormat::Json,
        )
        .expect("authorized API response");
        assert!(payload_has_sentinel(&api_success.result));

        let malformed_api_request_id = Uuid::new_v4().to_string();
        let malformed_target =
            format!("../../{MALFORMED_TARGET_SENTINEL}/{CALLER_PATH_SENTINEL}/secret.png");
        let malformed_api_bytes = serde_json::to_vec(&serde_json::json!({
            "apiVersion": "1",
            "requestId": malformed_api_request_id,
            "to": malformed_target,
            "format": "json"
        }))
        .expect("malformed API request bytes");
        let (malformed_api_status, malformed_api_headers, malformed_api_response) = post_api_bytes(
            &client,
            address,
            &fixture.api_coordinator_token.secret,
            malformed_api_bytes,
        )
        .await;
        assert_eq!(malformed_api_status, StatusCode::BAD_REQUEST);
        assert_eq!(
            malformed_api_headers[reqwest::header::CACHE_CONTROL],
            "no-store"
        );
        assert_eq!(malformed_api_headers[reqwest::header::PRAGMA], "no-cache");
        assert_canaries_absent_except(
            &malformed_api_response,
            &canaries,
            &[],
            "malformed API error envelope",
        );
        assert_eq!(
            decode_api_error(&malformed_api_response, malformed_api_status.as_u16())
                .expect("malformed API response")
                .error,
            TerminalSnapshotReasonCode::InvalidRequest
        );

        let malformed_host_filename = Uuid::new_v4().to_string();
        let malformed_host_bytes = format!(
            "{{\"kind\":\"terminal-snapshot\",\"requestId\":\"{malformed_host_filename}\",\
                \"token\":\"{MALFORMED_BODY_SENTINEL}\""
        )
        .into_bytes();
        assert!(contains_raw(
            &malformed_host_bytes,
            MALFORMED_BODY_SENTINEL.as_bytes()
        ));
        submit_uncorrelated_host_bytes(
            &fixture,
            &mut scanner,
            &fixture.paths.coordinator,
            &malformed_host_filename,
            &malformed_host_bytes,
        )
        .await;

        let uncorrelated_filename = Uuid::new_v4().to_string();
        assert_ne!(uncorrelated_filename, host_uncorrelated_request.request_id);
        let uncorrelated_bytes = serde_json::to_vec(&host_uncorrelated_request)
            .expect("uncorrelated host request bytes");
        assert!(contains_raw(
            &uncorrelated_bytes,
            host_uncorrelated_request.confirmation_tag.as_bytes()
        ));
        submit_uncorrelated_host_bytes(
            &fixture,
            &mut scanner,
            &fixture.paths.coordinator,
            &uncorrelated_filename,
            &uncorrelated_bytes,
        )
        .await;

        let registry_path = fixture.registry_path.clone();
        let client_id = fixture.api_coordinator_token.client_id.clone();
        fixture
            .snapshot_state
            .install_api_final_handoff_hook(move || {
                assert!(crate::api::auth::revoke(&registry_path, &client_id)
                    .expect("revoke at leakage final API handoff"));
            });
        let (_, api_final_status, api_final_headers, api_final_bytes) = post_api_snapshot(
            &client,
            address,
            &fixture.api_coordinator_token.secret,
            &live_target,
        )
        .await;
        assert_eq!(api_final_status, StatusCode::CONFLICT);
        assert_eq!(
            api_final_headers[reqwest::header::CACHE_CONTROL],
            "no-store"
        );
        assert_eq!(api_final_headers[reqwest::header::PRAGMA], "no-cache");
        assert_canaries_absent_except(
            &api_final_bytes,
            &canaries,
            &[],
            "API final-authority error envelope",
        );
        assert_eq!(
            decode_api_error(&api_final_bytes, api_final_status.as_u16())
                .expect("API final-authority response")
                .error,
            TerminalSnapshotReasonCode::AuthorityChanged
        );

        let settings = fixture.settings.clone();
        let settings_path = fixture.settings_path.clone();
        let collection = fixture.paths.collection.clone();
        fixture
            .snapshot_state
            .install_host_final_handoff_hook(move || {
                settings.blocking_write().terminal_snapshots_enabled = false;
                write_security_settings(&settings_path, &collection, false);
            });
        let host_final_bytes = submit_host_request(
            &fixture,
            &mut scanner,
            &fixture.paths.coordinator,
            &host_final_request,
        )
        .await;
        assert_canaries_absent_except(
            &host_final_bytes,
            &canaries,
            &[host_final_request.confirmation_tag.as_str()],
            "host final-authority error envelope",
        );
        let host_final = decode_host_response(
            &host_final_bytes,
            &host_final_request.request_id,
            &host_final_request.confirmation_tag,
            &live_target,
            TerminalSnapshotFormat::Json,
        )
        .expect("host final-authority response");
        assert_eq!(
            host_final.error,
            Some(TerminalSnapshotReasonCode::AuthorityChanged)
        );
        assert!(host_final.result.is_none());

        assert_eq!(fixture.local_backend.mutations(), 0);
        assert_audit_inventory(&config, &canaries);
        api_shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), start.join_handle)
            .await
            .expect("leakage API shutdown deadline")
            .expect("leakage API server task");
        log::logger().flush();
        let app_log = std::fs::read(config.join("app.log")).expect("application log bytes");
        assert_canaries_absent_except(&app_log, &canaries, &[], "application log");
        assert_cleanup_and_secondary_surfaces(&fixture, &config, &canaries);
    });
}
