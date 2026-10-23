use super::*;

/// The set of top-level keys added, removed, or changed between two objects.
fn mutated_keys(before: &Map<String, Value>, after: &Map<String, Value>) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for (key, value) in before {
        match after.get(key) {
            Some(other) if other == value => {}
            _ => {
                keys.insert(key.clone());
            }
        }
    }
    for key in after.keys() {
        if !before.contains_key(key) {
            keys.insert(key.clone());
        }
    }
    keys
}

fn object_of(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        other => panic!("not an object: {other}"),
    }
}

// S26
#[test]
fn the_premigration_unsafe_set_is_derived_from_the_project_decode() {
    // Every project row is written by the decode: the two active keys are
    // rebuilt from the selected pairs (an unresolvable registration selects
    // nothing), the archived key is rebuilt from the archived pairs (a row
    // that carries no usable absolute string contributes nothing), and the
    // three companions are removed outright. An instance base is passed
    // explicitly, so the test never depends on `production_instance_base()`.
    let temp = tempfile::tempdir().unwrap();
    let instance_base = temp.path();

    let mut value = json!({
        "logLevel": "info",
        "activityLogEnabled": true,
        "agents": [{"id": "codex", "label": "Codex", "command": "codex", "color": "#000"}],
        "projectPath": "no-such-project-directory",
        "projectPathRelativeToInstance": "a",
        "projectPaths": ["no-such-project-directory"],
        "projectPathsRelativeToInstance": ["a"],
        "archivedProjectPaths": [null],
        "archivedProjectPathsRelativeToInstance": ["b"],
    });
    let before = object_of(value.clone());

    let _ = apply_project_decode_to_value(
        &mut value,
        Some(instance_base),
        &projects::FsCandidateResolver,
    );
    let after = object_of(value);

    let mutated = mutated_keys(&before, &after);
    let expected: BTreeSet<String> = OVERLAY_PREMIGRATION_UNSAFE_KEYS
        .iter()
        .map(|key| (*key).to_string())
        .collect();
    // Stated limit: this observes one fixture, not all inputs. It is the
    // tripwire that fires if the decode starts writing a key the D16
    // shortcut does not list; it is not a proof.
    assert_eq!(mutated, expected);
}

// S27
#[test]
fn the_load_time_chain_mutates_exactly_the_pinned_key_set() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");

    let mut fixture = object_of(serde_json::to_value(AppSettings::default()).unwrap());
    fixture.remove("rootToken");
    fixture.insert(
        FIELD_TERMINAL_SNAPSHOTS_ENABLED.to_string(),
        Value::Bool(false),
    );
    fixture.insert(
        "terminalGeometry".to_string(),
        json!({"x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0}),
    );
    fixture.insert("sidebarZoom".to_string(), json!(1.5));
    fixture.insert("sidebarAlwaysOnTop".to_string(), json!(true));
    fixture.insert("startOnlyCoordinators".to_string(), json!(true));
    // #2190: pinned explicitly so the legacy carrier still flips the
    // destination (false -> true) whatever the struct default is.
    fixture.insert("restoreCoordinatorWakeState".to_string(), json!(false));
    fixture.insert(
        "agents".to_string(),
        json!([{"id": "codex", "label": "Codex", "command": "codex", "color": "#000000"}]),
    );
    fixture.insert(
        "codingAgentProfiles".to_string(),
        json!({"schemaVersion": 2, "profileSlots": {}, "profilesByAgent": {}}),
    );
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&Value::Object(fixture.clone())).unwrap(),
    )
    .unwrap();

    let loaded = load_settings_from_path(&path);
    let after = object_of(serde_json::to_value(&loaded).unwrap());

    let mut mutated = mutated_keys(&fixture, &after);
    for key in [
        FIELD_PROJECT_PATH,
        FIELD_PROJECT_PATH_REL,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    ] {
        mutated.remove(key);
    }

    let expected: BTreeSet<String> = [
        "agents",
        "codingAgentProfiles",
        "mainAlwaysOnTop",
        "mainGeometry",
        "mainZoom",
        "restoreCoordinatorWakeState",
        "rootToken",
        "startOnlyCoordinators",
    ]
    .iter()
    .map(|key| (*key).to_string())
    .collect();
    assert_eq!(mutated, expected);
}

