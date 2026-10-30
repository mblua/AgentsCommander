use super::*;

fn api_server_command_test_app(settings: AppSettings) -> tauri::App {
    let session_mgr = Arc::new(RwLock::new(SessionManager::new()));
    let store_dir = tempfile::tempdir().expect("create API store directory");
    let message_store = Arc::new(
        crate::api::message_store::MessageStore::open(
            store_dir
                .path()
                .join(crate::api::message_store::DB_FILENAME),
        )
        .expect("open API message store"),
    );
    let app = crate::test_support::test_builder()
        .manage(ApiServerHandle::default())
        .manage(crate::api::message_store::MessageStoreState::ready(
            message_store,
        ))
        .manage(store_dir)
        .manage(state_for(settings))
        .manage(Arc::clone(&session_mgr))
        .manage(crate::shutdown::ShutdownSignal::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let git_watcher =
        crate::pty::git_watcher::GitWatcher::new(Arc::clone(&session_mgr), app.handle().clone());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new(
        Arc::new(Mutex::new(HashMap::new())),
        idle_detector,
        git_watcher,
        None,
        None,
    )));
    app.manage(pty_mgr);
    app
}

#[test]
fn api_server_probe_addr_maps_wildcards_to_loopback() {
    assert_eq!(
        api_server_probe_addr("0.0.0.0", 9906).unwrap().to_string(),
        "127.0.0.1:9906"
    );
    assert_eq!(
        api_server_probe_addr("127.0.0.1", 9906)
            .unwrap()
            .to_string(),
        "127.0.0.1:9906"
    );
    assert_eq!(
        api_server_probe_addr("::", 9906).unwrap().to_string(),
        "[::1]:9906"
    );
    assert_eq!(
        api_server_probe_addr("::1", 9906).unwrap().to_string(),
        "[::1]:9906"
    );
}

#[tokio::test]
async fn api_server_probe_addr_detects_wildcard_listener_via_loopback() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .expect("bind wildcard test listener");
    let port = listener.local_addr().expect("listener local addr").port();
    let probe_addr = api_server_probe_addr("0.0.0.0", port).expect("probe addr");

    assert_eq!(probe_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert!(is_tcp_socket_listening(probe_addr).await);
}

#[tokio::test]
async fn api_server_status_reports_running_for_managed_wildcard_server() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .expect("bind wildcard test listener");
    let bound_addr = listener.local_addr().expect("listener local addr");
    let settings = AppSettings {
        api_server_bind: "0.0.0.0".to_string(),
        api_server_port: bound_addr.port(),
        ..AppSettings::default()
    };
    let app = tauri::test::mock_builder()
        .manage(ApiServerHandle::default())
        .manage(state_for(settings))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");

    let shutdown = CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let join = tauri::async_runtime::spawn(async move {
        let _listener = listener;
        task_shutdown.cancelled().await;
    });
    app.state::<ApiServerHandle>()
        .store_if_idle(ApiServerTask::new(join, shutdown, bound_addr))
        .expect("store api server task");

    let running = api_server_status(app.state::<ApiServerHandle>(), app.state::<SettingsState>())
        .await
        .expect("status succeeds");

    assert!(running);
    assert!(app
        .state::<ApiServerHandle>()
        .shutdown_running(Duration::from_secs(1))
        .await
        .expect("shutdown stored task"));
}

#[tokio::test]
async fn start_api_server_bind_failure_returns_err_and_status_false() {
    let settings = AppSettings {
        api_server_bind: "192.0.2.1".to_string(),
        api_server_port: 9906,
        ..AppSettings::default()
    };
    let app = api_server_command_test_app(settings);

    let err = start_api_server(
        app.handle().clone(),
        app.state::<ApiServerHandle>(),
        app.state::<SettingsState>(),
        app.state::<Arc<RwLock<SessionManager>>>(),
        app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>(),
        app.state::<crate::shutdown::ShutdownSignal>(),
    )
    .await
    .expect_err("nonlocal bind should fail readiness");

    assert!(err.contains("API server bind failed"), "{err}");
    assert!(!app.state::<ApiServerHandle>().has_running().unwrap());
    assert!(
        !api_server_status(app.state::<ApiServerHandle>(), app.state::<SettingsState>())
            .await
            .expect("status succeeds")
    );
}

