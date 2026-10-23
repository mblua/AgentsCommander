use super::*;

#[test]
fn profile_content_hash_is_deterministic_and_16_lowercase_hex() {
    let mut env = BTreeMap::new();
    env.insert("FOO".to_string(), "bar".to_string());
    let a = profile_content_hash("claude --x", &env);
    let b = profile_content_hash("claude --x", &env);
    assert_eq!(a, b, "same inputs must hash identically");
    assert_eq!(a.len(), 16, "hash must be 16 chars");
    assert!(
        a.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "hash must be lowercase hex: {a}"
    );
}

#[test]
fn profile_content_hash_placeholder_and_expansion_agree() {
    // #2431 (D10) - the executable reduces to its basename, so the raw
    // placeholder form and its expansion are the same identity. Supersedes
    // the old "raw hashes differently from expanded" property.
    let env = BTreeMap::new();
    let raw = profile_content_hash("%AC_REPLICA_ROOT%\\bin\\claude --x", &env);
    let expanded = profile_content_hash("C:\\replica\\bin\\claude --x", &env);
    assert_eq!(raw, expanded);
    assert_ne!(
        raw,
        profile_content_hash("C:\\replica\\bin\\codex --x", &env),
        "a different executable must still hash differently"
    );
}

#[test]
fn profile_content_hash_changes_on_agent_base_command_edit() {
    // #597 - the hash now fingerprints the EFFECTIVE command (base + cell), so
    // an edit to the agent base command flips it even with the cell unchanged.
    let s1 = settings_with_cell("codex --one", "A", cell("--p", BTreeMap::new()));
    let s2 = settings_with_cell("codex --two", "A", cell("--p", BTreeMap::new()));
    let h1 = build_agent_spawn_command(&s1, "codex", None, Some("A"))
        .unwrap()
        .profile_content_hash;
    let h2 = build_agent_spawn_command(&s2, "codex", None, Some("A"))
        .unwrap()
        .profile_content_hash;
    assert_ne!(
        h1, h2,
        "an agent-base edit must flip the effective-command hash"
    );
}

#[test]
fn profile_content_hash_flips_on_cell_command_edit() {
    let s1 = settings_with_cell("codex", "A", cell("claude", BTreeMap::new()));
    let s2 = settings_with_cell("codex", "A", cell("claude --foo", BTreeMap::new()));
    let h1 = build_agent_spawn_command(&s1, "codex", None, Some("A"))
        .unwrap()
        .profile_content_hash;
    let h2 = build_agent_spawn_command(&s2, "codex", None, Some("A"))
        .unwrap()
        .profile_content_hash;
    assert_ne!(h1, h2, "a cell command edit must flip the hash (drift)");
}

#[test]
fn profile_content_hash_uses_effective_cell_after_fallback() {
    let mut settings = AppSettings {
        agents: vec![agent("codex", "codex")],
        ..AppSettings::default()
    };
    {
        let cells = settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default();
        cells.insert("A".to_string(), cell("codex --a", BTreeMap::new()));
        cells.insert("C".to_string(), cell("codex --c", BTreeMap::new()));
    }

    // Request D with only A/C present -> falls back to C.
    let spawn = build_agent_spawn_command(&settings, "codex", None, Some("D")).unwrap();
    assert_eq!(spawn.profile_resolution.effective_profile, "C");
    assert_eq!(spawn.profile_resolution.cell.command, "codex --c");
    // The stamped hash is the EFFECTIVE (post-fallback C) cell composed with
    // the agent base command, not D. (#597)
    let agent_cfg = settings.agents.iter().find(|a| a.id == "codex").unwrap();
    let expected = profile_content_hash(
        &super::compose_effective_command(
            &agent_cfg.command,
            &spawn.profile_resolution.cell.command,
        ),
        &super::raw_merged_profile_env(agent_cfg, &spawn.profile_resolution.cell.env),
    );
    assert_eq!(spawn.profile_content_hash, expected);
}

