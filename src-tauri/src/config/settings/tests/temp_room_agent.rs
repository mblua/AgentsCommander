use super::*;
use crate::config::settings::TempRoomAgentChoice;

fn with_key(extra: serde_json::Value) -> AppSettings {
    let mut value = serde_json::to_value(AppSettings::default()).expect("serialize default");
    let object = value.as_object_mut().expect("settings object");
    object.remove("tempRoomAgent");
    if let serde_json::Value::Object(map) = extra {
        object.extend(map);
    }
    serde_json::from_value(value).expect("deserialize settings")
}

#[test]
fn missing_key_loads_none() {
    assert_eq!(with_key(serde_json::json!({})).temp_room_agent, None);
}

#[test]
fn explicit_null_profile_loads_none() {
    let s = with_key(serde_json::json!({
        "tempRoomAgent": {"agentId": "claude", "requestedProfile": null}
    }));
    let expected = TempRoomAgentChoice {
        agent_id: "claude".to_string(),
        requested_profile: None,
    };
    assert_eq!(s.temp_room_agent, Some(expected));
}

#[test]
fn missing_profile_loads_none() {
    let s = with_key(serde_json::json!({"tempRoomAgent": {"agentId": "codex"}}));
    let choice = s.temp_room_agent.expect("choice");
    assert_eq!(choice.agent_id, "codex");
    assert_eq!(choice.requested_profile, None);
}

#[test]
fn serialize_skips_none_and_uses_camel_case() {
    let value = serde_json::to_value(AppSettings::default()).expect("serialize none");
    assert!(value.get("tempRoomAgent").is_none());
    let s = AppSettings {
        temp_room_agent: Some(TempRoomAgentChoice {
            agent_id: "claude".to_string(),
            requested_profile: Some("B".to_string()),
        }),
        ..AppSettings::default()
    };
    let value = serde_json::to_value(&s).expect("serialize some");
    let choice = value.get("tempRoomAgent").expect("tempRoomAgent key");
    assert_eq!(
        choice,
        &serde_json::json!({"agentId": "claude", "requestedProfile": "B"})
    );
}

#[test]
fn debug_prints_temp_room_agent() {
    assert!(format!("{:?}", AppSettings::default()).contains("temp_room_agent"));
}

fn disk_object(path: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    let raw = std::fs::read_to_string(path).expect("read settings.json");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("parse settings.json");
    value.as_object().expect("settings object").clone()
}

#[test]
fn choice_round_trips_through_save_and_load() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("settings.json");
    let seed = serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "agents": [],
        "rootToken": "fixed-token",
    });
    std::fs::write(&path, seed.to_string()).expect("seed settings.json");

    let mut loaded = load_settings_from_path(&path);
    assert_eq!(loaded.temp_room_agent, None);
    let choice = TempRoomAgentChoice {
        agent_id: "codex".to_string(),
        requested_profile: Some("A".to_string()),
    };
    loaded.temp_room_agent = Some(choice.clone());
    save_settings_to_path_preserving_project_paths(&loaded, &path).expect("save some");
    assert_eq!(
        disk_object(&path)["tempRoomAgent"],
        serde_json::json!({"agentId": "codex", "requestedProfile": "A"})
    );
    let mut reloaded = load_settings_from_path(&path);
    assert_eq!(reloaded.temp_room_agent, Some(choice));

    reloaded.temp_room_agent = None;
    save_settings_to_path_preserving_project_paths(&reloaded, &path).expect("save none");
    assert!(!disk_object(&path).contains_key("tempRoomAgent"));
    assert_eq!(load_settings_from_path(&path).temp_room_agent, None);
}
