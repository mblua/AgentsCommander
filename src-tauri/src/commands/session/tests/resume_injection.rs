use super::*;

#[test]
fn pi_explicit_session_control_matches_only_decided_spellings() {
    for token in [
        "-c",
        "-r",
        "--continue",
        "--continue=true",
        "--resume",
        "--resume=id",
        "--session",
        "--session=id",
        "--session-id",
        "--session-id=id",
        "--fork",
        "--fork=id",
        "--no-session",
        "--no-session=true",
    ] {
        assert!(
            pi_has_explicit_session_control(&[token.to_string()]),
            "token={token:?}"
        );
    }
    for token in [
        "-cr",
        "--Continue",
        "--session-dir",
        "--session-dir=custom",
        "--session-directory",
        "--forked",
        "--no-sessions",
    ] {
        assert!(
            !pi_has_explicit_session_control(&[token.to_string()]),
            "token={token:?}"
        );
    }
}

#[test]
fn pi_non_conversation_policy_is_case_sensitive_and_position_aware() {
    for command in ["install", "remove", "uninstall", "update", "list", "config"] {
        assert!(pi_is_non_conversation_invocation(&[command.to_string()]));
        assert!(!pi_is_non_conversation_invocation(&[
            "--print".to_string(),
            command.to_string(),
        ]));
    }
    for flag in [
        "--help",
        "-h",
        "--version",
        "-v",
        "--export",
        "--export=file.jsonl",
        "--list-models",
        "--list-models=json",
    ] {
        assert!(pi_is_non_conversation_invocation(&[
            "--model".to_string(),
            "x".to_string(),
            flag.to_string(),
        ]));
    }
    for eligible in ["Install", "--Help", "--print", "-p", "--json", "--rpc"] {
        assert!(!pi_is_non_conversation_invocation(&[eligible.to_string()]));
    }
}

#[test]
fn inject_pi_resume_handles_direct_tokenized_and_embedded_commands() {
    let mut direct = vec!["--model".to_string(), "claude-sonnet".to_string()];
    assert!(inject_pi_resume("pi", &mut direct));
    assert_eq!(direct, vec!["--continue", "--model", "claude-sonnet"]);

    let mut tokenized = vec![
        "/C".to_string(),
        "pi.cmd".to_string(),
        "--session-dir".to_string(),
        r"C:\Pi State".to_string(),
    ];
    assert!(inject_pi_resume("cmd.exe", &mut tokenized));
    assert_eq!(
        tokenized,
        vec![
            "/C",
            "pi.cmd",
            "--continue",
            "--session-dir",
            r"C:\Pi State"
        ]
    );

    let original = r#"  "C:\Program Files\Pi\pi.cmd"  --model "x&&y" x^&y > out %VAR%&&echo  done"#;
    let expected =
        r#"  "C:\Program Files\Pi\pi.cmd" --continue  --model "x&&y" x^&y > out %VAR%&&echo  done"#;
    let mut embedded = vec!["/K".to_string(), original.to_string()];
    assert!(inject_pi_resume("cmd", &mut embedded));
    assert_eq!(embedded, vec!["/K", expected]);
    assert!(!inject_pi_resume("cmd", &mut embedded));
    assert_eq!(embedded, vec!["/K", expected]);
}

#[test]
fn inject_pi_resume_honors_every_selector_and_decoded_cmd_spelling() {
    for selector in [
        "-c",
        "-r",
        "--continue",
        "--continue=true",
        "--resume",
        "--resume=id",
        "--session",
        "--session=id",
        "--session-id",
        "--session-id=id",
        "--fork",
        "--fork=id",
        "--no-session",
        "--no-session=true",
    ] {
        let mut args = vec![selector.to_string(), "--model".to_string(), "x".to_string()];
        let original = args.clone();
        assert!(!inject_pi_resume("pi", &mut args), "selector={selector:?}");
        assert_eq!(args, original);
    }

    for text in [r#"pi "--resume" --model x"#, "pi --res^ume --model x"] {
        let mut args = vec!["/C".to_string(), text.to_string()];
        let original = args.clone();
        assert!(!inject_pi_resume("cmd.exe", &mut args), "text={text:?}");
        assert_eq!(args, original);
    }
}

#[test]
fn inject_pi_resume_distinguishes_session_dir_and_prefix_names() {
    for initial in [
        vec!["--session-dir".to_string(), "custom".to_string()],
        vec!["--session-dir=custom".to_string()],
        vec!["--session-directory".to_string()],
    ] {
        let mut args = initial.clone();
        assert!(inject_pi_resume("pi", &mut args));
        assert_eq!(args.first().map(String::as_str), Some("--continue"));
        assert_eq!(&args[1..], initial.as_slice());
    }

    let mut explicit = vec!["--session".to_string(), "id".to_string()];
    let original = explicit.clone();
    assert!(!inject_pi_resume("pi", &mut explicit));
    assert_eq!(explicit, original);
}

