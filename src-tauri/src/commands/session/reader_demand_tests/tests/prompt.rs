use super::*;
use crate::commands::session::{
    build_configured_agent_spawn_for_cwd, coordinator_create_seam, create_session_inner,
    destroy_session_inner, destroy_sessions_with_source, restart_reader_seam,
    restart_session_inner_with_intent, DestructionSource,
};

// ── Test 9: restart transfers reader ownership to the new session UUID ──

#[tokio::test]
async fn issue_3145_restart_barrier_publishes_latest_prompt_in_first_event() {
    use tauri::Listener;
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let old = h
        .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
        .await;
    let manager = h.manager.read().await.clone();
    manager.set_last_prompt(old, "A".into()).await;
    let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = events.clone();
    let listener = h.app.listen("session_created", move |event| {
        captured
            .lock()
            .unwrap()
            .push(serde_json::from_str(event.payload()).unwrap());
    });
    let barrier = restart_reader_seam::install_after_old_release(&old.to_string());
    let app = h.app.handle().clone();
    let sessions = h.manager.clone();
    let pty = h.pty.clone();
    let settings = h
        .app
        .state::<crate::config::settings::SettingsState>()
        .inner()
        .clone();
    let restart = tokio::spawn(async move {
        restart_session_inner_with_intent(
            &app,
            &sessions,
            &pty,
            &settings,
            old,
            None,
            None,
            Some(false),
            true,
            TrustedRestartIntent::User,
            None,
            crate::config::sessions_persistence::default_creation_gate_enforcement(),
        )
        .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        barrier.reached.notified(),
    )
    .await
    .unwrap();
    manager.set_last_prompt(old, "B".into()).await;
    barrier.release.notify_one();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), restart)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.last_prompt.as_deref(), Some("B"));
    assert_eq!(
        manager.list_sessions().await[0].last_prompt.as_deref(),
        Some("B")
    );
    assert_eq!(
        crate::config::sessions_persistence::snapshot_sessions(&manager).await[0]
            .last_prompt
            .as_deref(),
        Some("B")
    );
    let observed = events.lock().unwrap().clone();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0]["id"], result.id);
    assert_eq!(observed[0]["lastPrompt"], "B");
    h.app.unlisten(listener);
    h.release_all(Uuid::parse_str(&result.id).unwrap()).await;
    h.close().await;
}

#[tokio::test]
async fn issue_3145_restart_fresh_durable_and_failed_spawn_never_inherit() {
    for mode in [0, 1, 2] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h
            .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
            .await;
        let manager = h.manager.read().await.clone();
        manager.set_last_prompt(old, "A".into()).await;
        if mode == 1 {
            manager.set_start_fresh_on_restore(old, true).await;
        }
        if mode == 2 {
            h.backend.fail_spawn(2);
        }
        let result = h
            .restart_with(old, if mode == 0 { Some(true) } else { Some(false) })
            .await;
        if mode == 2 {
            assert!(result.is_err());
            assert!(manager.list_sessions().await.is_empty());
        } else {
            let result = result.unwrap();
            assert_eq!(result.last_prompt, None);
            h.release_all(Uuid::parse_str(&result.id).unwrap()).await;
        }
        h.close().await;
    }
}

#[tokio::test]
async fn issue_3145_restart_changed_agent_requested_or_effective_profile_abstains() {
    for mode in [0, 1, 2] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h
            .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
            .await;
        let manager = h.manager.read().await.clone();
        manager.set_last_prompt(old, "A".into()).await;
        let row = manager.get_session(old).await.unwrap();
        if mode != 0 {
            manager
                .set_profile_metadata(
                    old,
                    if mode == 1 {
                        None
                    } else {
                        row.requested_profile.clone()
                    },
                    if mode == 2 {
                        Some("previous-effective".into())
                    } else {
                        row.effective_profile.clone()
                    },
                    Vec::new(),
                    false,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
        }
        let settings = h.app.state::<crate::config::settings::SettingsState>();
        let result = restart_session_inner_with_intent(
            h.app.handle(),
            &h.manager,
            &h.pty,
            settings.inner(),
            old,
            (mode == 0).then(|| "claude".to_string()),
            row.requested_profile,
            Some(false),
            true,
            TrustedRestartIntent::User,
            None,
            crate::config::sessions_persistence::default_creation_gate_enforcement(),
        )
        .await
        .unwrap();
        assert_eq!(result.last_prompt, None, "incompatible domain {mode}");
        h.release_all(Uuid::parse_str(&result.id).unwrap()).await;
        h.close().await;
    }
}

