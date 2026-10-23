use super::super::*;
use serde_json::json;

mod canonical_split;

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
