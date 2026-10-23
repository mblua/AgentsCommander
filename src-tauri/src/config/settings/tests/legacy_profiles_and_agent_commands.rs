use super::*;

/// #2015: shared pre-v2 fixture: `letters.A.name` plus
/// `matrix.codex.A.argv` legacy data under `codingAgentProfiles`.
pub(super) const LEGACY_PROFILES_SETTINGS_FIXTURE: &str = r##"{
        "defaultShell": "bash",
        "defaultShellArgs": [],
        "rootToken": "existing-token",
        "agents": [{
            "id": "codex",
            "label": "Codex",
            "command": "codex",
            "color": "#000000"
        }],
        "codingAgentProfiles": {
            "schemaVersion": 1,
            "letters": { "A": { "name": "Baseline" } },
            "matrix": {
                "codex": {
                    "A": {
                        "enabled": true,
                        "argv": ["--model", "gpt'5"],
                        "env": {},
                        "notes": "legacy"
                    }
                }
            }
        }
    }"##;

#[test]
fn load_settings_backs_up_and_drops_legacy_profiles_shape() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, LEGACY_PROFILES_SETTINGS_FIXTURE).unwrap();

    let settings = super::super::load_settings_from_path(&path);

    assert_eq!(settings.root_token.as_deref(), Some("existing-token"));
    assert_eq!(settings.coding_agent_profiles.schema_version, 2);
    assert!(!settings
        .coding_agent_profiles
        .profiles_by_agent
        .get("codex")
        .is_some_and(|cells| cells.values().any(|cell| cell.command.contains("--model"))));

    let backup_path = temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json");
    assert_eq!(
        std::fs::read_to_string(backup_path).unwrap(),
        LEGACY_PROFILES_SETTINGS_FIXTURE
    );

    let saved_raw = std::fs::read_to_string(super::super::agents_instance_path(&path)).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved_raw).unwrap();
    assert_eq!(saved["codingAgentProfiles"]["schemaVersion"], 2);
    assert!(saved["codingAgentProfiles"].get("profileSlots").is_some());
    assert!(saved["codingAgentProfiles"]
        .get("profilesByBaseHash")
        .is_some());
    assert!(saved["codingAgentProfiles"]
        .get("profileLabelsByBaseHash")
        .is_some());
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(saved["codingAgentProfiles"].get(key).is_none());
    }
    assert_eq!(saved["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert!(saved["codingAgentProfiles"].get("matrix").is_none());
    assert!(saved["codingAgentProfiles"].get("letters").is_none());
}

#[test]
fn preserve_save_backs_up_legacy_disk_before_dropping_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, LEGACY_PROFILES_SETTINGS_FIXTURE).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, detected) = super::super::parse_settings_json(&contents, "test", None).unwrap();
    assert!(detected);

    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let backup_path = temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json");
    assert_eq!(
        std::fs::read_to_string(&backup_path).unwrap(),
        LEGACY_PROFILES_SETTINGS_FIXTURE
    );
    let saved_raw = std::fs::read_to_string(&path).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved_raw).unwrap();
    assert!(saved["codingAgentProfiles"].get("letters").is_none());
    assert!(saved["codingAgentProfiles"].get("matrix").is_none());

    // A later legacy-shaped disk file must not overwrite the first backup.
    std::fs::write(
        &path,
        LEGACY_PROFILES_SETTINGS_FIXTURE.replace("existing-token", "second-token"),
    )
    .unwrap();
    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&backup_path).unwrap(),
        LEGACY_PROFILES_SETTINGS_FIXTURE
    );
}

#[test]
fn reconcile_save_keeps_legacy_keys_and_writes_no_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, LEGACY_PROFILES_SETTINGS_FIXTURE).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, _) = super::super::parse_settings_json(&contents, "test", None).unwrap();

    super::super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();

    let saved_raw = std::fs::read_to_string(super::super::agents_instance_path(&path)).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved_raw).unwrap();
    assert!(saved["codingAgentProfiles"].get("letters").is_some());
    assert!(!temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json")
        .exists());
}

