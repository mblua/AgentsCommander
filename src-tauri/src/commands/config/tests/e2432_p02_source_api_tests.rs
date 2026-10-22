use super::*;
use crate::config::settings::{
    configuration_identity, pair_identity, AgentFileEdits, AgentFileSourceMenu, SettingsOwner,
    SourceKind,
};
use serde_json::{json, Value};

fn fixture() -> (tempfile::TempDir, SettingsState) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let settings = AppSettings {
        root_token: Some("P02-TEST-SECRET".into()),
        ..AppSettings::default()
    };
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let source = json!({"agents":[{"id":"same","label":"Registered","color":"red","baseCommand":"tool"}],
        "codingAgentProfiles":{"profilesByAgent":{"same":{"B":{"additionalArguments":"--registered"}}}}});
    std::fs::write(
        temp.path()
            .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
        serde_json::to_vec(&source).unwrap(),
    )
    .unwrap();
    let owner = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    (temp, owner)
}

#[tokio::test]
async fn e2432_source_fingerprints_bind_ref_marker_source_and_context() {
    let (temp, owner) = fixture();
    let source = list_agent_file_sources_inner(&owner)
        .await
        .unwrap()
        .sources
        .into_iter()
        .find(|source| source.source.kind == SourceKind::RegisteredInstance)
        .unwrap()
        .source;
    let snapshot = read_agent_file_source_inner(&owner, source).await.unwrap();
    let ticket = snapshot.tickets["same"]["default"].clone();
    let default = crate::config::coding_agent_profiles::ReplicaSelectionDefault {
        coding_agent_id: "same".into(),
        requested_profile: "B".into(),
        selection_locked: false,
        configuration_ref: Some(ticket.configuration_ref.clone()),
    };
    let fingerprint = selection_default_fingerprint(temp.path(), Some(&default)).unwrap();
    let mut changed = default.clone();
    changed.configuration_ref = None;
    assert_ne!(
        fingerprint,
        selection_default_fingerprint(temp.path(), Some(&changed)).unwrap()
    );
    let fingerprint =
        source_selection_fingerprint("target", &ticket, Some("source-default")).unwrap();
    assert_ne!(
        fingerprint,
        source_selection_fingerprint("target", &ticket, Some("manual")).unwrap()
    );
    assert_ne!(
        fingerprint,
        source_selection_fingerprint("other", &ticket, Some("source-default")).unwrap()
    );
    for field in ["context", "revision", "ref"] {
        let mut changed = ticket.clone();
        match field {
            "context" => changed.catalog_context_revision.push('x'),
            "revision" => {
                changed.expected_source_revision =
                    crate::config::settings::SourceRevision::Absent {}
            }
            _ => {
                changed.configuration_ref.profile_calculated_id =
                    Some(format!("cid1:{}", "a".repeat(64)))
            }
        }
        assert_ne!(
            fingerprint,
            source_selection_fingerprint("target", &changed, Some("source-default")).unwrap()
        );
    }
}

