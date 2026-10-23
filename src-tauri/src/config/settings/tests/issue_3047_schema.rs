use super::*;
use serde_json::json;

#[test]
fn issue_3047_schema_tolerates_metadata_types() {
    let cases = [
        (None, None),
        (Some(Value::Null), None),
        (Some(json!(true)), None),
        (Some(json!(17)), None),
        (Some(json!([])), None),
        (Some(json!({})), None),
        (Some(json!("forged")), Some("forged")),
        (Some(json!("")), Some("")),
    ];
    for (input, expected) in cases {
        let mut value = json!({"additionalArguments": "--keep"});
        if let Some(input) = input {
            value["id_hash"] = input;
        }
        let cell: ProfileCellConfig = serde_json::from_value(value).unwrap();
        assert_eq!(cell.id_hash.as_deref(), expected);
        assert_eq!(cell.command, "--keep");
        let encoded = serde_json::to_value(&cell).unwrap();
        assert!(encoded.get("idHash").is_none());
        assert_eq!(encoded.get("id_hash"), expected.map(|s| json!(s)).as_ref());
    }
}

#[test]
fn issue_3047_schema_roundtrip_preserves_content_and_alias() {
    for (commands, expected) in [
        (json!({"command": "--legacy"}), "--legacy"),
        (json!({"additionalArguments": "--canonical"}), "--canonical"),
        (
            json!({"command": "--legacy", "additionalArguments": "--canonical"}),
            "--canonical",
        ),
    ] {
        let mut value = json!({
            "id_hash": "untrusted",
            "enabled": false,
            "env": {"KEY": "value"},
            "notes": "keep",
            "preflight": "echo ready",
            "preflightTimeoutSeconds": 7
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(commands.as_object().unwrap().clone());
        let cell: ProfileCellConfig = serde_json::from_value(value).unwrap();
        assert_eq!(cell.command, expected);
        assert_eq!(cell.env.get("KEY").map(String::as_str), Some("value"));
        assert_eq!(cell.id_hash.as_deref(), Some("untrusted"));
        assert!(!cell.enabled);
        assert_eq!(cell.notes, "keep");
        assert_eq!(cell.preflight.as_deref(), Some("echo ready"));
        assert_eq!(cell.preflight_timeout_seconds, Some(7));
        let encoded = serde_json::to_value(&cell).unwrap();
        assert_eq!(encoded["additionalArguments"], expected);
        assert_eq!(encoded["id_hash"], "untrusted");
        assert!(encoded.get("command").is_none());
        assert!(encoded.get("idHash").is_none());
        assert_eq!(
            serde_json::from_value::<ProfileCellConfig>(encoded).unwrap(),
            cell
        );
    }
}

#[test]
fn issue_3047_schema_gap_ignores_only_hash() {
    let mut profiles = CodingAgentProfilesConfig::default();
    assert!(is_profile_gap(&profiles, "n"));
    for hash in [None, Some("forged".to_string())] {
        let mut cell = empty_profile_cell();
        cell.id_hash = hash;
        profiles
            .profiles_by_agent
            .insert("n".into(), BTreeMap::from([("A".into(), cell.clone())]));
        assert!(is_profile_gap(&profiles, "n"));
        let before = profiles.clone();
        assert!(is_profile_gap(&profiles, "n"));
        assert_eq!(profiles, before);
        for (field, value) in [
            ("notes", json!("keep")),
            ("env", json!({"KEY": "value"})),
            ("additionalArguments", json!("--keep")),
            ("preflight", json!("echo ready")),
            ("preflightTimeoutSeconds", json!(7)),
            ("enabled", json!(false)),
        ] {
            let mut value_cell = serde_json::to_value(&cell).unwrap();
            value_cell[field] = value;
            let mut real: ProfileCellConfig = serde_json::from_value(value_cell).unwrap();
            real.id_hash = None;
            profiles
                .profiles_by_agent
                .insert("n".into(), BTreeMap::from([("A".into(), real)]));
            assert!(!is_profile_gap(&profiles, "n"), "{field}");
        }
        profiles
            .profiles_by_agent
            .insert("n".into(), BTreeMap::from([("A".into(), cell)]));
        profiles
            .profile_labels_by_agent
            .insert("n".into(), BTreeMap::from([("A".into(), "Mine".into())]));
        assert!(!is_profile_gap(&profiles, "n"));
        profiles.profile_labels_by_agent.clear();
    }
    let mut hashed = empty_profile_cell();
    hashed.id_hash = Some("forged".into());
    assert_ne!(hashed, empty_profile_cell());
}
