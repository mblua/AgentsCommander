use super::*;

#[tokio::test]
async fn self_switch_established_source_to_pi_target_remains_supported() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    {
        let settings = app.state::<SettingsState>();
        settings
            .write()
            .await
            .agents
            .push(wake_agent("pi", "Pi", "pi"));
    }
    let (source_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "codex",
        Some("codex"),
        Some("A"),
        Some("A"),
    )
    .await;
    {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = state.read().await;
        mgr.mark_idle(source_id).await;
    }
    seed_self_handoff(&fixture.replica);
    let (path, message) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-pi-target",
        "rid-ss-pi-target",
        Some(token.to_string()),
        Some("pi"),
        Some("A"),
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::new();

    poller
        .handle_self_handoff_switch(&app, &path, &message, false)
        .await
        .unwrap();
    let response = read_response_json(&fixture.replica, "rid-ss-pi-target").unwrap();
    assert_eq!(response["status"], "queued");
    assert_eq!(response["target_coding_agent"], "pi");
    assert_eq!(response["target_profile"], "A");

    let target_agent = response["target_coding_agent"]
        .as_str()
        .unwrap()
        .to_string();
    let target_profile = response["target_profile"].as_str().unwrap().to_string();
    let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();
    let state_ids = Arc::new(Mutex::new(Vec::<Uuid>::new()));
    let state_ids_seen = Arc::clone(&state_ids);
    let app_for_state = app.clone();
    let session_state = move |session_id: Uuid| {
        let app = app_for_state.clone();
        let state_ids = Arc::clone(&state_ids_seen);
        async move {
            state_ids.lock().unwrap().push(session_id);
            let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = state.read().await;
            match mgr.get_session(session_id).await {
                Some(session) => (true, session.waiting_for_input),
                None => (false, false),
            }
        }
    };

    let app_for_persist = app.clone();
    let persist = move |cwd: PathBuf, agent: String, profile: String| {
        let app = app_for_persist.clone();
        async move {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
                &snapshot, &cwd, &agent, &profile,
            )
        }
    };

    let restarted_id = Arc::new(Mutex::new(None::<Uuid>));
    let restarted_id_seen = Arc::clone(&restarted_id);
    let app_for_restart = app.clone();
    let replica_for_restart = fixture.replica.clone();
    let restart = move |session_id: Uuid, agent: String, profile: String| {
        let app = app_for_restart.clone();
        let replica = replica_for_restart.clone();
        let restarted_id = Arc::clone(&restarted_id_seen);
        async move {
            assert_eq!(session_id, source_id);
            assert_eq!(agent, "pi");
            assert_eq!(profile, "A");
            {
                let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = state.read().await;
                mgr.destroy_session(session_id).await.unwrap();
            }
            let (new_id, _token) = seed_self_switch_session(
                &app,
                &replica,
                "pi",
                Some("pi"),
                Some(&profile),
                Some(&profile),
            )
            .await;
            {
                let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = state.read().await;
                mgr.mark_idle(new_id).await;
            }
            register_mock_pty_route(&app, new_id);
            *restarted_id.lock().unwrap() = Some(new_id);
            Ok(new_id.to_string())
        }
    };

    let injected_prompts = Arc::new(Mutex::new(Vec::<(Uuid, String)>::new()));
    let injected_prompts_seen = Arc::clone(&injected_prompts);
    let app_for_inject = app.clone();
    let inject = move |session_id: Uuid, prompt: String| {
        let app = app_for_inject.clone();
        let injected_prompts = Arc::clone(&injected_prompts_seen);
        async move {
            injected_prompts
                .lock()
                .unwrap()
                .push((session_id, prompt.clone()));
            crate::pty::inject::inject_text_into_session(&app, session_id, &prompt).await
        }
    };
    let boundaries = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let boundaries_seen = Arc::clone(&boundaries);
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let boundaries = Arc::clone(&boundaries_seen);
        async move {
            boundaries.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        source_id,
        fixture.replica.clone(),
        target_agent,
        target_profile,
        None,
        SELF_HANDOFF_FLAVOR_SWITCH,
        Arc::clone(&pending),
        Duration::ZERO,
        Duration::ZERO,
        // The real app-backed restart can exceed one second under the full
        // parallel suite. Only settle and poll need to be zero in this test.
        Duration::from_secs(30),
        session_state,
        persist,
        restart,
        inject,
        note_boundary,
    )
    .await;

    let target_id = restarted_id
        .lock()
        .unwrap()
        .expect("restart seam must return the configured Pi session id");
    assert_ne!(target_id, source_id);
    assert_eq!(*state_ids.lock().unwrap(), vec![source_id, target_id]);
    let prompts = injected_prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].0, target_id);
    assert!(prompts[0].1.contains("self-clear/"), "{}", prompts[0].1);
    assert_eq!(
        mock_pty_writes_for(&app, target_id),
        vec![
            prompts[0].1.as_bytes().to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec()
        ]
    );
    assert_eq!(
        *boundaries.lock().unwrap(),
        vec![(target_id, SelfClearBoundary::ContentInjected)]
    );
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    assert!(manager.get_session(source_id).await.is_none());
    let target = manager.get_session(target_id).await.unwrap();
    assert_eq!(target.shell, "pi");
    assert_eq!(target.agent_id.as_deref(), Some("pi"));
    assert_eq!(target.requested_profile.as_deref(), Some("A"));
    assert_eq!(target.effective_profile.as_deref(), Some("A"));
    assert!(pending.0.lock().unwrap().is_empty());
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.replica.join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["tooling"]["currentCodingAgent"], "pi");
    assert_eq!(saved["tooling"]["profile"], "A");
}

