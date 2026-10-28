use super::{
    add_mailbox_session, enqueue_shared_pty_engine_operation, native_fixture, pump, sender_token,
    shared_engine_writes, support_commands, timeout, unrelated_progress, Arc, Duration,
    MailboxPoller, MailboxTestHooks, Manager, Mutex, Ordering, PathBuf, PtyManager,
    SelectionCoordinator, SessionBackendKind, SessionManager, SessionStatus, Uuid,
    CANONICAL_WAKE_FROM, CANONICAL_WAKE_TO,
};

async fn p02_ready_fixture() -> (
    support_commands::SessionRequestFixture,
    PathBuf,
    crate::config::teams::VerifiedPtyInputRoute,
    Uuid,
    Uuid,
) {
    let (fixture, sender, peer, exited) = native_fixture().await;
    let app = fixture.app.handle();
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    manager.destroy_session(exited).await.unwrap();
    let sid = add_mailbox_session(app, &peer, "P02 target", SessionStatus::Idle, None).await;
    let token = sender_token(&fixture, &sender).await;
    let sender_id = Uuid::parse_str(
        &manager
            .find_unique_live_by_token(Uuid::parse_str(&token).unwrap())
            .await
            .unwrap()
            .id,
    )
    .unwrap();
    let route = crate::config::teams::verify_pty_input_route(
        &sender,
        false,
        CANONICAL_WAKE_TO,
        &[fixture
            ._temp
            .path()
            .join("proj-a")
            .to_string_lossy()
            .to_string()],
    )
    .unwrap();
    let pty = app.state::<Arc<Mutex<PtyManager>>>();
    for (id, cwd, replica) in [
        (sender_id, &sender, &route.sender.replica_identity),
        (sid, &peer, &route.target.replica_identity),
    ] {
        pty.lock()
            .unwrap()
            .record_route_with_identities(
                id,
                SessionBackendKind::LocalProcess,
                Some(crate::path_identity::verify_directory(cwd).unwrap()),
                Some(replica.clone()),
            )
            .unwrap();
    }
    fixture.backend.set_live(sender_id);
    fixture.backend.set_live(sid);
    let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
    idle.register_session(sid, crate::session::profile::IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(sid);
    (fixture, sender, route, sender_id, sid)
}

fn p02_gate(
    poller: &MailboxPoller,
    before: bool,
) -> (
    Arc<crate::session::selection::ReplacementFixtureControl>,
    tokio::sync::oneshot::Sender<()>,
) {
    let control = Arc::new(crate::session::selection::ReplacementFixtureControl::default());
    let (resume, gate) = tokio::sync::oneshot::channel();
    let slot = if before {
        &control.preaccept_gate
    } else {
        &control.accepted_core_gate
    };
    *slot.lock().unwrap() = Some(gate);
    *poller
        .test_hooks
        .as_ref()
        .unwrap()
        .replacement_fixture
        .lock()
        .unwrap() = Some(control.clone());
    (control, resume)
}

async fn p02_until(mut ready: impl FnMut() -> bool) {
    timeout(Duration::from_secs(8), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("P02 accepted core/settlement barrier");
}

#[tokio::test]
async fn p02_host_poll_progress_marker_and_late_terminal() {
    use crate::api::message_store::MessageStoreState;
    let (fixture, sender, _, _, sid) = p02_ready_fixture().await;
    let app = fixture.app.handle();
    let token = sender_token(&fixture, &sender).await;
    let pty = app.state::<Arc<Mutex<PtyManager>>>();
    let input_writer = PtyManager::acquire_input_writer(pty.inner(), sid)
        .await
        .unwrap();
    let state = app.state::<MessageStoreState>();
    let store = state.store.as_ref().unwrap();
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let stamp = crate::phone::types::canonical_pty_timestamp(now);
    let envelope = serde_json::json!({
        "id": id, "token": token, "from": CANONICAL_WAKE_FROM, "to": CANONICAL_WAKE_TO,
        "body": "", "mode": "wake", "getOutput": false, "preferredAgent": "",
        "priority": "normal", "timestamp": stamp, "action": "pty-input",
        "ptyInput": { "version": crate::phone::types::PTY_INPUT_VERSION,
            "text": "P02 exact host input", "enter": "agent-submit", "injectionId": id,
            "opId": id, "issuedAt": stamp, "nonce": Uuid::new_v4().to_string(),
            "expiresAt": crate::phone::types::canonical_pty_timestamp(
                now + chrono::Duration::minutes(10)), "agentId": null }
    });
    let outbox = sender
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let path = outbox.join(format!("{id}.json"));
    std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    let mut poller = MailboxPoller::new_with_test_hooks(MailboxTestHooks::default());
    let (control, resume) = p02_gate(&poller, false);
    {
        let observation = Box::pin(poller.poll(app));
        tokio::pin!(observation);
        tokio::select! {
            result = &mut observation => panic!("P02 poll returned before core: {result:?}"),
            _ = p02_until(|| control.accepted_core_entered.load(Ordering::Acquire)) => {}
        }
        resume.send(()).unwrap();
        timeout(Duration::from_secs(7), observation)
            .await
            .expect("P02 host observation bounded")
            .unwrap();
    }
    assert_eq!(
        poller.host_pty_slots.available_permits(),
        7,
        "P02 host permit retained"
    );
    assert!(
        !store
            .query_pty_input_by_injection(&id)
            .unwrap()
            .unwrap()
            .terminal
    );
    assert_eq!(
        store
            .query_pty_input_by_injection(&id)
            .unwrap()
            .unwrap()
            .status,
        crate::phone::types::PtyInputPublicStatus::Queued
    );
    let marker: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(marker["kind"], "pty-input-marker");
    unrelated_progress(&mut poller, &fixture, &sender).await;
    drop(input_writer);
    p02_until(|| {
        store
            .query_pty_input_by_injection(&id)
            .unwrap()
            .unwrap()
            .terminal
    })
    .await;
    p02_until(|| poller.host_pty_slots.available_permits() == 8).await;
    pump(&mut poller, &fixture).await;
    assert!(!path.exists());
    let artifact = outbox.join("delivered").join(format!("{id}.json"));
    let bytes = std::fs::read(&artifact).unwrap();
    let result: crate::phone::types::PtyInputHostArtifact = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        result.result.status,
        crate::phone::types::PtyInputPublicStatus::Injected
    );
    let expected_writes = vec![
        b"P02 exact host input".to_vec(),
        b"\r".to_vec(),
        b"\r".to_vec(),
    ];
    assert_eq!(fixture.backend.writes_for(sid), expected_writes);
    pump(&mut poller, &fixture).await;
    assert_eq!(
        bytes,
        std::fs::read(artifact).unwrap(),
        "P02 terminal repair once"
    );
    assert_eq!(fixture.backend.writes_for(sid), expected_writes);
    app.state::<SelectionCoordinator>().close_and_join().await;
}

#[tokio::test]
async fn p02_budget_eight_cores_ninth_queued_and_per_id_exclusion() {
    use crate::api::message_store::MessageStoreState;
    use crate::phone::types::PtyInputSourcePlane::HostCli;
    let (fixture, sender_path, route, sender, target) = p02_ready_fixture().await;
    let app = fixture.app.handle();
    let state = app.state::<MessageStoreState>();
    let store = state.store.as_ref().unwrap();
    let ids: Vec<_> = (0..9)
        .map(|_| enqueue_shared_pty_engine_operation(store, &route, sender, b"budget source"))
        .collect();
    let mut poller = MailboxPoller::new_with_test_hooks(MailboxTestHooks::default());
    let mut waiters = Vec::new();
    let mut releases = Vec::new();
    for id in &ids[..8] {
        let (control, resume) = p02_gate(&poller, false);
        let mut waiter =
            Box::pin(poller.dispatch_pty_input_operation(app, &state, id, HostCli, None));
        assert!(futures::poll!(&mut waiter).is_pending());
        p02_until(|| control.accepted_core_entered.load(Ordering::Acquire)).await;
        waiters.push(waiter);
        releases.push(resume);
    }
    let (ninth, resume_ninth) = p02_gate(&poller, false);
    poller
        .dispatch_pty_input_operation(app, &state, &ids[8], HostCli, None)
        .await;
    assert!(
        !ninth.preaccept_entered.load(Ordering::Acquire),
        "ninth submits no core"
    );
    assert!(
        !store
            .query_pty_input_by_injection(&ids[8])
            .unwrap()
            .unwrap()
            .terminal
    );
    timeout(Duration::from_secs(7), futures::future::join_all(waiters))
        .await
        .expect("eight bounded observers");
    assert_eq!(
        poller.host_pty_slots.available_permits(),
        0,
        "P02 host capacity retained"
    );
    assert_eq!(
        store
            .query_pty_input_by_injection(&ids[8])
            .unwrap()
            .unwrap()
            .status,
        crate::phone::types::PtyInputPublicStatus::Queued
    );
    unrelated_progress(&mut poller, &fixture, &sender_path).await;
    assert_eq!(
        poller.host_pty_slots.available_permits(),
        0,
        "unrelated real delivery must not release host cores"
    );
    releases.remove(0).send(()).unwrap();
    p02_until(|| poller.host_pty_slots.available_permits() == 1).await;
    let mut next =
        Box::pin(poller.dispatch_pty_input_operation(app, &state, &ids[8], HostCli, None));
    assert!(futures::poll!(&mut next).is_pending());
    p02_until(|| ninth.accepted_core_entered.load(Ordering::Acquire)).await;
    assert_eq!(
        poller.host_pty_slots.available_permits(),
        0,
        "ninth owns only released slot"
    );
    drop(releases);
    drop(resume_ninth);
    timeout(Duration::from_secs(7), next).await.unwrap();
    p02_until(|| poller.host_pty_slots.available_permits() == 8).await;
    *poller
        .test_hooks
        .as_ref()
        .unwrap()
        .replacement_fixture
        .lock()
        .unwrap() = None;
    let duplicate = enqueue_shared_pty_engine_operation(store, &route, sender, b"locked source");
    let held = store.try_operation_lock(&duplicate).unwrap().unwrap();
    let writes = shared_engine_writes(app, target);
    poller
        .dispatch_pty_input_operation(app, &state, &duplicate, HostCli, None)
        .await;
    assert_eq!(
        shared_engine_writes(app, target),
        writes,
        "same-ID lock excludes duplicate effects"
    );
    assert_eq!(poller.host_pty_slots.available_permits(), 8);
    assert!(
        !store
            .query_pty_input_by_injection(&duplicate)
            .unwrap()
            .unwrap()
            .terminal
    );
    drop(held);
    app.state::<SelectionCoordinator>().close_and_join().await;
}

#[tokio::test]
async fn p02_host_preaccept_timeout_releases_unsubmitted_capacity() {
    let (fixture, _, _, _) = native_fixture().await;
    let app = fixture.app.handle();
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let poller = MailboxPoller::new_with_test_hooks(MailboxTestHooks::default());
    let (control, resume) = p02_gate(&poller, true);
    let id = Uuid::new_v4().to_string();
    let mut observer = Box::pin(poller.dispatch_pty_input_operation(
        app,
        &state,
        &id,
        crate::phone::types::PtyInputSourcePlane::HostCli,
        None,
    ));
    assert!(futures::poll!(&mut observer).is_pending());
    p02_until(|| control.preaccept_entered.load(Ordering::Acquire)).await;
    assert_eq!(poller.host_pty_slots.available_permits(), 7);
    timeout(Duration::from_secs(7), observer)
        .await
        .expect("preaccept is inside host deadline");
    assert_eq!(poller.host_pty_slots.available_permits(), 8);
    assert!(!control.accepted_core_entered.load(Ordering::Acquire));
    assert!(resume.send(()).is_err(), "unsubmitted hook was cancelled");
    app.state::<SelectionCoordinator>().close_and_join().await;
}

#[tokio::test]
async fn p02_container_preserves_full_wait_without_host_permit() {
    for before in [true, false] {
        let (fixture, _, _, _) = native_fixture().await;
        let app = fixture.app.handle();
        let state = app.state::<crate::api::message_store::MessageStoreState>();
        let poller = MailboxPoller::new_with_test_hooks(MailboxTestHooks::default());
        let (control, resume) = p02_gate(&poller, before);
        let id = Uuid::new_v4().to_string();
        let mut observer = Box::pin(poller.dispatch_pty_input_operation(
            app,
            &state,
            &id,
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            None,
        ));
        assert!(futures::poll!(&mut observer).is_pending());
        p02_until(|| {
            if before {
                control.preaccept_entered.load(Ordering::Acquire)
            } else {
                control.accepted_core_entered.load(Ordering::Acquire)
            }
        })
        .await;
        assert!(
            timeout(Duration::from_secs(6), &mut observer)
                .await
                .is_err(),
            "P02 container still awaiting"
        );
        assert_eq!(poller.host_pty_slots.available_permits(), 8);
        resume.send(()).unwrap();
        timeout(Duration::from_secs(3), observer).await.unwrap();
        app.state::<SelectionCoordinator>().close_and_join().await;
    }
}

#[tokio::test]
async fn p02_host_finite_success_is_terminal_before_five_seconds() {
    use crate::api::message_store::MessageStoreState;
    use crate::phone::types::{PtyInputPublicStatus, PtyInputSourcePlane};
    let (fixture, _, route, sender, target) = p02_ready_fixture().await;
    let app = fixture.app.handle();
    let state = app.state::<MessageStoreState>();
    let store = state.store.as_ref().unwrap();
    let id = enqueue_shared_pty_engine_operation(store, &route, sender, b"finite host source");
    let poller = MailboxPoller::new_with_test_hooks(MailboxTestHooks::default());
    let started = tokio::time::Instant::now();
    timeout(
        Duration::from_secs(5),
        poller.dispatch_pty_input_operation(app, &state, &id, PtyInputSourcePlane::HostCli, None),
    )
    .await
    .expect("finite host core returns before its observation deadline");
    assert!(started.elapsed() < Duration::from_secs(5));
    let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
    assert!(
        result.terminal,
        "observer return alone is not finite success"
    );
    assert_eq!(result.status, PtyInputPublicStatus::Injected);
    assert_eq!(poller.host_pty_slots.available_permits(), 8);
    assert_eq!(
        shared_engine_writes(app, target),
        vec![
            b"finite host source".to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec()
        ]
    );
    app.state::<SelectionCoordinator>().close_and_join().await;
}
