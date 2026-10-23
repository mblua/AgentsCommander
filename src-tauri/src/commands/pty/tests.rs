use super::*;
use std::sync::{Arc, Mutex};

use crate::config::coordinator_clocks::{CoordinatorClocks, CoordinatorClocksState};
use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;

mod typing_hold;

#[tokio::test]
async fn e2992_p42_invalid_defers_hold() {
    use crate::config::settings::{
        SettingsAvailability, SettingsOwner, SettingsOwnerError, SettingsPublisher, SettingsState,
    };
    use crate::pty::manager::{
        ResponseCloseError, ResponseClosePublisher, ResponseCloseState, ResponseDeliveryProof,
        ResponseRouteIdentity, ReturnDeliveryProof,
    };
    use std::time::Instant;

    // Local fixture establishes real close eligibility through existing APIs.
    let eligible_app = |settings: SettingsState| {
        let state = ResponseCloseState::default();
        let baseline = Instant::now()
            .checked_sub(Duration::from_secs(120))
            .unwrap();
        for publisher in [
            ResponseClosePublisher::Manager,
            ResponseClosePublisher::Pty,
            ResponseClosePublisher::RawOutput,
            ResponseClosePublisher::Detector,
            ResponseClosePublisher::Menu,
            ResponseClosePublisher::Hold,
        ] {
            state.bind_publisher(publisher);
        }
        state.mark_work_bootstrapped();
        state.try_enable_readiness().unwrap();
        state.apply_settings(true, 30, baseline);
        let member = ResponseRouteIdentity {
            target: "p42:room/member".into(),
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 1,
            carrier_version: 1,
            backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        };
        let coordinator = ResponseRouteIdentity {
            target: "p42:room/coordinator".into(),
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 2,
            carrier_version: 1,
            backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        };
        for (identity, eligible) in [(&member, true), (&coordinator, false)] {
            state.register_route(identity.session_id, identity.generation, baseline);
            state.publish_manager_row(
                identity.session_id,
                identity.token,
                identity.generation,
                identity.carrier_version,
                eligible,
                false,
                baseline,
            );
            state.publish_manager_facts(identity.clone(), eligible, false, baseline);
        }
        let id = member.session_id;
        state.observe_detector(id, false, baseline);
        state.publish_menu(id, Some(false), baseline);
        state.publish_hold(id, None, true, baseline);
        let mut work = state.reserve_work(&member.target, "request").unwrap();
        work.accept_durable("request").unwrap();
        drop(work);
        let writer = state.reserve_writer(id, member.generation).unwrap();
        let context = state
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                "request".into(),
                "request".into(),
                baseline,
            )
            .unwrap();
        drop(writer);
        state.mark_request_submitted(&context, true);
        state
            .register_request(
                ResponseDeliveryProof {
                    context,
                    member: member.clone(),
                    coordinator: coordinator.clone(),
                    published_carrier_version: member.carrier_version,
                },
                baseline,
            )
            .unwrap();
        let context = state
            .admit_return(&member, &coordinator, "ack".into(), "ack".into(), baseline)
            .unwrap();
        state
            .observe_return_success(ReturnDeliveryProof { context }, baseline)
            .unwrap();
        assert!(state
            .select_ticket(id, Instant::now() + Duration::from_secs(60))
            .is_ok());
        let app = tauri::test::mock_builder()
            .manage(settings)
            .manage(state.clone())
            .manage(crate::pty::input_activity::new_typing_hold_state())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        (app, state, id)
    };

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let settings = crate::config::settings::AppSettings {
        typing_hold_seconds: 17,
        ..Default::default()
    };
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let owner = SettingsOwner::open_active(&path, Duration::ZERO).unwrap();
    let (app, publisher, id) = eligible_app(Arc::clone(&owner));
    assert_eq!(
        typing_hold_window(app.handle()).await.unwrap(),
        Duration::from_secs(17)
    );
    assert!(!typing_hold_defers_injection(app.handle(), id).await);
    let before_key = Instant::now();
    record_typing_hold_keystroke(app.handle(), id, b"ready");
    let after_key = Instant::now();
    let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
    assert!(hold
        .lock()
        .unwrap()
        .is_hold_active(id, Duration::from_secs(17)));
    assert!(matches!(
        publisher.select_ticket(id, before_key + Duration::from_secs(46)),
        Err(ResponseCloseError::Unknown)
    ));
    assert!(publisher
        .select_ticket(id, after_key + Duration::from_secs(60))
        .is_ok());

    let report = owner
        .with_operation("p42-invalid", |_, guard| {
            guard.publish(SettingsPublisher::SettingsSave, |path| {
                std::fs::write(path, b"{").map_err(|error| error.to_string())
            })
        })
        .await
        .unwrap();
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    assert!(owner.current_snapshot().await.is_err());
    assert!(typing_hold_window(app.handle()).await.is_err());
    assert!(typing_hold_defers_injection(app.handle(), id).await);
    // The old hold is expired at this query; only the key's unknown signal
    // should revoke eligibility. No failed settings snapshot refreshes it.
    assert!(publisher
        .select_ticket(id, Instant::now() + Duration::from_secs(120))
        .is_ok());
    hold.lock().unwrap().reset(id);
    record_typing_hold_keystroke(app.handle(), id, b"invalid");
    assert!(
        hold.lock()
            .unwrap()
            .is_hold_active(id, Duration::from_secs(17)),
        "independent key evidence survives settings failure"
    );
    assert!(
        matches!(
            publisher.select_ticket(id, Instant::now() + Duration::from_secs(120)),
            Err(ResponseCloseError::Unknown)
        ),
        "Invalid key must revoke close eligibility"
    );
    let refreshed = Instant::now();
    publisher.refresh_hold_snapshot(id, Duration::from_secs(17), false, refreshed);
    assert!(
        publisher
            .select_ticket(id, refreshed + Duration::from_secs(31))
            .is_ok(),
        "an observed open refresh recovers the retained policy"
    );
    owner.drain_blocking(Duration::ZERO);

    // Legacy write forwarding is valid only on this separate Staged owner.
    let settings =
        crate::config::settings::new_settings_state(crate::config::settings::AppSettings {
            typing_hold_seconds: 17,
            ..Default::default()
        });
    let (busy_app, busy_publisher, busy_id) = eligible_app(Arc::clone(&settings));
    let write = settings.write().await;
    assert_eq!(
        settings.try_current_read().err(),
        Some(SettingsOwnerError::Busy)
    );
    record_typing_hold_keystroke(busy_app.handle(), busy_id, b"busy");
    assert!(busy_app
        .state::<crate::pty::input_activity::TypingHoldState>()
        .lock()
        .unwrap()
        .is_hold_active(busy_id, Duration::from_secs(17)));
    assert!(
        matches!(
            busy_publisher.select_ticket(busy_id, Instant::now() + Duration::from_secs(120)),
            Err(ResponseCloseError::Unknown)
        ),
        "Busy key must revoke close eligibility"
    );
    drop(write);
    let refreshed = Instant::now();
    busy_publisher.refresh_hold_snapshot(busy_id, Duration::from_secs(17), false, refreshed);
    assert!(busy_publisher
        .select_ticket(busy_id, refreshed + Duration::from_secs(31))
        .is_ok());
}

