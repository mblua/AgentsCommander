use super::*;
use crate::phone::terminal_snapshot::task::SnapshotTaskHost;

fn request_name(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012x}.json")
}

fn oracle(names: &[String], cursor: &str) -> Vec<String> {
    let mut ordered = names.to_vec();
    ordered.sort();
    let after: Vec<_> = ordered
        .iter()
        .filter(|name| name.as_str() > cursor)
        .cloned()
        .collect();
    if after.is_empty() {
        ordered
            .into_iter()
            .filter(|name| name.as_str() <= cursor)
            .take(MAX_PER_POLL)
            .collect()
    } else {
        after.into_iter().take(MAX_PER_POLL).collect()
    }
}

fn assert_selection(
    names: Vec<String>,
    cursor: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    for name in names.iter().rev() {
        std::fs::write(directory.path().join(name), b"")?;
    }
    for name in [
        "invalid.json",
        "00000000-0000-1000-8000-000000000001.json",
        "noise",
    ] {
        std::fs::write(directory.path().join(name), b"")?;
    }
    let identity = crate::path_identity::verify_directory(directory.path())?;
    let mut scanner = SnapshotMailboxScanner::delivery_only();
    if let Some(value) = &cursor {
        scanner.cursors.insert(identity.object_id, value.clone());
    }
    let expected = oracle(&names, cursor.as_deref().unwrap_or_default());
    assert_eq!(
        scanner.select_requests(directory.path(), identity.object_id),
        Some(expected.clone())
    );
    let expected_cursor = expected.last().cloned().or(cursor);
    assert_eq!(
        scanner.cursors.get(&identity.object_id),
        expected_cursor.as_ref()
    );
    Ok(())
}

#[test]
fn selection_matches_sorted_oracle() -> Result<(), Box<dyn std::error::Error>> {
    assert_selection(Vec::new(), None)?;
    assert_selection(Vec::new(), Some(request_name(7)))?;
    assert_selection(vec![request_name(7)], Some(request_name(7)))?;
    assert_selection(
        (1..=700).map(request_name).collect(),
        Some(request_name(350)),
    )?;
    assert_selection(
        (1..=700).map(request_name).collect(),
        Some(request_name(700)),
    )?;
    assert_selection((1..=700).map(request_name).collect(), None)?;
    assert_selection(
        vec![request_name(1), request_name(8)],
        Some(request_name(7)),
    )?;
    Ok(())
}

#[test]
fn directory_capacity_and_open_failure_preserve_cursor() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let identity = crate::path_identity::verify_directory(directory.path())?;
    let mut scanner = SnapshotMailboxScanner::delivery_only();
    let cursor = request_name(10);
    scanner.cursors.insert(identity.object_id, cursor.clone());
    let names: Vec<_> = (1..=MAX_DIRECTORY_ENTRIES).map(request_name).collect();
    for name in names.iter().rev() {
        std::fs::write(directory.path().join(name), b"")?;
    }
    let expected = oracle(&names, &cursor);
    assert_eq!(
        scanner.select_requests(directory.path(), identity.object_id),
        Some(expected.clone())
    );
    assert_eq!(scanner.cursors.get(&identity.object_id), expected.last());
    let before = scanner.cursors.clone();
    std::fs::write(directory.path().join("overflow"), b"")?;
    assert_eq!(
        scanner.select_requests(directory.path(), identity.object_id),
        None
    );
    assert_eq!(scanner.cursors, before);
    assert_eq!(
        scanner.select_requests(&directory.path().join("missing"), identity.object_id),
        None
    );
    assert_eq!(scanner.cursors, before);
    Ok(())
}

#[test]
fn empty_directory_keeps_cursor() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let identity = crate::path_identity::verify_directory(directory.path())?;
    let mut scanner = SnapshotMailboxScanner::delivery_only();
    let cursor = request_name(7);
    scanner.cursors.insert(identity.object_id, cursor.clone());
    assert_eq!(
        scanner.select_requests(directory.path(), identity.object_id),
        Some(Vec::new())
    );
    assert_eq!(scanner.cursors.get(&identity.object_id), Some(&cursor));
    Ok(())
}

type SessionOwner = Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>;
type PtyOwner = Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>;

const SERVICE_ORDER: [&str; 5] = ["session", "pty", "settings", "restore", "purge"];
const TRACE_MESSAGE: &str = "snapshot service lookup trace";

/// Inert backend: every call that would do work is counted and refused.
#[derive(Default)]
struct InertBackend {
    unexpected: std::sync::atomic::AtomicUsize,
}

impl InertBackend {
    fn refuse(&self) -> crate::errors::AppError {
        self.unexpected
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::errors::AppError::Other("inert test backend".to_string())
    }
}