#[test]
fn compose_effective_command_joins_and_drops_empty_sides() {
    assert_eq!(
        super::compose_effective_command("claude", "--x"),
        "claude --x"
    );
    assert_eq!(super::compose_effective_command("claude", ""), "claude");
    assert_eq!(super::compose_effective_command("", "--x"), "--x");
    assert_eq!(super::compose_effective_command("", ""), "");
    // Each side is trimmed; no double space at the seam.
    assert_eq!(
        super::compose_effective_command("  claude  ", "  --x  "),
        "claude --x"
    );
}

#[test]
fn build_spawn_empty_cell_uses_base_command_only() {
    let settings = settings_with_cell("codex --yolo", "A", cell("", BTreeMap::new()));
    let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
    assert_eq!(spawn.shell, "codex");
    assert_eq!(spawn.shell_args, vec!["--yolo"]);
}

#[test]
fn build_spawn_concatenation_handles_base_with_args_and_quoted_params() {
    // The whole `<base> <cell>` line tokenizes once; quotes/embedded spaces survive.
    let settings = settings_with_cell(
        "cmd.exe /c claude",
        "A",
        cell("--model \"gpt 5\"", BTreeMap::new()),
    );
    let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
    assert_eq!(spawn.shell, "cmd.exe");
    assert_eq!(spawn.shell_args, vec!["/c", "claude", "--model", "gpt 5"]);
}

#[test]
fn build_spawn_errors_when_base_and_cell_both_empty() {
    let settings = settings_with_cell("", "A", cell("", BTreeMap::new()));
    let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();
    assert!(
        err.contains("agent command is empty"),
        "both-empty must report an empty command: {err}"
    );
}

