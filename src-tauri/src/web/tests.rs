use super::*;
use crate::errors::AppError;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::Manager;
use tokio::sync::{mpsc, Semaphore};

// §1453 D2: Display preserva byte a byte los strings historicos de app.log,
// incluida la forma IPv6 bracketed que solo el SocketAddr produce bien.
#[test]
fn start_server_error_display_matches_legacy_log_format() {
    let invalid = StartServerError::InvalidAddr {
        bind: "notanip".to_string(),
        port: 8888,
        detail: "invalid socket address syntax".to_string(),
    };
    assert_eq!(
        invalid.to_string(),
        "Invalid web server bind address notanip:8888: invalid socket address syntax"
    );

    let bindfail = StartServerError::BindFailed {
        bind: "192.168.1.12".to_string(),
        addr: "192.168.1.12:8888".parse().unwrap(),
        detail: "The requested address is not valid in its context. (os error 10049)".to_string(),
    };
    assert_eq!(
        bindfail.to_string(),
        include_str!("tests/fixtures/bind-error.txt")
    );

    // El crudo bracketed se conserva para la guardia, y Display usa el addr.
    let v6 = StartServerError::BindFailed {
        bind: "[::1]".to_string(),
        addr: "[::1]:8888".parse().unwrap(),
        detail: "os error 10049".to_string(),
    };
    assert_eq!(
        v6.to_string(),
        "Failed to bind web server on [::1]:8888: os error 10049"
    );
}

#[test]
fn binary_pty_write_success_allows_user_message_note() {
    assert!(commands::binary_pty_write_succeeded(
        Ok(()),
        uuid::Uuid::new_v4()
    ));
}

#[test]
fn binary_pty_write_failure_blocks_user_message_note() {
    let uuid = uuid::Uuid::new_v4();
    let result = Err(AppError::SessionNotFound(uuid.to_string()));

    assert!(!commands::binary_pty_write_succeeded(result, uuid));
}

#[tokio::test]
async fn websocket_upgrade_guard_precedes_on_upgrade() {
    let generation_token = CancellationToken::new();
    let admission = WebSocketAdmission::new(generation_token);
    let shutdown = crate::shutdown::ShutdownSignal::new();
    assert!(admission.open(&shutdown));

    let guard = acquire_websocket_upgrade_guard(&admission)
        .expect("open generation admits upgrade before callback creation");
    admission.close();
    assert_eq!(
        acquire_websocket_upgrade_guard(&admission).err(),
        Some(axum::http::StatusCode::SERVICE_UNAVAILABLE)
    );
    let wait = admission.wait();
    tokio::pin!(wait);
    assert!(tokio::time::timeout(Duration::from_millis(25), &mut wait)
        .await
        .is_err());
    drop(guard);
    tokio::time::timeout(Duration::from_secs(1), wait)
        .await
        .expect("upgrade guard drains after callback cleanup");
}