/// #1768 fix case. The port is held continuously from reservation to service, so nothing can
/// take it in between. **This is the discriminating test**: section 7.3 runs it against the
/// unfixed shape (mutant M1) and records it red, then against the fixed shape and records it
/// green.
#[tokio::test]
async fn api_start_server_adopts_the_prebound_listener_and_never_releases_the_port() {
    let app = api_server_command_test_app(AppSettings::default());
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback port");
    let port = listener
        .local_addr()
        .expect("reserved loopback address")
        .port();
    // Mutation tripwire. This duplicate handle keeps the port bound even if the
    // implementation drops the listener it was handed, so a drop-then-rebind implementation
    // fails readiness instead of quietly passing.
    let keeper = listener.try_clone().expect("duplicate reserved listener");

    let session_mgr = app.state::<Arc<RwLock<SessionManager>>>();
    let pty_mgr = app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let start = crate::api::start_server_on_listener(
        listener,
        app.handle().clone(),
        session_mgr.inner().clone(),
        pty_mgr.inner().clone(),
        shutdown.clone(),
    );

    let addr = crate::api::wait_for_startup_ready(start.readiness)
        .await
        .expect("prebound listener adopted");
    assert_eq!(addr.port(), port, "server must serve the reserved port");

    // The port assertion and a bare `TcpStream::connect` are BOTH satisfied by an
    // implementation that keeps `keeper`'s port bound and serves Axum on a different socket:
    // `keeper` is a listening socket, so the OS completes the TCP handshake into its backlog
    // even though nothing ever accepts. Only a timed application-level response proves that
    // THIS server is the thing answering on the reserved address, so that is what is asserted.
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("healthz client");
    let url = format!("http://{}/api/v1/healthz", addr);
    let body = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        client
            .get(&url)
            .send()
            .await
            .expect("healthz request")
            .text()
            .await
            .expect("healthz body")
    })
    .await
    .expect("healthz deadline on the reserved address");
    assert_eq!(
        body, "{\"ok\":true}",
        "the reserved address must be served by this server"
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), start.join_handle)
        .await
        .expect("API server shutdown deadline")
        .expect("API server task");
    drop(keeper);
}

/// #1768 readiness-error **contract** test for the unchanged `Address` path. It pins the
/// wording and the shape of the bind-failure readiness error that CI logs show. It is NOT a
/// control for this change: it exercises only the `Address` path, which this plan does not
/// alter, so it passes identically on the unfixed and the fixed code. Section 7.3 M1 is the
/// red-before / green-after evidence; this test is not.
#[tokio::test]
async fn api_start_server_address_path_bind_failure_readiness_contract() {
    let app = api_server_command_test_app(AppSettings::default());
    let adversary = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("adversary loopback port");
    let port = adversary
        .local_addr()
        .expect("adversary loopback address")
        .port();

    let session_mgr = app.state::<Arc<RwLock<SessionManager>>>();
    let pty_mgr = app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let start = crate::api::start_server(
        "127.0.0.1".to_string(),
        port,
        app.handle().clone(),
        session_mgr.inner().clone(),
        pty_mgr.inner().clone(),
        shutdown.clone(),
    );

    let err = crate::api::wait_for_startup_ready(start.readiness)
        .await
        .expect_err("a taken port must fail readiness");
    assert!(err.contains("API server bind failed"), "{err}");
    assert!(err.contains(&port.to_string()), "{err}");

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), start.join_handle)
        .await
        .expect("API server shutdown deadline")
        .expect("API server task");
    drop(adversary);
}

struct MintApiClientFixture {
    _temp: tempfile::TempDir,
    path: PathBuf,
    project: PathBuf,
    settings: AppSettings,
    root: String,
    expected_fqn: String,
}

fn mint_api_client_fixture() -> MintApiClientFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_alice");
    let replica = ac_root.join("wg-1-devs").join("__agent_alice");
    std::fs::create_dir_all(&matrix).expect("create matrix");
    std::fs::create_dir_all(&replica).expect("create replica");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_alice"}"#,
    )
    .expect("write replica config");

    let root_path = std::fs::canonicalize(&replica).expect("canonical replica");
    let root = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&root_path);
    let expected_fqn = crate::config::teams::agent_fqn_from_path(&root);
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().to_string()],
        ..AppSettings::default()
    };

    MintApiClientFixture {
        path: temp.path().join(auth::REGISTRY_FILENAME),
        _temp: temp,
        project,
        settings,
        root,
        expected_fqn,
    }
}

