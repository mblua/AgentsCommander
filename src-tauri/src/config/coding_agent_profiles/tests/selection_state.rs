use super::*;

#[test]
fn issue_1937_selection_state_clear_locked_to_unlocked_publishes() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
        ),
    );
    let expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();
    let outcome =
        clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected).unwrap();
    assert_eq!(
        outcome,
        SelectionWriteOutcome {
            changed: true,
            published: true,
            lock_transition: Some(SelectionLockTransition::LockedToUnlocked),
        }
    );
    let saved = config_value(&fixture.replica);
    assert_eq!(saved["tooling"]["selectionLocked"], false);
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "B");
    match read_replica_selection_state(&fixture.replica) {
        ReplicaSelectionState::Unlocked { pair, .. } => {
            assert_eq!(pair, Some(self::pair("codex", "B")));
        }
        other => panic!("expected Unlocked, got {other:?}"),
    }
}

#[test]
fn issue_1937_selection_state_already_unlocked_no_publish() {
    let fixture = selection_fixture();

    // Absent flag: exact deliberately non-pretty bytes retained.
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"currentCodingAgent":"codex","profile":"B"}"#),
    );
    let before = config_bytes(&fixture.replica);
    let expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();
    assert_eq!(
        clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected).unwrap(),
        SelectionWriteOutcome::UNCHANGED
    );
    assert_eq!(config_bytes(&fixture.replica), before);

    // Explicit false: same no-publish path.
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":false}"#,
        ),
    );
    let before = config_bytes(&fixture.replica);
    let expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();
    assert_eq!(
        clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected).unwrap(),
        SelectionWriteOutcome::UNCHANGED
    );
    assert_eq!(config_bytes(&fixture.replica), before);

    // Legacy debris selects the explicit refusal callback for decisions only.
    // The already-unlocked path must bypass that callback and preserve bytes.
    let temp_obstruction = validated_replica_dir(&fixture.settings, &fixture.replica)
        .unwrap()
        .join(format!(".config.json.{}.tmp", std::process::id()));
    std::fs::create_dir(&temp_obstruction).unwrap();
    let publish = |tmp: &std::path::Path, target: &std::path::Path| {
        assert!(
            tmp.is_file(),
            "fresh staging acquired before injected refusal"
        );
        if crate::config::agent_config::temp_config_path(target) == temp_obstruction {
            assert!(temp_obstruction.is_dir());
            Err("fixture temp config publish failure".into())
        } else {
            crate::config::agent_config::publish_temp_config(tmp, target)
        }
    };
    assert_eq!(
        clear_replica_selection_lock_with_publish(
            &fixture.settings,
            &fixture.replica,
            &expected,
            Some(&publish)
        )
        .unwrap(),
        SelectionWriteOutcome::UNCHANGED
    );
    assert!(temp_obstruction.is_dir());
    assert_eq!(config_bytes(&fixture.replica), before);

    // Positive control: a true flag reaches the publisher and is refused
    // after fresh acquisition — the error is never masked as success.
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
        ),
    );
    let locked_before = config_bytes(&fixture.replica);
    let locked_expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();
    let error = clear_replica_selection_lock_with_publish(
        &fixture.settings,
        &fixture.replica,
        &locked_expected,
        Some(&publish),
    )
    .unwrap_err();
    assert!(error.contains("temp config"), "{error}");
    assert!(temp_obstruction.is_dir());
    assert_eq!(config_bytes(&fixture.replica), locked_before);
}

#[test]
fn issue_1937_selection_state_clear_stale_race_is_not_already_unlocked() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"currentCodingAgent":"codex","profile":"B"}"#),
    );
    let unlocked_expectation = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();

    // The flag races to true after the read.
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
        ),
    );
    let before = config_bytes(&fixture.replica);
    let error =
        clear_replica_selection_lock(&fixture.settings, &fixture.replica, &unlocked_expectation)
            .unwrap_err();
    assert!(error.contains("stale"), "{error}");
    assert_eq!(config_bytes(&fixture.replica), before);
}

#[test]
fn issue_1937_selection_state_lock_timeout_is_not_success() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"currentCodingAgent":"codex","profile":"B"}"#),
    );
    let expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();

    // Hold the #1938 sidecar: the guarded call must time out and propagate,
    // never translate into AlreadyUnlocked success.
    let sidecar = std::fs::canonicalize(&fixture.replica)
        .unwrap()
        .join(".config.json.lock");
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&sidecar)
        .unwrap();
    holder.lock().unwrap();

    let before = config_bytes(&fixture.replica);
    let error =
        clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected).unwrap_err();
    assert!(error.contains("configLockTimeout"), "{error}");
    assert_eq!(config_bytes(&fixture.replica), before);
    drop(holder);
}

