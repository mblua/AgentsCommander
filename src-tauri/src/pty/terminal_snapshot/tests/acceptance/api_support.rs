use super::*;

pub(super) fn direct_api_state(fixture: &AcceptanceFixture) -> crate::api::ApiState {
    crate::api::ApiState {
        window_screenshot_limiter: std::sync::Arc::new(crate::api::WindowScreenshotLimiter::new()),
        store: Arc::new(crate::api::auth::ApiClientStore::new(
            fixture.registry_path.clone(),
        )),
        message_store: Arc::clone(
            fixture
                .message_store
                .as_ref()
                .expect("fixture message store is alive"),
        ),
        lockout: Arc::new(crate::api::auth::FailedAuthLockout::default()),
        app_handle: fixture
            .app
            .as_ref()
            .expect("fixture app is alive")
            .handle()
            .clone(),
        session_mgr: Arc::clone(&fixture.session_manager),
        pty_mgr: Arc::clone(&fixture.pty_manager),
    }
}

fn direct_api_body_for_format(
    request_id: &str,
    target: &str,
    format: TerminalSnapshotFormat,
) -> axum::body::Body {
    axum::body::Body::from(
        serde_json::to_vec(&serde_json::json!({
            "apiVersion": "1",
            "requestId": request_id,
            "to": target,
            "format": format.to_string()
        }))
        .expect("direct API body"),
    )
}

pub(super) fn direct_api_body(request_id: &str, target: &str) -> axum::body::Body {
    direct_api_body_for_format(request_id, target, TerminalSnapshotFormat::Json)
}

pub(super) async fn run_direct_api_request(
    state: crate::api::ApiState,
    token: String,
    body: axum::body::Body,
) -> axum::response::Response {
    let uri: axum::http::Uri = "/api/v1/terminal-snapshot"
        .parse()
        .expect("terminal snapshot URI");
    let request = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri(uri.clone())
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(body)
        .expect("direct terminal snapshot request");
    crate::api::handlers::terminal_snapshot::post(
        axum::extract::State(state),
        axum::extract::ConnectInfo("127.0.0.1:1173".parse().expect("direct API peer address")),
        axum::extract::OriginalUri(uri),
        request,
    )
    .await
}

pub(super) async fn await_api_barrier<T>(
    receiver: tokio::sync::oneshot::Receiver<T>,
    label: &'static str,
) -> T {
    tokio::time::timeout(Duration::from_secs(10), receiver)
        .await
        .expect(label)
        .expect(label)
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ApiAbortPoint {
    BeforeCapture,
    AfterResponseBytes,
    FinalHandoff,
}

pub(super) async fn abort_direct_api_request_at(
    fixture: &AcceptanceFixture,
    point: ApiAbortPoint,
    request_id: &str,
    target: &str,
) -> ApiLifecycleCounts {
    let state = direct_api_state(fixture);
    let token = fixture.api_coordinator_token.secret.clone();
    let body = direct_api_body(request_id, target);
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        start_rx.await.expect("start direct API cancellation");
        run_direct_api_request(state, token, body).await
    });
    let abort = task.abort_handle();
    let state_for_hook = Arc::clone(&fixture.snapshot_state);
    let (arrived_tx, arrived_rx) = tokio::sync::oneshot::channel();
    let hook = move || {
        let counts = api_lifecycle_counts(&state_for_hook);
        let _ = arrived_tx.send(counts);
        abort.abort();
    };
    match point {
        ApiAbortPoint::BeforeCapture => {
            fixture.snapshot_state.install_api_before_capture_hook(hook)
        }
        ApiAbortPoint::AfterResponseBytes => fixture
            .snapshot_state
            .install_api_after_response_bytes_hook(hook),
        ApiAbortPoint::FinalHandoff => fixture.snapshot_state.install_api_final_handoff_hook(hook),
    }
    start_tx.send(()).expect("release direct API start gate");
    let counts = await_api_barrier(arrived_rx, "API cancellation hook was not reached").await;
    let joined = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("aborted API handler did not finish");
    match joined {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("aborted API handler returned a response"),
    }
    counts
}

