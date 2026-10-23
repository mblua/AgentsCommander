use super::super::*;
use std::collections::BTreeSet;

fn settings_path_in(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("settings.json")
}

fn tip_json(link_url: &str) -> String {
    format!(r#"{{"title":"t","body":"b","link":{{"label":"l","url":"{link_url}"}}}}"#)
}

#[test]
fn the_embedded_agent_help_passes_the_parser() {
    let shipped = parse_agent_help_file(EMBEDDED_AGENT_HELP_JSON).unwrap();
    assert_eq!(&shipped, shipped_agent_help());
    assert_eq!(shipped.schema_version, AGENT_HELP_SCHEMA_VERSION);

    let catalog: Value = serde_json::from_str(include_str!(
        "../../../../resources/coding-agents/agents.default.json"
    ))
    .unwrap();
    let agents = catalog["agents"].as_array().unwrap();
    let expected: BTreeSet<String> = crate::config::coding_agents_catalog::BUILTIN_AGENT_SUPPORT
        .iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(key, _)| {
            let agent = agents
                .iter()
                .find(|agent| agent["key"] == *key)
                .unwrap_or_else(|| panic!("built-in {key} is not in the catalog"));
            let command = agent["baseCommand"].as_str().unwrap();
            crate::config::coding_agents_catalog::command_executable_basename(command).unwrap()
        })
        .collect();
    assert_eq!(expected.len(), 8);
    let actual: BTreeSet<String> = shipped.by_command.keys().cloned().collect();
    assert_eq!(actual, expected);
}

#[test]
fn the_embedded_agent_help_has_no_by_agent_rows_and_no_guide_order() {
    assert!(shipped_agent_help().by_agent.is_empty());
    let serialized = serde_json::to_string(&AgentHelpFile::default()).unwrap();
    assert!(!serialized.contains("\"guideOrder\""), "{serialized}");
    let serialized = serde_json::to_string(shipped_agent_help()).unwrap();
    assert!(!serialized.contains("\"guideOrder\""), "{serialized}");
    // D-A: a present key, empty or not, must not round-trip. An `Option` field with
    // `skip_serializing_if` would keep `Some([])` and `Some([..])` and fail here.
    for input in [
        r#"{"schemaVersion":1,"guideOrder":[]}"#,
        r#"{"schemaVersion":1,"guideOrder":["claude"]}"#,
    ] {
        let parsed = parse_agent_help_file(input).unwrap();
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert!(!serialized.contains("\"guideOrder\""), "{serialized}");
    }
}

#[test]
fn a_json_array_is_rejected() {
    let err = parse_agent_help_file("[]").unwrap_err();
    assert!(err.contains("is not a JSON object"), "{err}");
}

#[test]
fn a_wrong_schema_version_is_rejected() {
    let err = parse_agent_help_file(r#"{"schemaVersion":2}"#).unwrap_err();
    assert!(err.contains('2') && err.contains('1'), "{err}");
    assert!(err.contains("schemaVersion"), "{err}");
}

#[test]
fn a_non_https_docs_url_is_rejected() {
    for url in [
        "http://x/y",
        "javascript:alert(1)",
        "https://",
        "https:///path",
    ] {
        let contents =
            format!(r#"{{"schemaVersion":1,"byCommand":{{"claude":{{"docsUrl":"{url}"}}}}}}"#);
        let err = parse_agent_help_file(&contents).unwrap_err();
        assert_eq!(err, "entry claude: docsUrl is not an https URL", "{url}");
    }
    let ok = r#"{"schemaVersion":1,"byCommand":{"claude":{"docsUrl":"https://x.dev/y"}}}"#;
    assert!(parse_agent_help_file(ok).is_ok());
}

#[test]
fn a_non_https_tip_link_is_rejected() {
    let contents = format!(
        r#"{{"schemaVersion":1,"byAgent":{{"a1":{{"tips":[{},{}]}}}}}}"#,
        tip_json("https://ok.dev"),
        tip_json("http://bad.dev")
    );
    let err = parse_agent_help_file(&contents).unwrap_err();
    assert_eq!(err, "entry a1 tip 1: link url is not an https URL");
    let contents = format!(
        r#"{{"schemaVersion":1,"general":{{"tips":[{}]}}}}"#,
        tip_json("ftp://x")
    );
    let err = parse_agent_help_file(&contents).unwrap_err();
    assert_eq!(err, "entry general tip 0: link url is not an https URL");
}

#[test]
fn unknown_fields_are_ignored() {
    let contents = r#"{"schemaVersion":1,"guideOrder":["x"],"extra":true,
                "byCommand":{"claude":{"label":"C","future":1,"tips":[]}}}"#;
    let file = parse_agent_help_file(contents).unwrap();
    assert_eq!(file.by_command["claude"].label.as_deref(), Some("C"));
}

#[test]
fn a_missing_local_file_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let (file, reason) = load_local_agent_help_file(&settings_path_in(&dir));
    assert_eq!(file, AgentHelpFile::default());
    assert_eq!(reason, None);
}