#[test]
fn issue_1937_selection_state_default_read_shapes() {
    let fixture = selection_fixture();

    assert_eq!(
        read_replica_selection_default(&fixture.matrix).unwrap(),
        None
    );

    std::fs::write(
        fixture.matrix.join("config.json"),
        r#"{"context":["Role.md"]}"#,
    )
    .unwrap();
    assert_eq!(
        read_replica_selection_default(&fixture.matrix).unwrap(),
        None
    );
    std::fs::write(
        fixture.matrix.join("config.json"),
        r#"{"tooling":{"defaultProfile":"B"}}"#,
    )
    .unwrap();
    assert_eq!(
        read_replica_selection_default(&fixture.matrix).unwrap(),
        None
    );

    std::fs::write(
        fixture.matrix.join("config.json"),
        concat!(
            r#"{"tooling":{"replicaSelectionDefault":{"codingAgentId":"codex","#,
            r#""requestedProfile":"b","selectionLocked":true}}}"#,
        ),
    )
    .unwrap();
    assert_eq!(
        read_replica_selection_default(&fixture.matrix).unwrap(),
        Some(ReplicaSelectionDefault {
            configuration_ref: None,
            coding_agent_id: "codex".to_string(),
            requested_profile: "B".to_string(),
            selection_locked: true,
        })
    );

    for tooling in [
        r#"{"replicaSelectionDefault":5}"#,
        r#"{"replicaSelectionDefault":{"requestedProfile":"B","selectionLocked":false}}"#,
        concat!(
            r#"{"replicaSelectionDefault":{"codingAgentId":"","requestedProfile":"B","#,
            r#""selectionLocked":false}}"#,
        ),
        concat!(
            r#"{"replicaSelectionDefault":{"codingAgentId":"codex","requestedProfile":"AB","#,
            r#""selectionLocked":false}}"#,
        ),
        r#"{"replicaSelectionDefault":{"codingAgentId":"codex","requestedProfile":"B"}}"#,
        concat!(
            r#"{"replicaSelectionDefault":{"codingAgentId":"codex","requestedProfile":"B","#,
            r#""selectionLocked":"yes"}}"#,
        ),
    ] {
        std::fs::write(
            fixture.matrix.join("config.json"),
            format!(r#"{{"tooling":{tooling}}}"#),
        )
        .unwrap();
        let error = read_replica_selection_default(&fixture.matrix).unwrap_err();
        assert!(
            error.contains("replicaSelectionDefault"),
            "{tooling}: {error}"
        );
    }

    std::fs::write(fixture.matrix.join("config.json"), r#"{"tooling":5}"#).unwrap();
    assert!(read_replica_selection_default(&fixture.matrix).is_err());
    std::fs::write(fixture.matrix.join("config.json"), "{ not json").unwrap();
    assert!(read_replica_selection_default(&fixture.matrix).is_err());
}

#[test]
fn issue_1937_selection_state_default_write_cas_and_preservation() {
    let fixture = selection_fixture();
    let default = ReplicaSelectionDefault {
        configuration_ref: None,
        coding_agent_id: "codex".to_string(),
        requested_profile: "b".to_string(),
        selection_locked: false,
    };

    let outcome = write_replica_selection_default(&fixture.matrix, &default, None).unwrap();
    assert!(outcome.changed && outcome.published);
    assert_eq!(outcome.lock_transition, None);
    let saved = config_value(&fixture.matrix);
    assert_eq!(
        saved["tooling"]["replicaSelectionDefault"]["requestedProfile"],
        "B"
    );

    // An expected-prior of None is now stale.
    let stale = write_replica_selection_default(&fixture.matrix, &default, None).unwrap_err();
    assert!(stale.contains("stale"), "{stale}");

    // Update with the exact prior: other tooling and unknown keys survive.
    let mut matrix = config_value(&fixture.matrix);
    matrix["tooling"]["defaultProfile"] = serde_json::json!("A");
    matrix["customProjectKey"] = serde_json::json!(["keep"]);
    std::fs::write(
        fixture.matrix.join("config.json"),
        serde_json::to_string(&matrix).unwrap(),
    )
    .unwrap();
    let prior = read_replica_selection_default(&fixture.matrix).unwrap();
    let next = ReplicaSelectionDefault {
        configuration_ref: None,
        coding_agent_id: "claude".to_string(),
        requested_profile: "C".to_string(),
        selection_locked: true,
    };
    write_replica_selection_default(&fixture.matrix, &next, prior.as_ref()).unwrap();
    let saved = config_value(&fixture.matrix);
    assert_eq!(
        saved["tooling"]["replicaSelectionDefault"]["codingAgentId"],
        "claude"
    );
    assert_eq!(
        saved["tooling"]["replicaSelectionDefault"]["selectionLocked"],
        true
    );
    assert_eq!(saved["tooling"]["defaultProfile"], "A");
    assert_eq!(saved["customProjectKey"][0], "keep");

    // A wrong prior is stale and publishes nothing.
    let before = config_bytes(&fixture.matrix);
    let wrong = write_replica_selection_default(&fixture.matrix, &default, None).unwrap_err();
    assert!(wrong.contains("stale"), "{wrong}");
    assert_eq!(config_bytes(&fixture.matrix), before);
}

#[test]
fn issue_1937_selection_state_null_clear_rejected_for_protected_pair() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
        ),
    );
    let before = config_bytes(&fixture.replica);
    let error =
        set_instance_profile_override(&fixture.settings, &fixture.replica, None).unwrap_err();
    assert!(error.contains("protected"), "{error}");
    assert_eq!(config_bytes(&fixture.replica), before);

    // A new valid requested letter is permitted and preserves Coding Agent
    // and the lock flag.
    set_instance_profile_override(&fixture.settings, &fixture.replica, Some("d")).unwrap();
    let saved = config_value(&fixture.replica);
    assert_eq!(saved["tooling"]["profile"], "D");
    assert_eq!(saved["tooling"]["instanceProfileOverride"], "D");
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["selectionLocked"], true);

    // A legacy unlocked config without a pair may still be cleared.
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"instanceProfileOverride":"C"}"#),
    );
    set_instance_profile_override(&fixture.settings, &fixture.replica, None).unwrap();
    let saved = config_value(&fixture.replica);
    assert!(saved["tooling"].get("profile").is_none());
    assert!(saved["tooling"].get("instanceProfileOverride").is_none());
}