#[test]
fn legacy_profiles_shape_detection_is_exact() {
    let rows: [(serde_json::Value, bool); 16] = [
        (serde_json::json!({}), false),
        (serde_json::json!({"codingAgentProfiles": []}), false),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": 2}}),
            false,
        ),
        (serde_json::json!({"codingAgentProfiles": {}}), false),
        (
            serde_json::json!({"codingAgentProfiles": {
                "schemaVersion": 2,
                "profileSlots": {"A": {"label": "x"}},
                "profilesByAgent": {"claude": {"A": {"enabled": true, "command": "--m", "en\
                    v": {}, "notes": ""}}},
                "profileLabelsByAgent": {"claude": {"A": "L"}}
            }}),
            false,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": "1"}}),
            false,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"profileSlots": {"A": "name"}}}),
            false,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"profilesByAgent": {"claude": {"A": ["arg\
                v"]}}}}),
            false,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": 1}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"letters": {}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"matrix": {}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"agentDefaults": {}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": 2, "profileSlots": {"A": {"\
                name": "Baseline"}}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"profileSlots": {"A": {"label": "x", "nam\
                e": "y"}}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": 2, "profilesByAgent": {"cla\
                ude": {"A": {"argv": ["--model", "x"]}}}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"profilesByAgent": {"claude": {"B": {"arg\
                s": []}}}}}),
            true,
        ),
    ];

    for (value, expected) in rows {
        let before = value.clone();
        assert_eq!(
            super::super::legacy_profiles_shape_present(value.as_object().unwrap()),
            expected,
            "{value}"
        );
        assert_eq!(value, before, "the predicate must not mutate the value");
    }
}

#[test]
fn legacy_profiles_shape_warns_backs_up_and_keeps_v2_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let original = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "rootToken": "existing-token",
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex",
                "color": "#000000"
            }],
            "codingAgentProfiles": {
                "schemaVersion": 2,
                "profileSlots": { "A": { "name": "Baseline" } },
                "profileLabelsByAgent": { "codex": { "A": "Kept" } }
            }
        }"##;
    std::fs::write(&path, original).unwrap();

    let settings = super::super::load_settings_from_path(&path);

    let backup_path = temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json");
    assert_eq!(std::fs::read_to_string(backup_path).unwrap(), original);
    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["codex"]["A"],
        "Kept"
    );
    assert_eq!(settings.coding_agent_profiles.profile_slots["A"].label, "");
}

#[test]
fn profile_labels_survive_the_real_loader_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let original = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "rootToken": "round-trip-token",
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex",
                "color": "#000000"
            }],
            "codingAgentProfiles": {
                "schemaVersion": 2,
                "profileSlots": { "A": { "label": "" } },
                "defaultProfileByAgent": {},
                "profilesByAgent": {
                    "codex": {
                        "A": { "enabled": true, "command": "", "env": {}, "notes": "" }
                    }
                },
                "profileLabelsByAgent": { "codex": { "A": "First" } }
            }
        }"##;
    std::fs::write(&path, original).unwrap();

    let mut settings = super::super::load_settings_from_path(&path);
    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["codex"]["A"],
        "First"
    );

    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert("B".to_string(), "Second".to_string());
    super::super::save_settings_to_path_preserving_project_paths_typed(&settings, &path).unwrap();

    let reloaded = super::super::load_settings_from_path(&path);
    assert_eq!(
        reloaded.coding_agent_profiles.profile_labels_by_agent["codex"]["A"],
        "First"
    );
    assert_eq!(
        reloaded.coding_agent_profiles.profile_labels_by_agent["codex"]["B"],
        "Second"
    );
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(super::super::agents_instance_path(&path)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        raw["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::super::configuration_identity("codex", &[])]["A"],
        "First"
    );
    assert_eq!(
        raw["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::super::configuration_identity("codex", &[])]["B"],
        "Second"
    );
    assert!(!temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json")
        .exists());
}

#[test]
fn legacy_env_source_and_isolated_home_alias_serialize_as_v2_names() {
    let json = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex",
                "color": "#000000",
                "isolateCodexHome": true,
                "envs": [{
                    "key": "OPENAI_API_BASE",
                    "value": "https://example.test",
                    "source": "agentsCommander",
                    "enabled": true
                }]
            }]
        }"##;

    let (settings, _migrated) = super::super::parse_settings_json(json, "test", None).unwrap();
    let out = serde_json::to_string(&settings).unwrap();

    assert!(settings.agents[0].isolated_home);
    assert_eq!(
        settings.agents[0].envs[0].source,
        CodingAgentEnvSource::System
    );
    assert!(out.contains("\"isolatedHome\":true"));
    assert!(out.contains("\"source\":\"system\""));
    assert!(!out.contains("isolateCodexHome"));
    assert!(!out.contains("agentsCommander"));
}

