use super::super::*;
use serde_json::json;

fn registered() -> Value {
    json!({
        "agents":[
            {"id":"one","label":"One","color":"#123456","baseCommand":"tool"},
            {"id":"two","label":"Two","color":"#123456","baseCommand":"loot"}
        ],
        "codingAgentProfiles":{
            "schemaVersion":2,
            "profilesByAgent":{
                "one":{"B":{"notes":"shared","extra":{"keep":null}}},
                "two":{"B":{"notes":"shared","extra":{"keep":null}}}
            },
            "profileLabelsByAgent":{"one":{"B":"label"},"two":{"B":"label"}},
            "profileSlots":{"B":{"label":"slot"}},
            "defaultProfileByAgent":{"matrix":"B"},
            "codexHomeKeyByAgent":{"one":"leaf"}
        },
        "unrelated":{"keep":[1,null]}
    })
}

fn write_source(path: &Path, source: &Value) {
    let mut settings = serde_json::to_value(AppSettings::default()).unwrap();
    settings["agents"] = json!([]);
    settings["rootToken"] = json!("retained-token");
    std::fs::write(path, serde_json::to_vec(&settings).unwrap()).unwrap();
    std::fs::write(
        agents_instance_path(path),
        serde_json::to_vec(source).unwrap(),
    )
    .unwrap();
}

fn canonical(source: &Value) -> Value {
    let view = project_physical_profiles(source, true).unwrap();
    canonical_profile_source(source, &view, true).unwrap()
}

#[test]
fn e2432_canonical_serializer_registered_alias_edit_and_raw_output() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let source = registered();
    write_source(&path, &source);
    let original = std::fs::read(agents_instance_path(&path)).unwrap();
    let mut settings = load_settings_for_cli_from_path(&path);
    assert_eq!(
        std::fs::read(agents_instance_path(&path)).unwrap(),
        original
    );
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("one")
        .unwrap()
        .get_mut("B")
        .unwrap()
        .notes = "edited".into();
    let saved = save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(
        saved.coding_agent_profiles.profiles_by_agent["two"]["B"].notes,
        "edited"
    );
    let raw: Value =
        serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
    let profiles = &raw["codingAgentProfiles"];
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(profiles.get(key).is_none());
    }
    let hash = configuration_identity("tool", &[]);
    assert_eq!(profiles["profileBindingVersion"], 1);
    assert_eq!(profiles["profilesByBaseHash"].as_object().unwrap().len(), 1);
    assert_eq!(
        profiles["profilesByBaseHash"][&hash]["B"]["extra"],
        json!({"keep":null})
    );
    assert_eq!(profiles["defaultProfileByAgent"], json!({"matrix":"B"}));
    assert_eq!(profiles["codexHomeKeyByAgent"], json!({"one":"leaf"}));
    assert_eq!(raw["unrelated"], source["unrelated"]);
    let mut stale = raw.clone();
    stale["codingAgentProfiles"]["profilesByAgent"] = json!({"one":{"B":{"notes":"stale"}}});
    assert_eq!(
        project_physical_profiles(&stale, true).unwrap()["codingAgentProfiles"]["profilesByAgent"]
            ["one"]["B"]["notes"],
        "edited"
    );
}