#[test]
fn mint_api_client_command_helper_returns_token_and_stores_hash_only_with_default_expiry() {
    let fixture = mint_api_client_fixture();
    let issued_at = "2026-01-01T00:00:00Z";
    let expected_expires_at = (chrono::DateTime::parse_from_rfc3339(issued_at)
        .unwrap()
        .with_timezone(&chrono::Utc)
        + chrono::Duration::hours(MINT_API_CLIENT_DEFAULT_TTL_HOURS))
    .to_rfc3339();

    let response = mint_api_client_with_path(
        &fixture.path,
        &fixture.settings,
        fixture.root.clone(),
        vec![
            " send ".to_string(),
            "".to_string(),
            "list-peers-lean".to_string(),
        ],
        Some("gui mint".to_string()),
        None,
        "client-1".to_string(),
        "plain-secret".to_string(),
        issued_at.to_string(),
    )
    .expect("mint succeeds");

    assert_eq!(response.client_id, "client-1");
    assert_eq!(response.token, "plain-secret");
    assert_eq!(response.bound_root, fixture.root);
    assert_eq!(response.bound_fqn, fixture.expected_fqn);
    assert_eq!(response.scopes, vec!["send", "list-peers-lean"]);
    assert_eq!(
        response.expires_at.as_deref(),
        Some(expected_expires_at.as_str())
    );
    assert_eq!(response.note, MINT_API_CLIENT_NOTE);

    let raw = std::fs::read_to_string(&fixture.path).expect("read registry");
    assert!(raw.contains("client-1"));
    assert!(raw.contains("sha256:"));
    assert!(!raw.contains("plain-secret"));

    let registry = auth::list(&fixture.path);
    assert_eq!(registry.clients.len(), 1);
    let client = &registry.clients[0];
    assert_eq!(client.client_id, "client-1");
    assert_eq!(client.label, "gui mint");
    assert_eq!(client.bound_fqn, fixture.expected_fqn);
    assert_eq!(client.bound_root, response.bound_root);
    assert_eq!(client.scopes, response.scopes);
    assert_eq!(client.expires_at, response.expires_at);
    assert_ne!(client.token_hash, "plain-secret");
}

#[test]
fn mint_api_client_rejects_unknown_root() {
    let fixture = mint_api_client_fixture();
    let unknown_root = fixture
        .project
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_missing");

    let err = mint_api_client_with_path(
        &fixture.path,
        &fixture.settings,
        unknown_root.to_string_lossy().to_string(),
        vec!["send".to_string()],
        None,
        None,
        "client-1".to_string(),
        "plain-secret".to_string(),
        "2026-01-01T00:00:00Z".to_string(),
    )
    .expect_err("unknown root should fail");

    assert!(err.contains("unknown or invalid root"), "{err}");
    assert!(!fixture.path.exists());
}

#[test]
fn mint_api_client_rejects_expiry_beyond_max() {
    let fixture = mint_api_client_fixture();
    let issued_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let over_max =
        (issued_at + chrono::Duration::days(MINT_API_CLIENT_MAX_TTL_DAYS + 1)).to_rfc3339();

    let err = mint_api_client_with_path(
        &fixture.path,
        &fixture.settings,
        fixture.root,
        vec!["send".to_string()],
        None,
        Some(over_max),
        "client-1".to_string(),
        "plain-secret".to_string(),
        issued_at.to_rfc3339(),
    )
    .expect_err("over-max expiry should fail");

    assert!(
        err.contains("expires exceeds maximum API client lifetime"),
        "{err}"
    );
    assert!(!fixture.path.exists());
}

#[test]
fn api_warning_port_preserves_managed_owner_identity() {
    let app = api_server_command_test_app(AppSettings::default());
    let root = app.state::<crate::app_state::AppOwners>();
    let first = crate::api::ApiMessageWarnings::message_warnings(app.handle());
    let second = crate::api::ApiMessageWarnings::message_warnings(app.handle());
    assert!(Arc::ptr_eq(&first, &root.messages));
    assert!(Arc::ptr_eq(&second, &root.messages));
}

