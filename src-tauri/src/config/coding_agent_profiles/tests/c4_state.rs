use super::*;

/// C4 (#2470): the seeded state file of every row that seeds one. All four
/// state keys, plus the exact D7 marker `stamp_split_marker` writes.
#[rustfmt::skip]
const C4_SEEDED_STATE: &str =
    concat!(
        r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"#,
        r#""lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"#,
        r#""keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#,
    );

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

/// C4 (#2470): no state file exists beside the tracked file. Absence is
/// `NotFound` from `symlink_metadata` and nothing else: an existing entry
/// of any kind is a failure, a JSON `null`, an empty file, a directory and
/// a dangling symlink included, and so is every other error. Round 4's
/// expectation `"null"` through `c4_state_is` admitted the JSON `null`, the
/// directory and every read error, and failed an empty file by the wrong
/// assertion, a parse panic rather than an absence check.
fn c4_no_state_file(state: &std::path::Path, site: &str) {
    match std::fs::symlink_metadata(state) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(m) => panic!(
            "{site} left a state entry, is_dir={}: {state:?}",
            m.is_dir()
        ),
        Err(e) => panic!("{site} could not observe {state:?}: {e}"),
    }
}

/// C4 (#2470): `c4_no_state_file` must reject `state`. The panic message of
/// the rejected call reaches stderr and is expected output, not a failure.
fn c4_rejects(state: &std::path::Path, case: &str) {
    let probe = state.to_path_buf();
    let outcome = std::panic::catch_unwind(move || c4_no_state_file(&probe, "control"));
    assert!(outcome.is_err(), "{case}: c4_no_state_file accepted it");
}

/// C4 (#2470): the four negative controls of the absence contract, on a
/// directory of the row's own, so no site fixture is disturbed.
fn c4_absence_controls(dir: &std::path::Path) {
    let state = c4_state_path(&dir.join("config.json"));
    c4_no_state_file(&state, "control: nothing at the path");
    for bytes in ["null", ""] {
        std::fs::write(&state, bytes).expect("plant a state file");
        c4_rejects(&state, "control: an existing state file");
    }
    std::fs::remove_file(&state).expect("remove the planted file");
    std::fs::create_dir(&state).expect("plant a state directory");
    c4_rejects(&state, "control: a state directory");
}

#[rustfmt::skip]
const C4_STATE_AFTER_SITE_3A: &str =
    concat!(
        r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"#,
        r#""lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"c4hash"},"#,
        r#""split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","#,
        r#""profileContentHash"]}}"#,
    );

/// C4 (#2470) E12, sites 3a, 3b, 8, 9, 10 and 11, each on its own fixture
/// except 9 and 10, which need the same replica.
#[test]
fn c4_sites_3_8_9_10_11_leave_no_state_key_in_the_tracked_file() {
    let journal = tempfile::tempdir().unwrap();
    let _journal = crate::config::agent_config::journal_redirect::set(journal.path());
    let seed = |dir: &Path| -> PathBuf {
        write_config(dir, &seed_selection_config("{}"));
        let tracked = dir.join("config.json");
        std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).unwrap();
        tracked
    };
    let tooling = |dir: &Path| config_value(dir)["tooling"].clone();

    let fx = selection_fixture();
    let tracked = seed(&fx.replica);
    assert_eq!(
        set_replica_profile_content_hash(&fx.replica, "c4hash"),
        Ok(())
    );
    c4_tracked_is_clean(&tracked, "site 3a");
    c4_state_is(&c4_state_path(&tracked), C4_STATE_AFTER_SITE_3A, "site 3a");
    assert_eq!(tooling(&fx.replica), serde_json::json!({}));

    let fx = selection_fixture();
    let tracked = seed(&fx.matrix);
    assert_eq!(
        set_agent_default_profile(&fx.settings, &fx.matrix, "b"),
        Ok(())
    );
    c4_tracked_is_clean(&tracked, "site 3b");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 3b");
    assert_eq!(
        tooling(&fx.matrix)["defaultProfile"],
        serde_json::json!("B")
    );

    let fx = selection_fixture();
    let default = ReplicaSelectionDefault {
        configuration_ref: None,
        coding_agent_id: "codex".to_string(),
        requested_profile: "B".to_string(),
        selection_locked: false,
    };
    let outcome = write_replica_selection_default(&fx.matrix, &default, None);
    assert!(outcome.is_ok(), "site 8: {outcome:?}");
    c4_tracked_is_clean(&fx.matrix.join("config.json"), "site 8");
    c4_no_state_file(&c4_state_path(&fx.matrix.join("config.json")), "site 8");
    assert_eq!(
        tooling(&fx.matrix)["replicaSelectionDefault"],
        serde_json::json!({
            "codingAgentId": "codex",
            "requestedProfile": "B",
            "selectionLocked": false
        })
    );

    let fx = selection_fixture();
    let tracked = seed(&fx.replica);
    let pair = ReplicaSelectionPair {
        coding_agent_id: "codex".to_string(),
        requested_profile: "B".to_string(),
    };
    let expected = read_replica_selection_state(&fx.replica)
        .expectation()
        .expect("a valid unlocked replica");
    assert_eq!(
        write_replica_selection(
            &fx.settings,
            &fx.replica,
            &pair,
            SelectionWriteIntent::IndividualAssignLock,
            &expected,
        ),
        Ok(SelectionWriteOutcome {
            changed: true,
            published: true,
            lock_transition: Some(SelectionLockTransition::UnlockedToLocked),
        })
    );
    c4_tracked_is_clean(&tracked, "site 9");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 9");
    assert_eq!(
        tooling(&fx.replica)["currentCodingAgent"],
        serde_json::json!("codex")
    );
    assert_eq!(tooling(&fx.replica)["profile"], serde_json::json!("B"));
    assert_eq!(
        tooling(&fx.replica)["selectionLocked"],
        serde_json::json!(true)
    );

    let expected = read_replica_selection_state(&fx.replica)
        .expectation()
        .expect("a valid locked replica");
    assert_eq!(
        clear_replica_selection_lock(&fx.settings, &fx.replica, &expected),
        Ok(SelectionWriteOutcome {
            changed: true,
            published: true,
            lock_transition: Some(SelectionLockTransition::LockedToUnlocked),
        })
    );
    c4_tracked_is_clean(&tracked, "site 10");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 10");
    assert_eq!(
        tooling(&fx.replica)["selectionLocked"],
        serde_json::json!(false)
    );

    let fx = selection_fixture();
    let tracked = seed(&fx.replica);
    assert_eq!(
        set_instance_profile_override(&fx.settings, &fx.replica, Some("c")),
        Ok(())
    );
    c4_tracked_is_clean(&tracked, "site 11");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 11");
    assert_eq!(tooling(&fx.replica)["profile"], serde_json::json!("C"));
    assert_eq!(
        tooling(&fx.replica)["instanceProfileOverride"],
        serde_json::json!("C")
    );
    assert_eq!(
        tooling(&fx.replica)["instanceProfileOverrideSource"],
        serde_json::json!("manual")
    );

    let control_dir = tempfile::tempdir().unwrap();
    c4_absence_controls(control_dir.path());
}
