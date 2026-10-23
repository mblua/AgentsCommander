use super::*;

// #2433 descriptor persistence

fn descriptor_settings() -> AppSettings {
    let agent = |id: &str, command: &str| AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    };
    let cell = |enabled: bool, command: &str| ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        enabled,
        command: command.to_string(),
        env: BTreeMap::new(),
        notes: String::new(),
    };
    let mut settings = AppSettings {
        agents: vec![agent("claude", "Claude --Foo"), agent("codex", "codex")],
        ..AppSettings::default()
    };
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "claude".to_string(),
        BTreeMap::from([
            ("A".to_string(), cell(true, "--a")),
            ("B".to_string(), cell(false, "--b")),
            ("C".to_string(), cell(true, "")),
        ]),
    );
    settings
}

#[test]
fn descriptor_identity_skips_disabled_cells() {
    let settings = descriptor_settings();
    let agent = &settings.agents[0];
    let cells = &settings.coding_agent_profiles.profiles_by_agent["claude"];
    let (command, identity) = coding_agent_descriptor("claude", &settings).unwrap();
    assert_eq!(
        command,
        crate::config::agent_command::canonical_command_text("Claude --Foo")
    );
    let expected: BTreeMap<String, String> = ["A", "C"]
        .iter()
        .map(|l| {
            (
                l.to_string(),
                crate::config::agent_command::cell_identity(agent, &cells[*l]),
            )
        })
        .collect();
    assert_eq!(identity, expected);

    let (_, none) = coding_agent_descriptor("codex", &settings).unwrap();
    assert!(none.is_empty());
    assert!(coding_agent_descriptor("gone", &settings).is_none());
}

/// #2816 (C3) - the state file beside the tracked `path`, after asserting
/// that the tracked file keeps none of the descriptor's state keys.
fn read_state_beside(path: &std::path::Path) -> serde_json::Value {
    let tracked = read_json(path);
    for key in ["lastCodingAgent", "codingAgents"] {
        assert!(tracked["tooling"].get(key).is_none(), "{key} in {path:?}");
    }
    read_json(&path.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
}

fn read_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Seeds both config files the spawn-time write opens; returns their paths.
fn seed_both_configs(repo: &std::path::Path, seed: &serde_json::Value) -> [PathBuf; 2] {
    let instance = repo.join(crate::config::agent_local_dir_name());
    std::fs::create_dir_all(&instance).unwrap();
    let paths = [instance.join("config.json"), repo.join("config.json")];
    for path in &paths {
        std::fs::write(path, serde_json::to_string(seed).unwrap()).unwrap();
    }
    paths
}

#[test]
fn spawn_writes_the_descriptor_for_the_launched_agent_only() {
    let settings = descriptor_settings();
    let temp = tempfile::tempdir().unwrap();
    let other = serde_json::json!({"app": "codex", "lastUsed": "2026-01-01T00:00:00+00:00"});
    let paths = seed_both_configs(
        temp.path(),
        &serde_json::json!({"tooling": {"codingAgents": {"codex": other}}}),
    );

    // Expected values derived here, not through the code under test.
    let agent = &settings.agents[0];
    let cells = &settings.coding_agent_profiles.profiles_by_agent["claude"];
    let digest = |l: &str| crate::config::agent_command::cell_identity(agent, &cells[l]);
    let expected_identity = serde_json::json!({"A": digest("A"), "C": digest("C")});
    let expected_command = crate::config::agent_command::canonical_command_text("Claude --Foo");

    let write = prepare_last_coding_agent_write("claude", None, &settings);
    save_last_coding_agent(&temp.path().to_string_lossy(), "sid", &write).unwrap();

    for path in &paths {
        let value = read_state_beside(path);
        let agents = &value["tooling"]["codingAgents"];
        assert_eq!(value["tooling"]["lastCodingAgent"], "claude", "{path:?}");
        assert_eq!(agents["claude"]["app"], "claude", "{path:?}");
        assert_eq!(agents["claude"]["acSessionId"], "sid", "{path:?}");
        assert_eq!(
            agents["claude"]["command"],
            serde_json::json!(expected_command)
        );
        assert_eq!(agents["claude"]["identity"], expected_identity, "{path:?}");
        assert_eq!(agents["codex"], other, "{path:?}");
    }
}

#[test]
fn spawn_with_an_unknown_agent_keeps_the_existing_descriptor() {
    let settings = descriptor_settings();
    let temp = tempfile::tempdir().unwrap();
    let good = serde_json::json!({
        "app": "Gone", "lastUsed": "2026-01-01T00:00:00+00:00",
        "command": "gone --x", "identity": {"A": "aa"}
    });
    let paths = seed_both_configs(
        temp.path(),
        &serde_json::json!({"tooling": {"codingAgents": {"gone": good}}}),
    );

    let write = prepare_last_coding_agent_write("gone", None, &settings);
    save_last_coding_agent(&temp.path().to_string_lossy(), "sid", &write).unwrap();

    for path in &paths {
        let value = read_state_beside(path);
        let entry = &value["tooling"]["codingAgents"]["gone"];
        assert_eq!(entry["command"], "gone --x", "{path:?}");
        assert_eq!(
            entry["identity"],
            serde_json::json!({"A": "aa"}),
            "{path:?}"
        );
        assert_eq!(entry["app"], "Unknown", "{path:?}");
        assert_eq!(entry["acSessionId"], "sid", "{path:?}");
    }
}

/// #2716 (B3, option C) E33: a restart of a session whose stored agent id is
/// set refuses under the mark instead of coming back as a plain terminal.
#[tokio::test]
async fn a_restart_refuses_instead_of_becoming_a_plain_terminal() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let cwd_text = cwd.path().to_string_lossy().to_string();
    let spawn = super::super::build_configured_agent_spawn_for_cwd(
        &test_settings(),
        "claude",
        &cwd_text,
        None,
    )
    .unwrap()
    .expect("claude is configured");
    let old = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd_text.clone(),
        Some("agent fixture".to_string()),
        Some("claude".to_string()),
        Some(spawn.trusted_agent_label.clone()),
        true,
        Vec::new(),
        true,
        Some(spawn),
        Some(super::super::ResolvedAgentHostShell {
            program: "test-shell".to_string(),
            args: Vec::new(),
        }),
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("create the agent session");
    assert_eq!(
        old.agent_id.as_deref(),
        Some("claude"),
        "fixture: no stored agent"
    );
    let old_id = Uuid::parse_str(&old.id).unwrap();
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    let sidecar = cwd.path().join("agents.30.instance.no-git.json");
    let settings = app.state::<crate::config::settings::SettingsState>();
    settings.write().await.agents_layer = crate::config::settings::AgentsLayerState::Unreadable {
        path: sidecar.clone(),
        reason: "test".to_string(),
    };

    let error = super::super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(true),
        true,
    )
    .await
    .expect_err("the restart must refuse under the mark");

    assert!(
        error.contains(&crate::config::settings::agents_unreadable_session_error(
            &sidecar
        )),
        "{error}"
    );
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        1,
        "a plain-terminal replacement was spawned"
    );
    close_test_coordinator(&app).await;
}

