use super::*;

const C3_CLEANUP_PREFIX: &str =
    "Config pair r/config.json and r/config.state.no-git.json cleanup failed: ";

/// #2816 (C3) E10 - the cleanup-wrapped TRACKED text keeps its wire code.
#[test]
fn issue_2816_selection_error_code_keeps_the_tracked_tooling_code() {
    assert_eq!(
        super::selection_error_code(&format!(
            "{C3_CLEANUP_PREFIX}'tooling' must be a JSON object in the tracked config file"
        )),
        "invalidSelectionState"
    );
}

/// #2816 (C3) E10 - the arm is not widened: the state-side text and an
/// unrelated write failure stay `configWriteFailed`.
#[test]
fn issue_2816_selection_error_code_does_not_reclassify_the_state_side() {
    assert_eq!(
        super::selection_error_code(&format!(
            "{C3_CLEANUP_PREFIX}'tooling' must be a JSON object in the state file"
        )),
        "configWriteFailed"
    );
    assert_eq!(
        super::selection_error_code("Failed to write temp config r/.config.json.1.tmp: denied"),
        "configWriteFailed"
    );
}

/// #2816 (C3) E10 - the write path: a tracked `tooling` that turns
/// malformed after the enumeration still classifies as before.
#[tokio::test]
async fn issue_2816_write_path_malformed_tracked_tooling_keeps_its_code() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let settings_snapshot = settings.read().await.clone();
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings_snapshot,
        &replica,
        &super::ProfileAssignmentScope::Replica,
        &[],
    )
    .expect("enumerate");
    let target = &enumeration.candidates[0].target;
    let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
        configuration_ref: None,
        profile_override_source: None,
        identity: target.identity_path.clone(),
        pair: super::target_pair(target),
        locked: false,
    };
    // The tracked `tooling` turns malformed after the enumeration. No state
    // file exists, so the cleanup's tracked check is the one measured.
    let mut value: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
    value["tooling"] = json!(5);
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite");
    let changed_bytes = config_bytes(&replica);
    assert!(!replica
        .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
        .exists());

    let error = crate::config::coding_agent_profiles::write_replica_selection(
        &settings_snapshot,
        &replica,
        &crate::config::coding_agent_profiles::ReplicaSelectionPair {
            coding_agent_id: "agent-0".to_string(),
            requested_profile: "B".to_string(),
        },
        crate::config::coding_agent_profiles::SelectionWriteIntent::BulkOrdinary,
        &expected,
    )
    .expect_err("a malformed tracked tooling must fail");
    assert_eq!(
        super::selection_error_code(&error),
        "invalidSelectionState",
        "{error}"
    );
    assert_eq!(config_bytes(&replica), changed_bytes);
}

#[tokio::test]
async fn issue_1937_selection_api_per_file_cas_rejects_changed_target() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let settings_snapshot = settings.read().await.clone();
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings_snapshot,
        &replica,
        &super::ProfileAssignmentScope::Replica,
        &[],
    )
    .expect("enumerate");
    let target = &enumeration.candidates[0].target;
    let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
        configuration_ref: None,
        profile_override_source: None,
        identity: target.identity_path.clone(),
        pair: super::target_pair(target),
        locked: false,
    };
    // The stored pair changes after the enumeration but before the write.
    let mut value: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
    value["tooling"]["profile"] = json!("C");
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite");
    let changed_bytes = config_bytes(&replica);

    let error = crate::config::coding_agent_profiles::write_replica_selection(
        &settings_snapshot,
        &replica,
        &crate::config::coding_agent_profiles::ReplicaSelectionPair {
            coding_agent_id: "agent-0".to_string(),
            requested_profile: "B".to_string(),
        },
        crate::config::coding_agent_profiles::SelectionWriteIntent::BulkOrdinary,
        &expected,
    )
    .expect_err("stale expectation must fail its CAS");
    assert!(error.starts_with("stale replica selection"), "{error}");
    assert_eq!(
        config_bytes(&replica),
        changed_bytes,
        "failed CAS writes nothing"
    );
}