fn assert_common_blocking_overlap_denial(state: &TerminalSnapshotState) {
    let (requester_key, target_key) = {
        let limiter = state.limiter.lock().expect("active common limiter");
        assert_eq!(limiter.requester_in_flight.len(), 1);
        assert_eq!(limiter.target_in_flight.len(), 1);
        assert_eq!(limiter.global_in_flight, 1);
        (
            limiter
                .requester_in_flight
                .keys()
                .next()
                .expect("active requester key")
                .clone(),
            limiter
                .target_in_flight
                .keys()
                .next()
                .expect("active target key")
                .clone(),
        )
    };

    assert!(matches!(
        state.admit_requester(requester_key),
        Err(TerminalSnapshotReasonCode::RateLimited)
    ));

    let target_probe = state
        .admit_requester(format!("target-overlap-probe:{}", Uuid::new_v4()))
        .expect("second global slot for target overlap probe");
    assert!(matches!(
        target_probe.promote_target(target_key),
        Err(TerminalSnapshotReasonCode::RateLimited)
    ));
    drop(target_probe);

    let global_probe = state
        .admit_requester(format!("global-overlap-probe:{}", Uuid::new_v4()))
        .expect("second global slot for global overlap probe");
    global_probe
        .promote_target(format!("global-overlap-target:{}", Uuid::new_v4()))
        .expect("unique target in second global slot");
    assert!(matches!(
        state.admit_requester(format!("third-global-probe:{}", Uuid::new_v4())),
        Err(TerminalSnapshotReasonCode::RateLimited)
    ));
    drop(global_probe);
    assert_api_lifecycle_active(api_lifecycle_counts(state), true);
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn timeout_api_at_common_blocking_stage(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
    target: &str,
    request_id: &str,
    format: TerminalSnapshotFormat,
    stage: TerminalSnapshotBlockingStage,
    control: Arc<TerminalSnapshotBlockingControl>,
    expected_audit_total: usize,
    prove_overlap_denial: bool,
    expect_retained_payload: bool,
    before_release: impl FnOnce(),
) {
    fixture
        .snapshot_state
        .install_blocking_control(stage, Arc::clone(&control));
    let state = direct_api_state(fixture);
    let token = fixture.api_coordinator_token.secret.clone();
    let body = direct_api_body_for_format(request_id, target, format);
    let task = tokio::spawn(async move { run_direct_api_request(state, token, body).await });

    control.wait_until_entered();
    assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
    if expect_retained_payload {
        assert!(control.retained_payload_bytes() > 0);
    } else {
        assert_eq!(control.retained_payload_bytes(), 0);
    }
    if prove_overlap_denial {
        assert_common_blocking_overlap_denial(&fixture.snapshot_state);
    }

    control.expire_deadline();
    let response = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("controlled common blocking deadline was not observed")
        .expect("controlled common blocking API task");
    let status = response.status();
    let bytes = axum::body::to_bytes(
        response.into_body(),
        terminal_snapshot_renderer::MAX_ERROR_BYTES,
    )
    .await
    .expect("controlled common blocking timeout response");
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(
        decode_api_error(&bytes, status.as_u16())
            .expect("strict controlled timeout response")
            .error,
        TerminalSnapshotReasonCode::SnapshotTimeout
    );
    assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
    assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);
    assert_api_audit_row(
        config,
        canaries,
        expected_audit_total,
        "failed",
        Some(TerminalSnapshotReasonCode::SnapshotTimeout),
        Some(request_id),
    );

    before_release();
    control.release();
    control.wait_until_completed();
    assert_api_lifecycle_idle(&fixture.snapshot_state);
    assert_eq!(fixture.snapshot_state.test_api_success_handoffs(), 0);
    assert_api_audit_row(
        config,
        canaries,
        expected_audit_total,
        "failed",
        Some(TerminalSnapshotReasonCode::SnapshotTimeout),
        Some(request_id),
    );
    assert!(!fixture.snapshot_state.has_blocking_controls());
}