struct FreshIntentFixture {
    app: tauri::App<tauri::test::MockRuntime>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    clocks: CoordinatorClocksState,
    session_id: Uuid,
    fqn: String,
}

fn user_input_test_app(
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    clocks: CoordinatorClocksState,
) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(session_mgr)
        .manage(clocks)
        .manage(crate::pty::input_activity::new_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build user input test app")
}

/// #1682 - `user_input_test_app`'s builder plus a managed `IdleDetector`, so
/// test 7's control-write mark has somewhere to live. A separate fixture
/// deliberately: `user_input_test_app` is shared by more than one existing
/// test and must stay byte-unchanged.
fn user_input_test_app_with_idle_detector(
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    clocks: CoordinatorClocksState,
) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(session_mgr)
        .manage(clocks)
        .manage(crate::pty::input_activity::new_state())
        .manage(crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {}))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build user input + idle detector test app")
}

async fn fresh_intent_fixture() -> FreshIntentFixture {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let app = user_input_test_app(session_mgr.clone(), clocks.clone());
    let cwd = "C:/ac-test/project/.ac/wg-871-dev-team/__agent_tech-lead".to_string();
    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let session = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            cwd,
            None,
            None,
            Vec::<SessionRepo>::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create coordinator session")
    };

    {
        let mgr = session_mgr.read().await;
        mgr.set_start_fresh_on_restore(session.id, true).await;
    }
    {
        let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
        assert!(guard.mark_start_fresh(&fqn, chrono::Utc::now()));
    }

    FreshIntentFixture {
        app,
        session_mgr,
        clocks,
        session_id: session.id,
        fqn,
    }
}

