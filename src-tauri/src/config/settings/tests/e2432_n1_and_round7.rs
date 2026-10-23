use super::*;

#[test]
fn e2432_n1_absent_main_startup_and_ordinary_first_save() {
    for startup in [false, true] {
        for kind in ["agents", "profiles"] {
            let temp = tempfile::tempdir().unwrap();
            let path = b3_settings_path(temp.path());
            let overlay_path = b3_overlay_path(temp.path());
            b3_write(&overlay_path, &n1_test_overlay(kind));
            let overlay_bytes = std::fs::read(&overlay_path).unwrap();
            let before = hash_activation_files(temp.path());
            let mut draft = super::super::load_settings_for_cli_from_path(&path);
            assert!(draft.coding_agent_identity_physical.is_none());
            assert_eq!(hash_activation_files(temp.path()), before);
            assert!(!path.exists());
            assert!(!super::super::agents_instance_path(&path).exists());
            let written = if startup {
                super::super::load_settings_from_path(&path)
            } else {
                draft.root_token = Some("ordinary-first-save".into());
                let written =
                    super::super::save_settings_to_path_preserving_project_paths(&draft, &path)
                        .unwrap();
                assert!(
                    draft.coding_agent_identity_physical.is_none(),
                    "comparison reference must not become input identity"
                );
                written
            };
            assert!(path.exists(), "first save {kind}/{startup}");
            assert!(written.root_token.is_some());
            assert_eq!(std::fs::read(&overlay_path).unwrap(), overlay_bytes);
            assert!(!super::super::agents_instance_path(&path).exists());
            let physical = b3_object(&path);
            assert!(!physical
                .get("agents")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .any(|row| row["id"] == "overlay-only"));
            let profiles = physical.get("codingAgentProfiles");
            assert!(profiles
                .and_then(|p| p.get("profilesByAgent"))
                .and_then(|p| p.get("overlay-only"))
                .is_none());
            assert!(profiles
                .and_then(|p| p.get("defaultProfileByAgent"))
                .and_then(|p| p.get("overlay-matrix"))
                .is_none());
            assert_ne!(
                profiles
                    .and_then(|p| p.pointer("/profileSlots/A/label"))
                    .and_then(serde_json::Value::as_str),
                Some("overlay-slot")
            );
            let reloaded = super::super::load_settings_for_cli_from_path(&path);
            assert_eq!(
                serde_json::to_value(&written.agents).unwrap(),
                serde_json::to_value(&reloaded.agents).unwrap()
            );
            assert_eq!(
                written.coding_agent_profiles,
                reloaded.coding_agent_profiles
            );
            if kind == "agents" {
                assert_eq!(written.agents[0].id, "overlay-only");
            } else {
                assert_eq!(
                    written.coding_agent_profiles.profile_slots["A"].label,
                    "overlay-slot"
                );
                assert_eq!(
                    written.coding_agent_profiles.profiles_by_agent["overlay-only"]["A"].command,
                    "--overlay"
                );
            }
        }
    }
}

#[test]
fn e2432_n1_absent_main_real_owned_edits_refuse_before_publishers() {
    for kind in ["agents", "profiles", "default"] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&b3_overlay_path(temp.path()), &n1_test_overlay(kind));
        let mut draft = super::super::load_settings_for_cli_from_path(&path);
        assert!(draft.coding_agent_identity_physical.is_none());
        match kind {
            "agents" => draft.agents[0].command = "changed-owned-command".into(),
            "profiles" => {
                draft
                    .coding_agent_profiles
                    .profiles_by_agent
                    .get_mut("overlay-only")
                    .unwrap()
                    .get_mut("A")
                    .unwrap()
                    .command = "changed-owned-profile".into()
            }
            _ => {
                draft
                    .coding_agent_profiles
                    .default_profile_by_agent
                    .insert("overlay-matrix".into(), "B".into());
            }
        }
        let before = hash_activation_files(temp.path());
        let error = super::super::save_settings_to_path_preserving_project_paths(&draft, &path)
            .unwrap_err();
        assert!(
            error.contains("overlayOwnedProfileGroup"),
            "{kind}: {error}"
        );
        assert_eq!(hash_activation_files(temp.path()), before);
        assert!(!path.exists());
        assert!(!super::super::project_paths_path(&path).exists());
    }
}

