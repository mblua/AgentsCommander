use super::*;

#[test]
fn issue_2859_optional_preflight_serde_preserves_legacy_and_raw_values() {
    let legacy = serde_json::json!({
        "id": "row", "label": "Row", "command": "codex", "color": "#000000"
    });
    for null_fields in [false, true] {
        let mut row = legacy.clone();
        let mut cell = serde_json::json!({});
        if null_fields {
            for value in [&mut row, &mut cell] {
                value["preflight"] = serde_json::Value::Null;
                value["preflightTimeoutSeconds"] = serde_json::Value::Null;
            }
        }
        let row: AgentConfig = serde_json::from_value(row).unwrap();
        let cell: ProfileCellConfig = serde_json::from_value(cell).unwrap();
        assert!(row.preflight.is_none() && row.preflight_timeout_seconds.is_none());
        assert!(cell.preflight.is_none() && cell.preflight_timeout_seconds.is_none());
        for value in [
            serde_json::to_value(row).unwrap(),
            serde_json::to_value(cell).unwrap(),
        ] {
            assert!(value.get("preflight").is_none());
            assert!(value.get("preflightTimeoutSeconds").is_none());
        }
    }
    for timeout in [1, 60, 123, u32::MAX] {
        for command in ["", "  ", " echo  raw command "] {
            let mut row = legacy.clone();
            row["preflight"] = serde_json::json!(command);
            row["preflightTimeoutSeconds"] = serde_json::json!(timeout);
            let row: AgentConfig = serde_json::from_value(row).unwrap();
            let cell: ProfileCellConfig = serde_json::from_value(serde_json::json!({
                "enabled": false, "preflight": command, "preflightTimeoutSeconds": timeout
            }))
            .unwrap();
            for value in [
                serde_json::to_value(row).unwrap(),
                serde_json::to_value(cell).unwrap(),
            ] {
                assert_eq!(value["preflight"], command);
                assert_eq!(value["preflightTimeoutSeconds"], timeout);
            }
        }
    }
}

