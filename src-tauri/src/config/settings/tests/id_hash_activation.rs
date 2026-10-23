use super::*;

pub(super) fn hash_activation_assert(settings: &super::super::AppSettings) {
    for agent in &settings.agents {
        let mut leaves = vec![agent.command.clone()];
        for env in &agent.envs {
            leaves.extend([env.key.clone(), env.value.clone()]);
        }
        assert_eq!(agent.id_hash_debug, Some(leaves.clone()));
        if let Some(cells) = settings
            .coding_agent_profiles
            .profiles_by_agent
            .get(&agent.id)
        {
            let first = settings
                .agents
                .iter()
                .find(|row| row.id == agent.id)
                .unwrap();
            for cell in cells.values() {
                let mut pair = vec![first.command.clone()];
                for env in &first.envs {
                    pair.extend([env.key.clone(), env.value.clone()]);
                }
                pair.push(cell.command.clone());
                for (key, value) in &cell.env {
                    pair.extend([key.clone(), value.clone()]);
                }
                assert_eq!(cell.id_hash_debug, Some(pair));
            }
        }
        assert_eq!(
            agent.id_hash.as_ref(),
            Some(&super::super::configuration_identity(
                &agent.command,
                &agent.envs
            ))
        );
    }
}

#[test]
fn id_hash_activation_three_loaders_derive_without_writes_or_backups() {
    for sidecar in [false, true] {
        let (temp, path) = hash_activation_seed(sidecar);
        let physical = if sidecar {
            b3_sidecar_path(temp.path())
        } else {
            path.clone()
        };
        let mut raw = serde_json::Value::Object(b3_object(&physical));
        raw["agents"][0]["id_hash_debug"] = serde_json::json!(["old-base"]);
        raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["id_hash_debug"] =
            serde_json::json!(["old-pair"]);
        b3_write(&physical, &raw);
        // CLI views remain read-only before the admitted GUI conversion.
        let before_cli = hash_activation_files(temp.path());
        super::super::load_settings_for_cli_from_path(&path);
        super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert_eq!(hash_activation_files(temp.path()), before_cli);
        let first_gui = super::super::load_settings_from_path(&path);
        hash_activation_assert(&first_gui);
        let stored = b3_object(&b3_sidecar_path(temp.path()));
        assert_eq!(stored["codingAgentProfiles"]["profileBindingVersion"], 1);
        for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
            assert!(stored["codingAgentProfiles"].get(key).is_none());
        }
        let before = hash_activation_files(temp.path());
        for (loader, settings) in b3_all_loaders(&path) {
            assert_eq!(settings.agents[0].id, "alpha", "{loader}");
            hash_activation_assert(&settings);
            assert_eq!(
                settings.agents[0].id_hash_debug,
                Some(vec!["claude".into()]),
                "{loader}"
            );
            assert_eq!(
                settings.coding_agent_profiles.profiles_by_agent["alpha"]["A"].id_hash_debug,
                Some(vec!["claude".into(), "".into()]),
                "{loader}"
            );
            assert_eq!(hash_activation_files(temp.path()), before, "{loader}");
        }
    }
}

#[test]
fn id_hash_activation_direct_save_backfills_literal_and_reaches_fixed_point() {
    for supplied in [None, Some("forged".to_string())] {
        let (temp, path) = hash_activation_seed(false);
        let mut settings =
            super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        settings.agents[0].id_hash = supplied;
        let written =
            super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
        let raw = b3_object(&b3_sidecar_path(temp.path()));
        assert_eq!(
            raw["agents"][0]["id_hash"],
            written.agents[0].id_hash.clone().unwrap()
        );
        assert!(raw["agents"][0].get("idHash").is_none());
        assert_eq!(written.agents[0].id, "alpha");
        let first = hash_activation_files(temp.path());
        let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
        assert_eq!(hash_activation_files(temp.path()), first);
    }
}

#[test]
fn id_hash_activation_all_untrusted_metadata_is_replaced() {
    for supplied in [
        serde_json::Value::Null,
        serde_json::json!(false),
        serde_json::json!(42),
        serde_json::json!(1.5),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!(""),
        serde_json::json!("cid1:stale"),
        serde_json::json!("forged"),
    ] {
        let (temp, path) = hash_activation_seed(true);
        let sidecar = b3_sidecar_path(temp.path());
        let mut raw = serde_json::Value::Object(b3_object(&sidecar));
        raw["agents"][0]["id_hash"] = supplied.clone();
        raw["agents"][0]["id_hash_debug"] = supplied;
        b3_write(&sidecar, &raw);
        let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        hash_activation_assert(&loaded);
        let written =
            super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
        assert_eq!(
            b3_object(&sidecar)["agents"][0]["id_hash"],
            written.agents[0].id_hash.clone().unwrap()
        );
        assert_eq!(
            b3_object(&sidecar)["agents"][0]["id_hash_debug"],
            serde_json::json!(["claude"])
        );
    }
}

