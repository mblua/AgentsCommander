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