async fn record_fresh(f: &FreshIntentFixture) -> bool {
    let mgr = f.session_mgr.read().await;
    mgr.get_session(f.session_id)
        .await
        .expect("session should exist")
        .start_fresh_on_restore
}

fn mirror_fresh(f: &FreshIntentFixture) -> bool {
    f.clocks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .start_fresh_at(&f.fqn)
        .is_some()
}

fn inject_continue_after_restore(start_fresh_on_restore: bool) -> bool {
    !start_fresh_on_restore
}

#[tokio::test]
async fn restart_non_substantive_terminal_writes_keep_restore_fresh() {
    let f = fresh_intent_fixture().await;
    for chunk in [
        b"\x1b[I".as_slice(),
        b"\x1b[A".as_slice(),
        b"\x1b]11;rgb:1234/5678/9abc\x07".as_slice(),
        b"\r".as_slice(),
    ] {
        note_user_message_to_session(
            f.app.handle(),
            f.session_id,
            UserInputSource::Terminal(chunk),
        )
        .await;
        assert!(record_fresh(&f).await);
        assert!(mirror_fresh(&f));
    }

    assert!(!inject_continue_after_restore(record_fresh(&f).await));
}

#[tokio::test]
async fn restart_substantive_terminal_prompt_allows_resume_on_restore() {
    let f = fresh_intent_fixture().await;
    note_user_message_to_session(
        f.app.handle(),
        f.session_id,
        UserInputSource::Terminal(b"do the thing\r"),
    )
    .await;

    assert!(!record_fresh(&f).await);
    assert!(!mirror_fresh(&f));
    assert!(inject_continue_after_restore(record_fresh(&f).await));
}

#[tokio::test]
async fn a_non_substantive_terminal_write_does_not_arm() {
    let f = fresh_intent_fixture().await;
    assert!(
        !f.session_mgr
            .read()
            .await
            .get_session(f.session_id)
            .await
            .unwrap()
            .agent_turn_armed
    );
    for chunk in [
        b"\x1b[I".as_slice(),
        b"\x1b[A".as_slice(),
        b"\x1b]11;rgb:1234/5678/9abc\x07".as_slice(),
        b"\r".as_slice(),
        b"\x03".as_slice(),
    ] {
        note_user_message_to_session(
            f.app.handle(),
            f.session_id,
            UserInputSource::Terminal(chunk),
        )
        .await;
        assert!(
            !f.session_mgr
                .read()
                .await
                .get_session(f.session_id)
                .await
                .unwrap()
                .agent_turn_armed
        );
    }

    // The control: without it this test stays green even if the
    // `if substantive` guard is deleted.
    note_user_message_to_session(
        f.app.handle(),
        f.session_id,
        UserInputSource::Terminal(b"do the thing\r"),
    )
    .await;
    assert!(
        f.session_mgr
            .read()
            .await
            .get_session(f.session_id)
            .await
            .unwrap()
            .agent_turn_armed
    );
}

