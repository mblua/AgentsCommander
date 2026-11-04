use super::the_pty::{boundary_rig, BoundaryRig};
use super::*;

type OperationSnapshot = (String, String, String, String, Option<Vec<u8>>, String);

fn query_operation_snapshot(
    conn: &rusqlite::Connection,
    injection_id: &String,
) -> OperationSnapshot {
    conn.query_row(
        "SELECT \
    sender_fqn,target_fqn,op_id,request_fingerprint,payload,status FROM \
    pty_input_operations WHERE injection_id=?1",
        [injection_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )
    .unwrap()
}

async fn one_poll_identities(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    tracker: &crate::pty::manager::ResponseCloseState,
    sender_id: Uuid,
    target_id: Uuid,
) -> (
    crate::pty::manager::ResponseRouteIdentity,
    crate::pty::manager::ResponseRouteIdentity,
) {
    use crate::pty::manager::ResponseRouteIdentity;
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    let member_row = manager.get_session(target_id).await.unwrap();
    let coordinator_row = manager.get_session(sender_id).await.unwrap();
    let member = ResponseRouteIdentity {
        target: CANONICAL_WAKE_TO.into(),
        session_id: target_id,
        token: member_row.token,
        generation: tracker.generation(target_id).unwrap(),
        carrier_version: 1,
        backend: crate::pty::backend::SessionBackendKind::LocalProcess,
    };
    let coordinator = ResponseRouteIdentity {
        target: CANONICAL_WAKE_FROM.into(),
        session_id: sender_id,
        token: coordinator_row.token,
        generation: tracker.generation(sender_id).unwrap(),
        ..member.clone()
    };
    (member, coordinator)
}

fn bind_one_poll_tracker(
    tracker: &crate::pty::manager::ResponseCloseState,
    member: &crate::pty::manager::ResponseRouteIdentity,
    coordinator: &crate::pty::manager::ResponseRouteIdentity,
    now: std::time::Instant,
) {
    use crate::pty::manager::ResponseClosePublisher;
    for publisher in [
        ResponseClosePublisher::Manager,
        ResponseClosePublisher::Pty,
        ResponseClosePublisher::RawOutput,
        ResponseClosePublisher::Detector,
        ResponseClosePublisher::Menu,
        ResponseClosePublisher::Hold,
    ] {
        tracker.bind_publisher(publisher);
    }
    tracker.mark_work_bootstrapped();
    tracker.try_enable_readiness().unwrap();
    tracker.apply_settings(true, 30, now);
    for (identity, eligible) in [(member, true), (coordinator, false)] {
        tracker.register_route(identity.session_id, identity.generation, now);
        tracker.publish_manager_row(
            identity.session_id,
            identity.token,
            identity.generation,
            identity.carrier_version,
            eligible,
            false,
            now,
        );
        tracker.publish_manager_facts(identity.clone(), eligible, false, now);
    }
    tracker.observe_detector(member.session_id, false, now);
    tracker.publish_menu(member.session_id, Some(false), now);
    tracker.publish_hold(member.session_id, None, true, now);
}

fn republish_one_poll_facts(
    tracker: &crate::pty::manager::ResponseCloseState,
    member: &crate::pty::manager::ResponseRouteIdentity,
    coordinator: &crate::pty::manager::ResponseRouteIdentity,
) {
    // Same facts/generation after ordinary engine changes; no counter resets.
    let now = std::time::Instant::now() + Duration::from_secs(3600);
    tracker.publish_manager_row(
        member.session_id,
        member.token,
        member.generation,
        member.carrier_version,
        true,
        false,
        now,
    );
    tracker.publish_manager_facts(member.clone(), true, false, now);
    tracker.publish_manager_row(
        coordinator.session_id,
        coordinator.token,
        coordinator.generation,
        coordinator.carrier_version,
        false,
        false,
        now,
    );
    tracker.publish_manager_facts(coordinator.clone(), false, false, now);
    tracker.observe_detector(member.session_id, false, now);
    tracker.publish_menu(member.session_id, Some(false), now);
    tracker.publish_hold(member.session_id, None, true, now);
}

struct OnePollCase<'a> {
    app: &'a tauri::AppHandle<tauri::test::MockRuntime>,
    tracker: &'a crate::pty::manager::ResponseCloseState,
    member: &'a crate::pty::manager::ResponseRouteIdentity,
    coordinator: &'a crate::pty::manager::ResponseRouteIdentity,
    target_id: Uuid,
    scenario_target: &'a String,
    scenario_key: &'a String,
    now: std::time::Instant,
}

