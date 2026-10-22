use super::super::*;
use serde_json::json;

fn seed_json(path: &std::path::Path, value: &Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn disk_object(path: &std::path::Path) -> Map<String, Value> {
    let raw = std::fs::read_to_string(path).unwrap();
    serde_json::from_str::<Value>(&raw)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

#[test]
fn issue_2348_display_state_serde_is_lowercase_and_defaults_to_normal() {
    assert_eq!(
        MainWindowDisplayState::default(),
        MainWindowDisplayState::Normal
    );
    assert_eq!(
        serde_json::to_value(MainWindowDisplayState::Normal).unwrap(),
        json!("normal")
    );
    assert_eq!(
        serde_json::to_value(MainWindowDisplayState::Maximized).unwrap(),
        json!("maximized")
    );
    assert_eq!(
        serde_json::from_value::<MainWindowDisplayState>(json!("maximized")).unwrap(),
        MainWindowDisplayState::Maximized
    );
    // Minimized and fullscreen must stay unrepresentable.
    assert!(serde_json::from_value::<MainWindowDisplayState>(json!("minimized")).is_err());
    assert!(serde_json::from_value::<MainWindowDisplayState>(json!("fullscreen")).is_err());
}

#[test]
fn issue_2348_absent_field_loads_as_normal_and_a_save_round_trips_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    seed_json(
        &path,
        &json!({
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [],
            "rootToken": "fixed-token",
        }),
    );

    let loaded = load_settings_from_path(&path);
    assert_eq!(
        loaded.main_window_display_state,
        MainWindowDisplayState::Normal
    );
    assert!(loaded.main_geometry.is_none());

    let written = save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    assert_eq!(
        written.main_window_display_state,
        MainWindowDisplayState::Normal
    );
    let disk = disk_object(&path);
    assert_eq!(disk[FIELD_MAIN_DISPLAY_STATE], json!("normal"));
    assert_eq!(disk[FIELD_MAIN_GEOMETRY], Value::Null);
}

#[test]
fn issue_2348_preserve_copies_only_present_disk_keys_independently_for_both_keys() {
    for key in [FIELD_MAIN_GEOMETRY, FIELD_MAIN_DISPLAY_STATE] {
        let caller = json!("caller");
        let disk_value = json!("disk");

        // disk present / caller present -> disk replaces caller.
        let mut out = Map::from_iter([(key.to_string(), caller.clone())]);
        let disk = Map::from_iter([(key.to_string(), disk_value.clone())]);
        preserve_main_window_placement_keys(&mut out, &disk);
        assert_eq!(out.get(key), Some(&disk_value), "{key} disk/caller");

        // disk present / caller absent -> disk is inserted.
        let mut out = Map::new();
        let disk = Map::from_iter([(key.to_string(), disk_value.clone())]);
        preserve_main_window_placement_keys(&mut out, &disk);
        assert_eq!(out.get(key), Some(&disk_value), "{key} disk only");

        // disk absent / caller present -> caller value remains.
        let mut out = Map::from_iter([(key.to_string(), caller.clone())]);
        preserve_main_window_placement_keys(&mut out, &Map::new());
        assert_eq!(out.get(key), Some(&caller), "{key} caller only");

        // disk absent / caller absent -> key remains absent.
        let mut out = Map::new();
        preserve_main_window_placement_keys(&mut out, &Map::new());
        assert!(!out.contains_key(key), "{key} neither side");

        // An explicit null carries no placement truth: it must not erase
        // the caller's value (the load-time migration regression guard).
        let mut out = Map::from_iter([(key.to_string(), caller.clone())]);
        let disk = Map::from_iter([(key.to_string(), Value::Null)]);
        preserve_main_window_placement_keys(&mut out, &disk);
        assert_eq!(out.get(key), Some(&caller), "{key} null disk");
    }
}

#[test]
fn issue_2348_legacy_terminal_geometry_migration_survives_an_unrelated_save() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    seed_json(
        &path,
        &json!({
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [],
            "rootToken": "legacy-token",
            "terminalGeometry": { "x": 11.0, "y": 22.0, "width": 333.0, "height": 444.0 },
        }),
    );

    let loaded = load_settings_from_path(&path);
    let migrated = loaded
        .main_geometry
        .as_ref()
        .expect("the loader must seed mainGeometry from legacy terminalGeometry");
    assert_eq!((migrated.x, migrated.y), (11.0, 22.0));
    assert_eq!((migrated.width, migrated.height), (333.0, 444.0));
    assert_eq!(
        loaded.main_window_display_state,
        MainWindowDisplayState::Normal
    );

    // An otherwise-unrelated whole-settings save must not drop the value the
    // load-time migration synthesized into memory: the disk file has no
    // mainGeometry, so the present-only Preserve rule must leave the caller's.
    let written = save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    let written_geometry = written
        .main_geometry
        .as_ref()
        .expect("the re-decoded settings must still carry the migrated geometry");
    assert_eq!((written_geometry.x, written_geometry.y), (11.0, 22.0));
    let disk = disk_object(&path);
    assert_eq!(disk[FIELD_MAIN_GEOMETRY]["width"], json!(333.0));
    assert_eq!(disk[FIELD_MAIN_DISPLAY_STATE], json!("normal"));
}

#[test]
fn issue_2348_overlay_owned_placement_keys_keep_the_base_values_on_a_save() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    seed_json(
        &path,
        &json!({
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [],
            "rootToken": "base-token",
            "mainGeometry": { "x": 1.0, "y": 2.0, "width": 300.0, "height": 400.0 },
            "mainWindowDisplayState": "maximized",
        }),
    );
    seed_json(
        &temp.path().join("settings.50.personal.no-git.json"),
        &json!({
            "mainGeometry": { "x": 9.0, "y": 8.0, "width": 700.0, "height": 600.0 },
            "mainWindowDisplayState": "normal",
        }),
    );

    let loaded = load_settings_from_path(&path);
    assert!(loaded
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY));
    assert!(loaded
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_DISPLAY_STATE));
    assert_eq!(
        loaded.main_window_display_state,
        MainWindowDisplayState::Normal
    );

    let written = save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    // Base file keeps the base values; memory keeps the overlay's effective values.
    let disk = disk_object(&path);
    assert_eq!(disk[FIELD_MAIN_GEOMETRY]["x"], json!(1.0));
    assert_eq!(disk[FIELD_MAIN_DISPLAY_STATE], json!("maximized"));
    assert_eq!(
        written.main_window_display_state,
        MainWindowDisplayState::Normal
    );
    let effective = written.main_geometry.as_ref().unwrap();
    assert_eq!((effective.x, effective.y), (9.0, 8.0));
}
