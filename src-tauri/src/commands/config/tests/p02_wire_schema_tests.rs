use super::*;
use serde_json::json;

#[test]
fn p02_typed_ipc_snapshot_emits_only_canonical_configuration_names() {
    let mut settings = AppSettings {
        agents: vec![serde_json::from_value(json!({
            "id":"one","label":"One","color":"#123456","baseCommand":"tool","command":"old"
        }))
        .unwrap()],
        ..AppSettings::default()
    };
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "one".into(),
        std::collections::BTreeMap::from([(
            "A".into(),
            serde_json::from_value(json!({"additionalArguments":"--new","command":"--old"}))
                .unwrap(),
        )]),
    );
    let wire = serde_json::to_value(SettingsSnapshot {
        settings,
        project_path_resolution: ProjectPathResolution {
            active_registration_count: 0,
            archived_registration_count: 0,
            issues: vec![],
            reconciliation_error: None,
        },
        settings_file_path: None,
        overlay_owns_agents: false,
        agents_layer_unreadable: false,
        agents_file_path: None,
    })
    .unwrap();
    assert_eq!(wire["agents"][0]["baseCommand"], "tool");
    assert!(wire["agents"][0].get("command").is_none());
    let cell = &wire["codingAgentProfiles"]["profilesByAgent"]["one"]["A"];
    assert_eq!(cell["additionalArguments"], "--new");
    assert!(cell.get("command").is_none());
}
