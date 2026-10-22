use super::*;

#[test]
fn install_commands_2736_optional_absent_deserializes_to_none() {
    let mut plain = shipped_def_json(&["claude"]).remove(0);
    plain.as_object_mut().unwrap().remove("installCommands");
    let definition: CodingAgentDefinition = serde_json::from_value(plain).unwrap();
    assert!(definition.install_commands.is_none());
    assert!(validate_definition(&definition).is_ok());
    assert!(!serde_json::to_string(&definition)
        .unwrap()
        .contains("installCommands"));
}

#[test]
fn install_commands_2736_rejects_a_blank_or_control_character_value() {
    let base: CodingAgentDefinition =
        serde_json::from_value(shipped_def_json(&["claude"]).remove(0)).unwrap();
    let cases = [
        (
            InstallCommands {
                default: "   ".to_string(),
                windows: None,
                macos: None,
                linux: None,
            },
            "   ",
        ),
        (
            InstallCommands {
                default: "npm i -g ok".to_string(),
                windows: Some("secretwin\u{7}cmd".to_string()),
                macos: None,
                linux: None,
            },
            "secretwin",
        ),
        (
            InstallCommands {
                default: "npm i -g ok".to_string(),
                windows: None,
                macos: Some("secretmac\u{2028}cmd".to_string()),
                linux: None,
            },
            "secretmac",
        ),
    ];
    for (install, secret) in cases {
        let mut def = base.clone();
        def.install_commands = Some(install);
        let error = validate_definition(&def).expect_err("must be rejected");
        assert!(error.contains("installCommands."), "{error}");
        if !secret.trim().is_empty() {
            assert!(!error.contains(secret), "error echoes the value: {error}");
        }
        let reason = definition_problem_reason(&def);
        assert!(
            !reason.contains(secret) || secret.trim().is_empty(),
            "{reason}"
        );
        let raw = serde_json::to_value(&def).unwrap();
        let problem = raw_install_commands_problem(&raw).expect("raw problem");
        if !secret.trim().is_empty() {
            assert!(!problem.contains(secret), "{problem}");
        }
    }
}

#[test]
fn install_commands_2736_requires_default_when_the_object_is_present() {
    let error = serde_json::from_value::<InstallCommands>(serde_json::json!({"windows": "x"}))
        .expect_err("default is required");
    assert!(
        error.to_string().contains("missing field `default`"),
        "{error}"
    );

    let mut bad = shipped_def_json(&["claude"]).remove(0);
    bad["installCommands"] = serde_json::json!({"windows": "x"});
    let good = shipped_def_json(&["codex"]).remove(0);
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(&serde_json::to_string(&vec![bad, good]).unwrap()),
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert!(report.catalog.iter().all(|def| def.key != "claude"));
    assert!(report.catalog.iter().any(|def| def.key == "codex"));
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == "invalidDefinition"),
        "{:?}",
        report.warnings
    );
}

#[test]
fn install_commands_2736_base_entry_with_a_non_object_value_is_omitted_with_a_warning() {
    let mut raw = shipped_def_json(&["claude"]).remove(0);
    raw["installCommands"] = serde_json::json!("npm install -g secret-pkg");
    let problem = raw_install_commands_problem(&raw).expect("non-object is a problem");
    assert!(!problem.contains("secret-pkg"), "{problem}");
    for value in [
        serde_json::json!({"default": 7}),
        serde_json::json!({"default": "ok", "linux": 7}),
        serde_json::json!({"default": "ok", "linux": "  "}),
    ] {
        raw["installCommands"] = value.clone();
        assert!(raw_install_commands_problem(&raw).is_some(), "{value}");
    }
    raw["installCommands"] = serde_json::json!({"default": "ok"});
    assert!(raw_install_commands_problem(&raw).is_none());

    let mut bad = shipped_def_json(&["claude"]).remove(0);
    bad["installCommands"] = serde_json::json!("npm install -g secret-pkg");
    let good = shipped_def_json(&["codex"]).remove(0);
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(&serde_json::to_string(&vec![bad, good]).unwrap()),
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert_eq!(report.catalog.len(), 1);
    assert_eq!(report.catalog[0].key, "codex");
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.code == "invalidDefinition")
        .expect("invalidDefinition");
    assert!(
        warning.reason.contains("installCommands"),
        "{}",
        warning.reason
    );
    assert!(!warning.reason.contains("secret-pkg"), "{}", warning.reason);
}

