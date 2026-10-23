use super::*;

pub(super) async fn create_session(
    manager: &SessionManager,
    root: &Path,
    coordinator: bool,
    backend: SessionBackendKind,
) -> Session {
    manager
        .create_session(
            "shell".to_string(),
            Vec::new(),
            root.to_string_lossy().to_string(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            coordinator,
            backend,
        )
        .await
        .expect("fixture session")
}

const FIXTURE_MINT_ATTEMPTS: u32 = 6;
const FIXTURE_MINT_RETRY_DELAY: Duration = Duration::from_millis(25);

/// Mint a fixture API token, absorbing a transient registry write failure.
///
/// `auth::write_registry` carries no retry of its own, so on Windows every mint
/// after the first replaces an existing registry through `ReplaceFileW` and a
/// single transient failure there is terminal. That primitive is pre-existing
/// and out of scope here, but treating it as infallible made the fixture panic
/// during construction, before any snapshot request was issued. A fixture that
/// cannot construct itself is not evidence of anything, so retry a small fixed
/// number of times and name the underlying error if every attempt fails.
pub(super) async fn mint_fixture_token(
    token_manager: &crate::pty::container_tokens::ContainerApiTokenManager,
    session_id: Uuid,
    bound_root: &str,
    label: &str,
) -> crate::pty::container_tokens::ContainerApiToken {
    let mut last_error = String::new();
    for attempt in 1..=FIXTURE_MINT_ATTEMPTS {
        match token_manager.mint_for_session(session_id, bound_root) {
            Ok(token) => return token,
            Err(error) => {
                last_error = error.to_string();
                if attempt < FIXTURE_MINT_ATTEMPTS {
                    tokio::time::sleep(FIXTURE_MINT_RETRY_DELAY * attempt).await;
                }
            }
        }
    }
    panic!("{label} API token failed {FIXTURE_MINT_ATTEMPTS} attempts: {last_error}");
}

pub(super) fn binding_for(
    token: &crate::pty::container_tokens::ContainerApiToken,
    root: &crate::path_identity::VerifiedPathIdentity,
) -> crate::pty::container_backend::ContainerCredentialBinding {
    crate::pty::container_backend::ContainerCredentialBinding {
        client_id: token.client_id.clone(),
        credential_generation: token.credential_generation.clone(),
        bound_session_id: token.bound_session_id.clone(),
        bound_root_object_id: root.object_id,
        credential_token_hash: token.token_hash.clone(),
    }
}

pub(super) fn record_route(manager: &PtyManager, id: Uuid, kind: SessionBackendKind, root: &Path) {
    let identity = crate::path_identity::verify_directory(root).expect("route identity");
    manager
        .record_route_with_identities(id, kind, Some(identity.clone()), Some(identity))
        .expect("fixture route");
}

pub(super) fn write_security_settings(path: &Path, collection: &Path, enabled: bool) {
    std::fs::write(
        path,
        serde_json::to_vec(&serde_json::json!({
            "terminalSnapshotsEnabled": enabled,
            "projectPaths": [collection.to_string_lossy().to_string()]
        }))
        .expect("settings JSON"),
    )
    .expect("write strict security settings");
}

pub(super) fn create_mailbox_directories(root: &Path) {
    let local = root.join(crate::config::agent_local_dir_name());
    for directory in [
        local.join("outbox").join("terminal-snapshot-requests"),
        local.join("terminal-snapshot-responses"),
    ] {
        std::fs::create_dir_all(&directory).expect("snapshot mailbox directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .expect("private snapshot mailbox directory");
        }
    }
}

pub(super) fn terminal_canary_output() -> Vec<u8> {
    format!(
        "\u{1b}]0;{OSC_TITLE_SENTINEL}\u{7}\
            \u{1b}]8;;https://snapshot.invalid/{OSC_HYPERLINK_SENTINEL}\
            \u{1b}\\\u{1b}]8;;\u{1b}\\\u{1b}]52;c;{OSC_CLIPBOARD_SENTINEL}\u{7}{SCREEN_SENTINEL}"
    )
    .into_bytes()
}

pub(super) fn host_request(
    session: &Session,
    from: &str,
    target: &str,
) -> crate::phone::terminal_snapshot::HostTerminalSnapshotRequest {
    let issued = chrono::Utc::now();
    let mut request = crate::phone::terminal_snapshot::HostTerminalSnapshotRequest {
        kind: "terminal-snapshot".to_string(),
        version: 1,
        request_id: Uuid::new_v4().to_string(),
        token: session.token.to_string(),
        from: from.to_string(),
        to: target.to_string(),
        format: TerminalSnapshotFormat::Json,
        issued_at: terminal_snapshot_renderer::canonical_timestamp(issued),
        expires_at: terminal_snapshot_renderer::canonical_timestamp(
            issued + chrono::Duration::seconds(15),
        ),
        nonce: "a".repeat(64),
        confirmation_tag: String::new(),
    };
    request.confirmation_tag = crate::phone::terminal_snapshot::confirmation_tag(&request);
    request
}

pub(super) fn retag_host_request(
    mut request: crate::phone::terminal_snapshot::HostTerminalSnapshotRequest,
    nonce: &str,
) -> crate::phone::terminal_snapshot::HostTerminalSnapshotRequest {
    request.nonce = nonce.to_string();
    request.confirmation_tag = crate::phone::terminal_snapshot::confirmation_tag(&request);
    request
}

pub(super) async fn submit_host_request(
    fixture: &AcceptanceFixture,
    scanner: &mut crate::phone::terminal_snapshot::SnapshotMailboxScanner,
    root: &Path,
    request: &crate::phone::terminal_snapshot::HostTerminalSnapshotRequest,
) -> Vec<u8> {
    let local = root.join(crate::config::agent_local_dir_name());
    let request_directory = local.join("outbox").join("terminal-snapshot-requests");
    let response_directory = local.join("terminal-snapshot-responses");
    let request_path = request_directory.join(format!("{}.json", request.request_id));
    std::fs::write(
        &request_path,
        serde_json::to_vec(request).expect("host request JSON"),
    )
    .expect("host request file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&request_path, std::fs::Permissions::from_mode(0o600))
            .expect("private host request");
    }
    let root_identity =
        crate::path_identity::verify_directory(root).expect("host root preflight identity");
    let local_identity =
        crate::path_identity::verify_directory(&local).expect("host local preflight identity");
    let request_directory_identity = crate::path_identity::verify_directory(&request_directory)
        .expect("host request directory preflight identity");
    let response_directory_identity = crate::path_identity::verify_directory(&response_directory)
        .expect("host response directory preflight identity");
    assert!(crate::path_identity::is_verified_descendant(
        &local_identity,
        &root_identity
    ));
    assert!(crate::path_identity::is_verified_descendant(
        &request_directory_identity,
        &local_identity
    ));
    assert!(crate::path_identity::is_verified_descendant(
        &response_directory_identity,
        &local_identity
    ));
    let request_identity = crate::path_identity::verify_regular_file(&request_path)
        .expect("host request preflight identity");
    drop(
        fixture
            .snapshot_state
            .reserve_existing_artifact(
                &request_directory,
                &request_directory_identity,
                request_identity.object_id,
            )
            .expect("host request preflight reservation"),
    );
    assert!(fixture
        .app
        .as_ref()
        .expect("fixture app is alive")
        .try_state::<Arc<TerminalSnapshotState>>()
        .is_some());
    scanner.begin_cycle();
    scanner.scan_root(
        fixture.app.as_ref().expect("fixture app is alive").handle(),
        root,
    );
    scanner.finish_cycle();
    scanner.join_pending_tasks_for_test().await;
    let response_path = response_directory.join(format!("{}.json", request.request_id));
    let bytes = std::fs::read(&response_path).expect("host response bytes after task completion");
    #[cfg(not(unix))]
    let identity = crate::path_identity::verify_regular_file(&response_path)
        .expect("host response identity after task completion");
    std::fs::remove_file(&response_path).expect("remove consumed host response");
    #[cfg(not(unix))]
    fixture.snapshot_state.untrack_artifact(&identity);
    bytes
}

pub(super) fn write_host_request_bytes(
    root: &Path,
    filename_request_id: &str,
    bytes: &[u8],
) -> (PathBuf, PathBuf) {
    let local = root.join(crate::config::agent_local_dir_name());
    let request_path = local
        .join("outbox")
        .join("terminal-snapshot-requests")
        .join(format!("{filename_request_id}.json"));
    let response_path = local
        .join("terminal-snapshot-responses")
        .join(format!("{filename_request_id}.json"));
    std::fs::write(&request_path, bytes).expect("write raw host request");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&request_path, std::fs::Permissions::from_mode(0o600))
            .expect("private raw host request");
    }
    (request_path, response_path)
}