/// #2716 (B3, option C) E32: entry point 1. `create_session` reaches
/// `build_configured_agent_spawn_for_cwd(&cfg, aid, ..)?` before anything
/// else; with the mark set by the real loader it answers notice 3, naming
/// the agents file and without option S's fourth sentence.
#[test]
fn create_session_refuses_under_the_mark() {
    use crate::config::instance_artifacts::{AGENTS_INSTANCE_FILE_NAME, SETTINGS_FILE_NAME};
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join(SETTINGS_FILE_NAME);
    std::fs::write(
        &settings_path,
        r##"{
                "defaultShell": "test-shell",
                "defaultShellArgs": [],
                "agents": [
                    {"id":"claude","label":"Claude Code","command":"claude","color":"#d97706"}
                ]
            }"##,
    )
    .unwrap();
    let sidecar = temp.path().join(AGENTS_INSTANCE_FILE_NAME);
    std::fs::write(&sidecar, "{ not json").unwrap();
    let cfg = crate::config::settings::load_settings_from_path(&settings_path);
    assert_eq!(cfg.agents_layer.unreadable_path(), Some(sidecar.as_path()));
    assert!(cfg.agents.iter().any(|agent| agent.id == "claude"));
    let cwd = temp.path().to_string_lossy().into_owned();

    let error = super::super::build_configured_agent_spawn_for_cwd(&cfg, "claude", &cwd, None)
        .expect_err("entry point 1 must refuse under the mark");

    assert_eq!(
        error,
        crate::config::settings::agents_unreadable_session_error(&sidecar)
    );
    assert!(error.contains(&sidecar.display().to_string()));
    assert!(!error.contains("changes to your sessions are not saved"));

    // Repaired: the same fixture builds the agent spawn.
    std::fs::remove_file(&sidecar).unwrap();
    let cfg = crate::config::settings::load_settings_from_path(&settings_path);
    assert!(cfg.agents_layer.unreadable_path().is_none());
    assert!(
        super::super::build_configured_agent_spawn_for_cwd(&cfg, "claude", &cwd, None)
            .unwrap()
            .is_some()
    );
}
// ---------------------------------------------------------------- #2786 C1
// E14: `currentCodingAgent` and the descriptor come from one merged
// snapshot. The state file's descriptor (`new`) must win over `old`.
#[test]
fn matched_selection_takes_the_descriptor_from_the_state_file() {
    use crate::config::agent_config::load_probe_harness as h;
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", "new");
    let selection = super::super::matched_selection(
        &h::snapshot_settings(),
        &tmp.path().to_string_lossy(),
        None,
        None,
        None,
    );
    assert!(
        matches!(
            &selection,
            super::super::MatchedSelection::Agent { agent_id, .. } if agent_id == "via-new"
        ),
        "{selection:?}"
    );
}