#[test]
fn issue_2859_preflight_timeout_rejects_malformed_wire_values_at_both_levels() {
    for raw in [
        "-1",
        "1.5",
        "\"60\"",
        "4294967296",
        "NaN",
        "Infinity",
        "1e999",
    ] {
        let row = format!(
            concat!(
                r##"{{"id":"row","label":"Row","command":"codex","color":"#000000","##,
                r##""preflightTimeoutSeconds":{raw}}}"##
            ),
            raw = raw
        );
        let cell = format!(r#"{{"preflightTimeoutSeconds":{raw}}}"#);
        assert!(serde_json::from_str::<AgentConfig>(&row).is_err(), "{raw}");
        assert!(
            serde_json::from_str::<ProfileCellConfig>(&cell).is_err(),
            "{raw}"
        );
    }
    assert!(super::super::validate_preflight_timeout_seconds(Some(0)).is_err());
    for value in [None, Some(1), Some(60), Some(u32::MAX)] {
        super::super::validate_preflight_timeout_seconds(value).unwrap();
    }
}

#[test]
fn issue_2859_preflight_validation_checks_disabled_and_invalid_letter_before_repair() {
    let mut settings = settings_with_agents(&[("Row", "codex")]);
    settings.agents[0].preflight = Some("secret row command".into());
    settings.agents[0].preflight_timeout_seconds = Some(0);
    let error = super::super::validate_and_repair_settings(&mut settings).unwrap_err();
    assert!(error.contains("agent-0") && error.contains("preflightTimeoutSeconds"));
    assert!(!error.contains("secret"));
    settings.agents[0].preflight_timeout_seconds = None;
    let mut cell = super::super::empty_profile_cell();
    cell.enabled = false;
    cell.preflight = Some("secret cell command".into());
    cell.preflight_timeout_seconds = Some(0);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("orphan".into())
        .or_default()
        .insert("invalid-letter".into(), cell);
    let error = super::super::validate_and_repair_settings(&mut settings).unwrap_err();
    assert!(
        error.contains("orphan")
            && error.contains("invalid-letter")
            && error.contains("preflightTimeoutSeconds")
    );
    assert!(!error.contains("secret"));
    assert!(
        settings.coding_agent_profiles.profiles_by_agent["orphan"].contains_key("invalid-letter")
    );
}

#[test]
fn issue_2859_preflight_copy_repair_and_protected_merge_keep_row_cell_identity() {
    let mut settings = settings_with_agents(&[("First", "codex"), ("Second", "codex")]);
    for (index, row) in settings.agents.iter_mut().enumerate() {
        row.preflight = Some(format!(" raw  row {index} "));
        row.preflight_timeout_seconds = Some(60 + index as u32);
        let mut cell = super::super::empty_profile_cell();
        cell.enabled = false;
        cell.preflight_timeout_seconds = Some(100 + index as u32);
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry(row.id.clone())
            .or_default()
            .insert("B".into(), cell);
    }
    super::super::validate_and_repair_settings(&mut settings).unwrap();
    let copied = settings.clone();
    let merged = super::super::merge_protected_coding_agent_settings(&settings, copied);
    let restored: AppSettings =
        serde_json::from_value(serde_json::to_value(&merged).unwrap()).unwrap();
    for (index, row) in restored.agents.iter().enumerate() {
        assert_eq!(
            row.preflight.as_deref(),
            Some(format!(" raw  row {index} ").as_str())
        );
        assert_eq!(row.preflight_timeout_seconds, Some(60 + index as u32));
        let cell = &restored.coding_agent_profiles.profiles_by_agent[&row.id]["B"];
        assert_eq!(cell.preflight_timeout_seconds, Some(100 + index as u32));
        assert!(cell.preflight.is_none() && !cell.enabled);
    }
}

#[test]
fn issue_2859_preflight_overlay_save_retains_base_and_effective_whole_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::super::SETTINGS_FILE_NAME);
    let mut base = settings_with_agents(&[("Row", "codex")]);
    base.root_token = Some("test-token".into());
    base.agents[0].preflight = Some(" base  command ".into());
    base.agents[0].preflight_timeout_seconds = Some(60);
    super::super::validate_and_repair_settings(&mut base).unwrap();
    let mut cell = super::super::empty_profile_cell();
    cell.preflight_timeout_seconds = Some(123);
    base.coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .insert("B".into(), cell);
    let original = serde_json::to_string(&base).unwrap();
    std::fs::write(&path, &original).unwrap();
    let mut overlay = serde_json::to_value(&base).unwrap();
    overlay["agents"][0]["preflight"] = serde_json::json!(" overlay  command ");
    overlay["agents"][0]["preflightTimeoutSeconds"] = serde_json::json!(456);
    overlay["codingAgentProfiles"]["profilesByAgent"]["agent-0"]["B"]["preflightTimeoutSeconds"] =
        serde_json::json!(789);
    std::fs::write(
        dir.path()
            .join(super::super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        serde_json::to_vec(&serde_json::json!({
            "agents": overlay["agents"], "codingAgentProfiles": overlay["codingAgentProfiles"]
        }))
        .unwrap(),
    )
    .unwrap();
    let (mut effective, _) =
        super::super::parse_settings_json(&original, "test", Some(&path)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "load must not eagerly rewrite fields"
    );
    super::super::validate_and_repair_settings(&mut effective).unwrap();
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&effective, &path).unwrap();
    assert_eq!(
        written.agents[0].preflight.as_deref(),
        Some(" overlay  command ")
    );
    assert_eq!(written.agents[0].preflight_timeout_seconds, Some(456));
    assert_eq!(
        written.coding_agent_profiles.profiles_by_agent["agent-0"]["B"].preflight_timeout_seconds,
        Some(789)
    );
    std::fs::remove_file(
        dir.path()
            .join(super::super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
    )
    .unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let (base_again, _) =
        super::super::parse_settings_json(&contents, "test", Some(&path)).unwrap();
    assert_eq!(
        base_again.agents[0].preflight.as_deref(),
        Some(" base  command ")
    );
    assert_eq!(base_again.agents[0].preflight_timeout_seconds, Some(60));
    assert_eq!(
        base_again.coding_agent_profiles.profiles_by_agent["agent-0"]["B"]
            .preflight_timeout_seconds,
        Some(123)
    );
}