#[test]
fn e2432_alias_conflict_base_fork_deletion_recreation_and_source_isolation() {
    let source = canonical(&registered());
    let mut view = project_physical_profiles(&source, true).unwrap();
    view["codingAgentProfiles"]["profilesByAgent"]["one"]["B"]["notes"] = json!("first");
    view["codingAgentProfiles"]["profilesByAgent"]["two"]["B"]["notes"] = json!("second");
    assert!(canonical_profile_source(&source, &view, true)
        .unwrap_err()
        .contains("sourceProfileConflict"));
    let mut view = project_physical_profiles(&source, true).unwrap();
    view["agents"][0]["baseCommand"] = json!("different");
    let fork = canonical_profile_source(&source, &view, true).unwrap();
    assert_eq!(
        fork["codingAgentProfiles"]["profilesByBaseHash"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    let mut removed = project_physical_profiles(&fork, true).unwrap();
    removed["agents"] = json!([]);
    let orphaned = canonical_profile_source(&fork, &removed, true).unwrap();
    assert_eq!(
        orphaned["codingAgentProfiles"]["profilesByBaseHash"],
        fork["codingAgentProfiles"]["profilesByBaseHash"]
    );
    let recreated = canonical_profile_source(
        &orphaned,
        &project_physical_profiles(&fork, true).unwrap(),
        true,
    )
    .unwrap();
    assert_eq!(
        recreated["codingAgentProfiles"]["profilesByBaseHash"],
        fork["codingAgentProfiles"]["profilesByBaseHash"]
    );
    let mut other = registered();
    other["codingAgentProfiles"]["profilesByAgent"]["one"]["B"]["notes"] = json!("other");
    other["codingAgentProfiles"]["profilesByAgent"]["two"]["B"]["notes"] = json!("other");
    assert_ne!(
        canonical(&other)["codingAgentProfiles"]["profilesByBaseHash"],
        source["codingAgentProfiles"]["profilesByBaseHash"]
    );
}

#[test]
fn e2432_orphan_wire_registered() {
    let mut source = registered();
    let profiles = source["codingAgentProfiles"].as_object_mut().unwrap();
    profiles["profilesByAgent"]["absent"] = Value::Null;
    profiles["profileLabelsByAgent"]["labels-only"] = json!([]);
    profiles.insert(
        "identityByAgent".into(),
        json!({"identity-only":null,"absent":"short-not-BASE"}),
    );
    let saved = canonical(&source);
    let orphans = &saved["codingAgentProfiles"]["legacyProfileOrphans"];
    assert_eq!(
        orphans["absent"],
        json!([{"cells":null,"identity":"short-not-BASE"}])
    );
    assert_eq!(orphans["labels-only"], json!([{"labels":[]}]));
    assert_eq!(orphans["identity-only"], json!([{"identity":null}]));
    assert!(saved["codingAgentProfiles"]
        .get("identityByAgent")
        .is_none());
    assert!(
        project_physical_profiles(&saved, true).unwrap()["codingAgentProfiles"]["profilesByAgent"]
            .get("absent")
            .is_none()
    );
}

#[test]
fn e2432_orphan_collision_idempotence() {
    let mut source = canonical(&registered());
    source["codingAgentProfiles"]["legacyProfileOrphans"] = json!({"X":[
        {"cells":{"B":{"notes":"old","extension":null}},"unknown":[1]},
        {"cells":{"B":{"notes":"old","extension":null}},"unknown":[1]}
    ]});
    source["codingAgentProfiles"]["profilesByAgent"] = json!({"X":{"B":{"notes":"new"}}});
    let once = canonical(&source);
    let records = &once["codingAgentProfiles"]["legacyProfileOrphans"]["X"];
    assert_eq!(records.as_array().unwrap().len(), 3);
    assert_eq!(records[0], records[1]);
    assert_eq!(records[2], json!({"cells":{"B":{"notes":"new"}}}));
    let twice = canonical(&once);
    assert_eq!(twice, once);
    let mut repeated = twice.clone();
    repeated["codingAgentProfiles"]["profilesByAgent"] =
        source["codingAgentProfiles"]["profilesByAgent"].clone();
    assert_eq!(canonical(&repeated), twice);
    let mut recreated = project_physical_profiles(&twice, true).unwrap();
    recreated["agents"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"X","label":"X","color":"red","baseCommand":"new-command"}));
    let recreated = canonical_profile_source(&twice, &recreated, true).unwrap();
    let view = project_physical_profiles(&recreated, true).unwrap();
    assert!(view["codingAgentProfiles"]["profilesByAgent"]["X"]
        .get("B")
        .is_none());
    assert_eq!(
        recreated["codingAgentProfiles"]["legacyProfileOrphans"],
        twice["codingAgentProfiles"]["legacyProfileOrphans"]
    );
}