#[tokio::test]
async fn old_websocket_frames_after_stop_never_dispatch() {
    let generation_token = CancellationToken::new();
    let admission = WebSocketAdmission::new(generation_token.clone());
    let shutdown = crate::shutdown::ShutdownSignal::new();
    assert!(admission.open(&shutdown));
    admission.close();
    generation_token.cancel();

    let text_general = Arc::new(AtomicUsize::new(0));
    let text_switch = Arc::new(AtomicUsize::new(0));
    let binary_pty = Arc::new(AtomicUsize::new(0));
    for counter in [&text_general, &text_switch, &binary_pty] {
        let counter = Arc::clone(counter);
        assert!(
            !run_admitted_websocket_frame(&admission, move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
            })
            .await
        );
    }
    assert_eq!(text_general.load(Ordering::SeqCst), 0);
    assert_eq!(text_switch.load(Ordering::SeqCst), 0);
    assert_eq!(binary_pty.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stop_drains_admitted_text_and_binary_semantics() {
    let generation_token = CancellationToken::new();
    let admission = Arc::new(WebSocketAdmission::new(generation_token.clone()));
    let shutdown = crate::shutdown::ShutdownSignal::new();
    assert!(admission.open(&shutdown));
    let release = Arc::new(Semaphore::new(0));
    let (entered_sender, mut entered_receiver) = mpsc::unbounded_channel();
    let effects = Arc::new([
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    ]);
    let mut tasks = Vec::new();
    for index in 0..3 {
        let admission = Arc::clone(&admission);
        let release = Arc::clone(&release);
        let entered_sender = entered_sender.clone();
        let effects = Arc::clone(&effects);
        tasks.push(tokio::spawn(async move {
            run_admitted_websocket_frame(&admission, move || async move {
                entered_sender
                    .send(index)
                    .expect("test observes admitted semantic handler");
                let permit = release.acquire().await.expect("release semaphore open");
                permit.forget();
                effects[index].fetch_add(1, Ordering::SeqCst);
            })
            .await
        }));
    }
    drop(entered_sender);
    let mut entered = Vec::new();
    for _ in 0..3 {
        entered.push(
            tokio::time::timeout(Duration::from_secs(1), entered_receiver.recv())
                .await
                .expect("handler admission timed out")
                .expect("handler admission channel closed"),
        );
    }
    entered.sort_unstable();
    assert_eq!(entered, [0, 1, 2]);

    admission.close();
    generation_token.cancel();
    let wait = admission.wait();
    tokio::pin!(wait);
    assert!(tokio::time::timeout(Duration::from_millis(25), &mut wait)
        .await
        .is_err());
    release.add_permits(3);
    for task in tasks {
        assert!(task.await.expect("semantic task panicked"));
    }
    tokio::time::timeout(Duration::from_secs(1), wait)
        .await
        .expect("admitted semantic work drains");
    assert_eq!(effects[0].load(Ordering::SeqCst), 1); // Text general.
    assert_eq!(effects[1].load(Ordering::SeqCst), 1); // Text switch_session.
    assert_eq!(effects[2].load(Ordering::SeqCst), 1); // Binary PTY.
}

#[tokio::test]
async fn socket_cleanup_reaps_both_halves_without_aborting_admitted_handler() {
    let generation_token = CancellationToken::new();
    let connection_token = generation_token.child_token();
    let admission = Arc::new(WebSocketAdmission::new(generation_token.clone()));
    let shutdown = crate::shutdown::ShutdownSignal::new();
    assert!(admission.open(&shutdown));
    let connection_guard = admission
        .try_acquire()
        .expect("running generation admits connection");
    let handler_effect = Arc::new(AtomicBool::new(false));
    let sender_reaped = Arc::new(AtomicBool::new(false));
    let receiver_reaped = Arc::new(AtomicBool::new(false));
    let (handler_entered_sender, mut handler_entered_receiver) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));

    let sender_token = connection_token.clone();
    let sender_reaped_for_task = Arc::clone(&sender_reaped);
    let send_task = tokio::spawn(async move {
        sender_token.cancelled().await;
        sender_reaped_for_task.store(true, Ordering::SeqCst);
    });
    let recv_admission = Arc::clone(&admission);
    let recv_effect = Arc::clone(&handler_effect);
    let recv_release = Arc::clone(&release);
    let receiver_reaped_for_task = Arc::clone(&receiver_reaped);
    let recv_task = tokio::spawn(async move {
        assert!(
            run_admitted_websocket_frame(&recv_admission, move || async move {
                handler_entered_sender
                    .send(())
                    .expect("test observes admitted receive handler");
                let permit = recv_release
                    .acquire()
                    .await
                    .expect("release semaphore open");
                permit.forget();
                recv_effect.store(true, Ordering::SeqCst);
            })
            .await
        );
        receiver_reaped_for_task.store(true, Ordering::SeqCst);
    });
    handler_entered_receiver
        .recv()
        .await
        .expect("receive handler entered");
    let mut cleanup = tokio::spawn(reap_websocket_halves(
        send_task,
        recv_task,
        connection_token,
        connection_guard,
    ));

    admission.close();
    generation_token.cancel();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !sender_reaped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("send half did not observe cancellation");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut cleanup)
            .await
            .is_err()
    );
    assert!(!handler_effect.load(Ordering::SeqCst));
    assert!(!receiver_reaped.load(Ordering::SeqCst));

    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(1), cleanup)
        .await
        .expect("socket cleanup timed out")
        .expect("socket cleanup task panicked");
    tokio::time::timeout(Duration::from_secs(1), admission.wait())
        .await
        .expect("connection and frame guards did not drain");
    assert!(handler_effect.load(Ordering::SeqCst));
    assert!(sender_reaped.load(Ordering::SeqCst));
    assert!(receiver_reaped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn spawn_server_on_listener_stops_for_generation_or_global_signal() {
    let generation_shutdown = crate::shutdown::ShutdownSignal::new();
    let generation_token = CancellationToken::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind generation listener");
    let generation_server = spawn_server_on_listener(
        listener,
        Router::new(),
        generation_token.clone(),
        generation_shutdown.clone(),
        |future| ServerTask::from_native(tauri::async_runtime::spawn(future)),
    );
    generation_token.cancel();
    tokio::time::timeout(Duration::from_secs(1), generation_server)
        .await
        .expect("generation cancellation did not stop listener")
        .expect("generation server task panicked");
    assert!(!generation_shutdown.is_cancelled());

    let global_shutdown = crate::shutdown::ShutdownSignal::new();
    let independent_token = CancellationToken::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind global listener");
    let global_server = spawn_server_on_listener(
        listener,
        Router::new(),
        independent_token.clone(),
        global_shutdown.clone(),
        |future| ServerTask::from_native(tauri::async_runtime::spawn(future)),
    );
    global_shutdown.trigger();
    tokio::time::timeout(Duration::from_secs(1), global_server)
        .await
        .expect("global shutdown did not stop listener")
        .expect("global server task panicked");
    assert!(independent_token.is_cancelled());
}

struct ControlledDispatcher {
    calls: Mutex<Vec<serde_json::Value>>,
    binaries: AtomicUsize,
    effects: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
    blocked: bool,
    broadcaster: WsBroadcaster,
}

impl ControlledDispatcher {
    fn new(broadcaster: WsBroadcaster, blocked: bool) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            binaries: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            blocked,
            broadcaster,
        }
    }

    async fn effect(&self) {
        self.entered.add_permits(1);
        if self.blocked {
            self.release.acquire().await.unwrap().forget();
        }
        self.effects.fetch_add(1, Ordering::SeqCst);
    }
}

