use super::*;
use crate::pty::manager::{
    ResponseClosePublisher, ResponseCloseState, ResponseDeliveryProof, ResponseRouteIdentity,
    ReturnDeliveryProof,
};

/// (#1399 T6 / G8) A claim cannot be re-taken while its previous outcome is
/// undrained: `claim_and_spawn_wake` refuses, spawns nothing, and leaves
/// the file untouched. Without the guard this returns Spawned and lands a
/// second attempt inside one cycle.
#[tokio::test(flavor = "current_thread")]
async fn claim_and_spawn_refuses_while_the_previous_outcome_is_undrained() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let tracker = crate::pty::manager::ResponseCloseState::default();
    assert!(app.manage(tracker.clone()));
    let hooks = MailboxTestHooks::default();
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let source = write_wake_message(
        &fixture.sender_cwd,
        "undrained-outcome",
        "2026-07-15T00:00:01+00:00",
    );
    let message = wake_message_to_target();
    let admitted = poller
        .admit_response_source(&app, &source, &message, true, "authenticated body")
        .await
        .unwrap();
    assert!(admitted.state.same_state(&tracker));
    assert!(admitted.delivery_context.is_none());
    let retained_key = admitted.key.clone();
    let retained_digest = admitted.digest.clone();
    poller.live_claims.insert(wake_claim_path(&source));

    let handoff = poller.claim_and_spawn_wake(&app, &source, wake_message_to_target());

    assert!(matches!(handoff, WakeHandoff::LaneBusy));
    assert_eq!(poller.wake_workers.len(), 0);
    assert_eq!(poller.wake_lanes.len(), 0);
    assert!(source.exists());
    assert!(!wake_claim_path(&source).exists());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    {
        let sources = poller.response_sources.lock().unwrap();
        assert_eq!(sources[&source].key, retained_key);
        assert_eq!(sources[&source].digest, retained_digest);
        assert!(sources[&source].delivery_context.is_none());
    }

    let capped_hooks = MailboxTestHooks::default();
    let mut capped_poller = MailboxPoller::new_with_test_hooks(capped_hooks.clone());
    let capped_source = write_wake_message(
        &fixture.sender_cwd,
        "outstanding-ledger-cap",
        "2026-07-15T00:00:02+00:00",
    );
    for index in 0..crate::phone::wake_lanes::WAKE_WORKER_LIMIT {
        assert!(capped_poller.live_claims.insert(
            fixture
                .sender_cwd
                .join(format!("dummy-{index}.json.in-flight"))
        ));
    }

    let capped_handoff =
        capped_poller.claim_and_spawn_wake(&app, &capped_source, wake_message_to_target());

    assert!(matches!(capped_handoff, WakeHandoff::LaneBusy));
    assert_eq!(capped_poller.wake_workers.len(), 0);
    assert_eq!(capped_poller.wake_lanes.len(), 0);
    assert!(capped_source.exists());
    assert!(!wake_claim_path(&capped_source).exists());
    assert!(capped_hooks.spawn_calls.lock().unwrap().is_empty());
    // F: current-thread pre-poll cancellation; recovery is composition with C.
    for cancelled in [true, false] {
        run_pre_poll_cancel_case(cancelled).await;
    }
}

struct PrePollCancelCase {
    hooks: MailboxTestHooks,
    now: std::time::Instant,
    coordinator: ResponseRouteIdentity,
    member: ResponseRouteIdentity,
    tracker: ResponseCloseState,
    session_id: Uuid,
    app: tauri::AppHandle<tauri::test::MockRuntime>,
    fixture: MailboxFixture,
}