fn assert_no_publication(path: &Path, settings_bytes: &[u8], sidecar_bytes: &[u8]) {
    assert_eq!(std::fs::read(path).unwrap(), settings_bytes);
    assert_eq!(
        std::fs::read(agents_instance_path(path)).unwrap(),
        sidecar_bytes
    );
    for entry in std::fs::read_dir(path.parent().unwrap()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            name == SETTINGS_FILE_NAME
                || name == AGENTS_INSTANCE_FILE_NAME
                || name.ends_with(".lock"),
            "unexpected publisher output: {name}"
        );
    }
}

#[test]
fn e2432_orphan_malformed_preflight() {
    for invalid in [
        Value::Null,
        json!([]),
        json!({"X":[]}),
        json!({"X":{}}),
        json!({"X":[null]}),
        json!({"X":[{}]}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(SETTINGS_FILE_NAME);
        let mut source = registered();
        source["codingAgentProfiles"]["legacyProfileOrphans"] = invalid;
        write_source(&path, &source);
        let settings_bytes = std::fs::read(&path).unwrap();
        let sidecar_bytes = std::fs::read(agents_instance_path(&path)).unwrap();
        let error = registered_profile_raw_gate(&path).unwrap_err();
        assert!(error.contains("invalidLegacyProfileOrphans"));
        let loaded = load_settings_from_path(&path);
        assert_no_publication(&path, &settings_bytes, &sidecar_bytes);
        assert!(save_settings_to_path_preserving_project_paths(&loaded, &path).is_err());
        {
            let _lock = SettingsFileLock::acquire_contextual(
                &path,
                std::time::Duration::from_secs(2),
                "e2432_orphan_preference",
            )
            .unwrap();
            assert!(save_settings_value_locked(
                &loaded,
                &path,
                ProjectWriteMode::Preserve,
                TerminalSnapshotGateWriteMode::Explicit(false),
                MainWindowPlacementWriteMode::Preserve
            )
            .is_err());
        }
        let context = SourceReadContext::from_settings(&AppSettings::default(), &path).unwrap();
        let snapshot =
            read_source_snapshot(&context, &context.source(SourceKind::RegisteredInstance))
                .unwrap();
        assert_eq!(snapshot.snapshot.availability, SourceAvailability::Invalid);
        assert!(snapshot.snapshot.agents.is_empty());
        assert!(!snapshot.snapshot.capabilities.editable_save);
        assert_no_publication(&path, &settings_bytes, &sidecar_bytes);
    }
    for valid in [
        json!({}),
        json!({"X":[{"cells":null,"unknown":{"nested":[]}}]}),
    ] {
        assert!(validate_profile_orphans(Some(&valid)).is_ok());
    }
}

#[test]
fn e2432_duplicate_registered_compatibility() {
    for duplicate in [
        r#""x":1,"x":1"#,
        r#""x":1,"\u0078":2"#,
        r#""unknown":{"nested":1,"nested":2}"#,
        r#""codingAgentProfiles":{},"codingAgentProfiles":{}"#,
        r#""codingAgentProfiles":{"profilesByAgent":{},"profilesByAgent":{}}"#,
        r#""codingAgentProfiles":{"profilesByAgent":{"one":{},"one":{}}}"#,
        r#""codingAgentProfiles":{"profilesByAgent":{"one":{"B":{"notes":"a","notes":"b"}}}}"#,
        r#""codingAgentProfiles":{"legacyProfileOrphans":{"X":[{"cells":null,"cells":null}]}}"#,
    ] {
        for sidecar in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join(SETTINGS_FILE_NAME);
            write_source(&path, &registered());
            let target = if sidecar {
                agents_instance_path(&path)
            } else {
                path.clone()
            };
            let mut original: Value =
                serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
            if duplicate.starts_with("\"codingAgentProfiles\"") {
                original
                    .as_object_mut()
                    .unwrap()
                    .remove("codingAgentProfiles");
            }
            let mut raw = serde_json::to_string(&original).unwrap();
            raw.pop();
            raw.push(',');
            raw.push_str(duplicate);
            if !sidecar {
                raw.push_str(r#", "projectPaths":[],"projectPath":null"#);
            }
            raw.push('}');
            std::fs::write(&target, &raw).unwrap();
            let settings_bytes = std::fs::read(&path).unwrap();
            let sidecar_bytes = std::fs::read(agents_instance_path(&path)).unwrap();
            let diagnostic = duplicate_profile_document(raw.as_bytes(), &target).unwrap();
            assert!(diagnostic.contains("duplicateJsonMember"));
            assert!(diagnostic.contains("line"));
            assert!(diagnostic.contains("column"));
            assert!(diagnostic.contains(&target.to_string_lossy().to_string()));
            assert!(read_agents_instance_file(&agents_instance_path(&path)).is_ok());
            let cli = load_settings_for_cli_from_path(&path);
            assert_eq!(cli.agents[0].id, "one");
            let loaded = load_settings_from_path(&path);
            assert_eq!(loaded.agents[0].id, "one");
            assert!(loaded.root_token.is_some());
            assert!(loaded.project_paths_persistence_error.is_none());
            assert_no_publication(&path, &settings_bytes, &sidecar_bytes);
            assert!(save_settings_to_path_preserving_project_paths(&loaded, &path).is_err());
            {
                let _lock = SettingsFileLock::acquire_contextual(
                    &path,
                    std::time::Duration::from_secs(2),
                    "e2432_preference",
                )
                .unwrap();
                assert!(save_settings_value_locked(
                    &loaded,
                    &path,
                    ProjectWriteMode::Preserve,
                    TerminalSnapshotGateWriteMode::Explicit(false),
                    MainWindowPlacementWriteMode::Preserve
                )
                .is_err());
            }
            let context = SourceReadContext::from_settings(&AppSettings::default(), &path).unwrap();
            let snapshot =
                read_source_snapshot(&context, &context.source(SourceKind::RegisteredInstance))
                    .unwrap();
            assert_eq!(snapshot.snapshot.availability, SourceAvailability::Invalid);
            assert!(snapshot.snapshot.agents.is_empty());
            assert_no_publication(&path, &settings_bytes, &sidecar_bytes);
            write_source(&path, &registered());
            let fresh = load_settings_for_cli_from_path(&path);
            assert!(save_settings_to_path_preserving_project_paths(&fresh, &path).is_ok());
        }
    }
}

#[test]
fn e2432_duplicate_overlay_participation_and_isolation() {
    for participating in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(SETTINGS_FILE_NAME);
        write_source(&path, &registered());
        let overlay = path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
        let raw = if participating {
            br#"{"codingAgentProfiles":{"profileSlots":{"A":{"label":"a","label":"b"}}}}"#
                .as_slice()
        } else {
            br#"{"unrelated":{"x":1,"x":2}}"#.as_slice()
        };
        std::fs::write(&overlay, raw).unwrap();
        assert_eq!(registered_profile_raw_gate(&path).is_err(), participating);
        let settings = load_settings_for_cli_from_path(&path);
        if participating {
            let before = std::fs::read(&path).unwrap();
            let source_before = std::fs::read(agents_instance_path(&path)).unwrap();
            assert!(save_settings_to_path_preserving_project_paths(&settings, &path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(
                std::fs::read(agents_instance_path(&path)).unwrap(),
                source_before
            );
        } else {
            assert!(save_settings_to_path_preserving_project_paths(&settings, &path).is_ok());
        }
        assert_eq!(std::fs::read(&overlay).unwrap(), raw);
    }
}

#[test]
fn e2432_legacy_conflict_rejects_before_every_publisher() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let mut source = registered();
    source["codingAgentProfiles"]["profilesByAgent"]["two"]["B"]["notes"] = json!("different");
    write_source(&path, &source);
    let before = std::fs::read(&path).unwrap();
    let source_before = std::fs::read(agents_instance_path(&path)).unwrap();
    let loaded = load_settings_from_path(&path);
    assert!(registered_profile_raw_gate(&path)
        .unwrap_err()
        .contains("sourceProfileConflict"));
    assert!(save_settings_to_path_preserving_project_paths(&loaded, &path).is_err());
    assert_no_publication(&path, &before, &source_before);
}
#[test]
fn e2432_main_legacy_owner_preserves_unknown_and_orphan_values() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let mut main = serde_json::to_value(AppSettings::default()).unwrap();
    let mut source = registered();
    source["codingAgentProfiles"]["profilesByAgent"]["missing"] = json!({"raw":null});
    main["agents"] = source["agents"].clone();
    main["codingAgentProfiles"] = source["codingAgentProfiles"].clone();
    main["unrecognized"] = json!({"nested":[null,7]});
    std::fs::write(&path, serde_json::to_vec(&main).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(!agents_instance_path(&path).exists());
    let loaded = load_settings_for_cli_from_path(&path);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!agents_instance_path(&path).exists());
    let written = save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    let saved_main: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved_main["unrecognized"], main["unrecognized"]);
    assert!(saved_main.get("agents").is_none());
    assert!(saved_main.get("codingAgentProfiles").is_none());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
    assert_eq!(saved["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert_eq!(
        saved["codingAgentProfiles"]["legacyProfileOrphans"]["missing"],
        json!([{"cells":{"raw":null}}])
    );
    assert!(saved["codingAgentProfiles"]
        .get("profilesByAgent")
        .is_none());
    // Equal BASE shares every payload field; each alias still reports the
    // exact original row strings in its recomputed diagnostic metadata.
    let payload = |id: &str| {
        let agent = written.agents.iter().find(|agent| agent.id == id).unwrap();
        let mut cells = written.coding_agent_profiles.profiles_by_agent[id].clone();
        for cell in cells.values_mut() {
            assert_eq!(
                cell.id_hash,
                Some(pair_identity(&agent.command, &agent.envs, cell))
            );
            assert_eq!(
                cell.id_hash_debug,
                Some(pair_identity_debug(&agent.command, &agent.envs, cell))
            );
            cell.id_hash_debug = None;
        }
        cells
    };
    assert_eq!(payload("one"), payload("two"));
}

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

