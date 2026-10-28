use super::*;

// ── interpret_self_clear_response_exit_code (mirror close-session's table) ──

#[test]
fn queued_status_returns_zero() {
    let resp = include_str!("tests/fixtures/queued.json");
    assert_eq!(interpret_self_clear_response_exit_code(resp), 0);
}

#[test]
fn already_queued_status_returns_zero() {
    let resp = include_str!("tests/fixtures/already_queued.json");
    assert_eq!(interpret_self_clear_response_exit_code(resp), 0);
}

#[test]
fn unknown_status_returns_two() {
    let resp = r#"{"status":"weird_new_state","action":"self-handoff-and-clear"}"#;
    assert_eq!(interpret_self_clear_response_exit_code(resp), 2);
}

#[test]
fn unparseable_json_returns_two() {
    assert_eq!(interpret_self_clear_response_exit_code("not json"), 2);
    assert_eq!(interpret_self_clear_response_exit_code(""), 2);
    assert_eq!(interpret_self_clear_response_exit_code("{partial"), 2);
}

#[test]
fn missing_status_field_returns_two() {
    let resp = r#"{"action":"self-handoff-and-clear","session_id":"s"}"#;
    assert_eq!(interpret_self_clear_response_exit_code(resp), 2);
}

#[test]
fn non_string_status_returns_two() {
    let resp = r#"{"status":42,"action":"self-handoff-and-clear"}"#;
    assert_eq!(interpret_self_clear_response_exit_code(resp), 2);
}

/// Valid JSON that is not an object (array, scalar, null) still fails the
/// `.get("status")` lookup and must return exit 2.
#[test]
fn non_object_json_returns_two() {
    assert_eq!(interpret_self_clear_response_exit_code("null"), 2);
    assert_eq!(interpret_self_clear_response_exit_code("[1,2,3]"), 2);
    assert_eq!(interpret_self_clear_response_exit_code("\"queued\""), 2);
    assert_eq!(interpret_self_clear_response_exit_code("42"), 2);
    assert_eq!(interpret_self_clear_response_exit_code("true"), 2);
}

// ── clap parse smoke ──

#[test]
fn self_clear_args_parse_with_default_timeout() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "self-handoff-and-clear",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--root",
        "anything",
    ])
    .expect("clap should accept self-handoff-and-clear with token + root");
    let cmd = parsed.command.expect("subcommand present");
    match cmd {
        crate::cli::Commands::SelfClear(args) => {
            assert_eq!(args.timeout, 15, "--timeout must default to 15");
            assert_eq!(
                args.token.as_deref(),
                Some("11111111-1111-1111-1111-111111111111")
            );
            assert_eq!(args.root.as_deref(), Some("anything"));
        }
        _ => panic!("expected SelfClear subcommand"),
    }
}

#[test]
fn self_clear_args_parse_explicit_timeout() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "self-handoff-and-clear",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--root",
        "anything",
        "--timeout",
        "42",
    ])
    .expect("clap should accept explicit --timeout");
    let cmd = parsed.command.expect("subcommand present");
    match cmd {
        crate::cli::Commands::SelfClear(args) => assert_eq!(args.timeout, 42),
        _ => panic!("expected SelfClear subcommand"),
    }
}

/// Plain self-clear and the existing handoff command coexist with distinct actions.
#[test]
fn plain_and_legacy_clear_names_parse_as_distinct_variants() {
    use clap::Parser;
    for (name, plain) in [("self-clear", true), ("self-handoff-and-clear", false)] {
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            name,
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
        ])
        .unwrap();
        assert!(matches!(
            (parsed.command.unwrap(), plain),
            (crate::cli::Commands::PlainSelfClear(_), true)
                | (crate::cli::Commands::SelfClear(_), false)
        ));
    }
    assert_eq!(crate::phone::mailbox::PLAIN_SELF_CLEAR_ACTION, "self-clear");
    assert_eq!(
        crate::phone::mailbox::SELF_CLEAR_ACTION,
        "self-handoff-and-clear"
    );
}

