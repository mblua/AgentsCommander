use super::*;

#[test]
fn load_save_load_save_is_byte_stable() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    let first = super::super::load_settings_from_path(&path);
    super::super::save_settings_to_path_preserving_project_paths(&first, &path).unwrap();
    let settings_bytes = std::fs::read(&path).unwrap();
    let sidecar_bytes = std::fs::read(b3_sidecar_path(temp.path())).unwrap();

    let mut second = super::super::load_settings_from_path(&path);
    assert!(!super::super::export_agents_to_instance_file(
        &mut second,
        &path
    ));
    super::super::save_settings_to_path_preserving_project_paths(&second, &path).unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), settings_bytes);
    assert_eq!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        sidecar_bytes
    );
}

/// E16 and E17: an agents file holding A under an overlay owning `agents` with B.
fn b3_seed_overlay_over_sidecar(dir: &std::path::Path) -> (std::path::PathBuf, Vec<u8>) {
    let path = b3_settings_path(dir);
    let sidecar_bytes = b3_seed_sidecar(dir, "Sidecar A", "Sidecar P");
    b3_write(&path, &b3_migrated_fixture());
    b3_write(
        &b3_overlay_path(dir),
        &serde_json::json!({ "agents": [b3_agent("alpha", "Overlay B")] }),
    );
    (path, sidecar_bytes)
}

#[test]
fn the_overlay_still_beats_the_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let (path, _) = b3_seed_overlay_over_sidecar(temp.path());

    for (loader, settings) in b3_all_loaders(&path) {
        assert_eq!(
            b3_labels(&settings),
            vec!["Overlay B".to_string()],
            "{loader} let the agents file override the overlay"
        );
    }
}

#[test]
fn a_preserve_save_does_not_freeze_the_overlay_into_the_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let (path, _) = b3_seed_overlay_over_sidecar(temp.path());
    let settings = super::super::load_settings_for_cli_from_path(&path);
    assert_eq!(b3_labels(&settings), vec!["Overlay B".to_string()]);

    super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Sidecar A");
    let reloaded = super::super::load_settings_for_cli_from_path(&path);
    assert_eq!(b3_labels(&reloaded), vec!["Overlay B".to_string()]);
}

#[test]
fn reconcile_never_writes_the_callers_groups() {
    // An existing agents file is left byte for byte.
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let sidecar_bytes = b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    b3_write(&path, &b3_migrated_fixture());
    let mut settings = super::super::load_settings_for_cli_from_path(&path);
    settings.agents[0].label = "Stale B".to_string();
    super::super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();
    assert_eq!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        sidecar_bytes
    );

    // No agents file: it is created from the disk object, not from the caller.
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_legacy_fixture("Disk A", "Disk P"));
    let mut settings = super::super::load_settings_for_cli_from_path(&path);
    settings.agents[0].label = "Stale B".to_string();
    super::super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Disk A");
    assert_eq!(
        sidecar["codingAgentProfiles"]["profileLabelsByAgent"]["alpha"]["A"],
        "Disk P"
    );
}

#[test]
fn an_unparseable_sidecar_blocks_the_direct_save_in_both_modes() {
    for reconcile in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_legacy_fixture("Legacy", "Legacy P"));
        std::fs::write(b3_sidecar_path(temp.path()), b"[1, 2").unwrap();
        let settings = super::super::load_settings_for_cli_from_path(&path);

        let result = if reconcile {
            super::super::save_settings_with_project_paths_to_path(&settings, &path).map(|_| ())
        } else {
            super::super::save_settings_to_path_preserving_project_paths(&settings, &path)
                .map(|_| ())
        };

        assert!(result.is_err(), "reconcile={reconcile}");
        assert_eq!(
            std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
            b"[1, 2",
            "reconcile={reconcile}"
        );
        let disk = b3_object(&path);
        assert!(
            disk.contains_key("agents") && disk.contains_key("codingAgentProfiles"),
            "reconcile={reconcile}"
        );
    }
}

#[test]
fn the_sidecar_is_merged_into_the_base_before_the_overlay() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    b3_write(&path, &b3_migrated_fixture());
    for (loader, settings) in b3_all_loaders(&path) {
        assert_eq!(
            b3_labels(&settings),
            vec!["Sidecar A".to_string()],
            "{loader}"
        );
    }

    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({ "logLevel": "debug" }),
    );
    for (loader, settings) in b3_all_loaders(&path) {
        assert!(!settings.local_overlay_state.is_empty(), "{loader}");
        assert_eq!(
            b3_labels(&settings),
            vec!["Sidecar A".to_string()],
            "{loader}"
        );
    }
}

