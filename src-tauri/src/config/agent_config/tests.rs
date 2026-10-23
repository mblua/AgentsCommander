use super::*;
use std::path::PathBuf;

#[test]
fn prepared_identity_pair_serializer_cleanup_and_split_parity() {
    use crate::config::local_config_io::{read_config_pair_physical, PhysicalState};
    let decision_seeds = [
        None,
        Some(
            r#"{"unknown":[1],"tooling":{"lastCodingAgent":"tracked","selectionLocked":true,"configurationRef":{"keep":1}}}"#,
        ),
    ];
    let mut marked = serde_json::json!({"unknownState":null,"tooling":{"lastCodingAgent":"state","codingAgents":{"a":{"unknown":7}}}});
    stamp_split_marker(marked.as_object_mut().unwrap());
    let marked = marked.to_string();
    for decisions_seed in decision_seeds {
        for state_seed in [
            None,
            Some(
                r#"{"unknownState":null,"tooling":{"lastCodingAgent":"state","codingAgents":{"a":{"unknown":7}}}}"#,
            ),
            Some(marked.as_str()),
        ] {
            for changed in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let old_dir = temp.path().join("old");
                let new_dir = temp.path().join("new");
                std::fs::create_dir(&old_dir).unwrap();
                std::fs::create_dir(&new_dir).unwrap();
                let old = old_dir.join("config.json");
                let new = new_dir.join("config.json");
                for dir in [&old_dir, &new_dir] {
                    if let Some(bytes) = decisions_seed {
                        std::fs::write(dir.join("config.json"), bytes).unwrap();
                    }
                    if let Some(bytes) = state_seed {
                        std::fs::write(state_file_path(dir), bytes).unwrap();
                    }
                }
                let before = [
                    read_config_pair_physical(&new).unwrap(),
                    read_config_pair_physical(&state_file_path(&new_dir)).unwrap(),
                ];
                let mutate = |d: &mut JsonMap, s: &mut JsonMap| {
                    if changed {
                        d.entry("tooling")
                            .or_insert_with(|| serde_json::json!({}))
                            .as_object_mut()
                            .unwrap()
                            .insert(
                                "configurationRef".into(),
                                serde_json::json!({"version":1,"owned":"new"}),
                            );
                        s.entry("tooling")
                            .or_insert_with(|| serde_json::json!({}))
                            .as_object_mut()
                            .unwrap()
                            .insert("lastCodingAgent".into(), serde_json::json!("new"));
                    }
                    Ok(())
                };
                let plan = prepare_identity_reference_pair(before.clone(), mutate).unwrap();
                assert_eq!(
                    [
                        read_config_pair_physical(&new).unwrap(),
                        read_config_pair_physical(&state_file_path(&new_dir)).unwrap()
                    ],
                    before
                );
                assert_eq!(
                    std::fs::read_dir(&new_dir).unwrap().count(),
                    usize::from(decisions_seed.is_some()) + usize::from(state_seed.is_some())
                );
                let observed = std::cell::RefCell::new(Vec::new());
                update_agent_config_with_stage(
                    &old,
                    Some(&temp.path().join("journal")),
                    &|stage| {
                        let index = match stage {
                            "after_cleanup_state_publish" => 1,
                            "after_cleanup_tracked_write" => 2,
                            "after_caller_state_publish" => 3,
                            "after_caller_decisions_write" => 4,
                            _ => panic!("unknown stage"),
                        };
                        observed.borrow_mut().push((
                            index,
                            [
                                read_config_pair_physical(&old).unwrap(),
                                read_config_pair_physical(&state_file_path(&old_dir)).unwrap(),
                            ],
                        ));
                    },
                    mutate,
                )
                .unwrap();
                for (index, tuple) in observed.into_inner() {
                    assert_eq!(tuple, plan.stages()[index]);
                }
                execute_identity_reference_pair(&new, &plan).unwrap();
                assert_eq!(
                    [
                        read_config_pair_physical(&new).unwrap(),
                        read_config_pair_physical(&state_file_path(&new_dir)).unwrap()
                    ],
                    [
                        read_config_pair_physical(&old).unwrap(),
                        read_config_pair_physical(&state_file_path(&old_dir)).unwrap()
                    ]
                );
                if let PhysicalState::Bytes { bytes, .. } = &plan.stages()[4][0] {
                    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                    if decisions_seed.is_some() {
                        assert_eq!(value["unknown"], serde_json::json!([1]));
                        assert_eq!(value["tooling"]["selectionLocked"], true);
                    }
                }
            }
        }
    }
}

#[test]
fn prepared_identity_malformed_tooling_zero_write() {
    use crate::config::local_config_io::PhysicalState;
    for (d, s) in [
        (r#"{"tooling":null}"#, r#"{}"#),
        (r#"{}"#, r#"{"tooling":[]}"#),
    ] {
        assert!(prepare_identity_reference_pair(
            [
                PhysicalState::from_bytes(d.as_bytes().to_vec()),
                PhysicalState::from_bytes(s.as_bytes().to_vec())
            ],
            |_, _| panic!("cleanup must reject before caller")
        )
        .is_err());
    }
}

const T1: &str = "2026-09-02T01:00:00+00:00";
const T2: &str = "2026-09-02T02:00:00+00:00";
const T3: &str = "2026-09-02T03:00:00+00:00";

/// Path of the per-instance config the writer under test targets.
fn instance_config(dir: &Path) -> PathBuf {
    dir.join(crate::config::agent_local_dir_name())
        .join("config.json")
}

/// Seed `dir`'s per-instance config with `value`, creating the instance dir.
fn seed_instance_config(dir: &Path, value: &serde_json::Value) {
    let path = instance_config(dir);
    std::fs::create_dir_all(path.parent().expect("instance dir")).expect("create instance dir");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(value).expect("serialize"),
    )
    .expect("seed config");
}

/// Raw JSON currently stored in `dir`'s per-instance config.
fn stored(dir: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(instance_config(dir)).expect("read stored config");
    serde_json::from_str(&raw).expect("stored config is JSON")
}

/// #2816 (C3) - raw JSON of the state file beside the tracked `config`.
fn state_beside(config: &Path) -> serde_json::Value {
    let path = state_file_path(config.parent().expect("config dir"));
    let raw = std::fs::read_to_string(path).expect("read state file");
    serde_json::from_str(&raw).expect("state file is JSON")
}

/// #2816 (C3) - no state key may remain in the tracked `config`; an absent
/// tracked file holds none.
fn assert_no_state_key_tracked(config: &Path) {
    let Ok(raw) = std::fs::read_to_string(config) else {
        return;
    };
    let tracked: serde_json::Value = serde_json::from_str(&raw).expect("tracked config is JSON");
    for key in STATE_KEYS {
        assert!(
            tracked["tooling"].get(key).is_none(),
            "{key} is still tracked in {}",
            config.display()
        );
    }
}

fn set(dir: &Path, at: &str) -> Result<bool, String> {
    set_last_agent_message_at(dir.to_str().expect("utf-8 temp path"), at)
}

fn read(dir: &Path) -> Option<String> {
    read_last_agent_message_at(dir.to_str().expect("utf-8 temp path"))
}

#[test]
fn set_last_agent_message_at_writes_only_the_instance_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    assert_eq!(set(dir, T2), Ok(true));

    assert_eq!(
        state_beside(&instance_config(dir))["tooling"]["lastAgentMessageAt"],
        serde_json::json!(T2)
    );
    assert_no_state_key_tracked(&instance_config(dir));
    // D2: the root copy is deliberately not a write target for this stamp.
    assert!(
        !dir.join("config.json").exists(),
        "the root config must not be written"
    );
}

#[test]
fn set_last_agent_message_at_preserves_existing_tooling_and_unknown_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    seed_instance_config(
        dir,
        &serde_json::json!({
            "tooling": {
                "lastCodingAgent": "claude",
                "codingAgents": {
                    "claude": { "app": "Claude Code", "lastUsed": T1 }
                }
            },
            "repos": ["repo-AgentsCommander"]
        }),
    );

    assert_eq!(set(dir, T2), Ok(true));

    let after = state_beside(&instance_config(dir));
    assert_no_state_key_tracked(&instance_config(dir));
    assert_eq!(
        after["tooling"]["lastCodingAgent"],
        serde_json::json!("claude")
    );
    assert_eq!(
        after["tooling"]["codingAgents"]["claude"],
        serde_json::json!({ "app": "Claude Code", "lastUsed": T1 })
    );
    assert_eq!(
        stored(dir)["repos"],
        serde_json::json!(["repo-AgentsCommander"])
    );
    assert_eq!(
        after["tooling"]["lastAgentMessageAt"],
        serde_json::json!(T2)
    );
}

#[test]
fn set_last_agent_message_at_is_monotonic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    assert_eq!(set(dir, T2), Ok(true));

    // Older instant: skipped, stored value untouched.
    assert_eq!(set(dir, T1), Ok(false));
    assert_eq!(read(dir), Some(T2.to_string()));

    // Newer instant: written.
    assert_eq!(set(dir, T3), Ok(true));
    assert_eq!(read(dir), Some(T3.to_string()));

    // An unparseable stored value is not a reason to skip.
    seed_instance_config(
        dir,
        &serde_json::json!({ "tooling": { "lastAgentMessageAt": "not-a-timestamp" } }),
    );
    assert_eq!(set(dir, T1), Ok(true));
    assert_eq!(read(dir), Some(T1.to_string()));
}

#[test]
fn a_session_restart_rewrite_preserves_the_stamp() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let repo = dir.to_str().expect("utf-8 temp path");

    assert_eq!(set(dir, T2), Ok(true));
    set_last_coding_agent(repo, "claude", "Claude Code", Some("sid"), None)
        .expect("restart rewrite");

    assert_eq!(read(dir), Some(T2.to_string()));
    assert_eq!(
        state_beside(&instance_config(dir))["tooling"]["lastCodingAgent"],
        serde_json::json!("claude")
    );
    assert_no_state_key_tracked(&instance_config(dir));
}

#[test]
fn read_last_agent_message_at_binds_present_and_absent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    // Absent file.
    assert_eq!(read(dir), None);

    // Invalid JSON.
    let path = instance_config(dir);
    std::fs::create_dir_all(path.parent().expect("instance dir")).expect("create instance dir");
    std::fs::write(&path, "{ not json").expect("write invalid json");
    assert_eq!(read(dir), None);

    // Tooling absent.
    seed_instance_config(dir, &serde_json::json!({}));
    assert_eq!(read(dir), None);

    // Tooling present without the key.
    seed_instance_config(
        dir,
        &serde_json::json!({ "tooling": { "lastCodingAgent": "claude" } }),
    );
    assert_eq!(read(dir), None);

    // Tooling set to a non-object.
    seed_instance_config(dir, &serde_json::json!({ "tooling": 5 }));
    assert_eq!(read(dir), None);

    // #2816 (C3) consequence 5: the write on that seed is rejected by the
    // pair's cleanup and the tracked file is left byte for byte.
    let tracked = instance_config(dir);
    assert!(
        !state_file_path(tracked.parent().expect("instance dir")).exists(),
        "no state file, so the tracked check is the one measured"
    );
    let before = std::fs::read(&tracked).expect("tracked bytes before");
    let error = set(dir, T2).expect_err("a malformed tracked tooling is rejected");
    assert!(
        error
            .contains("cleanup failed: 'tooling' must be a JSON object in the tracked config file"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(&tracked).expect("tracked bytes after"),
        before
    );
    assert_eq!(stored(dir)["tooling"], serde_json::json!(5));

    // Control: a well-formed value is read back.
    seed_instance_config(dir, &serde_json::json!({}));
    assert_eq!(set(dir, T2), Ok(true));
    assert_eq!(read(dir), Some(T2.to_string()));
}

