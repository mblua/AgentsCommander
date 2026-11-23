//! #3263 - an owned-replacement wake that cannot write input ends with a
//! counted error instead of holding its worker forever.
use super::support_core::{make_mailbox_fixture_in, MailboxMockPtyBackend};
use super::*;
use crate::config::sessions_persistence::{classify_create_carrier_scope, CreateCarrierScope};
use crate::phone::mailbox::handoff::is_permanent_delivery_error;
use crate::phone::mailbox::pty_observation::PtyAuthorityState;
use crate::pty::backend::SessionBackendKind;
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::selection::{
    OwnedReplacementContinuation, ReplacementFixtureAction, ReplacementFixtureControl,
    ReplacementFixtureCut as Cut, SelectionCoordinator,
};

const CHILD: &str = "AC_3263_STUCK_WAKE_CHILD";
const WAIT: Duration = Duration::from_secs(15);

/// The fixture is unsafe in-process (#2854 stack, global `config_dir()`,
/// process-wide carrier protections), so every integration test runs in a child.
async fn run_child(name: &str) {
    let config = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("AGENTSCOMMANDER_CONFIG_DIR", config.path())
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", config.path())
        .env("RUST_MIN_STACK", "33554432")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().unwrap();
    use tokio::io::AsyncReadExt;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let err = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let status = tokio::time::timeout(Duration::from_secs(45), child.wait()).await;
    let timed_out = status.is_err();
    if timed_out {
        child.kill().await.unwrap();
    }
    let output = std::process::Output {
        status: if let Ok(status) = status {
            status.unwrap()
        } else {
            child.wait().await.unwrap()
        },
        stdout: out.await.unwrap(),
        stderr: err.await.unwrap(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!timed_out, "#3263 child timed out:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("test result: ok. 1 passed;"),
        "{stdout}\n{stderr}"
    );
    assert!(output.status.success(), "{stdout}\n{stderr}");
}

struct ChildFixture {
    fixture: MailboxFixture,
    coordinator: SelectionCoordinator,
    control: Arc<ReplacementFixtureControl>,
    poller: MailboxPoller,
}

fn fixture_control(
    pause: Option<tokio::sync::oneshot::Receiver<()>>,
) -> Arc<ReplacementFixtureControl> {
    let control = Arc::new(ReplacementFixtureControl::default());
    control.mock_backend.store(true, Ordering::Release);
    control.ticket_only.store(false, Ordering::Release);
    if let Some(resume) = pause {
        control
            .actions
            .lock()
            .unwrap()
            .push(ReplacementFixtureAction::Pause {
                cut: Cut::SourceCoreEntered,
                resume,
            });
    }
    control
}

async fn child_fixture(pause: Option<tokio::sync::oneshot::Receiver<()>>) -> ChildFixture {
    let fixture = make_mailbox_fixture_in(tempfile::tempdir().unwrap());
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    let control = fixture_control(pause);
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    ChildFixture {
        fixture,
        coordinator,
        control,
        poller,
    }
}

async fn close_child(coordinator: &SelectionCoordinator) {
    coordinator.shutdown_token().cancel();
    tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .unwrap();
}

fn second_message() -> OutboxMessage {
    let mut msg = wake_message_to_target();
    msg.id = "issue-3263-second".into();
    msg
}

#[tokio::test]
async fn issue_3263_owned_retained_wake_ends_with_counted_error() {
    const NAME: &str = concat!(
        "phone::mailbox::tests::stuck_wake::",
        "issue_3263_owned_retained_wake_ends_with_counted_error"
    );
    if std::env::var_os(CHILD).is_none() {
        run_child(NAME).await;
        return;
    }
    let child = child_fixture(None).await;
    let app = child.fixture.app.handle();
    set_mock_pty_write_failure(app, true);
    let msg = wake_message_to_target();
    let first = tokio::time::timeout(WAIT, child.poller.deliver_wake(app, &msg)).await;
    let error = first.expect("wake 1 ends within 15 s").unwrap_err();
    assert!(error.starts_with("wake retained without input"), "{error}");
    assert!(error.contains("stage=inject_failed"), "{error}");
    assert!(error.contains("rollback=none"), "{error}");
    assert!(!is_permanent_delivery_error(&error), "{error}");

    set_mock_pty_write_failure(app, false);
    let returned = tokio::time::timeout(
        WAIT,
        retained_source_wait_cut(&child.control, Cut::AdapterReturned),
    )
    .await
    .expect("replacement adapter returned");
    let id = returned.binding.expect("replacement binding").session_id();
    let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
    idle.register_session(id, crate::session::profile::IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(id);
    let hooks = child.poller.test_hooks.as_ref().unwrap();
    hooks.real_inject_sessions.lock().unwrap().insert(id);
    let second = second_message();
    let delivered = tokio::time::timeout(WAIT, child.poller.deliver_wake(app, &second))
        .await
        .expect("wake 2 ends within 15 s");
    assert!(delivered.is_ok(), "wake 2: {:?}", delivered.err());
    close_child(&child.coordinator).await;
}

#[tokio::test]
async fn issue_3263_pre_publish_refuses_replaced_cwd() {
    const NAME: &str = concat!(
        "phone::mailbox::tests::stuck_wake::",
        "issue_3263_pre_publish_refuses_replaced_cwd"
    );
    if std::env::var_os(CHILD).is_none() {
        run_child(NAME).await;
        return;
    }
    let (resume, gate) = tokio::sync::oneshot::channel();
    let child = child_fixture(Some(gate)).await;
    let app = child.fixture.app.handle();
    let msg = wake_message_to_target();
    let backend = app
        .pty_routes()
        .lock()
        .unwrap()
        .backend_for_kind(SessionBackendKind::LocalProcess);
    let backend = backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .unwrap();
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let cwd = &child.fixture.target_cwd;
    let away = cwd.with_extension("original");
    let (first, entered) = tokio::join!(
        tokio::time::timeout(WAIT, child.poller.deliver_wake(app, &msg)),
        async {
            let entered = tokio::time::timeout(
                WAIT,
                retained_source_wait_cut(&child.control, Cut::SourceCoreEntered),
            )
            .await
            .expect("wake 1 reaches SourceCoreEntered");
            std::fs::rename(cwd, &away).unwrap();
            std::fs::create_dir(cwd).unwrap();
            resume.send(()).unwrap();
            entered
        }
    );
    let scope = {
        let permit = acquire_carrier_mutation_permit().await;
        classify_create_carrier_scope(&permit, &msg.to, cwd)
    };
    std::fs::remove_dir(cwd).unwrap();
    std::fs::rename(&away, cwd).unwrap();

    let error = first.expect("wake 1 ends within 15 s").unwrap_err();
    assert_counted_source_error(&error);
    assert!(error.contains("rollback=refused"), "{error}");
    assert!(matches!(
        scope, CreateCarrierScope::Protected { preparing_owner: Some(owner), .. }
        if owner == entered.operation
    ));
    assert_eq!(child.coordinator.fixture_root_count(), Some(1));
    let owner = child.control.observe_owner(&child.coordinator).unwrap();
    assert!(!owner.cleanup_claimed && !owner.rollback_completed && !owner.terminal);
    let binding = entered.binding.expect("reserved B binding");
    let manager = app.pty_sessions().read().await.clone();
    assert!(manager.get_pending_session(binding).await.is_none());
    assert_eq!(backend.spawn_attempts(binding.session_id()), 0);
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
    assert!(!child.control.submitted.load(Ordering::Acquire));
    close_child(&child.coordinator).await;
}

fn assert_counted_source_error(error: &str) {
    assert!(error.starts_with("wake retained without input"), "{error}");
    assert!(error.contains("stage=source_invalid"), "{error}");
    assert!(!is_permanent_delivery_error(error), "{error}");
}

#[tokio::test]
async fn issue_3263_pre_publish_retained_then_second_wake_admitted() {
    const NAME: &str = concat!(
        "phone::mailbox::tests::stuck_wake::",
        "issue_3263_pre_publish_retained_then_second_wake_admitted"
    );
    if std::env::var_os(CHILD).is_none() {
        run_child(NAME).await;
        return;
    }
    let child = child_fixture(None).await;
    let app = child.fixture.app.handle();
    let msg = wake_message_to_target();
    let probe = OwnedReplacementContinuation::install_wake_rollback_fixture();
    probe.refuse_precreate_source.store(true, Ordering::Relaxed);
    let first = tokio::time::timeout(WAIT, child.poller.deliver_wake(app, &msg)).await;
    let error = first.expect("wake 1 ends within 15 s").unwrap_err();
    assert_counted_source_error(&error);
    assert!(error.contains("rollback=done"), "{error}");
    assert!(!probe.refuse_precreate_source.load(Ordering::Relaxed));
    assert_eq!(child.coordinator.fixture_root_count(), Some(0));
    let first = tokio::time::timeout(
        WAIT,
        retained_source_wait_cut(&child.control, Cut::SourceCoreEntered),
    )
    .await
    .expect("wake 1 reached SourceCoreEntered");

    let (hold, gate) = tokio::sync::oneshot::channel::<()>();
    let fresh = fixture_control(Some(gate));
    let hooks = child.poller.test_hooks.as_ref().unwrap();
    *hooks.replacement_fixture.lock().unwrap() = Some(fresh.clone());
    let second = second_message();
    let reached = tokio::time::timeout(WAIT, async {
        tokio::select! {
            biased;
            receipt = retained_source_wait_cut(&fresh, Cut::SourceCoreEntered) => receipt,
            result = child.poller.deliver_wake(app, &second) => {
                panic!("wake 2 ended before admission: {:?}", result.err())
            }
        }
    })
    .await;
    let admitted = reached.expect("wake 2 must pass admission (released owner)");
    assert_ne!(admitted.operation, first.operation);
    drop(hold);
    close_child(&child.coordinator).await;
}

#[test]
fn issue_3263_pre_publish_exits_roll_back() {
    let source = include_str!("../peer_fallback.rs");
    let start = source
        .find("async fn execute_owned_peer_replacement")
        .expect("owned core");
    let rest = &source[start + "async fn ".len()..];
    let end = rest.find("async fn ").unwrap_or(rest.len());
    let body: String = rest[..end]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(body.contains(r#"end_before_publish(continuation,&manager,pty,"source_invalid")"#));
    assert!(
        body.contains(r#"end_before_publish(continuation,&manager,pty,"context_not_prepared")"#)
    );
    assert!(body.contains(r#"retained("create_retained",None)"#));
    assert!(!body.contains(r#"end_before_publish(continuation,&manager,pty,"create_retained")"#));
}

#[test]
fn issue_3263_mailbox_host_create_uses_wake_variant() {
    const SRC: &str = include_str!("../../../commands/phone/mailbox.rs");
    let start = SRC
        .find("async fn create_session_inner<'a>(")
        .expect("MailboxHost create impl");
    let rest = &SRC[start + "async fn ".len()..];
    let end = rest.find("async fn ").unwrap_or(rest.len());
    let body: String = rest[..end]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(body.contains("crate::commands::session::create_session_inner_for_wake("));
    assert!(!body.contains("crate::commands::session::create_session_inner("));
}

#[test]
fn issue_3263_create_retained_error_is_counted() {
    use crate::session::create_contracts::ERR_WAKE_CREATE_RETAINED;
    assert!(!is_permanent_delivery_error(ERR_WAKE_CREATE_RETAINED));
}

#[tokio::test]
async fn issue_3263_stalled_wake_delivery_ends_at_deadline() {
    const NAME: &str = concat!(
        "phone::mailbox::tests::stuck_wake::",
        "issue_3263_stalled_wake_delivery_ends_at_deadline"
    );
    if std::env::var_os(CHILD).is_none() {
        run_child(NAME).await;
        return;
    }
    let child = child_fixture(None).await;
    let app = child.fixture.app.handle();
    let (release, gate) = tokio::sync::oneshot::channel();
    *child.control.before_release_gate.lock().unwrap() = Some(gate);
    let hooks = child.poller.test_hooks.as_ref().unwrap();
    *hooks.wake_delivery_deadline.lock().unwrap() = Some(Duration::from_secs(5));
    let msg = wake_message_to_target();
    let temp = tempfile::tempdir().unwrap();
    let outbox = temp.path().join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let source = outbox.join(format!("{}.json", msg.id));
    std::fs::write(&source, serde_json::to_vec(&msg).unwrap()).unwrap();
    let claim = crate::phone::mailbox::handoff::wake_claim_path(&source);
    std::fs::rename(&source, &claim).unwrap();
    let validated = crate::phone::mailbox::poller::ValidatedWake {
        message: msg,
        response: None,
    };
    let (delivered, stalled) = tokio::join!(
        tokio::time::timeout(
            WAIT,
            crate::phone::mailbox::poller::deliver_claimed_wake(
                &child.poller,
                app,
                &claim,
                &source,
                &validated,
            ),
        ),
        tokio::time::timeout(
            WAIT,
            retained_source_wait_cut(&child.control, Cut::AdapterReturned)
        )
    );
    stalled.expect("owned core reaches AdapterReturned");
    let error = delivered.expect("delivery ends within 15 s").unwrap_err();
    assert!(error.starts_with("wake delivery timed out"), "{error}");
    assert!(error.contains("stage=deadline"), "{error}");
    assert!(source.exists() && !claim.exists());
    let cuts: Vec<Cut> = child
        .control
        .receipts
        .lock()
        .unwrap()
        .iter()
        .map(|receipt| receipt.cut)
        .collect();
    assert!(!cuts.contains(&Cut::SourceRetainedReturn), "{cuts:?}");
    assert!(!cuts.contains(&Cut::AfterReleaseBeforeSend), "{cuts:?}");
    release.send(()).unwrap();
    close_child(&child.coordinator).await;
}