#[test]
fn the_merge_point_is_between_the_detection_and_the_pre_merge_clone() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    // A legacy-shape key inside the agents file must not flip the flag.
    let mut sidecar = b3_object(&b3_sidecar_path(temp.path()));
    sidecar["codingAgentProfiles"]
        .as_object_mut()
        .unwrap()
        .insert("letters".to_string(), serde_json::json!({}));
    b3_write(
        &b3_sidecar_path(temp.path()),
        &serde_json::Value::Object(sidecar),
    );
    let mut base = b3_migrated_fixture();
    base["agents"] = serde_json::json!("not an agents array");
    b3_write(&path, &base);
    // An overlay leaf that fails the decode, so the base fallback runs.
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({ "defaultShellArgs": "not an array" }),
    );

    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, legacy_profiles) =
        super::super::parse_settings_json(&contents, "test", Some(&path))
            .expect("the undecodable fallback must carry the agents file's agents");

    assert!(!legacy_profiles);
    assert_eq!(b3_labels(&settings), vec!["Sidecar A".to_string()]);
    assert!(
        matches!(
            settings.local_overlay_state.rejection(),
            Some(crate::config::local_overlay::OverlayRejection::MergedValueUndecodable(_))
        ),
        "the fallback did not run: {:?}",
        settings.local_overlay_state.rejection()
    );
}

// #2716 (B3) - rows E25 to E31 and E40 of the phase plan.

fn b3_seed_corrupt_sidecar(dir: &std::path::Path) {
    std::fs::write(b3_sidecar_path(dir), "{ this is not json").unwrap();
}

fn b3_marked(settings: &super::super::AppSettings) -> Option<std::path::PathBuf> {
    settings
        .agents_layer
        .unreadable_path()
        .map(|p| p.to_path_buf())
}

#[test]
fn the_terminal_snapshot_cas_reads_the_sidecar_and_never_writes_an_empty_agents_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_migrated_fixture());
    b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    let current = super::super::load_settings_for_cli_from_path(&path);
    let gate = current.terminal_snapshots_enabled;

    let returned = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &current, &path, gate, !gate,
    )
    .expect("the CAS write must succeed");

    assert_eq!(
        returned.terminal_snapshots_enabled, !gate,
        "the CAS did not write"
    );

    assert_eq!(b3_labels(&returned), vec!["Sidecar A".to_string()]);
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(
        sidecar["agents"][0]["label"], "Sidecar A",
        "the CAS wrote {:?} into the agents file",
        sidecar["agents"]
    );
    assert!(!b3_object(&path).contains_key("agents"));
}

#[test]
fn a_deleted_sidecar_loses_only_the_agents_and_keeps_every_other_setting() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_migrated_fixture());
    assert!(!b3_sidecar_path(temp.path()).exists());

    let loaded = super::super::load_settings_from_path(&path);

    assert_eq!(loaded.default_shell, "test-shell");
    assert_eq!(loaded.root_token.as_deref(), Some("base-token"));
    assert!(loaded.agents.is_empty());
}

#[test]
fn a_migrated_settings_file_passes_the_non_project_validation() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let migrated = b3_migrated_fixture();
    super::super::validate_non_project_settings(migrated.as_object().unwrap())
        .expect("a settings file without agents must pass the gate");
    b3_write(&path, &migrated);
    b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    let loaded = super::super::load_settings_for_cli_from_path(&path);

    super::super::save_settings_to_path_preserving_project_paths(&loaded, &path)
        .expect("a Preserve save over a migrated file must succeed");
}

