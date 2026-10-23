use super::*;

fn e2432_reference(requested: Option<&str>) -> ConfigurationRef {
    use crate::config::settings::{SourceFamily, SourceKind, SourceRef, SourceRole, SourceScope};
    ConfigurationRef {
        version: 1,
        source: SourceRef {
            family: SourceFamily::Agents,
            role: SourceRole::Catalog,
            scope: SourceScope::Project,
            kind: SourceKind::CatalogProject,
            context_root: "fixture-project".into(),
        },
        calculated_id: format!("cid1:{}", "a".repeat(64)),
        requested_profile: requested.map(str::to_owned),
        profile_calculated_id: Some(format!("cid1:{}", "b".repeat(64))),
        legacy_entry_hint: Some("codex".into()),
    }
}

fn e2432_seed_source(fx: &SelectionFixture, marker: Option<&str>, letter: &str, locked: bool) {
    let mut tooling = serde_json::json!({
        "currentCodingAgent": "codex", "profile": letter,
        "instanceProfileOverride": letter, "selectionLocked": locked,
        "configurationRef": e2432_reference(None), "unrelated": 42
    });
    if let Some(marker) = marker {
        tooling["instanceProfileOverrideSource"] = marker.into();
    }
    write_config(&fx.replica, &seed_selection_config(&tooling.to_string()));
}

#[test]
fn e2432_source_null_marker_controls_alias_authority_and_fallback() {
    let fx = selection_fixture();
    for marker in [
        Some("source-default"),
        Some("manual"),
        None,
        Some("historical"),
    ] {
        let alias = if marker == Some("source-default") {
            "B"
        } else {
            "D"
        };
        e2432_seed_source(&fx, marker, alias, false);
        let read = read_source_null_profile_result(&fx.replica).unwrap();
        let mut scoped = settings_with_cells(&[("codex", vec!["C", "D", "E"])]);
        scoped
            .coding_agent_profiles
            .profiles_by_agent
            .get_mut("codex")
            .unwrap()
            .get_mut("D")
            .unwrap()
            .enabled = false;
        scoped
            .coding_agent_profiles
            .default_profile_by_agent
            .insert(
                "dev-rust".into(),
                if alias == "B" { "C" } else { "E" }.into(),
            );
        let resolution = resolve_profile(
            &scoped,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: None,
                agent_matrix_name: Some("dev-rust"),
                requested_profile: read.profile.as_deref(),
                requested_profile_authoritative: true,
            },
        );
        assert_eq!(
            resolution.requested_profile,
            if alias == "B" { "C" } else { "D" }
        );
        assert_eq!(resolution.effective_profile, "C");
        assert_eq!(config_value(&fx.replica)["tooling"]["profile"], alias);
    }
    e2432_seed_source(&fx, Some("source-default"), "B", false);
    let mut value = config_value(&fx.replica);
    value["tooling"]["instanceProfileOverride"] = "D".into();
    write_config(&fx.replica, &value.to_string());
    let read = read_source_null_profile_result(&fx.replica).unwrap();
    assert!(read.profile.is_none());
    assert!(read.warning.is_some());
}