#[tokio::test]
async fn a_substantive_terminal_prompt_arms() {
    let f = fresh_intent_fixture().await;
    assert!(
        !f.session_mgr
            .read()
            .await
            .get_session(f.session_id)
            .await
            .unwrap()
            .agent_turn_armed
    );
    note_user_message_to_session(
        f.app.handle(),
        f.session_id,
        UserInputSource::Terminal(b"do the thing\r"),
    )
    .await;
    assert!(
        f.session_mgr
            .read()
            .await
            .get_session(f.session_id)
            .await
            .unwrap()
            .agent_turn_armed
    );

    // The latch is never cleared, so a later non-substantive write leaves it set.
    note_user_message_to_session(
        f.app.handle(),
        f.session_id,
        UserInputSource::Terminal(b"\x1b[I"),
    )
    .await;
    assert!(
        f.session_mgr
            .read()
            .await
            .get_session(f.session_id)
            .await
            .unwrap()
            .agent_turn_armed
    );
}

/// #1682 - the terminal plane: a PTY write marks the session, and the
/// substantive submission that follows clears the mark. The \x1b[I step is the
/// control, and the last step is the assertion that goes red if the clear is
/// omitted from the `if substantive` block.
/// #2411 Typed input clears the auto-closed marker (production `pty.rs` clear).
#[tokio::test]
async fn typed_input_clears_auto_closed_marker() {
    let f = fresh_intent_fixture().await;
    {
        let mut guard = f.clocks.lock().unwrap_or_else(|e| e.into_inner());
        assert!(guard.mark_auto_closed(&f.fqn, chrono::Utc::now()));
    }
    let handle = f.app.handle().clone();
    note_user_message_to_session(&handle, f.session_id, UserInputSource::CompleteMessage).await;
    assert_eq!(
        f.clocks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .auto_closed_at(&f.fqn),
        None
    );
}

#[tokio::test]
async fn a_terminal_write_marks_and_a_substantive_one_clears() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let app = user_input_test_app_with_idle_detector(session_mgr.clone(), clocks.clone());
    let cwd = "C:/ac-test/project/.ac/wg-871-dev-team/__agent_tech-lead".to_string();
    let session = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            cwd,
            None,
            None,
            Vec::<SessionRepo>::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create coordinator session")
    };
    let handle = app.handle().clone();
    let idle = handle
        .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
        .inner()
        .clone();
    idle.register_session(session.id, crate::session::profile::IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(session.id);
    assert!(
        idle.control_write_age(session.id).is_none(),
        "no mark before any write"
    );

    // The control: this path never writes to the PTY, so it never reaches
    // `mark_successful_pty_write_busy` and must leave the mark unset.
    note_user_message_to_session(&handle, session.id, UserInputSource::Terminal(b"\x1b[I")).await;
    assert!(
        idle.control_write_age(session.id).is_none(),
        "note_user_message_to_session does not write to the PTY and must not mark"
    );

    // The write itself, as `pty_write` performs it.
    mark_successful_pty_write_busy(&handle, session.id, 3).await;
    assert!(
        idle.control_write_age(session.id).is_some(),
        "a PTY write into an idle session must mark"
    );

    note_user_message_to_session(
        &handle,
        session.id,
        UserInputSource::Terminal(b"do the thing\r"),
    )
    .await;
    assert!(
        idle.control_write_age(session.id).is_none(),
        "a substantive submission must clear the mark"
    );
}

#[test]
fn boundary_metadata_failure_dominates_applied_and_unchanged_outcomes() {
    use BoundaryMetadataOutcome as O;
    assert_eq!(
        combine_boundary_metadata(O::Applied, O::Unchanged),
        O::Applied
    );
    assert_eq!(
        combine_boundary_metadata(O::Unchanged, O::Unchanged),
        O::Unchanged
    );
    assert_eq!(combine_boundary_metadata(O::Failed, O::Applied), O::Failed);
    assert_eq!(combine_boundary_metadata(O::Applied, O::Failed), O::Failed);
}

#[tokio::test]
async fn restart_injected_body_allows_resume_on_restore() {
    let f = fresh_intent_fixture().await;
    note_post_boundary_content_to_session(f.app.handle(), f.session_id).await;

    assert!(!record_fresh(&f).await);
    assert!(!mirror_fresh(&f));
    assert!(inject_continue_after_restore(record_fresh(&f).await));
}

