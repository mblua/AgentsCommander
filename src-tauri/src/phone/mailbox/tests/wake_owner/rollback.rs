use futures::future::BoxFuture;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::{future::Future, path::Path, pin::Pin};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::Instant;

use super::{
    add_action, assert_deadline, assert_preparing, assert_replacement_runs,
    assert_wake_keeps_old_claim, at_cut, at_recorded_cut, close_coordinator, completes,
    deadline_events, make_mailbox_fixture, mock_backend, observe_wake, pause_at,
    prepare_retained_exited_respawn, replace_cwd, retained_source_wait_cut, rollback_backend,
    rollback_poller, run_child_cases, wake_message_to_target, Action, CapturedCore, Control, Cut,
    MailboxFixture, MailboxMockPtyBackend, MailboxPoller, Mutex, OutboxMessage,
    OwnedReplacementContinuation, SelectionCoordinator, SessionBackendKind, Uuid, WAIT,
    WAKE_CORE_CAPTURE,
};
use crate::path_identity::verify_directory;
use crate::phone::mailbox::handoff::wake_claim_path;
use crate::phone::mailbox::host::{MailboxHost, WakeCreateProbe};
use crate::phone::mailbox::poller::{deliver_claimed_wake, wake_deadline, ValidatedWake};
use crate::phone::mailbox::pty_claim::MailboxTestHooks;
use crate::phone::mailbox::pty_observation::{PtyAuthorityState, WakeDeliveryState};
use crate::phone::types::outbox_delivered_receipt;
use crate::pty::manager::PtyManager;
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

pub(super) type WakeCreateEvent = (
    &'static str,
    Option<Uuid>,
    Option<(Instant, Instant)>,
    oneshot::Sender<bool>,
);

pub(crate) struct WakeCreateEvents(mpsc::UnboundedReceiver<WakeCreateEvent>);

impl WakeCreateEvents {
    pub(super) async fn recv(&mut self) -> Option<WakeCreateEvent> {
        self.0.recv().await
    }

    pub(super) async fn at(
        &mut self,
        stage: &str,
        wait: Duration,
    ) -> (Option<Uuid>, oneshot::Sender<bool>) {
        let (actual, binding, _, resume) = tokio::time::timeout(wait, self.recv())
            .await
            .expect("wake fixture deadline")
            .unwrap();
        assert_eq!(actual, stage);
        (binding, resume)
    }
}

struct WakeDeadlineWarnings(Mutex<Vec<String>>);

static WAKE_DEADLINE_WARNINGS: WakeDeadlineWarnings = WakeDeadlineWarnings(Mutex::new(Vec::new()));

impl log::Log for WakeDeadlineWarnings {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() == log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let message = record.args().to_string();
            if message.contains("#3263 wake ended without input")
                && message.contains("stage=deadline")
            {
                self.0.lock().unwrap().push(message);
            }
        }
    }

    fn flush(&self) {}
}