// S28
#[test]
fn the_save_time_repair_chain_mutates_exactly_the_pinned_key_set() {
    let mut settings = AppSettings {
        agents: vec![serde_json::from_value::<AgentConfig>(agent_json("codex")).unwrap()],
        api_server_bind: "  127.0.0.1  ".to_string(),
        ..AppSettings::default()
    };
    // Dirty `agents`: a container image with surrounding whitespace is what
    // `normalize_agent_backend_configs` rewrites.
    settings.agents[0].backend.kind = SessionBackendKind::ContainerTransport;
    settings.agents[0].backend.image = Some("  example/image:tag  ".to_string());
    settings.coding_agent_profiles.schema_version = 1;
    let before = object_of(serde_json::to_value(&settings).unwrap());

    validate_and_repair_settings(&mut settings).unwrap();
    let after = object_of(serde_json::to_value(&settings).unwrap());

    let expected: BTreeSet<String> = ["agents", "codingAgentProfiles", "apiServerBind"]
        .iter()
        .map(|key| (*key).to_string())
        .collect();
    assert_eq!(mutated_keys(&before, &after), expected);
}

// S29
#[test]
fn the_migration_destination_table_is_pinned_to_serialized_field_names() {
    assert_eq!(
        OVERLAY_MIGRATION_DESTINATION_KEYS,
        &[
            "agents",
            "mainAlwaysOnTop",
            "mainGeometry",
            "mainZoom",
            "restoreCoordinatorWakeState",
        ]
    );
    assert_eq!(OVERLAY_MIGRATION_DESTINATION_KEYS[0], OVERLAY_KEY_AGENTS);
    assert_eq!(
        OVERLAY_MIGRATION_DESTINATION_KEYS[1],
        OVERLAY_KEY_MAIN_ALWAYS_ON_TOP
    );
    assert_eq!(
        OVERLAY_MIGRATION_DESTINATION_KEYS[2],
        OVERLAY_KEY_MAIN_GEOMETRY
    );
    assert_eq!(OVERLAY_MIGRATION_DESTINATION_KEYS[3], OVERLAY_KEY_MAIN_ZOOM);
    assert_eq!(
        OVERLAY_MIGRATION_DESTINATION_KEYS[4],
        OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE
    );

    let settings = AppSettings {
        main_geometry: Some(WindowGeometry {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }),
        restore_coordinator_wake_state: true,
        ..AppSettings::default()
    };
    let object = object_of(serde_json::to_value(&settings).unwrap());
    for key in OVERLAY_MIGRATION_DESTINATION_KEYS {
        assert!(
            object.contains_key(*key),
            "{key} is not a serialized AppSettings field"
        );
    }
}

// #1757 (D11/D12) - an overlay that owns `agents` suppresses the migration.
#[test]
fn an_overlay_owned_agents_array_suppresses_the_1757_migration() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([codex_agent_with(json!([folder_trust_entry()]))]);
    let path = seed(
        temp.path(),
        Some(&base),
        // The overlay's array MUST be the non-empty one-entry shape: that is
        // precisely what the migration would act on if the guard were absent.
        Some(&json!({"agents": [codex_agent_with(json!([folder_trust_entry()]))]})),
    );

    let settings = load_settings_from_path(&path);
    assert!(
        settings
            .local_overlay_state
            .owns_top_level(OVERLAY_KEY_AGENTS),
        "the fixture stopped producing an overlay-owned agents array"
    );
    // #1905: the overlay-owned array is not exported (one info! per agent), so
    // no blocking-menus `.local` file appears and layer 0 keeps serving the
    // overlay's one-entry array rather than the shipped two.
    assert!(!blocking_menus_local_path(&path).exists());
    assert_eq!(
        BlockingMenusStore::load_from_settings_path(&path)
            .resolve_for(&settings.agents[0])
            .len(),
        1
    );

    fn in_memory(settings: &AppSettings) -> usize {
        settings
            .agents
            .iter()
            .find(|a| a.id == "codex")
            .expect("codex agent")
            .blocking_menus
            .as_ref()
            .expect("blocking_menus present")
            .len()
    }
    fn on_disk(path: &Path) -> usize {
        let object = disk_object(path);
        object["agents"][0]["blockingMenus"]
            .as_array()
            .expect("blockingMenus array")
            .len()
    }

    // Deleting the owns_top_level(OVERLAY_KEY_AGENTS) early return makes this
    // in-memory number 2. The on-disk number is 1 either way, because
    // restore_base writes the captured base array back on every save.
    assert_eq!(
        in_memory(&settings),
        1,
        "the migration must not touch an overlay-owned agents array in memory"
    );
    assert_eq!(on_disk(&path), 1, "the base file must stay clean");

    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    assert_eq!(in_memory(&settings), 1);
    assert_eq!(on_disk(&path), 1, "restore_base must keep the base array");
}

