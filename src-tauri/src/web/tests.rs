use super::*;
use crate::errors::AppError;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
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
    assert!(binary_pty_write_succeeded(Ok(()), uuid::Uuid::new_v4()));
}

#[test]
fn binary_pty_write_failure_blocks_user_message_note() {
    let uuid = uuid::Uuid::new_v4();
    let result = Err(AppError::SessionNotFound(uuid.to_string()));

    assert!(!binary_pty_write_succeeded(result, uuid));
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
    );
    global_shutdown.trigger();
    tokio::time::timeout(Duration::from_secs(1), global_server)
        .await
        .expect("global shutdown did not stop listener")
        .expect("global server task panicked");
    assert!(independent_token.is_cancelled());
}