#[test]
fn inject_pi_resume_preserves_non_conversation_invocations() {
    for command in ["install", "remove", "uninstall", "update", "list", "config"] {
        let mut args = vec![command.to_string(), "package".to_string()];
        let original = args.clone();
        assert!(!inject_pi_resume("pi", &mut args), "command={command:?}");
        assert_eq!(args, original);
    }
    for flag in [
        "--help",
        "-h",
        "--version",
        "-v",
        "--export",
        "--export=file",
        "--list-models",
        "--list-models=json",
    ] {
        let mut args = vec!["--model".to_string(), "x".to_string(), flag.to_string()];
        let original = args.clone();
        assert!(!inject_pi_resume("pi", &mut args), "flag={flag:?}");
        assert_eq!(args, original);
    }

    for conversational in ["--print", "-p", "--json", "--rpc", "Install", "--Help"] {
        let mut args = vec![conversational.to_string()];
        assert!(inject_pi_resume("pi", &mut args));
        assert_eq!(args.first().map(String::as_str), Some("--continue"));
    }
}

#[test]
fn inject_pi_resume_inspects_only_the_first_cmd_segment() {
    let original = "pi --model x&&echo --resume";
    let expected = "pi --continue --model x&&echo --resume";
    let mut args = vec!["/C".to_string(), original.to_string()];
    assert!(inject_pi_resume("cmd.exe", &mut args));
    assert_eq!(args, vec!["/C", expected]);
}

#[test]
fn inject_pi_resume_leaves_malformed_unsupported_and_non_pi_inputs_unchanged() {
    let cases = [
        ("cmd.exe", vec!["/C".to_string(), "pi>out".to_string()]),
        (
            "cmd.exe",
            vec!["/C".to_string(), "pi \"unterminated".to_string()],
        ),
        (
            "cmd.exe",
            vec!["/C".to_string(), "npx".to_string(), "pi".to_string()],
        ),
        ("powershell.exe", vec!["pi".to_string()]),
    ];
    for (shell, mut args) in cases {
        let original = args.clone();
        assert!(!inject_pi_resume(shell, &mut args), "shell={shell:?}");
        assert_eq!(args, original);
    }
}

#[test]
fn maybe_inject_pi_resume_requires_kind_trusted_spawn_and_known_state() {
    let spawn = inert_pi_spawn();
    let base = vec!["--model".to_string(), "claude-sonnet".to_string()];

    let mut eligible = base.clone();
    assert!(maybe_inject_pi_resume(
        Some(CodingAgentKind::Pi),
        Some(&spawn),
        false,
        "pi",
        &mut eligible,
    ));
    assert_eq!(eligible, vec!["--continue", "--model", "claude-sonnet"]);

    for (kind, trusted, fresh) in [
        (Some(CodingAgentKind::Pi), None, false),
        (Some(CodingAgentKind::Pi), Some(&spawn), true),
        (Some(CodingAgentKind::Claude), Some(&spawn), false),
        (None, Some(&spawn), false),
    ] {
        let mut args = base.clone();
        assert!(!maybe_inject_pi_resume(
            kind, trusted, fresh, "pi", &mut args,
        ));
        assert_eq!(args, base);
    }
}

#[test]
fn pi_tuned_profile_flag_stays_false_while_direct_shell_capability_is_separate() {
    let mut settings = AppSettings::default();
    settings
        .auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    let requested = crate::config::settings::resolve_auto_self_clear(&settings, "dev-rust", false);
    assert!(requested);
    // The tuned profile retains #1069's value. Launch gating uses #1059's
    // independent exact-stem direct-shell capability instead.
    assert!(!CodingAgentKind::Pi.profile().auto_self_clear_supported && requested);
}

#[test]
fn heuristic_agent_metadata_cannot_authorize_pi_mutation() {
    let settings = AppSettings {
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "unrelated-cmd".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Unrelated cmd recipe".to_string(),
            command: "cmd".to_string(),
            color: "#000000".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        ..AppSettings::default()
    };
    let configured_args = vec!["/C".to_string(), "pi".to_string()];
    let (heuristic_id, _) = resolve_actual_agent("cmd", &configured_args, None, None, &settings);
    assert_eq!(heuristic_id.as_deref(), Some("unrelated-cmd"));

    let mut args = configured_args;
    assert!(!maybe_inject_pi_resume(
        Some(CodingAgentKind::Pi),
        None,
        false,
        "cmd",
        &mut args,
    ));
    assert_eq!(args, vec!["/C", "pi"]);
}

