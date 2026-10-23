use super::*;

#[test]
fn e2432_round7_sidecar_success_main_failure_restarts_canonical_first() {
    let (temp, path) = hash_activation_seed(false);
    let path = std::fs::canonicalize(temp.path())
        .unwrap()
        .join(path.file_name().unwrap());
    let main_before = std::fs::read(&path).unwrap();
    let owner =
        super::super::SettingsOwner::open_active(&path, std::time::Duration::from_secs(2)).unwrap();
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::super::SettingsSaveStage::AtomicReplace))
    });
    let report = owner
        .with_operation_blocking("e2432-round7-transfer", |current, guard| {
            super::super::registered_profile_raw_gate(guard.settings_path())?;
            guard.publish(super::super::SettingsPublisher::SettingsSave, |physical| {
                super::super::save_settings_value_locked(
                    current,
                    physical,
                    super::super::ProjectWriteMode::Preserve,
                    super::super::TerminalSnapshotGateWriteMode::Preserve,
                    super::super::MainWindowPlacementWriteMode::Preserve,
                )
                .map_err(|error| error.to_string())
            })
        })
        .unwrap();
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert_eq!(
        report.outcome,
        super::super::OperationOutcome::PartialOrUnknown
    );
    assert!(report.value.is_none());
    assert!(report.error.is_some());
    assert_eq!(std::fs::read(&path).unwrap(), main_before);
    let sidecar = super::super::agents_instance_path(&path);
    let committed = b3_object(&sidecar);
    let committed_revision = super::super::source_bytes_revision(&std::fs::read(&sidecar).unwrap());
    assert_eq!(committed["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert!(committed["codingAgentProfiles"]
        .get("profilesByAgent")
        .is_none());
    owner.drain_blocking(std::time::Duration::from_secs(2));
    let before_restart = hash_activation_files(temp.path());
    let restarted = super::super::load_settings_from_path(&path);
    assert_eq!(hash_activation_files(temp.path()), before_restart);
    assert_eq!(
        super::super::source_bytes_revision(&std::fs::read(&sidecar).unwrap()),
        committed_revision
    );
    assert_eq!(
        serde_json::to_value(&restarted.agents).unwrap(),
        serde_json::to_value(super::super::load_settings_for_cli_from_path(&path).agents).unwrap()
    );
    let mut edited = restarted;
    edited
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "fresh canonical edit".into();
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&edited, &path).unwrap();
    assert_eq!(
        written.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
        "fresh canonical edit"
    );
    let hash = super::super::configuration_identity("claude", &[]);
    assert_eq!(
        b3_object(&sidecar)["codingAgentProfiles"]["profilesByBaseHash"][&hash]["A"]["notes"],
        "fresh canonical edit"
    );
    assert_ne!(
        super::super::source_bytes_revision(&std::fs::read(&sidecar).unwrap()),
        committed_revision
    );
    assert_eq!(
        b3_object(&path).get("codingAgentProfiles"),
        serde_json::from_slice::<serde_json::Value>(&main_before)
            .unwrap()
            .get("codingAgentProfiles")
    );
    assert_o2_stable_full_load(&path);
}

#[test]
fn e2432_round7_shadowed_equal_strips_only_on_legitimate_publication() {
    let (_temp, path) = hash_activation_seed(true);
    super::super::load_settings_from_path(&path);
    let sidecar = b3_object(&super::super::agents_instance_path(&path));
    let mut main = b3_object(&path);
    for key in super::super::AGENTS_INSTANCE_KEYS {
        main.insert(key.into(), sidecar[key].clone());
    }
    b3_write(&path, &serde_json::Value::Object(main));
    assert_o2_stable_full_load(&path);
    let mut settings = super::super::load_settings_for_cli_from_path(&path);
    settings.log_level = Some("debug".into());
    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let main = b3_object(&path);
    assert_eq!(main["logLevel"], "debug");
    for key in super::super::AGENTS_INSTANCE_KEYS {
        assert!(!main.contains_key(key));
    }
    assert_o2_stable_full_load(&path);
}