#[test]
fn id_hash_activation_malformed_real_fields_refuse_overwrite() {
    for bad in [
        serde_json::json!("not-array"),
        serde_json::json!([{"id":"alpha","label":"A","color":"red","command":7}]),
        serde_json::json!([{"id":"alpha","label":"A","color":"red","command":"tool","envs":{}}]),
    ] {
        let (temp, path) = hash_activation_seed(true);
        let sidecar = b3_sidecar_path(temp.path());
        let mut raw = serde_json::Value::Object(b3_object(&sidecar));
        raw["agents"] = bad.clone();
        b3_write(&sidecar, &raw);
        let before = hash_activation_files(temp.path());
        let error = super::super::read_agents_instance_file(&sidecar).unwrap_err();
        assert!(error.contains("undecodable agents array"), "{bad}: {error}");
        // Existing strict loading exposes an unreadable layer in a valid
        // unrelated-settings snapshot; it does not always return Err.
        let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert!(
            matches!(
                &loaded.agents_layer,
                super::super::AgentsLayerState::Unreadable { path: failed, .. } if failed == &sidecar
            ),
            "{bad}"
        );
        assert!(loaded.agents.is_empty(), "{bad}");
        assert!(
            super::super::save_settings_to_path_preserving_project_paths(
                &super::super::AppSettings::default(),
                &path
            )
            .is_err()
        );
        assert_eq!(hash_activation_files(temp.path()), before);
    }
}

#[test]
fn id_hash_activation_overlay_owned_physical_and_effective_remain_distinct() {
    for (sidecar, field) in [
        (false, "baseCommand"),
        (true, "baseCommand"),
        (false, "command"),
        (true, "command"),
    ] {
        let (temp, path) = hash_activation_seed(sidecar);
        let physical_path = if sidecar {
            b3_sidecar_path(temp.path())
        } else {
            path.clone()
        };
        let mut physical = serde_json::Value::Object(b3_object(&physical_path));
        let physical_hash = if sidecar {
            super::super::configuration_identity("claude", &[])
        } else {
            "retained-physical-metadata".into()
        };
        physical["agents"][0]["id_hash"] = serde_json::json!(physical_hash);
        physical["agents"][0]["id_hash_debug"] = serde_json::json!(["retained-forged"]);
        physical["agents"][0]["unknownRowField"] = serde_json::json!({"keep":9});
        b3_write(&physical_path, &physical);
        let raw_before = physical["agents"].clone();
        let mut overlay = raw_before.clone();
        overlay[0].as_object_mut().unwrap().remove("baseCommand");
        overlay[0].as_object_mut().unwrap().remove("command");
        overlay[0][field] = serde_json::json!("codex --different");
        overlay[0]["envs"] =
            serde_json::json!([{"key":"MODE","value":"effective","enabled":false}]);
        overlay[0]["id_hash"] = serde_json::json!("forged-effective");
        b3_write(
            &b3_overlay_path(temp.path()),
            &serde_json::json!({"agents":overlay}),
        );
        let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        let written =
            super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
        hash_activation_assert(&written);
        assert_eq!(written.agents[0].command, "codex --different");
        assert_eq!(b3_object(&physical_path)["agents"], raw_before);
        assert_eq!(written.agents[0].id, "alpha");
        let reload = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert_eq!(
            serde_json::to_value(&reload.agents).unwrap(),
            serde_json::to_value(&written.agents).unwrap()
        );
        assert_ne!(
            written.agents[0].id_hash.as_deref(),
            Some(physical_hash.as_str())
        );
    }
}