#[test]
fn a_broken_local_file_is_ignored_whole_and_reports_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = settings_path_in(&dir);
    std::fs::write(
        agent_help_local_path(&settings_path),
        r#"{"schemaVersion":1,"byCommand":{
                    "claude":{"docsUrl":"https://good.dev"},
                    "codex":{"docsUrl":"http://bad.dev"}}}"#,
    )
    .unwrap();
    let (file, reason) = load_local_agent_help_file(&settings_path);
    assert!(file.by_command.is_empty());
    let reason = reason.expect("a broken file must report why");
    assert!(
        reason.contains("entry codex: docsUrl is not an https URL"),
        "{reason}"
    );
}

#[test]
fn a_local_file_has_no_caps() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = settings_path_in(&dir);
    let body = "x".repeat(5000);
    let mut by_command = BTreeMap::new();
    for index in 0..200 {
        by_command.insert(
            format!("cmd{index}"),
            AgentHelpEntry {
                tips: vec![AgentHelpTip {
                    title: "t".to_string(),
                    body: body.clone(),
                    link: None,
                }],
                ..AgentHelpEntry::default()
            },
        );
    }
    let written = AgentHelpFile {
        by_command,
        ..AgentHelpFile::default()
    };
    std::fs::write(
        agent_help_local_path(&settings_path),
        serde_json::to_string(&written).unwrap(),
    )
    .unwrap();
    let (file, reason) = load_local_agent_help_file(&settings_path);
    assert_eq!(reason, None);
    assert_eq!(file.by_command.len(), 200);
    assert_eq!(file, written);
}

fn canonical_shipped_bytes() -> Vec<u8> {
    pretty_json_bytes(shipped_agent_help()).unwrap()
}

#[test]
fn the_shipped_file_is_materialized_canonically() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = settings_path_in(&dir);
    let shipped = agent_help_shipped_path(&settings_path);
    assert_eq!(shipped, dir.path().join("agent-help.json"));

    assert!(refresh_shipped_agent_help_file(&settings_path));
    let written = std::fs::read(&shipped).unwrap();
    assert_eq!(written, canonical_shipped_bytes());
    assert_ne!(written, EMBEDDED_AGENT_HELP_JSON.as_bytes());

    let modified = std::fs::metadata(&shipped).unwrap().modified().unwrap();
    assert!(!refresh_shipped_agent_help_file(&settings_path));
    assert_eq!(std::fs::read(&shipped).unwrap(), written);
    assert_eq!(
        std::fs::metadata(&shipped).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn a_hand_edited_shipped_file_is_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = settings_path_in(&dir);
    let shipped = agent_help_shipped_path(&settings_path);
    std::fs::write(&shipped, r#"{"schemaVersion":1,"note":"mine"}"#).unwrap();

    assert!(refresh_shipped_agent_help_file(&settings_path));
    assert_eq!(std::fs::read(&shipped).unwrap(), canonical_shipped_bytes());
}

#[test]
fn a_write_failure_is_swallowed() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = settings_path_in(&dir);
    // Retain legacy debris; inject refusal at the real publisher after fresh acquisition.
    std::fs::create_dir(
        dir.path()
            .join(format!(".agent-help.json.{}.tmp", std::process::id())),
    )
    .unwrap();

    assert!(!refresh_shipped_agent_help_file_with_publish(
        &settings_path,
        &|tmp, target| {
            assert!(
                tmp.is_file(),
                "fresh staging acquired before injected refusal"
            );
            assert_eq!(target, agent_help_shipped_path(&settings_path));
            Err("fixture temp config publish failure".into())
        },
    ));
    assert!(!agent_help_shipped_path(&settings_path).exists());
}

