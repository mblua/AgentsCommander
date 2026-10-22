use super::super::*;

fn physical_settings(dir: &Path, value: &Value) -> AppSettings {
    let path = dir.join(SETTINGS_FILE_NAME);
    std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    load_settings_from_path(&path)
}

#[test]
fn derived_agent_id_binding_projection_ignores_presentation_and_sorts_ids() {
    let dir = tempfile::tempdir().unwrap();
    let original = serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "agents": [
            {"id":"alpha","label":"Alpha","command":"claude","color":"#112233","isolatedHome":false},
            {"id":"beta","label":"Beta","command":"codex","color":"#445566","isolatedHome":false},
        ],
        "rootToken": "fixture-token",
    });
    let first = physical_settings(dir.path(), &original);
    let revision = bootstrap_identity_revision(&first).unwrap();
    let settings_path = dir.path().join(SETTINGS_FILE_NAME);
    let agents_path = agents_instance_path(&settings_path);
    // Capture the complete physical baseline BEFORE the presentation edit.
    // Reusing the original legacy JSON would drop migrated profile ownership.
    let mut after: Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    let mut after_groups: Value =
        serde_json::from_slice(&std::fs::read(&agents_path).unwrap()).unwrap();
    let agents = after_groups["agents"].as_array_mut().unwrap();
    assert!(!agents.is_empty());
    assert_eq!(agents.len(), 2);
    for row in agents.iter_mut() {
        row["label"] = serde_json::json!("display changed");
        row["color"] = serde_json::json!("red");
        row["order"] = serde_json::json!(99);
    }
    agents.reverse();
    after["rootToken"] = serde_json::json!("changed-root-token");
    after["sidebarZoom"] = serde_json::json!(2.0);
    // Preserve all profile/HOME/ownership groups and unknown fields verbatim.
    std::fs::write(&agents_path, serde_json::to_vec(&after_groups).unwrap()).unwrap();
    let second = physical_settings(dir.path(), &after);
    let before_physical = first.coding_agent_identity_physical.as_ref().unwrap();
    let after_physical = second.coding_agent_identity_physical.as_ref().unwrap();
    for key in ["codingAgentProfiles", "quotaSources", "groupOwnership"] {
        assert_eq!(
            before_physical[key], after_physical[key],
            "identity input {key} changed"
        );
    }
    assert_eq!(bootstrap_identity_revision(&second).unwrap(), revision);
}

