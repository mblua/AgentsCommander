use super::*;

#[test]
fn common_blocking_late_completion_retains_authority_without_disclosure() {
    if std::env::var_os(COMMON_BLOCKING_CHILD_ENV).is_none() {
        let temporary_root = std::env::current_dir()
            .expect("common blocking parent current directory")
            .join("target")
            .join("terminal-snapshot-acceptance-temp");
        std::fs::create_dir_all(&temporary_root).expect("common blocking parent temporary root");
        let evidence = tempfile::Builder::new()
            .prefix("common-blocking-evidence-")
            .tempdir_in(temporary_root)
            .expect("common blocking evidence directory");
        let canary_file = evidence.path().join("canaries.json");
        let output = std::process::Command::new(
            std::env::current_exe().expect("common blocking test executable"),
        )
        .args([
            "--exact",
            COMMON_BLOCKING_TEST_NAME,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(COMMON_BLOCKING_CHILD_ENV, "1")
        .env(COMMON_BLOCKING_CANARY_FILE_ENV, &canary_file)
        .output()
        .expect("spawn isolated common blocking test");
        let canaries: Vec<String> = std::fs::read(&canary_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| {
                [
                    SCREEN_SENTINEL,
                    OSC_TITLE_SENTINEL,
                    OSC_HYPERLINK_SENTINEL,
                    OSC_CLIPBOARD_SENTINEL,
                    LATE_BLOCKING_PANIC_SENTINEL,
                    BASE64_PANIC_SENTINEL,
                ]
                .into_iter()
                .map(str::to_string)
                .collect()
            });
        assert_canaries_absent_except(
            &output.stdout,
            &canaries,
            &[],
            "common blocking child stdout",
        );
        assert_canaries_absent_except(
            &output.stderr,
            &canaries,
            &[],
            "common blocking child stderr",
        );
        if !output.status.success() {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            for canary in &canaries {
                stdout = stdout.replace(canary, "<redacted-canary>");
                stderr = stderr.replace(canary, "<redacted-canary>");
            }
            panic!("isolated common blocking test failed; stdout={stdout:?}; stderr={stderr:?}");
        }
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("common blocking current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("common blocking temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("common-blocking-")
        .tempdir_in(temporary_root)
        .expect("common blocking temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("common blocking config directory");
    let _env = ConfigEnvGuard::set(&config);
    std::env::set_var("RUST_LOG", "vt100=trace,agentscommander=trace");
    crate::logging::init_logger();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("common blocking runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
        let target = format!("{PROJECT}:{WORKGROUP}/member-live");
        let late_panic_payload = format!(
            "{LATE_BLOCKING_PANIC_SENTINEL}|{BASE64_PANIC_SENTINEL}|{}|{OSC_TITLE_SENTINEL}",
            fixture.api_coordinator_token.secret
        );
        let mut canaries = vec![
            SCREEN_SENTINEL.to_string(),
            OSC_TITLE_SENTINEL.to_string(),
            OSC_HYPERLINK_SENTINEL.to_string(),
            OSC_CLIPBOARD_SENTINEL.to_string(),
            LATE_BLOCKING_PANIC_SENTINEL.to_string(),
            BASE64_PANIC_SENTINEL.to_string(),
            fixture.api_coordinator_token.secret.clone(),
        ];
        canaries.sort();
        canaries.dedup();
        let canary_file = PathBuf::from(
            std::env::var_os(COMMON_BLOCKING_CANARY_FILE_ENV)
                .expect("common blocking canary file path"),
        );
        std::fs::write(
            canary_file,
            serde_json::to_vec(&canaries).expect("common blocking canary manifest"),
        )
        .expect("write common blocking canary manifest");

        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);
        assert_eq!(fixture.local_backend.mutations(), 0);
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            0
        );

        let capture_id = Uuid::new_v4().to_string();
        timeout_api_at_common_blocking_stage(
            &fixture,
            &config,
            &canaries,
            &target,
            &capture_id,
            TerminalSnapshotFormat::Json,
            TerminalSnapshotBlockingStage::Capture,
            TerminalSnapshotBlockingControl::new(None),
            1,
            true,
            false,
            || {
                fixture
                    .local_backend
                    .fanout
                    .remove_session(fixture.live_member.id)
            },
        )
        .await;
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            1
        );
        fixture
            .local_backend
            .install(fixture.live_member.id, &terminal_canary_output());

        let json_id = Uuid::new_v4().to_string();
        timeout_api_at_common_blocking_stage(
            &fixture,
            &config,
            &canaries,
            &target,
            &json_id,
            TerminalSnapshotFormat::Json,
            TerminalSnapshotBlockingStage::JsonPayload,
            TerminalSnapshotBlockingControl::new(None),
            2,
            false,
            false,
            || {},
        )
        .await;

        let png_id = Uuid::new_v4().to_string();
        timeout_api_at_common_blocking_stage(
            &fixture,
            &config,
            &canaries,
            &target,
            &png_id,
            TerminalSnapshotFormat::Png,
            TerminalSnapshotBlockingStage::PngPayload,
            TerminalSnapshotBlockingControl::new(None),
            3,
            false,
            false,
            || {},
        )
        .await;

        let envelope_id = Uuid::new_v4().to_string();
        timeout_api_at_common_blocking_stage(
            &fixture,
            &config,
            &canaries,
            &target,
            &envelope_id,
            TerminalSnapshotFormat::Png,
            TerminalSnapshotBlockingStage::ApiEnvelope,
            TerminalSnapshotBlockingControl::new(None),
            4,
            false,
            true,
            || {},
        )
        .await;

        let panic_id = Uuid::new_v4().to_string();
        timeout_api_at_common_blocking_stage(
            &fixture,
            &config,
            &canaries,
            &target,
            &panic_id,
            TerminalSnapshotFormat::Json,
            TerminalSnapshotBlockingStage::JsonPayload,
            TerminalSnapshotBlockingControl::new(Some(late_panic_payload)),
            5,
            false,
            false,
            || {},
        )
        .await;

        let success_id = Uuid::new_v4().to_string();
        let success_response = tokio::time::timeout(
            Duration::from_secs(10),
            run_direct_api_request(
                direct_api_state(&fixture),
                fixture.api_coordinator_token.secret.clone(),
                direct_api_body(&success_id, &target),
            ),
        )
        .await
        .expect("common blocking capacity-reuse request did not finish");
        let success_status = success_response.status();
        let success_bytes = axum::body::to_bytes(
            success_response.into_body(),
            terminal_snapshot_renderer::MAX_TRANSPORT_BYTES,
        )
        .await
        .expect("common blocking capacity-reuse response");
        assert_eq!(success_status, StatusCode::OK);
        let success = decode_api_success(
            &success_bytes,
            &success_id,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("strict common blocking capacity-reuse success");
        assert!(payload_has_sentinel(&success.result));
        drop(success);
        drop(success_bytes);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 1);
        assert_api_audit_row(&config, &canaries, 6, "succeeded", None, Some(&success_id));
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            6
        );

        let resource_state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let first_control = TerminalSnapshotBlockingControl::new(None);
        resource_state.install_blocking_control(
            TerminalSnapshotBlockingStage::TestResourceRetention,
            Arc::clone(&first_control),
        );
        let first_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let first_allocation = RetainedMaximumAllocation {
            bytes: vec![0u8; terminal_snapshot_renderer::MAX_RGB_BYTES],
            dropped: Arc::clone(&first_dropped),
        };
        let first_permit = resource_state
            .admit_requester("maximum-requester-a".to_string())
            .expect("first maximum late-work permit");
        first_permit
            .promote_target("maximum-target-a".to_string())
            .expect("first maximum late-work target");
        let first_audit = common_blocking_test_audit(Uuid::new_v4());
        first_audit.accept_payload(terminal_snapshot_renderer::MAX_RGB_BYTES as u64);
        let first_state = Arc::clone(&resource_state);
        let first_task = tokio::spawn(async move {
            let result = run_blocking_with_deadline(
                &first_state,
                TerminalSnapshotBlockingStage::TestResourceRetention,
                std::time::Instant::now() + Duration::from_secs(60),
                &first_permit,
                &first_audit,
                move || {
                    assert_eq!(
                        first_allocation.bytes.len(),
                        terminal_snapshot_renderer::MAX_RGB_BYTES
                    );
                    drop(first_allocation);
                    Ok::<(), TerminalSnapshotReasonCode>(())
                },
            )
            .await;
            first_audit.finalize_failure(TerminalSnapshotReasonCode::SnapshotTimeout);
            drop(first_permit);
            result
        });
        first_control.wait_until_entered();

        let second_control =
            TerminalSnapshotBlockingControl::new(Some(LATE_BLOCKING_PANIC_SENTINEL.to_string()));
        resource_state.install_blocking_control(
            TerminalSnapshotBlockingStage::TestResourceRetention,
            Arc::clone(&second_control),
        );
        let second_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let second_allocation = RetainedMaximumAllocation {
            bytes: vec![0u8; terminal_snapshot_renderer::MAX_RGB_BYTES],
            dropped: Arc::clone(&second_dropped),
        };
        let second_permit = resource_state
            .admit_requester("maximum-requester-b".to_string())
            .expect("second maximum late-work permit");
        second_permit
            .promote_target("maximum-target-b".to_string())
            .expect("second maximum late-work target");
        let second_audit = common_blocking_test_audit(Uuid::new_v4());
        second_audit.accept_payload(terminal_snapshot_renderer::MAX_RGB_BYTES as u64);
        let second_state = Arc::clone(&resource_state);
        let second_task = tokio::spawn(async move {
            let result = run_blocking_with_deadline(
                &second_state,
                TerminalSnapshotBlockingStage::TestResourceRetention,
                std::time::Instant::now() + Duration::from_secs(60),
                &second_permit,
                &second_audit,
                move || {
                    assert_eq!(
                        second_allocation.bytes.len(),
                        terminal_snapshot_renderer::MAX_RGB_BYTES
                    );
                    drop(second_allocation);
                    Ok::<(), TerminalSnapshotReasonCode>(())
                },
            )
            .await;
            second_audit.finalize_failure(TerminalSnapshotReasonCode::SnapshotTimeout);
            drop(second_permit);
            result
        });
        second_control.wait_until_entered();

        assert_eq!(
            api_lifecycle_counts(&resource_state),
            ApiLifecycleCounts {
                ingress_available: SNAPSHOT_INGRESS_LIMIT,
                requester_in_flight: 2,
                target_in_flight: 2,
                global_in_flight: SNAPSHOT_GLOBAL_IN_FLIGHT,
            }
        );
        assert!(!first_dropped.load(Ordering::SeqCst));
        assert!(!second_dropped.load(Ordering::SeqCst));
        assert!(matches!(
            resource_state.admit_requester("maximum-requester-c".to_string()),
            Err(TerminalSnapshotReasonCode::RateLimited)
        ));

        first_control.expire_deadline();
        second_control.expire_deadline();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), first_task)
                .await
                .expect("first maximum controlled timeout")
                .expect("first maximum waiter"),
            Err(TerminalSnapshotReasonCode::SnapshotTimeout)
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), second_task)
                .await
                .expect("second maximum controlled timeout")
                .expect("second maximum waiter"),
            Err(TerminalSnapshotReasonCode::SnapshotTimeout)
        ));
        assert!(!first_dropped.load(Ordering::SeqCst));
        assert!(!second_dropped.load(Ordering::SeqCst));
        assert_eq!(
            api_lifecycle_counts(&resource_state).global_in_flight,
            SNAPSHOT_GLOBAL_IN_FLIGHT
        );

        first_control.release();
        first_control.wait_until_completed();
        assert!(first_dropped.load(Ordering::SeqCst));
        assert!(!second_dropped.load(Ordering::SeqCst));
        assert_eq!(api_lifecycle_counts(&resource_state).global_in_flight, 1);

        second_control.release();
        second_control.wait_until_completed();
        assert!(second_dropped.load(Ordering::SeqCst));
        assert_api_lifecycle_idle(&resource_state);
        assert!(!resource_state.has_blocking_controls());
        let reuse = resource_state
            .admit_requester("maximum-requester-a".to_string())
            .expect("maximum late-work requester capacity reused");
        reuse
            .promote_target("maximum-target-a".to_string())
            .expect("maximum late-work target capacity reused");
        drop(reuse);
        assert_api_lifecycle_idle(&resource_state);

        assert_eq!(fixture.local_backend.mutations(), 0);
        assert_no_api_test_hooks(&fixture.snapshot_state);
        assert_common_blocking_audit_inventory(&config, &canaries);
        assert_cleanup_and_secondary_surfaces(&fixture, &config, &canaries);
        log::logger().flush();
    });
}