#[tokio::test]
async fn handle_self_switch_pi_source_queues_with_resolved_targets() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    // A Pi source needs no `pi` agent push: the target is `codex` (already in
    // the fixture wake_agents) and the source agent_id is never
    // configured-checked, only used as a target-resolution fallback.
    let (session_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "pi",
        Some("pi"),
        Some("A"),
        Some("B"),
    )
    .await;
    seed_self_handoff(&fixture.replica);
    std::fs::write(fixture.replica.join("SELF-FORGET.md"), "topic to forget").unwrap();

    let (path, msg) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-pi-queue",
        "rid-ss-pi-queue",
        Some(token.to_string()),
        Some("codex"),
        Some("c"),
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::new();
    poller
        .handle_self_handoff_switch(&app, &path, &msg, false)
        .await
        .unwrap();

    let response = read_response_json(&fixture.replica, "rid-ss-pi-queue").unwrap();
    assert_eq!(response["action"], SELF_SWITCH_ACTION);
    assert_eq!(response["status"], "queued");
    assert_eq!(response["target_coding_agent"], "codex");
    assert_eq!(response["target_profile"], "C");
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(pending_self_clear_len(&app).await, 1);
    assert_eq!(count_forget_archives(&fixture.replica), 1);
    assert!(!fixture.replica.join("SELF-FORGET.md").is_file());
    assert!(!path.exists());
}

