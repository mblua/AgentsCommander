use super::*;

// ── T15: interpret_self_restart_response_exit_code (mirrors self_clear's table) ──

#[test]
fn queued_status_returns_zero() {
    let resp = include_str!("tests/fixtures/queued.json");
    assert_eq!(interpret_self_restart_response_exit_code(resp), 0);
}

#[test]
fn already_queued_status_returns_zero() {
    let resp = include_str!("tests/fixtures/already_queued.json");
    assert_eq!(interpret_self_restart_response_exit_code(resp), 0);
}

#[test]
fn unknown_status_returns_two() {
    let resp = r#"{"status":"weird_new_state","action":"self-handoff-and-restart"}"#;
    assert_eq!(interpret_self_restart_response_exit_code(resp), 2);
}

#[test]
fn unparseable_json_returns_two() {
    assert_eq!(interpret_self_restart_response_exit_code("not json"), 2);
    assert_eq!(interpret_self_restart_response_exit_code(""), 2);
    assert_eq!(interpret_self_restart_response_exit_code("{partial"), 2);
}

#[test]
fn missing_status_field_returns_two() {
    let resp = r#"{"action":"self-handoff-and-restart","session_id":"s"}"#;
    assert_eq!(interpret_self_restart_response_exit_code(resp), 2);
}

#[test]
fn non_string_status_returns_two() {
    let resp = r#"{"status":42,"action":"self-handoff-and-restart"}"#;
    assert_eq!(interpret_self_restart_response_exit_code(resp), 2);
}

/// Valid JSON that is not an object (array, scalar, null) still fails the
/// `.get("status")` lookup and must return exit 2.
#[test]
fn non_object_json_returns_two() {
    assert_eq!(interpret_self_restart_response_exit_code("null"), 2);
    assert_eq!(interpret_self_restart_response_exit_code("[1,2,3]"), 2);
    assert_eq!(interpret_self_restart_response_exit_code("\"queued\""), 2);
    assert_eq!(interpret_self_restart_response_exit_code("42"), 2);
    assert_eq!(interpret_self_restart_response_exit_code("true"), 2);
}

// ── T16: clap parse smoke ──

#[test]
fn self_restart_args_parse_with_default_timeout() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "self-handoff-and-restart",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--root",
        "anything",
    ])
    .expect("clap should accept self-handoff-and-restart with token + root");
    let cmd = parsed.command.expect("subcommand present");
    match cmd {
        crate::cli::Commands::SelfRestart(args) => {
            assert_eq!(args.timeout, 15, "--timeout must default to 15");
            assert_eq!(
                args.token.as_deref(),
                Some("11111111-1111-1111-1111-111111111111")
            );
            assert_eq!(args.root.as_deref(), Some("anything"));
        }
        _ => panic!("expected SelfRestart subcommand"),
    }
}

#[test]
fn self_restart_args_parse_explicit_timeout() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "self-handoff-and-restart",
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
        crate::cli::Commands::SelfRestart(args) => assert_eq!(args.timeout, 42),
        _ => panic!("expected SelfRestart subcommand"),
    }
}

/// The verb ships under its full name only. A bare `self-restart` must be REJECTED so a
/// guessed invocation fails loudly rather than silently doing nothing.
#[test]
fn bare_self_restart_subcommand_name_is_rejected() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "self-restart",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--root",
        "anything",
    ]);
    assert!(
        parsed.is_err(),
        "the bare `self-restart` name must be rejected; the verb is `self-handoff-and-restart`"
    );
}

// ── T17: help content, including the two shipped-string prohibitions ──