#[test]
fn issue_2859_legacy_load_does_not_persist_preflight_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::super::SETTINGS_FILE_NAME);
    let mut settings = settings_with_agents(&[("Row", "codex")]);
    settings.root_token = Some("test-token".into());
    super::super::validate_and_repair_settings(&mut settings).unwrap();
    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let agents_path = dir.path().join(super::super::AGENTS_INSTANCE_FILE_NAME);
    let before_settings = std::fs::read(&path).unwrap();
    let before_agents = std::fs::read(&agents_path).unwrap();
    let loaded = super::super::load_settings_from_path(&path);
    assert_eq!(loaded.agents.len(), 1);
    assert!(loaded.agents[0].preflight.is_none());
    assert!(loaded.agents[0].preflight_timeout_seconds.is_none());
    let cell = &loaded.coding_agent_profiles.profiles_by_agent["agent-0"]["A"];
    assert!(cell.preflight.is_none() && cell.preflight_timeout_seconds.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), before_settings);
    assert_eq!(std::fs::read(&agents_path).unwrap(), before_agents);
    assert!(!String::from_utf8(before_agents)
        .unwrap()
        .contains("preflight"));
}

pub(super) fn settings_with_agents(commands: &[(&str, &str)]) -> AppSettings {
    AppSettings {
        agents: commands
            .iter()
            .enumerate()
            .map(|(idx, (label, command))| AgentConfig {
                preflight: None,
                preflight_timeout_seconds: None,
                id: format!("agent-{idx}"),
                id_hash: None,
                id_hash_debug: None,
                label: (*label).to_string(),
                command: (*command).to_string(),
                color: "#000000".to_string(),
                order: None,
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                blocking_menus: None,
                backend: Default::default(),
            })
            .collect(),
        ..AppSettings::default()
    }
}

/// #2162 test 10: the save-time collision predicate. Decision logic only --
/// the `log::warn!` emission itself is declared not verified by test.
#[test]
fn dest_os_token_stem_detects_collisions() {
    for (dest, expected) in [
        (".claude.linux", Some(".claude")),
        (".claude.WINDOWS", Some(".claude")),
        (".claude.macos", Some(".claude")),
        (".claude", None),
        (".claudelinux", None),
        (".linux", Some("")),
    ] {
        assert_eq!(
            config_seed_dest_os_token_stem(dest),
            expected.map(|e: &str| e.to_string()),
            "dest {dest}"
        );
    }
}

#[test]
fn validate_config_seed_dest_accepts_dotfile_names() {
    for ok in [".claude", ".claude-amp", ".codex", ".opencode", "config"] {
        assert!(
            super::super::validate_config_seed_dest(ok).is_ok(),
            "should accept {ok:?}"
        );
    }
    // Leading/trailing whitespace is trimmed before validation.
    assert!(super::super::validate_config_seed_dest("  .claude  ").is_ok());
}

#[test]
fn validate_config_seed_dest_rejects_unsafe_names() {
    let bad = [
        "",                  // empty
        "   ",               // whitespace only
        ".config/opencode",  // forward separator (nested)
        ".config\\x",        // backslash separator
        "..",                // parent
        "a..b",              // contains ..
        "C:foo",             // drive-relative colon
        "x:y",               // ADS colon
        "%AC_REPLICA_ROOT%", // placeholder marker
        "$HOME",             // shell marker
        ".claude.",          // trailing dot (trim does NOT strip dots)
        "CON",               // reserved device
        "nul",               // reserved device (case-insensitive)
        "COM1",              // reserved device
        "lpt9.claude",       // reserved device before first dot
    ];
    for name in bad {
        assert!(
            super::super::validate_config_seed_dest(name).is_err(),
            "should reject {name:?}"
        );
    }
}

