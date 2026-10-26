use super::*;

async fn wait_for_web_lifecycle(
    handle: &WebServerHandle,
    expected: WebServerLifecycle,
) -> WebServerLifecycleSnapshot {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = handle.snapshot();
            if snapshot.lifecycle == expected {
                return snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("web lifecycle did not converge")
}

async fn start_test_web_generation(
    handle: &WebServerHandle,
    bind: &str,
    port: u16,
) -> (u64, Arc<crate::web::WebSocketAdmission>) {
    let lifecycle = handle.clone();
    let bind = bind.to_string();
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let (admission_sender, admission_receiver) = oneshot::channel();
    let waiter = handle.begin_start(
        shutdown,
        move |generation, admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(generation, bind, port));
            assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
            let server = tauri::async_runtime::spawn(async move {
                generation_token.cancelled().await;
            });
            Ok::<_, String>(Some(server))
        },
    );
    let admission = tokio::time::timeout(Duration::from_secs(2), admission_receiver)
        .await
        .expect("factory did not expose admission")
        .expect("factory dropped admission sender");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), waiter.wait())
            .await
            .expect("start waiter timed out"),
        Ok(true)
    );
    let snapshot = wait_for_web_lifecycle(handle, WebServerLifecycle::Running).await;
    assert_eq!(snapshot.endpoint, Some(("127.0.0.1".to_string(), port)));
    (
        snapshot.generation.expect("running generation has id"),
        admission,
    )
}

#[tokio::test]
async fn web_server_stop_during_start_is_sticky() {
    let handle = WebServerHandle::default();
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let lifecycle = handle.clone();
    let (entered_sender, entered_receiver) = oneshot::channel();
    let (release_sender, release_receiver) = oneshot::channel();
    let (admission_sender, admission_receiver) = oneshot::channel();
    let first = handle.begin_start(
        shutdown.clone(),
        move |generation, admission, _generation_token| async move {
            assert!(entered_sender.send(()).is_ok());
            assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
            release_receiver
                .await
                .expect("test releases blocked start factory");
            if !lifecycle.publish_effective_endpoint(generation, "127.0.0.1".to_string(), 8765) {
                return Err(WEB_SERVER_START_CANCELLED.to_string());
            }
            panic!("a stopped generation must not reach bind");
        },
    );
    entered_receiver.await.expect("factory entered");
    let admission = admission_receiver.await.expect("admission exposed");

    let unexpected_factory_calls = Arc::new(AtomicU64::new(0));
    let calls = Arc::clone(&unexpected_factory_calls);
    let second = handle.begin_start(
        shutdown,
        move |_generation, _admission, _generation_token| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(None)
        },
    );
    let stop = handle.begin_stop().expect("starting generation is owned");
    let stopping = handle.snapshot();
    assert_eq!(stopping.lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(stopping.endpoint, None);
    assert!(admission.try_acquire().is_none());
    assert!(release_sender.send(()).is_ok());

    let first_result = first.wait().await;
    let second_result = second.wait().await;
    assert_eq!(first_result, second_result);
    assert_eq!(first_result, Err(WEB_SERVER_START_CANCELLED.to_string()));
    assert_eq!(unexpected_factory_calls.load(Ordering::SeqCst), 0);
    assert_eq!(stop.wait().await, Ok(()));
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
    assert!(admission.is_empty());
}

async fn assert_stop_after_start_output_cancels_result(
    factory_output: Result<Option<tauri::async_runtime::JoinHandle<()>>, String>,
) {
    let handle = WebServerHandle::default();
    let (output_reached, release_output) = handle.gate_next_start_output();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |_generation, _admission, _generation_token| async move { factory_output },
    );

    assert!(
        output_reached
            .await
            .expect("supervisor reports the observed lifecycle"),
        "factory output must be complete while the generation is still Starting"
    );
    let stop = handle
        .begin_stop()
        .expect("Stop linealizes before start-result publication");
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    release_output
        .send(())
        .expect("release start-result publication");

    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    assert_eq!(stop.wait().await, Ok(()));
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
}

#[tokio::test]
async fn web_server_stop_between_factory_output_and_start_result_publication_is_sticky() {
    assert_stop_after_start_output_cancels_result(Err("late factory error".to_string())).await;
    assert_stop_after_start_output_cancels_result(Ok(None)).await;
}

