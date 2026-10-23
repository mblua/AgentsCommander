use super::*;

#[test]
fn settings_draft_events_flag_agent_base_command_edit() {
    // #592/#597 - a bare agent base-command edit changes the effective-command
    // drift hash, so save_settings_draft must emit coding_agent_profiles_updated
    // even though the profile cells and envs are untouched. This is the bug the
    // user hit editing `claude` -> `claude-amp -c ...` in Settings -> Coding Agents.
    let before = settings_with_single_agent();
    let mut after = settings_with_single_agent();
    after.agents[0].command = "codex --resume".to_string();

    let events = super::settings_draft_update_events(&before, &after);
    assert!(
        events.profiles_changed,
        "an agent base-command edit must flag a profiles refresh"
    );
    assert!(events.env_agent_ids.is_empty(), "no env changed");
}

#[test]
fn id_hash_activation_events_ignore_only_cell_metadata() {
    let mut before = settings_with_single_agent();
    crate::config::settings::validate_and_repair_settings(&mut before).unwrap();
    before
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash_debug = Some(vec!["old".into()]);
    let mut metadata = before.clone();
    metadata.agents[0].id_hash_debug = Some(vec!["forged-base".into()]);
    metadata
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash_debug = Some(vec!["forged-pair".into()]);
    let debug_events = super::settings_draft_update_events(&before, &metadata);
    assert!(!debug_events.profiles_changed);
    assert!(debug_events.env_agent_ids.is_empty());
    metadata
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash = Some("client-metadata".into());
    let events = super::settings_draft_update_events(&before, &metadata);
    assert!(!events.profiles_changed);
    assert!(events.env_agent_ids.is_empty());
    assert_ne!(before.coding_agent_profiles, metadata.coding_agent_profiles);
    for change in [
        "command",
        "env",
        "enabled",
        "notes",
        "preflight",
        "timeout",
        "cell",
        "agent",
        "default",
        "label",
        "slots",
        "schema",
        "identity",
        "home",
    ] {
        let mut after = metadata.clone();
        let profiles = &mut after.coding_agent_profiles;
        let cell = profiles
            .profiles_by_agent
            .get_mut("agent-0")
            .unwrap()
            .get_mut("A")
            .unwrap();
        match change {
            "command" => cell.command.push_str("--real"),
            "env" => {
                cell.env.insert("K".into(), "V".into());
            }
            "enabled" => cell.enabled = !cell.enabled,
            "notes" => cell.notes.push_str("real"),
            "preflight" => cell.preflight = Some("real".into()),
            "timeout" => cell.preflight_timeout_seconds = Some(99),
            "cell" => {
                profiles
                    .profiles_by_agent
                    .get_mut("agent-0")
                    .unwrap()
                    .remove("A");
            }
            "agent" => {
                profiles
                    .profiles_by_agent
                    .insert("new".into(), BTreeMap::new());
            }
            "default" => {
                profiles
                    .default_profile_by_agent
                    .insert("agent-0".into(), "B".into());
            }
            "label" => {
                profiles.profile_labels_by_agent.insert(
                    "agent-0".into(),
                    BTreeMap::from([("A".into(), "real".into())]),
                );
            }
            "slots" => {
                profiles.profile_slots.clear();
            }
            "schema" => profiles.schema_version += 1,
            "identity" => {
                profiles
                    .identity_by_agent
                    .insert("agent-0".into(), "real".into());
            }
            "home" => {
                profiles
                    .codex_home_key_by_agent
                    .insert("agent-0".into(), "real".into());
            }
            _ => unreachable!(),
        }
        let events = super::settings_draft_update_events(&before, &after);
        assert!(events.profiles_changed, "{change}");
        assert!(events.env_agent_ids.is_empty(), "{change}");
    }
}

#[test]
fn settings_draft_events_quiet_for_non_drift_field_edit() {
    // A non-drift field (label) changing must NOT emit a profiles refresh, so the
    // fix does not over-fire on unrelated settings saves.
    let before = settings_with_single_agent();
    let mut after = settings_with_single_agent();
    after.agents[0].label = "Renamed".to_string();

    let events = super::settings_draft_update_events(&before, &after);
    assert!(!events.profiles_changed, "a label edit is not drift");
    assert!(events.env_agent_ids.is_empty());
}