#[test]
fn is_empty_tracks_the_stamp() {
    assert!(AgentTooling::default().is_empty());
    assert!(!AgentTooling {
        last_agent_message_at: Some(T2.to_string()),
        ..Default::default()
    }
    .is_empty());
}

// ------------------------------------------------------------------
// #1939 tooling-shape discriminator for config metadata writes.
// ------------------------------------------------------------------

fn codex_entry() -> CodingAgentEntry {
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
            r#"{"tooling":{"selectionLocked":"yes","lastCodingAgent":"claude"},"repos":["repo-a"]}"#,
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
            r#"{"identity":"../../_agent_dev-rust","repos":["repo-a"],"tooling":{"lastCodingAgent":"claude"}}"#,
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

fn fixed_entry(command: &str, identity: &[(&str, &str)]) -> CodingAgentEntry {
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

/// #2786 (C1) E7 - seed `config.json` and the state file in a fresh dir.
fn loader_fixture(
    decisions: Option<serde_json::Value>,
    state: Option<serde_json::Value>,
) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    if let Some(value) = decisions {
        std::fs::write(temp.path().join("config.json"), value.to_string()).expect("seed decisions");
    }
    if let Some(value) = state {
        std::fs::write(
            temp.path()
                .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            value.to_string(),
        )
        .expect("seed state");
    }
    temp
}

/// Every file in `dir` with its bytes, to prove the loader wrote nothing.
fn dir_bytes(dir: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| {
            let entry = entry.expect("entry");
            (
                entry.file_name(),
                std::fs::read(entry.path()).expect("read"),
            )
        })
        .collect();
    files.sort();
    files
}

/// The four state keys, read through the loader, as one JSON object.
fn loaded_state_keys(dir: &Path) -> serde_json::Value {
    let before = dir_bytes(dir);
    let config = read_agent_local_config(dir).expect("the loader returns a config");
    let raw = read_agent_local_config_json(dir)
        .expect("parseable")
        .expect("present");
    assert_eq!(dir_bytes(dir), before, "the loader must write nothing");
    serde_json::json!({
        "lastCodingAgent": config.tooling.last_coding_agent,
        "codingAgents": config.tooling.coding_agents.keys().collect::<Vec<_>>(),
        "lastAgentMessageAt": config.tooling.last_agent_message_at,
        "profileContentHash": raw["tooling"]["profileContentHash"],
    })
}

fn all_four(tag: &str) -> serde_json::Value {
    serde_json::json!({
        "lastCodingAgent": format!("{tag}-agent"),
        "codingAgents": {format!("{tag}-agent"): {"app": tag}},
        "lastAgentMessageAt": format!("{tag}-at"),
        "profileContentHash": format!("{tag}-hash"),
    })
}

fn expect_all_four(tag: &str) -> serde_json::Value {
    serde_json::json!({
        "lastCodingAgent": format!("{tag}-agent"),
        "codingAgents": [format!("{tag}-agent")],
        "lastAgentMessageAt": format!("{tag}-at"),
        "profileContentHash": format!("{tag}-hash"),
    })
}

#[test]
fn the_loader_is_key_wise_and_marker_aware() {
    let marker = serde_json::json!({"v": 1, "keys": STATE_KEYS});

    // 1. State only.
    let temp = loader_fixture(
        None,
        Some(serde_json::json!({"tooling": all_four("state")})),
    );
    assert_eq!(loaded_state_keys(temp.path()), expect_all_four("state"));

    // 2. Decisions only, with a non-state key the loader must keep.
    let mut decisions = all_four("tracked");
    decisions["telegramBot"] = serde_json::json!("bot");
    let temp = loader_fixture(Some(serde_json::json!({"tooling": decisions})), None);
    assert_eq!(loaded_state_keys(temp.path()), expect_all_four("tracked"));
    assert_eq!(
        read_agent_local_config(temp.path())
            .expect("config")
            .tooling
            .telegram_bot
            .as_deref(),
        Some("bot")
    );

    // 3. A state file with two of the four keys hides none of the others.
    let temp = loader_fixture(
        Some(serde_json::json!({"tooling": all_four("tracked")})),
        Some(serde_json::json!({"tooling": {
            "lastCodingAgent": "state-agent",
            "profileContentHash": "state-hash",
        }})),
    );
    assert_eq!(
        loaded_state_keys(temp.path()),
        serde_json::json!({
            "lastCodingAgent": "state-agent",
            "codingAgents": ["tracked-agent"],
            "lastAgentMessageAt": "tracked-at",
            "profileContentHash": "state-hash",
        })
    );

    // 4. Both files, marker present: the tracked value wins.
    let temp = loader_fixture(
        Some(serde_json::json!({"tooling": all_four("tracked")})),
        Some(serde_json::json!({"tooling": all_four("state"), "split": marker})),
    );
    assert_eq!(loaded_state_keys(temp.path()), expect_all_four("tracked"));

    // 5. Both files, no marker: the state file wins.
    let temp = loader_fixture(
        Some(serde_json::json!({"tooling": all_four("tracked")})),
        Some(serde_json::json!({"tooling": all_four("state")})),
    );
    assert_eq!(loaded_state_keys(temp.path()), expect_all_four("state"));

    // 6. Marker shapes: only an integer `v` >= 1 is present.
    let shapes = [
        (serde_json::json!(null), "state"),
        (serde_json::json!(3), "state"),
        (serde_json::json!({"v": "1"}), "state"),
        (serde_json::json!({}), "state"),
        (serde_json::json!({"v": 2, "keys": []}), "tracked"),
    ];
    for (shape, winner) in shapes {
        let temp = loader_fixture(
            Some(serde_json::json!({"tooling": all_four("tracked")})),
            Some(serde_json::json!({"tooling": all_four("state"), "split": shape.clone()})),
        );
        assert_eq!(
            loaded_state_keys(temp.path()),
            expect_all_four(winner),
            "split = {shape}"
        );
    }
}

/// #2786 (C1) - a duplicate known field is still a typed rejection, as it
/// was for the direct typed readers: parsing through `Value` must not
/// collapse it to the last value. A state file with one is ignored.
#[test]
fn the_typed_loader_rejects_duplicate_known_fields() {
    let duplicate = r#"{"tooling":{"lastCodingAgent":"a","lastCodingAgent":"b"}}"#;
    assert!(serde_json::from_str::<AgentLocalConfig>(duplicate).is_err());

    let temp = loader_fixture(None, None);
    std::fs::write(temp.path().join("config.json"), duplicate).expect("seed decisions");
    assert!(read_agent_local_config(temp.path()).is_none());

    let temp = loader_fixture(
        Some(serde_json::json!({"tooling": {"lastCodingAgent": "tracked"}})),
        None,
    );
    std::fs::write(
        temp.path()
            .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
        duplicate,
    )
    .expect("seed state");
    assert_eq!(
        read_agent_local_config(temp.path())
            .expect("config")
            .tooling
            .last_coding_agent
            .as_deref(),
        Some("tracked")
    );
}

/// #2786 C1 E7e: the raw wrapper filters the STATE file through the typed
/// parse and leaves `config.json` on its base `Value` semantics.
#[test]
fn the_raw_loader_filters_the_state_file_but_not_the_tracked_file() {
    let state_name = crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME;
    let raw = |dir: &Path| {
        read_agent_local_config_json(dir)
            .expect("json")
            .expect("some")
    };

    // (1) a tracked duplicate is last wins, with no state file.
    let temp = loader_fixture(None, None);
    std::fs::write(
        temp.path().join("config.json"),
        r#"{"tooling":{"lastCodingAgent":"a","lastCodingAgent":"b"}}"#,
    )
    .expect("seed decisions");
    assert_eq!(raw(temp.path())["tooling"]["lastCodingAgent"], "b");

    // (2) an unrelated malformed tracked field is tolerated.
    let temp = loader_fixture(
        Some(serde_json::json!({"tooling": {"lastCodingAgent": "claude", "codingAgents": null}})),
        None,
    );
    assert_eq!(raw(temp.path())["tooling"]["lastCodingAgent"], "claude");

    // (3) and (4): a state file that fails the typed parse is ignored and
    // the tracked value serves the key.
    let tracked = serde_json::json!({"tooling": {
        "lastCodingAgent": "tracked",
        "codingAgents": {"tracked": {"command": "tracked"}}}});
    for (leg, state) in [
        (
            "3 codingAgents null",
            r#"{"tooling":{"codingAgents":null}}"#,
        ),
        (
            "4 duplicate known field",
            r#"{"tooling":{"lastCodingAgent":"a","lastCodingAgent":"b"}}"#,
        ),
    ] {
        let temp = loader_fixture(Some(tracked.clone()), None);
        std::fs::write(temp.path().join(state_name), state).expect("seed state");
        assert_eq!(raw(temp.path())["tooling"], tracked["tooling"], "leg {leg}");
    }

    // (5) control: a well-formed state file for the same key does overlay.
    let temp = loader_fixture(
        Some(tracked.clone()),
        Some(serde_json::json!({"tooling": {"lastCodingAgent": "state"}})),
    );
    assert_eq!(raw(temp.path())["tooling"]["lastCodingAgent"], "state");
}

// ---- #2786 C1 r28: harness child-ownership rows ----
// These are NOT mutant classes of E15. They prove the harness itself never
// leaves a child behind when supervision fails, and they drive a BLOCKING
// inner row, so the child is certainly alive when the injected fault fires.

/// Blocks until the harness terminates it. It is armed, but it loads
/// nothing, so it opens no observation and records nothing.
#[test]
#[ignore = "#2786 C1: driven only by the harness child-ownership rows"]
fn inner_sleeps_until_killed() {
    println!("SLEEPER-READY");
    std::thread::sleep(std::time::Duration::from_secs(120));
}

#[test]
fn an_injected_wait_error_still_terminates_and_reaps_the_child() {
    use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
    let r = run_inner_with_fault(SLEEPER, Fault::WaitError);
    let reported = r
        .wait_error
        .as_deref()
        .expect("the injected wait error must be reported, not discarded");
    assert!(reported.contains("injected try_wait failure"), "{reported}");
    let c = r
        .cleanup
        .as_ref()
        .expect("an errored wait must hand back a cleanup report");
    assert!(
        c.reaped_ok(),
        "the child was not reaped after the wait error: {}",
        c.describe()
    );
    assert!(
        c.kill_error.is_none(),
        "terminating the child failed, and the harness says so: {}",
        c.describe()
    );
    assert!(!r.ok, "a supervision failure is never a green child");
}

#[test]
fn a_timed_out_child_is_terminated_and_reaped() {
    use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
    let r = run_inner_with_fault(SLEEPER, Fault::Timeout);
    assert!(r.timed_out, "the past deadline must time the child out");
    assert!(r.wait_error.is_none(), "{:?}", r.wait_error);
    let c = r
        .cleanup
        .as_ref()
        .expect("a timeout must hand back a cleanup report");
    assert!(
        c.reaped_ok(),
        "the child was not reaped after the timeout: {}",
        c.describe()
    );
    // r29 item 3: this row checks the KILL error report too, on its own,
    // exactly as the wait-error row does. Without it, a harness that
    // reported a failed termination still passed this row in silence. The
    // check is deliberately NOT in the shared `assert_one_test_ran`: two
    // redundant guards hide each other from mutation.
    assert!(
        c.kill_error.is_none(),
        "terminating the timed-out child reported an error: {}",
        c.describe()
    );
    assert!(!r.ok, "a timed-out child is never a green child");
}