#[test]
fn derived_agent_id_binding_projection_ignores_only_exact_debug_locations() {
    let mut settings = AppSettings::default();
    let raw = serde_json::json!({
        "agents": [{"id":"alpha", "baseCommand":"tool", "id_hash":"base-hash",
            "unknownIdentity":{"id_hash_debug":["nested-agent"]}}],
        "codingAgentProfiles":{"profilesByAgent":{"alpha":{"A":{
            "additionalArguments":"--flag", "id_hash":"pair-hash",
            "unknownIdentity":{"id_hash_debug":["nested-cell"]}
        }}}}
    });
    settings.coding_agent_identity_physical = Some(Arc::new(raw.clone()));
    let baseline = bootstrap_identity_revision(&settings).unwrap();
    let binding = bootstrap_binding_revision(&settings, "alpha").unwrap();
    // Mutate the same captured raw image directly, without a loader or disk rewrite.
    for location in ["agent", "cell", "both"] {
        for debug in [
            serde_json::json!(["forged"]),
            serde_json::json!([]),
            serde_json::json!(["replacement", ""]),
            serde_json::json!(false),
            serde_json::json!(["mixed", 9]),
            serde_json::json!(null),
        ] {
            let mut image = raw.clone();
            if location != "cell" {
                image["agents"][0]["id_hash_debug"] = debug.clone();
            }
            if location != "agent" {
                image["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["id_hash_debug"] =
                    debug;
            }
            let mut changed = settings.clone();
            changed.coding_agent_identity_physical = Some(Arc::new(image));
            assert_eq!(bootstrap_identity_revision(&changed).unwrap(), baseline);
            assert_eq!(
                bootstrap_binding_revision(&changed, "alpha").unwrap(),
                binding
            );
        }
    }
    for location in ["agent", "cell"] {
        let command_key = if location == "agent" {
            "baseCommand"
        } else {
            "additionalArguments"
        };
        for key in ["id_hash", command_key, "unknownIdentity"] {
            let mut image = raw.clone();
            let record = if location == "agent" {
                &mut image["agents"][0]
            } else {
                &mut image["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]
            };
            record[key] = if key == "unknownIdentity" {
                serde_json::json!({"id_hash_debug":["nested-changed"]})
            } else {
                serde_json::json!("changed")
            };
            let mut changed = settings.clone();
            changed.coding_agent_identity_physical = Some(Arc::new(image));
            assert_ne!(
                bootstrap_identity_revision(&changed).unwrap(),
                baseline,
                "{location}/{key}"
            );
        }
    }
    assert_eq!(
        settings
            .coding_agent_identity_physical
            .as_ref()
            .unwrap()
            .as_ref(),
        &raw
    );
}

#[test]
fn derived_agent_id_binding_projection_changes_for_execution_home_and_quota() {
    let dir = tempfile::tempdir().unwrap();
    let settings = physical_settings(
        dir.path(),
        &serde_json::json!({
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [
                {"id":"alpha","label":"Alpha","command":"claude","color":"#112233","isolatedHome":false},
                {"id":"beta","label":"Beta","command":"codex","color":"#445566","isolatedHome":false},
            ],
            "rootToken": "fixture-token",
        }),
    );
    let revision = bootstrap_identity_revision(&settings).unwrap();
    let raw = settings
        .coding_agent_identity_physical
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    for (key, value) in [
        ("command", serde_json::json!("different-command")),
        ("envs", serde_json::json!([{"key":"MODE","value":"other"}])),
        ("isolatedHome", serde_json::json!(true)),
    ] {
        let mut changed = settings.clone();
        let mut image = raw.clone();
        image["agents"][0][key] = value;
        changed.coding_agent_identity_physical = Some(Arc::new(image));
        assert_ne!(
            bootstrap_identity_revision(&changed).unwrap(),
            revision,
            "{key}"
        );
    }
    let mut changed = settings.clone();
    let mut image = raw;
    image["quotaSources"] = serde_json::json!({"agent": {"source":"other"}});
    changed.coding_agent_identity_physical = Some(Arc::new(image));
    assert_ne!(bootstrap_identity_revision(&changed).unwrap(), revision);
}

#[test]
fn derived_agent_id_binding_save_narrow_and_cas_return_physical_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SETTINGS_FILE_NAME);
    physical_settings(
        dir.path(),
        &serde_json::json!({
            "defaultShell":"test-shell",
            "defaultShellArgs":[],
            "agents": [{"id":"alpha","label":"Alpha","command":"claude","color":"red"}],
            "rootToken":"fixture-token"
        }),
    );
    // Unknown fields in an overlay-owned group must survive typed decode.
    let overlay_path = path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
    std::fs::write(&overlay_path, serde_json::to_vec(&serde_json::json!({
            "agents": [{"id":"alpha","label":"Overlay","command":"claude","color":"red","unknownIdentity":{"keep":7}}]
        })).unwrap()).unwrap();
    let mut current = load_settings_for_cli_from_path(&path);
    assert_eq!(
        current.coding_agent_identity_physical.as_ref().unwrap()["agents"][0]["unknownIdentity"]
            ["keep"],
        7
    );
    assert!(current.local_overlay_state.owns_top_level("agents"));
    let baseline = bootstrap_identity_revision(&current).unwrap();
    for mode in 0..4 {
        let before_bytes = (
            std::fs::read(&path).unwrap(),
            std::fs::read(agents_instance_path(&path)).unwrap(),
            std::fs::read(&overlay_path).unwrap(),
        );
        current = match mode {
            0 => save_settings_to_path_preserving_project_paths(&current, &path).unwrap(),
            1 => set_main_window_placement_at_path(
                &current,
                &path,
                &WindowGeometry {
                    x: 0.0,
                    y: 0.0,
                    width: 800.0,
                    height: 600.0,
                },
                MainWindowDisplayState::Normal,
            )
            .unwrap(),
            2 => compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, true, false)
                .unwrap(),
            _ => compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, false)
                .unwrap(),
        };
        if mode == 3 {
            assert_eq!(
                before_bytes,
                (
                    std::fs::read(&path).unwrap(),
                    std::fs::read(agents_instance_path(&path)).unwrap(),
                    std::fs::read(&overlay_path).unwrap(),
                )
            );
        }
        let fresh = load_settings_for_cli_from_path(&path);
        assert_eq!(
            current.coding_agent_identity_physical,
            fresh.coding_agent_identity_physical
        );
        assert_eq!(
            bootstrap_binding_revision(&current, "alpha").unwrap(),
            baseline
        );
        let raw = current.coding_agent_identity_physical.as_ref().unwrap();
        assert_eq!(raw["agents"][0]["unknownIdentity"]["keep"], 7);
        assert!(raw["groupOwnership"]["overlayPaths"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(["agents"])));
    }
    // A decode-invalid overlay uses the raw base, just as the live loader does.
    std::fs::write(
        &overlay_path,
        serde_json::to_vec(&serde_json::json!({
            "agents": [{"id":"alpha","label":"Overlay","command":"different","color":"red"}],
            "sidebarZoom":"invalid"
        }))
        .unwrap(),
    )
    .unwrap();
    let written = save_settings_to_path_preserving_project_paths(&current, &path).unwrap();
    let fresh = load_settings_for_cli_from_path(&path);
    assert_eq!(
        written.coding_agent_identity_physical,
        fresh.coding_agent_identity_physical
    );
    std::fs::remove_file(&overlay_path).unwrap();
    current = load_settings_for_cli_from_path(&path);
    let before_edit = bootstrap_binding_revision(&current, "alpha").unwrap();
    current.agents[0].command = "claude --different-execution".into();
    let written = save_settings_to_path_preserving_project_paths(&current, &path).unwrap();
    let after_edit = bootstrap_binding_revision(&written, "alpha").unwrap();
    assert_ne!(after_edit, before_edit);
    assert_eq!(
        after_edit,
        bootstrap_binding_revision(&load_settings_for_cli_from_path(&path), "alpha").unwrap()
    );
    assert_eq!(
        written.coding_agent_identity_physical.as_ref().unwrap()["agents"][0]["baseCommand"],
        "claude --different-execution"
    );
    assert_eq!(
        written.coding_agent_identity_physical.as_ref().unwrap()["agents"][0]["command"],
        "claude"
    );
}