// ---- #2133 (P6) - the remote validator, caps, cache and stamp. ----

fn remote_with_entries(entries: Vec<(String, AgentHelpEntry)>) -> String {
    let file = AgentHelpFile {
        by_command: entries.into_iter().collect(),
        ..AgentHelpFile::default()
    };
    serde_json::to_string(&file).unwrap()
}

fn labelled(index: usize) -> (String, AgentHelpEntry) {
    (
        format!("cmd{index:02}"),
        AgentHelpEntry {
            label: Some(format!("C{index}")),
            ..AgentHelpEntry::default()
        },
    )
}

fn with_params(params: String) -> String {
    remote_with_entries(vec![(
        "claude".to_string(),
        AgentHelpEntry {
            params_example: Some(params),
            ..AgentHelpEntry::default()
        },
    )])
}

fn tip(body: String) -> AgentHelpTip {
    AgentHelpTip {
        title: "t".to_string(),
        body,
        link: None,
    }
}

fn with_tips(tips: Vec<AgentHelpTip>) -> String {
    remote_with_entries(vec![(
        "claude".to_string(),
        AgentHelpEntry {
            tips,
            ..AgentHelpEntry::default()
        },
    )])
}

#[test]
fn a_by_agent_row_is_rejected_in_the_remote_layer() {
    let contents = r#"{"schemaVersion":1,"byAgent":{"a1":{"label":"x"}}}"#;
    assert!(parse_agent_help_file(contents).is_ok());
    let err = validate_remote_agent_help_file(contents).unwrap_err();
    assert_eq!(err, "byAgent must be empty");
}

#[test]
fn too_many_entries_are_rejected() {
    let forty = remote_with_entries((0..40).map(labelled).collect());
    assert_eq!(
        validate_remote_agent_help_file(&forty)
            .unwrap()
            .by_command
            .len(),
        40
    );
    let forty_one = remote_with_entries((0..41).map(labelled).collect());
    let err = validate_remote_agent_help_file(&forty_one).unwrap_err();
    assert!(err.contains("41 entries"), "{err}");
}

#[test]
fn an_oversized_params_example_is_rejected() {
    assert!(validate_remote_agent_help_file(&with_params("p".repeat(256))).is_ok());
    let err = validate_remote_agent_help_file(&with_params("p".repeat(257))).unwrap_err();
    assert_eq!(
        err,
        "entry claude: paramsExample is 257 bytes; more than 256 bytes are not allowed"
    );
}

#[test]
fn an_oversized_tip_body_is_rejected() {
    assert!(validate_remote_agent_help_file(&with_tips(vec![tip("b".repeat(1200))])).is_ok());
    let err = validate_remote_agent_help_file(&with_tips(vec![
        tip("ok".to_string()),
        tip("b".repeat(1201)),
    ]))
    .unwrap_err();
    assert_eq!(
        err,
        "entry claude tip 1: body is 1201 bytes; more than 1200 bytes are not allowed"
    );
}

