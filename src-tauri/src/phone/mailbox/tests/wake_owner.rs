use super::super::handoff::wake_claim_path;
use super::super::poller::{deliver_claimed_wake, OwnedPeerWakeStep, ValidatedWake};
use super::super::pty_observation::{PtyAuthorityState, WakeDeliveryState};
use super::claim_and::prime_route_tracker;
use super::support_core::{make_mailbox_fixture_in, MailboxFixture};
use super::*;
use crate::config::sessions_persistence::{classify_create_carrier_scope, CreateCarrierScope};
use crate::phone::mailbox::host::MailboxHost;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::{
    CreateAdmission, CreateClassification, ResponseCloseState, ResponseDeliveryProof,
    ResponseRouteIdentity, ReturnDeliveryProof,
};
use crate::session::selection::{
    OwnedReplacementContinuation, ReplacementFixtureAction as Action, ReplacementFixtureControl,
    ReplacementFixtureCut as Cut, ReplacementFixtureReceipt, SourceExecutorGuard,
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

async fn reacquire_lexical_guards(app: &impl MailboxHost, target: &str) {
    let gate = app
        .message_store()
        .unwrap()
        .target_gate
        .as_ref()
        .unwrap()
        .clone();
    let stripe = gate
        .try_target_lock(target)
        .unwrap()
        .expect("stripe released");
    let exact = tokio::time::timeout(WAIT, gate.acquire_exact(target))
        .await
        .expect("exact released");
    drop((exact, stripe));
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
    let context = tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
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
                coordinator: coordinator.clone(),
                published_carrier_version: member.carrier_version,
            },
            now,
        )
        .unwrap();
    let context = tracker
        .admit_return(
            &member,
            &coordinator,
            "wake-owner-release-return".into(),
            "release-return-body".into(),
            now,
        )
        .unwrap();
    tracker
        .observe_return_success(ReturnDeliveryProof { context }, now)
        .unwrap();

    let held = tracker.begin_delivery(&member.target).unwrap();
    let eligible_at = Instant::now() + Duration::from_secs(60);
    assert!(
        tracker
            .select_ticket(member.session_id, eligible_at)
            .is_err(),
        "a held dispatch must block response close",
    );
    drop(held);
    assert!(
        tracker
            .select_ticket(member.session_id, eligible_at)
            .is_ok(),
        "the refused wake must leave no dispatch reservation",
    );
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
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
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
    let create = tracker
        .reserve_create_owner(&msg.to, Uuid::new_v4(), classification)
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
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    let app = fixture.app.handle();
    let control = Arc::new(ReplacementFixtureControl::default());
    control.mock_backend.store(true, Ordering::Release);
    control.ticket_only.store(false, Ordering::Release);
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control);
    let mut poller = MailboxPoller::new_with_test_hooks(hooks);
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
    let outcome = tokio::time::timeout(
        WAIT,
        SelectionCoordinator::fixture_refuse_wake_binding(
            observed.clone(),
            deliver_claimed_wake(&poller, app, &claim, &origin, &wake),
        ),
    )
    .await
    .expect("bind refusal returns");
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
    poller
        .wake_outcomes_tx
        .send((origin.clone(), outcome))
        .unwrap();
    poller.drain_wake_outcomes().await;
    assert_eq!(poller.retry_tracker[&origin].attempt_count, 1);
    assert_eq!(WARNINGS.0.load(Ordering::Relaxed), 1);

    std::fs::rename(&origin, &claim).unwrap();
    tokio::time::timeout(
        WAIT,
        deliver_claimed_wake(&poller, app, &claim, &origin, &wake),
    )
    .await
    .expect("next wake can run after bind refusal")
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
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let msg = wake_message_to_target();
    let poller = MailboxPoller::default();
    let captured = Arc::new(CapturedCore {
        capture_future: true,
        ..CapturedCore::default()
    });
    let result = tokio::time::timeout(
        WAIT,
        WAKE_CORE_CAPTURE.scope(captured.clone(), poller.deliver_wake(app, &msg)),
    )
    .await
    .expect("producer captured");
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

    let gate = app
        .message_store()
        .unwrap()
        .target_gate
        .as_ref()
        .unwrap()
        .clone();
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

