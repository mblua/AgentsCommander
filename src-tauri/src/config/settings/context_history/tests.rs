use super::*;
use crate::config::settings::{load_settings_from_path, AppSettings};
use serde_json::json;

const KEY: &str = "contextHistoryColorDays";

fn settings_json(value: Option<Value>) -> Value {
    let mut settings = AppSettings {
        project_paths: vec!["C:/fixture/project-a".to_string()],
        typing_hold_seconds: 9,
        ..AppSettings::default()
    };
    settings.context_history_color_days = ContextHistoryColorDays::Seven;
    let mut raw = serde_json::to_value(&settings).unwrap();
    let object = raw.as_object_mut().unwrap();
    match value {
        Some(value) => object.insert(KEY.to_string(), value),
        None => object.remove(KEY),
    };
    raw
}

fn load(value: Option<Value>) -> AppSettings {
    serde_json::from_value(settings_json(value)).expect("a bad period never fails the load")
}

#[test]
fn accepts_only_one_seven_and_thirty() {
    for (wire, expected) in [
        (1, ContextHistoryColorDays::One),
        (7, ContextHistoryColorDays::Seven),
        (30, ContextHistoryColorDays::Thirty),
    ] {
        assert_eq!(load(Some(json!(wire))).context_history_color_days, expected);
        assert_eq!(expected.days(), wire);
    }
}

#[test]
fn missing_and_fresh_default_to_thirty() {
    assert_eq!(
        load(None).context_history_color_days,
        ContextHistoryColorDays::Thirty
    );
    assert_eq!(
        AppSettings::default().context_history_color_days,
        ContextHistoryColorDays::Thirty
    );
}

#[test]
fn invalid_values_fall_back_to_thirty_and_keep_other_fields() {
    let invalid = [
        json!(14),
        json!(90),
        json!(-1),
        json!(1.5),
        json!(0),
        json!("7"),
        Value::Null,
        json!([7]),
        json!({ "days": 7 }),
        json!(true),
    ];
    for value in invalid {
        let settings = load(Some(value.clone()));
        assert_eq!(
            settings.context_history_color_days,
            ContextHistoryColorDays::Thirty,
            "{value}"
        );
        assert_eq!(settings.project_paths, ["C:/fixture/project-a"], "{value}");
        assert_eq!(settings.typing_hold_seconds, 9, "{value}");
    }
}

#[test]
fn serializes_as_a_camel_case_number_and_round_trips() {
    for days in [
        ContextHistoryColorDays::One,
        ContextHistoryColorDays::Seven,
        ContextHistoryColorDays::Thirty,
    ] {
        let settings = AppSettings {
            context_history_color_days: days,
            ..AppSettings::default()
        };
        let raw = serde_json::to_value(&settings).unwrap();
        assert_eq!(raw[KEY], json!(days.days()));
        assert!(raw.get("context_history_color_days").is_none());
        let back: AppSettings = serde_json::from_value(raw).unwrap();
        assert_eq!(back.context_history_color_days, days);
    }
}

#[test]
fn loading_an_invalid_period_never_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    // A first load may persist unrelated startup migrations (root token, ...).
    let seed = serde_json::to_vec_pretty(&settings_json(Some(json!(7)))).unwrap();
    std::fs::write(&path, seed).unwrap();
    load_settings_from_path(&path);
    let mut stable: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stable[KEY] = json!(14);
    let bytes = serde_json::to_vec_pretty(&stable).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let settings = load_settings_from_path(&path);
    assert_eq!(
        settings.context_history_color_days,
        ContextHistoryColorDays::Thirty
    );
    assert_eq!(settings.typing_hold_seconds, 9);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

const ENABLED: &str = "contextHistoryEnabled";
const RETENTION: &str = "contextHistoryRetentionDays";

/// #4049: fixture with one field replaced (or removed when `None`).
fn load_with(key: &str, value: Option<Value>) -> AppSettings {
    let mut raw = settings_json(Some(json!(7)));
    let object = raw.as_object_mut().unwrap();
    object.insert(ENABLED.to_string(), json!(true));
    object.insert(RETENTION.to_string(), json!(90));
    match value {
        Some(value) => object.insert(key.to_string(), value),
        None => object.remove(key),
    };
    serde_json::from_value(raw).expect("a bad #4049 field never fails the load")
}

#[test]
fn fresh_and_missing_fields_use_off_and_thirty_days() {
    let fresh = AppSettings::default();
    assert!(!fresh.context_history_enabled);
    assert_eq!(fresh.context_history_retention_days.days(), 30);
    assert!(!load_with(ENABLED, None).context_history_enabled);
    assert_eq!(
        load_with(RETENTION, None).context_history_retention_days,
        ContextHistoryRetentionDays::default()
    );
}