#[test]
fn too_many_tips_are_rejected() {
    let twelve = (0..12).map(|_| tip("b".to_string())).collect();
    assert!(validate_remote_agent_help_file(&with_tips(twelve)).is_ok());
    let thirteen = (0..13).map(|_| tip("b".to_string())).collect();
    let err = validate_remote_agent_help_file(&with_tips(thirteen)).unwrap_err();
    assert!(err.starts_with("entry claude: holds 13 tips"), "{err}");
}

#[test]
fn the_caps_are_measured_in_utf8_bytes() {
    let ok = "\u{e9}".repeat(128);
    assert_eq!((ok.chars().count(), ok.len()), (128, 256));
    assert!(validate_remote_agent_help_file(&with_params(ok)).is_ok());
    let over = "\u{e9}".repeat(129);
    assert_eq!(over.len(), 258);
    let err = validate_remote_agent_help_file(&with_params(over)).unwrap_err();
    assert!(err.contains("258 bytes"), "{err}");
}

#[test]
fn one_violation_rejects_the_whole_file() {
    let mut entries: Vec<(String, AgentHelpEntry)> = (0..39).map(labelled).collect();
    entries.push((
        "zz-oversized".to_string(),
        AgentHelpEntry {
            label: Some("L".repeat(81)),
            ..AgentHelpEntry::default()
        },
    ));
    let result = validate_remote_agent_help_file(&remote_with_entries(entries));
    let err = result.expect_err("one bad entry rejects the file, no partial layer");
    assert!(err.starts_with("entry zz-oversized: label"), "{err}");
}

#[test]
fn a_non_200_status_is_rejected() {
    let body = br#"{"schemaVersion":1}"#;
    assert!(accept_remote_agent_help_response(200, body).is_ok());
    let err = accept_remote_agent_help_response(404, body).unwrap_err();
    assert_eq!(err, "HTTP status 404");
}

#[test]
fn an_oversized_body_is_rejected() {
    let mut body = br#"{"schemaVersion":1,"note":""#.to_vec();
    body.resize(REMOTE_AGENT_HELP_MAX_BYTES - 2, b'n');
    body.extend_from_slice(br#""}"#);
    assert_eq!(body.len(), REMOTE_AGENT_HELP_MAX_BYTES);
    assert!(accept_remote_agent_help_response(200, &body).is_ok());
    body.insert(30, b'n');
    let err = accept_remote_agent_help_response(200, &body).unwrap_err();
    assert_eq!(err, "body larger than 65536 bytes");
}

#[test]
fn a_non_utf8_body_is_rejected() {
    let err = accept_remote_agent_help_response(200, &[b'{', 0xff, b'}']).unwrap_err();
    assert_eq!(err, "body is not UTF-8");
}

#[test]
fn a_corrupt_cache_is_revalidated_and_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = settings_path_in(&dir);
    let cache = agent_help_remote_path(&settings_path);
    assert_eq!(cache, dir.path().join("agent-help.remote.json"));
    assert_eq!(
        load_remote_agent_help_file(&settings_path),
        AgentHelpFile::default()
    );

    std::fs::write(&cache, b"{ not json").unwrap();
    assert_eq!(
        load_remote_agent_help_file(&settings_path),
        AgentHelpFile::default()
    );
    // Valid JSON the remote gate rejects: re-validated at read, not just parsed.
    std::fs::write(
        &cache,
        r#"{"schemaVersion":1,"byAgent":{"a":{"label":"x"}}}"#,
    )
    .unwrap();
    assert_eq!(
        load_remote_agent_help_file(&settings_path),
        AgentHelpFile::default()
    );
}