#[test]
fn id_hash_activation_rejected_overlay_and_terminal_cas_derive_final_view() {
    let (temp, path) = hash_activation_seed(true);
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({"defaultShellArgs":"bad"}),
    );
    let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    hash_activation_assert(&loaded);
    assert!(loaded.local_overlay_state.rejection().is_some());
    let mut stale = loaded.clone();
    stale.agents[0].id_hash = Some("forged".into());
    let unchanged = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &stale,
        &path,
        stale.terminal_snapshots_enabled,
        stale.terminal_snapshots_enabled,
    )
    .unwrap();
    hash_activation_assert(&unchanged);
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&stale, &path).unwrap();
    hash_activation_assert(&written);
    assert_eq!(
        b3_object(&b3_sidecar_path(temp.path()))["agents"][0]["id_hash"],
        written.agents[0].id_hash.clone().unwrap()
    );
}

#[test]
fn id_hash_activation_main_publisher_failure_keeps_diagnostic_and_no_success() {
    let (temp, path) = hash_activation_seed(true);
    let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let previous = std::fs::read(&path).unwrap();
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::super::SettingsSaveStage::AtomicReplace))
    });
    let result = super::super::save_settings_to_path_preserving_project_paths(&loaded, &path);
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    // Preserve the existing partial-publication behavior and report failure.
    assert!(b3_object(&b3_sidecar_path(temp.path()))["agents"][0]["id_hash"].is_string());
    hash_activation_assert(
        &super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap(),
    );
}

#[test]
fn id_hash_activation_legacy_duplicates_preserve_rows_and_refuse_ambiguous_changes() {
    let (temp, path) = hash_activation_seed(true);
    let sidecar = b3_sidecar_path(temp.path());
    let mut raw = serde_json::Value::Object(b3_object(&sidecar));
    let row = raw["agents"][0].clone();
    raw["agents"].as_array_mut().unwrap().push(row.clone());
    let mut third = row.clone();
    third["id"] = serde_json::json!("beta");
    third["command"] = serde_json::json!("unique-command");
    raw["agents"].as_array_mut().unwrap().push(third);
    b3_write(&sidecar, &raw);
    let mut settings = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    settings.agents.reverse();
    super::super::normalize_agent_order(&mut settings.agents);
    settings.log_level = Some("debug".into());
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(written.agents.iter().filter(|a| a.id == "alpha").count(), 2);
    hash_activation_assert(&written);
    let mut increased = written.clone();
    increased.agents.push(serde_json::from_value(row).unwrap());
    let before_identity = b3_object(&sidecar);
    let mut after_identity = before_identity.clone();
    after_identity.insert(
        "agents".into(),
        serde_json::to_value(&increased.agents).unwrap(),
    );
    assert!(super::super::validate_physical_identity_delta(
        &before_identity,
        &after_identity,
        true,
        false
    )
    .unwrap_err()
    .contains("DuplicateConfigurationIdentity"));
    let mut edited = written.clone();
    edited.agents[1].command = "repair-extra-character".into();
    let mut partially_deleted = written.clone();
    partially_deleted.agents.remove(1);
    for candidate in [increased, edited, partially_deleted] {
        let before = hash_activation_files(temp.path());
        let error = super::super::save_settings_to_path_preserving_project_paths(&candidate, &path)
            .unwrap_err();
        assert!(error.contains("duplicate agent ID 'alpha'"));
        assert!(error.contains("unprovable row correspondence"));
        assert_eq!(hash_activation_files(temp.path()), before);
        let reloaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert_eq!(
            serde_json::to_value(&reloaded.agents).unwrap(),
            serde_json::to_value(&written.agents).unwrap()
        );
        assert_eq!(reloaded.log_level, written.log_level);
        hash_activation_assert(&reloaded);
    }
}

#[test]
fn e2432_duplicate_rows_keep_existing_general_save_control() {
    id_hash_activation_legacy_duplicates_preserve_rows_and_refuse_ambiguous_changes();
}