#[test]
fn enabled_accepts_only_booleans() {
    assert!(load_with(ENABLED, Some(json!(true))).context_history_enabled);
    assert!(!load_with(ENABLED, Some(json!(false))).context_history_enabled);
    for value in [
        json!(1),
        json!(0),
        json!("true"),
        Value::Null,
        json!([true]),
        json!({}),
    ] {
        let settings = load_with(ENABLED, Some(value.clone()));
        assert!(!settings.context_history_enabled, "{value}");
        assert_eq!(
            settings.context_history_retention_days.days(),
            90,
            "{value}"
        );
        assert_eq!(settings.typing_hold_seconds, 9, "{value}");
    }
}

#[test]
fn retention_accepts_only_integers_one_to_365() {
    for wire in [1, 30, 365] {
        let settings = load_with(RETENTION, Some(json!(wire)));
        assert_eq!(settings.context_history_retention_days.days(), wire);
    }
    let invalid = [
        json!(0),
        json!(366),
        json!(-1),
        json!(1.5),
        json!(30.0),
        json!(u64::MAX),
        json!("30"),
        Value::Null,
        json!(true),
        json!([30]),
        json!({ "days": 30 }),
    ];
    for value in invalid {
        let settings = load_with(RETENTION, Some(value.clone()));
        assert_eq!(
            settings.context_history_retention_days.days(),
            30,
            "{value}"
        );
        assert!(settings.context_history_enabled, "{value}");
        assert_eq!(settings.project_paths, ["C:/fixture/project-a"], "{value}");
        assert_eq!(settings.typing_hold_seconds, 9, "{value}");
    }
}

#[test]
fn new_fields_serialize_as_camel_case_and_round_trip() {
    let settings = load_with(RETENTION, Some(json!(365)));
    let raw = serde_json::to_value(&settings).unwrap();
    assert_eq!(raw[ENABLED], json!(true));
    assert_eq!(raw[RETENTION], json!(365));
    assert!(raw.get("context_history_enabled").is_none());
    assert!(raw.get("context_history_retention_days").is_none());
    let back: AppSettings = serde_json::from_value(raw).unwrap();
    assert!(back.context_history_enabled);
    assert_eq!(back.context_history_retention_days.days(), 365);
}

#[test]
fn loading_invalid_new_fields_never_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    // A first load may persist unrelated startup migrations (root token, ...).
    let seed = serde_json::to_vec_pretty(&settings_json(Some(json!(7)))).unwrap();
    std::fs::write(&path, seed).unwrap();
    load_settings_from_path(&path);
    let mut stable: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stable[ENABLED] = json!("yes");
    stable[RETENTION] = json!(0);
    let bytes = serde_json::to_vec_pretty(&stable).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let settings = load_settings_from_path(&path);
    assert!(!settings.context_history_enabled);
    assert_eq!(settings.context_history_retention_days.days(), 30);
    assert_eq!(settings.typing_hold_seconds, 9);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn new_fields_survive_save_reload_and_unrelated_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut settings = load_with(RETENTION, Some(json!(120)));
    let saved = crate::config::settings::save::save_settings_to_path_preserving_project_paths(
        &settings, &path,
    )
    .unwrap();
    assert!(saved.context_history_enabled);
    assert_eq!(saved.context_history_retention_days.days(), 120);
    settings = load_settings_from_path(&path);
    settings.typing_hold_seconds = 11;
    crate::config::settings::save::save_settings_to_path_preserving_project_paths(&settings, &path)
        .unwrap();
    let reloaded = load_settings_from_path(&path);
    assert_eq!(reloaded.typing_hold_seconds, 11);
    assert!(reloaded.context_history_enabled);
    assert_eq!(reloaded.context_history_retention_days.days(), 120);
    assert_eq!(
        reloaded.context_history_color_days,
        ContextHistoryColorDays::Seven
    );
}

const WINDOWS: &str = "contextHistorySessionWindows";
const COLOUR: &str = "contextHistoryColorSessions";

/// #4091: fixture with nondefault #4049 fields plus the given windows/colour
/// (each removed when `None`).
fn sessions_json(windows: Option<Value>, colour: Option<Value>) -> Value {
    let mut raw = settings_json(Some(json!(7)));
    let object = raw.as_object_mut().unwrap();
    object.insert(RETENTION.to_string(), json!(90));
    for (key, value) in [(WINDOWS, windows), (COLOUR, colour)] {
        match value {
            Some(value) => object.insert(key.to_string(), value),
            None => object.remove(key),
        };
    }
    raw
}

fn load_sessions(windows: Option<Value>, colour: Option<Value>) -> AppSettings {
    let mut settings: AppSettings = serde_json::from_value(sessions_json(windows, colour))
        .expect("a bad #4091 field never fails the load");
    settings.normalize_context_history_sessions();
    settings
}