#[test]
fn e2432_n1_captured_sidecar_home_is_not_overlay_or_default_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let defaults = serde_json::to_value(super::super::AppSettings::default()).unwrap();
    let sidecar_path = super::super::agents_instance_path(&path);
    let mut sidecar = serde_json::json!({"agents":defaults["agents"],"codingAgentProfile\
        s":defaults["codingAgentProfiles"]});
    sidecar["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"overlay-only":"physical-home"});
    b3_write(&sidecar_path, &sidecar);
    let mut overlay = n1_test_overlay("agents");
    overlay["codingAgentProfiles"] =
        serde_json::json!({"codexHomeKeyByAgent":{"overlay-only":"forged-home"}});
    b3_write(&b3_overlay_path(temp.path()), &overlay);
    let raw_sidecar = std::fs::read(&sidecar_path).unwrap();
    let raw_overlay = std::fs::read(b3_overlay_path(temp.path())).unwrap();
    let mut draft = super::super::load_settings_for_cli_from_path(&path);
    assert!(!path.exists());
    assert!(draft.coding_agent_identity_physical.is_none());
    assert_eq!(
        draft.coding_agent_profiles.codex_home_key_by_agent["overlay-only"],
        "physical-home"
    );
    draft.root_token = Some("ordinary-first-save".into());
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&draft, &path).unwrap();
    assert!(draft.coding_agent_identity_physical.is_none());
    assert_eq!(
        written.coding_agent_profiles.codex_home_key_by_agent["overlay-only"],
        "physical-home"
    );
    assert_eq!(std::fs::read(&sidecar_path).unwrap(), raw_sidecar);
    assert_eq!(
        std::fs::read(b3_overlay_path(temp.path())).unwrap(),
        raw_overlay
    );
    assert_eq!(written.agents[0].id, "overlay-only");
    assert_eq!(
        written.coding_agent_profiles,
        super::super::load_settings_for_cli_from_path(&path).coding_agent_profiles
    );
}

#[test]
fn e2432_n1_stale_overlay_ownership_and_present_main_stay_refused() {
    for case in [
        "changed-value",
        "changed-ownership",
        "typed-invalid",
        "main-present",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        let overlay = b3_overlay_path(temp.path());
        b3_write(&overlay, &n1_test_overlay("agents"));
        let draft = super::super::load_settings_for_cli_from_path(&path);
        assert!(draft.coding_agent_identity_physical.is_none());
        match case {
            "changed-value" => b3_write(
                &overlay,
                &serde_json::json!({"agents":[{"id":"overlay-only","label":"Overlay","color":"re\
                    d","command":"changed"}]}),
            ),
            "changed-ownership" => b3_write(&overlay, &n1_test_overlay("profiles")),
            "typed-invalid" => b3_write(&overlay, &serde_json::json!({"agents":"not-an-array"})),
            _ => b3_write(
                &path,
                &serde_json::to_value(super::super::AppSettings::default()).unwrap(),
            ),
        }
        let before = hash_activation_files(temp.path());
        let error = super::super::save_settings_to_path_preserving_project_paths(&draft, &path)
            .unwrap_err();
        assert!(
            error.contains("overlayProfileComparisonUnavailable")
                || (case == "changed-value" && error.contains("overlayOwnedProfileGroup")),
            "{case}: {error}"
        );
        assert_eq!(hash_activation_files(temp.path()), before);
    }
}

