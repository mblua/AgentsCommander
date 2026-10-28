mod host_pty;

use super::*;
use crate::phone::mailbox::teardown::{
    observation_budget, observe_close_attempt, observe_purge, run_purge_batch,
    wait_background_destroy, PurgePhase, PurgeProgress, PurgeTargets, TeardownOutcome,
    OBSERVATION_ALLOWANCE,
};
use crate::session::purge_guard::PurgeGuard;
use crate::session::selection::SelectionCoordinator;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;
use tokio::time::{sleep, timeout};

fn targets(ids: Vec<Uuid>) -> PurgeTargets {
    PurgeTargets {
        session_ids: ids.iter().copied().collect(),
        ids,
        fqns: HashSet::from(["proj:room/peer".into()]),
    }
}

async fn reached(notify: &Notify) {
    timeout(Duration::from_secs(10), notify.notified())
        .await
        .expect("barrier reached");
}

async fn stopped(progress: &PurgeProgress) {
    timeout(Duration::from_secs(3), async {
        while progress.snapshot().unwrap().phase != PurgePhase::Stopped {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owner settled");
}

fn assert_retained(guard: &PurgeGuard, sid: Uuid) {
    assert!(guard.is_active(), "lease retained");
    assert!(guard.blocks_session(sid), "session exclusion retained");
    assert!(
        guard.blocks_agent("proj:room/peer"),
        "FQN exclusion retained"
    );
    assert!(!guard.blocks_agent("proj:room/unrelated"));
}

#[tokio::test]
async fn p01_pending_query_error_retains_lease() {
    let guard = Arc::new(PurgeGuard::default());
    let sid = Uuid::new_v4();
    let next = Uuid::new_v4();
    let progress = Arc::new(PurgeProgress::new(vec![sid, next]));
    let error_reached = Arc::new(Notify::new());
    let rows = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let (notify, count, attempts) = (error_reached.clone(), rows.clone(), calls.clone());
    let worker = tokio::spawn(run_purge_batch(
        guard.clone(),
        targets(vec![sid, next]),
        progress.clone(),
        Duration::from_millis(60),
        move |_| {
            attempts.fetch_add(1, Ordering::SeqCst);
            let (notify, count) = (notify.clone(), count.clone());
            async move {
                let absent = wait_background_destroy(
                    move || {
                        notify.notify_one();
                        Err("query unavailable".into())
                    },
                    move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        async { false }
                    },
                )
                .await;
                if absent {
                    TeardownOutcome::AlreadyGone
                } else {
                    TeardownOutcome::Failed("duplicate_unconfirmed".into())
                }
            }
        },
    ));
    let observer = observe_purge(progress.clone(), worker);
    let observation = tokio::spawn(observer);
    reached(&error_reached).await;
    let snapshot = timeout(Duration::from_secs(2), observation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_retained(&guard, sid);
    assert!(snapshot.reason.unwrap().contains("observer_timeout"));
    assert_eq!(snapshot.phase, PurgePhase::Started(sid));
    assert_eq!(rows.load(Ordering::SeqCst), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(timeout(
        Duration::from_millis(80),
        guard.acquire(HashSet::new(), HashSet::new())
    )
    .await
    .is_err());
    assert_retained(&guard, sid);
    assert_ne!(progress.snapshot().unwrap().phase, PurgePhase::Stopped);
    // The isolated Tokio runtime tears down this deliberately unresolved owner.
}

#[tokio::test]
async fn p01_purge_timeout_retains_until_settlement_and_never_starts_next() {
    let guard = Arc::new(PurgeGuard::default());
    let sid = Uuid::new_v4();
    let ids = vec![sid, Uuid::new_v4()];
    let progress = Arc::new(PurgeProgress::new(ids.clone()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (entry, gate, count) = (entered.clone(), release.clone(), calls.clone());
    let worker = tokio::spawn(run_purge_batch(
        guard.clone(),
        targets(ids),
        progress.clone(),
        Duration::from_millis(70),
        move |_| {
            let (entry, gate) = (entry.clone(), gate.clone());
            count.fetch_add(1, Ordering::SeqCst);
            async move {
                entry.notify_one();
                gate.notified().await;
                TeardownOutcome::Closed
            }
        },
    ));
    let observer = tokio::spawn(observe_purge(progress.clone(), worker));
    reached(&entered).await;
    let snapshot = observer.await.unwrap().unwrap();
    assert!(snapshot.stop);
    assert!(snapshot.results.is_empty());
    assert_retained(&guard, sid);
    // A second production observer fails bounded acquisition and removes its waiter.
    let second = Arc::new(PurgeProgress::new(vec![Uuid::new_v4()]));
    let second_calls = calls.clone();
    let second_worker = tokio::spawn(run_purge_batch(
        guard.clone(),
        targets(second.snapshot().unwrap().ids),
        second.clone(),
        OBSERVATION_ALLOWANCE,
        move |_| {
            second_calls.fetch_add(100, Ordering::SeqCst);
            async { TeardownOutcome::Closed }
        },
    ));
    let second_result = timeout(Duration::from_secs(7), observe_purge(second, second_worker))
        .await
        .unwrap()
        .unwrap();
    assert!(!second_result.started);
    assert!(second_result.stop);
    assert_retained(&guard, sid);
    release.notify_one();
    stopped(&progress).await;
    assert!(!guard.is_active());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let lease = timeout(
        Duration::from_millis(100),
        guard.acquire(HashSet::new(), HashSet::new()),
    )
    .await
    .expect("cancelled waiter removed");
    drop(lease);
}

#[tokio::test]
async fn p01_purge_parent_drop_before_grant_and_after_started() {
    for after_start in [false, true] {
        let guard = Arc::new(PurgeGuard::default());
        let sid = Uuid::new_v4();
        let progress = Arc::new(PurgeProgress::new(vec![sid, Uuid::new_v4()]));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let (entry, gate, count) = (entered.clone(), release.clone(), calls.clone());
        let worker = tokio::spawn(run_purge_batch(
            guard.clone(),
            targets(progress.snapshot().unwrap().ids),
            progress.clone(),
            OBSERVATION_ALLOWANCE,
            move |_| {
                let (entry, gate) = (entry.clone(), gate.clone());
                count.fetch_add(1, Ordering::SeqCst);
                async move {
                    entry.notify_one();
                    gate.notified().await;
                    TeardownOutcome::Closed
                }
            },
        ));
        let observer = observe_purge(progress.clone(), worker);
        if after_start {
            let observation = tokio::spawn(observer);
            reached(&entered).await;
            observation.abort();
            assert!(observation.await.unwrap_err().is_cancelled());
            assert_retained(&guard, sid);
            release.notify_one();
            stopped(&progress).await;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        } else {
            // Even an unpolled observer cancels an acquired-but-ungranted owner.
            timeout(Duration::from_secs(1), async {
                while progress.snapshot().unwrap().phase != PurgePhase::AcquiredAwaitGrant {
                    sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            drop(observer);
            sleep(Duration::from_millis(40)).await;
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
        assert!(!guard.is_active());
        let lease = timeout(
            Duration::from_millis(200),
            guard.acquire(HashSet::new(), HashSet::new()),
        )
        .await
        .unwrap();
        drop(lease);
    }
}

async fn native_fixture() -> (
    support_commands::SessionRequestFixture,
    PathBuf,
    PathBuf,
    Uuid,
) {
    let fixture = make_session_request_fixture();
    let project = fixture._temp.path().join("proj-a");
    let ac = project.join(".ac");
    let room = ac.join("wg-1-dev-team");
    let sender = room.join("__agent_tech-lead");
    let peer = room.join("__agent_dev-rust");
    for name in ["tech-lead", "dev-rust"] {
        let origin = ac.join(format!("_agent_{name}"));
        let replica = room.join(format!("__agent_{name}"));
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            serde_json::json!({
                "identity": format!("../../_agent_{name}")
            })
            .to_string(),
        )
        .unwrap();
    }
    let team = ac.join("_team_dev-team");
    std::fs::create_dir_all(&team).unwrap();
    std::fs::write(
        team.join("config.json"),
        serde_json::json!({
            "agents": ["../_agent_dev-rust"], "coordinator": "../_agent_tech-lead"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(room.join("TASK.md"), "---\ntitle: fixture\n---\n").unwrap();
    fixture
        .app
        .state::<SettingsState>()
        .write()
        .await
        .project_paths = vec![path_string(&project)];
    let app = fixture.app.handle();
    let (sender_id, _) = seed_raise_hand_session(app, &sender, true, SessionStatus::Idle).await;
    let sid = add_mailbox_session(
        app,
        &peer,
        "wg-1-dev-team/dev-rust",
        SessionStatus::Exited(0),
        None,
    )
    .await;
    let pty = app.state::<Arc<Mutex<PtyManager>>>();
    pty.lock()
        .unwrap()
        .record_route(sid, SessionBackendKind::LocalProcess);
    fixture.backend.set_live(sid);
    assert!(app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .get_session(sender_id)
        .await
        .is_some());
    (fixture, sender, peer, sid)
}

fn request(sender: &Path, id: &str, action: &str, token: String) -> PathBuf {
    let mut message = wake_message_to_target();
    message.id = id.into();
    message.request_id = Some(id.into());
    message.from = "proj-a:wg-1-dev-team/tech-lead".into();
    message.to = String::new();
    message.token = Some(token);
    message.action = Some(action.into());
    message.target = if action == "close-session" {
        Some("proj-a:wg-1-dev-team/dev-rust".into())
    } else {
        None
    };
    message.force = Some(true);
    message.timeout_secs = Some(0);
    message.dry_run = Some(false);
    message.timestamp = chrono::Utc::now().to_rfc3339();
    let outbox = sender
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let path = outbox.join(format!("{id}.json"));
    std::fs::write(&path, serde_json::to_vec(&message).unwrap()).unwrap();
    path
}

async fn sender_token(fixture: &support_commands::SessionRequestFixture, sender: &Path) -> String {
    let manager = fixture
        .app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let manager = manager.read().await.clone();
    manager
        .list_sessions()
        .await
        .into_iter()
        .find(|session| session.working_directory == path_string(sender))
        .unwrap()
        .token
}

async fn pump(poller: &mut MailboxPoller, fixture: &support_commands::SessionRequestFixture) {
    timeout(
        Duration::from_secs(8),
        Box::pin(poller.poll(fixture.app.handle())),
    )
    .await
    .expect("mailbox observation bounded")
    .unwrap();
}

async fn unrelated_progress(
    poller: &mut MailboxPoller,
    fixture: &support_commands::SessionRequestFixture,
    sender: &Path,
) {
    let token = sender_token(fixture, sender).await;
    let raise_id = Uuid::new_v4().to_string();
    let (raise, _) = build_raise_hand_message(sender, "p01-raise", &raise_id, Some(token));
    pump(poller, fixture).await;
    assert!(!raise.exists(), "raise-hand delivered while teardown held");
    assert_eq!(
        read_response_json(sender, &raise_id).unwrap()["status"],
        "raised"
    );
    let other = fixture._temp.path().join("proj-a/other");
    std::fs::create_dir_all(&other).unwrap();
    let other_id = add_mailbox_session(
        fixture.app.handle(),
        &other,
        "other",
        SessionStatus::Idle,
        None,
    )
    .await;
    register_mock_pty_route(fixture.app.handle(), other_id);
    fixture.backend.set_live(other_id);
    poller
        .test_hooks
        .as_ref()
        .expect("native mailbox fixture requires shared test hooks")
        .real_inject_sessions
        .lock()
        .unwrap()
        .insert(other_id);
    let path = write_wake_message(sender, "p01-wake", &chrono::Utc::now().to_rfc3339());
    let mut message: OutboxMessage =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    message.from = "proj-a:wg-1-dev-team/tech-lead".into();
    message.to = "proj-a/other".into();
    message.request_id = Some(Uuid::new_v4().to_string());
    std::fs::write(&path, serde_json::to_vec(&message).unwrap()).unwrap();
    timeout(Duration::from_secs(8), async {
        loop {
            pump(poller, fixture).await;
            if sender
                .join(crate::config::agent_local_dir_name())
                .join("outbox/delivered/p01-wake.json")
                .exists()
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("unrelated wake delivered");
    assert!(
        !fixture.backend.writes_for(other_id).is_empty(),
        "wake actually injected"
    );
}

async fn native_timeout_case(purge: bool, graceful: bool) {
    let (fixture, sender, _peer, sid) = native_fixture().await;
    let app = fixture.app.handle();
    let coordinator = app.state::<SelectionCoordinator>().inner().clone();
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    // Empty-to close actions use the fixture master credential; purge requires a session token.
    let token = if purge {
        sender_token(&fixture, &sender).await
    } else {
        MAILBOX_MASTER_TOKEN.to_string()
    };
    let action = if purge {
        PURGE_WG_ACTION
    } else {
        "close-session"
    };
    let blocked_id = Uuid::new_v4().to_string();
    let path = request(&sender, &blocked_id, action, token);
    if graceful {
        let mut message: OutboxMessage =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        message.force = Some(false);
        std::fs::write(&path, serde_json::to_vec(&message).unwrap()).unwrap();
    }
    let barrier = crate::commands::session::restart_reader_seam::install_after_old_release(
        &format!("destroy:{sid}"),
    );
    let hooks = MailboxTestHooks::default();
    let mut poller = MailboxPoller::new_with_test_hooks(hooks);
    {
        let observation = poller.poll(app);
        tokio::pin!(observation);
        tokio::select! {
            result = &mut observation => panic!("poll returned before native barrier: {result:?}"),
            _ = reached(&barrier.reached) => {}
        }
        timeout(Duration::from_secs(7), &mut observation)
            .await
            .expect("observer deadline releases poll")
            .unwrap();
    }
    let response_path = sender
        .join(crate::config::agent_local_dir_name())
        .join(format!("responses/{blocked_id}.json"));
    let before = std::fs::read(&response_path).unwrap();
    let response: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(
        response["status"],
        if purge { "partial_failure" } else { "timeout" }
    );
    assert_eq!(response["outcome_unknown"], true);
    assert_eq!(response["may_complete_later"], true);
    assert!(coordinator.background_destroy_is_pending(sid).unwrap());
    let guard = app.state::<Arc<PurgeGuard>>().inner().clone();
    if purge {
        assert!(guard.is_active());
        assert!(guard.blocks_session(sid));
        assert!(guard.blocks_agent("proj-a:wg-1-dev-team/dev-rust"));
        assert!(!guard.blocks_agent("proj-a/other"));
    }
    unrelated_progress(&mut poller, &fixture, &sender).await;
    barrier.release.notify_one();
    timeout(Duration::from_secs(5), async {
        while coordinator.background_destroy_is_pending(sid).unwrap() || guard.is_active() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real late settlement releases ownership");
    assert!(!manager.contains_public_or_pending(sid).await);
    assert_eq!(
        std::fs::read(response_path).unwrap(),
        before,
        "late work cannot rewrite response"
    );
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn p01_close_force_native_timeout_keeps_mailbox_progress() {
    native_timeout_case(false, false).await;
}

#[tokio::test]
async fn p01_close_graceful_native_timeout_keeps_mailbox_progress() {
    native_timeout_case(false, true).await;
}

#[tokio::test]
async fn p01_purge_native_timeout_keeps_mailbox_progress() {
    native_timeout_case(true, false).await;
}

#[tokio::test]
async fn p01_purge_success_releases_before_response() {
    let (fixture, sender, _peer, sid) = native_fixture().await;
    let token = sender_token(&fixture, &sender).await;
    let success_id = Uuid::new_v4().to_string();
    request(&sender, &success_id, PURGE_WG_ACTION, token);
    let mut poller = MailboxPoller::default();
    pump(&mut poller, &fixture).await;
    let response = read_response_json(&sender, &success_id).expect("purge response exists");
    assert_eq!(response["status"], "purged", "{response}");
    assert_eq!(response["purged"], 1);
    let guard = fixture.app.state::<Arc<PurgeGuard>>();
    assert!(!guard.is_active(), "response must follow lease release");
    assert!(!guard.blocks_session(sid));
    assert!(!guard.blocks_agent("proj-a:wg-1-dev-team/dev-rust"));
    // Observe the production terminal publication order, including empty batches.
    for ids in [vec![], vec![Uuid::new_v4()]] {
        let guard = Arc::new(PurgeGuard::default());
        let progress = Arc::new(PurgeProgress::new(ids.clone()));
        let worker = tokio::spawn(run_purge_batch(
            guard.clone(),
            targets(ids),
            progress.clone(),
            OBSERVATION_ALLOWANCE,
            |_| async { TeardownOutcome::Closed },
        ));
        let snapshot = observe_purge(progress, worker).await.unwrap();
        assert_eq!(snapshot.phase, PurgePhase::Stopped);
        assert!(!guard.is_active(), "Stopped must follow lease release");
    }
    fixture
        .app
        .state::<SelectionCoordinator>()
        .close_and_join()
        .await;
}

#[tokio::test]
async fn p01_pending_duplicate_purge_waits_for_key_even_without_public_row() {
    let (fixture, sender, _peer, sid) = native_fixture().await;
    let app = fixture.app.handle();
    let coordinator = app.state::<SelectionCoordinator>().inner().clone();
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let barrier = crate::commands::session::restart_reader_seam::install_after_old_release(
        &format!("destroy:{sid}"),
    );
    let token = sender_token(&fixture, &sender).await;
    let first_close_id = Uuid::new_v4().to_string();
    request(
        &sender,
        &first_close_id,
        "close-session",
        MAILBOX_MASTER_TOKEN.into(),
    );
    let mut poller = MailboxPoller::default();
    {
        let observation = poller.poll(app);
        tokio::pin!(observation);
        tokio::select! {
            result = &mut observation => panic!("native barrier not reached: {result:?}"),
            _ = reached(&barrier.reached) => {}
        }
        timeout(Duration::from_secs(7), observation)
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(
        read_response_json(&sender, &first_close_id).unwrap()["status"],
        "timeout"
    );
    let duplicate_id = Uuid::new_v4().to_string();
    request(&sender, &duplicate_id, PURGE_WG_ACTION, token);
    let guard = app.state::<Arc<PurgeGuard>>().inner().clone();
    {
        let observation = poller.poll(app);
        tokio::pin!(observation);
        tokio::select! {
            result = &mut observation => panic!("duplicate settled with key present: {result:?}"),
            _ = async {
                timeout(Duration::from_secs(2), async {
                    while !guard.is_active() { sleep(Duration::from_millis(5)).await; }
                }).await.unwrap();
                // Inject row disappearance; the real native admission/key stays untouched.
                manager.destroy_session(sid).await.unwrap();
                assert!(!manager.contains_public_or_pending(sid).await);
            } => {}
        }
        timeout(Duration::from_secs(7), observation)
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        guard.is_active(),
        "row absence alone must not release protection"
    );
    assert_eq!(
        read_response_json(&sender, &duplicate_id).unwrap()["status"],
        "partial_failure"
    );
    assert!(coordinator.background_destroy_is_pending(sid).unwrap());

    assert!(guard.blocks_session(sid));
    assert!(guard.blocks_agent("proj-a:wg-1-dev-team/dev-rust"));
    barrier.release.notify_one();
    timeout(Duration::from_secs(5), async {
        while guard.is_active() || coordinator.background_destroy_is_pending(sid).unwrap() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn p01_close_blocked_input_writer_and_graceful_fallback() {
    let (fixture, _sender, _peer, sid) = native_fixture().await;
    let app = fixture.app.handle();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let permit = PtyManager::acquire_input_writer(&pty, sid).await.unwrap();
    let accepted = Arc::new(AtomicBool::new(false));
    let outcome = observe_close_attempt(
        observation_budget(false, 0).unwrap(),
        accepted.clone(),
        MailboxPoller::default().close_session_attempt(app, sid, false, 0, accepted),
    )
    .await;
    assert_eq!(
        outcome,
        TeardownOutcome::Uncertain {
            reason: "observer_timeout".into(),
            may_complete_later: false,
        }
    );
    assert!(fixture.backend.writes_for(sid).is_empty());
    drop(permit);
    fixture.backend.fail_write.store(true, Ordering::SeqCst);
    let outcome = MailboxPoller::default()
        .close_session_attempt(app, sid, false, 0, Arc::new(AtomicBool::new(false)))
        .await;
    assert_eq!(
        outcome,
        TeardownOutcome::Closed,
        "write failure falls back to real force"
    );
    app.state::<SelectionCoordinator>().close_and_join().await;
}

#[tokio::test]
async fn p01_close_immediate_destroy_error_duplicate_missing_and_root() {
    let (fixture, sender, peer, sid) = native_fixture().await;
    let app = fixture.app.handle();
    let token = MAILBOX_MASTER_TOKEN.to_string();
    fixture.backend.fail_kill_once.store(true, Ordering::SeqCst);
    let error_id = Uuid::new_v4().to_string();
    request(&sender, &error_id, "close-session", token.clone());
    let mut poller = MailboxPoller::default();
    pump(&mut poller, &fixture).await;
    let error = read_response_json(&sender, &error_id).unwrap();
    assert_eq!(error["status"], "timeout");
    assert!(error["reason"].as_str().unwrap().contains("destroy_failed"));
    assert_eq!(error["sessions_closed"], 0);
    assert_eq!(error["may_complete_later"], false);
    // A retry after known settlement succeeds. Multiple IDs for one FQN are all confirmed.
    let second = add_mailbox_session(
        app,
        &peer,
        "wg-1-dev-team/dev-rust",
        SessionStatus::Exited(0),
        None,
    )
    .await;
    register_mock_pty_route(app, second);
    let two_id = Uuid::new_v4().to_string();
    request(&sender, &two_id, "close-session", token.clone());
    pump(&mut poller, &fixture).await;
    let response = read_response_json(&sender, &two_id).unwrap();
    assert_eq!(response["status"], "closed");
    assert_eq!(response["sessions_closed"], 2);
    assert!(response["session_ids"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!(sid)));
    let empty_id = Uuid::new_v4().to_string();
    request(&sender, &empty_id, "close-session", token);
    pump(&mut poller, &fixture).await;
    assert_eq!(
        read_response_json(&sender, &empty_id).unwrap()["status"],
        "no_match"
    );
    let absent = poller
        .close_session_attempt(
            app,
            Uuid::new_v4(),
            true,
            0,
            Arc::new(AtomicBool::new(false)),
        )
        .await;
    assert_eq!(absent, TeardownOutcome::AlreadyGone);
    let root = add_mailbox_session(
        app,
        &peer,
        "wg-1-dev-team/dev-rust",
        SessionStatus::Idle,
        None,
    )
    .await;
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    manager.set_is_root_agent(root, true).await;
    register_mock_pty_route(app, root);
    let outcome = poller
        .close_session_attempt(app, root, true, 0, Arc::new(AtomicBool::new(false)))
        .await;
    assert_eq!(
        outcome,
        TeardownOutcome::Closed,
        "retained exited root counts as success"
    );
    assert!(matches!(
        manager.get_session(root).await.unwrap().status,
        SessionStatus::Exited(_)
    ));
    app.state::<SelectionCoordinator>().close_and_join().await;
}

#[tokio::test]
async fn p01_close_auth_rejected_without_destroy() {
    let (fixture, sender, _peer, sid) = native_fixture().await;
    let auth_id = Uuid::new_v4().to_string();
    let path = request(
        &sender,
        &auth_id,
        "close-session",
        Uuid::new_v4().to_string(),
    );
    let mut poller = MailboxPoller::default();
    pump(&mut poller, &fixture).await;
    assert!(!path.exists());
    let rejected = path
        .parent()
        .unwrap()
        .join(format!("rejected/{auth_id}.reason.txt"));
    assert!(rejected.exists(), "invalid token rejected");
    assert_eq!(fixture.backend.kill_attempts.load(Ordering::SeqCst), 0);
    assert!(fixture.backend.has_session(sid));
    fixture
        .app
        .state::<SelectionCoordinator>()
        .close_and_join()
        .await;
}

#[tokio::test]
async fn p01_pending_key_gone_row_present_is_unconfirmed_and_key_poll_precedes_row() {
    let pending = Arc::new(AtomicBool::new(true));
    let rows = Arc::new(AtomicUsize::new(0));
    let queried = Arc::new(Notify::new());
    let (key, count, notify) = (pending.clone(), rows.clone(), queried.clone());
    let witness = tokio::spawn(wait_background_destroy(
        move || {
            notify.notify_one();
            Ok(key.load(Ordering::SeqCst))
        },
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { true }
        },
    ));
    reached(&queried).await;
    assert_eq!(rows.load(Ordering::SeqCst), 0);
    pending.store(false, Ordering::SeqCst);
    assert!(!timeout(Duration::from_secs(1), witness)
        .await
        .unwrap()
        .unwrap());
    assert_eq!(rows.load(Ordering::SeqCst), 1);
    assert!(wait_background_destroy(|| Ok(false), || async { false }).await);
}

#[tokio::test]
async fn p01_purge_delayed_notification_uses_started_deadline_and_stops_next_id() {
    let guard = Arc::new(PurgeGuard::default());
    let ids = vec![Uuid::new_v4(), Uuid::new_v4()];
    let progress = Arc::new(PurgeProgress::new(ids.clone()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (entry, gate, count) = (entered.clone(), release.clone(), calls.clone());
    let worker = tokio::spawn(run_purge_batch(
        guard.clone(),
        targets(ids),
        progress.clone(),
        Duration::from_millis(40),
        move |_| {
            let (entry, gate) = (entry.clone(), gate.clone());
            count.fetch_add(1, Ordering::SeqCst);
            async move {
                entry.notify_one();
                gate.notified().await;
                TeardownOutcome::Closed
            }
        },
    ));
    let observer = observe_purge(progress.clone(), worker);
    tokio::pin!(observer);
    tokio::select! {
        result = &mut observer => panic!("attempt not entered: {result:?}"),
        _ = reached(&entered) => {}
    }
    // Do not poll the observer: callback completion after the stored deadline cannot start ID2.
    sleep(Duration::from_millis(80)).await;
    release.notify_one();
    stopped(&progress).await;
    let result = observer.await.unwrap();
    assert!(result.stop);
    assert!(result.reason.unwrap().contains("observer_timeout"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!guard.is_active());
}
