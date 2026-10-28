use super::*;

#[test]
fn queued_status_returns_zero() {
    let resp = r#"{"action":"self-handoff-and-switch","status":"queued","session_id":"s"}"#;
    assert_eq!(interpret_self_switch_response_exit_code(resp), 0);
}

#[test]
fn already_queued_status_returns_zero() {
    let resp = r#"{"action":"self-handoff-and-switch","status":"already_queued","session_id":"s"}"#;
    assert_eq!(interpret_self_switch_response_exit_code(resp), 0);
}

#[test]
fn unknown_status_returns_two() {
    let resp = r#"{"status":"weird","action":"self-handoff-and-switch"}"#;
    assert_eq!(interpret_self_switch_response_exit_code(resp), 2);
}

#[test]
fn normalize_profile_arg_uppercases_valid_letter() {
    assert_eq!(
        normalize_profile_arg(Some("b".into())).unwrap().as_deref(),
        Some("B")
    );
}

#[test]
fn normalize_profile_arg_rejects_non_letter() {
    assert!(normalize_profile_arg(Some("aa".into())).is_err());
    assert!(normalize_profile_arg(Some("1".into())).is_err());
}

#[test]
fn self_switch_args_parse_with_default_timeout() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "self-handoff-and-switch",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--root",
        "anything",
    ])
    .expect("clap should accept self-handoff-and-switch with token + root");
    let cmd = parsed.command.expect("subcommand present");
    match cmd {
        crate::cli::Commands::SelfSwitch(args) => {
            assert_eq!(args.timeout, 15);
            assert_eq!(
                args.token.as_deref(),
                Some("11111111-1111-1111-1111-111111111111")
            );
            assert_eq!(args.root.as_deref(), Some("anything"));
            assert_eq!(args.coding_agent, None);
            assert_eq!(args.profile, None);
            assert!(!args.list_coding_agents);
        }
        _ => panic!("expected SelfSwitch subcommand"),
    }
}

#[test]
fn self_switch_args_parse_coding_agent_and_profile() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "self-handoff-and-switch",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--root",
        "anything",
        "--coding-agent",
        "agent_1719526800000_3",
        "--profile",
        "c",
        "--timeout",
        "42",
    ])
    .expect("clap should accept switch args");
    let cmd = parsed.command.expect("subcommand present");
    match cmd {
        crate::cli::Commands::SelfSwitch(args) => {
            assert_eq!(args.timeout, 42);
            assert_eq!(args.coding_agent.as_deref(), Some("agent_1719526800000_3"));
            assert_eq!(
                normalize_profile_arg(args.profile).unwrap().as_deref(),
                Some("C")
            );
            assert!(!args.list_coding_agents);
        }
        _ => panic!("expected SelfSwitch subcommand"),
    }
}

#[test]
fn self_switch_list_coding_agents_parses_without_token_or_root() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "self-handoff-and-switch",
        "--list-coding-agents",
    ])
    .expect("discovery should not require token or root at clap parse time");
    let cmd = parsed.command.expect("subcommand present");
    match cmd {
        crate::cli::Commands::SelfSwitch(args) => {
            assert!(args.list_coding_agents);
            assert_eq!(args.token, None);
            assert_eq!(args.root, None);
            assert_eq!(args.coding_agent, None);
            assert_eq!(args.profile, None);
        }
        _ => panic!("expected SelfSwitch subcommand"),
    }
}

#[test]
fn self_switch_help_documents_forgotten_summary_behavior() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let mut subcommand = cmd
        .get_subcommands()
        .find(|cmd| cmd.get_name() == "self-handoff-and-switch")
        .expect("self-handoff-and-switch subcommand")
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

fn agent(id: &str, label: &str, command: &str) -> crate::config::settings::AgentConfig {
    crate::config::settings::AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: label.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

fn enabled_cell(command: &str) -> crate::config::settings::ProfileCellConfig {
    crate::config::settings::ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        enabled: true,
        command: command.to_string(),
        env: std::collections::BTreeMap::new(),
        notes: String::new(),
    }
}

