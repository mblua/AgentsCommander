use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{test::MockRuntime, AppHandle, Listener};
use uuid::Uuid;

use super::{
    add_action, at_recorded_cut, close_coordinator, completes, make_mailbox_fixture, mock_backend,
    pause_at, prepare_additional_retained_member, prepare_retained_exited_respawn,
    provision_retained_fixture_member, rollback_backend, rollback_poller, run_child_cases,
    run_retained_source_child_scenario, wake_message_to_target, Action, CapturedCore, Control, Cut,
    MailboxPoller, OwnedReplacementContinuation, ReplacementFixtureReceipt, WAKE_CORE_CAPTURE,
};
use crate::phone::mailbox::pty_claim::MailboxTestHooks;
use crate::phone::mailbox::pty_observation::PtyAuthorityState;
use crate::phone::types::OutboxMessage;
use crate::pty::manager::{
    PtyManager, ResponseCloseState, ResponseDeliveryProof, ResponseRouteIdentity,
    ReturnDeliveryProof,
};
use crate::session::manager::{acquire_carrier_mutation_permit, SessionManager};
use crate::session::selection::{ReplacementFixtureOutcome, SelectionCoordinator};

#[tokio::test]
async fn wake_owner_queued_and_committed_controls() {
    let test = "wake_owner_queued_and_committed_controls";
    let cases = [
        "queued-drain",
        "panic-commit-before-disarm",
        "recv-after-release",
    ];
    let Some(scenario) = run_child_cases(module_path!(), test, &cases).await else {
        return;
    };
    run_retained_source_child_scenario(false, true, &scenario).await;
}