#[test]
fn e2432_id_hash_activation_anagram_alias_is_source_local() {
    let (_temp, path) = hash_activation_seed(true);
    let mut settings = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let mut alias = settings.agents[0].clone();
    alias.id = "beta".into();
    alias.label = "Alias".into();
    alias.command = "eduacl".into();
    settings.agents.push(alias);
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(written.agents[0].id_hash, written.agents[1].id_hash);
    // Sharing compares payloads, while debug metadata keeps each physical
    // row's original command/env strings. Pair hashes still compare equal.
    let payload = |id: &str| {
        let agent = written.agents.iter().find(|agent| agent.id == id).unwrap();
        let mut cells = written.coding_agent_profiles.profiles_by_agent[id].clone();
        for cell in cells.values_mut() {
            assert_eq!(
                cell.id_hash,
                Some(super::super::pair_identity(
                    &agent.command,
                    &agent.envs,
                    cell
                ))
            );
            assert_eq!(
                cell.id_hash_debug,
                Some(super::super::pair_identity_debug(
                    &agent.command,
                    &agent.envs,
                    cell
                ))
            );
            cell.id_hash_debug = None;
        }
        cells
    };
    assert_eq!(payload("alpha"), payload("beta"));
    let (other_temp, other_path) = hash_activation_seed(true);
    let other_before = hash_activation_files(other_temp.path());
    let other = super::super::load_settings_for_cli_strict_from_path(Some(&other_path)).unwrap();
    assert_eq!(other.agents.len(), 1);
    let mut edited = written;
    edited
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "same-source edit".into();
    let edited =
        super::super::save_settings_to_path_preserving_project_paths(&edited, &path).unwrap();
    assert_eq!(
        edited.coding_agent_profiles.profiles_by_agent["beta"]["A"].notes,
        "same-source edit"
    );
    assert_eq!(hash_activation_files(other_temp.path()), other_before);
}

#[test]
fn id_hash_activation_profile_delta_preserves_history_and_rejects_new_pair() {
    let (temp, path) = hash_activation_seed(true);
    let sidecar = b3_sidecar_path(temp.path());
    let mut raw = serde_json::Value::Object(b3_object(&sidecar));
    raw["codingAgentProfiles"]["profilesByAgent"]["alpha"] = serde_json::json!({
        "A":{"command":"--one"},"B":{"command":"--one"},"C":{"command":"--two"}
    });
    let mut other = raw["agents"][0].clone();
    other["id"] = serde_json::json!("beta");
    other["command"] = serde_json::json!("distinct-command");
    other["baseCommand"] = serde_json::json!("distinct-command");
    raw["agents"].as_array_mut().unwrap().push(other);
    raw["codingAgentProfiles"]["profilesByAgent"]["beta"] =
        serde_json::json!({"A":{"command":"--one"}});
    b3_write(&sidecar, &raw);
    let mut loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("beta")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .command = "--unrelated-edit".into();
    loaded.agents[0].label = "Renamed".into();
    super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .get_mut("C")
        .unwrap()
        .command = "--unique".into();
    super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .get_mut("C")
        .unwrap()
        .command = "--one".into();
    let before = hash_activation_files(temp.path());
    let error =
        super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap_err();
    assert!(error.contains("DuplicateProfileIdentity") && error.contains("Renamed"));
    assert_eq!(hash_activation_files(temp.path()), before);
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .remove("B");
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .remove("C");
    assert!(super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).is_ok());
}

#[test]
fn id_hash_activation_legacy_before_is_read_when_only_profiles_are_externalized() {
    let (temp, path) = hash_activation_seed(false);
    let mut base = serde_json::Value::Object(b3_object(&path));
    let mut duplicate = base["agents"][0].clone();
    duplicate["id"] = serde_json::json!("beta");
    base["agents"].as_array_mut().unwrap().push(duplicate);
    b3_write(
        &b3_sidecar_path(temp.path()),
        &serde_json::json!({
            "codingAgentProfiles": base.as_object_mut().unwrap().remove("codingAgentProfile\
                s").unwrap()
        }),
    );
    b3_write(&path, &base);
    let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    assert_eq!(written.agents.len(), 2);
    hash_activation_assert(&written);
}

pub(super) fn b3_all_loaders(
    path: &std::path::Path,
) -> Vec<(&'static str, super::super::AppSettings)> {
    vec![
        (
            "load_settings_from_path",
            super::super::load_settings_from_path(path),
        ),
        (
            "load_settings_for_cli",
            super::super::load_settings_for_cli_from_path(path),
        ),
        (
            "load_settings_for_cli_strict",
            super::super::load_settings_for_cli_strict_from_path(Some(path)).unwrap(),
        ),
    ]
}

