use super::*;

// ── #2348 - whole-settings writers inherit central placement preservation ──

fn placement_geometry(x: f64, y: f64) -> WindowGeometry {
    WindowGeometry {
        x,
        y,
        width: 800.0,
        height: 600.0,
    }
}

/// Seed a `settings.json` with the placement pair present, absent, or both,
/// so a Preserve save has (or lacks) disk truth to copy.
fn write_placement_settings_file(
    dir: &Path,
    geometry: Option<(f64, f64)>,
    state: Option<MainWindowDisplayState>,
) {
    let mut value = serde_json::to_value(settings_with_single_agent()).unwrap();
    let object = value.as_object_mut().unwrap();
    match geometry {
        Some((x, y)) => {
            object.insert(
                "mainGeometry".to_string(),
                json!({ "x": x, "y": y, "width": 800.0, "height": 600.0 }),
            );
        }
        None => {
            object.remove("mainGeometry");
        }
    }
    match state {
        Some(state) => {
            object.insert(
                "mainWindowDisplayState".to_string(),
                serde_json::to_value(state).unwrap(),
            );
        }
        None => {
            object.remove("mainWindowDisplayState");
        }
    }
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
}

fn assert_disk_placement(dir: &Path, expected_state: MainWindowDisplayState, expected_x: f64) {
    let raw = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    let disk: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        disk["mainWindowDisplayState"],
        serde_json::to_value(expected_state).unwrap()
    );
    assert_eq!(disk["mainGeometry"]["x"], json!(expected_x));
}

#[tokio::test]
async fn issue_2348_settings_update_cannot_regress_the_disk_placement_pair() {
    for (label, disk_state, stale_state) in [
        (
            "maximized disk, normal stale",
            MainWindowDisplayState::Maximized,
            MainWindowDisplayState::Normal,
        ),
        (
            "normal disk, maximized stale",
            MainWindowDisplayState::Normal,
            MainWindowDisplayState::Maximized,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        write_placement_settings_file(temp.path(), Some((1.0, 2.0)), Some(disk_state));

        let mut current = settings_with_single_agent();
        current.main_geometry = Some(placement_geometry(1.0, 2.0));
        current.main_window_display_state = disk_state;
        let state = state_for(current.clone());

        let mut stale = current.clone();
        stale.main_geometry = Some(placement_geometry(999.0, 999.0));
        stale.main_window_display_state = stale_state;

        let (saved, _) = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        assert_eq!(saved.main_window_display_state, disk_state, "{label}");
        assert_eq!(
            saved.main_geometry.as_ref().map(|g| (g.x, g.y)),
            Some((1.0, 2.0)),
            "{label}"
        );
        assert_disk_placement(temp.path(), disk_state, 1.0);
    }
}

#[tokio::test]
async fn issue_2348_settings_draft_cannot_regress_the_disk_placement_pair() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    write_placement_settings_file(
        temp.path(),
        Some((1.0, 2.0)),
        Some(MainWindowDisplayState::Maximized),
    );

    let mut current = settings_with_single_agent();
    current.main_geometry = Some(placement_geometry(1.0, 2.0));
    current.main_window_display_state = MainWindowDisplayState::Maximized;
    let state = state_for(current.clone());

    let mut draft = current.clone();
    draft.main_geometry = Some(placement_geometry(999.0, 999.0));
    draft.main_window_display_state = MainWindowDisplayState::Normal;

    let (saved, _events) = persist_settings_draft_update_with_saver(&state, draft, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(
            candidate,
            &settings_path,
        )
    })
    .await
    .unwrap();

    assert_eq!(
        saved.main_window_display_state,
        MainWindowDisplayState::Maximized
    );
    assert_eq!(saved.main_geometry.as_ref().map(|g| g.x), Some(1.0));
    assert_disk_placement(temp.path(), MainWindowDisplayState::Maximized, 1.0);
}

#[tokio::test]
async fn issue_2348_cli_style_whole_settings_payload_cannot_regress_the_disk_placement_pair() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    write_placement_settings_file(
        temp.path(),
        Some((1.0, 2.0)),
        Some(MainWindowDisplayState::Maximized),
    );

    let mut stale = settings_with_single_agent();
    stale.main_geometry = Some(placement_geometry(999.0, 999.0));
    stale.main_window_display_state = MainWindowDisplayState::Normal;

    crate::config::settings::save_settings_with_project_paths_to_path(&stale, &settings_path)
        .unwrap();

    assert_disk_placement(temp.path(), MainWindowDisplayState::Maximized, 1.0);
}

#[tokio::test]
async fn issue_2348_absent_disk_placement_keys_keep_the_caller_values() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    write_placement_settings_file(temp.path(), None, None);

    let mut candidate = settings_with_single_agent();
    candidate.main_geometry = Some(placement_geometry(11.0, 22.0));
    candidate.main_window_display_state = MainWindowDisplayState::Normal;

    let written = crate::config::settings::save_settings_to_path_preserving_project_paths(
        &candidate,
        &settings_path,
    )
    .unwrap();

    assert_eq!(written.main_geometry.as_ref().map(|g| g.x), Some(11.0));
    assert_eq!(
        written.main_window_display_state,
        MainWindowDisplayState::Normal
    );
    let raw = std::fs::read_to_string(&settings_path).unwrap();
    let disk: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(disk["mainGeometry"]["x"], json!(11.0));
    assert_eq!(disk["mainWindowDisplayState"], json!("normal"));
}