// ---- #2786 C1 r30 item 5: DIRECT controls on the failed-kill branch ----
// Each one drives the blocking sleeper, injects its fault, and asserts the
// OUTCOME, the BOUND, the row's RED verdict and the pid's state before and
// after cleanup. The report-injection control k1 is kept as it was: it
// proves the REPORTING, never this branch.

/// The shared part: assert that `assert_one_test_ran`, the very assertion
/// every observation row uses, rejects this run. That is what "the row is
/// RED" means; nothing weaker and no new assertion.
fn assert_the_shared_gate_rejects(r: &load_probe_harness::ChildRun, needle: &str) {
    use load_probe_harness::assert_one_test_ran;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = std::panic::catch_unwind(|| {
        assert_one_test_ran(r, load_probe_harness::SLEEPER);
    });
    std::panic::set_hook(hook);
    let err = caught.expect_err("the shared gate must reject this run");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_else(|| {
        err.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .unwrap_or_default()
    });
    assert!(
        msg.contains(needle),
        "the shared gate rejected it for the wrong reason, no {needle:?} in: {msg}"
    );
    println!("ROW-RED shared gate rejected the run: {msg}");
}

/// r31 F4: the cleanup guard, adopted as the FIRST statement of a control
/// row, before any fallible assertion.
///
/// r30's Pending and Failed rows ran every assertion before they killed
/// anything, so the dev's 6 s mutant panicked with the 120 s sleeper still
/// running and nothing left to clean it up. This guard takes the still-
/// owned `Child` out of the report at the top of the row. From then on it
/// owns the process on EVERY exit path: the normal one, where the row
/// calls `prove_alive_then_reap`, and the panic one, where `Drop` kills
/// and reaps it inside a bounded poll.
///
/// It owns a real `Child`, not a pid. A pid can only be killed, and on a
/// Unix host killing this process's own child without waiting leaves a
/// zombie that `ps` still reports. `wait()` on the handle is the portable
/// reap, on both platforms, with no external command and no dependency.
struct AdoptedChild {
    pid: u32,
    child: Option<std::process::Child>,
}

impl AdoptedChild {
    fn adopt(r: &mut load_probe_harness::ChildRun) -> AdoptedChild {
        match r.cleanup.as_mut() {
            Some(c) => AdoptedChild {
                pid: c.pid,
                child: c.unreaped.take(),
            },
            None => AdoptedChild {
                pid: 0,
                child: None,
            },
        }
    }

    /// Prove the pid is ALIVE, prove the detector can also report a query
    /// it could not answer, then reap through the owned handle and prove
    /// the pid is gone. All four legs, every time, bounded.
    ///
    /// Why this is not the shell teardown artefact the r29 evidence hit:
    /// the `Alive` reading is taken here, inside the test process that
    /// spawned the child and which is still running, before anything is
    /// torn down. No command substitution, no wrapper shell, no job object.
    fn prove_alive_then_reap(&mut self) {
        use load_probe_harness::{bounded_kill_and_reap, pid_state, pid_state_by_filter};
        use load_probe_harness::{PidState, BAD_PID_SELECTOR};
        let pid = self.pid;
        let mut child = self
            .child
            .take()
            .expect("#2786 C1 r31 F4: the guard must own the un-reaped child");
        let before = pid_state(pid);
        assert_eq!(
            before,
            PidState::Alive,
            "the injected failed kill must leave pid {pid} running; the detector said                  {before:?}"
        );
        println!("PID-BEFORE-CLEANUP pid={pid} state={before:?} (the harness did NOT reap it)");
        let bad = pid_state_by_filter(BAD_PID_SELECTOR);
        assert!(
            matches!(bad, PidState::QueryError(_)),
            "a malformed query must be a query error, not a verdict: {bad:?}"
        );
        println!("PID-QUERY-ERROR-CONTROL {bad:?}");
        let report = bounded_kill_and_reap(&mut child, pid, std::time::Duration::from_secs(10));
        println!("OWNED-REAP {report}");
        assert!(
            report.contains("REAPED status"),
            "the owned handle must really reap the child: {report}"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut after = pid_state(pid);
        while after == PidState::Alive && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            after = pid_state(pid);
        }
        assert_eq!(
            after,
            PidState::Dead,
            "pid {pid} survived the owned reap: {after:?}"
        );
        println!("PID-AFTER-CLEANUP pid={pid} state={after:?}");
    }
}

impl Drop for AdoptedChild {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let r = load_probe_harness::bounded_kill_and_reap(
                &mut c,
                self.pid,
                std::time::Duration::from_secs(10),
            );
            eprintln!("#2786 C1 r31 F4: ADOPTED-CLEANUP on the unwind path: {r}");
        }
    }
}

#[test]
fn a_failed_kill_with_a_live_child_is_reported_as_pending_within_the_budget() {
    use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
    let t0 = std::time::Instant::now();
    let mut r = run_inner_with_fault(SLEEPER, Fault::KillFails);
    let elapsed = t0.elapsed();
    // r31 F4: FIRST, before a single fallible assertion. From here the
    // guard owns the surviving child on every exit path, panic included.
    let mut adopted = AdoptedChild::adopt(&mut r);
    assert!(
        adopted.child.is_some(),
        "the report must hand over the un-reaped child, not just its pid"
    );
    let c = r
        .cleanup
        .as_ref()
        .expect("a failed kill must hand back a cleanup report");
    assert_eq!(
        c.outcome.label(),
        "CLEANUP-PENDING",
        "a failed kill on a live child must be PENDING: {}",
        c.describe()
    );
    assert!(
        !c.reaped_ok(),
        "PENDING is never a reaped child: {}",
        c.describe()
    );
    assert!(
        c.kill_error
            .as_deref()
            .is_some_and(|e| e.contains("injected kill failure")),
        "the failed kill must be reported: {}",
        c.describe()
    );
    // The bound, from both sides: the poll ran its full 5 s ceiling and
    // the row did not hang. 30 s is the ceiling of the assertion, not an
    // expected duration.
    assert!(
        elapsed >= std::time::Duration::from_secs(5)
            && elapsed <= std::time::Duration::from_secs(30),
        "the bounded poll took {elapsed:?}"
    );
    println!("PENDING-CONTROL {} in {elapsed:?}", c.describe());
    assert_the_shared_gate_rejects(&r, "was not reaped");
    adopted.prove_alive_then_reap();
}

#[test]
fn a_failed_kill_with_an_errored_poll_is_reported_as_failed_at_once() {
    use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
    let t0 = std::time::Instant::now();
    let mut r = run_inner_with_fault(SLEEPER, Fault::KillFailsAndReapErrors);
    let elapsed = t0.elapsed();
    // r31 F4: FIRST, before a single fallible assertion. From here the
    // guard owns the surviving child on every exit path, panic included.
    let mut adopted = AdoptedChild::adopt(&mut r);
    assert!(
        adopted.child.is_some(),
        "the report must hand over the un-reaped child, not just its pid"
    );
    let c = r
        .cleanup
        .as_ref()
        .expect("an errored reap must hand back a cleanup report");
    assert_eq!(
        c.outcome.label(),
        "REAP-FAILED",
        "an errored poll must be REAP-FAILED: {}",
        c.describe()
    );
    assert!(
        !c.reaped_ok(),
        "FAILED is never a reaped child: {}",
        c.describe()
    );
    assert!(
        c.outcome.detail().contains("injected try_wait failure"),
        "the poll error must be carried, not discarded: {}",
        c.describe()
    );
    // It must NOT spend the 5 s budget: the error breaks out at once.
    assert!(
        elapsed <= std::time::Duration::from_secs(20),
        "the errored poll took {elapsed:?}"
    );
    println!("FAILED-CONTROL {} in {elapsed:?}", c.describe());
    assert_the_shared_gate_rejects(&r, "was not reaped");
    adopted.prove_alive_then_reap();
}

#[test]
fn a_successful_kill_is_reported_as_reaped_and_leaves_no_live_pid() {
    use load_probe_harness::{pid_state, run_inner_with_fault, Fault, PidState, SLEEPER};
    let t0 = std::time::Instant::now();
    let r = run_inner_with_fault(SLEEPER, Fault::Timeout);
    let elapsed = t0.elapsed();
    let c = r
        .cleanup
        .as_ref()
        .expect("a timeout must hand back a cleanup report");
    assert_eq!(c.outcome.label(), "reaped", "{}", c.describe());
    assert!(c.reaped_ok(), "{}", c.describe());
    assert!(c.kill_error.is_none(), "{}", c.describe());
    assert!(
        elapsed <= std::time::Duration::from_secs(30),
        "the reaping path took {elapsed:?}"
    );
    let after = pid_state(c.pid);
    assert_eq!(
        after,
        PidState::Dead,
        "a reaped child must leave no live pid: {after:?}"
    );
    println!(
        "REAPED-CONTROL {} in {elapsed:?} pid state after={after:?}",
        c.describe()
    );
    // Same shared gate, same rejection: a timed-out run is never green,
    // for a different reason, so the two verdicts stay distinguishable.
    assert_the_shared_gate_rejects(&r, "timed out");
}

#[test]
fn an_unwind_between_spawn_and_reap_still_reaps_the_child() {
    use load_probe_harness::{run_inner_with_fault, take_unwind_cleanups, Fault, SLEEPER};
    let _ = take_unwind_cleanups();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = std::panic::catch_unwind(|| run_inner_with_fault(SLEEPER, Fault::PanicWhileOwned));
    std::panic::set_hook(hook);
    assert!(caught.is_err(), "the injected unwind must reach the row");
    let done = take_unwind_cleanups();
    assert_eq!(
        done.len(),
        1,
        "the guard must reap exactly one child on the unwind path: {done:?}"
    );
    assert!(
        done[0].reaped_ok(),
        "the child was not reaped on the unwind path: {}",
        done[0].describe()
    );
    // r29 item 3: the unwind path checks the KILL error report on its own
    // too. See the comment in the timeout row for why it is not shared.
    assert!(
        done[0].kill_error.is_none(),
        "terminating the child on the unwind path reported an error: {}",
        done[0].describe()
    );
}

// #2786 C1 E15, third selector: an off-thread load IS counted. One load on
// a spawned, joined thread and one on the test thread, two directories.
#[test]
fn the_load_probe_observes_every_thread() {
    super::load_probe_harness::expect_child_pass(
        "config::agent_config::tests::inner_the_load_probe_observes_every_thread",
    );
}

#[test]
#[ignore = "#2786 C1: runs only in the dedicated armed process"]
fn inner_the_load_probe_observes_every_thread() {
    let off = tempfile::tempdir().expect("tempdir");
    let on = tempfile::tempdir().expect("tempdir");
    let obs = super::load_probe::Observation::open();
    let off_dir = off.path().to_path_buf();
    std::thread::spawn(move || super::read_agent_local_config(&off_dir))
        .join()
        .expect("the off-thread load");
    super::read_agent_local_config_json(on.path()).expect("the on-thread load");
    println!("REACHED-CALLER");
    let loads = obs.loads();
    let want = vec![off.path().to_path_buf(), on.path().to_path_buf()];
    assert_eq!(loads, want, "recorded {loads:?}, expected exactly {want:?}");
}