#[test]
fn e2432_dl1_membership_and_n1_absence_revision_barriers() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let overlay = b3_overlay_path(temp.path());
    b3_write(&path, &serde_json::json!({}));
    std::fs::write(&overlay, b"{rejected").unwrap();
    let captured = super::super::collect_registered_profile_gate(&path);
    assert!(captured.admits_project_migration(&path));
    assert!(!captured
        .participant_documents()
        .iter()
        .any(|(p, _)| p == &overlay));
    std::fs::write(&overlay, b"{different rejected").unwrap();
    assert!(
        super::super::recheck_registered_profile_gate(&captured.participant_documents()).is_ok()
    );
    b3_write(&overlay, &n1_test_overlay("agents"));
    let participating = super::super::capture_registered_profile_gate(&path).unwrap();
    std::fs::remove_file(&overlay).unwrap();
    std::fs::create_dir(&overlay).unwrap();
    assert!(super::super::recheck_registered_profile_gate(&participating).is_err());
    std::fs::remove_dir(&overlay).unwrap();
    b3_write(&overlay, &n1_test_overlay("agents"));
    std::fs::remove_file(&path).unwrap();
    let absent = super::super::capture_registered_profile_gate(&path).unwrap();
    let draft = super::super::load_settings_for_cli_from_path(&path);
    assert!(
        super::super::absent_main_overlay_comparison(&draft, &path, &None, &None, &absent).is_ok()
    );
    b3_write(&path, &serde_json::json!({}));
    assert!(super::super::recheck_registered_profile_gate(&absent).is_err());
    let present = super::super::capture_registered_profile_gate(&path).unwrap();
    assert!(
        super::super::absent_main_overlay_comparison(&draft, &path, &None, &None, &present)
            .is_err()
    );
    std::fs::remove_file(&path).unwrap();
    b3_write(&overlay, &n1_test_overlay("profiles"));
    assert!(super::super::recheck_registered_profile_gate(&absent).is_err());
}

pub(super) fn assert_o2_stable_full_load(path: &std::path::Path) {
    let before = hash_activation_files(path.parent().unwrap());
    let (diagnostics, _capture) = capture_settings_save_diagnostics();
    // An attempted final main publication reports this existing fault even
    // when equal bytes would otherwise hide an unnecessary write.
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((
            path.to_path_buf(),
            super::super::SettingsSaveStage::TempCreate,
        ));
    });
    let loaded = super::super::load_settings_from_path(path);
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(loaded.project_paths_persistence_error.is_none());
    assert!(
        diagnostics.borrow().is_empty(),
        "unexpected automatic publication: {:?}",
        diagnostics.borrow()
    );
    assert_eq!(hash_activation_files(path.parent().unwrap()), before);
}