// S30
#[test]
fn the_default_arms_render_their_diagnostics_before_returning_defaults() {
    use crate::config::local_overlay::OverlayDiagnostic;

    let cases: [(&str, Option<&str>); 4] = [
        ("invalid-json", Some("{ not json")),
        ("top-level-array", Some("[1, 2]")),
        ("unreadable", None), // a directory at the overlay path
        (
            "all-ineligible",
            Some(r#"{"rootToken": "x", "projectPaths": []}"#),
        ),
    ];

    for (label, bytes) in cases {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let local = temp.path().join("settings.50.personal.no-git.json");
        match bytes {
            Some(bytes) => std::fs::write(&local, bytes).unwrap(),
            None => std::fs::create_dir(&local).unwrap(),
        }

        let settings = default_settings_with_overlay(&path, "test");
        let control = AppSettings::default();
        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            serde_json::to_value(&control).unwrap(),
            "{label}: the value returned must equal AppSettings::default()"
        );

        // The diagnostics the arm renders, observed on the overlay it loaded.
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        let overlay = LocalSettingsOverlay::load_and_merge_paired(
            &path,
            &mut value,
            OVERLAY_INELIGIBLE_DISK_KEYS,
            OVERLAY_INELIGIBLE_LEGACY_KEYS,
            OVERLAY_DERIVED_ID_CLOSURES,
            CONFIGURATION_COMMAND_PAIRS,
        );
        let records = overlay.diagnostics("test");
        assert!(!records.is_empty(), "{label}: expected a diagnostic");
        if label == "all-ineligible" {
            assert!(records
                .iter()
                .all(|r| matches!(r, OverlayDiagnostic::IneligibleKeyDropped { .. })));
        } else {
            assert!(records
                .iter()
                .any(|r| matches!(r, OverlayDiagnostic::Rejected { .. })));
        }

        // The same arm through the real loader, with an absent base.
        let loaded = load_settings_from_path(&path);
        assert!(
            loaded.local_overlay_state.is_empty(),
            "{label}: no overlay may be owned"
        );
    }

    // The control: no local file at all renders nothing.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    let overlay = LocalSettingsOverlay::load_and_merge_paired(
        &path,
        &mut value,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    assert!(overlay.diagnostics("test").is_empty());
}

// S31
#[test]
fn owning_a_migration_destination_suppresses_the_migration_in_memory() {
    let geometry = json!({"x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0});

    // R2: sidebarZoom 1.5 -> mainZoom, overridden to the default value 1.0.
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["sidebarZoom"] = json!(1.5);
    let path = seed(temp.path(), Some(&base), Some(&json!({"mainZoom": 1.0})));
    assert_eq!(load_settings_from_path(&path).main_zoom, 1.0);
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    assert_eq!(load_settings_from_path(&control).main_zoom, 1.5);

    // R3: sidebarAlwaysOnTop true -> mainAlwaysOnTop, overridden to false.
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["sidebarAlwaysOnTop"] = json!(true);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"mainAlwaysOnTop": false})),
    );
    assert!(!load_settings_from_path(&path).main_always_on_top);
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    assert!(load_settings_from_path(&control).main_always_on_top);

    // R1: terminalGeometry -> mainGeometry, overridden to an explicit null.
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["terminalGeometry"] = geometry.clone();
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"mainGeometry": null})),
    );
    assert!(load_settings_from_path(&path).main_geometry.is_none());
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    assert!(load_settings_from_path(&control).main_geometry.is_some());

    // R4: startOnlyCoordinators true -> restoreCoordinatorWakeState, false.
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["startOnlyCoordinators"] = json!(true);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"restoreCoordinatorWakeState": false})),
    );
    assert!(!load_settings_from_path(&path).restore_coordinator_wake_state);
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    assert!(load_settings_from_path(&control).restore_coordinator_wake_state);
}

