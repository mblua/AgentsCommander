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