#[test]
fn e2432_round7_read_only_catalog_decoys_never_supply_migration_data() {
    let (_temp, path) = hash_activation_seed(true);
    let initial = super::super::load_settings_for_cli_from_path(&path);
    let mut context = super::super::SourceReadContext::from_settings(&initial, &path).unwrap();
    context.project = false;
    context.catalog_root = path.parent().unwrap().to_path_buf();
    let mut catalogs = Vec::new();
    for kind in [
        super::super::SourceKind::CatalogBase,
        super::super::SourceKind::CatalogPersonal,
    ] {
        let source = context.source(kind);
        let locator = context.locator(&source).unwrap();
        std::fs::create_dir_all(locator.parent().unwrap()).unwrap();
        let decoy = serde_json::json!({"schemaVersion":2,
                "agents":[{"key":"alpha","label":"Decoy","description":"Independent","color":"#1122\
                    33","baseCommand":"claude"}],
                "codingAgentProfiles":{"alpha":{"A":{"notes":"independent decoy"}}}});
        b3_write(&locator, &decoy);
        catalogs.push((source, locator.clone(), std::fs::read(locator).unwrap()));
    }
    let main_before = std::fs::read(&path).unwrap();
    let sidecar = super::super::agents_instance_path(&path);
    let registered_before = std::fs::read(&sidecar).unwrap();
    super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    super::super::capture_project_settings(&path, false).unwrap();
    super::super::list_active_sources(&context);
    let registered_source = context.source(super::super::SourceKind::RegisteredInstance);
    for source in catalogs
        .iter()
        .map(|(source, _, _)| source)
        .chain(std::iter::once(&registered_source))
    {
        let snapshot = super::super::read_source_snapshot(&context, source).unwrap();
        assert_eq!(
            snapshot.snapshot.availability,
            super::super::SourceAvailability::Available
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), main_before);
    assert_eq!(std::fs::read(&sidecar).unwrap(), registered_before);
    let migrated = super::super::load_settings_from_path(&path);
    assert_ne!(
        migrated.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
        "independent decoy"
    );
    for (_, locator, bytes) in catalogs {
        assert_eq!(std::fs::read(locator).unwrap(), bytes);
    }
    assert_o2_stable_full_load(&path);
}

#[test]
fn e2432_round7_canonical_main_does_not_relocate_on_startup() {
    let (_temp, path) = hash_activation_seed(false);
    let main = serde_json::Value::Object(b3_object(&path));
    let projected = super::super::project_physical_profiles(&main, true).unwrap();
    let canonical = super::super::canonical_profile_source(&main, &projected, true).unwrap();
    b3_write(&path, &canonical);
    std::fs::remove_file(super::super::agents_instance_path(&path)).unwrap();
    assert_o2_stable_full_load(&path);
    assert!(!super::super::agents_instance_path(&path).exists());
    let mut settings = super::super::load_settings_for_cli_from_path(&path);
    settings.log_level = Some("debug".into());
    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert!(!super::super::agents_instance_path(&path).exists());
    assert_eq!(
        b3_object(&path)["codingAgentProfiles"],
        canonical["codingAgentProfiles"]
    );
}