fn rollback_poller(ticket_only: bool) -> (MailboxPoller, Arc<ReplacementFixtureControl>) {
    let control = Arc::new(ReplacementFixtureControl::default());
    control.mock_backend.store(true, Ordering::Release);
    control.ticket_only.store(ticket_only, Ordering::Release);
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    (MailboxPoller::new_with_test_hooks(hooks), control)
}

async fn assert_wake_keeps_old_claim(app: &impl MailboxHost, wake: &OutboxMessage) {
    let poller = MailboxPoller::default();
    let result = tokio::time::timeout(Duration::from_millis(250), poller.deliver_wake(app, wake));
    assert!(
        !matches!(result.await, Ok(Ok(_))),
        "unsafe wake unexpectedly delivered"
    );
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
    control: &ReplacementFixtureControl,
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
    let result = tokio::time::timeout(
        WAIT,
        WAKE_CORE_CAPTURE.scope(capture.clone(), poller.deliver_wake(app, wake)),
    )
    .await
    .expect("wake returns");
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

async fn preparing_owner(fixture: &MailboxFixture, target: &str) -> Option<Uuid> {
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let CreateCarrierScope::Protected {
        preparing_owner, ..
    } = classify_create_carrier_scope(&permit, target, &fixture.target_cwd)
    else {
        panic!("the old carrier must still be protected");
    };
    preparing_owner
}

fn rollback_backend(app: &impl MailboxHost) -> Arc<dyn PtyBackend> {
    app.pty_routes()
        .lock()
        .unwrap()
        .backend_for_kind(SessionBackendKind::LocalProcess)
}

fn mock_backend(backend: &Arc<dyn PtyBackend>) -> &super::support_core::MailboxMockPtyBackend {
    backend.as_any().downcast_ref().unwrap()
}

#[tokio::test]
async fn wake_owner_pending_source_refusal_requires_typed_receipt() {
    let test = "wake_owner_pending_source_refusal_requires_typed_receipt";
    if std::env::var_os(CHILD).is_none() {
        for scenario in ["typed", "generic", "guard-window"] {
            run_child_case(test, scenario).await;
        }
        return;
    }
    let scenario = std::env::var(SCENARIO).unwrap();
    let generic = scenario == "generic";
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let manager = app.pty_sessions().read().await.clone();
    let pty = app.pty_routes();
    let backend = rollback_backend(app);
    let kills = mock_backend(&backend).kill_attempts.load(Ordering::SeqCst);
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    probe.refuse_pending_source.store(true, Ordering::Relaxed);
    probe.generic_retained.store(generic, Ordering::Relaxed);
    // The original caller returns before any physical cleanup; the retry must do it.
    let (poller, control) = rollback_poller(true);
    let wake = wake_message_to_target();
    let first = observe_wake(app, &poller, &wake);
    tokio::pin!(first);
    if scenario == "guard-window" {
        let release = probe.before_return.arm();
        at_cut(first.as_mut(), probe.before_return.entered.notified()).await;
        let gate = app.message_store().unwrap().target_gate.as_ref().unwrap();
        assert!(gate.try_target_lock(&wake.to).unwrap().is_none());
        let mut exact = Box::pin(gate.acquire_exact(&wake.to));
        let poll = std::future::poll_fn(|cx| std::task::Poll::Ready(exact.as_mut().poll(cx)));
        assert!(poll.await.is_pending());
        drop(exact);
        let inserted = retained_source_wait_cut(&control, Cut::Inserted).await;
        MailboxPoller::default()
            .fixture_probe_once(app, &wake, previous)
            .await;
        assert_eq!(
            *probe.probe_candidate.lock().unwrap(),
            Some(inserted.operation)
        );
        let pending = manager.get_pending_session(inserted.binding.unwrap()).await;
        assert!(
            pending.is_some(),
            "old pending row removed before both guards dropped"
        );
        assert_eq!(probe.removed.load(Ordering::Relaxed), 0);
        assert_eq!(coordinator.fixture_root_count(), Some(1));
        probe.before_claim.store(1, Ordering::Relaxed);
        release.send(()).unwrap();
    } else {
        probe.before_claim.store(1, Ordering::Relaxed);
    }
    let (result, owner) = first.await;
    assert!(result.unwrap_err().contains("create_retained"));
    assert!(!probe.refuse_pending_source.load(Ordering::Relaxed));
    assert_eq!(probe.before_claim.load(Ordering::Relaxed), 0);
    let binding = owner.binding().unwrap();
    assert!(manager.get_pending_session(binding).await.is_some());
    assert!(manager.get_session(previous).await.is_some());
    assert_eq!(
        preparing_owner(&fixture, &wake.to).await,
        Some(owner.operation())
    );
    let observation = owner.fixture_wake_rollback();
    let token = manager.get_pending_session(binding).await.unwrap().token;
    assert_eq!(observation.refused_token, (!generic).then_some(token));
    assert!(observation.recorded_error.is_none() && !observation.backend_submitted);
    assert!(!control.submitted.load(Ordering::Acquire));
    assert!(!pty.lock().unwrap().has_session(binding.session_id()));

    if generic {
        assert_wake_keeps_old_claim(app, &wake).await;
        assert!(manager.get_pending_session(binding).await.is_some());
        assert_eq!(
            preparing_owner(&fixture, &wake.to).await,
            Some(owner.operation())
        );
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
    assert_eq!(
        mock_backend(&backend).kill_attempts.load(Ordering::SeqCst),
        kills
    );
    close_coordinator(&coordinator).await;
}

#[tokio::test]
async fn wake_owner_same_claim_recovery() {
    let test = "wake_owner_same_claim_recovery";
    if std::env::var_os(CHILD).is_none() {
        for scenario in [
            "before-close",
            "after-removal",
            "ambiguous-close",
            "unrecorded-error",
            "changed-generation",
            "changed-row",
            "changed-binding",
            "changed-preparing",
            "changed-cwd",
            "tracker-failed",
        ] {
            run_child_case(test, scenario).await;
        }
        return;
    }
    let scenario = std::env::var(SCENARIO).unwrap();
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
    let (coordinator, _, _, previous) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let manager = app.pty_sessions().read().await.clone();
    let pty = app.pty_routes();
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    match scenario.as_str() {
        "before-close" | "changed-generation" | "changed-row" | "changed-binding"
        | "changed-cwd" => probe.before_close.store(8, Ordering::Relaxed),
        "after-removal" | "changed-preparing" => probe.before_release.store(8, Ordering::Relaxed),
        "ambiguous-close" => backend.fail_kill_once.store(true, Ordering::SeqCst),
        "tracker-failed" => probe
            .fail_tracker_before_removal
            .store(true, Ordering::Relaxed),
        "unrecorded-error" => {
            probe.refuse_precreate_source.store(true, Ordering::Relaxed);
            probe.before_claim.store(2, Ordering::Relaxed);
        }
        _ => unreachable!(),
    }
    let (poller, control) = rollback_poller(false);
    if scenario != "unrecorded-error" {
        control
            .actions
            .lock()
            .unwrap()
            .push(Action::SimulatedPrecommitFailure);
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
    let (cursor, removed, killed) = match scenario.as_str() {
        "before-close" | "changed-generation" | "changed-row" | "changed-binding"
        | "changed-cwd" => (Some("claimed"), 0, 0),
        "after-removal" | "changed-preparing" => (Some("row-absent"), 1, 1),
        "tracker-failed" => (Some("runtime-absent"), 0, 1),
        "ambiguous-close" => (Some("close-submitted"), 0, 1),
        "unrecorded-error" => (None, 0, 0),
        _ => unreachable!(),
    };
    assert_eq!(before.cursor, cursor);
    assert_eq!(before.recorded_error.is_some(), cursor.is_some());
    assert_eq!(
        manager.get_pending_session(binding).await.is_none(),
        removed > 0 || scenario == "unrecorded-error"
    );
    assert_eq!(probe.removed.load(Ordering::Relaxed), removed);
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills + killed);
    assert_eq!(
        preparing_owner(&fixture, &wake.to).await,
        Some(owner.operation())
    );
    probe.before_close.store(0, Ordering::Relaxed);
    probe.before_release.store(0, Ordering::Relaxed);
    probe.before_claim.store(0, Ordering::Relaxed);

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
            let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
            let challenger = MailboxPoller::default();
            let hold_probe = probe.after_probe.arm();
            let second = challenger.fixture_probe_owned_wake(app, &wake, previous);
            tokio::pin!(second);
            at_cut(second.as_mut(), probe.before_probe_permit.notified()).await;
            let old = fixture.target_cwd.with_extension("original");
            std::fs::rename(&fixture.target_cwd, &old).unwrap();
            std::fs::create_dir(&fixture.target_cwd).unwrap();
            std::fs::copy(
                old.join("config.json"),
                fixture.target_cwd.join("config.json"),
            )
            .unwrap();
            drop(permit);
            at_cut(second.as_mut(), probe.after_probe.entered.notified()).await;
            assert!(manager.get_pending_session(binding).await.is_some());
            assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
            drop(hold_probe);
        }
        "tracker-failed" => assert!(!probe.fail_tracker_before_removal.load(Ordering::Relaxed)),
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
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let wake = wake_message_to_target();
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let (old_poller, old_control) = rollback_poller(false);
    old_control
        .actions
        .lock()
        .unwrap()
        .push(Action::SimulatedPrecommitFailure);
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
    let (release_new, new_gate) = tokio::sync::oneshot::channel();
    new_control.actions.lock().unwrap().push(Action::Pause {
        cut: Cut::SourceCoreEntered,
        resume: new_gate,
    });
    let capture = Arc::new(CapturedCore::default());
    let mut competitor = wake.clone();
    competitor.id = format!("sealed-competitor-{}", Uuid::new_v4());
    let new_wake =
        WAKE_CORE_CAPTURE.scope(capture.clone(), new_poller.deliver_wake(app, &competitor));
    tokio::pin!(new_wake);
    at_cut(new_wake.as_mut(), probe.after_probe.entered.notified()).await;
    release_guard.send(()).unwrap();
    at_cut(old_wake.as_mut(), probe.before_terminal.entered.notified()).await;
    assert_eq!(preparing_owner(&fixture, &wake.to).await, None);
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    assert!(!old_control.observe_owner(&coordinator).unwrap().terminal);

    release_probe.send(()).unwrap();
    let new = at_recorded_cut(new_wake.as_mut(), &new_control, Cut::SourceCoreEntered).await;
    assert_ne!(old.binding, new.binding);
    assert_ne!(old.operation, new.operation);
    assert_eq!(coordinator.fixture_root_count(), Some(2));
    let old_state = old_control.observe_owner(&coordinator).unwrap();
    assert!(old_state.rollback_completed && !old_state.terminal && !old_state.transferred);
    assert_eq!(
        preparing_owner(&fixture, &wake.to).await,
        Some(new.operation)
    );
    release_old.send(()).unwrap();
    let (old_result, old_owner) = old_wake.await;
    assert!(old_result.is_err());
    assert!(old_owner.fixture_wake_rollback().terminal);
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    assert!(old_control.observe_owner(&coordinator).is_none());
    assert_eq!(
        preparing_owner(&fixture, &wake.to).await,
        Some(new.operation)
    );
    let new_owner = capture.owner.lock().unwrap().take().unwrap();
    assert_eq!(new_owner.binding(), new.binding);
    assert!(new_owner.fixture_wake_rollback().holds_create_and_work);
    release_new.send(()).unwrap();
    tokio::time::timeout(WAIT, new_wake).await.unwrap().unwrap();
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    close_coordinator(&coordinator).await;
}

#[tokio::test]
async fn wake_owner_queued_and_committed_controls() {
    let test = "wake_owner_queued_and_committed_controls";
    if std::env::var_os(CHILD).is_none() {
        for scenario in [
            "queued-drain",
            "panic-commit-before-disarm",
            "recv-after-release",
        ] {
            run_child_case(test, scenario).await;
        }
        return;
    }
    let scenario = std::env::var(SCENARIO).unwrap();
    run_retained_source_child_scenario(false, true, &scenario).await;
}

#[tokio::test]
async fn wake_owner_does_not_settle_pty_handoff() {
    let test = "wake_owner_does_not_settle_pty_handoff";
    if std::env::var_os(CHILD).is_none() {
        run_child_case(test, "").await;
        return;
    }
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
    let (coordinator, route, sender, previous) = prepare_retained_exited_respawn(&fixture).await;
    use crate::phone::types::PtyInputSourcePlane;
    use crate::session::manager::acquire_carrier_mutation_permit;
    use crate::session::selection::ReplacementAdmissionStep;
    let app = fixture.app.handle();
    let store = app.message_store().unwrap().store.as_ref().unwrap();
    let injection = enqueue_shared_pty_engine_operation(store, &route, sender, b"rollback handoff");
    let initial = store
        .query_pty_input_by_injection(&injection)
        .unwrap()
        .unwrap();
    let claimed = store
        .claim_pty_input(
            PtyInputSourcePlane::HostCli,
            Some(&injection),
            &Uuid::new_v4().to_string(),
            chrono::Utc::now(),
        )
        .unwrap()
        .unwrap();
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
    let rolled_back = owner
        .rollback_failed_create(&manager, pty, "setup failed")
        .await;
    assert!(rolled_back);
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
    if std::env::var_os(CHILD).is_none() {
        for scenario in ["queued", "partial-commit", "reply-lost"] {
            run_child_case(test, scenario).await;
        }
        return;
    }
    let scenario = std::env::var(SCENARIO).unwrap();
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
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
    let (release_reply, resume) = tokio::sync::oneshot::channel();
    let (drop_reply, reply_signal) = tokio::sync::oneshot::channel();
    let cut = match scenario.as_str() {
        "queued" => Cut::TicketSubmitted,
        "partial-commit" => {
            let action = Action::CommitBeforeDisarmPanicOnce;
            control.actions.lock().unwrap().push(action);
            Cut::WorkerUnwound
        }
        "reply-lost" => {
            *control.live_receiver_drop.lock().unwrap() = Some(reply_signal);
            let action = Action::Pause {
                cut: Cut::AfterReleaseBeforeSend,
                resume,
            };
            control.actions.lock().unwrap().push(action);
            Cut::AfterReleaseBeforeSend
        }
        _ => unreachable!(),
    };
    let wake = wake_message_to_target();
    let first = poller.deliver_wake(app, &wake);
    tokio::pin!(first);
    let receipt = at_recorded_cut(first.as_mut(), &control, cut).await;
    let binding = receipt.binding.unwrap();
    if cut == Cut::AfterReleaseBeforeSend {
        drop_reply.send(()).unwrap();
        at_recorded_cut(first.as_mut(), &control, Cut::ReceiverLost).await;
    }
    let backend = rollback_backend(app);
    let backend = mock_backend(&backend);
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let live = backend.live.lock().unwrap().clone();
    assert!(live.contains(&binding.session_id()));
    let manager = app.pty_sessions().read().await.clone();
    assert_eq!(
        manager.get_pending_session(binding).await.is_some(),
        scenario == "queued"
    );
    let roots = coordinator.fixture_root_count();
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    let mut competing_wake = wake.clone();
    competing_wake.id = format!("competitor-{}", Uuid::new_v4());
    MailboxPoller::default()
        .fixture_probe_once(app, &competing_wake, previous)
        .await;
    let retained = control.observe_owner(&coordinator).unwrap();
    assert!(!retained.rollback_completed && !retained.terminal);
    assert_eq!(
        *probe.probe_candidate.lock().unwrap(),
        Some(receipt.operation),
        "the competing admission reached the rollback probe for the same A"
    );
    assert_eq!(coordinator.fixture_root_count(), roots);
    assert_eq!(probe.removed.load(Ordering::Relaxed), 0);
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
    assert_eq!(*backend.live.lock().unwrap(), live);
    coordinator.shutdown_token().cancel();
    let _ = release_blocker.send(());
    let _ = release_reply.send(());
    close_coordinator(&coordinator).await;
}

async fn close_coordinator(coordinator: &SelectionCoordinator) {
    coordinator.shutdown_token().cancel();
    tokio::time::timeout(WAIT, coordinator.close_and_join())
        .await
        .unwrap();
}