impl WebCommandDispatcher for ControlledDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: u64,
        cmd: &'a str,
        args: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send + 'a>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(serde_json::json!([id, cmd, args]));
            self.effect().await;
            self.broadcaster
                .broadcast_event("command_effect", &serde_json::json!(id));
            if cmd == "error" {
                serde_json::json!({"id": id, "error": "sentinel error"})
            } else {
                serde_json::json!({"sentinel": [id, cmd, args]})
            }
        })
    }

    fn binary<'a>(
        &'a self,
        data: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            assert_eq!(data, b"binary input");
            self.binaries.fetch_add(1, Ordering::SeqCst);
            self.effect().await;
        })
    }
}

fn controlled_context(blocked: bool) -> (WebSocketConnectionContext, Arc<ControlledDispatcher>) {
    let broadcaster = WsBroadcaster::new();
    let dispatcher = Arc::new(ControlledDispatcher::new(broadcaster.clone(), blocked));
    let generation_token = CancellationToken::new();
    let admission = Arc::new(WebSocketAdmission::new(generation_token.clone()));
    assert!(admission.open(&crate::shutdown::ShutdownSignal::new()));
    let context = WebSocketConnectionContext {
        broadcaster,
        dispatcher: dispatcher.clone(),
        admission,
        generation_token,
    };
    (context, dispatcher)
}