fn assert_one_poll_preserved(
    case: &OnePollCase<'_>,
    shutdown_after_start: bool,
    original: &OperationSnapshot,
    preserved: &OperationSnapshot,
) {
    let (tracker, member, coordinator) = (case.tracker, case.member, case.coordinator);
    let (app, target_id, now) = (case.app, case.target_id, case.now);
    let (scenario_target, scenario_key) = (case.scenario_target, case.scenario_key);
    assert_eq!(
        (&preserved.0, &preserved.1, &preserved.2, &preserved.3),
        (&original.0, &original.1, &original.2, &original.3)
    );
    assert_eq!(tracker.generation(target_id), Some(member.generation));
    assert!(!tracker.target_is_closing(scenario_target));
    if shutdown_after_start {
        assert!(matches!(
            preserved.5.as_str(),
            "queued" | "preparing" | "retry" | "actuating"
        ));
        assert_eq!(preserved.4, original.4);
        assert!(tracker
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                scenario_key.clone(),
                original.3.clone(),
                now
            )
            .is_ok());
        assert!(shared_engine_writes(app, target_id).is_empty());
    } else {
        assert_eq!(preserved.5, "injected");
        assert_eq!(
            shared_engine_writes(app, target_id),
            vec![
                b"one-poll durable body".to_vec(),
                b"\r".to_vec(),
                b"\r".to_vec()
            ]
        );
        assert!(matches!(
            tracker.reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                scenario_key.clone(),
                original.3.clone(),
                now
            ),
            Err(crate::pty::manager::ResponseCloseError::Conflict)
        ));
    }
}

fn one_poll_diagnostic_probe(
    tracker: &crate::pty::manager::ResponseCloseState,
    member: &crate::pty::manager::ResponseRouteIdentity,
    coordinator: &crate::pty::manager::ResponseRouteIdentity,
    scenario_target: &String,
    sensitivity: u64,
) {
    use crate::pty::manager::{ResponseDeliveryProof, ReturnDeliveryProof};
    let dispatch = if sensitivity == 1 {
        Some(tracker.begin_delivery(scenario_target).unwrap())
    } else {
        None
    };
    let provisional = if sensitivity == 3 {
        Some(
            tracker
                .reserve_work(
                    scenario_target,
                    &format!("diagnostic-held-{}", Uuid::new_v4()),
                )
                .unwrap(),
        )
    } else {
        None
    };
    let probe_key = format!("diagnostic-request-{}", Uuid::new_v4());
    let probe_digest = "diagnostic-body";
    let mut admission = tracker.reserve_work(scenario_target, &probe_key).unwrap();
    assert_eq!(admission.accept_durable(probe_digest), Ok(true));
    drop(admission);
    let probe_at = std::time::Instant::now() + Duration::from_secs(7200 + sensitivity * 60);
    let context = tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            probe_key.clone(),
            probe_digest.into(),
            probe_at,
        )
        .unwrap();
    // Unit-returning API; successful register below validates submitted=true.
    tracker.mark_request_submitted(&context, true);
    let registered = tracker.register_request(
        ResponseDeliveryProof {
            context,
            member: member.clone(),
            coordinator: coordinator.clone(),
            published_carrier_version: member.carrier_version,
        },
        probe_at,
    );
    if sensitivity == 3 {
        assert!(
            registered.is_err(),
            "retained real provisional admission must be detected"
        );
        tracker.settle_terminal(scenario_target, &probe_key, probe_at);
        drop(provisional);
        drop(dispatch);
        return;
    }
    registered.unwrap();
    let context = tracker
        .admit_return(
            member,
            coordinator,
            format!("diagnostic-return-{}", Uuid::new_v4()),
            "diagnostic-return-body".into(),
            probe_at,
        )
        .unwrap();
    tracker
        .observe_return_success(ReturnDeliveryProof { context }, probe_at)
        .unwrap();
    let ticket = tracker.select_ticket(member.session_id, probe_at + Duration::from_secs(30));
    if sensitivity == 1 {
        assert!(
            ticket.is_err(),
            "retained real SAME-target dispatch must be detected"
        );
    } else {
        assert!(
            ticket.is_ok(),
            "same-object final success proves dispatch/provisional/writer counters zero"
        );
    }
    drop(dispatch);
    drop(provisional);
}