#[test]
fn config_seed_is_active_requires_enabled_and_nonempty_dest() {
    use super::super::ConfigSeedConfig;
    assert!(ConfigSeedConfig {
        enabled: true,
        dest: ".claude".to_string()
    }
    .is_active());
    assert!(!ConfigSeedConfig {
        enabled: false,
        dest: ".claude".to_string()
    }
    .is_active());
    assert!(!ConfigSeedConfig {
        enabled: true,
        dest: "   ".to_string()
    }
    .is_active());
}

#[test]
fn config_seed_serde_round_trips_camel_case_and_omits_when_absent() {
    use super::super::AgentConfig;
    // Present -> serializes as nested `configSeed { enabled, dest }`.
    let mut agent = AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "claude".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Claude".to_string(),
        command: "claude".to_string(),
        color: "#fff".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: Some(super::super::ConfigSeedConfig {
            enabled: true,
            dest: ".claude".to_string(),
        }),
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    };
    let json = serde_json::to_string(&agent).unwrap();
    assert!(json.contains("\"configSeed\""), "{json}");
    assert!(json.contains("\"dest\":\".claude\""), "{json}");
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.config_seed.as_ref().map(|c| c.dest.as_str()),
        Some(".claude")
    );

    // Absent -> key omitted (skip_serializing_if), and old files deserialize to None.
    agent.config_seed = None;
    let json = serde_json::to_string(&agent).unwrap();
    assert!(!json.contains("configSeed"), "{json}");
    let back: AgentConfig =
        serde_json::from_str(r##"{"id":"x","label":"X","command":"claude","color":"#000"}"##)
            .unwrap();
    assert!(back.config_seed.is_none());
}

#[test]
fn agent_backend_config_defaults_to_local_process_and_omits_default() {
    use super::super::AgentConfig;

    let agent: AgentConfig =
        serde_json::from_str(r##"{"id":"x","label":"X","command":"codex","color":"#000"}"##)
            .unwrap();

    assert_eq!(
        crate::pty::backend::SessionBackendKind::from(&agent.backend),
        crate::pty::backend::SessionBackendKind::LocalProcess
    );
    let json = serde_json::to_string(&agent).unwrap();
    assert!(!json.contains("backend"), "{json}");

    let mut local_with_image = agent;
    local_with_image.backend.image = Some("hand-edited:latest".to_string());
    let json = serde_json::to_string(&local_with_image).unwrap();
    assert!(!json.contains("backend"), "{json}");
}

#[test]
fn agent_backend_config_serializes_container_image() {
    use super::super::AgentConfig;

    let mut settings = settings_with_agents(&[("Claude", "claude")]);
    settings.agents[0].backend.kind = crate::pty::backend::SessionBackendKind::ContainerTransport;
    settings.agents[0].backend.image = Some("agentscommander/ac-claude:latest".to_string());
    super::super::validate_and_repair_settings(&mut settings).unwrap();

    let json = serde_json::to_string(&settings.agents[0]).unwrap();
    assert!(json.contains("\"backend\""), "{json}");
    assert!(json.contains("\"kind\":\"containerTransport\""), "{json}");
    assert!(
        json.contains("\"image\":\"agentscommander/ac-claude:latest\""),
        "{json}"
    );
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.backend.image.as_deref(),
        Some("agentscommander/ac-claude:latest")
    );
}