// #2786 C1 E15, fourth selector: inside the armed process a wrapper load
// with no open observation panics, naming the directory.
#[test]
fn a_load_with_no_open_observation_fails_closed() {
    super::load_probe_harness::expect_child_pass(
        "config::agent_config::tests::inner_a_load_with_no_open_observation_fails_closed",
    );
}

#[test]
#[ignore = "#2786 C1: runs only in the dedicated armed process"]
fn inner_a_load_with_no_open_observation_fails_closed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _slot = super::load_probe::NoObservation::open();
    println!("REACHED-CALLER");
    let err = std::panic::catch_unwind(|| super::read_agent_local_config(tmp.path()))
        .expect_err("a load with no open observation must panic");
    let text = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(
        text.contains("wrapper load with no open Observation")
            && text.contains(&format!("{:?}", tmp.path())),
        "the panic must name the directory: {text:?}"
    );
}

// ── #2807 (C2): the config pair entry point ─────────────────────────────

use serde_json::{json, Value};
use std::time::{Duration, Instant};

const CLEANUP_STATE: &str = "after_cleanup_state_publish";
const CLEANUP_TRACKED: &str = "after_cleanup_tracked_write";
const CALLER_STATE: &str = "after_caller_state_publish";
const CALLER_DECISIONS: &str = "after_caller_decisions_write";
/// Fired by no production build: only E1f's scratch mutant opens this gap.
const MUTANT_GAP: &str = "mutant_gap_open";

const PAIR_CHILD_FQN: &str = "config::agent_config::tests::config_pair_child_entry";
const PAIR_CHILD_DECISIONS_ENV: &str = "AC_2807_CHILD_DECISIONS";
const PAIR_CHILD_JOURNAL_ENV: &str = "AC_2807_CHILD_JOURNAL";
const PAIR_CHILD_RENDEZVOUS_ENV: &str = "AC_2807_CHILD_RENDEZVOUS";
const PAIR_CHILD_STOPS_ENV: &str = "AC_2807_CHILD_STOPS";
const PAIR_CHILD_MUTATE_ENV: &str = "AC_2807_CHILD_MUTATE";
const PAIR_CHILD_SEQUENCE_ENV: &str = "AC_2807_CHILD_SEQUENCE";
const READY_FILE: &str = "paused.ready";
const RELEASE_FILE: &str = "paused.release";
const STAGES_LOG: &str = "stages.log";
const HOOK_BOUND: Duration = Duration::from_secs(60);

/// One agent directory under an `.ac` root, plus an instance journal
/// directory outside it.
struct Pair {
    tmp: tempfile::TempDir,
    dir: PathBuf,
    journal: PathBuf,
}

fn pair() -> Pair {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join(".ac").join("_agent_a");
    let journal = tmp.path().join("journal");
    std::fs::create_dir_all(&dir).expect("create agent dir");
    std::fs::create_dir_all(&journal).expect("create journal dir");
    Pair { tmp, dir, journal }
}

fn put_json(path: &Path, value: &Value) {
    let mut text = serde_json::to_string_pretty(value).expect("serialize");
    text.push('\n');
    std::fs::write(path, text).expect("seed file");
}

fn get_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read file")).expect("JSON")
}

fn bytes_of(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn split_marker() -> Value {
    json!({ "v": 1, "keys": STATE_KEYS })
}

/// A state file that carries the split marker.
fn marked(tooling: Value) -> Value {
    json!({ "tooling": tooling, "split": split_marker() })
}

fn state_keys_in(file: &Value) -> Vec<&'static str> {
    STATE_KEYS
        .into_iter()
        .filter(|key| file["tooling"].get(key).is_some())
        .collect()
}

fn no_change(_: &mut JsonMap, _: &mut JsonMap) -> Result<(), String> {
    Ok(())
}

fn set_tooling(map: &mut JsonMap, key: &str, value: Value) {
    map.entry("tooling")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("tooling object")
        .insert(key.to_string(), value);
}

/// Every file under `root`, with its bytes; a directory is `None`.
fn tree_of(root: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                out.insert(path.clone(), None);
                visit(&path, out);
            } else {
                out.insert(path.clone(), Some(std::fs::read(&path).expect("read")));
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    visit(root, &mut out);
    out
}

fn append_line(path: &Path, line: &str) {
    use std::io::Write as _;
    // One write in append mode: atomic for a short line, from any process.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()))
        .expect("append line");
}