#[test]
fn e2432_o2_sidecar_full_load_converts_once_and_keeps_raw_roots() {
    let (temp, path) = hash_activation_seed(true);
    let sidecar = super::super::agents_instance_path(&path);
    let mut raw = serde_json::Value::Object(b3_object(&sidecar));
    raw["codingAgentProfiles"]["letters"] = serde_json::Value::Null;
    raw["codingAgentProfiles"]["matrix"] = serde_json::json!([{"raw":null}]);
    raw["codingAgentProfiles"]["agentDefaults"] = serde_json::json!({"unknown":[7]});
    raw["codingAgentProfiles"]["profileSlots"]["A"]["name"] = serde_json::json!("opaque");
    raw["codingAgentProfiles"]["profileSlots"]["A"]["extension"] = serde_json::json!([null, 9]);
    raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["additionalArguments"] =
        serde_json::json!("--model x");
    raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["argv"] =
        serde_json::json!(["opaque"]);
    raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["args"] =
        serde_json::json!({"raw":null});
    raw["codingAgentProfiles"]["legacyProfileOrphans"] =
        serde_json::json!({"old":[{"cells":null,"extension":[7]}]});
    b3_write(&sidecar, &raw);
    let backup = path.with_file_name(
        super::super::super::instance_artifacts::SETTINGS_MIGRATION_BACKUP_384_V1_NAME,
    );
    std::fs::write(&backup, b"first main backup").unwrap();
    assert!(!super::super::legacy_profiles_shape_present(&b3_object(
        &path
    )));

    let loaded = super::super::load_settings_from_path(&path);
    assert_eq!(
        loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].command,
        "--model x"
    );
    let saved = b3_object(&sidecar);
    let profiles = &saved["codingAgentProfiles"];
    assert_eq!(profiles["profileBindingVersion"], 1);
    for key in ["letters", "matrix", "agentDefaults", "legacyProfileOrphans"] {
        assert_eq!(
            profiles.get(key),
            raw["codingAgentProfiles"].get(key),
            "{key}"
        );
    }
    assert_eq!(profiles["profileSlots"]["A"]["name"], "opaque");
    assert_eq!(
        profiles["profileSlots"]["A"]["extension"],
        serde_json::json!([null, 9])
    );
    let hash = super::super::configuration_identity("claude", &[]);
    assert_eq!(
        profiles["profilesByBaseHash"][&hash]["A"]["argv"],
        serde_json::json!(["opaque"])
    );
    assert_eq!(
        profiles["profilesByBaseHash"][&hash]["A"]["args"],
        serde_json::json!({"raw":null})
    );
    assert!(profiles.get("profilesByAgent").is_none());
    assert!(profiles.get("profileLabelsByAgent").is_none());
    assert!(profiles.get("identityByAgent").is_none());
    assert_eq!(std::fs::read(&backup).unwrap(), b"first main backup");
    assert_o2_stable_full_load(&path);
    assert_o2_stable_full_load(&path);
    assert_eq!(std::fs::read(&backup).unwrap(), b"first main backup");
    let backups: Vec<_> = hash_activation_files(temp.path())
        .into_keys()
        .filter(|name| name.contains("pre-384"))
        .collect();
    assert_eq!(
        backups,
        vec![backup.file_name().unwrap().to_string_lossy().into_owned()]
    );
}

#[test]
fn e2432_o2_sidecar_stale_maps_cleanup_remains_actionable_without_replay() {
    let (_temp, path) = hash_activation_seed(true);
    let sidecar = super::super::agents_instance_path(&path);
    let raw = serde_json::Value::Object(b3_object(&sidecar));
    let projected = super::super::project_physical_profiles(&raw, true).unwrap();
    let mut canonical = super::super::canonical_profile_source(&raw, &projected, true).unwrap();
    canonical["codingAgentProfiles"]["letters"] = serde_json::json!([null, {"opaque":7}]);
    canonical["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
        "alpha":{"A":{"notes":"stale cannot overwrite BASE"}}, "lost":null
    });
    canonical["codingAgentProfiles"]["profileLabelsByAgent"] =
        serde_json::json!({"labels-only":null});
    canonical["codingAgentProfiles"]["identityByAgent"] =
        serde_json::json!({"identity-only":"not BASE"});
    b3_write(&sidecar, &canonical);
    assert!(!super::super::legacy_profiles_shape_present(&b3_object(
        &path
    )));
    let loaded = super::super::load_settings_from_path(&path);
    assert_ne!(
        loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
        "stale cannot overwrite BASE"
    );
    let saved = b3_object(&sidecar);
    let profiles = &saved["codingAgentProfiles"];
    assert_eq!(
        profiles["letters"],
        canonical["codingAgentProfiles"]["letters"]
    );
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(profiles.get(key).is_none(), "{key}");
    }
    assert_eq!(
        profiles["legacyProfileOrphans"]["lost"],
        serde_json::json!([{"cells":null}])
    );
    assert_eq!(
        profiles["legacyProfileOrphans"]["labels-only"],
        serde_json::json!([{"labels":null}])
    );
    assert_eq!(
        profiles["legacyProfileOrphans"]["identity-only"],
        serde_json::json!([{"identity":"not BASE"}])
    );
    let backup = path.with_file_name(
        super::super::super::instance_artifacts::SETTINGS_MIGRATION_BACKUP_384_V1_NAME,
    );
    assert!(!backup.exists());
    assert_o2_stable_full_load(&path);
    assert_eq!(b3_object(&sidecar)["codingAgentProfiles"], *profiles);
    assert!(!backup.exists());
}

