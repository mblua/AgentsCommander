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

async fn root_restore_case(
    fresh: Option<bool>,
    provider: Option<&str>,
    args: &[&str],
    configured: bool,
) -> (
    crate::pty::backend::BackendSpawnSpec,
    SessionInfo,
    Option<crate::config::agent_command::AgentSpawnCommand>,
) {
    let cwd = tempfile::tempdir().unwrap();
    let cwd_text = cwd.path().to_string_lossy().into_owned();
    let mut settings = test_settings();
    let claude_config = cwd.path().join("claude-config");
    if provider == Some("claude") {
        let projects = super::super::claude_projects_dir_for_config_dir(
            claude_config.to_str().unwrap(),
            &cwd_text,
        );
        std::fs::create_dir_all(projects).unwrap();
        settings.agents[0]
            .envs
            .push(crate::config::settings::CodingAgentEnv {
                key: "CLAUDE_CONFIG_DIR".into(),
                value: claude_config.to_string_lossy().into_owned(),
                source: Default::default(),
                enabled: true,
            });
    }
    let spawn = if configured {
        let mut spawn = super::super::build_configured_agent_spawn_for_cwd(
            &settings,
            provider.unwrap(),
            &cwd_text,
            None,
        )
        .unwrap()
        .unwrap();
        spawn.shell_args = args.iter().map(|arg| (*arg).into()).collect();
        Some(spawn)
    } else {
        None
    };
    let mut recipe = crate::config::sessions_persistence::PersistedSession {
        name: "3431 persisted Root".into(),
        shell: provider.unwrap_or("ordinary-shell").into(),
        shell_args: args.iter().map(|arg| (*arg).into()).collect(),
        agent_label: Some("persisted label".into()),
        working_directory: cwd_text.clone(),
        agent_id: provider.map(str::to_owned),
        ..Default::default()
    };
    if configured {
        recipe.shell = "stale-recipe-shell".into();
        recipe.shell_args = vec!["stale-recipe-argument".into()];
    }
    if let Some(fresh) = fresh {
        recipe.start_fresh_on_restore = fresh;
    } else {
        let mut value = serde_json::to_value(&recipe).unwrap();
        assert!(value
            .as_object_mut()
            .unwrap()
            .remove("startFreshOnRestore")
            .is_some());
        recipe = serde_json::from_value(value).unwrap();
        assert!(!recipe.start_fresh_on_restore);
    }
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings, sessions.clone(), pty.clone());
    let no_mirror = app
        .try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        .is_none();
    let transaction =
        crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
    let host_shell = spawn
        .as_ref()
        .map(|_| crate::pty::backend::ResolvedAgentHostShell {
            program: "configured-host-shell".into(),
            args: vec!["host-only-argument".into()],
        });
    super::super::coordinator_create_seam::force_root(&cwd_text);
    let result = super::super::create_persisted_root_for_restore(
        &transaction,
        &sessions,
        &pty,
        &recipe,
        cwd_text.clone(),
        spawn.clone(),
        host_shell.clone(),
    )
    .await;
    super::super::coordinator_create_seam::unforce_root(&cwd_text);
    drop(transaction);
    close_test_coordinator(&app).await;
    assert!(
        no_mirror,
        "Root fixture must have no coordinator clocks mirror"
    );
    let info = result.expect("persisted Root creation");
    let mut specs = backend.specs.lock().unwrap();
    assert_eq!(specs.len(), 1, "exactly one backend spawn");
    let spec = specs.remove(0);
    assert!(info.is_root_agent, "fixture must reach Root creation");
    assert_eq!(info.start_fresh_on_restore, fresh.unwrap_or(false));
    assert_eq!(spec.resolved_agent_host_shell, host_shell);
    assert_eq!(
        spec.cmd,
        spawn.as_ref().map_or(&recipe.shell, |s| &s.shell).as_str()
    );
    assert_eq!(
        info.agent_label,
        spawn
            .as_ref()
            .map(|s| s.trusted_agent_label.clone())
            .or(recipe.agent_label.filter(|_| provider.is_some()))
    );
    (spec, info, spawn)
}

#[tokio::test]
async fn issue_3431_root_restore_intent() {
    let mut failures = Vec::new();
    for fresh in [Some(true), Some(false), None] {
        let (spec, _, _) = root_restore_case(fresh, Some("codex"), &["-m", "model"], false).await;
        let expected = if fresh == Some(true) {
            vec!["-m", "model"]
        } else {
            vec!["resume", "--last", "-m", "model"]
        };
        eprintln!(
            "intent fresh={fresh:?} observed={:?} expected={expected:?}",
            spec.args
        );
        if spec.args != expected {
            failures.push(format!("fresh={fresh:?}: {:?} != {expected:?}", spec.args));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn issue_3431_root_restore_recipe() {
    let mut failures = Vec::new();
    for fresh in [true, false] {
        for configured in [true, false] {
            let args = if configured {
                ["-m", "configured"]
            } else {
                ["-m", "persisted"]
            };
            let (spec, _, spawn) =
                root_restore_case(Some(fresh), Some("codex"), &args, configured).await;
            let mut expected = spawn.map_or_else(
                || args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
                |spawn| spawn.shell_args,
            );
            if !fresh {
                expected.splice(0..0, ["resume".into(), "--last".into()]);
            }
            eprintln!(
                "recipe fresh={fresh} configured={configured} observed={:?}",
                spec.args
            );
            if spec.args != expected {
                failures.push(format!(
                    "{fresh}/{configured}: {:?} != {expected:?}",
                    spec.args
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn issue_3431_root_restore_command_controls() {
    let mut failures = Vec::new();
    for fresh in [true, false] {
        for (provider, args) in [
            (Some("codex"), vec!["exec", "task"]),
            (Some("codex"), vec!["resume", "--last"]),
            (None, vec!["ordinary-argument"]),
            (Some("claude"), vec!["--model", "test-model"]),
        ] {
            let claude = provider == Some("claude");
            let (spec, _, _) = root_restore_case(Some(fresh), provider, &args, claude).await;
            let mut expected: Vec<String> = args.iter().map(|arg| (*arg).into()).collect();
            if claude {
                if fresh {
                    let session_id = spec.args.last().expect("fresh Claude session id");
                    if Uuid::parse_str(session_id).is_err() {
                        failures.push(format!("fresh Claude resumed: {:?}", spec.args));
                        continue;
                    }
                    expected.extend(["--session-id".into(), session_id.clone()]);
                } else {
                    expected.push("--continue".into());
                }
            }
            eprintln!(
                "control fresh={fresh} provider={provider:?} observed={:?}",
                spec.args
            );
            if spec.args != expected {
                failures.push(format!(
                    "{fresh}/{provider:?}: {:?} != {expected:?}",
                    spec.args
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