#[test]
fn fresh_defaults_are_coherent() {
    let fresh = AppSettings::default();
    assert_eq!(
        fresh.context_history_session_windows.counts(),
        [25, 100, 300]
    );
    assert_eq!(fresh.context_history_color_sessions, Some(300));
    let raw = serde_json::to_value(&fresh).unwrap();
    assert_eq!(raw[WINDOWS], json!([25, 100, 300]));
    assert_eq!(raw[COLOUR], json!(300));
}

#[test]
fn valid_custom_arrays_load_unchanged() {
    let max: Vec<u32> = (1..=8).map(|n| n * 1250).collect();
    for windows in [json!([1]), json!([5, 10]), json!(max), json!([10000])] {
        let settings = load_sessions(Some(windows.clone()), None);
        let raw = serde_json::to_value(&settings).unwrap();
        assert_eq!(raw[WINDOWS], windows);
        assert_eq!(raw[RETENTION], json!(90), "{windows}");
    }
}

#[test]
fn invalid_arrays_fall_back_to_defaults_and_keep_other_fields() {
    let invalid = [
        json!([]),
        json!([0, 10]),
        json!([1.5]),
        json!(["25"]),
        json!([null]),
        json!([true]),
        json!([-1, 5]),
        json!([25, 25]),
        json!([100, 25]),
        json!([1, 2, 3, 4, 5, 6, 7, 8, 9]),
        json!([10001]),
        json!([u64::MAX]),
        json!(25),
        json!("25,100"),
        Value::Null,
        json!({ "0": 25 }),
    ];
    for value in invalid {
        let settings = load_sessions(Some(value.clone()), Some(json!(100)));
        assert_eq!(
            settings.context_history_session_windows,
            ContextHistorySessionWindows::default(),
            "{value}"
        );
        assert_eq!(
            settings.context_history_color_sessions,
            Some(100),
            "{value}"
        );
        assert_eq!(
            settings.context_history_retention_days.days(),
            90,
            "{value}"
        );
        assert_eq!(settings.typing_hold_seconds, 9, "{value}");
        assert_eq!(settings.project_paths, ["C:/fixture/project-a"], "{value}");
    }
}

#[test]
fn missing_windows_use_defaults() {
    let settings = load_sessions(None, None);
    assert_eq!(
        settings.context_history_session_windows.counts(),
        [25, 100, 300]
    );
    assert_eq!(settings.context_history_color_sessions, Some(300));
}

#[test]
fn colour_falls_back_to_the_last_custom_member() {
    let windows = || Some(json!([25, 100, 300, 1000]));
    for colour in [
        None,
        Some(Value::Null),
        Some(json!(0)),
        Some(json!(1.5)),
        Some(json!("300")),
        Some(json!(true)),
        Some(json!([300])),
        Some(json!(-300)),
        Some(json!(10001)),
        Some(json!(50)),
    ] {
        let settings = load_sessions(windows(), colour.clone());
        assert_eq!(
            settings.context_history_color_sessions,
            Some(1000),
            "{colour:?}"
        );
        assert_eq!(settings.typing_hold_seconds, 9, "{colour:?}");
    }
    let explicit = load_sessions(windows(), Some(json!(300)));
    assert_eq!(explicit.context_history_color_sessions, Some(300));
    let raw = serde_json::to_value(&explicit).unwrap();
    assert_eq!(raw[COLOUR], json!(300));
}

#[test]
fn old_colour_days_never_become_a_session_count() {
    let mut raw = sessions_json(Some(json!([1, 7, 30])), None);
    raw[KEY] = json!(7);
    let mut settings: AppSettings = serde_json::from_value(raw).unwrap();
    assert_eq!(settings.context_history_color_sessions, None);
    settings.normalize_context_history_sessions();
    assert_eq!(settings.context_history_color_sessions, Some(30));
    assert_eq!(
        settings.context_history_color_days,
        ContextHistoryColorDays::Seven
    );
}

#[test]
fn production_load_repairs_colour_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    // A first load may persist unrelated startup migrations (root token, ...).
    let seed = serde_json::to_vec_pretty(&sessions_json(None, None)).unwrap();
    std::fs::write(&path, seed).unwrap();
    load_settings_from_path(&path);
    let mut stable: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stable[WINDOWS] = json!([25, 100, 300, 1000]);
    for colour in [None, Some(Value::Null), Some(json!("x")), Some(json!(50))] {
        match &colour {
            Some(value) => stable[COLOUR] = value.clone(),
            None => {
                stable.as_object_mut().unwrap().remove(COLOUR);
            }
        }
        let bytes = serde_json::to_vec_pretty(&stable).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let settings = load_settings_from_path(&path);
        assert_eq!(
            settings.context_history_color_sessions,
            Some(1000),
            "{colour:?}"
        );
        assert_eq!(settings.context_history_retention_days.days(), 90);
        assert_eq!(settings.typing_hold_seconds, 9);
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{colour:?}");
    }
}

