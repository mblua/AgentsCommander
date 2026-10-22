use super::*;
use crate::config::settings::{configuration_identity, profile_identity, IdentityValidationCode};
use serde_json::{json, Value};

fn definition(key: &str, command: &str) -> Value {
    json!({"key":key, "label":key, "description":"own", "color":"#123456", "command":command})
}

fn parse(value: &Value) -> Result<CodingAgentCatalogSchema2, IdentityCatalogError> {
    parse_catalog_schema2(&serde_json::to_vec(value).unwrap())
}

#[test]
fn identity_schema2_complete_model_and_local_profiles_roundtrip() {
    let mut agent = definition("own", "custom --base");
    agent["envs"] = json!([{"key":"SECRET", "value":"configured", "enabled":false}]);
    agent["instructionsFilename"] = json!("AGENTS.md");
    agent["configSeed"] = json!({"enabled":true,"dest":".custom"});
    agent["installCommands"] =
        json!({"default":"install custom", "windows":"install custom-windows"});
    agent["updateCommands"] = json!(["update custom"]);
    agent["isolatedHome"] = json!(true);
    let catalog = parse(&json!({"schemaVersion":2,"agents":[agent],
        "codingAgentProfiles":{"own":{"Z":{"command":"--model mine","env":{"MODEL":"mine"},"enabled":false,"notes":"own notes"}}},
        "profileLabels":{"own":{"Z":"Own label"}}
    })).unwrap();
    assert_eq!(catalog.agents[0].command, "custom --base");
    assert!(!catalog.agents[0].envs[0].enabled);
    assert_eq!(
        catalog.agents[0]
            .install_commands
            .as_ref()
            .unwrap()
            .windows
            .as_deref(),
        Some("install custom-windows")
    );
    assert_eq!(catalog.profile_labels["own"]["Z"], "Own label");
    assert!(!catalog.coding_agent_profiles["own"]["Z"].enabled);
    assert!(!catalog.coding_agent_profiles["own"].contains_key("A"));
    assert_eq!(
        parse(&serde_json::to_value(&catalog).unwrap()).unwrap(),
        catalog
    );
    assert!(catalog.validate_identities().is_ok());
}

#[test]
fn identity_schema2_defaults_are_own_and_no_profile_is_invented() {
    let catalog =
        parse(&json!({"schemaVersion":2,"agents":[definition("own", "custom")]})).unwrap();
    let agent = &catalog.agents[0];
    assert!(agent.envs.is_empty());
    assert!(agent.removable);
    assert!(!agent.auto_update);
    assert!(agent.config_seed.is_none());
    assert!(catalog.coding_agent_profiles.is_empty());
    assert!(catalog.profile_labels.is_empty());
    let empty = parse(&serde_json::to_value(CodingAgentCatalogSchema2::empty()).unwrap()).unwrap();
    assert!(empty.agents.is_empty());
}

#[test]
fn identity_schema2_requires_version_agents_and_explicit_nonempty_command() {
    for value in [
        json!({"schemaVersion":2}),
        json!({"agents":[]}),
        json!({"schemaVersion":3,"agents":[]}),
        json!({"schemaVersion":"2","agents":[]}),
        json!({"schemaVersion":2,"agents":[{"key":"own","label":"Own","description":"own","color":"#fff"}]}),
        json!({"schemaVersion":2,"agents":[definition("own", " \u{2003}")]}),
    ] {
        assert!(matches!(
            parse(&value),
            Err(IdentityCatalogError::InvalidFormat(_))
        ));
    }
}

#[test]
fn identity_schema2_rejects_schema1_without_patch_composition_or_conversion() {
    for value in [
        json!({"schemaVersion":1,"agents":[]}),
        json!({"schemaVersion":1,"agents":[{"key":"claude","label":"patch only"}]}),
    ] {
        assert_eq!(
            parse(&value).unwrap_err(),
            IdentityCatalogError::IndependentFormatRequired
        );
    }
    // Additive schema2 parsing does not alter the legacy schema1 DTO.
    let legacy: CodingAgentCatalog = serde_json::from_value(json!({"agents":[]})).unwrap();
    assert_eq!(legacy.schema_version, CATALOG_SCHEMA_VERSION);
}

#[test]
fn identity_schema2_unique_keys_and_local_map_ownership() {
    for value in [
        json!({"schemaVersion":2,"agents":[definition("own","one"),definition("own","two")]}),
        json!({"schemaVersion":2,"agents":[definition("INVALID_KEY","one")]}),
    ] {
        assert!(matches!(
            parse(&value),
            Err(IdentityCatalogError::InvalidFormat(_))
        ));
    }
}

#[test]
fn identity_schema2_letters_are_exact_a_through_z_in_both_maps() {
    for letter in ["", "a", "AA", "Å", "[", " A"] {
        for field in ["codingAgentProfiles", "profileLabels"] {
            let mut value = json!({"schemaVersion":2,"agents":[definition("own","one")]});
            value[field] = json!({"own":{letter: if field == "profileLabels" { json!("Label") } else { json!({}) }}});
            assert!(
                matches!(parse(&value), Err(IdentityCatalogError::InvalidFormat(_))),
                "{field}: {letter:?}"
            );
        }
    }
}

#[test]
fn identity_schema2_duplicate_json_members_and_trailing_data_rejected() {
    for bytes in [
        br#"{"schemaVersion":2,"schemaVersion":2,"agents":[]}"#.as_slice(),
        br#"{"schemaVersion":2,"agents":[]} {}"#,
        br#"{"schemaVersion":2,"agents":[],"profileLabels":{"own":{"A":"one","A":"two"}}}"#,
    ] {
        assert!(matches!(
            parse_catalog_schema2(bytes),
            Err(IdentityCatalogError::InvalidFormat(_))
        ));
    }
}

#[test]
fn identity_schema2_historical_duplicates_diagnosed_with_existing_name() {
    let catalog = parse(&json!({"schemaVersion":2,"agents":[definition("existing","tool"),definition("second","loot")]})).unwrap();
    let error = catalog.validate_identities().unwrap_err();
    assert_eq!(
        error.code,
        IdentityValidationCode::DuplicateConfigurationIdentity
    );
    assert_eq!(error.existing_name, "existing");
    assert_eq!(error.entry_hint, "second");
    assert!(
        parse(&json!({"schemaVersion":2,"agents":[definition("second","loot")]}))
            .unwrap()
            .validate_identities()
            .is_ok()
    );
}

#[test]
fn identity_schema2_pid_only_edit_and_decorative_changes() {
    let mut value = json!({"schemaVersion":2,"agents":[definition("own","tool")],"codingAgentProfiles":{"own":{"A":{"command":"--model one"}}}});
    let before = parse(&value).unwrap();
    value["agents"][0]["label"] = json!("Renamed");
    value["agents"][0]["color"] = json!("#fff");
    value["codingAgentProfiles"]["own"]["A"]["command"] = json!("--model two");
    let after = parse(&value).unwrap();
    assert_eq!(
        configuration_identity(&before.agents[0].command, &before.agents[0].envs),
        configuration_identity(&after.agents[0].command, &after.agents[0].envs)
    );
    assert_ne!(
        profile_identity(&before.coding_agent_profiles["own"]["A"]),
        profile_identity(&after.coding_agent_profiles["own"]["A"])
    );
}