#[test]
fn e2432_o2_protected_legacy_keeps_baseline_save_cause() {
    let (temp, path) = hash_activation_seed(true);
    let mut main = b3_object(&path);
    main.insert(
        "codingAgentProfiles".into(),
        serde_json::json!({"schemaVersion":2,"letters":{"old":true}}),
    );
    b3_write(&path, &serde_json::Value::Object(main));
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({"codingAgentProfiles":{"profileSlots":{"A":{"label":"overlay"}}}}),
    );
    let sidecar = super::super::agents_instance_path(&path);
    let raw_profiles = b3_object(&sidecar)["codingAgentProfiles"].clone();
    let (diagnostics, _capture) = capture_settings_save_diagnostics();
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::super::SettingsSaveStage::TempCreate));
    });
    let loaded = super::super::load_settings_from_path(&path);
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(loaded
        .local_overlay_state
        .owns_top_level("codingAgentProfiles"));
    assert!(
        !diagnostics.borrow().is_empty(),
        "baseline protected legacy save was suppressed"
    );
    assert_eq!(b3_object(&sidecar)["codingAgentProfiles"], raw_profiles);
}

#[test]
fn e2432_o2_ordinary_repair_keeps_baseline_save_cause() {
    let (_temp, path) = hash_activation_seed(true);
    let sidecar = super::super::agents_instance_path(&path);
    let raw = serde_json::Value::Object(b3_object(&sidecar));
    let projected = super::super::project_physical_profiles(&raw, true).unwrap();
    let mut canonical = super::super::canonical_profile_source(&raw, &projected, true).unwrap();
    canonical["codingAgentProfiles"]["profileSlots"] = serde_json::json!({});
    b3_write(&sidecar, &canonical);
    assert!(!super::super::actionable_profile_legacy_work(&canonical, false).unwrap());
    let (diagnostics, _capture) = capture_settings_save_diagnostics();
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::super::SettingsSaveStage::TempCreate));
    });
    let loaded = super::super::load_settings_from_path(&path);
    super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(loaded.coding_agent_profiles.profile_slots.contains_key("A"));
    assert!(
        !diagnostics.borrow().is_empty(),
        "ordinary repair publication was suppressed"
    );
}

#[test]
fn e2432_o2_sidecar_rejections_keep_every_source_unchanged() {
    for case in ["conflict", "quarantine", "duplicate"] {
        let (temp, path) = hash_activation_seed(true);
        let sidecar = super::super::agents_instance_path(&path);
        let mut raw = serde_json::Value::Object(b3_object(&sidecar));
        raw["codingAgentProfiles"]["letters"] = serde_json::json!([null]);
        if case == "conflict" {
            raw["agents"]
                .as_array_mut()
                .unwrap()
                .push(b3_agent("beta", "B"));
            raw["codingAgentProfiles"]["profilesByAgent"]["beta"] =
                serde_json::json!({"A":{"notes":"different complete set"}});
        } else if case == "quarantine" {
            raw["codingAgentProfiles"]["legacyProfileOrphans"] = serde_json::Value::Null;
        }
        let mut bytes = raw.to_string();
        if case == "duplicate" {
            bytes.pop();
            bytes.push_str(",\"unknown\":1,\"unknown\":1}");
        }
        std::fs::write(&sidecar, bytes).unwrap();
        let before = hash_activation_files(temp.path());
        let loaded = super::super::load_settings_from_path(&path);
        assert_eq!(hash_activation_files(temp.path()), before, "startup {case}");
        assert!(
            super::super::save_settings_to_path_preserving_project_paths(&loaded, &path).is_err(),
            "save {case}"
        );
        assert_eq!(hash_activation_files(temp.path()), before, "save {case}");
    }
}

