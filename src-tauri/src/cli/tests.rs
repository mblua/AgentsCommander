use super::*;

#[test]
fn validate_cli_token_does_not_echo_invalid_input() {
    let supplied = "super-secret-token-with-hidden-garbage";
    let err = validate_cli_token(&Some(supplied.to_string())).unwrap_err();

    assert!(err.contains("invalid token supplied"));
    assert!(!err.contains(supplied));
    assert!(!err.contains(&supplied[..8]));
    let legacy_phrase = ["Session", "Credentials"].join(" ");
    assert!(!err.contains(&legacy_phrase));
    assert!(!err.to_ascii_lowercase().contains("fallback"));
    assert!(!err.to_ascii_lowercase().contains("visible"));
}

#[test]
fn validate_cli_token_missing_token_documents_env_only() {
    let err = validate_cli_token(&None).unwrap_err();

    assert!(err.contains("AGENTSCOMMANDER_TOKEN"));
    assert!(err.contains("restart or respawn"));
    let legacy_phrase = ["Session", "Credentials"].join(" ");
    assert!(!err.contains(&legacy_phrase));
    assert!(!err.to_ascii_lowercase().contains("fallback"));
    assert!(!err.to_ascii_lowercase().contains("visible"));
}

#[test]
fn cli_after_help_documents_token_validation_model() {
    use clap::CommandFactory;
    let cmd = Cli::command();
    let after = cmd
        .get_after_help()
        .expect("Cli after_help should be present")
        .to_string();
    assert!(
        after.contains("TOKEN VALIDATION MODEL"),
        "after_help missing TOKEN VALIDATION MODEL block: {}",
        after
    );
}

/// #654: internal `role-experiment` + the test/automation verbs and the
/// test-only top-level flags must NOT clutter `--help`. `hide = true` is a
/// display-only attribute, so each must remain a registered subcommand/arg
/// (the test harness still invokes them by name).
#[test]
fn internal_verbs_and_test_flags_are_hidden_from_help() {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    let help = cmd.render_long_help().to_string();

    let hidden_verbs = [
        "role-experiment",
        "test-reset",
        "window-info",
        "ui-query",
        "ui-terminal",
        "ui-pointer",
        "ui-key",
        "ui-click",
        "ui-context-click",
        "ui-hover",
        "ui-set",
        "ui-type",
        "ui-backend",
        "ui-wait",
    ];
    for name in hidden_verbs {
        assert!(
            !help.contains(name),
            "hidden verb `{}` must not appear in --help:\n{}",
            name,
            help
        );
        let sub = cmd
            .get_subcommands()
            .find(|c| c.get_name() == name)
            .unwrap_or_else(|| panic!("`{}` must still be a registered subcommand", name));
        assert!(sub.is_hide_set(), "`{}` should be marked hidden", name);
    }

    // #944 - the array above is a manual allowlist. A new ui-* verb that forgets
    // BOTH `#[command(hide = true)]` and its entry here would ship visible in
    // `--help` with no test failing. Make the invariant structural: every ui-*
    // subcommand is internal, no exceptions, no maintenance.
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        if name.starts_with("ui-") {
            assert!(
                sub.is_hide_set(),
                "`{name}` is a ui-* automation verb and must be hidden from --help"
            );
        }
    }

    for flag in [
        "--window-x",
        "--window-y",
        "--window-width",
        "--window-height",
        "--window-maximized",
        "--ui-automation",
    ] {
        assert!(
            !help.contains(flag),
            "hidden flag `{}` must not appear in --help:\n{}",
            flag,
            help
        );
    }

    // Sanity: a normal public verb is still listed.
    assert!(
        help.contains("send"),
        "public verbs should still appear in --help:\n{}",
        help
    );
}

#[test]
fn hidden_internal_verbs_still_parse_by_name() {
    use clap::Parser;
    let parsed = Cli::try_parse_from(["agentscommander", "window-info"])
        .expect("hidden `window-info` verb must still parse by name");
    assert!(
        matches!(parsed.command, Some(Commands::WindowInfo(_))),
        "expected WindowInfo subcommand"
    );

    let parsed = Cli::try_parse_from([
        "agentscommander",
        "ui-terminal",
        "--session-id",
        "session-a",
        "top",
    ])
    .expect("hidden `ui-terminal` verb must still parse by name");
    assert!(
        matches!(parsed.command, Some(Commands::UiTerminal(_))),
        "expected UiTerminal subcommand"
    );
}

#[test]
fn hidden_test_only_flags_still_parse() {
    use clap::Parser;
    let parsed = Cli::try_parse_from([
        "agentscommander",
        "--window-x",
        "100",
        "--window-y",
        "200",
        "--window-width",
        "800",
        "--window-height",
        "600",
        "--window-maximized",
        "--ui-automation",
    ])
    .expect("hidden test-only flags must still parse");
    assert_eq!(parsed.window_x, Some(100.0));
    assert_eq!(parsed.window_y, Some(200.0));
    assert_eq!(parsed.window_width, Some(800.0));
    assert_eq!(parsed.window_height, Some(600.0));
    assert!(parsed.window_maximized);
    assert!(parsed.ui_automation);
    assert!(parsed.command.is_none());
}

