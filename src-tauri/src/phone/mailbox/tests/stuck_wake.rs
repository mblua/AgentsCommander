//! #3263 - an owned-replacement wake that cannot write input ends with a
//! counted error instead of holding its worker forever.
use super::support_core::make_mailbox_fixture_in;
use super::*;
use crate::phone::mailbox::handoff::is_permanent_delivery_error;
use crate::session::selection::{
    ReplacementFixtureAction, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    SelectionCoordinator,
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
async fn issue_3263_pre_publish_retained_then_second_wake_admitted() {
    const NAME: &str = concat!(
        "phone::mailbox::tests::stuck_wake::",
        "issue_3263_pre_publish_retained_then_second_wake_admitted"
    );
    if std::env::var_os(CHILD).is_none() {
        run_child(NAME).await;
        return;
    }
    let (resume, gate) = tokio::sync::oneshot::channel();
    let child = child_fixture(Some(gate)).await;
    let app = child.fixture.app.handle();
    let msg = wake_message_to_target();
    let cwd = child.fixture.target_cwd.clone();
    let mut away = cwd.clone().into_os_string();
    away.push(".away");
    let away = PathBuf::from(away);
    let (first, renamed) = tokio::join!(
        tokio::time::timeout(WAIT, child.poller.deliver_wake(app, &msg)),
        async {
            tokio::time::timeout(
                WAIT,
                retained_source_wait_cut(&child.control, Cut::SourceCoreEntered),
            )
            .await
            .expect("wake 1 reaches SourceCoreEntered");
            let renamed = std::fs::rename(&cwd, &away);
            let renamed = renamed.and_then(|()| std::fs::create_dir(&cwd));
            resume.send(()).unwrap();
            renamed
        }
    );
    renamed.expect("rename the target cwd away");
    let error = first.expect("wake 1 ends within 15 s").unwrap_err();
    assert!(error.starts_with("wake retained without input"), "{error}");
    assert!(error.contains("stage=source_invalid"), "{error}");
    assert!(error.contains("rollback=done"), "{error}");
    std::fs::remove_dir(&cwd).unwrap();
    std::fs::rename(&away, &cwd).unwrap();

    let (hold, gate) = tokio::sync::oneshot::channel::<()>();
    let fresh = fixture_control(Some(gate));
    let hooks = child.poller.test_hooks.as_ref().unwrap();
    *hooks.replacement_fixture.lock().unwrap() = Some(fresh.clone());
    let second = second_message();
    let reached = tokio::time::timeout(WAIT, async {
        tokio::select! {
            biased;
            _ = retained_source_wait_cut(&fresh, Cut::SourceCoreEntered) => (),
            result = child.poller.deliver_wake(app, &second) => {
                panic!("wake 2 ended before admission: {:?}", result.err())
            }
        }
    })
    .await;
    assert!(
        reached.is_ok(),
        "wake 2 must pass admission (released owner)"
    );
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