#[test]
fn e2432_source_prepared_publication_manual_clear_and_stale_are_atomic() {
    let fx = selection_fixture();
    e2432_seed_source(&fx, Some("source-default"), "B", false);
    let expected = read_replica_selection_state(&fx.replica)
        .expectation()
        .unwrap();
    let mut prepared = PreparedSourceSelection {
        pair: ReplicaSelectionPair {
            coding_agent_id: "codex".into(),
            requested_profile: "C".into(),
        },
        configuration_ref: e2432_reference(None),
        profile_override_source: Some("source-default".into()),
    };
    assert!(
        write_prepared_source_selection(
            &fx.settings,
            &fx.replica,
            &prepared,
            SelectionWriteIntent::Individual,
            &expected
        )
        .unwrap()
        .published
    );
    assert_eq!(config_value(&fx.replica)["tooling"]["profile"], "C");
    assert!(read_source_null_profile_result(&fx.replica)
        .unwrap()
        .profile
        .is_none());
    let before = config_bytes(&fx.replica);
    assert!(write_prepared_source_selection(
        &fx.settings,
        &fx.replica,
        &prepared,
        SelectionWriteIntent::BulkForceReviewed,
        &expected
    )
    .is_err());
    assert_eq!(config_bytes(&fx.replica), before);
    let expected = read_replica_selection_state(&fx.replica)
        .expectation()
        .unwrap();
    prepared.pair.requested_profile = "D".into();
    prepared.profile_override_source = Some("manual".into());
    prepared.configuration_ref.profile_calculated_id = Some(format!("cid1:{}", "c".repeat(64)));
    set_prepared_source_profile_override(&fx.settings, &fx.replica, &prepared, &expected).unwrap();
    let after = config_value(&fx.replica);
    assert!(after["tooling"]["configurationRef"]["requestedProfile"].is_null());
    assert_eq!(after["tooling"]["profile"], "D");
    assert_eq!(after["tooling"]["instanceProfileOverride"], "D");
    assert_eq!(after["tooling"]["instanceProfileOverrideSource"], "manual");
    assert_eq!(
        after["tooling"]["configurationRef"]["profileCalculatedId"],
        format!("cid1:{}", "c".repeat(64))
    );
    assert_eq!(after["tooling"]["unrelated"], 42);
    for locked in [false, true] {
        e2432_seed_source(&fx, Some("manual"), "D", locked);
        let before = config_bytes(&fx.replica);
        assert!(set_instance_profile_override(&fx.settings, &fx.replica, None).is_err());
        assert_eq!(config_bytes(&fx.replica), before);
    }
    e2432_seed_source(&fx, Some("manual"), "D", false);
    let expected = read_replica_selection_state(&fx.replica)
        .expectation()
        .unwrap();
    prepared.pair.requested_profile = "E".into();
    prepared.profile_override_source = Some("source-default".into());
    let before = config_bytes(&fx.replica);
    assert!(write_prepared_source_selection(
        &fx.settings,
        &fx.replica,
        &prepared,
        SelectionWriteIntent::Individual,
        &expected
    )
    .is_err());
    assert_eq!(config_bytes(&fx.replica), before);
    prepared.pair.requested_profile = "D".into();
    prepared.profile_override_source = Some("manual".into());
    assert!(
        write_prepared_source_selection(
            &fx.settings,
            &fx.replica,
            &prepared,
            SelectionWriteIntent::Individual,
            &expected
        )
        .unwrap()
        .published
    );
    // A deliberate legacy replacement removes stale provenance in its publish.
    let expected = read_replica_selection_state(&fx.replica)
        .expectation()
        .unwrap();
    write_replica_selection(
        &fx.settings,
        &fx.replica,
        &prepared.pair,
        SelectionWriteIntent::Individual,
        &expected,
    )
    .unwrap();
    assert!(config_value(&fx.replica)["tooling"]
        .get("configurationRef")
        .is_none());
}

#[test]
fn e2432_source_reference_default_roundtrip_invalid_and_lock_controls() {
    let fx = selection_fixture();
    let default = ReplicaSelectionDefault {
        coding_agent_id: "codex".into(),
        requested_profile: "B".into(),
        selection_locked: true,
        configuration_ref: Some(e2432_reference(None)),
    };
    write_replica_selection_default(&fx.matrix, &default, None).unwrap();
    assert_eq!(
        read_replica_selection_default(&fx.matrix).unwrap(),
        Some(default.clone())
    );
    set_agent_default_profile(&fx.settings, &fx.matrix, "E").unwrap();
    assert_eq!(
        read_replica_selection_default(&fx.matrix).unwrap(),
        Some(default.clone())
    );
    let mut value = serde_json::json!({"identity": REPLICA_IDENTITY});
    apply_replica_selection_default(&mut value, &default).unwrap();
    assert_eq!(
        value["tooling"]["instanceProfileOverrideSource"],
        "source-default"
    );
    write_config(&fx.replica, &value.to_string());
    let expected = read_replica_selection_state(&fx.replica)
        .expectation()
        .unwrap();
    let prepared = PreparedSourceSelection {
        pair: expected.pair.clone().unwrap(),
        configuration_ref: default.configuration_ref.clone().unwrap(),
        profile_override_source: Some("source-default".into()),
    };
    let before = config_bytes(&fx.replica);
    for intent in [
        SelectionWriteIntent::Individual,
        SelectionWriteIntent::BulkOrdinary,
    ] {
        assert!(
            !write_prepared_source_selection(
                &fx.settings,
                &fx.replica,
                &prepared,
                intent,
                &expected
            )
            .unwrap()
            .published
        );
        assert_eq!(config_bytes(&fx.replica), before);
    }
    for bad in [
        Value::Null,
        serde_json::json!({}),
        serde_json::to_value(e2432_reference(Some("D"))).unwrap(),
    ] {
        let mut invalid = value.clone();
        invalid["tooling"]["configurationRef"] = bad;
        write_config(&fx.replica, &invalid.to_string());
        let before = config_bytes(&fx.replica);
        assert!(read_replica_selection_state(&fx.replica)
            .expectation()
            .is_err());
        assert!(write_prepared_source_selection(
            &fx.settings,
            &fx.replica,
            &prepared,
            SelectionWriteIntent::BulkForceReviewed,
            &expected
        )
        .is_err());
        assert_eq!(config_bytes(&fx.replica), before);
    }
    // Marker and reference drift alone invalidate an otherwise equal pair.
    for field in ["instanceProfileOverrideSource", "configurationRef"] {
        let mut changed = value.clone();
        if field == "configurationRef" {
            changed["tooling"][field]["profileCalculatedId"] =
                format!("cid1:{}", "d".repeat(64)).into();
        } else {
            changed["tooling"][field] = "historical".into();
        }
        write_config(&fx.replica, &changed.to_string());
        let before = config_bytes(&fx.replica);
        assert!(write_prepared_source_selection(
            &fx.settings,
            &fx.replica,
            &prepared,
            SelectionWriteIntent::BulkForceReviewed,
            &expected
        )
        .is_err());
        assert_eq!(config_bytes(&fx.replica), before);
    }
}

