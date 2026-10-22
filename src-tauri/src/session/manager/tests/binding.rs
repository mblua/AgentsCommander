use super::*;

fn birth_binding() -> CodingAgentBinding {
    CodingAgentBinding {
        id: Uuid::new_v4().to_string(),
        seed: Some(serde_json::json!({
            "identityRevision": format!("r1:0:{}", "a".repeat(64)),
            "originalAgentId": "original"
        })),
        origin: Some(serde_json::json!({
            "sessionsPath": "C:/sessions.json",
            "rawFileSHA256": "witness",
            "originalOrdinal": 3
        })),
    }
}

#[tokio::test]
async fn response_close_borrowed_binding_helpers_keep_admission_and_validation() {
    let manager = SessionManager::new();
    let (session, pending) = pending_fixture(&manager, false).await;
    let birth = birth_binding();
    let permit = acquire_carrier_mutation_permit().await;
    let wrong = PendingCreateBinding::new(pending.session_id(), Uuid::new_v4());
    assert_eq!(
        manager
            .install_pending_coding_agent_binding_with_carrier_permit(&permit, wrong, None)
            .await,
        Err("pending coding-agent binding capability mismatch".into())
    );
    manager
        .install_pending_coding_agent_binding_with_carrier_permit(&permit, pending, None)
        .await
        .unwrap();
    assert!(manager.coding_agent_binding(session.id).await.is_none());
    let mut malformed = birth.clone();
    malformed.id = "not-a-uuid".into();
    assert_eq!(
        manager
            .install_pending_coding_agent_binding_with_carrier_permit(
                &permit,
                pending,
                Some(malformed),
            )
            .await,
        Err("invalid_coding_agent_binding".into())
    );
    let mut malformed = birth.clone();
    malformed.seed = Some(serde_json::json!({"identityRevision":"", "originalAgentId":"original"}));
    assert_eq!(
        manager
            .install_pending_coding_agent_binding_with_carrier_permit(
                &permit,
                pending,
                Some(malformed),
            )
            .await,
        Err("invalid_coding_agent_binding_seed".into())
    );
    manager
        .install_pending_coding_agent_binding_with_carrier_permit(
            &permit,
            pending,
            Some(birth.clone()),
        )
        .await
        .unwrap();
    let mut conflict = birth.clone();
    conflict.seed.as_mut().unwrap()["originalAgentId"] = serde_json::json!("different");
    assert_eq!(
        manager
            .install_pending_coding_agent_binding_with_carrier_permit(
                &permit,
                pending,
                Some(conflict),
            )
            .await,
        Err("coding_agent_binding_seed_conflict".into())
    );
    assert_eq!(
        manager.coding_agent_binding(session.id).await.unwrap().seed,
        birth.seed
    );
    manager
        .rollback_pending_create_with_carrier_permit(&permit, pending)
        .await
        .unwrap();
}

#[tokio::test]
async fn response_close_borrowed_failed_recipes_keep_only_enrolled_rows_and_original_seed() {
    let manager = SessionManager::new();
    let birth = birth_binding();
    let foreign = birth_binding();
    let original_seed = birth.seed.clone().unwrap();
    {
        let mut state = manager.state.write().await;
        state
            .binding_seeds
            .insert(birth.id.clone(), original_seed.clone());
        state
            .failed_binding_recipes
            .insert(birth.id.clone(), serde_json::json!({"old":true}));
    }
    let permit = acquire_carrier_mutation_permit().await;
    let mut updated = birth.clone();
    updated.seed.as_mut().unwrap()["originalAgentId"] = serde_json::json!("new-seed");
    updated.origin = Some(serde_json::json!({"updated":true}));
    let recipe = serde_json::json!({"retained":true});
    manager
        .refresh_failed_binding_recipes_with_carrier_permit(
            &permit,
            vec![
                (updated.clone(), recipe.clone()),
                (foreign.clone(), recipe.clone()),
            ],
        )
        .await;
    let snapshot = manager.aggregate_snapshot().await;
    assert_eq!(snapshot.failed_binding_recipes.len(), 1);
    assert_eq!(snapshot.failed_binding_recipes[&birth.id], recipe);
    assert_eq!(snapshot.binding_seeds[&birth.id], original_seed);
    assert_eq!(snapshot.binding_origins[&birth.id], updated.origin.unwrap());
    assert_eq!(snapshot.binding_seeds[&foreign.id], foreign.seed.unwrap());
    assert!(snapshot.sessions.is_empty());
}