async fn observed_event(receiver: &mut mpsc::Receiver<broadcast::WsOutMsg>) -> serde_json::Value {
    let message = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("broadcast timed out")
        .expect("broadcast closed");
    let broadcast::WsOutMsg::Text(text) = message else {
        panic!("expected JSON event");
    };
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn text_dispatch_preserves_inputs_envelope_and_event_order() {
    let (context, dispatcher) = controlled_context(false);
    let mut events = context.broadcaster.subscribe();
    let args = serde_json::json!({"nested": [{"value": [true, null, "input"]}]});
    handle_text_message(
        &context,
        &serde_json::json!({"id": 42, "cmd": "sentinel", "args": args}).to_string(),
    )
    .await;
    assert_eq!(
        *dispatcher.calls.lock().unwrap(),
        vec![serde_json::json!([42, "sentinel", args])]
    );
    assert_eq!(
        observed_event(&mut events).await,
        serde_json::json!({"event": "command_effect", "payload": 42})
    );
    assert_eq!(
        observed_event(&mut events).await,
        serde_json::json!({"event": "__cmd_response", "payload": {
            "id": 42, "data": {"sentinel": [42, "sentinel", args]}
        }})
    );
    for invalid in ["not JSON", "{}", r#"{"id": 9, "cmd": null}"#] {
        handle_text_message(&context, invalid).await;
    }
    assert_eq!(dispatcher.calls.lock().unwrap().len(), 1);
    assert!(events.try_recv().is_err());
    handle_text_message(&context, r#"{"cmd":"error"}"#).await;
    assert_eq!(
        dispatcher.calls.lock().unwrap()[1],
        serde_json::json!([0, "error", {}])
    );
    assert_eq!(observed_event(&mut events).await["event"], "command_effect");
    assert_eq!(
        observed_event(&mut events).await,
        serde_json::json!({"event": "__cmd_response", "payload": {
            "id": 0, "data": {"id": 0, "error": "sentinel error"}
        }})
    );
}

fn controlled_frame(binary: bool) -> Message {
    if binary {
        Message::Binary(b"binary input".to_vec().into())
    } else {
        Message::Text(r#"{"id":42,"cmd":"sentinel","args":{"nested":true}}"#.into())
    }
}

async fn assert_frame_admission(binary: bool) {
    let (context, dispatcher) = controlled_context(false);
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        handle_ws_frame(&context, controlled_frame(binary)),
    )
    .await
    .unwrap());
    assert_eq!(dispatcher.effects.load(Ordering::SeqCst), 1);
    assert_eq!(dispatcher.calls.lock().unwrap().len(), usize::from(!binary));
    assert_eq!(
        dispatcher.binaries.load(Ordering::SeqCst),
        usize::from(binary)
    );
    context.admission.close();
    context.generation_token.cancel();
    assert!(!tokio::time::timeout(
        Duration::from_secs(1),
        handle_ws_frame(&context, controlled_frame(binary)),
    )
    .await
    .unwrap());
    assert_eq!(dispatcher.effects.load(Ordering::SeqCst), 1);
    assert_eq!(dispatcher.calls.lock().unwrap().len(), usize::from(!binary));
    assert_eq!(
        dispatcher.binaries.load(Ordering::SeqCst),
        usize::from(binary)
    );
}

#[tokio::test]
async fn frame_text_admission() {
    assert_frame_admission(false).await;
}

#[tokio::test]
async fn frame_binary_admission() {
    assert_frame_admission(true).await;
}

async fn assert_frame_drain(binary: bool) {
    let (context, dispatcher) = controlled_context(true);
    let mut events = context.broadcaster.subscribe();
    let task_context = context.clone();
    let mut frame =
        tokio::spawn(async move { handle_ws_frame(&task_context, controlled_frame(binary)).await });
    tokio::time::timeout(Duration::from_secs(1), dispatcher.entered.acquire())
        .await
        .expect("port not entered")
        .unwrap()
        .forget();
    context.admission.close();
    context.generation_token.cancel();
    let drain = context.admission.wait();
    tokio::pin!(drain);
    assert!(tokio::time::timeout(Duration::from_millis(25), &mut frame)
        .await
        .is_err());
    assert!(tokio::time::timeout(Duration::from_millis(25), &mut drain)
        .await
        .is_err());
    assert_eq!(dispatcher.effects.load(Ordering::SeqCst), 0);
    dispatcher.release.add_permits(1);
    assert!(tokio::time::timeout(Duration::from_secs(1), frame)
        .await
        .unwrap()
        .unwrap());
    assert_eq!(dispatcher.effects.load(Ordering::SeqCst), 1);
    if !binary {
        assert_eq!(observed_event(&mut events).await["event"], "command_effect");
        assert_eq!(
            observed_event(&mut events).await,
            serde_json::json!({"event": "__cmd_response", "payload": {
                "id": 42, "data": {"sentinel": [42, "sentinel", {"nested": true}]}
            }})
        );
    }
    tokio::time::timeout(Duration::from_secs(1), drain)
        .await
        .expect("frame did not drain");
}

#[tokio::test]
async fn frame_text_drain() {
    assert_frame_drain(false).await;
}

#[tokio::test]
async fn frame_binary_drain() {
    assert_frame_drain(true).await;
}

fn real_adapter_state() -> (tauri::App, commands::WsState) {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let settings = crate::config::settings::new_settings_state(
        crate::config::settings::AppSettings::default(),
    );
    let app = crate::test_support::test_builder()
        .manage(Arc::clone(&settings))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
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
    let state = commands::WsState::new(
        session_mgr,
        pty_mgr,
        settings,
        WsBroadcaster::new(),
        app.handle().clone(),
    );
    (app, state)
}

fn begin_test_generation(
    handle: &crate::WebServerHandle,
    state: &commands::WsState,
    app: &tauri::AppHandle,
) -> crate::WebServerStartWaiter {
    crate::commands::config::begin_web_server_start(
        handle.clone(),
        Arc::clone(&state.settings),
        Arc::new(WebAccessToken::new("p02-loopback-token".to_string())),
        Arc::clone(&state.session_mgr),
        Arc::clone(&state.pty_mgr),
        state.broadcaster.clone(),
        app.clone(),
        crate::shutdown::ShutdownSignal::new(),
    )
}

async fn stopped_generation(handle: &crate::WebServerHandle) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while handle.snapshot().lifecycle != crate::WebServerLifecycle::Stopped {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("generation did not settle Stopped");
}

async fn loopback_client(port: u16) -> tokio::net::TcpStream {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let request = format!(
        "GET /ws?token=p02-loopback-token HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Upgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        response.push(client.read_u8().await.unwrap());
    }
    assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 101"));
    client
}

async fn real_command_response(client: &mut tokio::net::TcpStream) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let request = br#"{"id":73,"cmd":"list_sessions","args":{}}"#;
    // RFC6455 client frames must be masked; a zero mask preserves these payload bytes.
    let mut frame = vec![
        0x81,
        0x80 | u8::try_from(request.len()).unwrap(),
        0,
        0,
        0,
        0,
    ];
    frame.extend_from_slice(request);
    client.write_all(&frame).await.unwrap();
    assert_eq!(client.read_u8().await.unwrap(), 0x81);
    let length = match client.read_u8().await.unwrap() {
        126 => usize::from(client.read_u16().await.unwrap()),
        n @ 0..=125 => usize::from(n),
        n => panic!("unexpected frame length {n}"),
    };
    let mut payload = vec![0; length];
    client.read_exact(&mut payload).await.unwrap();
    serde_json::from_slice(&payload).unwrap()
}