async fn one_poll_shutdown_case(shutdown_after_start: bool) {
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    assert!(app.manage(crate::shutdown::ShutdownSignal::new()));
    let (route, sender_id, target_id) =
        seed_shared_pty_engine_sessions(&fixture, SessionStatus::Idle).await;
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let tracker = pty.lock().unwrap().response_close_state();
    assert!(app.manage(tracker.clone()));
    let (member, coordinator) = one_poll_identities(&app, &tracker, sender_id, target_id).await;
    let path = fixture._temp.path().join("one-poll-shutdown.sqlite3");
    let store = Arc::new(crate::api::message_store::MessageStore::open(path.clone()).unwrap());
    store.bind_response_close(tracker.clone());
    let state = crate::api::message_store::MessageStoreState::ready(Arc::clone(&store));

    let now = std::time::Instant::now() + Duration::from_secs(3600);
    bind_one_poll_tracker(&tracker, &member, &coordinator, now);

    let held_permit = if shutdown_after_start {
        Some(
            PtyManager::acquire_input_writer(&pty, target_id)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let injection_id =
        enqueue_shared_pty_engine_operation(&store, &route, sender_id, b"one-poll durable body");
    let conn = rusqlite::Connection::open(&path).unwrap();
    let original = query_operation_snapshot(&conn, &injection_id);
    assert_eq!(original.1, member.target);
    let scenario_target = original.1.clone();
    let scenario_key = format!("pty:{}:{}:{}", original.0.len(), original.0, original.2);
    assert!(tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            scenario_key.clone(),
            original.3.clone(),
            now
        )
        .is_ok());
    let hooks = MailboxTestHooks::default();
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let mut engine = Box::pin(poller.dispatch_pty_input_operation(
        &app,
        &state,
        &injection_id,
        crate::phone::types::PtyInputSourcePlane::HostCli,
        None,
    ));
    if shutdown_after_start {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        assert!(matches!(
            std::future::Future::poll(engine.as_mut(), &mut context),
            std::task::Poll::Pending
        ));
        app.state::<crate::shutdown::ShutdownSignal>().trigger();
    }
    tokio::time::timeout(Duration::from_secs(10), engine)
        .await
        .expect("engine must finish; timeout is failure");
    assert_eq!(state.target_gate.as_ref().unwrap().exact_entry_count(), 0);
    drop(held_permit);
    // Phase1 durable snapshot precedes synthetic counter diagnostic.
    let preserved = query_operation_snapshot(&conn, &injection_id);
    let case = OnePollCase {
        app: &app,
        tracker: &tracker,
        member: &member,
        coordinator: &coordinator,
        target_id,
        scenario_target: &scenario_target,
        scenario_key: &scenario_key,
        now,
    };
    assert_one_poll_preserved(&case, shutdown_after_start, &original, &preserved);
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    let preservation_snapshot = preserved.clone();
    assert_eq!(preservation_snapshot.1, scenario_target);
    republish_one_poll_facts(&tracker, &member, &coordinator);

    // Phase 2: synthetic TEST-LOCAL discharge ONLY. This does not prove FS/SQL
    // terminalization, peer submission, or P05 proof. Phase 1 alone proves retention.
    assert_eq!(member.target, scenario_target);
    assert!(!tracker.target_is_closing(&scenario_target));
    tracker.settle_terminal(&scenario_target, &scenario_key, std::time::Instant::now());
    // A leaked dispatch/provisional counter is not changed by settle_terminal.
    // Each fresh probe uses the SAME tracker, target, generation and facts.
    for sensitivity in 0..5 {
        one_poll_diagnostic_probe(
            &tracker,
            &member,
            &coordinator,
            &scenario_target,
            sensitivity,
        );
    }

    // Diagnostic leaves the phase1 SQL snapshot byte-identical.
    let after = query_operation_snapshot(&conn, &injection_id);
    assert_eq!(after, preservation_snapshot);
}

#[tokio::test]
async fn shared_pty_engine_worker_drains_promptly_on_shutdown_without_writing() {
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    // Inject a controllable shutdown signal that the real engine will observe
    // through app state, exactly as the dispatcher's outer loop does.
    app.manage(crate::shutdown::ShutdownSignal::new());
    let (route, sender_id, target_id) =
        seed_shared_pty_engine_sessions(&fixture, SessionStatus::Idle).await;
    let store = Arc::new(
        crate::api::message_store::MessageStore::open(
            fixture._temp.path().join("shared-engine-shutdown.sqlite3"),
        )
        .unwrap(),
    );
    let state = crate::api::message_store::MessageStoreState::ready(Arc::clone(&store));

    // Hold the target permit so a real engine worker blocks in its pre-boundary
    // permit wait on a still-valid (full 10-minute) operation.
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let held_permit = PtyManager::acquire_input_writer(&pty, target_id)
        .await
        .unwrap();
    let injection_id =
        enqueue_shared_pty_engine_operation(&store, &route, sender_id, b"must never be written");

    let worker = {
        let app = app.clone();
        let state = state.clone();
        let injection_id = injection_id.clone();
        tokio::spawn(async move {
            MailboxPoller::default()
                .dispatch_pty_input_operation(
                    &app,
                    &state,
                    &injection_id,
                    crate::phone::types::PtyInputSourcePlane::HostCli,
                    None,
                )
                .await;
        })
    };

    // Let the worker reach and block on the held permit, then signal shutdown.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !worker.is_finished(),
        "the engine worker must still be blocked on the held permit before shutdown"
    );
    app.state::<crate::shutdown::ShutdownSignal>().trigger();

    // A blocked real engine worker drains promptly on shutdown, far inside the
    // 10-minute request window, and never writes a byte.
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .expect("the engine worker must drain on shutdown, not hold its slot until TTL")
        .expect("the worker task must not panic");
    assert!(shared_engine_writes(&app, target_id).is_empty());
    assert_eq!(state.target_gate.as_ref().unwrap().exact_entry_count(), 0);
    drop(held_permit);
    // G: shutdown after one engine-future poll, NOT exact permit-wait proof.
    // Legacy300ms above is unchanged and does not establish this new cut.
    for shutdown_after_start in [true, false] {
        one_poll_shutdown_case(shutdown_after_start).await;
    }
}

