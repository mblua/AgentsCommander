use super::*;

#[tokio::test]
async fn issue_3145_cold_duplicate_recipes_block_before_first_restore_publication() {
    use tauri::Listener;
    for fresh in [false, true] {
        let cwd = tempfile::tempdir().unwrap();
        let cwd_text = cwd.path().to_string_lossy().into_owned();
        let settings = test_settings();
        let spawn =
            super::super::build_configured_agent_spawn_for_cwd(&settings, "codex", &cwd_text, None)
                .unwrap()
                .unwrap();
        let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(settings, sessions.clone(), pty.clone());
        let transaction =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        let recipe = crate::config::sessions_persistence::PersistedSession {
            name: "first duplicate".into(),
            shell: spawn.shell.clone(),
            shell_args: spawn.shell_args.clone(),
            working_directory: cwd_text.clone(),
            agent_id: Some("codex".into()),
            requested_profile: Some(spawn.profile_resolution.requested_profile.clone()),
            effective_profile: Some(spawn.profile_resolution.effective_profile.clone()),
            start_fresh_on_restore: fresh,
            ..Default::default()
        };
        let manager = sessions.read().await.clone();
        crate::config::sessions_persistence::initialize_prompt_retention(&manager).await;
        let key = crate::config::sessions_persistence::prompt_key(
            &cwd_text,
            recipe.agent_id.as_deref(),
            recipe.requested_profile.as_deref(),
            recipe.effective_profile.as_deref(),
        )
        .unwrap();
        manager
            .install_retained_prompts(Some(HashMap::from([(
                key.clone(),
                crate::session::manager::RetainedPrompt {
                    last_prompt: Some("unproven old cache".into()),
                    blocked: false,
                },
            )])))
            .await;
        let mut dormant_competitor = recipe.clone();
        dormant_competitor.name = "second dormant duplicate".into();
        dormant_competitor.status = Some(SessionStatus::Exited(0));
        crate::config::sessions_persistence::block_duplicate_restore_prompt_domains(
            &manager,
            &[recipe.clone(), dormant_competitor],
        )
        .await;
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured = events.clone();
        app.listen("session_created", move |event| {
            captured
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        let result = super::super::create_session_inner_for_restore(
            &transaction,
            &sessions,
            &pty,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            cwd_text,
            Some(recipe.name.clone()),
            Some("codex".into()),
            None,
            true,
            Vec::new(),
            fresh,
            Some(spawn),
            None,
            None,
            Some(fresh),
            None,
            None,
            Some(&recipe),
        )
        .await
        .unwrap();
        assert_eq!(result.last_prompt, None);
        assert!(manager.retained_prompt_snapshot().await.unwrap()[&key].blocked);
        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert!(events[0]["lastPrompt"].is_null());
        drop(transaction);
        close_test_coordinator(&app).await;
    }
}

#[tokio::test]
async fn issue_3145_wake_and_dormant_publish_initial_prompt_before_observation() {
    use tauri::Listener;
    for (dormant, root) in [(false, false), (false, true), (true, false), (true, true)] {
        let cwd = tempfile::tempdir().unwrap();
        let cwd_text = cwd.path().to_string_lossy().into_owned();
        let settings = test_settings();
        let spawn =
            super::super::build_configured_agent_spawn_for_cwd(&settings, "codex", &cwd_text, None)
                .unwrap()
                .unwrap();
        let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(settings, sessions.clone(), pty.clone());
        let transaction =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        if root {
            super::super::coordinator_create_seam::force_root(&cwd_text);
        }
        let recipe = crate::config::sessions_persistence::PersistedSession {
            name: "restore prompt".into(),
            shell: spawn.shell.clone(),
            shell_args: spawn.shell_args.clone(),
            working_directory: cwd_text.clone(),
            agent_id: Some("codex".into()),
            requested_profile: Some(spawn.profile_resolution.requested_profile.clone()),
            effective_profile: Some(spawn.profile_resolution.effective_profile.clone()),
            last_prompt: Some("A".into()),
            ..Default::default()
        };
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured = events.clone();
        app.listen("session_created", move |event| {
            captured
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        let info = if dormant {
            transaction
                .restore_dormant_inline(crate::session::selection::DormantRestoreRequest {
                    persisted: recipe.clone(),
                    working_directory: cwd_text,
                    is_coordinator: false,
                    is_root_agent: root,
                })
                .await
                .unwrap()
        } else {
            super::super::create_session_inner_for_restore(
                &transaction,
                &sessions,
                &pty,
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                cwd_text,
                Some(recipe.name.clone()),
                Some("codex".into()),
                None,
                true,
                Vec::new(),
                false,
                Some(spawn),
                None,
                None,
                Some(false),
                None,
                None,
                Some(&recipe),
            )
            .await
            .unwrap()
        };
        assert_eq!(info.last_prompt.as_deref(), Some("A"));
        assert_eq!(
            sessions.read().await.list_sessions().await[0]
                .last_prompt
                .as_deref(),
            Some("A")
        );
        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["lastPrompt"], "A");
        assert_eq!(info.is_root_agent, root);
        super::super::coordinator_create_seam::unforce_root(&recipe.working_directory);
        drop(transaction);
        close_test_coordinator(&app).await;
    }
}

#[tokio::test]
async fn issue_3145_reused_root_live_and_dormant_seed_event_never_overwrite_new_input() {
    use tauri::Listener;
    for dormant in [false, true] {
        for newer in [None, Some("N"), Some("")] {
            let cwd = tempfile::tempdir().unwrap();
            let cwd_text = cwd.path().to_string_lossy().into_owned();
            let settings = test_settings();
            let spawn = super::super::build_configured_agent_spawn_for_cwd(
                &settings, "codex", &cwd_text, None,
            )
            .unwrap()
            .unwrap();
            let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
            let backend = Arc::new(ScriptedSpawnBackend::default());
            let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
                backend,
            )));
            let app = session_test_app(settings, sessions.clone(), pty.clone());
            let transaction =
                crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
            let mut recipe = crate::config::sessions_persistence::PersistedSession {
                name: "reused root".into(),
                shell: spawn.shell.clone(),
                shell_args: spawn.shell_args.clone(),
                working_directory: cwd_text.clone(),
                agent_id: Some("codex".into()),
                requested_profile: Some(spawn.profile_resolution.requested_profile.clone()),
                effective_profile: Some(spawn.profile_resolution.effective_profile.clone()),
                ..Default::default()
            };
            let info = if dormant {
                transaction
                    .restore_dormant_inline(crate::session::selection::DormantRestoreRequest {
                        persisted: recipe.clone(),
                        working_directory: cwd_text,
                        is_coordinator: false,
                        is_root_agent: true,
                    })
                    .await
                    .unwrap()
            } else {
                super::super::create_session_inner_for_restore(
                    &transaction,
                    &sessions,
                    &pty,
                    spawn.shell.clone(),
                    spawn.shell_args.clone(),
                    cwd_text,
                    Some(recipe.name.clone()),
                    Some("codex".into()),
                    None,
                    true,
                    Vec::new(),
                    false,
                    Some(spawn),
                    None,
                    None,
                    Some(false),
                    None,
                    None,
                    Some(&recipe),
                )
                .await
                .unwrap()
            };
            let id = Uuid::parse_str(&info.id).unwrap();
            let manager = sessions.read().await.clone();
            manager.set_is_root_agent(id, true).await;
            if let Some(text) = newer {
                manager.set_last_prompt(id, text.into()).await;
            }
            let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
            let captured = events.clone();
            app.listen("last_prompt", move |event| {
                captured
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(event.payload()).unwrap());
            });
            recipe.last_prompt = Some("A".into());
            super::super::seed_root_last_prompt(app.handle(), &sessions, id, &recipe).await;
            assert_eq!(
                manager
                    .get_session(id)
                    .await
                    .unwrap()
                    .last_prompt
                    .as_deref(),
                Some(newer.unwrap_or("A"))
            );
            let observed = events.lock().unwrap().clone();
            assert_eq!(observed.len(), usize::from(newer.is_none()));
            if newer.is_none() {
                assert_eq!(observed[0]["text"], "A");
            }
            drop(transaction);
            close_test_coordinator(&app).await;
        }
    }
}
