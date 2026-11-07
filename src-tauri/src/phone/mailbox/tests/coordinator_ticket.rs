use crate::phone::mailbox::pty_claim::MailboxTestHooks;
use crate::phone::mailbox::tests::{make_mailbox_app, CodexClearScannerFixture};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

fn fixture_code(source: &str) -> String {
    rust_source_scrub::scrub(source, rust_source_scrub::Literals::Drop)
        .expect("fixture source must lex")
        .split_whitespace()
        .collect::<String>()
        .replace(
            "crate::commands::session::note_coordinator_create_on_app(app,&cwd)",
            "app.begin_coordinator_create(&cwd)",
        )
}

fn ticket_order(source: &str) -> Result<(), &'static str> {
    let code = fixture_code(source);
    let begin = code
        .find("app.begin_coordinator_create(&cwd)")
        .ok_or("begin")?;
    let error = code[begin..]
        .find("hooks.internal_spawn_error")
        .ok_or("error")?
        + begin;
    let failed = code[error..].find("t.failed();").ok_or("failed")? + error;
    let returned = code[failed..].find("returnErr(msg);").ok_or("return")? + failed;
    let succeeded = code[returned..].find("t.succeeded();").ok_or("succeeded")? + returned;
    let insert = code[begin..].find(".create_session(").ok_or("insert")? + begin;
    if !(begin < error
        && error < failed
        && failed < returned
        && returned < succeeded
        && succeeded < insert)
    {
        return Err("ticket before insert");
    }
    Ok(())
}

fn late_ticket_mutant(source: &str) -> String {
    let success = "if let Some(t) = ticket {\n                t.succeeded();\n            }";
    let compact = "if let Some(t) = ticket {\n        t.succeeded();\n    }";
    let spelling = if source.contains(success) {
        success
    } else {
        compact
    };
    assert!(source.contains(spelling), "success block exists");
    let without = source.replacen(spelling, "", 1);
    let at = without
        .find("mgr.rename_session")
        .expect("insertion completed");
    let mut mutant = without;
    mutant.insert_str(at, spelling);
    mutant
}

#[test]
fn issue_3946_fixture_order() {
    let source = include_str!("../wake_injection/fixture.rs");
    assert_eq!(ticket_order(source), Ok(()));
    assert_eq!(
        ticket_order(&late_ticket_mutant(source)),
        Err("ticket before insert")
    );
    let scanner = fixture_code(include_str!("support_core.rs"));
    let scanner = scanner
        .split_once("fnprepare_spawned_clear_session<")
        .unwrap()
        .1;
    let scanner = scanner
        .split_once("pub(crate)asyncfnreal_session")
        .unwrap()
        .0;
    let mut rest = scanner;
    for witness in [
        "app.pty_routes_opt()else{returnOk(());}",
        "letmanager=pty.lock()",
        "downcast_ref::<MailboxMockPtyBackend>()",
        "backend.prepare_clear.load",
        "backend.rows.lock()",
        "backend.set_live(id)",
        ".try_record_route(id,SessionBackendKind::LocalProcess)",
        "app.pty_readiness()",
        "idle.register_session(id,",
        "idle.set_pty_input_ready_for_test(id)",
        "hooks.pty_presence.lock()",
        "hooks.real_inject_sessions.lock()",
        "Ok(())",
    ] {
        rest = rest.split_once(witness).expect(witness).1;
    }
    assert!(
        !scanner.contains("drop(manager)"),
        "manager held through real inject"
    );
    assert!(scanner.contains("crate::session::profile::IdleTuning::DEFAULT"));
}