#[tokio::test]
async fn e2432_p02_contextless_default_ticket_and_source_decoys() {
    use crate::config::coding_agent_profiles::{resolve_profile, ProfileResolutionRequest};
    let (_temp, owner) = fixture();
    let list = list_agent_file_sources_inner(&owner).await.unwrap();
    let registered_ref = list
        .sources
        .iter()
        .find(|s| s.source.kind == SourceKind::RegisteredInstance)
        .unwrap()
        .source
        .clone();
    let registered = read_agent_file_source_inner(&owner, registered_ref)
        .await
        .unwrap();
    let null = &registered.tickets["same"]["default"];
    assert!(null.configuration_ref.requested_profile.is_none());
    assert_eq!(
        serde_json::to_value(null).unwrap()["ref"]["requestedProfile"],
        json!(null)
    );
    let mut projection = crate::config::settings::agent_file_projection(&registered).unwrap();
    let agent = projection.agents[0].clone();
    assert_eq!(
        null.configuration_ref.calculated_id,
        configuration_identity("tool", &[])
    );
    assert_eq!(
        null.configuration_ref.profile_calculated_id,
        Some(pair_identity(
            "tool",
            &[],
            &crate::config::settings::empty_profile_cell()
        ))
    );
    projection
        .coding_agent_profiles
        .default_profile_by_agent
        .insert("target".into(), "B".into());
    let later = resolve_profile(
        &projection,
        ProfileResolutionRequest {
            coding_agent_id: "same",
            launch_path: None,
            agent_matrix_name: Some("target"),
            requested_profile: None,
            requested_profile_authoritative: false,
        },
    );
    assert_eq!(later.effective_profile, "B");
    assert_ne!(
        null.configuration_ref.profile_calculated_id,
        Some(pair_identity(&agent.command, &agent.envs, &later.cell))
    );
    assert_eq!(
        registered.tickets["same"]["B"]
            .configuration_ref
            .profile_calculated_id,
        Some(pair_identity(&agent.command, &agent.envs, &later.cell))
    );

    // Same stable key AND BASE in another physical file cannot donate B or slots.
    let catalog_source = crate::config::settings::list_agent_files(
        &owner.current_snapshot().await.unwrap(),
        &_temp.path().join(SETTINGS_FILE_NAME),
    )
    .unwrap()
    .sources[0]
        .source
        .clone();
    assert_eq!(catalog_source, registered.source);
    let catalog_path = crate::config::coding_agents_catalog::independent_catalog_path(
        _temp.path(),
        SourceKind::CatalogPersonal,
    )
    .unwrap();
    std::fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
    std::fs::write(&catalog_path, serde_json::to_vec(&json!({"schemaVersion":1,"agents":[{"key":"same","label":"Catalog","color":"blue","baseCommand":"tool"}],"profileSlots":{"C":{"label":"Own C"}},"codingAgentProfiles":{"same":{"C":{"additionalArguments":"--catalog"}}}})).unwrap()).unwrap();
    let listed = list_agent_file_sources_inner(&owner).await.unwrap();
    let catalog_ref = listed
        .sources
        .iter()
        .find(|s| s.source.kind == SourceKind::CatalogPersonal)
        .unwrap()
        .source
        .clone();
    let catalog = read_agent_file_source_inner(&owner, catalog_ref)
        .await
        .unwrap();
    assert!(!catalog.tickets["same"].contains_key("B"));
    assert!(catalog.tickets["same"].contains_key("C"));
    assert!(!registered.profile_slots.contains_key("C"));
    assert_eq!(catalog.rows[0].base_hash, registered.rows[0].base_hash);
    assert_ne!(
        catalog.tickets["same"]["C"]
            .configuration_ref
            .profile_calculated_id,
        registered.tickets["same"]["B"]
            .configuration_ref
            .profile_calculated_id
    );
}

#[tokio::test]
async fn e2432_p02_public_save_normalizes_then_returns_fresh_and_rejects_stale() {
    let (_temp, owner) = fixture();
    let mut list = list_agent_file_sources_inner(&owner).await.unwrap();
    let before = read_agent_file_source_inner(&owner, list.sources.remove(0).source)
        .await
        .unwrap();
    let edit: AgentFileEdits = serde_json::from_value(json!({"rows":[],"profileSets":[{"baseHash":configuration_identity("tool", &[]),"cells":{"B":{"additionalArguments":"--edited"}},"labels":{"B":"Edited"}}]})).unwrap();
    let reply = save_agent_file_source_inner(
        &owner,
        before.source.clone(),
        before.revision.clone(),
        before.context_revision.clone(),
        edit,
    )
    .await
    .unwrap();
    assert_eq!(
        reply.outcome,
        crate::config::settings::AgentFileSaveOutcome::Committed
    );
    assert!(!serde_json::to_string(&reply)
        .unwrap()
        .contains("P02-TEST-SECRET"));
    let after = reply.snapshot.unwrap();
    assert_ne!(after.revision, before.revision);
    assert_eq!(
        after.tickets["same"]["B"].expected_source_revision,
        after.revision
    );
    assert!(reply.effective_settings.is_some());
    let stale = save_agent_file_source_inner(
        &owner,
        before.source.clone(),
        before.revision,
        before.context_revision,
        AgentFileEdits::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        stale.outcome,
        crate::config::settings::AgentFileSaveOutcome::Rejected
    );
    assert_eq!(stale.snapshot.unwrap().revision, after.revision);
    remember_agent_file_source_inner(&owner, AgentFileSourceMenu::Picker, before.source)
        .await
        .unwrap();
    let listed = list_agent_file_sources_inner(&owner).await.unwrap();
    assert!(listed.last_selected.settings.is_none());
    assert!(listed.last_selected.picker.is_some());
}
#[tokio::test]
async fn e2432_p02_project_personal_actual_save_and_primary_drift() {
    use crate::config::settings::{AgentFileSaveOutcome, SourceScope};
    let temp = tempfile::tempdir().unwrap();
    let instance = temp.path().join("instance");
    let project = temp.path().join("project");
    let second = temp.path().join("second");
    for dir in [&instance, &project, &second] {
        std::fs::create_dir(dir).unwrap();
    }
    let path = instance.join(SETTINGS_FILE_NAME);
    let mut settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let catalog = crate::config::coding_agents_catalog::independent_catalog_path(
        &project.join(".ac"),
        SourceKind::CatalogPersonal,
    )
    .unwrap();
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let raw = json!({"schemaVersion":2,"agents":[{"key":"same","label":"Catalog","description":"Own","color":"blue","baseCommand":"catalog-tool","extension":{"keep":null}}],"extension":{"preserve":42},"codingAgentProfiles":{"same":{"B":{"additionalArguments":"--own"}}}});
    std::fs::write(&catalog, serde_json::to_vec(&raw).unwrap()).unwrap();
    let owner = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    let list = list_agent_file_sources_inner(&owner).await.unwrap();
    let source = list
        .sources
        .iter()
        .find(|s| {
            s.source.scope == SourceScope::Project && s.source.kind == SourceKind::CatalogPersonal
        })
        .unwrap()
        .source
        .clone();
    let before = read_agent_file_source_inner(&owner, source.clone())
        .await
        .unwrap();
    assert!(before.capabilities.editable_save);
    let edit = serde_json::from_value(json!({"rows":[{"entryHint":"same","action":"upsert","patch":{"label":"Edited"}}],"profileSets":[]})).unwrap();
    let reply = save_agent_file_source_inner(
        &owner,
        source.clone(),
        before.revision.clone(),
        before.context_revision.clone(),
        edit,
    )
    .await
    .unwrap();
    assert_eq!(reply.outcome, AgentFileSaveOutcome::Committed);
    let after = reply.snapshot.unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&catalog).unwrap()).unwrap();
    assert_eq!(saved["schemaVersion"], json!(2));
    assert_eq!(saved["extension"], raw["extension"]);
    assert_eq!(
        saved["agents"][0]["extension"],
        raw["agents"][0]["extension"]
    );
    assert_eq!(saved["agents"][0]["label"], json!("Edited"));
    assert_eq!(after.rows[0].raw["label"], json!("Edited"));
    // Registry changed on disk: old context/source cannot publish into the old project.
    settings.project_paths = vec![second.to_string_lossy().into_owned()];
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let bytes = std::fs::read(&catalog).unwrap();
    let rejected = save_agent_file_source_inner(
        &owner,
        source,
        after.revision,
        before.context_revision,
        AgentFileEdits::default(),
    )
    .await
    .unwrap();
    assert_eq!(rejected.outcome, AgentFileSaveOutcome::Rejected);
    assert_eq!(std::fs::read(&catalog).unwrap(), bytes);
}

