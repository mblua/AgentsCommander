use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{future::Future, path::Path, pin::Pin};
use tokio::sync::{oneshot, Notify};

use super::{
    add_action, assert_preparing, assert_replacement_runs, assert_wake_keeps_old_claim, at_cut,
    at_recorded_cut, close_coordinator, make_mailbox_fixture, mock_backend, observe_wake, pause_at,
    prepare_retained_exited_respawn, replace_cwd, retained_source_wait_cut, rollback_backend,
    rollback_poller, run_child_cases, wake_message_to_target, Action, Control, Cut, MailboxFixture,
    MailboxPoller, Mutex, OutboxMessage, OwnedReplacementContinuation, SelectionCoordinator,
    SessionBackendKind, Uuid,
};
use crate::path_identity::verify_directory;
use crate::phone::mailbox::pty_observation::{PtyAuthorityState, WakeDeliveryState};
use crate::session::manager::{
    acquire_carrier_mutation_permit, PendingCreateBinding, SessionManager,
};
use crate::session::selection::OwnedSourceStop;

#[tokio::test]
async fn wake_owner_pending_source_refusal_requires_typed_receipt() {
    let test = "wake_owner_pending_source_refusal_requires_typed_receipt";
    let changed_cases = ["producer-cwd", "setup-cwd", "setup-replica"];
    let cases = [["typed", "generic", "guard-window"], changed_cases].concat();
    let Some(scenario) = run_child_cases(module_path!(), test, &cases).await else {
        return;
    };
    let generic = scenario == "generic";
    let setup_error = scenario.starts_with("setup-");
    let changed = changed_cases.contains(&scenario.as_str());
    let mut fixture = make_mailbox_fixture();
    let replica = fixture.target_cwd.clone();
    let app = fixture.app.handle();
    if changed {
        fixture.target_cwd = replica.join("nested");
        std::fs::create_dir(&fixture.target_cwd).unwrap();
    }
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    if changed {
        app.pty_settings().write().await.project_paths.clear();
    }
    let manager = app.pty_sessions().read().await.clone();
    let pty = app.pty_routes();
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let transient = &probe.refuse_pending_source;
    transient.store(!changed, Ordering::Relaxed);
    probe.generic_retained.store(generic, Ordering::Relaxed);
    // The original caller returns before any physical cleanup; the retry must do it.
    let (poller, control) = rollback_poller(true);
    let wake = wake_message_to_target();
    let first = observe_wake(app, &poller, &wake);
    tokio::pin!(first);
    let expected_token = observe_pending_source_cut(
        first.as_mut(),
        (&control, &coordinator, &manager),
        (&fixture, &wake, previous),
        (&scenario, &replica, changed, setup_error),
        || (probe.before_return.arm(), &probe.before_return.entered),
        (&probe.probe_candidate, &probe.removed, &probe.before_claim),
    )
    .await;
    let (result, owner) = first.await;
    let error = result.unwrap_err();
    let reasons = ["create_retained", "fixture pending setup failure"];
    assert!(error.contains(reasons[usize::from(setup_error)]));
    assert!(!transient.load(Ordering::Relaxed));
    assert_eq!(probe.before_claim.load(Ordering::Relaxed), 0);
    let binding = owner.binding().unwrap();
    let pending = manager.get_pending_session(binding).await;
    let token = pending.expect("producer kept the exact pending row").token;
    assert!(manager.get_session(previous).await.is_some());
    assert_preparing(&fixture, &wake.to, Some(owner.operation())).await;
    let observation = owner.fixture_wake_rollback();
    let expected_refusal = (!generic && !setup_error).then_some(token);
    assert_eq!(observation.refused_token, expected_refusal);
    if changed {
        assert_eq!(Some(token), expected_token);
        assert_eq!(observation.cursor, None);
        assert_eq!(backend.spawn_attempts(binding.session_id()), 0);
        let receipts = control.receipts.lock().unwrap();
        assert!(!receipts.iter().any(|r| r.cut == Cut::TicketSubmitted));
        drop(receipts);
        let source = control.observe_owner(&coordinator).unwrap();
        assert_eq!(source.source_returned_failed, Some(setup_error));
    }
    assert!(observation.recorded_error.is_none() && !observation.backend_submitted);
    assert!(!control.submitted.load(Ordering::Acquire));
    assert!(!pty.lock().unwrap().has_session(binding.session_id()));
    if generic || changed {
        assert_wake_keeps_old_claim(app, &wake).await;
        assert!(manager.get_pending_session(binding).await.is_some());
        assert_preparing(&fixture, &wake.to, Some(owner.operation())).await;
        assert_eq!(probe.removed.load(Ordering::Relaxed), 0);
        assert_eq!(coordinator.fixture_root_count(), Some(1));
    } else {
        let (poller, _) = rollback_poller(false);
        let new = assert_replacement_runs(app, &poller, &wake).await;
        assert_ne!(new.binding(), Some(binding));
        assert!(manager.get_pending_session(binding).await.is_none());
        assert_eq!(probe.removed.load(Ordering::Relaxed), 1);
        let observation = owner.fixture_wake_rollback();
        assert!(observation.terminal && observation.recorded_error.is_none());
        assert!(!observation.backend_submitted && observation.finalized_binding.is_none());
        assert_eq!(coordinator.fixture_root_count(), Some(0));
    }
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
    close_coordinator(&coordinator).await;
}

