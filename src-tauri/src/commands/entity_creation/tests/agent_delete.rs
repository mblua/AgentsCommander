use super::*;

#[test]
fn remove_replica_dir_reports_typed_outcomes_without_path_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let present = tmp.path().join("__agent_present");
    std::fs::create_dir_all(&present).unwrap();
    assert_eq!(remove_replica_dir(&present), ReplicaRemovalOutcome::Removed);
    assert!(!present.exists());

    // Absent path resolves via exact `symlink_metadata` NotFound, never `Path::exists`.
    let absent = tmp.path().join("__agent_absent");
    assert_eq!(
        remove_replica_dir(&absent),
        ReplicaRemovalOutcome::AlreadyAbsent
    );
    // Re-removing the just-removed directory is also proven-absent.
    assert_eq!(
        remove_replica_dir(&present),
        ReplicaRemovalOutcome::AlreadyAbsent
    );
}

#[tokio::test]
async fn agent_delete_metadata_precondition_mismatch_aborts_without_writes() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let (config_path, before) = write_team_value(
        &base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_dev-rust"],
            "coordinator": "_agent_lead",
            "repos": []
        }),
    );
    let mut after = before.clone();
    after.extend_from_slice(b"\n");
    let metadata = PreparedAgentDeleteMetadata {
        ac_root: base.clone(),
        team_mutations: vec![AgentTeamMutation {
            team_name: "dev-team".to_string(),
            config_path: config_path.clone(),
            before_json: before.clone(),
            after_json: after,
        }],
        agent_name: "dev-rust".to_string(),
        settings_changed: Arc::new(AtomicBool::new(false)),
    };
    // A lock-unaware writer changes the config after prepare but before commit.
    std::fs::write(
        &config_path,
        b"{\"agents\":[],\"coordinator\":\"_agent_lead\",\"repos\":[]}\n",
    )
    .unwrap();
    let changed = std::fs::read(&config_path).unwrap();
    let settings = settings_state(AppSettings::default());
    let base_owned = base.clone();
    let result = tokio::task::spawn_blocking(move || {
        let guard = TeamConfigMutationGuard::acquire(&base_owned).unwrap();
        persist_agent_delete_metadata_blocking(
            &metadata,
            &settings,
            &guard,
            |candidate: &AppSettings| -> Result<AppSettings, String> { Ok(candidate.clone()) },
        )
    })
    .await
    .unwrap();
    assert!(result.is_err(), "precondition mismatch must abort");
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        changed,
        "no team write may occur on a precondition mismatch"
    );
}

#[tokio::test]
async fn pending_create_under_agent_target_blocks_delete_and_rolls_back() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let replica = base.join("wg-1-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).expect("create replica");
    let settings = settings_state(AppSettings::default());
    let manager = test_manager();

    // Insert a pending create whose workdir sits at the replica target. Public
    // reads hide it (so the public preflight passes), but the deletion-only
    // snapshot must observe it and block the reversible delete.
    let workdir = replica.to_string_lossy().into_owned();
    let mgr = { manager.read().await.clone() };
    mgr.insert_pending_session_for_test(workdir).await;

    let result = run_agent_delete_for_test(&base, &agent_dir, &settings, &manager).await;
    assert!(
        result.is_err(),
        "a pending create under a delete target must block the delete"
    );
    assert!(
        agent_dir.is_dir(),
        "agent origin must be restored after the block"
    );
    assert!(replica.is_dir(), "replica must be restored after the block");
}

