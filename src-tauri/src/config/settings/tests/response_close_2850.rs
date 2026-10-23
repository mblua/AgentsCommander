use super::local_overlay_1737::{base_fixture, seed};
use super::*;

#[test]
fn publication_state_reports_bool_and_holds_lock_through_callback() {
    let dir = Path::new("publication-state-test");
    let registry = std::sync::Mutex::new(HashSet::new());
    for unreadable in [false, true] {
        if unreadable {
            registry.lock().unwrap().insert(dir.to_path_buf());
        }
        let returned = try_with_agents_layer_publication_state_in(&registry, dir, |actual| {
            assert_eq!(actual, unreadable);
            assert!(matches!(
                registry.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ));
            42
        })
        .unwrap();
        assert_eq!(returned, 42);
        assert!(registry.try_lock().is_ok());
    }
}

#[test]
fn publication_state_try_uses_existing_registry_without_calling_on_contention() {
    let _guard = agents_unreadable_dirs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let called = std::cell::Cell::new(false);
    let result =
        try_with_agents_layer_publication_state(Path::new("publication-contention-test"), |_| {
            called.set(true)
        });
    assert_eq!(result, Err(AgentsPublicationLockError::WouldBlock));
    assert!(!called.get());
}

#[test]
fn publication_state_poison_refuses_callback_without_recovering_registry() {
    let registry = std::sync::Mutex::new(HashSet::new());
    let panic = std::panic::catch_unwind(|| {
        let _guard = registry.lock().unwrap();
        panic!("poison isolated publication registry");
    });
    assert!(panic.is_err());
    let called = std::cell::Cell::new(false);
    let result = try_with_agents_layer_publication_state_in(
        &registry,
        Path::new("publication-poison-test"),
        |_| called.set(true),
    );
    assert_eq!(result, Err(AgentsPublicationLockError::Poisoned));
    assert!(!called.get());
    assert!(registry.is_poisoned());
}

// Capture every disk artifact, including migration/rotation backups.
// The existing writer may create its persistent lock file.
fn disk_bytes(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_none_or(|ext| ext != "lock"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect()
}

fn assert_repaired(settings: &AppSettings) {
    assert!(!settings.response_close_enabled);
    assert_eq!(settings.response_close_idle_seconds, 30);
    assert_eq!(settings.typing_hold_seconds, 7);
}

fn overlay(seconds: u32) -> Value {
    serde_json::json!({
        "responseCloseEnabled": false,
        "responseCloseIdleSeconds": seconds,
        "typingHoldSeconds": 7
    })
}

#[test]
fn legacy_defaults_and_boundary_preferences_persist_and_reload() {
    let defaults = AppSettings::default();
    assert!(defaults.response_close_enabled);
    assert_eq!(defaults.response_close_idle_seconds, 30);
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path(), Some(&base_fixture()), None);
    let before = disk_bytes(temp.path());
    let legacy = load_settings_for_cli_from_path(&path);
    assert!(legacy.response_close_enabled);
    assert_eq!(legacy.response_close_idle_seconds, 30);
    assert_eq!(disk_bytes(temp.path()), before);
    for enabled in [true, false] {
        for seconds in [1, 30, 3600] {
            let mut candidate = legacy.clone();
            candidate.response_close_enabled = enabled;
            candidate.response_close_idle_seconds = seconds;
            validate_and_repair_settings(&mut candidate).unwrap();
            let written =
                save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();
            let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(raw["responseCloseEnabled"], enabled);
            assert_eq!(raw["responseCloseIdleSeconds"], seconds);
            assert!(raw.get("response_close_enabled").is_none());
            let reloaded = load_settings_from_path(&path);
            for settings in [written, reloaded] {
                assert_eq!(settings.response_close_enabled, enabled);
                assert_eq!(settings.response_close_idle_seconds, seconds);
            }
        }
    }
}

#[test]
fn f1_absent_base_cli_overlay_repairs_without_any_write() {
    for seconds in [0, 999999] {
        let temp = tempfile::tempdir().unwrap();
        let path = seed(temp.path(), None, Some(&overlay(seconds)));
        let before = disk_bytes(temp.path());
        for settings in [
            load_settings_for_cli_from_path(&path),
            load_settings_for_cli_strict_from_path(Some(&path)).unwrap(),
        ] {
            assert_repaired(&settings);
            assert!(settings.root_token.is_none());
            for key in [
                "responseCloseEnabled",
                "responseCloseIdleSeconds",
                "typingHoldSeconds",
            ] {
                assert!(settings.local_overlay_state.owns_top_level(key));
            }
        }
        assert!(!path.exists());
        assert_eq!(disk_bytes(temp.path()), before);
    }
}