#[tokio::test]
async fn issue_3265_native_api_join_contract() {
    use crate::api::{ApiJoinTask, NativeApiSpawner};

    let app = api_server_command_test_app(AppSettings::default());
    let (sent, received) = tokio::sync::oneshot::channel();
    let finite: tauri::async_runtime::JoinHandle<()> =
        app.handle().spawn_owned(Box::pin(async move {
            sent.send(()).expect("finite native task signal");
        }));
    tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .expect("finite task deadline")
        .expect("finite task ran");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !finite.inner().is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("native inner finished deadline");
    finite.await.expect("finite native result");

    let panicked = app.handle().spawn_owned(Box::pin(async {
        panic!("issue_3265_native_api_join_contract panic");
    }));
    let error = tokio::time::timeout(Duration::from_secs(5), panicked)
        .await
        .expect("panic deadline")
        .expect_err("native panic must propagate");
    assert!(error.to_string().contains("panic"), "{error}");

    let (started, observed) = tokio::sync::oneshot::channel();
    let pending = app.handle().spawn_owned(Box::pin(async move {
        started.send(()).expect("pending native task signal");
        std::future::pending::<()>().await;
    }));
    tokio::time::timeout(Duration::from_secs(5), observed)
        .await
        .expect("pending start deadline")
        .expect("pending task started");
    ApiJoinTask::abort(&pending);
    let error = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .expect("abort completion deadline")
        .expect_err("aborted native task must fail");
    assert!(!error.to_string().is_empty());
}

type ApiOwnerObservation = (
    &'static str,
    Option<Arc<crate::config::injected_messages::MessageWarnings>>,
);

struct ApiRuntimeProbe {
    native: tauri::AppHandle,
    observations: Arc<Mutex<Vec<ApiOwnerObservation>>>,
}

impl Clone for ApiRuntimeProbe {
    fn clone(&self) -> Self {
        self.record("host-clone");
        Self {
            native: self.native.clone(),
            observations: Arc::clone(&self.observations),
        }
    }
}

impl ApiRuntimeProbe {
    fn new(native: tauri::AppHandle) -> Self {
        Self {
            native,
            observations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn record(&self, point: &'static str) {
        self.observations.lock().unwrap().push((point, None));
    }

    fn owner(
        &self,
        point: &'static str,
        owner: &Arc<crate::config::injected_messages::MessageWarnings>,
    ) {
        self.observations
            .lock()
            .unwrap()
            .push((point, Some(Arc::clone(owner))));
    }
}

impl crate::api::NativeApiSpawner for ApiRuntimeProbe {
    type Join = tauri::async_runtime::JoinHandle<()>;

    fn spawn_owned(
        &self,
        future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
    ) -> Self::Join {
        self.record("spawn");
        crate::api::NativeApiSpawner::spawn_owned(&self.native, future)
    }
}

impl crate::api::ApiMessageWarnings for ApiRuntimeProbe {
    fn message_warnings(&self) -> Arc<crate::config::injected_messages::MessageWarnings> {
        let owner = crate::api::ApiMessageWarnings::message_warnings(&self.native);
        self.owner("lookup", &owner);
        owner
    }
}

impl crate::api::ApiRequestResources for ApiRuntimeProbe {
    fn required_settings(&self) -> SettingsState {
        crate::api::ApiRequestResources::required_settings(&self.native)
    }

    fn try_settings(&self) -> Option<SettingsState> {
        crate::api::ApiRequestResources::try_settings(&self.native)
    }

    fn snapshot_state(&self) -> Option<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>> {
        crate::api::ApiRequestResources::snapshot_state(&self.native)
    }

    fn restore_flag(&self) -> Option<Arc<crate::app_state::RestoreInProgress>> {
        crate::api::ApiRequestResources::restore_flag(&self.native)
    }

    fn purge_guard(&self) -> Option<Arc<crate::session::purge_guard::PurgeGuard>> {
        crate::api::ApiRequestResources::purge_guard(&self.native)
    }
}

impl crate::api::dispatcher::DispatcherDelivery for ApiRuntimeProbe {
    fn purging(&self) -> bool {
        crate::api::dispatcher::DispatcherDelivery::purging(&self.native)
    }

    fn message_store_state(&self) -> Option<crate::api::message_store::MessageStoreState> {
        let state = crate::api::dispatcher::DispatcherDelivery::message_store_state(&self.native);
        self.record(if state.as_ref().is_some_and(|state| state.store.is_ok()) {
            "validated"
        } else {
            "invalid"
        });
        state
    }

    fn restoring(&self) -> bool {
        crate::api::dispatcher::DispatcherDelivery::restoring(&self.native)
    }

    fn deliver_wake<'a>(
        &'a self,
        msg: crate::phone::types::OutboxMessage,
        messages: Arc<crate::config::injected_messages::MessageWarnings>,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.owner("wake-capture", &messages);
            let result = crate::api::dispatcher::DispatcherDelivery::deliver_wake(
                &self.native,
                msg,
                messages,
            )
            .await;
            assert!(result.is_err(), "unknown target cannot deliver wake");
            self.record("wake-complete");
            result
        })
    }

    fn dispatch_pty<'a>(
        &'a self,
        state: &'a crate::api::message_store::MessageStoreState,
        id: &'a str,
        client: &'a Arc<crate::api::auth::ApiClientStore>,
        messages: Arc<crate::config::injected_messages::MessageWarnings>,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            self.owner("pty-capture", &messages);
            crate::api::dispatcher::DispatcherDelivery::dispatch_pty(
                &self.native,
                state,
                id,
                client,
                messages,
            )
            .await;
            self.record("pty-complete");
        })
    }
}