#[tokio::test]
async fn e2432_p02_preference_endpoint_rejects_value_after_actual_failed_adoption() {
    use crate::config::settings::{remember_agent_file_locked, OperationOutcome};
    let (temp, owner) = fixture();
    let source = list_agent_file_sources_inner(&owner)
        .await
        .unwrap()
        .sources
        .remove(0)
        .source;
    let worker_owner = Arc::clone(&owner);
    let sidecar = temp
        .path()
        .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
    let report = tokio::task::spawn_blocking(move || {
        worker_owner
            .with_operation_blocking("p02-preference-endpoint-recapture", |current, guard| {
                let preferences = remember_agent_file_locked(
                    current,
                    guard,
                    AgentFileSourceMenu::Picker,
                    &source,
                )?;
                std::fs::write(sidecar, b"{").unwrap();
                Ok(preferences)
            })
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
    assert!(report.value.is_some());
    let error = finish_remember_agent_file_report(&owner, report)
        .await
        .unwrap_err();
    assert!(error.contains("partialOrUnknown"));
}

#[tokio::test]
async fn e2432_p02_native_catalog_events_silent_registered_events_live() {
    use crate::config::settings::{AgentFileSaveOutcome, SourceScope};
    use tauri::{Listener, Manager};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let initial = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    std::fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
    std::fs::write(temp.path().join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME), br#"{"agents":[{"id":"registered","label":"Registered","color":"red","baseCommand":"tool"}]}"#).unwrap();
    let catalog = crate::config::coding_agents_catalog::independent_catalog_path(
        &project.join(".ac"),
        SourceKind::CatalogPersonal,
    )
    .unwrap();
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    std::fs::write(&catalog, br#"{"schemaVersion":1,"agents":[{"key":"own","label":"Original","color":"blue","baseCommand":"catalog-tool"}]}"#).unwrap();
    let owner = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    let broadcaster = WsBroadcaster::new();
    let mut ws = broadcaster.subscribe();
    let app = crate::test_support::test_builder()
        .manage(owner.clone())
        .manage(broadcaster)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let (tx, events) = std::sync::mpsc::channel();
    for event in [
        "coding_agent_settings_updated",
        "coding_agent_profiles_updated",
    ] {
        let tx = tx.clone();
        app.listen_any(event, move |_| {
            tx.send(event).unwrap();
        });
    }
    let list = list_agent_file_sources_inner(&owner).await.unwrap();
    let source = list
        .sources
        .iter()
        .find(|s| {
            s.source.scope == SourceScope::Project && s.source.kind == SourceKind::CatalogPersonal
        })
        .unwrap()
        .source
        .clone();
    let before = read_agent_file_source_inner(&owner, source.clone())
        .await
        .unwrap();
    let epoch = owner.identity().epoch;
    let ram = serde_json::to_value(owner.current_snapshot().await.unwrap()).unwrap();
    let edits = serde_json::from_value(json!({"rows":[{"entryHint":"own","action":"upsert","patch":{"label":"Edited"}}],"profileSets":[]})).unwrap();
    let reply = save_agent_file_source(
        app.handle().clone(),
        app.state(),
        source.clone(),
        before.revision.clone(),
        before.context_revision.clone(),
        edits,
    )
    .await
    .unwrap();
    assert_eq!(reply.outcome, AgentFileSaveOutcome::Committed);
    assert!(reply
        .diagnostics
        .iter()
        .any(|d| d.code == "effectiveSettingsUnchanged"));
    assert_eq!(
        serde_json::to_value(reply.effective_settings.unwrap()).unwrap(),
        ram
    );
    assert_eq!(
        reply.snapshot.as_ref().unwrap().rows[0].raw["label"],
        json!("Edited")
    );
    let actual: Value = serde_json::from_slice(&std::fs::read(&catalog).unwrap()).unwrap();
    assert_eq!(actual["agents"][0]["label"], json!("Edited"));
    let fresh = read_agent_file_source_inner(&owner, source.clone())
        .await
        .unwrap();
    assert_ne!(fresh.revision, before.revision);
    assert_eq!(fresh.revision, reply.snapshot.unwrap().revision);
    assert!(events.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(matches!(
        ws.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let stale = save_agent_file_source(
        app.handle().clone(),
        app.state(),
        source.clone(),
        before.revision,
        before.context_revision,
        AgentFileEdits::default(),
    )
    .await
    .unwrap();
    assert_eq!(stale.outcome, AgentFileSaveOutcome::Rejected);
    assert!(events.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(matches!(
        ws.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let bytes = std::fs::read(&catalog).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&catalog)
            .unwrap();
        let edits = serde_json::from_value(json!({"rows":[{"entryHint":"own","action":"upsert","patch":{"label":"Uncertain"}}],"profileSets":[]})).unwrap();
        let uncertain = save_agent_file_source(
            app.handle().clone(),
            app.state(),
            source,
            fresh.revision.clone(),
            fresh.context_revision,
            edits,
        )
        .await
        .unwrap();
        assert_eq!(uncertain.outcome, AgentFileSaveOutcome::PartialOrUnknown);
        assert_eq!(uncertain.snapshot.unwrap().revision, fresh.revision);
        assert_eq!(std::fs::read(&catalog).unwrap(), bytes);
        assert!(events.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(matches!(
            ws.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        drop(held);
    }
    assert_eq!(owner.identity().epoch, epoch);
    assert_eq!(
        serde_json::to_value(owner.current_snapshot().await.unwrap()).unwrap(),
        ram
    );
    let registered = list
        .sources
        .iter()
        .find(|s| s.source.kind == SourceKind::RegisteredInstance)
        .unwrap()
        .source
        .clone();
    let snapshot = read_agent_file_source_inner(&owner, registered.clone())
        .await
        .unwrap();
    let edits = serde_json::from_value(json!({"rows":[{"entryHint":"registered","action":"upsert","patch":{"label":"Adopted"}}],"profileSets":[]})).unwrap();
    let reply = save_agent_file_source(
        app.handle().clone(),
        app.state(),
        registered,
        snapshot.revision,
        snapshot.context_revision,
        edits,
    )
    .await
    .unwrap();
    assert_eq!(reply.outcome, AgentFileSaveOutcome::Committed);
    assert!(owner.identity().epoch > epoch);
    for expected in [
        "coding_agent_settings_updated",
        "coding_agent_profiles_updated",
    ] {
        assert_eq!(
            events.recv_timeout(Duration::from_secs(1)).unwrap(),
            expected
        );
        let message = ws.try_recv().expect("managed WS sink must be live");
        match message {
            crate::web::broadcast::WsOutMsg::Text(text) => assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap()["event"],
                json!(expected)
            ),
            other => panic!("unexpected WS message {other:?}"),
        }
    }
    assert!(events.try_recv().is_err());
    assert!(matches!(
        ws.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}