#[test]
fn f2_gui_malformed_base_repairs_overlay_without_authorization_then_recovers() {
    for seconds in [0, 999999] {
        let temp = tempfile::tempdir().unwrap();
        let path = seed(temp.path(), None, Some(&overlay(seconds)));
        std::fs::write(&path, b"{malformed-base").unwrap();
        let before = disk_bytes(temp.path());
        let (records, _capture) = super::capture_settings_save_diagnostics();
        let mut settings = load_settings_from_path(&path);
        assert_repaired(&settings);
        assert!(settings.root_token.is_none());
        assert!(settings.project_paths_persistence_error.is_some());
        assert!(settings.project_paths.is_empty());
        assert!(settings.project_path.is_none());
        assert!(settings
            .local_overlay_state
            .owns_top_level("responseCloseIdleSeconds"));
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        let fields = super::parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["stage"], "project_paths_refresh_disk_gate");
        drop(records);
        assert_eq!(disk_bytes(temp.path()), before);
        settings.sidebar_zoom = 1.25;
        validate_and_repair_settings(&mut settings).unwrap();
        let error =
            save_settings_to_path_preserving_project_paths_typed(&settings, &path).unwrap_err();
        assert_eq!(error.stage, SettingsSaveStage::PreserveDiskGate);
        assert_eq!(disk_bytes(temp.path()), before);
        std::fs::write(&path, serde_json::to_vec(&base_fixture()).unwrap()).unwrap();
        let recovered = load_settings_from_path(&path);
        assert_repaired(&recovered);
        assert!(recovered.project_paths_persistence_error.is_none());
        assert!(recovered.root_token.is_some());
    }
}

#[test]
fn f3_malformed_base_cli_fallback_and_strict_refusal_preserve_bytes() {
    for seconds in [0, 999999] {
        let temp = tempfile::tempdir().unwrap();
        let path = seed(temp.path(), None, Some(&overlay(seconds)));
        std::fs::write(&path, b"{malformed-base").unwrap();
        let before = disk_bytes(temp.path());
        let settings = load_settings_for_cli_from_path(&path);
        assert_repaired(&settings);
        assert!(settings.root_token.is_none());
        let error = load_settings_for_cli_strict_from_path(Some(&path)).unwrap_err();
        assert!(error.contains("settings.json exists but could not be parsed"));
        assert!(settings.project_paths_persistence_error.is_some());
        assert!(settings.project_paths.is_empty());
        assert_eq!(disk_bytes(temp.path()), before);
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::create_dir(&path).unwrap();
    assert!(load_settings_for_cli_strict_from_path(Some(&path)).is_err());
    assert!(path.is_dir());
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
}

#[test]
fn valid_base_disk_repair_and_cas_no_write_preserve_false_and_bytes() {
    for seconds in [0, 999999] {
        for with_overlay in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut base = base_fixture();
            base["responseCloseEnabled"] = Value::Bool(false);
            base["responseCloseIdleSeconds"] = Value::from(seconds);
            base["typingHoldSeconds"] = Value::from(7);
            let local = overlay(seconds);
            let path = seed(temp.path(), Some(&base), with_overlay.then_some(&local));
            // Establish the current split-file schema before simulating
            // an external seconds edit, so Reconcile has nothing to migrate.
            let initial = load_settings_for_cli_from_path(&path);
            save_settings_to_path_preserving_project_paths(&initial, &path).unwrap();
            let mut raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            raw["responseCloseIdleSeconds"] = Value::from(seconds);
            std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
            let before = disk_bytes(temp.path());
            let settings = load_settings_from_path(&path);
            assert_repaired(&settings);
            assert_repaired(&load_settings_for_cli_from_path(&path));
            assert_repaired(&load_settings_for_cli_strict_from_path(Some(&path)).unwrap());
            let cas =
                compare_and_set_terminal_snapshots_enabled_at_path(&settings, &path, true, true)
                    .unwrap();
            assert_repaired(&cas);
            assert_eq!(disk_bytes(temp.path()), before);

            // Reconcile keeps unrelated disk fields, then re-decodes them.
            let reconciled = save_settings_value(
                &settings,
                &path,
                ProjectWriteMode::Reconcile {
                    active: true,
                    archived: true,
                },
            )
            .unwrap();
            assert_repaired(&reconciled);
            let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(raw["responseCloseIdleSeconds"], seconds);
            assert_eq!(disk_bytes(temp.path()), before);
            if with_overlay {
                assert_eq!(
                    std::fs::read(temp.path().join("settings.50.personal.no-git.json")).unwrap(),
                    before["settings.50.personal.no-git.json"]
                );
            }
            let mut unrelated = reconciled;
            unrelated.sidebar_zoom = 1.25;
            let agent = serde_json::from_value(serde_json::json!({
                "id": "response-test", "label": "Response test",
                "command": "codex", "color": "#000000"
            }))
            .unwrap();
            crate::config::coding_agent_mutations::apply_coding_agent_op(
                &mut unrelated,
                &crate::config::coding_agent_mutations::CodingAgentOp::Add { agent },
            )
            .unwrap();
            let written =
                save_settings_to_path_preserving_project_paths(&unrelated, &path).unwrap();
            assert_repaired(&written);
            assert_eq!(written.sidebar_zoom, 1.25);
            assert_eq!(written.agents[0].id, "response-test");
            assert_repaired(&load_settings_from_path(&path));
        }
    }
}

