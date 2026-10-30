use super::*;

fn controlled_retry_wait() -> (
    RetryWait,
    mpsc::UnboundedReceiver<tokio::sync::oneshot::Sender<()>>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let wait: RetryWait = Arc::new(move |delay| {
        assert_eq!(delay, Duration::from_secs(2));
        let (release, gate) = tokio::sync::oneshot::channel();
        tx.send(release).unwrap();
        Box::pin(async move {
            gate.await.expect("release retry wait");
        })
    });
    (wait, rx)
}

async fn next_retry(
    rx: &mut mpsc::UnboundedReceiver<tokio::sync::oneshot::Sender<()>>,
) -> tokio::sync::oneshot::Sender<()> {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

fn held_ports(fixture: &RoomFixture) -> Arc<RecordingPorts> {
    let ports = Arc::new(RecordingPorts::new(
        dials(true, true),
        Some(production_target(&fixture.room)),
    ));
    ports
        .attempts
        .lock()
        .unwrap()
        .push_back((Err("typing_hold_deferred: held".into()), None));
    ports
}

#[tokio::test]
async fn serial_hold_can_fill_the_existing_transition_channel() {
    let fixture = room_fixture();
    let ports = held_ports(&fixture);
    let (wait, mut waits) = controlled_retry_wait();
    let (tx, rx) = mpsc::channel(1024);
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(run_with_ports(
        ports.clone(),
        rx,
        shutdown,
        Arc::new(Instant::now),
        wait,
    ));
    tx.send(transition(TransitionKind::CiStarted, &fixture.room, "A"))
        .await
        .unwrap();
    let release = next_retry(&mut waits).await;
    for i in 0..1024 {
        tx.try_send(transition(
            TransitionKind::CiStarted,
            &fixture.room,
            &format!("B-{i}"),
        ))
        .unwrap();
    }
    let x = transition(TransitionKind::CiStarted, &fixture.room, "X-1025");
    let dropped = match tx.try_send(x) {
        Err(mpsc::error::TrySendError::Full(t)) => t,
        result => panic!("expected Full for X: {result:?}"),
    };
    assert_eq!(dropped.repo_path, "X-1025");
    assert_eq!(ports.deliveries().len(), 1);
    assert_eq!(ports.dial_reads.load(Ordering::SeqCst), 1);
    assert_eq!(ports.completed.load(Ordering::SeqCst), 0);
    // A remains admitted even when its preference is turned off in the wait.
    ports.set_dials(dials(false, false));
    drop(tx);
    release.send(()).unwrap();
    task.await.unwrap();
    assert_eq!(ports.resolutions(), 1);
    assert_eq!(ports.completed.load(Ordering::SeqCst), 1);
    {
        let originals = ports.originals.lock().unwrap();
        assert_eq!(originals.len(), 2);
        assert_eq!(originals[0], originals[1]);
    }
    // Queue-only BASE control: consuming one item makes this same ID fit.
    let (base_tx, mut base_rx) = mpsc::channel(1024);
    for _ in 0..1024 {
        base_tx
            .try_send(transition(
                TransitionKind::CiStarted,
                &fixture.room,
                "queued",
            ))
            .unwrap();
    }
    base_rx.recv().await.unwrap();
    base_tx.try_send(dropped).unwrap();
}

#[tokio::test]
async fn shutdown_during_a_full_hold_wait_keeps_no_delivery_success() {
    let fixture = room_fixture();
    let ports = held_ports(&fixture);
    let (wait, mut waits) = controlled_retry_wait();
    let (tx, rx) = mpsc::channel(1024);
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(run_with_ports(
        ports.clone(),
        rx,
        shutdown.clone(),
        Arc::new(Instant::now),
        wait,
    ));
    tx.send(transition(TransitionKind::CiStarted, &fixture.room, "A"))
        .await
        .unwrap();
    let release = next_retry(&mut waits).await;
    for _ in 0..1024 {
        tx.try_send(transition(TransitionKind::CiStarted, &fixture.room, "B"))
            .unwrap();
    }
    shutdown.cancel();
    task.await.unwrap();
    assert!(release.send(()).is_err());
    assert_eq!(ports.deliveries().len(), 1);
    assert_eq!(ports.dial_reads.load(Ordering::SeqCst), 1);
    assert_eq!(ports.completed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn closed_sender_does_not_drop_the_active_deferred_notice() {
    let fixture = room_fixture();
    let ports = held_ports(&fixture);
    let (wait, mut waits) = controlled_retry_wait();
    let (tx, rx) = mpsc::channel(1);
    let task = tokio::spawn(run_with_ports(
        ports.clone(),
        rx,
        CancellationToken::new(),
        Arc::new(Instant::now),
        wait,
    ));
    tx.send(transition(TransitionKind::CiStarted, &fixture.room, "A"))
        .await
        .unwrap();
    let release = next_retry(&mut waits).await;
    drop(tx);
    assert_eq!(ports.completed.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    task.await.unwrap();
    assert_eq!(ports.deliveries().len(), 2);
    assert_eq!(ports.completed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runner_takes_stamp_and_b_preference_after_previous_real_completion() {
    for b_enabled in [false, true] {
        let fixture = room_fixture();
        let ports = held_ports(&fixture);
        let (completion_release, completion_gate) = tokio::sync::oneshot::channel();
        ports
            .attempts
            .lock()
            .unwrap()
            .push_back((Ok(()), Some(completion_gate)));
        let (wait, mut waits) = controlled_retry_wait();
        let t0 = Instant::now();
        let clock = Arc::new(Mutex::new(t0));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let read_clock = clock.clone();
        let read_log = reads.clone();
        let now: NowSource = Arc::new(move || {
            let value = *read_clock.lock().unwrap();
            read_log.lock().unwrap().push(value);
            value
        });
        let (tx, rx) = mpsc::channel(2);
        let task = tokio::spawn(run_with_ports(
            ports.clone(),
            rx,
            CancellationToken::new(),
            now,
            wait,
        ));
        tx.send(transition(TransitionKind::CiStarted, &fixture.room, "A"))
            .await
            .unwrap();
        let release = next_retry(&mut waits).await;
        ports.set_dials(dials(!b_enabled, true));
        tx.send(transition(TransitionKind::CiStarted, &fixture.room, "B"))
            .await
            .unwrap();
        assert_eq!(reads.lock().unwrap().as_slice(), &[t0]);
        assert_eq!(ports.dial_reads.load(Ordering::SeqCst), 1);
        // Clear the first notification so the next one is the retry call.
        ports.delivery_started.notified().await;
        release.send(()).unwrap();
        ports.delivery_started.notified().await;
        assert_eq!(ports.dial_reads.load(Ordering::SeqCst), 1);
        assert_eq!(ports.completed.load(Ordering::SeqCst), 0);
        ports.set_dials(dials(b_enabled, true));
        *clock.lock().unwrap() = t0 + Duration::from_secs(6);
        completion_release.send(()).unwrap();
        drop(tx);
        task.await.unwrap();
        assert_eq!(
            reads.lock().unwrap().as_slice(),
            &[t0, t0 + Duration::from_secs(6)]
        );
        assert_eq!(ports.resolutions(), if b_enabled { 2 } else { 1 });
        assert_eq!(
            ports.completed.load(Ordering::SeqCst),
            if b_enabled { 2 } else { 1 }
        );
    }
}

#[test]
fn serial_recv_stamp_preserves_the_accepted_cap_shift() {
    let ports = Arc::new(RecordingPorts::new(dials(true, true), None));
    let mut base = RemoteAlertState::new(ports.clone());
    let mut candidate = RemoteAlertState::new(ports);
    let t0 = Instant::now();
    let key = ("room".into(), "B".into());
    assert!(base.admit(&key, t0 + Duration::from_secs(3)));
    assert!(candidate.admit(&key, t0 + Duration::from_secs(6)));
    for _ in 0..11 {
        assert!(base.admit(&key, t0 + Duration::from_secs(100)));
        assert!(candidate.admit(&key, t0 + Duration::from_secs(100)));
    }
    assert!(base.admit(&key, t0 + Duration::from_secs(3604)));
    assert!(!candidate.admit(&key, t0 + Duration::from_secs(3604)));
    assert!(!base.cap_warned.contains(&key));
    assert!(candidate.cap_warned.contains(&key));
    assert_eq!(base.windows[&key][0], t0 + Duration::from_secs(100));
    assert_eq!(candidate.windows[&key][0], t0 + Duration::from_secs(6));
}

#[tokio::test]
async fn deferral_consumes_exactly_one_admission_slot() {
    for terminal in [Ok(()), Err("pipe closed".to_string())] {
        let fixture = room_fixture();
        let ports = held_ports(&fixture);
        ports.attempts.lock().unwrap().extend([
            (Err("menu_guard_deferred: held".into()), None),
            (Err("typing_hold_deferred: held".into()), None),
            (terminal, None),
        ]);
        let (wait, mut waits) = controlled_retry_wait();
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let mut state =
            RemoteAlertState::with_controls(ports.clone(), CancellationToken::new(), wait);
        state.admission_snapshots = Some(snapshots.clone());
        let input = transition(TransitionKind::CiStarted, &fixture.room, "A");
        let key = (input.room_dir.clone(), input.repo_path.clone());
        let controller = tokio::spawn(async move {
            for _ in 0..3 {
                next_retry(&mut waits).await.send(()).unwrap();
            }
        });
        assert!(state.handle(input, Instant::now()).await);
        controller.await.unwrap();
        assert_eq!(*snapshots.lock().unwrap(), vec![1, 1, 1]);
        assert_eq!(state.windows[&key].len(), 1);
        assert_eq!(ports.dial_reads.load(Ordering::SeqCst), 1);
        assert_eq!(ports.resolutions(), 1);
        let originals = ports.originals.lock().unwrap();
        assert_eq!(originals.len(), 4);
        assert!(originals.iter().all(|attempt| attempt == &originals[0]));
    }
}

#[tokio::test]
async fn simultaneous_shutdown_and_ready_timer_selects_stop() {
    let fixture = room_fixture();
    let ports = held_ports(&fixture);
    let shutdown = CancellationToken::new();
    let cancel = shutdown.clone();
    let ready: RetryWait = Arc::new(move |delay| {
        assert_eq!(delay, PENDING_RETRY_INTERVAL);
        cancel.cancel();
        Box::pin(async {})
    });
    let (tx, rx) = mpsc::channel(2);
    tx.send(transition(TransitionKind::CiStarted, &fixture.room, "A"))
        .await
        .unwrap();
    tx.send(transition(TransitionKind::CiStarted, &fixture.room, "B"))
        .await
        .unwrap();
    run_with_ports(ports.clone(), rx, shutdown, Arc::new(Instant::now), ready).await;
    assert_eq!(ports.deliveries().len(), 1);
    assert_eq!(ports.dial_reads.load(Ordering::SeqCst), 1);
    assert_eq!(ports.completed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn production_constructor_uses_the_supplied_shutdown_token_during_hold() {
    let fixture = room_fixture();
    let ports = held_ports(&fixture);
    let (tx, rx) = mpsc::channel(2);
    let shutdown = CancellationToken::new();
    let task = tauri::async_runtime::spawn(run(ports.clone(), rx, shutdown.clone()));
    tx.send(transition(TransitionKind::CiStarted, &fixture.room, "A"))
        .await
        .unwrap();
    ports.delivery_started.notified().await;
    tx.send(transition(TransitionKind::CiStarted, &fixture.room, "B"))
        .await
        .unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ports.deliveries().len(), 1);
    assert_eq!(ports.dial_reads.load(Ordering::SeqCst), 1);
    assert_eq!(ports.completed.load(Ordering::SeqCst), 0);
}
