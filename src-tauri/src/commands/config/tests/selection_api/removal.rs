use super::*;

#[tokio::test]
async fn issue_1937_selection_api_removal_zero_protected_is_a_pure_no_op() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let second = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("C", "agent-0"),
    );
    let matrix = fixture.ac_root.join("_agent_a-rust");
    let settings = state_for(selection_api_settings(&fixture));
    let anchor_bytes = config_bytes(&anchor);
    let second_bytes = config_bytes(&second);
    let mut dir_before: Vec<std::ffi::OsString> = std::fs::read_dir(&anchor)
        .expect("read anchor dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    dir_before.sort();

    let preview =
        api_removal_preview(&settings, &anchor, super::ProfileAssignmentScope::Workgroup).await;
    assert!(preview.counts_complete);
    assert_eq!(preview.candidate_count, 2);
    assert_eq!(preview.protected_count, 0);
    assert_eq!(preview.already_unlocked_count, 2);
    assert_eq!(preview.invalid_count, 0);
    assert_eq!(preview.target_fingerprint.len(), 64);

    let result = api_removal_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        &preview.target_fingerprint,
    )
    .await
    .expect("zero-protected removal");
    assert_eq!(result.removed_count, 0);
    assert_eq!(result.already_unlocked_paths.len(), 2);
    assert!(result.failed_replica_paths.is_empty());
    assert_eq!(result.remaining_protected_count, Some(0));
    assert!(result.errors.is_empty());

    assert_eq!(config_bytes(&anchor), anchor_bytes);
    assert_eq!(config_bytes(&second), second_bytes);
    let mut dir_after: Vec<std::ffi::OsString> = std::fs::read_dir(&anchor)
        .expect("read anchor dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    dir_after.sort();
    assert_eq!(
        dir_after, dir_before,
        "zero-protected removal creates no sidecar or temp file"
    );
    assert!(!sidecar_path(&anchor).exists(), "no sidecar for a no-op");
    assert!(!sidecar_path(&second).exists(), "no sidecar for a no-op");
    assert!(
        !matrix.join("config.json").exists(),
        "removal never writes a Matrix default"
    );

    // The single-replica scope has the same zero-protected shape.
    let single =
        api_removal_preview(&settings, &anchor, super::ProfileAssignmentScope::Replica).await;
    assert_eq!(single.candidate_count, 1);
    assert_eq!(single.protected_count, 0);
    let single_result = api_removal_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Replica,
        &single.target_fingerprint,
    )
    .await
    .expect("single zero-protected removal");
    assert_eq!(single_result.removed_count, 0);
    assert_eq!(config_bytes(&anchor), anchor_bytes);
}

#[tokio::test]
async fn issue_1937_selection_api_removal_preserves_pair_and_unknown_tooling() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        json!({
            "profile": "A",
            "currentCodingAgent": "agent-0",
            "selectionLocked": true,
            "lastCodingAgent": "legacy-agent",
            "profileContentHash": "deadbeef",
        }),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview =
        api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
    assert_eq!(preview.protected_count, 1);

    let result = api_removal_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        &preview.target_fingerprint,
    )
    .await
    .expect("removal");
    assert_eq!(result.removed_count, 1);
    assert_eq!(result.remaining_protected_count, Some(0));
    assert!(result.errors.is_empty());
    let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
    assert_eq!(saved["tooling"]["selectionLocked"], json!(false));
    assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
    assert_eq!(saved["tooling"]["profile"], json!("A"));
    let state: Value = serde_json::from_slice(
        &std::fs::read(replica.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
            .expect("read state file"),
    )
    .expect("parse state");
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("legacy-agent"));
    assert_eq!(state["tooling"]["profileContentHash"], json!("deadbeef"));
    assert!(saved["tooling"].get("lastCodingAgent").is_none());
    assert!(saved["tooling"].get("profileContentHash").is_none());
}

#[tokio::test]
async fn issue_1937_selection_api_removal_mixed_skips_the_unlocked_target() {
    let fixture = selection_api_fixture();
    let first = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "c-rust",
        locked_tooling("C", "agent-0"),
    );
    let unlocked_fourth = selection_api_replica(
        &fixture,
        "room-1-team",
        "d-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let fourth_bytes = config_bytes(&unlocked_fourth);
    let preview =
        api_removal_preview(&settings, &first, super::ProfileAssignmentScope::Workgroup).await;
    assert_eq!(preview.candidate_count, 4);
    assert_eq!(preview.protected_count, 3);
    assert_eq!(preview.already_unlocked_count, 1);

    let result = api_removal_apply(
        &settings,
        &first,
        super::ProfileAssignmentScope::Workgroup,
        &preview.target_fingerprint,
    )
    .await
    .expect("mixed removal");
    assert_eq!(result.removed_count, 3);
    assert_eq!(result.already_unlocked_paths.len(), 1);
    assert!(result.already_unlocked_paths[0].contains("d-rust"));
    assert_eq!(
        config_bytes(&unlocked_fourth),
        fourth_bytes,
        "no writer for the fourth"
    );
    assert!(
        !sidecar_path(&unlocked_fourth).exists(),
        "the unlocked fourth target must not create a write sidecar"
    );
    assert!(
        sidecar_path(&first).exists(),
        "the protected targets did write"
    );
    assert_eq!(result.remaining_protected_count, Some(0));
    assert!(result.errors.is_empty());
}

#[tokio::test]
async fn issue_1937_selection_api_removal_stale_scope_rejects_without_effects() {
    let fixture = selection_api_fixture();
    let first = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    let second = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("B", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview =
        api_removal_preview(&settings, &first, super::ProfileAssignmentScope::Workgroup).await;
    // Another window unlocks the first replica after the preview.
    let mut value: Value = serde_json::from_slice(&config_bytes(&first)).expect("parse");
    value["tooling"]["selectionLocked"] = json!(false);
    std::fs::write(
        first.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite");
    let first_bytes = config_bytes(&first);
    let second_bytes = config_bytes(&second);

    let error = api_removal_apply(
        &settings,
        &first,
        super::ProfileAssignmentScope::Workgroup,
        &preview.target_fingerprint,
    )
    .await
    .expect_err("scope-level stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&first), first_bytes);
    assert_eq!(
        config_bytes(&second),
        second_bytes,
        "the second stays locked"
    );
}