#[test]
fn settings_draft_events_flag_agent_env_edit() {
    // Regression guard: the pre-existing env path still flags the agent id (the
    // base env is also a drift-hash input, surfaced via the env event).
    let mut before = settings_with_single_agent();
    crate::config::settings::validate_and_repair_settings(&mut before).unwrap();
    let mut after = before.clone();
    after.agents[0].envs = vec![CodingAgentEnv {
        key: "FOO".to_string(),
        value: "bar".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];

    after
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash = Some("derived-after-env-edit".into());
    after
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash_debug = Some(vec!["forged-env-debug".into()]);
    let events = super::settings_draft_update_events(&before, &after);
    assert_eq!(events.env_agent_ids, vec!["agent-0".to_string()]);
    assert!(
        !events.profiles_changed,
        "env-only edit is not a profiles change"
    );
}

#[tokio::test]
async fn restart_failure_classification_depends_on_session_survival() {
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "codex".to_string(),
            Vec::new(),
            "C:/work/repo".to_string(),
            Some("codex".to_string()),
            Some("Codex".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let session_mgr = Arc::new(RwLock::new(manager));

    let (survived_code, survived_destroyed) =
        super::classify_restart_failure(&session_mgr, session.id).await;
    assert_eq!(survived_code, "restartFailed");
    assert!(!survived_destroyed);

    let (missing_code, missing_destroyed) =
        super::classify_restart_failure(&session_mgr, uuid::Uuid::new_v4()).await;
    assert_eq!(missing_code, "destroyedButNotRecreated");
    assert!(missing_destroyed);
}

#[tokio::test]
async fn invalid_env_settings_update_leaves_live_settings_unchanged() {
    let mut original = settings_with_single_agent();
    original.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    original.agents[0].isolated_home = true;
    let state = state_for(original.clone());

    let err = persist_coding_agent_env_settings_update(
        &state,
        "agent-0",
        vec![CodingAgentEnv {
            key: "AGENTSCOMMANDER_TOKEN".to_string(),
            value: "bad".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }],
        false,
    )
    .await
    .unwrap_err();

    assert!(err.contains("reserved by AgentsCommander"), "{err}");
    let live = state.read().await;
    assert_eq!(live.agents[0].envs, original.agents[0].envs);
    assert_eq!(
        live.agents[0].isolated_home,
        original.agents[0].isolated_home
    );
}

#[tokio::test]
async fn invalid_profile_settings_update_leaves_live_settings_unchanged() {
    let original = settings_with_single_agent();
    let mut invalid_profiles = original.coding_agent_profiles.clone();
    invalid_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([("AGENTSCOMMANDER_ROOT".to_string(), "bad".to_string())]),
                notes: String::new(),
            },
        );
    let state = state_for(original.clone());

    let err = persist_coding_agent_profiles_update(&state, invalid_profiles)
        .await
        .unwrap_err();

    assert!(err.contains("reserved by AgentsCommander"), "{err}");
    let live = state.read().await;
    assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
}

#[tokio::test]
async fn invalid_settings_draft_transaction_leaves_generic_profiles_and_env_unchanged() {
    let mut original = settings_with_single_agent();
    original.sidebar_style = "noir-minimal".to_string();
    original.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    original.agents[0].isolated_home = true;
    original
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "B".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "codex --current-profile".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    let state = state_for(original.clone());

    let mut draft = original.clone();
    draft.sidebar_style = "command-center".to_string();
    draft.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "draft".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    draft.agents[0].isolated_home = false;
    draft
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "C".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "codex --draft-profile".to_string(),
                env: BTreeMap::from([("AGENTSCOMMANDER_TOKEN".to_string(), "bad".to_string())]),
                notes: String::new(),
            },
        );

    let err = persist_settings_draft_update_with_saver(
        &state,
        draft,
        |_| -> Result<AppSettings, String> { panic!("save must not run") },
    )
    .await
    .unwrap_err();

    assert!(err.contains("reserved by AgentsCommander"), "{err}");
    let live = state.read().await;
    assert_eq!(live.sidebar_style, original.sidebar_style);
    assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
    assert_eq!(live.agents[0].envs, original.agents[0].envs);
    assert_eq!(
        live.agents[0].isolated_home,
        original.agents[0].isolated_home
    );
}

#[tokio::test]
async fn settings_draft_transaction_save_failure_leaves_live_settings_unchanged() {
    let mut original = settings_with_single_agent();
    original.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    original.agents[0].isolated_home = true;
    let state = state_for(original.clone());

    let mut draft = original.clone();
    draft.sidebar_style = "command-center".to_string();
    draft.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "draft".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    draft.agents[0].isolated_home = false;

    let err = persist_settings_draft_update_with_saver(&state, draft, |_| {
        Err("simulated save failure".to_string())
    })
    .await
    .unwrap_err();

    assert_eq!(err, "simulated save failure");
    let live = state.read().await;
    assert_eq!(live.sidebar_style, original.sidebar_style);
    assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
    assert_eq!(live.agents[0].envs, original.agents[0].envs);
    assert_eq!(
        live.agents[0].isolated_home,
        original.agents[0].isolated_home
    );
}

async fn assert_narrow_setter_save_failure_rolls_back(
    field_name: &'static str,
    mutate_candidate: impl FnOnce(&mut AppSettings) + Send,
    assert_candidate: impl FnOnce(&AppSettings) + Send,
) {
    let mut original = settings_with_single_agent();
    original.sounds_enabled = true;
    original.theme_light = false;
    let state = state_for(original.clone());
    let expected_err = format!("simulated save failure for {field_name}");

    let err = persist_narrow_settings_update_with_saver(
        &state,
        |candidate| {
            mutate_candidate(candidate);
            assert_candidate(candidate);
        },
        |_| Err(expected_err.clone()),
    )
    .await
    .unwrap_err();

    assert_eq!(err, expected_err);
    let live = state.read().await;
    assert_eq!(live.sounds_enabled, original.sounds_enabled, "{field_name}");
    assert_eq!(live.theme_light, original.theme_light, "{field_name}");
}

#[tokio::test]
async fn narrow_settings_setter_save_failure_leaves_live_settings_unchanged() {
    assert_narrow_setter_save_failure_rolls_back(
        "sounds_enabled",
        |candidate| candidate.sounds_enabled = false,
        |candidate| assert!(!candidate.sounds_enabled),
    )
    .await;
    assert_narrow_setter_save_failure_rolls_back(
        "theme_light",
        |candidate| candidate.theme_light = true,
        |candidate| assert!(candidate.theme_light),
    )
    .await;
}
