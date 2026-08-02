use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use futures::future::BoxFuture;
use tauri::Manager;
use terminal_snapshot_renderer::{
    decode_api_error, decode_api_success, decode_host_response, TerminalSnapshotFormat,
    TerminalSnapshotPayload, TerminalSnapshotReasonCode,
};
use uuid::Uuid;

use super::*;
use crate::errors::AppError;
use crate::pty::backend::{
    BackendSpawnSpec, PtyBackend, SessionBackendKind, TerminalScreenCopyRead,
};
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::manager::BackendWriteAuthority;
use crate::pty::output::{PtyOutputTarget, PtyScreenSnapshot, SessionIoFanout};
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionRepo};
use crate::telegram::manager::OutputSenderMap;

const ACCEPTANCE_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_ACCEPTANCE_CHILD";
const ACCEPTANCE_TEST_NAME: &str = "pty::terminal_snapshot::acceptance_tests::real_host_and_api_daemon_paths_enforce_no_oracle_and_final_handoff";
const LEAKAGE_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_LEAKAGE_CHILD";
const LEAKAGE_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_LEAKAGE_CANARY_FILE";
const LEAKAGE_TEST_NAME: &str = "pty::terminal_snapshot::acceptance_tests::real_host_and_api_daemon_paths_enforce_secondary_leakage_confinement";
const PANIC_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_PANIC_CHILD";
const PANIC_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_PANIC_CANARY_FILE";
const PANIC_TEST_NAME: &str = "pty::terminal_snapshot::acceptance_tests::snapshot_production_panic_boundaries_are_payload_free";
const SCREEN_SENTINEL: &str = "ACSNAP_CELL_CANARY_1173_Z9Q7";
const OSC_TITLE_SENTINEL: &str = "ACSNAP_OSC_TITLE_CANARY_1173_Z9Q7";
const OSC_HYPERLINK_SENTINEL: &str = "ACSNAP_OSC_HYPERLINK_CANARY_1173_Z9Q7";
const OSC_CLIPBOARD_SENTINEL: &str = "ACSNAP_OSC_CLIPBOARD_CANARY_1173_Z9Q7";
const MALFORMED_BODY_SENTINEL: &str = "ACSNAP_MALFORMED_BODY_CANARY_1173_Z9Q7";
const MALFORMED_TARGET_SENTINEL: &str = "ACSNAP_MALFORMED_TARGET_CANARY_1173_Z9Q7";
const CALLER_PATH_SENTINEL: &str = "ACSNAP_CALLER_PATH_CANARY_1173_Z9Q7";
const API_PANIC_SENTINEL: &str = "ACSNAP_API_PANIC_CANARY_1173_P8T4";
const BLOCKING_PANIC_SENTINEL: &str = "ACSNAP_BLOCKING_PANIC_CANARY_1173_B3M6";
const HOST_PANIC_SENTINEL: &str = "ACSNAP_HOST_PANIC_CANARY_1173_H5R2";
const PNG_PANIC_SENTINEL: &str = "ACSNAP_PNG_BYTES_CANARY_1173_G7V4";
const BASE64_PANIC_SENTINEL: &str = "QUNTTkFQX0JBU0U2NF9DQU5BUllfMTE3M19LOFEz";
const HOST_DENIAL_NONCE: &str = "8e9b656f9206198da204c61ef683102b19d1e52c1d8ea385394b04af1c4c26fd";
const HOST_SUCCESS_NONCE: &str = "371f6c57e75bb56177c857f37c31c76f79b552ffa5751a1e3cd11d63b0ec30a5";
const HOST_UNCORRELATED_NONCE: &str =
    "b6bf86d49adfe8b0f8fd34f1767dfc5415e37c3e1fdb781364b5b49de3231b34";
const HOST_FINAL_NONCE: &str = "cfe821546eab901d4b548d56b77f42d5587865d709a76a317855613bd4072525";
const HOST_PANIC_NONCE: &str = "d4ce25af58bfe2f151aebfa3b5865a627a52da87eb8d4ad303049568dc718d33";
const PROJECT: &str = "project";
const WORKGROUP: &str = "wg-1-dev-team";

struct ConfigEnvGuard {
    previous: Option<OsString>,
}

impl ConfigEnvGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR");
        std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", path);
        Self { previous }
    }
}

