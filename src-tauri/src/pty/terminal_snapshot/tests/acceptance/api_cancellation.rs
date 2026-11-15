use super::fixture::{
    assert_capacity_reuse_request, run_isolated_child, CapacityReuseMessages, IsolatedChild,
};
use super::*;

#[test]
fn api_async_cancellation_reclaims_authority_without_disclosure() {
    if std::env::var_os(API_CANCELLATION_CHILD_ENV).is_none() {
        run_api_cancellation_parent();
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
    // SAFETY: runs only in the isolated --test-threads=1 child process
    unsafe {
        std::env::set_var("RUST_LOG", "vt100=trace,agentscommander=trace");
    }
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

        assert_disconnected_body_rejected(&fixture, &config, &canaries).await;
        assert_pending_body_drop_reclaims(&fixture, &config, &canaries).await;
        assert_abort_points_reclaim(&fixture, &config, &canaries, &target).await;
        assert_capacity_reuse_success(&fixture, &config, &canaries, &target).await;
        log::logger().flush();
    });
}

fn run_api_cancellation_parent() {
    run_isolated_child(&IsolatedChild {
        parent: "API cancellation",
        evidence_prefix: "api-cancellation-evidence-",
        executable: "API cancellation",
        failure: "API cancellation",
        surface: "cancellation child",
        test_name: API_CANCELLATION_TEST_NAME,
        child_env: API_CANCELLATION_CHILD_ENV,
        canary_file_env: API_CANCELLATION_CANARY_FILE_ENV,
        fallback_canaries: &[
            SCREEN_SENTINEL,
            OSC_TITLE_SENTINEL,
            OSC_HYPERLINK_SENTINEL,
            OSC_CLIPBOARD_SENTINEL,
            BODY_DISCONNECT_SENTINEL,
        ],
    });
}

async fn assert_disconnected_body_rejected(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
) {
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
            direct_api_state(fixture),
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
        config,
        canaries,
        1,
        "rejected",
        Some(TerminalSnapshotReasonCode::InvalidRequest),
        None,
    );
    assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);
}

async fn assert_pending_body_drop_reclaims(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
) {
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
        direct_api_state(fixture),
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
        config,
        canaries,
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
}

async fn assert_abort_points_reclaim(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    target: &str,
) {
    let before_capture_id = Uuid::new_v4().to_string();
    let before_capture_counts = abort_direct_api_request_at(
        fixture,
        ApiAbortPoint::BeforeCapture,
        &before_capture_id,
        target,
    )
    .await;
    assert_api_lifecycle_active(before_capture_counts, true);
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_api_audit_row(
        config,
        canaries,
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
        fixture,
        ApiAbortPoint::AfterResponseBytes,
        &after_bytes_id,
        target,
    )
    .await;
    assert_api_lifecycle_active(after_bytes_counts, true);
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_api_audit_row(
        config,
        canaries,
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
        fixture,
        ApiAbortPoint::FinalHandoff,
        &final_handoff_id,
        target,
    )
    .await;
    assert_api_lifecycle_active(final_handoff_counts, true);
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_api_audit_row(
        config,
        canaries,
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
}

async fn assert_capacity_reuse_success(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    target: &str,
) {
    assert_capacity_reuse_request(
        fixture,
        config,
        canaries,
        target,
        &CapacityReuseMessages {
            timeout: "capacity-reuse API request did not finish",
            response: "capacity-reuse API response",
            decode: "strict capacity-reuse API success",
        },
        3,
    )
    .await;
    assert_eq!(fixture.local_backend.mutations(), 0);
    assert_no_api_test_hooks(&fixture.snapshot_state);
    assert_cleanup_and_secondary_surfaces(fixture, config, canaries);
}
