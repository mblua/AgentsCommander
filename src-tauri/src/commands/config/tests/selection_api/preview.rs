use super::*;

#[tokio::test]
async fn issue_1937_selection_api_preview_replica_reports_saved_pair_and_state() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
    )
    .await;

    assert_eq!(preview.scope, super::ProfileAssignmentScope::Replica);
    assert!(preview.counts_complete);
    assert_eq!(preview.candidate_count, 1);
    assert_eq!(preview.protected_count, 1);
    assert_eq!(preview.invalid_count, 0);
    assert_eq!(preview.conflict_count, 0);
    assert!(preview.decisions.is_none());
    assert_eq!(
        preview.target_count, 1,
        "a deliberate replica target counts"
    );
    assert_eq!(preview.targets.len(), 1);
    let target = &preview.targets[0];
    assert_eq!(target.selection_state, super::SelectionState::Locked);
    assert_eq!(
        target.saved_pair,
        Some(super::SavedPair {
            coding_agent_id: Some("agent-0".to_string()),
            requested_profile: Some("B".to_string()),
        })
    );
    assert!(target.selection_error.is_none());
    assert_eq!(preview.target_fingerprint.len(), 64);
    assert_eq!(
        preview.target_fingerprint,
        preview.target_fingerprint.to_lowercase()
    );
}

#[tokio::test]
async fn issue_1937_selection_api_scopes_enumerate_canonical_matrices_only() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-2-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    // Homonymous Matrix in another configured project: same directory name,
    // different canonical origin Matrix, so Kind must exclude it.
    let other_project = fixture._temp.path().join("other-project");
    let other_fixture = SelectionApiFixture {
        _temp: tempfile::tempdir().expect("second tempdir"),
        project: other_project,
        ac_root: PathBuf::new(),
    };
    let other_ac = other_fixture.project.join(".ac");
    selection_api_replica_at(
        &other_fixture,
        &other_ac.join("_agent_dev-rust"),
        &other_ac.join("room-1-team").join("__agent_dev-rust"),
        "dev-rust",
        locked_tooling("C", "agent-0"),
    );
    let mut settings = selection_api_settings(&fixture);
    settings
        .project_paths
        .push(other_fixture.project.to_string_lossy().to_string());
    let settings = state_for(settings);

    let room = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    assert!(room.counts_complete);
    assert_eq!(room.candidate_count, 1, "one Workgroup means one room");

    let kind = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Kind,
        super::AssignmentMode::Ordinary,
    )
    .await;
    assert!(kind.counts_complete);
    assert_eq!(
        kind.candidate_count, 2,
        "the canonical Matrix, not the name"
    );
    assert_eq!(kind.protected_count, 1);
    assert_eq!(kind.invalid_count, 0);
    let paths: Vec<&str> = kind
        .targets
        .iter()
        .map(|target| target.replica_path.as_str())
        .collect();
    assert!(paths.iter().any(|path| path.contains("room-1-team")));
    assert!(paths.iter().any(|path| path.contains("room-2-team")));
    assert!(
        paths.iter().all(|path| path.contains("project")),
        "the homonymous other-project replica must stay excluded: {paths:?}"
    );
}

#[tokio::test]
async fn issue_1937_selection_api_preview_counts_offline_and_live_replicas() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let live = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "codex".to_string(),
            Vec::new(),
            live.to_string_lossy().to_string(),
            Some("agent-0".to_string()),
            Some("Codex".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create live session");
    let settings = state_for(selection_api_settings(&fixture));
    let preview = super::preview_coding_agent_profile_selection_inner(
        &Arc::new(RwLock::new(manager)),
        &settings,
        api_preview_request(
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        ),
    )
    .await
    .expect("preview");

    assert_eq!(preview.candidate_count, 2, "offline replicas stay listed");
    assert_eq!(preview.live_session_count, 1);
    let live_target = preview
        .targets
        .iter()
        .find(|target| target.replica_path.contains("b-rust"))
        .expect("live target");
    assert_eq!(live_target.live_session_ids, vec![session.id.to_string()]);
    let offline_target = preview
        .targets
        .iter()
        .find(|target| target.replica_path.contains("a-rust"))
        .expect("offline target");
    assert!(offline_target.live_session_ids.is_empty());
}

