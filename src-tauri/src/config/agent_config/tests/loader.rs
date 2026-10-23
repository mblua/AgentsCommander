use super::*;

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
pub(super) fn dir_bytes(dir: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
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

pub(super) fn all_four(tag: &str) -> serde_json::Value {
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