pub(super) fn assert_selected_project_decode_fallback_case(startup: bool, unreadable: bool) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let project = temp.path().join("A");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let a = project.to_string_lossy().replace('\\', "/");
    // Valid source object, unrelated typed field cannot decode.
    let source = serde_json::to_vec(&serde_json::json!({
        "rootToken":42,"projectPaths":[a],"projectPath":a
    }))
    .unwrap();
    std::fs::write(&path, &source).unwrap();
    let overlay_path = b3_overlay_path(temp.path());
    let overlay = br####"{"roomNumberMask":"###"}"####;
    std::fs::write(&overlay_path, overlay).unwrap();
    let agents_path = super::super::agents_instance_path(&path);
    let defaults = serde_json::to_value(super::super::AppSettings::default()).unwrap();
    let agents = if unreadable {
        b"{broken agents".to_vec()
    } else {
        serde_json::to_vec(&serde_json::json!({
            "agents":defaults["agents"],
            "codingAgentProfiles":defaults["codingAgentProfiles"]
        }))
        .unwrap()
    };
    std::fs::write(&agents_path, &agents).unwrap();
    let (settings, legacy_profiles, original) = if startup {
        super::super::read_settings_for_startup(&path)
    } else {
        super::super::capture_project_settings(&path, false).unwrap()
    };
    let expected = crate::config::projects::display_canonical(
        std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .as_ref(),
    );
    assert_eq!(settings.project_paths, vec![expected.clone()]);
    assert_eq!(settings.project_path, Some(expected));
    assert!(settings.project_paths_persistence_error.is_none());
    assert_eq!(settings.room_number_mask, "###");
    assert_eq!(settings.root_token, None);
    assert!(!legacy_profiles);
    assert!(original.is_none());
    assert_eq!(
        settings.agents_layer.unreadable_path().is_some(),
        unreadable
    );
    assert_eq!(
        super::super::agents_layer_unreadable_for_dir(temp.path()),
        unreadable
    );
    assert_eq!(
        super::super::try_with_agents_layer_publication_state(temp.path(), |flag| flag).unwrap(),
        unreadable
    );
    assert_eq!(std::fs::read(&agents_path).unwrap(), agents);
    assert_eq!(std::fs::read(&overlay_path).unwrap(), overlay);
    let sidecar = super::super::project_paths_path(&path);
    if startup {
        assert!(super::super::raw_project_projection(&b3_object(&path)).is_empty());
    } else {
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert!(!sidecar.exists());
        assert!(!temp
            .path()
            .join(super::super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME)
            .exists());
    }
    assert_selected_project_decode_fallback_preserve(
        &settings,
        &path,
        &agents_path,
        &agents,
        &overlay_path,
        overlay,
    );
}

fn assert_selected_project_decode_fallback_preserve(
    settings: &super::super::AppSettings,
    path: &std::path::Path,
    agents_path: &std::path::Path,
    agents: &[u8],
    overlay_path: &std::path::Path,
    overlay: &[u8],
) {
    let sidecar = super::super::project_paths_path(path);
    let before_sidecar = std::fs::read(&sidecar).ok();
    match super::super::save_settings_to_path_preserving_project_paths_typed(settings, path) {
        Ok(_) => {
            assert_eq!(std::fs::read(agents_path).unwrap(), agents);
            assert!(super::super::raw_project_projection(&b3_object(path)).is_empty());
        }
        Err(error) => {
            eprintln!("P1 Preserve explicitly refused: {error:?}");
            assert_eq!(std::fs::read(agents_path).unwrap(), agents);
        }
    }
    if let Some(bytes) = before_sidecar {
        assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
    }
    assert_eq!(std::fs::read(overlay_path).unwrap(), overlay);
}

#[test]
fn unavailable_project_fallback_retains_agents_publication_mark() {
    for unreadable in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let source = br#"{"rootToken":42}"#;
        let destination = b"{broken project source";
        let agents = if unreadable {
            b"{broken".as_slice()
        } else {
            b"{}".as_slice()
        };
        std::fs::write(&path, source).unwrap();
        std::fs::write(super::super::project_paths_path(&path), destination).unwrap();
        std::fs::write(super::super::agents_instance_path(&path), agents).unwrap();
        for startup in [false, true] {
            let (settings, _, _) = if startup {
                super::super::read_settings_for_startup(&path)
            } else {
                super::super::capture_project_settings(&path, false).unwrap()
            };
            assert!(settings.project_paths_persistence_error.is_some());
            assert!(settings.project_paths.is_empty());
            assert!(settings.project_path.is_none());
            assert_eq!(
                settings.agents_layer.unreadable_path().is_some(),
                unreadable
            );
            assert_eq!(
                super::super::try_with_agents_layer_publication_state(temp.path(), |flag| flag)
                    .unwrap(),
                unreadable
            );
            assert_eq!(std::fs::read(&path).unwrap(), source);
            assert_eq!(
                std::fs::read(super::super::project_paths_path(&path)).unwrap(),
                destination
            );
            assert_eq!(
                std::fs::read(super::super::agents_instance_path(&path)).unwrap(),
                agents
            );
            assert!(settings.root_token.is_none());
        }
    }
}