#[test]
fn malformed_overlay_falls_back_without_changing_disk() {
    for local in [
        "{broken",
        r#"{"responseCloseIdleSeconds":"30"}"#,
        r#"{"responseCloseEnabled":1}"#,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut base = base_fixture();
        base["responseCloseEnabled"] = Value::Bool(false);
        base["responseCloseIdleSeconds"] = Value::from(3600);
        let path = seed(temp.path(), Some(&base), None);
        let current = load_settings_for_cli_from_path(&path);
        save_settings_to_path_preserving_project_paths(&current, &path).unwrap();
        std::fs::write(temp.path().join("settings.50.personal.no-git.json"), local).unwrap();
        let before = disk_bytes(temp.path());
        let settings = load_settings_from_path(&path);
        assert!(!settings.response_close_enabled);
        assert_eq!(settings.response_close_idle_seconds, 3600);
        assert_eq!(disk_bytes(temp.path()), before);
    }
}

#[tokio::test]
async fn invalid_explicit_updates_reject_before_either_writer_and_keep_live_and_disk() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path(), Some(&base_fixture()), None);
    let original = load_settings_from_path(&path);
    let state: SettingsState = crate::config::settings::new_settings_state(original.clone());
    let before = disk_bytes(temp.path());
    for seconds in [0, 3601, 999999] {
        let mut invalid = original.clone();
        invalid.response_close_enabled = false;
        invalid.response_close_idle_seconds = seconds;
        for error in [
            crate::commands::config::persist_protected_settings_update(&state, invalid.clone())
                .await
                .unwrap_err(),
            crate::commands::config::persist_settings_draft_update(&state, invalid)
                .await
                .unwrap_err(),
        ] {
            assert!(error.contains("responseCloseIdleSeconds"));
        }
        assert_eq!(
            serde_json::to_value(&*state.read().await).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
        assert_eq!(disk_bytes(temp.path()), before);
    }
    for value in [
        Value::from("30"),
        Value::from(-1),
        serde_json::json!(1.5),
        Value::Null,
        Value::Bool(true),
        Value::from(u64::MAX),
    ] {
        let mut payload = serde_json::to_value(&original).unwrap();
        payload["responseCloseIdleSeconds"] = value;
        assert!(serde_json::from_value::<AppSettings>(payload).is_err());
    }
    let mut payload = serde_json::to_value(&original).unwrap();
    payload["responseCloseEnabled"] = Value::from("false");
    assert!(serde_json::from_value::<AppSettings>(payload).is_err());
    assert_eq!(disk_bytes(temp.path()), before);
}