#[test]
fn the_mark_is_set_only_by_an_unreadable_sidecar() {
    crate::logging::test_install_logger();
    // Corrupt: the mark is set, every other setting survives, notice 1 is raised.
    {
        let _drain_guard = crate::logging::ERROR_SINK_TEST_DRAIN_LOCK.blocking_lock();
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        b3_seed_corrupt_sidecar(temp.path());
        let _ = crate::logging::error_sink().drain();

        let loaded = super::super::load_settings_from_path(&path);

        let sidecar = b3_sidecar_path(temp.path());
        assert_eq!(b3_marked(&loaded), Some(sidecar.clone()), "corrupt leg");
        assert_eq!(loaded.default_shell, "test-shell");
        assert_eq!(loaded.root_token.as_deref(), Some("base-token"));
        assert!(loaded.agents.is_empty());
        let notice = super::super::agents_unreadable_startup_notice(&sidecar);
        assert!(notice
            .ends_with("While this file is unreadable, changes to your sessions are not saved."));
        let captured = crate::logging::error_sink().drain();
        assert!(
            captured.iter().any(|e| e.message.contains(&notice)),
            "notice 1 was not captured: {:?}",
            captured.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
    // Absent: clear.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        let loaded = super::super::load_settings_from_path(&path);
        assert_eq!(b3_marked(&loaded), None, "absent leg");
    }
    // Parseable: clear.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
        let loaded = super::super::load_settings_from_path(&path);
        assert_eq!(b3_marked(&loaded), None, "parseable leg");
    }
}

#[test]
fn the_mark_survives_the_defaults_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &serde_json::json!({ "defaultShell": 42, "rootToken": "base-token" }),
    );
    b3_seed_corrupt_sidecar(temp.path());

    let loaded = super::super::load_settings_from_path(&path);

    assert_ne!(
        loaded.root_token.as_deref(),
        Some("base-token"),
        "the settings file decoded, so the fallback did not run"
    );
    assert_eq!(
        b3_marked(&loaded),
        Some(b3_sidecar_path(temp.path())),
        "the defaults fallback lost the mark"
    );
    assert!(super::super::agents_layer_unreadable_for_dir(temp.path()));
}

#[test]
fn the_mark_is_never_serialized_and_cannot_be_forged() {
    // Outward: a save with the mark set writes the same bytes as one without it.
    for reconcile in [false, true] {
        let mut bytes = Vec::new();
        for marked in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = b3_settings_path(temp.path());
            b3_write(&path, &b3_migrated_fixture());
            b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
            let mut settings = super::super::load_settings_for_cli_from_path(&path);
            if marked {
                settings.agents_layer = super::super::AgentsLayerState::Unreadable {
                    path: b3_sidecar_path(temp.path()),
                    reason: "forged for the test".to_string(),
                };
            }
            if reconcile {
                super::super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();
            } else {
                super::super::save_settings_to_path_preserving_project_paths(&settings, &path)
                    .unwrap();
            }
            let settings_bytes = std::fs::read(&path).unwrap();
            let sidecar_bytes = std::fs::read(b3_sidecar_path(temp.path())).unwrap();
            for raw in [&settings_bytes, &sidecar_bytes] {
                let text = String::from_utf8_lossy(raw);
                assert!(!text.contains("agentsLayer") && !text.contains("agents_layer"));
            }
            bytes.push((settings_bytes, sidecar_bytes));
        }
        assert_eq!(
            bytes[0], bytes[1],
            "reconcile={reconcile}: the mark changed the bytes"
        );
    }
    // Inward: a renderer-shaped object carrying the key decodes with the mark clear.
    let mut value = serde_json::to_value(super::super::AppSettings::default()).unwrap();
    let object = value.as_object_mut().unwrap();
    let forged = serde_json::json!({ "Unreadable": { "path": "C:/x", "reason": "forged" } });
    object.insert("agentsLayer".to_string(), forged.clone());
    object.insert("agents_layer".to_string(), forged);
    let decoded: super::super::AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(
        decoded.agents_layer,
        super::super::AgentsLayerState::Readable
    );
}

#[test]
fn both_spawn_builders_refuse_under_the_mark() {
    let temp = tempfile::tempdir().unwrap();
    let sidecar = b3_sidecar_path(temp.path());
    let mut settings = super::super::AppSettings {
        agents: vec![serde_json::from_value(b3_agent("alpha", "Alpha")).unwrap()],
        agents_layer: super::super::AgentsLayerState::Unreadable {
            path: sidecar.clone(),
            reason: "test".to_string(),
        },
        ..Default::default()
    };
    let cwd = temp.path().to_string_lossy().into_owned();
    let expected = super::super::agents_unreadable_session_error(&sidecar);

    // Leg one: build.
    let built = crate::commands::session::build_configured_agent_spawn_for_cwd(
        &settings, "alpha", &cwd, None,
    );
    assert_eq!(built.err().as_deref(), Some(expected.as_str()), "build leg");
    // Leg two: resolve.
    let resolved = crate::commands::session::resolve_configured_agent_spawn_for_cwd(
        &settings, "alpha", &cwd, None, false,
    );
    assert_eq!(
        resolved.err().as_deref(),
        Some(expected.as_str()),
        "resolve leg"
    );

    // Mark clear, unknown id: both answer Ok(None).
    settings.agents_layer = super::super::AgentsLayerState::Readable;
    assert!(matches!(
        crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings, "unknown", &cwd, None
        ),
        Ok(None)
    ));
    assert!(matches!(
        crate::commands::session::resolve_configured_agent_spawn_for_cwd(
            &settings, "unknown", &cwd, None, false
        ),
        Ok(None)
    ));
}