/// With a `#[cfg(test)]` activation token, workgroup deletion still prunes
/// legacy replica-config rows read from current metadata.
#[test]
fn gated_workgroup_delete_prunes_scope_with_activation_token() {
    use crate::config::seed_manifest::ManifestActivationToken;
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    let ac = project.join(".ac");
    std::fs::create_dir_all(ac.join("wg-1-team")).unwrap();

    // Seed a raw current-metadata legacy row under wg-1-team. Canonical
    // writers intentionally omit this row kind.
    let token = ManifestActivationToken::for_test();
    let manifest_path = ac.join("seed-manifest.toml");
    let wg1_legacy = concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 1\n",
        "coverage_version = 2\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\", ",
        "\"coding_agent_catalog\"]\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/wg-1-team/__agent_x/.claude/settings.json\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"replica_config_file\"\n",
        "scope = \"config:.ac/wg-1-team/__agent_x/.claude\"\n",
        "source = \"workspace_base\"\n",
        "last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n"
    )
    .as_bytes()
    .to_vec();
    assert!(wg1_legacy.starts_with(
        b"# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n"
    ));
    assert!(wg1_legacy.ends_with(b"\n"));
    assert!(!wg1_legacy.ends_with(b"\n\n"));
    std::fs::write(&manifest_path, &wg1_legacy).unwrap();
    assert!(
        std::fs::read_to_string(&manifest_path)
            .unwrap()
            .contains("wg-1-team"),
        "row must be seeded before the prune"
    );

    // A Deleted logical path prunes the workgroup scope under the gate.
    gated_workgroup_delete_with(
        &project,
        "wg-1-team",
        Some(&token),
        || {},
        || WgDeleteOutcome::Deleted,
    )
    .unwrap();

    let after = std::fs::read(&manifest_path).unwrap();
    assert_eq!(
        after,
        concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 2\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\", ",
            "\"coding_agent_catalog\"]\n",
            "files = []\n"
        )
        .as_bytes(),
        "Deleted must write the canonical empty manifest"
    );

    // A Blocked outcome (logical path intact) must NOT prune.
    let wg2_legacy = concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 1\n",
        "coverage_version = 2\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\", ",
        "\"coding_agent_catalog\"]\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/wg-2-team/__agent_y/.claude/settings.json\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"replica_config_file\"\n",
        "scope = \"config:.ac/wg-2-team/__agent_y/.claude\"\n",
        "source = \"workspace_base\"\n",
        "last_seeded_at = \"2026-07-16T19:42:12.456Z\"\n"
    )
    .as_bytes()
    .to_vec();
    assert!(wg2_legacy.starts_with(
        b"# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n"
    ));
    assert!(wg2_legacy.ends_with(b"\n"));
    assert!(!wg2_legacy.ends_with(b"\n\n"));
    std::fs::write(&manifest_path, &wg2_legacy).unwrap();
    gated_workgroup_delete_with(
        &project,
        "wg-2-team",
        Some(&token),
        || {},
        || WgDeleteOutcome::Blocked(std::io::Error::other("held")),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&manifest_path).unwrap(),
        wg2_legacy,
        "a Blocked delete must preserve raw current metadata exactly"
    );
}

#[test]
fn delete_agent_plan_uses_path_slug_not_role_display_name() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Displayed Rust Agent");

    let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");

    assert_eq!(plan.agent_name, "dev-rust");
    assert_eq!(plan.agent_ref, "_agent_dev-rust");
    assert_eq!(plan.origin_dir, agent_dir);
}

#[tokio::test]
async fn delete_agent_unreferenced_origin_only_prunes_settings() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let mut settings = AppSettings::default();
    settings
        .auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    settings
        .coding_agent_profiles
        .default_profile_by_agent
        .insert("dev-rust".to_string(), "A".to_string());
    let mut cells = BTreeMap::new();
    cells.insert("A".to_string(), profile_cell("codex"));
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .insert("dev-rust".to_string(), cells.clone());
    let mut labels = BTreeMap::new();
    labels.insert("A".to_string(), "Rust".to_string());
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert("dev-rust".to_string(), labels.clone());
    let settings = settings_state(settings);
    let manager = test_manager();

    let metadata = run_agent_delete_for_test(&base, &agent_dir, &settings, &manager)
        .await
        .expect("delete agent");

    assert!(!agent_dir.exists());
    assert!(metadata.settings_changed.load(Ordering::SeqCst));
    let saved = settings.read().await;
    assert!(!saved.auto_self_clear_by_agent.contains_key("dev-rust"));
    assert!(!saved
        .coding_agent_profiles
        .default_profile_by_agent
        .contains_key("dev-rust"));
    assert_eq!(
        saved.coding_agent_profiles.profiles_by_agent["dev-rust"],
        cells
    );
    assert_eq!(
        saved.coding_agent_profiles.profile_labels_by_agent["dev-rust"],
        labels
    );
}

