use super::*;
use crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest;

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
        Err(error) => {
            panic!("queued screenshot request retained a freshness registry lock: {error}")
        }
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
        move |window_id, lease| -> WindowScreenshotCaptureFutureForTest {
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