#[test]
fn validate_and_repair_normalizes_backend_image_edges() {
    let mut blank_container = settings_with_agents(&[("Claude", "claude")]);
    blank_container.agents[0].backend.kind =
        crate::pty::backend::SessionBackendKind::ContainerTransport;
    blank_container.agents[0].backend.image = Some("   ".to_string());
    super::super::validate_and_repair_settings(&mut blank_container).unwrap();
    assert!(blank_container.agents[0].backend.image.is_none());

    let mut local_with_image = settings_with_agents(&[("Codex", "codex")]);
    local_with_image.agents[0].backend.image = Some(" custom:latest ".to_string());
    super::super::validate_and_repair_settings(&mut local_with_image).unwrap();
    assert!(local_with_image.agents[0].backend.image.is_none());
    let json = serde_json::to_string(&local_with_image.agents[0]).unwrap();
    assert!(!json.contains("backend"), "{json}");
}

#[test]
fn validate_and_repair_rejects_leading_dash_container_image() {
    let mut settings = settings_with_agents(&[("Claude", "claude")]);
    settings.agents[0].backend.kind = crate::pty::backend::SessionBackendKind::ContainerTransport;
    settings.agents[0].backend.image = Some(" --privileged".to_string());

    let err = super::super::validate_and_repair_settings(&mut settings).unwrap_err();
    assert!(err.contains("must not start with"), "{err}");
}

#[test]
fn screenshot_hotkey_defaults_when_absent() {
    // #714 an old settings file without the key deserializes to "Ctrl+Q".
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("screenshotCaptureHotkey");
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.screenshot_capture_hotkey, "Ctrl+Q");
}

#[test]
fn screenshot_hotkey_round_trips_camel_case() {
    let s = AppSettings {
        screenshot_capture_hotkey: "Control+P".to_string(),
        ..AppSettings::default()
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        json.contains("\"screenshotCaptureHotkey\":\"Control+P\""),
        "{json}"
    );
    let back: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.screenshot_capture_hotkey, "Control+P");
}

#[test]
fn validate_and_repair_rejects_invalid_screenshot_hotkey() {
    let mut s = AppSettings {
        screenshot_capture_hotkey: "Ctrl+Shift+Q".to_string(),
        ..AppSettings::default()
    };
    assert!(super::super::validate_and_repair_settings(&mut s).is_err());

    s.screenshot_capture_hotkey = "Ctrl+Q".to_string();
    assert!(super::super::validate_and_repair_settings(&mut s).is_ok());
}

/// #2281 Cross-language parity contract (plan section "Cross-language
/// validator parity"): the 22 accepted letters `A-Z` minus `W/R/C/V` in
/// upper case, plus four mixed-case/padded spellings, so 26 entries.
/// `src/shared/app-hotkey.test.ts` (phase 5) re-asserts this list verbatim.
const PARITY_ACCEPTED: [&str; 26] = [
    "Ctrl+Shift+A",
    "Ctrl+Shift+B",
    "Ctrl+Shift+D",
    "Ctrl+Shift+E",
    "Ctrl+Shift+F",
    "Ctrl+Shift+G",
    "Ctrl+Shift+H",
    "Ctrl+Shift+I",
    "Ctrl+Shift+J",
    "Ctrl+Shift+K",
    "Ctrl+Shift+L",
    "Ctrl+Shift+M",
    "Ctrl+Shift+N",
    "Ctrl+Shift+O",
    "Ctrl+Shift+P",
    "Ctrl+Shift+Q",
    "Ctrl+Shift+S",
    "Ctrl+Shift+T",
    "Ctrl+Shift+U",
    "Ctrl+Shift+X",
    "Ctrl+Shift+Y",
    "Ctrl+Shift+Z",
    "ctrl+shift+e",
    "CTRL+SHIFT+E",
    "Control+Shift+Z",
    " Ctrl + Shift + E ",
];

/// #2281 The 13 rejected literals in the plan's numbered order. Entry 6 is
/// the digit leak, entry 5 proves the reserved check is case-insensitive and
/// entry 13 is the Kelvin sign U+212A, which a `to_uppercase()` mirror would
/// wrongly accept as `K`.
const PARITY_REJECTED: [&str; 13] = [
    "Ctrl+Shift+W",
    "Ctrl+Shift+R",
    "Ctrl+Shift+C",
    "Ctrl+Shift+V",
    "ctrl+shift+w",
    "Ctrl+Shift+2",
    "Ctrl+E",
    "Shift+E",
    "Alt+Shift+E",
    "Ctrl+Shift+EE",
    "Ctrl+Shift+",
    "",
    "Ctrl+Shift+\u{212A}",
];