#[tokio::test]
async fn wake_owner_same_claim_recovery() {
    let test = "wake_owner_same_claim_recovery";
    let cases = [
        ("before-close", Some("claimed"), 0, 0),
        ("after-removal", Some("row-absent"), 1, 1),
        ("ambiguous-close", Some("close-submitted"), 0, 1),
        ("unrecorded-error", None, 0, 0),
        ("changed-generation", Some("claimed"), 0, 0),
        ("changed-row", Some("claimed"), 0, 0),
        ("changed-binding", Some("claimed"), 0, 0),
        ("changed-preparing", Some("row-absent"), 1, 1),
        ("changed-cwd", Some("claimed"), 0, 0),
        ("tracker-failed", Some("runtime-absent"), 0, 1),
    ];
    let names = cases.map(|(name, ..)| name);
    let Some(scenario) = run_child_cases(module_path!(), test, &names).await else {
        return;
    };
    let index = names.iter().position(|&name| name == scenario).unwrap();
    let (_, cursor, removed, killed) = cases[index];
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let manager = app.pty_sessions().read().await.clone();
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let fail_tracker = &probe.fail_tracker_before_removal;
    match cursor {
        Some("claimed") => probe.before_close.store(8, Ordering::Relaxed),
        Some("row-absent") => probe.before_release.store(8, Ordering::Relaxed),
        Some("close-submitted") => backend.fail_kill_once.store(true, Ordering::SeqCst),
        Some("runtime-absent") => fail_tracker.store(true, Ordering::Relaxed),
        None => {
            probe.refuse_precreate_source.store(true, Ordering::Relaxed);
            probe.before_claim.store(2, Ordering::Relaxed);
        }
        _ => unreachable!(),
    }
    let (poller, control) = rollback_poller(false);
    if scenario != "unrecorded-error" {
        add_action(&control, Action::SimulatedPrecommitFailure);
    }
    let wake = wake_message_to_target();
    let (result, owner) = observe_wake(app, &poller, &wake).await;
    let error = result.unwrap_err();
    if scenario == "unrecorded-error" {
        assert!(error.contains("stage=source_invalid rollback=refused"));
        assert_eq!(probe.before_claim.load(Ordering::Relaxed), 0);
    } else {
        assert!(error.contains("simulated fixture precommit setup failure"));
    }
    let before = owner.fixture_wake_rollback();
    let binding = owner.binding().unwrap();
    assert_eq!(before.cursor, cursor);
    assert_eq!(before.recorded_error.is_some(), cursor.is_some());
    let row_absent = manager.get_pending_session(binding).await.is_none();
    assert_eq!(row_absent, removed > 0 || scenario == "unrecorded-error");
    assert_eq!(probe.removed.load(Ordering::Relaxed), removed);
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills + killed);
    assert_preparing(&fixture, &wake.to, Some(owner.operation())).await;
    reset_rollback_counters([
        &probe.before_close,
        &probe.before_release,
        &probe.before_claim,
    ]);
    let pause_after_probe = || (probe.after_probe.arm(), &probe.after_probe.entered);
    perturb_rollback(
        &scenario,
        (&owner, binding),
        (&manager, &fixture, &wake, previous),
        (&probe.before_probe_permit, pause_after_probe),
        (&backend.kill_attempts, kills),
        fail_tracker,
    )
    .await;
    if matches!(scenario.as_str(), "before-close" | "after-removal") {
        let (poller, _) = rollback_poller(false);
        let new = assert_replacement_runs(app, &poller, &wake).await;
        let after = owner.fixture_wake_rollback();
        assert_eq!((after.nonce, after.attempt), (before.nonce, before.attempt));
        assert!(after.terminal);
        assert_ne!(new.binding(), Some(binding));
        assert!(manager.get_pending_session(binding).await.is_none());
        assert_eq!(probe.removed.load(Ordering::Relaxed), 1);
        assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills + 1);
        assert_eq!(coordinator.fixture_root_count(), Some(0));
    } else {
        assert_wake_keeps_old_claim(app, &wake).await;
        assert_eq!(owner.fixture_wake_rollback(), before);
        assert_eq!(probe.removed.load(Ordering::Relaxed), removed);
        assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills + killed);
        assert_eq!(coordinator.fixture_root_count(), Some(1));
    }
    close_coordinator(&coordinator).await;
}