#[test]
fn room_idle_confirmation_legacy_defaults_and_boundary_preferences_persist_and_reload() {
    let defaults = AppSettings::default();
    assert!(defaults.response_close_enabled);
    assert_eq!(defaults.room_idle_confirmation_seconds, 15);
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path(), Some(&base_fixture()), None);
    let before = disk_bytes(temp.path());
    let legacy = load_settings_for_cli_from_path(&path);
    assert!(legacy.response_close_enabled);
    assert_eq!(legacy.room_idle_confirmation_seconds, 15);
    assert_eq!(disk_bytes(temp.path()), before);
    for enabled in [true, false] {
        for seconds in [1, 15, 3600] {
            let mut candidate = legacy.clone();
            candidate.response_close_enabled = enabled;
            candidate.room_idle_confirmation_seconds = seconds;
            validate_and_repair_settings(&mut candidate).unwrap();
            let written =
                save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();
            let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(raw["responseCloseEnabled"], enabled);
            assert_eq!(raw["roomIdleConfirmationSeconds"], seconds);
            assert!(raw.get("room_idle_confirmation_seconds").is_none());
            let reloaded = load_settings_from_path(&path);
            for settings in [written, reloaded] {
                assert_eq!(settings.response_close_enabled, enabled);
                assert_eq!(settings.room_idle_confirmation_seconds, seconds);
            }
        }
    }
}

#[test]
fn room_idle_confirmation_f1_absent_base_cli_overlay_repairs_without_any_write() {
    let assert_repaired = |settings: &AppSettings| {
        assert!(!settings.response_close_enabled);
        assert_eq!(settings.room_idle_confirmation_seconds, 15);
        assert_eq!(settings.typing_hold_seconds, 7);
    };
    let overlay = |seconds: u32| {
        serde_json::json!({
            "responseCloseEnabled": false,
            "roomIdleConfirmationSeconds": seconds,
            "typingHoldSeconds": 7
        })
    };
    for seconds in [0, 3601, u32::MAX] {
        let temp = tempfile::tempdir().unwrap();
        let path = seed(temp.path(), None, Some(&overlay(seconds)));
        let before = disk_bytes(temp.path());
        for settings in [
            load_settings_for_cli_from_path(&path),
            load_settings_for_cli_strict_from_path(Some(&path)).unwrap(),
        ] {
            assert_repaired(&settings);
            assert!(settings.root_token.is_none());
            for key in [
                "responseCloseEnabled",
                "roomIdleConfirmationSeconds",
                "typingHoldSeconds",
            ] {
                assert!(settings.local_overlay_state.owns_top_level(key));
            }
        }
        assert!(!path.exists());
        assert_eq!(disk_bytes(temp.path()), before);
    }
}

#[test]
fn room_idle_confirmation_f2_gui_malformed_base_repairs_overlay_without_authorization_then_recovers(
) {
    let assert_repaired = |settings: &AppSettings| {
        assert!(!settings.response_close_enabled);
        assert_eq!(settings.room_idle_confirmation_seconds, 15);
        assert_eq!(settings.typing_hold_seconds, 7);
    };
    let overlay = |seconds: u32| {
        serde_json::json!({
            "responseCloseEnabled": false,
            "roomIdleConfirmationSeconds": seconds,
            "typingHoldSeconds": 7
        })
    };
    for seconds in [0, 3601, u32::MAX] {
        let temp = tempfile::tempdir().unwrap();
        let path = seed(temp.path(), None, Some(&overlay(seconds)));
        std::fs::write(&path, b"{malformed-base").unwrap();
        let before = disk_bytes(temp.path());
        let (records, _capture) = super::capture_settings_save_diagnostics();
        let mut settings = load_settings_from_path(&path);
        assert_repaired(&settings);
        assert!(settings.root_token.is_none());
        assert!(settings.project_paths_persistence_error.is_some());
        assert!(settings.project_paths.is_empty());
        assert!(settings.project_path.is_none());
        assert!(settings
            .local_overlay_state
            .owns_top_level("roomIdleConfirmationSeconds"));
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        let fields = super::parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["stage"], "project_paths_refresh_disk_gate");
        drop(records);
        assert_eq!(disk_bytes(temp.path()), before);
        settings.sidebar_zoom = 1.25;
        validate_and_repair_settings(&mut settings).unwrap();
        let error =
            save_settings_to_path_preserving_project_paths_typed(&settings, &path).unwrap_err();
        assert_eq!(error.stage, SettingsSaveStage::PreserveDiskGate);
        assert_eq!(disk_bytes(temp.path()), before);
        std::fs::write(&path, serde_json::to_vec(&base_fixture()).unwrap()).unwrap();
        let recovered = load_settings_from_path(&path);
        assert_repaired(&recovered);
        assert!(recovered.project_paths_persistence_error.is_none());
        assert!(recovered.root_token.is_some());
    }
}