impl Drop for ConfigEnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", previous);
        } else {
            std::env::remove_var("AGENTSCOMMANDER_TEST_CONFIG_DIR");
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BackendLookupCounts {
    liveness: usize,
    copies: usize,
}

struct FixtureBackend {
    fanout: SessionIoFanout,
    live: Mutex<HashSet<Uuid>>,
    lookups: Mutex<HashMap<Uuid, BackendLookupCounts>>,
    mutations: std::sync::atomic::AtomicUsize,
}

impl FixtureBackend {
    fn new() -> Arc<Self> {
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        Arc::new(Self {
            fanout: SessionIoFanout::new(output_senders, idle_detector, None),
            live: Mutex::new(HashSet::new()),
            lookups: Mutex::new(HashMap::new()),
            mutations: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn install(&self, id: Uuid, output: &[u8]) {
        self.live.lock().expect("fixture live lock").insert(id);
        self.fanout
            .register_session(id, crate::session::profile::IdleTuning::DEFAULT, 4, 40);
        self.fanout.handle_output(
            &PtyOutputTarget::noop(),
            id,
            &id.to_string(),
            output.to_vec(),
        );
    }

    fn counts(&self, id: Uuid) -> BackendLookupCounts {
        self.lookups
            .lock()
            .expect("fixture lookup lock")
            .get(&id)
            .copied()
            .unwrap_or_default()
    }

    fn mutations(&self) -> usize {
        self.mutations.load(Ordering::SeqCst)
    }

    fn record_lookup(&self, id: Uuid, update: impl FnOnce(&mut BackendLookupCounts)) {
        let mut lookups = self.lookups.lock().expect("fixture lookup lock");
        update(lookups.entry(id).or_default());
    }
}

impl PtyBackend for FixtureBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(&self, spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), AppError>> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            self.live.lock().expect("fixture live lock").insert(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), AppError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), AppError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        self.live.lock().expect("fixture live lock").remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().expect("fixture live lock").contains(&id)
    }

    fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        self.record_lookup(id, |counts| counts.liveness += 1);
        if self.has_session(id) {
            ContextSessionLiveness::Live
        } else {
            ContextSessionLiveness::SessionOver
        }
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        self.fanout.get_screen_snapshot(id)
    }

    fn copy_terminal_screen(&self, id: Uuid) -> TerminalScreenCopyRead {
        self.record_lookup(id, |counts| counts.copies += 1);
        self.fanout.copy_terminal_screen(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.fanout.get_pty_size(id)
    }

    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        match self.fanout.get_screen_rows(id) {
            Some(rows) => ScreenRowsRead::Rows(rows),
            None if self.has_session(id) => ScreenRowsRead::Unavailable,
            None => ScreenRowsRead::SessionOver,
        }
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

struct ReplicaPaths {
    collection: PathBuf,
    coordinator: PathBuf,
    worker: PathBuf,
    live_member: PathBuf,
    exited_member: PathBuf,
}

impl ReplicaPaths {
    fn create(root: &Path) -> Self {
        let collection = root.join("projects");
        let workspace = collection.join(PROJECT).join(".ac");
        let team = workspace.join("_team_dev-team");
        let workgroup = workspace.join(WORKGROUP);
        std::fs::create_dir_all(&team).expect("team directory");
        for name in [
            "coordinator",
            "worker",
            "member-live",
            "member-exited",
            "member-tampered",
        ] {
            std::fs::create_dir_all(workspace.join(format!("_agent_{name}")))
                .expect("origin agent directory");
            std::fs::create_dir_all(workgroup.join(format!("__agent_{name}")))
                .expect("replica directory");
        }
        std::fs::write(
            team.join("config.json"),
            r#"{"agents":["../_agent_worker","../_agent_member-live","../_agent_member-exited","../_agent_member-tampered"],"coordinator":"../_agent_coordinator"}"#,
        )
        .expect("team config");
        for name in ["coordinator", "worker", "member-live", "member-exited"] {
            std::fs::write(
                workgroup.join(format!("__agent_{name}/config.json")),
                format!(r#"{{"identity":"../../_agent_{name}"}}"#),
            )
            .expect("replica config");
        }
        std::fs::write(
            workgroup.join("__agent_member-tampered/config.json"),
            r#"{"identity":"../../_agent_worker"}"#,
        )
        .expect("tampered replica config");
        Self {
            collection,
            coordinator: workgroup.join("__agent_coordinator"),
            worker: workgroup.join("__agent_worker"),
            live_member: workgroup.join("__agent_member-live"),
            exited_member: workgroup.join("__agent_member-exited"),
        }
    }
}

struct AcceptanceFixture {
    _temporary: tempfile::TempDir,
    app: tauri::App,
    snapshot_state: Arc<TerminalSnapshotState>,
    settings: crate::config::settings::SettingsState,
    settings_path: PathBuf,
    registry_path: PathBuf,
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_manager: Arc<std::sync::Mutex<PtyManager>>,
    local_backend: Arc<FixtureBackend>,
    paths: ReplicaPaths,
    host_coordinator: Session,
    host_worker: Session,
    live_member: Session,
    api_coordinator_token: crate::pty::container_tokens::ContainerApiToken,
    api_worker_token: crate::pty::container_tokens::ContainerApiToken,
    _container_receivers:
        Vec<tokio::sync::mpsc::Receiver<crate::pty::container_backend::HostToBridgeFrame>>,
}

impl AcceptanceFixture {
    async fn new(temporary: tempfile::TempDir) -> Self {
        let paths = ReplicaPaths::create(temporary.path());
        let config = temporary.path().join("config");
        std::fs::create_dir_all(&config).expect("config directory");
        let settings_path = config.join("settings.json");
        write_security_settings(&settings_path, &paths.collection, true);

        let app_settings = crate::config::settings::AppSettings {
            terminal_snapshots_enabled: true,
            project_paths: vec![paths.collection.to_string_lossy().to_string()],
            ..Default::default()
        };
        let settings: crate::config::settings::SettingsState =
            Arc::new(tokio::sync::RwLock::new(app_settings));

        let manager = SessionManager::new();
        let host_coordinator = create_session(
            &manager,
            &paths.coordinator,
            true,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let host_worker = create_session(
            &manager,
            &paths.worker,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let live_member = create_session(
            &manager,
            &paths.live_member,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await;
        let exited_member = create_session(
            &manager,
            &paths.exited_member,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await;
        manager.mark_exited(exited_member.id, 0).await;
        let api_coordinator = create_session(
            &manager,
            &paths.coordinator,
            true,
            SessionBackendKind::ContainerTransport,
        )
        .await;
        let api_worker = create_session(
            &manager,
            &paths.worker,
            false,
            SessionBackendKind::ContainerTransport,
        )
        .await;
        let session_manager = Arc::new(tokio::sync::RwLock::new(manager));

        let local_backend = FixtureBackend::new();
        local_backend.install(host_coordinator.id, b"host coordinator");
        local_backend.install(host_worker.id, b"host worker");
        local_backend.install(live_member.id, &terminal_canary_output());
        local_backend.install(exited_member.id, b"exited target");

        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let container_backend = Arc::new(
            crate::pty::container_backend::ContainerTransportBackend::new(
                output_senders,
                idle_detector,
                None,
                None,
            ),
        );
        let registry_path = config.join(crate::api::auth::REGISTRY_FILENAME);
        let token_manager = crate::pty::container_tokens::ContainerApiTokenManager::new_for_path(
            registry_path.clone(),
        );
        let api_coordinator_token = token_manager
            .mint_for_session(
                api_coordinator.id,
                paths.coordinator.to_string_lossy().as_ref(),
            )
            .expect("coordinator API token");
        let api_worker_token = token_manager
            .mint_for_session(api_worker.id, paths.worker.to_string_lossy().as_ref())
            .expect("worker API token");
        let coordinator_binding = binding_for(
            &api_coordinator_token,
            &crate::path_identity::verify_directory(&paths.coordinator)
                .expect("coordinator identity"),
        );
        let worker_binding = binding_for(
            &api_worker_token,
            &crate::path_identity::verify_directory(&paths.worker).expect("worker identity"),
        );
        let container_receivers = vec![
            container_backend.install_protocol_snapshot_session_for_test(
                api_coordinator.id,
                coordinator_binding,
                4,
                40,
                b"API coordinator".to_vec(),
            ),
            container_backend.install_protocol_snapshot_session_for_test(
                api_worker.id,
                worker_binding,
                4,
                40,
                b"API worker".to_vec(),
            ),
        ];

        let local_trait: Arc<dyn PtyBackend> = local_backend.clone();
        let pty = PtyManager::new_for_test_with_container_backend(local_trait, container_backend);
        record_route(
            &pty,
            host_coordinator.id,
            SessionBackendKind::LocalProcess,
            &paths.coordinator,
        );
        record_route(
            &pty,
            host_worker.id,
            SessionBackendKind::LocalProcess,
            &paths.worker,
        );
        record_route(
            &pty,
            live_member.id,
            SessionBackendKind::LocalProcess,
            &paths.live_member,
        );
        record_route(
            &pty,
            exited_member.id,
            SessionBackendKind::LocalProcess,
            &paths.exited_member,
        );
        record_route(
            &pty,
            api_coordinator.id,
            SessionBackendKind::ContainerTransport,
            &paths.coordinator,
        );
        record_route(
            &pty,
            api_worker.id,
            SessionBackendKind::ContainerTransport,
            &paths.worker,
        );
        let pty_manager = Arc::new(std::sync::Mutex::new(pty));

        for root in [&paths.coordinator, &paths.worker] {
            create_mailbox_directories(root);
        }
        let shutdown = crate::shutdown::ShutdownSignal::new();
        let snapshot_state = TerminalSnapshotState::new(shutdown);
        let restore = Arc::new(crate::RestoreInProgress(
            std::sync::atomic::AtomicBool::new(false),
        ));
        let purge = Arc::new(crate::session::purge_guard::PurgeGuard::default());
        let message_store = Arc::new(
            crate::api::message_store::MessageStore::open(config.join("api-messages.sqlite3"))
                .expect("API message store"),
        );
        let app = tauri::Builder::default()
            .any_thread()
            .manage(Arc::clone(&snapshot_state))
            .manage(settings.clone())
            .manage(Arc::clone(&session_manager))
            .manage(Arc::clone(&pty_manager))
            .manage(restore)
            .manage(purge)
            .manage(crate::api::message_store::MessageStoreState::ready(
                message_store,
            ))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("terminal snapshot acceptance app");

        Self {
            _temporary: temporary,
            app,
            snapshot_state,
            settings,
            settings_path,
            registry_path,
            session_manager,
            pty_manager,
            local_backend,
            paths,
            host_coordinator,
            host_worker,
            live_member,
            api_coordinator_token,
            api_worker_token,
            _container_receivers: container_receivers,
        }
    }
}

async fn create_session(
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

fn binding_for(
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

fn record_route(manager: &PtyManager, id: Uuid, kind: SessionBackendKind, root: &Path) {
    let identity = crate::path_identity::verify_directory(root).expect("route identity");
    manager
        .record_route_with_identities(id, kind, Some(identity.clone()), Some(identity))
        .expect("fixture route");
}

fn write_security_settings(path: &Path, collection: &Path, enabled: bool) {
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

fn create_mailbox_directories(root: &Path) {
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

fn terminal_canary_output() -> Vec<u8> {
    format!(
        "\u{1b}]0;{OSC_TITLE_SENTINEL}\u{7}\u{1b}]8;;https://snapshot.invalid/{OSC_HYPERLINK_SENTINEL}\u{1b}\\\u{1b}]8;;\u{1b}\\\u{1b}]52;c;{OSC_CLIPBOARD_SENTINEL}\u{7}{SCREEN_SENTINEL}"
    )
    .into_bytes()
}

fn host_request(
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

fn retag_host_request(
    mut request: crate::phone::terminal_snapshot::HostTerminalSnapshotRequest,
    nonce: &str,
) -> crate::phone::terminal_snapshot::HostTerminalSnapshotRequest {
    request.nonce = nonce.to_string();
    request.confirmation_tag = crate::phone::terminal_snapshot::confirmation_tag(&request);
    request
}

async fn submit_host_request(
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
        .try_state::<Arc<TerminalSnapshotState>>()
        .is_some());
    scanner.begin_cycle();
    scanner.scan_root(fixture.app.handle(), root);
    scanner.finish_cycle();
    scanner.join_pending_tasks_for_test().await;
    let response_path = response_directory.join(format!("{}.json", request.request_id));
    let bytes = std::fs::read(&response_path).expect("host response bytes after task completion");
    let identity = crate::path_identity::verify_regular_file(&response_path)
        .expect("host response identity after task completion");
    std::fs::remove_file(&response_path).expect("remove consumed host response");
    fixture.snapshot_state.untrack_artifact(&identity);
    bytes
}

fn write_host_request_bytes(
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

fn audit_contains_request(settings_path: &Path, request_id: &str) -> bool {
    let audit = settings_path.with_file_name("api-audit.log");
    std::fs::read(audit)
        .ok()
        .is_some_and(|bytes| contains_raw(&bytes, request_id.as_bytes()))
}

async fn submit_uncorrelated_host_bytes(
    fixture: &AcceptanceFixture,
    scanner: &mut crate::phone::terminal_snapshot::SnapshotMailboxScanner,
    root: &Path,
    filename_request_id: &str,
    bytes: &[u8],
) {
    let (request_path, response_path) = write_host_request_bytes(root, filename_request_id, bytes);
    scanner.begin_cycle();
    scanner.scan_root(fixture.app.handle(), root);
    scanner.finish_cycle();
    scanner.join_pending_tasks_for_test().await;
    assert!(!request_path.exists());
    assert!(audit_contains_request(
        &fixture.settings_path,
        filename_request_id
    ));
    assert!(!response_path.exists());
}

async fn submit_host_request_expect_no_response(
    fixture: &AcceptanceFixture,
    scanner: &mut crate::phone::terminal_snapshot::SnapshotMailboxScanner,
    root: &Path,
    request: &crate::phone::terminal_snapshot::HostTerminalSnapshotRequest,
) {
    let bytes = serde_json::to_vec(request).expect("host panic request wire");
    let (request_path, response_path) = write_host_request_bytes(root, &request.request_id, &bytes);
    scanner.begin_cycle();
    scanner.scan_root(fixture.app.handle(), root);
    scanner.finish_cycle();
    scanner.join_pending_tasks_for_test().await;
    assert!(!request_path.exists());
    assert!(!response_path.exists());
    assert!(audit_contains_request(
        &fixture.settings_path,
        &request.request_id
    ));
}

async fn post_api_snapshot(
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

async fn post_api_bytes(
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

fn available_loopback_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved loopback address")
        .port()
}

fn contains_raw(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn assert_canaries_absent_except(
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

fn snapshot_audit_rows(config: &Path, canaries: &[String]) -> Vec<serde_json::Value> {
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

fn assert_audit_inventory(config: &Path, canaries: &[String]) {
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

fn assert_panic_audit_inventory(config: &Path, canaries: &[String]) {
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

fn assert_cleanup_and_secondary_surfaces(
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
        if name == "settings.json" {
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

fn payload_has_sentinel(payload: &TerminalSnapshotPayload) -> bool {
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
        let start = crate::api::start_server(
            "127.0.0.1".to_string(),
            available_loopback_port(),
            fixture.app.handle().clone(),
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
        let start = crate::api::start_server(
            "127.0.0.1".to_string(),
            available_loopback_port(),
            fixture.app.handle().clone(),
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
        let malformed_target = format!(
            "../../{MALFORMED_TARGET_SENTINEL}/{CALLER_PATH_SENTINEL}/secret.png"
        );
        let malformed_api_bytes = serde_json::to_vec(&serde_json::json!({
            "apiVersion": "1",
            "requestId": malformed_api_request_id,
            "to": malformed_target,
            "format": "json"
        }))
        .expect("malformed API request bytes");
        let (malformed_api_status, malformed_api_headers, malformed_api_response) =
            post_api_bytes(
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
            "{{\"kind\":\"terminal-snapshot\",\"requestId\":\"{malformed_host_filename}\",\"token\":\"{MALFORMED_BODY_SENTINEL}\""
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
        assert_ne!(
            uncorrelated_filename,
            host_uncorrelated_request.request_id
        );
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
        assert_eq!(api_final_headers[reqwest::header::CACHE_CONTROL], "no-store");
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
        let canary_file = PathBuf::from(
            std::env::var_os(PANIC_CANARY_FILE_ENV).expect("panic canary file path"),
        );
        std::fs::write(
            canary_file,
            serde_json::to_vec(&canaries).expect("panic canary manifest"),
        )
        .expect("write panic canary manifest");
        log::trace!(target: "vt100", "{OSC_TITLE_SENTINEL}");
        log::trace!(target: "vt100::parser", "{OSC_CLIPBOARD_SENTINEL}");

        let api_shutdown = tokio_util::sync::CancellationToken::new();
        let start = crate::api::start_server(
            "127.0.0.1".to_string(),
            available_loopback_port(),
            fixture.app.handle().clone(),
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
            "{API_PANIC_SENTINEL}|{PNG_PANIC_SENTINEL}|{BASE64_PANIC_SENTINEL}|{}|{CALLER_PATH_SENTINEL}|{SCREEN_SENTINEL}",
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
            "{BLOCKING_PANIC_SENTINEL}|{PNG_PANIC_SENTINEL}|{BASE64_PANIC_SENTINEL}|{}|{CALLER_PATH_SENTINEL}|{OSC_TITLE_SENTINEL}",
            fixture.host_coordinator.token
        );
        let blocking_result: Result<(), TerminalSnapshotReasonCode> = run_blocking_with_deadline(
            std::time::Instant::now() + Duration::from_secs(5),
            &blocking_permit,
            &blocking_audit,
            move || std::panic::panic_any(blocking_panic_payload),
        )
        .await;
        assert_eq!(
            blocking_result,
            Err(TerminalSnapshotReasonCode::Internal)
        );
        blocking_audit.finalize_failure(TerminalSnapshotReasonCode::Internal);
        drop(blocking_permit);

        let host_panic_payload = format!(
            "{HOST_PANIC_SENTINEL}|{PNG_PANIC_SENTINEL}|{BASE64_PANIC_SENTINEL}|{}|{CALLER_PATH_SENTINEL}|{SCREEN_SENTINEL}",
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