#[test]
fn issue_1937_selection_state_wrapper_preserves_flag_and_rejects_locked() {
    let fixture = selection_fixture();

    // Unlocked: the pair is written, the absent flag stays absent.
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"lastCodingAgent":"claude"}"#),
    );
    set_replica_coding_agent_selection(&fixture.settings, &fixture.replica, "codex", "b").unwrap();
    let saved = config_value(&fixture.replica);
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "B");
    assert!(saved["tooling"].get("selectionLocked").is_none());
    assert_eq!(
        state_value(&fixture.replica)["tooling"]["lastCodingAgent"],
        "claude"
    );
    assert_untracked(&fixture.replica, &["lastCodingAgent"]);

    // Locked: rejected, stored bytes untouched.
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"A","selectionLocked":true}"#,
        ),
    );
    let before = config_bytes(&fixture.replica);
    let error =
        set_replica_coding_agent_selection(&fixture.settings, &fixture.replica, "codex", "B")
            .unwrap_err();
    assert!(error.contains("locked"), "{error}");
    assert_eq!(config_bytes(&fixture.replica), before);
}

#[test]
fn issue_1937_selection_state_content_hash_write_does_not_reset_malformed_tooling() {
    let fixture = selection_fixture();

    write_config(
        &fixture.replica,
        r#"{"identity":"../../_agent_dev-rust","tooling":5}"#,
    );
    let before = config_bytes(&fixture.replica);
    let error = set_replica_profile_content_hash(&fixture.replica, "deadbeef").unwrap_err();
    assert!(
        error.contains("'tooling' must be a JSON object in the tracked config file"),
        "{error}"
    );
    assert_eq!(config_bytes(&fixture.replica), before);

    // A malformed selectionLocked inside a valid tooling object stays exactly
    // as stored while the hash is updated.
    write_config(
        &fixture.replica,
        concat!(
            r#"{"identity":"../../_agent_dev-rust","tooling":{"selectionLocked":"yes","#,
            r#""currentCodingAgent":"codex"}}"#,
        ),
    );
    set_replica_profile_content_hash(&fixture.replica, "deadbeef").unwrap();
    let saved = config_value(&fixture.replica);
    assert_eq!(saved["tooling"]["selectionLocked"], "yes");
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(
        state_value(&fixture.replica)["tooling"]["profileContentHash"],
        "deadbeef"
    );
    assert_untracked(&fixture.replica, &["profileContentHash"]);
}

