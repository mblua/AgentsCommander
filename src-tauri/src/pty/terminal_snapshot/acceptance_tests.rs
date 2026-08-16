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
use crate::pty::output::{PtyScreenSnapshot, SessionIoFanout};
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
const API_CANCELLATION_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_API_CANCELLATION_CHILD";
const API_CANCELLATION_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_API_CANCELLATION_CANARY_FILE";
const API_CANCELLATION_TEST_NAME: &str = "pty::terminal_snapshot::acceptance_tests::api_async_cancellation_reclaims_authority_without_disclosure";
const COMMON_BLOCKING_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_COMMON_BLOCKING_CHILD";
const COMMON_BLOCKING_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_COMMON_BLOCKING_CANARY_FILE";
const COMMON_BLOCKING_TEST_NAME: &str = "pty::terminal_snapshot::acceptance_tests::common_blocking_late_completion_retains_authority_without_disclosure";
const HOST_FINALIZER_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_HOST_FINALIZER_CHILD";
const HOST_FINALIZER_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_HOST_FINALIZER_CANARY_FILE";
const HOST_FINALIZER_TEST_NAME: &str = "pty::terminal_snapshot::acceptance_tests::synchronous_host_finalizer_retains_authority_through_late_completion";
const HOST_CANCELLATION_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_HOST_CANCELLATION_CHILD";
const HOST_CANCELLATION_CANARY_FILE_ENV: &str =
    "AC_TERMINAL_SNAPSHOT_HOST_CANCELLATION_CANARY_FILE";
const HOST_CANCELLATION_TEST_NAME: &str = "pty::terminal_snapshot::acceptance_tests::host_timeout_cancellation_claims_only_the_unaccepted_request";
const SCANNER_SHUTDOWN_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_SCANNER_SHUTDOWN_CHILD";
const SCANNER_SHUTDOWN_TEST_NAME: &str =
    "pty::terminal_snapshot::acceptance_tests::scanner_app_shutdown_owns_composed_host_phases";
const BODY_DISCONNECT_SENTINEL: &str = "ACSNAP_BODY_DISCONNECT_1173_C6Q4";
const LATE_BLOCKING_PANIC_SENTINEL: &str = "ACSNAP_LATE_BLOCKING_PANIC_1173_R4M8";
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
const HOST_FINALIZER_PANIC_SENTINEL: &str = "ACSNAP_HOST_FINALIZER_PANIC_1173_V6K2";
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
            .register_session_for_test(id, crate::session::profile::IdleTuning::DEFAULT, 4, 40)
            .expect("register fixture session");
        let token = self
            .fanout
            .registration_token_for_session(id)
            .expect("fixture token");
        self.fanout
            .handle_output(&token, &id.to_string(), output.to_vec());
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
            r#"{"agents":["../_agent_worker","../_agent_member-live","../_agent_member-exited","../_agent_member-tampered","../_agent_coordinator"],"coordinator":"../_agent_coordinator"}"#,
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
    app: Option<tauri::App>,
    snapshot_state: Arc<TerminalSnapshotState>,
    settings: crate::config::settings::SettingsState,
    settings_path: PathBuf,
    registry_path: PathBuf,
    message_store: Option<Arc<crate::api::message_store::MessageStore>>,
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
    // Declared last so it drops after every other handle: on Windows a
    // temporary directory cannot be removed while the SQLite/WAL connection
    // of the API message store is still open.
    _temporary: tempfile::TempDir,
}

impl Drop for AcceptanceFixture {
    fn drop(&mut self) {
        // Tear down the app and the message store before the temporary
        // directory is removed. The app's managed state and every `ApiState`
        // clone the store, and a built-but-never-run tauri test app retains
        // its managed state after drop, so the store must explicitly close
        // its SQLite/WAL handles; on Windows a directory cannot be removed
        // while that connection is open.
        self.app.take();
        if let Some(store) = self.message_store.take() {
            store.close_for_test();
        }
    }
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
        let api_coordinator_token = mint_fixture_token(
            &token_manager,
            api_coordinator.id,
            paths.coordinator.to_string_lossy().as_ref(),
            "coordinator",
        )
        .await;
        let api_worker_token = mint_fixture_token(
            &token_manager,
            api_worker.id,
            paths.worker.to_string_lossy().as_ref(),
            "worker",
        )
        .await;
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
        let app = crate::test_support::test_builder()
            .manage(Arc::clone(&snapshot_state))
            .manage(settings.clone())
            .manage(Arc::clone(&session_manager))
            .manage(Arc::clone(&pty_manager))
            .manage(restore)
            .manage(purge)
            .manage(crate::api::message_store::MessageStoreState::ready(
                Arc::clone(&message_store),
            ))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("terminal snapshot acceptance app");