#[test]
fn e2432_round7_schema2_load_migrates_without_old_markers() {
    for in_sidecar in [false, true] {
        let (_temp, path) = hash_activation_seed(in_sidecar);
        let owner = if in_sidecar {
            super::super::agents_instance_path(&path)
        } else {
            path.clone()
        };
        let mut raw = serde_json::Value::Object(b3_object(&owner));
        raw["codingAgentProfiles"] = serde_json::json!({
            "schemaVersion":2,
            "profilesByAgent":{"alpha":{"A":{"command":"--model \
                x","notes":"retained","opaque":{"null":null}}}},
            "profileLabelsByAgent":{"alpha":{"A":"Own"}},
            "profileSlots":{"A":{"label":"Slot","opaque":[null,3]}},
            "defaultProfileByAgent":{"matrix":"A"},
            "codexHomeKeyByAgent":{"alpha":"home-leaf"},
            "identityByAgent":{"alpha":"not-BASE","identity-only":null}
        });
        b3_write(&owner, &raw);
        assert!(!super::super::legacy_profiles_shape_present(
            raw.as_object().unwrap()
        ));
        let before = super::super::load_settings_for_cli_from_path(&path);
        assert!(before.root_token.is_some());
        let loaded = super::super::load_settings_from_path(&path);
        let stored = b3_object(&super::super::agents_instance_path(&path));
        let profiles = &stored["codingAgentProfiles"];
        let hash = super::super::configuration_identity("claude", &[]);
        assert_eq!(profiles["profileBindingVersion"], 1);
        assert_eq!(
            profiles["profilesByBaseHash"][&hash],
            super::super::normalized_profile_cells(
                raw.pointer("/codingAgentProfiles/profilesByAgent/alpha")
            )
            .unwrap()
        );
        for key in [
            "profileLabelsByBaseHash",
            "profileSlots",
            "defaultProfileByAgent",
            "codexHomeKeyByAgent",
        ] {
            let expected = if key == "profileLabelsByBaseHash" {
                serde_json::json!({(hash.clone()):{"A":"Own"}})
            } else {
                raw["codingAgentProfiles"][key].clone()
            };
            assert_eq!(profiles[key], expected, "{key}");
        }
        for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
            assert!(profiles.get(key).is_none());
        }
        assert_eq!(
            profiles["legacyProfileOrphans"]["identity-only"],
            serde_json::json!([{"identity":null}])
        );
        assert_eq!(
            serde_json::to_value(&loaded.agents).unwrap(),
            serde_json::to_value(&before.agents).unwrap()
        );
        assert_eq!(loaded.root_token, before.root_token);
        assert_eq!(
            loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
            "retained"
        );
        if !in_sidecar {
            let main = b3_object(&path);
            assert!(!main.contains_key("agents"));
            assert!(!main.contains_key("codingAgentProfiles"));
        }
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_authoritative_split_routes_only_selected_raw_groups() {
    for sidecar_rows in [false, true] {
        let (_temp, path) = hash_activation_seed(true);
        let sidecar_path = super::super::agents_instance_path(&path);
        let mut main = b3_object(&path);
        let mut sidecar = b3_object(&sidecar_path);
        let mut rows = sidecar.remove("agents").unwrap();
        rows[0]["baseCommand"] = serde_json::json!("codex");
        rows[0].as_object_mut().unwrap().remove("command");
        rows[0]["opaqueRow"] = serde_json::json!({"keep":[null,7]});
        let mut profiles = sidecar.remove("codingAgentProfiles").unwrap();
        profiles["profilesByAgent"] =
            serde_json::json!({"alpha":{"A":{"command":"--selected","opaqueCell":[null]}}});
        profiles["profilesByAgent"]["beta"] =
            serde_json::json!({"A":{"notes":"distinct shadowed claude set","opaque":null}});
        profiles["profileLabelsByAgent"] = serde_json::json!({"alpha":{"A":"Selected"}});
        profiles["codexHomeKeyByAgent"] = serde_json::json!({"alpha":"selected-home"});
        if sidecar_rows {
            sidecar.insert("agents".into(), rows.clone());
            main.insert(
                "agents".into(),
                serde_json::json!([
                    b3_agent("alpha", "Shadowed claude"),
                    b3_agent("beta", "Conflicting shadowed claude")
                ]),
            );
            main.insert("codingAgentProfiles".into(), profiles.clone());
        } else {
            main.insert("agents".into(), rows.clone());
            main.insert(
                "codingAgentProfiles".into(),
                serde_json::json!({"profilesByAgent":{"alpha":{"A":{"notes":"shadowed"}}}}),
            );
            sidecar.insert("codingAgentProfiles".into(), profiles.clone());
        }
        b3_write(&path, &serde_json::Value::Object(main.clone()));
        b3_write(&sidecar_path, &serde_json::Value::Object(sidecar));
        let loaded = super::super::load_settings_from_path(&path);
        assert!(loaded.project_paths_persistence_error.is_none());
        let saved = b3_object(&sidecar_path);
        let hash = super::super::configuration_identity("codex", &[]);
        assert_eq!(
            saved["codingAgentProfiles"]["profilesByBaseHash"][&hash],
            super::super::normalized_profile_cells(profiles.pointer("/profilesByAgent/alpha"))
                .unwrap()
        );
        assert_eq!(
            saved["codingAgentProfiles"]["profilesByBaseHash"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert!(saved["codingAgentProfiles"]["profilesByBaseHash"]
            .get(super::super::configuration_identity("claude", &[]))
            .is_none());
        assert_eq!(
            saved["codingAgentProfiles"]["codexHomeKeyByAgent"],
            profiles["codexHomeKeyByAgent"]
        );
        assert_eq!(
            saved["codingAgentProfiles"]["legacyProfileOrphans"]["beta"],
            serde_json::json!([{"cells":profiles["profilesByAgent"]["beta"].clone()}])
        );
        assert_eq!(saved["agents"][0]["opaqueRow"], rows[0]["opaqueRow"]);
        assert_eq!(saved["agents"][0].get("command"), rows[0].get("command"));
        assert_eq!(saved["agents"][0]["baseCommand"], rows[0]["baseCommand"]);
        let expected: Vec<super::super::AgentConfig> = serde_json::from_value(rows).unwrap();
        let mut expected = expected;
        super::super::finalize_agent_order(&mut expected);
        super::super::refresh_agent_id_hashes(&mut expected);
        assert_eq!(
            serde_json::to_value(&loaded.agents).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let saved_main = b3_object(&path);
        let transferred = if sidecar_rows {
            "codingAgentProfiles"
        } else {
            "agents"
        };
        let unselected = if sidecar_rows {
            "agents"
        } else {
            "codingAgentProfiles"
        };
        assert!(!saved_main.contains_key(transferred));
        assert_eq!(saved_main.get(unselected), main.get(unselected));
        assert_eq!(
            loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].command,
            "--selected"
        );
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_empty_presence_blocks_main_donors() {
    for empty_rows in [false, true] {
        let (_temp, path) = hash_activation_seed(false);
        let sidecar = super::super::agents_instance_path(&path);
        let mut main = serde_json::Value::Object(b3_object(&path));
        main["codingAgentProfiles"]["profilesByAgent"] =
            serde_json::json!({"alpha":{"B":{"notes":"main"}}});
        b3_write(&path, &main);
        b3_write(
            &sidecar,
            &if empty_rows {
                serde_json::json!({"agents":[]})
            } else {
                serde_json::json!({"codingAgentProfiles":{}})
            },
        );
        let loaded = super::super::load_settings_from_path(&path);
        let saved = b3_object(&sidecar);
        let profiles = &saved["codingAgentProfiles"];
        if empty_rows {
            assert_eq!(saved["agents"], serde_json::json!([]));
            assert!(loaded.agents.is_empty());
            assert_eq!(profiles["profilesByBaseHash"], serde_json::json!({}));
            assert_eq!(
                profiles["legacyProfileOrphans"]["alpha"],
                serde_json::json!([{"cells":{"B":{"notes":"main"}}}])
            );
        } else {
            let hash = super::super::configuration_identity("claude", &[]);
            assert!(profiles["profilesByBaseHash"][&hash].get("B").is_none());
            assert_eq!(
                b3_object(&path)["codingAgentProfiles"],
                main["codingAgentProfiles"]
            );
        }
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_complete_collision_matrix_leaves_every_id_and_publisher_untouched() {
    for difference in [
        "notes",
        "env",
        "enabled",
        "letter",
        "unknown",
        "label",
        "destination",
    ] {
        let (_temp, path) = hash_activation_seed(true);
        let sidecar = super::super::agents_instance_path(&path);
        let mut source = serde_json::Value::Object(b3_object(&sidecar));
        let mut first = b3_agent("a-compatible", "Earlier compatible row");
        first["command"] = serde_json::json!("safe-tool");
        first["blockingMenus"] =
            serde_json::json!([{"pattern":"^Ready?","notification":"choose","enabled":true}]);
        source["agents"].as_array_mut().unwrap().insert(0, first);
        source["agents"]
            .as_array_mut()
            .unwrap()
            .push(b3_agent("beta", "Second alias"));
        let cells = serde_json::json!({"A":{"notes":"same","opaque":{"keep":null}}});
        source["codingAgentProfiles"]["profilesByAgent"] =
            serde_json::json!({"alpha":cells.clone(),"beta":cells.clone()});
        source["codingAgentProfiles"]["profileLabelsByAgent"] =
            serde_json::json!({"alpha":{"A":"same"},"beta":{"A":"same"}});
        match difference {
            "label" => {
                source["codingAgentProfiles"]["profileLabelsByAgent"]["beta"]["A"] =
                    serde_json::json!("different")
            }
            "letter" => {
                source["codingAgentProfiles"]["profilesByAgent"]["beta"]["B"] =
                    serde_json::json!({})
            }
            "destination" => {
                let hash = super::super::configuration_identity("claude", &[]);
                source["codingAgentProfiles"]["profilesByBaseHash"] =
                    serde_json::json!({(hash):{"A":{"notes":"prospective destination"}}});
            }
            field => {
                let value = match field {
                    "enabled" => serde_json::json!(false),
                    "env" => serde_json::json!({"KEY":"different"}),
                    "unknown" => serde_json::json!([null, 7]),
                    _ => serde_json::json!("different"),
                };
                source["codingAgentProfiles"]["profilesByAgent"]["beta"]["A"][field] = value;
            }
        }
        b3_write(&sidecar, &source);
        let mut main = serde_json::Value::Object(b3_object(&path));
        main["projectPaths"] = serde_json::json!([]);
        b3_write(&path, &main);
        std::fs::remove_file(super::super::project_paths_path(&path)).unwrap();
        let before = hash_activation_files(path.parent().unwrap());
        let menus = std::cell::Cell::new(0);
        let publish = |_: &std::path::Path, _: &std::path::Path| {
            menus.set(menus.get() + 1);
            Err("unexpected menu publisher".to_owned())
        };
        let loaded = super::super::load_settings_from_path_with_blocking_menus_publish(
            &path,
            Some(&publish),
        );
        assert!(
            loaded.project_paths_persistence_error.is_none(),
            "{difference}"
        );
        assert!(super::super::registered_profile_raw_gate(&path)
            .unwrap_err()
            .contains("sourceProfileConflict"));
        assert!(
            super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).is_err()
        );
        assert_eq!(menus.get(), 0);
        assert_eq!(
            hash_activation_files(path.parent().unwrap()),
            before,
            "{difference}"
        );
        let stored = b3_object(&sidecar);
        assert_eq!(
            stored["codingAgentProfiles"]["profilesByAgent"],
            source["codingAgentProfiles"]["profilesByAgent"]
        );
        // No denied flag is cached: correction permits the next load alone.
        source["codingAgentProfiles"]["profilesByAgent"]["beta"] = cells;
        source["codingAgentProfiles"]["profileLabelsByAgent"]["beta"]["A"] =
            serde_json::json!("same");
        source["codingAgentProfiles"]
            .as_object_mut()
            .unwrap()
            .remove("profilesByBaseHash");
        b3_write(&sidecar, &source);
        let retried = super::super::load_settings_from_path(&path);
        assert_eq!(
            retried.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
            "same"
        );
        assert!(b3_object(&sidecar)["codingAgentProfiles"]
            .get("profilesByAgent")
            .is_none());
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_sidecar_failure_keeps_complete_legacy_packet_for_retry() {
    for stage in [
        super::super::SettingsSaveStage::TempCreate,
        super::super::SettingsSaveStage::AtomicReplace,
    ] {
        let (_temp, path) = hash_activation_seed(false);
        let sidecar = super::super::agents_instance_path(&path);
        let before = hash_activation_files(path.parent().unwrap());
        let before_revision =
            super::super::source_bytes_revision(&std::fs::read(&sidecar).unwrap());
        super::super::PROJECT_MIGRATION_ATOMIC_FAULT
            .with(|slot| *slot.borrow_mut() = Some((sidecar.clone(), stage)));
        let (records, capture) = capture_settings_save_diagnostics();
        let loaded = super::super::load_settings_from_path(&path);
        super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
        assert!(!records.borrow().is_empty());
        drop(capture);
        assert!(loaded.project_paths_persistence_error.is_none());
        assert_eq!(hash_activation_files(path.parent().unwrap()), before);
        assert_eq!(
            super::super::source_bytes_revision(&std::fs::read(&sidecar).unwrap()),
            before_revision
        );
        assert!(b3_object(&path)["codingAgentProfiles"]
            .get("profilesByAgent")
            .is_some());
        let retried = super::super::load_settings_from_path(&path);
        assert!(retried.project_paths_persistence_error.is_none());
        assert_eq!(
            b3_object(&sidecar)["codingAgentProfiles"]["profileBindingVersion"],
            1
        );
        assert_ne!(
            super::super::source_bytes_revision(&std::fs::read(&sidecar).unwrap()),
            before_revision
        );
        assert!(!b3_object(&path).contains_key("codingAgentProfiles"));
        assert_o2_stable_full_load(&path);
    }
}