#[tokio::test]
async fn issue_3145_restart_mirror_forced_fresh_never_inherits() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let old = h
        .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
        .await;
    h.manager
        .read()
        .await
        .set_last_prompt(old, "A".into())
        .await;
    let cwd = crate::path_utils::normalize_windows_verbatim_path(
        &fixture.coordinator_path().to_string_lossy(),
    );
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(Default::default()));
    clocks.lock().unwrap().mark_start_fresh(
        &crate::config::teams::agent_fqn_from_path(&cwd),
        chrono::Utc::now(),
    );
    assert!(h.app.manage(clocks));
    coordinator_create_seam::force(&cwd);
    let result = h.restart(old).await.unwrap();
    coordinator_create_seam::unforce(&cwd);
    assert_eq!(result.last_prompt, None);
    assert!(
        h.manager
            .read()
            .await
            .get_session(Uuid::parse_str(&result.id).unwrap())
            .await
            .unwrap()
            .start_fresh_on_restore
    );
    h.release_all(Uuid::parse_str(&result.id).unwrap()).await;
    h.close().await;
}

#[tokio::test]
async fn issue_3145_manual_auto_close_reload_retains_input_during_teardown() {
    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    std::fs::create_dir_all(&root).expect("Root fixture creation succeeds");
    for source in [DestructionSource::ManualClose, DestructionSource::AutoClose] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h
            .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
            .await;
        let manager = h.manager.read().await.clone();
        manager.set_last_prompt(old, "A".into()).await;
        let barrier = restart_reader_seam::install_after_old_release(&format!("destroy:{old}"));
        let app = h.app.handle().clone();
        let destroy = tokio::spawn(async move {
            destroy_sessions_with_source(&app, vec![old], source, false).await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            barrier.reached.notified(),
        )
        .await
        .unwrap();
        manager.set_last_prompt(old, "B".into()).await;
        barrier.release.notify_one();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), destroy)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(outcome.succeeded(old));
        assert!(manager.list_sessions().await.is_empty());
        let checkpoint = tempfile::tempdir().unwrap();
        crate::config::sessions_persistence::persist_current_state_to_dir_result(
            &manager,
            checkpoint.path(),
        )
        .await
        .unwrap();
        let next = restart_harness(&fixture);
        let next_manager = next.manager.read().await.clone();
        crate::config::sessions_persistence::initialize_prompt_retention_in_dir(
            &next_manager,
            checkpoint.path(),
        )
        .await;
        assert!(next_manager.list_sessions().await.is_empty());
        let cwd = fixture.coordinator_path().to_string_lossy().into_owned();
        let spawn = build_configured_agent_spawn_for_cwd(&next.settings, "codex", &cwd, None)
            .unwrap()
            .unwrap();
        let resumed = create_session_inner(
            next.app.handle(),
            &next.manager,
            &next.pty,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            cwd,
            Some("resumed".into()),
            Some("codex".into()),
            None,
            true,
            Vec::new(),
            false,
            Some(spawn),
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .unwrap();
        assert_eq!(resumed.last_prompt.as_deref(), Some("B"));
        next.release_all(Uuid::parse_str(&resumed.id).unwrap())
            .await;
        h.close().await;
        next.close().await;
    }
}