#[tokio::test]
async fn issue_3265_api_warning_owner_startup_count_and_timing() {
    let app = api_server_command_test_app(AppSettings::default());
    let probe = ApiRuntimeProbe::new(app.handle().clone());
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("loopback listener");
    let sessions = app.state::<Arc<RwLock<SessionManager>>>().inner().clone();
    let pty = app
        .state::<Arc<Mutex<crate::pty::manager::PtyManager>>>()
        .inner()
        .clone();
    let shutdown = CancellationToken::new();
    let start = crate::api::start_server_on_listener(
        listener,
        probe.clone(),
        sessions.clone(),
        pty.clone(),
        shutdown.clone(),
    );
    crate::api::wait_for_startup_ready(start.readiness)
        .await
        .expect("API ready");
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), start.join_handle)
        .await
        .expect("native API shutdown deadline")
        .expect("native API join");
    {
        let observations = probe.observations.lock().unwrap();
        let names: Vec<_> = observations.iter().map(|item| item.0).collect();
        // The first clone transfers the argument; the next clones construct State.resources
        // and the owned server future. The router is built between those latter two clones.
        assert_eq!(
            &names[..6],
            &[
                "host-clone",
                "validated",
                "lookup",
                "host-clone",
                "host-clone",
                "spawn"
            ],
        );
        assert_eq!(names.iter().filter(|name| **name == "lookup").count(), 1);
        assert_eq!(names.iter().filter(|name| **name == "spawn").count(), 2);
        let owner = &app.state::<crate::app_state::AppOwners>().messages;
        assert!(Arc::ptr_eq(observations[2].1.as_ref().unwrap(), owner));
    }

    for failed_store in [false, true] {
        let mut builder = crate::test_support::test_builder();
        if failed_store {
            builder = builder.manage(
                crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                    Err("issue_3265_store_failure".into()),
                    Err("issue_3265_gate_failure".into()),
                ),
            );
        }
        let missing = builder
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("native app without usable managed DB store");
        let failed_probe = ApiRuntimeProbe::new(missing.handle().clone());
        let failed = crate::api::start_server_core(
            "127.0.0.1".into(),
            0,
            failed_probe.clone(),
            sessions.clone(),
            pty.clone(),
            CancellationToken::new(),
        );
        let error = crate::api::wait_for_startup_ready(failed.readiness)
            .await
            .expect_err("unusable managed store must fail");
        assert!(error.contains(if failed_store {
            "issue_3265_store_failure"
        } else {
            "unavailable"
        }));
        tokio::time::timeout(Duration::from_secs(5), failed.join_handle)
            .await
            .expect("early failure native join deadline")
            .expect("early failure empty native task");
        let observations = failed_probe.observations.lock().unwrap();
        let names: Vec<_> = observations.iter().map(|item| item.0).collect();
        assert_eq!(names, ["host-clone", "invalid", "spawn"]);
    }
}