#[test]
fn e2432_invalid_rows_keep_preferences_and_startup_token_persistence() {
    for kind in [
        "duplicate-equal",
        "duplicate-different",
        "empty-command",
        "invalid-command",
    ] {
        invalid_row_persistence_case(kind);
    }
    for protected_main in [true, false] {
        for duplicate in [true, false] {
            invalid_row_without_profiles_case(protected_main, duplicate);
        }
    }
}

fn invalid_row_persistence_case(kind: &str) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let mut raw = registered();
    if kind.starts_with("duplicate") {
        let mut duplicate = raw["agents"][0].clone();
        if kind == "duplicate-different" {
            duplicate["baseCommand"] = json!("different-tool");
        }
        raw["agents"].as_array_mut().unwrap().push(duplicate);
    } else {
        raw["agents"][0]["baseCommand"] = if kind == "empty-command" {
            json!("")
        } else {
            json!("claude --continue")
        };
    }
    if kind == "invalid-command" {
        let command = raw["agents"][0]["baseCommand"].as_str().unwrap();
        assert!(validate_agent_command_text("fixture negative control", command).is_err());
        // Newlines are token separators in the existing command parser,
        // so they cannot stand in for a rejected command.
        assert!(validate_agent_command_text("fixture positive control", "tool\ninvalid").is_ok());
    }
    write_source(&path, &raw);
    let mut main: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    main.as_object_mut().unwrap().remove("rootToken");
    std::fs::write(&path, serde_json::to_vec(&main).unwrap()).unwrap();
    let sidecar_before = std::fs::read(agents_instance_path(&path)).unwrap();
    let mut loaded = load_settings_for_cli_from_path(&path);
    assert_eq!(loaded.agents.len(), raw["agents"].as_array().unwrap().len());
    assert!(loaded.root_token.is_none());
    assert_eq!(
        std::fs::read(agents_instance_path(&path)).unwrap(),
        sidecar_before
    );
    let context = SourceReadContext::from_settings(&loaded, &path).unwrap();
    let selected = context.source(SourceKind::RegisteredInstance);
    let snapshot = read_source_snapshot(&context, &selected).unwrap();
    assert_eq!(
        snapshot.snapshot.availability,
        SourceAvailability::Invalid,
        "{kind}"
    );
    assert!(!snapshot.snapshot.capabilities.editable_save);
    assert!(!snapshot.snapshot.capabilities.available_read);
    assert!(snapshot.snapshot.agents.is_empty());
    assert!(canonical_profile_source(&raw, &raw, true).is_err());
    loaded.log_level = Some("debug".into());
    let written = save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    assert_eq!(written.log_level.as_deref(), Some("debug"));
    let preference = compare_and_set_terminal_snapshots_enabled_at_path(
        &written,
        &path,
        written.terminal_snapshots_enabled,
        !written.terminal_snapshots_enabled,
    )
    .unwrap();
    assert_ne!(
        preference.terminal_snapshots_enabled,
        written.terminal_snapshots_enabled
    );
    let mut edited = preference.clone();
    edited
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("one")
        .unwrap()
        .get_mut("B")
        .unwrap()
        .notes = "ambiguous edit".into();
    let before = std::fs::read(agents_instance_path(&path)).unwrap();
    assert!(save_settings_to_path_preserving_project_paths(&edited, &path).is_err());
    assert_eq!(std::fs::read(agents_instance_path(&path)).unwrap(), before);
    let startup = load_settings_from_path(&path);
    assert!(startup.project_paths_persistence_error.is_none());
    let token = startup.root_token.as_ref().unwrap();
    let persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["rootToken"], token.as_str());
    assert_eq!(
        load_settings_from_path(&path).root_token,
        startup.root_token
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
    assert_eq!(saved["codingAgentProfiles"], raw["codingAgentProfiles"]);
    assert!(saved["codingAgentProfiles"]
        .get("profileBindingVersion")
        .is_none());
}

