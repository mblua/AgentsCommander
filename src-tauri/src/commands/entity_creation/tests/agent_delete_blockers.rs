use super::*;

#[test]
fn delete_agent_coordinator_blocks_and_touches_nothing() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let (config_path, before) = write_team_value(
        &base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_dev-rust"],
            "coordinator": "../_agent_dev-rust",
            "repos": []
        }),
    );

    let err = collect_agent_delete_plan(&base, &agent_dir).expect_err("coordinator blocks");

    assert!(err.contains("orchestrator of team(s): dev-team"), "{err}");
    assert!(agent_dir.is_dir());
    assert_eq!(std::fs::read(&config_path).expect("read config"), before);
}

#[test]
#[cfg(windows)]
fn delete_agent_case_mismatched_coordinator_blocks_on_windows() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "DevRust", "Dev Rust");
    write_team_value(
        &base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_devrust"],
            "coordinator": "_agent_devrust",
            "repos": []
        }),
    );

    let err = collect_agent_delete_plan(&base, &agent_dir).expect_err("coordinator blocks");

    assert!(err.contains("orchestrator of team(s): dev-team"), "{err}");
}

#[test]
#[cfg(windows)]
fn delete_agent_case_mismatched_refs_are_pruned_on_windows() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "DevRust", "Dev Rust");
    let config_path = write_team_value(
        &base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_devrust", "_agent_architect"],
            "coordinator": "_agent_architect",
            "repos": [{ "url": "u", "agents": ["_agent_devrust"] }]
        }),
    )
    .0;

    let guard = TeamConfigMutationGuard::acquire(&base).expect("team guard");
    let plan = collect_agent_delete_plan_guarded(&base, &agent_dir, &guard).expect("collect plan");
    write_team_config_json_atomic(
        &base,
        &guard,
        &config_path,
        &plan.team_mutations[0].after_json,
    )
    .expect("write mutation");

    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).expect("read config"))
            .expect("parse config");
    assert_eq!(config["agents"], serde_json::json!(["_agent_architect"]));
    assert_eq!(config["repos"][0]["agents"], serde_json::json!([]));
}

#[tokio::test]
async fn delete_agent_unrelated_stale_team_ref_does_not_block() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-docs", "Dev Docs");
    write_team_value(
        &base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_ghost", "_agent_dev-docs"],
            "coordinator": "_agent_ghost",
            "repos": [{ "url": "u", "agents": ["_agent_dev-docs", "_agent_ghost"] }]
        }),
    );
    let settings = settings_state(AppSettings::default());
    let manager = test_manager();

    run_agent_delete_for_test(&base, &agent_dir, &settings, &manager)
        .await
        .expect("delete with stale unrelated ref");

    assert!(!agent_dir.exists());
}

#[test]
fn delete_agent_relevant_invalid_team_config_blocks_and_touches_nothing() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-docs", "Dev Docs");
    let team_dir = base.join("_team_dev-team");
    std::fs::create_dir_all(&team_dir).expect("create team");
    let config_path = team_dir.join("config.json");
    let before = b"{ invalid _agent_dev-docs".to_vec();
    std::fs::write(&config_path, &before).expect("write config");

    let err = collect_agent_delete_plan(&base, &agent_dir).expect_err("invalid relevant config");

    assert!(err.contains("cannot verify team 'dev-team'"), "{err}");
    assert!(agent_dir.is_dir());
    assert_eq!(std::fs::read(&config_path).expect("read config"), before);
}

#[test]
fn delete_agent_invalid_team_config_uses_token_match_not_substring() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev", "Dev");
    let team_dir = base.join("_team_dev-team");
    std::fs::create_dir_all(&team_dir).expect("create team");
    std::fs::write(team_dir.join("config.json"), b"{ invalid developer").expect("write config");

    let plan = collect_agent_delete_plan(&base, &agent_dir).expect("substring should not block");

    assert!(plan.team_mutations.is_empty());
}

#[tokio::test]
async fn delete_agent_live_origin_session_blocks() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let manager = test_manager();
    add_live_session(&manager, &agent_dir).await;
    let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");

    let err = preflight_agent_delete(&plan, &manager)
        .await
        .expect_err("live origin blocks");

    assert!(err.contains("Cannot delete agent 'dev-rust'"), "{err}");
    assert!(err.contains("Session 1"), "{err}");
    assert!(agent_dir.is_dir());
}

#[tokio::test]
async fn delete_agent_live_replica_session_blocks() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let replica = base.join("wg-1-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).expect("create replica");
    let manager = test_manager();
    add_live_session(&manager, &replica).await;
    let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");

    let err = preflight_agent_delete(&plan, &manager)
        .await
        .expect_err("live replica blocks");

    assert!(err.contains("Cannot delete agent 'dev-rust'"), "{err}");
    assert!(err.contains("Session 1"), "{err}");
    assert!(agent_dir.is_dir());
    assert!(replica.is_dir());
}