#[test]
fn issue_1937_selection_state_read_absent_invalid_and_duplicate_json() {
    let fixture = selection_fixture();

    // Absent config.json is Invalid, never Unlocked.
    assert!(matches!(
        read_replica_selection_state(&fixture.replica),
        ReplicaSelectionState::Invalid { .. }
    ));

    // Invalid JSON.
    write_config(&fixture.replica, "{ not json");
    assert!(matches!(
        read_replica_selection_state(&fixture.replica),
        ReplicaSelectionState::Invalid { .. }
    ));

    // Duplicate keys are rejected by the strict duplicate-free parse.
    write_config(
        &fixture.replica,
        r#"{"identity":"../../_agent_dev-rust","identity":"../../_agent_dev-rust"}"#,
    );
    let duplicate = read_replica_selection_state(&fixture.replica);
    assert!(
        matches!(duplicate, ReplicaSelectionState::Invalid { .. }),
        "{duplicate:?}"
    );
}

#[test]
fn issue_1937_selection_state_read_malformed_shapes_are_never_unlocked() {
    let fixture = selection_fixture();

    for tooling in [
        "5",
        r#"{"selectionLocked":"yes"}"#,
        r#"{"selectionLocked":true}"#,
        r#"{"selectionLocked":true,"currentCodingAgent":"codex"}"#,
        r#"{"selectionLocked":true,"profile":"B","currentCodingAgent":""}"#,
    ] {
        write_config(&fixture.replica, &seed_selection_config(tooling));
        let state = read_replica_selection_state(&fixture.replica);
        assert!(
            matches!(state, ReplicaSelectionState::Invalid { .. }),
            "tooling {tooling} must be Invalid, got {state:?}"
        );
        assert!(!state.is_locked());
    }
}

#[test]
fn issue_1937_selection_state_read_missing_pair_on_legacy_unlocked_is_valid_and_assignable() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"lastCodingAgent":"claude"}"#),
    );

    let state = read_replica_selection_state(&fixture.replica);
    match &state {
        ReplicaSelectionState::Unlocked { pair, warning, .. } => {
            assert!(pair.is_none());
            assert!(warning.is_none());
        }
        other => panic!("expected Unlocked with no pair, got {other:?}"),
    }

    // A missing pair on a legacy unlocked config may be deliberately
    // assigned.
    let expected = state.expectation().unwrap();
    let outcome = write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "B"),
        SelectionWriteIntent::Individual,
        &expected,
    )
    .unwrap();
    assert!(outcome.changed && outcome.published);
    assert_eq!(outcome.lock_transition, None);
    let saved = config_value(&fixture.replica);
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "B");
    assert_eq!(saved["tooling"]["instanceProfileOverride"], "B");
    assert_eq!(saved["tooling"]["instanceProfileOverrideSource"], "manual");
    assert!(saved["tooling"].get("selectionLocked").is_none());
    assert_eq!(
        state_value(&fixture.replica)["tooling"]["lastCodingAgent"],
        "claude"
    );
    assert_untracked(&fixture.replica, &["lastCodingAgent"]);
}