impl WakeCreateProbe {
    pub(crate) fn pause_at(
        budget: Duration,
        cuts: &[(&'static str, usize)],
    ) -> (Self, WakeCreateEvents) {
        log::set_logger(&WAKE_DEADLINE_WARNINGS).unwrap();
        log::set_max_level(log::LevelFilter::Warn);
        let (events, receiver) = mpsc::unbounded_channel();
        let hold_input = cuts.iter().any(|&(stage, _)| stage == "input");
        let cuts = Mutex::new(VecDeque::from(cuts.to_vec()));
        let observe = Arc::new(move |stage, binding| -> BoxFuture<'static, bool> {
            let events = events.clone();
            let mut cuts = cuts.lock().unwrap();
            let pause = cuts
                .front_mut()
                .filter(|cut| cut.0 == stage)
                .is_some_and(|cut| {
                    cut.1 -= 1;
                    cut.1 == 0
                });
            if pause {
                cuts.pop_front();
            }
            let timing = (pause && stage == "standard_response_prepared").then(|| {
                let deadline = wake_deadline().expect("prepared response has a wake deadline");
                let observed_at = Instant::now();
                assert!(
                    observed_at < deadline,
                    "prepared response reached within the deadline"
                );
                (observed_at, deadline)
            });
            Box::pin(async move {
                if !pause {
                    return false;
                }
                let (resume, wait) = oneshot::channel();
                events.send((stage, binding, timing, resume)).unwrap();
                wait.await.unwrap_or(false)
            })
        });
        let prepared = Arc::new(Mutex::new(None));
        (
            Self {
                budget,
                observe,
                prepared,
                hold_input: Arc::new(AtomicBool::new(hold_input)),
            },
            WakeCreateEvents(receiver),
        )
    }

    pub(in crate::phone::mailbox) async fn observe_input_dispatch<F: Future>(
        app: &impl MailboxHost,
        session_id: Uuid,
        injection: F,
    ) -> F::Output {
        let Some(probe) = app
            .wake_create_probe()
            .filter(|probe| probe.hold_input.swap(false, Ordering::Relaxed))
        else {
            return injection.await;
        };
        // Poll the canonical injector with its actual per-session writer occupied.
        let writer = PtyManager::acquire_input_writer(app.pty_routes(), session_id)
            .await
            .unwrap();
        let mut injection = Box::pin(injection);
        assert!(
            futures::poll!(injection.as_mut()).is_pending(),
            "peer injection must wait for the occupied PTY writer"
        );
        (probe.observe)("input", Some(session_id)).await;
        drop(writer);
        injection.await
    }

    pub(super) async fn assert_input_pending(app: &impl MailboxHost, id: Uuid) {
        let mut writer = std::pin::pin!(PtyManager::acquire_input_writer(app.pty_routes(), id));
        assert!(
            futures::poll!(writer.as_mut()).is_pending(),
            "the real PTY writer must be occupied at the input cut"
        );
    }

    pub(super) fn assert_native_pending(
        app: &impl MailboxHost,
        owner: &Mutex<Option<OwnedReplacementContinuation>>,
        id: Uuid,
    ) {
        let owner = owner.lock().unwrap();
        let owner = owner.as_ref().unwrap();
        let state = owner.fixture_observe_publication();
        assert_eq!(state.binding.session_id(), id);
        assert!(owner.fixture_wake_source_observation().active);
        assert!(owner.fixture_wake_rollback().backend_submitted);
        assert!(!state.terminal && !state.publication);
        assert_eq!(app.selection_opt().unwrap().fixture_root_count(), Some(1));
        let pty = app.pty_routes().lock().unwrap();
        assert!(
            pty.backend_for_kind(SessionBackendKind::LocalProcess)
                .has_session(id),
            "native spawn must already have started"
        );
        assert!(pty.backend_kind(id).is_none(), "spawn result still pending");
    }

    pub(crate) async fn assert_bound_conflict(
        &self,
        app: &impl MailboxHost,
        pending_before: &HashSet<Uuid>,
    ) {
        let binding = self.prepared.lock().unwrap().unwrap();
        let manager = app.pty_sessions().read().await.clone();
        assert!(pending_before.contains(&binding.session_id()));
        assert!(manager.get_pending_session(binding).await.is_some());
        let capacity = app.selection_opt().unwrap().port_test_state();
        assert_eq!(capacity.queue_capacity, capacity.expected_queue_capacity);
        assert_eq!(
            capacity.admission_available,
            capacity.expected_admission_capacity
        );
    }

    pub(crate) fn assert_deadline_warning(reason: &str, diagnostic: &str) {
        assert!(reason.contains(diagnostic), "{reason}");
        let warnings = std::mem::take(&mut *WAKE_DEADLINE_WARNINGS.0.lock().unwrap());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].ends_with(reason), "{warnings:?}");
        println!("{}", warnings[0]);
    }
}