pub(super) fn audit_contains_request(settings_path: &Path, request_id: &str) -> bool {
    let audit = settings_path.with_file_name("api-audit.log");
    std::fs::read(audit)
        .ok()
        .is_some_and(|bytes| contains_raw(&bytes, request_id.as_bytes()))
}

pub(super) async fn submit_uncorrelated_host_bytes(
    fixture: &AcceptanceFixture,
    scanner: &mut crate::phone::terminal_snapshot::SnapshotMailboxScanner,
    root: &Path,
    filename_request_id: &str,
    bytes: &[u8],
) {
    let (request_path, response_path) = write_host_request_bytes(root, filename_request_id, bytes);
    scanner.begin_cycle();
    scanner.scan_root(
        fixture.app.as_ref().expect("fixture app is alive").handle(),
        root,
    );
    scanner.finish_cycle();
    scanner.join_pending_tasks_for_test().await;
    assert!(!request_path.exists());
    assert!(audit_contains_request(
        &fixture.settings_path,
        filename_request_id
    ));
    assert!(!response_path.exists());
}

pub(super) async fn submit_host_request_expect_no_response(
    fixture: &AcceptanceFixture,
    scanner: &mut crate::phone::terminal_snapshot::SnapshotMailboxScanner,
    root: &Path,
    request: &crate::phone::terminal_snapshot::HostTerminalSnapshotRequest,
) {
    let bytes = serde_json::to_vec(request).expect("host panic request wire");
    let (request_path, response_path) = write_host_request_bytes(root, &request.request_id, &bytes);
    scanner.begin_cycle();
    scanner.scan_root(
        fixture.app.as_ref().expect("fixture app is alive").handle(),
        root,
    );
    scanner.finish_cycle();
    scanner.join_pending_tasks_for_test().await;
    assert!(!request_path.exists());
    assert!(!response_path.exists());
    assert!(audit_contains_request(
        &fixture.settings_path,
        &request.request_id
    ));
}