fn lines_of(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

impl Pair {
    fn decisions(&self) -> PathBuf {
        self.dir.join("config.json")
    }

    fn state(&self) -> PathBuf {
        state_file_path(&self.dir)
    }

    fn seed_decisions(&self, value: Value) {
        put_json(&self.decisions(), &value);
    }

    fn seed_state(&self, value: Value) {
        put_json(&self.state(), &value);
    }

    fn tracked(&self) -> Value {
        get_json(&self.decisions())
    }

    fn stored_state(&self) -> Value {
        get_json(&self.state())
    }

    /// The value a reader gets, through the loader.
    fn read(&self, key: &str) -> Value {
        read_agent_local_config_json(&self.dir)
            .expect("loader")
            .expect("a config")["tooling"][key]
            .clone()
    }

    fn write<F>(&self, mutate: F) -> Result<(), String>
    where
        F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
    {
        update_agent_config_in(&self.decisions(), Some(&self.journal), mutate)
    }

    fn write_with_publish<F>(&self, mutate: F) -> Result<(), String>
    where
        F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
    {
        update_agent_config_with_activity_and_publish(
            &self.decisions(),
            Some(&self.journal),
            &|_: &str| {},
            mutate,
            None,
            Some(&|tmp, target| self.publish(tmp, target)),
        )
    }

    fn publish(&self, tmp: &Path, target: &Path) -> Result<(), String> {
        if (target == self.decisions() && self.decisions_temp().is_dir())
            || (target == self.state() && self.state_temp().is_dir())
        {
            assert!(
                tmp.is_file(),
                "fresh staging acquired before injected refusal"
            );
            return Err("fixture temp config publish failure".into());
        }
        crate::config::local_config_io::publish_temp_config(tmp, target)
    }

    /// A writer outside this binary's protocol: it puts a state key back
    /// into the tracked file with a plain write.
    fn foreign_write(&self, key: &str, value: Value) {
        let mut tracked = self.tracked();
        set_tooling(tracked.as_object_mut().expect("object"), key, value);
        put_json(&self.decisions(), &tracked);
    }

    fn publish_temp(&self, file_name: &str) -> PathBuf {
        self.dir
            .join(format!(".{file_name}.{}.tmp", std::process::id()))
    }

    fn decisions_temp(&self) -> PathBuf {
        self.publish_temp("config.json")
    }

    fn state_temp(&self) -> PathBuf {
        self.publish_temp(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
    }

    fn journal_bytes(&self) -> Option<Vec<u8>> {
        bytes_of(
            &self
                .journal
                .join(crate::config::instance_artifacts::NAMING_MIGRATION_STATE_NAME),
        )
    }
}

/// A child process driving `update_agent_config_with_stage`, stopped at
/// named stages through one rendezvous subdirectory per stage.
struct PairChild {
    child: std::process::Child,
    rendezvous: PathBuf,
}

fn spawn_pair_child(
    pair: &Pair,
    stops: &[&str],
    mutate: &Value,
    sequence: Option<&Path>,
) -> PairChild {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let rendezvous = pair.tmp.path().join(format!(
        "rendezvous-{}",
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    for stage in stops {
        std::fs::create_dir_all(rendezvous.join(stage)).expect("create rendezvous");
    }
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("current test exe"));
    command
        .args(["--exact", PAIR_CHILD_FQN, "--nocapture", "--test-threads=1"])
        .env(PAIR_CHILD_DECISIONS_ENV, pair.decisions())
        .env(PAIR_CHILD_JOURNAL_ENV, &pair.journal)
        .env(PAIR_CHILD_RENDEZVOUS_ENV, &rendezvous)
        .env(PAIR_CHILD_STOPS_ENV, stops.join(","))
        .env(PAIR_CHILD_MUTATE_ENV, mutate.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    if let Some(sequence) = sequence {
        command.env(PAIR_CHILD_SEQUENCE_ENV, sequence);
    }
    PairChild {
        child: command.spawn().expect("spawn the pair child"),
        rendezvous,
    }
}

impl PairChild {
    fn ready(&self, stage: &str) -> bool {
        self.rendezvous.join(stage).join(READY_FILE).exists()
    }

    /// The first of `stages` the child stops at, or `None` when the bound
    /// passes or the child exits first.
    fn first_ready<'a>(&mut self, stages: &[&'a str], bound: Duration) -> Option<&'a str> {
        let started = Instant::now();
        loop {
            if let Some(stage) = stages.iter().copied().find(|stage| self.ready(stage)) {
                return Some(stage);
            }
            let exited = !matches!(self.child.try_wait(), Ok(None));
            if exited || started.elapsed() >= bound {
                return stages.iter().copied().find(|stage| self.ready(stage));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn release(&self, stage: &str) {
        std::fs::write(self.rendezvous.join(stage).join(RELEASE_FILE), b"release")
            .expect("release the child");
    }

    fn kill(&mut self) -> String {
        let killed = self.child.kill();
        format!("kill {killed:?}, exit {:?}", self.child.wait())
    }

    /// Waits for the child to end by itself.
    fn exit(&mut self) -> std::process::ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(
                started.elapsed() < HOOK_BOUND,
                "the pair child did not end within 60 s"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Every stage the child fired, in order.
    fn stages(&self) -> Vec<String> {
        lines_of(&self.rendezvous.join(STAGES_LOG))
    }

    /// Stops the child at `stage`, kills it there, and returns the stages
    /// it fired.
    fn kill_at(&mut self, stage: &str) -> Vec<String> {
        let reached = self.first_ready(&[stage], HOOK_BOUND);
        let stages = self.stages();
        let status = self.kill();
        assert_eq!(
            reached,
            Some(stage),
            "the child never stopped at {stage}: fired {stages:?}, {status}"
        );
        assert_eq!(
            stages.last().map(String::as_str),
            Some(stage),
            "the kill must happen at {stage}"
        );
        stages
    }
}

impl Drop for PairChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Not a test of its own: the body a spawned child runs, inert otherwise.
#[test]
fn config_pair_child_entry() {
    let Some(decisions) = std::env::var_os(PAIR_CHILD_DECISIONS_ENV) else {
        return;
    };
    let decisions = PathBuf::from(decisions);
    let journal = std::env::var_os(PAIR_CHILD_JOURNAL_ENV).map(PathBuf::from);
    let rendezvous = PathBuf::from(std::env::var_os(PAIR_CHILD_RENDEZVOUS_ENV).expect("dir"));
    let stops = std::env::var(PAIR_CHILD_STOPS_ENV).expect("stops");
    let stops: Vec<&str> = stops.split(',').filter(|s| !s.is_empty()).collect();
    let sequence = std::env::var_os(PAIR_CHILD_SEQUENCE_ENV).map(PathBuf::from);
    let mutate: Value =
        serde_json::from_str(&std::env::var(PAIR_CHILD_MUTATE_ENV).expect("mutate"))
            .expect("mutate JSON");

    let on_stage = |stage: &str| {
        append_line(&rendezvous.join(STAGES_LOG), stage);
        if !stops.contains(&stage) {
            return;
        }
        // The line first, the ready signal second, so a kill loses no line.
        if let Some(sequence) = &sequence {
            let line = match stage {
                CLEANUP_TRACKED => "child:paused",
                MUTANT_GAP => "child:at-gap",
                CALLER_STATE => "child:published v3",
                other => panic!("no sequence line for {other}"),
            };
            append_line(sequence, line);
        }
        let dir = rendezvous.join(stage);
        std::fs::write(dir.join(READY_FILE), b"ready").expect("announce pause");
        let started = Instant::now();
        while !dir.join(RELEASE_FILE).exists() {
            assert!(started.elapsed() < HOOK_BOUND, "pause exceeded 60 s");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    update_agent_config_with_stage(&decisions, journal.as_deref(), &on_stage, |d, s| {
        for (side, map) in [("decisions", d), ("state", s)] {
            let Some(entries) = mutate.get(side).and_then(Value::as_object) else {
                continue;
            };
            for (key, value) in entries {
                set_tooling(map, key, value.clone());
            }
        }
        Ok(())
    })
    .expect("the child's write");
}

fn sample_state_values() -> [(&'static str, Value); 4] {
    [
        ("lastCodingAgent", json!("claude")),
        (
            "codingAgents",
            json!({ "claude": { "app": "Claude Code", "lastUsed": T1 } }),
        ),
        ("lastAgentMessageAt", json!(T2)),
        ("profileContentHash", json!("9f2c41")),
    ]
}

/// The JSON text of a value, so "byte for byte" is compared as bytes.
fn text(value: &Value) -> String {
    serde_json::to_string(value).expect("serialize")
}

// -- E1 ------------------------------------------------------------------

#[test]
fn every_state_key_moves_without_loss() {
    // (a) decisions-only, per key.
    for (key, value) in sample_state_values() {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { key: value.clone(), "profile": "keep" } }));
        p.write(no_change).expect("migrate");
        let state = p.stored_state();
        assert_eq!(
            text(&state["tooling"][key]),
            text(&value),
            "(a) {key} must land in the state file byte for byte"
        );
        assert_eq!(
            p.tracked(),
            json!({ "tooling": { "profile": "keep" } }),
            "(a) {key} must leave the decisions file and nothing else may"
        );
        assert_eq!(state["split"], split_marker(), "(a) {key}: marker present");
    }

    // (b) conflict on a pair that was never split: the state value survives.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "tracked" } }));
    p.seed_state(json!({ "tooling": { "lastCodingAgent": "state" } }));
    p.write(no_change).expect("migrate a conflict");
    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("state"),
        "(b) with no marker the state file's value must survive"
    );
    assert!(
        state_keys_in(&p.tracked()).is_empty(),
        "(b) the decisions file must lose its copy"
    );

    // (c) a second run on the migrated tree.
    let p = pair();
    let all: serde_json::Map<String, Value> = sample_state_values()
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    p.seed_decisions(json!({ "tooling": all.clone() }));
    p.write(no_change).expect("first run");
    let (tracked, state) = (bytes_of(&p.decisions()), bytes_of(&p.state()));
    p.write(no_change).expect("second run");
    assert_eq!(
        p.stored_state()["tooling"],
        Value::Object(all),
        "(c) a second run must keep every state value"
    );
    assert_eq!(
        (bytes_of(&p.decisions()), bytes_of(&p.state())),
        (tracked, state),
        "(c) a second run must write neither file"
    );

    // (d) no state file, and the decisions write is forced to fail.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    let tracked = bytes_of(&p.decisions());
    std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
    p.write_with_publish(no_change)
        .expect_err("(d) the decisions write must fail");
    assert_eq!(
        bytes_of(&p.state()).map(|_| p.stored_state()["tooling"]["lastCodingAgent"].clone()),
        Some(json!("claude")),
        "(d) the state file must already hold the value when the decisions write fails"
    );
    assert_eq!(
        bytes_of(&p.decisions()),
        tracked,
        "(d) decisions file unchanged"
    );
}

// -- E1b -----------------------------------------------------------------

#[test]
fn a_value_written_back_by_an_older_binary_wins() {
    /// The foreign `v2` is in the tracked file: a read returns it and
    /// writes nothing, and the next write leaves it in the state file.
    fn check(shape: &str, p: &Pair, journal: &Path, failures: &mut Vec<String>) {
        let before = tree_of(&p.dir);
        let read = p.read("lastCodingAgent");
        if read != json!("v2") {
            failures.push(format!("({shape}) a read must return v2, got {read}"));
        }
        if tree_of(&p.dir) != before {
            failures.push(format!("({shape}) a read must write nothing"));
        }
        if let Err(e) = update_agent_config_in(&p.decisions(), Some(journal), no_change) {
            failures.push(format!("({shape}) the next write failed: {e}"));
            return;
        }
        let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
        if state != json!("v2") {
            failures.push(format!(
                "({shape}) the next write must leave v2 in the state file, got {state}"
            ));
        }
        let left = state_keys_in(&p.tracked());
        if !left.is_empty() {
            failures.push(format!(
                "({shape}) the tracked file must hold no state key, holds {left:?}"
            ));
        }
    }
    let migrated = || {
        let p = pair();
        p.seed_decisions(json!({ "tooling": {} }));
        p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
        p
    };
    let mut failures = Vec::new();

    // (a) after a completed migration.
    let p = migrated();
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("a", &p, &p.journal, &mut failures);

    // (c) a pair created after phase C, built by the entry point itself.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "keep" } }));
    p.write(|_, state| {
        set_tooling(state, "lastCodingAgent", json!("v1"));
        Ok(())
    })
    .expect("create the pair through the entry point");
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("c", &p, &p.journal, &mut failures);

    // (d) two journal directories, neither consulted.
    let p = migrated();
    let other_journal = p.tmp.path().join("journal-b");
    std::fs::create_dir_all(&other_journal).expect("second journal dir");
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("d, first journal", &p, &p.journal, &mut failures);
    p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("d, second journal", &p, &other_journal, &mut failures);

    // (e) the journal write forced to fail: its directory is a file.
    let p = migrated();
    let broken_journal = p.tmp.path().join("journal-is-a-file");
    std::fs::write(&broken_journal, b"not a directory").expect("block the journal");
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("e", &p, &broken_journal, &mut failures);

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// -- E1c -----------------------------------------------------------------

#[test]
fn a_first_write_that_also_mutates_keeps_its_new_value() {
    fn leg(name: &str, p: &Pair, new: &str, failures: &mut Vec<String>) {
        let mut child = spawn_pair_child(
            p,
            &[CALLER_STATE],
            &json!({ "state": { "lastCodingAgent": new } }),
            None,
        );
        child.kill_at(CALLER_STATE);
        let read = p.read("lastCodingAgent");
        if read != json!(new) {
            failures.push(format!(
                "({name}) after the kill a read must return {new}, got {read}"
            ));
        }
        p.write(no_change).expect("the parent's write");
        let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
        if state != json!(new) {
            failures.push(format!(
                "({name}) the surviving value must be {new}, the state file holds {state}"
            ));
        }
    }
    let mut failures = Vec::new();

    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old" } }));
    leg("migrate and mutate", &p, "v_new", &mut failures);

    // (c') the tracked file is still dirty from an earlier crash.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v0" } }));
    p.seed_state(json!({ "tooling": { "lastCodingAgent": "v0" } }));
    leg("dirty tracked file", &p, "v1", &mut failures);

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// -- E1d -----------------------------------------------------------------

#[test]
fn a_foreign_value_and_a_new_value_both_survive_their_own_sequence() {
    let foreign = || {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v2" } }));
        p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
        p
    };
    let v3 = json!({ "state": { "lastCodingAgent": "v3" } });

    // Killed after our own write: the survivor is v3.
    let p = foreign();
    let stages = spawn_pair_child(&p, &[CALLER_STATE], &v3, None).kill_at(CALLER_STATE);
    assert_eq!(stages, [CLEANUP_STATE, CLEANUP_TRACKED, CALLER_STATE]);
    assert_eq!(p.read("lastCodingAgent"), json!("v3"));
    p.write(no_change).expect("re-run");
    assert_eq!(p.stored_state()["tooling"]["lastCodingAgent"], json!("v3"));

    // Killed inside the cleanup: the survivor is v2, not yet absorbed. This
    // is the preservation window: marker present beside a tracked state
    // key, and both files agree.
    let p = foreign();
    let stages = spawn_pair_child(&p, &[CLEANUP_STATE], &v3, None).kill_at(CLEANUP_STATE);
    assert_eq!(stages, [CLEANUP_STATE]);
    let state = p.stored_state();
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("v2"));
    assert_eq!(
        state["split"],
        split_marker(),
        "the marker is never removed"
    );
    assert_eq!(p.tracked()["tooling"]["lastCodingAgent"], json!("v2"));
    assert_eq!(p.read("lastCodingAgent"), json!("v2"));
    p.write(no_change).expect("re-run");
    let state = p.stored_state();
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("v2"));
    assert_eq!(state["split"], split_marker());
    assert!(state_keys_in(&p.tracked()).is_empty());

    // A state-only mutation: the decisions map does not change, so its
    // stage never fires and the child ends by itself.
    let p = pair();
    p.seed_decisions(json!({ "tooling": {} }));
    p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
    let mut child = spawn_pair_child(&p, &[CALLER_STATE, CALLER_DECISIONS], &v3, None);
    assert_eq!(
        child.first_ready(&[CALLER_STATE, CALLER_DECISIONS], HOOK_BOUND),
        Some(CALLER_STATE)
    );
    child.release(CALLER_STATE);
    assert!(child.exit().success(), "the state-only write must finish");
    assert_eq!(child.stages(), [CALLER_STATE]);
    assert!(!child.ready(CALLER_DECISIONS));
    assert_eq!(p.read("lastCodingAgent"), json!("v3"));
}

// -- E1f -----------------------------------------------------------------

#[test]
fn nothing_lands_between_the_cleanup_and_the_caller_sequence() {
    const LOCK_TIMEOUT: Duration = Duration::from_secs(60);
    const DISPATCH_BOUND: Duration = Duration::from_secs(2);
    const JOIN_BOUND: Duration = Duration::from_secs(5);

    fn join_within<T>(handle: std::thread::JoinHandle<T>, bound: Duration) -> Result<T, String> {
        let started = Instant::now();
        while !handle.is_finished() {
            if started.elapsed() >= bound {
                return Err(format!("the thread did not finish within {bound:?}"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().map_err(|_| "the thread panicked".to_string())
    }

    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v1" } }));
    let sequence = p.tmp.path().join("sequence.log");
    let mut child = spawn_pair_child(
        &p,
        &[CLEANUP_TRACKED, MUTANT_GAP, CALLER_STATE],
        &json!({ "state": { "lastCodingAgent": "v3" } }),
        Some(&sequence),
    );

    // (Xt) abort: no more evidence is wanted, so take the one order that
    // cannot block. Both releases, kill and reap the child, then `go`, then
    // a bounded join.
    type Contender = std::thread::JoinHandle<Result<(), String>>;
    let abort = |child: &mut PairChild,
                 contender: Option<(std::sync::mpsc::Sender<()>, Contender)>,
                 why: String|
     -> ! {
        for stage in [CLEANUP_TRACKED, MUTANT_GAP, CALLER_STATE] {
            child.release(stage);
        }
        let last_stage = child.stages().last().cloned();
        let child_status = child.kill();
        let thread_status = contender.map(|(go, handle)| {
            let _ = go.send(());
            join_within(handle, JOIN_BOUND)
        });
        panic!(
            "{why}\nsequence: {:?}\nlast stage: {last_stage:?}\nchild: {child_status}\nthread: {thread_status:?}",
            lines_of(&sequence)
        );
    };

    // (1) the child pauses inside the cleanup with the guard held.
    if child.first_ready(&[CLEANUP_TRACKED], HOOK_BOUND) != Some(CLEANUP_TRACKED) {
        abort(
            &mut child,
            None,
            "the child never paused in the cleanup".into(),
        );
    }

    // (2) the competing writer, a pre-C binary: it takes the decisions
    // sidecar, then waits at `go` still holding it.
    let (go, go_gate) = std::sync::mpsc::channel::<()>();
    let lock_path = std::fs::canonicalize(&p.dir)
        .expect("canonical agent dir")
        .join(".config.json.lock");
    let (decisions_path, thread_sequence) = (p.decisions(), sequence.clone());
    let handle: Contender = std::thread::spawn(move || {
        append_line(&thread_sequence, "parent:acquiring");
        let _lock = crate::config::local_config_io::acquire_sidecar_write_lock(
            &lock_path,
            LOCK_TIMEOUT,
            "contenderLockTimeout",
            "contending config write lock",
        )?;
        append_line(&thread_sequence, "parent:acquired");
        go_gate
            .recv_timeout(HOOK_BOUND)
            .map_err(|e| format!("go was never released: {e}"))?;
        let mut tracked = get_json(&decisions_path);
        set_tooling(
            tracked.as_object_mut().expect("object"),
            "lastCodingAgent",
            json!("v2"),
        );
        put_json(&decisions_path, &tracked);
        append_line(&thread_sequence, "parent:published v2");
        Ok(())
    });
    let has = |line: &str| lines_of(&sequence).iter().any(|l| l == line);
    let started = Instant::now();
    while !has("parent:acquiring") {
        if started.elapsed() >= JOIN_BOUND {
            abort(
                &mut child,
                Some((go, handle)),
                "the thread never started".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // (3) secondary check: the lock is not taken while the cleanup pause
    // holds. It passes on both builds.
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(500) {
        if has("parent:acquired") {
            abort(
                &mut child,
                Some((go, handle)),
                "the lock was taken while the child held it in the cleanup".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // (4) the whole branch dispatch: which stage the child reports next.
    child.release(CLEANUP_TRACKED);
    match child.first_ready(&[CALLER_STATE, MUTANT_GAP], DISPATCH_BOUND) {
        // (5t) the child owns the lock: child first, thread second.
        Some(CALLER_STATE) => {
            child.kill();
            let _ = go.send(());
            match join_within(handle, HOOK_BOUND) {
                Ok(Ok(())) => {}
                other => panic!("the contender failed: {other:?}"),
            }
        }
        // (6t) the thread owns the lock: thread first, child second.
        Some(MUTANT_GAP) => {
            let _ = go.send(());
            match join_within(handle, HOOK_BOUND) {
                Ok(Ok(())) => {}
                other => abort(&mut child, None, format!("the contender failed: {other:?}")),
            }
            child.release(MUTANT_GAP);
            if child.first_ready(&[CALLER_STATE], DISPATCH_BOUND) != Some(CALLER_STATE) {
                abort(
                    &mut child,
                    None,
                    "the child never published after the gap".into(),
                );
            }
            child.kill();
        }
        other => abort(
            &mut child,
            Some((go, handle)),
            format!("no stage within 2 s of the cleanup release: {other:?}"),
        ),
    }

    // The blocking assertion, on the final marker file.
    let lines = lines_of(&sequence);
    let at = |line: &str| {
        lines
            .iter()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("{line} is missing from {lines:?}"))
    };
    assert!(
        at("parent:acquired") > at("child:published v3"),
        "parent:acquired must not appear before child:published v3: {lines:?}"
    );
    assert_eq!(
        lines,
        [
            "child:paused",
            "parent:acquiring",
            "child:published v3",
            "parent:acquired",
            "parent:published v2"
        ]
    );

    // End state: the tracked file holds v2, the newer write, and it wins.
    assert_eq!(p.tracked()["tooling"]["lastCodingAgent"], json!("v2"));
    p.write(no_change).expect("reconcile");
    assert_eq!(p.stored_state()["tooling"]["lastCodingAgent"], json!("v2"));
    assert!(state_keys_in(&p.tracked()).is_empty());
}

// -- E1e -----------------------------------------------------------------

#[test]
fn a_post_migration_write_survives_a_kill() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
    p.seed_state(marked(json!({ "lastCodingAgent": "v_old" })));
    let mut child = spawn_pair_child(
        &p,
        &[CALLER_DECISIONS],
        &json!({
            "state": { "lastCodingAgent": "v_new" },
            "decisions": { "profile": "new" }
        }),
        None,
    );
    child.kill_at(CALLER_DECISIONS);

    let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
    let profile = p.tracked()["tooling"]["profile"].clone();
    assert_eq!(
        state,
        json!("v_new"),
        "the state value was lost: the state file holds {state} while the decisions file \
         already holds profile {profile}"
    );
    assert_eq!(profile, json!("new"));
    assert!(state_keys_in(&p.tracked()).is_empty());
    assert_eq!(p.read("lastCodingAgent"), json!("v_new"));
}

// -- E2 ------------------------------------------------------------------

#[test]
fn the_state_file_wins_when_there_is_no_marker() {
    // First leg, in this process. The marker is stamped by the caller's
    // sequence, so it is not on disk when the cleanup has finished.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "A" } }));
    p.seed_state(json!({ "tooling": { "lastCodingAgent": "B" } }));
    let marker_after_cleanup = std::cell::RefCell::new(None);
    let on_stage = |stage: &str| {
        if stage == CLEANUP_TRACKED {
            *marker_after_cleanup.borrow_mut() = Some(p.stored_state().get("split").cloned());
        }
    };
    update_agent_config_with_stage(&p.decisions(), Some(&p.journal), &on_stage, no_change)
        .expect("migrate a conflict");
    let state = p.stored_state();
    assert_eq!(
        state["tooling"]["lastCodingAgent"],
        json!("B"),
        "with no marker the state file's value must survive"
    );
    assert!(state_keys_in(&p.tracked()).is_empty());
    assert_eq!(
        state["split"],
        split_marker(),
        "the caller's sequence stamps it"
    );

    // Second leg, E2 crossed with E4: three kills, three runs. The second
    // state key, held only by the tracked file, makes the state side
    // change, so every named stage is reachable. Both results of a kill
    // are collected before the row fails, so a failing build still shows
    // the read and the re-run.
    let mut failures = Vec::new();
    for kill in [CLEANUP_STATE, CLEANUP_TRACKED, CALLER_STATE] {
        // The third kill is control-only: a build that has already lost
        // the value may never fire that stage, so it is not awaited.
        if kill == CALLER_STATE && !failures.is_empty() {
            break;
        }
        let p = pair();
        p.seed_decisions(json!({ "tooling": {
            "lastCodingAgent": "A",
            "codingAgents": { "claude": { "app": "Claude Code", "lastUsed": T1 } }
        } }));
        p.seed_state(json!({ "tooling": { "lastCodingAgent": "B" } }));
        spawn_pair_child(&p, &[kill], &json!({}), None).kill_at(kill);
        let read = p.read("lastCodingAgent");
        if read != json!("B") {
            failures.push(format!(
                "killed at {kill}: a read must return B, got {read}"
            ));
        }
        p.write(no_change).expect("re-run");
        let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
        if state != json!("B") {
            failures.push(format!(
                "killed at {kill}: the re-run must leave B in the state file, it holds {state}"
            ));
        }
        println!("E2 killed at {kill}: read {read}, state after the re-run {state}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    assert_eq!(
        marker_after_cleanup.into_inner(),
        Some(None),
        "the cleanup's publish must not carry the marker"
    );
}

// -- E3 ------------------------------------------------------------------

#[test]
fn a_second_migration_writes_nothing() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude", "profile": "keep" } }));
    p.write(no_change).expect("migrate");
    let journal = p.journal_bytes();
    assert!(
        String::from_utf8_lossy(journal.as_deref().expect("the audit note"))
            .contains("lastCodingAgent"),
        "the first run records the keys it moved"
    );
    let before = (bytes_of(&p.decisions()), bytes_of(&p.state()));
    p.write(no_change).expect("second run");
    assert_eq!(
        (bytes_of(&p.decisions()), bytes_of(&p.state())),
        before,
        "a second run must write neither config file"
    );
    assert_eq!(p.journal_bytes(), journal, "the journal must not grow");

    // Rule 6: a decision-only write creates no state file.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
    p.write(|decisions, _state| {
        set_tooling(decisions, "profile", json!("new"));
        Ok(())
    })
    .expect("decision-only write");
    assert_eq!(p.tracked()["tooling"]["profile"], json!("new"));
    assert!(
        !p.state().exists(),
        "a decision-only write must create no state file"
    );
    assert_eq!(p.journal_bytes(), None, "nothing moved, so no note");
}

// -- E4 ------------------------------------------------------------------

#[test]
fn an_interrupted_move_leaves_the_value_in_both_files() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    // Killed at whichever cleanup stage fires first, between the two writes.
    let mut child = spawn_pair_child(&p, &[CLEANUP_STATE, CLEANUP_TRACKED], &json!({}), None);
    let stage = child.first_ready(&[CLEANUP_STATE, CLEANUP_TRACKED], HOOK_BOUND);
    let status = child.kill();
    assert!(
        stage.is_some(),
        "the child never reached the cleanup: {status}"
    );

    let tracked = p.tracked()["tooling"]["lastCodingAgent"].clone();
    let state = bytes_of(&p.state())
        .map(|_| p.stored_state()["tooling"]["lastCodingAgent"].clone())
        .unwrap_or(Value::Null);
    assert_eq!(
        (&tracked, &state),
        (&json!("claude"), &json!("claude")),
        "after the kill at {stage:?} the value must be in both files: tracked {tracked}, state {state}"
    );

    p.write(no_change).expect("re-run");
    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("claude")
    );
    assert!(
        state_keys_in(&p.tracked()).is_empty(),
        "after the re-run the value is only in the state file"
    );
}

// -- E5 ------------------------------------------------------------------

#[test]
fn a_refusal_keeps_every_value() {
    let unmigrated = || {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old", "profile": "old" } }));
        p
    };
    let both = |p: &Pair| (bytes_of(&p.decisions()), bytes_of(&p.state()));
    let mutate_both = |decisions: &mut JsonMap, state: &mut JsonMap| {
        set_tooling(state, "lastCodingAgent", json!("v_new"));
        set_tooling(decisions, "profile", json!("new"));
        Ok(())
    };

    // Leg 1, lock refusal: the one five-second wait of the battery.
    let p = unmigrated();
    let before = both(&p);
    let held = crate::config::local_config_io::acquire_sidecar_write_lock(
        &std::fs::canonicalize(&p.dir)
            .expect("canonical agent dir")
            .join(".config.json.lock"),
        Duration::from_secs(1),
        "testLockTimeout",
        "test lock",
    )
    .expect("hold the decisions sidecar");
    let err = p.write(no_change).expect_err("the lock refusal");
    drop(held);
    assert!(err.contains("configLockTimeout"), "{err}");
    assert_eq!(both(&p), before, "lock refusal: both files byte-identical");
    assert_eq!(p.read("lastCodingAgent"), json!("v_old"));

    // Leg 2, Io refusal inside the cleanup: the decisions publish fails
    // after the state write succeeded.
    let p = unmigrated();
    let tracked = bytes_of(&p.decisions());
    std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
    p.write_with_publish(no_change).expect_err("the Io refusal");
    assert_eq!(bytes_of(&p.decisions()), tracked, "Io: decisions unchanged");
    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("v_old"),
        "Io: the state file carries the copied key"
    );
    assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
    std::fs::remove_dir(p.decisions_temp()).expect("unblock");
    p.write(no_change).expect("the next run converges");
    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("v_old")
    );
    assert_eq!(p.stored_state()["split"], split_marker());
    assert!(state_keys_in(&p.tracked()).is_empty());

    // Leg 3: a tracked `tooling` that is not an object is never repaired.
    let p = pair();
    p.seed_decisions(json!({ "tooling": 7 }));
    p.seed_state(json!({ "tooling": { "lastCodingAgent": "v_old" } }));
    let before = both(&p);
    let err = p.write(no_change).expect_err("a malformed tooling");
    assert!(err.contains("'tooling' must be a JSON object"), "{err}");
    assert_eq!(both(&p), before, "tooling: both files byte-identical");

    // Leg 4, the caller's partial publish: its state publish succeeds and
    // its decisions publish fails. The marker is present on the `Err`.
    let p = unmigrated();
    let block = |stage: &str| {
        if stage == CLEANUP_TRACKED {
            std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
        }
    };
    update_agent_config_with_activity_and_publish(
        &p.decisions(),
        Some(&p.journal),
        &block,
        mutate_both,
        None,
        Some(&|tmp, target| p.publish(tmp, target)),
    )
    .expect_err("the caller's decisions publish");
    let state = p.stored_state();
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("v_new"));
    assert_eq!(
        state["split"],
        split_marker(),
        "partial: the marker is present"
    );
    assert_eq!(
        p.tracked(),
        json!({ "tooling": { "profile": "old" } }),
        "partial: no state key in the tracked file, and the decision is at its old value"
    );
    assert_eq!(p.read("lastCodingAgent"), json!("v_new"));
    std::fs::remove_dir(p.decisions_temp()).expect("unblock");

    // Leg 5, the caller's state publish fails: nothing of its sequence
    // reaches disk.
    let p = unmigrated();
    let block = |stage: &str| {
        if stage == CLEANUP_TRACKED {
            std::fs::create_dir(p.state_temp()).expect("block the state publish");
        }
    };
    update_agent_config_with_activity_and_publish(
        &p.decisions(),
        Some(&p.journal),
        &block,
        mutate_both,
        None,
        Some(&|tmp, target| p.publish(tmp, target)),
    )
    .expect_err("the caller's state publish");
    assert_eq!(
        p.stored_state(),
        json!({ "tooling": { "lastCodingAgent": "v_old" } }),
        "state publish: the cleanup's value and no marker"
    );
    assert_eq!(p.tracked(), json!({ "tooling": { "profile": "old" } }));
    assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
    std::fs::remove_dir(p.state_temp()).expect("unblock");
    p.write(no_change).expect("the next call");
    assert_eq!(
        p.stored_state()["split"],
        split_marker(),
        "the next call stamps it"
    );

    // Variant: `mutate` fails on a pair that already carries the marker.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
    p.seed_state(marked(json!({ "lastCodingAgent": "v_old" })));
    let before = both(&p);
    let err = p
        .write(|_, _| Err("the caller refused".to_string()))
        .expect_err("a failing mutate");
    assert_eq!(err, "the caller refused");
    assert_eq!(both(&p), before, "mutate: both files byte-identical");
    assert_eq!(p.stored_state()["split"], split_marker());
    assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
}

// -- E5b -----------------------------------------------------------------

#[test]
fn a_read_only_tree_is_not_modified() {
    let p = pair();
    let all: serde_json::Map<String, Value> = sample_state_values()
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    p.seed_decisions(json!({ "tooling": all }));
    let before = tree_of(p.tmp.path());

    for (key, value) in sample_state_values() {
        assert_eq!(p.read(key), value, "{key} must come back");
    }
    let typed = read_agent_local_config(&p.dir).expect("typed read");
    assert_eq!(typed.tooling.last_coding_agent.as_deref(), Some("claude"));

    let after = tree_of(p.tmp.path());
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "a read must create or remove no file"
    );
    assert!(after == before, "a read must modify no file");
}

// -- E5c -----------------------------------------------------------------

#[test]
fn a_journal_failure_does_not_change_the_outcome() {
    let migrated = |p: &Pair| {
        let state = p.stored_state();
        assert_eq!(state["tooling"]["lastCodingAgent"], json!("claude"));
        assert_eq!(state["split"], split_marker());
        assert!(state_keys_in(&p.tracked()).is_empty());
    };

    // The journal directory is a file, so the note cannot be written.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    let broken_journal = p.tmp.path().join("journal-is-a-file");
    std::fs::write(&broken_journal, b"not a directory").expect("block the journal");
    let result = update_agent_config_in(&p.decisions(), Some(&broken_journal), no_change);
    assert_eq!(
        result,
        Ok(()),
        "a lost audit note must not fail a completed write"
    );
    migrated(&p);
    assert_eq!(
        std::fs::read(&broken_journal).expect("the blocking file"),
        b"not a directory"
    );

    // No journal directory: same outcome, no note attempted.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    assert_eq!(
        update_agent_config_in(&p.decisions(), None, no_change),
        Ok(())
    );
    migrated(&p);
    assert_eq!(p.journal_bytes(), None);
}

// -- E7 ------------------------------------------------------------------

fn ignore_rows_present(ac_root: &Path) -> bool {
    let content = std::fs::read_to_string(ac_root.join(".gitignore")).unwrap_or_default();
    crate::config::naming_migration::config_state_ignore_rows()
        .iter()
        .all(|(pattern, _)| content.lines().any(|line| line == pattern))
}

#[test]
fn the_sweep_runs_before_the_first_write() {
    // A `.gitignore` the sweep cannot read or write: a directory.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    let gitignore = p.tmp.path().join(".ac").join(".gitignore");
    std::fs::create_dir(&gitignore).expect("block the .gitignore");
    let tracked = bytes_of(&p.decisions());
    let err = p.write(no_change).expect_err("a failed sweep");
    assert!(err.contains(&gitignore.display().to_string()), "{err}");
    assert!(
        !p.state().exists(),
        "a failed sweep must leave no unignored state file on disk"
    );
    assert_eq!(
        bytes_of(&p.decisions()),
        tracked,
        "decisions file unchanged"
    );

    // No `.gitignore` and a decision-only write: the file is created.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
    p.write(|decisions, _state| {
        set_tooling(decisions, "profile", json!("new"));
        Ok(())
    })
    .expect("decision-only write");
    assert!(ignore_rows_present(&p.tmp.path().join(".ac")));
    assert!(!p.state().exists());

    // No `.ac` ancestor: the call succeeds and creates no `.gitignore`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("agent");
    std::fs::create_dir_all(&dir).expect("agent dir");
    put_json(
        &dir.join("config.json"),
        &json!({ "tooling": { "lastCodingAgent": "claude" } }),
    );
    update_agent_config_in(&dir.join("config.json"), None, no_change).expect("no .ac");
    assert_eq!(
        get_json(&state_file_path(&dir))["tooling"]["lastCodingAgent"],
        json!("claude")
    );
    assert!(
        !tree_of(tmp.path())
            .keys()
            .any(|path| path.file_name().is_some_and(|name| name == ".gitignore")),
        "no .gitignore anywhere"
    );
}

/// 3.2 step 1 - the nearest `.ac` ancestor, for each path shape.
#[test]
fn the_sweep_finds_the_ac_root_for_every_path_shape() {
    let instance = crate::config::agent_local_dir_name();
    let shapes = [
        ("Matrix", PathBuf::from("_agent_a")),
        ("per-instance", Path::new("_agent_a").join(&instance)),
        ("replica", Path::new("room-1").join("__agent_b")),
    ];
    for (shape, relative) in shapes {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        let dir = ac_root.join(relative);
        std::fs::create_dir_all(&dir).expect("agent dir");
        assert_eq!(
            nearest_ac_root(&dir.join("config.json")),
            Some(ac_root.as_path()),
            "{shape}"
        );
        update_agent_config_in(&dir.join("config.json"), None, no_change).expect(shape);
        assert!(ignore_rows_present(&ac_root), "{shape}: rows appended");
        // A second call adds nothing.
        let content = std::fs::read(ac_root.join(".gitignore")).expect("read");
        update_agent_config_in(&dir.join("config.json"), None, no_change).expect(shape);
        assert_eq!(
            std::fs::read(ac_root.join(".gitignore")).expect("read"),
            content
        );
    }
    assert_eq!(nearest_ac_root(Path::new("agent/config.json")), None);
}

// -- #2816 (C3): the writers reach the pair -------------------------------

/// E1.
#[test]
fn the_stamp_lands_in_the_state_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    seed_instance_config(dir, &json!({ "repos": ["repo-a"] }));
    let config = instance_config(dir);

    assert_eq!(set(dir, T2), Ok(true));
    assert_eq!(
        state_beside(&config)["tooling"]["lastAgentMessageAt"],
        json!(T2)
    );
    assert_eq!(
        stored(dir),
        json!({ "repos": ["repo-a"] }),
        "the decisions file holds no stamp"
    );

    // A stored newer value is still kept.
    assert_eq!(set(dir, T1), Ok(false));
    assert_eq!(
        state_beside(&config)["tooling"]["lastAgentMessageAt"],
        json!(T2)
    );
    assert_no_state_key_tracked(&config);
}

/// E2.
#[test]
fn the_last_agent_and_its_map_land_in_the_state_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    seed_instance_config(
        dir,
        &json!({
            "repos": ["repo-a"],
            "tooling": { "codingAgents": { "claude": {
                "app": "Old", "futureKey": { "x": 1 }
            } } }
        }),
    );
    let config = instance_config(dir);
    upsert_config(
        &config,
        "claude",
        &fixed_entry("claude", &[("A", "aa")]),
        true,
    )
    .expect("upsert");

    let state = state_beside(&config);
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("claude"));
    let entry = &state["tooling"]["codingAgents"]["claude"];
    assert_eq!(entry["app"], json!("Claude Code"));
    assert_eq!(
        entry["futureKey"],
        json!({ "x": 1 }),
        "#2433: an unknown key inside the entry survives"
    );
    assert_no_state_key_tracked(&config);
    assert_eq!(stored(dir)["repos"], json!(["repo-a"]));
}

/// E2b.
#[test]
fn upsert_config_migrates_and_keeps_its_new_value() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old", "profile": "B" } }));
    assert!(bytes_of(&p.state()).is_none(), "no state file yet");

    upsert_config(&p.decisions(), "v_new", &codex_entry(), true).expect("upsert");

    let state = p.stored_state();
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("v_new"));
    assert_eq!(state["split"], split_marker());
    assert!(
        state_keys_in(&p.tracked()).is_empty(),
        "no state key stays tracked"
    );
    assert_eq!(p.tracked()["tooling"]["profile"], json!("B"));
}

