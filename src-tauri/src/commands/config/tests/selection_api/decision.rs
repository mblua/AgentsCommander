use super::*;

#[tokio::test]
async fn issue_1937_selection_api_rejects_decision_outside_bulk_assign_and_lock() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let bytes_before = config_bytes(&anchor);

    let ordinary = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        Some(super::ConflictDecision::ForceReviewed),
        Some("whatever"),
    )
    .await
    .expect_err("ordinary plus decision must be rejected");
    assert!(
        ordinary.contains("only valid with assignmentMode assignAndLock"),
        "{ordinary}"
    );

    let single = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::ForceReviewed),
        Some("whatever"),
    )
    .await
    .expect_err("replica plus decision must be rejected");
    assert!(single.contains("not valid for replica scope"), "{single}");

    assert_eq!(
        config_bytes(&anchor),
        bytes_before,
        "no writes on rejection"
    );
    assert!(!sidecar_path(&anchor).exists(), "no sidecar on rejection");
}

#[tokio::test]
async fn issue_1937_selection_api_bulk_assign_and_lock_requires_decision() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    let second = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let bytes_before = config_bytes(&anchor);
    let second_bytes = config_bytes(&second);
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
    )
    .await;

    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect_err("conflicts need a decision");
    assert!(error.contains("conflictDecisionRequired"), "{error}");
    // A force decision without the current preview fingerprint is rejected.
    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::ForceReviewed),
        None,
    )
    .await
    .expect_err("a force decision still needs the reviewed fingerprint");
    // #2051 - a missing fingerprint is reported as missing, never stale.
    assert!(error.contains("confirmationMissing"), "{error}");
    assert!(!error.contains("Target selection changed"), "{error}");
    assert_eq!(config_bytes(&anchor), bytes_before);
    assert_eq!(config_bytes(&second), second_bytes);
    assert!(!sidecar_path(&anchor).exists());
    assert!(!sidecar_path(&second).exists());
}

#[tokio::test]
async fn issue_1937_selection_api_decision_snapshots_are_per_decision() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
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
    let decisions = preview.decisions.as_ref().expect("decisions");

    // Force with the unlockedOnly hash is a stale approval: no force applied.
    let stale = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::ForceReviewed),
        Some(&decisions.unlocked_only.fingerprint),
    )
    .await
    .expect_err("force with the wrong decision hash");
    assert!(stale.contains("stalePreview"), "{stale}");

    // UnlockedOnly with its own hash skips every protected replica.
    let unlocked_only = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::UnlockedOnly),
        Some(&decisions.unlocked_only.fingerprint),
    )
    .await
    .expect("unlockedOnly apply");
    assert_eq!(unlocked_only.updated_count, 0);
    assert_eq!(unlocked_only.skipped_locked_paths.len(), 2);
    assert!(!unlocked_only.force_applied);
    assert_eq!(unlocked_only.locked_after_apply_count, Some(2));

    // ForceReviewed with its own hash overwrites the protected targets.
    let forced = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::ForceReviewed),
        Some(&decisions.force_reviewed.fingerprint),
    )
    .await
    .expect("forceReviewed apply");
    assert_eq!(forced.updated_count, 2);
    assert!(forced.skipped_locked_paths.is_empty());
    assert!(
        forced.force_applied,
        "a protected target was actually overwritten"
    );
    assert_eq!(forced.locked_after_apply_count, Some(2));
    assert!(
        forced.newly_protected_paths.is_empty(),
        "already locked stays locked"
    );
}

#[tokio::test]
async fn issue_1937_selection_api_stale_approvals_are_rejected_without_writes() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));

    // (a) the stored pair changes after the preview.
    let pair_preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    let mut value: Value = serde_json::from_slice(&config_bytes(&anchor)).expect("parse config");
    value["tooling"]["profile"] = json!("C");
    std::fs::write(
        anchor.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite config");
    let changed_bytes = config_bytes(&anchor);
    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&pair_preview.target_fingerprint),
    )
    .await
    .expect_err("pair change must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&anchor), changed_bytes);

    // (b) the flag changes after the preview.
    let flag_preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    let mut value: Value = serde_json::from_slice(&config_bytes(&anchor)).expect("parse config");
    value["tooling"]["selectionLocked"] = json!(true);
    value["tooling"]["currentCodingAgent"] = json!("agent-0");
    std::fs::write(
        anchor.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite config");
    let changed_bytes = config_bytes(&anchor);
    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&flag_preview.target_fingerprint),
    )
    .await
    .expect_err("flag change must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&anchor), changed_bytes);

    // (c) membership grows after the preview.
    let membership_preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    let added = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let added_bytes = config_bytes(&added);
    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&membership_preview.target_fingerprint),
    )
    .await
    .expect_err("membership change must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&added), added_bytes);

    // (d) a relevant live session appears after the preview.
    let manager = SessionManager::new();
    let live = selection_api_replica(
        &fixture,
        "room-1-team",
        "c-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let session_mgr = Arc::new(RwLock::new(manager));
    let session_preview = super::preview_coding_agent_profile_selection_inner(
        &session_mgr,
        &settings,
        api_preview_request(
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        ),
    )
    .await
    .expect("preview with sessions");
    {
        let manager = session_mgr.write().await;
        manager
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
            .expect("create session");
    }
    let live_bytes = config_bytes(&live);
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        Arc::new(SelectionApiPtyBackend),
    )));
    let error = super::apply_coding_agent_profile_selection_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        &settings,
        api_apply_request(
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&session_preview.target_fingerprint),
        ),
    )
    .await
    .expect_err("session change must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&live), live_bytes);
}