#[tokio::test]
async fn self_switch_pi_source_to_established_target_completes() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    let (source_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "pi",
        Some("pi"),
        Some("A"),
        Some("A"),
    )
    .await;
    {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = state.read().await;
        mgr.mark_idle(source_id).await;
    }
    seed_self_handoff(&fixture.replica);
    let (path, message) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-pi-to-codex",
        "rid-ss-pi-to-codex",
        Some(token.to_string()),
        Some("codex"),
        Some("A"),
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::new();

    poller
        .handle_self_handoff_switch(&app, &path, &message, false)
        .await
        .unwrap();
    let response = read_response_json(&fixture.replica, "rid-ss-pi-to-codex").unwrap();
    assert_eq!(response["status"], "queued");
    assert_eq!(response["target_coding_agent"], "codex");
    assert_eq!(response["target_profile"], "A");

    let target_agent = response["target_coding_agent"]
        .as_str()
        .unwrap()
        .to_string();
    let target_profile = response["target_profile"].as_str().unwrap().to_string();
    let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();
    let state_ids = Arc::new(Mutex::new(Vec::<Uuid>::new()));
    let state_ids_seen = Arc::clone(&state_ids);
    let app_for_state = app.clone();
    let session_state = move |session_id: Uuid| {
        let app = app_for_state.clone();
        let state_ids = Arc::clone(&state_ids_seen);
        async move {
            state_ids.lock().unwrap().push(session_id);
            let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = state.read().await;
            match mgr.get_session(session_id).await {
                Some(session) => (true, session.waiting_for_input),
                None => (false, false),
            }
        }
    };

    let app_for_persist = app.clone();
    let persist = move |cwd: PathBuf, agent: String, profile: String| {
        let app = app_for_persist.clone();
        async move {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
                &snapshot, &cwd, &agent, &profile,
            )
        }
    };

    let restarted_id = Arc::new(Mutex::new(None::<Uuid>));
    let restarted_id_seen = Arc::clone(&restarted_id);
    let app_for_restart = app.clone();
    let replica_for_restart = fixture.replica.clone();
    let restart = move |session_id: Uuid, agent: String, profile: String| {
        let app = app_for_restart.clone();
        let replica = replica_for_restart.clone();
        let restarted_id = Arc::clone(&restarted_id_seen);
        async move {
            assert_eq!(session_id, source_id);
            assert_eq!(agent, "codex");
            assert_eq!(profile, "A");
            {
                let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = state.read().await;
                mgr.destroy_session(session_id).await.unwrap();
            }
            let (new_id, _token) = seed_self_switch_session(
                &app,
                &replica,
                "codex",
                Some("codex"),
                Some(&profile),
                Some(&profile),
            )
            .await;
            {
                let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = state.read().await;
                mgr.mark_idle(new_id).await;
            }
            register_mock_pty_route(&app, new_id);
            *restarted_id.lock().unwrap() = Some(new_id);
            Ok(new_id.to_string())
        }
    };

    let injected_prompts = Arc::new(Mutex::new(Vec::<(Uuid, String)>::new()));
    let injected_prompts_seen = Arc::clone(&injected_prompts);
    let app_for_inject = app.clone();
    let inject = move |session_id: Uuid, prompt: String| {
        let app = app_for_inject.clone();
        let injected_prompts = Arc::clone(&injected_prompts_seen);
        async move {
            injected_prompts
                .lock()
                .unwrap()
                .push((session_id, prompt.clone()));
            crate::pty::inject::inject_text_into_session(&app, session_id, &prompt).await
        }
    };
    let boundaries = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let boundaries_seen = Arc::clone(&boundaries);
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let boundaries = Arc::clone(&boundaries_seen);
        async move {
            boundaries.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        source_id,
        fixture.replica.clone(),
        target_agent,
        target_profile,
        None,
        SELF_HANDOFF_FLAVOR_SWITCH,
        Arc::clone(&pending),
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(30),
        session_state,
        persist,
        restart,
        inject,
        note_boundary,
    )
    .await;

    let target_id = restarted_id
        .lock()
        .unwrap()
        .expect("restart seam must return the configured target session id");
    assert_ne!(target_id, source_id);
    assert_eq!(*state_ids.lock().unwrap(), vec![source_id, target_id]);
    let prompts = injected_prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].0, target_id);
    assert!(prompts[0].1.contains("self-clear/"), "{}", prompts[0].1);
    assert_eq!(
        mock_pty_writes_for(&app, target_id),
        vec![
            prompts[0].1.as_bytes().to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec()
        ]
    );
    assert_eq!(
        *boundaries.lock().unwrap(),
        vec![(target_id, SelfClearBoundary::ContentInjected)]
    );
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    assert!(manager.get_session(source_id).await.is_none());
    let target = manager.get_session(target_id).await.unwrap();
    assert_eq!(target.shell, "codex");
    assert_eq!(target.agent_id.as_deref(), Some("codex"));
    assert!(pending.0.lock().unwrap().is_empty());
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.replica.join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "A");
}

