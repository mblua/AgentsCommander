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

#[test]
fn choice_round_trips_through_serde() {
    let s = AppSettings {
        temp_room_agent: Some(TempRoomAgentChoice {
            agent_id: "codex".to_string(),
            requested_profile: Some("A".to_string()),
        }),
        ..AppSettings::default()
    };
    let value = serde_json::to_value(&s).expect("serialize");
    let back: AppSettings = serde_json::from_value(value).expect("round trip");
    assert_eq!(back.temp_room_agent, s.temp_room_agent);
}