#[tokio::test]
async fn issue_1937_selection_api_all_protected_bulk_offers_both_decisions() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("B", "agent-0"),
    );
    // Equal pair as well: a valid lock is still a conflict.
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
        locked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
    )
    .await;

    assert_eq!(preview.candidate_count, 3);
    assert_eq!(preview.protected_count, 3);
    assert_eq!(preview.conflict_count, 3, "valid protected candidates only");
    assert_eq!(preview.target_count, 0, "top-level uses unlockedOnly");
    let decisions = preview.decisions.as_ref().expect("both decisions");
    assert_eq!(decisions.unlocked_only.eligible_count, 0);
    assert_eq!(decisions.unlocked_only.skipped_locked_count, 3);
    assert_eq!(decisions.force_reviewed.eligible_count, 3);
    assert_eq!(decisions.force_reviewed.skipped_locked_count, 0);
    assert_ne!(
        decisions.unlocked_only.fingerprint,
        decisions.force_reviewed.fingerprint
    );
    assert_eq!(
        preview.target_fingerprint,
        decisions.unlocked_only.fingerprint
    );
}

#[tokio::test]
async fn issue_1937_selection_api_absent_assignment_mode_is_ordinary_legacy() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let request: super::PreviewCodingAgentProfileSelectionRequest = serde_json::from_value(json!({
        "targetReplicaPath": replica.to_string_lossy(),
        "codingAgentId": "agent-0",
        "profile": "B",
        "scope": "replica",
        "restartSessions": false,
    }))
    .expect("deserialize legacy preview request");
    assert_eq!(request.assignment_mode, super::AssignmentMode::Ordinary);

    let settings = state_for(selection_api_settings(&fixture));
    let preview = super::preview_coding_agent_profile_selection_inner(
        &empty_session_manager(),
        &settings,
        request,
    )
    .await
    .expect("legacy preview");
    assert!(!preview.requires_explicit_confirmation);
    let result = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
        None,
        None,
    )
    .await
    .expect("legacy replica apply without fingerprint");
    assert_eq!(result.updated_count, 1);
    assert_eq!(result.newly_protected_paths.len(), 0);
}

#[tokio::test]
async fn issue_1937_selection_api_apply_ordinary_bulk_skips_locked() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    let unlocked = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let locked_bytes = config_bytes(&anchor);
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    assert_eq!(
        preview.target_count, 1,
        "actionable count skips the locked one"
    );
    assert!(preview.decisions.is_none());

    let result = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect("ordinary bulk apply");

    assert_eq!(result.updated_count, 1);
    assert_eq!(result.newly_protected_paths.len(), 0);
    assert_eq!(result.skipped_locked_paths.len(), 1);
    assert!(result.skipped_locked_paths[0].contains("a-rust"));
    assert!(result.errors.is_empty());
    assert_eq!(result.locked_after_apply_count, Some(1));
    assert!(!result.force_applied);
    assert_eq!(
        config_bytes(&anchor),
        locked_bytes,
        "locked bytes untouched"
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&config_bytes(&unlocked)).expect("parse unlocked config");
    assert_eq!(saved["tooling"]["profile"], json!("B"));
    assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
    assert!(saved["tooling"].get("selectionLocked").is_none());
}

#[tokio::test]
async fn issue_1937_selection_api_replica_assign_and_lock_is_atomic() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
    )
    .await;
    assert!(preview.requires_explicit_confirmation);
    assert!(
        preview.decisions.is_none(),
        "replica needs no conflict dialog"
    );

    let result = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect("replica assign and lock");

    assert_eq!(result.updated_count, 1);
    assert_eq!(
        result.newly_protected_paths,
        vec![result.updated_replica_paths[0].clone()]
    );
    assert!(!result.force_applied);
    let saved: serde_json::Value =
        serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
    assert_eq!(saved["tooling"]["selectionLocked"], json!(true));
    assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
    assert_eq!(saved["tooling"]["profile"], json!("B"));
}