#[tokio::test]
async fn delete_agent_cascade_removes_team_repo_refs_and_replicas() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    create_test_agent(&base, "architect", "Architect");
    let wg_dir = base.join("wg-1-dev-team");
    let replica = wg_dir.join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).expect("create replica");
    std::fs::create_dir_all(wg_dir.join("repo-service")).expect("create repo");
    std::fs::create_dir_all(wg_dir.join("messaging")).expect("create messaging");
    let config_path = write_team_value(
        &base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_architect", "_agent_dev-rust", "../_agent_dev-rust"],
            "coordinator": "_agent_architect",
            "repos": [
                {
                    "url": "https://example.test/service.git",
                    "agents": ["_agent_dev-rust", "_agent_architect"]
                },
                { "url": "https://example.test/docs.git", "agents": ["../_agent_dev-rust"] }
            ]
        }),
    )
    .0;
    let settings = settings_state(AppSettings::default());
    let manager = test_manager();

    run_agent_delete_for_test(&base, &agent_dir, &settings, &manager)
        .await
        .expect("delete agent");

    assert!(!agent_dir.exists());
    assert!(!replica.exists());
    assert!(wg_dir.join("repo-service").is_dir());
    assert!(wg_dir.join("messaging").is_dir());
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).expect("read config"))
            .expect("parse config");
    assert_eq!(config["agents"], serde_json::json!(["_agent_architect"]));
    assert_eq!(
        config["repos"][0]["agents"],
        serde_json::json!(["_agent_architect"])
    );
    assert_eq!(config["repos"][1]["agents"], serde_json::json!([]));
    assert_eq!(
        config["contextAlertPercentages"],
        serde_json::json!([]),
        "a relevant legacy raw write must insert the disabled canonical key"
    );
}

#[test]
fn raw_agent_delete_preserves_unknown_keys_and_sorts_populated_alerts() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let (config_path, _) = write_team_value(
        &base,
        "dev-team",
        serde_json::json!({
            "agents": ["_agent_dev-rust", "_agent_architect"],
            "coordinator": "_agent_architect",
            "repos": [{ "url": "u", "agents": ["_agent_dev-rust"] }],
            "contextAlertPercentages": [90, 50, 75],
            "futureUnknown": { "nested": [1, 2, 3] }
        }),
    );
    let guard = TeamConfigMutationGuard::acquire(&base).expect("guard");
    let plan =
        collect_agent_delete_plan_guarded(&base, &agent_dir, &guard).expect("raw delete plan");
    assert_eq!(plan.team_mutations.len(), 1);
    write_team_config_json_atomic(
        &base,
        &guard,
        &config_path,
        &plan.team_mutations[0].after_json,
    )
    .expect("apply raw mutation");
    drop(guard);

    let current: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).expect("current bytes"))
            .expect("parse current");
    assert_eq!(current["agents"], serde_json::json!(["_agent_architect"]));
    assert_eq!(current["repos"][0]["agents"], serde_json::json!([]));
    assert_eq!(
        current["contextAlertPercentages"],
        serde_json::json!([50, 75, 90])
    );
    assert_eq!(
        current["futureUnknown"],
        serde_json::json!({ "nested": [1, 2, 3] })
    );
}

#[test]
fn raw_agent_delete_rejects_each_malformed_relevant_alert_shape_without_writes() {
    let invalid_values = [
        serde_json::json!("50"),
        serde_json::json!([50.5]),
        serde_json::json!([0]),
        serde_json::json!([101]),
        serde_json::json!([1, 2, 3, 4]),
        serde_json::json!([50, 50]),
        serde_json::json!([-1]),
        serde_json::json!([256]),
    ];

    for (index, invalid) in invalid_values.into_iter().enumerate() {
        let tmp = create_test_ac_root();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        let (config_path, before) = write_team_value(
            &base,
            &format!("invalid-{}", index),
            serde_json::json!({
                "agents": ["_agent_dev-rust"],
                "coordinator": "_agent_architect",
                "repos": [],
                "contextAlertPercentages": invalid,
                "keep": true
            }),
        );

        let err = collect_agent_delete_plan(&base, &agent_dir)
            .expect_err("malformed relevant alert data must block");
        assert!(err.contains("invalid contextAlertPercentages"), "{err}");
        assert!(agent_dir.is_dir());
        assert_eq!(std::fs::read(&config_path).expect("config bytes"), before);
    }
}

