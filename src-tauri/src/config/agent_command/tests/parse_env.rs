use super::*;

#[test]
fn normalizes_plain_command_with_args() {
    let got = normalize_legacy_agent_command("codex --yolo").unwrap();
    assert_eq!(got.shell, "codex");
    assert_eq!(got.shell_args, vec!["--yolo"]);
}

#[test]
#[cfg(windows)]
fn normalize_launch_path_for_spawn_converts_verbatim_unc() {
    let path = normalize_launch_path_for_spawn(Some(std::path::Path::new(
        r"\\?\UNC\server\share\repo\.ac\wg-1\__agent_dev",
    )))
    .expect("normalized path");

    assert_eq!(
        path,
        std::path::PathBuf::from(r"\\server\share\repo\.ac\wg-1\__agent_dev")
    );
}

#[test]
fn preserves_quoted_arg_with_spaces() {
    let got = normalize_legacy_agent_command("codex --model \"gpt 5\"").unwrap();
    assert_eq!(got.shell, "codex");
    assert_eq!(got.shell_args, vec!["--model", "gpt 5"]);
}

#[test]
fn supports_quoted_windows_executable_path() {
    let got =
        normalize_legacy_agent_command("\"C:\\Program Files\\Codex\\codex.exe\" --yolo").unwrap();
    assert_eq!(got.shell, "C:\\Program Files\\Codex\\codex.exe");
    assert_eq!(got.shell_args, vec!["--yolo"]);
}

#[test]
fn preserves_empty_quoted_arg() {
    let got = normalize_legacy_agent_command("codex --config \"\" --flag").unwrap();
    assert_eq!(got.shell, "codex");
    assert_eq!(got.shell_args, vec!["--config", "", "--flag"]);
}

#[test]
fn stringifies_migration_tokens_with_spaces_quotes_and_empty_args() {
    let tokens = vec![
        "C:\\Program Files\\Codex\\codex.exe".to_string(),
        "--config".to_string(),
        "".to_string(),
        "a\"b".to_string(),
    ];
    let text = super::stringify_agent_command_tokens(&tokens);
    let normalized = normalize_legacy_agent_command(&text).unwrap();
    let mut round_trip = vec![normalized.shell];
    round_trip.extend(normalized.shell_args);
    assert_eq!(round_trip, tokens);
}