#[test]
fn inject_antigravity_resume_prefixes_direct_agy_args() {
    let mut args = vec!["-m".to_string(), "gpt-5".to_string()];
    assert!(super::super::inject_antigravity_resume("agy", &mut args));
    assert_eq!(
        args,
        vec![
            "--continue".to_string(),
            "-m".to_string(),
            "gpt-5".to_string()
        ]
    );

    let mut args = vec!["-m".to_string(), "gpt-5".to_string()];
    assert!(super::super::inject_antigravity_resume(
        "antigravity",
        &mut args
    ));
    assert_eq!(
        args,
        vec![
            "--continue".to_string(),
            "-m".to_string(),
            "gpt-5".to_string()
        ]
    );
}

#[test]
fn inject_antigravity_resume_inserts_into_cmd_tokenized_wrapper() {
    let mut args = vec![
        "/C".to_string(),
        "agy".to_string(),
        "-m".to_string(),
        "gpt-5".to_string(),
    ];
    assert!(super::super::inject_antigravity_resume(
        "cmd.exe", &mut args
    ));
    assert_eq!(
        args,
        vec![
            "/C".to_string(),
            "agy".to_string(),
            "--continue".to_string(),
            "-m".to_string(),
            "gpt-5".to_string()
        ]
    );
}

#[test]
fn inject_antigravity_resume_inserts_into_embedded_cmd_string() {
    let mut args = vec!["/K".to_string(), "git pull && agy -m gpt-5".to_string()];
    assert!(super::super::inject_antigravity_resume(
        "cmd.exe", &mut args
    ));
    assert_eq!(
        args,
        vec![
            "/K".to_string(),
            "git pull && agy --continue -m gpt-5".to_string()
        ]
    );
}

#[test]
fn inject_antigravity_resume_skips_when_continue_or_conversation_present() {
    for skip_args in [
        vec!["--continue".to_string(), "gpt-5".to_string()],
        vec!["-c".to_string(), "gpt-5".to_string()],
        vec!["--conversation".to_string(), "abc123".to_string()],
        vec!["--conversation=abc123".to_string()],
        vec!["--CONTINUE".to_string(), "gpt-5".to_string()],
        vec!["--Conversation".to_string(), "abc123".to_string()],
    ] {
        let mut args = skip_args.clone();
        assert!(
            !super::super::inject_antigravity_resume("agy", &mut args),
            "args={skip_args:?}"
        );
        assert_eq!(args, skip_args);
    }

    // Tokenized cmd form also skips on a resume marker after the executable.
    let mut args = vec![
        "/C".to_string(),
        "agy".to_string(),
        "--conversation".to_string(),
        "abc123".to_string(),
    ];
    assert!(!super::super::inject_antigravity_resume(
        "cmd.exe", &mut args
    ));
    assert_eq!(
        args,
        vec![
            "/C".to_string(),
            "agy".to_string(),
            "--conversation".to_string(),
            "abc123".to_string()
        ]
    );
}