#[test]
fn room_idle_confirmation_f3_malformed_base_cli_fallback_and_strict_refusal_preserve_bytes() {
    let assert_repaired = |settings: &AppSettings| {
        assert!(!settings.response_close_enabled);
        assert_eq!(settings.room_idle_confirmation_seconds, 15);
        assert_eq!(settings.typing_hold_seconds, 7);
    };
    let overlay = |seconds: u32| {
        serde_json::json!({
            "responseCloseEnabled": false,
            "roomIdleConfirmationSeconds": seconds,
            "typingHoldSeconds": 7
        })
    };
    for seconds in [0, 3601, u32::MAX] {
        let temp = tempfile::tempdir().unwrap();
        let path = seed(temp.path(), None, Some(&overlay(seconds)));
        std::fs::write(&path, b"{malformed-base").unwrap();
        let before = disk_bytes(temp.path());
        let settings = load_settings_for_cli_from_path(&path);
        assert_repaired(&settings);
        assert!(settings.root_token.is_none());
        let error = load_settings_for_cli_strict_from_path(Some(&path)).unwrap_err();
        assert!(error.contains("settings.json exists but could not be parsed"));
        assert!(settings.project_paths_persistence_error.is_some());
        assert!(settings.project_paths.is_empty());
        assert_eq!(disk_bytes(temp.path()), before);
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::create_dir(&path).unwrap();
    assert!(load_settings_for_cli_strict_from_path(Some(&path)).is_err());
    assert!(path.is_dir());
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
}

#[test]
fn room_idle_confirmation_valid_base_disk_repair_and_cas_no_write_preserve_false_and_bytes() {
    let assert_repaired = |settings: &AppSettings| {
        assert!(!settings.response_close_enabled);
        assert_eq!(settings.room_idle_confirmation_seconds, 15);
        assert_eq!(settings.typing_hold_seconds, 7);
    };
    let overlay = |seconds: u32| {
        serde_json::json!({
            "responseCloseEnabled": false,
            "roomIdleConfirmationSeconds": seconds,
            "typingHoldSeconds": 7
        })
    };
    for seconds in [0, 3601, u32::MAX] {
        for with_overlay in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut base = base_fixture();
            base["responseCloseEnabled"] = Value::Bool(false);
            base["roomIdleConfirmationSeconds"] = Value::from(seconds);
            base["typingHoldSeconds"] = Value::from(7);
            let local = overlay(seconds);
            let path = seed(temp.path(), Some(&base), with_overlay.then_some(&local));
            // Establish the current split-file schema before simulating
            // an external seconds edit, so Reconcile has nothing to migrate.
            let initial = load_settings_for_cli_from_path(&path);
            save_settings_to_path_preserving_project_paths(&initial, &path).unwrap();
            let mut raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            raw["roomIdleConfirmationSeconds"] = Value::from(seconds);
            std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
            let before = disk_bytes(temp.path());
            let settings = load_settings_from_path(&path);
            assert_repaired(&settings);
            assert_repaired(&load_settings_for_cli_from_path(&path));
            assert_repaired(&load_settings_for_cli_strict_from_path(Some(&path)).unwrap());
            let cas =
                compare_and_set_terminal_snapshots_enabled_at_path(&settings, &path, true, true)
                    .unwrap();
            assert_repaired(&cas);
            assert_eq!(disk_bytes(temp.path()), before);

            // Reconcile keeps unrelated disk fields, then re-decodes them.
            let reconciled = save_settings_value(
                &settings,
                &path,
                ProjectWriteMode::Reconcile {
                    active: true,
                    archived: true,
                },
            )
            .unwrap();
            assert_repaired(&reconciled);
            let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(raw["roomIdleConfirmationSeconds"], seconds);
            assert_eq!(disk_bytes(temp.path()), before);
            if with_overlay {
                assert_eq!(
                    std::fs::read(temp.path().join("settings.50.personal.no-git.json")).unwrap(),
                    before["settings.50.personal.no-git.json"]
                );
            }
            let mut unrelated = reconciled;
            unrelated.sidebar_zoom = 1.25;
            let agent = serde_json::from_value(serde_json::json!({
                "id": "response-test", "label": "Response test",
                "command": "codex", "color": "#000000"
            }))
            .unwrap();
            crate::config::coding_agent_mutations::apply_coding_agent_op(
                &mut unrelated,
                &crate::config::coding_agent_mutations::CodingAgentOp::Add { agent },
            )
            .unwrap();
            let written =
                save_settings_to_path_preserving_project_paths(&unrelated, &path).unwrap();
            assert_repaired(&written);
            assert_eq!(written.sidebar_zoom, 1.25);
            assert_eq!(written.agents[0].id, "response-test");
            assert_repaired(&load_settings_from_path(&path));
        }
    }
}