// ------------------------------------------------------------------
// #2010 KEEP with the same locked pair (unchanged, no publish).
// ------------------------------------------------------------------

// ------------------------------------------------------------------
// #2816 (C3) - the split writer and the require-existing sites.
// ------------------------------------------------------------------

/// E3.
#[test]
fn the_two_tooling_keys_go_to_opposite_files() {
    let fixture = selection_fixture();
    write_config(&fixture.matrix, "{}");

    // The decision first: a later decision write would migrate a misplaced
    // hash and hide it.
    set_agent_default_profile(&fixture.settings, &fixture.matrix, "b").unwrap();
    set_replica_profile_content_hash(&fixture.matrix, "cafef00d").unwrap();

    let state = state_value(&fixture.matrix);
    let tracked = config_value(&fixture.matrix);
    assert_eq!(state["tooling"]["profileContentHash"], "cafef00d");
    assert!(state["tooling"].get("defaultProfile").is_none(), "{state}");
    assert_eq!(tracked["tooling"]["defaultProfile"], "B");
    assert_untracked(&fixture.matrix, &["profileContentHash"]);
}

/// E3b (a): with both files absent, sites 9 and 10 reject earlier, on the
/// replica identity, and create nothing. The seeded second leg proves the
/// rows are sensitive to the missing identity and nothing more.
#[test]
fn an_absent_config_still_refuses_and_creates_nothing() {
    const IDENTITY_TAIL: &str =
        "__agent_dev-rust' has no config.json identity; expected '../../_agent_dev-rust'";
    let is_identity_rejection = |result: &Result<SelectionWriteOutcome, String>| {
        matches!(result, Err(error)
            if error.starts_with("Room replica '") && error.ends_with(IDENTITY_TAIL))
    };
    let expected = ReplicaSelectionExpectation {
        configuration_ref: None,
        profile_override_source: None,
        identity: REPLICA_IDENTITY.to_string(),
        pair: None,
        locked: false,
    };

    // Site 9.
    let fixture = selection_fixture();
    let write = || {
        write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "B"),
            SelectionWriteIntent::Individual,
            &expected,
        )
    };
    let result = write();
    assert!(is_identity_rejection(&result), "site 9: {result:?}");
    assert!(!fixture.replica.join("config.json").exists(), "site 9");
    assert!(!state_path(&fixture.replica).exists(), "site 9");
    write_config(&fixture.replica, &seed_selection_config("{}"));
    let result = write();
    assert!(!is_identity_rejection(&result), "site 9 seeded: {result:?}");

    // Site 10.
    let fixture = selection_fixture();
    let clear = || clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected);
    let result = clear();
    assert!(is_identity_rejection(&result), "site 10: {result:?}");
    assert!(!fixture.replica.join("config.json").exists(), "site 10");
    assert!(!state_path(&fixture.replica).exists(), "site 10");
    write_config(&fixture.replica, &seed_selection_config("{}"));
    let result = clear();
    assert!(
        !is_identity_rejection(&result),
        "site 10 seeded: {result:?}"
    );
}

/// E3b, consequence 4: a new effect of C2's validation, not a preserved
/// contract.
#[test]
fn a_malformed_state_tooling_now_fails_the_selection_write() {
    let fixture = selection_fixture();
    write_config(&fixture.replica, &seed_selection_config("{}"));
    std::fs::write(state_path(&fixture.replica), r#"{"tooling":5}"#).unwrap();
    let before = config_bytes(&fixture.replica);
    let expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();

    let error = write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "B"),
        SelectionWriteIntent::Individual,
        &expected,
    )
    .unwrap_err();

    assert!(error.starts_with("Config pair "), "{error}");
    assert!(
        error.ends_with("cleanup failed: 'tooling' must be a JSON object in the state file"),
        "{error}"
    );
    assert_eq!(config_bytes(&fixture.replica), before);
    assert_eq!(
        std::fs::read_to_string(state_path(&fixture.replica)).unwrap(),
        r#"{"tooling":5}"#
    );
}

/// E4, site 11, through its nearest reachable caller.
#[test]
fn the_profile_override_writes_the_decisions_file_and_no_state_file() {
    let fixture = selection_fixture();
    write_config(&fixture.replica, &seed_selection_config("{}"));

    set_instance_profile_override(&fixture.settings, &fixture.replica, Some("c")).unwrap();

    let tooling = config_value(&fixture.replica)["tooling"].clone();
    assert_eq!(tooling["profile"], "C");
    assert_eq!(tooling["instanceProfileOverride"], "C");
    assert_eq!(tooling["instanceProfileOverrideSource"], "manual");
    assert!(
        !state_path(&fixture.replica).exists(),
        "site 11 created a state file"
    );
}

