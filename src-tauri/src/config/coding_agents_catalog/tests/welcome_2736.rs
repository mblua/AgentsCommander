use super::*;

#[test]
fn command_account_key_separates_two_paths_to_the_same_name() {
    assert_eq!(
        command_account_key("/a/claude"),
        Some("/a/claude".to_string())
    );
    assert_ne!(
        command_account_key("/a/claude"),
        command_account_key("claude")
    );
}

#[test]
fn command_account_key_is_none_for_an_empty_command() {
    assert_eq!(command_account_key(""), None);
    assert_eq!(command_account_key("   "), None);
}

#[test]
fn command_account_key_honours_quoting() {
    assert_eq!(
        command_account_key("\"C:/Program Files/a/claude.exe\" --x"),
        Some("C:/Program Files/a/claude.exe".to_string())
    );
}

#[test]
fn tested_level_2736_table_keys_equal_the_enabled_support_rows_in_order() {
    let tested: Vec<&str> = BUILTIN_TESTED_LEVEL.iter().map(|(key, _)| *key).collect();
    let enabled: Vec<&str> = BUILTIN_AGENT_SUPPORT
        .iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(key, _)| *key)
        .collect();
    assert_eq!(tested, enabled);
}

#[test]
fn tested_level_2736_assigns_high_medium_low_exactly_as_specified() {
    for key in ["claude", "codex", "pi"] {
        assert_eq!(tested_level_for(key), Some(TestedLevel::High), "{key}");
    }
    assert_eq!(tested_level_for("antigravity"), Some(TestedLevel::Medium));
    for key in ["hermes", "cursor", "opencode", "grok"] {
        assert_eq!(tested_level_for(key), Some(TestedLevel::Low), "{key}");
    }
    assert_eq!(tested_level_for("muse"), None);
    assert_eq!(tested_level_for("my-agent"), None);
}

#[test]
fn tested_level_2736_serializes_to_lowercase_wire_strings() {
    assert_eq!(
        serde_json::to_value(TestedLevel::High).unwrap(),
        serde_json::json!("high")
    );
    assert_eq!(
        serde_json::to_value(TestedLevel::Medium).unwrap(),
        serde_json::json!("medium")
    );
    assert_eq!(
        serde_json::to_value(TestedLevel::Low).unwrap(),
        serde_json::json!("low")
    );
}

#[test]
fn install_command_2736_prefers_the_platform_key_over_default() {
    let commands = InstallCommands {
        default: "npm i -g x".to_string(),
        windows: Some("winget install x".to_string()),
        macos: None,
        linux: None,
    };
    assert_eq!(
        install_command_for_os(&commands, "windows"),
        "winget install x"
    );
    assert_eq!(install_command_for_os(&commands, "macos"), "npm i -g x");
    assert_eq!(install_command_for_os(&commands, "linux"), "npm i -g x");
    let all = InstallCommands {
        default: "d".to_string(),
        windows: Some("w".to_string()),
        macos: Some("m".to_string()),
        linux: Some("l".to_string()),
    };
    assert_eq!(install_command_for_os(&all, "windows"), "w");
    assert_eq!(install_command_for_os(&all, "macos"), "m");
    assert_eq!(install_command_for_os(&all, "linux"), "l");
}

#[test]
fn install_command_2736_unknown_os_name_falls_back_to_default() {
    let all = InstallCommands {
        default: "d".to_string(),
        windows: Some("w".to_string()),
        macos: Some("m".to_string()),
        linux: Some("l".to_string()),
    };
    assert_eq!(install_command_for_os(&all, "freebsd"), "d");
    assert_eq!(install_command_for_os(&all, ""), "d");
}

#[test]
fn command_is_present_2736_true_for_a_resolvable_token_false_otherwise() {
    let stub = |token: &str| (token == "claude").then(|| PathBuf::from("/bin/claude"));
    assert!(command_is_present_with("claude", stub));
    assert!(command_is_present_with("claude --flag", stub));
    assert!(!command_is_present_with("codex", stub));
    assert!(!command_is_present_with("", stub));
    assert!(!command_is_present_with("   ", stub));
}

#[test]
fn command_is_present_2736_executes_no_process() {
    // The injected closure is the ONLY resolution step: no version probe
    // (which would spawn a process) sits on this path.
    let mut calls = Vec::new();
    let present = command_is_present_with("\"my tool\" --x", |token| {
        calls.push(token.to_string());
        None
    });
    assert!(!present);
    assert_eq!(calls, vec!["my tool".to_string()]);
    let mut empty_calls = 0;
    assert!(!command_is_present_with("", |_| {
        empty_calls += 1;
        Some(PathBuf::from("x"))
    }));
    assert_eq!(empty_calls, 0);
}

#[test]
fn welcome_status_2736_row_per_entry_in_catalog_order_with_computed_fields() {
    let catalog = vec![
        welcome_def_2736(
            "codex",
            "codex",
            Some(serde_json::json!({ "default": "npm i -g codex" })),
        ),
        welcome_def_2736("claude", "claude --x", None),
        welcome_def_2736(
            "my-agent",
            "my-agent",
            Some(serde_json::json!({ "default": "d", "windows": "d", "macos": "d", "linux": "d" })),
        ),
    ];
    let rows = welcome_status_for_with(&catalog, |command| command == "claude --x");
    assert_eq!(
        rows,
        vec![
            CodingAgentWelcomeStatus {
                key: "codex".to_string(),
                installed: false,
                tested_level: Some(TestedLevel::High),
                install_command: Some("npm i -g codex".to_string()),
            },
            CodingAgentWelcomeStatus {
                key: "claude".to_string(),
                installed: true,
                tested_level: Some(TestedLevel::High),
                install_command: None,
            },
            CodingAgentWelcomeStatus {
                key: "my-agent".to_string(),
                installed: false,
                tested_level: None,
                install_command: Some("d".to_string()),
            },
        ]
    );
    let wire = serde_json::to_value(&rows).unwrap();
    assert_eq!(wire[0]["testedLevel"], serde_json::json!("high"));
    assert_eq!(wire[1]["installCommand"], serde_json::Value::Null);
    assert_eq!(wire[2]["testedLevel"], serde_json::Value::Null);
}

#[test]
fn welcome_status_2736_never_adds_a_field_to_the_persisted_definition() {
    let def = welcome_def_2736(
        "claude",
        "claude",
        Some(serde_json::json!({ "default": "d" })),
    );
    let value = serde_json::to_value(&def).unwrap();
    let object = value.as_object().expect("definition object");
    assert!(!object.contains_key("testedLevel"));
    assert!(!object.contains_key("installed"));
}