impl crate::pty::backend::PtyBackend for InertBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        _spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        let error = self.refuse();
        Box::pin(async move { Err(error) })
    }
    fn write(
        &self,
        _authority: &crate::pty::input_route::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Err(self.refuse())
    }
    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Err(self.refuse())
    }
    fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
        Err(self.refuse())
    }
    fn has_session(&self, _id: Uuid) -> bool {
        false
    }
    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }
    fn register_response_watcher(&self, _id: Uuid, _request: String, _dir: PathBuf) {
        let _ = self.refuse();
    }
    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        let _ = self.refuse();
        false
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        let _ = self.refuse();
        (0, 0)
    }
}

#[derive(Default)]
struct RecordingSlots {
    snapshot: Option<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>,
    session: Option<SessionOwner>,
    pty: Option<PtyOwner>,
    settings: Option<crate::config::settings::SettingsState>,
    restore: Option<Arc<crate::RestoreInProgress>>,
    purge: Option<Arc<crate::session::purge_guard::PurgeGuard>>,
    backend: Option<Arc<InertBackend>>,
    trace: std::sync::Mutex<Vec<&'static str>>,
    snapshot_lookups: std::sync::atomic::AtomicUsize,
}

/// Clone shares the slots and the trace; cloning looks nothing up.
#[derive(Clone, Default)]
struct RecordingHost(Arc<RecordingSlots>);

impl SnapshotTaskHost for RecordingHost {
    fn spawn_snapshot_task(
        &self,
        future: futures::future::BoxFuture<'static, ()>,
    ) -> std::pin::Pin<Box<dyn crate::phone::terminal_snapshot::task::SnapshotTask>> {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        app.handle().spawn_snapshot_task(future)
    }
}

#[tokio::test]
async fn snapshot_task_forwards_completion_and_abort() {
    let host = RecordingHost::default();
    let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();
    let task = host.spawn_snapshot_task(Box::pin(async move {
        completed_tx.send(()).expect("completion receiver");
    }));
    task.await.expect("native completion");
    completed_rx.await.expect("forwarded future ran");

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let task = host.spawn_snapshot_task(Box::pin(async move {
        started_tx.send(()).expect("start receiver");
        std::future::pending::<()>().await;
    }));
    started_rx.await.expect("blocked task started");
    task.abort();
    assert!(task.await.is_err(), "aborted native task must fail to join");
    assert!(host.trace().is_empty());
    assert_eq!(host.snapshot_lookups(), 0);
}

impl RecordingHost {
    fn record<'a, T>(&'a self, service: &'static str, slot: &'a Option<T>) -> Option<&'a T> {
        self.0.trace.lock().expect("trace lock").push(service);
        slot.as_ref()
    }

    fn trace(&self) -> Vec<&'static str> {
        self.0.trace.lock().expect("trace lock").clone()
    }

    fn snapshot_lookups(&self) -> usize {
        self.0
            .snapshot_lookups
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn unexpected_backend_calls(&self) -> usize {
        self.0.backend.as_ref().map_or(0, |backend| {
            backend.unexpected.load(std::sync::atomic::Ordering::SeqCst)
        })
    }
}

impl TerminalSnapshotHost for RecordingHost {
    fn snapshot_state_opt(
        &self,
    ) -> Option<&Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>> {
        self.0
            .snapshot_lookups
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0.snapshot.as_ref()
    }
    fn session_manager_opt(&self) -> Option<&SessionOwner> {
        self.record("session", &self.0.session)
    }
    fn pty_manager_opt(&self) -> Option<&PtyOwner> {
        self.record("pty", &self.0.pty)
    }
    fn settings_opt(&self) -> Option<&crate::config::settings::SettingsState> {
        self.record("settings", &self.0.settings)
    }
    fn restore_opt(&self) -> Option<&Arc<crate::RestoreInProgress>> {
        self.record("restore", &self.0.restore)
    }
    fn purge_opt(&self) -> Option<&Arc<crate::session::purge_guard::PurgeGuard>> {
        self.record("purge", &self.0.purge)
    }
}