#[tokio::test]
async fn delete_agent_post_stage_live_session_recheck_rolls_back() {
    let tmp = create_test_ac_root();
    let real_base = test_base(&tmp);
    let agent_dir = create_test_agent(&real_base, "dev-rust", "Dev Rust");
    let marker = real_base.join("marker");
    std::fs::create_dir_all(&marker).expect("create marker");
    let base = marker.join("..");
    let selected_agent_path = base.join("_agent_dev-rust");
    let (config_path, before_config) = write_team_value(
        &real_base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_dev-rust"],
            "coordinator": "_agent_architect",
            "repos": []
        }),
    );
    let settings = settings_state(AppSettings::default());
    let manager = test_manager();
    let plan = collect_agent_delete_plan(&base, &selected_agent_path).expect("collect plan");
    preflight_agent_delete(&plan, &manager)
        .await
        .expect("preflight");
    let _metadata = prepare_agent_delete_metadata(&plan, &settings)
        .await
        .expect("metadata");
    let staged = stage_agent_delete_targets(&plan, &manager)
        .await
        .expect("stage");
    assert!(!agent_dir.exists());
    let post_stage_fallback_key = path_key_for_delete(&selected_agent_path);
    assert!(
        plan.target_keys.contains(&post_stage_fallback_key),
        "post-stage recheck must use the pre-stage lexical target key"
    );
    add_live_session(&manager, &selected_agent_path).await;

    let err =
        ensure_no_live_sessions_under_target_keys(&plan.target_keys, &plan.agent_name, &manager)
            .await
            .expect_err("post-stage live session blocks");
    let rollback = rollback_staged_agent_delete_targets(&staged);
    let msg = format_agent_delete_post_stage_live_failure(err, rollback);

    assert!(msg.contains("Cannot delete agent 'dev-rust'"), "{msg}");
    assert!(agent_dir.is_dir());
    for target in staged {
        assert!(!target.staged_path.exists(), "staged path left behind");
    }
    assert_eq!(
        std::fs::read(&config_path).expect("read config"),
        before_config
    );
}

#[tokio::test]
async fn delete_agent_orphan_replica_removed() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let replica = base.join("wg-1-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).expect("create orphan replica");
    let settings = settings_state(AppSettings::default());
    let manager = test_manager();

    run_agent_delete_for_test(&base, &agent_dir, &settings, &manager)
        .await
        .expect("delete orphan replica");

    assert!(!agent_dir.exists());
    assert!(!replica.exists());
}

#[test]
fn delete_agent_stage_targets_rolls_back_on_second_target_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let first = tmp.path().join("_agent_dev-rust");
    let second = tmp.path().join("wg-1-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&first).expect("first");
    std::fs::create_dir_all(&second).expect("second");
    let plan = AgentDeletePlan {
        ac_root: tmp.path().to_path_buf(),
        agent_name: "dev-rust".to_string(),
        agent_ref: "_agent_dev-rust".to_string(),
        origin_dir: first.clone(),
        target_keys: Vec::new(),
        targets: vec![
            AgentDeleteTarget {
                original_path: first.clone(),
                original_key: path_key_for_delete(&first),
                label: "origin".to_string(),
            },
            AgentDeleteTarget {
                original_path: second.clone(),
                original_key: path_key_for_delete(&second),
                label: "replica".to_string(),
            },
        ],
        team_mutations: Vec::new(),
    };
    let manager = test_manager();

    let err = stage_agent_delete_targets_with_rename(
        &plan,
        |from: &Path, to: &Path| {
            if from == second {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "forced",
                ))
            } else {
                std::fs::rename(from, to)
            }
        },
        &manager,
    )
    .expect_err("second target fails");

    assert!(matches!(err, AgentDeleteStageError::Other(_)));
    assert!(first.is_dir());
    assert!(second.is_dir());
}

#[test]
fn delete_agent_stage_targets_rollback_failure_reports_hidden_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let first = tmp.path().join("_agent_dev-rust");
    let second = tmp.path().join("wg-1-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&first).expect("first");
    std::fs::create_dir_all(&second).expect("second");
    let plan = AgentDeletePlan {
        ac_root: tmp.path().to_path_buf(),
        agent_name: "dev-rust".to_string(),
        agent_ref: "_agent_dev-rust".to_string(),
        origin_dir: first.clone(),
        target_keys: Vec::new(),
        targets: vec![
            AgentDeleteTarget {
                original_path: first.clone(),
                original_key: path_key_for_delete(&first),
                label: "origin".to_string(),
            },
            AgentDeleteTarget {
                original_path: second.clone(),
                original_key: path_key_for_delete(&second),
                label: "replica".to_string(),
            },
        ],
        team_mutations: Vec::new(),
    };
    let manager = test_manager();

    let err = stage_agent_delete_targets_with_rename(
        &plan,
        |from: &Path, to: &Path| {
            if from == second || to == first {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "forced",
                ))
            } else {
                std::fs::rename(from, to)
            }
        },
        &manager,
    )
    .expect_err("rollback fails");

    let AgentDeleteStageError::RollbackFailed(msg) = err else {
        panic!("expected rollback failure");
    };
    assert!(msg.contains("Hidden cleanup dirs remain"), "{msg}");
    assert!(!msg.starts_with("BLOCKERS:"), "{msg}");
    assert!(!first.exists());
    assert!(second.is_dir());
}

