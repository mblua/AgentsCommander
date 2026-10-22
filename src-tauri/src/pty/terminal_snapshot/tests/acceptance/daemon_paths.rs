use super::*;

#[test]
fn real_host_and_api_daemon_paths_enforce_no_oracle_and_final_handoff() {
    if std::env::var_os(ACCEPTANCE_CHILD_ENV).is_none() {
        let status = std::process::Command::new(
            std::env::current_exe().expect("terminal snapshot acceptance test executable"),
        )
        .args([
            "--exact",
            ACCEPTANCE_TEST_NAME,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(ACCEPTANCE_CHILD_ENV, "1")
        .status()
        .expect("spawn isolated terminal snapshot acceptance test");
        assert!(status.success(), "isolated acceptance test failed");
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("acceptance current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("acceptance temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("daemon-e2e-")
        .tempdir_in(temporary_root)
        .expect("acceptance temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("acceptance config directory");
    let _env = ConfigEnvGuard::set(&config);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("acceptance runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
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
            .expect("snapshot API listener");
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("snapshot API client");
        let mut scanner = crate::phone::terminal_snapshot::SnapshotMailboxScanner::default();

        fixture.snapshot_state.reset_test_target_lookup_counts();
        let targets = [
            format!("{PROJECT}:{WORKGROUP}/member-live"),
            format!("{PROJECT}:{WORKGROUP}/member-exited"),
            format!("{PROJECT}:{WORKGROUP}/member-missing"),
            format!("{PROJECT}:{WORKGROUP}/member-tampered"),
        ];
        let mut api_error_body = None;
        for target in &targets {
            let (_, status, _, bytes) =
                post_api_snapshot(&client, address, &fixture.api_worker_token.secret, target).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            let error = decode_api_error(&bytes, status.as_u16()).expect("fixed API error");
            assert_eq!(error.error, TerminalSnapshotReasonCode::NotAuthorized);
            if let Some(expected) = api_error_body.as_ref() {
                assert_eq!(&bytes, expected, "unauthorized API targets must be uniform");
            } else {
                api_error_body = Some(bytes);
            }

            let request = host_request(
                &fixture.host_worker,
                &format!("{PROJECT}:{WORKGROUP}/worker"),
                target,
            );
            let bytes =
                submit_host_request(&fixture, &mut scanner, &fixture.paths.worker, &request).await;
            let response = decode_host_response(
                &bytes,
                &request.request_id,
                &request.confirmation_tag,
                target,
                TerminalSnapshotFormat::Json,
            )
            .expect("fixed host error");
            assert_eq!(
                response.error,
                Some(TerminalSnapshotReasonCode::NotAuthorized)
            );
            assert!(response.result.is_none());
            assert!(!bytes
                .windows(SCREEN_SENTINEL.len())
                .any(|window| { window == SCREEN_SENTINEL.as_bytes() }));
        }
        assert_eq!(
            fixture.snapshot_state.test_target_lookup_counts(),
            TerminalSnapshotTestLookupCounts {
                target_session_lookups: 0,
                target_route_lookups: 0,
            }
        );
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id),
            BackendLookupCounts::default()
        );

        let live_target = format!("{PROJECT}:{WORKGROUP}/member-live");
        let host_success_request = host_request(
            &fixture.host_coordinator,
            &format!("{PROJECT}:{WORKGROUP}/coordinator"),
            &live_target,
        );
        let host_success_bytes = submit_host_request(
            &fixture,
            &mut scanner,
            &fixture.paths.coordinator,
            &host_success_request,
        )
        .await;
        let host_success = decode_host_response(
            &host_success_bytes,
            &host_success_request.request_id,
            &host_success_request.confirmation_tag,
            &live_target,
            TerminalSnapshotFormat::Json,
        )
        .expect("host daemon success");
        let host_payload = host_success.result.as_ref().expect("host success payload");
        assert!(payload_has_sentinel(host_payload));
        assert_eq!(host_success.error, None);

        let (api_request_id, api_status, api_headers, api_success_bytes) = post_api_snapshot(
            &client,
            address,
            &fixture.api_coordinator_token.secret,
            &live_target,
        )
        .await;
        assert_eq!(api_status, StatusCode::OK);
        assert_eq!(api_headers[reqwest::header::CACHE_CONTROL], "no-store");
        assert_eq!(api_headers[reqwest::header::PRAGMA], "no-cache");
        let api_success = decode_api_success(
            &api_success_bytes,
            &api_request_id,
            &live_target,
            TerminalSnapshotFormat::Json,
        )
        .expect("API daemon success");
        assert!(payload_has_sentinel(&api_success.result));
        assert!(fixture.local_backend.counts(fixture.live_member.id).copies >= 2);
        assert_eq!(fixture.local_backend.mutations(), 0);

        let registry_path = fixture.registry_path.clone();
        let client_id = fixture.api_coordinator_token.client_id.clone();
        fixture
            .snapshot_state
            .install_api_final_handoff_hook(move || {
                assert!(crate::api::auth::revoke(&registry_path, &client_id)
                    .expect("revoke at API final handoff"));
            });
        let (_, api_race_status, _, api_race_bytes) = post_api_snapshot(
            &client,
            address,
            &fixture.api_coordinator_token.secret,
            &live_target,
        )
        .await;
        assert_eq!(api_race_status, StatusCode::CONFLICT);
        let api_race = decode_api_error(&api_race_bytes, api_race_status.as_u16())
            .expect("API final-race response");
        assert_eq!(api_race.error, TerminalSnapshotReasonCode::AuthorityChanged);
        assert!(!api_race_bytes
            .windows(SCREEN_SENTINEL.len())
            .any(|window| { window == SCREEN_SENTINEL.as_bytes() }));

        let settings = fixture.settings.clone();
        let settings_path = fixture.settings_path.clone();
        let collection = fixture.paths.collection.clone();
        fixture
            .snapshot_state
            .install_host_final_handoff_hook(move || {
                settings.blocking_write().terminal_snapshots_enabled = false;
                write_security_settings(&settings_path, &collection, false);
            });
        let host_race_request = host_request(
            &fixture.host_coordinator,
            &format!("{PROJECT}:{WORKGROUP}/coordinator"),
            &live_target,
        );
        let host_race_bytes = submit_host_request(
            &fixture,
            &mut scanner,
            &fixture.paths.coordinator,
            &host_race_request,
        )
        .await;
        let host_race = decode_host_response(
            &host_race_bytes,
            &host_race_request.request_id,
            &host_race_request.confirmation_tag,
            &live_target,
            TerminalSnapshotFormat::Json,
        )
        .expect("host final-race response");
        assert_eq!(
            host_race.error,
            Some(TerminalSnapshotReasonCode::AuthorityChanged)
        );
        assert!(host_race.result.is_none());
        assert!(!host_race_bytes
            .windows(SCREEN_SENTINEL.len())
            .any(|window| { window == SCREEN_SENTINEL.as_bytes() }));

        api_shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), start.join_handle)
            .await
            .expect("API shutdown deadline")
            .expect("API server task");
    });
}