#[test]
fn validate_agent_commands_allows_plain_claude() {
    let settings = settings_with_agents(&[("Claude", "claude")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn repair_profiles_adds_a_letter_and_a_cells_for_agents() {
    let mut settings = settings_with_agents(&[("Codex", "codex")]);
    settings.coding_agent_profiles.profile_slots.clear();
    settings.coding_agent_profiles.profile_slots.insert(
        "AA".to_string(),
        ProfileSlotConfig {
            label: "bad".into(),
        },
    );

    let changed =
        repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    assert!(changed);
    assert!(settings
        .coding_agent_profiles
        .profile_slots
        .contains_key("A"));
    assert!(!settings
        .coding_agent_profiles
        .profile_slots
        .contains_key("AA"));
    assert!(settings.coding_agent_profiles.profiles_by_agent["agent-0"].contains_key("A"));
}

#[test]
fn stale_full_settings_update_preserves_profiles_envs_and_isolation() {
    let mut current = settings_with_agents(&[("Codex", "codex")]);
    current.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    current.agents[0].isolated_home = true;
    current
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "B".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "codex --current".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    let mut stale = current.clone();
    stale.agents[0].envs.clear();
    stale.agents[0].isolated_home = false;
    stale.coding_agent_profiles.profiles_by_agent.clear();
    stale.sidebar_style = "command-center".to_string();

    let merged = merge_protected_coding_agent_settings(&current, stale);

    assert_eq!(merged.sidebar_style, "command-center");
    assert_eq!(merged.agents[0].envs[0].value, "current");
    assert!(merged.agents[0].isolated_home);
    assert!(merged.coding_agent_profiles.profiles_by_agent["agent-0"].contains_key("B"));
}

#[test]
fn validate_agent_commands_rejects_relative_codex_home_env() {
    let mut settings = settings_with_agents(&[("Codex", "codex")]);
    settings.agents[0].envs = vec![CodingAgentEnv {
        key: "CODEX_HOME".to_string(),
        value: "relative/path".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];

    let err = validate_agent_commands(&settings).unwrap_err();

    assert!(err.contains("CODEX_HOME must be an absolute path"), "{err}");
}

#[test]
fn validate_agent_commands_rejects_claude_continue() {
    let settings = settings_with_agents(&[("Claude", "claude --continue")]);
    let err = validate_agent_commands(&settings).unwrap_err();
    assert!(err.contains("Claude commands must not include --continue or -c"));
}

#[test]
fn validate_rejects_banned_continue_flag_in_cell_params() {
    // #597 - the cell holds params only; the provider token (claude) lives in
    // the base. The ban must still catch `--continue` typed in the cell, which
    // only works if validation runs on the COMPOSED command.
    let mut settings = settings_with_agents(&[("Claude", "claude")]);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--continue".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    let err = validate_agent_commands(&settings).unwrap_err();
    assert!(
        err.contains("must not include --continue"),
        "composed claude + --continue cell must be rejected: {err}"
    );
}

#[test]
fn validate_allows_continue_flag_when_base_is_not_claude() {
    // #597 - the --continue ban is claude-specific. With a codex base the
    // composed `codex --continue` is allowed, proving validation keys off the
    // composed provider token, not the raw cell text.
    let mut settings = settings_with_agents(&[("Codex", "codex")]);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--continue".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_allows_all_explicit_pi_session_controls_with_provider_overlap() {
    let selectors = [
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
    ];
    for provider in ["claude", "codex", "gemini"] {
        for selector in selectors {
            let command = format!("pi --provider {provider} {selector}");
            let settings = settings_with_agents(&[("Pi", command.as_str())]);
            assert!(
                validate_agent_commands(&settings).is_ok(),
                "provider={provider:?} selector={selector:?}"
            );
        }
    }
}

#[test]
fn validate_agent_commands_allows_explicit_pi_controls_in_composed_profile_cells() {
    let selectors = [
        "-c",
        "-r",
        "--continue",
        "--resume",
        "--session",
        "--session-id",
        "--fork",
        "--no-session",
    ];
    for provider in ["claude", "codex", "gemini"] {
        for selector in selectors {
            let base = format!("pi --provider {provider}");
            let mut settings = settings_with_agents(&[("Pi", base.as_str())]);
            settings
                .coding_agent_profiles
                .profiles_by_agent
                .entry("agent-0".to_string())
                .or_default()
                .insert(
                    "A".to_string(),
                    ProfileCellConfig {
                        id_hash: None,
                        id_hash_debug: None,
                        preflight: None,
                        preflight_timeout_seconds: None,
                        enabled: true,
                        command: format!("--model {provider} {selector}"),
                        env: BTreeMap::new(),
                        notes: String::new(),
                    },
                );
            assert!(
                validate_agent_commands(&settings).is_ok(),
                "provider={provider:?} selector={selector:?}"
            );
        }
    }
}

/// #1873 - a directly configured Muse recipe is user-authored: a Codex-looking
/// value plus manual `resume --last` passes, and the legacy Codex scan never
/// runs. Base recipe and composed profile cell behave the same.
#[test]
fn validate_agent_commands_allows_direct_muse_arguments_with_legacy_provider_collision() {
    for command in [
        "muse",
        "muse --workspace /tmp/codex resume --last",
        "/opt/muse/bin/muse --workspace /tmp/codex resume --last",
        "muse resume 0f3d2a5c-9c1e-4b7e-8f6a-2b1c3d4e5f60",
        "muse --no-session-log --continue -c",
        "muse --model claude-sonnet --provider agy --continue",
    ] {
        let settings = settings_with_agents(&[("Muse", command)]);
        assert!(
            validate_agent_commands(&settings).is_ok(),
            "command={command:?}"
        );
    }
    let mut settings = settings_with_agents(&[("Muse", "muse")]);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--workspace /tmp/codex resume --last".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    assert!(validate_agent_commands(&settings).is_ok());
}

/// #1873 - the same token sequence behind an `env` wrapper is not Muse, so
/// the pre-existing Codex manual-resume rejection still fires before spawn.
#[test]
fn validate_agent_commands_rejects_wrapped_muse_collision_before_spawn() {
    for command in [
        "env muse --workspace /tmp/codex resume --last",
        "cmd /C muse --workspace /tmp/codex resume --last",
    ] {
        let settings = settings_with_agents(&[("Muse", command)]);
        let err = validate_agent_commands(&settings).unwrap_err();
        assert_eq!(
            err,
            "Agent \"Muse\": Codex commands must not include resume or --last; AgentsCommander \
                    injects codex resume --last automatically",
            "command={command:?}"
        );
    }
}

#[test]
fn validate_agent_commands_allows_plain_codex() {
    let settings = settings_with_agents(&[("Codex", "codex")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_allows_codex_search() {
    let settings = settings_with_agents(&[("Codex", "codex --search")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_allows_explicit_codex_help() {
    let settings = settings_with_agents(&[("Codex", "codex help")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_rejects_codex_resume_last() {
    let settings = settings_with_agents(&[("Codex", "codex resume --last")]);
    let err = validate_agent_commands(&settings).unwrap_err();
    assert!(err.contains("Codex commands must not include resume or --last"));
}

#[test]
fn validate_agent_commands_rejects_cmd_wrapper_codex_resume_last() {
    let settings = settings_with_agents(&[("Codex", "cmd /C codex resume --last")]);
    let err = validate_agent_commands(&settings).unwrap_err();
    assert!(err.contains("Codex commands must not include resume or --last"));
}

#[test]
fn validate_agent_commands_allows_codex_config_value_with_resume_text() {
    let settings =
        settings_with_agents(&[("Codex", "codex -c instruction=\"resume later\" --search")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

// #529 - instructions filename validation surfaces at settings save (G8/Q6).

#[test]
fn validate_agent_commands_rejects_unsafe_instructions_filename() {
    for bad in [
        "../x.md",
        "a/b.md",
        "a\\b.md",
        "C:x.md",
        "CON.md",
        "last_ac_context.md",
    ] {
        let mut settings = settings_with_agents(&[("Codex", "codex")]);
        settings.agents[0].instructions_filename = Some(bad.to_string());
        let err = validate_agent_commands(&settings).unwrap_err();
        assert!(
            err.contains("instructions filename is invalid"),
            "{bad:?} should be rejected, got: {err}"
        );
    }
}

#[test]
fn validate_agent_commands_allows_safe_or_empty_instructions_filename() {
    // Valid names plus empty/whitespace (which means "use the default").
    for ok in ["AGENTS.md", "CLAUDE.md", "MyTeam.md", "  ", ""] {
        let mut settings = settings_with_agents(&[("Codex", "codex")]);
        settings.agents[0].instructions_filename = Some(ok.to_string());
        assert!(
            validate_agent_commands(&settings).is_ok(),
            "{ok:?} should be allowed"
        );
    }
    // Absent (None) is allowed too.
    let settings = settings_with_agents(&[("Codex", "codex")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn instructions_filename_serde_round_trips_and_omits_when_absent() {
    // Absent -> omitted (skip_serializing_if) and deserializes back to None.
    let mut settings = settings_with_agents(&[("Codex", "codex")]);
    let json = serde_json::to_string(&settings).unwrap();
    assert!(!json.contains("instructionsFilename"));
    let back: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.agents[0].instructions_filename, None);

    // Present -> serialized under the camelCase key and round-trips.
    settings.agents[0].instructions_filename = Some("Squad.md".to_string());
    let json = serde_json::to_string(&settings).unwrap();
    assert!(json.contains("\"instructionsFilename\":\"Squad.md\""));
    let back: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.agents[0].instructions_filename.as_deref(),
        Some("Squad.md")
    );
}