#[test]
fn issue_1937_selection_state_read_coherent_and_divergent_duals() {
    let fixture = selection_fixture();

    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"B","instanceProfileOverride":"B"}"#,
        ),
    );
    match read_replica_selection_state(&fixture.replica) {
        ReplicaSelectionState::Unlocked { pair, warning, .. } => {
            assert_eq!(pair, Some(self::pair("codex", "B")));
            assert!(warning.is_none());
        }
        other => panic!("expected Unlocked pair, got {other:?}"),
    }

    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"B","instanceProfileOverride":"C"}"#,
        ),
    );
    match read_replica_selection_state(&fixture.replica) {
        ReplicaSelectionState::Unlocked { pair, warning, .. } => {
            assert_eq!(pair, Some(self::pair("codex", "B")), "profile wins");
            let warning = warning.expect("divergent dual fields warn");
            assert!(warning.contains("differs"), "{warning}");
        }
        other => panic!("expected Unlocked pair, got {other:?}"),
    }
}

#[test]
fn issue_1937_selection_state_write_preserves_unknown_history_and_hash() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"claude","profile":"A","instanceProfileOverride":"A","lastCodingAgent":"claude","profileContentHash":"deadbeef","codingAgents":{"claude":{"app":"Claude Code","lastUsed":"2026-09-02T01:00:00+00:00"}}}"#,
        ),
    );
    let mut seeded: Value = serde_json::from_str(&config_bytes(&fixture.replica)).unwrap();
    seeded["customTopLevel"] = serde_json::json!({"keep": true});
    write_config(&fixture.replica, &seeded.to_string());

    let state = read_replica_selection_state(&fixture.replica);
    let expected = state.expectation().unwrap();
    let outcome = write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "C"),
        SelectionWriteIntent::Individual,
        &expected,
    )
    .unwrap();
    assert!(outcome.changed && outcome.published);

    let saved = config_value(&fixture.replica);
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "C");
    let state = state_value(&fixture.replica);
    assert_eq!(state["tooling"]["lastCodingAgent"], "claude");
    assert_eq!(state["tooling"]["profileContentHash"], "deadbeef");
    assert_eq!(
        state["tooling"]["codingAgents"]["claude"]["app"],
        "Claude Code"
    );
    assert_untracked(
        &fixture.replica,
        &["lastCodingAgent", "profileContentHash", "codingAgents"],
    );
    assert_eq!(saved["customTopLevel"]["keep"], true);
}

#[test]
fn issue_1937_selection_state_expected_state_cas_is_stale_without_overwrite() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"currentCodingAgent":"codex","profile":"B"}"#),
    );
    let stale_expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();

    // A concurrent writer moves the pair; the stale expectation must not
    // overwrite it.
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"currentCodingAgent":"claude","profile":"A"}"#),
    );
    let before = config_bytes(&fixture.replica);
    let error = write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "C"),
        SelectionWriteIntent::Individual,
        &stale_expected,
    )
    .unwrap_err();
    assert!(error.contains("stale"), "{error}");
    assert_eq!(config_bytes(&fixture.replica), before);

    let current = read_replica_selection_state(&fixture.replica);
    let mut wrong_identity = current.expectation().unwrap();
    wrong_identity.identity = "../../_agent_other".to_string();
    let error = write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "C"),
        SelectionWriteIntent::Individual,
        &wrong_identity,
    )
    .unwrap_err();
    assert!(error.contains("stale"), "{error}");

    let mut wrong_flag = current.expectation().unwrap();
    wrong_flag.locked = true;
    let error = write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "C"),
        SelectionWriteIntent::Individual,
        &wrong_flag,
    )
    .unwrap_err();
    assert!(error.contains("stale"), "{error}");
    assert_eq!(config_bytes(&fixture.replica), before);
}