/// The positive needles are checked against a whitespace-normalized rendering, because
/// clap re-wraps `after_help` to the render width and a wrap landing inside a multi-word
/// needle would make the assertion fail for a reason that has nothing to do with the
/// prose. The negative needles are checked against BOTH, since a prohibited substring
/// must be absent however the text is folded.
#[test]
fn self_restart_help_documents_scope_recipe_and_no_enter_caveat() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let mut subcommand = cmd
        .get_subcommands()
        .find(|cmd| cmd.get_name() == "self-handoff-and-restart")
        .expect("self-handoff-and-restart subcommand")
        .clone();
    let raw = subcommand.render_long_help().to_string();
    let help = raw.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(help.contains("SELF-HANDOFF.md"), "{raw}");
    assert!(
        help.contains("self-clear/<timestamp>_SELF-HANDOFF.md"),
        "{raw}"
    );
    assert!(help.contains("Root Agent"), "{raw}");
    assert!(help.contains("same coding agent"), "{raw}");
    assert!(help.contains("Room replica"), "{raw}");
    assert!(help.contains("continuously idle for 30s"), "{raw}");
    assert!(
        help.contains("does not change your Selection-UI coding-agent or profile assignment"),
        "{raw}"
    );
    // D1's no-Enter caveat: on a shell that is not a direct coding-agent CLI the resume
    // prompt is delivered, not submitted.
    assert!(help.contains("WITHOUT an appended Enter"), "{raw}");
    assert!(help.contains("delivered but not submitted"), "{raw}");

    // Round-2 BL1 / R2: both of these claims are FALSE and must never ship in a
    // user-facing string. Every restart rewrites tooling.lastCodingAgent, and the respawn
    // is rebuilt from current settings rather than replayed from a stored argv.
    for prohibited in ["no settings", "writes no settings", "same parameters"] {
        assert!(!raw.contains(prohibited), "{prohibited}: {raw}");
        assert!(!help.contains(prohibited), "{prohibited}: {help}");
    }
    // The repository writes prose without em-dashes, and this help says "Room replica".
    assert!(!raw.contains('\u{2014}'), "{raw}");
    for needle in ["workgroup", "Workgroup", "WORKGROUP"] {
        assert!(!raw.contains(needle), "{needle}: {raw}");
    }
}

#[test]
fn self_restart_queued_status_is_honest_about_the_recipe_and_the_selection() {
    let status = self_restart_queued_status(30);
    assert!(status.contains("continuously idle for 30s"), "{status}");
    assert!(
        status.contains("same configured coding agent and the same profile letter"),
        "{status}"
    );
    assert!(!status.contains("no settings"), "{status}");
    assert!(!status.contains("same parameters"), "{status}");
    assert!(!status.contains('\u{2014}'), "{status}");
}

// ── T18: sender derivation, asserted from the restart module ──

/// The Root Agent's self-handoff-and-restart must send as the reserved
/// `ROOT_AGENT_SENDER`, NOT the path-derived FQN. Asserted from THIS module so a future
/// divergence on the restart path surfaces here. The canonical Root directory must
/// exist so its identity can be verified.
///
/// T18 alone is NOT sufficient for the Root Agent scope: it asserts the helper in
/// isolation and nothing in it proves the restart path calls it. T26 below closes the
/// builder-to-helper link; the two Root `process_message` e2es in `phone/mailbox.rs` close
/// the "a correctly stamped message survives the daemon's anti-spoof gates" half.
#[test]
fn self_restart_sender_for_root_agent_uses_reserved_constant() {
    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    std::fs::create_dir_all(&root).expect("Root fixture creation succeeds");
    assert_eq!(
        crate::cli::self_clear::resolve_self_clear_sender(&root),
        crate::config::root_agent::ROOT_AGENT_SENDER,
        "Root self-handoff-and-restart must send as ROOT_AGENT_SENDER so the daemon anti-spoof \
        accepts it"
    );
}

// ── T26: the builder stamps the canonical sender, the action, and an empty `to` ──

#[test]
fn build_self_restart_outbox_message_stamps_canonical_root_sender() {
    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    std::fs::create_dir_all(&root).expect("Root fixture creation succeeds");
    let root_msg =
        build_self_restart_outbox_message(&root, Some("tok".into()), "m".into(), "r".into());
    assert_eq!(
        root_msg.from,
        crate::config::root_agent::ROOT_AGENT_SENDER,
        "the BUILDER itself must stamp ROOT_AGENT_SENDER for the Root cwd"
    );
    assert_eq!(
        root_msg.action.as_deref(),
        Some(crate::phone::mailbox::SELF_RESTART_ACTION)
    );
    assert!(root_msg.to.is_empty());

    // Over-reach guard: a builder that returned the reserved constant unconditionally
    // would pass the row above. A non-Root agent must still get the path FQN.
    let replica_msg = build_self_restart_outbox_message(
        "C:/proj/.ac/wg-1-team/__agent_dev-rust",
        None,
        "m2".into(),
        "r2".into(),
    );
    assert_eq!(replica_msg.from, "proj:wg-1-team/dev-rust");
    assert_ne!(
        replica_msg.from,
        crate::config::root_agent::ROOT_AGENT_SENDER
    );
    assert_eq!(
        replica_msg.action.as_deref(),
        Some(crate::phone::mailbox::SELF_RESTART_ACTION)
    );
    assert!(replica_msg.to.is_empty());
}
