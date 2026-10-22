use super::*;
use crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest;

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
            move |window_id, _lease| -> WindowScreenshotCaptureFutureForTest {
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
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer \
                {token}\r\nConnection: close\r\n\r\n"
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
        move |window_id, _lease| -> WindowScreenshotCaptureFutureForTest {
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
            "GET /api/v1/windows/1/screenshot HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer \
                {token}\r\nConnection: close\r\n\r\n"
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
        move |window_id, lease| -> WindowScreenshotCaptureFutureForTest {
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
                        Err(_) => Err(crate::screenshot::WindowScreenshotCaptureError::Unavailable),
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