#[tokio::test]
async fn issue_2051_selection_api_replica_lock_changed_target_is_stale_without_writes() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));

    // 1. approve the replica + lock pair on this state.
    let preview = api_preview(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
    )
    .await;

    // 2. the stored pair changes after the preview.
    let mut value: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
    value["tooling"]["profile"] = json!("C");
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite config");
    let changed_bytes = config_bytes(&replica);

    // 3. the stale approval is rejected before any write.
    let error = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect_err("a changed target must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&replica), changed_bytes, "no write on stale");
    assert!(!sidecar_path(&replica).exists(), "no sidecar on stale");
    let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
    assert_ne!(
        saved["tooling"]["selectionLocked"],
        json!(true),
        "no lock on stale"
    );

    // 4. control: a fresh preview over the changed target still applies, so
    // step 3 rejected because of the change, not because lock is broken.
    let fresh = api_preview(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
    )
    .await;
    let result = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        None,
        Some(&fresh.target_fingerprint),
    )
    .await
    .expect("a fresh preview applies");
    assert_eq!(result.updated_count, 1);
    assert_eq!(
        result.newly_protected_paths,
        vec![result.updated_replica_paths[0].clone()]
    );
}

#[tokio::test]
async fn issue_2051_selection_api_replica_lock_missing_fingerprint_is_not_stale() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let before = config_bytes(&replica);

    let error = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        None,
        None,
    )
    .await
    .expect_err("a lock apply without a fingerprint must be refused");

    // #2051 - nothing was sent is a client contract failure, never a
    // target change: the message must not blame a stale preview.
    assert!(error.contains("confirmationMissing"), "{error}");
    assert!(!error.contains("Target selection changed"), "{error}");
    assert_eq!(
        config_bytes(&replica),
        before,
        "no write on a missing fingerprint"
    );
    assert!(!sidecar_path(&replica).exists(), "no sidecar on rejection");
    let saved: serde_json::Value =
        serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
    assert_ne!(
        saved["tooling"]["selectionLocked"],
        json!(true),
        "no lock written"
    );
}

#[tokio::test]
async fn issue_2010_selection_api_replica_locked_same_pair_keeps_lock() {
    for mode in [
        super::AssignmentMode::Ordinary,
        super::AssignmentMode::AssignAndLock,
    ] {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            locked_tooling("B", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let before = config_bytes(&replica);
        let preview = api_preview(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            mode,
        )
        .await;

        let result = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            mode,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect("locked same-pair replica apply");

        assert!(result.errors.is_empty(), "{mode:?}: {:?}", result.errors);
        assert_eq!(result.updated_count, 0, "{mode:?}");
        assert!(result.updated_replica_paths.is_empty(), "{mode:?}");
        assert!(result.newly_protected_paths.is_empty(), "{mode:?}");
        assert!(result.skipped_locked_paths.is_empty(), "{mode:?}");
        assert_eq!(result.restarted_count, 0, "{mode:?}");
        assert!(!result.force_applied, "{mode:?}");
        assert_eq!(config_bytes(&replica), before, "{mode:?}");
        let saved: serde_json::Value =
            serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
        assert_eq!(saved["tooling"]["selectionLocked"], json!(true), "{mode:?}");
    }
}

#[tokio::test]
async fn issue_2010_selection_api_replica_locked_different_pair_still_errors() {
    for mode in [
        super::AssignmentMode::Ordinary,
        super::AssignmentMode::AssignAndLock,
    ] {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            locked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let before = config_bytes(&replica);
        let preview = api_preview(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            mode,
        )
        .await;

        let result = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            mode,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect("apply reaches the writer");

        assert_eq!(result.errors.len(), 1, "{mode:?}: {:?}", result.errors);
        assert!(
            result.errors[0].message.contains("locked"),
            "{mode:?}: {:?}",
            result.errors
        );
        assert_eq!(result.updated_count, 0, "{mode:?}");
        assert_eq!(config_bytes(&replica), before, "{mode:?}");
    }
}