#[tokio::test]
async fn wake_owner_deadline_before_dispatch() {
    const NAME: &str = "wake_owner_deadline_before_dispatch";
    let Some(scenario) = run_child_cases(module_path!(), NAME, &["standard", "logical"]).await
    else {
        return;
    };
    let logical = scenario == "logical";
    let stage = if logical {
        "logical_prepared"
    } else {
        "standard_response_prepared"
    };
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, _, sender, _) = prepare_retained_exited_respawn(&fixture).await;
    let manager = app.pty_sessions().read().await.clone();
    let wake = dispatch_deadline_wake(&manager, sender, logical).await;
    let (poller, control) = rollback_poller(true);
    let mut events = deadline_events(
        app,
        &[("first_refresh", 1), ("final_refresh", 1), (stage, 1)],
    );
    let rollback = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let producer_return = rollback.before_return.arm();
    let capture = Arc::new(CapturedCore::default());
    let outbox = tempfile::tempdir().unwrap();
    let origin = outbox.path().join(format!("{}.json", wake.id));
    let claim = wake_claim_path(&origin);
    let actor = WAKE_CORE_CAPTURE.scope(
        capture.clone(),
        deadline_claim_with_response(&poller, app, &wake, &origin),
    );
    tokio::pin!(actor);
    let (_, resume) = at_cut(actor.as_mut(), events.at("first_refresh", WAIT)).await;
    let before_transfer = control.observe_owner(&coordinator).unwrap().effects_started;
    resume.send(false).unwrap();
    let (id, resume) = at_cut(actor.as_mut(), events.at("final_refresh", WAIT)).await;
    let id = id.unwrap();
    manager.mark_idle(id).await;
    resume.send(false).unwrap();

    let event = at_cut(actor.as_mut(), events.recv()).await.unwrap();
    let resume = assert_dispatch_cut(event, stage, id, logical);
    let owner = capture.owner.lock().unwrap().take().unwrap();
    let before = owner.fixture_observe_publication();
    assert_eq!(before.binding.session_id(), id);
    assert!(before.publication && !before.terminal && !before.work_transferred);
    assert!(before.work_attempt.is_none() && !before.not_started());
    assert!(before.no_input_attempt().is_none());
    let source = control.observe_owner(&coordinator).unwrap();
    assert!(source.source_active);
    assert_eq!(source.effects_started, before_transfer + 1);
    let route = app
        .pty_routes()
        .lock()
        .unwrap()
        .route_identities(id)
        .unwrap();
    let binding_token = manager.get_session(id).await.unwrap().token;
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    assert!(backend.live.lock().unwrap().contains(&id));
    assert_eq!(backend.spawn_attempts(id), 1);
    assert_eq!(backend.write_attempts.load(Ordering::SeqCst), 0);

    assert_deadline(completes(actor).await, "work_transfer");
    assert!(
        origin.is_file() && !claim.exists(),
        "the original claim is retryable"
    );
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    assert!(owner.fixture_wake_source_observation().active);
    resume.send(false).unwrap();
    completes(rollback.before_return.entered.notified()).await;
    assert_eq!(
        backend.write_attempts.load(Ordering::SeqCst),
        0,
        "expired dispatch must perform zero backend write attempts"
    );
    assert!(!outbox_delivered_receipt(outbox.path(), &wake.id).exists());
    let after = owner.fixture_observe_publication();
    assert_eq!(
        (after.operation, after.binding),
        (before.operation, before.binding)
    );
    assert_eq!(after.no_input_attempt(), Some(after.work_attempt.unwrap()));
    assert!(after.publication && !after.work_transferred);
    assert_eq!(manager.get_session(id).await.unwrap().token, binding_token);
    assert_eq!(
        app.pty_routes().lock().unwrap().route_identities(id),
        Some(route)
    );

    producer_return.send(()).unwrap();
    assert_deadline_owner_retry(app, &coordinator, &owner, &wake, backend, id).await;
    assert!(WAKE_DEADLINE_WARNINGS.0.lock().unwrap().is_empty());
    close_coordinator(&coordinator).await;
}

async fn dispatch_deadline_wake(
    manager: &SessionManager,
    sender: Uuid,
    logical: bool,
) -> OutboxMessage {
    let mut wake = wake_message_to_target();
    wake.id = Uuid::new_v4().to_string();
    wake.token = Some(manager.get_session(sender).await.unwrap().token.to_string());
    if logical {
        wake.command = Some("clear".into());
        wake.body.clear();
    }
    wake
}

async fn deadline_claim_with_response(
    poller: &MailboxPoller,
    app: &impl MailboxHost,
    wake: &OutboxMessage,
    origin: &Path,
) -> Result<(), String> {
    let content = serde_json::to_string(wake).unwrap();
    std::fs::write(origin, &content).unwrap();
    let work = poller
        .admit_response_source(app, origin, wake, true, &content)
        .await
        .expect("the exact source was durably accepted");
    let claim = wake_claim_path(origin);
    std::fs::rename(origin, &claim).unwrap();
    let validated = ValidatedWake {
        message: wake.clone(),
        response: Some(work),
    };
    deliver_claimed_wake(poller, app, &claim, origin, &validated).await
}

fn assert_dispatch_cut(
    (stage, binding, timing, resume): WakeCreateEvent,
    expected: &str,
    id: Uuid,
    logical: bool,
) -> oneshot::Sender<bool> {
    assert_eq!(stage, expected);
    assert_eq!(binding, Some(id));
    if logical {
        assert!(timing.is_none());
    } else {
        let (observed_at, deadline) = timing.expect("standard response producer timing");
        assert!(observed_at < deadline);
        println!("standard B={id} observed_at={observed_at:?} deadline={deadline:?}");
    }
    resume
}

async fn assert_deadline_owner_retry(
    app: &impl MailboxHost,
    coordinator: &SelectionCoordinator,
    owner: &OwnedReplacementContinuation,
    wake: &OutboxMessage,
    backend: &MailboxMockPtyBackend,
    id: Uuid,
) {
    completes(async {
        while coordinator.fixture_root_count() != Some(0) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(owner.fixture_observe_publication().terminal);
    let manager = app.pty_sessions().read().await.clone();
    manager.mark_idle(id).await;
    let hooks = MailboxTestHooks::default();
    hooks.real_inject_sessions.lock().unwrap().insert(id);
    let retry = MailboxPoller::new_with_test_hooks(hooks);
    let live_before = backend.live.lock().unwrap().clone();
    completes(retry.deliver_wake(app, wake)).await.unwrap();
    assert_eq!(backend.spawn_attempts(id), 1);
    assert_eq!(*backend.live.lock().unwrap(), live_before);
    assert!(!backend.writes_for(id).is_empty());
}