async fn observe_pending_source_cut<'a, F: Future>(
    mut actor: Pin<&mut F>,
    (control, coordinator, manager): (&Control, &SelectionCoordinator, &SessionManager),
    (fixture, wake, previous): (&MailboxFixture, &OutboxMessage, Uuid),
    (scenario, replica, changed, setup_error): (&str, &Path, bool, bool),
    pause_before_return: impl FnOnce() -> (oneshot::Sender<()>, &'a Notify),
    (probe_candidate, removed, before_claim): (&Mutex<Option<Uuid>>, &AtomicUsize, &AtomicUsize),
) -> Option<Uuid> {
    let mut expected_token = None;
    if changed {
        let validation = Cut::BeforeSourceValidation;
        let release = pause_at(control, validation);
        let cut = at_recorded_cut(actor.as_mut(), control, validation).await;
        let binding = cut.binding.unwrap();
        let pending = manager.get_pending_session(binding).await.unwrap();
        assert_eq!(Path::new(&pending.working_directory), fixture.target_cwd);
        expected_token = Some(pending.token);
        if scenario == "setup-replica" {
            let object = |path: &Path| verify_directory(path).unwrap().object_id;
            let cwd = &fixture.target_cwd;
            let original = object(cwd);
            let old = replace_cwd(replica);
            std::fs::rename(old.join("nested"), cwd).unwrap();
            assert_eq!(object(cwd), original);
            assert_ne!(object(replica), object(&old));
        } else {
            replace_cwd(&fixture.target_cwd);
        }
        let (handoff, entered) = pause_before_return();
        if setup_error {
            drop(release);
        } else {
            release.send(()).unwrap();
        }
        at_cut(actor.as_mut(), entered.notified()).await;
        let state = control.observe_owner(coordinator).unwrap();
        assert!(state.source_active && !state.rollback_completed && !state.cleanup_claimed);
        let stop = (!setup_error).then_some(OwnedSourceStop::AuthorityChanged);
        assert_eq!(state.source_stop, stop);
        let kept = manager.get_pending_session(binding).await;
        assert_eq!(
            kept.map(|row| row.token),
            expected_token,
            "exact B before source handoff"
        );
        assert_preparing(fixture, &wake.to, Some(cut.operation)).await;
        assert_eq!(coordinator.fixture_root_count(), Some(1));
        handoff.send(()).unwrap();
    } else if scenario == "guard-window" {
        let app = fixture.app.handle();
        let (release, entered) = pause_before_return();
        at_cut(actor.as_mut(), entered.notified()).await;
        let gate = app.message_store().unwrap().target_gate.as_ref().unwrap();
        assert!(gate.try_target_lock(&wake.to).unwrap().is_none());
        let mut exact = Box::pin(gate.acquire_exact(&wake.to));
        assert!(futures::poll!(exact.as_mut()).is_pending());
        drop(exact);
        let inserted = retained_source_wait_cut(control, Cut::Inserted).await;
        let challenger = MailboxPoller::default();
        challenger.fixture_probe_once(app, wake, previous).await;
        assert_eq!(*probe_candidate.lock().unwrap(), Some(inserted.operation));
        let binding = inserted.binding.unwrap();
        let pending = manager.get_pending_session(binding).await;
        assert!(pending.is_some(), "pending B kept while both guards held");
        assert_eq!(removed.load(Ordering::Relaxed), 0);
        assert_eq!(coordinator.fixture_root_count(), Some(1));
        before_claim.store(1, Ordering::Relaxed);
        release.send(()).unwrap();
    } else {
        before_claim.store(1, Ordering::Relaxed);
    }
    expected_token
}