#[tokio::test]
async fn ctrl_c_cancelled_terminal_line_keeps_restore_fresh() {
    let f = fresh_intent_fixture().await;
    for chunk in [
        b"do the thing".as_slice(),
        b"\x03".as_slice(),
        b"\r".as_slice(),
    ] {
        note_user_message_to_session(
            f.app.handle(),
            f.session_id,
            UserInputSource::Terminal(chunk),
        )
        .await;
    }

    assert!(record_fresh(&f).await);
    assert!(mirror_fresh(&f));
    assert!(!inject_continue_after_restore(record_fresh(&f).await));
}

/// (#1793) The #260 case: `IdleDetector::register_session` seeds
/// `activity[id]` at spawn, so a session that has printed nothing still
/// reaches `waiting_for_input`. The seed alone must NOT read as printable
/// output, which is the whole reason this helper exists.
#[test]
fn session_printed_since_is_false_for_a_session_that_only_has_the_spawn_seed() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let app = user_input_test_app_with_idle_detector(session_mgr, clocks);
    let id = Uuid::new_v4();
    app.state::<Arc<crate::pty::idle_detector::IdleDetector>>()
        .register_session(id, crate::session::profile::IdleTuning::DEFAULT);

    let t = Instant::now();

    assert!(
        !session_printed_since(app.handle(), id, t),
        "the spawn seed alone must never count as printable output"
    );
}

/// (#1793) Real printable output after the reference instant does count.
#[test]
fn session_printed_since_is_true_after_real_printable_output() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let app = user_input_test_app_with_idle_detector(session_mgr, clocks);
    let id = Uuid::new_v4();
    app.state::<Arc<crate::pty::idle_detector::IdleDetector>>()
        .register_session(id, crate::session::profile::IdleTuning::DEFAULT);

    let t = Instant::now();
    app.state::<Arc<crate::pty::idle_detector::IdleDetector>>()
        .record_activity_with_bytes(id, 1);

    assert!(
        session_printed_since(app.handle(), id, t),
        "printable output after the reference instant must be visible"
    );
}

/// (#1793) Fail CLOSED without a managed detector. This is the OPPOSITE
/// direction from every neighbouring `try_state` helper in this file, so it
/// is pinned explicitly: a missing detector must never authorise a write.
#[test]
fn session_printed_since_fails_closed_without_a_managed_detector() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let app = user_input_test_app(session_mgr, clocks);

    assert!(
        !session_printed_since(app.handle(), Uuid::new_v4(), Instant::now()),
        "no managed detector must fail closed, never open"
    );
}

// ---------------------------------------------------------------------
// #2232 phase 3, section 10.1: the budget recharge source filter.
// ---------------------------------------------------------------------

/// A coordinator session whose cwd sits inside a real `room-<N>-*` root, so
/// `room_root_of_orchestrator` resolves to a temp directory this test owns.
async fn capture_budget_fixture(
    temp: &tempfile::TempDir,
) -> (FreshIntentFixture, std::path::PathBuf) {
    let room = temp.path().join("room-2232-dev-team");
    let replica = room.join("__agent_tech-lead");
    std::fs::create_dir_all(&replica).expect("replica dir");

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
    let app = user_input_test_app(session_mgr.clone(), clocks.clone());
    let cwd = replica.to_string_lossy().into_owned();
    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let session = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            cwd,
            None,
            None,
            Vec::<SessionRepo>::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create coordinator session")
    };
    crate::capture::state::forget_room_for_tests(&room);
    (
        FreshIntentFixture {
            app,
            session_mgr,
            clocks,
            session_id: session.id,
            fqn,
        },
        room,
    )
}

/// Spend the room's whole budget, so a recharge is observable.
fn exhaust_budget(room: &std::path::Path) {
    crate::capture::state::spend_budget_for_tests(room, crate::capture::state::BUDGET_CAP);
    assert_eq!(crate::capture::state::budget(room), 0);
}