#[tokio::test]
async fn wake_owner_competing_admission_preserves_nonrollbackable_owner() {
    let test = "wake_owner_competing_admission_preserves_nonrollbackable_owner";
    let cases = ["queued", "partial-commit", "reply-lost"];
    let Some(scenario) = run_child_cases(module_path!(), test, &cases).await else {
        return;
    };
    let fixture = make_mailbox_fixture();
    let member =
        (scenario == "queued").then(|| provision_retained_fixture_member(&fixture, "blocker"));
    let (coordinator, _, sender, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let mut blocker = wake_message_to_target();
    if let Some((replica, target)) = &member {
        prepare_additional_retained_member(&fixture, &coordinator, sender, replica, target).await;
        blocker.to.clone_from(target);
        blocker.id = format!("queue-blocker-{}", Uuid::new_v4());
    }
    let (other, other_control) = rollback_poller(false);
    let (release_blocker, gate) = tokio::sync::oneshot::channel();
    *other_control.before_release_gate.lock().unwrap() = Some(gate);
    let other_wake = other.deliver_wake(app, &blocker);
    tokio::pin!(other_wake);
    if member.is_some() {
        at_recorded_cut(other_wake.as_mut(), &other_control, Cut::AdapterReturned).await;
    }

    let (poller, control) = rollback_poller(scenario != "partial-commit");
    OwnedReplacementContinuation::install_wake_rollback_fixture();
    let cut = match scenario.as_str() {
        "queued" => Cut::TicketSubmitted,
        "partial-commit" => {
            add_action(&control, Action::CommitBeforeDisarmPanicOnce);
            Cut::WorkerUnwound
        }
        "reply-lost" => Cut::AfterReleaseBeforeSend,
        _ => unreachable!(),
    };
    let wake = wake_message_to_target();
    let first = poller.deliver_wake(app, &wake);
    tokio::pin!(first);
    let receipt = if scenario == "reply-lost" {
        observe_detached_reply_loss(app, &coordinator, &poller, &control, &wake).await
    } else {
        at_recorded_cut(first.as_mut(), &control, cut).await
    };
    assert_competing_probe_retains_owner(
        app,
        &coordinator,
        &control,
        &wake,
        previous,
        receipt,
        &scenario,
    )
    .await;
    coordinator.shutdown_token().cancel();
    let _ = release_blocker.send(());
    close_coordinator(&coordinator).await;
}

async fn assert_worker_first_receipts(
    app: &AppHandle<MockRuntime>,
    control: &Control,
    coordinator: &SelectionCoordinator,
    receipt: ReplacementFixtureReceipt,
    owner: &OwnedReplacementContinuation,
) {
    {
        let receipts = control.receipts.lock().unwrap();
        let mut exact = receipts
            .iter()
            .filter(|r| r.operation == receipt.operation && r.binding == receipt.binding);
        assert_eq!(
            exact.clone().filter(|r| r.cut == Cut::ReceiverLost).count(),
            1
        );
        assert!(exact.any(|r| r.cut == Cut::AdmissionReleased
            && r.outcome == Some(ReplacementFixtureOutcome::Finalized)));
        assert!(exact.any(|r| r.cut == Cut::ReceiverLost));
    }
    assert_eq!(
        control.observe_retained(coordinator),
        Some((true, true, false, false, 0))
    );
    let retained = owner.fixture_observe_publication();
    assert_eq!(retained.operation, receipt.operation);
    assert_eq!(Some(retained.binding), receipt.binding);
    assert!(retained.publication && !retained.terminal && !retained.work_transferred);
    assert_eq!(retained.holds, (true, true));
    assert!(!retained.not_started() && retained.no_input_attempt().is_none());
    assert!(retained.work_attempt.is_none() && owner.fixture_wake_source_observation().stopped);
    let id = retained.binding.session_id();
    let manager = app.pty_sessions().read().await.clone();
    assert!(manager.get_session(id).await.is_some());
    assert!(app
        .pty_routes()
        .lock()
        .unwrap()
        .route_identities(id)
        .is_some());
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    assert!(backend.live.lock().unwrap().contains(&id));
    assert_eq!(backend.spawn_attempts(id), 1);
    assert_eq!(backend.write_attempts.load(Ordering::SeqCst), 0);
    assert!(backend.writes_for(id).is_empty());
}

async fn assert_competing_probe_retains_owner(
    app: &AppHandle<MockRuntime>,
    coordinator: &SelectionCoordinator,
    control: &Control,
    wake: &OutboxMessage,
    previous: Uuid,
    receipt: ReplacementFixtureReceipt,
    scenario: &str,
) {
    let probe = OwnedReplacementContinuation::fixture_wake_rollback_state().unwrap();
    let binding = receipt.binding.unwrap();
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    let id = binding.session_id();
    assert_eq!(backend.spawn_attempts(id), 1, "one B launch before probe");
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let live = backend.live.lock().unwrap().clone();
    assert!(live.contains(&binding.session_id()));
    let manager = app.pty_sessions().read().await.clone();
    assert_eq!(
        manager.get_pending_session(binding).await.is_some(),
        scenario == "queued"
    );
    let roots = coordinator.fixture_root_count();
    let mut competing_wake = wake.clone();
    competing_wake.id = format!("competitor-{}", Uuid::new_v4());
    MailboxPoller::default()
        .fixture_probe_once(app, &competing_wake, previous)
        .await;
    let retained = control.observe_owner(coordinator).unwrap();
    assert!(!retained.rollback_completed && !retained.terminal);
    let candidate = *probe.probe_candidate.lock().unwrap();
    assert_eq!(candidate, Some(receipt.operation), "probe reached same A");
    assert_eq!(coordinator.fixture_root_count(), roots);
    assert_eq!(probe.removed.load(Ordering::Relaxed), 0);
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
    assert_eq!(*backend.live.lock().unwrap(), live);
    assert_eq!(backend.spawn_attempts(id), 1, "one B launch after probe");
}

type PublicationEvents = Arc<Mutex<Vec<(&'static str, Option<String>)>>>;

fn record_publication_events(app: &AppHandle<MockRuntime>) -> PublicationEvents {
    let events = PublicationEvents::default();
    for event in ["session_created", "session_destroyed", "session_switched"] {
        let events = events.clone();
        app.listen(event, move |observation| {
            let payload: serde_json::Value = serde_json::from_str(observation.payload()).unwrap();
            let id = payload["id"].as_str().map(str::to_owned);
            events.lock().unwrap().push((event, id));
        });
    }
    events
}

fn captured_owner(capture: &CapturedCore) -> OwnedReplacementContinuation {
    capture.owner.lock().unwrap().as_ref().unwrap().clone()
}

async fn assert_no_input_release(
    app: &AppHandle<MockRuntime>,
    coordinator: &SelectionCoordinator,
    control: &Control,
    owner: &OwnedReplacementContinuation,
    previous: Uuid,
    events: &PublicationEvents,
) {
    let receipt = owner.fixture_observe_publication();
    let id = receipt.binding.session_id();
    assert!(receipt.publication && receipt.terminal);
    assert_eq!(receipt.holds, (false, false));
    assert!(!receipt.work_transferred);
    assert!(
        control.observe_owner(coordinator).is_none(),
        "exact old root removed"
    );
    let manager = app.pty_sessions().read().await.clone();
    assert!(manager.get_session(previous).await.is_none());
    assert!(manager.get_session(id).await.is_some());
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    assert_eq!(backend.spawn_attempts(id), 1);
    assert_eq!(backend.write_attempts.load(Ordering::SeqCst), 0);
    assert!(backend.writes_for(id).is_empty());
    let selection = serde_json::to_value(manager.selection_payload().await).unwrap();
    let selected_id = selection["id"].as_str().map(str::to_owned);
    assert_eq!(
        *events.lock().unwrap(),
        [
            ("session_created", Some(id.to_string())),
            ("session_destroyed", Some(previous.to_string())),
            ("session_switched", selected_id),
        ]
    );
}

async fn reuse_live_replacement(
    app: &AppHandle<MockRuntime>,
    manager: &SessionManager,
    wake: &OutboxMessage,
    id: Uuid,
) {
    manager.mark_idle(id).await;
    let hooks = MailboxTestHooks::default();
    hooks.real_inject_sessions.lock().unwrap().insert(id);
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let mut retry = wake.clone();
    retry.id = format!("publication-retry-{}", Uuid::new_v4());
    completes(poller.deliver_wake(app, &retry)).await.unwrap();
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    assert_eq!(backend.spawn_attempts(id), 1);
    assert!(!backend.writes_for(id).is_empty(), "retry wrote to exact B");
}

async fn observe_detached_reply_loss(
    app: &AppHandle<MockRuntime>,
    coordinator: &SelectionCoordinator,
    poller: &MailboxPoller,
    control: &Control,
    wake: &OutboxMessage,
) -> ReplacementFixtureReceipt {
    let capture = Arc::new(CapturedCore::default());
    let mut waiter =
        Box::pin(WAKE_CORE_CAPTURE.scope(capture.clone(), poller.deliver_wake(app, wake)));
    let publication = OwnedReplacementContinuation::install_wake_publication_fixture();
    *publication.before_disposal.lock().unwrap() =
        Some(Box::new(|| panic!("reply loss must retain reservations")));
    let resume_worker = pause_at(control, Cut::AfterReleaseBeforeSend);
    let (stop_source, signal) = tokio::sync::oneshot::channel();
    *control.source_panic.lock().unwrap() = Some(signal);
    let submitted = at_recorded_cut(waiter.as_mut(), control, Cut::TicketSubmitted).await;
    let published = at_recorded_cut(waiter.as_mut(), control, Cut::AfterReleaseBeforeSend).await;
    assert_eq!(submitted.operation, published.operation);
    assert_eq!(submitted.binding, published.binding);
    super::at_cut(waiter.as_mut(), async {
        while control.source_panic.lock().unwrap().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    drop(waiter);

    let owner = captured_owner(&capture);
    assert!(!stop_source.is_closed());
    assert!(owner.fixture_wake_source_observation().active);
    let active = owner.fixture_observe_publication();
    assert!(active.publication && !active.terminal);
    assert_eq!(active.holds, (true, true));
    assert!(publication.before_disposal.lock().unwrap().is_some());

    stop_source.send(()).unwrap();
    completes(async {
        while !owner.fixture_wake_source_observation().stopped {
            tokio::task::yield_now().await;
        }
        super::reacquire_lexical_guards(app, &wake.to).await;
    })
    .await;
    let stopped = owner.fixture_observe_publication();
    assert!(!stopped.not_started() && stopped.no_input_attempt().is_none());
    assert_eq!(
        control.observe_retained(coordinator),
        Some((true, true, false, false, 0))
    );

    resume_worker.send(()).unwrap();
    completes(coordinator.snapshot()).await.unwrap();
    assert_worker_first_receipts(app, control, coordinator, published, &owner).await;
    assert!(publication.before_disposal.lock().unwrap().is_some());
    published
}

#[tokio::test]
async fn wake_owner_real_finalizer_reply_loss_and_unwind() {
    let test = "wake_owner_real_finalizer_reply_loss_and_unwind";
    let Some(_) = run_child_cases(module_path!(), test, &["source-unwind"]).await else {
        return;
    };
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let (poller, control) = rollback_poller(true);
    let wake = wake_message_to_target();
    observe_detached_reply_loss(app, &coordinator, &poller, &control, &wake).await;
    close_coordinator(&coordinator).await;
}

#[tokio::test]
async fn wake_owner_real_refresh_pre_input_receipts() {
    let test = "wake_owner_real_refresh_pre_input_receipts";
    let cases = [
        "r7",
        "r9",
        "generic-error",
        "wrong-attempt",
        "ambiguous",
        "input-attempt",
        "received",
    ];
    let Some(scenario) = run_child_cases(module_path!(), test, &cases).await else {
        return;
    };
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let (poller, control) = rollback_poller(true);
    let gates = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let publication = OwnedReplacementContinuation::install_wake_publication_fixture();
    let release = gates.before_return.arm();
    let disposals = Arc::new(AtomicUsize::new(0));
    let observed_disposals = disposals.clone();
    *publication.after_disposal.lock().unwrap() = Some(Box::new(move || {
        observed_disposals.fetch_add(1, Ordering::Relaxed);
    }));
    let refresh = match scenario.as_str() {
        "r7" => 1,
        "input-attempt" | "received" => 0,
        _ => 2,
    };
    publication.refuse_refresh.store(refresh, Ordering::Relaxed);
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    backend
        .fail_write
        .store(scenario == "input-attempt", Ordering::SeqCst);
    let events = record_publication_events(app);
    let capture = Arc::new(CapturedCore::default());
    let wake = wake_message_to_target();
    let first = WAKE_CORE_CAPTURE.scope(capture.clone(), poller.deliver_wake(app, &wake));
    tokio::pin!(first);
    super::at_cut(first.as_mut(), gates.before_return.entered.notified()).await;
    let owner = captured_owner(&capture);
    let observation = owner.fixture_observe_publication();
    let id = observation.binding.session_id();
    assert!(observation.publication);
    match scenario.as_str() {
        "received" => {
            assert!(observation.terminal && observation.work_transferred);
            assert!(!backend.writes_for(id).is_empty());
        }
        "input-attempt" => {
            assert!(backend.write_attempts.load(Ordering::SeqCst) > 0);
            assert!(backend.writes_for(id).is_empty());
        }
        "r7" => {
            assert!(control.receipts.lock().unwrap().iter().any(|receipt| {
                receipt.cut == Cut::AdmissionReleased
                    && receipt.outcome == Some(ReplacementFixtureOutcome::Finalized)
                    && receipt.operation == observation.operation
                    && receipt.binding == Some(observation.binding)
            }));
            assert!(observation.not_started() && observation.work_attempt.is_none());
            publication.refuse_refresh.store(0, Ordering::Relaxed);
        }
        scenario => {
            assert!(observation.no_input_attempt().is_some());
            assert_eq!(observation.no_input_attempt(), observation.work_attempt);
            if scenario != "r9" {
                owner.fixture_invalidate_no_input(scenario);
            }
        }
    }
    release.send(()).unwrap();
    let result = completes(first).await;
    if scenario == "received" {
        assert!(result.is_ok());
        assert!(control.observe_owner(&coordinator).is_none());
    } else if matches!(scenario.as_str(), "r7" | "r9") {
        let reason = if scenario == "r7" {
            "work_transfer_unavailable"
        } else {
            "completion_info_lost"
        };
        assert!(result.unwrap_err().contains(reason));
        assert_eq!(disposals.load(Ordering::Relaxed), 1);
        assert_no_input_release(app, &coordinator, &control, &owner, previous, &events).await;
        let manager = app.pty_sessions().read().await.clone();
        reuse_live_replacement(app, &manager, &wake, id).await;
    } else {
        assert!(result.is_err());
        let retained = owner.fixture_observe_publication();
        assert!(!retained.terminal && !retained.work_transferred);
        assert_eq!(retained.holds, (true, true));
        assert!(control.observe_owner(&coordinator).is_some());
    }
    close_coordinator(&coordinator).await;
}

fn hold_pty(pty: Arc<Mutex<PtyManager>>) -> (impl FnOnce() + Send + 'static, impl FnOnce()) {
    use std::sync::mpsc::channel;
    let (start, wait_start) = channel();
    let (locked, wait_locked) = channel();
    let (stop, wait_stop) = channel();
    let contender = std::thread::spawn(move || {
        wait_start.recv_timeout(Duration::from_secs(10)).unwrap();
        let _held = pty.lock().unwrap();
        locked.send(()).unwrap();
        wait_stop.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    let acquire = move || {
        start.send(()).unwrap();
        wait_locked.recv_timeout(Duration::from_secs(10)).unwrap();
    };
    let release = move || {
        stop.send(()).unwrap();
        contender.join().unwrap();
    };
    (acquire, release)
}

async fn arm_policy_close(
    manager: &SessionManager,
    tracker: &ResponseCloseState,
    member: Uuid,
    sender: Uuid,
    wake: &OutboxMessage,
) -> Instant {
    let permit = acquire_carrier_mutation_permit().await;
    let (member, sender) = manager
        .with_current_carrier_snapshot(&permit, |current| {
            let identity = |id, target: &str| {
                let row = current.rows.iter().find(|row| row.id == id).unwrap();
                ResponseRouteIdentity {
                    target: target.into(),
                    session_id: id,
                    token: row.token,
                    generation: tracker.generation(id).unwrap(),
                    backend: row.backend_kind,
                    carrier_version: current.versions[&id],
                }
            };
            (identity(member, &wake.to), identity(sender, &wake.from))
        })
        .await;
    let now = Instant::now();
    super::prime_route_tracker(tracker, &member, &sender, now);
    let key = "publication-close-probe";
    let digest = "publication-close-body";
    let mut work = tracker.reserve_work(&member.target, key).unwrap();
    assert_eq!(work.accept_durable(digest), Ok(true));
    drop(work);
    let context = tracker
        .reserve_policy_candidate(
            member.clone(),
            sender.clone(),
            key.into(),
            digest.into(),
            now,
        )
        .unwrap();
    tracker.mark_request_submitted(&context, true);
    tracker
        .register_request(
            ResponseDeliveryProof {
                context,
                member: member.clone(),
                coordinator: sender.clone(),
                published_carrier_version: member.carrier_version,
            },
            now,
        )
        .unwrap();
    let context = tracker
        .admit_return(
            &member,
            &sender,
            "publication-close-return".into(),
            "return".into(),
            now,
        )
        .unwrap();
    tracker
        .observe_return_success(ReturnDeliveryProof { context }, now)
        .unwrap();
    Instant::now() + Duration::from_secs(60)
}

#[tokio::test]
async fn wake_owner_pre_input_close_and_contention_controls() {
    let test = "wake_owner_pre_input_close_and_contention_controls";
    let cases = [
        "policy",
        "manual-discard",
        "exited",
        "route-absent",
        "closing",
        "new-generation",
        "poisoned",
        "proof-contention",
        "disposal-contention",
        "disposal-failure",
    ];
    let Some(scenario) = run_child_cases(module_path!(), test, &cases).await else {
        return;
    };
    run_close_and_contention_control(&scenario).await;
}

async fn run_close_and_contention_control(scenario: &str) {
    let fixture = make_mailbox_fixture();
    let (coordinator, _, sender, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let (poller, control) = rollback_poller(true);
    let gates = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let publication = OwnedReplacementContinuation::install_wake_publication_fixture();
    publication.refuse_refresh.store(1, Ordering::Relaxed);
    let resume_source = gates.before_return.arm();
    let events = record_publication_events(app);
    let capture = Arc::new(CapturedCore::default());
    let wake = wake_message_to_target();
    let first = WAKE_CORE_CAPTURE.scope(capture.clone(), poller.deliver_wake(app, &wake));
    tokio::pin!(first);
    super::at_cut(first.as_mut(), gates.before_return.entered.notified()).await;
    let owner = captured_owner(&capture);
    let active = owner.fixture_observe_publication();
    let id = active.binding.session_id();
    assert!(active.publication && active.not_started() && !active.terminal);
    assert_eq!(active.holds, (true, true));
    let manager = app.pty_sessions().read().await.clone();
    let pty = app.pty_routes().clone();
    let tracker = pty.lock().unwrap().response_close_state();
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    let close_control = arrange_close_control(
        scenario,
        app,
        &manager,
        id,
        sender,
        &wake,
        (&publication.before_disposal, &publication.after_disposal),
    )
    .await;
    assert_eq!(owner.fixture_observe_publication().holds, (true, true));
    resume_source.send(()).unwrap();
    assert!(completes(first)
        .await
        .unwrap_err()
        .contains("work_transfer_unavailable"));
    assert_eq!(backend.write_attempts.load(Ordering::SeqCst), 0);
    finish_contending_pty(
        scenario,
        &pty,
        &control,
        &coordinator,
        (&publication.before_disposal, &publication.after_disposal),
        close_control.release_pty,
    );
    if matches!(scenario, "policy" | "disposal-contention") {
        assert_no_input_release(app, &coordinator, &control, &owner, previous, &events).await;
        assert_policy_after_disposal(&tracker, id, close_control.eligible_at);
        reuse_live_replacement(app, &manager, &wake, id).await;
    } else {
        assert_refused_close_control(&owner, &control, &coordinator, &tracker, scenario);
        if scenario == "disposal-failure" {
            assert!(publication.before_disposal.lock().unwrap().is_none());
        }
    }
    close_coordinator(&coordinator).await;
}

type DisposalProbe = Mutex<Option<Box<dyn FnOnce() + Send>>>;

#[derive(Default)]
struct CloseControl {
    release_pty: Option<Box<dyn FnOnce()>>,
    eligible_at: Option<Instant>,
}

async fn arrange_close_control(
    scenario: &str,
    app: &AppHandle<MockRuntime>,
    manager: &SessionManager,
    id: Uuid,
    sender: Uuid,
    wake: &OutboxMessage,
    (before_disposal, after_disposal): (&DisposalProbe, &DisposalProbe),
) -> CloseControl {
    let pty = app.pty_routes();
    let mut control = CloseControl::default();
    match scenario {
        "policy" => {
            let tracker = pty.lock().unwrap().response_close_state();
            let eligible = arm_policy_close(manager, &tracker, id, sender, wake).await;
            assert!(
                tracker.select_ticket(id, eligible).is_err(),
                "original holds veto close"
            );
            control.eligible_at = Some(eligible);
        }
        "proof-contention" => {
            let (acquire, release) = hold_pty(pty.clone());
            acquire();
            control.release_pty = Some(Box::new(release));
        }
        "disposal-contention" => {
            let proof_guard = pty.clone();
            *before_disposal.lock().unwrap() =
                Some(Box::new(move || assert_pty_guard_is_held(proof_guard)));
            let (acquire, release) = hold_pty(pty.clone());
            *after_disposal.lock().unwrap() = Some(Box::new(acquire));
            control.release_pty = Some(Box::new(release));
        }
        "disposal-failure" => {
            let tracker = pty.lock().unwrap().response_close_state();
            *before_disposal.lock().unwrap() = Some(Box::new(move || {
                tracker.fail_closed();
            }));
        }
        _ => refuse_current_publication(scenario, app, manager, id).await,
    }
    control
}

fn finish_contending_pty(
    scenario: &str,
    pty: &Mutex<PtyManager>,
    control: &Control,
    coordinator: &SelectionCoordinator,
    (before_disposal, after_disposal): (&DisposalProbe, &DisposalProbe),
    release: Option<Box<dyn FnOnce()>>,
) {
    if scenario == "disposal-contention" {
        assert!(before_disposal.lock().unwrap().is_none());
        assert!(after_disposal.lock().unwrap().is_none());
        assert!(
            pty.try_lock().is_err(),
            "contender holds PTY through exact root removal"
        );
        assert!(control.observe_owner(coordinator).is_none());
    }
    if let Some(release) = release {
        release();
    }
}

fn assert_policy_after_disposal(tracker: &ResponseCloseState, id: Uuid, eligible: Option<Instant>) {
    if let Some(eligible) = eligible {
        assert!(
            tracker.select_ticket(id, eligible).is_ok(),
            "same policy after disposal"
        );
    }
}

fn assert_refused_close_control(
    owner: &OwnedReplacementContinuation,
    control: &Control,
    coordinator: &SelectionCoordinator,
    tracker: &ResponseCloseState,
    scenario: &str,
) {
    let retained = owner.fixture_observe_publication();
    assert!(!retained.terminal && !retained.work_transferred);
    assert!(control.observe_owner(coordinator).is_some());
    if scenario == "disposal-failure" {
        assert_eq!(retained.holds, (false, false));
        assert!(!tracker.is_healthy());
    } else {
        assert_eq!(retained.holds, (true, true));
    }
}

async fn refuse_current_publication(
    scenario: &str,
    app: &AppHandle<MockRuntime>,
    manager: &SessionManager,
    id: Uuid,
) {
    let pty = app.pty_routes();
    let tracker = pty.lock().unwrap().response_close_state();
    let generation = tracker.generation(id).unwrap();
    match scenario {
        "manual-discard" => {
            let backend = rollback_backend(app);
            let backend = mock_backend(&backend);
            let kills = backend.kill_attempts.load(Ordering::SeqCst);
            assert!(pty.lock().unwrap().route_identities(id).is_some());
            completes(crate::commands::session::destroy_session_inner(app, id))
                .await
                .unwrap();
            assert!(pty.lock().unwrap().route_identities(id).is_none());
            assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills + 1);
            assert!(!backend.live.lock().unwrap().contains(&id));
        }
        "exited" => {
            manager.mark_exited(id, 18).await;
            assert!(matches!(
                manager.get_session(id).await.unwrap().status,
                crate::session::session::SessionStatus::Exited(18)
            ));
        }
        "route-absent" => {
            tracker.remove_route(id, generation);
            assert!(tracker.generation(id).is_none());
        }
        "closing" => {
            tracker.fixture_set_closing(id, generation);
            assert_eq!(tracker.is_closing(id, generation), Ok(true));
        }
        "new-generation" => {
            tracker.register_route(id, generation + 1, Instant::now());
            assert_eq!(tracker.generation(id), Some(generation + 1));
        }
        "poisoned" => {
            assert!(pty.lock().unwrap().route_identities(id).is_some());
            tracker.fixture_poison_tracker();
        }
        _ => unreachable!(),
    }
}

fn assert_pty_guard_is_held(pty: Arc<Mutex<PtyManager>>) {
    std::thread::spawn(move || {
        assert!(matches!(
            pty.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
    })
    .join()
    .unwrap();
}