#[test]
fn issue_1937_selection_state_intent_table() {
    let cases = [
        (SelectionWriteIntent::Individual, true, false, None),
        (
            SelectionWriteIntent::IndividualAssignLock,
            true,
            true,
            Some(SelectionLockTransition::UnlockedToLocked),
        ),
        (SelectionWriteIntent::BulkOrdinary, true, false, None),
        (
            SelectionWriteIntent::BulkAssignLockUnlockedOnly,
            true,
            true,
            Some(SelectionLockTransition::UnlockedToLocked),
        ),
        (
            SelectionWriteIntent::BulkForceReviewed,
            true,
            true,
            Some(SelectionLockTransition::UnlockedToLocked),
        ),
    ];
    for (intent, expect_change, expect_flag_true, transition) in cases {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"currentCodingAgent":"claude","profile":"A"}"#),
        );
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        let outcome = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "B"),
            intent,
            &expected,
        )
        .unwrap();
        assert_eq!(outcome.changed, expect_change, "{intent:?}");
        assert_eq!(outcome.published, expect_change, "{intent:?}");
        assert_eq!(outcome.lock_transition, transition, "{intent:?}");
        let saved = config_value(&fixture.replica);
        assert_eq!(
            saved["tooling"]["currentCodingAgent"], "codex",
            "{intent:?}"
        );
        assert_eq!(saved["tooling"]["profile"], "B", "{intent:?}");
        let stored_flag = saved["tooling"]
            .get("selectionLocked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(stored_flag, expect_flag_true, "{intent:?}");
    }

    // Locked replicas: individual intents are rejected (bytes untouched);
    // bulk intents skip without publishing; force applies and keeps locked.
    for intent in [
        SelectionWriteIntent::Individual,
        SelectionWriteIntent::IndividualAssignLock,
    ] {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"claude","profile":"A","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "B"),
            intent,
            &expected,
        )
        .unwrap_err();
        assert!(error.contains("locked"), "{intent:?}: {error}");
        assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
    }
    for intent in [
        SelectionWriteIntent::BulkOrdinary,
        SelectionWriteIntent::BulkAssignLockUnlockedOnly,
    ] {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"claude","profile":"A","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        let outcome = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "B"),
            intent,
            &expected,
        )
        .unwrap();
        assert_eq!(outcome, SelectionWriteOutcome::UNCHANGED, "{intent:?}");
        assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
    }

    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"claude","profile":"A","selectionLocked":true}"#,
        ),
    );
    let before = config_bytes(&fixture.replica);
    let expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();
    let outcome = write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "B"),
        SelectionWriteIntent::BulkForceReviewed,
        &expected,
    )
    .unwrap();
    assert!(outcome.changed && outcome.published);
    assert_eq!(outcome.lock_transition, None, "already locked");
    assert_ne!(config_bytes(&fixture.replica), before);
    let saved = config_value(&fixture.replica);
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["selectionLocked"], true);
}

#[test]
fn issue_1937_selection_state_malformed_is_never_force_repaired() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(
            r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":"yes"}"#,
        ),
    );
    let before = config_bytes(&fixture.replica);
    let invalid = read_replica_selection_state(&fixture.replica);
    assert!(matches!(invalid, ReplicaSelectionState::Invalid { .. }));
    assert!(invalid.expectation().is_err());

    let error = write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "B"),
        SelectionWriteIntent::BulkForceReviewed,
        &ReplicaSelectionExpectation {
            configuration_ref: None,
            profile_override_source: None,
            identity: REPLICA_IDENTITY.to_string(),
            pair: Some(pair("codex", "B")),
            locked: false,
        },
    )
    .unwrap_err();
    assert!(error.contains("invalid"), "{error}");
    assert_eq!(config_bytes(&fixture.replica), before);

    let error = clear_replica_selection_lock(
        &fixture.settings,
        &fixture.replica,
        &ReplicaSelectionExpectation {
            configuration_ref: None,
            profile_override_source: None,
            identity: REPLICA_IDENTITY.to_string(),
            pair: Some(pair("codex", "B")),
            locked: true,
        },
    )
    .unwrap_err();
    assert!(error.contains("invalid"), "{error}");
    assert_eq!(config_bytes(&fixture.replica), before);
}

#[test]
fn issue_1937_selection_state_requested_letter_survives_resolution_and_content_changes() {
    let fixture = selection_fixture();
    write_config(
        &fixture.replica,
        &seed_selection_config(r#"{"currentCodingAgent":"codex"}"#),
    );
    let expected = read_replica_selection_state(&fixture.replica)
        .expectation()
        .unwrap();
    // Settings only enable A..C, so resolving the stored D falls back; the
    // stored requested letter must stay D.
    write_replica_selection(
        &fixture.settings,
        &fixture.replica,
        &pair("codex", "D"),
        SelectionWriteIntent::Individual,
        &expected,
    )
    .unwrap();
    let resolution = resolve_profile(
        &fixture.settings,
        ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: Some(&fixture.replica),
            agent_matrix_name: None,
            requested_profile: None,
            requested_profile_authoritative: false,
        },
    );
    assert_eq!(resolution.requested_profile, "D");
    assert_eq!(resolution.effective_profile, "C");

    // Content/config changes do not rewrite the stored requested letter.
    let changed_settings = settings_with_cells(&[("codex", vec![])]);
    let resolution = resolve_profile(
        &changed_settings,
        ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: Some(&fixture.replica),
            agent_matrix_name: None,
            requested_profile: None,
            requested_profile_authoritative: false,
        },
    );
    assert_eq!(resolution.requested_profile, "D");
    assert_eq!(resolution.effective_profile, "A");
    assert_eq!(
        read_replica_selection_state(&fixture.replica).pair(),
        Some(&pair("codex", "D"))
    );
}
