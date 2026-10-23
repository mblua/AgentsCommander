use super::activity_stamp::{
    assert_no_state_key_tracked, instance_config, seed_instance_config, state_beside, T1,
};
use super::*;

// ------------------------------------------------------------------
// #1939 tooling-shape discriminator for config metadata writes.
// ------------------------------------------------------------------

pub(super) fn codex_entry() -> CodingAgentEntry {
    CodingAgentEntry {
        app: "Codex".to_string(),
        ac_session_id: Some("sid".to_string()),
        last_used: T1.to_string(),
        command: String::new(),
        identity: BTreeMap::new(),
    }
}

fn root_config(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

#[test]
fn issue_1937_selection_state_tooling_shape_missing_and_object_succeed_on_both_targets() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    for path in [root_config(dir), instance_config(dir)] {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");

        // Missing file: the write creates the tooling object.
        upsert_config(&path, "codex", &codex_entry(), true).expect("missing config must succeed");
        let saved = state_beside(&path);
        assert_eq!(saved["tooling"]["lastCodingAgent"], "codex");
        assert_no_state_key_tracked(&path);

        // Present object: preserved and updated.
        upsert_config(&path, "claude", &codex_entry(), true).expect("object tooling must succeed");
        let saved = state_beside(&path);
        assert_eq!(saved["tooling"]["lastCodingAgent"], "claude");
        assert_eq!(saved["tooling"]["codingAgents"]["claude"]["app"], "Codex");
        assert_no_state_key_tracked(&path);
    }
}

#[test]
fn issue_1937_selection_state_tooling_shape_non_object_fails_without_rewrite_on_both_targets() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    for path in [root_config(dir), instance_config(dir)] {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        for literal in ["null", "5", "\"tooling\"", "[]", "false"] {
            let original = format!(r#"{{"tooling":{literal},"repos":["repo-a"]}}"#);
            std::fs::write(&path, &original).expect("seed");
            let error = upsert_config(&path, "codex", &codex_entry(), true)
                .expect_err("non-object tooling must fail");
            assert!(
                error.contains("'tooling' must be a JSON object"),
                "{literal}: {error}"
            );
            assert_eq!(
                std::fs::read_to_string(&path).expect("read"),
                original,
                "{literal}: bytes must be preserved"
            );
        }
    }
}

#[test]
fn issue_1937_selection_state_tooling_shape_nested_coding_agents_repairs_on_both_targets() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    for path in [root_config(dir), instance_config(dir)] {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        std::fs::write(
            &path,
            r#"{"tooling":{"lastCodingAgent":"claude","codingAgents":7},"repos":["repo-a"]}"#,
        )
        .expect("seed");
        upsert_config(&path, "codex", &codex_entry(), true).expect("nested repair must succeed");
        let state = state_beside(&path);
        assert_eq!(state["tooling"]["codingAgents"]["codex"]["app"], "Codex");
        assert_eq!(state["tooling"]["lastCodingAgent"], "codex");
        assert_no_state_key_tracked(&path);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(saved["repos"][0], "repo-a");
    }
}

#[test]
fn issue_1937_selection_state_tooling_shape_malformed_selection_locked_is_preserved() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    for path in [root_config(dir), instance_config(dir)] {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        std::fs::write(
            &path,
            concat!(
                r#"{"tooling":{"selectionLocked":"yes","lastCodingAgent":"claude"},"#,
                r#""repos":["repo-a"]}"#
            ),
        )
        .expect("seed");
        upsert_config(&path, "codex", &codex_entry(), true).expect("metadata write must succeed");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(saved["tooling"]["selectionLocked"], "yes");
        assert_eq!(state_beside(&path)["tooling"]["lastCodingAgent"], "codex");
        assert_no_state_key_tracked(&path);
        assert_eq!(saved["repos"][0], "repo-a");
    }
}

#[test]
fn issue_1937_selection_state_tooling_shape_plain_repo_metadata_stays_compatible() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    for path in [root_config(dir), instance_config(dir)] {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        std::fs::write(
            &path,
            concat!(
                r#"{"identity":"../../_agent_dev-rust","repos":["repo-a"],"#,
                r#""tooling":{"lastCodingAgent":"claude"}}"#
            ),
        )
        .expect("seed");
        upsert_config(&path, "codex", &codex_entry(), true)
            .expect("valid plain-repo metadata must stay compatible");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(saved["identity"], "../../_agent_dev-rust");
        assert_eq!(saved["repos"][0], "repo-a");
        assert_eq!(state_beside(&path)["tooling"]["lastCodingAgent"], "codex");
        assert_no_state_key_tracked(&path);
    }
}

// ── #2433 descriptor persistence ──

