use super::super::handoff::wake_claim_path;
use super::super::poller::{deliver_claimed_wake, OwnedPeerWakeStep, ValidatedWake};
use super::super::pty_observation::{PtyAuthorityState, WakeDeliveryState};
use super::claim_and::prime_route_tracker;
use super::support_core::{make_mailbox_fixture, MailboxFixture, MailboxMockPtyBackend};
use super::*;
use crate::config::sessions_persistence::{classify_create_carrier_scope, CreateCarrierScope};
use crate::phone::mailbox::host::MailboxHost;
use crate::phone::types::PtyInputSourcePlane;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::{
    CreateAdmission, CreateClassification, ResponseCloseState, ResponseDeliveryProof,
    ResponseRouteIdentity, ReturnDeliveryProof,
};
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::selection::ReplacementAdmissionStep;
use crate::session::selection::{
    OwnedReplacementContinuation, OwnedSourceStop, ReplacementFixtureAction as Action,
    ReplacementFixtureControl as Control, ReplacementFixtureCut as Cut, ReplacementFixtureOutcome,
    ReplacementFixtureReceipt, SourceExecutorGuard,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Instant;

type WakeCoreResult = Result<OwnedPeerWakeStep, String>;
type WakeCoreFuture = Pin<Box<dyn Future<Output = WakeCoreResult> + Send>>;

#[derive(Default)]
struct CapturedCore {
    capture_future: bool,
    owner: Mutex<Option<OwnedReplacementContinuation>>,
    future: Mutex<Option<WakeCoreFuture>>,
}

tokio::task_local! {
    static WAKE_CORE_CAPTURE: Arc<CapturedCore>;
}

pub(in crate::phone::mailbox) fn fixture_capture_owner(
    continuation: &OwnedReplacementContinuation,
) {
    let _ = WAKE_CORE_CAPTURE.try_with(|captured| {
        *captured.owner.lock().unwrap() = Some(continuation.clone());
    });
}

pub(in crate::phone::mailbox) async fn run_owned_persist_core(
    operation: impl Future<Output = WakeCoreResult> + Send + 'static,
) -> WakeCoreResult {
    let mut operation = Some(operation);
    let _ = WAKE_CORE_CAPTURE.try_with(|captured| {
        if captured.capture_future {
            *captured.future.lock().unwrap() = operation
                .take()
                .map(|operation| -> WakeCoreFuture { Box::pin(operation) });
        }
    });
    match operation {
        Some(operation) => {
            crate::config::sessions_persistence::run_owned_persist_core(operation).await
        }
        None => Err("fixture captured wake core before poll".into()),
    }
}

const CHILD: &str = "AC_4819_WAKE_OWNER_CHILD";
const SCENARIO: &str = "AC_4819_WAKE_OWNER_SCENARIO";
const WAIT: Duration = Duration::from_secs(15);

// Isolate the config directory and process-wide carrier protection for each case.
async fn run_child_case(test: &str, scenario: &str) {
    let config = tempfile::tempdir().unwrap();
    let module = module_path!().split_once("::").unwrap().1;
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .arg(format!("{module}::{test}"))
        .args(["--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env(SCENARIO, scenario)
        .env("AGENTSCOMMANDER_CONFIG_DIR", config.path())
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", config.path())
        .env("RUST_MIN_STACK", "33554432")
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .expect("wake owner child exited within 45 seconds")
        .expect("wake owner child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed;"),
        "{scenario}\n{stdout}\n{stderr}",
    );
}

async fn run_child_cases(test: &str, scenarios: &[&str]) -> Option<String> {
    if std::env::var_os(CHILD).is_some() {
        return Some(std::env::var(SCENARIO).unwrap());
    }
    for scenario in scenarios {
        run_child_case(test, scenario).await;
    }
    None
}

struct WakeWarnings(std::sync::atomic::AtomicUsize);

impl log::Log for WakeWarnings {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() == log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let message = record.args().to_string();
            if message.contains("#3263 wake ended without input")
                && message.contains("stage=source_unbound")
            {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn flush(&self) {}
}

async fn completes<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(WAIT, future)
        .await
        .expect("wake fixture deadline")
}

async fn reacquire_lexical_guards(app: &impl MailboxHost, target: &str) {
    let gate = app.message_store().unwrap().target_gate.as_ref().unwrap();
    let stripe = gate
        .try_target_lock(target)
        .unwrap()
        .expect("stripe released");
    drop((completes(gate.acquire_exact(target)).await, stripe));
}

fn assert_dispatch_released(tracker: &ResponseCloseState, msg: &OutboxMessage) {
    // The real carrier is retired. Arm a temporary route on the same tracker and target.
    let member = ResponseRouteIdentity {
        target: msg.to.clone(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        generation: 1,
        backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        carrier_version: 1,
    };
    let coordinator = ResponseRouteIdentity {
        target: msg.from.clone(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        ..member.clone()
    };
    let now = Instant::now();
    prime_route_tracker(tracker, &member, &coordinator, now);
    let key = "wake-owner-release-probe";
    let digest = "release-probe-body";
    let mut request = tracker.reserve_work(&member.target, key).unwrap();
    assert_eq!(request.accept_durable(digest), Ok(true));
    drop(request);
    let (key, digest) = (key.into(), digest.into());
    let context = tracker
        .reserve_policy_candidate(member.clone(), coordinator.clone(), key, digest, now)
        .unwrap();
    tracker.mark_request_submitted(&context, true);
    let request = ResponseDeliveryProof {
        context,
        member: member.clone(),
        coordinator: coordinator.clone(),
        published_carrier_version: member.carrier_version,
    };
    tracker.register_request(request, now).unwrap();
    let key = "wake-owner-release-return".into();
    let digest = "release-return-body".into();
    let context = tracker
        .admit_return(&member, &coordinator, key, digest, now)
        .unwrap();
    let proof = ReturnDeliveryProof { context };
    tracker.observe_return_success(proof, now).unwrap();

    let eligible_at = Instant::now() + Duration::from_secs(60);
    let ticket = || tracker.select_ticket(member.session_id, eligible_at);
    let held = tracker.begin_delivery(&member.target).unwrap();
    assert!(ticket().is_err(), "held dispatch blocks response close");
    drop(held);
    assert!(ticket().is_ok(), "refused wake released dispatch");
    tracker.remove_route(member.session_id, member.generation);
    tracker.remove_route(coordinator.session_id, coordinator.generation);
}

async fn assert_unpublished_and_reacquire(
    app: &impl MailboxHost,
    msg: &OutboxMessage,
    cwd: &Path,
    previous: Uuid,
    reserved: Uuid,
) {
    let manager = app.pty_sessions().read().await.clone();
    let permit = acquire_carrier_mutation_permit().await;
    let directory = crate::config::config_dir().unwrap();
    let classification = manager
        .with_current_carrier_snapshot(&permit, |current| {
            assert!(!current.pending_ids.contains(&reserved));
            assert!(!current.rows.iter().any(|row| row.id == reserved));
            let carrier = crate::config::sessions_persistence::retained_carrier_for_replacement(
                &permit, &directory, &msg.to, previous, current,
            )
            .expect("original retired carrier still current");
            let CreateCarrierScope::Protected {
                epoch,
                preparing_owner: None,
                ..
            } = classify_create_carrier_scope(&permit, &msg.to, cwd)
            else {
                panic!("bind refusal must leave carrier protected without Preparing");
            };
            CreateClassification::Protected {
                identity: ResponseRouteIdentity {
                    target: msg.to.clone(),
                    session_id: previous,
                    token: carrier.token,
                    generation: carrier.retired_local_generation().unwrap(),
                    backend: crate::pty::backend::SessionBackendKind::LocalProcess,
                    carrier_version: carrier.version,
                },
                publication_epoch: epoch,
            }
        })
        .await;
    let tracker = {
        let pty = app.pty_routes().lock().unwrap();
        assert!(!pty.has_session(reserved));
        pty.response_close_state()
    };
    let operation = Uuid::new_v4();
    let create = tracker
        .reserve_create_owner(&msg.to, operation, classification)
        .unwrap();
    assert!(matches!(create, CreateAdmission::Reserved(_)));
    drop((create, permit));
    assert_dispatch_released(&tracker, msg);
    reacquire_lexical_guards(app, &msg.to).await;
}

#[tokio::test]
async fn wake_owner_bind_refusal_leaves_current_carrier_and_counts_retry() {
    if std::env::var_os(CHILD).is_none() {
        let test = "wake_owner_bind_refusal_leaves_current_carrier_and_counts_retry";
        run_child_case(test, "").await;
        return;
    }
    static WARNINGS: WakeWarnings = WakeWarnings(std::sync::atomic::AtomicUsize::new(0));
    log::set_logger(&WARNINGS).unwrap();
    log::set_max_level(log::LevelFilter::Warn);
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    let app = fixture.app.handle();
    let (mut poller, _) = rollback_poller(false);
    let msg = wake_message_to_target();
    let outbox = fixture._temp.path().join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let origin = outbox.join(format!("{}.json", msg.id));
    let claim = wake_claim_path(&origin);
    std::fs::write(&claim, serde_json::to_vec(&msg).unwrap()).unwrap();
    let wake = ValidatedWake {
        message: msg,
        response: None,
    };
    let observed = Arc::new(OnceLock::new());
    let wake_delivery = deliver_claimed_wake(&poller, app, &claim, &origin, &wake);
    let refusal =
        SelectionCoordinator::fixture_refuse_wake_binding(observed.clone(), wake_delivery);
    let outcome = completes(refusal).await;
    assert_eq!(
        outcome.as_ref().unwrap_err(),
        "wake retained without input: stage=source_unbound rollback=none",
    );
    assert_eq!(WARNINGS.0.load(Ordering::Relaxed), 1);
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    let reserved = *observed.get().expect("pre-bind probe observed reserved B");
    assert_unpublished_and_reacquire(app, &wake.message, &fixture.target_cwd, previous, reserved)
        .await;

    assert!(origin.exists() && !claim.exists());
    assert!(poller.live_claims.insert(claim.clone()));
    let outcomes = &poller.wake_outcomes_tx;
    outcomes.send((origin.clone(), outcome)).unwrap();
    poller.drain_wake_outcomes().await;
    assert_eq!(poller.retry_tracker[&origin].attempt_count, 1);
    assert_eq!(WARNINGS.0.load(Ordering::Relaxed), 1);

    std::fs::rename(&origin, &claim).unwrap();
    completes(deliver_claimed_wake(&poller, app, &claim, &origin, &wake))
        .await
        .expect("next wake delivered");
    assert!(!origin.exists() && !claim.exists());
    close_coordinator(&coordinator).await;
}

#[tokio::test]
async fn wake_owner_dropped_before_poll_stops_without_effects() {
    if std::env::var_os(CHILD).is_none() {
        run_child_case("wake_owner_dropped_before_poll_stops_without_effects", "").await;
        return;
    }
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let msg = wake_message_to_target();
    let poller = MailboxPoller::default();
    let captured = Arc::new(CapturedCore {
        capture_future: true,
        ..CapturedCore::default()
    });
    let wake = WAKE_CORE_CAPTURE.scope(captured.clone(), poller.deliver_wake(app, &msg));
    let result = completes(wake).await;
    assert_eq!(
        result.unwrap_err(),
        "fixture captured wake core before poll"
    );
    let owner = captured.owner.lock().unwrap().take().unwrap();
    let before = owner.fixture_wake_source_observation();
    assert!(before.active && !before.stopped);
    assert_eq!(before.effects_started, 0);
    assert_eq!(coordinator.fixture_root_count(), Some(1));

    // A different nonce cannot bind, and dropping that guard cannot settle the current owner.
    let mut other = SourceExecutorGuard::unbound();
    assert!(!other.bind(&owner.fixture_mismatched_wake_executor()));
    drop(other);
    assert_eq!(owner.fixture_wake_source_observation(), before);

    let gate = app.message_store().unwrap().target_gate.as_ref().unwrap();
    assert!(gate.try_target_lock(&msg.to).unwrap().is_none());
    assert_wake_keeps_old_claim(app, &msg).await;
    assert_eq!(owner.fixture_wake_source_observation(), before);
    drop(captured.future.lock().unwrap().take().unwrap());
    let after = owner.fixture_wake_source_observation();
    assert!(after.stopped && !after.active);
    assert_eq!(after.effects_started, 0);
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    reacquire_lexical_guards(app, &msg.to).await;
    let (poller, _) = rollback_poller(false);
    let new = assert_replacement_runs(app, &poller, &msg).await;
    assert_ne!(owner.binding(), new.binding());
    assert!(owner.fixture_wake_rollback().terminal);
    assert!(!owner.fixture_wake_rollback().backend_submitted);
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    close_coordinator(&coordinator).await;
}

fn rollback_poller(ticket_only: bool) -> (MailboxPoller, Arc<Control>) {
    let control = Arc::new(Control::default());
    control.mock_backend.store(true, Ordering::Release);
    control.ticket_only.store(ticket_only, Ordering::Release);
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    (MailboxPoller::new_with_test_hooks(hooks), control)
}

async fn assert_wake_keeps_old_claim(app: &impl MailboxHost, wake: &OutboxMessage) {
    let poller = MailboxPoller::default();
    let delivery = poller.deliver_wake(app, wake);
    let result = tokio::time::timeout(Duration::from_millis(250), delivery).await;
    assert!(!matches!(result, Ok(Ok(_))), "unsafe wake delivered");
}

fn add_action(control: &Control, action: Action) {
    control.actions.lock().unwrap().push(action);
}

fn pause_at(control: &Control, cut: Cut) -> tokio::sync::oneshot::Sender<()> {
    let (release, resume) = tokio::sync::oneshot::channel();
    add_action(control, Action::Pause { cut, resume });
    release
}

async fn at_cut<F: Future, C: Future>(actor: Pin<&mut F>, cut: C) -> C::Output {
    tokio::select! {
        biased;
        reached = tokio::time::timeout(WAIT, cut) => reached.expect("wake reached its cut"),
        _ = actor => panic!("wake returned before its cut"),
    }
}

async fn at_recorded_cut<F: Future>(
    actor: Pin<&mut F>,
    control: &Control,
    cut: Cut,
) -> ReplacementFixtureReceipt {
    at_cut(actor, retained_source_wait_cut(control, cut)).await
}

async fn observe_wake(
    app: &impl MailboxHost,
    poller: &MailboxPoller,
    wake: &OutboxMessage,
) -> (Result<OutboxMessage, String>, OwnedReplacementContinuation) {
    let capture = Arc::new(CapturedCore::default());
    let actor = WAKE_CORE_CAPTURE.scope(capture.clone(), poller.deliver_wake(app, wake));
    let result = completes(actor).await;
    let owner = capture
        .owner
        .lock()
        .unwrap()
        .take()
        .expect("wake admitted exactly one owner");
    (result, owner)
}

async fn assert_replacement_runs(
    app: &impl MailboxHost,
    poller: &MailboxPoller,
    wake: &OutboxMessage,
) -> OwnedReplacementContinuation {
    let (result, owner) = observe_wake(app, poller, wake).await;
    result.expect("the new wake runs a replacement");
    owner
}

async fn assert_preparing(fixture: &MailboxFixture, target: &str, operation: Option<Uuid>) {
    let permit = acquire_carrier_mutation_permit().await;
    let scope = classify_create_carrier_scope(&permit, target, &fixture.target_cwd);
    assert!(
        matches!(scope, CreateCarrierScope::Protected { preparing_owner, .. }
        if preparing_owner == operation),
        "old carrier must retain the exact Preparing owner"
    );
}

fn replace_cwd(cwd: &Path) -> PathBuf {
    let old = cwd.with_extension("original");
    std::fs::rename(cwd, &old).unwrap();
    std::fs::create_dir(cwd).unwrap();
    let config = old.join("config.json");
    if config.is_file() {
        std::fs::copy(config, cwd.join("config.json")).unwrap();
    }
    old
}

fn rollback_backend(app: &impl MailboxHost) -> Arc<dyn PtyBackend> {
    app.pty_routes()
        .lock()
        .unwrap()
        .backend_for_kind(SessionBackendKind::LocalProcess)
}

fn mock_backend(backend: &Arc<dyn PtyBackend>) -> &MailboxMockPtyBackend {
    backend.as_any().downcast_ref().unwrap()
}

#[tokio::test]
async fn wake_owner_pending_source_refusal_requires_typed_receipt() {
    let test = "wake_owner_pending_source_refusal_requires_typed_receipt";
    let changed_cases = ["producer-cwd", "setup-cwd", "setup-replica"];
    let cases = [["typed", "generic", "guard-window"], changed_cases].concat();
    let Some(scenario) = run_child_cases(test, &cases).await else {
        return;
    };
    let generic = scenario == "generic";
    let setup_error = scenario.starts_with("setup-");
    let changed = changed_cases.contains(&scenario.as_str());
    let replace_replica = scenario == "setup-replica";
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
    let mut expected_token = None;
    if changed {
        let validation = Cut::BeforeSourceValidation;
        let release = pause_at(&control, validation);
        let cut = at_recorded_cut(first.as_mut(), &control, validation).await;
        let binding = cut.binding.unwrap();
        let pending = manager.get_pending_session(binding).await.unwrap();
        assert_eq!(Path::new(&pending.working_directory), fixture.target_cwd);
        expected_token = Some(pending.token);
        if replace_replica {
            let object = |path: &Path| {
                crate::path_identity::verify_directory(path)
                    .unwrap()
                    .object_id
            };
            let cwd = &fixture.target_cwd;
            let original = object(cwd);
            let old = replace_cwd(&replica);
            std::fs::rename(old.join("nested"), cwd).unwrap();
            assert_eq!(object(cwd), original);
            assert_ne!(object(&replica), object(&old));
        } else {
            replace_cwd(&fixture.target_cwd);
        }
        let handoff = probe.before_return.arm();
        if setup_error {
            drop(release);
        } else {
            release.send(()).unwrap();
        }
        at_cut(first.as_mut(), probe.before_return.entered.notified()).await;
        let state = control.observe_owner(&coordinator).unwrap();
        assert!(state.source_active && !state.rollback_completed && !state.cleanup_claimed);
        let stop = (!setup_error).then_some(OwnedSourceStop::AuthorityChanged);
        assert_eq!(state.source_stop, stop);
        let kept = manager
            .get_pending_session(binding)
            .await
            .map(|row| row.token);
        assert_eq!(kept, expected_token, "exact B before source handoff");
        assert_preparing(&fixture, &wake.to, Some(cut.operation)).await;
        assert_eq!(coordinator.fixture_root_count(), Some(1));
        handoff.send(()).unwrap();
    } else if scenario == "guard-window" {
        let release = probe.before_return.arm();
        at_cut(first.as_mut(), probe.before_return.entered.notified()).await;
        let gate = app.message_store().unwrap().target_gate.as_ref().unwrap();
        assert!(gate.try_target_lock(&wake.to).unwrap().is_none());
        let mut exact = Box::pin(gate.acquire_exact(&wake.to));
        assert!(futures::poll!(exact.as_mut()).is_pending());
        drop(exact);
        let inserted = retained_source_wait_cut(&control, Cut::Inserted).await;
        MailboxPoller::default()
            .fixture_probe_once(app, &wake, previous)
            .await;
        assert_eq!(
            *probe.probe_candidate.lock().unwrap(),
            Some(inserted.operation)
        );
        let binding = inserted.binding.unwrap();
        let pending = manager.get_pending_session(binding).await;
        assert!(pending.is_some(), "pending B kept while both guards held");
        assert_eq!(probe.removed.load(Ordering::Relaxed), 0);
        assert_eq!(coordinator.fixture_root_count(), Some(1));
        probe.before_claim.store(1, Ordering::Relaxed);
        release.send(()).unwrap();
    } else {
        probe.before_claim.store(1, Ordering::Relaxed);
    }
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
    let Some(scenario) = run_child_cases(test, &names).await else {
        return;
    };
    let index = names.iter().position(|&name| name == scenario).unwrap();
    let (_, cursor, removed, killed) = cases[index];
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let manager = app.pty_sessions().read().await.clone();
    let pty = app.pty_routes();
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
    assert_eq!(
        manager.get_pending_session(binding).await.is_none(),
        removed > 0 || scenario == "unrecorded-error"
    );
    assert_eq!(probe.removed.load(Ordering::Relaxed), removed);
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills + killed);
    assert_preparing(&fixture, &wake.to, Some(owner.operation())).await;
    for counter in [
        &probe.before_close,
        &probe.before_release,
        &probe.before_claim,
    ] {
        counter.store(0, Ordering::Relaxed);
    }

    match scenario.as_str() {
        "changed-row" => manager.fixture_change_rollback_row(binding, false).await,
        "changed-binding" => manager.fixture_change_rollback_row(binding, true).await,
        "changed-preparing" => owner.fixture_replace_rollback_preparing(&manager).await,
        "changed-generation" => {
            let pty = pty.lock().unwrap();
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
            let hold_probe = probe.after_probe.arm();
            let second = challenger.fixture_probe_owned_wake(app, &wake, previous);
            tokio::pin!(second);
            at_cut(second.as_mut(), probe.before_probe_permit.notified()).await;
            replace_cwd(&fixture.target_cwd);
            drop(permit);
            at_cut(second.as_mut(), probe.after_probe.entered.notified()).await;
            assert!(manager.get_pending_session(binding).await.is_some());
            assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
            drop(hold_probe);
        }
        "tracker-failed" => assert!(!fail_tracker.load(Ordering::Relaxed)),
        _ => (),
    }
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

#[tokio::test]
async fn wake_owner_sealed_rollback_allows_new_preparing() {
    let test = "wake_owner_sealed_rollback_allows_new_preparing";
    if std::env::var_os(CHILD).is_none() {
        run_child_case(test, "").await;
        return;
    }
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let wake = wake_message_to_target();
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let (old_poller, old_control) = rollback_poller(false);
    add_action(&old_control, Action::SimulatedPrecommitFailure);
    let release_guard = probe.before_return.arm();
    let release_old = probe.before_terminal.arm();
    let old_wake = observe_wake(app, &old_poller, &wake);
    tokio::pin!(old_wake);
    at_cut(old_wake.as_mut(), probe.before_return.entered.notified()).await;
    let old = retained_source_wait_cut(&old_control, Cut::Inserted).await;
    let old_state = old_control.observe_owner(&coordinator).unwrap();
    assert!(old_state.source_active && old_state.rollback_completed && !old_state.terminal);

    let (new_poller, new_control) = rollback_poller(false);
    let release_probe = probe.after_probe.arm();
    let release_new = pause_at(&new_control, Cut::SourceCoreEntered);
    let capture = Arc::new(CapturedCore::default());
    let mut competitor = wake.clone();
    competitor.id = format!("sealed-competitor-{}", Uuid::new_v4());
    let new_wake =
        WAKE_CORE_CAPTURE.scope(capture.clone(), new_poller.deliver_wake(app, &competitor));
    tokio::pin!(new_wake);
    at_cut(new_wake.as_mut(), probe.after_probe.entered.notified()).await;
    release_guard.send(()).unwrap();
    at_cut(old_wake.as_mut(), probe.before_terminal.entered.notified()).await;
    assert_preparing(&fixture, &wake.to, None).await;
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    assert!(!old_control.observe_owner(&coordinator).unwrap().terminal);

    release_probe.send(()).unwrap();
    let new = at_recorded_cut(new_wake.as_mut(), &new_control, Cut::SourceCoreEntered).await;
    assert_ne!(old.binding, new.binding);
    assert_ne!(old.operation, new.operation);
    assert_eq!(coordinator.fixture_root_count(), Some(2));
    let old_state = old_control.observe_owner(&coordinator).unwrap();
    assert!(old_state.rollback_completed && !old_state.terminal && !old_state.transferred);
    assert_preparing(&fixture, &wake.to, Some(new.operation)).await;
    release_old.send(()).unwrap();
    let (old_result, old_owner) = old_wake.await;
    assert!(old_result.is_err());
    assert!(old_owner.fixture_wake_rollback().terminal);
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    assert!(old_control.observe_owner(&coordinator).is_none());
    assert_preparing(&fixture, &wake.to, Some(new.operation)).await;
    let new_owner = capture.owner.lock().unwrap().take().unwrap();
    assert_eq!(new_owner.binding(), new.binding);
    assert!(new_owner.fixture_wake_rollback().holds_create_and_work);
    release_new.send(()).unwrap();
    completes(new_wake).await.unwrap();
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    close_coordinator(&coordinator).await;
}

#[tokio::test]
async fn wake_owner_queued_and_committed_controls() {
    let test = "wake_owner_queued_and_committed_controls";
    let cases = [
        "queued-drain",
        "panic-commit-before-disarm",
        "recv-after-release",
    ];
    let Some(scenario) = run_child_cases(test, &cases).await else {
        return;
    };
    run_retained_source_child_scenario(false, true, &scenario).await;
}

#[tokio::test]
async fn wake_owner_does_not_settle_pty_handoff() {
    let test = "wake_owner_does_not_settle_pty_handoff";
    if std::env::var_os(CHILD).is_none() {
        run_child_case(test, "").await;
        return;
    }
    let fixture = make_mailbox_fixture();
    let (coordinator, route, sender, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let store = app.message_store().unwrap().store.as_ref().unwrap();
    let injection = enqueue_shared_pty_engine_operation(store, &route, sender, b"rollback handoff");
    let initial = store
        .query_pty_input_by_injection(&injection)
        .unwrap()
        .unwrap();
    let plane = PtyInputSourcePlane::HostCli;
    let nonce = Uuid::new_v4().to_string();
    let claimed = store.claim_pty_input(plane, Some(&injection), &nonce, chrono::Utc::now());
    let claimed = claimed.unwrap().unwrap();
    let wake = wake_message_to_target();
    let challenger = MailboxPoller::default();
    let request = challenger.fixture_wake_request(app, &wake).await;
    let manager = app.pty_sessions().read().await.clone();
    let prepared = MailboxPoller::prepare_pty_replacement(
        &coordinator,
        &manager,
        &request,
        &claimed,
        &initial,
        &route.target,
        previous,
    )
    .await
    .unwrap()
    .unwrap();
    let pty = app.pty_routes();
    let tracker = pty.lock().unwrap().response_close_state();
    let permit = acquire_carrier_mutation_permit().await;
    let admission = manager
        .with_current_carrier_snapshot(&permit, |current| {
            coordinator.admit_owned_replacement(&permit, current, &tracker, pty, prepared)
        })
        .await
        .unwrap();
    drop(permit);
    let ReplacementAdmissionStep::Accepted(owner) = admission else {
        panic!("PTY replacement was not admitted");
    };
    // Stop the PTY source after real rollback and before its durable transfer.
    let mut source = SourceExecutorGuard::unbound();
    assert!(source.bind(&owner));
    let rollback = owner.rollback_failed_create(&manager, pty, "setup failed");
    assert!(rollback.await);
    drop(source);
    assert!(owner.fixture_wake_source_observation().stopped);
    assert!(owner.precommit_rollback_completed());
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    challenger.fixture_probe_once(app, &wake, previous).await;
    let terminal = owner.fixture_wake_rollback().terminal;
    assert!(!terminal, "PTY owns its own durable handoff");
    assert_eq!(*probe.probe_candidate.lock().unwrap(), None);
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    close_coordinator(&coordinator).await;
}

#[tokio::test]
async fn wake_owner_competing_admission_preserves_nonrollbackable_owner() {
    let test = "wake_owner_competing_admission_preserves_nonrollbackable_owner";
    let cases = ["queued", "partial-commit", "reply-lost"];
    let Some(scenario) = run_child_cases(test, &cases).await else {
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

async fn close_coordinator(coordinator: &SelectionCoordinator) {
    coordinator.shutdown_token().cancel();
    completes(coordinator.close_and_join()).await;
}