#[tokio::test]
async fn web_server_stop_waits_for_generation_drain() {
    let handle = WebServerHandle::default();
    let (_generation, admission) = start_test_web_generation(&handle, "127.0.0.1", 8765).await;
    let connection_guard = admission
        .try_acquire()
        .expect("running generation admits connection");
    let frame_guard = admission
        .try_acquire()
        .expect("running generation admits frame");
    let waiter = handle.begin_stop().expect("running generation is owned");
    let mut stop_task = tokio::spawn(waiter.wait());

    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut stop_task)
            .await
            .is_err()
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    drop(frame_guard);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut stop_task)
            .await
            .is_err()
    );
    drop(connection_guard);

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), stop_task)
            .await
            .expect("drain stop task timed out")
            .expect("drain stop task panicked"),
        Ok(())
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
}

#[tokio::test]
async fn web_server_stop_timeout_is_shared_and_fail_closed() {
    let handle = WebServerHandle::default();
    let (_generation, admission) = start_test_web_generation(&handle, "127.0.0.1", 8766).await;
    let retained_guard = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let first = handle
        .begin_stop_with_timeout(Duration::from_millis(40))
        .expect("running generation is owned");
    let second = handle
        .begin_stop_with_timeout(Duration::from_secs(30))
        .expect("stopping generation remains owned");
    assert_eq!(first.deadline(), second.deadline());

    let (first_result, second_result) = tokio::join!(first.wait(), second.wait());
    assert_eq!(first_result, second_result);
    assert_eq!(
        first_result,
        Err("Timed out waiting for web server generation to stop".to_string())
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    let rejected_start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        |_generation, _admission, _generation_token| async { Ok::<_, String>(None) },
    );
    assert_eq!(
        rejected_start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );

    drop(retained_guard);
    wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopped).await;
}

#[tokio::test]
async fn web_server_lost_stop_channel_is_fail_closed() {
    let handle = WebServerHandle::default();
    let (_generation, admission) = start_test_web_generation(&handle, "127.0.0.1", 8770).await;
    let retained_guard = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let generation_stop = handle.begin_stop().expect("running generation is owned");
    let (sender, receiver) = watch::channel(None);
    drop(sender);
    let lost_channel = WebServerStopWaiter {
        receiver,
        deadline: Instant::now() + Duration::from_secs(1),
    };

    assert_eq!(
        lost_channel.wait().await,
        Err("Web server stop supervisor channel closed before terminal state".to_string())
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);

    drop(retained_guard);
    generation_stop
        .wait()
        .await
        .expect("real generation Stop drains after channel-loss seam");
}

#[tokio::test]
async fn web_server_restart_uses_new_generation_and_ignores_stale_completion() {
    let handle = WebServerHandle::default();
    let (generation_one, _admission_one) =
        start_test_web_generation(&handle, "127.0.0.1", 8767).await;
    handle
        .begin_stop()
        .expect("first generation is owned")
        .wait()
        .await
        .expect("first generation drains");
    let (generation_two, _admission_two) =
        start_test_web_generation(&handle, "127.0.0.1", 8768).await;
    assert!(generation_two > generation_one);
    let current = handle.snapshot();

    assert!(!handle.move_generation_to_stopping(generation_one));
    assert!(!handle.finish_generation(generation_one));
    assert_eq!(handle.snapshot(), current);

    handle
        .begin_stop()
        .expect("second generation is owned")
        .wait()
        .await
        .expect("second generation drains");
}

#[tokio::test]
async fn web_server_finished_outer_task_converges_after_start_terminal() {
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let waiter = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |generation, _admission, _generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(
                generation,
                "127.0.0.1".to_string(),
                8769,
            ));
            Ok::<_, String>(Some(tauri::async_runtime::spawn(async {})))
        },
    );
    assert_eq!(waiter.wait().await, Ok(true));
    wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopped).await;
}

fn web_server_loopback_test_state() -> (
    tauri::App,
    Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>,
    Arc<Mutex<crate::pty::manager::PtyManager>>,
    SettingsState,
) {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(
        crate::session::manager::SessionManager::new(),
    ));
    let settings: SettingsState =
        crate::config::settings::new_settings_state(AppSettings::default());
    let app = crate::test_support::test_builder()
        .manage(Arc::clone(&settings))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build loopback websocket test app");
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
    (app, session_mgr, pty_mgr, settings)
}