        Self {
            app: Some(app),
            snapshot_state,
            settings,
            settings_path,
            registry_path,
            message_store: Some(message_store),
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
            _temporary: temporary,
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
async fn mint_fixture_token(
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

async fn submit_host_request_expect_no_response(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApiLifecycleCounts {
    ingress_available: usize,
    requester_in_flight: usize,
    target_in_flight: usize,
    global_in_flight: usize,
}

fn api_lifecycle_counts(state: &TerminalSnapshotState) -> ApiLifecycleCounts {
    let limiter = state.limiter.lock().expect("snapshot limiter state");
    ApiLifecycleCounts {
        ingress_available: state.ingress.available_permits(),
        requester_in_flight: limiter.requester_in_flight.values().copied().sum(),
        target_in_flight: limiter.target_in_flight.values().copied().sum(),
        global_in_flight: limiter.global_in_flight,
    }
}

fn assert_api_lifecycle_active(counts: ApiLifecycleCounts, target_promoted: bool) {
    assert_eq!(counts.ingress_available, SNAPSHOT_INGRESS_LIMIT);
    assert_eq!(counts.requester_in_flight, 1);
    assert_eq!(counts.target_in_flight, usize::from(target_promoted));
    assert_eq!(counts.global_in_flight, 1);
}

fn assert_api_lifecycle_idle(state: &TerminalSnapshotState) {
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

fn install_host_cancellation_barrier(
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

fn wait_for_host_cancellation_barrier(receiver: &std::sync::mpsc::Receiver<()>) {
    receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("host cancellation barrier was not reached");
}

fn host_cancellation_marker(
    request_directory: &Path,
    request: &crate::phone::terminal_snapshot::HostTerminalSnapshotRequest,
) -> PathBuf {
    request_directory.join(format!(
        ".{}.{}.terminal-snapshot-cancelled",
        request.request_id, request.nonce
    ))
}

fn consume_host_response(
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
    #[cfg(not(unix))]
    fixture.snapshot_state.untrack_artifact(&identity);
    bytes
}

fn assert_no_api_test_hooks(state: &TerminalSnapshotState) {
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

struct PreparedHostFinalizer {
    request_id: String,
    selected_session_id: Uuid,
    response_bytes: Vec<u8>,
    finalization: TerminalSnapshotFinalization,
}

async fn prepare_host_finalizer(
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
enum HostFinalizerHandoff {
    Success,
    Failure(TerminalSnapshotReasonCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostFinalizerWorkerOutcome {
    Returned(Result<(), TerminalSnapshotReasonCode>),
    Panicked,
}

struct HostPublicationAuthorityDrop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for HostPublicationAuthorityDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct RunningHostFinalizer {
    control: Arc<TerminalSnapshotHostFinalizerControl>,
    handle: Option<tokio::task::JoinHandle<()>>,
    outcome: tokio::sync::oneshot::Receiver<HostFinalizerWorkerOutcome>,
    authority_dropped: Arc<std::sync::atomic::AtomicBool>,
    handoffs: Arc<Mutex<Vec<HostFinalizerHandoff>>>,
    response_bytes: usize,
}

fn host_response_digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

fn start_host_finalizer(
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
    fn wait_until_blocked_and_detach(&mut self) {
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

    async fn finish(self) -> (HostFinalizerWorkerOutcome, Vec<HostFinalizerHandoff>) {
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

fn assert_host_finalizer_audit(
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

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_mounted_router_valid_auth_percent_ff_is_canonical_400() {
    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();

    for raw_id in ["%FF", "%30"] {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) => panic!("test API listener must bind: {error}"),
        };
        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("test API listener must expose its address: {error}"),
        };
        let router = crate::api::build_router(direct_api_state(&fixture));
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await;
        });
        let response = match reqwest::Client::new()
            .get(format!(
                "http://{address}/api/v1/windows/{raw_id}/screenshot"
            ))
            .bearer_auth(&token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => panic!("screenshot request must complete: {error}"),
        };
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => panic!("error response body must be readable: {error}"),
        };
        assert!(body.contains("invalid_window_id"));
        server.abort();
        let _ = server.await;
    }
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_mounted_router_invalid_auth_percent_ff_precedes_decode() {
    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let router = crate::api::build_router(direct_api_state(&fixture));
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    let response = match reqwest::Client::new()
        .get(format!("http://{address}/api/v1/windows/%FF/screenshot"))
        .bearer_auth("invalid-token")
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("screenshot request must complete: {error}"),
    };
    assert!(response.status().is_client_error());
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => panic!("error response body must be readable: {error}"),
    };
    assert!(!body.contains("invalid_window_id"));
    let structurally_nonmatching = match reqwest::Client::new()
        .get(format!(
            "http://{address}/api/v1/windows/123/screenshot/extra"
        ))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("nonmatching screenshot request must complete: {error}"),
    };
    assert_eq!(
        structurally_nonmatching.status(),
        axum::http::StatusCode::NOT_FOUND
    );
    server.abort();
    let _ = server.await;
}

#[cfg(target_os = "windows")]
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires an unlocked interactive Windows desktop and visible Notepad"]
async fn authenticated_live_window_png_capture() {
    fn png_artifact_paths(root: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
        fn collect(
            root: &std::path::Path,
            paths: &mut Vec<std::path::PathBuf>,
        ) -> Result<(), String> {
            let entries = std::fs::read_dir(root).map_err(|error| {
                format!(
                    "failed to enumerate screenshot artifact directory {}: {error}",
                    root.display()
                )
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!(
                        "failed to enumerate an entry in screenshot artifact directory {}: {error}",
                        root.display()
                    )
                })?;
                let path = entry.path();
                let metadata = std::fs::metadata(&path).map_err(|error| {
                    format!(
                        "failed to inspect screenshot artifact candidate {}: {error}",
                        path.display()
                    )
                })?;
                if metadata.is_dir() {
                    collect(&path, paths)?;
                } else if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
                {
                    paths.push(path);
                }
            }
            Ok(())
        }

        let mut paths = Vec::new();
        collect(root, &mut paths)?;
        paths.sort();
        Ok(paths)
    }

    fn native_window_ids() -> std::collections::BTreeSet<String> {
        let windows = match xcap::Window::all() {
            Ok(windows) => windows,
            Err(error) => panic!("interactive window snapshot must succeed: {error}"),
        };
        windows
            .into_iter()
            .map(|window| {
                let pid = window.pid();
                let title = window.title();
                let app_name = window.app_name();
                match window.id() {
                    Ok(id) => id.to_string(),
                    Err(error) => panic!(
                        "interactive window ID snapshot must succeed for every window; \
                         context pid={pid:?}, title={title:?}, app_name={app_name:?}: {error}"
                    ),
                }
            })
            .collect()
    }

    struct ChildGuard(std::process::Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    async fn start_live_capture_request(
        address: std::net::SocketAddr,
        token: &str,
        window_id: &str,
    ) -> tokio::net::TcpStream {
        use tokio::io::AsyncWriteExt;

        let mut stream = match tokio::net::TcpStream::connect(address).await {
            Ok(stream) => stream,
            Err(error) => panic!("interactive detached client must connect: {error}"),
        };
        let request = format!(
            "GET /api/v1/windows/{window_id}/screenshot HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: keep-alive\r\n\r\n"
        );
        if let Err(error) = stream.write_all(request.as_bytes()).await {
            panic!("interactive detached request must write: {error}");
        }
        if let Err(error) = stream.flush().await {
            panic!("interactive detached request must flush: {error}");
        }
        stream
    }

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-live-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let artifact_root = temporary.path().to_path_buf();
    let screenshot_artifacts_before = match png_artifact_paths(&artifact_root) {
        Ok(paths) => paths,
        Err(error) => panic!("interactive artifact snapshot before capture must succeed: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let mut notepad = match std::process::Command::new("notepad.exe").spawn() {
        Ok(child) => ChildGuard(child),
        Err(error) => panic!("Notepad must start for the interactive capture proof: {error}"),
    };
    let notepad_pid = notepad.0.id();

    let mut target_window_id = None;
    let mut last_snapshot_window_count = 0;
    let mut matching_process_windows = 0;
    let mut matching_notepad_title_windows = 0;
    let mut matching_notepad_app_name_windows = 0;
    let mut matching_process_zero_ids = 0;
    let mut matching_process_id_errors = 0;
    for _ in 0..40 {
        let windows = match xcap::Window::all() {
            Ok(windows) => windows,
            Err(error) => panic!("interactive window enumeration must succeed: {error}"),
        };
        last_snapshot_window_count = windows.len();
        for window in windows {
            let belongs_to_notepad = match window.pid() {
                Ok(pid) => pid == notepad_pid,
                Err(_) => false,
            };
            let has_notepad_title = match window.title() {
                Ok(title) => title.to_ascii_lowercase().contains("notepad"),
                Err(_) => false,
            };
            let has_notepad_app_name = match window.app_name() {
                Ok(app_name) => app_name.to_ascii_lowercase().contains("notepad"),
                Err(_) => false,
            };
            if belongs_to_notepad || has_notepad_title || has_notepad_app_name {
                if has_notepad_title {
                    matching_notepad_title_windows += 1;
                }
                if has_notepad_app_name {
                    matching_notepad_app_name_windows += 1;
                }
                matching_process_windows += 1;
                target_window_id = match window.id() {
                    Ok(id) if id != 0 => Some(id.to_string()),
                    Ok(_) => {
                        matching_process_zero_ids += 1;
                        None
                    }
                    Err(_) => {
                        matching_process_id_errors += 1;
                        None
                    }
                };
                if target_window_id.is_some() {
                    break;
                }
            }
        }
        if target_window_id.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let target_window_id = match target_window_id {
        Some(window_id) => window_id,
        None => {
            let child_exited = match notepad.0.try_wait() {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(_) => false,
            };
            panic!(
                "Notepad did not expose a nonzero native window ID: last snapshot had \
                 {last_snapshot_window_count} windows, {matching_process_windows} matched the \
                 child process, title, or app name, {matching_notepad_title_windows} matched the \
                 title, {matching_notepad_app_name_windows} matched the app name, \
                 {matching_process_zero_ids} had zero IDs, {matching_process_id_errors} ID \
                 inspections failed, and the launched child exited: {child_exited}"
            )
        }
    };
    let visible_window_ids_before = native_window_ids();

    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let state = direct_api_state(&fixture);
    let router = crate::api::build_router(state.clone());
    let _server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));

    let client = reqwest::Client::new();
    let response = match client
        .get(format!(
            "http://{address}/api/v1/windows/{target_window_id}/screenshot"
        ))
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("interactive screenshot request must complete: {error}"),
    };
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let png = match response.bytes().await {
        Ok(png) => png,
        Err(error) => panic!("interactive PNG response body must be readable: {error}"),
    };
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let screenshot_artifacts_after_initial_capture = match png_artifact_paths(&artifact_root) {
        Ok(paths) => paths,
        Err(error) => {
            panic!("interactive artifact snapshot after initial capture must succeed: {error}")
        }
    };
    assert_eq!(
        screenshot_artifacts_after_initial_capture, screenshot_artifacts_before,
        "native window capture must not persist a PNG artifact"
    );
    assert_eq!(
        native_window_ids(),
        visible_window_ids_before,
        "native window capture must not create an overlay window"
    );

    let current_window_ids = native_window_ids();
    let absent_window_id = ["18446744073709551615", "18446744073709551614"]
        .into_iter()
        .find(|candidate| !current_window_ids.contains(*candidate));
    let absent_window_id = match absent_window_id {
        Some(window_id) => window_id,
        None => panic!("interactive absence snapshot unexpectedly contained both sentinel IDs"),
    };
    let absent_response = match client
        .get(format!(
            "http://{address}/api/v1/windows/{absent_window_id}/screenshot"
        ))
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("absent-window screenshot request must complete: {error}"),
    };
    assert_eq!(absent_response.status(), axum::http::StatusCode::NOT_FOUND);
    let absent_body = match absent_response.bytes().await {
        Ok(body) => body,
        Err(error) => panic!("absent-window response body must be readable: {error}"),
    };
    assert!(!absent_body.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(
        absent_body.len() < 4096,
        "an absent window must return a small JSON error, not a monitor-sized image"
    );

    // Drive the real native worker through three admitted requests. The first
    // connection is closed while it is live, so its worker must retain the
    // lease independently of a client response body.
    let mut detached_client = start_live_capture_request(address, &token, &target_window_id).await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let queued_client_one = start_live_capture_request(address, &token, &target_window_id).await;
    let queued_client_two = start_live_capture_request(address, &token, &target_window_id).await;
    let full_admission_observed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if state.window_screenshot_limiter.try_admit().is_err() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    if full_admission_observed.is_err() {
        let observed_limiter_state = match state.window_screenshot_limiter.try_admit() {
            Ok(admission) => {
                drop(admission);
                "admission capacity remained available"
            }
            Err(_) => "admission capacity was full",
        };
        panic!(
            "interactive detached-worker setup did not fill the limiter within five seconds; \
             observed limiter state: {observed_limiter_state}"
        );
    }

    let mut delivered_byte = [0_u8; 1];
    let delivered_before_close = match detached_client.try_read(&mut delivered_byte) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
        Err(error) => panic!("interactive detached client must inspect unread bytes: {error}"),
    };
    assert_eq!(
        delivered_before_close, 0,
        "the manually closed client must receive no response bytes before the native worker finishes"
    );
    {
        use tokio::io::AsyncWriteExt;

        if let Err(error) = detached_client.shutdown().await {
            panic!("interactive detached client must close its connection: {error}");
        }
    }
    drop(detached_client);

    let busy_response = match client
        .get(format!(
            "http://{address}/api/v1/windows/{target_window_id}/screenshot"
        ))
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("fourth interactive screenshot request must complete: {error}"),
    };
    assert_eq!(
        busy_response.status(),
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "a fourth request must be refused while the detached native worker owns its lease"
    );
    let busy_body = match busy_response.bytes().await {
        Ok(body) => body,
        Err(error) => panic!("interactive capture-busy body must be readable: {error}"),
    };
    assert!(
        String::from_utf8_lossy(&busy_body).contains("capture_busy"),
        "the detached-worker refusal must use the typed capture_busy envelope"
    );

    let released_admission = match tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match state.window_screenshot_limiter.try_admit() {
                Ok(admission) => break admission,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
    })
    .await
    {
        Ok(admission) => admission,
        Err(_) => {
            let observed_limiter_state = match state.window_screenshot_limiter.try_admit() {
                Ok(admission) => {
                    drop(admission);
                    "admission capacity became available after the timeout"
                }
                Err(_) => "admission capacity remained full",
            };
            panic!(
                "detached native worker did not release an admission permit within ten seconds; \
                 observed limiter state: {observed_limiter_state}"
            );
        }
    };
    drop(released_admission);

    let post_release_response = match client
        .get(format!(
            "http://{address}/api/v1/windows/{target_window_id}/screenshot"
        ))
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("post-release interactive screenshot request must complete: {error}"),
    };
    assert_eq!(
        post_release_response.status(),
        axum::http::StatusCode::OK,
        "a request must be admitted after the detached native worker releases its lease"
    );
    let post_release_png = match post_release_response.bytes().await {
        Ok(png) => png,
        Err(error) => panic!("post-release interactive PNG must be readable: {error}"),
    };
    assert!(
        post_release_png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "the post-release request must receive its own PNG response rather than a retained shared result"
    );
    drop(queued_client_one);
    drop(queued_client_two);

    let screenshot_artifacts_after_detached_capture = match png_artifact_paths(&artifact_root) {
        Ok(paths) => paths,
        Err(error) => {
            panic!("interactive artifact snapshot after detached capture must succeed: {error}")
        }
    };
    assert_eq!(
        screenshot_artifacts_after_detached_capture, screenshot_artifacts_before,
        "a detached native capture must not persist a PNG artifact"
    );
    assert_eq!(
        native_window_ids(),
        visible_window_ids_before,
        "a detached native capture must not create an overlay window"
    );
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_route_local_factory_success_is_raw_png_and_audited() {
    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let state = direct_api_state(&fixture);
    let expected_png = b"\x89PNG\r\n\x1a\nroute-local-factory".to_vec();
    let factory_png = expected_png.clone();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture_factory = std::sync::Arc::new({
        let calls = std::sync::Arc::clone(&calls);
        move |window_id, _lease| -> crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest {
            let calls = std::sync::Arc::clone(&calls);
            let expected_png = factory_png.clone();
            Box::pin(async move {
                let mut calls = match calls.lock() {
                    Ok(calls) => calls,
                    Err(poisoned) => poisoned.into_inner(),
                };
                calls.push(window_id);
                Ok(expected_png)
            })
        }
    });
    let router = axum::Router::new()
        .route(
            crate::api::handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
            crate::api::handlers::window_screenshot::mount_window_screenshot_route_for_test(
                capture_factory,
            ),
        )
        .with_state(state.clone());

    let _audit_lock = crate::api::audit::lock_window_screenshot_audits_for_test();
    let _ = crate::api::audit::take_window_screenshot_audits_for_test();
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let _server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));
    let response = match reqwest::Client::new()
        .get(format!("http://{address}/api/v1/windows/1/screenshot"))
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("screenshot route request must complete: {error}"),
    };

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    let expected_content_length = expected_png.len().to_string();
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some(expected_content_length.as_str())
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => panic!("screenshot response body must be readable: {error}"),
    };
    assert_eq!(body.as_ref(), expected_png.as_slice());
    assert_eq!(
        crate::api::audit::take_window_screenshot_audits_for_test(),
        vec![crate::api::audit::WindowScreenshotAuditStatus::Succeeded]
    );
    {
        let calls = match calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert_eq!(calls.as_slice(), ["1"]);
    }

    let admission = match state.window_screenshot_limiter.try_admit() {
        Ok(admission) => admission,
        Err(error) => panic!("success must release its admission permit: {error:?}"),
    };
    let active = match state.window_screenshot_limiter.acquire_active().await {
        Ok(active) => active,
        Err(error) => panic!("success must release its active permit: {error:?}"),
    };
    drop(active);
    drop(admission);
}