fn invalid_row_without_profiles_case(protected_main: bool, duplicate: bool) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let mut source = registered();
    source
        .as_object_mut()
        .unwrap()
        .remove("codingAgentProfiles");
    if duplicate {
        let row = source["agents"][0].clone();
        source["agents"].as_array_mut().unwrap().push(row);
    } else {
        source["agents"][0]["baseCommand"] = json!("");
    }
    assert!(physical_profile_rows(&source, true).is_err());
    let mut main = serde_json::to_value(AppSettings::default()).unwrap();
    for key in ["agents", "codingAgentProfiles", "rootToken"] {
        main.as_object_mut().unwrap().remove(key);
    }
    if protected_main {
        main["agents"] = source["agents"].clone();
        std::fs::write(
            temp.path().join(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            serde_json::to_vec(&json!({"agents":[{
                "id":"overlay", "label":"Overlay", "color":"#123456",
                "baseCommand":"overlay-tool"
            }]}))
            .unwrap(),
        )
        .unwrap();
    } else {
        std::fs::write(
            agents_instance_path(&path),
            serde_json::to_vec(&source).unwrap(),
        )
        .unwrap();
    }
    std::fs::write(&path, serde_json::to_vec(&main).unwrap()).unwrap();
    let baseline = load_settings_for_cli_from_path(&path);
    assert!(baseline.root_token.is_none());
    assert_eq!(
        startup_profile_legacy_work(
            &baseline,
            &path,
            Some(&std::fs::read_to_string(&path).unwrap()),
            false,
        )
        .unwrap(),
        Some(false),
    );
    let loaded = load_settings_from_path(&path);
    assert!(loaded.project_paths_persistence_error.is_none());
    let token = loaded.root_token.clone().unwrap();
    let persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["rootToken"], token.as_str());
    if protected_main {
        assert_eq!(persisted["agents"], source["agents"]);
        assert_eq!(loaded.agents[0].command, "overlay-tool");
    } else {
        assert_eq!(
            loaded.agents.len(),
            source["agents"].as_array().unwrap().len()
        );
    }
    let mut preference = loaded;
    preference.log_level = Some("debug".into());
    let saved = save_settings_to_path_preserving_project_paths(&preference, &path).unwrap();
    assert_eq!(saved.log_level.as_deref(), Some("debug"));
    let reloaded = load_settings_for_cli_from_path(&path);
    assert_eq!(reloaded.root_token.as_ref(), Some(&token));
    assert_eq!(reloaded.log_level.as_deref(), Some("debug"));
    let owner_path = if protected_main {
        path.clone()
    } else {
        agents_instance_path(&path)
    };
    let owner: Value = serde_json::from_slice(&std::fs::read(owner_path).unwrap()).unwrap();
    assert_eq!(
        owner["agents"].as_array().unwrap().len(),
        source["agents"].as_array().unwrap().len()
    );
    assert_invalid_row_members(&owner, &source);
    assert!(owner
        .pointer("/codingAgentProfiles/profileBindingVersion")
        .is_none());
    assert!(owner
        .pointer("/codingAgentProfiles/profilesByBaseHash")
        .is_none());
}