/// Host with the snapshot state (when `snapshot`) and every service except `missing`.
fn recording_host(snapshot: bool, missing: Option<&str>) -> RecordingHost {
    let present = |service: &str| missing != Some(service);
    let backend = Arc::new(InertBackend::default());
    let pty_backend: Arc<dyn crate::pty::backend::PtyBackend> = backend.clone();
    RecordingHost(Arc::new(RecordingSlots {
        snapshot: snapshot.then(|| {
            crate::pty::terminal_snapshot::TerminalSnapshotState::new(
                crate::shutdown::ShutdownSignal::new(),
            )
        }),
        session: present("session").then(|| {
            Arc::new(tokio::sync::RwLock::new(
                crate::session::manager::SessionManager::new(),
            ))
        }),
        pty: present("pty").then(|| {
            Arc::new(std::sync::Mutex::new(
                crate::pty::manager::PtyManager::new_for_test(pty_backend),
            ))
        }),
        settings: present("settings").then(|| {
            crate::config::settings::new_settings_state(
                crate::config::settings::AppSettings::default(),
            )
        }),
        restore: present("restore").then(|| {
            Arc::new(crate::RestoreInProgress(
                std::sync::atomic::AtomicBool::new(false),
            ))
        }),
        purge: present("purge")
            .then(|| Arc::new(crate::session::purge_guard::PurgeGuard::default())),
        backend: Some(backend),
        ..RecordingSlots::default()
    }))
}

fn isolated_scanner() -> SnapshotMailboxScanner {
    SnapshotMailboxScanner {
        shutdown_owner: SnapshotScannerShutdownOwner::unregistered(),
        cursors: HashMap::new(),
        observed: std::collections::HashSet::new(),
        startup_sweep_complete: false,
    }
}

fn create_mailbox_directories(root: &Path) -> (PathBuf, PathBuf) {
    let local = root.join(crate::config::agent_local_dir_name());
    let request_directory = local.join("outbox").join(REQUEST_DIRECTORY);
    let response_directory = local.join(RESPONSE_DIRECTORY);
    for directory in [&request_directory, &response_directory] {
        std::fs::create_dir_all(directory).expect("snapshot mailbox directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .expect("private snapshot mailbox directory");
        }
    }
    (request_directory, response_directory)
}

fn write_request_bytes(request_directory: &Path, request_id: &str, bytes: &[u8]) -> PathBuf {
    let path = request_directory.join(format!("{request_id}.json"));
    std::fs::write(&path, bytes).expect("write host request");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("private host request");
    }
    path
}

fn valid_request() -> super::super::HostTerminalSnapshotRequest {
    let now = chrono::Utc::now();
    let mut request = super::super::HostTerminalSnapshotRequest {
        kind: super::super::REQUEST_KIND.to_string(),
        version: super::super::REQUEST_VERSION,
        request_id: Uuid::new_v4().to_string(),
        token: Uuid::new_v4().to_string(),
        from: "project:wg-1-team/lead".to_string(),
        to: "project:wg-1-team/member".to_string(),
        format: terminal_snapshot_renderer::TerminalSnapshotFormat::Json,
        issued_at: terminal_snapshot_renderer::canonical_timestamp(now),
        expires_at: terminal_snapshot_renderer::canonical_timestamp(
            now + chrono::Duration::seconds(15),
        ),
        nonce: "a1".repeat(32),
        confirmation_tag: String::new(),
    };
    request.confirmation_tag = super::super::confirmation_tag(&request);
    assert!(request.validate(now).is_ok(), "fixture request is valid");
    request
}

/// Runs one real scan and joins its tasks; returns the tracked task count seen before the join.
async fn scan_once(host: &RecordingHost, root: &Path) -> usize {
    let mut scanner = isolated_scanner();
    scanner.begin_cycle();
    scanner.scan_root(host, root);
    scanner.finish_cycle();
    let tracked = scanner.shutdown_owner.task_count();
    scanner.join_pending_tasks_for_test().await;
    assert_eq!(scanner.shutdown_owner.task_count(), 0);
    tracked
}

fn assert_no_processing_artifact(request_directory: &Path) {
    let leftovers: Vec<_> = std::fs::read_dir(request_directory)
        .expect("request directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".terminal-snapshot-processing"))
        .collect();
    assert!(leftovers.is_empty(), "processing artifacts: {leftovers:?}");
}

/// Reads the correlated failure response and returns its error code.
fn failure_response(
    response_directory: &Path,
    request: &super::super::HostTerminalSnapshotRequest,
) -> String {
    let path = response_directory.join(format!("{}.json", request.request_id));
    let bytes = std::fs::read(path).expect("correlated response");
    let response: serde_json::Value = serde_json::from_slice(&bytes).expect("response JSON");
    let object = response.as_object().expect("response object");
    let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "apiVersion",
            "confirmationTag",
            "detail",
            "error",
            "expiresAt",
            "requestId"
        ],
        "failure response carries no success payload"
    );
    assert_eq!(object["requestId"], request.request_id.as_str());
    assert_eq!(object["confirmationTag"], request.confirmation_tag.as_str());
    object["error"].as_str().expect("error code").to_string()
}

