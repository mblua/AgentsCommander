use super::*;

/// #1245: the writer and `config::teams::team_members` enforced mutually
/// exclusive invariants over the same `agents` array, so every team config
/// the application was able to write was rejected by the privileged
/// validator. The two assertions below cannot both hold unless the writer
/// and the validator agree on where the coordinator belongs, which is what
/// stops them drifting apart again.
#[test]
fn team_config_written_by_the_app_passes_the_privileged_validator() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join("proj-a").join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let wg_dir = ac_root.join("wg-1-dev-team");
    std::fs::create_dir_all(&team_dir).expect("team directory");
    for agent in ["tech-lead", "dev-rust"] {
        // `resolve_agent_ref` requires the matrix directory to exist already.
        std::fs::create_dir_all(ac_root.join(format!("_agent_{agent}"))).expect("matrix directory");
        let replica = wg_dir.join(format!("__agent_{agent}"));
        std::fs::create_dir_all(&replica).expect("replica directory");
        std::fs::write(
            replica.join("config.json"),
            format!(r#"{{"identity":"../../_agent_{agent}"}}"#),
        )
        .expect("replica config");
    }

    let config = TeamConfigResult {
        agents: vec!["_agent_tech-lead".into(), "_agent_dev-rust".into()],
        coordinator: "_agent_tech-lead".into(),
        repos: vec![],
        context_alert_percentages: vec![],
    };
    let bytes = normalized_team_config_bytes(&ac_root, &config)
        .expect("the writer requires the coordinator inside `agents`");
    std::fs::write(team_dir.join("config.json"), &bytes).expect("team config");

    let project_paths = vec![tmp.path().to_string_lossy().to_string()];
    crate::config::teams::verify_pty_input_route(
        &wg_dir.join("__agent_tech-lead"),
        false,
        "proj-a:wg-1-dev-team/dev-rust",
        &project_paths,
    )
    .expect("the privileged validator must accept the bytes the writer produced");

    let without_coordinator = TeamConfigResult {
        agents: vec!["_agent_dev-rust".into()],
        ..config
    };
    assert_eq!(
        normalized_team_config_bytes(&ac_root, &without_coordinator).unwrap_err(),
        "Orchestrator must be one of the selected agents"
    );
}

#[test]
#[cfg(windows)]
fn git_cli_path_strips_windows_verbatim_prefix() {
    assert_eq!(
        git_cli_path(Path::new(r"\\?\C:\tmp\repo-Hello-World")),
        PathBuf::from(r"C:\tmp\repo-Hello-World")
    );
    assert_eq!(
        git_cli_path(Path::new(r"\\?\UNC\server\share\repo-Hello-World")),
        PathBuf::from(r"\\server\share\repo-Hello-World")
    );
}

#[test]
#[cfg(not(windows))]
fn git_cli_path_preserves_non_windows_path() {
    let path = Path::new("/tmp/repo-Hello-World");
    assert_eq!(git_cli_path(path), path);
}

#[test]
fn create_agent_matrix_layout_creates_root_and_canonical_subdirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let agent_dir = tmp.path().join("_agent_test");

    assert!(!agent_dir.exists(), "agent_dir must not exist before call");

    create_agent_matrix_layout(&agent_dir).expect("create_agent_matrix_layout");

    assert!(
        agent_dir.is_dir(),
        "helper must create the matrix root itself"
    );
    for canonical in &["memory", "plans", "skills"] {
        assert!(
            agent_dir.join(canonical).is_dir(),
            "expected canonical state dir {}/",
            canonical
        );
    }
    assert!(agent_dir.join("inbox").is_dir(), "expected inbox/");
    assert!(agent_dir.join("outbox").is_dir(), "expected outbox/");
}

#[test]
fn create_agent_matrix_layout_is_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let agent_dir = tmp.path().join("_agent_test");

    create_agent_matrix_layout(&agent_dir).expect("first call");
    let sentinel = agent_dir.join("memory").join("sentinel.md");
    std::fs::write(&sentinel, b"x").expect("write sentinel");

    create_agent_matrix_layout(&agent_dir).expect("second call");
    assert!(
        sentinel.is_file(),
        "second call must not wipe pre-existing files in memory/"
    );
}