/// E2c.
#[test]
fn a_state_only_write_creates_no_tracked_file_and_still_reads_back() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    upsert_config(&dir.join("config.json"), "codex", &codex_entry(), true).expect("upsert");

    let state_name = crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME;
    let names: Vec<String> = dir_bytes(dir)
        .into_iter()
        .map(|(name, _)| name.to_string_lossy().to_string())
        .collect();
    assert_eq!(
        names,
        [
            crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_LOCK.to_string(),
            ".config.json.lock".to_string(),
            format!(".{state_name}.lock"),
            state_name.to_string(),
        ],
        "only the state file and the three lock sidecars exist"
    );
    assert!(!dir.join("config.json").exists());

    let read = read_agent_local_config_json(dir)
        .expect("parseable")
        .expect("the state file alone answers");
    assert_eq!(read["tooling"]["lastCodingAgent"], json!("codex"));
}

/// E3c.
#[test]
fn an_absent_file_waits_for_a_concurrent_creator() {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("__agent_a");
    std::fs::create_dir_all(&dir).expect("replica dir");
    let config = dir.join("config.json");

    // The creator's position: the decisions sidecar is held and
    // `config.json` is still absent.
    let lock_path = std::fs::canonicalize(&dir)
        .expect("canonical dir")
        .join(".config.json.lock");
    let held = crate::config::local_config_io::acquire_sidecar_write_lock(
        &lock_path,
        Duration::from_secs(5),
        "configLockTimeout",
        "local config write lock",
    )
    .expect("hold the decisions sidecar");

    let (about_to_call, started) = channel();
    let (finished, done) = channel();
    let worker_config = config.clone();
    let worker = std::thread::spawn(move || {
        about_to_call.send(()).expect("signal the start");
        let result = update_existing_agent_config(&worker_config, |decisions, _| {
            decisions.insert("seen".to_string(), json!(true));
            Ok(())
        });
        let _ = finished.send(());
        result
    });
    started
        .recv_timeout(Duration::from_secs(5))
        .expect("the worker starts");

    // The bounded wait: the worker must still be blocked on the lock.
    if done.recv_timeout(Duration::from_millis(750)).is_ok() {
        let result = worker.join().expect("the worker thread panicked");
        panic!("the worker completed while the creator still held the lock: {result:?}");
    }

    put_json(&config, &json!({ "identity": "created" }));
    drop(held);

    worker
        .join()
        .expect("the worker thread panicked")
        .expect("a creator mid-publish is waited for, not refused");
    assert_eq!(
        get_json(&config),
        json!({ "identity": "created", "seen": true })
    );
}