#[cfg(target_os = "windows")]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn window_screenshot_revocation_after_launch_does_not_cancel_in_flight_capture() {
    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let client_id = fixture.api_coordinator_token.client_id.clone();
    let state = direct_api_state(&fixture);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let launched = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let capture_factory = std::sync::Arc::new({
        let calls = std::sync::Arc::clone(&calls);
        let launched = std::sync::Arc::clone(&launched);
        let release = std::sync::Arc::clone(&release);
        move |window_id, lease| -> crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest {
            let calls = std::sync::Arc::clone(&calls);
            let launched = std::sync::Arc::clone(&launched);
            let release = std::sync::Arc::clone(&release);
            Box::pin(async move {
                {
                    let mut calls = match calls.lock() {
                        Ok(calls) => calls,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    calls.push(window_id);
                }
                launched.notify_one();
                release.notified().await;
                drop(lease);
                Ok(b"\x89PNG\r\n\x1a\nlaunched".to_vec())
            })
        }
    });
    let router = axum::Router::new()
        .route(
            crate::api::handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
            crate::api::handlers::window_screenshot::mount_window_screenshot_route_for_test(
                capture_factory,
            ),
        )
        .with_state(state);
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let _server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));

    let _audit_lock = crate::api::audit::lock_window_screenshot_audits_for_test();
    let _ = crate::api::audit::take_window_screenshot_audits_for_test();
    let active = tokio::spawn({
        let token = token.clone();
        async move {
            match reqwest::Client::new()
                .get(format!("http://{address}/api/v1/windows/1/screenshot"))
                .bearer_auth(token)
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => panic!("in-flight screenshot request must complete: {error}"),
            }
        }
    });
    launched.notified().await;
    match crate::api::auth::revoke(&fixture.registry_path, &client_id) {
        Ok(true) => {}
        Ok(false) => panic!("fixture credential revocation must revoke the valid client"),
        Err(error) => panic!("fixture credential revocation must succeed: {error}"),
    }
    release.notify_one();
    let active = match active.await {
        Ok(response) => response,
        Err(error) => panic!("in-flight request task must complete: {error}"),
    };
    assert_eq!(active.status(), axum::http::StatusCode::OK);

    let rejected = match reqwest::Client::new()
        .get(format!("http://{address}/api/v1/windows/2/screenshot"))
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("post-revocation screenshot request must complete: {error}"),
    };
    assert!(!rejected.status().is_success());
    assert_eq!(
        crate::api::audit::take_window_screenshot_audits_for_test(),
        vec![crate::api::audit::WindowScreenshotAuditStatus::Succeeded]
    );
    let calls = match calls.lock() {
        Ok(calls) => calls,
        Err(poisoned) => poisoned.into_inner(),
    };
    assert_eq!(calls.as_slice(), ["1"]);
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_route_local_factory_result_mappings_are_audited_and_released() {
    #[derive(Clone, Copy)]
    enum Outcome {
        NotFound,
        TooLarge,
        Unavailable,
    }

    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    let _audit_lock = crate::api::audit::lock_window_screenshot_audits_for_test();
    let cases = [
        (
            Outcome::NotFound,
            axum::http::StatusCode::NOT_FOUND,
            "window_not_found",
            crate::api::audit::WindowScreenshotAuditStatus::WindowNotFound,
        ),
        (
            Outcome::TooLarge,
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "capture_too_large",
            crate::api::audit::WindowScreenshotAuditStatus::CaptureTooLarge,
        ),
        (
            Outcome::Unavailable,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "capture_unavailable",
            crate::api::audit::WindowScreenshotAuditStatus::CaptureUnavailable,
        ),
    ];

    for (outcome, expected_status, expected_code, expected_audit) in cases {
        let temporary = match tempfile::Builder::new()
            .prefix("window-screenshot-fixture-")
            .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
        {
            Ok(temporary) => temporary,
            Err(error) => panic!("fixture temporary directory must be created: {error}"),
        };
        let fixture = AcceptanceFixture::new(temporary).await;
        let token = fixture.api_coordinator_token.secret.clone();
        let state = direct_api_state(&fixture);
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture_factory = std::sync::Arc::new({
            let calls = std::sync::Arc::clone(&calls);
            move |window_id, _lease| -> crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest {
                let calls = std::sync::Arc::clone(&calls);
                Box::pin(async move {
                    let mut calls = match calls.lock() {
                        Ok(calls) => calls,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    calls.push(window_id);
                    match outcome {
                        Outcome::NotFound => {
                            Err(crate::screenshot::WindowScreenshotCaptureError::NotFound)
                        }
                        Outcome::TooLarge => {
                            Err(crate::screenshot::WindowScreenshotCaptureError::TooLarge)
                        }
                        Outcome::Unavailable => {
                            Err(crate::screenshot::WindowScreenshotCaptureError::Unavailable)
                        }
                    }
                })
            }
        });
        let router = axum::Router::new()
            .route(
                crate::api::handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
                crate::api::handlers::window_screenshot::mount_window_screenshot_route_for_test(
                    capture_factory,
                ),
            )
            .with_state(state.clone());
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) => panic!("test API listener must bind: {error}"),
        };
        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("test API listener must expose its address: {error}"),
        };
        let _server = ServerGuard(tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await;
        }));

        let _ = crate::api::audit::take_window_screenshot_audits_for_test();
        let response = match reqwest::Client::new()
            .get(format!("http://{address}/api/v1/windows/1/screenshot"))
            .bearer_auth(&token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => panic!("screenshot route request must complete: {error}"),
        };
        assert_eq!(response.status(), expected_status);
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => panic!("screenshot error body must be readable: {error}"),
        };
        assert!(body.contains(expected_code));
        assert!(!body.contains("native diagnostic text must remain internal"));
        assert_eq!(
            crate::api::audit::take_window_screenshot_audits_for_test(),
            vec![expected_audit]
        );
        {
            let calls = match calls.lock() {
                Ok(calls) => calls,
                Err(poisoned) => poisoned.into_inner(),
            };
            assert_eq!(calls.as_slice(), ["1"]);
        }

        let admission = match state.window_screenshot_limiter.try_admit() {
            Ok(admission) => admission,
            Err(error) => panic!("error response must release admission: {error:?}"),
        };
        let active = match state.window_screenshot_limiter.acquire_active().await {
            Ok(active) => active,
            Err(error) => panic!("error response must release active slot: {error:?}"),
        };
        drop(active);
        drop(admission);
    }
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_full_flow_authenticated_raw_path_validation_is_uncaptured() {
    use axum::response::IntoResponse;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let state = direct_api_state(&fixture);
    let factory_calls = std::sync::Arc::new(AtomicUsize::new(0));
    let _audit_capture = crate::api::audit::lock_window_screenshot_audits_for_test();

    // Occupy every admission permit before the table. A malformed raw ID must
    // still produce its authenticated 400 while no permit can be acquired.
    let mut held_admissions = Vec::new();
    while let Ok(admission) = state.window_screenshot_limiter.try_admit() {
        held_admissions.push(admission);
    }
    assert!(
        !held_admissions.is_empty(),
        "the screenshot limiter must expose at least one admission permit"
    );

    for (case, raw_path) in [
        ("empty", "/api/v1/windows//screenshot"),
        ("non_digit", "/api/v1/windows/not-a-number/screenshot"),
        ("signed", "/api/v1/windows/-1/screenshot"),
        ("whitespace_padded", "/api/v1/windows/%201%20/screenshot"),
        ("leading_zero", "/api/v1/windows/01/screenshot"),
        (
            "twenty_one_digits",
            "/api/v1/windows/100000000000000000000/screenshot",
        ),
        (
            "u64_overflow",
            "/api/v1/windows/18446744073709551616/screenshot",
        ),
    ] {
        let mut headers = axum::http::HeaderMap::new();
        let authorization = match axum::http::HeaderValue::from_str(&format!("Bearer {token}")) {
            Ok(authorization) => authorization,
            Err(error) => panic!("valid screenshot authorization header must build: {error}"),
        };
        headers.insert(axum::http::header::AUTHORIZATION, authorization);
        let uri = match axum::http::Uri::from_str(raw_path) {
            Ok(uri) => uri,
            Err(error) => panic!("{case} raw screenshot URI must parse: {error}"),
        };
        let factory_calls_for_case = std::sync::Arc::clone(&factory_calls);
        let response = match crate::api::handlers::window_screenshot::get_with_capture_for_test(
            state.clone(),
            headers,
            match "127.0.0.1:49152".parse() {
                Ok(address) => address,
                Err(error) => panic!("test client address must parse: {error}"),
            },
            axum::extract::OriginalUri(uri),
            move |_window_id, _lease| {
                factory_calls_for_case.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    panic!("an invalid raw window ID must never invoke the capture factory")
                })
            },
        )
        .await
        {
            Ok(_) => panic!("{case} raw window ID unexpectedly reached a capture response"),
            Err(error) => error.into_response(),
        };
        assert_eq!(
            response.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "{case} must remain an authenticated invalid-window-ID response"
        );
        let body = match axum::body::to_bytes(response.into_body(), 4096).await {
            Ok(body) => body,
            Err(error) => panic!("{case} invalid-window-ID body must be readable: {error}"),
        };
        assert!(
            String::from_utf8_lossy(&body).contains("invalid_window_id"),
            "{case} must use the typed invalid_window_id envelope"
        );
        assert_eq!(
            crate::api::audit::take_window_screenshot_audits_for_test(),
            vec![crate::api::audit::WindowScreenshotAuditStatus::InvalidWindowId],
            "{case} must record exactly one final redacted invalid-ID audit"
        );
        assert_eq!(
            factory_calls.load(Ordering::SeqCst),
            0,
            "{case} must not invoke the capture factory"
        );
        assert!(
            state.window_screenshot_limiter.try_admit().is_err(),
            "{case} must not acquire an admission permit before raw-ID validation"
        );
    }

    drop(held_admissions);
    let released_admission = match state.window_screenshot_limiter.try_admit() {
        Ok(admission) => admission,
        Err(error) => panic!("all table-held admissions must release: {error:?}"),
    };
    drop(released_admission);
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_route_local_factory_auth_and_raw_path_order_is_audited() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    async fn raw_request(address: std::net::SocketAddr, token: &str, path: &str) -> String {
        let mut stream = match tokio::net::TcpStream::connect(address).await {
            Ok(stream) => stream,
            Err(error) => panic!("raw screenshot test client must connect: {error}"),
        };
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        );
        if let Err(error) = stream.write_all(request.as_bytes()).await {
            panic!("raw screenshot test request must write: {error}");
        }
        let mut response = Vec::new();
        if let Err(error) = stream.read_to_end(&mut response).await {
            panic!("raw screenshot test response must read: {error}");
        }
        match String::from_utf8(response) {
            Ok(response) => response,
            Err(error) => panic!("raw screenshot response must be UTF-8 JSON: {error}"),
        }
    }

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let state = direct_api_state(&fixture);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture_factory = std::sync::Arc::new({
        let calls = std::sync::Arc::clone(&calls);
        move |window_id, _lease| -> crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest {
            let calls = std::sync::Arc::clone(&calls);
            Box::pin(async move {
                let mut calls = match calls.lock() {
                    Ok(calls) => calls,
                    Err(poisoned) => poisoned.into_inner(),
                };
                calls.push(window_id);
                Ok(b"not reached".to_vec())
            })
        }
    });
    let router = axum::Router::new()
        .route(
            crate::api::handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
            crate::api::handlers::window_screenshot::mount_window_screenshot_route_for_test(
                capture_factory,
            ),
        )
        .with_state(state.clone());
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let _server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));

    let _audit_lock = crate::api::audit::lock_window_screenshot_audits_for_test();
    let _ = crate::api::audit::take_window_screenshot_audits_for_test();
    let percent_ff = raw_request(address, &token, "/api/v1/windows/%FF/screenshot").await;
    assert!(percent_ff.starts_with("HTTP/1.1 400"));
    assert!(percent_ff.contains("invalid_window_id"));
    assert_eq!(
        crate::api::audit::take_window_screenshot_audits_for_test(),
        vec![crate::api::audit::WindowScreenshotAuditStatus::InvalidWindowId]
    );

    let percent_zero = raw_request(address, &token, "/api/v1/windows/%30/screenshot").await;
    assert!(percent_zero.starts_with("HTTP/1.1 400"));
    assert!(percent_zero.contains("invalid_window_id"));
    assert_eq!(
        crate::api::audit::take_window_screenshot_audits_for_test(),
        vec![crate::api::audit::WindowScreenshotAuditStatus::InvalidWindowId]
    );

    let invalid_auth =
        raw_request(address, "invalid-token", "/api/v1/windows/%FF/screenshot").await;
    assert!(!invalid_auth.starts_with("HTTP/1.1 400"));
    assert!(!invalid_auth.contains("invalid_window_id"));
    assert!(crate::api::audit::take_window_screenshot_audits_for_test().is_empty());

    let structural_miss = raw_request(address, &token, "/api/v1/windows/1/screenshot/extra").await;
    assert!(structural_miss.starts_with("HTTP/1.1 404"));
    assert!(crate::api::audit::take_window_screenshot_audits_for_test().is_empty());
    let calls = match calls.lock() {
        Ok(calls) => calls,
        Err(poisoned) => poisoned.into_inner(),
    };
    assert!(calls.is_empty());
    drop(calls);

    let admission_one = match state.window_screenshot_limiter.try_admit() {
        Ok(admission) => admission,
        Err(error) => panic!("raw validation must leave admission capacity free: {error:?}"),
    };
    let admission_two = match state.window_screenshot_limiter.try_admit() {
        Ok(admission) => admission,
        Err(error) => panic!("raw validation must leave admission capacity free: {error:?}"),
    };
    let admission_three = match state.window_screenshot_limiter.try_admit() {
        Ok(admission) => admission,
        Err(error) => panic!("raw validation must leave admission capacity free: {error:?}"),
    };
    assert!(state.window_screenshot_limiter.try_admit().is_err());
    drop(admission_three);
    drop(admission_two);
    drop(admission_one);
}