#[tokio::test]
async fn self_switch_pi_source_omitted_target_hard_resets_to_pi() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    // The target resolves to `pi` (the source agent_id fallback), so `pi`
    // must be configured for target spawn validation.
    {
        let settings = app.state::<SettingsState>();
        settings
            .write()
            .await
            .agents
            .push(wake_agent("pi", "Pi", "pi"));
    }
    let (source_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "pi",
        Some("pi"),
        Some("A"),
        Some("A"),
    )
    .await;
    {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = state.read().await;
        mgr.mark_idle(source_id).await;
    }
    seed_self_handoff(&fixture.replica);
    // Both --coding-agent and --profile omitted: the target resolves from the
    // source agent_id fallback back to `pi` (the #1081 hard-reset headline).
    let (path, message) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-pi-hard-reset",
        "rid-ss-pi-hard-reset",
        Some(token.to_string()),
        None,
        None,
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::new();

    poller
        .handle_self_handoff_switch(&app, &path, &message, false)
        .await
        .unwrap();
    let response = read_response_json(&fixture.replica, "rid-ss-pi-hard-reset").unwrap();
    assert_eq!(response["status"], "queued");
    assert_eq!(response["target_coding_agent"], "pi");
    assert_eq!(response["target_profile"], "A");

    let target_agent = response["target_coding_agent"]
        .as_str()
        .unwrap()
        .to_string();
    let target_profile = response["target_profile"].as_str().unwrap().to_string();
    let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();
    let state_ids = Arc::new(Mutex::new(Vec::<Uuid>::new()));
    let state_ids_seen = Arc::clone(&state_ids);
    let app_for_state = app.clone();
    let session_state = move |session_id: Uuid| {
        let app = app_for_state.clone();
        let state_ids = Arc::clone(&state_ids_seen);
        async move {
            state_ids.lock().unwrap().push(session_id);
            let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = state.read().await;
            match mgr.get_session(session_id).await {
                Some(session) => (true, session.waiting_for_input),
                None => (false, false),
            }
        }
    };

    let app_for_persist = app.clone();
    let persist = move |cwd: PathBuf, agent: String, profile: String| {
        let app = app_for_persist.clone();
        async move {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
                &snapshot, &cwd, &agent, &profile,
            )
        }
    };

    let restarted_id = Arc::new(Mutex::new(None::<Uuid>));
    let restarted_id_seen = Arc::clone(&restarted_id);
    let app_for_restart = app.clone();
    let replica_for_restart = fixture.replica.clone();
    let restart = move |session_id: Uuid, agent: String, profile: String| {
        let app = app_for_restart.clone();
        let replica = replica_for_restart.clone();
        let restarted_id = Arc::clone(&restarted_id_seen);
        async move {
            assert_eq!(session_id, source_id);
            assert_eq!(agent, "pi");
            assert_eq!(profile, "A");
            {
                let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = state.read().await;
                mgr.destroy_session(session_id).await.unwrap();
            }
            let (new_id, _token) = seed_self_switch_session(
                &app,
                &replica,
                "pi",
                Some("pi"),
                Some(&profile),
                Some(&profile),
            )
            .await;
            {
                let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = state.read().await;
                mgr.mark_idle(new_id).await;
            }
            register_mock_pty_route(&app, new_id);
            *restarted_id.lock().unwrap() = Some(new_id);
            Ok(new_id.to_string())
        }
    };

    let injected_prompts = Arc::new(Mutex::new(Vec::<(Uuid, String)>::new()));
    let injected_prompts_seen = Arc::clone(&injected_prompts);
    let app_for_inject = app.clone();
    let inject = move |session_id: Uuid, prompt: String| {
        let app = app_for_inject.clone();
        let injected_prompts = Arc::clone(&injected_prompts_seen);
        async move {
            injected_prompts
                .lock()
                .unwrap()
                .push((session_id, prompt.clone()));
            crate::pty::inject::inject_text_into_session(&app, session_id, &prompt).await
        }
    };
    let boundaries = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let boundaries_seen = Arc::clone(&boundaries);
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let boundaries = Arc::clone(&boundaries_seen);
        async move {
            boundaries.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        source_id,
        fixture.replica.clone(),
        target_agent,
        target_profile,
        None,
        SELF_HANDOFF_FLAVOR_SWITCH,
        Arc::clone(&pending),
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(30),
        session_state,
        persist,
        restart,
        inject,
        note_boundary,
    )
    .await;

    let target_id = restarted_id
        .lock()
        .unwrap()
        .expect("restart seam must return the reseeded Pi session id");
    assert_ne!(target_id, source_id);
    assert_eq!(*state_ids.lock().unwrap(), vec![source_id, target_id]);
    let prompts = injected_prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].0, target_id);
    assert!(prompts[0].1.contains("self-clear/"), "{}", prompts[0].1);
    assert_eq!(
        mock_pty_writes_for(&app, target_id),
        vec![
            prompts[0].1.as_bytes().to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec()
        ]
    );
    assert_eq!(
        *boundaries.lock().unwrap(),
        vec![(target_id, SelfClearBoundary::ContentInjected)]
    );
    let manager = {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = state.read().await;
        guard.clone()
    };
    assert!(manager.get_session(source_id).await.is_none());
    let target = manager.get_session(target_id).await.unwrap();
    assert_eq!(target.shell, "pi");
    assert_eq!(target.agent_id.as_deref(), Some("pi"));
    assert!(pending.0.lock().unwrap().is_empty());
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.replica.join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["tooling"]["currentCodingAgent"], "pi");
    assert_eq!(saved["tooling"]["profile"], "A");
}