#[test]
fn issue_3946_fixture_order_base() {
    let Some(dir) = std::env::var_os("ISSUE_3946_BASE_DIR") else {
        println!("ISSUE3946 P14 BASE_NOT_REQUESTED");
        return;
    };
    let source = std::fs::read_to_string(
        std::path::PathBuf::from(dir).join("phone/mailbox/wake_injection.rs"),
    )
    .expect("exported pinned fixture source");
    assert_eq!(ticket_order(&source), Ok(()));
    assert_eq!(
        ticket_order(&late_ticket_mutant(&source)),
        Err("ticket before insert")
    );
    println!("ISSUE3946 P14 BASE_PASS 1");
    println!("ISSUE3946 P14 MUTANT_REJECTED 1");
}

#[test]
fn fixture_scanner_absent_pty_leaves_hooks_empty() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let hooks = MailboxTestHooks::default();
    CodexClearScannerFixture::prepare_spawned_clear_session(app.handle(), &hooks, Uuid::nil())
        .unwrap();
    assert!(hooks.pty_presence.lock().unwrap().is_empty());
    assert!(hooks.real_inject_sessions.lock().unwrap().is_empty());
}

#[test]
fn fixture_scanner_registers_route_readiness_and_real_injection() {
    let root = tempfile::tempdir().unwrap();
    let app = make_mailbox_app(root.path());
    let routes = app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>();
    let manager = routes.lock().unwrap();
    let backend = manager.backend_for_kind(crate::pty::backend::SessionBackendKind::LocalProcess);
    let backend = backend
        .as_any()
        .downcast_ref::<super::MailboxMockPtyBackend>()
        .unwrap();
    backend
        .prepare_clear
        .store(true, std::sync::atomic::Ordering::SeqCst);
    drop(manager);
    let hooks = MailboxTestHooks::default();
    let id = Uuid::new_v4();
    CodexClearScannerFixture::prepare_spawned_clear_session(app.handle(), &hooks, id).unwrap();
    assert_eq!(hooks.pty_presence.lock().unwrap().get(&id), Some(&true));
    assert!(hooks.real_inject_sessions.lock().unwrap().contains(&id));
    let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
    assert!(idle.purge_readiness(&[id])[0].watcher_idle);
}

#[test]
fn fixture_ticket_settles_before_insertion_and_failure_inserts_nothing() {
    let root = tempfile::tempdir().unwrap();
    let app = make_mailbox_app(root.path());
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(Default::default()));
    app.manage(clocks.clone());
    let cwd = "C:/ac-test/project/.ac/wg-2413-dev-team/__agent_tech-lead";
    let fqn = crate::config::teams::agent_fqn_from_path(cwd);
    let stamp = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let sessions =
            app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let resolved = crate::phone::mailbox::notice::ResolvedWakeAgentCommand {
            shell: "codex".into(),
            shell_args: vec![],
            agent_id: Some("codex".into()),
            agent_label: None,
            source: "fixture".into(),
            raw_command: "codex".into(),
        };
        let message = super::root_outbox_message("ticket".into(), None);
        let hooks = MailboxTestHooks::default();
        *hooks.spawn_is_coordinator.lock().unwrap() = true;
        let poller =
            crate::phone::mailbox::poller::MailboxPoller::new_with_test_hooks(hooks.clone());
        for fail in [true, false] {
            assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, stamp));
            if fail {
                *hooks.internal_spawn_error.lock().unwrap() = Some("injected create error".into());
            }
            let guard = sessions.write().await;
            let create = poller.spawn_wake_session(
                app.handle(),
                &message,
                &resolved,
                cwd.into(),
                "ticket".into(),
                false,
                false,
                "codex".into(),
                vec![],
                None,
                None,
                None,
            );
            tokio::pin!(create);
            if fail {
                assert_eq!(create.await.unwrap_err(), "injected create error");
                assert!(guard.list_sessions().await.is_empty());
                assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), Some(stamp));
            } else {
                assert!(futures::poll!(&mut create).is_pending());
                // Creation is blocked on this write guard: settlement already happened.
                assert!(guard.list_sessions().await.is_empty());
                assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
                assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
                drop(guard);
                create.await.unwrap();
                assert_eq!(sessions.read().await.list_sessions().await.len(), 1);
            }
        }
    });
}
