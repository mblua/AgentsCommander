use super::*;

#[test]
fn e2432_canonical_split_physical_owners_never_donate_rows() {
    // The bounded ordinary direction starts with protected physical main
    // rows; the sidecar receives profiles only and strict reads stay local.
    let directional = tempfile::tempdir().unwrap();
    let directional_path = directional.path().join(SETTINGS_FILE_NAME);
    let mut physical = serde_json::to_value(AppSettings::default()).unwrap();
    let legacy = registered();
    physical["agents"] = legacy["agents"].clone();
    physical["codingAgentProfiles"] = legacy["codingAgentProfiles"].clone();
    physical["rootToken"] = json!("directional-token");
    std::fs::write(&directional_path, serde_json::to_vec(&physical).unwrap()).unwrap();
    let overlay_path = directional.path().join(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
    let mut overlay_rows = legacy["agents"].clone();
    overlay_rows[0]["baseCommand"] = json!("overlay-command");
    overlay_rows[1]["baseCommand"] = json!("overlay-second");
    std::fs::write(
        &overlay_path,
        serde_json::to_vec(&json!({"agents":overlay_rows})).unwrap(),
    )
    .unwrap();
    let protected = load_settings_from_path(&directional_path);
    let directional_sidecar = agents_instance_path(&directional_path);
    let stored: Value =
        serde_json::from_slice(&std::fs::read(&directional_sidecar).unwrap()).unwrap();
    let old_hash = configuration_identity("tool", &[]);
    let old_set = stored["codingAgentProfiles"]["profilesByBaseHash"][&old_hash].clone();
    assert_eq!(old_set["B"]["notes"], "shared");
    assert_eq!(old_set["B"]["extra"], json!({"keep":null}));
    assert!(stored.get("agents").is_none());
    let after_main: Value =
        serde_json::from_slice(&std::fs::read(&directional_path).unwrap()).unwrap();
    assert_eq!(after_main["agents"], physical["agents"]);
    assert_eq!(protected.agents[0].command, "overlay-command");
    assert_eq!(
        startup_profile_legacy_work(
            &protected,
            &directional_path,
            Some(&std::fs::read_to_string(&directional_path).unwrap()),
            false
        )
        .unwrap(),
        Some(false)
    );
    let stable_main = std::fs::read(&directional_path).unwrap();
    let stable_sidecar = std::fs::read(&directional_sidecar).unwrap();
    let second = load_settings_from_path(&directional_path);
    assert_eq!(std::fs::read(&directional_path).unwrap(), stable_main);
    assert_eq!(std::fs::read(&directional_sidecar).unwrap(), stable_sidecar);
    assert_eq!(
        second.coding_agent_profiles,
        protected.coding_agent_profiles
    );
    let context = SourceReadContext::from_settings(&protected, &directional_path).unwrap();
    let strict =
        read_source_snapshot(&context, &context.source(SourceKind::RegisteredInstance)).unwrap();
    assert!(strict.snapshot.agents.is_empty());
    let mut forbidden = protected.clone();
    forbidden.agents[0].command = "forbidden".into();
    assert!(save_settings_to_path_preserving_project_paths(&forbidden, &directional_path).is_err());
    assert_eq!(std::fs::read(&directional_path).unwrap(), stable_main);
    assert_eq!(std::fs::read(&directional_sidecar).unwrap(), stable_sidecar);

    split_alias_and_base_edits(&directional_path, &directional_sidecar, &overlay_path);
    split_missing_binding_edits(&directional_path, &directional_sidecar, &overlay_path);
    split_new_row_alias_edit(&directional_path);
    split_complete_target_edits(&directional_path, &directional_sidecar);
    split_empty_new_row_edit(&directional_path, &directional_sidecar);
    split_publication_failure_cuts(&directional_path, &directional_sidecar);
    split_inverse_owners();
}

fn split_alias_and_base_edits(
    path: &std::path::Path,
    sidecar: &std::path::Path,
    overlay: &std::path::Path,
) {
    let directional_path = path;
    let directional_sidecar = sidecar;
    let overlay_path = overlay;
    std::fs::remove_file(overlay_path).unwrap();
    let mut draft = load_settings_for_cli_from_path(directional_path);
    draft
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("one")
        .unwrap()
        .get_mut("B")
        .unwrap()
        .notes = "alias edit".into();
    let shared = save_settings_to_path_preserving_project_paths(&draft, directional_path).unwrap();
    assert_eq!(
        shared.coding_agent_profiles.profiles_by_agent["two"]["B"].notes,
        "alias edit"
    );
    let mut conflicting = shared.clone();
    conflicting
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("one")
        .unwrap()
        .get_mut("B")
        .unwrap()
        .notes = "first proposal".into();
    conflicting
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("two")
        .unwrap()
        .get_mut("B")
        .unwrap()
        .notes = "second proposal".into();
    let before_main = std::fs::read(directional_path).unwrap();
    let before_sidecar = std::fs::read(directional_sidecar).unwrap();
    assert!(
        save_settings_to_path_preserving_project_paths(&conflicting, directional_path)
            .unwrap_err()
            .contains("sourceProfileConflict")
    );
    assert_eq!(std::fs::read(directional_path).unwrap(), before_main);
    assert_eq!(std::fs::read(directional_sidecar).unwrap(), before_sidecar);
    for env_edit in [false, true] {
        let mut draft = load_settings_for_cli_from_path(directional_path);
        let old_command = draft.agents[0].command.clone();
        let old_envs = draft.agents[0].envs.clone();
        let previous_hash = configuration_identity(&old_command, &old_envs);
        let previous: Value =
            serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
        if env_edit {
            draft.agents[0].envs = serde_json::from_value(json!([
                {"key":"ROUND11","value":"yes","enabled":true}
            ]))
            .unwrap();
        } else {
            draft.agents[0].command = "new-physical-command".into();
        }
        draft
            .coding_agent_profiles
            .profiles_by_agent
            .get_mut("one")
            .unwrap()
            .get_mut("B")
            .unwrap()
            .notes = if env_edit { "env edit" } else { "command edit" }.into();
        let new_hash = configuration_identity(&draft.agents[0].command, &draft.agents[0].envs);
        let saved =
            save_settings_to_path_preserving_project_paths(&draft, directional_path).unwrap();
        let next: Value =
            serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
        assert!(next.get("agents").is_none());
        assert_eq!(
            next["codingAgentProfiles"]["profilesByBaseHash"][&previous_hash],
            previous["codingAgentProfiles"]["profilesByBaseHash"][&previous_hash]
        );
        assert_eq!(
            next["codingAgentProfiles"]["profilesByBaseHash"][&new_hash]["B"]["extra"],
            json!({"keep":null})
        );
        assert_eq!(
            load_settings_for_cli_from_path(directional_path).coding_agent_profiles,
            saved.coding_agent_profiles
        );
        assert_eq!(
            saved.coding_agent_profiles.profiles_by_agent["two"]["B"].notes,
            "alias edit"
        );
    }
}

fn split_missing_binding_edits(
    path: &std::path::Path,
    sidecar: &std::path::Path,
    overlay: &std::path::Path,
) {
    let directional_path = path;
    let directional_sidecar = sidecar;
    let overlay_path = overlay;
    // A physical external BASE change is already in the fresh baseline;
    // preference save cannot fork its old set or persist an empty binding.
    let mut external: Value =
        serde_json::from_slice(&std::fs::read(directional_path).unwrap()).unwrap();
    external["agents"][0]["baseCommand"] = json!("external-unbound");
    std::fs::write(directional_path, serde_json::to_vec(&external).unwrap()).unwrap();
    let bindings: Value =
        serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
    let mut preference = load_settings_for_cli_from_path(directional_path);
    preference.theme_light = !preference.theme_light;
    save_settings_to_path_preserving_project_paths(&preference, directional_path).unwrap();
    let unchanged: Value =
        serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
    assert_eq!(unchanged, bindings);
    std::fs::write(
        overlay_path,
        serde_json::to_vec(&json!({
            "agents":[{"id":"one","label":"Overlay","color":"#123456","baseCommand":"overlay"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut protected_missing = load_settings_for_cli_from_path(directional_path);
    protected_missing
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("one")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "refused protected binding".into();
    let before_main = std::fs::read(directional_path).unwrap();
    let before_sidecar = std::fs::read(directional_sidecar).unwrap();
    assert!(
        save_settings_to_path_preserving_project_paths(&protected_missing, directional_path)
            .unwrap_err()
            .contains("crossOwnerCanonicalProfileEdit")
    );
    assert_eq!(std::fs::read(directional_path).unwrap(), before_main);
    assert_eq!(std::fs::read(directional_sidecar).unwrap(), before_sidecar);
    std::fs::remove_file(overlay_path).unwrap();
    let mut explicit = load_settings_for_cli_from_path(directional_path);
    explicit
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("one")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "explicit binding".into();
    save_settings_to_path_preserving_project_paths(&explicit, directional_path).unwrap();
    assert_eq!(
        load_settings_for_cli_from_path(directional_path)
            .coding_agent_profiles
            .profiles_by_agent["one"]["A"]
            .notes,
        "explicit binding"
    );
}

fn split_new_row_alias_edit(path: &std::path::Path) {
    let directional_path = path;
    // A new physical row attaches a complete existing target unchanged.
    let mut new_row = load_settings_for_cli_from_path(directional_path);
    let mut third = new_row.agents[1].clone();
    third.id = "three".into();
    third.label = "Three".into();
    third.command = "tool".into();
    new_row.agents.push(third);
    let attached =
        save_settings_to_path_preserving_project_paths(&new_row, directional_path).unwrap();
    assert_eq!(
        attached.coding_agent_profiles.profiles_by_agent["three"]["B"].notes,
        "alias edit"
    );
    let mut edited = attached.clone();
    edited
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("three")
        .unwrap()
        .get_mut("B")
        .unwrap()
        .notes = "new row edit".into();
    let shared = save_settings_to_path_preserving_project_paths(&edited, directional_path).unwrap();
    assert_eq!(
        shared.coding_agent_profiles.profiles_by_agent["two"]["B"].notes,
        "new row edit"
    );
}

fn split_complete_target_edits(path: &std::path::Path, sidecar: &std::path::Path) {
    let directional_path = path;
    let directional_sidecar = sidecar;
    // Complete target equality accepts a move; differences, including opaque
    // target fields and labels, reject before any physical publication.
    for difference in ["unknown", "label", "simultaneous"] {
        let mut target: Value =
            serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
        let mut proposal = load_settings_for_cli_from_path(directional_path);
        let target_command = "destination-command";
        let target_hash = configuration_identity(target_command, &proposal.agents[0].envs);
        let current_hash =
            configuration_identity(&proposal.agents[0].command, &proposal.agents[0].envs);
        target["codingAgentProfiles"]["profilesByBaseHash"][&target_hash] =
            target["codingAgentProfiles"]["profilesByBaseHash"][&current_hash].clone();
        target["codingAgentProfiles"]["profileLabelsByBaseHash"][&target_hash] =
            target["codingAgentProfiles"]["profileLabelsByBaseHash"][&current_hash].clone();
        if difference == "unknown" {
            target["codingAgentProfiles"]["profilesByBaseHash"][&target_hash]["A"]["unknown"] =
                json!({"different":true});
        } else if difference == "label" {
            target["codingAgentProfiles"]["profileLabelsByBaseHash"][&target_hash]["A"] =
                json!("different");
        }
        std::fs::write(directional_sidecar, serde_json::to_vec(&target).unwrap()).unwrap();
        proposal = load_settings_for_cli_from_path(directional_path);
        proposal.agents[0].command = target_command.into();
        if difference == "simultaneous" {
            proposal
                .coding_agent_profiles
                .profiles_by_agent
                .get_mut("one")
                .unwrap()
                .get_mut("A")
                .unwrap()
                .notes = "simultaneous change".into();
        }
        let before_main = std::fs::read(directional_path).unwrap();
        let before_sidecar = std::fs::read(directional_sidecar).unwrap();
        assert!(
            save_settings_to_path_preserving_project_paths(&proposal, directional_path)
                .unwrap_err()
                .contains("sourceProfileConflict")
        );
        assert_eq!(std::fs::read(directional_path).unwrap(), before_main);
        assert_eq!(std::fs::read(directional_sidecar).unwrap(), before_sidecar);
        target["codingAgentProfiles"]["profilesByBaseHash"]
            .as_object_mut()
            .unwrap()
            .remove(&target_hash);
        target["codingAgentProfiles"]["profileLabelsByBaseHash"]
            .as_object_mut()
            .unwrap()
            .remove(&target_hash);
        std::fs::write(directional_sidecar, serde_json::to_vec(&target).unwrap()).unwrap();
    }

    let mut equal = load_settings_for_cli_from_path(directional_path);
    let old = configuration_identity(&equal.agents[0].command, &equal.agents[0].envs);
    let equal_hash = configuration_identity("equal-target", &equal.agents[0].envs);
    let mut target: Value =
        serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
    target["codingAgentProfiles"]["profilesByBaseHash"][&equal_hash] =
        target["codingAgentProfiles"]["profilesByBaseHash"][&old].clone();
    target["codingAgentProfiles"]["profileLabelsByBaseHash"][&equal_hash] =
        target["codingAgentProfiles"]["profileLabelsByBaseHash"][&old].clone();
    std::fs::write(directional_sidecar, serde_json::to_vec(&target).unwrap()).unwrap();
    equal = load_settings_for_cli_from_path(directional_path);
    equal.agents[0].command = "equal-target".into();
    save_settings_to_path_preserving_project_paths(&equal, directional_path).unwrap();
    let equal_raw: Value =
        serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
    assert_eq!(
        equal_raw["codingAgentProfiles"]["profilesByBaseHash"],
        target["codingAgentProfiles"]["profilesByBaseHash"]
    );

    let mut divergent = load_settings_for_cli_from_path(directional_path);
    divergent.agents[0].command = "same-target".into();
    divergent.agents[0].envs.clear();
    divergent.agents[1].command = "target-same".into();
    assert_eq!(
        configuration_identity(&divergent.agents[0].command, &[]),
        configuration_identity(&divergent.agents[1].command, &[])
    );
    let before_main = std::fs::read(directional_path).unwrap();
    let before_sidecar = std::fs::read(directional_sidecar).unwrap();
    assert!(
        save_settings_to_path_preserving_project_paths(&divergent, directional_path)
            .unwrap_err()
            .contains("sourceProfileConflict")
    );
    assert_eq!(std::fs::read(directional_path).unwrap(), before_main);
    assert_eq!(std::fs::read(directional_sidecar).unwrap(), before_sidecar);
}

fn split_empty_new_row_edit(path: &std::path::Path, sidecar: &std::path::Path) {
    let directional_path = path;
    let directional_sidecar = sidecar;
    let mut empty_new = load_settings_for_cli_from_path(directional_path);
    let mut fourth = empty_new.agents[1].clone();
    fourth.id = "four".into();
    fourth.label = "Four".into();
    fourth.command = "new-unbound-row".into();
    empty_new.agents.push(fourth);
    let empty_saved =
        save_settings_to_path_preserving_project_paths(&empty_new, directional_path).unwrap();
    let empty_hash = configuration_identity("new-unbound-row", &[]);
    let raw: Value = serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
    assert_eq!(
        raw["codingAgentProfiles"]["profilesByBaseHash"][&empty_hash],
        json!({
            "A":{"enabled":true,"additionalArguments":"","env":{},"notes":""}
        })
    );
    assert_eq!(
        raw["codingAgentProfiles"]["profileLabelsByBaseHash"][&empty_hash],
        json!({})
    );
    let mut profile_edit = empty_saved;
    profile_edit
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("four")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "four edit".into();
    save_settings_to_path_preserving_project_paths(&profile_edit, directional_path).unwrap();
    assert_eq!(
        load_settings_for_cli_from_path(directional_path)
            .coding_agent_profiles
            .profiles_by_agent["four"]["A"]
            .notes,
        "four edit"
    );
}

fn split_publication_failure_cuts(path: &std::path::Path, sidecar: &std::path::Path) {
    let directional_path = path;
    let directional_sidecar = sidecar;
    // The existing atomic hook exercises both physical failure cuts. A
    // sidecar-only target after main failure remains an inert hash orphan.
    for fail_main in [false, true] {
        let mut candidate = load_settings_for_cli_from_path(directional_path);
        candidate.agents[0].command = if fail_main {
            "after-sidecar"
        } else {
            "before-sidecar"
        }
        .into();
        let before_main = std::fs::read(directional_path).unwrap();
        let before_sidecar = std::fs::read(directional_sidecar).unwrap();
        PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
            *slot.borrow_mut() = Some((
                if fail_main {
                    directional_path.to_path_buf()
                } else {
                    directional_sidecar.to_path_buf()
                },
                SettingsSaveStage::TempCreate,
            ))
        });
        let result = save_settings_to_path_preserving_project_paths(&candidate, directional_path);
        PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
        assert!(result.is_err());
        assert_eq!(std::fs::read(directional_path).unwrap(), before_main);
        if !fail_main {
            assert_eq!(std::fs::read(directional_sidecar).unwrap(), before_sidecar);
        } else {
            let published: Value =
                serde_json::from_slice(&std::fs::read(directional_sidecar).unwrap()).unwrap();
            let hash =
                configuration_identity(&candidate.agents[0].command, &candidate.agents[0].envs);
            assert!(published["codingAgentProfiles"]["profilesByBaseHash"]
                .get(&hash)
                .is_some());
            assert!(published.get("agents").is_none());
            assert_ne!(
                load_settings_for_cli_from_path(directional_path).agents[0].command,
                candidate.agents[0].command
            );
            let restarted = load_settings_from_path(directional_path);
            assert_ne!(restarted.agents[0].command, candidate.agents[0].command);
        }
    }
}

fn split_inverse_owners() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let mut sidecar = registered();
    sidecar
        .as_object_mut()
        .unwrap()
        .remove("codingAgentProfiles");
    write_source(&path, &sidecar);
    let mut main: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    main["codingAgentProfiles"] = json!({"profilesByAgent":{"one":{"B":{"notes":"neighbor"}}}});
    main = canonical(&main);
    std::fs::write(&path, serde_json::to_vec(&main).unwrap()).unwrap();
    let loaded = load_settings_for_cli_from_path(&path);
    assert!(!loaded.coding_agent_profiles.profiles_by_agent["one"].contains_key("B"));
    save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    let saved_main: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let saved_sidecar: Value =
        serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
    assert!(saved_sidecar.get("codingAgentProfiles").is_none());
    assert_eq!(
        saved_main["codingAgentProfiles"]["profilesByBaseHash"],
        json!({})
    );
    assert_eq!(
        saved_main["codingAgentProfiles"]["legacyProfileOrphans"]["one"],
        json!([{"cells":{"B":{"notes":"neighbor"}}}])
    );
    assert_eq!(
        saved_main["codingAgentProfiles"],
        main["codingAgentProfiles"]
    );
    let mut edited = load_settings_for_cli_from_path(&path);
    edited
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("one")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "requires canonical cross-owner edit".into();
    let before_main = std::fs::read(&path).unwrap();
    let before_sidecar = std::fs::read(agents_instance_path(&path)).unwrap();
    assert!(
        save_settings_to_path_preserving_project_paths(&edited, &path)
            .unwrap_err()
            .contains("crossOwnerCanonicalProfileEdit")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before_main);
    assert_eq!(
        std::fs::read(agents_instance_path(&path)).unwrap(),
        before_sidecar
    );
    split_inverse_shadowed_rows();
}

fn split_inverse_shadowed_rows() {
    // Inverse canonical split: sidecar rows shadow main rows with the same
    // ID and a different command. A preference CAS must never adopt the
    // main row's BASE cells, including through writer re-decode.
    let inverse = tempfile::tempdir().unwrap();
    let inverse_path = inverse.path().join(SETTINGS_FILE_NAME);
    let mut shadowed = registered();
    shadowed["agents"] = json!([{
        "id":"one", "label":"One", "baseCommand":"claude", "color":"#123456"
    }]);
    shadowed["codingAgentProfiles"] = json!({"profilesByAgent":{
        "one":{"B":{"notes":"shadow-only","extension":{"keep":7}}}
    }});
    let mut inverse_main = canonical(&shadowed);
    inverse_main["defaultShell"] = json!("test-shell");
    inverse_main["defaultShellArgs"] = json!([]);
    inverse_main["rootToken"] = json!("inverse-token");
    inverse_main["terminalSnapshotsEnabled"] = json!(false);
    let physical_profiles = inverse_main["codingAgentProfiles"].clone();
    assert!(
        physical_profiles["profilesByBaseHash"][&configuration_identity("claude", &[])]["B"]
            .is_object()
    );
    let row: AgentConfig = serde_json::from_value(json!({
        "id":"one", "label":"One", "baseCommand":"codex", "color":"#123456", "order":0
    }))
    .unwrap();
    let mut row = serde_json::to_value(row).unwrap();
    row["id_hash"] = json!(configuration_identity("codex", &[]));
    row["id_hash_debug"] = json!(["codex"]);
    let inverse_sidecar = json!({"agents":[row],"opaque":{"keep":true}});
    std::fs::write(
        &inverse_path,
        serde_json::to_vec_pretty(&inverse_main).unwrap(),
    )
    .unwrap();
    let inverse_agents = agents_instance_path(&inverse_path);
    std::fs::write(
        &inverse_agents,
        serde_json::to_vec_pretty(&inverse_sidecar).unwrap(),
    )
    .unwrap();
    std::fs::write(project_paths_path(&inverse_path), b"{}").unwrap();
    let main_bytes = std::fs::read(&inverse_path).unwrap();
    let agents_bytes = std::fs::read(&inverse_agents).unwrap();
    let no_op = compare_and_set_terminal_snapshots_enabled_at_path(
        &AppSettings::default(),
        &inverse_path,
        false,
        false,
    )
    .unwrap();
    assert_eq!(no_op.agents[0].command, "codex");
    assert!(no_op.coding_agent_profiles.profiles_by_agent.is_empty());
    assert_eq!(std::fs::read(&inverse_path).unwrap(), main_bytes);
    assert_eq!(std::fs::read(&inverse_agents).unwrap(), agents_bytes);
    let toggled =
        compare_and_set_terminal_snapshots_enabled_at_path(&no_op, &inverse_path, false, true)
            .unwrap();
    assert_eq!(toggled.agents[0].command, "codex");
    assert!(toggled.coding_agent_profiles.profiles_by_agent.is_empty());
    let after: Value = serde_json::from_slice(&std::fs::read(&inverse_path).unwrap()).unwrap();
    assert_eq!(after["codingAgentProfiles"], physical_profiles);
    assert_eq!(after["agents"], inverse_main["agents"]);
    assert_eq!(std::fs::read(&inverse_agents).unwrap(), agents_bytes);
    let stable = std::fs::read(&inverse_path).unwrap();
    let reloaded =
        compare_and_set_terminal_snapshots_enabled_at_path(&toggled, &inverse_path, true, true)
            .unwrap();
    assert_eq!(
        reloaded.coding_agent_profiles,
        toggled.coding_agent_profiles
    );
    assert_eq!(std::fs::read(&inverse_path).unwrap(), stable);
    assert_eq!(std::fs::read(&inverse_agents).unwrap(), agents_bytes);
    let loaded = load_settings_from_path(&inverse_path);
    assert_eq!(loaded.agents[0].command, "codex");
    assert!(loaded.coding_agent_profiles.profiles_by_agent.is_empty());
    assert_eq!(std::fs::read(&inverse_path).unwrap(), stable);
    assert_eq!(std::fs::read(&inverse_agents).unwrap(), agents_bytes);
}