#[test]
fn the_export_moves_both_groups_and_leaves_memory_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    let before = super::super::load_settings_for_cli_from_path(&path);
    assert!(!b3_sidecar_path(temp.path()).exists());

    let loaded = super::super::load_settings_from_path(&path);

    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Alpha Recognisable");
    assert_eq!(
        sidecar["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::super::configuration_identity("claude", &[])]["A"],
        "Recognisable P"
    );
    let disk = b3_object(&path);
    assert!(!disk.contains_key("agents"), "{disk:?}");
    assert!(!disk.contains_key("codingAgentProfiles"), "{disk:?}");
    assert_eq!(
        serde_json::to_value(&loaded.agents).unwrap(),
        serde_json::to_value(&before.agents).unwrap()
    );
    // The export's save adds the #2450 identity rows, as any save does; every
    // user-authored value is unchanged.
    let mut loaded_profiles = loaded.coding_agent_profiles.clone();
    loaded_profiles.identity_by_agent.clear();
    assert_eq!(loaded_profiles, before.coding_agent_profiles);
    assert_eq!(b3_labels(&loaded), vec!["Alpha Recognisable".to_string()]);
    assert_eq!(b3_marker(&loaded).as_deref(), Some("Recognisable P"));
}

#[test]
fn all_three_loaders_see_the_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    super::super::load_settings_from_path(&path);
    assert!(!b3_object(&path).contains_key("agents"));

    for (loader, settings) in b3_all_loaders(&path) {
        assert_eq!(
            b3_labels(&settings),
            vec!["Alpha Recognisable".to_string()],
            "{loader} did not see the agents file"
        );
        assert_eq!(
            b3_marker(&settings).as_deref(),
            Some("Recognisable P"),
            "{loader} did not see the agents file"
        );
    }
}

#[test]
fn a_cli_strict_load_then_save_does_not_write_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    super::super::load_settings_from_path(&path);

    let settings = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Alpha Recognisable");
    assert_eq!(sidecar["agents"].as_array().unwrap().len(), 1);
    assert_eq!(
        sidecar["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::super::configuration_identity("claude", &[])]["A"],
        "Recognisable P"
    );
}

#[test]
fn reconcile_mode_drops_the_legacy_keys_from_the_disk_object() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    let settings = super::super::load_settings_for_cli_from_path(&path);

    super::super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();

    let disk = b3_object(&path);
    assert!(!disk.contains_key("agents"), "{disk:?}");
    assert!(!disk.contains_key("codingAgentProfiles"), "{disk:?}");
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Alpha Recognisable");
}

#[test]
fn preserve_mode_drops_the_legacy_keys_from_the_typed_object() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    let settings = super::super::load_settings_for_cli_from_path(&path);

    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let disk = b3_object(&path);
    assert!(!disk.contains_key("agents"), "{disk:?}");
    assert!(!disk.contains_key("codingAgentProfiles"), "{disk:?}");
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Alpha Recognisable");
}

/// E9 and E10: a directory at the agents file's name.
fn b3_assert_a_failed_sidecar_write_strips_nothing(reconcile: bool) {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    std::fs::create_dir(b3_sidecar_path(temp.path())).unwrap();
    let settings = super::super::load_settings_for_cli_from_path(&path);
    let before = std::fs::read(&path).unwrap();

    let result = if reconcile {
        super::super::save_settings_with_project_paths_to_path(&settings, &path).map(|_| ())
    } else {
        super::super::save_settings_to_path_preserving_project_paths(&settings, &path).map(|_| ())
    };

    assert!(result.is_err(), "the save must fail");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let disk = b3_object(&path);
    assert!(disk.contains_key("agents") && disk.contains_key("codingAgentProfiles"));
    assert!(b3_sidecar_path(temp.path()).is_dir());
}

#[test]
fn reconcile_mode_keeps_both_keys_when_the_sidecar_write_fails() {
    b3_assert_a_failed_sidecar_write_strips_nothing(true);
}

#[test]
fn preserve_mode_keeps_both_keys_when_the_sidecar_write_fails() {
    b3_assert_a_failed_sidecar_write_strips_nothing(false);
}

/// E10b. The overlay reuses the base agent's id, so the D13 derived-id closure
/// adds no `codingAgentProfiles` path and the overlay owns `agents` only.
#[test]
fn a_preserve_save_keeps_an_overlay_owned_key_when_no_sidecar_holds_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_legacy_fixture("Base A", "Recognisable P"));
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({ "agents": [b3_agent("alpha", "Overlay B")] }),
    );
    let settings = super::super::load_settings_for_cli_from_path(&path);
    assert!(settings
        .local_overlay_state
        .owns_top_level(super::super::OVERLAY_KEY_AGENTS));
    assert!(!settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles"));

    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert!(!sidecar.contains_key("agents"), "{sidecar:?}");
    assert!(sidecar.contains_key("codingAgentProfiles"), "{sidecar:?}");
    let disk = b3_object(&path);
    assert!(!disk.contains_key("codingAgentProfiles"), "{disk:?}");
    assert_eq!(disk["agents"][0]["label"], "Base A", "{disk:?}");
    std::fs::remove_file(b3_overlay_path(temp.path())).unwrap();
    let reloaded = super::super::load_settings_for_cli_from_path(&path);
    assert_eq!(b3_labels(&reloaded), vec!["Base A".to_string()]);
}