#[tokio::test]
async fn issue_1937_selection_api_skipped_locked_never_restarts_live_session() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "codex".to_string(),
            Vec::new(),
            anchor.to_string_lossy().to_string(),
            Some("agent-0".to_string()),
            Some("Codex".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session");
    let session_mgr = Arc::new(RwLock::new(manager));
    let settings = state_for(selection_api_settings(&fixture));
    let bytes_before = config_bytes(&anchor);
    let mut preview_request = api_preview_request(
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    );
    preview_request.restart_sessions = true;
    let preview = super::preview_coding_agent_profile_selection_inner(
        &session_mgr,
        &settings,
        preview_request,
    )
    .await
    .expect("preview");
    assert_eq!(
        preview.live_session_count, 0,
        "the actionable count skips the protected replica's session"
    );
    assert_eq!(preview.target_count, 0, "the only replica is protected");
    assert_eq!(preview.targets.len(), 1);
    assert_eq!(
        preview.targets[0].live_session_ids,
        vec![session.id.to_string()],
        "the protected target still reports its live session"
    );

    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        Arc::new(SelectionApiPtyBackend),
    )));
    let mut request = api_apply_request(
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&preview.target_fingerprint),
    );
    request.restart_sessions = true;
    let result = super::apply_coding_agent_profile_selection_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        &settings,
        request,
    )
    .await
    .expect("skipping a locked replica is not an error")
    .0;

    assert_eq!(result.updated_count, 0);
    assert_eq!(
        result.restarted_count, 0,
        "no lifecycle invocation for a skip"
    );
    assert_eq!(result.skipped_locked_paths.len(), 1);
    assert!(result.errors.is_empty());
    assert_eq!(config_bytes(&anchor), bytes_before);
    assert!(
        session_mgr
            .read()
            .await
            .get_session(session.id)
            .await
            .is_some(),
        "the skipped replica's session must survive untouched"
    );
}

#[tokio::test]
async fn issue_1937_selection_api_later_target_race_yields_partial_results() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let later = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    let later_bytes = config_bytes(&later);

    // Hold the later target's sidecar lock to force a real per-file write
    // failure after the first target has already committed.
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sidecar_path(&later))
        .expect("open sidecar");
    lock_file.lock().expect("hold later target lock");
    let result = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect("earlier writes stay committed");
    lock_file.unlock().expect("release sidecar");
    drop(lock_file);

    assert_eq!(result.updated_count, 1, "the first target still committed");
    assert!(result.updated_replica_paths[0].contains("a-rust"));
    assert_eq!(
        config_bytes(&later),
        later_bytes,
        "the failed target is untouched"
    );
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].code, "configLockTimeout");
    assert!(result.errors[0].replica_paths[0].contains("b-rust"));
    assert_eq!(result.locked_after_apply_count, Some(0));
}

#[tokio::test]
async fn issue_1937_selection_api_invalid_state_is_visible_and_never_written() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        json!({"selectionLocked": true}),
    );
    let unlocked = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let invalid_bytes = config_bytes(&anchor);
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    assert_eq!(preview.invalid_count, 1);
    assert_eq!(preview.protected_count, 0);
    assert_eq!(
        preview.target_count, 1,
        "only the valid replica is actionable"
    );
    let invalid = preview
        .targets
        .iter()
        .find(|target| target.replica_path.contains("a-rust"))
        .expect("invalid target listed");
    assert_eq!(invalid.selection_state, super::SelectionState::Invalid);
    assert!(invalid.saved_pair.is_none());
    assert!(invalid.selection_error.is_some());

    let result = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect("valid targets still apply");
    assert_eq!(result.updated_count, 1);
    assert_eq!(result.invalid_paths.len(), 1);
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].code, "invalidSelectionState");
    assert_eq!(config_bytes(&anchor), invalid_bytes);
    assert_eq!(
        result.locked_after_apply_count, None,
        "invalid state is unknowable"
    );
    let saved: Value = serde_json::from_slice(&config_bytes(&unlocked)).expect("parse");
    assert_eq!(saved["tooling"]["profile"], json!("B"));
}

#[tokio::test]
async fn issue_1937_selection_api_anchor_must_be_a_configured_replica() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let matrix = fixture.ac_root.join("_agent_dev-rust");
    // Origin Matrix is not a lockable replica target.
    let mut unconfigured = settings_with_single_agent();
    unconfigured.project_paths = Vec::new();
    let unconfigured = state_for(unconfigured);
    let error = api_preview_result(
        &unconfigured,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
    )
    .await
    .expect_err("an unconfigured project root must be rejected");
    assert!(
        error.contains("outside configured AC project roots"),
        "{error}"
    );

    let settings = state_for(selection_api_settings(&fixture));
    let error = api_preview_result(
        &settings,
        &matrix,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
    )
    .await
    .expect_err("the origin Matrix must be rejected");
    assert!(error.contains("not a Room replica"), "{error}");

    // A `__agent_*` directly under `.ac` has no room parent.
    let roomless = fixture.ac_root.join("__agent_dev-rust");
    std::fs::create_dir_all(&roomless).expect("create roomless replica dir");
    let error = api_preview_result(
        &settings,
        &roomless,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
    )
    .await
    .expect_err("a roomless replica must be rejected");
    assert!(
        error.contains("room-*") || error.contains("Room"),
        "{error}"
    );
}