async fn setup_pre_poll_cancel_case() -> PrePollCancelCase {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "wake-cancel",
        "codex",
        SessionStatus::Idle,
    )
    .await;

    let tracker = ResponseCloseState::default();
    assert!(app.manage(tracker.clone()));
    let (coordinator_id, coordinator_token) =
        seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running).await;
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    let target_row = manager.get_session(session_id).await.unwrap();
    let member = ResponseRouteIdentity {
        target: CANONICAL_WAKE_TO.into(),
        session_id,
        token: target_row.token,
        generation: 1,
        carrier_version: 1,
        backend: crate::pty::backend::SessionBackendKind::LocalProcess,
    };
    let coordinator = ResponseRouteIdentity {
        target: CANONICAL_WAKE_FROM.into(),
        session_id: coordinator_id,
        token: coordinator_token,
        generation: 2,
        ..member.clone()
    };

    let now = std::time::Instant::now() + Duration::from_secs(3600);
    prime_pre_poll_cancel_tracker(&tracker, &member, &coordinator, now);

    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(session_id, true);
    hooks
        .inject_results
        .lock()
        .unwrap()
        .push_back(Err("transient non-cancel twin".into()));
    app.state::<SettingsState>()
        .write()
        .await
        .project_paths
        .push(fixture.sender_cwd.to_string_lossy().to_string());
    PrePollCancelCase {
        hooks,
        now,
        coordinator,
        member,
        tracker,
        session_id,
        app,
        fixture,
    }
}

fn prime_pre_poll_cancel_tracker(
    tracker: &ResponseCloseState,
    member: &ResponseRouteIdentity,
    coordinator: &ResponseRouteIdentity,
    now: std::time::Instant,
) {
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

async fn run_pre_poll_cancel_case(cancelled: bool) {
    let case = setup_pre_poll_cancel_case().await;
    let PrePollCancelCase {
        hooks,
        now,
        coordinator,
        member,
        tracker,
        session_id,
        app,
        fixture,
    } = &case;
    let (now, session_id) = (*now, *session_id);
    let source = write_wake_message(
        &fixture.sender_cwd,
        "pre-poll-cancel",
        "2026-07-15T00:00:04+00:00",
    );
    let content = std::fs::read_to_string(&source).unwrap();
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let mut slot = None;
    poller
        .process_message_content(
            app,
            &source,
            OutboxOrigin::Replica,
            &content,
            Some(&mut slot),
        )
        .await
        .unwrap();
    let validated = slot.unwrap();
    let original = validated.response.as_ref().unwrap().clone();
    assert_eq!(original.target, member.target);
    assert!(tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            original.key.clone(),
            original.digest.clone(),
            now
        )
        .is_ok());
    assert!(matches!(
        poller.claim_and_spawn_validated_wake(app, &source, validated),
        WakeHandoff::Spawned
    ));
    if cancelled {
        poller.wake_workers.abort_all();
    }
    // First yield after abort; report construction lives inside the unpolled future.
    poller.settle_wake_workers().await;
    assert_pre_poll_settled_claim(cancelled, &poller, &source, hooks, session_id);
    let (_retained, preservation_snapshot) =
        assert_pre_poll_phase1_retention(&case, &mut poller, &source, &original, &content);

    let scenario_target = original.target.clone();
    let scenario_key = original.key.clone();

    // Phase 2: synthetic TEST-LOCAL discharge ONLY. This does not prove FS/SQL
    // terminalization, peer submission, or P05 proof. Phase 1 alone proves retention.
    assert_eq!(member.target, scenario_target);
    assert!(!tracker.target_is_closing(&scenario_target));
    tracker.settle_terminal(&scenario_target, &scenario_key, std::time::Instant::now());
    // A leaked dispatch/provisional counter is not changed by settle_terminal.
    // Each fresh probe uses the SAME tracker, target, generation and facts.
    for sensitivity in 0..5 {
        probe_pre_poll_sensitivity(&case, &scenario_target, sensitivity);
    }
    assert_pre_poll_retained_after(&poller, tracker, &source, preservation_snapshot);
}