#[test]
fn create_agent_matrix_on_disk_creates_full_layout_without_template() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    let settings = AppSettings::default();
    let project_s = project.to_string_lossy().to_string();

    let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_s,
        name: "Architect",
        description: "Build plans",
        role_template_id: None,
        settings: &settings,
        config_dir: None,
    })
    .expect("create matrix");

    let agent_dir = ac_root.join("_agent_architect");
    assert_eq!(created.agent_dir, agent_dir);
    assert_eq!(created.safe_name, "architect");
    assert_eq!(created.display_name, "ProjectAlpha/architect");
    assert!(agent_dir.join("Role.md").is_file());
    assert!(agent_dir.join("config.json").is_file());
    for canonical in AGENT_MATRIX_DIRS {
        assert!(
            agent_dir.join(canonical).is_dir(),
            "missing canonical dir {}",
            canonical
        );
    }
}

#[test]
fn create_agent_matrix_on_disk_rejects_non_ac_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let invalid_ac_root = project.join(".workspace");
    std::fs::create_dir_all(&invalid_ac_root).expect("create invalid workspace");
    let settings = AppSettings::default();
    let project_s = project.to_string_lossy().to_string();

    let err = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_s,
        name: "Architect",
        description: "Build plans",
        role_template_id: None,
        settings: &settings,
        config_dir: None,
    })
    .unwrap_err();

    assert!(err.contains(".ac directory not found"));
    assert!(!invalid_ac_root.join("_agent_architect").exists());
}

#[test]
fn create_agent_matrix_on_disk_uses_ac_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    let settings = AppSettings::default();
    let project_s = project.to_string_lossy().to_string();

    let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_s,
        name: "Architect",
        description: "Build plans",
        role_template_id: None,
        settings: &settings,
        config_dir: None,
    })
    .expect("create matrix");

    assert_eq!(created.agent_dir, ac_root.join("_agent_architect"));
}

#[test]
fn create_agent_matrix_on_disk_resolves_template_before_mutation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let ac_root = project.join(".ac");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    std::fs::create_dir_all(&config_dir).expect("create config");
    let settings = AppSettings::default();
    let project_s = project.to_string_lossy().to_string();

    let err = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_s,
        name: "Architect",
        description: "Build plans",
        role_template_id: Some("agency:not-real"),
        settings: &settings,
        config_dir: Some(&config_dir),
    })
    .expect_err("invalid template");

    assert!(err.contains("missing"));
    assert!(
        !ac_root.join("_agent_architect").exists(),
        "invalid template must not create the target matrix directory"
    );
}

#[test]
fn create_agent_matrix_on_disk_existing_root_fails_without_overwrite() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let ac_root = project.join(".ac");
    let agent_dir = ac_root.join("_agent_architect");
    std::fs::create_dir_all(&agent_dir).expect("create existing matrix root");
    std::fs::write(agent_dir.join("Role.md"), "keep role").expect("write existing role");
    std::fs::write(agent_dir.join("config.json"), "keep config").expect("write existing config");
    let settings = AppSettings::default();
    let project_s = project.to_string_lossy().to_string();

    let err = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_s,
        name: "Architect",
        description: "Build plans",
        role_template_id: None,
        settings: &settings,
        config_dir: None,
    })
    .expect_err("existing root");

    assert_eq!(err, "Agent 'architect' already exists");
    assert_eq!(
        std::fs::read_to_string(agent_dir.join("Role.md")).expect("read existing role"),
        "keep role"
    );
    assert_eq!(
        std::fs::read_to_string(agent_dir.join("config.json")).expect("read existing config"),
        "keep config"
    );
}

#[test]
fn create_agent_matrix_on_disk_applies_local_template_and_skills() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let ac_root = project.join(".ac");
    let config_dir = tmp.path().join("config");
    let template_dir = config_dir.join("agent-templates").join("my-template");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    std::fs::create_dir_all(template_dir.join("skills").join("example"))
        .expect("create template skill dir");
    std::fs::write(
        template_dir.join("Role.md"),
        "---\nname: My Template\n---\n\n# Template Body\n\nUse this profile.\n",
    )
    .expect("write template role");
    std::fs::write(
        template_dir.join("skills").join("example").join("SKILL.md"),
        "# Skill\n",
    )
    .expect("write template skill");
    let settings = AppSettings::default();
    let project_s = project.to_string_lossy().to_string();

    let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_s,
        name: "Architect",
        description: "Build plans",
        role_template_id: Some("local:my-template"),
        settings: &settings,
        config_dir: Some(&config_dir),
    })
    .expect("create matrix from template");

    let role = std::fs::read_to_string(created.role_path).expect("read Role.md");
    assert!(role.contains("## Role Profile"));
    assert!(role.contains("# Template Body"));
    assert!(role.contains("Use this profile."));
    assert!(created
        .agent_dir
        .join("skills")
        .join("example")
        .join("SKILL.md")
        .is_file());
}