fn reset_rollback_counters(counters: [&AtomicUsize; 3]) {
    for counter in counters {
        counter.store(0, Ordering::Relaxed);
    }
}

async fn perturb_rollback<'a>(
    scenario: &str,
    (owner, binding): (&OwnedReplacementContinuation, PendingCreateBinding),
    (manager, fixture, wake, previous): (&SessionManager, &MailboxFixture, &OutboxMessage, Uuid),
    (before_permit, pause): (&Notify, impl FnOnce() -> (oneshot::Sender<()>, &'a Notify)),
    (kill_attempts, kills): (&AtomicUsize, usize),
    fail_tracker: &AtomicBool,
) {
    match scenario {
        "changed-row" => manager.fixture_change_rollback_row(binding, false).await,
        "changed-binding" => manager.fixture_change_rollback_row(binding, true).await,
        "changed-preparing" => owner.fixture_replace_rollback_preparing(manager).await,
        "changed-generation" => {
            let pty = fixture.app.handle().pty_routes().lock().unwrap();
            let id = binding.session_id();
            let (cwd, replica, generation) = pty.route_identities(id).unwrap();
            pty.remove_route_if_kind(id, SessionBackendKind::LocalProcess);
            pty.record_route_with_identities(id, SessionBackendKind::LocalProcess, cwd, replica)
                .unwrap();
            assert_ne!(pty.route_identities(id).unwrap().2, generation);
        }
        "changed-cwd" => {
            let permit = acquire_carrier_mutation_permit().await;
            let challenger = MailboxPoller::default();
            let (hold_probe, entered) = pause();
            let second = challenger.fixture_probe_owned_wake(fixture.app.handle(), wake, previous);
            tokio::pin!(second);
            at_cut(second.as_mut(), before_permit.notified()).await;
            replace_cwd(&fixture.target_cwd);
            drop(permit);
            at_cut(second.as_mut(), entered.notified()).await;
            assert!(manager.get_pending_session(binding).await.is_some());
            assert_eq!(kill_attempts.load(Ordering::SeqCst), kills);
            drop(hold_probe);
        }
        "tracker-failed" => assert!(!fail_tracker.load(Ordering::Relaxed)),
        _ => (),
    }
}
