use super::super::handoff::wake_claim_path;
use super::super::poller::{deliver_claimed_wake, OwnedPeerWakeStep, ValidatedWake};
use super::super::pty_observation::WakeDeliveryState;
use super::claim_and::prime_route_tracker;
use super::support_core::make_mailbox_fixture_in;
use super::*;
use crate::config::sessions_persistence::{classify_create_carrier_scope, CreateCarrierScope};
use crate::phone::mailbox::host::MailboxHost;
use crate::pty::manager::{
    CreateAdmission, CreateClassification, ResponseCloseState, ResponseDeliveryProof,
    ResponseRouteIdentity, ReturnDeliveryProof,
};
use crate::session::selection::{
    OwnedReplacementContinuation, ReplacementFixtureControl, SourceExecutorGuard,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Instant;

type WakeCoreResult = Result<OwnedPeerWakeStep, String>;
type WakeCoreFuture = Pin<Box<dyn Future<Output = WakeCoreResult> + Send>>;

#[derive(Default)]
struct CapturedCore {
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
        *captured.future.lock().unwrap() = operation
            .take()
            .map(|operation| -> WakeCoreFuture { Box::pin(operation) });
    });
    match operation {
        Some(operation) => {
            crate::config::sessions_persistence::run_owned_persist_core(operation).await
        }
        None => Err("fixture captured wake core before poll".into()),
    }
}

const CHILD: &str = "AC_4819_WAKE_OWNER_CHILD";
const WAIT: Duration = Duration::from_secs(15);

// Isolate the config directory and process-wide carrier protection for each case.
async fn run_child(test: &str) {
    let config = tempfile::tempdir().unwrap();
    let module = module_path!().split_once("::").unwrap().1;
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .arg(format!("{module}::{test}"))
        .args(["--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
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
        "{stdout}\n{stderr}",
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
        run_child("wake_owner_bind_refusal_leaves_current_carrier_and_counts_retry").await;
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
        run_child("wake_owner_dropped_before_poll_stops_without_effects").await;
        return;
    }
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    let app = fixture.app.handle();
    let msg = wake_message_to_target();
    let poller = MailboxPoller::default();
    let captured = Arc::new(CapturedCore::default());
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
    drop(captured.future.lock().unwrap().take().unwrap());
    let after = owner.fixture_wake_source_observation();
    assert!(after.stopped && !after.active);
    assert_eq!(after.effects_started, 0);
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    reacquire_lexical_guards(app, &msg.to).await;
    close_coordinator(&coordinator).await;
}

async fn close_coordinator(coordinator: &SelectionCoordinator) {
    coordinator.shutdown_token().cancel();
    tokio::time::timeout(WAIT, coordinator.close_and_join())
        .await
        .unwrap();
}
