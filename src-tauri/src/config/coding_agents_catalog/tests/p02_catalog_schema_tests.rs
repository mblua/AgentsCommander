use super::*;
use serde_json::json;

#[test]
fn p02_retention_preserves_typed_publication_order_and_fresh_bytes() {
    let definition: CodingAgentDefinition = serde_json::from_value(json!({
        "key":"own", "label":"Own", "description":"d", "color":"#123456", "baseCommand":"tool"
    }))
    .unwrap();
    let definitions = [definition];
    let canonical = json!({"agents":[{"key":"own","baseCommand":"tool"}]});
    assert_eq!(
        build_managed_base_bytes_with_original(&definitions, Some(&canonical)),
        build_managed_base_bytes(&definitions)
    );
    let original = json!({"agents":[{"key":"own","command":"old"}]});
    let managed = build_managed_base_bytes_with_original(&definitions, Some(&original));
    assert!(managed.starts_with(b"{\n  \"schemaVersion\": 1,\n  \"agents\": ["));
    let value: serde_json::Value = serde_json::from_slice(&managed).unwrap();
    assert_eq!(value["agents"][0]["baseCommand"], "tool");
    assert_eq!(value["agents"][0]["command"], "old");

    let row = LocalRowWire {
        key: "own".into(),
        command: Some("tool".into()),
        ..LocalRowWire::default()
    };
    let fresh = concat!(
        "{\n  \"schemaVersion\": 1,\n  \"agents\": [\n    {\n",
        "      \"key\": \"own\",\n      \"baseCommand\": \"tool\"\n",
        "    }\n  ],\n  \"order\": [\n    \"own\"\n  ]\n}\n"
    );
    let order = Some(vec!["own".to_owned()]);
    assert_eq!(
        local_layer_bytes(vec![row.clone()], order.clone()),
        fresh.as_bytes()
    );
    assert_eq!(
        local_layer_bytes_with_original(vec![row.clone()], order.clone(), Some(&canonical)),
        fresh.as_bytes()
    );
    let retained = fresh.replace(
        "\"baseCommand\": \"tool\"\n",
        "\"baseCommand\": \"tool\",\n      \"command\": \"old\"\n",
    );
    assert_eq!(
        local_layer_bytes_with_original(vec![row], order, Some(&original)),
        retained.as_bytes()
    );
}

#[test]
fn p02_catalog_wire_selection_local_precedence_and_historical_ordered_hash() {
    let legacy = json!({"key":"own","label":"Own","description":"d","color":"#123456","command":"tool --fixed"});
    let definition: CodingAgentDefinition = serde_json::from_value(legacy.clone()).unwrap();
    let historical = br##"[{"key":"own","label":"Own","description":"d","color":"#123456","command":"tool --fixed","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]"##;
    assert_eq!(
        managed_content_sha256(std::slice::from_ref(&definition)),
        sha256_hex(historical)
    );
    let mut canonical = legacy.clone();
    canonical["baseCommand"] = json!("tool --fixed");
    canonical.as_object_mut().unwrap().remove("command");
    let new_definition: CodingAgentDefinition = serde_json::from_value(canonical.clone()).unwrap();
    assert_eq!(definition, new_definition);
    assert_eq!(
        managed_content_sha256(std::slice::from_ref(&new_definition)),
        sha256_hex(historical)
    );
    assert_ne!(
        sha256_hex(&serde_json::to_vec(&legacy).unwrap()),
        sha256_hex(&serde_json::to_vec(&canonical).unwrap())
    );
    let wire = serde_json::to_value(&new_definition).unwrap();
    assert_eq!(wire["baseCommand"], "tool --fixed");
    assert!(wire.get("command").is_none());
    let layer = parse_local_layer(
        br#"{"schemaVersion":1,"agents":[{"key":"own","command":"higher"}]}"#,
        LayerDescription::Local,
    )
    .unwrap();
    let composed = compose_local_layer(&[new_definition], &layer, LayerDescription::Local).unwrap();
    assert_eq!(composed[0].command, "higher");
    for version in [1, 2] {
        let root = json!({"schemaVersion":version,"agents":[canonical.clone()]});
        assert_eq!(
            parse_independent_catalog(&serde_json::to_vec(&root).unwrap(), true)
                .unwrap()
                .agents[0]
                .command,
            "tool --fixed"
        );
    }
    for bad in [json!(null), json!(false), json!(42), json!([]), json!({})] {
        for key in ["baseCommand", "command"] {
            let mut mixed = legacy.clone();
            mixed["baseCommand"] = json!("valid");
            mixed[key] = bad.clone();
            assert!(serde_json::from_value::<CodingAgentDefinition>(mixed).is_err());
            let patch = json!({"schemaVersion":1,"agents":[{"key":"own","baseCommand":"valid","command":bad.clone()}]});
            assert!(parse_local_layer(
                &serde_json::to_vec(&patch).unwrap(),
                LayerDescription::Local
            )
            .is_err());
        }
    }
}

#[test]
fn p02_extraction_retains_legacy_and_historical_journal_bytes() {
    let source = br##"{"schemaVersion":1,"agents":[{"key":"own","label":"Own","description":"d","color":"#123456","command":"tool"}]}"##;
    let new_bytes = extract_legacy_local(source, &[]).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&new_bytes).unwrap();
    assert_eq!(value["agents"][0]["baseCommand"], "tool");
    assert_eq!(value["agents"][0]["command"], "tool");
    let historical = extract_legacy_local_mode(source, &[], true).unwrap();
    let old: serde_json::Value = serde_json::from_slice(&historical).unwrap();
    assert_eq!(old["agents"][0]["command"], "tool");
    assert!(old["agents"][0].get("baseCommand").is_none());
    assert_eq!(
        extract_legacy_local_mode(source, &[], true).unwrap(),
        historical
    );
}