// S32
#[test]
fn the_248_legacy_carrier_survives_while_the_overlay_owns_the_destination() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["startOnlyCoordinators"] = json!(true);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"restoreCoordinatorWakeState": false})),
    );

    let settings = load_settings_from_path(&path);
    assert!(!settings.restore_coordinator_wake_state);
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(
        disk_object(&path).get("startOnlyCoordinators"),
        Some(&json!(true)),
        "the legacy carrier is deferred, not dropped"
    );

    std::fs::remove_file(temp.path().join("settings.50.personal.no-git.json")).unwrap();
    let reloaded = load_settings_from_path(&path);
    assert!(reloaded.restore_coordinator_wake_state);
    assert!(
        disk_object(&path).get("startOnlyCoordinators").is_none(),
        "removing the overlay lets the migration finish"
    );

    // The control: with no overlay the key is removed on the first save.
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    let settings = load_settings_from_path(&control);
    assert!(settings.restore_coordinator_wake_state);
    assert!(disk_object(&control).get("startOnlyCoordinators").is_none());
}

// S33
#[test]
fn a_skip_serializing_owned_key_keeps_its_override_across_every_save() {
    let geometry = json!({"x": 100.0, "y": 100.0, "width": 800.0, "height": 600.0});
    let temp = tempfile::tempdir().unwrap();
    let mut base = watcher_base();
    base["watchersGeometry"] = geometry.clone();
    base["watchers"]["a"]["commands"] = json!(["claude"]);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"watchersGeometry": null, "watchers": {"a": {"commands": null}}})),
    );

    let mut settings = load_settings_from_path(&path);
    assert!(
        settings.watchers_geometry.is_none(),
        "the operator's intent"
    );
    assert!(settings.watchers["a"].valid().unwrap().commands.is_none());

    settings.gemini_api_key = "unrelated-change".to_string();
    let written = save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let object = disk_object(&path);
    assert_eq!(object["watchersGeometry"], geometry);
    assert_eq!(object["watchers"]["a"]["commands"], json!(["claude"]));
    assert_eq!(object["geminiApiKey"], json!("unrelated-change"));
    assert!(
        written.watchers_geometry.is_none(),
        "the value the caller adopts must still be the override"
    );
    assert!(written.watchers["a"].valid().unwrap().commands.is_none());

    // A second save of the returned value must not resurrect the base value.
    let again = save_settings_to_path_preserving_project_paths(&written, &path).unwrap();
    assert!(again.watchers_geometry.is_none());
    assert_eq!(disk_object(&path)["watchersGeometry"], geometry);

    // The control: with no local file the same load-and-save keeps the base.
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    let mut settings = load_settings_from_path(&control);
    settings.gemini_api_key = "unrelated-change".to_string();
    let written = save_settings_to_path_preserving_project_paths(&settings, &control).unwrap();
    assert!(written.watchers_geometry.is_some());
}

// S34
#[test]
fn the_cas_early_return_honours_the_removal_rule_too() {
    let geometry = json!({"x": 100.0, "y": 100.0, "width": 800.0, "height": 600.0});
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["watchersGeometry"] = geometry;
    base["terminalSnapshotsEnabled"] = json!(false);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"watchersGeometry": null})),
    );

    let current = load_settings_from_path(&path);
    assert!(current.watchers_geometry.is_none());

    let candidate =
        compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, false).unwrap();
    assert!(
        candidate.watchers_geometry.is_none(),
        "the disk-decoded candidate must not hand back the base geometry"
    );
}