pub(super) fn fixed_entry(command: &str, identity: &[(&str, &str)]) -> CodingAgentEntry {
    CodingAgentEntry {
        app: "Claude Code".to_string(),
        ac_session_id: Some("sid".to_string()),
        last_used: T1.to_string(),
        command: command.to_string(),
        identity: identity
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

#[test]
fn coding_agent_entry_round_trips_command_and_identity() {
    let entry = fixed_entry("claude --foo", &[("A", "9f2c1ab4de77c001"), ("B", "3e0a")]);
    let value = serde_json::to_value(&entry).expect("serialize");
    assert_eq!(value["command"], serde_json::json!("claude --foo"));
    assert_eq!(
        value["identity"],
        serde_json::json!({"A": "9f2c1ab4de77c001", "B": "3e0a"})
    );
    let back: CodingAgentEntry = serde_json::from_value(value.clone()).expect("deserialize");
    assert_eq!(serde_json::to_value(&back).expect("reserialize"), value);
}

#[test]
fn entry_without_identity_deserializes() {
    let old = serde_json::json!({"app": "Codex", "acSessionId": "s", "lastUsed": T1});
    let entry: CodingAgentEntry = serde_json::from_value(old).expect("old shape loads");
    assert!(entry.identity.is_empty());
    assert!(entry.command.is_empty());
}

#[test]
fn upsert_merges_into_an_existing_entry() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    seed_instance_config(
        dir,
        &serde_json::json!({"tooling": {"codingAgents": {"claude": {
            "app": "Old", "acSessionId": "old-sid", "futureKey": {"x": 1}
        }}}}),
    );
    let mut entry = fixed_entry("claude", &[("A", "aa")]);
    entry.ac_session_id = None;
    upsert_config(&instance_config(dir), "claude", &entry, true).expect("upsert");

    let written = &state_beside(&instance_config(dir))["tooling"]["codingAgents"]["claude"];
    assert_no_state_key_tracked(&instance_config(dir));
    assert_eq!(written["futureKey"], serde_json::json!({"x": 1}));
    assert_eq!(written["command"], serde_json::json!("claude"));
    assert_eq!(written["identity"], serde_json::json!({"A": "aa"}));
    assert_eq!(written["app"], serde_json::json!("Claude Code"));
    // A known optional key this write omits does not linger.
    assert!(written.get("acSessionId").is_none(), "{written}");
}

#[test]
fn agent_with_no_enabled_cells_writes_an_empty_identity_object() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let repo = dir.to_str().expect("utf-8 temp path");
    set_last_coding_agent(
        repo,
        "claude",
        "Claude Code",
        None,
        Some(("claude", &BTreeMap::new())),
    )
    .expect("write");
    for config in [instance_config(dir), root_config(dir)] {
        assert_no_state_key_tracked(&config);
        let value = state_beside(&config);
        let entry = &value["tooling"]["codingAgents"]["claude"];
        assert_eq!(
            entry.get("identity"),
            Some(&serde_json::json!({})),
            "{entry}"
        );
        assert_eq!(entry["command"], serde_json::json!("claude"));
    }
}

#[test]
fn writing_the_same_descriptor_twice_is_byte_identical() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let path = instance_config(dir);
    std::fs::create_dir_all(path.parent().expect("instance dir")).expect("mkdir");
    let entry = fixed_entry("claude --x", &[("A", "aa"), ("C", "cc")]);
    let state = state_file_path(path.parent().expect("instance dir"));
    upsert_config(&path, "claude", &entry, true).expect("first");
    let first = std::fs::read(&state).expect("read first");
    upsert_config(&path, "claude", &entry, true).expect("second");
    assert_eq!(std::fs::read(&state).expect("read second"), first);
    assert_no_state_key_tracked(&path);
}

#[test]
fn write_without_descriptor_leaves_the_existing_descriptor_untouched() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let repo = dir.to_str().expect("utf-8 temp path");
    let identity = BTreeMap::from([("A".to_string(), "aa".to_string())]);
    set_last_coding_agent(repo, "gone", "Gone", None, Some(("gone --x", &identity)))
        .expect("first write");
    set_last_coding_agent(repo, "gone", "Gone", Some("sid2"), None).expect("second write");
    for config in [instance_config(dir), root_config(dir)] {
        assert_no_state_key_tracked(&config);
        let value = state_beside(&config);
        let entry = &value["tooling"]["codingAgents"]["gone"];
        assert_eq!(entry["command"], serde_json::json!("gone --x"), "{entry}");
        assert_eq!(entry["identity"], serde_json::json!({"A": "aa"}), "{entry}");
        assert_eq!(entry["acSessionId"], serde_json::json!("sid2"));
    }
}

#[test]
fn write_without_descriptor_on_a_fresh_entry_writes_no_descriptor_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let repo = dir.to_str().expect("utf-8 temp path");
    set_last_coding_agent(repo, "gone", "Gone", None, None).expect("write");
    let value = state_beside(&instance_config(dir));
    assert_no_state_key_tracked(&instance_config(dir));
    let entry = &value["tooling"]["codingAgents"]["gone"];
    assert!(entry.get("command").is_none(), "{entry}");
    assert!(entry.get("identity").is_none(), "{entry}");
}
