use super::*;
use crate::session::session::SessionStatus;

/// Test 9 (old-id raise, timing variant 2): a raise paused across the
/// restart and resumed **after** the old row is gone installs for a dead
/// id; its own post-install liveness recheck self-cleans it.
#[tokio::test]
async fn an_old_id_raise_resumed_after_old_row_removal_self_cleans() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let old = h
        .live_session(fixture.coordinator_path(), CodingAgentKind::Claude)
        .await;
    let old_key = old.to_string();

    let barrier = crate::commands::telegram::reader_demand_seam::install_before_install(&old_key);
    let app = h.app.handle().clone();
    let raise = tokio::spawn(async move {
        crate::commands::telegram::raise_reader_demand(&app, old, ReaderConsumer::Room, None).await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        barrier.reached.notified(),
    )
    .await
    .expect("the old-id raise reaches the pre-install seam");

    // The restart runs to completion while the raise is still paused.
    let restarted = h.restart(old).await.expect("restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    assert_ne!(new, old);
    assert!(h.snapshot(old).await.0.is_none());
    assert!(!h.captures.is_open(&old_key));

    // Resumed after old-row removal: the raise installs, then the liveness
    // recheck releases the reader it just created.
    barrier.release.notify_one();
    let raised = tokio::time::timeout(std::time::Duration::from_secs(10), raise)
        .await
        .expect("the raise resumes")
        .expect("the raise task joins");
    assert!(!raised, "a raise resumed after removal must self-clean");
    let (reader, demands) = h.snapshot(old).await;
    assert_eq!(reader, None);
    assert!(demands.is_empty());
    assert!(!h.captures.is_open(&old_key));
    assert!(h.captures.slot(&old_key).is_none());

    // Exactly one reader exists, and it is the replacement's.
    assert!(h.snapshot(new).await.0.is_some());
    assert!(h.captures.is_open(&new.to_string()));
    h.release_all(new).await;
    h.close().await;
}

/// Test 9 (old-id raise, timing variant 1): a raise **installed after the
/// early release but before the commit** is swept by the post-commit
/// release. This is the deterministic proof that the success path cleans
/// the old id even when a demand lands in the window.
#[tokio::test]
async fn an_old_id_raise_installed_before_commit_is_swept_after_it() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let old = h
        .live_session(fixture.coordinator_path(), CodingAgentKind::Claude)
        .await;
    let old_key = old.to_string();

    let raise_barrier =
        crate::commands::telegram::reader_demand_seam::install_before_install(&old_key);
    let restart_barrier =
        crate::commands::session::restart_reader_seam::install_after_old_release(&old_key);

    let app = h.app.handle().clone();
    let raise = tokio::spawn(async move {
        crate::commands::telegram::raise_reader_demand(&app, old, ReaderConsumer::Room, None).await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        raise_barrier.reached.notified(),
    )
    .await
    .expect("the old-id raise reaches the pre-install seam");

    let restart = {
        let app = h.app.handle().clone();
        let manager = Arc::clone(&h.manager);
        let pty = Arc::clone(&h.pty);
        let settings = h
            .app
            .state::<crate::config::settings::SettingsState>()
            .inner()
            .clone();
        tokio::spawn(async move {
            crate::commands::session::restart_session_inner_with_intent(
                &app,
                &manager,
                &pty,
                &settings,
                old,
                None,
                None,
                Some(false),
                true,
                TrustedRestartIntent::User,
                None,
                crate::config::sessions_persistence::default_creation_gate_enforcement(),
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        restart_barrier.reached.notified(),
    )
    .await
    .expect("the restart reaches the post-release seam");

    // Install inside the window: the old row is still live, so the raise's
    // liveness recheck passes and the reader stays installed.
    raise_barrier.release.notify_one();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.snapshot(old).await.0.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the paused raise must install before the commit"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(h.captures.is_open(&old_key));

    restart_barrier.release.notify_one();
    let restarted = restart
        .await
        .expect("join restart")
        .expect("restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    let raised = tokio::time::timeout(std::time::Duration::from_secs(10), raise)
        .await
        .expect("the raise resumes")
        .expect("the raise task joins");
    assert!(
        raised,
        "installed before the commit, the raise returns true"
    );

    // The post-commit release swept the old id.
    let (reader, demands) = h.snapshot(old).await;
    assert_eq!(reader, None, "the post-commit release sweeps the old id");
    assert!(demands.is_empty());
    assert!(!h.captures.is_open(&old_key));
    assert!(h.captures.slot(&old_key).is_none());
    // Exactly one reader, on the new id.
    assert!(h.snapshot(new).await.0.is_some());
    assert!(h.captures.is_open(&new.to_string()));
    h.release_all(new).await;
    h.close().await;
}

/// Test 9 (failure after runtime loss): when the replacement spawn fails
/// after the old PTY is gone, the old reader and its slot must close.
#[tokio::test]
async fn a_spawn_failure_after_runtime_loss_closes_the_old_reader() {
    for kind in [CodingAgentKind::Claude, CodingAgentKind::Codex] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h.live_session(fixture.coordinator_path(), kind).await;
        assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
        let old_key = old.to_string();
        assert!(h.captures.is_open(&old_key));

        // Session create was spawn #1; fail the replacement spawn.
        h.backend.fail_spawn(2);
        let error = h
            .restart(old)
            .await
            .expect_err("replacement spawn must fail");
        assert!(
            error.contains("synthetic scripted spawn failure"),
            "{error}"
        );

        assert!(h.snapshot(old).await.0.is_none());
        assert!(!h.captures.is_open(&old_key));
        assert!(h.captures.slot(&old_key).is_none());
        assert!(h.manager.read().await.get_session(old).await.is_none());
        h.close().await;
    }
}

/// Test 9 (dormant failure): a failed replacement for a dormant old session
/// retains the old row, reader and slot for retry.
#[tokio::test]
async fn a_dormant_restart_failure_keeps_the_old_reader_for_retry() {
    for kind in [CodingAgentKind::Claude, CodingAgentKind::Codex] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h.live_session(fixture.coordinator_path(), kind).await;
        h.backend.kill(old).expect("kill the dormant old");
        h.app
            .state::<crate::session::selection::SelectionCoordinator>()
            .container_lifecycle_sender()
            .route_lost(old, 23)
            .await
            .expect("reconcile the dormant route");
        assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
        let old_key = old.to_string();

        h.backend.fail_spawn(2);
        let error = h
            .restart(old)
            .await
            .expect_err("dormant replacement must fail");
        assert!(
            error.contains("synthetic scripted spawn failure"),
            "{error}"
        );

        let retained = h
            .manager
            .read()
            .await
            .get_session(old)
            .await
            .expect("the dormant row is retained");
        assert!(matches!(retained.status, SessionStatus::Exited(_)));
        assert!(h.snapshot(old).await.0.is_some(), "the old reader survives");
        assert!(h.captures.is_open(&old_key), "the old slot survives");
        h.release_all(old).await;
        h.close().await;
    }
}

/// Test 9 (preflight): a failure before teardown keeps the old runtime, so
/// its reader, demand set and slot survive untouched.
#[tokio::test]
async fn a_preflight_failure_keeps_the_old_reader_running() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let old = h.live_session(cwd, CodingAgentKind::Claude).await;
    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
    let old_key = old.to_string();
    let before = h.snapshot(old).await;

    h.app
        .state::<crate::config::settings::SettingsState>()
        .write()
        .await
        .archived_project_paths = vec![cwd.to_string_lossy().into_owned()];
    let error = h
        .restart(old)
        .await
        .expect_err("archived project blocks restart");
    assert!(
        error.contains("Cannot start a session in archived project"),
        "{error}"
    );

    assert_eq!(h.snapshot(old).await, before, "the old reader is untouched");
    assert!(h.captures.is_open(&old_key));
    assert!(h.captures.slot(&old_key).is_some());
    h.release_all(old).await;
    h.close().await;
}

/// Test 19: **create/destroy race.** The detached create-time Room raise is
/// paused after eligibility resolution; destroying the session during that
/// pause leaves no reader task, no demand and no open capture slot.
#[tokio::test]
async fn a_destroy_during_the_detached_room_raise_leaves_no_reader_state() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    let key = id.to_string();

    let barrier = crate::commands::telegram::reader_demand_seam::install_before_install(&key);
    let app = h.app.handle().clone();
    let room = fixture.room_path().to_path_buf();
    let raise = tokio::spawn(async move { raise_room_reader_demand_in(&app, &room, id).await });

    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        barrier.reached.notified(),
    )
    .await
    .expect("the detached raise reaches the pause seam");

    // Destroy wins the race while the raise is paused, exactly as the
    // production destroy path does: remove the row, then release the
    // session's demands.
    h.manager
        .read()
        .await
        .destroy_session(id)
        .await
        .expect("destroy session");
    crate::commands::telegram::release_all_reader_demands(h.app.handle(), id).await;

    barrier.release.notify_one();
    let raised = tokio::time::timeout(std::time::Duration::from_secs(10), raise)
        .await
        .expect("the raise resumes")
        .expect("the raise task joins");
    assert!(
        !raised,
        "a raise whose session died must not leave a running reader"
    );

    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(!tg.reader_is_running(id), "the reader is discarded");
        assert!(tg.reader_demands(id).is_empty(), "no demand remains");
    }
    assert!(!h.captures.is_open(&key), "the capture slot is closed");
    assert!(h.captures.slot(&key).is_none());
}