/// E4: sites 8, 13, 9 and 10. Sites 5 and 11 are driven in their own
/// modules.
#[test]
fn the_unchanged_writers_still_write_the_decisions_file() {
    use crate::config::coding_agent_profiles as profiles;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix).expect("matrix dir");
    std::fs::create_dir_all(&replica).expect("replica dir");
    std::fs::write(matrix.join("Role.md"), "# Role\n").expect("role");
    let no_state_file = |dir: &Path, site: &str| {
        assert!(
            !state_file_path(dir).exists(),
            "{site} created a state file"
        );
    };

    // Site 8.
    let default = profiles::ReplicaSelectionDefault {
        configuration_ref: None,
        coding_agent_id: "codex".to_string(),
        requested_profile: "B".to_string(),
        selection_locked: false,
    };
    profiles::write_replica_selection_default(&matrix, &default, None).expect("site 8");
    assert_eq!(
        get_json(&matrix.join("config.json"))["tooling"]["replicaSelectionDefault"],
        json!({ "codingAgentId": "codex", "requestedProfile": "B", "selectionLocked": false })
    );
    no_state_file(&matrix, "site 8");

    // Site 13.
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).expect("root dir");
    crate::config::root_agent::merge_root_agent_config(&root.join("config.json")).expect("site 13");
    let merged = get_json(&root.join("config.json"));
    assert_eq!(merged["tooling"], json!({}));
    assert!(merged["context"].is_array(), "{merged}");
    no_state_file(&root, "site 13");

    // Sites 9 and 10, on an existing file.
    let replica_config = replica.join("config.json");
    put_json(
        &replica_config,
        &json!({ "identity": "../../_agent_dev-rust", "tooling": {} }),
    );
    let settings = crate::config::settings::AppSettings {
        project_paths: vec![project.to_string_lossy().to_string()],
        ..Default::default()
    };
    let expected = profiles::read_replica_selection_state(&replica)
        .expectation()
        .expect("a valid unlocked replica");
    let selection = profiles::ReplicaSelectionPair {
        coding_agent_id: "codex".to_string(),
        requested_profile: "B".to_string(),
    };
    profiles::write_replica_selection(
        &settings,
        &replica,
        &selection,
        profiles::SelectionWriteIntent::IndividualAssignLock,
        &expected,
    )
    .expect("site 9");
    let tooling = get_json(&replica_config)["tooling"].clone();
    assert_eq!(tooling["currentCodingAgent"], json!("codex"));
    assert_eq!(tooling["profile"], json!("B"));
    assert_eq!(tooling["selectionLocked"], json!(true));
    no_state_file(&replica, "site 9");

    let expected = profiles::read_replica_selection_state(&replica)
        .expectation()
        .expect("a valid locked replica");
    profiles::clear_replica_selection_lock(&settings, &replica, &expected).expect("site 10");
    assert_eq!(
        get_json(&replica_config)["tooling"]["selectionLocked"],
        json!(false)
    );
    no_state_file(&replica, "site 10");
}