#[test]
fn the_cli_loaders_also_set_the_mark() {
    type Loader = fn(&std::path::Path) -> super::super::AppSettings;
    let loaders: [(&str, Loader); 2] = [
        ("load_settings_for_cli", |p| {
            super::super::load_settings_for_cli_from_path(p)
        }),
        ("load_settings_for_cli_strict", |p| {
            super::super::load_settings_for_cli_strict_from_path(Some(p)).unwrap()
        }),
    ];
    for (name, load) in loaders {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        b3_seed_corrupt_sidecar(temp.path());
        let key = path.parent().unwrap().to_path_buf();

        let loaded = load(&path);
        assert_eq!(
            b3_marked(&loaded),
            Some(b3_sidecar_path(temp.path())),
            "{name}"
        );
        assert!(
            super::super::agents_layer_unreadable_for_dir(&key),
            "{name}: key not inserted"
        );

        b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
        let loaded = load(&path);
        assert_eq!(b3_marked(&loaded), None, "{name}");
        assert!(
            !super::super::agents_layer_unreadable_for_dir(&key),
            "{name}: key not removed"
        );
    }
}

/// #2716 (B3) E43, r12o: with no agents file on disk, only the export's lock
/// can stop its write. Held by this thread, the export on another thread
/// cannot acquire it, writes nothing and strips nothing; the three-agent file
/// written under the guard is what a later load returns.
#[test]
fn the_export_cannot_write_a_sidecar_while_the_lock_is_held() {
    let two_agents = |dir: &std::path::Path| {
        let path = b3_settings_path(dir);
        let mut legacy = b3_legacy_fixture("Alpha", "P");
        legacy["agents"] =
            serde_json::json!([b3_agent("alpha", "Alpha"), b3_agent("beta", "Beta")]);
        legacy["codingAgentProfiles"]["profileLabelsByAgent"]["beta"] =
            legacy["codingAgentProfiles"]["profileLabelsByAgent"]["alpha"].clone();
        b3_write(&path, &legacy);
        path
    };

    // Positive control: the same fixture with no lock held exports two agents.
    let control = tempfile::tempdir().unwrap();
    let control_path = two_agents(control.path());
    let mut settings = super::super::load_settings_for_cli_from_path(&control_path);
    assert!(super::super::export_agents_to_instance_file(
        &mut settings,
        &control_path
    ));
    let exported = b3_object(&b3_sidecar_path(control.path()));
    assert_eq!(exported["agents"].as_array().map(Vec::len), Some(2));

    // Under the lock.
    let temp = tempfile::tempdir().unwrap();
    let path = two_agents(temp.path());
    let seeded = std::fs::read(&path).unwrap();
    let guard =
        super::super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
    let export_path = path.clone();
    let exported = std::thread::spawn(move || {
        let mut settings = super::super::load_settings_for_cli_from_path(&export_path);
        super::super::export_agents_to_instance_file(&mut settings, &export_path)
    })
    .join()
    .unwrap();
    assert!(
        !b3_sidecar_path(temp.path()).exists(),
        "the export wrote the agents file while the lock was held"
    );
    assert!(!exported, "the export ran while the lock was held");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        seeded,
        "the legacy keys changed"
    );

    // Still under the guard: another writer's three agents.
    b3_write(
        &b3_sidecar_path(temp.path()),
        &serde_json::json!({
            "agents": [
                b3_agent("alpha", "Alpha"),
                b3_agent("beta", "Beta"),
                b3_agent("gamma", "Gamma"),
            ],
        }),
    );
    drop(guard);

    let loaded = super::super::load_settings_from_path(&path);
    assert_eq!(
        b3_labels(&loaded),
        vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()]
    );
}
