use super::*;

#[test]
fn issue_3047_pair_table_and_independent_preimages() {
    use sha2::{Digest, Sha256};
    // Independently reproduce both normalization stages of a pair.
    let normalize = |parts: &[&str]| {
        let mut chars: Vec<char> = parts
            .concat()
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|c| !c.is_whitespace())
            .collect();
        chars.sort_unstable();
        chars.iter().collect::<String>()
    };
    let expected = |base_parts: &[&str], profile_parts: &[&str]| {
        let base = normalize(base_parts);
        let profile = normalize(profile_parts);
        format!("cid1:{:x}", Sha256::digest(normalize(&[&base, &profile])))
    };
    let mut agents: Vec<super::super::AgentConfig> = serde_json::from_value(serde_json::json!([
        {"id":"a","label":"Alpha","color":"red","baseCommand":"Codex",
         "envs":[{"key":"K","value":"V","enabled":false}]},
        {"id":"b","label":"Beta","color":"blue","baseCommand":"Claude"}
    ]))
    .unwrap();
    let mut profiles: super::super::CodingAgentProfilesConfig =
        serde_json::from_value(serde_json::json!({"profilesByAgent": {
            "a":{"A":{"additionalArguments":"--model x","env":{"P":"Q"}},
                 "B":{"additionalArguments":"--model y","enabled":false}},
            "b":{"A":{"additionalArguments":"--effort low"},
                 "B":{"additionalArguments":"--effort high"}}
        }}))
        .unwrap();
    super::super::refresh_agent_id_hashes(&mut agents);
    let base = agents[0].id_hash.clone();
    super::super::refresh_pair_id_hashes(&agents, &mut profiles);
    let original = profiles.clone();
    for (id, letter, leaves) in [
        ("a", "A", vec!["Codex", "K", "V", "--model x", "P", "Q"]),
        ("a", "B", vec!["Codex", "K", "V", "--model y"]),
        ("b", "A", vec!["Claude", "--effort low"]),
        ("b", "B", vec!["Claude", "--effort high"]),
    ] {
        assert_eq!(
            profiles.profiles_by_agent[id][letter].id_hash_debug,
            Some(leaves.into_iter().map(String::from).collect())
        );
    }
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected(&["Codex", "K", "V"], &["--model x", "P", "Q"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"].id_hash,
        Some(expected(&["Codex", "K", "V"], &["--model y"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["b"]["A"].id_hash,
        Some(expected(&["Claude"], &["--effort low"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["b"]["B"].id_hash,
        Some(expected(&["Claude"], &["--effort high"]))
    );
    assert_ne!(
        profiles.profiles_by_agent["b"]["A"].id_hash,
        profiles.profiles_by_agent["b"]["B"].id_hash
    );
    profiles
        .profiles_by_agent
        .get_mut("a")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .command = "--model z".into();
    super::super::refresh_pair_id_hashes(&agents, &mut profiles);
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected(&["Codex", "K", "V"], &["--model z", "P", "Q"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"],
        original.profiles_by_agent["a"]["B"]
    );
    assert_eq!(
        profiles.profiles_by_agent["b"],
        original.profiles_by_agent["b"]
    );
    assert_eq!(agents[0].id_hash, base);
    agents[0].command = "Other".into();
    agents[0].envs[0].value = "W".into();
    super::super::refresh_pair_id_hashes(&agents, &mut profiles);
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected(&["Other", "K", "W"], &["--model z", "P", "Q"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"].id_hash,
        Some(expected(&["Other", "K", "W"], &["--model y"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["b"],
        original.profiles_by_agent["b"]
    );
    profiles
        .profiles_by_agent
        .get_mut("a")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .env
        .insert("P".into(), "R".into());
    super::super::refresh_pair_id_hashes(&agents, &mut profiles);
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected(&["Other", "K", "W"], &["--model z", "P", "R"]))
    );
    assert_eq!(
        agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    // Original leaves retain vector order, disabled/duplicate/empty rows,
    // whitespace and Unicode. Expected hashes use two separate stages.
    agents[0].command = "İΣΣ\u{2003} oo --".into();
    agents[0].envs = serde_json::from_value(serde_json::json!([
        {"key":" K ","value":"İ","enabled":false},
        {"key":"","value":""},
        {"key":" K ","value":"İ","enabled":true}
    ]))
    .unwrap();
    let cell = profiles
        .profiles_by_agent
        .get_mut("a")
        .unwrap()
        .get_mut("A")
        .unwrap();
    cell.command = "Σ İ\u{a0}oo".into();
    cell.env = BTreeMap::from([("Z".into(), "Last".into()), ("A".into(), "".into())]);
    cell.id_hash_debug = Some(vec!["forged-pair".into()]);
    super::super::refresh_agent_id_hashes(&mut agents);
    let leaves = vec!["İΣΣ\u{2003} oo --", " K ", "İ", "", "", " K ", "İ"];
    assert_eq!(
        agents[0].id_hash_debug,
        Some(leaves.iter().map(|s| (*s).into()).collect())
    );
    let base_hash = agents[0].id_hash.clone();
    agents[0].id_hash_debug = Some(vec!["forged-base".into()]);
    super::super::refresh_pair_id_hashes(&agents, &mut profiles);
    let mut pair = leaves.clone();
    pair.extend(["Σ İ\u{a0}oo", "A", "", "Z", "Last"]);
    let base_preimage = normalize(&leaves);
    let profile_preimage = normalize(&pair[leaves.len()..]);
    let expected_hash = format!(
        "cid1:{:x}",
        Sha256::digest(normalize(&[&base_preimage, &profile_preimage]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected_hash)
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash_debug,
        Some(pair.into_iter().map(String::from).collect())
    );
    assert_eq!(agents[0].id_hash, base_hash);
}

#[test]
fn issue_3047_pair_equivalence_exclusions_empty_orphan_and_first_match() {
    let mut agents: Vec<super::super::AgentConfig> = serde_json::from_value(serde_json::json!([
        {"id":"a","label":"First","color":"red","command":"İ A\u{2003}b"},
        {"id":"a","label":"Second","color":"blue","command":"wrong"}
    ]))
    .unwrap();
    let mut profiles: super::super::CodingAgentProfilesConfig =
        serde_json::from_value(serde_json::json!({"profilesByAgent": {
            "a":{"A":{"command":" XY"},"B":{}},
            "orphan":{"A":{"id_hash":"forged","id_hash_debug":["forged"]}}
        }}))
        .unwrap();
    super::super::refresh_pair_id_hashes(&agents, &mut profiles);
    let original = profiles.profiles_by_agent["a"]["A"].clone();
    assert_eq!(
        original.id_hash,
        Some(super::super::configuration_identity("i\u{307}abxy", &[]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"].id_hash,
        Some(super::super::configuration_identity(
            &agents[0].command,
            &[]
        ))
    );
    assert!(!profiles.profiles_by_agent["a"].contains_key("C"));
    assert_eq!(profiles.profiles_by_agent["orphan"]["A"].id_hash, None);
    assert_eq!(
        profiles.profiles_by_agent["orphan"]["A"].id_hash_debug,
        None
    );
    assert_eq!(
        original.id_hash_debug,
        Some(vec!["İ A\u{2003}b".into(), " XY".into()])
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"].id_hash_debug,
        Some(vec!["İ A\u{2003}b".into(), "".into()])
    );
    agents[0].label = "Renamed".into();
    agents[0].command = "b aİ".into();
    profiles
        .default_profile_by_agent
        .insert("a".into(), "B".into());
    profiles.profile_labels_by_agent.insert(
        "a".into(),
        std::collections::BTreeMap::from([("A".into(), "Renamed profile".into())]),
    );
    let cell = profiles
        .profiles_by_agent
        .get_mut("a")
        .unwrap()
        .get_mut("A")
        .unwrap();
    cell.command = "y\u{a0}x".into();
    cell.enabled = false;
    cell.notes = "ignored".into();
    cell.preflight = Some("ignored too".into());
    super::super::refresh_pair_id_hashes(&agents, &mut profiles);
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        original.id_hash
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash_debug,
        Some(vec!["b aİ".into(), "y\u{a0}x".into()])
    );
    assert_ne!(
        profiles.profiles_by_agent["a"]["A"].id_hash_debug,
        original.id_hash_debug
    );
    let cell = &profiles.profiles_by_agent["a"]["A"];
    let pid = super::super::profile_identity(cell);
    let mut forged = cell.clone();
    forged.id_hash = Some("client".into());
    assert_eq!(super::super::profile_identity(&forged), pid);
    assert_ne!(
        super::super::pair_identity("ab", &[], cell),
        super::super::pair_identity("aab", &[], cell)
    );
}

fn assert_issue_3047_pair_adoption_view(
    settings: &super::super::AppSettings,
    orphan: &str,
    sidecar: bool,
) {
    let cells = &settings.coding_agent_profiles.profiles_by_agent["alpha"];
    let mut expected_a = super::super::empty_profile_cell();
    expected_a.id_hash = Some(super::super::configuration_identity("claude", &[]));
    expected_a.id_hash_debug = Some(vec!["claude".into(), "".into()]);
    assert_eq!(cells["A"], expected_a, "{sidecar}/{orphan}");
    assert_eq!(cells.len(), 1, "quarantine never supplies active cells");
    assert!(!settings
        .coding_agent_profiles
        .profiles_by_agent
        .contains_key("o"));
}

fn check_issue_3047_pair_adoption_case(sidecar: bool, orphan: &str, fail_publish: bool) {
    let (temp, path) = hash_activation_seed(sidecar);
    let physical = if sidecar {
        b3_sidecar_path(temp.path())
    } else {
        path.clone()
    };
    let mut raw = serde_json::Value::Object(b3_object(&physical));
    let agent: super::super::AgentConfig =
        serde_json::from_value(raw["agents"][0].clone()).unwrap();
    raw["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({});
    raw["codingAgentProfiles"]["identityByAgent"] = serde_json::json!({});
    raw["codingAgentProfiles"]["profileLabelsByAgent"] = serde_json::json!({});
    if orphan != "missing" {
        let letter = if orphan == "B" { "B" } else { "A" };
        raw["codingAgentProfiles"]["profilesByAgent"]["o"] = serde_json::json!({
            letter:{"additionalArguments":"--special","notes":"payload"}
        });
        raw["codingAgentProfiles"]["identityByAgent"]["o"] =
            serde_json::json!(if orphan == "unmatched" {
                "1234567890abcdef".to_string()
            } else {
                super::super::agent_profile_identity(&agent)
            });
    }
    b3_write(&physical, &raw);
    let before = hash_activation_files(temp.path());
    let cli = super::super::load_settings_for_cli_from_path(&path);
    let strict = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    for settings in [&cli, &strict] {
        assert_issue_3047_pair_adoption_view(settings, orphan, sidecar);
    }
    assert_eq!(hash_activation_files(temp.path()), before);
    let main_before = std::fs::read(&path).unwrap();
    if fail_publish {
        super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
            *slot.borrow_mut() =
                Some((path.clone(), super::super::SettingsSaveStage::AtomicReplace))
        });
    }
    let (records, _capture_guard) = capture_settings_save_diagnostics();
    let gui = super::super::load_settings_from_path(&path);
    // Inspect the GUI diagnostic and the same typed failure boundary.
    if fail_publish {
        let captured = records.borrow();
        assert_eq!(captured.len(), 1);
        let fields = parse_settings_save_diagnostic(&captured[0]);
        assert_eq!(fields["stage"], "atomic_replace");
        assert_eq!(
            fields["target_path"],
            super::super::render_diagnostic_path(&path)
        );
        drop(captured);
        let error = super::super::save_settings_to_path_preserving_project_paths_typed(&gui, &path)
            .unwrap_err();
        assert_eq!(error.stage, super::super::SettingsSaveStage::AtomicReplace);
        assert_eq!(error.target_path.as_ref(), &path);
    }
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert_eq!(
        gui.coding_agent_profiles.profiles_by_agent,
        strict.coding_agent_profiles.profiles_by_agent
    );
    if fail_publish {
        assert_eq!(std::fs::read(&path).unwrap(), main_before);
        let stored = b3_object(&b3_sidecar_path(temp.path()));
        let profiles = &stored["codingAgentProfiles"];
        assert_eq!(profiles["profileBindingVersion"], 1);
        assert!(profiles["profilesByBaseHash"]
            [&super::super::configuration_identity("claude", &[])]["A"]
            .is_object());
        assert!(profiles.get("profilesByAgent").is_none());
        if orphan != "missing" {
            let mut expected = serde_json::json!({
                "cells":raw["codingAgentProfiles"]["profilesByAgent"]["o"],
                "identity":raw["codingAgentProfiles"]["identityByAgent"]["o"]
            });
            if let Some(labels) = raw["codingAgentProfiles"]["profileLabelsByAgent"].get("o") {
                expected["labels"] = labels.clone();
            }
            assert_eq!(
                profiles["legacyProfileOrphans"]["o"],
                serde_json::json!([expected])
            );
        }
    } else {
        let reload = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert_eq!(gui.coding_agent_profiles, reload.coding_agent_profiles);
    }
}

#[test]
fn issue_3047_pair_load_save_fixed_point_and_untrusted_metadata() {
    for sidecar in [false, true] {
        for supplied in [
            serde_json::json!(null),
            serde_json::json!(false),
            serde_json::json!(42),
            serde_json::json!(1.5),
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!(""),
            serde_json::json!("forged"),
            serde_json::json!(["forged", ""]),
            serde_json::json!(["mixed", 42]),
        ] {
            let (temp, path) = hash_activation_seed(sidecar);
            let physical = if sidecar {
                b3_sidecar_path(temp.path())
            } else {
                path.clone()
            };
            let mut raw = serde_json::Value::Object(b3_object(&physical));
            raw["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
                "alpha":{"A":{"additionalArguments":"--model \
                    x","id_hash":supplied,"id_hash_debug":supplied,
                    "sentinel":{"keep":true}}}
            });
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
            for (loader, loaded) in b3_all_loaders(&path) {
                let cell = &loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"];
                assert_eq!(
                    cell.id_hash_debug,
                    Some(vec!["claude".into(), "--model x".into()])
                );
                assert_eq!(
                    cell.id_hash,
                    Some(super::super::configuration_identity("claude--modelx", &[])),
                    "{loader}"
                );
                assert_eq!(hash_activation_files(temp.path()), before, "{loader}");
            }
            let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
            let written =
                super::super::save_settings_to_path_preserving_project_paths(&loaded, &path)
                    .unwrap();
            let groups = b3_object(&b3_sidecar_path(temp.path()));
            let stored_cell = &groups["codingAgentProfiles"]["profilesByBaseHash"]
                [&super::super::configuration_identity("claude", &[])]["A"];
            assert_eq!(
                stored_cell,
                &serde_json::json!({"additionalArguments":"--model \
                    x","enabled":true,"env":{},"notes":"","sentinel":{"keep":true}})
            );
            assert_eq!(
                written.coding_agent_profiles.profiles_by_agent["alpha"]["A"].id_hash_debug,
                Some(vec!["claude".into(), "--model x".into()])
            );
            let stable = hash_activation_files(temp.path());
            let reloaded =
                super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
            super::super::save_settings_to_path_preserving_project_paths(&reloaded, &path).unwrap();
            assert_eq!(hash_activation_files(temp.path()), stable);
            let mut stale = reloaded.clone();
            stale
                .coding_agent_profiles
                .profiles_by_agent
                .get_mut("alpha")
                .unwrap()
                .get_mut("A")
                .unwrap()
                .id_hash = Some("stale".into());
            stale
                .coding_agent_profiles
                .profiles_by_agent
                .get_mut("alpha")
                .unwrap()
                .get_mut("A")
                .unwrap()
                .id_hash_debug = Some(vec!["stale".into()]);
            let cas = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
                &stale,
                &path,
                stale.terminal_snapshots_enabled,
                stale.terminal_snapshots_enabled,
            )
            .unwrap();
            assert_eq!(cas.coding_agent_profiles, reloaded.coding_agent_profiles);
        }
    }
    // Missing-A repair and matching orphan adoption happen after parse.
    // Both CLI loaders must return final hashes without publishing anything.
    for sidecar in [false, true] {
        for orphan in ["missing", "A", "B", "unmatched"] {
            for fail_publish in [false, true] {
                check_issue_3047_pair_adoption_case(sidecar, orphan, fail_publish);
            }
        }
    }
}

fn assert_issue_3047_pair_projection_rejection(
    error: super::super::SettingsSaveError,
    sidecar: bool,
    path: &std::path::Path,
    sidecar_path: &std::path::Path,
) {
    assert_eq!(
        error.stage,
        super::super::SettingsSaveStage::PreserveDiskGate
    );
    if sidecar {
        assert_eq!(error.target_path.as_path(), sidecar_path);
        assert!(matches!(&error.legacy_outward,
                super::super::SettingsSaveLegacyOutward::DiskRead(text)
                if text.contains("Refusing to overwrite") && text.contains("undecodable agents \
                    array")));
    } else {
        assert_eq!(error.target_path.as_path(), path);
        assert!(matches!(
            error.cause,
            super::super::SettingsSaveCause::Semantic(
                super::super::SettingsSaveReason::DiskSettingsValidationRejected
            )
        ));
        assert!(matches!(&error.legacy_outward,
                super::super::SettingsSaveLegacyOutward::DiskValidation(text)
                if text.contains("non-project fields are invalid")));
        assert!(!sidecar_path.exists());
    }
}

#[test]
fn issue_3047_pair_protected_base_projection_and_early_rejections() {
    for sidecar in [false, true] {
        for invalid in [false, true] {
            let (temp, path) = hash_activation_seed(sidecar);
            let physical = if sidecar {
                b3_sidecar_path(temp.path())
            } else {
                path.clone()
            };
            let mut raw = serde_json::Value::Object(b3_object(&physical));
            raw["agents"][0]["sentinel"] = serde_json::json!({"keep":19});
            raw["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
                "alpha":{"A":{"additionalArguments":"--model x","sentinel":"cell"}}
            });
            b3_write(&physical, &raw);
            let mut overlay_agent = b3_agent("alpha", "Overlay");
            overlay_agent["command"] = serde_json::json!("codex --overlay");
            b3_write(
                &b3_overlay_path(temp.path()),
                &serde_json::json!({"agents":[overlay_agent]}),
            );
            let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
            assert!(loaded.local_overlay_state.owns_top_level("agents"));
            if invalid {
                raw["agents"][0]["label"] = serde_json::json!(17);
                b3_write(&physical, &raw);
            }
            if !sidecar {
                std::fs::remove_file(b3_sidecar_path(temp.path())).unwrap();
            }
            let before = std::fs::read(&physical).unwrap();
            let mut out = serde_json::to_value(&loaded).unwrap();
            out["codingAgentProfiles"] = raw["codingAgentProfiles"].clone();
            let result = super::super::save_agents_instance_file(
                &loaded,
                &path,
                &super::super::ProjectWriteMode::Preserve,
                None,
                out.as_object().unwrap(),
                super::super::SettingsSaveStage::PreserveDiskGate,
                (None, None, None),
            );
            if invalid {
                let error = result.unwrap_err();
                assert_issue_3047_pair_projection_rejection(
                    error,
                    sidecar,
                    &path,
                    &b3_sidecar_path(temp.path()),
                );
                assert_eq!(std::fs::read(&physical).unwrap(), before);
            } else {
                let groups = result.unwrap();
                assert_eq!(
                    groups["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["id_hash"],
                    super::super::configuration_identity("claude--modelx", &[])
                );
                assert_eq!(
                    groups["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["sentinel"],
                    "cell"
                );
                if sidecar {
                    assert_eq!(groups["agents"], raw["agents"]);
                } else {
                    assert_eq!(b3_object(&path)["agents"], raw["agents"]);
                    assert!(!groups.contains_key("agents"));
                }
            }
        }
    }
}

#[test]
fn issue_3047_pair_unresolvable_first_row_and_empty_physical_base() {
    for first in [
        serde_json::json!({"id":"a","baseCommand":17}),
        serde_json::json!({"id":"a","baseCommand":"codex","envs":"bad"}),
    ] {
        let agents = serde_json::json!([first, {"id":"a","baseCommand":"valid"}]);
        let before = agents.clone();
        let mut profiles = serde_json::json!({"profilesByAgent":{
            "a":{"A":{"command":"--model \
                x","id_hash":"stale","id_hash_debug":["stale"],"unknown":1}}
        }});
        super::super::refresh_physical_pair_id_hashes(Some(&agents), &mut profiles);
        assert!(profiles["profilesByAgent"]["a"]["A"]
            .get("id_hash_debug")
            .is_none());
        assert!(profiles["profilesByAgent"]["a"]["A"]
            .get("id_hash")
            .is_none());
        assert_eq!(profiles["profilesByAgent"]["a"]["A"]["unknown"], 1);
        assert_eq!(agents, before);
        let (temp, path) = hash_activation_seed(true);
        let mut groups = serde_json::Value::Object(b3_object(&b3_sidecar_path(temp.path())));
        groups["agents"] = agents;
        b3_write(&b3_sidecar_path(temp.path()), &groups);
        let bytes = std::fs::read(b3_sidecar_path(temp.path())).unwrap();
        assert!(super::super::save_agents_instance_file(
            &super::super::AppSettings::default(),
            &path,
            &super::super::ProjectWriteMode::Preserve,
            None,
            &serde_json::Map::new(),
            super::super::SettingsSaveStage::PreserveDiskGate,
            (None, None, None),
        )
        .is_err());
        assert_eq!(std::fs::read(b3_sidecar_path(temp.path())).unwrap(), bytes);
    }
    for agents in [None, Some(serde_json::json!([]))] {
        let mut profiles = serde_json::json!({"profilesByAgent":{
            "a":{"A":{"id_hash":"stale","id_hash_debug":["stale"]}}
        }});
        super::super::refresh_physical_pair_id_hashes(agents.as_ref(), &mut profiles);
        assert!(profiles["profilesByAgent"]["a"]["A"]
            .get("id_hash_debug")
            .is_none());
        assert!(profiles["profilesByAgent"]["a"]["A"]
            .get("id_hash")
            .is_none());
    }
    let agents = serde_json::json!([{"id":"a","command":"codex","label":17,
            "envs":[{"key":"K","value":"V","enabled":false}]},
            {"id":"a","baseCommand":"wrong"}]);
    let mut profiles = serde_json::json!({"profilesByAgent":{
        "a":{"A":{"command":"--model x","env":{"P":"Q"},"unknown":1}}
    }});
    let before = profiles.clone();
    super::super::refresh_physical_pair_id_hashes(Some(&agents), &mut profiles);
    assert_eq!(
        profiles["profilesByAgent"]["a"]["A"]["id_hash"],
        super::super::configuration_identity("codexKV--modelxPQ", &[])
    );
    let mut expected = before;
    expected["profilesByAgent"]["a"]["A"]["id_hash"] =
        serde_json::json!("cid1:87849ab8d8f223bc755ecbc34b1e772ca61469dd5651dcf52063949e23001036");
    expected["profilesByAgent"]["a"]["A"]["id_hash_debug"] =
        serde_json::json!(["codex", "K", "V", "--model x", "P", "Q"]);
    assert_eq!(profiles, expected);
    let mut profiles = serde_json::json!({"profilesByAgent":{
        "a":{"A":{},"B":{"additionalArguments":17,"id_hash":"stale",
            "id_hash_debug":["stale"],"unknown":7},"C":false}
    }});
    super::super::refresh_physical_pair_id_hashes(Some(&agents), &mut profiles);
    assert_eq!(
        profiles["profilesByAgent"]["a"]["A"]["id_hash_debug"],
        serde_json::json!(["codex", "K", "V", ""])
    );
    assert_eq!(
        profiles["profilesByAgent"]["a"]["B"],
        serde_json::json!({"additionalArguments":17,"unknown":7})
    );
    assert_eq!(profiles["profilesByAgent"]["a"]["C"], false);
}

fn check_issue_3047_pair_ownership_case(sidecar: bool, protected: &str) {
    let (temp, path) = hash_activation_seed(sidecar);
    let physical = if sidecar {
        b3_sidecar_path(temp.path())
    } else {
        path.clone()
    };
    let mut raw = serde_json::Value::Object(b3_object(&physical));
    raw["agents"][0]["sentinel"] = serde_json::json!({"agent": 19});
    raw["codingAgentProfiles"] = serde_json::json!({
        "schemaVersion": 2,
        "sentinel": {"sibling": 23},
        "profilesByAgent": {"alpha":{"A":{"command":"--model \
            x","id_hash":"physical", "id_hash_debug":["forged-physical"], "sentinel":17}}}
    });
    b3_write(&physical, &raw);
    if protected != "neither" {
        let mut overlay = serde_json::Map::new();
        let mut value = raw[protected].clone();
        if protected == "agents" {
            value[0].as_object_mut().unwrap().remove("baseCommand");
            value[0]["command"] = serde_json::json!("codex");
        } else {
            value["profilesByAgent"]["alpha"]["A"]["command"] = serde_json::json!("--effort high");
        }
        overlay.insert(protected.into(), value);
        b3_write(
            &b3_overlay_path(temp.path()),
            &serde_json::Value::Object(overlay),
        );
    }
    let mut loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    if protected == "agents" {
        loaded
            .coding_agent_profiles
            .profiles_by_agent
            .get_mut("alpha")
            .unwrap()
            .get_mut("A")
            .unwrap()
            .command = "--model y".into();
    } else {
        loaded.agents[0].command = "other-base".into();
    }
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    let cell = &written.coding_agent_profiles.profiles_by_agent["alpha"]["A"];
    let effective_base = if protected == "agents" {
        "codex"
    } else {
        "other-base"
    };
    let effective_args = if protected == "codingAgentProfiles" {
        "--effort high"
    } else if protected == "agents" {
        "--model y"
    } else {
        "--model x"
    };
    assert_eq!(
        cell.id_hash_debug,
        Some(vec![effective_base.into(), effective_args.into()])
    );
    assert_eq!(
        cell.id_hash,
        Some(super::super::configuration_identity(
            &format!("{effective_base}{effective_args}"),
            &[]
        ))
    );
    let groups = b3_object(&b3_sidecar_path(temp.path()));
    if protected == "codingAgentProfiles" {
        let retained = groups
            .get(protected)
            .cloned()
            .unwrap_or_else(|| b3_object(&path)[protected].clone());
        let mut expected = raw[protected].clone();
        if !sidecar {
            expected["identityByAgent"] = serde_json::json!({
                "alpha": super::super::agent_profile_identity(&written.agents[0])
            });
        }
        assert_eq!(retained, expected);
    } else {
        let expected_base = if protected == "agents" {
            "claude"
        } else {
            "other-base"
        };
        let expected_args = if protected == "agents" {
            "--model y"
        } else {
            "--model x"
        };
        let hash = super::super::configuration_identity(expected_base, &[]);
        let main = b3_object(&path);
        let profiles = groups
            .get("codingAgentProfiles")
            .or_else(|| main.get("codingAgentProfiles"))
            .expect("physical profile owner");
        assert_eq!(profiles["profileBindingVersion"], 1);
        assert_eq!(
            profiles["profilesByBaseHash"][&hash]["A"],
            serde_json::json!({
                "command":"--model x", "additionalArguments":expected_args,
                "enabled":true, "env":{}, "notes":"", "sentinel":17
            })
        );
        assert_eq!(profiles["sentinel"], raw["codingAgentProfiles"]["sentinel"]);
        for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
            assert!(profiles.get(key).is_none());
        }
    }
    if protected == "agents" {
        let retained = groups
            .get("agents")
            .cloned()
            .unwrap_or_else(|| b3_object(&path)["agents"].clone());
        assert_eq!(retained, raw["agents"]);
    }
    let reloaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    assert_eq!(
        reloaded.coding_agent_profiles,
        written.coding_agent_profiles
    );
}

#[test]
fn issue_3047_pair_ownership_matrix_and_base_or_profile_only_edits() {
    for sidecar in [false, true] {
        for protected in ["agents", "codingAgentProfiles", "neither"] {
            check_issue_3047_pair_ownership_case(sidecar, protected);
        }
    }
    // Presence is independent of emptiness. Exercise the real writer with
    // sparse legacy groups, including D13's partially protected group.
    for case in [
        "present-absent",
        "absent-present",
        "absent-absent",
        "empty-present",
        "sidecar-empty",
    ] {
        let (temp, path) = hash_activation_seed(false);
        let mut raw = serde_json::Value::Object(b3_object(&path));
        raw["agents"] = serde_json::json!([]);
        let sparse =
            serde_json::json!({"schemaVersion":2,"sentinel":{"raw":29},"profilesByAgent":{}});
        if case.starts_with("absent-") {
            raw.as_object_mut().unwrap().remove("codingAgentProfiles");
        } else {
            raw["codingAgentProfiles"] = sparse.clone();
        }
        let overlay = match case {
            "present-absent" => {
                serde_json::json!({"codingAgentProfiles":{"identityByAgent":{"o":"1234567890abcde\
                    f"}}})
            }
            "absent-present" => {
                serde_json::json!({"agents":[b3_agent("alpha", "Effective only")]})
            }
            _ => {
                serde_json::json!({"codingAgentProfiles":{"profilesByAgent":{"o":{"A":{"command":"-\
                    -model x"}}}}})
            }
        };
        if case == "sidecar-empty" {
            b3_write(
                &b3_sidecar_path(temp.path()),
                &serde_json::json!({"agents":[],"codingAgentProfiles":{}}),
            );
        }
        b3_write(&path, &raw);
        b3_write(&b3_overlay_path(temp.path()), &overlay);
        let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert!(
            loaded
                .local_overlay_state
                .owns_top_level("codingAgentProfiles"),
            "{case}"
        );
        super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
        let main = b3_object(&path);
        let groups = b3_object(&b3_sidecar_path(temp.path()));
        match case {
            "present-absent" => assert_eq!(main["codingAgentProfiles"], sparse),
            "absent-present" => assert_eq!(
                main["codingAgentProfiles"],
                serde_json::json!({"identityByAgent":{}})
            ),
            "absent-absent" => assert!(!main.contains_key("codingAgentProfiles")),
            "empty-present" => {
                let mut expected = sparse.clone();
                expected["identityByAgent"] = serde_json::json!({});
                assert_eq!(main["codingAgentProfiles"], expected);
            }
            "sidecar-empty" => {
                assert_eq!(main["codingAgentProfiles"], sparse);
                assert_eq!(groups["codingAgentProfiles"], serde_json::json!({}));
            }
            _ => unreachable!(),
        }
        if case != "sidecar-empty" {
            assert!(!groups.contains_key("codingAgentProfiles"), "{case}");
        }
    }
}

#[test]
fn issue_3047_pair_effective_base_without_physical_base_and_rejected_overlay() {
    let (temp, path) = hash_activation_seed(true);
    let sidecar = b3_sidecar_path(temp.path());
    let mut groups = serde_json::Value::Object(b3_object(&sidecar));
    groups.as_object_mut().unwrap().remove("agents");
    groups["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
        "alpha":{"A":{"additionalArguments":"--model x","id_hash":"stale"}}
    });
    b3_write(&sidecar, &groups);
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({
            "agents":[b3_agent("alpha", "Effective only")]
        }),
    );
    let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    assert_eq!(
        written.coding_agent_profiles.profiles_by_agent["alpha"]["A"].id_hash,
        Some(super::super::configuration_identity("claude--modelx", &[]))
    );
    let raw = b3_object(&sidecar);
    assert!(!raw.contains_key("agents"));
    assert!(loaded
        .local_overlay_state
        .owns_top_level("codingAgentProfiles"));
    assert!(!groups.as_object().unwrap().contains_key("agents"));
    assert_eq!(raw["codingAgentProfiles"], groups["codingAgentProfiles"]);
    assert_eq!(
        raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["id_hash"],
        "stale"
    );

    // Rejected overlays still derive the final accepted physical view.
    let (temp, path) = hash_activation_seed(true);
    let sidecar = b3_sidecar_path(temp.path());
    let mut groups = serde_json::Value::Object(b3_object(&sidecar));
    groups["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
        "alpha":{"A":{"additionalArguments":"--model x","id_hash":"stale"}}
    });
    b3_write(&sidecar, &groups);
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({"defaultShellArgs":"bad"}),
    );
    let loaded = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    assert!(loaded.local_overlay_state.rejection().is_some());
    assert_eq!(
        loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].id_hash,
        Some(super::super::configuration_identity("claude--modelx", &[]))
    );
    let previous = std::fs::read(&path).unwrap();
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::super::SettingsSaveStage::AtomicReplace))
    });
    let result = super::super::save_settings_to_path_preserving_project_paths(&loaded, &path);
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    let saved = b3_object(&sidecar);
    assert_eq!(saved["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert!(saved["codingAgentProfiles"]
        .get("profilesByAgent")
        .is_none());
    assert_eq!(
        saved["codingAgentProfiles"]["profilesByBaseHash"]
            [&super::super::configuration_identity("claude", &[])]["A"],
        serde_json::json!({"additionalArguments":"--model x","enabled":true,"env":{},"notes":""})
    );
}