#[test]
fn sidebar_compact_hotkey_defaults_when_absent() {
    // #2281 an old settings file without the key deserializes to "Ctrl+Shift+E".
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("sidebarCompactHotkey");
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.sidebar_compact_hotkey, "Ctrl+Shift+E");
}

#[test]
fn sidebar_compact_hotkey_round_trips_camel_case() {
    let s = AppSettings {
        sidebar_compact_hotkey: "Control+Shift+B".to_string(),
        ..AppSettings::default()
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        json.contains("\"sidebarCompactHotkey\":\"Control+Shift+B\""),
        "{json}"
    );
    let back: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.sidebar_compact_hotkey, "Control+Shift+B");
}

#[test]
fn sidebar_compact_hotkey_accepts_the_parity_table() {
    assert_eq!(PARITY_ACCEPTED.len(), 26);
    for value in PARITY_ACCEPTED {
        assert!(
            super::super::validate_sidebar_compact_hotkey(value).is_ok(),
            "expected Ok for {value:?}"
        );
    }
}

#[test]
fn sidebar_compact_hotkey_rejects_the_parity_table() {
    assert_eq!(PARITY_REJECTED.len(), 13);
    for value in PARITY_REJECTED {
        let err = super::super::validate_sidebar_compact_hotkey(value)
            .expect_err(&format!("expected Err for {value:?}"));
        assert!(
            err.starts_with("Sidebar compact hotkey: "),
            "unexpected error for {value:?}: {err}"
        );
    }
}

#[test]
fn sidebar_compact_hotkey_call_site_preserves_a_valid_value() {
    // Positive control: a validator that rejected everything would pass the
    // reject table, so prove a valid non-default value survives the call site.
    let mut s = AppSettings {
        sidebar_compact_hotkey: "Ctrl+Shift+B".to_string(),
        ..AppSettings::default()
    };
    assert!(super::super::validate_and_repair_settings(&mut s).is_ok());
    assert_eq!(s.sidebar_compact_hotkey, "Ctrl+Shift+B");
}

#[test]
fn sidebar_compact_hotkey_call_site_rejects_an_invalid_value() {
    // Negative control: proves the validator is wired into
    // `validate_and_repair_settings` and not merely defined.
    let mut s = AppSettings {
        sidebar_compact_hotkey: "Ctrl+Shift+W".to_string(),
        ..AppSettings::default()
    };
    let err = super::super::validate_and_repair_settings(&mut s).unwrap_err();
    assert!(err.contains("Sidebar compact hotkey"), "{err}");
}

#[test]
fn room_number_mask_defaults_when_absent() {
    // #2507 an old settings file without the key deserializes to "#".
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    assert!(value
        .as_object_mut()
        .unwrap()
        .remove("roomNumberMask")
        .is_some());
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.room_number_mask, "#");
}

#[test]
fn room_number_mask_round_trips_camel_case() {
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value["roomNumberMask"] = serde_json::json!("####");
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    let back = serde_json::to_value(&parsed).unwrap();
    assert_eq!(back["roomNumberMask"], serde_json::json!("####"));
}

#[test]
fn room_number_mask_accepts_and_rejects_the_parity_table() {
    for (value, width) in [("#", 1), ("##", 2), ("#########", 9)] {
        assert_eq!(super::super::validate_room_number_mask(value), Ok(width));
    }
    for value in ["", " ##", "##1", "2", "#-#", "##########"] {
        let err = super::super::validate_room_number_mask(value)
            .expect_err(&format!("expected Err for {value:?}"));
        assert!(
            err.starts_with("Room number mask: "),
            "unexpected error for {value:?}: {err}"
        );
    }
}