/// #2516 T1, the positive control: a disable landing between readiness
/// and install (slow path, no reader anywhere) installs no Room demand.
#[tokio::test]
async fn disabling_between_readiness_and_install_installs_no_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = room_raise_seam::install_before_add(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        disable_room(&h, &fixture),
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}

/// #2516 T2 (fast path): the session already runs a reader for its Bot
/// demand, so the disable removes **nothing**. The raise must still see it
/// and leave the reader Room-free.
#[tokio::test]
async fn disabling_between_readiness_and_add_leaves_a_bot_reader_room_free() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    assert!(
        crate::commands::telegram::raise_reader_demand(
            h.app.handle(),
            id,
            ReaderConsumer::Bot,
            None,
        )
        .await,
        "the Bot demand starts a reader"
    );

    let barrier = room_raise_seam::install_before_add(&id.to_string());
    race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        disable_room(&h, &fixture),
    )
    .await;

    let tg = h.bridge().await;
    let tg = tg.lock().await;
    assert_eq!(
        tg.reader_demands(id).into_iter().collect::<Vec<_>>(),
        vec![ReaderConsumer::Bot],
        "the disable must keep the Room demand off a Bot reader"
    );
    assert!(tg.reader_is_running(id), "the Bot reader keeps running");
}

/// #2516 T3, the negative control: the same pause with **no**
/// interference still installs the Room demand.
#[tokio::test]
async fn a_raise_with_no_interference_still_installs_the_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = room_raise_seam::install_before_add(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        async {},
    )
    .await;
    assert!(raised, "an undisturbed raise answers true");
    let tg = h.bridge().await;
    let tg = tg.lock().await;
    assert!(
        tg.reader_demands(id).contains(&ReaderConsumer::Room),
        "an undisturbed raise installs the Room demand"
    );
}

/// #2516 T4: a destroy-style `release_all` landing between readiness and
/// install installs no Room demand.
#[tokio::test]
async fn destroying_between_readiness_and_install_installs_no_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = room_raise_seam::install_before_add(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        crate::commands::telegram::release_all_reader_demands(h.app.handle(), id),
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}

/// #2516 T6: the raise clears the first bridge lock with an unmoved epoch
/// and parks at the existing `reader_demand_seam`, between the two locks;
/// a disable there must be caught by the second guard.
#[tokio::test]
async fn disabling_between_the_two_bridge_locks_installs_no_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier =
        crate::commands::telegram::reader_demand_seam::install_before_install(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        disable_room(&h, &fixture),
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}

/// #2516 T7: a release landing after the epoch sample and before the
/// readiness read. `release_all` leaves the room config alone, so
/// readiness still resolves `Ready` and the raise really reaches a guard.
#[tokio::test]
async fn destroying_before_the_readiness_read_installs_no_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = room_raise_seam::install_before_readiness(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        crate::commands::telegram::release_all_reader_demands(h.app.handle(), id),
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}
