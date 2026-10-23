use super::*;

fn physical_binding_test_settings(dir: &std::path::Path) -> AppSettings {
    let path = dir.join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
    std::fs::write(&path, serde_json::to_vec(&test_settings()).unwrap()).unwrap();
    crate::config::settings::load_settings_from_path(&path)
}

#[tokio::test]
async fn derived_agent_id_binding_creation_restart_and_plain_shell() {
    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let physical = tempfile::tempdir().unwrap();
    let app = session_test_app(
        physical_binding_test_settings(physical.path()),
        session_mgr.clone(),
        pty_mgr.clone(),
    );
    let mut births = Vec::new();
    for index in 0..2 {
        let work = cwd.path().join(format!("binding-{index}"));
        std::fs::create_dir(&work).unwrap();
        let info = super::super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "claude".into(),
            Vec::new(),
            work.to_string_lossy().to_string(),
            Some(format!("binding-{index}")),
            Some("claude".into()),
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .unwrap();
        assert_eq!(info.agent_id.as_deref(), Some("claude"));
        let id = Uuid::parse_str(&info.id).unwrap();
        let manager = session_mgr.read().await.clone();
        let binding = manager.coding_agent_binding(id).await.unwrap();
        assert_ne!(binding.id, info.id);
        assert_eq!(binding.seed.as_ref().unwrap()["originalAgentId"], "claude");
        births.push((info, binding));
    }
    assert_ne!(births[0].1.id, births[1].1.id);
    let manager = session_mgr.read().await.clone();
    let saved_before_failure =
        crate::config::sessions_persistence::snapshot_sessions(&manager).await;
    let invalid_dir = cwd.path().join("file-not-a-directory");
    std::fs::write(&invalid_dir, b"preserve foreign bytes").unwrap();
    assert!(
        crate::config::sessions_persistence::save_sessions_to_dir_for_test(
            &invalid_dir,
            &saved_before_failure
        )
        .is_err()
    );
    let after_failure = crate::config::sessions_persistence::snapshot_sessions(&manager).await;
    let row = after_failure
        .iter()
        .find(|row| row.id.as_deref() == Some(births[0].0.id.as_str()))
        .unwrap();
    assert_eq!(row.coding_agent_binding_id.as_ref(), Some(&births[0].1.id));
    assert_eq!(
        std::fs::read(&invalid_dir).unwrap(),
        b"preserve foreign bytes"
    );
    let old_id = Uuid::parse_str(&births[0].0.id).unwrap();
    let settings = app.state::<crate::config::settings::SettingsState>();
    let restarted = super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(false),
        true,
    )
    .await
    .unwrap();
    assert_ne!(restarted.id, births[0].0.id);
    let after = manager
        .coding_agent_binding(Uuid::parse_str(&restarted.id).unwrap())
        .await
        .unwrap();
    assert_eq!(after.id, births[0].1.id);
    assert_eq!(after.seed, births[0].1.seed);
    assert!(manager.coding_agent_binding(old_id).await.is_none());
    let plain =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    assert!(manager
        .coding_agent_binding(Uuid::parse_str(&plain.id).unwrap())
        .await
        .is_none());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn derived_agent_id_binding_save_replace_create_uses_post_edit_revision() {
    let physical = tempfile::tempdir().unwrap();
    let path = physical
        .path()
        .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
    let current = physical_binding_test_settings(physical.path());
    let before = crate::config::settings::bootstrap_binding_revision(&current, "claude").unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        Arc::new(ScriptedSpawnBackend::default()),
    )));
    let app = session_test_app(current, session_mgr.clone(), pty_mgr.clone());
    let settings = app.state::<crate::config::settings::SettingsState>();
    let mut births: Vec<(
        crate::session::session::SessionInfo,
        crate::session::manager::CodingAgentBinding,
    )> = Vec::new();
    for index in 0..2 {
        let mut draft = settings.read().await.clone();
        if index == 1 {
            draft.agents[0].command = "claude --different-execution".into();
            draft.agents[0]
                .envs
                .push(crate::config::settings::CodingAgentEnv {
                    key: "BINDING_REVISION_TEST".into(),
                    value: "edited".into(),
                    source: Default::default(),
                    enabled: true,
                });
        }
        let written =
            crate::config::settings::save_settings_to_path_preserving_project_paths(&draft, &path)
                .unwrap();
        assert!(written.coding_agent_identity_physical.is_some());
        let expected =
            crate::config::settings::bootstrap_binding_revision(&written, "claude").unwrap();
        let fresh = crate::config::settings::load_settings_from_path(&path);
        assert_eq!(
            expected,
            crate::config::settings::bootstrap_binding_revision(&fresh, "claude").unwrap()
        );
        if index == 1 {
            assert_ne!(
                expected,
                births[0].1.seed.as_ref().unwrap()["identityRevision"]
            );
        }
        *settings.write().await = written;
        let work = cwd.path().join(format!("post-save-{index}"));
        std::fs::create_dir(&work).unwrap();
        let info = super::super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "claude".into(),
            Vec::new(),
            work.to_string_lossy().to_string(),
            Some(format!("post-save-{index}")),
            Some("claude".into()),
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .unwrap();
        let manager = session_mgr.read().await.clone();
        let binding = manager
            .coding_agent_binding(Uuid::parse_str(&info.id).unwrap())
            .await
            .unwrap();
        assert_eq!(binding.seed.as_ref().unwrap()["identityRevision"], expected);
        births.push((info, binding));
    }
    // Save may materialize identity rows, so compare against its real output.
    assert_ne!(
        births[1].1.seed.as_ref().unwrap()["identityRevision"],
        before
    );
    let restarted = super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        Uuid::parse_str(&births[0].0.id).unwrap(),
        None,
        None,
        Some(false),
        true,
    )
    .await
    .unwrap();
    let manager = session_mgr.read().await.clone();
    let after = manager
        .coding_agent_binding(Uuid::parse_str(&restarted.id).unwrap())
        .await
        .unwrap();
    assert_eq!(after.id, births[0].1.id);
    assert_eq!(after.seed, births[0].1.seed);
    assert_ne!(after.seed, births[1].1.seed);
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn derived_agent_id_binding_wake_restore_preserves_birth_seed_and_origin() {
    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let physical = tempfile::tempdir().unwrap();
    let app = session_test_app(
        physical_binding_test_settings(physical.path()),
        session_mgr.clone(),
        pty_mgr.clone(),
    );
    let transaction =
        crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
    let birth = crate::session::manager::CodingAgentBinding {
        id: Uuid::new_v4().to_string(),
        seed: Some(serde_json::json!({
            "identityRevision": format!("r1:0:{}", "a".repeat(64)),
            "originalAgentId": "claude"
        })),
        origin: Some(serde_json::json!({
            "sessionsPath": "C:/original-sessions.json",
            "rawFileSHA256": "physical-original",
            "originalOrdinal": 5
        })),
    };
    let info = super::super::create_session_inner_for_restore(
        &transaction,
        &session_mgr,
        &pty_mgr,
        "claude".into(),
        Vec::new(),
        cwd.path().to_string_lossy().to_string(),
        Some("wake restore".into()),
        Some("claude".into()),
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        Some(false),
        None,
        Some(birth.clone()),
        None,
    )
    .await
    .unwrap();
    let manager = session_mgr.read().await.clone();
    assert_eq!(info.agent_id.as_deref(), Some("claude"));
    let restored = manager
        .coding_agent_binding(Uuid::parse_str(&info.id).unwrap())
        .await
        .unwrap();
    assert_ne!(birth.id, info.id);
    assert_eq!(restored.id, birth.id);
    assert_eq!(restored.seed, birth.seed);
    assert_eq!(restored.origin, birth.origin);
    let snapshot = crate::config::sessions_persistence::snapshot_sessions(&manager).await;
    assert_eq!(
        snapshot[0].coding_agent_binding_id.as_ref(),
        Some(&birth.id)
    );
    drop(transaction);
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn derived_agent_id_binding_seedless_restore_and_new_birth_preserve_compatibility() {
    for case in ["removed", "ambiguous", "unreadable", "missing-physical"] {
        let physical = tempfile::tempdir().unwrap();
        let path = physical
            .path()
            .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
        let mut raw = serde_json::to_value(test_settings()).unwrap();
        if case == "removed" {
            raw["agents"] = serde_json::json!([]);
        }
        if case == "ambiguous" {
            let repeated = raw["agents"][0].clone();
            raw["agents"].as_array_mut().unwrap().push(repeated);
        }
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        if case == "unreadable" {
            std::fs::write(
                physical
                    .path()
                    .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
                b"invalid agents JSON",
            )
            .unwrap();
        }
        let cfg = if case == "missing-physical" {
            test_settings()
        } else {
            crate::config::settings::load_settings_from_path(&path)
        };
        let expected_error = match case {
            "removed" => "coding_agent_binding_entry_missing_or_ambiguous",
            "ambiguous" => "coding_agent_identity_duplicate",
            "unreadable" => "coding_agent_identity_physical_unreadable",
            _ => "coding_agent_identity_physical_missing",
        };
        assert_eq!(
            crate::config::settings::bootstrap_binding_revision(&cfg, "claude").unwrap_err(),
            expected_error
        );
        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            Arc::new(ScriptedSpawnBackend::default()),
        )));
        let app = session_test_app(cfg, session_mgr.clone(), pty_mgr.clone());
        let transaction =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        let original_recipe = crate::config::sessions_persistence::PersistedSession {
            name: case.into(),
            shell: "test-shell".into(),
            working_directory: cwd.path().to_string_lossy().to_string(),
            agent_id: Some("claude".into()),
            ..Default::default()
        };
        let sessions_path = physical.path().join("sessions.json");
        let bytes = serde_json::to_vec(&vec![original_recipe.clone()]).unwrap();
        std::fs::write(&sessions_path, &bytes).unwrap();
        let mut loaded =
            crate::config::sessions_persistence::load_sessions_from_dir(physical.path());
        assert_eq!(loaded.len(), 1);
        let mut recipe = loaded.remove(0);
        let loaded_id = recipe.coding_agent_binding_id.clone();
        let loaded_origin = recipe.coding_agent_binding_origin.clone();
        use sha2::{Digest, Sha256};
        assert_eq!(
            loaded_origin.as_ref().unwrap()["rawFileSHA256"],
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert_eq!(
            loaded_origin.as_ref().unwrap()["sessionsPath"],
            sessions_path.to_string_lossy().as_ref()
        );
        assert_eq!(loaded_origin.as_ref().unwrap()["originalOrdinal"], 0);
        let settings = app.state::<crate::config::settings::SettingsState>();
        let cfg = settings.read().await;
        assert!(recipe.capture_binding_seed(&cfg).is_err());
        drop(cfg);
        assert_eq!(recipe.coding_agent_binding_id, loaded_id);
        assert_eq!(recipe.coding_agent_binding_origin, loaded_origin);
        assert_eq!(recipe.shell, original_recipe.shell);
        let birth = recipe.binding_metadata().unwrap().unwrap();
        assert!(birth.seed.is_none());
        let info = super::super::create_session_inner_for_restore(
            &transaction,
            &session_mgr,
            &pty_mgr,
            recipe.shell.clone(),
            recipe.shell_args.clone(),
            recipe.working_directory.clone(),
            Some(recipe.name.clone()),
            recipe.agent_id.clone(),
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            Some(false),
            None,
            Some(birth.clone()),
            None,
        )
        .await
        .unwrap();
        let manager = session_mgr.read().await.clone();
        assert_eq!(info.shell, recipe.shell);
        let restored = manager
            .coding_agent_binding(Uuid::parse_str(&info.id).unwrap())
            .await
            .unwrap();
        assert_eq!(restored.id, birth.id);
        assert_eq!(restored.seed, birth.seed);
        assert_eq!(restored.origin, birth.origin);
        let snapshot = crate::config::sessions_persistence::snapshot_sessions(&manager).await;
        let stored = snapshot
            .iter()
            .find(|row| row.id.as_deref() == Some(info.id.as_str()))
            .unwrap();
        assert_eq!(
            stored.coding_agent_binding_id,
            recipe.coding_agent_binding_id
        );
        assert_eq!(
            stored.coding_agent_binding_origin,
            recipe.coding_agent_binding_origin
        );
        assert!(stored.coding_agent_binding_seed.is_none());
        drop(transaction);
        // Central creation adds no capture-only veto; public launch gates stay intact.
        let new_cwd = cwd.path().join("new-birth");
        std::fs::create_dir(&new_cwd).unwrap();
        let new = super::super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "claude".into(),
            Vec::new(),
            new_cwd.to_string_lossy().to_string(),
            Some("seedless new birth".into()),
            Some("claude".into()),
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .unwrap();
        let new_binding = manager
            .coding_agent_binding(Uuid::parse_str(&new.id).unwrap())
            .await;
        if case == "removed" {
            assert!(new.agent_id.is_none());
            assert!(new_binding.is_none());
        } else {
            assert_eq!(new.agent_id.as_deref(), Some("claude"));
            let new_binding = new_binding.unwrap();
            assert!(new_binding.seed.is_none());
            assert_ne!(new_binding.id, birth.id);
        }
        if case == "removed" {
            let settings = app.state::<crate::config::settings::SettingsState>();
            let restarted = super::super::restart_session_inner_with_activation(
                app.handle(),
                &session_mgr,
                &pty_mgr,
                settings.inner(),
                Uuid::parse_str(&info.id).unwrap(),
                None,
                None,
                Some(false),
                true,
            )
            .await
            .unwrap();
            let after = manager
                .coding_agent_binding(Uuid::parse_str(&restarted.id).unwrap())
                .await
                .unwrap();
            assert_eq!(after.id, birth.id);
            assert_eq!(after.seed, birth.seed);
            assert_eq!(after.origin, birth.origin);
        }
        close_test_coordinator(&app).await;
    }
}
