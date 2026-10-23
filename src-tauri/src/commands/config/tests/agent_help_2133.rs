use super::super::{agent_help_overlay_payload, AgentHelpOverlayPayload};
use crate::config::settings::shipped_agent_help;
use serde_json::json;

fn payload_with_local(contents: Option<&str>) -> AgentHelpOverlayPayload {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.json");
    if let Some(contents) = contents {
        std::fs::write(dir.path().join("agent-help.local.json"), contents).unwrap();
    }
    agent_help_overlay_payload(Some(settings_path))
}

#[test]
fn agent_help_with_no_local_file_returns_nulls() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!dir.path().join("agent-help.local.json").exists());
    let payload = agent_help_overlay_payload(Some(dir.path().join("settings.json")));
    assert!(payload.local.is_none());
    assert!(payload.remote.is_none());
    assert!(payload.local_error.is_none());
}

#[test]
fn agent_help_serves_the_local_layer() {
    let payload = payload_with_local(Some(
        r#"{"schemaVersion":1,
            "byCommand":{"claude":{"paramsExample":"--mine","docsUrl":"https://mine.dev"}},
            "byAgent":{"agent-1":{"label":"Mine"}}}"#,
    ));
    assert_eq!(payload.local_error, None);
    let local = payload.local.expect("the local layer is served");
    assert_eq!(
        local.by_command["claude"].params_example.as_deref(),
        Some("--mine")
    );
    assert_eq!(
        local.by_command["claude"].docs_url.as_deref(),
        Some("https://mine.dev")
    );
    assert_eq!(local.by_agent["agent-1"].label.as_deref(), Some("Mine"));
}

#[test]
fn agent_help_reports_a_broken_local_file() {
    let payload = payload_with_local(Some("{ not json"));
    assert!(payload.local.is_none());
    let reason = payload.local_error.expect("a broken file reports why");
    assert!(!reason.trim().is_empty());
}

#[test]
fn agent_help_never_serves_the_embedded_layer() {
    let shipped = shipped_agent_help();
    assert!(shipped.general.is_some() || !shipped.by_command.is_empty());
    let payload = payload_with_local(None);
    assert!(payload.local.is_none());
    assert!(payload.remote.is_none());
}

fn payload_with_layers(local: Option<&str>, remote: Option<&str>) -> AgentHelpOverlayPayload {
    let dir = tempfile::tempdir().unwrap();
    if let Some(local) = local {
        std::fs::write(dir.path().join("agent-help.local.json"), local).unwrap();
    }
    if let Some(remote) = remote {
        std::fs::write(dir.path().join("agent-help.remote.json"), remote).unwrap();
    }
    agent_help_overlay_payload(Some(dir.path().join("settings.json")))
}

#[test]
fn agent_help_serves_the_remote_layer() {
    // (a) a valid cache is served, whole entry intact.
    let payload = payload_with_layers(
        None,
        Some(
            r#"{"schemaVersion":1,"byCommand":{"codex":{"label":"Codex",
                "paramsExample":"--full-auto","docsUrl":"https://r.dev/codex",
                "tips":[{"title":"T","body":"B","link":{"label":"L","url":"https://r.dev/t"}}]}}}"#,
        ),
    );
    assert!(payload.local.is_none());
    let remote = serde_json::to_value(payload.remote.expect("remote is served")).unwrap();
    assert_eq!(
        remote["byCommand"]["codex"],
        json!({
            "label": "Codex",
            "paramsExample": "--full-auto",
            "docsUrl": "https://r.dev/codex",
            "tips": [{"title": "T", "body": "B", "link": {"label": "L", "url": "https://r.dev/t"}}]
        })
    );

    // (b) a `general`-only cache is not empty.
    let payload = payload_with_layers(None, Some(r#"{"schemaVersion":1,"general":{"label":"G"}}"#));
    let remote = payload.remote.expect("a general-only cache is served");
    assert_eq!(remote.general.unwrap().label.as_deref(), Some("G"));
    assert!(remote.by_command.is_empty());

    // (c) a cache that parses to an empty file is not served.
    let payload = payload_with_layers(None, Some(r#"{"schemaVersion":1}"#));
    assert!(payload.remote.is_none());
}

#[test]
fn local_still_wins_over_remote_on_the_wire() {
    let payload = payload_with_layers(
        Some(r#"{"schemaVersion":1,"byCommand":{"claude":{"label":"Mine"}}}"#),
        Some(concat!(
            r#"{"schemaVersion":1,"byCommand":{"claude":{"label":"Theirs"},"#,
            r#""codex":{"label":"C"}}}"#,
        )),
    );
    let local = payload.local.expect("local is served");
    let remote = payload.remote.expect("remote is served");
    assert_eq!(local.by_command.len(), 1);
    assert_eq!(local.by_command["claude"].label.as_deref(), Some("Mine"));
    assert_eq!(remote.by_command.len(), 2);
    assert_eq!(remote.by_command["claude"].label.as_deref(), Some("Theirs"));
    assert_eq!(remote.by_command["codex"].label.as_deref(), Some("C"));
    assert_eq!(payload.local_error, None);
}

fn sorted_keys(value: &serde_json::Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn the_payload_serializes_to_camel_case_keys() {
    // A valid local file, read through the injected-path helper.
    let served = payload_with_local(Some(
        r#"{"schemaVersion":1,"byCommand":{"claude":{"label":"L",
            "paramsExample":"--p","docsUrl":"https://d.dev",
            "tips":[{"title":"t","body":"b"}]}}}"#,
    ));
    let value = serde_json::to_value(&served).unwrap();
    assert_eq!(sorted_keys(&value), ["local", "localError", "remote"]);
    assert_eq!(value["localError"], json!(null));
    assert_eq!(value["remote"], json!(null));
    let entry = &value["local"]["byCommand"]["claude"];
    assert_eq!(entry["paramsExample"], json!("--p"));
    assert_eq!(entry["docsUrl"], json!("https://d.dev"));
    assert!(entry.get("params_example").is_none());
    assert!(entry.get("docs_url").is_none());

    // A broken local file, same path: the reason travels as `localError`.
    let broken = payload_with_local(Some(
        r#"{"schemaVersion":1,"byCommand":{"claude":{"docsUrl":"http://d.dev"}}}"#,
    ));
    let value = serde_json::to_value(&broken).unwrap();
    assert_eq!(sorted_keys(&value), ["local", "localError", "remote"]);
    assert_eq!(value["local"], json!(null));
    let reason = value["localError"]
        .as_str()
        .expect("localError is a string");
    assert!(reason.contains("docsUrl is not an https URL"), "{reason}");
    assert!(value.get("local_error").is_none());
}