fn assert_invalid_row_members(owner: &Value, source: &Value) {
    for (stored, original) in owner["agents"]
        .as_array()
        .unwrap()
        .iter()
        .zip(source["agents"].as_array().unwrap())
    {
        for (key, value) in original.as_object().unwrap() {
            assert_eq!(stored.get(key), Some(value));
        }
    }
}

#[test]
fn e2432_disabled_env_and_supplied_hashes() {
    let mut raw = registered();
    for row in raw["agents"].as_array_mut().unwrap() {
        row["envs"] = json!([{"key":"K","value":"V","enabled":false}]);
        row["id_hash"] = json!("forged");
        row["id_hash_debug"] = json!(["forged"]);
    }
    let saved = canonical(&raw);
    let envs: Vec<CodingAgentEnv> =
        serde_json::from_value(raw["agents"][0]["envs"].clone()).unwrap();
    let hash = configuration_identity("tool", &envs);
    assert_ne!(hash, configuration_identity("tool", &[]));
    assert_eq!(saved["agents"][0]["id_hash"], hash);
    assert_ne!(saved["agents"][0]["id_hash_debug"], json!(["forged"]));
    assert_eq!(
        saved["codingAgentProfiles"]["profilesByBaseHash"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert!(saved["codingAgentProfiles"]["profilesByBaseHash"]
        .get(&hash)
        .is_some());
}

#[test]
fn e2432_inventory_keeps_instance_and_primary_project_sources() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    write_source(&path, &registered());
    let project = temp.path().join("primary");
    std::fs::create_dir(&project).unwrap();
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    let context = SourceReadContext::from_settings(&settings, &path).unwrap();
    let mut instance = context.clone();
    instance.project = false;
    instance.catalog_root = context.settings_path.parent().unwrap().to_path_buf();
    for owner in [&context, &instance] {
        for kind in [
            SourceKind::CatalogBase,
            SourceKind::CatalogProject,
            SourceKind::CatalogPersonal,
        ] {
            if !owner.project && kind == SourceKind::CatalogProject {
                continue;
            }
            let source = owner.source(kind);
            let locator = owner.locator(&source).unwrap();
            std::fs::create_dir_all(locator.parent().unwrap()).unwrap();
            std::fs::write(locator, br#"{"schemaVersion":2,"agents":[]}"#).unwrap();
        }
    }
    let sources = list_active_sources(&context);
    assert_eq!(sources.len(), 6);
    for source in &sources {
        let snapshot = read_source_snapshot(&context, source).unwrap();
        assert!(snapshot.snapshot.capabilities.available_read);
        assert_eq!(
            snapshot.snapshot.capabilities.editable_save,
            source.kind == SourceKind::RegisteredInstance
                || (source.kind == SourceKind::CatalogPersonal
                    && source.scope == SourceScope::Project)
        );
        assert!(!snapshot.snapshot.capabilities.editable_create);
        assert!(!snapshot.snapshot.capabilities.repair_by_owner);
    }
    let source = context.source(SourceKind::CatalogBase);
    let locator = context.locator(&source).unwrap();
    std::fs::remove_file(&locator).unwrap();
    assert_eq!(list_active_sources(&context).len(), 5);
    std::fs::create_dir(&locator).unwrap();
    assert_eq!(list_active_sources(&context).len(), 6);
    let invalid = read_source_snapshot(&context, &source).unwrap();
    assert_eq!(invalid.snapshot.availability, SourceAvailability::Invalid);
    assert!(!invalid.snapshot.capabilities.available_read);
    assert!(invalid.snapshot.agents.is_empty());
}
