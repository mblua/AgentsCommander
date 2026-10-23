use super::*;

#[test]
fn resolve_root_agent_command_prefers_valid_explicit_agent() {
    let settings = test_settings();

    let (shell, args, agent_id, label) =
        resolve_root_agent_command(&settings, Some("codex"), Some("claude")).unwrap();

    assert_eq!(shell, "codex");
    assert!(args.is_empty());
    assert_eq!(agent_id.as_deref(), Some("codex"));
    assert_eq!(label.as_deref(), Some("Codex"));
}

#[test]
fn resolve_root_agent_command_uses_last_coding_agent_when_explicit_missing() {
    let settings = test_settings();

    let (shell, _args, agent_id, label) =
        resolve_root_agent_command(&settings, None, Some("codex")).unwrap();

    assert_eq!(shell, "codex");
    assert_eq!(agent_id.as_deref(), Some("codex"));
    assert_eq!(label.as_deref(), Some("Codex"));
}

#[test]
fn resolve_root_agent_command_falls_back_to_first_configured_agent() {
    let settings = test_settings();

    let (shell, _args, agent_id, label) =
        resolve_root_agent_command(&settings, Some("stale"), Some("also-stale")).unwrap();

    assert_eq!(shell, "claude");
    assert_eq!(agent_id.as_deref(), Some("claude"));
    assert_eq!(label.as_deref(), Some("Claude Code"));
}

#[test]
fn resolve_root_agent_command_rejects_default_shell_without_agents() {
    let mut settings = AppSettings {
        default_shell: "pwsh".to_string(),
        default_shell_args: vec!["-NoLogo".to_string()],
        ..AppSettings::default()
    };
    settings.agents.clear();

    let err = resolve_root_agent_command(&settings, Some("stale"), Some("also-stale")).unwrap_err();

    assert!(err.contains("No resolvable coding agent"));
}

fn write_current_coding_agent(dir: &std::path::Path, agent_id: &str) {
    std::fs::write(
        dir.join("config.json"),
        format!(r#"{{"tooling":{{"currentCodingAgent":"{}"}}}}"#, agent_id),
    )
    .unwrap();
}

// #537 read-side: a freshly assigned currentCodingAgent must win over the
// agent the live session was launched with (stored_agent_id) when the restart
// carries no explicit agent (the plain restart button passes None).
#[test]
fn restart_honors_current_coding_agent_when_no_explicit_request() {
    let settings = test_settings();
    let tmp = tempfile::tempdir().unwrap();
    write_current_coding_agent(tmp.path(), "codex");

    let selected = resolve_restart_selected_agent_id(
        &settings,
        &tmp.path().to_string_lossy(),
        None,
        Some("claude"),
    );

    assert_eq!(selected.as_deref(), Some("codex"));
}

#[test]
fn restart_explicit_agent_overrides_current_coding_agent() {
    let settings = test_settings();
    let tmp = tempfile::tempdir().unwrap();
    write_current_coding_agent(tmp.path(), "codex");

    let selected = resolve_restart_selected_agent_id(
        &settings,
        &tmp.path().to_string_lossy(),
        Some("claude"),
        Some("codex"),
    );

    assert_eq!(selected.as_deref(), Some("claude"));
}

#[test]
fn restart_ignores_unconfigured_current_coding_agent_and_keeps_stored() {
    let settings = test_settings();
    let tmp = tempfile::tempdir().unwrap();
    write_current_coding_agent(tmp.path(), "ghost");

    let selected = resolve_restart_selected_agent_id(
        &settings,
        &tmp.path().to_string_lossy(),
        None,
        Some("claude"),
    );

    assert_eq!(selected.as_deref(), Some("claude"));
}

// Root agents and plain terminals have no currentCodingAgent → behavior
// unchanged: fall straight through to the stored launch agent.
#[test]
fn restart_falls_back_to_stored_when_no_current_coding_agent() {
    let settings = test_settings();
    let tmp = tempfile::tempdir().unwrap();

    let selected = resolve_restart_selected_agent_id(
        &settings,
        &tmp.path().to_string_lossy(),
        None,
        Some("claude"),
    );

    assert_eq!(selected.as_deref(), Some("claude"));
}

#[test]
fn restart_requested_profile_prefers_explicit_then_stored() {
    assert_eq!(
        effective_restart_requested_profile(Some("C".to_string()), Some("B".to_string())),
        Some("C".to_string())
    );
    assert_eq!(
        effective_restart_requested_profile(None, Some("B".to_string())),
        Some("B".to_string())
    );
    assert_eq!(effective_restart_requested_profile(None, None), None);
}

#[test]
fn build_configured_agent_spawn_for_cwd_honors_requested_profile() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = test_settings();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "C".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                // #597 - the cell holds params only; they append to the agent
                // base command (`codex`) to launch `codex --profile-c`.
                command: "--profile-c".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    let cwd = temp.path().to_string_lossy().to_string();
    let spawn =
        super::super::build_configured_agent_spawn_for_cwd(&settings, "codex", &cwd, Some("C"))
            .unwrap()
            .unwrap();

    assert_eq!(spawn.profile_resolution.requested_profile, "C");
    assert_eq!(spawn.profile_resolution.effective_profile, "C");
    assert_eq!(spawn.shell_args, vec!["--profile-c".to_string()]);
}

