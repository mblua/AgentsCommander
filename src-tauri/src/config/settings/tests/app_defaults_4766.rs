//! Pins `AppSettings::default()` so building it from the serde field
//! defaults keeps every value the hand-written literal had.

use super::*;
use crate::config::settings::context_history::DEFAULT_SESSION_WINDOWS;
use serde_json::Value;

const EXPECTED_APP_SETTINGS_DEFAULT_JSON: &str =
    include_str!("fixtures/app_settings_default_4766.json");

/// Keys that depend on the platform or the test binary name, so they are
/// checked by their own tests instead of the fixture.
const UNPINNED_KEYS: [&str; 4] = [
    "defaultShell",
    "defaultShellArgs",
    "webServerPort",
    "apiServerPort",
];

fn filtered_default_value() -> Value {
    let mut value = serde_json::to_value(AppSettings::default()).expect("serialize default");
    let object = value
        .as_object_mut()
        .expect("default serializes to an object");
    for key in UNPINNED_KEYS {
        object.remove(key);
    }
    value
}

#[test]
fn app_settings_default_matches_pinned_fixture() {
    let expected: Value =
        serde_json::from_str(EXPECTED_APP_SETTINGS_DEFAULT_JSON).expect("parse fixture");
    assert_eq!(filtered_default_value(), expected);
}

#[test]
fn app_settings_default_shell_matches_platform() {
    let settings = AppSettings::default();
    if cfg!(target_os = "windows") {
        assert_eq!(settings.default_shell, "powershell.exe");
        assert_eq!(settings.default_shell_args, vec!["-NoLogo".to_string()]);
    } else {
        assert_eq!(settings.default_shell, "/bin/bash");
        assert!(settings.default_shell_args.is_empty());
    }
}

#[test]
fn app_settings_default_ports_match_profile() {
    let settings = AppSettings::default();
    assert_eq!(
        settings.web_server_port,
        crate::config::profile::web_server_port()
    );
    assert_eq!(
        settings.api_server_port,
        crate::config::profile::api_server_port()
    );
}

/// Prints the filtered default so the fixture can be checked or
/// captured. Ignored by default; it asserts nothing.
#[test]
#[ignore = "capture helper: run with --ignored to print the fixture"]
fn print_app_settings_default_4766() {
    println!(
        "{}",
        serde_json::to_string_pretty(&filtered_default_value()).unwrap()
    );
}

#[test]
fn app_settings_default_skip_serialized_fields_are_none() {
    let settings = AppSettings::default();
    assert!(settings.sidebar_geometry.is_none());
    assert!(settings.terminal_geometry.is_none());
    assert!(settings.watchers_geometry.is_none());
    assert!(settings.root_token.is_none());
    assert!(settings.agent_file_source_by_menu.is_none());
    assert!(settings.legacy_start_only_coordinators.is_none());
    assert!(settings.watchers.is_empty());
    assert!(settings.quota_sources.is_empty());
}

#[test]
fn app_settings_default_overrides_differ_from_missing_keys() {
    let settings = AppSettings::default();
    assert!(settings.restore_coordinator_wake_state);
    assert!(settings.restart_resume_wake_working_agents);
    assert_eq!(
        settings.context_history_color_sessions,
        DEFAULT_SESSION_WINDOWS.last().copied()
    );
}