#[test]
fn stringifies_tokens_with_frontend_msvc_quoting_cases() {
    let cases = [
        (vec!["codex", "--model", "gpt 5"], "codex --model \"gpt 5\""),
        (
            vec!["codex", "--model", "gpt\"5"],
            "codex --model \"gpt\\\"5\"",
        ),
        (vec!["codex", "--model", "gpt'5"], "codex --model \"gpt'5\""),
        (vec!["codex", "--config", ""], "codex --config \"\""),
        (
            vec!["codex", r#"C:\tmp\"quoted"#],
            r##"codex "C:\tmp\\\"quoted""##,
        ),
        (
            vec!["codex", r#"C:\Program Files\Codex\codex.exe"#],
            r##"codex "C:\Program Files\Codex\codex.exe""##,
        ),
    ];

    for (tokens, expected) in cases {
        let tokens: Vec<String> = tokens.into_iter().map(str::to_string).collect();
        let text = super::stringify_agent_command_tokens(&tokens);
        assert_eq!(text, expected);

        let normalized = normalize_legacy_agent_command(&text).unwrap();
        assert_eq!(normalized.shell, tokens[0]);
        assert_eq!(normalized.shell_args, tokens[1..]);
    }
}

#[test]
fn parses_quoted_backslashes_like_frontend_argv_parser() {
    let normalized = normalize_legacy_agent_command(
        r##"codex "C:\Program Files\Codex\codex.exe" "C:\tmp\\\"quoted""##,
    )
    .unwrap();

    assert_eq!(normalized.shell, "codex");
    assert_eq!(
        normalized.shell_args,
        vec![
            r#"C:\Program Files\Codex\codex.exe"#.to_string(),
            r#"C:\tmp\"quoted"#.to_string(),
        ]
    );
}

#[test]
fn rejects_unclosed_quote() {
    let err = normalize_legacy_agent_command("codex \"unterminated").unwrap_err();
    assert!(err.contains("unclosed double quote"));
}

#[test]
fn rejects_empty_quoted_executable() {
    let err = normalize_legacy_agent_command("\"\" --flag").unwrap_err();
    assert!(err.contains("agent executable is empty"));
}

#[test]
fn effective_env_helpers_apply_removals_and_last_wins() {
    let spawn = spawn_with_env(
        vec![
            ("FOO".to_string(), "one".to_string()),
            ("BAR".to_string(), "remove".to_string()),
            ("FOO".to_string(), "last".to_string()),
        ],
        vec!["BAR".to_string()],
    );

    assert_eq!(
        spawn.effective_child_env(),
        vec![
            ("FOO".to_string(), "one".to_string()),
            ("FOO".to_string(), "last".to_string())
        ]
    );
    assert_eq!(spawn.effective_env_value("FOO"), Some("last"));
    assert_eq!(spawn.effective_env_value("BAR"), None);

    if cfg!(windows) {
        assert_eq!(spawn.effective_env_value("foo"), Some("last"));
    } else {
        assert_eq!(spawn.effective_env_value("foo"), None);
    }
}

#[test]
fn build_spawn_concatenates_agent_base_and_cell_params() {
    // #597 - the cell holds params only; they append to the agent base command,
    // so base `codex` + cell `--profile fast` launches `codex --profile fast`.
    let settings = settings_with_cell("codex", "B", cell("--profile fast", BTreeMap::new()));

    let spawn = build_agent_spawn_command(&settings, "codex", None, Some("B")).unwrap();

    assert_eq!(spawn.shell, "codex");
    assert_eq!(spawn.shell_args, vec!["--profile", "fast"]);
    assert_eq!(spawn.profile_resolution.effective_profile, "B");
}

#[test]
fn pi_model_overlap_never_generates_codex_home() {
    for (id, command) in [
        ("pi", "pi --model codex-model"),
        ("pi-cmd", "cmd /C pi --model codex-model"),
    ] {
        let mut pi = agent(id, command);
        pi.isolated_home = true;
        let settings = AppSettings {
            agents: vec![pi],
            ..AppSettings::default()
        };

        let spawn = build_agent_spawn_command(&settings, id, None, Some("A")).unwrap();

        assert!(!spawn.generated_env.contains_key("CODEX_HOME"));
        assert!(spawn.effective_codex_home.is_none());
        assert!(!spawn
            .env_remove_keys
            .iter()
            .any(|key| key.eq_ignore_ascii_case("CODEX_HOME")));
    }
}

#[test]
fn env_merge_precedence_is_agent_profile_generated() {
    let mut codex = agent("codex", "codex");
    codex.envs = vec![
        CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "agent".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
        CodingAgentEnv {
            key: "CODEX_HOME".to_string(),
            value: std::env::temp_dir()
                .join("agent-codex")
                .to_string_lossy()
                .to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
    ];
    codex.isolated_home = true;
    let mut settings = AppSettings {
        agents: vec![codex],
        ..AppSettings::default()
    };
    let profile_home = std::env::temp_dir().join("profile-codex");
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([
                    ("OPENAI_API_BASE".to_string(), "profile".to_string()),
                    (
                        "CODEX_HOME".to_string(),
                        profile_home.to_string_lossy().to_string(),
                    ),
                ]),
                notes: String::new(),
            },
        );

    let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
    let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();

    assert_eq!(
        env.get("OPENAI_API_BASE").map(String::as_str),
        Some("profile")
    );
    assert_eq!(
        env.get("CODEX_HOME").map(String::as_str),
        spawn
            .effective_codex_home
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .as_deref()
    );
    assert!(spawn.generated_env.contains_key("CODEX_HOME"));
}

#[test]
fn reserved_env_keys_are_rejected_case_insensitively_on_windows() {
    let mut codex = agent("codex", "codex");
    codex.envs = vec![CodingAgentEnv {
        key: if cfg!(windows) {
            "agentscommander_token".to_string()
        } else {
            "AGENTSCOMMANDER_TOKEN".to_string()
        },
        value: "bad".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    let settings = AppSettings {
        agents: vec![codex],
        ..AppSettings::default()
    };

    let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();
    assert!(err.contains("reserved"), "{err}");
}

#[test]
fn profile_codex_home_is_effective_when_isolation_is_off() {
    let profile_home = std::env::temp_dir().join("profile-codex-home");
    let mut settings = AppSettings {
        agents: vec![agent("codex", "codex")],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([(
                    "CODEX_HOME".to_string(),
                    profile_home.to_string_lossy().to_string(),
                )]),
                notes: String::new(),
            },
        );

    let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();

    assert_eq!(
        spawn.effective_codex_home.as_deref(),
        Some(profile_home.as_path())
    );
    assert!(spawn.generated_env.is_empty());
}

// #2561 - the agent command joins with `\`, a separator only on Windows.
#[cfg(windows)]
#[test]
fn ac_root_command_expands_after_parse_for_replica_launch_root() {
    let temp = tempfile::tempdir().unwrap();
    let replica = temp
        .path()
        .join("root with spaces")
        .join(".ac")
        .join("wg-7-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();
    // #597 - under concatenation the binary (placeholder included) lives in the
    // agent base command and the cell holds the param. The placeholder must
    // still expand on the composed token list so the launched shell is the
    // expanded binary path.
    let mut settings = AppSettings {
        agents: vec![agent("codex", "%AC_REPLICA_ROOT%\\bin\\codex.exe")],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--flag".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    let spawn = build_agent_spawn_command(&settings, "codex", Some(&replica), Some("A"))
        .expect("replica launch root should expand");

    // build_agent_spawn_command canonicalizes the launch root before expanding
    // %AC_REPLICA_ROOT% (see placeholders.rs). On Windows, canonicalization resolves an
    // 8.3 short path component (e.g. a CI runner's RUNNER~1) back to its long
    // form (runneradmin), whereas the temp dir may be reported in 8.3 short form
    // when the username exceeds 8 chars. Normalize the expected value through the
    // same canonicalize + verbatim-prefix strip so the comparison does not depend
    // on which form the OS hands back.
    let canonical_root = std::fs::canonicalize(&replica).unwrap();
    let canonical_text = canonical_root.to_string_lossy();
    let expected_root = canonical_text
        .strip_prefix(r"\\?\")
        .map(std::path::PathBuf::from)
        .unwrap_or(canonical_root);

    assert_eq!(
        spawn.shell,
        expected_root
            .join("bin")
            .join("codex.exe")
            .to_string_lossy()
    );
    assert_eq!(spawn.shell_args, vec!["--flag"]);
}

#[test]
fn ac_root_in_normal_repo_launch_returns_clear_error() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo-thing");
    std::fs::create_dir_all(&repo).unwrap();
    let settings = AppSettings {
        agents: vec![agent("codex", "%AC_REPLICA_ROOT%\\bin\\codex.exe")],
        ..AppSettings::default()
    };

    let err = build_agent_spawn_command(&settings, "codex", Some(&repo), Some("A"))
        .expect_err("normal repo launch must reject AC_REPLICA_ROOT");

    assert!(err.contains("%AC_REPLICA_ROOT% requires an AC replica or root-agent launch root"));
}

#[test]
fn workspace_and_matrix_placeholders_expand_in_profile_env_for_replica_launch() {
    let temp = tempfile::tempdir().unwrap();
    let replica = temp
        .path()
        .join("root with spaces")
        .join(".ac")
        .join("wg-7-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();
    let mut settings = AppSettings {
        agents: vec![agent("claude", "claude")],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("claude".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([
                    (
                        "CLAUDE_CONFIG_DIR".to_string(),
                        "%AC_WORKSPACE_ROOT%\\.claude".to_string(),
                    ),
                    (
                        "CLAUDE_MATRIX_DIR".to_string(),
                        "%AC_MATRIX_ROOT%\\.claude".to_string(),
                    ),
                ]),
                notes: String::new(),
            },
        );

    let spawn = build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A"))
        .expect("replica launch root should expand workspace + matrix placeholders");
    let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();

    // Mirror the canonicalize + verbatim-prefix strip the builder applies, then
    // derive the .ac workspace ancestor and the matrix dir from it.
    let canonical_root = std::fs::canonicalize(&replica).unwrap();
    let canonical_text = canonical_root.to_string_lossy();
    let expected_replica = canonical_text
        .strip_prefix(r"\\?\")
        .map(std::path::PathBuf::from)
        .unwrap_or(canonical_root);
    let expected_ac_root = expected_replica
        .parent()
        .and_then(|parent| parent.parent())
        .expect("replica has a .ac workspace ancestor");
    let expected_matrix = expected_ac_root.join("_agent_dev-rust");

    let expected_config = format!("{}\\.claude", expected_ac_root.to_string_lossy());
    let expected_matrix_claude = format!("{}\\.claude", expected_matrix.to_string_lossy());
    assert_eq!(env.get("CLAUDE_CONFIG_DIR"), Some(&expected_config));
    assert_eq!(env.get("CLAUDE_MATRIX_DIR"), Some(&expected_matrix_claude));
}

#[test]
fn matrix_codex_home_expands_to_absolute_path_for_replica_launch() {
    let temp = tempfile::tempdir().unwrap();
    let replica = temp
        .path()
        .join("root with spaces")
        .join(".ac")
        .join("wg-7-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();
    let mut settings = AppSettings {
        agents: vec![agent("codex", "codex")],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([(
                    "CODEX_HOME".to_string(),
                    "%AC_MATRIX_ROOT%\\.codex".to_string(),
                )]),
                notes: String::new(),
            },
        );

    let spawn = build_agent_spawn_command(&settings, "codex", Some(&replica), Some("A"))
        .expect("matrix CODEX_HOME should expand to an absolute path");

    let canonical_root = std::fs::canonicalize(&replica).unwrap();
    let canonical_text = canonical_root.to_string_lossy();
    let expected_replica = canonical_text
        .strip_prefix(r"\\?\")
        .map(std::path::PathBuf::from)
        .unwrap_or(canonical_root);
    let expected_matrix = expected_replica
        .parent()
        .and_then(|parent| parent.parent())
        .expect("replica has a .ac workspace ancestor")
        .join("_agent_dev-rust");

    let home = spawn
        .effective_codex_home
        .expect("codex home resolves from the matrix placeholder");
    assert!(home.is_absolute(), "{}", home.display());
    // Mirror the builder's string concatenation (matrix + literal "\.codex"). Comparing
    // the whole path keeps the assertion exact on Windows and still correct on platforms
    // where the backslash is an ordinary character rather than a path separator, so the
    // test does not silently become Windows-only.
    let expected_home =
        std::path::PathBuf::from(format!("{}\\.codex", expected_matrix.to_string_lossy()));
    assert_eq!(home, expected_home);
}

#[test]
fn non_path_env_allows_literal_dollar_but_rejects_unknown_percent_marker() {
    let mut codex = agent("codex", "codex");
    codex.envs = vec![
        CodingAgentEnv {
            key: "SECRET".to_string(),
            value: "s3cr$tP4ss".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
        CodingAgentEnv {
            key: "BAD_MARKER".to_string(),
            value: "%UNKNOWN%".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
    ];
    let settings = AppSettings {
        agents: vec![codex],
        ..AppSettings::default()
    };

    let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();

    assert!(err.contains("unknown placeholder"), "{err}");
}

#[cfg(windows)]
#[test]
fn duplicate_env_keys_are_rejected_case_insensitively_on_windows() {
    let mut codex = agent("codex", "codex");
    codex.envs = vec![
        CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "one".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
        CodingAgentEnv {
            key: "openai_api_base".to_string(),
            value: "two".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
    ];
    let settings = AppSettings {
        agents: vec![codex],
        ..AppSettings::default()
    };

    let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();

    assert!(err.contains("duplicate env keys"), "{err}");
}

// #529 - instructions filename resolver + safety validation.
