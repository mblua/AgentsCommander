use super::{build_purge_wg_message, setup_purge_fixture};
use crate::phone::mailbox::notice::OutboxOrigin;
use crate::phone::mailbox::poller::{
    evaluate_gate, MailboxPoller, PurgeGatePeer, PurgeGateSession,
};
use crate::phone::mailbox::pty_claim::{MailboxTestEvent, MailboxTestHooks};
use crate::phone::mailbox::tests::{
    add_mailbox_session, app_handle, make_mailbox_app, MailboxMockPtyBackend,
};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;

struct RootChild {
    process: Child,
    directory: Option<tempfile::TempDir>,
    reaped: bool,
}

impl RootChild {
    fn fail(&mut self, reason: &str) -> ! {
        let directory = self.directory.take().unwrap().keep();
        panic!(
            "{reason}; child diagnostics retained at {}",
            directory.display()
        );
    }
}

impl Drop for RootChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }
}

fn run_child(name: &str, case: &str, root_absent: bool) {
    let name = name.strip_prefix("agentscommander_lib::").unwrap_or(name);
    let directory = tempfile::tempdir().unwrap();
    let (_, peer) = setup_purge_fixture(&directory, "dev-team");
    let config = peer.join("config");
    std::fs::create_dir(&config).unwrap();
    let root = config.join(crate::config::ROOT_AGENT_DIR_NAME);
    assert!(!root.exists());
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(directory.path().join("stdout.txt")).unwrap())
        .stderr(std::fs::File::create(directory.path().join("stderr.txt")).unwrap())
        .env("AGENTSCOMMANDER_CONFIG_DIR", &config)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &config)
        .env("AC_3318_PURGE_CHILD", case)
        .env("AC_3318_PURGE_FIXTURE", directory.path());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut owner = RootChild {
        process: command.spawn().unwrap(),
        directory: Some(directory),
        reaped: false,
    };
    let started = Instant::now();
    let status = loop {
        match owner.process.try_wait() {
            Ok(Some(status)) => {
                owner.reaped = true;
                break status;
            }
            Ok(None) if started.elapsed() < Duration::from_secs(120) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => owner.fail("child deadline"),
            Err(_) => owner.fail("child poll failed"),
        }
    };
    let stdout =
        std::fs::read_to_string(owner.directory.as_ref().unwrap().path().join("stdout.txt"))
            .unwrap();
    if !status.success()
        || !stdout.contains(&format!("AC_3318_PURGE_DONE {case}"))
        || !stdout.contains(&format!("test {name} ..."))
        || !stdout.contains("test result: ok. 1 passed; 0 failed; 0 ignored;")
        || (root_absent && root.exists())
    {
        owner.fail("child result, count, receipt or absent Root assertion failed");
    }
    eprintln!(
        "#3318 purge child case={case} elapsed={:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn h2_purge_root_protection() {
    let Ok(case) = std::env::var("AC_3318_PURGE_CHILD") else {
        for case in ["missing-root", "missing-cwd", "ordinary", "root", "flag"] {
            run_child(
                concat!(module_path!(), "::h2_purge_root_protection"),
                case,
                case == "missing-root",
            );
        }
        return;
    };
    let fixture = std::env::var("AC_3318_PURGE_FIXTURE").unwrap();
    let configured = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").unwrap();
    assert_eq!(crate::config::config_dir().unwrap(), Path::new(&configured));
    let root = Path::new(&configured).join(crate::config::ROOT_AGENT_DIR_NAME);
    assert!(!root.exists());
    if case != "missing-root" {
        std::fs::create_dir(&root).unwrap();
    }
    let room = Path::new(&fixture).join("proj-a/.ac/wg-1-dev-team");
    let sender_cwd = room.join("__agent_tech-lead");
    let cwd = room.join("__agent_dev-rust/ordinary");
    std::fs::create_dir(&cwd).unwrap();
    let target = if case == "root" { &root } else { &cwd };
    let app_struct = make_mailbox_app(Path::new(&fixture));
    let app = app_handle(&app_struct);
    let id = add_mailbox_session(
        &app,
        target,
        "wg-1-dev-team/dev-rust",
        SessionStatus::Idle,
        None,
    )
    .await;
    let sender_id = add_mailbox_session(
        &app,
        &sender_cwd,
        "wg-1-dev-team/tech-lead",
        SessionStatus::Idle,
        None,
    )
    .await;
    let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    manager
        .read()
        .await
        .set_is_root_agent(id, case == "flag")
        .await;
    let token = manager
        .read()
        .await
        .get_session(sender_id)
        .await
        .unwrap()
        .token
        .to_string();
    let pty = app.state::<Arc<Mutex<PtyManager>>>();
    {
        let pty = pty.lock().unwrap();
        pty.record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess);
        pty.backend_for_kind(crate::pty::backend::SessionBackendKind::LocalProcess)
            .as_any()
            .downcast_ref::<MailboxMockPtyBackend>()
            .unwrap()
            .set_live(id);
        assert!(pty.has_session(id));
    }
    let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
    idle.set_pty_input_ready_for_test(id);
    let snapshots = manager.read().await.list_sessions().await;
    let selected = crate::phone::mailbox::settle::filter_sessions_by_fqn(
        &snapshots,
        "proj-a:wg-1-dev-team/dev-rust",
    );
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, id.to_string());
    assert!(!matches!(selected[0].status, SessionStatus::Exited(_)));
    assert_eq!(selected[0].is_root_agent, case == "flag");
    let ready = evaluate_gate(
        &[PurgeGatePeer {
            fqn: "proj-a:wg-1-dev-team/dev-rust".into(),
            all_session_ids: vec![id.to_string()],
            live: vec![PurgeGateSession {
                session_id: id,
                mirror_idle: true,
                readiness: idle.purge_readiness(&[id])[0],
            }],
        }],
        Duration::from_millis(3000),
    );
    assert!(ready.passed, "positive control must reach the destroy loop");
    if case == "missing-cwd" {
        std::fs::remove_dir(&cwd).unwrap();
    }
    let request_id = "33180002-0000-4000-8000-000000000001";
    let path = build_purge_wg_message(
        &sender_cwd,
        "h2-purge",
        request_id,
        "proj-a:wg-1-dev-team/tech-lead",
        Some(&token),
        false,
        3000,
        None,
    );
    let hooks = MailboxTestHooks::default();
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .unwrap();
    let response = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("responses")
        .join(format!("{request_id}.json"));
    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(response).unwrap()).unwrap();
    let remaining = manager.read().await.get_session(id).await;
    if case == "ordinary" {
        // The pre-existing cfg(test) force_close_session branch removes the real manager
        // record. This proves purge's Guard B and destroy dispatch, not its transaction.
        assert_eq!(body["status"], "purged", "{body}");
        assert_eq!(body["purged"], 1);
        assert_eq!(body["failed"], 0);
        assert_eq!(body["already_closed"], 0);
        assert_eq!(body["peers"][0]["outcome"], "closed");
        assert_eq!(*hooks.destroy_calls.lock().unwrap(), vec![id]);
        assert!(hooks
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, MailboxTestEvent::Destroy(sid) if *sid == id)));
        assert!(remaining.is_none());
    } else {
        assert_eq!(body["status"], "failed_root_guard", "{body}");
        assert_eq!(body["offending_session_id"], id.to_string());
        assert!(hooks.destroy_calls.lock().unwrap().is_empty());
        assert!(!hooks
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, MailboxTestEvent::Destroy(sid) if *sid == id)));
        let remaining = remaining.unwrap();
        assert_eq!(remaining.is_root_agent, case == "flag");
        assert!(!matches!(remaining.status, SessionStatus::Exited(_)));
        assert!(pty.lock().unwrap().has_session(id));
    }
    if case == "missing-root" {
        assert!(!root.exists());
    }
    println!("AC_3318_PURGE_DONE {case}");
}
