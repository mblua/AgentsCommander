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
    let scoped = "if let Some(t) = ticket {
            t.succeeded();
        }";
    let spelling = if source.contains(success) {
        success
    } else if source.contains(scoped) {
        scoped
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
    super::support_core::fixture_scanner_absent_pty_leaves_hooks_empty();
}

#[test]
fn fixture_scanner_registers_route_readiness_and_real_injection() {
    super::support_core::fixture_scanner_registers_route_readiness_and_real_injection();
}

#[test]
fn fixture_ticket_settles_before_insertion_and_failure_inserts_nothing() {
    super::support_core::fixture_ticket_settles_before_insertion_and_failure_inserts_nothing();
}