#[test]
fn agent_matrix_dirs_contains_memory_plans_skills() {
    let names: std::collections::HashSet<&str> = AGENT_MATRIX_DIRS.iter().copied().collect();
    for required in &["memory", "plans", "skills"] {
        assert!(
            names.contains(required),
            "AGENT_MATRIX_DIRS must contain {} (issue #209)",
            required
        );
    }
}

#[test]
fn default_agent_matrix_config_includes_context_and_role() {
    let config = default_agent_matrix_config();

    assert_eq!(config["tooling"], serde_json::json!({}));
    assert_eq!(
        config["context"],
        serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "Role.md"])
    );
}

#[test]
fn team_config_normalization_stores_portable_agent_refs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    let architect = ac_root.join("_agent_architect");
    let dev_rust = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&architect).expect("architect matrix");
    std::fs::create_dir_all(&dev_rust).expect("dev-rust matrix");

    let config = TeamConfigResult {
        agents: vec![
            "architect".to_string(),
            "_agent_dev-rust".to_string(),
            "architect".to_string(),
        ],
        coordinator: "_agent_architect".to_string(),
        repos: vec![RepoAssignment {
            url: "https://example.test/repo.git".to_string(),
            agents: vec!["architect".to_string(), "_agent_dev-rust".to_string()],
        }],
        context_alert_percentages: vec![90, 50, 75],
    };

    let normalized =
        normalize_team_config_for_project(&ac_root, &config).expect("normalize config");
    assert_eq!(
        normalized.agents,
        vec![
            "_agent_architect".to_string(),
            "_agent_dev-rust".to_string()
        ]
    );
    assert_eq!(normalized.coordinator, "_agent_architect");
    assert_eq!(
        normalized.repos[0].agents,
        vec![
            "_agent_architect".to_string(),
            "_agent_dev-rust".to_string()
        ]
    );
    assert_eq!(normalized.context_alert_percentages, vec![50, 75, 90]);

    write_team_config(&ac_root, "dev-team", &config).expect("write config");
    let written = std::fs::read_to_string(ac_root.join("_team_dev-team").join("config.json"))
        .expect("read config");
    assert!(
        !written.contains(&tmp.path().to_string_lossy().to_string()),
        "team config must not persist absolute project paths: {}",
        written
    );
}

#[test]
fn team_config_normalization_rejects_filesystem_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir_all(ac_root.join("_agent_architect")).expect("architect matrix");

    let absolute_config = TeamConfigResult {
        agents: vec![ac_root
            .join("_agent_architect")
            .to_string_lossy()
            .to_string()],
        coordinator: "_agent_architect".to_string(),
        repos: Vec::new(),
        context_alert_percentages: Vec::new(),
    };
    let err = normalize_team_config_for_project(&ac_root, &absolute_config)
        .expect_err("absolute path refs must be rejected");
    assert!(
        err.contains("filesystem paths"),
        "unexpected error: {}",
        err
    );

    let windows_config = TeamConfigResult {
        agents: vec![r"C:\Users\maria\project\.ac\_agent_architect".to_string()],
        coordinator: "_agent_architect".to_string(),
        repos: Vec::new(),
        context_alert_percentages: Vec::new(),
    };
    let err = normalize_team_config_for_project(&ac_root, &windows_config)
        .expect_err("windows path refs must be rejected");
    assert!(
        err.contains("filesystem paths"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn replica_creation_rejects_filesystem_agent_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    let wg_dir = ac_root.join("wg-1-dev-team");
    let matrix_dir = ac_root.join("_agent_architect");
    std::fs::create_dir_all(&matrix_dir).expect("architect matrix");
    std::fs::create_dir_all(&wg_dir).expect("workgroup");

    let err = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
        ac_root: ac_root.clone(),
        wg_dir: wg_dir.clone(),
        agent_path: matrix_dir.to_string_lossy().to_string(),
        team_repos: Vec::new(),
    })
    .expect_err("absolute path refs must be rejected");

    assert!(
        err.contains("filesystem paths"),
        "unexpected error: {}",
        err
    );
    assert!(
        !wg_dir
            .join(format!("__agent_{}", matrix_dir.to_string_lossy()))
            .exists(),
        "replica creation must not create path-derived agent directories"
    );
}