/// Test 17, assertion 1: substantive terminal input recharges to the cap.
#[tokio::test]
async fn substantive_terminal_input_recharges_the_budget() {
    let temp = tempfile::tempdir().expect("temp");
    let (f, room) = capture_budget_fixture(&temp).await;
    exhaust_budget(&room);

    note_user_message_to_session(
        f.app.handle(),
        f.session_id,
        UserInputSource::Terminal(b"do the thing\r"),
    )
    .await;

    assert_eq!(
        crate::capture::state::budget(&room),
        crate::capture::state::BUDGET_CAP
    );
}

/// Test 17, assertion 2: the same text from the web does **not** recharge.
#[tokio::test]
async fn substantive_web_input_does_not_recharge_the_budget() {
    let temp = tempfile::tempdir().expect("temp");
    let (f, room) = capture_budget_fixture(&temp).await;
    exhaust_budget(&room);

    note_user_message_to_session(
        f.app.handle(),
        f.session_id,
        UserInputSource::Web(b"do the thing\r"),
    )
    .await;

    assert_eq!(crate::capture::state::budget(&room), 0);
}

/// Test 17, assertion 3: inbound Telegram, which arrives as a complete
/// message, does **not** recharge. This is what stops anyone with chat
/// access, including a bot, keeping an automatic chain alive forever.
#[tokio::test]
async fn a_complete_message_does_not_recharge_the_budget() {
    let temp = tempfile::tempdir().expect("temp");
    let (f, room) = capture_budget_fixture(&temp).await;
    exhaust_budget(&room);

    note_user_message_to_session(
        f.app.handle(),
        f.session_id,
        UserInputSource::CompleteMessage,
    )
    .await;

    assert_eq!(crate::capture::state::budget(&room), 0);
}

/// Test 17, assertion 4: a lone control key on the terminal is not
/// substantive, so it does **not** recharge.
#[tokio::test]
async fn a_non_substantive_terminal_write_does_not_recharge_the_budget() {
    let temp = tempfile::tempdir().expect("temp");
    let (f, room) = capture_budget_fixture(&temp).await;
    exhaust_budget(&room);

    for chunk in [b"\x1b[A".as_slice(), b"\r".as_slice(), b"\x03".as_slice()] {
        note_user_message_to_session(
            f.app.handle(),
            f.session_id,
            UserInputSource::Terminal(chunk),
        )
        .await;
        assert_eq!(crate::capture::state::budget(&room), 0, "chunk={chunk:?}");
    }
}

/// Test 27: on a current-thread runtime, substantive terminal input still
/// progresses while another holder keeps the room's advisory lock. The room
/// lookup is async and the recharge is a lock-free in-memory leaf, so
/// neither can block the reactor.
#[test]
fn substantive_terminal_input_progresses_while_the_room_lock_is_held() {
    let temp = tempfile::tempdir().expect("temp");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("current-thread runtime");
    runtime.block_on(async {
        let (f, room) = capture_budget_fixture(&temp).await;
        exhaust_budget(&room);

        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let holder_room = room.clone();
        let holder = std::thread::spawn(move || {
            let _guard = crate::capture::state::hold_room_lock_for_tests(&holder_room);
            held_tx.send(()).expect("signal held");
            release_rx.recv().expect("release");
        });
        held_rx.recv().expect("lock held");

        let started = Instant::now();
        note_user_message_to_session(
            f.app.handle(),
            f.session_id,
            UserInputSource::Terminal(b"do the thing\r"),
        )
        .await;
        let elapsed = started.elapsed();

        // The reactor is still live: a timer set after the call completes.
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::time::sleep(std::time::Duration::from_millis(1)),
        )
        .await
        .expect("the reactor kept running");

        release_tx.send(()).expect("release");
        holder.join().expect("holder");

        assert_eq!(
            crate::capture::state::budget(&room),
            crate::capture::state::BUDGET_CAP
        );
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "the input path waited {elapsed:?} on a held file lock"
        );
    });
}