struct IdentityMutationRig {
    sender_id: Uuid,
    target_id: Uuid,
    route: crate::config::teams::VerifiedPtyInputRoute,
    pty: Arc<Mutex<PtyManager>>,
    sender_cwd_identity: crate::path_identity::VerifiedPathIdentity,
    permit: crate::pty::input_route::PtyInputPermit,
    idle: Arc<crate::pty::idle_detector::IdleDetector>,
    sessions: SessionManager,
    settings: SettingsState,
}

async fn identity_mutation_rig(
    fixture: &MailboxFixture,
) -> (
    IdentityMutationRig,
    crate::pty::input_route::PtyAuthorityRouteProof,
) {
    use crate::pty::backend::SessionBackendKind;
    use crate::session::profile::IdleTuning;

    let app = fixture.app.handle().clone();
    let sender_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "coordinator",
        SessionStatus::Running,
        None,
    )
    .await;
    let target_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "member",
        SessionStatus::Idle,
        None,
    )
    .await;
    let paths = vec![fixture._temp.path().to_string_lossy().to_string()];
    let route = crate::config::teams::verify_pty_input_route(
        &fixture.sender_cwd,
        false,
        CANONICAL_WAKE_TO,
        &paths,
    )
    .expect("verified route");
    let target_cwd_identity =
        crate::path_identity::verify_directory(&fixture.target_cwd).expect("target cwd identity");
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            target_id,
            SessionBackendKind::LocalProcess,
            Some(target_cwd_identity.clone()),
            Some(route.target.replica_identity.clone()),
        )
        .expect("record target route");
    let sender_cwd_identity =
        crate::path_identity::verify_directory(&fixture.sender_cwd).expect("sender cwd identity");
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            sender_id,
            SessionBackendKind::LocalProcess,
            Some(sender_cwd_identity.clone()),
            Some(route.sender.replica_identity.clone()),
        )
        .expect("record sender route");
    let authority_route =
        PtyManager::authority_route_proof(&pty, sender_id).expect("sender authority route proof");
    let permit = PtyManager::acquire_input_writer(&pty, target_id)
        .await
        .expect("target input permit");
    let route_guard = PtyManager::lock_route_for_verified_write(
        &permit,
        SessionBackendKind::LocalProcess,
        &target_cwd_identity,
        &route.target.replica_identity,
    )
    .expect("route is valid before mutation");
    drop(route_guard);

    let idle = app
        .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
        .inner()
        .clone();
    idle.register_session(target_id, IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(target_id);
    let sessions = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let manager = state.read().await.clone();
        manager
    };
    let settings = app.state::<SettingsState>().inner().clone();
    let rig = IdentityMutationRig {
        sender_id,
        target_id,
        route,
        pty,
        sender_cwd_identity,
        permit,
        idle,
        sessions,
        settings,
    };
    (rig, authority_route)
}