#[tokio::test]
async fn derived_agent_id_binding_pending_rollback_retires_only_its_binding() {
    let manager = SessionManager::new();
    let (first, pending) = pending_fixture(&manager, false).await;
    let (other, other_pending) = pending_fixture(&manager, false).await;
    let birth = birth_binding();
    let other_birth = birth_binding();
    manager
        .install_pending_coding_agent_binding(pending, Some(birth.clone()))
        .await
        .unwrap();
    manager
        .install_pending_coding_agent_binding(other_pending, Some(other_birth.clone()))
        .await
        .unwrap();
    assert!(manager.aggregate_snapshot().await.sessions.is_empty());
    manager.rollback_pending_create(pending).await.unwrap();
    assert!(manager.coding_agent_binding(first.id).await.is_none());
    assert_eq!(
        manager.coding_agent_binding(other.id).await.unwrap().id,
        other_birth.id
    );
    assert!(!manager
        .aggregate_snapshot()
        .await
        .binding_seeds
        .contains_key(&birth.id));
    manager
        .rollback_pending_create(other_pending)
        .await
        .unwrap();
}

#[tokio::test]
async fn derived_agent_id_binding_failed_affected_survives_live_only_snapshot_until_success() {
    let manager = SessionManager::new();
    let birth = birth_binding();
    let failed = serde_json::json!({
        "name":"affected failure", "shell":"shell", "workingDirectory":"C:/failed", "shellArgs":[],
        "agentId":"original", "codingAgentBindingId":birth.id, "codingAgentBindingSeed":birth.seed,
    });
    // Fixture represents enrollment by a receipt consumer (D09). D07 does
    // not activate receipts or extend general failed-legacy retention.
    manager
        .state
        .write()
        .await
        .failed_binding_recipes
        .insert(birth.id.clone(), serde_json::to_value(&failed).unwrap());
    manager
        .refresh_failed_binding_recipes(vec![
            (birth.clone(), failed.clone()),
            (birth_binding(), failed.clone()),
        ])
        .await;
    for _ in 0..2 {
        let snapshot = manager.aggregate_snapshot().await;
        assert_eq!(snapshot.failed_binding_recipes.len(), 1);
        assert_eq!(snapshot.failed_binding_recipes[&birth.id], failed);
        assert!(snapshot.sessions.is_empty());
        assert!(manager.list_sessions().await.is_empty());
    }
    let (restored, pending) = pending_fixture(&manager, false).await;
    manager
        .install_pending_coding_agent_binding(pending, Some(birth))
        .await
        .unwrap();
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_dormant(pending, dormant_witness(restored.id), 7);
    manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Keep,
            SelectionCause::Restore,
            mutations,
        )
        .await
        .unwrap();
    assert!(manager
        .aggregate_snapshot()
        .await
        .failed_binding_recipes
        .is_empty());
}

#[tokio::test]
async fn derived_agent_id_binding_ram_birth_aggregate_preserves_original_when_metadata_changes() {
    let manager = SessionManager::new();
    let (session, pending) = pending_fixture(&manager, false).await;
    let birth = birth_binding();
    manager
        .install_pending_coding_agent_binding(pending, Some(birth.clone()))
        .await
        .unwrap();
    let live = live_witness(session.id);
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(pending, live);
    manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Live(live),
            SelectionCause::SessionCreated(TrustedCreateIntent::User),
            mutations,
        )
        .await
        .unwrap();
    // Before a successful disk write, birth metadata remains owned by RAM.
    // A later disk configuration cannot become this binding's birth seed.
    manager
        .state
        .write()
        .await
        .sessions
        .get_mut(&session.id)
        .unwrap()
        .agent_id = Some("reused-id-after-external-rekey".into());
    let snapshot = manager.aggregate_snapshot().await;
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.coding_agent_bindings[&session.id], birth.id);
    assert_eq!(
        snapshot.binding_seeds[&birth.id]["originalAgentId"],
        "original"
    );
    assert_eq!(
        snapshot.binding_origins.get(&birth.id),
        birth.origin.as_ref()
    );
}

#[test]
fn terminal_snapshot_fact_debug_uses_only_structural_fields() {
    const NAME_CANARY: &str = "NAME_1173_S2K9";
    const PATH_CANARY: &str = r"C:\PATH_1173_S2K9\replica";
    let id = Uuid::parse_str("11730000-0000-4000-8000-00000000c229").unwrap();
    let requester = TerminalSnapshotRequesterFact {
        id,
        created_at: chrono::Utc::now(),
        working_directory: PATH_CANARY.to_string(),
        backend_kind: SessionBackendKind::ContainerTransport,
        is_coordinator: true,
        is_root_agent: false,
    };
    let session = TerminalSnapshotSessionFact {
        id,
        created_at: chrono::Utc::now(),
        name: NAME_CANARY.to_string(),
        status: SessionStatus::Running,
        working_directory: PATH_CANARY.to_string(),
        backend_kind: SessionBackendKind::LocalProcess,
    };
    let diagnostic = format!("{requester:?}\n{session:?}");
    let id_text = id.to_string();
    for forbidden in [NAME_CANARY, PATH_CANARY, id_text.as_str()] {
        assert!(!diagnostic.contains(forbidden));
    }
    for structural in [
        "working_directory_bytes",
        "name_bytes",
        "status: Running",
        "backend_kind: ContainerTransport",
    ] {
        assert!(diagnostic.contains(structural));
    }
}