#[test]
pub(crate) fn install_commands_2736_local_layer_patches_windows_and_inherits_default() {
    let dir = seed_dir();
    managed_claude_base_2736(dir.path());
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":{"windows":"winget install claude"}}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let ic = claude_of(&report)
        .install_commands
        .as_ref()
        .expect("composed");
    assert_eq!(ic.default, expected_install_2736("claude").unwrap());
    assert_eq!(ic.windows.as_deref(), Some("winget install claude"));
    let expected = scope_b_expected_commands_2800("claude").unwrap();
    assert_eq!(ic.macos.as_deref(), expected["macos"].as_str());
    assert_eq!(ic.linux.as_deref(), expected["linux"].as_str());
}

#[test]
fn install_commands_2736_local_null_clears_the_object() {
    let dir = seed_dir();
    managed_claude_base_2736(dir.path());
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":null}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(claude_of(&report).install_commands.is_none());
}

#[test]
fn install_commands_2736_local_null_on_one_os_key_clears_only_that_key() {
    let dir = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["installCommands"] = serde_json::json!({
        "default": "npm install -g @anthropic-ai/claude-code",
        "windows": "win cmd",
        "linux": "linux cmd"
    });
    write_managed_base(dir.path(), &[claude], "stale-revision", true);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":{"windows":null}}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(
        report.warnings.iter().all(|w| w.code != "localInvalid"),
        "{:?}",
        report.warnings
    );
    let ic = claude_of(&report).install_commands.as_ref().expect("kept");
    assert_eq!(ic.default, "npm install -g @anthropic-ai/claude-code");
    assert!(ic.windows.is_none(), "windows cleared");
    assert_eq!(ic.linux.as_deref(), Some("linux cmd"), "linux inherited");
}

#[test]
pub(crate) fn install_commands_2736_one_bad_local_value_discards_the_entire_local_layer() {
    let dir = seed_dir();
    let revision = managed_content_sha256(&supported_shipped_definitions());
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude", "codex"]),
        &revision,
        true,
    );
    for local in [
        r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"windows":"  "}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"default":7}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":"npm i"}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"default":""}}]}"##,
    ] {
        write_local(dir.path(), local);
        let report = load_catalog_report(dir.path());
        assert!(
            report.warnings.iter().any(|w| w.code == "localInvalid"),
            "{local}: {:?}",
            report.warnings
        );
        let codex = report
            .catalog
            .iter()
            .find(|def| def.key == "codex")
            .unwrap();
        assert_eq!(
            codex.label, "Codex",
            "{local}: the valid row is NOT applied"
        );
        assert_eq!(
            claude_of(&report)
                .install_commands
                .as_ref()
                .unwrap()
                .default,
            expected_install_2736("claude").unwrap(),
            "{local}: the base entry stays intact"
        );
    }
}

#[test]
fn install_commands_2736_new_local_key_without_install_commands_is_accepted() {
    let fields = parse_local_fields(
        serde_json::json!({
            "key": "mine", "label": "Mine", "description": "d", "color": "#111",
            "command": "mytool", "envs": [], "isolatedHome": false,
            "removable": true, "updateCommands": [], "autoUpdate": false
        })
        .as_object()
        .unwrap(),
        "local agents[0]",
    )
    .expect("valid new row");
    let definition = build_new_definition("mine", &fields, LayerDescription::Local)
        .expect("not missing anything");
    assert!(definition.install_commands.is_none());

    let dir = seed_dir();
    managed_claude_base_2736(dir.path());
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let mine = report.catalog.iter().find(|def| def.key == "mine").unwrap();
    assert!(mine.install_commands.is_none());
}

#[test]
fn install_commands_2736_unknown_nested_field_is_a_whole_layer_error() {
    let dir = seed_dir();
    managed_claude_base_2736(dir.path());
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MINE","installCommands":{"default":"x","freebsd":"y"}}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(
        report.warnings.iter().any(|w| w.code == "localInvalid"),
        "{:?}",
        report.warnings
    );
    assert_eq!(claude_of(&report).label, "Claude Code");
}