#[tokio::test]
async fn real_loopback_websocket_is_closed_and_reaped_by_generation_stop() {
    let (app, session_mgr, pty_mgr, settings) = web_server_loopback_test_state();
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let generation_token = CancellationToken::new();
    let admission = Arc::new(crate::web::WebSocketAdmission::new(
        generation_token.clone(),
    ));
    assert!(admission.open(&shutdown));
    let broadcaster = crate::web::broadcast::WsBroadcaster::new();
    let dispatcher =
        crate::web::commands::WebCommandsDispatcher::new(crate::web::commands::WsState::new(
            Arc::clone(&session_mgr),
            pty_mgr,
            settings,
            broadcaster.clone(),
            app.handle().clone(),
        ));
    let router = crate::web::build_router(
        Arc::new(crate::web::auth::WebAccessToken::new(
            "loopback-test-token".to_string(),
        )),
        session_mgr,
        Arc::new(dispatcher),
        broadcaster,
        Arc::clone(&admission),
        generation_token.clone(),
        None,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback websocket listener");
    let addr = listener.local_addr().expect("loopback listener address");
    let server =
        crate::web::spawn_server_on_listener(listener, router, generation_token.clone(), shutdown);
    let mut client = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect loopback websocket client");
    let request = format!(
        "GET /ws?token=loopback-test-token HTTP/1.1\r\nHost: {addr}\r\nUpgrade: \
            websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: \
            dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    client
        .write_all(request.as_bytes())
        .await
        .expect("write RFC 6455 handshake");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut chunk = [0_u8; 512];
        while !response.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = client
                .read(&mut chunk)
                .await
                .expect("read RFC 6455 handshake");
            assert!(read > 0, "server closed before handshake completed");
            response.extend_from_slice(&chunk[..read]);
        }
    })
    .await
    .expect("RFC 6455 handshake timed out");
    assert!(
        String::from_utf8_lossy(&response).starts_with("HTTP/1.1 101"),
        "unexpected handshake: {}",
        String::from_utf8_lossy(&response)
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while admission.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("connection guard was not tracked");

    admission.close();
    generation_token.cancel();
    let mut eof = [0_u8; 1];
    let (server_result, read_result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(server, client.read(&mut eof), admission.wait())
    })
    .await
    .expect("generation stop did not reap loopback websocket");
    server_result.expect("loopback server task panicked");
    assert_eq!(read_result.expect("read client EOF"), 0);
    assert!(admission.is_empty());
}

#[test]
fn web_server_handle_bind_failure_roundtrip() {
    let handle = WebServerHandle::default();
    assert!(handle.last_bind_failure().is_none());

    let failure = crate::web::StartServerError::BindFailed {
        bind: "192.168.1.12".to_string(),
        addr: "192.168.1.12:8888".parse().unwrap(),
        detail: "os error 10049".to_string(),
    };
    handle.record_bind_failure(failure.clone());
    assert_eq!(
        handle
            .last_bind_failure()
            .expect("failure must be recorded"),
        failure
    );

    handle.clear_bind_failure();
    assert!(handle.last_bind_failure().is_none());
}

#[tokio::test]
async fn api_server_handle_shutdown_cancels_running_task() {
    let handle = ApiServerHandle::default();
    let shutdown = CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let join = tauri::async_runtime::spawn(async move {
        task_shutdown.cancelled().await;
    });

    assert!(handle
        .store_if_idle(ApiServerTask::new(join, shutdown, api_test_addr(9906)))
        .unwrap());
    assert!(handle.has_running().unwrap());
    assert_eq!(
        handle.running_bound_addr().unwrap(),
        Some(api_test_addr(9906))
    );
    assert!(handle
        .shutdown_running(Duration::from_secs(1))
        .await
        .unwrap());
    assert!(!handle.has_running().unwrap());
}

#[tokio::test]
async fn api_server_handle_rejects_duplicate_running_task() {
    let handle = ApiServerHandle::default();
    let first_shutdown = CancellationToken::new();
    let first_task_shutdown = first_shutdown.clone();
    let first_join = tauri::async_runtime::spawn(async move {
        first_task_shutdown.cancelled().await;
    });
    assert!(handle
        .store_if_idle(ApiServerTask::new(
            first_join,
            first_shutdown,
            api_test_addr(9906),
        ))
        .unwrap());

    let second_shutdown = CancellationToken::new();
    let second_observer = second_shutdown.clone();
    let second_task_shutdown = second_shutdown.clone();
    let second_join = tauri::async_runtime::spawn(async move {
        second_task_shutdown.cancelled().await;
    });
    assert!(!handle
        .store_if_idle(ApiServerTask::new(
            second_join,
            second_shutdown,
            api_test_addr(9907),
        ))
        .unwrap());
    assert!(second_observer.is_cancelled());
    assert!(handle.has_running().unwrap());

    assert!(handle
        .shutdown_running(Duration::from_secs(1))
        .await
        .unwrap());
}
