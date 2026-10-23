use super::*;

#[test]
fn api_async_cancellation_reclaims_authority_without_disclosure() {
    if std::env::var_os(API_CANCELLATION_CHILD_ENV).is_none() {
        let temporary_root = std::env::current_dir()
            .expect("API cancellation parent current directory")
            .join("target")
            .join("terminal-snapshot-acceptance-temp");
        std::fs::create_dir_all(&temporary_root).expect("API cancellation parent temporary root");
        let evidence = tempfile::Builder::new()
            .prefix("api-cancellation-evidence-")
            .tempdir_in(temporary_root)
            .expect("API cancellation evidence directory");
        let canary_file = evidence.path().join("canaries.json");
        let output = std::process::Command::new(
            std::env::current_exe().expect("API cancellation test executable"),
        )
        .args([
            "--exact",
            API_CANCELLATION_TEST_NAME,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(API_CANCELLATION_CHILD_ENV, "1")
        .env(API_CANCELLATION_CANARY_FILE_ENV, &canary_file)
        .output()
        .expect("spawn isolated API cancellation test");
        let canaries: Vec<String> = std::fs::read(&canary_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| {
                [
                    SCREEN_SENTINEL,
                    OSC_TITLE_SENTINEL,
                    OSC_HYPERLINK_SENTINEL,
                    OSC_CLIPBOARD_SENTINEL,
                    BODY_DISCONNECT_SENTINEL,
                ]
                .into_iter()
                .map(str::to_string)
                .collect()
            });
        assert_canaries_absent_except(&output.stdout, &canaries, &[], "cancellation child stdout");
        assert_canaries_absent_except(&output.stderr, &canaries, &[], "cancellation child stderr");
        if !output.status.success() {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            for canary in &canaries {
                stdout = stdout.replace(canary, "<redacted-canary>");
                stderr = stderr.replace(canary, "<redacted-canary>");
            }
            panic!("isolated API cancellation test failed; stdout={stdout:?}; stderr={stderr:?}");
        }
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("API cancellation current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("API cancellation temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("api-cancellation-")
        .tempdir_in(temporary_root)
        .expect("API cancellation temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("API cancellation config directory");
    let _env = ConfigEnvGuard::set(&config);
    std::env::set_var("RUST_LOG", "vt100=trace,agentscommander=trace");
    crate::logging::init_logger();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("API cancellation runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
        let target = format!("{PROJECT}:{WORKGROUP}/member-live");
        let mut canaries = vec![
            SCREEN_SENTINEL.to_string(),
            OSC_TITLE_SENTINEL.to_string(),
            OSC_HYPERLINK_SENTINEL.to_string(),
            OSC_CLIPBOARD_SENTINEL.to_string(),
            BODY_DISCONNECT_SENTINEL.to_string(),
            fixture.api_coordinator_token.secret.clone(),
        ];
        canaries.sort();
        canaries.dedup();
        let canary_file = PathBuf::from(
            std::env::var_os(API_CANCELLATION_CANARY_FILE_ENV)
                .expect("API cancellation canary file path"),
        );
        std::fs::write(
            canary_file,
            serde_json::to_vec(&canaries).expect("API cancellation canary manifest"),
        )
        .expect("write API cancellation canary manifest");

        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);
        assert_eq!(fixture.local_backend.mutations(), 0);
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            0
        );

        let state_for_disconnect = Arc::clone(&fixture.snapshot_state);
        let (disconnect_polled_tx, disconnect_polled_rx) = tokio::sync::oneshot::channel();
        let disconnect_stream = futures_util::stream::once(async move {
            let _ = disconnect_polled_tx.send(api_lifecycle_counts(&state_for_disconnect));
            Err::<axum::body::Bytes, _>(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                BODY_DISCONNECT_SENTINEL,
            ))
        });
        let disconnect_response = tokio::time::timeout(
            Duration::from_secs(10),
            run_direct_api_request(
                direct_api_state(&fixture),
                fixture.api_coordinator_token.secret.clone(),
                axum::body::Body::from_stream(disconnect_stream),
            ),
        )
        .await
        .expect("disconnected API body handler did not finish");
        assert_api_lifecycle_active(
            await_api_barrier(
                disconnect_polled_rx,
                "authenticated disconnected body was not polled",
            )
            .await,
            false,
        );
        let disconnect_status = disconnect_response.status();
        let disconnect_bytes = axum::body::to_bytes(
            disconnect_response.into_body(),
            terminal_snapshot_renderer::MAX_ERROR_BYTES,
        )
        .await
        .expect("fixed disconnected-body response");
        assert_eq!(disconnect_status, StatusCode::BAD_REQUEST);
        assert_eq!(
            decode_api_error(&disconnect_bytes, disconnect_status.as_u16())
                .expect("strict disconnected-body error")
                .error,
            TerminalSnapshotReasonCode::InvalidRequest
        );
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_api_audit_row(
            &config,
            &canaries,
            1,
            "rejected",
            Some(TerminalSnapshotReasonCode::InvalidRequest),
            None,
        );
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);

        let state_for_pending_body = Arc::clone(&fixture.snapshot_state);
        let (pending_polled_tx, pending_polled_rx) = tokio::sync::oneshot::channel();
        let (pending_dropped_tx, pending_dropped_rx) = tokio::sync::oneshot::channel();
        let mut pending_polled_tx = Some(pending_polled_tx);
        let pending_drop_probe = BodyStreamDropProbe(Some(pending_dropped_tx));
        let pending_stream = futures_util::stream::poll_fn(move |_context| {
            let _retain_probe = &pending_drop_probe;
            if let Some(sender) = pending_polled_tx.take() {
                let _ = sender.send(api_lifecycle_counts(&state_for_pending_body));
            }
            std::task::Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
        });
        let mut pending_handler = Box::pin(run_direct_api_request(
            direct_api_state(&fixture),
            fixture.api_coordinator_token.secret.clone(),
            axum::body::Body::from_stream(pending_stream),
        ));
        let pending_counts = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::select! {
                counts = pending_polled_rx => counts.expect("pending body poll counts"),
                response = &mut pending_handler => {
                    panic!("pending body returned status {}", response.status())
                }
            }
        })
        .await
        .expect("pending authenticated body was not polled");
        assert_api_lifecycle_active(pending_counts, false);
        drop(pending_handler);
        await_api_barrier(
            pending_dropped_rx,
            "dropped API future retained its pending body stream",
        )
        .await;
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_api_audit_row(
            &config,
            &canaries,
            2,
            "failed",
            Some(TerminalSnapshotReasonCode::Internal),
            None,
        );
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            0
        );

        let before_capture_id = Uuid::new_v4().to_string();
        let before_capture_counts = abort_direct_api_request_at(
            &fixture,
            ApiAbortPoint::BeforeCapture,
            &before_capture_id,
            &target,
        )
        .await;
        assert_api_lifecycle_active(before_capture_counts, true);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_api_audit_row(
            &config,
            &canaries,
            3,
            "failed",
            Some(TerminalSnapshotReasonCode::Internal),
            Some(&before_capture_id),
        );
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            0
        );
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);

        let after_bytes_id = Uuid::new_v4().to_string();
        let after_bytes_counts = abort_direct_api_request_at(
            &fixture,
            ApiAbortPoint::AfterResponseBytes,
            &after_bytes_id,
            &target,
        )
        .await;
        assert_api_lifecycle_active(after_bytes_counts, true);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_api_audit_row(
            &config,
            &canaries,
            4,
            "failed",
            Some(TerminalSnapshotReasonCode::Internal),
            Some(&after_bytes_id),
        );
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            1
        );
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);

        let final_handoff_id = Uuid::new_v4().to_string();
        let final_handoff_counts = abort_direct_api_request_at(
            &fixture,
            ApiAbortPoint::FinalHandoff,
            &final_handoff_id,
            &target,
        )
        .await;
        assert_api_lifecycle_active(final_handoff_counts, true);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_api_audit_row(
            &config,
            &canaries,
            5,
            "failed",
            Some(TerminalSnapshotReasonCode::Internal),
            Some(&final_handoff_id),
        );
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            2
        );
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);

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
        .expect("capacity-reuse API request did not finish");
        let success_status = success_response.status();
        let success_bytes = axum::body::to_bytes(
            success_response.into_body(),
            terminal_snapshot_renderer::MAX_TRANSPORT_BYTES,
        )
        .await
        .expect("capacity-reuse API response");
        assert_eq!(success_status, StatusCode::OK);
        let success = decode_api_success(
            &success_bytes,
            &success_id,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("strict capacity-reuse API success");
        assert!(payload_has_sentinel(&success.result));
        drop(success);
        drop(success_bytes);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 1);
        assert_api_audit_row(&config, &canaries, 6, "succeeded", None, Some(&success_id));
        assert_eq!(
            fixture.local_backend.counts(fixture.live_member.id).copies,
            3
        );
        assert_eq!(fixture.local_backend.mutations(), 0);
        assert_no_api_test_hooks(&fixture.snapshot_state);
        assert_cleanup_and_secondary_surfaces(&fixture, &config, &canaries);
        log::logger().flush();
    });
}
