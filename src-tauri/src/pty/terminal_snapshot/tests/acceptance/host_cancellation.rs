use super::*;

#[test]
fn host_timeout_cancellation_claims_only_the_unaccepted_request() {
    if std::env::var_os(HOST_CANCELLATION_CHILD_ENV).is_none() {
        let temporary_root = std::env::current_dir()
            .expect("host cancellation parent current directory")
            .join("target")
            .join("terminal-snapshot-acceptance-temp");
        std::fs::create_dir_all(&temporary_root).expect("host cancellation parent temporary root");
        let evidence = tempfile::Builder::new()
            .prefix("host-cancellation-evidence-")
            .tempdir_in(temporary_root)
            .expect("host cancellation evidence directory");
        let canary_file = evidence.path().join("canaries.json");
        let output = std::process::Command::new(
            std::env::current_exe().expect("host cancellation test executable"),
        )
        .args([
            "--exact",
            HOST_CANCELLATION_TEST_NAME,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(HOST_CANCELLATION_CHILD_ENV, "1")
        .env(HOST_CANCELLATION_CANARY_FILE_ENV, &canary_file)
        .output()
        .expect("spawn isolated host cancellation test");
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
                ]
                .into_iter()
                .map(str::to_string)
                .collect()
            });
        assert_canaries_absent_except(
            &output.stdout,
            &canaries,
            &[],
            "host cancellation child stdout",
        );
        assert_canaries_absent_except(
            &output.stderr,
            &canaries,
            &[],
            "host cancellation child stderr",
        );
        if !output.status.success() {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            for canary in &canaries {
                stdout = stdout.replace(canary, "<redacted-canary>");
                stderr = stderr.replace(canary, "<redacted-canary>");
            }
            panic!("isolated host cancellation test failed; stdout={stdout:?}; stderr={stderr:?}");
        }
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("host cancellation current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("host cancellation temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("host-cancellation-")
        .tempdir_in(temporary_root)
        .expect("host cancellation temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("host cancellation config directory");
    let _env = ConfigEnvGuard::set(&config);
    std::env::set_var("RUST_LOG", "vt100=trace,agentscommander=trace");
    crate::logging::init_logger();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("host cancellation runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
        let target = format!("{PROJECT}:{WORKGROUP}/member-live");
        let requester = format!("{PROJECT}:{WORKGROUP}/coordinator");
        let make_request = |nonce: char| {
            let issued = chrono::Utc::now();
            let mut request = host_request(&fixture.host_coordinator, &requester, &target);
            request.issued_at = terminal_snapshot_renderer::canonical_timestamp(issued);
            request.expires_at = terminal_snapshot_renderer::canonical_timestamp(
                issued + chrono::Duration::seconds(30),
            );
            request.nonce = nonce.to_string().repeat(64);
            request.confirmation_tag = crate::phone::terminal_snapshot::confirmation_tag(&request);
            request
        };
        let cancelled_before_claim = make_request('1');
        let disappeared_before_claim = make_request('2');
        let daemon_won = make_request('3');
        let expired_after_bytes = make_request('4');
        let capacity_reuse = make_request('5');
        let requests = [
            &cancelled_before_claim,
            &disappeared_before_claim,
            &daemon_won,
            &expired_after_bytes,
            &capacity_reuse,
        ];
        let mut canaries = vec![
            SCREEN_SENTINEL.to_string(),
            OSC_TITLE_SENTINEL.to_string(),
            OSC_HYPERLINK_SENTINEL.to_string(),
            OSC_CLIPBOARD_SENTINEL.to_string(),
            CALLER_PATH_SENTINEL.to_string(),
            fixture.host_coordinator.token.to_string(),
        ];
        for request in requests {
            canaries.push(request.nonce.clone());
            canaries.push(request.confirmation_tag.clone());
        }
        canaries.sort();
        canaries.dedup();
        let canary_file = PathBuf::from(
            std::env::var_os(HOST_CANCELLATION_CANARY_FILE_ENV)
                .expect("host cancellation canary file path"),
        );
        std::fs::write(
            canary_file,
            serde_json::to_vec(&canaries).expect("host cancellation canary manifest"),
        )
        .expect("write host cancellation canary manifest");

        let root = &fixture.paths.coordinator;
        let local = root.join(crate::config::agent_local_dir_name());
        let request_directory = local.join("outbox").join("terminal-snapshot-requests");
        let response_directory = local.join("terminal-snapshot-responses");
        let caller_output = root
            .join(CALLER_PATH_SENTINEL)
            .join("cancelled-snapshot.png");
        let mut scanner = crate::phone::terminal_snapshot::SnapshotMailboxScanner::default();
        assert_api_lifecycle_idle(&fixture.snapshot_state);

        let cancelled_bytes =
            serde_json::to_vec(&cancelled_before_claim).expect("pre-claim cancellation request");
        let (cancelled_path, cancelled_response) =
            write_host_request_bytes(root, &cancelled_before_claim.request_id, &cancelled_bytes);
        let cancelled_identity = crate::path_identity::verify_regular_file(&cancelled_path)
            .expect("pre-claim cancellation identity");
        let cancelled_id = Uuid::parse_str(&cancelled_before_claim.request_id)
            .expect("pre-claim cancellation UUID");
        let cancelled_marker =
            host_cancellation_marker(&request_directory, &cancelled_before_claim);
        assert!(crate::cli::terminal_snapshot::cancel_request_for_test(
            &cancelled_path,
            &request_directory,
            cancelled_id,
            &cancelled_before_claim.nonce,
            &cancelled_identity,
        ));
        assert!(!cancelled_path.exists());
        assert!(!cancelled_marker.exists());
        scanner.begin_cycle();
        scanner.scan_root(
            fixture.app.as_ref().expect("fixture app is alive").handle(),
            root,
        );
        scanner.finish_cycle();
        scanner.join_pending_tasks_for_test().await;
        assert!(!cancelled_response.exists());
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &cancelled_before_claim.request_id
        ));
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert!(!caller_output.exists());

        let disappeared_bytes =
            serde_json::to_vec(&disappeared_before_claim).expect("pre-claim disappeared request");
        let (disappeared_path, disappeared_response) = write_host_request_bytes(
            root,
            &disappeared_before_claim.request_id,
            &disappeared_bytes,
        );
        let disappeared_identity = crate::path_identity::verify_regular_file(&disappeared_path)
            .expect("pre-claim disappeared identity");
        let current = crate::path_identity::verify_regular_file(&disappeared_path)
            .expect("pre-claim current disappeared identity");
        assert!(crate::path_identity::same_object(
            &disappeared_identity,
            &current
        ));
        std::fs::remove_file(&disappeared_path).expect("remove request before claim");
        scanner.begin_cycle();
        scanner.scan_root(
            fixture.app.as_ref().expect("fixture app is alive").handle(),
            root,
        );
        scanner.finish_cycle();
        scanner.join_pending_tasks_for_test().await;
        assert!(!disappeared_response.exists());
        assert!(!host_cancellation_marker(&request_directory, &disappeared_before_claim).exists());
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &disappeared_before_claim.request_id
        ));
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert!(!caller_output.exists());

        let (processing_entered, processing_release) = install_host_cancellation_barrier(
            &fixture.snapshot_state,
            TerminalSnapshotHostCancellationStage::Processing,
        );
        let daemon_won_bytes = serde_json::to_vec(&daemon_won).expect("daemon-won request");
        let (daemon_won_path, daemon_won_response) =
            write_host_request_bytes(root, &daemon_won.request_id, &daemon_won_bytes);
        let daemon_won_identity = crate::path_identity::verify_regular_file(&daemon_won_path)
            .expect("daemon-won request identity");
        scanner.begin_cycle();
        scanner.scan_root(
            fixture.app.as_ref().expect("fixture app is alive").handle(),
            root,
        );
        scanner.finish_cycle();
        wait_for_host_cancellation_barrier(&processing_entered);
        assert!(!daemon_won_path.exists());
        assert!(!daemon_won_response.exists());
        let daemon_won_marker = host_cancellation_marker(&request_directory, &daemon_won);
        assert!(!daemon_won_marker.exists());
        let processing = std::fs::read_dir(&request_directory)
            .expect("daemon-won processing directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(&format!(".{}.", daemon_won.request_id))
                            && name.ends_with(".terminal-snapshot-processing")
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(processing.len(), 1);
        let processing_identity = crate::path_identity::verify_regular_file(&processing[0])
            .expect("daemon-won processing identity");
        assert!(crate::path_identity::same_object(
            &daemon_won_identity,
            &processing_identity
        ));
        assert!(!crate::cli::terminal_snapshot::cancel_request_for_test(
            &daemon_won_path,
            &request_directory,
            Uuid::parse_str(&daemon_won.request_id).expect("daemon-won UUID"),
            &daemon_won.nonce,
            &daemon_won_identity,
        ));
        assert_eq!(
            api_lifecycle_counts(&fixture.snapshot_state),
            ApiLifecycleCounts {
                ingress_available: SNAPSHOT_INGRESS_LIMIT - 1,
                requester_in_flight: 0,
                target_in_flight: 0,
                global_in_flight: 0,
            }
        );
        processing_release
            .send(())
            .expect("release daemon-won processing");
        scanner.join_pending_tasks_for_test().await;
        let daemon_won_response = consume_host_response(&fixture, root, &daemon_won);
        let daemon_won_decoded = decode_host_response(
            &daemon_won_response,
            &daemon_won.request_id,
            &daemon_won.confirmation_tag,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("daemon-won success response");
        assert!(payload_has_sentinel(
            daemon_won_decoded
                .result
                .as_ref()
                .expect("daemon-won success payload")
        ));
        assert_host_finalizer_audit(
            &config,
            &canaries,
            1,
            &daemon_won.request_id,
            "succeeded",
            None,
        );
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert!(!caller_output.exists());

        let finalizer_control = TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::FinalDeadline,
            None,
        );
        fixture
            .snapshot_state
            .install_next_host_finalizer_control(Arc::clone(&finalizer_control));
        let (bytes_entered, bytes_release) = install_host_cancellation_barrier(
            &fixture.snapshot_state,
            TerminalSnapshotHostCancellationStage::ResponseBytesReady,
        );
        let (publish_entered, publish_release) = install_host_cancellation_barrier(
            &fixture.snapshot_state,
            TerminalSnapshotHostCancellationStage::BeforePublish,
        );
        let expired_bytes =
            serde_json::to_vec(&expired_after_bytes).expect("post-bytes timeout request");
        let (expired_path, expired_response) =
            write_host_request_bytes(root, &expired_after_bytes.request_id, &expired_bytes);
        let expired_identity = crate::path_identity::verify_regular_file(&expired_path)
            .expect("post-bytes timeout identity");
        scanner.begin_cycle();
        scanner.scan_root(
            fixture.app.as_ref().expect("fixture app is alive").handle(),
            root,
        );
        scanner.finish_cycle();
        wait_for_host_cancellation_barrier(&bytes_entered);
        assert!(!expired_path.exists());
        assert!(!expired_response.exists());
        assert_eq!(
            std::fs::read_dir(&request_directory)
                .expect("post-bytes request directory")
                .count(),
            0
        );
        let expired_marker = host_cancellation_marker(&request_directory, &expired_after_bytes);
        assert!(!expired_marker.exists());
        assert!(!crate::cli::terminal_snapshot::cancel_request_for_test(
            &expired_path,
            &request_directory,
            Uuid::parse_str(&expired_after_bytes.request_id).expect("post-bytes timeout UUID"),
            &expired_after_bytes.nonce,
            &expired_identity,
        ));
        assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &expired_after_bytes.request_id
        ));
        finalizer_control.expire_deadline();
        bytes_release
            .send(())
            .expect("release complete host response bytes");

        wait_for_host_cancellation_barrier(&publish_entered);
        assert!(!expired_response.exists());
        assert!(!expired_marker.exists());
        assert!(!crate::cli::terminal_snapshot::cancel_request_for_test(
            &expired_path,
            &request_directory,
            Uuid::parse_str(&expired_after_bytes.request_id).expect("pre-publish timeout UUID"),
            &expired_after_bytes.nonce,
            &expired_identity,
        ));
        assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &expired_after_bytes.request_id
        ));
        publish_release
            .send(())
            .expect("release timed-out host publication");
        scanner.join_pending_tasks_for_test().await;
        let expired_response = consume_host_response(&fixture, root, &expired_after_bytes);
        assert!(!contains_raw(&expired_response, SCREEN_SENTINEL.as_bytes()));
        let expired_decoded = decode_host_response(
            &expired_response,
            &expired_after_bytes.request_id,
            &expired_after_bytes.confirmation_tag,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("post-bytes timeout response");
        assert_eq!(
            expired_decoded.error,
            Some(TerminalSnapshotReasonCode::SnapshotTimeout)
        );
        assert!(expired_decoded.result.is_none());
        assert_host_finalizer_audit(
            &config,
            &canaries,
            2,
            &expired_after_bytes.request_id,
            "failed",
            Some(TerminalSnapshotReasonCode::SnapshotTimeout),
        );
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert!(!caller_output.exists());

        let reuse_bytes = submit_host_request(&fixture, &mut scanner, root, &capacity_reuse).await;
        let reuse = decode_host_response(
            &reuse_bytes,
            &capacity_reuse.request_id,
            &capacity_reuse.confirmation_tag,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("host cancellation capacity reuse response");
        assert!(payload_has_sentinel(
            reuse.result.as_ref().expect("capacity reuse payload")
        ));
        assert_host_finalizer_audit(
            &config,
            &canaries,
            3,
            &capacity_reuse.request_id,
            "succeeded",
            None,
        );
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert!(!fixture.snapshot_state.has_blocking_controls());
        assert!(!caller_output.exists());
        assert_eq!(
            std::fs::read_dir(&request_directory)
                .expect("final host cancellation request inventory")
                .count(),
            0
        );
        assert_eq!(
            std::fs::read_dir(&response_directory)
                .expect("final host cancellation response inventory")
                .count(),
            0
        );
        assert_eq!(fixture.local_backend.mutations(), 0);
        log::logger().flush();
        let app_log =
            std::fs::read(config.join("app.log")).expect("host cancellation application log");
        assert_canaries_absent_except(&app_log, &canaries, &[], "host cancellation app log");
        assert_cleanup_and_secondary_surfaces(&fixture, &config, &canaries);
    });
}