#[test]
fn the_cache_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = settings_path_in(&dir);
    let entry = AgentHelpEntry {
        label: Some("Codex".to_string()),
        docs_url: Some("https://r.dev/codex".to_string()),
        tips: vec![tip("b".to_string())],
        ..AgentHelpEntry::default()
    };
    let written = AgentHelpFile {
        by_command: BTreeMap::from([("codex".to_string(), entry.clone())]),
        ..AgentHelpFile::default()
    };
    let source_url = "https://example.test/agent-help.json";
    let fetched_at = chrono::DateTime::parse_from_rfc3339("2026-09-24T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    write_remote_agent_help_cache(&settings_path, written, source_url, fetched_at).unwrap();

    let loaded = load_remote_agent_help_file(&settings_path);
    assert_eq!(loaded.by_command["codex"], entry);
    let note = loaded.note.expect("the cache carries a note");
    assert!(note.contains(source_url), "{note}");
    assert!(
        note.contains(&format!("source ref {REMOTE_AGENT_HELP_SOURCE_REF}")),
        "{note}"
    );
    assert!(note.contains("2026-09-24T12:00:00Z"), "{note}");
    assert!(note.contains("agent-help.local.json"), "{note}");
}

#[test]
fn the_check_stamp_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = settings_path_in(&dir);
    assert_eq!(
        agent_help_remote_check_path(&settings_path),
        dir.path().join("agent-help-remote-check.json")
    );
    assert_eq!(read_remote_agent_help_check_stamp(&settings_path), None);
    let at = chrono::DateTime::parse_from_rfc3339("2026-09-24T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    write_remote_agent_help_check_stamp(&settings_path, at).unwrap();
    assert_eq!(read_remote_agent_help_check_stamp(&settings_path), Some(at));
}

#[test]
fn the_settings_bool_defaults_to_true() {
    assert!(AppSettings::default().remote_agent_help_enabled);
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    let obj = value.as_object_mut().unwrap();
    assert_eq!(
        obj.remove("remoteAgentHelpEnabled"),
        Some(Value::Bool(true))
    );
    let back: AppSettings = serde_json::from_value(value.clone()).unwrap();
    assert!(back.remote_agent_help_enabled);
    value["remoteAgentHelpEnabled"] = Value::Bool(false);
    let off: AppSettings = serde_json::from_value(value).unwrap();
    assert!(!off.remote_agent_help_enabled);
    assert!(off.remote_blocking_menus_enabled);
}

#[test]
fn the_shipped_agent_help_passes_the_remote_validator() {
    let validated = validate_remote_agent_help_file(EMBEDDED_AGENT_HELP_JSON)
        .expect("the published bytes pass the remote gate as they are");
    assert_eq!(&validated, shipped_agent_help());
}

// ---- #2133 (P7) - the published URL and file. ----

#[test]
fn the_remote_agent_help_url_pins_the_schema_version_and_source_ref() {
    let segments: Vec<&str> = REMOTE_AGENT_HELP_URL.split('/').collect();
    assert_eq!(segments.len(), 10);
    assert_eq!(
        &segments[..5],
        [
            "https:",
            "",
            "raw.githubusercontent.com",
            "mblua",
            "AgentsCommander"
        ]
    );
    assert_eq!(segments[5], REMOTE_AGENT_HELP_SOURCE_REF);
    assert_eq!(segments[8], format!("v{AGENT_HELP_SCHEMA_VERSION}"));
}

#[test]
fn the_remote_agent_help_url_matches_the_served_path() {
    let prefix = format!(
        "https://raw.githubusercontent.com/mblua/AgentsCommander/{REMOTE_AGENT_HELP_SOURCE_REF}/"
    );
    assert_eq!(
        REMOTE_AGENT_HELP_URL.strip_prefix(&prefix),
        Some("remote-resources/agent-help/v1/agent-help.json")
    );
}

#[test]
fn the_published_remote_file_passes_the_validator() {
    let raw = include_str!("../../../../../remote-resources/agent-help/v1/agent-help.json");
    let published = validate_remote_agent_help_file(raw)
        .expect("the published agent-help passes the remote gate");
    assert!(published.by_agent.is_empty());
    assert_eq!(published.by_command, shipped_agent_help().by_command);
    assert_eq!(published.general, shipped_agent_help().general);
}