pub(super) struct RetainedMaximumAllocation {
    pub(super) bytes: Vec<u8>,
    pub(super) dropped: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for RetainedMaximumAllocation {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

pub(super) fn common_blocking_test_audit(request_id: Uuid) -> TerminalSnapshotAuditGuard {
    let audit =
        TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
    audit.accept_requester("project:wg-1-dev-team/coordinator");
    audit.accept_request(&TerminalSnapshotServiceRequest {
        request_id,
        target: "project:wg-1-dev-team/member-live".to_string(),
        format: TerminalSnapshotFormat::Png,
        source_plane: TerminalSnapshotSourcePlane::ContainerApi,
        host_authorization_deadline: None,
    });
    audit
}

pub(super) fn assert_common_blocking_audit_inventory(config: &Path, canaries: &[String]) {
    let rows = snapshot_audit_rows(config, canaries);
    assert_eq!(rows.len(), 8);
    let mut request_ids = HashSet::new();
    let mut succeeded = 0usize;
    let mut timed_out = 0usize;
    for row in rows {
        let request_id = row
            .get("requestId")
            .and_then(serde_json::Value::as_str)
            .expect("common blocking audit request id");
        assert!(request_ids.insert(request_id.to_string()));
        assert_eq!(
            row.get("sourcePlane").and_then(serde_json::Value::as_str),
            Some(TerminalSnapshotSourcePlane::ContainerApi.as_str())
        );
        match row.get("status").and_then(serde_json::Value::as_str) {
            Some("succeeded") => {
                succeeded += 1;
                assert!(row.get("reasonCode").is_none());
            }
            Some("failed") => {
                timed_out += 1;
                assert_eq!(
                    row.get("reasonCode").and_then(serde_json::Value::as_str),
                    Some(TerminalSnapshotReasonCode::SnapshotTimeout.as_str())
                );
            }
            status => panic!("unexpected common blocking audit status {status:?}"),
        }
        assert!(row.get("acceptedAt").is_some());
        assert!(row.get("completedAt").is_some());
    }
    assert_eq!(succeeded, 1);
    assert_eq!(timed_out, 7);
}

pub(super) fn assert_api_audit_row(
    config: &Path,
    canaries: &[String],
    expected_total: usize,
    expected_status: &str,
    expected_reason: Option<TerminalSnapshotReasonCode>,
    expected_request_id: Option<&str>,
) {
    let rows = snapshot_audit_rows(config, canaries);
    assert_eq!(rows.len(), expected_total);
    let row = rows.last().expect("latest API cancellation audit row");
    assert_eq!(
        row.get("sourcePlane").and_then(serde_json::Value::as_str),
        Some(TerminalSnapshotSourcePlane::ContainerApi.as_str())
    );
    assert_eq!(
        row.get("status").and_then(serde_json::Value::as_str),
        Some(expected_status)
    );
    assert_eq!(
        row.get("reasonCode").and_then(serde_json::Value::as_str),
        expected_reason.map(TerminalSnapshotReasonCode::as_str)
    );
    assert_eq!(
        row.get("requestId").and_then(serde_json::Value::as_str),
        expected_request_id
    );
    assert!(row.get("acceptedAt").is_some());
    assert!(row.get("completedAt").is_some());
}

pub(super) struct BodyStreamDropProbe(pub(super) Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for BodyStreamDropProbe {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

/// A loopback listener the caller keeps until the API server adopts it (#1768). Returning the
/// socket rather than a bare port number is the whole point: nothing else can take the port
/// between reservation and service.
pub(super) fn reserved_loopback_listener() -> std::net::TcpListener {
    std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback port")
}

pub(super) fn contains_raw(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

pub(super) fn assert_canaries_absent_except(
    bytes: &[u8],
    canaries: &[String],
    allowed: &[&str],
    surface: &str,
) {
    for (index, canary) in canaries.iter().enumerate() {
        if allowed.iter().any(|allowed| *allowed == canary) {
            continue;
        }
        if contains_raw(bytes, canary.as_bytes()) {
            panic!("forbidden canary index {index} reached {surface}");
        }
    }
}

fn collect_regular_files(root: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    let entries = std::fs::read_dir(root).expect("read leakage surface directory");
    for entry in entries {
        let entry = entry.expect("read leakage surface entry");
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).expect("leakage surface metadata");
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_regular_files(&path, files);
        } else if metadata.is_file() {
            files.push((
                path.clone(),
                std::fs::read(&path).expect("read raw leakage surface bytes"),
            ));
        }
    }
}

pub(super) fn snapshot_audit_rows(config: &Path, canaries: &[String]) -> Vec<serde_json::Value> {
    let bytes = std::fs::read(config.join("api-audit.log")).expect("snapshot audit log");
    assert_canaries_absent_except(&bytes, canaries, &[], "api audit raw bytes");
    String::from_utf8(bytes)
        .expect("UTF-8 audit log")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|value| {
            value.get("event").and_then(|event| event.as_str()) == Some("terminal_snapshot")
        })
        .collect()
}

pub(super) fn assert_audit_inventory(config: &Path, canaries: &[String]) {
    let rows = snapshot_audit_rows(config, canaries);
    assert_eq!(
        rows.len(),
        9,
        "one audit row is required per sentinel operation"
    );
    let allowed: HashSet<&str> = [
        "event",
        "requestId",
        "requesterFqn",
        "targetFqn",
        "sourcePlane",
        "format",
        "selectedSessionId",
        "selectedBackend",
        "rows",
        "columns",
        "sequence",
        "capturedAt",
        "payloadBytes",
        "acceptedAt",
        "completedAt",
        "status",
        "reasonCode",
    ]
    .into_iter()
    .collect();
    let mut reasons: HashMap<String, usize> = HashMap::new();
    let mut succeeded = 0usize;
    for row in rows {
        let object = row.as_object().expect("snapshot audit object");
        assert!(object.keys().all(|key| allowed.contains(key.as_str())));
        if object.get("status").and_then(|value| value.as_str()) == Some("succeeded") {
            succeeded += 1;
        }
        if let Some(reason) = object.get("reasonCode").and_then(|value| value.as_str()) {
            *reasons.entry(reason.to_string()).or_default() += 1;
        }
    }
    assert_eq!(succeeded, 2);
    assert_eq!(reasons.get("not_authorized"), Some(&2));
    assert_eq!(reasons.get("invalid_request"), Some(&3));
    assert_eq!(reasons.get("authority_changed"), Some(&2));
}

pub(super) fn assert_panic_audit_inventory(config: &Path, canaries: &[String]) {
    let rows = snapshot_audit_rows(config, canaries);
    assert_eq!(
        rows.len(),
        3,
        "every panic boundary must audit exactly once"
    );
    let mut source_planes: HashMap<String, usize> = HashMap::new();
    for row in rows {
        let object = row.as_object().expect("snapshot panic audit object");
        assert_eq!(
            object.get("status").and_then(|value| value.as_str()),
            Some("failed")
        );
        assert_eq!(
            object.get("reasonCode").and_then(|value| value.as_str()),
            Some("internal")
        );
        if let Some(source_plane) = object.get("sourcePlane").and_then(|value| value.as_str()) {
            *source_planes.entry(source_plane.to_string()).or_default() += 1;
        }
    }
    assert_eq!(
        source_planes.get(TerminalSnapshotSourcePlane::ContainerApi.as_str()),
        Some(&2)
    );
    assert_eq!(
        source_planes.get(TerminalSnapshotSourcePlane::HostCli.as_str()),
        Some(&1)
    );
}

pub(super) fn assert_cleanup_and_secondary_surfaces(
    fixture: &AcceptanceFixture,
    config: &Path,
    canaries: &[String],
) {
    fixture.snapshot_state.sweep_artifacts(true);
    let registry = fixture
        .snapshot_state
        .artifacts
        .lock()
        .expect("artifact registry");
    assert!(registry.files.is_empty());
    assert_eq!(registry.reservations, 0);
    drop(registry);

    for root in [&fixture.paths.coordinator, &fixture.paths.worker] {
        let local = root.join(crate::config::agent_local_dir_name());
        for directory in [
            local.join("outbox").join("terminal-snapshot-requests"),
            local.join("terminal-snapshot-responses"),
        ] {
            assert_eq!(
                std::fs::read_dir(directory)
                    .expect("protocol cleanup directory")
                    .count(),
                0,
                "request, processing, response, and temporary files must be gone"
            );
        }
    }

    let mut files = Vec::new();
    collect_regular_files(fixture._temporary.path(), &mut files);
    assert!(!files.is_empty());
    let mut sqlite_files = 0usize;
    let mut settings_files = 0usize;
    for (path, bytes) in files {
        let surface = path.to_string_lossy();
        assert_canaries_absent_except(&bytes, canaries, &[], &surface);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.starts_with("api-messages.sqlite3") {
            sqlite_files += 1;
        }
        if name == "settings.30.instance.no-git.json" {
            settings_files += 1;
        }
        assert!(!name.contains("terminal-snapshot-processing"));
        assert!(!name.contains("terminal-snapshot-response-tmp"));
        assert!(!name.contains("terminal-snapshot-request-tmp"));
        assert!(!name.contains("terminal-snapshot-cancelled"));
    }
    assert!(
        sqlite_files >= 1,
        "SQLite persistence surface was not inspected"
    );
    assert!(
        settings_files >= 1,
        "serialized settings surface was not inspected"
    );
    assert!(config.join("app.log").is_file());
}

pub(super) fn payload_has_sentinel(payload: &TerminalSnapshotPayload) -> bool {
    match payload {
        TerminalSnapshotPayload::Json { snapshot } => snapshot.screen.lines.iter().any(|line| {
            line.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .contains(SCREEN_SENTINEL)
        }),
        TerminalSnapshotPayload::Png { .. } => false,
    }
}