// #592 - compute_profile_outdated integration: loaded hash vs current config.
pub(super) fn make_info(
    cwd: &str,
    agent_id: Option<&str>,
    requested_profile: Option<&str>,
    loaded_hash: Option<String>,
) -> SessionInfo {
    SessionInfo {
        id: "00000000-0000-0000-0000-000000000000".to_string(),
        name: "s".to_string(),
        shell: "codex".to_string(),
        shell_args: Vec::new(),
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: "2026-06-21T00:00:00Z".to_string(),
        working_directory: cwd.to_string(),
        status: SessionStatus::Running,
        waiting_for_input: false,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: agent_id.map(str::to_string),
        agent_label: None,
        git_repos: Vec::new(),
        workgroup_task: None,
        is_coordinator: false,
        is_root_agent: false,
        token: "t".to_string(),
        agent_kind: None,
        requested_profile: requested_profile.map(str::to_string),
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        profile_content_hash: loaded_hash,
        trusted_configured_spawn: false,
        profile_outdated: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

#[test]
fn compute_profile_outdated_flips_when_effective_cell_changes() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();

    let mut settings = test_settings();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--v1".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    // #597 - the hash the session launched with: agent base + cell A params,
    // composed via the SAME helpers the spawn path uses. The codex borrow ends
    // with this block, before the get_mut mutation below.
    let loaded = {
        let codex = settings.agents.iter().find(|a| a.id == "codex").unwrap();
        crate::config::agent_command::profile_content_hash(
            &crate::config::agent_command::compose_effective_command(&codex.command, "--v1"),
            &crate::config::agent_command::raw_merged_profile_env(codex, &BTreeMap::new()),
        )
    };
    let mut info = make_info(&cwd, Some("codex"), Some("A"), Some(loaded));

    // Config unchanged -> not outdated.
    assert!(!compute_profile_outdated(&settings, &info));

    // Edit the effective cell params -> outdated.
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("codex")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .command = "--v2".to_string();
    assert!(compute_profile_outdated(&settings, &info));

    // A plain-shell session (no agent) never drifts.
    info.agent_id = None;
    assert!(!compute_profile_outdated(&settings, &info));
}

#[test]
fn compute_profile_outdated_flips_on_agent_base_command_edit() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();

    let mut settings = test_settings();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--v1".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    // #597 - stamp loaded from agent base `codex` + cell `--v1`.
    let loaded = {
        let codex = settings.agents.iter().find(|a| a.id == "codex").unwrap();
        crate::config::agent_command::profile_content_hash(
            &crate::config::agent_command::compose_effective_command(&codex.command, "--v1"),
            &crate::config::agent_command::raw_merged_profile_env(codex, &BTreeMap::new()),
        )
    };
    let info = make_info(&cwd, Some("codex"), Some("A"), Some(loaded));

    // Base unchanged -> not outdated.
    assert!(!compute_profile_outdated(&settings, &info));

    // Edit the agent base command -> outdated (the base is now in the hash).
    settings
        .agents
        .iter_mut()
        .find(|a| a.id == "codex")
        .unwrap()
        .command = "codex-next".into();
    assert!(compute_profile_outdated(&settings, &info));
}