#[tokio::test]
async fn delete_agent_metadata_team_write_failure_restores_configs_and_rolls_back_dirs() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let (config_a, before_a) = write_team_value(
        &base,
        "alpha",
        serde_json::json!({
            "agents": ["_agent_dev-rust"],
            "coordinator": "_agent_architect",
            "repos": []
        }),
    );
    let (config_b, before_b) = write_team_value(
        &base,
        "beta",
        serde_json::json!({
            "agents": ["_agent_dev-rust"],
            "coordinator": "_agent_architect",
            "repos": []
        }),
    );
    let settings = settings_state(AppSettings::default());
    let manager = test_manager();
    let guard = TeamConfigMutationGuard::acquire(&base).expect("team guard");
    let plan = collect_agent_delete_plan_guarded(&base, &agent_dir, &guard).expect("collect plan");
    let metadata = prepare_agent_delete_metadata(&plan, &settings)
        .await
        .expect("metadata");
    let staged = stage_agent_delete_targets(&plan, &manager)
        .await
        .expect("stage");
    let writes = AtomicUsize::new(0);

    let persist_err = persist_agent_delete_metadata_with_writers(
        &metadata,
        &settings,
        &guard,
        |path, bytes| {
            let count = writes.fetch_add(1, TestOrdering::SeqCst);
            if count == 1 {
                Err("forced team write failure".to_string())
            } else {
                write_team_config_json_atomic(&base, &guard, path, bytes)
            }
        },
        |candidate| Ok(candidate.clone()),
    )
    .await
    .expect_err("team write failure");
    let restore = restore_agent_delete_metadata_snapshots(&metadata, &settings, &guard).await;
    drop(guard);
    let rollback = rollback_staged_agent_delete_targets(&staged);
    let msg = format_agent_delete_metadata_failure(persist_err, restore, rollback);

    assert!(msg.contains("forced team write failure"), "{msg}");
    assert!(agent_dir.is_dir());
    assert_eq!(std::fs::read(&config_a).expect("read a"), before_a);
    assert_eq!(std::fs::read(&config_b).expect("read b"), before_b);
}

#[tokio::test]
async fn delete_agent_metadata_settings_save_failure_restores_configs_and_rolls_back_dirs() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let (config_path, before_config) = write_team_value(
        &base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_dev-rust"],
            "coordinator": "_agent_architect",
            "repos": []
        }),
    );
    let mut settings = AppSettings::default();
    settings
        .auto_self_clear_by_agent
        .insert("dev-rust".to_string(), false);
    settings
        .coding_agent_profiles
        .default_profile_by_agent
        .insert("dev-rust".to_string(), "A".to_string());
    let settings = settings_state(settings);
    let manager = test_manager();
    let guard = TeamConfigMutationGuard::acquire(&base).expect("team guard");
    let plan = collect_agent_delete_plan_guarded(&base, &agent_dir, &guard).expect("collect plan");
    let metadata = prepare_agent_delete_metadata(&plan, &settings)
        .await
        .expect("metadata");
    let staged = stage_agent_delete_targets(&plan, &manager)
        .await
        .expect("stage");

    let persist_err =
        persist_agent_delete_metadata_with_saver(&metadata, &settings, &guard, |_| {
            Err("forced settings save failure".to_string())
        })
        .await
        .expect_err("settings save failure");
    let restore = restore_agent_delete_metadata_snapshots(&metadata, &settings, &guard).await;
    drop(guard);
    let rollback = rollback_staged_agent_delete_targets(&staged);
    let msg = format_agent_delete_metadata_failure(persist_err, restore, rollback);

    assert!(msg.contains("forced settings save failure"), "{msg}");
    assert!(agent_dir.is_dir());
    assert_eq!(
        std::fs::read(&config_path).expect("read config"),
        before_config
    );
    let saved = settings.read().await;
    assert_eq!(saved.auto_self_clear_by_agent.get("dev-rust"), Some(&false));
    assert_eq!(
        saved
            .coding_agent_profiles
            .default_profile_by_agent
            .get("dev-rust")
            .map(String::as_str),
        Some("A")
    );
}
