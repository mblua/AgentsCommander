use super::*;

fn hash_activation_env(value: &str) -> Vec<CodingAgentEnv> {
    vec![CodingAgentEnv {
        key: "MODE".into(),
        value: value.into(),
        source: CodingAgentEnvSource::User,
        enabled: false,
    }]
}

fn hash_activation_state() -> (tempfile::TempDir, PathBuf, SettingsState) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut settings = settings_with_single_agent();
    settings.root_token = Some("hash-test-token".into());
    settings.agents[0].envs = hash_activation_env("live");
    crate::config::settings::normalize_agent_order(&mut settings.agents);
    crate::config::settings::validate_and_repair_settings(&mut settings).unwrap();
    settings
        .coding_agent_profiles
        .codex_home_key_by_agent
        .insert("agent-0".into(), "physical-home".into());
    let mut cell = crate::config::settings::empty_profile_cell();
    cell.command = "--profile-b".into();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .insert("B".into(), cell);
    settings
        .coding_agent_profiles
        .default_profile_by_agent
        .insert("agent-0".into(), "B".into());
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "agent-0".into(),
            BTreeMap::from([("B".into(), "Profile sentinel".into())]),
        );
    // HOME carrier is backend-owned physical state. A caller-only value
    // is intentionally removed by the existing saver when disk has none.
    std::fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
    let written =
        crate::config::settings::save_settings_to_path_preserving_project_paths(&settings, &path)
            .unwrap();
    assert_eq!(
        written
            .coding_agent_profiles
            .codex_home_key_by_agent
            .get("agent-0")
            .map(String::as_str),
        Some("physical-home")
    );
    (
        temp,
        path,
        crate::config::settings::new_settings_state(written),
    )
}

fn hash_activation_assert_written(
    written: &AppSettings,
    reload: &AppSettings,
    expected_envs: &[CodingAgentEnv],
    path: &Path,
) {
    let row = &written.agents[0];
    let physical: Value = serde_json::from_slice(
        &std::fs::read(
            path.with_file_name(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
        )
        .unwrap(),
    )
    .unwrap();
    let physical_agent: AgentConfig =
        serde_json::from_value(physical["agents"][0].clone()).unwrap();
    assert_eq!(
        physical_agent.id_hash,
        Some(crate::config::settings::configuration_identity(
            &physical_agent.command,
            &physical_agent.envs
        ))
    );
    assert_eq!(physical_agent.envs, expected_envs);
    let mut expected_debug = vec![row.command.clone()];
    for env in expected_envs {
        expected_debug.extend([env.key.clone(), env.value.clone()]);
    }
    assert_eq!(row.id_hash_debug, Some(expected_debug.clone()));
    assert_eq!(physical_agent.id_hash_debug, Some(expected_debug));
    assert_eq!(row.id, "agent-0");
    assert_eq!(row.order, Some(0));
    assert_eq!(
        written.coding_agent_profiles.default_profile_by_agent["agent-0"],
        "B"
    );
    assert_eq!(
        written.coding_agent_profiles.profile_labels_by_agent["agent-0"]["B"],
        "Profile sentinel"
    );
    assert_eq!(row.envs, expected_envs);
    assert_eq!(
        row.id_hash,
        Some(crate::config::settings::configuration_identity(
            &row.command,
            expected_envs
        ))
    );
    assert_eq!(
        serde_json::to_value(&reload.agents).unwrap(),
        serde_json::to_value(&written.agents).unwrap()
    );
    assert_eq!(
        written.coding_agent_profiles.codex_home_key_by_agent["agent-0"],
        "physical-home"
    );
}

#[tokio::test]
async fn id_hash_activation_protected_whole_uses_restored_live_env_and_real_saver() {
    let (_temp, path, state) = hash_activation_state();
    let original = state.read().await.clone();
    let mut incoming = original.clone();
    incoming.agents[0].command = "codex extra-character".into();
    incoming.agents[0].envs = hash_activation_env("client-stale");
    incoming.agents[0].id_hash = Some("forged".into());
    incoming
        .coding_agent_profiles
        .codex_home_key_by_agent
        .clear();
    let (written, _) =
        persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();
    let reload = crate::config::settings::load_settings_from_path(&path);
    hash_activation_assert_written(&written, &reload, &original.agents[0].envs, &path);
    assert_eq!(
        serde_json::to_value(&state.read().await.agents).unwrap(),
        serde_json::to_value(&written.agents).unwrap()
    );
    assert_ne!(written.agents[0].id_hash, original.agents[0].id_hash);
    let mut expected_profiles = original.coding_agent_profiles.profiles_by_agent.clone();
    for (letter, cell) in expected_profiles.get_mut("agent-0").unwrap() {
        let mut debug = vec![written.agents[0].command.clone()];
        for env in &original.agents[0].envs {
            debug.extend([env.key.clone(), env.value.clone()]);
        }
        debug.push(cell.command.clone());
        for (key, value) in &cell.env {
            debug.extend([key.clone(), value.clone()]);
        }
        cell.id_hash_debug = Some(debug);
        // Independent preimage from the final command, restored live env and
        // the original payload. Disabled env rows still participate.
        let mut preimage = written.agents[0].command.clone();
        for env in &original.agents[0].envs {
            preimage.push_str(&env.key);
            preimage.push_str(&env.value);
        }
        preimage.push_str(&cell.command);
        for (key, value) in &cell.env {
            preimage.push_str(key);
            preimage.push_str(value);
        }
        let mut chars: Vec<_> = preimage
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|c| !c.is_whitespace())
            .collect();
        chars.sort_unstable();
        cell.id_hash = Some(format!(
            "cid1:{:x}",
            <super::Sha256 as super::Digest>::digest(chars.iter().collect::<String>())
        ));
        assert_eq!(
            written.coding_agent_profiles.profiles_by_agent["agent-0"][letter].id_hash,
            cell.id_hash
        );
    }
    assert_eq!(
        written.coding_agent_profiles.profiles_by_agent,
        expected_profiles
    );
    let before = serde_json::to_value(&original).unwrap();
    let after = serde_json::to_value(&written).unwrap();
    for key in ["quotaSources", "projectPaths", "rootToken"] {
        assert_eq!(after[key], before[key]);
    }
}