async fn identity_mutation_boundary<'a>(
    rig: &'a IdentityMutationRig,
    authority_route: &'a crate::pty::input_route::PtyAuthorityRouteProof,
    final_external_check: fn() -> bool,
) -> Result<
    crate::pty::input_route::PtyRouteWriteGuard<'a>,
    crate::pty::idle_detector::PtyInputBoundaryFailure,
> {
    use crate::pty::backend::SessionBackendKind;
    fn current_recipe(session: &crate::session::session::Session, settings: &AppSettings) -> bool {
        session_has_current_pty_submission_provenance(&SessionInfo::from(session), settings)
    }
    rig.sessions
        .prepare_pty_input_boundary(
            rig.target_id,
            &rig.route.target,
            SessionBackendKind::LocalProcess,
            rig.sender_id,
            &rig.route.sender,
            SessionBackendKind::LocalProcess,
            authority_route,
            &rig.permit,
            &rig.idle,
            &rig.settings,
            current_recipe,
            final_external_check,
        )
        .await
}

#[tokio::test]
async fn final_pty_boundary_rejects_identity_mutation_and_stamps_busy() {
    use crate::pty::backend::SessionBackendKind;
    use crate::pty::idle_detector::PtyInputBoundaryFailure;

    let fixture = make_mailbox_fixture();
    let (rig, mut authority_route) = identity_mutation_rig(&fixture).await;
    let (sender_id, pty) = (rig.sender_id, &rig.pty);
    let team_config = fixture
        .sender_cwd
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("_team_dev-team")
        .join("config.json");
    let original_team = std::fs::read(&team_config).expect("read original team configuration");
    let target_config = fixture.target_cwd.join("config.json");
    let original_target =
        std::fs::read(&target_config).expect("read original target configuration");

    std::fs::write(
        &team_config,
        r#"{"agents":["../_agent_tech-lead"],"coordinator":"../_agent_dev-rust"}"#,
    )
    .expect("mutate sender authority");
    let sender_failure = identity_mutation_boundary(&rig, &authority_route, || true).await;
    assert!(matches!(
        &sender_failure,
        Err(PtyInputBoundaryFailure::RouteUnavailable)
    ));
    drop(sender_failure);

    std::fs::write(&team_config, original_team).expect("restore team configuration");
    std::fs::write(&target_config, r#"{"identity":"../../_agent_tech-lead"}"#)
        .expect("mutate target authority");
    let target_failure = identity_mutation_boundary(&rig, &authority_route, || true).await;
    assert!(matches!(
        &target_failure,
        Err(PtyInputBoundaryFailure::RouteUnavailable)
    ));
    drop(target_failure);

    std::fs::write(&target_config, original_target).expect("restore target configuration");

    pty.lock()
        .unwrap()
        .remove_route_if_kind(sender_id, SessionBackendKind::LocalProcess);
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            sender_id,
            SessionBackendKind::LocalProcess,
            Some(rig.sender_cwd_identity.clone()),
            Some(rig.route.sender.replica_identity.clone()),
        )
        .expect("replace sender route");
    let stale_authority_failure = identity_mutation_boundary(&rig, &authority_route, || true).await;
    assert!(matches!(
        &stale_authority_failure,
        Err(PtyInputBoundaryFailure::RouteUnavailable)
    ));
    drop(stale_authority_failure);
    authority_route = PtyManager::authority_route_proof(pty, sender_id)
        .expect("replacement sender authority route proof");

    let final_external_failure = identity_mutation_boundary(&rig, &authority_route, || false).await;
    assert!(matches!(
        final_external_failure,
        Err(PtyInputBoundaryFailure::RouteUnavailable)
    ));

    let first = identity_mutation_boundary(&rig, &authority_route, || true).await;
    let boundary_guard = match first {
        Ok(guard) => guard,
        Err(error) => panic!("restored boundary must succeed: {error:?}"),
    };
    drop(boundary_guard);
    let second = identity_mutation_boundary(&rig, &authority_route, || true).await;
    assert!(matches!(second, Err(PtyInputBoundaryFailure::Busy)));
}