#[test]
fn room_idle_confirmation_malformed_overlay_falls_back_without_changing_disk() {
    for local in [
        "{broken",
        r#"{"roomIdleConfirmationSeconds":"15"}"#,
        r#"{"roomIdleConfirmationSeconds":null}"#,
        r#"{"roomIdleConfirmationSeconds":1.5}"#,
        r#"{"roomIdleConfirmationSeconds":-1}"#,
        r#"{"roomIdleConfirmationSeconds":4294967296}"#,
        "[]",
        "null",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut base = base_fixture();
        base["responseCloseEnabled"] = Value::Bool(false);
        base["roomIdleConfirmationSeconds"] = Value::from(3600);
        let path = seed(temp.path(), Some(&base), None);
        let current = load_settings_for_cli_from_path(&path);
        save_settings_to_path_preserving_project_paths(&current, &path).unwrap();
        std::fs::write(temp.path().join("settings.50.personal.no-git.json"), local).unwrap();
        let before = disk_bytes(temp.path());
        let settings = load_settings_from_path(&path);
        assert!(!settings.response_close_enabled);
        assert_eq!(settings.room_idle_confirmation_seconds, 3600);
        assert_eq!(disk_bytes(temp.path()), before);
    }
}

#[tokio::test]
async fn room_idle_invalid_explicit_updates_reject_before_writers_keep_live_and_disk() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path(), Some(&base_fixture()), None);
    let original = load_settings_from_path(&path);
    let state: SettingsState = crate::config::settings::new_settings_state(original.clone());
    let before = disk_bytes(temp.path());
    for seconds in [0, 3601, u32::MAX] {
        let mut invalid = original.clone();
        invalid.response_close_enabled = false;
        invalid.room_idle_confirmation_seconds = seconds;
        // The whole writer now returns its effective-change receipt on success.
        let protected: Result<(AppSettings, bool), String> =
            crate::commands::config::persist_protected_settings_update(&state, invalid.clone())
                .await;
        for error in [
            protected.unwrap_err(),
            crate::commands::config::persist_settings_draft_update(&state, invalid)
                .await
                .unwrap_err(),
        ] {
            assert!(error.contains("roomIdleConfirmationSeconds"));
        }
        assert_eq!(
            serde_json::to_value(&*state.read().await).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
        assert_eq!(disk_bytes(temp.path()), before);
    }
    for value in [
        Value::from("30"),
        Value::from(-1),
        serde_json::json!(1.5),
        Value::Null,
        Value::Bool(true),
        Value::from(u64::MAX),
    ] {
        let mut payload = serde_json::to_value(&original).unwrap();
        payload["roomIdleConfirmationSeconds"] = value;
        assert!(serde_json::from_value::<AppSettings>(payload).is_err());
    }
    let mut payload = serde_json::to_value(&original).unwrap();
    payload["responseCloseEnabled"] = Value::from("false");
    assert!(serde_json::from_value::<AppSettings>(payload).is_err());
    assert_eq!(disk_bytes(temp.path()), before);
}