#[test]
fn raw_agent_delete_leaves_unrelated_invalid_alert_config_untouched() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
    let (config_path, before) = write_team_value(
        &base,
        "unrelated",
        serde_json::json!({
            "agents": ["_agent_architect"],
            "coordinator": "_agent_architect",
            "repos": [],
            "contextAlertPercentages": [0],
            "keep": "verbatim"
        }),
    );

    let plan = collect_agent_delete_plan(&base, &agent_dir).expect("unrelated plan");
    assert!(plan.team_mutations.is_empty());
    assert_eq!(std::fs::read(&config_path).expect("config bytes"), before);
}

#[test]
fn issue_3294_delete_error_order_positive_control() {
    use crate::filesystem::projects::entities::validate_delete_root_parts;
    let removes = std::cell::Cell::new(0);
    let attempt = |reparse, symlink, directory| {
        validate_delete_root_parts(reparse, symlink, directory)?;
        removes.set(removes.get() + 1);
        Ok::<(), String>(())
    };
    assert_eq!(
        attempt(true, true, false).unwrap_err(),
        "delete_root_is_reparse_point"
    );
    assert_eq!(removes.get(), 0);
    assert_eq!(
        attempt(false, true, false).unwrap_err(),
        "delete_root_is_symlink"
    );
    assert_eq!(removes.get(), 0);
    assert_eq!(
        attempt(false, false, false).unwrap_err(),
        "delete_root_not_directory"
    );
    assert_eq!(removes.get(), 0);
    attempt(false, false, true).unwrap();
    assert_eq!(removes.get(), 1);
}

#[test]
fn issue_3294_not_found_positive_control() {
    use crate::filesystem::projects::entities::remove_replica_directory_with;
    use std::io::{Error, ErrorKind};
    let cases = [
        (
            Some(ErrorKind::NotFound),
            None,
            ReplicaRemovalOutcome::AlreadyAbsent,
            0,
        ),
        (
            None,
            Some(ErrorKind::NotFound),
            ReplicaRemovalOutcome::AlreadyAbsent,
            1,
        ),
        (
            Some(ErrorKind::PermissionDenied),
            None,
            ReplicaRemovalOutcome::Failed(
                "Failed to inspect replica directory: probe denied".into(),
            ),
            0,
        ),
        (
            None,
            Some(ErrorKind::PermissionDenied),
            ReplicaRemovalOutcome::Failed(
                "Failed to delete replica directory: remove denied".into(),
            ),
            1,
        ),
        (None, None, ReplicaRemovalOutcome::Removed, 1),
    ];
    for (probe_error, remove_error, expected, remove_count) in cases {
        let probes = std::cell::Cell::new(0);
        let removes = std::cell::Cell::new(0);
        let actual = remove_replica_directory_with(
            Path::new("unused-injected-path"),
            |_| {
                probes.set(probes.get() + 1);
                probe_error.map_or(Ok(()), |kind| Err(Error::new(kind, "probe denied")))
            },
            |_| {
                removes.set(removes.get() + 1);
                remove_error.map_or(Ok(()), |kind| Err(Error::new(kind, "remove denied")))
            },
        );
        assert_eq!(actual, expected);
        assert_eq!((probes.get(), removes.get()), (1, remove_count));
    }
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        remove_replica_dir(&tmp.path().join("absent")),
        ReplicaRemovalOutcome::AlreadyAbsent
    );

    // Inspect the real consumer, fixing the sole prune branch to the real outcomes.
    let consumer = include_str!("../../../cli/team.rs");
    let start = consumer
        .find("match remove_replica_dir(&replica_dir)")
        .unwrap();
    let matched = &consumer[start..];
    let open = matched.find('{').unwrap();
    let mut depth = 0;
    let end = matched[open..]
        .char_indices()
        .find_map(|(i, ch)| {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(open + i)
        })
        .unwrap();
    let body = &matched[open + 1..end];
    let failed = body.find("ReplicaRemovalOutcome::Failed(error)").unwrap();
    let success = &body[..failed];
    assert!(
        success.contains("ReplicaRemovalOutcome::Removed | ReplicaRemovalOutcome::AlreadyAbsent")
    );
    assert_eq!(success.matches("prune_replica_config_scope").count(), 1);
    let failure = &body[failed..];
    assert!(!failure.contains("prune_replica_config_scope"));
    let guard = failure.find("drop(guard)").unwrap();
    let gate = failure.find("drop(project_gate)").unwrap();
    let returned = failure.find("return Err(error)").unwrap();
    assert!(guard < gate && gate < returned);
}