#[tokio::test]
async fn self_switch_pi_source_none_agent_id_omitted_target_resolves_pi_via_replica() {
    // A real Pi session may spawn with agent_id = None. With no --coding-agent
    // and no live agent_id, target resolution takes the third arm
    // (read_replica_current_coding_agent), which must yield `pi`.
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    {
        let settings = app.state::<SettingsState>();
        settings
            .write()
            .await
            .agents
            .push(wake_agent("pi", "Pi", "pi"));
    }
    {
        let settings = app.state::<SettingsState>();
        let snapshot = settings.read().await.clone();
        crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
            &snapshot,
            &fixture.replica,
            "pi",
            "A",
        )
        .unwrap();
    }
    let (session_id, token) =
        seed_self_switch_session(&app, &fixture.replica, "pi", None, None, None).await;
    seed_self_handoff(&fixture.replica);
    let (path, message) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-pi-none-agent",
        "rid-ss-pi-none-agent",
        Some(token.to_string()),
        None,
        None,
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::new();

    poller
        .handle_self_handoff_switch(&app, &path, &message, false)
        .await
        .unwrap();
    let response = read_response_json(&fixture.replica, "rid-ss-pi-none-agent").unwrap();
    assert_eq!(response["status"], "queued");
    assert_eq!(response["target_coding_agent"], "pi");
    assert_eq!(response["target_profile"], "A");
    assert!(pending_self_clear_contains(&app, session_id).await);
}

#[tokio::test]
async fn handle_self_switch_pending_alias_reports_already_queued() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    let (session_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "codex",
        Some("codex"),
        None,
        Some("A"),
    )
    .await;
    seed_self_handoff(&fixture.replica);
    std::fs::write(fixture.replica.join("SELF-FORGET.md"), "new topic").unwrap();
    {
        let pending = app.state::<Arc<crate::PendingSelfClear>>();
        pending
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id);
    }

    let (path, msg) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-alias",
        "rid-ss-alias",
        Some(token.to_string()),
        None,
        None,
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::new();
    poller
        .handle_self_handoff_switch(&app, &path, &msg, false)
        .await
        .unwrap();

    let response = read_response_json(&fixture.replica, "rid-ss-alias").unwrap();
    assert_eq!(response["status"], "already_queued");
    assert_eq!(response["target_coding_agent"], "codex");
    assert_eq!(response["target_profile"], "A");
    assert_eq!(pending_self_clear_len(&app).await, 1);
    assert!(
        fixture.replica.join("SELF-FORGET.md").is_file(),
        "already_queued must not archive a new SELF-FORGET.md"
    );
}

