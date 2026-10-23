use super::*;
use serde_json::json;

fn agent(pair: Value) -> Value {
    let mut row = json!({"id":"one", "label":"One", "color":"#123456"});
    row.as_object_mut()
        .unwrap()
        .extend(pair.as_object().unwrap().clone());
    row
}

fn selected(new: Option<&str>, old: Option<&str>, expected: &str) {
    for key in ["baseCommand", "additionalArguments"] {
        let mut pair = Map::new();
        if let Some(new) = new {
            pair.insert(key.into(), json!(new));
        }
        if let Some(old) = old {
            pair.insert("command".into(), json!(old));
        }
        if key == "baseCommand" {
            let row: AgentConfig = serde_json::from_value(agent(Value::Object(pair))).unwrap();
            assert_eq!(row.command, expected);
            let wire = serde_json::to_value(row).unwrap();
            assert_eq!(wire[key], expected);
            assert!(wire.get("command").is_none());
        } else {
            let row: ProfileCellConfig = serde_json::from_value(Value::Object(pair)).unwrap();
            assert_eq!(row.command, expected);
            let wire = serde_json::to_value(row).unwrap();
            assert_eq!(wire[key], expected);
            assert!(wire.get("command").is_none());
        }
    }
}

#[test]
fn p02_literal_selection_matrix_and_frozen_whitespace() {
    for (new, old, expected) in [
        (Some("new"), Some("old"), "new"),
        (Some(""), Some("old"), "old"),
        (Some(" "), Some("old"), "old"),
        (Some("new"), Some(" "), "new"),
        (Some(""), Some(""), ""),
        (Some(" "), Some("\t"), " "),
        (Some(""), Some("\t"), ""),
        (None, Some("\t"), "\t"),
        (Some("new"), None, "new"),
        (None, Some("old"), "old"),
        (Some("\u{85}"), Some("legacy"), "\u{85}"),
        (Some("\u{FEFF}"), Some("legacy"), "legacy"),
        (Some("legacy"), Some("\u{85}"), "legacy"),
        (Some(""), Some("\u{85}"), "\u{85}"),
        (Some(""), Some("\u{FEFF}"), ""),
    ] {
        selected(new, old, expected);
    }
    let whitespace = "\u{9}\u{A}\u{B}\u{C}\u{D} \
        \u{A0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\
        \u{2009}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";
    for w in whitespace.chars() {
        let w = w.to_string();
        selected(Some(&w), Some("legacy"), "legacy");
        selected(Some(&w), Some("\t"), &w);
        selected(Some(""), Some(&w), "");
    }
    selected(Some(whitespace), Some("legacy"), "legacy");
    for content in ["\u{85}", "\u{180E}", "\u{200B}", "\u{1F680}", "new"] {
        let mixed = format!("{whitespace}{content}{whitespace}");
        selected(Some(&mixed), Some("legacy"), &mixed);
        selected(Some(""), Some(&mixed), &mixed);
    }
}

#[test]
fn p02_present_malformed_peer_rejects_and_absence_keeps_required_default() {
    assert!(serde_json::from_value::<AgentConfig>(agent(json!({}))).is_err());
    assert_eq!(
        serde_json::from_value::<ProfileCellConfig>(json!({}))
            .unwrap()
            .command,
        ""
    );
    for bad in [Value::Null, json!(false), json!(17), json!([]), json!({})] {
        for canonical_bad in [false, true] {
            for key in ["baseCommand", "additionalArguments"] {
                let mut pair = json!({"command":"old"});
                pair[key] = json!("new");
                pair[if canonical_bad { key } else { "command" }] = bad.clone();
                if key == "baseCommand" {
                    assert!(serde_json::from_value::<AgentConfig>(agent(pair)).is_err());
                } else {
                    assert!(serde_json::from_value::<ProfileCellConfig>(pair).is_err());
                }
            }
        }
    }
}

#[test]
fn p02_retention_uses_authoritative_legacy_by_identity_and_is_fixed_point() {
    let before = json!({"agents":[{"id":"one","command":"locked"}],"codingAgentProfiles":{"profiles\
        ByAgent":{"one":{"A":{"command":"--locked"}}}}});
    let mut after = json!({"agents":[{"id":"fresh","baseCommand":"fresh","command":"forged"},{"i\
        d":"one","baseCommand":"edited","command":"stale"}],"codingAgentProfiles":{"profilesByAgen\
        t":{"one":{"A":{"additionalArguments":"--edited","command":"--stale"},"B":{"command":"--ne\
        w"}}}}});
    retain_configuration_commands(&mut after, &before, "id", false, true, true).unwrap();
    assert_eq!(after["agents"][1]["command"], "locked");
    assert_eq!(after["agents"][1]["baseCommand"], "edited");
    assert!(after["agents"][0].get("command").is_none());
    let cells = &after["codingAgentProfiles"]["profilesByAgent"]["one"];
    assert_eq!(cells["A"]["command"], "--locked");
    assert_eq!(cells["A"]["additionalArguments"], "--edited");
    assert_eq!(cells["B"]["additionalArguments"], "--new");
    assert!(cells["B"].get("command").is_none());
    let saved = after.clone();
    retain_configuration_commands(&mut after, &saved, "id", false, true, true).unwrap();
    assert_eq!(after, saved);
    let mut malformed = before.clone();
    malformed["agents"][0]["command"] = Value::Null;
    assert!(
        retain_configuration_commands(&mut after, &malformed, "id", false, true, true).is_err()
    );
}

#[test]
fn p02_overlay_pairs_resolve_per_layer_restore_raw_and_reapply_effective() {
    for (base, overlay, expected) in [
        (
            json!({"additionalArguments":"lower"}),
            json!({"command":"higher"}),
            "higher",
        ),
        (
            json!({"command":"lower"}),
            json!({"additionalArguments":"higher"}),
            "higher",
        ),
        (
            json!({"additionalArguments":"lower"}),
            json!({"command":""}),
            "",
        ),
        (
            json!({"command":"lower"}),
            json!({"additionalArguments":" ","command":"\t"}),
            " ",
        ),
    ] {
        let original = json!({"codingAgentProfiles":{"profilesByAgent":{"one":{"A":base}}}});
        let mut merged = original.clone();
        let overlay = json!({"codingAgentProfiles":{"profilesByAgent":{"one":{"A":overlay}}}});
        let state = LocalSettingsOverlay::from_overlay_object_paired(
            &mut merged,
            overlay.as_object().unwrap().clone(),
            &[],
            &[],
            &[],
            CONFIGURATION_COMMAND_PAIRS,
        );
        assert!(state.rejection().is_none());
        let cell = &merged["codingAgentProfiles"]["profilesByAgent"]["one"]["A"];
        let decoded: ProfileCellConfig = serde_json::from_value(cell.clone()).unwrap();
        assert_eq!(decoded.command, expected);
        let mut physical = merged.as_object().unwrap().clone();
        state.restore_base(&mut physical);
        assert_eq!(Value::Object(physical.clone()), original);
        state.reapply_from(merged.as_object().unwrap(), &mut physical);
        assert_eq!(Value::Object(physical), merged);
    }
}
