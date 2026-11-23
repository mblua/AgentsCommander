use std::sync::atomic::Ordering;

use uuid::Uuid;

use super::{
    add_action, at_recorded_cut, close_coordinator, completes, make_mailbox_fixture, mock_backend,
    pause_at, prepare_additional_retained_member, prepare_retained_exited_respawn,
    provision_retained_fixture_member, rollback_backend, rollback_poller, run_child_cases,
    run_retained_source_child_scenario, wake_message_to_target, Action, Cut, MailboxPoller,
    OwnedReplacementContinuation,
};
use crate::phone::mailbox::pty_observation::PtyAuthorityState;
use crate::session::selection::ReplacementFixtureOutcome;

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

    let (poller, control) = rollback_poller(scenario == "queued");
    let mut reply = None;
    let cut = match scenario.as_str() {
        "queued" => Cut::TicketSubmitted,
        "partial-commit" => {
            add_action(&control, Action::CommitBeforeDisarmPanicOnce);
            Cut::WorkerUnwound
        }
        "reply-lost" => {
            let cut = Cut::AfterReleaseBeforeSend;
            let (drop_reply, signal) = tokio::sync::oneshot::channel();
            *control.live_receiver_drop.lock().unwrap() = Some(signal);
            reply = Some((drop_reply, pause_at(&control, cut)));
            cut
        }
        _ => unreachable!(),
    };
    let wake = wake_message_to_target();
    let first = poller.deliver_wake(app, &wake);
    tokio::pin!(first);
    let receipt = at_recorded_cut(first.as_mut(), &control, cut).await;
    let binding = receipt.binding.unwrap();
    if let Some((drop_reply, release_reply)) = reply {
        drop_reply.send(()).unwrap();
        at_recorded_cut(first.as_mut(), &control, Cut::ReceiverLost).await;
        release_reply.send(()).unwrap();
        assert!(first.await.unwrap_err().contains("fixture_retained"));
        completes(coordinator.snapshot()).await.unwrap();
        let receipts = control.receipts.lock().unwrap();
        let exact = receipts
            .iter()
            .filter(|r| r.operation == receipt.operation && r.binding == receipt.binding);
        let losses = exact.clone().filter(|r| r.cut == Cut::ReceiverLost);
        assert_eq!(losses.count(), 2, "receiver loss and worker send failure");
        assert!(exact.clone().any(|r| r.cut == Cut::AdmissionReleased
            && r.outcome == Some(ReplacementFixtureOutcome::Finalized)));
        assert!(!exact.clone().any(|r| r.cut == Cut::SourceReceived));
        drop(receipts);
        assert_eq!(
            control.observe_retained(&coordinator),
            Some((true, true, false, false, 0))
        );
        let owner = control.observe_owner(&coordinator).unwrap();
        assert_eq!(owner.source_returned_failed, Some(false));
    }
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
    if cut == Cut::AfterReleaseBeforeSend {
        assert!(manager.get_session(id).await.is_some());
        let pty = app.pty_routes().lock().unwrap();
        assert!(pty.route_identities(id).is_some());
    }
    let roots = coordinator.fixture_root_count();
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let mut competing_wake = wake.clone();
    competing_wake.id = format!("competitor-{}", Uuid::new_v4());
    MailboxPoller::default()
        .fixture_probe_once(app, &competing_wake, previous)
        .await;
    let retained = control.observe_owner(&coordinator).unwrap();
    assert!(!retained.rollback_completed && !retained.terminal);
    let candidate = *probe.probe_candidate.lock().unwrap();
    assert_eq!(candidate, Some(receipt.operation), "probe reached same A");
    assert_eq!(coordinator.fixture_root_count(), roots);
    assert_eq!(probe.removed.load(Ordering::Relaxed), 0);
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
    assert_eq!(*backend.live.lock().unwrap(), live);
    assert_eq!(backend.spawn_attempts(id), 1, "one B launch after probe");
    coordinator.shutdown_token().cancel();
    let _ = release_blocker.send(());
    close_coordinator(&coordinator).await;
}