#[tokio::test]
async fn issue_3265_api_warning_owner_reaches_both_native_dispatch_ports() {
    use crate::api::message_store::{EnqueueRequest, PtyInputEnqueueRequest, DEFAULT_CONTENT_TYPE};
    use crate::api::{ApiMessageWarnings, NativeApiSpawner};

    let app = api_server_command_test_app(AppSettings::default());
    let probe = ApiRuntimeProbe::new(app.handle().clone());
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().expect("ready managed store").clone();
    let client_dir = tempfile::tempdir().expect("isolated empty client registry");
    let clients = Arc::new(crate::api::auth::ApiClientStore::new(
        client_dir.path().join(crate::api::auth::REGISTRY_FILENAME),
    ));
    let namespace = format!("issue-3265-owner-{}", uuid::Uuid::new_v4());
    for index in 0..2 {
        let sender = format!("{namespace}:wg-1/sender-{index}");
        let target = format!("{namespace}:wg-1/missing-{index}");
        store
            .enqueue(EnqueueRequest {
                sender_fqn: sender.clone(),
                target_fqn: target.clone(),
                op_id: uuid::Uuid::new_v4().to_string(),
                content_type: DEFAULT_CONTENT_TYPE.into(),
                body: "unknown target, no PTY".into(),
                source_plane: "api_inline".into(),
                source_ref: None,
            })
            .expect("enqueue isolated wake");
        let now = chrono::Utc::now();
        store
            .enqueue_pty_input(PtyInputEnqueueRequest {
                injection_id: uuid::Uuid::new_v4().to_string(),
                sender_fqn: sender,
                target_fqn: target,
                op_id: uuid::Uuid::new_v4().to_string(),
                nonce_sha256: "a".repeat(64),
                request_fingerprint: "b".repeat(64),
                confirmation_tag: None,
                requested_agent_id: None,
                payload: b"unknown target, no PTY".to_vec(),
                source_plane: crate::phone::types::PtyInputSourcePlane::ContainerApi,
                sender_incarnation_fingerprint: "c".repeat(64),
                sender_identity_fingerprint: "d".repeat(64),
                target_identity_fingerprint: "e".repeat(64),
                authority_session_id: uuid::Uuid::new_v4().to_string(),
                authority_client_id: Some("absent-client".into()),
                authority_client_generation: Some(uuid::Uuid::new_v4().to_string()),
                issued_at: crate::phone::types::canonical_pty_timestamp(now),
                expires_at: crate::phone::types::canonical_pty_timestamp(
                    now + chrono::Duration::minutes(10),
                ),
            })
            .expect("enqueue isolated PTY rejection");
    }
    let owner = probe.message_warnings();
    let managed_owner = Arc::clone(&app.state::<crate::app_state::AppOwners>().messages);
    assert!(Arc::ptr_eq(&owner, &managed_owner));
    let shutdown = CancellationToken::new();
    let join: tauri::async_runtime::JoinHandle<()> =
        probe.spawn_owned(Box::pin(crate::api::dispatcher::run_dispatcher(
            store,
            clients,
            probe.clone(),
            shutdown.clone(),
            crate::api::dispatcher::DispatcherConfig::default(),
            owner,
        )));
    let completion = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let complete = {
                let observations = probe.observations.lock().unwrap();
                ["wake-complete", "pty-complete"]
                    .iter()
                    .all(|point| observations.iter().filter(|item| item.0 == *point).count() >= 2)
            };
            if complete {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), join)
        .await
        .expect("real dispatcher native join shutdown deadline")
        .expect("real dispatcher native task");
    completion.expect("both native delivery paths completed repeatedly");
    let observations = probe.observations.lock().unwrap();
    for point in ["wake-capture", "pty-capture"] {
        let owners: Vec<_> = observations.iter().filter(|item| item.0 == point).collect();
        assert!(owners.len() >= 2, "missing repeated {point}");
        for observation in owners {
            assert!(Arc::ptr_eq(observation.1.as_ref().unwrap(), &managed_owner));
        }
    }
}

impl crate::api::ApiSendSettings for ApiRuntimeProbe {
    fn api_send_settings(&self) -> SettingsState {
        crate::api::ApiSendSettings::api_send_settings(&self.native)
    }
}

impl crate::session::warnings::SessionWarningEvents for ApiRuntimeProbe {
    fn session_warning(&self, warning: crate::session::warnings::SessionWarning) {
        crate::session::warnings::SessionWarningEvents::session_warning(&self.native, warning);
    }
}