#[test]
fn issue_2010_locked_same_pair_individual_intents_are_unchanged() {
    for intent in [
        SelectionWriteIntent::Individual,
        SelectionWriteIntent::IndividualAssignLock,
    ] {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();

        // A lowercase request proves the letter is normalized before the
        // equality check and still matches the stored pair.
        let outcome = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "b"),
            intent,
            &expected,
        )
        .unwrap();
        assert_eq!(outcome, SelectionWriteOutcome::UNCHANGED, "{intent:?}");
        assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
        assert_eq!(
            config_value(&fixture.replica)["tooling"]["selectionLocked"],
            true,
            "{intent:?}"
        );

        // A directory at the known temp-config path proves the unchanged
        // path never even attempts a temp file.
        let temp_obstruction = fixture
            .replica
            .join(format!(".config.json.{}.tmp", std::process::id()));
        std::fs::create_dir(&temp_obstruction).unwrap();
        let outcome = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "b"),
            intent,
            &expected,
        )
        .unwrap();
        assert_eq!(outcome, SelectionWriteOutcome::UNCHANGED, "{intent:?}");
        assert!(temp_obstruction.is_dir());
        assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
    }
}

#[test]
fn issue_2010_locked_different_agent_rejected() {
    for intent in [
        SelectionWriteIntent::Individual,
        SelectionWriteIntent::IndividualAssignLock,
    ] {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("claude", "B"),
            intent,
            &expected,
        )
        .unwrap_err();
        assert!(error.contains("locked"), "{intent:?}: {error}");
        assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
    }
}

#[test]
fn issue_2010_locked_different_profile_rejected() {
    for intent in [
        SelectionWriteIntent::Individual,
        SelectionWriteIntent::IndividualAssignLock,
    ] {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "C"),
            intent,
            &expected,
        )
        .unwrap_err();
        assert!(error.contains("locked"), "{intent:?}: {error}");
        assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
    }
}

#[test]
fn issue_2010_locked_same_pair_stale_expectation_rejected() {
    for intent in [
        SelectionWriteIntent::Individual,
        SelectionWriteIntent::IndividualAssignLock,
    ] {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
            ),
        );
        let stale = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();

        // A concurrent writer moves the pair; the stale expectation must
        // fail the CAS before the same-pair rule can apply.
        let rewritten = seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"A","selectionLocked":true}"#,
        );
        write_config(&fixture.replica, &rewritten);
        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "A"),
            intent,
            &stale,
        )
        .unwrap_err();
        assert!(error.contains("stale"), "{intent:?}: {error}");
        assert_eq!(config_bytes(&fixture.replica), rewritten, "{intent:?}");
    }
}

#[test]
fn issue_2010_wrapper_locked_same_pair_is_no_write() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"A","selectionLocked":true}"#,
        ),
    );
    let before = config_bytes(&fixture.replica);

    // Lowercase letter: the wrapper normalizes it to the stored pair and
    // returns Ok without writing.
    set_replica_coding_agent_selection(&fixture.settings, &fixture.replica, "codex", "a").unwrap();
    assert_eq!(config_bytes(&fixture.replica), before);

    // Obstructed temp path: still no write attempt and still Ok.
    let temp_obstruction = fixture
        .replica
        .join(format!(".config.json.{}.tmp", std::process::id()));
    std::fs::create_dir(&temp_obstruction).unwrap();
    set_replica_coding_agent_selection(&fixture.settings, &fixture.replica, "codex", "a").unwrap();
    assert!(temp_obstruction.is_dir());
    assert_eq!(config_bytes(&fixture.replica), before);
}

/// #2786 (C1) E8 - `profileContentHash` has no struct field, so it is the
/// one state key a typed reader cannot route; it must still go state-first.
#[test]
fn the_content_hash_reads_through_the_loader() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path();
    std::fs::write(
        dir.join("config.json"),
        r#"{"tooling":{"profileContentHash":"tracked-hash"}}"#,
    )
    .expect("seed decisions");
    assert_eq!(
        read_replica_profile_content_hash(dir).as_deref(),
        Some("tracked-hash")
    );

    std::fs::write(
        dir.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
        r#"{"tooling":{"profileContentHash":"state-hash"}}"#,
    )
    .expect("seed state");
    assert_eq!(
        read_replica_profile_content_hash(dir).as_deref(),
        Some("state-hash")
    );
}