/// #1614 AC5 / D21: no retired token in ANY printed CLI help, complete by
/// construction rather than by enumeration.
///
/// Round 1 enumerated 20 subcommands by hand and the list was missing the
/// only two commands that print the two defects round 1 shipped:
/// `team create` (the only command that prints team.rs's third `help`
/// string) and `role-experiment` (the only command that prints the
/// `--retain-room` value placeholder). An enumeration cannot gate a defect
/// class whose failure mode is omission, so this walks clap's own tree.
///
/// It needs no built binary and no process spawn, so it runs under
/// `cargo test`. It runs on the `rust-regression-windows` leg only,
/// because that is the only CI leg that runs the Rust test suite.
#[test]
fn no_clap_printed_help_carries_the_retired_token() {
    use clap::CommandFactory;
    let mut root = Cli::command();
    root.build();
    // The root command name is the package name, `agentscommander`,
    // because `Cli` sets no `#[command(name = ...)]`. Take it before the
    // walk consumes `root`, and derive every lookup key from it rather than
    // hard-coding it.
    let root_name = root.get_name().to_string();
    assert_eq!(
        root_name, "agentscommander",
        "clap's root command name is the Cargo package name"
    );
    // Carry the invocation path, so an assertion can name a leaf
    // unambiguously ("create" alone is a name three different parents use).
    let mut stack = vec![(String::new(), root)];
    let mut walked: std::collections::BTreeMap<String, String> = Default::default();
    while let Some((prefix, mut cmd)) = stack.pop() {
        let path = if prefix.is_empty() {
            cmd.get_name().to_string()
        } else {
            format!("{prefix} {}", cmd.get_name())
        };
        for sub in cmd.get_subcommands().cloned() {
            stack.push((path.clone(), sub));
        }
        let help = cmd.render_long_help().to_string();
        for needle in ["workgroup", "Workgroup", "WORKGROUP"] {
            assert!(!help.contains(needle), "{path}: {needle}");
        }
        for token in help.split(|c: char| !c.is_ascii_alphanumeric()) {
            assert_ne!(token, "WG", "{path}");
        }
        walked.insert(path, help);
    }

    // The real guard: the two omissions that caused G5 must be reachable,
    // and the depth-2 one must have been RENDERED, not merely listed by its
    // parent. A `!contains` assertion over a set that was never populated is
    // vacuously true, which is why membership is asserted separately.
    let team_create = walked
        .get(&format!("{root_name} team create"))
        .expect("`<root> team create` is not in the walked set");
    assert!(
        team_create.contains("Define a repo available to the team when rooms are created"),
        "team create help did not render team.rs's third help string"
    );
    // The hidden parent must be walked at all (`hide = true` suppresses
    // LISTING, not membership)...
    assert!(
        walked.contains_key(&format!("{root_name} role-experiment")),
        "`<root> role-experiment` is not in the walked set"
    );
    // ...and the corrected placeholder renders on `role-experiment run`,
    // which is the command that actually owns `RunArgs`. `role-experiment`
    // is a PARENT command whose help renders only `<COMMAND>`, so asserting
    // the placeholder on the parent panics on a CORRECT implementation.
    // Plan section 9.4 AC5 targets `role-experiment run` for exactly this
    // reason (round 5 corrected it; section 14 item 5 carries the same
    // correction), and the round-3 probe crate could not have caught it
    // because it was a synthetic flat tree with no parent/child argument
    // split.
    let role_exp_run = walked
        .get(&format!("{root_name} role-experiment run"))
        .expect("`<root> role-experiment run` is not in the walked set");
    assert!(
        role_exp_run.contains("--retain-room <RETAIN_ROOM>"),
        "role-experiment run help did not render the corrected value placeholder"
    );
    assert!(
        walked.contains_key(&format!("{root_name} role-experiment variant set")),
        "`<root> role-experiment variant set` is not in the walked set"
    );

    // Vacuity floor, derived rather than round: 40 `Commands` variants (38
    // unconditional plus two behind #[cfg(target_os = "windows")]) plus 35
    // in nine nested Subcommand enums plus the root itself. clap's synthetic
    // `help` nodes mirror the sibling tree and recurse, so the real
    // `walked.len()` is far above this; the floor is a vacuity guard and the
    // three membership assertions above are the criterion.
    let expected_min = if cfg!(target_os = "windows") { 75 } else { 73 };
    assert!(
        walked.len() >= expected_min,
        "walked {} commands, expected at least {expected_min}",
        walked.len()
    );
}

/// #1614 requirement (F): the deprecated spellings parse to the identical
/// value. All four combinations of {purge-wg, purge-room} x {--wg, --room}.
#[test]
fn purge_args_accept_both_flag_spellings() {
    use clap::Parser;
    for (sub, flag) in [
        ("purge-wg", "--wg"),
        ("purge-room", "--room"),
        ("purge-wg", "--room"),
        ("purge-room", "--wg"),
    ] {
        let cli = Cli::try_parse_from([
            "agentscommander",
            sub,
            "--token",
            "00000000-0000-0000-0000-000000000487",
            "--root",
            "C:/fake/root",
            flag,
            "room-1-dev-team",
        ])
        .unwrap_or_else(|e| panic!("{sub} {flag} must parse: {e}"));
        match cli.command {
            Some(Commands::PurgeWg(args)) => assert_eq!(
                args.wg.as_deref(),
                Some("room-1-dev-team"),
                "{sub} {flag} bound a different value"
            ),
            _ => panic!("{sub} {flag} did not dispatch to PurgeWg"),
        }
    }
}

/// #1614 requirement (F) for the room subcommand and its deprecated alias.
#[test]
fn room_subcommand_accepts_the_workgroup_alias() {
    use clap::Parser;
    for (sub, flag) in [("room", "--room"), ("workgroup", "--workgroup")] {
        let cli = Cli::try_parse_from([
            "agentscommander",
            sub,
            "remove",
            "--project",
            "ProjectAlpha",
            flag,
            "room-1-dev-team",
        ])
        .unwrap_or_else(|e| panic!("{sub} {flag} must parse: {e}"));
        assert!(
            matches!(cli.command, Some(Commands::Workgroup(_))),
            "{sub} {flag} did not dispatch to the room subcommand"
        );
    }
}

mod startup_message_tests;