async fn issue_3145_open_claude_without_forced_fresh(
    harness: &RestartHarness,
    cwd: &std::path::Path,
) -> SessionInfo {
    let cwd = cwd.to_string_lossy().into_owned();
    let spawn = build_configured_agent_spawn_for_cwd(&harness.settings, "claude", &cwd, None)
        .unwrap()
        .unwrap();
    create_session_inner(
        harness.app.handle(),
        &harness.manager,
        &harness.pty,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("initial conversation".into()),
        Some("claude".into()),
        None,
        true,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn issue_3145_initial_claude_launch_retains_input_after_close_reload_resume() {
    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    std::fs::create_dir_all(&root).expect("Root fixture creation succeeds");
    for accepted in ["A", ""] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let first = restart_harness(&fixture);
        let projects =
            first.claude_projects_dir(&first.claude_config_dir, fixture.coordinator_path());
        assert!(
            !projects.exists(),
            "first conversation has no resume probe directory"
        );
        let initial =
            issue_3145_open_claude_without_forced_fresh(&first, fixture.coordinator_path()).await;
        let initial_args = initial
            .effective_shell_args
            .as_deref()
            .unwrap_or(&initial.shell_args);
        assert!(!initial_args
            .iter()
            .flat_map(|arg| arg.split_whitespace())
            .map(|arg| arg.trim_matches('"'))
            .any(|arg| arg == "--continue" || arg == "-c"));
        let id = Uuid::parse_str(&initial.id).unwrap();
        let manager = first.manager.read().await.clone();
        let retained = manager.retained_prompt_snapshot().await.unwrap();
        assert_eq!(retained.len(), 1);
        assert!(
            !retained.values().next().unwrap().blocked,
            "AC-3145-006: natural first launch must not poison retention"
        );
        manager.set_last_prompt(id, accepted.into()).await;
        destroy_session_inner(first.app.handle(), id).await.unwrap();
        assert!(manager.list_sessions().await.is_empty());
        let checkpoint = tempfile::tempdir().unwrap();
        crate::config::sessions_persistence::persist_current_state_to_dir_result(
            &manager,
            checkpoint.path(),
        )
        .await
        .unwrap();
        let next = restart_harness(&fixture);
        let next_manager = next.manager.read().await.clone();
        crate::config::sessions_persistence::initialize_prompt_retention_in_dir(
            &next_manager,
            checkpoint.path(),
        )
        .await;
        assert!(next_manager.list_sessions().await.is_empty());
        // Only the provider's pre-existing launch probe is mocked. The PTY,
        // create, close, sidecar reload and publication paths are production.
        std::fs::create_dir_all(
            next.claude_projects_dir(&next.claude_config_dir, fixture.coordinator_path()),
        )
        .unwrap();
        let resumed =
            issue_3145_open_claude_without_forced_fresh(&next, fixture.coordinator_path()).await;
        let resume_args = resumed
            .effective_shell_args
            .as_deref()
            .unwrap_or(&resumed.shell_args);
        assert!(resume_args
            .iter()
            .flat_map(|arg| arg.split_whitespace())
            .any(|arg| arg.trim_matches('"') == "--continue"));
        assert_eq!(
            resumed.last_prompt.as_deref(),
            Some(accepted),
            "AC-3145-006: input survives close and reload into effective resume"
        );
        next.release_all(Uuid::parse_str(&resumed.id).unwrap())
            .await;
        first.close().await;
        next.close().await;
    }
}

#[tokio::test]
async fn issue_3145_configured_external_resume_stays_blocked_after_close_reload() {
    let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
    std::fs::create_dir_all(&root).expect("Root fixture creation succeeds");
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let mut first = restart_harness(&fixture);
    first.set_claude_command("claude --continue").await;
    let initial =
        issue_3145_open_claude_without_forced_fresh(&first, fixture.coordinator_path()).await;
    let id = Uuid::parse_str(&initial.id).unwrap();
    let manager = first.manager.read().await.clone();
    manager.set_last_prompt(id, "unattributed".into()).await;
    assert!(
        manager
            .retained_prompt_snapshot()
            .await
            .unwrap()
            .values()
            .next()
            .unwrap()
            .blocked
    );
    destroy_session_inner(first.app.handle(), id).await.unwrap();
    let checkpoint = tempfile::tempdir().unwrap();
    crate::config::sessions_persistence::persist_current_state_to_dir_result(
        &manager,
        checkpoint.path(),
    )
    .await
    .unwrap();
    let next = restart_harness(&fixture);
    let next_manager = next.manager.read().await.clone();
    crate::config::sessions_persistence::initialize_prompt_retention_in_dir(
        &next_manager,
        checkpoint.path(),
    )
    .await;
    std::fs::create_dir_all(
        next.claude_projects_dir(&next.claude_config_dir, fixture.coordinator_path()),
    )
    .unwrap();
    let resumed =
        issue_3145_open_claude_without_forced_fresh(&next, fixture.coordinator_path()).await;
    assert_eq!(resumed.last_prompt, None);
    next.release_all(Uuid::parse_str(&resumed.id).unwrap())
        .await;
    first.close().await;
    next.close().await;
}