#[test]
fn room_number_mask_call_site_blocks_an_invalid_save() {
    let mut bad = AppSettings {
        room_number_mask: "2".to_string(),
        ..AppSettings::default()
    };
    let err = super::super::validate_and_repair_settings(&mut bad).unwrap_err();
    assert!(err.starts_with("Room number mask: "), "{err}");
    // Anti-vacuous control: the same value with a valid mask passes.
    let mut good = AppSettings {
        room_number_mask: "###".to_string(),
        ..AppSettings::default()
    };
    assert!(super::super::validate_and_repair_settings(&mut good).is_ok());
    assert_eq!(good.room_number_mask, "###");
}

#[test]
fn screenshot_and_sidebar_compact_hotkeys_are_independent() {
    let mut s = AppSettings {
        sidebar_compact_hotkey: "Ctrl+Shift+B".to_string(),
        ..AppSettings::default()
    };
    assert_eq!(s.screenshot_capture_hotkey, "Ctrl+Q");
    s.screenshot_capture_hotkey = "Ctrl+P".to_string();
    assert_eq!(s.sidebar_compact_hotkey, "Ctrl+Shift+B");
    assert!(super::super::validate_and_repair_settings(&mut s).is_ok());
    assert_eq!(s.sidebar_compact_hotkey, "Ctrl+Shift+B");
    assert_eq!(s.screenshot_capture_hotkey, "Ctrl+P");
}

#[test]
fn typing_hold_seconds_defaults_to_30_when_absent() {
    // #2336 an old settings file without the key deserializes to 30.
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value.as_object_mut().unwrap().remove("typingHoldSeconds");
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.typing_hold_seconds, 30);

    let json = serde_json::to_string(&AppSettings::default()).unwrap();
    assert!(json.contains("\"typingHoldSeconds\":30"), "{json}");
}

#[test]
fn validate_and_repair_accepts_typing_hold_bounds_and_rejects_out_of_range() {
    for seconds in [
        super::super::TYPING_HOLD_SECONDS_MIN,
        30,
        super::super::TYPING_HOLD_SECONDS_MAX,
    ] {
        let mut s = AppSettings {
            typing_hold_seconds: seconds,
            ..AppSettings::default()
        };
        assert!(
            super::super::validate_typing_hold_settings(&s).is_ok(),
            "{seconds}"
        );
        assert!(
            super::super::validate_and_repair_settings(&mut s).is_ok(),
            "{seconds}"
        );
    }
    for seconds in [0, super::super::TYPING_HOLD_SECONDS_MAX + 1] {
        let s = AppSettings {
            typing_hold_seconds: seconds,
            ..AppSettings::default()
        };
        let err = super::super::validate_typing_hold_settings(&s).unwrap_err();
        assert!(err.contains("typingHoldSeconds"), "{err}");
        let mut repaired = s.clone();
        assert!(
            super::super::validate_and_repair_settings(&mut repaired).is_err(),
            "{seconds}"
        );
    }
}

/// #2336 - the Settings update path rejects an out-of-range typing-hold
/// window and leaves the live value unchanged: the error returns before the
/// save, so the persisted (and live) window stays at the old value. The
/// valid 1/3600 boundaries are covered by the validation test above.
#[tokio::test]
async fn invalid_typing_hold_settings_update_leaves_live_settings_unchanged() {
    let original = AppSettings::default();
    assert_eq!(original.typing_hold_seconds, 30);
    let state: super::super::SettingsState =
        crate::config::settings::new_settings_state(original.clone());

    let mut invalid = original;
    invalid.typing_hold_seconds = 0;
    let err = crate::commands::config::persist_protected_settings_update(&state, invalid)
        .await
        .expect_err("out-of-range typing hold must be rejected");
    assert!(err.contains("typingHoldSeconds"), "{err}");
    assert_eq!(state.read().await.typing_hold_seconds, 30);
}