/// #1149 - the inter-agent injection path sets `waiting_for_input = false`
/// and promotes `Idle` to `Running` directly, before `notify_pty_input_busy`
/// reaches `mark_busy`. Hooking only `mark_idle`/`mark_busy` would lose every
/// injection edge, so the boundary emits its own record and the `mark_busy`
/// that follows must then find no edge left.
#[tokio::test]
async fn pty_input_boundary_yields_exactly_one_busy_record() {
    use crate::pty::backend::SessionBackendKind;

    let fixture = make_mailbox_fixture();
    let BoundaryRig {
        sender_id,
        target_id,
        route,
        authority_route,
        permit,
        idle,
        sessions,
        settings,
    } = boundary_rig(&fixture).await;
    fn current_recipe(session: &crate::session::session::Session, settings: &AppSettings) -> bool {
        session_has_current_pty_submission_provenance(&SessionInfo::from(session), settings)
    }

    // Discard the fixture's own setup edges, including the `mark_idle` that
    // put the target into the state the boundary requires.
    let _ = crate::config::activity_log::capture::drain();

    let boundary_guard = sessions
        .prepare_pty_input_boundary(
            target_id,
            &route.target,
            SessionBackendKind::LocalProcess,
            sender_id,
            &route.sender,
            SessionBackendKind::LocalProcess,
            &authority_route,
            &permit,
            &idle,
            &settings,
            current_recipe,
            || true,
        )
        .await
        .expect("the boundary must succeed");
    drop(boundary_guard);

    let records = crate::config::activity_log::capture::drain();
    assert_eq!(records.len(), 1, "one injection is one opening edge");
    let record = serde_json::to_value(&records[0]).expect("the record serializes");
    assert_eq!(record["event"], "busy");
    assert_eq!(record["reason"], "pty_input_boundary");
    assert_eq!(record["sessionId"], target_id.to_string());

    sessions.mark_busy(target_id).await;
    assert!(
        crate::config::activity_log::capture::drain().is_empty(),
        "the mark_busy the idle callback reaches must add nothing"
    );
}