#[tokio::test]
async fn id_hash_activation_draft_keeps_edited_env_and_existing_events() {
    let (_temp, path, state) = hash_activation_state();
    let mut draft = state.read().await.clone();
    draft.agents[0].envs = hash_activation_env("draft-edited");
    draft.agents[0].id_hash = Some("forged".into());
    let (written, events) = persist_settings_draft_update_with_saver(&state, draft, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();
    let reload = crate::config::settings::load_settings_from_path(&path);
    hash_activation_assert_written(
        &written,
        &reload,
        &hash_activation_env("draft-edited"),
        &path,
    );
    assert_eq!(
        serde_json::to_value(&state.read().await.agents).unwrap(),
        serde_json::to_value(&written.agents).unwrap()
    );
    assert_eq!(events.env_agent_ids, vec!["agent-0".to_string()]);
    assert!(!events.profiles_changed);
}

#[tokio::test]
async fn id_hash_activation_narrow_env_uses_explicit_replacement_and_written() {
    let (_temp, path, state) = hash_activation_state();
    let mut captured = None;
    super::persist_coding_agent_env_settings_update_with_saver(
        &state,
        "agent-0",
        hash_activation_env("narrow-replacement"),
        true,
        |candidate| {
            let written = crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )?;
            captured = Some(written.clone());
            Ok(written)
        },
    )
    .await
    .unwrap();
    let written = captured.unwrap();
    let reload = crate::config::settings::load_settings_from_path(&path);
    hash_activation_assert_written(
        &written,
        &reload,
        &hash_activation_env("narrow-replacement"),
        &path,
    );
    assert_eq!(
        serde_json::to_value(&state.read().await.agents).unwrap(),
        serde_json::to_value(&written.agents).unwrap()
    );
    assert!(written.agents[0].isolated_home);
    let physical: Value = serde_json::from_slice(
        &std::fs::read(
            path.with_file_name(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        physical["agents"][0]["id_hash"],
        written.agents[0].id_hash.clone().unwrap()
    );
}

#[tokio::test]
async fn id_hash_activation_failed_real_save_never_adopts_whole_draft_or_narrow() {
    for route in ["whole", "draft", "narrow"] {
        let (_temp, path, state) = hash_activation_state();
        let before = serde_json::to_value(&*state.read().await).unwrap();
        let mut payload = state.read().await.clone();
        payload.agents[0].command = "codex edited".into();
        let sidecar =
            path.with_file_name(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
        std::fs::write(&sidecar, b"{ malformed").unwrap();
        let saver = |candidate: &AppSettings| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        };
        let failed = match route {
            "whole" => persist_protected_settings_update_with_saver(&state, payload, saver)
                .await
                .is_err(),
            "draft" => persist_settings_draft_update_with_saver(&state, payload, saver)
                .await
                .is_err(),
            _ => super::persist_coding_agent_env_settings_update_with_saver(
                &state,
                "agent-0",
                hash_activation_env("requested"),
                true,
                saver,
            )
            .await
            .is_err(),
        };
        assert!(failed, "{route}");
        assert_eq!(
            serde_json::to_value(&*state.read().await).unwrap(),
            before,
            "{route}"
        );
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"{ malformed");
    }
}
