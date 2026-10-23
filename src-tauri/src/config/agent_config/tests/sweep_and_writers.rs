use super::activity_stamp::{
    assert_no_state_key_tracked, instance_config, seed_instance_config, set, state_beside, stored,
    T1, T2,
};
use super::coding_agent_entries::{codex_entry, fixed_entry};
use super::loader::{all_four, dir_bytes};
use super::pair_fixture::{
    bytes_of, get_json, no_change, pair, put_json, set_tooling, split_marker, state_keys_in,
    tree_of,
};
use super::*;

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
const C4_SEEDED_STATE: &str = concat!(
    r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"#,
    r#""lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"#,
    r#""split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","#,
    r#""profileContentHash"]}}"#
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

#[rustfmt::skip]
const C4_STATE_AFTER_SITE_1: &str = concat!(
    r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"#,
    r#""lastAgentMessageAt":"2026-06-01T00:00:00Z","profileContentHash":"h"},"#,
    r#""split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","#,
    r#""profileContentHash"]}}"#
);
#[rustfmt::skip]
const C4_STATE_AFTER_SITE_2: &str = concat!(
    r#"{"tooling":{"lastCodingAgent":"claude","codingAgents":{"claude":{"app":"Claude Code","#,
    r#""acSessionId":"sid","lastUsed":"2026-09-02T01:00:00+00:00","command":"claude","#,
    r#""identity":{"A":"aa"}}},"lastAgentMessageAt":"2026-01-01T00:00:00Z","#,
    r#""profileContentHash":"h"},"#,
    r#""split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","#,
    r#""profileContentHash"]}}"#
);

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
