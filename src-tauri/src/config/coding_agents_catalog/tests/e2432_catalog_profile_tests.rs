use super::*;
use crate::config::settings as storage;
use serde_json::json;

// Exercise the existing production preparation, revision recheck and atomic
// publisher without activating the deferred coordination protocol.
fn save_catalog(
    settings: &storage::AppSettings,
    settings_path: &Path,
    candidate: &serde_json::Value,
) -> Result<serde_json::Value, storage::SourcePreparationError> {
    let context = storage::SourceReadContext::from_settings(settings, settings_path)?;
    let source = storage::list_active_sources(&context)
        .into_iter()
        .find(|source| {
            source.kind == storage::SourceKind::CatalogPersonal
                && source.scope == storage::SourceScope::Project
        })
        .unwrap();
    let snapshot = storage::read_source_snapshot(&context, &source)?;
    let prepared =
        storage::prepare_source_candidate(&snapshot, &snapshot.snapshot.revision, candidate, None)?;
    prepared.revalidate_before_write(&context)?;
    let path = local_catalog_path(&Path::new(&settings.project_paths[0]).join(".ac"));
    crate::config::local_config_io::write_file_atomic(&path, &prepared.serialized_after).unwrap();
    Ok(serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
}

fn source(schema: u32) -> serde_json::Value {
    let mut value = json!({
        "schemaVersion":schema,
        "agents":[{"key":"own","label":"Own","description":"Own","color":"#123456",
            "baseCommand":"tool","envs":[],"isolatedHome":false,"removable":true,
                "updateCommands":[],"autoUpdate":false,"catalogOnly":{"keep":true}}],
        "codingAgentProfiles":{"own":{"B":{"notes":"notes","extension":[null]}},"lost":null},
        "profileLabels":{"own":{"B":"label"},"labels-only":{"raw":null}},
        "identityByAgent":{"unrelated":"preserve"},
        "unrelated":[null,1]
    });
    if schema == 1 {
        value.as_object_mut().unwrap().remove("identityByAgent");
        value.as_object_mut().unwrap().remove("unrelated");
        value["agents"][0]
            .as_object_mut()
            .unwrap()
            .remove("catalogOnly");
    }
    value
}

#[test]
fn e2432_canonical_serializer_catalog1_catalog2() {
    for schema in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let settings = storage::AppSettings {
            project_paths: vec![temp.path().to_string_lossy().into_owned()],
            ..storage::AppSettings::default()
        };
        let path = local_catalog_path(&temp.path().join(".ac"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let raw = source(schema);
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let view = crate::config::settings::project_physical_profiles(&raw, false).unwrap();
        let saved = save_catalog(&settings, &settings_path, &view).unwrap();
        assert!(saved["agents"][0].get("id_hash").is_none());
        assert!(saved["agents"][0].get("id_hash_debug").is_none());
        assert_eq!(saved["schemaVersion"], schema);
        assert!(saved.get("codingAgentProfiles").is_none());
        assert!(saved.get("profileLabels").is_none());
        assert_eq!(saved["profileBindingVersion"], 1);
        assert_eq!(saved["profileSlots"], json!({}));
        assert_eq!(saved["identityByAgent"], raw["identityByAgent"]);
        assert_eq!(saved["agents"][0]["key"], raw["agents"][0]["key"]);
        assert_eq!(
            saved["agents"][0]["catalogOnly"],
            raw["agents"][0]["catalogOnly"]
        );
        assert_eq!(
            saved["agents"][0]["baseCommand"],
            raw["agents"][0]["baseCommand"]
        );
        assert_eq!(saved["unrelated"], raw["unrelated"]);
        assert_eq!(
            saved["legacyProfileOrphans"]["lost"],
            json!([{"cells":null}])
        );
        let loaded = crate::config::settings::project_physical_profiles(&saved, false).unwrap();
        assert_eq!(loaded["codingAgentProfiles"]["own"]["B"]["notes"], "notes");
        let twice = save_catalog(&settings, &settings_path, &loaded).unwrap();
        assert_eq!(twice, saved);
        let mut stale = twice.clone();
        stale["codingAgentProfiles"] = json!({"own":{"B":{"notes":"stale"}}});
        stale["profileLabels"] = json!({"own":{"B":"stale"}});
        std::fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
        let stale_view = storage::project_physical_profiles(&stale, false).unwrap();
        assert_eq!(
            stale_view["codingAgentProfiles"]["own"]["B"]["notes"],
            "notes"
        );
        let cleaned = save_catalog(&settings, &settings_path, &stale_view).unwrap();
        assert_eq!(cleaned, saved);
        if schema == 1 {
            let layer =
                parse_local_layer(&std::fs::read(&path).unwrap(), LayerDescription::Local).unwrap();
            let ordinary = compose_local_layer(&[], &layer, LayerDescription::Local).unwrap();
            assert_eq!(ordinary[0].command, "tool");
        } else {
            let decoded = parse_catalog_schema2(&serde_json::to_vec(&saved).unwrap()).unwrap();
            assert_eq!(decoded.agents[0].command, "tool");
            assert_eq!(decoded.coding_agent_profiles["own"]["B"].notes, "notes");
        }
    }
}

#[test]
fn e2432_orphan_wire_catalog() {
    for schema in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let settings = storage::AppSettings {
            project_paths: vec![temp.path().to_string_lossy().into_owned()],
            ..storage::AppSettings::default()
        };
        let path = local_catalog_path(&temp.path().join(".ac"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut raw = source(schema);
        raw["legacyProfileOrphans"] = json!({"lost":[{"labels":null,"unknown":{"keep":[]}}]});
        if schema == 1 {
            raw["agents"].as_array_mut().unwrap().extend([
                json!({"key":"partial","label":"No own command"}),
                json!({"key":"removed","remove":true}),
            ]);
            raw["codingAgentProfiles"]["partial"] = json!({"opaque":false});
            raw["codingAgentProfiles"]["removed"] = json!([]);
        }
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let view = crate::config::settings::project_physical_profiles(&raw, false).unwrap();
        assert!(view["codingAgentProfiles"].get("partial").is_none());
        assert!(view["codingAgentProfiles"].get("removed").is_none());
        let saved = save_catalog(&settings, &settings_path, &view).unwrap();
        assert!(saved["agents"][0].get("id_hash").is_none());
        assert!(saved["agents"][0].get("id_hash_debug").is_none());
        assert_eq!(
            saved["legacyProfileOrphans"]["lost"],
            json!([
                {"labels":null,"unknown":{"keep":[]}},{"cells":null}
            ])
        );
        assert_eq!(
            saved["legacyProfileOrphans"]["labels-only"],
            json!([{"labels":{"raw":null}}])
        );
        if schema == 1 {
            assert_eq!(
                saved["legacyProfileOrphans"]["partial"],
                json!([{"cells":{"opaque":false}}])
            );
            assert_eq!(
                saved["legacyProfileOrphans"]["removed"],
                json!([{"cells":[]}])
            );
        }
        let reload = storage::project_physical_profiles(&saved, false).unwrap();
        let twice = save_catalog(&settings, &settings_path, &reload).unwrap();
        assert_eq!(twice, saved);
        let mut recreated = storage::project_physical_profiles(&twice, false).unwrap();
        let mut row = source(1)["agents"][0].clone();
        row["key"] = json!("lost");
        row["baseCommand"] = json!("recreated-tool");
        recreated["agents"].as_array_mut().unwrap().push(row);
        let recreated = save_catalog(&settings, &settings_path, &recreated).unwrap();
        assert_eq!(
            recreated["legacyProfileOrphans"],
            saved["legacyProfileOrphans"]
        );
        let reloaded = storage::project_physical_profiles(&recreated, false).unwrap();
        assert!(reloaded["codingAgentProfiles"]["lost"].get("B").is_none());
        assert!(reloaded["profileLabels"]["lost"]
            .as_object()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn e2432_duplicate_catalog() {
    for schema in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let ac = temp.path().join(".ac");
        let path = local_catalog_path(&ac);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let definition: CodingAgentDefinition =
            serde_json::from_value(source(1)["agents"][0].clone()).unwrap();
        std::fs::write(manifest_path(&ac), build_managed_base_bytes(&[definition])).unwrap();
        let settings = storage::AppSettings {
            project_paths: vec![temp.path().to_string_lossy().into_owned()],
            ..storage::AppSettings::default()
        };
        let settings_path = temp.path().join("settings.json");
        for duplicate in [
            r#""x":1,"x":1"#,
            r#""x":1,"\u0078":2"#,
            r#""legacyProfileOrphans":{"X":[{"cells":{"nested":1,"nested":2}}]}"#,
            r#""codingAgentProfiles":{},"codingAgentProfiles":{}"#,
        ] {
            let raw = format!(r#"{{"schemaVersion":{schema},"agents":[],{duplicate}}}"#);
            std::fs::write(&path, raw.as_bytes()).unwrap();
            let error = parse_local_layer(raw.as_bytes(), LayerDescription::Local).unwrap_err();
            assert!(error.contains("duplicate JSON object member"));
            if schema == 2 {
                assert!(parse_catalog_schema2(raw.as_bytes())
                    .unwrap_err()
                    .to_string()
                    .contains("duplicate JSON object member"));
            }
            let report = load_catalog_report(&ac);
            assert_eq!(report.catalog.len(), 1);
            assert!(report
                .warnings
                .iter()
                .any(|warning| warning.reason.contains("duplicate JSON object member")));
            let context =
                storage::SourceReadContext::from_settings(&settings, &settings_path).unwrap();
            let selected = storage::list_active_sources(&context)
                .into_iter()
                .find(|source| source.kind == storage::SourceKind::CatalogPersonal)
                .unwrap();
            let snapshot = storage::read_source_snapshot(&context, &selected).unwrap();
            assert_eq!(
                snapshot.snapshot.availability,
                storage::SourceAvailability::Invalid
            );
            assert!(!snapshot.snapshot.capabilities.available_read);
            assert!(!snapshot.snapshot.capabilities.editable_save);
            assert!(snapshot.snapshot.agents.is_empty());
            assert!(save_catalog(&settings, &settings_path, &source(schema)).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), raw.as_bytes());
        }
        let raw = json!({"schemaVersion":schema,"agents":[],
            "legacyProfileOrphans":{"X":[{"cells":null},{"cells":null}]}});
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let view = storage::project_physical_profiles(&raw, false).unwrap();
        let saved = save_catalog(&settings, &settings_path, &view).unwrap();
        assert_eq!(saved["legacyProfileOrphans"], raw["legacyProfileOrphans"]);
    }
}

#[test]
fn e2432_schema2_layers_replace_complete_keys_and_keep_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let mut donor = source(1)["agents"][0].clone();
    donor["key"] = json!("replace");
    donor["envs"] = json!([{"key":"DONOR","value":"forbidden"}]);
    donor["instructionsFilename"] = json!("DONOR.md");
    donor["configSeed"] = json!({"enabled":true,"dest":".donor"});
    donor["installCommands"] = json!({"default":"donor install"});
    let base = vec![
        serde_json::from_value(source(1)["agents"][0].clone()).unwrap(),
        serde_json::from_value(donor).unwrap(),
    ];
    let complete = |key: &str, command: &str| {
        json!({
            "key":key,"label":key,"description":"own","color":"#123456","baseCommand":command
        })
    };
    let project = json!({"schemaVersion":2,"agents":[
        complete("project", "project-tool"), complete("replace", "project-replacement")
    ]});
    let mut personal = json!({"schemaVersion":2,"agents":[
        complete("personal", "personal-tool"), complete("replace", "personal-replacement")
    ]});
    let resolve = |personal: &serde_json::Value| {
        resolve_catalog_snapshot(
            temp.path(),
            CatalogSnapshot {
                base: Ok(Some(build_managed_base_bytes(&base))),
                project: Some(Ok(Some(serde_json::to_vec(&project).unwrap()))),
                local: Ok(Some(serde_json::to_vec(personal).unwrap())),
                journal: Ok(None),
            },
            CatalogSourceContext::Project,
        )
    };
    let resolved = resolve(&personal);
    assert!(resolved.unavailable.is_none());
    assert_eq!(
        resolved
            .catalog
            .iter()
            .map(|row| row.key.as_str())
            .collect::<Vec<_>>(),
        vec!["own", "replace", "project", "personal"]
    );
    let row = resolved
        .catalog
        .iter()
        .find(|row| row.key == "replace")
        .unwrap();
    assert_eq!(row.command, "personal-replacement");
    assert!(row.envs.is_empty());
    assert!(row.instructions_filename.is_none());
    assert!(row.config_seed.is_none());
    assert!(row.install_commands.is_none());
    personal["agents"][1]
        .as_object_mut()
        .unwrap()
        .remove("baseCommand");
    let rejected = resolve(&personal);
    assert!(rejected
        .warnings
        .iter()
        .any(|warning| warning.code == REPORT_CODE_LOCAL_INVALID));
    assert!(!rejected.catalog.iter().any(|row| row.key == "personal"));
    let row = rejected
        .catalog
        .iter()
        .find(|row| row.key == "replace")
        .unwrap();
    assert_eq!(row.command, "project-replacement");
    assert!(row.envs.is_empty());
    assert!(row.config_seed.is_none());
    assert!(rejected.catalog.iter().any(|row| row.key == "own"));
    assert!(rejected.catalog.iter().any(|row| row.key == "project"));
}

#[test]
fn e2432_catalog_conflict_rejects_without_publication() {
    for schema in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let settings = storage::AppSettings {
            project_paths: vec![temp.path().to_string_lossy().into_owned()],
            ..storage::AppSettings::default()
        };
        let path = local_catalog_path(&temp.path().join(".ac"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let raw = source(schema);
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let view = storage::project_physical_profiles(&raw, false).unwrap();
        let saved = save_catalog(&settings, &settings_path, &view).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut candidate = storage::project_physical_profiles(&saved, false).unwrap();
        let mut alias = candidate["agents"][0].clone();
        alias["key"] = json!("alias");
        candidate["agents"].as_array_mut().unwrap().push(alias);
        candidate["codingAgentProfiles"]["alias"] = json!({"B":{"notes":"conflict"}});
        candidate["profileLabels"]["alias"] = json!({"B":"different"});
        assert!(save_catalog(&settings, &settings_path, &candidate).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut legacy_conflict = raw.clone();
        legacy_conflict["agents"] = candidate["agents"].clone();
        legacy_conflict["codingAgentProfiles"]["alias"] = json!({"B":{"notes":"conflict"}});
        std::fs::write(&path, serde_json::to_vec(&legacy_conflict).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(save_catalog(&settings, &settings_path, &view).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