#[test]
fn resolve_agent_command_preserves_args() {
    let mut settings = test_settings();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .command = "codex --yolo".to_string();

    let (shell, args, label) = resolve_agent_command("codex", &settings).unwrap();

    assert_eq!(shell, "codex");
    assert_eq!(args, vec!["--yolo".to_string()]);
    assert_eq!(label.as_deref(), Some("Codex"));
}

#[test]
fn resolve_agent_command_rejects_invalid_selected_command() {
    let mut settings = test_settings();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .command = "codex \"unterminated".to_string();

    let err = resolve_agent_command("codex", &settings).unwrap_err();

    assert!(err.contains("selected agent 'codex'"));
    assert!(err.contains("agent id 'codex'"));
    assert!(err.contains("unclosed double quote"));
}

#[test]
fn resolve_root_agent_command_preserves_args() {
    let mut settings = test_settings();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .command = "codex --yolo".to_string();

    let (shell, args, agent_id, label) =
        resolve_root_agent_command(&settings, Some("codex"), None).unwrap();

    assert_eq!(shell, "codex");
    assert_eq!(args, vec!["--yolo".to_string()]);
    assert_eq!(agent_id.as_deref(), Some("codex"));
    assert_eq!(label.as_deref(), Some("Codex"));
}

#[test]
fn resolve_root_agent_command_rejects_invalid_last_coding_agent_command() {
    let mut settings = test_settings();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .command = "codex \"unterminated".to_string();

    let err = resolve_root_agent_command(&settings, None, Some("codex")).unwrap_err();

    assert!(err.contains("root lastCodingAgent 'codex'"));
    assert!(err.contains("agent id 'codex'"));
    assert!(err.contains("unclosed double quote"));
}

#[test]
fn existing_root_classifier_discards_live_record_without_pty() {
    assert_eq!(
        classify_existing_root(&SessionStatus::Running, false),
        ExistingRootAction::DiscardMissingPty
    );
    assert_eq!(
        classify_existing_root(&SessionStatus::Active, true),
        ExistingRootAction::ReuseLive
    );
    assert_eq!(
        classify_existing_root(&SessionStatus::Exited(0), false),
        ExistingRootAction::WakeDormant
    );
}

#[tokio::test]
async fn dormant_root_command_resolution_failure_preserves_existing_session() {
    let session_mgr = SessionManager::new();
    let settings = AppSettings {
        agents: Vec::new(),
        ..AppSettings::default()
    };

    let dormant = {
        let session = session_mgr
            .create_session(
                "codex".to_string(),
                Vec::new(),
                "C:\\test\\ac-root-agent".to_string(),
                Some("codex".to_string()),
                Some("Codex".to_string()),
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("failed to create dormant root session");
        session_mgr.set_is_root_agent(session.id, true).await;
        session_mgr.mark_exited(session.id, 0).await;
        session
    };

    assert_eq!(
        classify_existing_root(&SessionStatus::Exited(0), false),
        ExistingRootAction::WakeDormant
    );

    let err = resolve_root_agent_command(&settings, Some("stale"), Some("also-stale"))
        .expect_err("replacement command should fail before destroying dormant root");

    assert!(err.contains("No resolvable coding agent"));
    let preserved = session_mgr.get_session(dormant.id).await;
    assert!(matches!(
        preserved.as_ref().map(|s| &s.status),
        Some(SessionStatus::Exited(0))
    ));
    assert!(preserved.is_some_and(|s| s.is_root_agent));
}