fn disabled_cell(command: &str) -> crate::config::settings::ProfileCellConfig {
    crate::config::settings::ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        enabled: false,
        command: command.to_string(),
        env: std::collections::BTreeMap::new(),
        notes: String::new(),
    }
}

#[test]
fn discovery_lists_exact_ids_for_duplicate_backend_commands() {
    let mut settings = crate::config::settings::AppSettings {
        agents: vec![
            agent("codex-main", "Codex Main", "codex --model gpt-5"),
            agent("codex-review", "Codex Review", "codex --model gpt-5"),
        ],
        ..crate::config::settings::AppSettings::default()
    };
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "codex-main".to_string(),
        std::collections::BTreeMap::from([
            ("A".to_string(), enabled_cell("")),
            ("B".to_string(), enabled_cell("--profile-b")),
        ]),
    );
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "codex-review".to_string(),
        std::collections::BTreeMap::from([
            ("A".to_string(), enabled_cell("")),
            ("C".to_string(), enabled_cell("--profile-c")),
        ]),
    );

    let rendered = render_coding_agent_discovery(&settings);

    assert!(rendered.contains("- codex-main"));
    assert!(rendered.contains("- codex-review"));
    assert!(rendered.contains("self-handoff-and-switch --coding-agent=codex-main --profile B"));
    assert!(rendered.contains("self-handoff-and-switch --coding-agent=codex-review --profile C"));
    assert!(!rendered.contains("self-handoff-and-switch --coding-agent codex --profile"));
}

#[test]
fn discovery_reports_direct_profiles_and_fallback_rule() {
    let mut settings = crate::config::settings::AppSettings {
        agents: vec![agent("codex-main", "Codex Main", "codex")],
        ..crate::config::settings::AppSettings::default()
    };
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "codex-main".to_string(),
        std::collections::BTreeMap::from([
            ("A".to_string(), enabled_cell("")),
            ("B".to_string(), disabled_cell("--disabled")),
            ("D".to_string(), enabled_cell("--profile-d")),
        ]),
    );

    let rendered = render_coding_agent_discovery(&settings);

    assert!(rendered.contains("profiles without fallback: A, D"));
    assert!(rendered.contains("other A-Z letters fall back to the nearest lower profile listed"));
    assert!(!rendered.contains("self-handoff-and-switch --coding-agent=codex-main --profile B"));
}

#[test]
fn discovery_shell_quotes_copy_ids_with_spaces_and_metacharacters() {
    let settings = crate::config::settings::AppSettings {
        agents: vec![agent("codex main; Remove-Item *", "Codex Main", "codex")],
        ..crate::config::settings::AppSettings::default()
    };

    let rendered = render_coding_agent_discovery(&settings);

    assert!(rendered.contains(
        "self-handoff-and-switch --coding-agent='codex main; Remove-Item *' --profile A"
    ));
}

/// #2716 (B3, option C) E37: an unreadable agents layer is reported as such,
/// naming the path; a genuinely empty list still reads "none configured".
#[test]
fn an_unreadable_layer_is_not_an_empty_list() {
    let path = std::path::Path::new("C:/cfg/agents.30.instance.no-git.json");
    let mut settings = crate::config::settings::AppSettings {
        agents_layer: crate::config::settings::AgentsLayerState::Unreadable {
            path: path.to_path_buf(),
            reason: "test".to_string(),
        },
        ..Default::default()
    };
    settings.agents.clear();
    let marked = render_coding_agent_discovery(&settings);
    assert_eq!(
        marked,
        crate::config::settings::agents_unreadable_cli_error(path)
    );
    assert!(marked.contains(&path.display().to_string()));

    settings.agents_layer = crate::config::settings::AgentsLayerState::Readable;
    assert_eq!(
        render_coding_agent_discovery(&settings),
        "No configured coding agents found."
    );
}