#[test]
fn room_idle_confirmation_owner_composes_repaired_and_overlay_values_without_writes() {
    for seconds in [0, 1, 15, 3600, 3601, u32::MAX] {
        for with_overlay in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut base = base_fixture();
            base["roomIdleConfirmationSeconds"] =
                Value::from(if with_overlay { 123 } else { seconds });
            base["typingHoldSeconds"] = Value::from(7);
            let local = serde_json::json!({"roomIdleConfirmationSeconds": seconds});
            let path = seed(temp.path(), Some(&base), with_overlay.then_some(&local));
            let before = disk_bytes(temp.path());
            let owner =
                SettingsOwner::open_active(&path, std::time::Duration::from_secs(1)).unwrap();
            let settings = owner.blocking_current_snapshot().unwrap();
            let expected = if (1..=3600).contains(&seconds) {
                seconds
            } else {
                15
            };
            assert_eq!(settings.room_idle_confirmation_seconds, expected);
            assert_eq!(settings.typing_hold_seconds, 7);
            assert_eq!(
                settings
                    .local_overlay_state
                    .owns_top_level("roomIdleConfirmationSeconds"),
                with_overlay
            );
            assert_eq!(disk_bytes(temp.path()), before);
        }
    }
}

#[test]
fn room_idle_confirmation_malformed_base_keeps_bytes_and_refuses_save() {
    for raw in [
        r#"{"roomIdleConfirmationSeconds":"15"}"#,
        r#"{"roomIdleConfirmationSeconds":null}"#,
        r#"{"roomIdleConfirmationSeconds":1.5}"#,
        r#"{"roomIdleConfirmationSeconds":-1}"#,
        r#"{"roomIdleConfirmationSeconds":4294967296}"#,
        "[]",
        "null",
        "{broken",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let local = serde_json::json!({"typingHoldSeconds": 7});
        // Establish the existing split-file layout before corrupting the base.
        let path = seed(temp.path(), Some(&base_fixture()), Some(&local));
        let initial = load_settings_for_cli_from_path(&path);
        save_settings_to_path_preserving_project_paths(&initial, &path).unwrap();
        let raw_value = serde_json::from_str::<Value>(raw);
        let object_case = matches!(&raw_value, Ok(Value::Object(_)));
        let bytes = match raw_value {
            Ok(Value::Object(fields)) => {
                let mut base = base_fixture();
                base["responseCloseEnabled"] = Value::Bool(false);
                base.as_object_mut().unwrap().extend(fields);
                serde_json::to_vec(&base).unwrap()
            }
            _ => raw.as_bytes().to_vec(),
        };
        std::fs::write(&path, bytes).unwrap();
        let before = disk_bytes(temp.path());
        let settings = load_settings_from_path(&path);
        assert_eq!(
            (
                settings.room_idle_confirmation_seconds,
                settings.typing_hold_seconds
            ),
            (15, 7),
            "case {raw}"
        );
        assert!(settings.response_close_enabled, "case {raw}");
        assert_eq!(settings.root_token.is_some(), object_case, "case {raw}");
        assert_ne!(settings.root_token.as_deref(), Some("base-token"));
        assert_eq!(
            settings.project_paths_persistence_error.is_none(),
            object_case,
            "case {raw}"
        );
        let strict = load_settings_for_cli_strict_from_path(Some(&path));
        assert_eq!(strict.is_ok(), object_case, "case {raw}");
        if let Ok(strict) = strict {
            // Selection succeeded; typed base failure uses defaults plus overlay.
            assert_eq!(strict.room_idle_confirmation_seconds, 15);
            assert_eq!(strict.typing_hold_seconds, 7);
            assert!(strict.root_token.is_none());
            assert!(strict.project_paths_persistence_error.is_none());
        }
        let cli = load_settings_for_cli_from_path(&path);
        assert_eq!(cli.room_idle_confirmation_seconds, 15);
        assert_eq!(cli.typing_hold_seconds, 7);
        assert!(cli.root_token.is_none(), "case {raw}");
        assert_eq!(
            cli.project_paths_persistence_error.is_none(),
            object_case,
            "case {raw}"
        );
        assert!(cli.response_close_enabled, "case {raw}");
        assert!(settings.project_paths.is_empty());
        assert!(cli.project_paths.is_empty());
        assert_eq!(disk_bytes(temp.path()), before, "case {raw}");
        let error =
            save_settings_to_path_preserving_project_paths_typed(&settings, &path).unwrap_err();
        assert_eq!(
            error.stage,
            SettingsSaveStage::PreserveDiskGate,
            "case {raw}"
        );
        assert_eq!(disk_bytes(temp.path()), before, "case {raw}");
    }
}
