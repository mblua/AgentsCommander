mod publication;
mod rollback;

use super::super::handoff::wake_claim_path;
use super::super::poller::{deliver_claimed_wake, OwnedPeerWakeStep, ValidatedWake};
use super::super::pty_observation::{PtyAuthorityState, WakeDeliveryState};
use super::claim_and::prime_route_tracker;
use super::support_core::{make_mailbox_fixture, MailboxFixture, MailboxMockPtyBackend};
use super::*;
use crate::config::sessions_persistence::{
    classify_create_carrier_scope, persist_initial_carrier, retire_carrier_for_discard,
    CreateCarrierScope, InitialCarrierIntent, PublishedCarrier,
};
use crate::phone::mailbox::host::{MailboxHost, WakeCreateEvents, WakeCreateProbe};
use crate::phone::types::PtyInputSourcePlane;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::{
    CreateAdmission, CreateClassification, ResponseCloseState, ResponseDeliveryProof,
    ResponseRouteIdentity, ReturnDeliveryProof,
};
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::selection::ReplacementAdmissionStep;
use crate::session::selection::{
    OwnedReplacementContinuation, ReplacementFixtureAction as Action,
    ReplacementFixtureControl as Control, ReplacementFixtureCut as Cut, ReplacementFixtureReceipt,
    SourceExecutorGuard,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Instant;
use tauri::{test::MockRuntime, AppHandle, Manager};
use tokio::sync::oneshot;

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
async fn run_child_case(caller_module: &str, test: &str, scenario: &str) {
    let config = tempfile::tempdir().unwrap();
    let module = caller_module.split_once("::").unwrap().1;
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

async fn run_child_cases(caller_module: &str, test: &str, scenarios: &[&str]) -> Option<String> {
    if std::env::var_os(CHILD).is_some() {
        return Some(std::env::var(SCENARIO).unwrap());
    }
    for scenario in scenarios {
        run_child_case(caller_module, test, scenario).await;
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
        run_child_case(module_path!(), test, "").await;
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
        run_child_case(
            module_path!(),
            "wake_owner_dropped_before_poll_stops_without_effects",
            "",
        )
        .await;
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
async fn wake_owner_sealed_rollback_allows_new_preparing() {
    let test = "wake_owner_sealed_rollback_allows_new_preparing";
    if std::env::var_os(CHILD).is_none() {
        run_child_case(module_path!(), test, "").await;
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
async fn wake_owner_does_not_settle_pty_handoff() {
    let test = "wake_owner_does_not_settle_pty_handoff";
    if std::env::var_os(CHILD).is_none() {
        run_child_case(module_path!(), test, "").await;
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

async fn close_coordinator(coordinator: &SelectionCoordinator) {
    coordinator.shutdown_token().cancel();
    completes(coordinator.close_and_join()).await;
}

fn deadline_events(
    app: &AppHandle<MockRuntime>,
    cuts: &[(&'static str, usize)],
) -> WakeCreateEvents {
    let (probe, events) = WakeCreateProbe::pause_at(Duration::from_secs(4), cuts);
    assert!(app.manage(probe));
    events
}

async fn deadline_event(
    events: &mut WakeCreateEvents,
    stage: &str,
) -> (Option<Uuid>, oneshot::Sender<bool>) {
    let (actual, binding, resume) = completes(events.recv()).await.unwrap();
    assert_eq!(actual, stage);
    (binding, resume)
}

async fn deadline_claim(
    poller: &MailboxPoller,
    app: &impl MailboxHost,
    wake: &OutboxMessage,
) -> Result<(), String> {
    let temp = tempfile::tempdir().unwrap();
    let origin = temp.path().join(format!("{}.json", wake.id));
    let claim = wake_claim_path(&origin);
    std::fs::write(&claim, serde_json::to_vec(wake).unwrap()).unwrap();
    let message = wake.clone();
    let validated = ValidatedWake {
        message,
        response: None,
    };
    deliver_claimed_wake(poller, app, &claim, &origin, &validated)
        .await
        .inspect_err(|_| assert!(origin.is_file() && !claim.exists()))
}

fn assert_deadline(result: Result<(), String>, dispatch: &str) {
    let error = result.unwrap_err();
    let diagnostic = format!("stage=deadline last_dispatch={dispatch} settlement=unknown");
    WakeCreateProbe::assert_deadline_warning(&error, &diagnostic);
}

async fn generic_wake_conflict(fixture: &MailboxFixture) -> PublishedCarrier {
    let cwd = fixture.target_cwd.join("conflicting-carrier");
    std::fs::create_dir_all(&cwd).unwrap();
    let manager = SessionManager::new();
    let row = manager
        .create_session(
            "codex".into(),
            vec![],
            cwd.to_string_lossy().into_owned(),
            None,
            None,
            vec![],
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    manager.set_start_fresh_on_restore(row.id, true).await;
    let permit = acquire_carrier_mutation_permit().await;
    let version = manager
        .with_current_carrier_snapshot(&permit, |s| s.versions[&row.id])
        .await;
    drop(permit);
    let directory = fixture._temp.path().join("deadline-conflict");
    persist_initial_carrier(
        &manager,
        directory,
        vec![cwd.to_string_lossy().into_owned()],
        InitialCarrierIntent::new("deadline-conflict".into(), row.id, row.token, version, cwd),
    )
    .await
    .unwrap()
    .expect("the overlapping carrier is genuinely protected")
}

async fn assert_generic_conflict(fixture: &MailboxFixture, target: &str) {
    let permit = acquire_carrier_mutation_permit().await;
    assert!(matches!(
        classify_create_carrier_scope(&permit, target, &fixture.target_cwd),
        CreateCarrierScope::Waiting
    ));
}

async fn remove_deadline_fixture_origin(app: &impl MailboxHost, previous: Uuid) {
    let permit = acquire_carrier_mutation_permit().await;
    assert!(retire_carrier_for_discard(&permit, previous));
    drop(permit);
    let manager = app.pty_sessions().read().await.clone();
    assert_eq!(manager.remove_exited_sessions(&[previous]).await, 1);
}

async fn retry_generic_create(
    coordinator: &SelectionCoordinator,
    app: &impl MailboxHost,
    wake: &OutboxMessage,
) -> Uuid {
    let capacity = coordinator.port_test_state();
    assert_eq!(
        capacity.create_tickets_available,
        capacity.expected_create_ticket_capacity
    );
    assert_eq!(capacity.queue_capacity, capacity.expected_queue_capacity);
    reacquire_lexical_guards(app, &wake.to).await;
    let poller = MailboxPoller::default();
    let plan = poller.resolve_wake_spawn_plan(app, wake).await.unwrap();
    let (id, _, _) = completes(poller.spawn_peer_wake_fallback(app, wake, plan, false, false))
        .await
        .unwrap();
    assert_eq!(mock_backend(&rollback_backend(app)).spawn_attempts(id), 1);
    id
}

async fn pending_ids(app: &impl MailboxHost) -> std::collections::HashSet<Uuid> {
    let manager = app.pty_sessions().read().await.clone();
    let permit = acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |s| s.pending_ids.clone())
        .await
}

async fn resume_deadline_event(events: &mut WakeCreateEvents, stage: &str) {
    deadline_event(events, stage).await.1.send(false).unwrap();
}

#[tokio::test]
async fn wake_owner_generic_deadline_and_retained_controls() {
    const NAME: &str = "wake_owner_generic_deadline_and_retained_controls";
    let cases = [
        "publication",
        "archive",
        "resource",
        "bound",
        "retained",
        "generic",
        "live",
    ];
    let Some(case) = run_child_cases(module_path!(), NAME, &cases).await else {
        return;
    };
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    remove_deadline_fixture_origin(app, previous).await;
    let wake = wake_message_to_target();
    match case.as_str() {
        "generic" | "live" => {
            check_unowned_input(app, &coordinator, &wake, case == "generic").await;
        }
        "retained" => {
            check_generic_retained(app, &wake).await;
            retry_generic_create(&coordinator, app, &wake).await;
        }
        case => {
            exercise_generic_conflict(&fixture, &wake, case).await;
            if case != "bound" {
                retry_generic_create(&coordinator, app, &wake).await;
            }
        }
    }
    close_coordinator(&coordinator).await;
}

async fn exercise_generic_conflict(fixture: &MailboxFixture, wake: &OutboxMessage, case: &str) {
    let app = fixture.app.handle();
    let archive = case == "archive";
    let bound = case == "bound";
    let deferred = matches!(case, "resource" | "bound");
    let project = fixture.target_cwd.ancestors().nth(3).unwrap();
    let archived = vec![project.to_string_lossy().into_owned()];
    if archive {
        app.pty_settings().write().await.archived_project_paths = archived.clone();
    }
    let refusal = match case {
        "archive" => "archive_refused",
        "resource" => "resource_refused",
        _ => "publication_refused",
    };
    let mut cuts = vec![(refusal, 1), ("generic_exit", 1)];
    if deferred {
        cuts.insert(0, ("publication_before", if bound { 3 } else { 2 }));
    }
    if archive {
        cuts.insert(1, ("archive_exit", 1));
    }
    let mut events = deadline_events(app, &cuts);
    let poller = MailboxPoller::default();
    let actor = deadline_claim(&poller, app, wake);
    tokio::pin!(actor);
    let start = if deferred {
        let before = deadline_event(&mut events, "publication_before");
        Some(at_cut(actor.as_mut(), before).await.1)
    } else {
        None
    };
    let ids = pending_ids(app).await;
    assert_eq!(ids.len(), usize::from(bound));
    let _conflict = generic_wake_conflict(fixture).await;
    if let Some(resume) = start {
        resume.send(false).unwrap();
    }
    let (_, resume) = at_cut(actor.as_mut(), deadline_event(&mut events, refusal)).await;
    assert_generic_conflict(fixture, &wake.to).await;
    assert_deadline(completes(actor).await, "generic_create");
    resume.send(false).unwrap();
    if bound {
        let awaited = tokio::time::timeout(Duration::from_millis(300), events.recv());
        assert!(awaited.await.is_err(), "bound producer remains owned");
        assert_eq!(pending_ids(app).await, ids);
        return;
    }
    if archive {
        resume_deadline_event(&mut events, "archive_exit").await;
        let settings = app.pty_settings().read().await;
        assert_eq!(settings.archived_project_paths, archived);
    }
    resume_deadline_event(&mut events, "generic_exit").await;
    assert_generic_conflict(fixture, &wake.to).await;
    assert!(pending_ids(app).await.is_empty());
}

async fn check_unowned_input(
    app: &AppHandle<MockRuntime>,
    coordinator: &SelectionCoordinator,
    wake: &OutboxMessage,
    cold: bool,
) {
    let manager = app.pty_sessions().read().await.clone();
    let mut cuts = vec![("input", 1)];
    if cold {
        cuts.insert(0, ("generic_exit", 1));
    } else {
        let id = retry_generic_create(coordinator, app, wake).await;
        manager.mark_idle(id).await;
    }
    let mut events = deadline_events(app, &cuts);
    let poller = MailboxPoller::default();
    let actor = deadline_claim(&poller, app, wake);
    tokio::pin!(actor);
    if cold {
        let (id, resume) =
            at_cut(actor.as_mut(), deadline_event(&mut events, "generic_exit")).await;
        manager.mark_idle(id.unwrap()).await;
        resume.send(false).unwrap();
    }
    let (_, _input) = at_cut(actor.as_mut(), deadline_event(&mut events, "input")).await;
    assert_deadline(completes(actor).await, "input");
}

async fn check_generic_retained(app: &AppHandle<MockRuntime>, wake: &OutboxMessage) {
    use crate::session::create_contracts::ERR_WAKE_CREATE_RETAINED;
    app.pty_settings().write().await.resource_monitor_enabled = false;
    assert!(!app.pty_settings().read().await.resource_monitor_enabled);
    let mut events = deadline_events(app, &[("pending_mutex", 1)]);
    let poller = MailboxPoller::default();
    let actor = deadline_claim(&poller, app, wake);
    tokio::pin!(actor);
    let cut = "pending_mutex";
    let (binding, poison) = at_cut(actor.as_mut(), deadline_event(&mut events, cut)).await;
    poison.send(true).unwrap();
    let error = completes(actor).await.unwrap_err();
    assert!(error.ends_with(ERR_WAKE_CREATE_RETAINED), "{error}");
    let id = binding.unwrap();
    assert!(pending_ids(app).await.is_empty());
    let manager = app.pty_sessions().read().await.clone();
    assert!(manager.get_session(id).await.is_none());
    assert_eq!(mock_backend(&rollback_backend(app)).spawn_attempts(id), 0);
    assert!(!app.pty_settings().read().await.resource_monitor_enabled);
}

#[tokio::test]
async fn wake_owner_owned_deadline_reports_dispatch_and_settles_exact_owner() {
    const NAME: &str = "wake_owner_owned_deadline_reports_dispatch_and_settles_exact_owner";
    let cases = [
        "before_context",
        "first_refresh",
        "final_refresh",
        "input",
        "late_native",
    ];
    let Some(case) = run_child_cases(module_path!(), NAME, &cases).await else {
        return;
    };
    let fixture = make_mailbox_fixture();
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let pre_context = case == "before_context";
    let stage = match case.as_str() {
        "before_context" => "owned_admission",
        "first_refresh" | "late_native" => "owned_create",
        "final_refresh" => "work_transfer",
        _ => "input",
    };
    let cut = cases.into_iter().find(|name| *name == case).unwrap();
    let mut events = deadline_events(app, &[(cut, 1)]);
    let (poller, control) = rollback_poller(true);
    let rollback = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let producer_return = rollback.before_return.arm();
    let source_cut = if pre_context {
        Cut::SourceCoreEntered
    } else {
        Cut::BeforeBackendSpawn
    };
    let source = (pre_context || case == "late_native").then(|| pause_at(&control, source_cut));
    let capture = Arc::new(CapturedCore::default());
    let wake = wake_message_to_target();
    let actor = WAKE_CORE_CAPTURE.scope(capture.clone(), deadline_claim(&poller, app, &wake));
    tokio::pin!(actor);
    if let Some(resume) = source {
        at_recorded_cut(actor.as_mut(), &control, source_cut).await;
        assert_deadline(completes(actor).await, stage);
        resume.send(()).unwrap();
    } else {
        let (_, resume) = at_cut(actor.as_mut(), deadline_event(&mut events, &case)).await;
        if case == "input" {
            assert_deadline(completes(actor).await, stage);
            resume.send(false).unwrap();
        } else {
            let permit = acquire_carrier_mutation_permit().await;
            resume.send(false).unwrap();
            assert_deadline(completes(actor).await, stage);
            drop(permit);
        }
    }
    completes(rollback.before_return.entered.notified()).await;
    let owner = capture.owner.lock().unwrap().take().unwrap();
    let observation = owner.fixture_observe_publication();
    let id = observation.binding.session_id();
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    if pre_context {
        assert!(owner.precommit_rollback_completed());
        let before_publish = control.observe_owner(&coordinator).unwrap();
        assert_eq!(before_publish.effects_started, 1);
        assert!(!observation.publication && backend.spawn_attempts(id) == 0);
    } else if matches!(case.as_str(), "first_refresh" | "late_native") {
        assert!(observation.not_started() && observation.work_attempt.is_none());
        assert_eq!(backend.spawn_attempts(id), 1);
        assert_eq!(backend.write_attempts.load(Ordering::SeqCst), 0);
    } else if case == "final_refresh" {
        assert!(observation.no_input_attempt().is_some());
        assert_eq!(observation.no_input_attempt(), observation.work_attempt);
        assert_eq!(backend.write_attempts.load(Ordering::SeqCst), 0);
    } else {
        assert_eq!(backend.spawn_attempts(id), 1);
    }
    producer_return.send(()).unwrap();
    completes(async {
        while coordinator.fixture_root_count() != Some(0) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    if pre_context {
        assert_unpublished_and_reacquire(app, &wake, &fixture.target_cwd, previous, id).await;
    } else {
        let manager = app.pty_sessions().read().await.clone();
        manager.mark_idle(id).await;
        let hooks = MailboxTestHooks::default();
        hooks.real_inject_sessions.lock().unwrap().insert(id);
        let retry = MailboxPoller::new_with_test_hooks(hooks);
        completes(retry.deliver_wake(app, &wake)).await.unwrap();
        assert_eq!(backend.spawn_attempts(id), 1);
        assert!(!backend.writes_for(id).is_empty());
    }
    close_coordinator(&coordinator).await;
}
