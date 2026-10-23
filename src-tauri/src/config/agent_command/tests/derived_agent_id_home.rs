use super::*;

#[test]
fn derived_agent_id_home_legacy_fallback_and_present_leaf_are_equivalent() {
    let id = format!("Legacy.Codex-{}", uuid::Uuid::new_v4());
    let mut codex = agent(&id, "codex");
    codex.isolated_home = true;
    let mut settings = AppSettings {
        agents: vec![codex],
        ..AppSettings::default()
    };
    let legacy = resolve_agent_spawn_command(&settings, &id, None, Some("A"), false).unwrap();
    let leaf = super::sanitize_codex_home_id(&id);
    settings
        .coding_agent_profiles
        .codex_home_key_by_agent
        .insert(id.clone(), leaf.clone());
    let protected = resolve_agent_spawn_command(&settings, &id, None, Some("A"), false).unwrap();
    let expected = crate::config::config_dir()
        .unwrap()
        .join(crate::config::instance_artifacts::CODEX_HOME_DIR_NAME)
        .join(&leaf);
    assert_eq!(legacy.effective_codex_home, Some(expected.clone()));
    assert_eq!(protected.effective_codex_home, legacy.effective_codex_home);
    assert_eq!(protected.generated_env, legacy.generated_env);
    assert_eq!(protected.profile_content_hash, legacy.profile_content_hash);
    assert!(!expected.exists(), "resolution must not create HOME");
}

#[test]
fn derived_agent_id_home_present_leaf_survives_id_change() {
    let leaf = format!("preserved-{}", uuid::Uuid::new_v4());
    let mut codex = agent("new-id", "codex");
    codex.isolated_home = true;
    let mut settings = AppSettings {
        agents: vec![codex],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .codex_home_key_by_agent
        .insert("new-id".to_string(), leaf.clone());
    let resolved =
        resolve_agent_spawn_command(&settings, "new-id", None, Some("A"), false).unwrap();
    let expected = crate::config::config_dir()
        .unwrap()
        .join(crate::config::instance_artifacts::CODEX_HOME_DIR_NAME)
        .join(leaf);
    assert_eq!(resolved.effective_codex_home, Some(expected.clone()));
    assert!(!expected.exists());
}

#[test]
fn derived_agent_id_home_invalid_leaf_fails_only_affected_use() {
    let mut codex = agent("codex", "codex");
    codex.isolated_home = true;
    let mut settings = AppSettings {
        agents: vec![codex, agent("claude", "claude")],
        ..AppSettings::default()
    };
    for invalid in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        r"a\b",
        "C:drive",
        "UPPER",
        "space here",
        "ñ",
        "a\0b",
    ] {
        settings
            .coding_agent_profiles
            .codex_home_key_by_agent
            .insert("codex".to_string(), invalid.to_string());
        let error =
            resolve_agent_spawn_command(&settings, "codex", None, Some("A"), false).unwrap_err();
        assert!(
            error.contains("Invalid isolated Codex HOME key"),
            "{invalid:?}: {error}"
        );
        resolve_agent_spawn_command(&settings, "claude", None, Some("A"), false).unwrap();
        settings.agents[0].isolated_home = false;
        resolve_agent_spawn_command(&settings, "codex", None, Some("A"), false).unwrap();
        settings.agents[0].isolated_home = true;
    }
}