#[tokio::test]
async fn snapshot_missing_each_service() {
    for (index, missing) in SERVICE_ORDER.iter().enumerate() {
        let root = tempfile::tempdir().expect("requester root");
        let (request_directory, response_directory) = create_mailbox_directories(root.path());
        let request = valid_request();
        let bytes = serde_json::to_vec(&request).expect("request JSON");
        let request_path = write_request_bytes(&request_directory, &request.request_id, &bytes);
        let host = recording_host(true, Some(missing));
        assert_eq!(scan_once(&host, root.path()).await, 1, "{missing}");
        assert_eq!(host.trace(), SERVICE_ORDER[..=index], "{TRACE_MESSAGE}");
        assert_eq!(
            failure_response(&response_directory, &request),
            TerminalSnapshotReasonCode::ServiceUnavailable.as_str(),
            "{missing}"
        );
        assert!(!request_path.exists());
        assert_no_processing_artifact(&request_directory);
        assert_eq!(host.snapshot_lookups(), 1);
        assert_eq!(host.unexpected_backend_calls(), 0);
    }
}

#[tokio::test]
async fn snapshot_invalid_request_has_no_service_lookups() {
    let root = tempfile::tempdir().expect("requester root");
    let (request_directory, response_directory) = create_mailbox_directories(root.path());
    let mut request = valid_request();
    request.version = super::super::REQUEST_VERSION + 1;
    let bytes = serde_json::to_vec(&request).expect("request JSON");
    write_request_bytes(&request_directory, &request.request_id, &bytes);
    let host = recording_host(true, None);
    assert_eq!(scan_once(&host, root.path()).await, 1);
    assert!(
        host.trace().is_empty(),
        "{TRACE_MESSAGE}: {:?}",
        host.trace()
    );
    assert_eq!(
        failure_response(&response_directory, &request),
        TerminalSnapshotReasonCode::InvalidRequest.as_str()
    );
    assert_eq!(host.snapshot_lookups(), 1);
    assert_no_processing_artifact(&request_directory);

    let malformed_id = Uuid::new_v4().to_string();
    let malformed_path = write_request_bytes(&request_directory, &malformed_id, b"{");
    let host = recording_host(true, None);
    assert_eq!(scan_once(&host, root.path()).await, 1);
    assert!(
        host.trace().is_empty(),
        "{TRACE_MESSAGE}: {:?}",
        host.trace()
    );
    assert_eq!(host.snapshot_lookups(), 1);
    assert!(!malformed_path.exists());
    assert!(!response_directory
        .join(format!("{malformed_id}.json"))
        .exists());
    assert_no_processing_artifact(&request_directory);
}

#[tokio::test]
async fn snapshot_service_lookup_order() {
    let root = tempfile::tempdir().expect("requester root");
    let (request_directory, response_directory) = create_mailbox_directories(root.path());
    let request = valid_request();
    let bytes = serde_json::to_vec(&request).expect("request JSON");
    write_request_bytes(&request_directory, &request.request_id, &bytes);
    let host = recording_host(true, None);
    assert_eq!(scan_once(&host, root.path()).await, 1);
    assert_eq!(host.trace(), SERVICE_ORDER, "{TRACE_MESSAGE}");
    let error = failure_response(&response_directory, &request);
    assert_ne!(
        error,
        TerminalSnapshotReasonCode::ServiceUnavailable.as_str()
    );
    assert_eq!(host.snapshot_lookups(), 1);
    assert_eq!(host.unexpected_backend_calls(), 0);
    assert_no_processing_artifact(&request_directory);
}

#[tokio::test]
async fn snapshot_state_absent() {
    let root = tempfile::tempdir().expect("requester root");
    let (request_directory, response_directory) = create_mailbox_directories(root.path());
    let request = valid_request();
    let bytes = serde_json::to_vec(&request).expect("request JSON");
    let request_path = write_request_bytes(&request_directory, &request.request_id, &bytes);
    let host = recording_host(false, None);
    assert_eq!(scan_once(&host, root.path()).await, 0);
    assert!(
        host.trace().is_empty(),
        "{TRACE_MESSAGE}: {:?}",
        host.trace()
    );
    assert_eq!(host.snapshot_lookups(), 1);
    assert_eq!(std::fs::read(&request_path).expect("request intact"), bytes);
    assert!(!response_directory
        .join(format!("{}.json", request.request_id))
        .exists());
    assert_no_processing_artifact(&request_directory);

    let bare_root = tempfile::tempdir().expect("root without mailbox");
    let host = recording_host(true, None);
    assert_eq!(scan_once(&host, bare_root.path()).await, 0);
    assert_eq!(host.snapshot_lookups(), 0);
    assert!(
        host.trace().is_empty(),
        "{TRACE_MESSAGE}: {:?}",
        host.trace()
    );
}