#[cfg(target_os = "windows")]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn window_screenshot_route_local_factory_queue_is_bounded_and_disconnect_retains_lease() {
    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    async fn send_window_request(
        client: reqwest::Client,
        address: std::net::SocketAddr,
        token: String,
        window_id: &'static str,
    ) -> reqwest::Response {
        match client
            .get(format!(
                "http://{address}/api/v1/windows/{window_id}/screenshot"
            ))
            .bearer_auth(token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => panic!("gated screenshot request must complete: {error}"),
        }
    }

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let state = direct_api_state(&fixture);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let active_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_active = std::sync::Arc::new(tokio::sync::Notify::new());
    let capture_factory = std::sync::Arc::new({
        let calls = std::sync::Arc::clone(&calls);
        let active_started = std::sync::Arc::clone(&active_started);
        let release_active = std::sync::Arc::clone(&release_active);
        move |window_id, lease| -> crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest {
            let calls = std::sync::Arc::clone(&calls);
            let active_started = std::sync::Arc::clone(&active_started);
            let release_active = std::sync::Arc::clone(&release_active);
            Box::pin(async move {
                let is_first = {
                    let mut calls = match calls.lock() {
                        Ok(calls) => calls,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    let is_first = window_id == "1";
                    calls.push(window_id);
                    is_first
                };

                if is_first {
                    let worker = tokio::spawn(async move {
                        let _lease = lease;
                        active_started.notify_one();
                        release_active.notified().await;
                        Ok::<Vec<u8>, crate::screenshot::WindowScreenshotCaptureError>(
                            b"\x89PNG\r\n\x1a\ngated".to_vec(),
                        )
                    });
                    match worker.await {
                        Ok(result) => result,
                        Err(_) => Err(crate::screenshot::WindowScreenshotCaptureError::Unavailable),
                    }
                } else {
                    drop(lease);
                    Ok(b"\x89PNG\r\n\x1a\nqueued".to_vec())
                }
            })
        }
    });
    let router = axum::Router::new()
        .route(
            crate::api::handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
            crate::api::handlers::window_screenshot::mount_window_screenshot_route_for_test(
                capture_factory,
            ),
        )
        .with_state(state.clone());
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let _server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));

    let _audit_lock = crate::api::audit::lock_window_screenshot_audits_for_test();
    let _ = crate::api::audit::take_window_screenshot_audits_for_test();
    let active_request = tokio::spawn(send_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "1",
    ));
    active_started.notified().await;
    let queued_one = tokio::spawn(send_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "2",
    ));
    let queued_two = tokio::spawn(send_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "3",
    ));

    let mut queue_is_full = false;
    for _ in 0..50 {
        match state.window_screenshot_limiter.try_admit() {
            Ok(permit) => drop(permit),
            Err(_) => {
                queue_is_full = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        queue_is_full,
        "two requests must retain queued admission permits"
    );

    let fourth = send_window_request(reqwest::Client::new(), address, token.clone(), "4").await;
    assert_eq!(fourth.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        crate::api::audit::take_window_screenshot_audits_for_test(),
        vec![crate::api::audit::WindowScreenshotAuditStatus::CaptureBusy]
    );
    active_request.abort();
    let _ = active_request.await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let still_busy = send_window_request(reqwest::Client::new(), address, token.clone(), "5").await;
    assert_eq!(
        still_busy.status(),
        axum::http::StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        crate::api::audit::take_window_screenshot_audits_for_test(),
        vec![crate::api::audit::WindowScreenshotAuditStatus::CaptureBusy]
    );

    release_active.notify_one();
    let queued_one = match queued_one.await {
        Ok(response) => response,
        Err(error) => panic!("first queued request task must complete: {error}"),
    };
    let queued_two = match queued_two.await {
        Ok(response) => response,
        Err(error) => panic!("second queued request task must complete: {error}"),
    };
    assert_eq!(queued_one.status(), axum::http::StatusCode::OK);
    assert_eq!(queued_two.status(), axum::http::StatusCode::OK);

    let after_release = send_window_request(reqwest::Client::new(), address, token, "6").await;
    assert_eq!(after_release.status(), axum::http::StatusCode::OK);
    let calls = match calls.lock() {
        Ok(calls) => calls,
        Err(poisoned) => poisoned.into_inner(),
    };
    assert_eq!(calls.len(), 4);
    assert_eq!(calls.first().map(String::as_str), Some("1"));
    assert_eq!(calls.last().map(String::as_str), Some("6"));
    let mut queued_calls = calls[1..3].to_vec();
    queued_calls.sort();
    assert_eq!(queued_calls, ["2", "3"]);
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_each_request_uses_a_fresh_local_result() {
    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    async fn send_window_request(
        client: reqwest::Client,
        address: std::net::SocketAddr,
        token: String,
    ) -> reqwest::Response {
        match client
            .get(format!("http://{address}/api/v1/windows/1/screenshot"))
            .bearer_auth(token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => panic!("fresh-result screenshot request must complete: {error}"),
        }
    }

    async fn open_detached_request(
        address: std::net::SocketAddr,
        token: &str,
    ) -> tokio::net::TcpStream {
        use tokio::io::AsyncWriteExt;

        let mut stream = match tokio::net::TcpStream::connect(address).await {
            Ok(stream) => stream,
            Err(error) => panic!("detached fresh-result client must connect: {error}"),
        };
        let request = format!(
            "GET /api/v1/windows/1/screenshot HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        );
        if let Err(error) = stream.write_all(request.as_bytes()).await {
            panic!("detached fresh-result request must write: {error}");
        }
        if let Err(error) = stream.flush().await {
            panic!("detached fresh-result request must flush: {error}");
        }
        stream
    }

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let state = direct_api_state(&fixture);
    let result_vectors = [
        b"\x89PNG\r\n\x1a\nfirst-result".to_vec(),
        b"\x89PNG\r\n\x1a\nsecond-result".to_vec(),
        b"\x89PNG\r\n\x1a\nthird-result".to_vec(),
    ];
    let factory_result_vectors = result_vectors.clone();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let second_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_second = std::sync::Arc::new(tokio::sync::Notify::new());
    let second_done = std::sync::Arc::new(tokio::sync::Notify::new());
    let capture_factory = std::sync::Arc::new({
        let calls = std::sync::Arc::clone(&calls);
        let second_started = std::sync::Arc::clone(&second_started);
        let release_second = std::sync::Arc::clone(&release_second);
        let second_done = std::sync::Arc::clone(&second_done);
        move |window_id, lease| -> crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest {
            let calls = std::sync::Arc::clone(&calls);
            let second_started = std::sync::Arc::clone(&second_started);
            let release_second = std::sync::Arc::clone(&release_second);
            let second_done = std::sync::Arc::clone(&second_done);
            let call_index = {
                let mut calls = match calls.lock() {
                    Ok(calls) => calls,
                    Err(poisoned) => poisoned.into_inner(),
                };
                calls.push(window_id);
                calls.len() - 1
            };
            let vector = factory_result_vectors[call_index].clone();
            Box::pin(async move {
                if call_index == 1 {
                    let worker = tokio::spawn(async move {
                        let _lease = lease;
                        second_started.notify_one();
                        release_second.notified().await;
                        second_done.notify_one();
                        Ok::<Vec<u8>, crate::screenshot::WindowScreenshotCaptureError>(vector)
                    });
                    match worker.await {
                        Ok(result) => result,
                        Err(_) => {
                            Err(crate::screenshot::WindowScreenshotCaptureError::Unavailable)
                        }
                    }
                } else {
                    drop(lease);
                    Ok(vector)
                }
            })
        }
    });
    let router = axum::Router::new()
        .route(
            crate::api::handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
            crate::api::handlers::window_screenshot::mount_window_screenshot_route_for_test(
                capture_factory,
            ),
        )
        .with_state(state.clone());
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let _server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));

    let _audit_lock = crate::api::audit::lock_window_screenshot_audits_for_test();
    let _ = crate::api::audit::take_window_screenshot_audits_for_test();
    let first_response = send_window_request(reqwest::Client::new(), address, token.clone()).await;
    assert_eq!(first_response.status(), axum::http::StatusCode::OK);
    let first_png = match first_response.bytes().await {
        Ok(png) => png,
        Err(error) => panic!("first fresh-result PNG must be readable: {error}"),
    };
    assert_eq!(
        first_png.as_ref(),
        result_vectors[0].as_slice(),
        "the first request must receive the first fresh local result"
    );

    let mut detached_client = open_detached_request(address, &token).await;
    second_started.notified().await;
    let mut delivered_byte = [0_u8; 1];
    let delivered_before_disconnect = match detached_client.try_read(&mut delivered_byte) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
        Err(error) => panic!("detached fresh-result client must inspect unread bytes: {error}"),
    };
    assert_eq!(
        delivered_before_disconnect, 0,
        "the gated second request must not receive a response body before its worker is released"
    );
    {
        use tokio::io::AsyncWriteExt;

        if let Err(error) = detached_client.shutdown().await {
            panic!("detached fresh-result client must close its connection: {error}");
        }
    }
    drop(detached_client);
    release_second.notify_one();
    second_done.notified().await;

    let third_response = send_window_request(reqwest::Client::new(), address, token).await;
    assert_eq!(third_response.status(), axum::http::StatusCode::OK);
    let third_png = match third_response.bytes().await {
        Ok(png) => png,
        Err(error) => panic!("third fresh-result PNG must be readable: {error}"),
    };
    assert_eq!(
        third_png.as_ref(),
        result_vectors[2].as_slice(),
        "the third request must receive the third fresh local result, not a retained earlier result"
    );

    let calls = match calls.lock() {
        Ok(calls) => calls,
        Err(poisoned) => poisoned.into_inner(),
    };
    assert_eq!(
        calls.len(),
        3,
        "the factory must be invoked exactly three times"
    );
    assert!(
        calls.iter().all(|window_id| window_id == "1"),
        "all three requests must use the same window ID: {calls:?}"
    );
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_fixture_removes_its_temporary_directory() {
    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let temporary_path = temporary.path().to_path_buf();
    assert!(
        temporary_path.is_dir(),
        "fixture temporary directory must exist before teardown"
    );
    let fixture = AcceptanceFixture::new(temporary).await;
    let state = direct_api_state(&fixture);
    drop(state);
    drop(fixture);
    assert!(
        !temporary_path.exists(),
        "fixture teardown must remove the temporary directory; the app and the SQLite/WAL \
         connection must close before TempDir cleanup: {}",
        temporary_path.display()
    );
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_fresh_guard_is_released_before_queue_wait() {
    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    async fn send_window_request(
        client: reqwest::Client,
        address: std::net::SocketAddr,
        token: String,
        window_id: &'static str,
    ) -> reqwest::Response {
        match client
            .get(format!(
                "http://{address}/api/v1/windows/{window_id}/screenshot"
            ))
            .bearer_auth(token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => panic!("queued screenshot request must complete: {error}"),
        }
    }

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let state = direct_api_state(&fixture);
    let active_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_active = std::sync::Arc::new(tokio::sync::Notify::new());
    let capture_factory = std::sync::Arc::new({
        let active_started = std::sync::Arc::clone(&active_started);
        let release_active = std::sync::Arc::clone(&release_active);
        move |window_id, lease| -> crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest {
            let active_started = std::sync::Arc::clone(&active_started);
            let release_active = std::sync::Arc::clone(&release_active);
            Box::pin(async move {
                if window_id == "1" {
                    active_started.notify_one();
                    release_active.notified().await;
                }
                drop(lease);
                Ok(b"\x89PNG\r\n\x1a\nfreshness".to_vec())
            })
        }
    });
    let router = axum::Router::new()
        .route(
            crate::api::handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
            crate::api::handlers::window_screenshot::mount_window_screenshot_route_for_test(
                capture_factory,
            ),
        )
        .with_state(state.clone());
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let _server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));

    let active = tokio::spawn(send_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "1",
    ));
    active_started.notified().await;
    let queued = tokio::spawn(send_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "2",
    ));

    let admission_probe = match tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match state.window_screenshot_limiter.try_admit() {
                Ok(admission) => {
                    if state.window_screenshot_limiter.try_admit().is_err() {
                        break admission;
                    }
                    drop(admission);
                }
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    {
        Ok(admission) => admission,
        Err(_) => {
            let observed_limiter_state = match state.window_screenshot_limiter.try_admit() {
                Ok(admission) => {
                    drop(admission);
                    "admission capacity was available"
                }
                Err(_) => "admission capacity was full",
            };
            panic!(
                "timed out waiting for the queued screenshot request to fill the limiter; \
                 observed limiter state: {observed_limiter_state}"
            );
        }
    };

    let registry_parent = match fixture.registry_path.parent() {
        Some(parent) => parent.to_path_buf(),
        None => panic!("fixture registry path must have a parent"),
    };
    let registry_lock = match tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            crate::api::auth::hold_registry_lock_for_test(&registry_parent)
        }),
    )
    .await
    {
        Ok(Ok(lock)) => lock,
        Ok(Err(error)) => panic!("registry lock probe task must not panic: {error}"),
        Err(_) => panic!("queued screenshot request retained a freshness registry lock"),
    };
    drop(registry_lock);
    drop(admission_probe);

    release_active.notify_one();
    let active = match active.await {
        Ok(response) => response,
        Err(error) => panic!("active request task must complete: {error}"),
    };
    let queued = match queued.await {
        Ok(response) => response,
        Err(error) => panic!("queued request task must complete: {error}"),
    };
    assert_eq!(active.status(), axum::http::StatusCode::OK);
    assert_eq!(queued.status(), axum::http::StatusCode::OK);

    let fresh_after_probe = send_window_request(reqwest::Client::new(), address, token, "3").await;
    assert_eq!(fresh_after_probe.status(), axum::http::StatusCode::OK);
}

