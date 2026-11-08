use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::config::settings::{new_settings_state, AppSettings};

/// Settings with the given history switch and retention.
fn settings(enabled: bool, retention: u32) -> AppSettings {
    let mut raw = serde_json::to_value(AppSettings::default()).expect("settings json");
    raw["contextHistoryEnabled"] = serde_json::json!(enabled);
    raw["contextHistoryRetentionDays"] = serde_json::json!(retention);
    let settings: AppSettings = serde_json::from_value(raw).expect("settings");
    assert_eq!(settings.context_history_enabled, enabled);
    assert_eq!(settings.context_history_retention_days.days(), retention);
    settings
}

fn loaded(enabled: bool, retention_days: u32) -> LoadedHistoryConfig {
    LoadedHistoryConfig {
        config: HistoryConfig {
            enabled,
            retention_days,
        },
        status: ConfigStatus::Loaded,
    }
}

/// Replace the live history fields of `state` with those of `next`.
fn publish(state: &SettingsState, next: &AppSettings) {
    let mut guard = state.blocking_write();
    guard.context_history_enabled = next.context_history_enabled;
    guard.context_history_retention_days = next.context_history_retention_days;
}

#[test]
fn default_settings_load_disabled_with_thirty_days() {
    let env = DiskProjectEnvironment::for_tests();
    assert_eq!(env.load_config(Path::new("/p/proj/.ac")), loaded(false, 30));
}

#[test]
fn each_load_reads_the_live_settings_snapshot() {
    let state = new_settings_state(AppSettings::default());
    let env = DiskProjectEnvironment::new(Arc::clone(&state));
    let root = Path::new("/p/proj/.ac");
    publish(&state, &settings(true, 7));
    assert_eq!(env.load_config(root), loaded(true, 7));
    publish(&state, &settings(false, 365));
    assert_eq!(env.load_config(root), loaded(false, 365));
    publish(&state, &settings(true, 1));
    assert_eq!(env.load_config(root), loaded(true, 1));
}

#[test]
fn every_root_gets_the_same_configuration() {
    let env = DiskProjectEnvironment::new(new_settings_state(settings(true, 90)));
    for root in ["/p/one/.ac", "/p/two/.ac", "C:/elsewhere/.ac"] {
        assert_eq!(env.load_config(Path::new(root)), loaded(true, 90));
    }
}

/// A project root with the given legacy `heatmap.json` bytes, if any.
fn legacy_root(bytes: Option<&[u8]>) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    if let Some(bytes) = bytes {
        std::fs::write(ac_root.join("heatmap.json"), bytes).expect("write legacy");
    }
    (tmp, ac_root)
}

#[test]
fn legacy_heatmap_files_are_ignored_and_untouched() {
    let legacy: [Option<&[u8]>; 4] = [
        None,
        Some(br#"{"version":1,"enabled":true,"retentionDays":5}"#),
        Some(br#"{"version":1,"enabled":false,"retentionDays":365}"#),
        Some(b"{ not json"),
    ];
    for current in [settings(false, 30), settings(true, 12)] {
        let expected = loaded(
            current.context_history_enabled,
            current.context_history_retention_days.days(),
        );
        let env = DiskProjectEnvironment::new(new_settings_state(current));
        for bytes in legacy {
            let (_tmp, ac_root) = legacy_root(bytes);
            assert_eq!(env.load_config(&ac_root), expected);
            let path = ac_root.join("heatmap.json");
            match bytes {
                Some(bytes) => assert_eq!(std::fs::read(&path).expect("legacy bytes"), bytes),
                None => assert!(!path.exists(), "no file is created"),
            }
        }
    }
}

#[test]
fn an_unavailable_owner_fails_closed_without_a_path() {
    let state = new_settings_state(settings(true, 7));
    state.mark_invalid("test");
    let env = DiskProjectEnvironment::new(state);
    let result = env.load_config(Path::new("/p/proj/.ac"));
    assert_eq!(result.config, HistoryConfig::DISABLED);
    assert_eq!(
        result.status,
        ConfigStatus::Invalid(SETTINGS_UNAVAILABLE.to_string())
    );
}
