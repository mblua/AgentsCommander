use super::*;
use crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest;

#[cfg(target_os = "windows")]
struct ServerGuard(tokio::task::JoinHandle<()>);

#[cfg(target_os = "windows")]
impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(target_os = "windows")]
type CaptureFactoryForTest = std::sync::Arc<
    dyn Fn(String, crate::api::WindowScreenshotLease) -> WindowScreenshotCaptureFutureForTest
        + Send
        + Sync,
>;

#[cfg(target_os = "windows")]
async fn send_queued_window_request(
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

#[cfg(target_os = "windows")]
async fn send_revocation_window_request(
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

#[cfg(target_os = "windows")]
async fn create_window_screenshot_fixture() -> AcceptanceFixture {
    let temporary = match tempfile::Builder::new()
        .prefix("window-screenshot-fixture-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    {
        Ok(temporary) => temporary,
        Err(error) => panic!("fixture temporary directory must be created: {error}"),
    };
    AcceptanceFixture::new(temporary).await
}

#[cfg(target_os = "windows")]
async fn spawn_window_screenshot_server(
    state: crate::api::ApiState,
    capture_factory: CaptureFactoryForTest,
) -> (std::net::SocketAddr, ServerGuard) {
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
    let server = ServerGuard(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    }));
    (address, server)
}

#[cfg(target_os = "windows")]
async fn wait_for_full_admission(
    state: &crate::api::ApiState,
) -> tokio::sync::OwnedSemaphorePermit {
    match tokio::time::timeout(std::time::Duration::from_secs(5), async {
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
    }
}

#[cfg(target_os = "windows")]
async fn await_active_and_queued(
    active: tokio::task::JoinHandle<reqwest::Response>,
    queued: tokio::task::JoinHandle<reqwest::Response>,
) -> (reqwest::Response, reqwest::Response) {
    let active = match active.await {
        Ok(response) => response,
        Err(error) => panic!("active request task must complete: {error}"),
    };
    let queued = match queued.await {
        Ok(response) => response,
        Err(error) => panic!("queued request task must complete: {error}"),
    };
    (active, queued)
}

#[cfg(target_os = "windows")]
fn revocation_capture_factory(
    calls: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    active_started: &std::sync::Arc<tokio::sync::Notify>,
    release_active: &std::sync::Arc<tokio::sync::Notify>,
) -> CaptureFactoryForTest {
    std::sync::Arc::new({
        let calls = std::sync::Arc::clone(calls);
        let active_started = std::sync::Arc::clone(active_started);
        let release_active = std::sync::Arc::clone(release_active);
        move |window_id: String,
              lease: crate::api::WindowScreenshotLease|
              -> WindowScreenshotCaptureFutureForTest {
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
    })
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn window_screenshot_fresh_guard_is_released_before_queue_wait() {
    let fixture = create_window_screenshot_fixture().await;
    let token = fixture.api_coordinator_token.secret.clone();
    let state = direct_api_state(&fixture);
    let active_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_active = std::sync::Arc::new(tokio::sync::Notify::new());
    let capture_factory = std::sync::Arc::new({
        let active_started = std::sync::Arc::clone(&active_started);
        let release_active = std::sync::Arc::clone(&release_active);
        move |window_id, lease| -> WindowScreenshotCaptureFutureForTest {
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
    let (address, _server) = spawn_window_screenshot_server(state.clone(), capture_factory).await;

    let active = tokio::spawn(send_queued_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "1",
    ));
    active_started.notified().await;
    let queued = tokio::spawn(send_queued_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "2",
    ));

    let admission_probe = wait_for_full_admission(&state).await;

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
        Err(e) => panic!("queued screenshot request retained a freshness registry lock: {e}"),
    };
    drop(registry_lock);
    drop(admission_probe);

    release_active.notify_one();
    let (active, queued) = await_active_and_queued(active, queued).await;
    assert_eq!(active.status(), axum::http::StatusCode::OK);
    assert_eq!(queued.status(), axum::http::StatusCode::OK);

    let fresh_after_probe =
        send_queued_window_request(reqwest::Client::new(), address, token, "3").await;
    assert_eq!(fresh_after_probe.status(), axum::http::StatusCode::OK);
}

#[cfg(target_os = "windows")]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn window_screenshot_queued_revocation_blocks_launch_revalidation() {
    let fixture = create_window_screenshot_fixture().await;
    let token = fixture.api_coordinator_token.secret.clone();
    let client_id = fixture.api_coordinator_token.client_id.clone();
    let state = direct_api_state(&fixture);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let active_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_active = std::sync::Arc::new(tokio::sync::Notify::new());
    let capture_factory = revocation_capture_factory(&calls, &active_started, &release_active);
    let (address, _server) = spawn_window_screenshot_server(state.clone(), capture_factory).await;

    let _audit_lock = crate::api::audit::lock_window_screenshot_audits_for_test();
    let _ = crate::api::audit::take_window_screenshot_audits_for_test();
    let active = tokio::spawn(send_revocation_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "1",
    ));
    active_started.notified().await;
    let queued = tokio::spawn(send_revocation_window_request(
        reqwest::Client::new(),
        address,
        token.clone(),
        "2",
    ));

    let admission_probe = wait_for_full_admission(&state).await;
    match crate::api::auth::revoke(&fixture.registry_path, &client_id) {
        Ok(true) => {}
        Ok(false) => panic!("fixture credential revocation must revoke the valid client"),
        Err(error) => panic!("fixture credential revocation must succeed: {error}"),
    }
    drop(admission_probe);

    release_active.notify_one();
    let (active, queued) = await_active_and_queued(active, queued).await;
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