#[test]
fn profile_content_hash_changes_on_agent_base_env_edit() {
    // The base env is now in the hash; a value edit on an enabled row flips it.
    // LOWERCASE key so the Windows case-fold in raw_merged_profile_env does not
    // skew the assertion.
    fn settings_with_env_value(value: &str) -> AppSettings {
        let mut ag = agent("codex", "codex");
        ag.envs = vec![CodingAgentEnv {
            key: "kk".to_string(),
            value: value.to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        let mut settings = AppSettings {
            agents: vec![ag],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert("A".to_string(), cell("", BTreeMap::new()));
        settings
    }
    let h1 = build_agent_spawn_command(&settings_with_env_value("one"), "codex", None, Some("A"))
        .unwrap()
        .profile_content_hash;
    let h2 = build_agent_spawn_command(&settings_with_env_value("two"), "codex", None, Some("A"))
        .unwrap()
        .profile_content_hash;
    assert_ne!(h1, h2, "an agent base env edit must flip the hash");
}

#[test]
fn raw_merged_profile_env_overlays_cell_over_agent_raw() {
    let mut ag = agent("codex", "codex");
    ag.envs = vec![
        CodingAgentEnv {
            key: "ka".to_string(),
            value: "agent".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
        CodingAgentEnv {
            key: "kb".to_string(),
            value: "agent".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
        CodingAgentEnv {
            key: "koff".to_string(),
            value: "agent".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: false,
        },
    ];
    let cell_env = BTreeMap::from([
        ("kb".to_string(), "cell".to_string()),
        ("kc".to_string(), "cell".to_string()),
    ]);
    let merged = super::raw_merged_profile_env(&ag, &cell_env);
    // #2431 - keys fold to uppercase on every OS.
    let key = |k: &str| k.to_ascii_uppercase();
    assert_eq!(merged.get(&key("ka")).map(String::as_str), Some("agent"));
    assert_eq!(merged.get(&key("kb")).map(String::as_str), Some("cell")); // profile wins
    assert_eq!(merged.get(&key("kc")).map(String::as_str), Some("cell"));
    assert!(
        !merged.contains_key(&key("koff")),
        "disabled rows are excluded"
    );
}

#[test]
fn profile_content_hash_normalizes_env_keys_case_insensitively() {
    let mut lower = BTreeMap::new();
    lower.insert("Path".to_string(), "x".to_string());
    let mut upper = BTreeMap::new();
    upper.insert("PATH".to_string(), "x".to_string());
    assert_eq!(
        profile_content_hash("claude", &lower),
        profile_content_hash("claude", &upper),
        "a case-only env-key edit must not false-flag drift on any OS"
    );
}

// #2431 - portable profile digest.

#[test]
fn profile_content_hash_is_portable_across_path_shapes() {
    let env = BTreeMap::new();
    assert_eq!(
        profile_content_hash("C:\\tools\\claude.exe --Foo", &env),
        profile_content_hash("/usr/bin/claude --foo", &env)
    );
}

#[test]
fn profile_content_hash_keeps_argv_boundaries() {
    let env = BTreeMap::new();
    let h = |c: &str| profile_content_hash(c, &env);
    assert_ne!(h("claude \"a b\""), h("claude a b"));
    assert_ne!(h("claude \"\""), h("claude"));
    assert_ne!(h("claude a \"\" b"), h("claude a b"));
}

#[test]
fn profile_content_hash_env_values_stay_case_sensitive() {
    let lower = BTreeMap::from([("K".to_string(), "value".to_string())]);
    let upper = BTreeMap::from([("K".to_string(), "VALUE".to_string())]);
    assert_ne!(
        profile_content_hash("claude", &lower),
        profile_content_hash("claude", &upper)
    );
}

#[test]
fn profile_content_hash_ignores_disabled_env_rows() {
    let mut with_disabled = agent("codex", "claude");
    with_disabled.envs = vec![env_row("OFF", "x", false)];
    let plain = agent("codex", "claude");
    let c = cell("", BTreeMap::new());
    assert_eq!(
        super::cell_identity(&with_disabled, &c),
        super::cell_identity(&plain, &c)
    );
}

#[test]
fn env_precedence_is_platform_independent() {
    let mut ag = agent("codex", "claude");
    ag.envs = vec![env_row("path", "base", true)];
    let cell_env = BTreeMap::from([("PATH".to_string(), "cell".to_string())]);
    let merged = super::raw_merged_profile_env(&ag, &cell_env);
    assert_eq!(
        merged,
        BTreeMap::from([("PATH".to_string(), "cell".to_string())])
    );
}

#[test]
fn spawn_hash_equals_drift_recompute_for_a_windows_path_command() {
    let settings = settings_with_cell(
        "C:\\tools\\claude.exe --Foo",
        "A",
        cell("", BTreeMap::new()),
    );
    let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
    // The composition `commands::session::compute_profile_outdated` does by hand.
    let agent_cfg = settings.agents.iter().find(|a| a.id == "codex").unwrap();
    let drift = profile_content_hash(
        &super::compose_effective_command(
            &agent_cfg.command,
            &spawn.profile_resolution.cell.command,
        ),
        &super::raw_merged_profile_env(agent_cfg, &spawn.profile_resolution.cell.env),
    );
    assert_eq!(spawn.profile_content_hash, drift);
}

#[test]
fn profile_hash_preimage_tag_is_v2() {
    let preimage = super::profile_hash_preimage("claude --x", &BTreeMap::new());
    assert!(
        preimage.starts_with("v2\u{0}exe\u{0}claude\u{0}argc\u{0}1\u{0}a\u{0}--x\u{0}"),
        "{preimage:?}"
    );
    let fallback = super::profile_hash_preimage("claude \"unclosed", &BTreeMap::new());
    assert!(fallback.starts_with("v2\u{0}rawcmd\u{0}"), "{fallback:?}");
}

#[test]
fn canonical_command_text_round_trips_through_the_tokenizer() {
    // Vectors already canonical (lowercase exe basename, lowercase args), so
    // canonical_command_text(render(v)) must render v back unchanged.
    let table: &[&[&str]] = &[
        &["claude", "a b"],
        &["claude", "a", "b"],
        &["my tool", "--x"],
        &["it's", "--x"],
        &["back\\slash", "--x"],
        &["claude", "ends\\"],
        &["claude", "ends\\\\"],
        &["claude", "q\\\"x"],
        &["claude", "q\\\\\"x"],
        &["claude", "\"x"],
        &["claude", ""],
        &["claude", "a", "", "b"],
        &["claude", "c:\\x\\y"],
        &["claude", "it's here"],
    ];
    for row in table {
        let vector: Vec<String> = row.iter().map(|s| s.to_string()).collect();
        let text = vector
            .iter()
            .map(|t| super::render_command_token(t))
            .collect::<Vec<_>>()
            .join(" ");
        let parsed = normalize_legacy_agent_command(&text)
            .unwrap_or_else(|e| panic!("{text:?} failed to parse: {e}"));
        let mut round = vec![parsed.shell];
        round.extend(parsed.shell_args);
        assert_eq!(round, vector, "rendered {text:?}");
    }
    let canon = super::canonical_command_text;
    assert_ne!(canon("claude \"a b\""), canon("claude a b"));
    assert_eq!(canon("claude \"a b\""), "claude \"a b\"");
    assert_eq!(canon("claude a b"), "claude a b");
    assert_eq!(canon("\"/bin/my tool\""), "\"my tool\"");
    assert_ne!(canon("\"/bin/my tool\""), canon("/bin/my tool"));
    assert_eq!(canon("claude \"it's\""), "claude \"it's\"");
    assert_eq!(canon("C:\\tools\\Claude.EXE --Foo"), "claude --foo");
}

#[test]
fn cell_identity_matches_the_spawn_path() {
    let cell_env = BTreeMap::from([("kk".to_string(), "v".to_string())]);
    let mut settings = settings_with_cell("codex --base", "A", cell("--p \"x y\"", cell_env));
    settings.agents[0].envs = vec![env_row("Base", "b", true)];
    let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
    assert_eq!(
        super::cell_identity(&settings.agents[0], &spawn.profile_resolution.cell),
        spawn.profile_content_hash
    );
}

// #1551 - the shared program resolver lifted out of `commands::session`.

#[test]
fn resolve_program_explicit_path_requires_a_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("tool.exe");
    std::fs::write(&file, b"x").expect("write");
    assert_eq!(
        resolve_program(&file.to_string_lossy()),
        Some(file.clone()),
        "an explicit path to a file resolves to itself"
    );
    let missing = dir.path().join("missing.exe");
    assert_eq!(
        resolve_program(&missing.to_string_lossy()),
        None,
        "an explicit path that does not exist never falls back to PATH"
    );
    assert_eq!(
        resolve_program(&dir.path().to_string_lossy()),
        None,
        "a directory is not a program"
    );
}

#[test]
fn resolve_program_bare_name_uses_path() {
    let token = if cfg!(windows) { "cmd" } else { "sh" };
    let resolved = resolve_program(token).expect("a bare shell name resolves through PATH");
    let stem = resolved
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("file stem")
        .to_ascii_lowercase();
    assert_eq!(stem, token);
}

#[test]
fn agent_path_2589_resolve_prefers_the_inherited_winner() {
    use crate::config::agent_path::{compose, SearchPathInputs};

    let root = tempfile::tempdir().expect("tempdir");
    let inherited_dir = root.path().join("inherited");
    let added_dir = root.path().join("added");
    let name = "ac-2589-tool.exe";
    for dir in [&inherited_dir, &added_dir] {
        std::fs::create_dir_all(dir).expect("mkdir");
        let file = dir.join(name);
        std::fs::write(
            &file,
            b"#!/bin/sh
exit 0
",
        )
        .expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    let search_path = compose(&SearchPathInputs {
        inherited: Some(std::env::join_paths([&inherited_dir]).expect("join")),
        home: None,
        login_shell_path: Some(std::env::join_paths([&added_dir]).expect("join")),
    });
    assert_eq!(
        resolve_program_in(name, &search_path),
        Some(inherited_dir.join(name)),
        "the inherited PATH winner keeps winning"
    );
    let added_only = std::env::join_paths([&added_dir]).expect("join");
    assert_eq!(
        resolve_program_in(name, &added_only),
        Some(added_dir.join(name)),
        "positive control: the added dir alone resolves the token"
    );
}

#[test]
fn resolve_program_bare_unknown_is_none() {
    assert_eq!(resolve_program("definitely-not-a-program-1551"), None);
}

#[test]
fn is_bare_program_token_cases() {
    assert!(is_bare_program_token("claude"));
    assert!(!is_bare_program_token("./claude"));
    assert!(!is_bare_program_token(r"C:\x\claude.exe"));
    assert!(!is_bare_program_token("/usr/bin/claude"));
    assert!(!is_bare_program_token(r"bin\claude"));
}

// #2434 - tiered resolution of a stored coding-agent reference.