#[cfg(target_os = "windows")]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn window_screenshot_queued_revocation_blocks_launch_revalidation() {
    struct ServerGuard(tokio::task::JoinHandle<()>);

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    async fn send_window_request(
        client: reqwest::Client,
        address: std::net::SocketAddr,
        token: String,
        window_id: &'static str,
    ) -> reqwest::Response {
        match client
            .get(format!(
                "http://{address}/api/v1/windows/{window_id}/screenshot"
            ))
            .bearer_auth(token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => panic!("revocation screenshot request must complete: {error}"),
        }
    }

    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    let fixture = AcceptanceFixture::new(temporary).await;
    let token = fixture.api_coordinator_token.secret.clone();
    let client_id = fixture.api_coordinator_token.client_id.clone();
    let state = direct_api_state(&fixture);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let active_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_active = std::sync::Arc::new(tokio::sync::Notify::new());
    let capture_factory = std::sync::Arc::new({
        let calls = std::sync::Arc::clone(&calls);
        let active_started = std::sync::Arc::clone(&active_started);
        let release_active = std::sync::Arc::clone(&release_active);
        move |window_id, lease| -> crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest {
            let calls = std::sync::Arc::clone(&calls);
            let active_started = std::sync::Arc::clone(&active_started);
            let release_active = std::sync::Arc::clone(&release_active);
            Box::pin(async move {
                let is_first = {
                    let mut calls = match calls.lock() {
                        Ok(calls) => calls,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    let is_first = window_id == "1";
                    calls.push(window_id);
                    is_first
                };
                if is_first {
                    active_started.notify_one();
                    release_active.notified().await;
                }
                drop(lease);
                Ok(b"\x89PNG\r\n\x1a\nrevocation".to_vec())
            })
        }
    });
    let router = axum::Router::new()
        .route(
            crate::api::handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
            crate::api::handlers::window_screenshot::mount_window_screenshot_route_for_test(
                capture_factory,
            ),
        )
        .with_state(state.clone());
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("test API listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("test API listener must expose its address: {error}"),
    };
    let _server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));

    let _audit_lock = crate::api::audit::lock_window_screenshot_audits_for_test();
    let _ = crate::api::audit::take_window_screenshot_audits_for_test();
    let active = tokio::spawn(send_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "1",
    ));
    active_started.notified().await;
    let queued = tokio::spawn(send_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "2",
    ));

    let admission_probe = match tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match state.window_screenshot_limiter.try_admit() {
                Ok(admission) => {
                    if state.window_screenshot_limiter.try_admit().is_err() {
                        break admission;
                    }
                    drop(admission);
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    {
        Ok(admission) => admission,
        Err(_) => {
            let observed_limiter_state = match state.window_screenshot_limiter.try_admit() {
                Ok(admission) => {
                    drop(admission);
                    "admission capacity was available"
                }
                Err(_) => "admission capacity was full",
            };
            panic!(
                "timed out waiting for the queued screenshot request to fill the limiter; \
                 observed limiter state: {observed_limiter_state}"
            );
        }
    };
    match crate::api::auth::revoke(&fixture.registry_path, &client_id) {
        Ok(true) => {}
        Ok(false) => panic!("fixture credential revocation must revoke the valid client"),
        Err(error) => panic!("fixture credential revocation must succeed: {error}"),
    }
    drop(admission_probe);

    release_active.notify_one();
    let active = match active.await {
        Ok(response) => response,
        Err(error) => panic!("active request task must complete: {error}"),
    };
    let queued = match queued.await {
        Ok(response) => response,
        Err(error) => panic!("queued request task must complete: {error}"),
    };
    assert_eq!(active.status(), axum::http::StatusCode::OK);
    assert!(!queued.status().is_success());
    let queued_body = match queued.text().await {
        Ok(body) => body,
        Err(error) => panic!("queued revocation body must be readable: {error}"),
    };
    assert!(!queued_body.contains("window_not_found"));
    assert!(!queued_body.contains("capture_too_large"));
    assert!(!queued_body.contains("capture_unavailable"));
    assert_eq!(
        crate::api::audit::take_window_screenshot_audits_for_test(),
        vec![crate::api::audit::WindowScreenshotAuditStatus::Succeeded]
    );
    let calls = match calls.lock() {
        Ok(calls) => calls,
        Err(poisoned) => poisoned.into_inner(),
    };
    assert_eq!(calls.as_slice(), ["1"]);
    drop(calls);

    let admission = match state.window_screenshot_limiter.try_admit() {
        Ok(admission) => admission,
        Err(error) => panic!("revoked queue request must release admission: {error:?}"),
    };
    let active = match state.window_screenshot_limiter.acquire_active().await {
        Ok(active) => active,
        Err(error) => panic!("revoked queue request must release active slot: {error:?}"),
    };
    drop(active);
    drop(admission);
}

fn direct_api_state(fixture: &AcceptanceFixture) -> crate::api::ApiState {
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

fn direct_api_body(request_id: &str, target: &str) -> axum::body::Body {
    direct_api_body_for_format(request_id, target, TerminalSnapshotFormat::Json)
}

async fn run_direct_api_request(
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

async fn await_api_barrier<T>(
    receiver: tokio::sync::oneshot::Receiver<T>,
    label: &'static str,
) -> T {
    tokio::time::timeout(Duration::from_secs(10), receiver)
        .await
        .expect(label)
        .expect(label)
}

#[derive(Debug, Clone, Copy)]
enum ApiAbortPoint {
    BeforeCapture,
    AfterResponseBytes,
    FinalHandoff,
}

async fn abort_direct_api_request_at(
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
async fn timeout_api_at_common_blocking_stage(
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

struct RetainedMaximumAllocation {
    bytes: Vec<u8>,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for RetainedMaximumAllocation {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

fn common_blocking_test_audit(request_id: Uuid) -> TerminalSnapshotAuditGuard {
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

fn assert_common_blocking_audit_inventory(config: &Path, canaries: &[String]) {
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

fn assert_api_audit_row(
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

struct BodyStreamDropProbe(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for BodyStreamDropProbe {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScannerShutdownPhase {
    BeforeService,
    BlockingService,
    BeforeFinalizer,
    FinalizerOwned,
    CancellationRace,
    ReplacementCleanup,
}

impl ScannerShutdownPhase {
    fn label(self) -> &'static str {
        match self {
            Self::BeforeService => "before-service",
            Self::BlockingService => "blocking-service",
            Self::BeforeFinalizer => "before-finalizer",
            Self::FinalizerOwned => "finalizer-owned",
            Self::CancellationRace => "cancellation-race",
            Self::ReplacementCleanup => "replacement-cleanup",
        }
    }

    fn abortable(self) -> bool {
        matches!(self, Self::BeforeService | Self::ReplacementCleanup)
    }

    fn parse(value: &str) -> Option<Self> {
        [
            Self::BeforeService,
            Self::BlockingService,
            Self::BeforeFinalizer,
            Self::FinalizerOwned,
            Self::CancellationRace,
            Self::ReplacementCleanup,
        ]
        .into_iter()
        .find(|phase| phase.label() == value)
    }
}

fn scanner_processing_path(request_directory: &Path, request_id: &str) -> PathBuf {
    let matches = std::fs::read_dir(request_directory)
        .expect("scanner request directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!(".{request_id}."))
                        && name.ends_with(".terminal-snapshot-processing")
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "one scanner processing owner");
    matches.into_iter().next().expect("scanner processing path")
}

async fn run_composed_scanner_shutdown_phase(temporary_root: &Path, phase: ScannerShutdownPhase) {
    let temporary = tempfile::Builder::new()
        .prefix(&format!("scanner-shutdown-{}-", phase.label()))
        .tempdir_in(temporary_root)
        .expect("scanner shutdown temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("scanner shutdown config directory");
    let _env = ConfigEnvGuard::set(&config);
    let fixture = AcceptanceFixture::new(temporary).await;
    let target = format!("{PROJECT}:{WORKGROUP}/member-live");
    let requester = format!("{PROJECT}:{WORKGROUP}/coordinator");
    let nonce = match phase {
        ScannerShutdownPhase::BeforeService => "1".repeat(64),
        ScannerShutdownPhase::BlockingService => "2".repeat(64),
        ScannerShutdownPhase::BeforeFinalizer => "3".repeat(64),
        ScannerShutdownPhase::FinalizerOwned => "4".repeat(64),
        ScannerShutdownPhase::CancellationRace => "5".repeat(64),
        ScannerShutdownPhase::ReplacementCleanup => "6".repeat(64),
    };
    let request = retag_host_request(
        host_request(&fixture.host_coordinator, &requester, &target),
        &nonce,
    );
    let root = &fixture.paths.coordinator;
    let local = root.join(crate::config::agent_local_dir_name());
    let request_directory = local.join("outbox").join("terminal-snapshot-requests");
    let response_directory = local.join("terminal-snapshot-responses");
    let request_bytes = serde_json::to_vec(&request).expect("scanner shutdown request JSON");
    let (request_path, response_path) =
        write_host_request_bytes(root, &request.request_id, &request_bytes);
    let original_identity = crate::path_identity::verify_regular_file(&request_path)
        .expect("scanner shutdown request identity");

    let mut scanner = crate::phone::terminal_snapshot::SnapshotMailboxScanner::default();
    let owner = scanner.shutdown_owner_for_test();
    let start_control = phase
        .abortable()
        .then(crate::phone::terminal_snapshot::SnapshotScannerTaskStartControl::new);
    if let Some(control) = &start_control {
        owner.install_next_task_start_control(Arc::clone(control));
    }
    let blocking_control = (phase == ScannerShutdownPhase::BlockingService)
        .then(|| TerminalSnapshotBlockingControl::new(None));
    if let Some(control) = &blocking_control {
        fixture
            .snapshot_state
            .install_blocking_control(TerminalSnapshotBlockingStage::Capture, Arc::clone(control));
    }
    let cancellation_barrier = match phase {
        ScannerShutdownPhase::BeforeFinalizer => Some(install_host_cancellation_barrier(
            &fixture.snapshot_state,
            TerminalSnapshotHostCancellationStage::ResponseBytesReady,
        )),
        ScannerShutdownPhase::CancellationRace => Some(install_host_cancellation_barrier(
            &fixture.snapshot_state,
            TerminalSnapshotHostCancellationStage::Processing,
        )),
        _ => None,
    };
    let finalizer_control = (phase == ScannerShutdownPhase::FinalizerOwned).then(|| {
        TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::RevalidationEntry,
            None,
        )
    });
    if let Some(control) = &finalizer_control {
        fixture
            .snapshot_state
            .install_next_host_finalizer_control(Arc::clone(control));
    }

    scanner.begin_cycle();
    scanner.scan_root(
        fixture.app.as_ref().expect("fixture app is alive").handle(),
        root,
    );
    scanner.finish_cycle();
    if let Some(control) = &start_control {
        control.wait_until_entered();
    }
    if let Some(control) = &blocking_control {
        control.wait_until_entered();
    }
    if let Some((entered, _)) = &cancellation_barrier {
        wait_for_host_cancellation_barrier(entered);
    }
    if let Some(control) = &finalizer_control {
        control.wait_until_entered();
        assert!(control.retained_response_bytes() > 0);
    }

    let processing = matches!(
        phase,
        ScannerShutdownPhase::CancellationRace | ScannerShutdownPhase::ReplacementCleanup
    )
    .then(|| scanner_processing_path(&request_directory, &request.request_id));
    let mut replacement = None;
    if phase == ScannerShutdownPhase::CancellationRace {
        assert!(!crate::cli::terminal_snapshot::cancel_request_for_test(
            &request_path,
            &request_directory,
            Uuid::parse_str(&request.request_id).expect("scanner cancellation request UUID"),
            &request.nonce,
            &original_identity,
        ));
        assert!(!host_cancellation_marker(&request_directory, &request).exists());
    }
    if phase == ScannerShutdownPhase::ReplacementCleanup {
        let processing = processing.expect("replacement processing path");
        let displaced = request_directory.join("scanner-owned-displaced");
        std::fs::rename(&processing, &displaced).expect("displace scanner-owned processing file");
        std::fs::write(&processing, b"foreign processing replacement")
            .expect("write processing replacement");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&processing, std::fs::Permissions::from_mode(0o600))
                .expect("private processing replacement");
        }
        replacement = Some((processing.clone(), displaced));
    }

    owner.seal();
    assert!(owner.is_sealed());
    fixture.snapshot_state.shutdown.trigger();
    if let Some(control) = &blocking_control {
        control.expire_deadline();
    }
    let first_budget = if phase.abortable() {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(75)
    };
    let first = owner
        .seal_and_drain_until(tokio::time::Instant::now() + first_budget)
        .await;
    if phase.abortable() {
        assert!(first.terminal, "abortable scanner owner must be joined");
        assert_eq!(first.aborted, 1);
        assert_eq!(first.joined, 1);
        assert!(first.retained.is_empty());
        assert_eq!(owner.task_count(), 0);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
    } else {
        assert!(!first.terminal, "started scanner owner must be retained");
        assert_eq!(first.aborted, 0);
        assert_eq!(first.joined, 0);
        assert_eq!(first.retained.len(), 1);
        assert_eq!(owner.task_count(), 1);
        // A retained scanner is still reported in the shutdown diagnostics
        // below, but it must not suppress session persistence: its only
        // SessionManager access is a read-lock clone, so it cannot leave session
        // state inconsistent.
        assert!(
            crate::shutdown_persistence_allowed(true, true),
            "a retained snapshot scanner must not suppress session persistence"
        );
        let diagnostics = crate::combined_shutdown_retained_diagnostics_with_scanner(
            first.retained.clone(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("owner=terminalSnapshotScanner"));
        assert!(!diagnostics[0].contains(&request.request_id));
        assert!(!diagnostics[0].contains(SCREEN_SENTINEL));
        assert!(!diagnostics[0].contains('/'));
        assert!(!diagnostics[0].contains('\\'));
        if phase == ScannerShutdownPhase::FinalizerOwned {
            assert!(first.retained[0].contains("terminal-snapshot-finalizer"));
        } else {
            assert!(first.retained[0].contains("terminal-snapshot-service"));
        }
        assert!(fixture
            .app
            .as_ref()
            .expect("fixture app is alive")
            .try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .is_some());
        assert!(fixture
            .app
            .as_ref()
            .expect("fixture app is alive")
            .try_state::<Arc<std::sync::Mutex<PtyManager>>>()
            .is_some());
        match phase {
            ScannerShutdownPhase::CancellationRace => assert_eq!(
                api_lifecycle_counts(&fixture.snapshot_state),
                ApiLifecycleCounts {
                    ingress_available: SNAPSHOT_INGRESS_LIMIT - 1,
                    requester_in_flight: 0,
                    target_in_flight: 0,
                    global_in_flight: 0,
                }
            ),
            _ => assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true),
        }
    }

    if let Some(control) = &start_control {
        control.release();
    }
    if let Some(control) = &blocking_control {
        control.release();
        control.wait_until_completed();
    }
    if let Some((_, release)) = cancellation_barrier {
        release
            .send(())
            .expect("release scanner cancellation barrier");
    }
    if let Some(control) = &finalizer_control {
        control.release();
    }
    let terminal = owner
        .seal_and_drain_until(tokio::time::Instant::now() + Duration::from_secs(10))
        .await;
    assert!(terminal.terminal);
    assert_eq!(owner.task_count(), 0);
    assert!(crate::shutdown_persistence_allowed(true, true));
    assert_api_lifecycle_idle(&fixture.snapshot_state);

    let mut response_reason = None;
    if phase.abortable() {
        assert!(!response_path.exists());
    } else {
        let bytes = std::fs::read(&response_path).expect("shutdown metadata response");
        assert!(!contains_raw(&bytes, SCREEN_SENTINEL.as_bytes()));
        let decoded = decode_host_response(
            &bytes,
            &request.request_id,
            &request.confirmation_tag,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("strict shutdown response");
        assert!(decoded.result.is_none());
        response_reason = decoded.error;
        assert!(response_reason.is_some());
        #[cfg(not(unix))]
        let identity = crate::path_identity::verify_regular_file(&response_path)
            .expect("shutdown response identity");
        std::fs::remove_file(&response_path).expect("consume shutdown response");
        #[cfg(not(unix))]
        fixture.snapshot_state.untrack_artifact(&identity);
    }

    if let Some((replacement_path, displaced)) = replacement {
        assert_eq!(
            std::fs::read(&replacement_path).expect("read processing replacement"),
            b"foreign processing replacement"
        );
        assert!(displaced.exists());
        std::fs::remove_file(replacement_path).expect("remove processing replacement");
        std::fs::remove_file(displaced).expect("remove displaced scanner-owned file");
        #[cfg(not(unix))]
        fixture.snapshot_state.untrack_artifact(&original_identity);
    }
    assert!(!host_cancellation_marker(&request_directory, &request).exists());
    fixture.snapshot_state.sweep_artifacts_for_test(false);
    assert_eq!(fixture.snapshot_state.test_artifact_counts(), (0, 0, 0));
    assert_eq!(
        std::fs::read_dir(&request_directory)
            .expect("scanner request residue inventory")
            .count(),
        0
    );
    assert_eq!(
        std::fs::read_dir(&response_directory)
            .expect("scanner response residue inventory")
            .count(),
        0
    );

    let canaries = vec![
        SCREEN_SENTINEL.to_string(),
        OSC_TITLE_SENTINEL.to_string(),
        OSC_HYPERLINK_SENTINEL.to_string(),
        OSC_CLIPBOARD_SENTINEL.to_string(),
        fixture.host_coordinator.token.to_string(),
        request.nonce.clone(),
        request.confirmation_tag.clone(),
    ];
    let rows = snapshot_audit_rows(&config, &canaries);
    assert_eq!(rows.len(), 1, "one scanner shutdown audit event");
    let row = &rows[0];
    assert_eq!(
        row.get("requestId").and_then(serde_json::Value::as_str),
        Some(request.request_id.as_str())
    );
    assert_eq!(
        row.get("status").and_then(serde_json::Value::as_str),
        Some("failed")
    );
    if let Some(reason) = response_reason {
        assert_eq!(
            row.get("reasonCode").and_then(serde_json::Value::as_str),
            Some(reason.as_str())
        );
    }
    assert_eq!(fixture.local_backend.mutations(), 0);
}

#[test]
fn scanner_app_shutdown_owns_composed_host_phases() {
    let canaries = [
        SCREEN_SENTINEL,
        OSC_TITLE_SENTINEL,
        OSC_HYPERLINK_SENTINEL,
        OSC_CLIPBOARD_SENTINEL,
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let child_phase = std::env::var(SCANNER_SHUTDOWN_CHILD_ENV)
        .ok()
        .and_then(|value| ScannerShutdownPhase::parse(&value));
    if child_phase.is_none() {
        for phase in [
            ScannerShutdownPhase::BeforeService,
            ScannerShutdownPhase::BlockingService,
            ScannerShutdownPhase::BeforeFinalizer,
            ScannerShutdownPhase::FinalizerOwned,
            ScannerShutdownPhase::CancellationRace,
            ScannerShutdownPhase::ReplacementCleanup,
        ] {
            let output = std::process::Command::new(
                std::env::current_exe().expect("scanner shutdown test executable"),
            )
            .args([
                "--exact",
                SCANNER_SHUTDOWN_TEST_NAME,
                "--test-threads=1",
                "--nocapture",
            ])
            .env(SCANNER_SHUTDOWN_CHILD_ENV, phase.label())
            .output()
            .expect("spawn isolated scanner shutdown test");
            assert_canaries_absent_except(
                &output.stdout,
                &canaries,
                &[],
                "scanner shutdown child stdout",
            );
            assert_canaries_absent_except(
                &output.stderr,
                &canaries,
                &[],
                "scanner shutdown child stderr",
            );
            if !output.status.success() {
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
                for canary in &canaries {
                    stdout = stdout.replace(canary, "<redacted-canary>");
                    stderr = stderr.replace(canary, "<redacted-canary>");
                }
                panic!(
                    "isolated scanner shutdown phase={} failed; stdout={stdout:?}; stderr={stderr:?}",
                    phase.label()
                );
            }
        }
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("scanner shutdown current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("scanner shutdown temporary root");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("scanner shutdown runtime");
    runtime.block_on(run_composed_scanner_shutdown_phase(
        &temporary_root,
        child_phase.expect("validated scanner shutdown child phase"),
    ));
}

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

#[test]
fn synchronous_host_finalizer_retains_authority_through_late_completion() {
    if std::env::var_os(HOST_FINALIZER_CHILD_ENV).is_none() {
        let temporary_root = std::env::current_dir()
            .expect("host finalizer parent current directory")
            .join("target")
            .join("terminal-snapshot-acceptance-temp");
        std::fs::create_dir_all(&temporary_root).expect("host finalizer parent temporary root");
        let evidence = tempfile::Builder::new()
            .prefix("host-finalizer-evidence-")
            .tempdir_in(temporary_root)
            .expect("host finalizer evidence directory");
        let canary_file = evidence.path().join("canaries.json");
        let output = std::process::Command::new(
            std::env::current_exe().expect("host finalizer test executable"),
        )
        .args([
            "--exact",
            HOST_FINALIZER_TEST_NAME,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(HOST_FINALIZER_CHILD_ENV, "1")
        .env(HOST_FINALIZER_CANARY_FILE_ENV, &canary_file)
        .output()
        .expect("spawn isolated host finalizer test");
        let canaries: Vec<String> = std::fs::read(&canary_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| {
                [
                    SCREEN_SENTINEL,
                    OSC_TITLE_SENTINEL,
                    OSC_HYPERLINK_SENTINEL,
                    OSC_CLIPBOARD_SENTINEL,
                    HOST_FINALIZER_PANIC_SENTINEL,
                ]
                .into_iter()
                .map(str::to_string)
                .collect()
            });
        assert_canaries_absent_except(
            &output.stdout,
            &canaries,
            &[],
            "host finalizer child stdout",
        );
        assert_canaries_absent_except(
            &output.stderr,
            &canaries,
            &[],
            "host finalizer child stderr",
        );
        if !output.status.success() {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            for canary in &canaries {
                stdout = stdout.replace(canary, "<redacted-canary>");
                stderr = stderr.replace(canary, "<redacted-canary>");
            }
            panic!("isolated host finalizer test failed; stdout={stdout:?}; stderr={stderr:?}");
        }
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("host finalizer current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("host finalizer temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("host-finalizer-")
        .tempdir_in(temporary_root)
        .expect("host finalizer temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("host finalizer config directory");
    let _env = ConfigEnvGuard::set(&config);
    std::env::set_var("RUST_LOG", "vt100=trace,agentscommander=trace");
    crate::logging::init_logger();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("host finalizer runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
        let live_target = format!("{PROJECT}:{WORKGROUP}/member-live");
        let mut canaries = vec![
            SCREEN_SENTINEL.to_string(),
            OSC_TITLE_SENTINEL.to_string(),
            OSC_HYPERLINK_SENTINEL.to_string(),
            OSC_CLIPBOARD_SENTINEL.to_string(),
            HOST_FINALIZER_PANIC_SENTINEL.to_string(),
            fixture.host_coordinator.token.to_string(),
        ];
        canaries.sort();
        canaries.dedup();
        let canary_file = PathBuf::from(
            std::env::var_os(HOST_FINALIZER_CANARY_FILE_ENV)
                .expect("host finalizer canary file path"),
        );
        std::fs::write(
            canary_file,
            serde_json::to_vec(&canaries).expect("host finalizer canary manifest"),
        )
        .expect("write host finalizer canary manifest");

        let deadline_case = prepare_host_finalizer(&fixture, &live_target).await;
        let deadline_request_id = deadline_case.request_id.clone();
        let deadline_selected = deadline_case.selected_session_id;
        let liveness_before = fixture.local_backend.counts(deadline_selected).liveness;
        let deadline_control = TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::RouteVerification,
            None,
        );
        let mut running = start_host_finalizer(deadline_case, Arc::clone(&deadline_control));
        running.wait_until_blocked_and_detach();
        assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &deadline_request_id
        ));
        deadline_control.expire_deadline();
        deadline_control.release();
        let (outcome, handoffs) = running.finish().await;
        assert_eq!(
            outcome,
            HostFinalizerWorkerOutcome::Returned(Err(TerminalSnapshotReasonCode::SnapshotTimeout))
        );
        assert_eq!(
            handoffs,
            vec![HostFinalizerHandoff::Failure(
                TerminalSnapshotReasonCode::SnapshotTimeout
            )]
        );
        assert!(fixture.local_backend.counts(deadline_selected).liveness > liveness_before);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_host_finalizer_audit(
            &config,
            &canaries,
            1,
            &deadline_request_id,
            "failed",
            Some(TerminalSnapshotReasonCode::SnapshotTimeout),
        );

        let privacy_case = prepare_host_finalizer(&fixture, &live_target).await;
        let privacy_request_id = privacy_case.request_id.clone();
        let privacy_control = TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::RevalidationEntry,
            None,
        );
        let mut running = start_host_finalizer(privacy_case, Arc::clone(&privacy_control));
        running.wait_until_blocked_and_detach();
        assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &privacy_request_id
        ));
        fixture.settings.write().await.terminal_snapshots_enabled = false;
        write_security_settings(&fixture.settings_path, &fixture.paths.collection, false);
        privacy_control.release();
        let (outcome, handoffs) = running.finish().await;
        assert_eq!(
            outcome,
            HostFinalizerWorkerOutcome::Returned(Err(TerminalSnapshotReasonCode::AuthorityChanged))
        );
        assert_eq!(
            handoffs,
            vec![HostFinalizerHandoff::Failure(
                TerminalSnapshotReasonCode::AuthorityChanged
            )]
        );
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_host_finalizer_audit(
            &config,
            &canaries,
            2,
            &privacy_request_id,
            "failed",
            Some(TerminalSnapshotReasonCode::AuthorityChanged),
        );
        fixture.settings.write().await.terminal_snapshots_enabled = true;
        write_security_settings(&fixture.settings_path, &fixture.paths.collection, true);

        let route_case = prepare_host_finalizer(&fixture, &live_target).await;
        let route_request_id = route_case.request_id.clone();
        let route_selected = route_case.selected_session_id;
        let route_control = TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::RouteVerification,
            None,
        );
        let mut running = start_host_finalizer(route_case, Arc::clone(&route_control));
        running.wait_until_blocked_and_detach();
        assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &route_request_id
        ));
        fixture
            .pty_manager
            .lock()
            .expect("host finalizer PTY manager")
            .remove_route_if_kind(route_selected, SessionBackendKind::LocalProcess);
        route_control.release();
        let (outcome, handoffs) = running.finish().await;
        assert_eq!(
            outcome,
            HostFinalizerWorkerOutcome::Returned(Err(TerminalSnapshotReasonCode::AuthorityChanged))
        );
        assert_eq!(
            handoffs,
            vec![HostFinalizerHandoff::Failure(
                TerminalSnapshotReasonCode::AuthorityChanged
            )]
        );
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_host_finalizer_audit(
            &config,
            &canaries,
            3,
            &route_request_id,
            "failed",
            Some(TerminalSnapshotReasonCode::AuthorityChanged),
        );
        {
            let manager = fixture
                .pty_manager
                .lock()
                .expect("restore host finalizer PTY route");
            record_route(
                &manager,
                route_selected,
                SessionBackendKind::LocalProcess,
                &fixture.paths.live_member,
            );
        }

        let session_case = prepare_host_finalizer(&fixture, &live_target).await;
        let session_request_id = session_case.request_id.clone();
        let session_selected = session_case.selected_session_id;
        let session_control = TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::RouteVerification,
            None,
        );
        let mut running = start_host_finalizer(session_case, Arc::clone(&session_control));
        running.wait_until_blocked_and_detach();
        assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &session_request_id
        ));
        let manager = { fixture.session_manager.read().await.clone() };
        manager.mark_exited(session_selected, 0).await;
        session_control.release();
        let (outcome, handoffs) = running.finish().await;
        assert_eq!(
            outcome,
            HostFinalizerWorkerOutcome::Returned(Err(TerminalSnapshotReasonCode::AuthorityChanged))
        );
        assert_eq!(
            handoffs,
            vec![HostFinalizerHandoff::Failure(
                TerminalSnapshotReasonCode::AuthorityChanged
            )]
        );
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_host_finalizer_audit(
            &config,
            &canaries,
            4,
            &session_request_id,
            "failed",
            Some(TerminalSnapshotReasonCode::AuthorityChanged),
        );
        let replacement = create_session(
            &manager,
            &fixture.paths.live_member,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await;
        fixture
            .local_backend
            .install(replacement.id, &terminal_canary_output());
        {
            let pty = fixture
                .pty_manager
                .lock()
                .expect("replacement host finalizer PTY route");
            record_route(
                &pty,
                replacement.id,
                SessionBackendKind::LocalProcess,
                &fixture.paths.live_member,
            );
        }

        let panic_case = prepare_host_finalizer(&fixture, &live_target).await;
        let panic_request_id = panic_case.request_id.clone();
        let panic_payload = format!(
            "{HOST_FINALIZER_PANIC_SENTINEL}|{SCREEN_SENTINEL}|{}",
            fixture.host_coordinator.token
        );
        let panic_control = TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::RouteVerification,
            Some(panic_payload),
        );
        let mut running = start_host_finalizer(panic_case, Arc::clone(&panic_control));
        running.wait_until_blocked_and_detach();
        assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &panic_request_id
        ));
        panic_control.release();
        let (outcome, handoffs) = running.finish().await;
        assert_eq!(outcome, HostFinalizerWorkerOutcome::Panicked);
        assert!(
            handoffs.is_empty(),
            "a panicked finalizer must not hand off a response"
        );
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_host_finalizer_audit(
            &config,
            &canaries,
            5,
            &panic_request_id,
            "failed",
            Some(TerminalSnapshotReasonCode::Internal),
        );

        let reuse_case = prepare_host_finalizer(&fixture, &live_target).await;
        let reuse_request_id = reuse_case.request_id.clone();
        let reuse_control = TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::FinalDeadline,
            None,
        );
        let mut running = start_host_finalizer(reuse_case, Arc::clone(&reuse_control));
        running.wait_until_blocked_and_detach();
        assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true);
        assert!(!audit_contains_request(
            &fixture.settings_path,
            &reuse_request_id
        ));
        reuse_control.release();
        let (outcome, handoffs) = running.finish().await;
        assert_eq!(outcome, HostFinalizerWorkerOutcome::Returned(Ok(())));
        assert_eq!(handoffs, vec![HostFinalizerHandoff::Success]);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
        assert_host_finalizer_audit(&config, &canaries, 6, &reuse_request_id, "succeeded", None);

        assert_eq!(fixture.local_backend.mutations(), 0);
        assert!(!fixture.snapshot_state.has_blocking_controls());
        log::logger().flush();
        let app_log = std::fs::read(config.join("app.log")).expect("host finalizer app log");
        assert_canaries_absent_except(&app_log, &canaries, &[], "host finalizer app log");
        assert!(contains_raw(
            &app_log,
            b"[terminal-snapshot] stage=host_finalizer_task code=internal"
        ));
        assert_cleanup_and_secondary_surfaces(&fixture, &config, &canaries);
    });
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
            fixture.app.as_ref().expect("fixture app is alive").handle().clone(),
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
            fixture.app.as_ref().expect("fixture app is alive").handle().clone(),
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
            &fixture.snapshot_state,
            TerminalSnapshotBlockingStage::TestResourceRetention,
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