pub(super) async fn post_api_snapshot(
    client: &reqwest::Client,
    address: std::net::SocketAddr,
    token: &str,
    target: &str,
) -> (String, StatusCode, reqwest::header::HeaderMap, Vec<u8>) {
    let request_id = Uuid::new_v4().to_string();
    let response = client
        .post(format!("http://{address}/api/v1/terminal-snapshot"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "apiVersion": "1",
            "requestId": request_id,
            "to": target,
            "format": "json"
        }))
        .send()
        .await
        .expect("snapshot API response");
    let status = StatusCode::from_u16(response.status().as_u16()).expect("HTTP status");
    let headers = response.headers().clone();
    let bytes = response.bytes().await.expect("snapshot API bytes").to_vec();
    (request_id, status, headers, bytes)
}

pub(super) async fn post_api_bytes(
    client: &reqwest::Client,
    address: std::net::SocketAddr,
    token: &str,
    bytes: Vec<u8>,
) -> (StatusCode, reqwest::header::HeaderMap, Vec<u8>) {
    let response = client
        .post(format!("http://{address}/api/v1/terminal-snapshot"))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(bytes)
        .send()
        .await
        .expect("raw snapshot API response");
    let status = StatusCode::from_u16(response.status().as_u16()).expect("HTTP status");
    let headers = response.headers().clone();
    let bytes = response.bytes().await.expect("snapshot API bytes").to_vec();
    (status, headers, bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ApiLifecycleCounts {
    pub(super) ingress_available: usize,
    pub(super) requester_in_flight: usize,
    pub(super) target_in_flight: usize,
    pub(super) global_in_flight: usize,
}

pub(super) fn api_lifecycle_counts(state: &TerminalSnapshotState) -> ApiLifecycleCounts {
    let limiter = state.limiter.lock().expect("snapshot limiter state");
    ApiLifecycleCounts {
        ingress_available: state.ingress.available_permits(),
        requester_in_flight: limiter.requester_in_flight.values().copied().sum(),
        target_in_flight: limiter.target_in_flight.values().copied().sum(),
        global_in_flight: limiter.global_in_flight,
    }
}

pub(super) fn assert_api_lifecycle_active(counts: ApiLifecycleCounts, target_promoted: bool) {
    assert_eq!(counts.ingress_available, SNAPSHOT_INGRESS_LIMIT);
    assert_eq!(counts.requester_in_flight, 1);
    assert_eq!(counts.target_in_flight, usize::from(target_promoted));
    assert_eq!(counts.global_in_flight, 1);
}

pub(super) fn assert_api_lifecycle_idle(state: &TerminalSnapshotState) {
    assert_eq!(
        api_lifecycle_counts(state),
        ApiLifecycleCounts {
            ingress_available: SNAPSHOT_INGRESS_LIMIT,
            requester_in_flight: 0,
            target_in_flight: 0,
            global_in_flight: 0,
        }
    );
}

pub(super) fn install_host_cancellation_barrier(
    state: &TerminalSnapshotState,
    stage: TerminalSnapshotHostCancellationStage,
) -> (
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::SyncSender<()>,
) {
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    state.install_host_cancellation_hook(stage, move || {
        entered_tx
            .send(())
            .expect("host cancellation barrier observer");
        release_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("host cancellation barrier release");
    });
    (entered_rx, release_tx)
}

pub(super) fn wait_for_host_cancellation_barrier(receiver: &std::sync::mpsc::Receiver<()>) {
    receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("host cancellation barrier was not reached");
}

pub(super) fn host_cancellation_marker(
    request_directory: &Path,
    request: &crate::phone::terminal_snapshot::HostTerminalSnapshotRequest,
) -> PathBuf {
    request_directory.join(format!(
        ".{}.{}.terminal-snapshot-cancelled",
        request.request_id, request.nonce
    ))
}

pub(super) fn consume_host_response(
    fixture: &AcceptanceFixture,
    root: &Path,
    request: &crate::phone::terminal_snapshot::HostTerminalSnapshotRequest,
) -> Vec<u8> {
    let path = root
        .join(crate::config::agent_local_dir_name())
        .join("terminal-snapshot-responses")
        .join(format!("{}.json", request.request_id));
    let bytes = std::fs::read(&path).expect("host cancellation response bytes");
    #[cfg(not(unix))]
    let identity = crate::path_identity::verify_regular_file(&path)
        .expect("host cancellation response identity");
    std::fs::remove_file(&path).expect("consume host cancellation response");
    // `fixture` is read only by the non-Unix untracking below; keep the
    // signature stable on Unix rather than splitting the helper.
    #[cfg(unix)]
    let _ = fixture;
    #[cfg(not(unix))]
    fixture.snapshot_state.untrack_artifact(&identity);
    bytes
}

pub(super) fn assert_no_api_test_hooks(state: &TerminalSnapshotState) {
    assert!(state
        .test_state
        .api_before_capture
        .lock()
        .expect("API before-capture hook state")
        .is_none());
    assert!(state
        .test_state
        .api_after_response_bytes
        .lock()
        .expect("API response-bytes hook state")
        .is_none());
    assert!(state
        .test_state
        .api_before_final_binding
        .lock()
        .expect("API final-binding hook state")
        .is_none());
    assert!(!state.has_blocking_controls());
}

pub(super) struct PreparedHostFinalizer {
    pub(super) request_id: String,
    pub(super) selected_session_id: Uuid,
    pub(super) response_bytes: Vec<u8>,
    pub(super) finalization: TerminalSnapshotFinalization,
}

pub(super) async fn prepare_host_finalizer(
    fixture: &AcceptanceFixture,
    target: &str,
) -> PreparedHostFinalizer {
    let request_id = Uuid::new_v4();
    let wall_deadline = chrono::Utc::now() + chrono::Duration::seconds(30);
    let host_authorization_deadline = Some((
        std::time::Instant::now() + Duration::from_secs(30),
        wall_deadline,
    ));
    let request = TerminalSnapshotServiceRequest {
        request_id,
        target: target.to_string(),
        format: TerminalSnapshotFormat::Json,
        source_plane: TerminalSnapshotSourcePlane::HostCli,
        host_authorization_deadline,
    };
    let audit = TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::HostCli);
    audit.accept_request(&request);
    let context = TerminalSnapshotServiceContext {
        session_manager: Arc::clone(&fixture.session_manager),
        pty_manager: Arc::clone(&fixture.pty_manager),
        settings: fixture.settings.clone(),
        restore: fixture
            .app
            .as_ref()
            .expect("fixture app is alive")
            .state::<Arc<crate::RestoreInProgress>>()
            .inner()
            .clone(),
        purge: fixture
            .app
            .as_ref()
            .expect("fixture app is alive")
            .state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .inner()
            .clone(),
    };
    let admission = fixture
        .snapshot_state
        .pre_admit_requester(
            &context,
            TerminalSnapshotRequesterSelector::Host {
                token: fixture.host_coordinator.token,
                expected_root: crate::path_identity::verify_directory(&fixture.paths.coordinator)
                    .expect("host finalizer requester root"),
                claimed_from: format!("{PROJECT}:{WORKGROUP}/coordinator"),
            },
            TerminalSnapshotSourcePlane::HostCli,
            host_authorization_deadline,
            audit,
        )
        .await
        .expect("host finalizer requester admission");
    let prepared = fixture
        .snapshot_state
        .prepare_with_admission(admission, request)
        .await
        .expect("host finalizer prepared payload");
    let (payload, finalization) = prepared.into_parts();
    let selected_session_id = finalization.selected.fact.id;
    let confirmation_tag = "c".repeat(64);
    let response_bytes = finalization
        .build_host_response(
            payload,
            request_id.to_string(),
            confirmation_tag.clone(),
            terminal_snapshot_renderer::canonical_timestamp(
                chrono::Utc::now() + chrono::Duration::seconds(60),
            ),
        )
        .await
        .expect("complete host response bytes");
    let response = decode_host_response(
        &response_bytes,
        &request_id.to_string(),
        &confirmation_tag,
        target,
        TerminalSnapshotFormat::Json,
    )
    .expect("complete host response decode");
    assert!(payload_has_sentinel(
        response
            .result
            .as_ref()
            .expect("complete host response payload")
    ));
    PreparedHostFinalizer {
        request_id: request_id.to_string(),
        selected_session_id,
        response_bytes,
        finalization,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostFinalizerHandoff {
    Success,
    Failure(TerminalSnapshotReasonCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostFinalizerWorkerOutcome {
    Returned(Result<(), TerminalSnapshotReasonCode>),
    Panicked,
}

pub(super) struct HostPublicationAuthorityDrop(pub(super) Arc<std::sync::atomic::AtomicBool>);

impl Drop for HostPublicationAuthorityDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

pub(super) struct RunningHostFinalizer {
    pub(super) control: Arc<TerminalSnapshotHostFinalizerControl>,
    pub(super) handle: Option<tokio::task::JoinHandle<()>>,
    pub(super) outcome: tokio::sync::oneshot::Receiver<HostFinalizerWorkerOutcome>,
    pub(super) authority_dropped: Arc<std::sync::atomic::AtomicBool>,
    pub(super) handoffs: Arc<Mutex<Vec<HostFinalizerHandoff>>>,
    pub(super) response_bytes: usize,
}

fn host_response_digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

pub(super) fn start_host_finalizer(
    mut prepared: PreparedHostFinalizer,
    control: Arc<TerminalSnapshotHostFinalizerControl>,
) -> RunningHostFinalizer {
    let response_bytes = prepared.response_bytes.len();
    let expected_digest = host_response_digest(&prepared.response_bytes);
    prepared
        .finalization
        .install_host_finalizer_control(Arc::clone(&control));
    let authority_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let authority = HostPublicationAuthorityDrop(Arc::clone(&authority_dropped));
    let handoffs = Arc::new(Mutex::new(Vec::new()));
    let worker_handoffs = Arc::clone(&handoffs);
    let (outcome_tx, outcome) = tokio::sync::oneshot::channel();
    let handle = tokio::task::spawn_blocking(move || {
        let publish = move |outcome: Result<Vec<u8>, TerminalSnapshotReasonCode>| {
            let _authority = authority;
            let handoff = match outcome {
                Ok(bytes) => {
                    assert_eq!(bytes.len(), response_bytes);
                    assert_eq!(host_response_digest(&bytes), expected_digest);
                    HostFinalizerHandoff::Success
                }
                Err(reason) => HostFinalizerHandoff::Failure(reason),
            };
            worker_handoffs
                .lock()
                .expect("host finalizer handoffs")
                .push(handoff);
            Ok(())
        };
        let outcome = match crate::logging::catch_payload_unwind(|| {
            prepared
                .finalization
                .finalize_host(prepared.response_bytes, publish)
        }) {
            Ok(result) => HostFinalizerWorkerOutcome::Returned(result),
            Err(_) => {
                log::error!("[terminal-snapshot] stage=host_finalizer_task code=internal");
                HostFinalizerWorkerOutcome::Panicked
            }
        };
        let _ = outcome_tx.send(outcome);
    });
    RunningHostFinalizer {
        control,
        handle: Some(handle),
        outcome,
        authority_dropped,
        handoffs,
        response_bytes,
    }
}

impl RunningHostFinalizer {
    pub(super) fn wait_until_blocked_and_detach(&mut self) {
        self.control.wait_until_entered();
        assert_eq!(
            self.control.retained_response_bytes(),
            self.response_bytes,
            "the complete response must remain owned by the finalizer"
        );
        assert!(!self.authority_dropped.load(Ordering::SeqCst));
        assert!(self
            .handoffs
            .lock()
            .expect("host finalizer handoffs")
            .is_empty());
        let handle = self.handle.take().expect("host finalizer waiter handle");
        handle.abort();
        drop(handle);
    }

    pub(super) async fn finish(self) -> (HostFinalizerWorkerOutcome, Vec<HostFinalizerHandoff>) {
        assert!(self.handle.is_none(), "the async waiter must be detached");
        let outcome = tokio::time::timeout(Duration::from_secs(60), self.outcome)
            .await
            .expect("detached host finalizer completion deadline")
            .expect("detached host finalizer completion signal");
        assert!(self.authority_dropped.load(Ordering::SeqCst));
        let handoffs = std::mem::take(&mut *self.handoffs.lock().expect("host finalizer handoffs"));
        (outcome, handoffs)
    }
}

pub(super) fn assert_host_finalizer_audit(
    config: &Path,
    canaries: &[String],
    expected_total: usize,
    request_id: &str,
    status: &str,
    reason: Option<TerminalSnapshotReasonCode>,
) {
    let rows = snapshot_audit_rows(config, canaries);
    assert_eq!(rows.len(), expected_total);
    let matching = rows
        .iter()
        .filter(|row| row.get("requestId").and_then(|value| value.as_str()) == Some(request_id))
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "host finalizer audit must be exactly once"
    );
    let row = matching[0];
    assert_eq!(
        row.get("status").and_then(|value| value.as_str()),
        Some(status)
    );
    assert_eq!(
        row.get("reasonCode").and_then(|value| value.as_str()),
        reason.map(TerminalSnapshotReasonCode::as_str)
    );
}