#[tokio::test]
async fn handle_self_switch_rejects_origin_before_handoff_gate() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    let (_session_id, token) = seed_self_switch_session(
        &app,
        &fixture._origin,
        "codex",
        Some("codex"),
        None,
        Some("A"),
    )
    .await;

    let (path, msg) = build_self_switch_message(
        &fixture._origin,
        "msg-ss-origin",
        "rid-ss-origin",
        Some(token.to_string()),
        None,
        None,
        "proj-a/dev-rust",
    );
    let poller = MailboxPoller::new();
    poller
        .handle_self_handoff_switch(&app, &path, &msg, false)
        .await
        .unwrap();

    let reason = read_reject_reason(&fixture._origin, "msg-ss-origin").unwrap();
    assert!(reason.contains("Room replica"), "{reason}");
    assert!(!reason.contains("SELF-HANDOFF"), "{reason}");
    assert_eq!(pending_self_clear_len(&app).await, 0);
}

#[tokio::test]
async fn handle_self_switch_rejects_fake_replica_before_handoff_gate() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let fake = temp.path().join("__agent_fake");
    std::fs::create_dir_all(&fake).unwrap();
    let (_session_id, token) =
        seed_self_switch_session(&app, &fake, "codex", Some("codex"), None, Some("A")).await;

    let (path, msg) = build_self_switch_message(
        &fake,
        "msg-ss-fake",
        "rid-ss-fake",
        Some(token.to_string()),
        None,
        None,
        "proj-a:wg-1-dev-team/fake",
    );
    let poller = MailboxPoller::new();
    poller
        .handle_self_handoff_switch(&app, &path, &msg, false)
        .await
        .unwrap();

    let reason = read_reject_reason(&fake, "msg-ss-fake").unwrap();
    assert!(reason.contains("configured Room replica"), "{reason}");
    assert!(!reason.contains("SELF-HANDOFF"), "{reason}");
    assert_eq!(pending_self_clear_len(&app).await, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn p22_scan_order_first_hit() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("p22-first");
    let replica = base.join(".ac").join("wg-22-p22").join("__agent_p22-agent");
    std::fs::create_dir_all(&replica).unwrap();
    for child in ["child-a", "child-z"] {
        std::fs::create_dir(base.join(child)).unwrap();
    }
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    app.state::<SettingsState>().write().await.project_paths =
        vec![base.to_string_lossy().into_owned()];
    let poller = MailboxPoller::new();
    assert_eq!(
        poller.resolve_repo_path("wg-22-p22/p22-agent", &app).await,
        Some(replica.to_string_lossy().into_owned())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn p22_qualified_project() {
    let temp = tempfile::tempdir().unwrap();
    let alpha = temp.path().join("p22-alpha");
    let beta = temp.path().join("p22-beta");
    let relative = PathBuf::from(".ac")
        .join("wg-22-p22")
        .join("__agent_p22-agent");
    std::fs::create_dir_all(alpha.join(&relative)).unwrap();
    std::fs::create_dir_all(beta.join(&relative)).unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    app.state::<SettingsState>().write().await.project_paths = vec![
        beta.to_string_lossy().into_owned(),
        temp.path().to_string_lossy().into_owned(),
    ];
    let poller = MailboxPoller::new();
    assert_eq!(
        poller
            .resolve_repo_path("p22-alpha:wg-22-p22/p22-agent", &app)
            .await,
        Some(alpha.join(&relative).to_string_lossy().into_owned())
    );
    assert_eq!(
        poller
            .resolve_repo_path("p22-beta:wg-22-p22/p22-agent", &app)
            .await,
        Some(beta.join(&relative).to_string_lossy().into_owned())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn p22_cross_project_ambiguity() {
    let temp = tempfile::tempdir().unwrap();
    let alpha = temp.path().join("p22-alpha");
    let beta = temp.path().join("p22-beta");
    for project in [&alpha, &beta] {
        std::fs::create_dir_all(
            project
                .join(".ac")
                .join("wg-22-p22")
                .join("__agent_p22-agent"),
        )
        .unwrap();
    }
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    app.state::<SettingsState>().write().await.project_paths = vec![
        alpha.to_string_lossy().into_owned(),
        beta.to_string_lossy().into_owned(),
    ];
    assert_eq!(
        MailboxPoller::new()
            .resolve_repo_path("wg-22-p22/p22-agent", &app)
            .await,
        None
    );
}