#[test]
fn create_agent_replica_layout_creates_only_inbox_outbox() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let replica_dir = tmp.path().join("__agent_test");

    assert!(
        !replica_dir.exists(),
        "replica_dir must not exist before call"
    );

    create_agent_replica_layout(&replica_dir).expect("create_agent_replica_layout");

    assert!(
        replica_dir.is_dir(),
        "helper must create the replica root itself"
    );
    assert!(replica_dir.join("inbox").is_dir(), "expected inbox/");
    assert!(replica_dir.join("outbox").is_dir(), "expected outbox/");
    for canonical in &["memory", "plans", "skills"] {
        assert!(
            !replica_dir.join(canonical).exists(),
            "replica MUST NOT have {}/; origin matrix is canonical",
            canonical
        );
    }
}

#[test]
fn agent_matrix_and_replica_dir_sets_are_disjoint_on_canonical_state() {
    let canonical: std::collections::HashSet<&str> =
        ["memory", "plans", "skills"].into_iter().collect();
    let replica: std::collections::HashSet<&str> = AGENT_REPLICA_DIRS.iter().copied().collect();
    assert!(
        canonical.is_disjoint(&replica),
        "AGENT_REPLICA_DIRS must not include canonical state names; overlap: {:?}",
        canonical.intersection(&replica).collect::<Vec<_>>()
    );
}

#[test]
fn replica_identity_resolves_to_origin_matrix_role_md() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    let matrix_dir = ac_root.join("_agent_alpha");
    let replica_dir = ac_root.join("wg-1-team").join("__agent_alpha");
    std::fs::create_dir_all(&matrix_dir).expect("create matrix_dir");
    std::fs::create_dir_all(&replica_dir).expect("create replica_dir");

    let matrix_role = matrix_dir.join("Role.md");
    std::fs::write(&matrix_role, b"# Alpha\n").expect("write matrix Role.md");

    let identity = crate::config::replica_identity::expected_wg_replica_identity(&replica_dir)
        .expect("expected replica identity")
        .identity;

    assert_eq!(
        identity, "../../_agent_alpha",
        "expected identity to traverse wg-1-team -> .ac -> _agent_alpha"
    );

    let resolved = replica_dir.join(&identity).join("Role.md");
    assert_eq!(
        resolved
            .canonicalize()
            .expect("canonicalize resolved Role.md"),
        matrix_role
            .canonicalize()
            .expect("canonicalize matrix Role.md"),
        "replica Role.md context entry must resolve to origin matrix Role.md"
    );
}

#[test]
fn create_replica_rejects_filesystem_agent_ref() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("AgentsCommander_ac");
    let ac_root = project.join(".ac");
    let matrix_dir = ac_root.join("_agent_tech-lead");
    let wg_dir = ac_root.join("wg-2-dev-team");
    std::fs::create_dir_all(&matrix_dir).expect("create matrix");
    std::fs::create_dir_all(&wg_dir).expect("create wg");
    std::fs::write(matrix_dir.join("Role.md"), "# Tech Lead\n").expect("write role");
    let stale_agent_ref = tmp
        .path()
        .join("agentscommander-old")
        .join(".ac")
        .join("_agent_tech-lead")
        .to_string_lossy()
        .replace('\\', "/");

    let err = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
        ac_root,
        wg_dir: wg_dir.clone(),
        agent_path: stale_agent_ref,
        team_repos: Vec::new(),
    })
    .expect_err("filesystem path refs must be rejected");

    assert!(
        err.contains("filesystem paths"),
        "unexpected error: {}",
        err
    );
    assert!(
        !wg_dir.join("__agent_tech-lead").exists(),
        "path refs must not create replicas"
    );
}

#[test]
fn create_replica_writes_expected_local_identity_for_portable_ref() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("AgentsCommander_ac");
    let ac_root = project.join(".ac");
    let matrix_dir = ac_root.join("_agent_tech-lead");
    let wg_dir = ac_root.join("wg-2-dev-team");
    std::fs::create_dir_all(&matrix_dir).expect("create matrix");
    std::fs::create_dir_all(&wg_dir).expect("create wg");
    std::fs::write(matrix_dir.join("Role.md"), "# Tech Lead\n").expect("write role");

    let replica_dir = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
        ac_root,
        wg_dir,
        agent_path: "_agent_tech-lead".to_string(),
        team_repos: Vec::new(),
    })
    .expect("create replica");

    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(replica_dir.join("config.json")).expect("read config"),
    )
    .expect("parse config");
    assert_eq!(config["identity"], "../../_agent_tech-lead");
}

// ------------------------------------------------------------------
// #1939 replica config initialization (guarded, lossless, default).
// ------------------------------------------------------------------