#[test]
fn derived_agent_id_binding_project_replacement_refreshes_physical_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SETTINGS_FILE_NAME);
    let current = physical_settings(
        dir.path(),
        &serde_json::json!({
            "defaultShell":"test-shell",
            "defaultShellArgs":[],
            "agents": [{"id":"alpha","label":"Alpha","command":"claude","color":"red"}],
            "rootToken":"fixture-token"
        }),
    );
    let previous = bootstrap_binding_revision(&current, "alpha").unwrap();
    let agents_path = agents_instance_path(&path);
    let mut groups: Value = serde_json::from_slice(&std::fs::read(&agents_path).unwrap()).unwrap();
    groups["agents"][0]["unknownIdentity"] = serde_json::json!({"fresh":7});
    std::fs::write(&agents_path, serde_json::to_vec(&groups).unwrap()).unwrap();
    let actual = load_settings_for_cli_from_path(&path);
    let revision = bootstrap_binding_revision(&actual, "alpha").unwrap();
    assert_ne!(revision, previous);
    let mut transaction = begin_project_paths_transaction_at_path(&current, &path).unwrap();
    assert_eq!(
        transaction.fresh_settings().coding_agent_identity_physical,
        actual.coding_agent_identity_physical
    );
    let candidate = transaction.fresh_settings().clone();
    let written = transaction
        .commit_project_groups(&candidate, false, false)
        .unwrap();
    assert_eq!(
        bootstrap_binding_revision(&written, "alpha").unwrap(),
        revision
    );
    assert_eq!(
        written.coding_agent_identity_physical.as_ref().unwrap()["agents"][0]["unknownIdentity"]
            ["fresh"],
        7
    );
}

#[test]
fn derived_agent_id_binding_projection_missing_physical_never_uses_typed_fallback() {
    assert!(bootstrap_identity_revision(&AppSettings::default()).is_err());
    let dir = tempfile::tempdir().unwrap();
    let settings = physical_settings(
        dir.path(),
        &serde_json::json!({
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [
                {"id":"alpha","label":"Alpha","command":"claude","color":"#112233","isolatedHome":false},
                {"id":"beta","label":"Beta","command":"codex","color":"#445566","isolatedHome":false},
            ],
            "rootToken": "fixture-token",
        }),
    );
    let mut draft =
        serde_json::from_value::<AppSettings>(serde_json::to_value(&settings).unwrap()).unwrap();
    assert!(bootstrap_identity_revision(&draft).is_err());
    draft.coding_agent_identity_physical = settings.coding_agent_identity_physical.clone();
    let mut image = draft
        .coding_agent_identity_physical
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    let repeated = image["agents"][0].clone();
    assert!(repeated.is_object());
    image["agents"].as_array_mut().unwrap().push(repeated);
    draft.coding_agent_identity_physical = Some(Arc::new(image));
    assert!(bootstrap_identity_revision(&draft).is_err());
    assert_eq!(
        bootstrap_identity_revision(&draft).unwrap_err(),
        "coding_agent_identity_duplicate"
    );
}