#[tokio::test]
async fn disabled_web_settings_allow_manual_start_with_registered_adapter() {
    let (app, state) = real_adapter_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    {
        let mut settings = state.settings.write().await;
        settings.web_server_enabled = false;
        settings.web_server_bind = "127.0.0.1".to_string();
        settings.web_server_port = port;
    }
    assert!(
        app.manage(command_port::ManagedWebCommandDispatcher(Arc::new(
            commands::WebCommandsDispatcher::new(state.clone()),
        )))
    );
    let handle = crate::WebServerHandle::default();
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            begin_test_generation(&handle, &state, app.handle()).wait()
        )
        .await
        .unwrap(),
        Ok(true)
    );
    assert_eq!(
        handle.snapshot().endpoint,
        Some(("127.0.0.1".to_string(), port))
    );
    let mut client = tokio::time::timeout(Duration::from_secs(1), loopback_client(port))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), real_command_response(&mut client))
            .await
            .unwrap(),
        serde_json::json!({"event": "__cmd_response", "payload": {
            "id": 73, "data": {"id": 73, "result": []}
        }})
    );
    tokio::time::timeout(Duration::from_secs(1), handle.begin_stop().unwrap().wait())
        .await
        .unwrap()
        .unwrap();
    stopped_generation(&handle).await;
    assert!(!state.settings.read().await.web_server_enabled);
}