fn assert_pre_poll_settled_claim(
    cancelled: bool,
    poller: &MailboxPoller,
    source: &Path,
    hooks: &MailboxTestHooks,
    session_id: Uuid,
) {
    if cancelled {
        assert!(!source.exists());
        assert!(wake_claim_path(source).is_file());
        assert!(poller.live_claims.contains(&wake_claim_path(source)));
        assert!(!poller.retry_tracker.contains_key(source));
        assert!(hooks.inject_calls.lock().unwrap().is_empty());
    } else {
        assert!(source.is_file());
        assert!(!wake_claim_path(source).exists());
        assert!(poller.live_claims.is_empty());
        assert_eq!(
            poller
                .retry_tracker
                .get(source)
                .map(|state| state.attempt_count),
            Some(1)
        );
        assert_eq!(hooks.inject_calls.lock().unwrap().as_slice(), &[session_id]);
    }
}

type PreservationSnapshot = (String, String, String, Vec<u8>);

fn assert_pre_poll_phase1_retention(
    case: &PrePollCancelCase,
    poller: &mut MailboxPoller,
    source: &Path,
    original: &ResponseWakeWork,
    content: &str,
) -> (ResponseWakeWork, PreservationSnapshot) {
    let PrePollCancelCase {
        hooks,
        now,
        coordinator,
        member,
        tracker,
        ..
    } = case;
    let now = *now;
    // Phase 1: preservation before synthetic discharge. Exact pending proves
    // that the scenario target has a work entry; no counters are inferred here.
    let retained = poller
        .response_sources
        .lock()
        .unwrap()
        .get(source)
        .unwrap()
        .clone();
    assert!(retained.state.same_state(tracker));
    assert_eq!(
        (&retained.target, &retained.key, &retained.digest),
        (&original.target, &original.key, &original.digest)
    );
    assert_eq!(member.target, retained.target);
    assert!(retained.delivery_context.is_none());
    assert!(retained.return_context.is_none());
    assert!(!tracker.target_is_closing(&member.target));
    assert_eq!(
        tracker.generation(member.session_id),
        Some(member.generation)
    );
    assert!(tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            original.key.clone(),
            original.digest.clone(),
            now
        )
        .is_ok());
    let snapshot_path = if source.exists() {
        source.to_path_buf()
    } else {
        wake_claim_path(source)
    };
    let preservation_snapshot = (
        retained.target.clone(),
        retained.key.clone(),
        retained.digest.clone(),
        std::fs::read(snapshot_path).unwrap(),
    );
    assert_eq!(preservation_snapshot.3, content.as_bytes());
    assert_eq!(poller.wake_workers.len(), 0);
    assert_eq!(poller.wake_lanes.len(), 0);
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    assert!(matches!(
        poller.wake_outcomes_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    (retained, preservation_snapshot)
}

fn probe_pre_poll_sensitivity(case: &PrePollCancelCase, scenario_target: &str, sensitivity: u64) {
    let PrePollCancelCase {
        coordinator,
        member,
        tracker,
        ..
    } = case;
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

fn assert_pre_poll_retained_after(
    poller: &MailboxPoller,
    tracker: &ResponseCloseState,
    source: &Path,
    preservation_snapshot: PreservationSnapshot,
) {
    // Synthetic tracker discharge leaves phase1 association/disk intact.
    let retained_after = poller
        .response_sources
        .lock()
        .unwrap()
        .get(source)
        .unwrap()
        .clone();
    assert!(retained_after.state.same_state(tracker));
    assert_eq!(
        (
            retained_after.target,
            retained_after.key,
            retained_after.digest
        ),
        (
            preservation_snapshot.0,
            preservation_snapshot.1,
            preservation_snapshot.2
        )
    );
    assert!(retained_after.delivery_context.is_none());
    assert!(retained_after.return_context.is_none());
    let path_after = if source.exists() {
        source.to_path_buf()
    } else {
        wake_claim_path(source)
    };
    assert_eq!(std::fs::read(path_after).unwrap(), preservation_snapshot.3);
}