#[test]
fn save_validation_repairs_colour_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut settings = load_sessions(Some(json!([10, 20])), Some(json!(20)));
    settings.context_history_color_sessions = Some(999);
    crate::config::settings::validate_and_repair_settings(&mut settings).unwrap();
    assert_eq!(settings.context_history_color_sessions, Some(20));
    let saved = crate::config::settings::save::save_settings_to_path_preserving_project_paths(
        &settings, &path,
    )
    .unwrap();
    assert_eq!(saved.context_history_color_sessions, Some(20));
    let disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk[WINDOWS], json!([10, 20]));
    assert_eq!(disk[COLOUR], json!(20));
    let reloaded = load_settings_from_path(&path);
    assert_eq!(reloaded.context_history_session_windows.counts(), [10, 20]);
    assert_eq!(reloaded.context_history_color_sessions, Some(20));
    assert_eq!(reloaded.context_history_retention_days.days(), 90);
}

const SNAPSHOTS: &str = "terminalSnapshotsEnabled";

fn cas(
    current: &AppSettings,
    path: &std::path::Path,
    expected: bool,
    enabled: bool,
) -> AppSettings {
    crate::config::settings::compare_and_set_terminal_snapshots_enabled_at_path(
        current, path, expected, enabled,
    )
    .unwrap()
}

/// #4091 AC-4091-001: a seeded custom-window file whose colour is replaced
/// (or removed when `None`) after startup migrations settled.
fn seed_cas_file(path: &std::path::Path, colour: Option<Value>) -> Vec<u8> {
    let mut raw = sessions_json(Some(json!([25, 100, 300, 1000])), None);
    raw[SNAPSHOTS] = json!(false);
    std::fs::write(path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    load_settings_from_path(path);
    let mut stable: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    match colour {
        Some(value) => stable[COLOUR] = value,
        None => {
            stable.as_object_mut().unwrap().remove(COLOUR);
        }
    }
    let bytes = serde_json::to_vec_pretty(&stable).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn snapshot_cas_on_an_existing_file_keeps_colour_coherent() {
    let cases = [
        (None, 1000),
        (Some(Value::Null), 1000),
        (Some(json!(50)), 1000),
        (Some(json!(300)), 300),
    ];
    for (colour, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let bytes = seed_cas_file(&path, colour.clone());
        let current = AppSettings::default();
        let no_op = cas(&current, &path, false, false);
        assert_eq!(
            no_op.context_history_color_sessions,
            Some(expected),
            "{colour:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{colour:?}");
        let agents = serde_json::to_value(&no_op.agents).unwrap();
        let toggled = cas(&no_op, &path, false, true);
        assert_eq!(
            toggled.context_history_color_sessions,
            Some(expected),
            "{colour:?}"
        );
        let disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(disk[SNAPSHOTS], json!(true), "{colour:?}");
        assert_eq!(disk[COLOUR], json!(expected), "{colour:?}");
        assert_eq!(disk[WINDOWS], json!([25, 100, 300, 1000]), "{colour:?}");
        let reloaded = load_settings_from_path(&path);
        assert_eq!(reloaded.context_history_color_sessions, Some(expected));
        assert_eq!(reloaded.context_history_retention_days.days(), 90);
        assert_eq!(reloaded.typing_hold_seconds, 9);
        assert_eq!(serde_json::to_value(&reloaded.agents).unwrap(), agents);
    }
}

#[test]
fn snapshot_cas_creating_the_file_saves_the_last_window() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    // Raw decode, deliberately not normalized: colour is `None`.
    let current: AppSettings =
        serde_json::from_value(sessions_json(Some(json!([25, 100, 300, 1000])), None)).unwrap();
    assert_eq!(current.context_history_color_sessions, None);
    assert!(!path.exists());
    let created = cas(&current, &path, true, false);
    assert_eq!(created.context_history_color_sessions, Some(1000));
    let disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk[SNAPSHOTS], json!(false));
    assert_eq!(disk[COLOUR], json!(1000));
    assert_eq!(disk[WINDOWS], json!([25, 100, 300, 1000]));
    let reloaded = load_settings_from_path(&path);
    assert_eq!(reloaded.context_history_color_sessions, Some(1000));
    assert_eq!(reloaded.context_history_retention_days.days(), 90);
    assert_eq!(reloaded.typing_hold_seconds, 9);
}