#[tokio::test]
async fn missing_dispatcher_fails_before_settings_endpoint_or_bind_and_retry_succeeds() {
    let (app, state) = real_adapter_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut settings = state.settings.write().await;
    settings.web_server_bind = "127.0.0.1".to_string();
    settings.web_server_port = port;
    let handle = crate::WebServerHandle::default();
    // Holding the settings write lock proves missing wiring fails before reading settings.
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            begin_test_generation(&handle, &state, app.handle()).wait()
        )
        .await
        .expect("missing dispatcher blocked on settings"),
        Err("Web command dispatcher is not registered".to_string())
    );
    drop(settings);
    stopped_generation(&handle).await;
    assert_eq!(handle.snapshot().endpoint, None);
    assert!(handle.last_bind_failure().is_none());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("missing dispatcher left a listener");
    drop(listener);
    assert!(
        app.manage(command_port::ManagedWebCommandDispatcher(Arc::new(
            commands::WebCommandsDispatcher::new(state.clone()),
        )))
    );
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            begin_test_generation(&handle, &state, app.handle()).wait()
        )
        .await
        .unwrap(),
        Ok(true)
    );
    tokio::time::timeout(Duration::from_secs(1), handle.begin_stop().unwrap().wait())
        .await
        .unwrap()
        .unwrap();
    stopped_generation(&handle).await;
}

#[test]
fn bootstrap_registers_dispatcher_unconditionally_before_autostart() {
    let source = include_str!("../app_run/setup.rs");
    let source: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::split_whitespace)
        .collect();
    let anchor = "app.manage(commands::window::WatchersScopeState::default());";
    let region = source.split_once(anchor).unwrap().1;
    assert!(
        region.starts_with("letdispatcher="),
        "registration must be unconditional"
    );
    let manage = "if!app.manage(crate::web::command_port::ManagedWebCommandDispatcher(";
    assert_eq!(region.matches(manage).count(), 1);
    let position = region.find(manage).unwrap();
    let prefix = &region[..position];
    assert_eq!(prefix.matches('{').count(), prefix.matches('}').count());
    assert!(
        !prefix.contains("if"),
        "registration nested under a condition"
    );
    let enabled = region.find("ifweb_settings.web_server_enabled{").unwrap();
    let start = region
        .find("commands::config::begin_web_server_start(")
        .unwrap();
    assert!(position < enabled && enabled < start);
    assert!(region[position..enabled]
        .contains("returnErr(\"Webcommanddispatcherisalreadyregistered\".into());"));
}
