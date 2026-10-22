use super::*;
use crate::api::handlers::window_screenshot::WindowScreenshotCaptureFutureForTest;

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
            "GET /api/v1/windows/{window_id}/screenshot HTTP/1.1\r\nHost: \
                {address}\r\nAuthorization: Bearer {token}\r\nConnection: keep-alive\r\n\r\n"
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
        "the manually closed client must receive no response bytes before the native worker \
            finishes"
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
        "the post-release request must receive its own PNG response rather than a retained shared \
            result"
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
        move |window_id, _lease| -> WindowScreenshotCaptureFutureForTest {
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
        move |window_id, lease| -> WindowScreenshotCaptureFutureForTest {
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