#[test]
fn the_export_aborts_without_stripping() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    std::fs::create_dir(b3_sidecar_path(temp.path())).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut settings = super::super::load_settings_for_cli_from_path(&path);
    let agents = serde_json::to_value(&settings.agents).unwrap();
    let profiles = settings.coding_agent_profiles.clone();

    assert!(!super::super::export_agents_to_instance_file(
        &mut settings,
        &path
    ));
    let loaded = super::super::load_settings_from_path(&path);

    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(serde_json::to_value(&settings.agents).unwrap(), agents);
    assert_eq!(settings.coding_agent_profiles, profiles);
    assert_eq!(serde_json::to_value(&loaded.agents).unwrap(), agents);
    assert!(b3_sidecar_path(temp.path()).is_dir());
}

#[test]
fn an_overlay_that_owns_agents_blocks_the_export() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_legacy_fixture("Base A", "Recognisable P"));
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({ "agents": [b3_agent("alpha", "Overlay B")] }),
    );
    let mut settings = super::super::load_settings_for_cli_from_path(&path);
    assert!(!super::super::export_agents_to_instance_file(
        &mut settings,
        &path
    ));

    let loaded = super::super::load_settings_from_path(&path);

    assert!(
        !b3_sidecar_path(temp.path()).exists()
            || !b3_object(&b3_sidecar_path(temp.path())).contains_key("agents")
    );
    assert!(
        b3_object(&path).contains_key("agents"),
        "E12: the settings file keeps agents"
    );
    assert_eq!(b3_labels(&loaded), vec!["Overlay B".to_string()]);
}

#[test]
fn a_sidecar_that_disagrees_with_the_legacy_keys_is_not_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let sidecar_bytes = b3_seed_sidecar(temp.path(), "Sidecar", "Sidecar P");
    b3_write(&path, &b3_legacy_fixture("Legacy", "Legacy P"));
    let mut settings = super::super::load_settings_for_cli_from_path(&path);
    assert!(!super::super::export_agents_to_instance_file(
        &mut settings,
        &path
    ));

    let legacy = b3_object(&path);
    let loaded = super::super::load_settings_from_path(&path);

    assert_eq!(b3_labels(&loaded), vec!["Sidecar".to_string()]);
    assert_eq!(b3_marker(&loaded).as_deref(), Some("Sidecar P"));
    assert_ne!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        sidecar_bytes
    );
    let stable = std::fs::read(b3_sidecar_path(temp.path())).unwrap();
    super::super::load_settings_from_path(&path);
    assert_eq!(std::fs::read(b3_sidecar_path(temp.path())).unwrap(), stable);
    let disk = b3_object(&path);
    assert!(disk.contains_key("agents") && disk.contains_key("codingAgentProfiles"));

    super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();

    let disk = b3_object(&path);
    assert_eq!(disk["agents"], legacy["agents"]);
    assert_eq!(disk["codingAgentProfiles"], legacy["codingAgentProfiles"]);
    // The save may add the #2450 identity rows; the sidecar's values still win.
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Sidecar");
    assert_eq!(
        sidecar["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::super::configuration_identity("claude", &[])]["A"],
        "Sidecar P"
    );
}

#[test]
fn a_corrupt_sidecar_empties_nothing_and_blocks_the_strip() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_legacy_fixture("Legacy", "Legacy P"));
    std::fs::write(b3_sidecar_path(temp.path()), b"{ not json").unwrap();
    let before = std::fs::read(&path).unwrap();

    let loaded = super::super::load_settings_from_path(&path);

    assert_eq!(b3_labels(&loaded), vec!["Legacy".to_string()]);
    assert_eq!(b3_marker(&loaded).as_deref(), Some("Legacy P"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        b"{ not json"
    );
    assert!(super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).is_err());
    let disk = b3_object(&path);
    assert!(disk.contains_key("agents") && disk.contains_key("codingAgentProfiles"));
    assert_eq!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        b"{ not json"
    );
}