#[test]
fn inject_codex_resume_prefixes_direct_codex_args() {
    let mut args = vec![
        "-m".to_string(),
        "gpt-5".to_string(),
        "-c".to_string(),
        "model_reasoning_effort=\"high\"".to_string(),
    ];

    assert!(inject_codex_resume("codex", &mut args));
    assert_eq!(
        args,
        vec![
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
            "-c".to_string(),
            "model_reasoning_effort=\"high\"".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_inserts_into_cmd_tokenized_wrapper() {
    let mut args = vec![
        "/C".to_string(),
        "codex".to_string(),
        "-m".to_string(),
        "gpt-5".to_string(),
    ];

    assert!(inject_codex_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec![
            "/C".to_string(),
            "codex".to_string(),
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_inserts_into_embedded_cmd_wrapper() {
    let mut args = vec!["/K".to_string(), "git pull && codex -m gpt-5".to_string()];

    assert!(inject_codex_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec![
            "/K".to_string(),
            "git pull && codex resume --last -m gpt-5".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_skips_existing_resume_tokens() {
    let mut args = vec![
        "resume".to_string(),
        "--last".to_string(),
        "-m".to_string(),
        "gpt-5".to_string(),
    ];

    assert!(!inject_codex_resume("codex", &mut args));
    assert_eq!(
        args,
        vec![
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_skips_explicit_fork_subcommand() {
    let mut args = vec!["fork".to_string(), "--last".to_string()];

    assert!(!inject_codex_resume("codex", &mut args));
    assert_eq!(args, vec!["fork".to_string(), "--last".to_string()]);
}

#[test]
fn inject_codex_resume_skips_explicit_exec_subcommand_after_options() {
    let mut args = vec![
        "-m".to_string(),
        "gpt-5".to_string(),
        "exec".to_string(),
        "--json".to_string(),
    ];

    assert!(!inject_codex_resume("codex", &mut args));
    assert_eq!(
        args,
        vec![
            "-m".to_string(),
            "gpt-5".to_string(),
            "exec".to_string(),
            "--json".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_skips_explicit_help_subcommand_in_cmd_wrapper() {
    let mut args = vec!["/C".to_string(), "codex".to_string(), "help".to_string()];

    assert!(!inject_codex_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec!["/C".to_string(), "codex".to_string(), "help".to_string()]
    );
}

#[test]
fn inject_codex_resume_ignores_resume_text_inside_config_value() {
    let mut args = vec![
        "-c".to_string(),
        "instruction=\"resume later\"".to_string(),
        "--search".to_string(),
    ];

    assert!(inject_codex_resume("codex", &mut args));
    assert_eq!(
        args,
        vec![
            "resume".to_string(),
            "--last".to_string(),
            "-c".to_string(),
            "instruction=\"resume later\"".to_string(),
            "--search".to_string(),
        ]
    );
}

#[test]
fn resolve_actual_agent_keeps_requested_agent_when_shell_validates_it() {
    let settings = test_settings();

    let resolved = resolve_actual_agent(
        "codex",
        &["-m".to_string(), "gpt-5".to_string()],
        Some("codex"),
        Some("Codex Stable"),
        &settings,
    );

    assert_eq!(
        resolved,
        (Some("codex".to_string()), Some("Codex Stable".to_string()))
    );
}

#[test]
fn resolve_actual_agent_keeps_requested_agent_when_normalized_command_matches() {
    let mut settings = test_settings();
    settings.agents.push(AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "codex-yolo".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Codex Yolo".to_string(),
        command: "codex --yolo".to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    });

    let resolved = resolve_actual_agent(
        "codex",
        &["--yolo".to_string()],
        Some("codex-yolo"),
        Some("Codex Yolo"),
        &settings,
    );

    assert_eq!(
        resolved,
        (
            Some("codex-yolo".to_string()),
            Some("Codex Yolo".to_string())
        )
    );
}

#[test]
fn resolve_actual_agent_falls_back_to_detected_label_when_validated_match_has_no_stored_label() {
    let settings = test_settings();

    let resolved = resolve_actual_agent(
        "codex",
        &["-m".to_string(), "gpt-5".to_string()],
        Some("codex"),
        None,
        &settings,
    );

    assert_eq!(
        resolved,
        (Some("codex".to_string()), Some("Codex".to_string()))
    );
}

#[test]
fn resolve_actual_agent_clears_requested_agent_when_shell_is_unresolved() {
    let settings = test_settings();

    let resolved = resolve_actual_agent(
        "powershell.exe",
        &["-NoLogo".to_string()],
        Some("codex"),
        Some("Codex"),
        &settings,
    );

    assert_eq!(resolved, (None, None));
}

#[test]
fn resolve_actual_agent_uses_shell_resolved_agent_on_mismatch() {
    let settings = test_settings();

    let resolved = resolve_actual_agent("claude", &[], Some("codex"), Some("Codex"), &settings);

    assert_eq!(
        resolved,
        (Some("claude".to_string()), Some("Claude Code".to_string()))
    );
}

#[test]
fn resolve_agent_from_shell_skips_invalid_configured_command() {
    let mut settings = test_settings();
    settings.agents = vec![AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "broken-codex".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Broken Codex".to_string(),
        command: "codex \"unterminated".to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }];

    let resolved = resolve_agent_from_shell("codex", &[], &settings);

    assert_eq!(resolved, (None, None));
}

#[test]
fn effective_restart_skip_auto_resume_defaults_to_true_for_none() {
    // No explicit value → preserve legacy "fresh conversation" semantics
    // used by SessionItem, ProjectPanel context menu, AcDiscoveryPanel.
    assert!(super::super::effective_restart_skip_auto_resume(None));
}

#[test]
fn effective_restart_skip_auto_resume_respects_explicit_false() {
    // Deferred-wake path (ProjectPanel.handleReplicaClick) MUST be able
    // to opt in to provider auto-resume; otherwise antigravity/codex/claude
    // sessions re-open with a blank slate instead of continuing.
    assert!(!super::super::effective_restart_skip_auto_resume(Some(
        false
    )));
}

#[test]
fn effective_restart_skip_auto_resume_respects_explicit_true() {
    // Explicit true still works (future-proof against a caller that
    // wants to be explicit rather than rely on the default).
    assert!(super::super::effective_restart_skip_auto_resume(Some(true)));
}