#[test]
fn self_clear_help_documents_provider_resolved_clear() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let mut subcommand = cmd
        .get_subcommands()
        .find(|cmd| cmd.get_name() == "self-handoff-and-clear")
        .expect("self-handoff-and-clear subcommand")
        .clone();
    let help = subcommand.render_long_help().to_string();

    assert!(help.contains("provider-resolved clear text"), "{help}");
    assert!(help.contains("/new"), "{help}");
    assert!(help.contains("/clear"), "{help}");
    assert!(help.contains("exact-stem direct Pi shell"), "{help}");
    assert!(help.contains("continuously idle"), "{help}");
    assert!(help.contains("Outer cmd / pwsh wrappers"), "{help}");
    assert!(!help.contains("then injects /clear."), "{help}");
}

#[test]
fn self_clear_queued_status_documents_provider_resolved_clear() {
    let status = self_clear_queued_status(30);
    assert!(status.contains("provider-resolved clear text"), "{status}");
    assert!(status.contains("/new"), "{status}");
    assert!(status.contains("/clear"), "{status}");
    assert!(status.contains("exact-stem direct Pi shell"), "{status}");
    assert!(status.contains("continuously idle for 30s"), "{status}");
    assert!(!status.contains("Phase 1 injects /clear"), "{status}");
}

#[test]
fn self_clear_help_documents_forgotten_summary_behavior() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let mut subcommand = cmd
        .get_subcommands()
        .find(|cmd| cmd.get_name() == "self-handoff-and-clear")
        .expect("self-handoff-and-clear subcommand")
        .clone();
    let help = subcommand.render_long_help().to_string();
    let normalized = help.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(normalized.contains("following the self-handoff skill"));
    assert!(normalized.contains("If SELF-HANDOFF.md is missing"));
    assert!(!normalized.contains("who you are"));
    assert!(!normalized.contains("ZERO memory"));

    assert!(help.contains("SELF-FORGET.md"), "{help}");
    assert!(help.contains("240"), "{help}");
    assert!(help.contains("closed background"), "{help}");
    assert!(help.contains("SELF-HANDOFF.md"), "{help}");
    // #749 - the help documents the pre-inject handoff archive and its exact destination.
    assert!(
        help.contains("self-clear/<timestamp>_SELF-HANDOFF.md"),
        "{help}"
    );
}

// ── #617 HIGH-1: Root self-clear sender derivation (the actual fix) ──

/// The Root Agent's self-clear must send as the reserved `ROOT_AGENT_SENDER`,
/// NOT the path-derived FQN. This is the discriminating gate for the fix: on the
/// pre-fix code (`agent_name_from_root` on the Root cwd) this returns a path-like
/// FQN and the assertion fails; with `sender_for_root` it returns the reserved
/// constant. The canonical Root directory must exist so its identity can be verified.
#[test]
fn self_clear_sender_for_root_agent_uses_reserved_constant() {
    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    std::fs::create_dir_all(&root).expect("Root fixture creation succeeds");
    assert_eq!(
        resolve_self_clear_sender(&root),
        crate::config::root_agent::ROOT_AGENT_SENDER,
        "Root self-clear must send as ROOT_AGENT_SENDER so the daemon anti-spoof accepts it"
    );
}

/// A non-Root agent's self-clear is unaffected by the fix: it still resolves to
/// the path FQN, never the reserved Root URI. Guards against over-reach.
#[test]
fn self_clear_sender_for_non_root_is_path_fqn() {
    let sender = resolve_self_clear_sender("C:/proj/.ac/wg-1-team/__agent_dev-rust");
    assert_ne!(sender, crate::config::root_agent::ROOT_AGENT_SENDER);
    assert_eq!(sender, "proj:wg-1-team/dev-rust");
}