/// E4b.
#[test]
fn a_decision_only_write_migrates_the_state_keys() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": all_four("t"), "context": [] }));
    let _journal = journal_redirect::set(&p.journal);

    crate::config::root_agent::merge_root_agent_config(&p.decisions()).expect("merge");

    assert!(
        state_keys_in(&p.tracked()).is_empty(),
        "the four keys left the tracked file"
    );
    let state = p.stored_state();
    assert_eq!(state["tooling"], all_four("t"));
    assert_eq!(state["split"], split_marker());
    let journal = p
        .journal_bytes()
        .expect("the note is recorded in the directory the guard names");
    assert!(
        String::from_utf8_lossy(&journal).contains("moved to the state file"),
        "the journal carries the move note"
    );
}

/// E4c.
#[test]
fn a_test_without_the_guard_writes_no_journal() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old" } }));

    update_agent_config(&p.decisions(), no_change).expect("a production-form call");

    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("v_old"),
        "the call migrated the key"
    );
    let journal_name = crate::config::instance_artifacts::NAMING_MIGRATION_STATE_NAME;
    let journals: Vec<_> = tree_of(p.tmp.path())
        .into_keys()
        .filter(|path| path.file_name().is_some_and(|name| name == journal_name))
        .collect();
    assert!(journals.is_empty(), "a journal was written: {journals:?}");
    assert_eq!(
        production_journal_dir(),
        None,
        "a lib test without the guard has no journal directory"
    );
}

/// E3b (b): the one body all four require-existing sites call.
#[test]
fn the_require_existing_wrapper_refuses_an_absent_file_and_creates_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("__agent_a");
    std::fs::create_dir_all(&dir).expect("replica dir");
    let config = dir.join("config.json");

    let result = update_existing_agent_config(&config, |decisions, _| {
        decisions.insert("seen".to_string(), json!(true));
        Ok(())
    });

    assert!(!config.exists(), "the wrapper created config.json");
    assert!(
        !state_file_path(&dir).exists(),
        "the wrapper created a state file"
    );
    assert_eq!(
        result,
        Err(format!("Local config {} does not exist", config.display()))
    );
}

/// C4 (#2470): the seeded state file of every row that seeds one. All four
/// state keys, plus the exact D7 marker `stamp_split_marker` writes.
#[rustfmt::skip]
const C4_SEEDED_STATE: &str =
    r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;

/// C4 (#2470): the state file beside a tracked `config.json`.
fn c4_state_path(tracked: &std::path::Path) -> std::path::PathBuf {
    tracked.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

/// C4 (#2470): after one writer, the tracked file carries none of the four
/// state keys. This says nothing at all about the state file: `c4_state_is`
/// is the only helper that reads it, and the row calls them in that order.
fn c4_tracked_is_clean(tracked: &std::path::Path, site: &str) {
    let text = std::fs::read_to_string(tracked).expect("tracked file");
    let value: serde_json::Value = serde_json::from_str(&text).expect("tracked json");
    let tooling = value.get("tooling");
    c4_absent(tooling, "lastCodingAgent", site, &value);
    c4_absent(tooling, "codingAgents", site, &value);
    c4_absent(tooling, "lastAgentMessageAt", site, &value);
    c4_absent(tooling, "profileContentHash", site, &value);
}

fn c4_absent(t: Option<&serde_json::Value>, key: &str, site: &str, all: &serde_json::Value) {
    assert!(
        t.and_then(|t| t.get(key)).is_none(),
        "{site} left {key}: {all}"
    );
}

/// C4 (#2470): the state file EXISTS and is exactly `expected`, compared
/// as parsed JSON so the pretty-printing of `publish_pair_side` is never
/// the subject. `expected` is always a literal written in the row. Absence
/// is NOT admitted here and no longer reads as JSON `null`:
/// `c4_no_state_file` is the only helper that may conclude absence.
fn c4_state_is(state: &std::path::Path, expected: &str, site: &str) {
    let want: serde_json::Value = serde_json::from_str(expected).expect("expected json");
    let raw = std::fs::read_to_string(state).expect("state file");
    let got: serde_json::Value = serde_json::from_str(&raw).expect("state json");
    assert_eq!(got, want, "{site} state file");
}

#[rustfmt::skip]
const C4_STATE_AFTER_SITE_1: &str =
    r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"lastAgentMessageAt":"2026-06-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;
#[rustfmt::skip]
const C4_STATE_AFTER_SITE_2: &str =
    r#"{"tooling":{"lastCodingAgent":"claude","codingAgents":{"claude":{"app":"Claude Code","acSessionId":"sid","lastUsed":"2026-09-02T01:00:00+00:00","command":"claude","identity":{"A":"aa"}}},"lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;

/// C4 (#2470) E11, sites 1 and 2: each writes the state file and leaves the
/// tracked file without a state key.
#[test]
fn c4_sites_1_2_leave_no_state_key_in_the_tracked_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _journal = journal_redirect::set(tmp.path());
    let seed = json!({ "repos": ["repo-a"], "tooling": {} });

    let dir1 = tmp.path().join("site-1");
    seed_instance_config(&dir1, &seed);
    let tracked = instance_config(&dir1);
    std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).expect("seed state");
    assert_eq!(
        set_last_agent_message_at(&dir1.to_string_lossy(), "2026-06-01T00:00:00Z"),
        Ok(true)
    );
    c4_tracked_is_clean(&tracked, "site 1");
    c4_state_is(&c4_state_path(&tracked), C4_STATE_AFTER_SITE_1, "site 1");
    assert_eq!(stored(&dir1)["repos"], json!(["repo-a"]));

    let dir2 = tmp.path().join("site-2");
    seed_instance_config(&dir2, &seed);
    let tracked = instance_config(&dir2);
    std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).expect("seed state");
    assert_eq!(
        upsert_config(
            &tracked,
            "claude",
            &fixed_entry("claude", &[("A", "aa")]),
            true
        ),
        Ok(())
    );
    c4_tracked_is_clean(&tracked, "site 2");
    c4_state_is(&c4_state_path(&tracked), C4_STATE_AFTER_SITE_2, "site 2");
    assert_eq!(stored(&dir2)["repos"], json!(["repo-a"]));
}