#[test]
pub(crate) fn install_commands_2736_managed_base_refreshes_from_the_previous_revision() {
    let old: Vec<CodingAgentDefinition> = supported_shipped_definitions()
        .into_iter()
        .map(|mut def| {
            def.install_commands = None;
            def
        })
        .collect();
    let old_revision = managed_content_sha256(&old);
    let new_revision = managed_content_sha256(&supported_shipped_definitions());
    assert_ne!(old_revision, new_revision);
    let old_json: Vec<serde_json::Value> = old
        .iter()
        .map(|def| serde_json::to_value(def).unwrap())
        .collect();
    let dir = seed_dir();
    write_managed_base(dir.path(), &old_json, &old_revision, true);

    let report = load_catalog_report(dir.path());
    assert!(
        report.warnings.iter().any(|w| w.code == "refreshFailed"),
        "stale revision is reported: {:?}",
        report.warnings
    );
    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "stale base refreshes"
    );
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        build_managed_base_bytes(&supported_shipped_definitions())
    );
    assert_eq!(base_json(dir.path())["managed"]["revision"], new_revision);
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        claude_of(&report)
            .install_commands
            .as_ref()
            .unwrap()
            .default,
        expected_install_2736("claude").unwrap()
    );
}

#[test]
fn install_commands_2736_edited_managed_base_is_not_refreshed() {
    let old: Vec<CodingAgentDefinition> = supported_shipped_definitions()
        .into_iter()
        .map(|mut def| {
            def.install_commands = None;
            def
        })
        .collect();
    let old_revision = managed_content_sha256(&old);
    let mut edited = old.clone();
    edited[0].color = "#d97707".to_string();
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": edited,
        "managed": {
            "owner": "agentscommander",
            "version": 1,
            "revision": old_revision,
            "contentSha256": old_revision,
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let report = load_catalog_report(dir.path());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.code == "managedBaseEdited"),
        "{:?}",
        report.warnings
    );
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn install_commands_2736_unknown_field_on_an_older_build_reports_migration_pending() {
    assert!(!KNOWN_DEFINITION_FIELDS.contains(&"futureField2736"));
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["futureField2736"] = serde_json::json!({"keep": "me"});
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(&serde_json::to_string(&vec![claude]).unwrap()),
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert_eq!(report.catalog.len(), 1, "the entry survives");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.code == "migrationPending" && w.reason.contains("futureField2736")),
        "{:?}",
        report.warnings
    );
}

#[test]
pub(crate) fn install_commands_2736_pin_wire_keeps_presence_semantics() {
    let definition: CodingAgentDefinition =
        serde_json::from_value(shipped_def_json(&["claude"]).remove(0)).unwrap();
    let mut with_key = serde_json::Map::new();
    with_key.insert("key".to_string(), serde_json::json!("claude"));
    with_key.insert(
        "installCommands".to_string(),
        serde_json::to_value(definition.install_commands.as_ref().unwrap()).unwrap(),
    );
    let mut without_key = serde_json::Map::new();
    without_key.insert("key".to_string(), serde_json::json!("claude"));

    for wire in [
        pin_explicit_legacy_fields(&definition, &with_key),
        materialize_complete_legacy_fields(&definition, &with_key),
    ] {
        let value = serde_json::to_value(&wire).unwrap();
        assert_eq!(
            value["installCommands"],
            serde_json::to_value(definition.install_commands.as_ref().unwrap()).unwrap()
        );
    }
    for wire in [
        pin_explicit_legacy_fields(&definition, &without_key),
        materialize_complete_legacy_fields(&definition, &without_key),
    ] {
        let value = serde_json::to_value(&wire).unwrap();
        assert!(
            value.get("installCommands").is_none(),
            "absent legacy key is never pinned or forced: {value}"
        );
    }

    let mut legacy_claude = shipped_def_json(&["claude"]).remove(0);
    legacy_claude["installCommands"] = serde_json::json!({"default": "npm i -g mine"});
    let legacy = serde_json::json!({"schemaVersion": 1, "agents": [legacy_claude]}).to_string();
    let extracted = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
        .expect("legacy extraction with explicit installCommands");
    let local: serde_json::Value = serde_json::from_slice(&extracted).unwrap();
    assert_eq!(
        local["agents"][0]["installCommands"],
        serde_json::json!({"default": "npm i -g mine"})
    );

    let mut ambiguous = shipped_def_json(&["claude"]).remove(0);
    ambiguous["installCommands"] = serde_json::json!({"default": " "});
    let legacy = serde_json::json!({"schemaVersion": 1, "agents": [ambiguous]}).to_string();
    let error = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
        .expect_err("ambiguous installCommands refuses the migration");
    assert!(error.contains("installCommands"), "{error}");
}

#[test]
fn cursor_cli_command_is_agent() {
    let catalog = embedded_default_catalog();
    let cursor = catalog.agents.iter().find(|a| a.key == "cursor").unwrap();
    assert_eq!(cursor.command, "agent");
}
