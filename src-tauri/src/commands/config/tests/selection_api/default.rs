use super::*;

#[tokio::test]
async fn issue_1937_selection_api_default_round_trip_and_atomic_publication() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let matrix = fixture.ac_root.join("_agent_dev-rust");
    std::fs::write(
        matrix.join("config.json"),
        serde_json::to_vec(&json!({
            "tooling": { "defaultProfile": "A", "unknownKey": 123 },
        }))
        .expect("serialize matrix config"),
    )
    .expect("seed matrix config");
    let settings = state_for(selection_api_settings(&fixture));
    let replica_bytes = config_bytes(&replica);

    let initial = api_default(&settings, &replica).await;
    assert!(initial.default.is_none());
    assert_eq!(initial.default_fingerprint.len(), 64);
    // The returned replica path is the canonical identity, not the raw
    // spelling: on Windows the temp path may carry an 8.3 short component
    // (`RUNNER~1`) while the validated path is the long canonical form.
    assert_eq!(initial.target_replica_path, canonical_display(&replica));

    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "agent-0", "B", true, "wrong"),
    )
    .await
    .expect_err("stale default fingerprint");
    assert!(error.contains("stalePreview"), "{error}");

    let (set, payload) = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "agent-0", "B", true, &initial.default_fingerprint),
    )
    .await
    .expect("set default");
    assert_eq!(
        set.default,
        Some(super::SelectionDefault {
            coding_agent_id: "agent-0".to_string(),
            requested_profile: "B".to_string(),
            selection_locked: true,
        })
    );
    assert_eq!(payload["scope"], json!("default"));
    assert_eq!(payload["operation"], json!("default"));
    assert_eq!(payload["agentPath"], json!(set.matrix_path));
    assert_eq!(payload["affectedPaths"], json!([]));
    assert_eq!(payload["default"]["selectionLocked"], json!(true));

    let disk: Value =
        serde_json::from_slice(&std::fs::read(matrix.join("config.json")).expect("read"))
            .expect("parse matrix config");
    assert_eq!(
        disk["tooling"]["replicaSelectionDefault"],
        json!({
            "codingAgentId": "agent-0",
            "requestedProfile": "B",
            "selectionLocked": true,
        })
    );
    assert_eq!(disk["tooling"]["defaultProfile"], json!("A"));
    assert_eq!(disk["tooling"]["unknownKey"], json!(123));
    assert_eq!(
        config_bytes(&replica),
        replica_bytes,
        "replicas are untouched"
    );

    let reread = api_default(&settings, &replica).await;
    assert_eq!(reread.default, set.default);
    assert_eq!(reread.default_fingerprint, set.default_fingerprint);

    // A second window that still holds the pre-set fingerprint is stale.
    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(
            &replica,
            "agent-0",
            "C",
            false,
            &initial.default_fingerprint,
        ),
    )
    .await
    .expect_err("the old window must not overwrite the new default");
    assert!(error.contains("stalePreview"), "{error}");
    let unchanged: Value =
        serde_json::from_slice(&std::fs::read(matrix.join("config.json")).expect("read"))
            .expect("parse");
    assert_eq!(
        unchanged["tooling"]["replicaSelectionDefault"]["requestedProfile"],
        json!("B")
    );
}

#[tokio::test]
async fn issue_1937_selection_api_default_rejects_malformed_and_invalid_input() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let matrix = fixture.ac_root.join("_agent_dev-rust");
    let settings = state_for(selection_api_settings(&fixture));

    // A malformed stored default is an error, never a silent absence.
    std::fs::write(
        matrix.join("config.json"),
        serde_json::to_vec(&json!({
            "tooling": { "replicaSelectionDefault": { "codingAgentId": "agent-0" } },
        }))
        .expect("serialize malformed config"),
    )
    .expect("write malformed config");
    let error =
        super::get_replica_selection_default_inner(&settings, api_default_request(&replica))
            .await
            .expect_err("malformed default must fail the read");
    let error = error.to_string();
    assert!(error.contains("replicaSelectionDefault"), "{error}");

    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "agent-0", "B", true, "whatever"),
    )
    .await
    .expect_err("malformed prior default must fail the CAS read");
    assert!(error.contains("replicaSelectionDefault"), "{error}");

    std::fs::remove_file(matrix.join("config.json")).expect("remove malformed config");
    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "agent-0", "BB", true, "whatever"),
    )
    .await
    .expect_err("the profile must be a single letter");
    assert!(error.contains("single letter A through Z"), "{error}");

    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "missing-agent", "B", true, "whatever"),
    )
    .await
    .expect_err("the coding agent must be configured");
    assert!(error.contains("not configured"), "{error}");
}
