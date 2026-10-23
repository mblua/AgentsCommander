use super::*;
use crate::config::settings::CodingAgentEnvSource;
use crate::pty::backend::SessionBackendKind;

#[test]
fn single_editor_inactive_gate() {
    use crate::config::settings::{
        source_transition_effect_counts, SourceReadContext, SourceRevision,
    };
    let root = crate::config::settings::source_transition_test_root();
    let project = root.path().join("project");
    let config = root.path().join("C");
    std::fs::create_dir_all(project.join(".ac/coding-agents")).unwrap();
    std::fs::create_dir(&config).unwrap();
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    let context =
        SourceReadContext::from_settings(&settings, &config.join("settings.json")).unwrap();
    let source = crate::config::settings::SourceRef {
        family: crate::config::settings::SourceFamily::Agents,
        role: crate::config::settings::SourceRole::Catalog,
        scope: crate::config::settings::SourceScope::Project,
        kind: crate::config::settings::SourceKind::CatalogPersonal,
        context_root: std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    };
    let before = source_transition_effect_counts();
    let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
    let reloads = std::cell::Cell::new(0);
    let reload = || {
        reloads.set(reloads.get() + 1);
        Ok(())
    };
    for candidate in [
        serde_json::json!({"schemaVersion":2,"agents":[]}),
        serde_json::Value::Null,
    ] {
        assert!(matches!(
            commit_source_candidate(
                &settings,
                &context,
                &source,
                "request-1",
                &SourceRevision::Absent {},
                &candidate,
                &reload
            ),
            Err(SourceTransitionError::ActivationRequired)
        ));
    }
    let expected = SourceResumeExpectation {
        source: SourceRevision::Absent {},
        journal: SourceRevision::Absent {},
    };
    for operation in ["operation-1", "unknown", ""] {
        assert!(matches!(
            resume_identity_transition(&settings, &context, &source, operation, &expected, &reload),
            Err(SourceTransitionError::ActivationRequired)
        ));
    }
    let status = get_identity_transition(&context, &source, "unknown").unwrap();
    assert_eq!(status.state, IdentityTransitionReadState::NotFound);
    assert_eq!(source_transition_effect_counts(), before);
    assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
    assert_eq!(reloads.get(), 0);
    assert_eq!(std::fs::read_dir(&config).unwrap().count(), 0);
    assert_eq!(
        std::fs::read_dir(project.join(".ac/coding-agents"))
            .unwrap()
            .count(),
        0
    );
    let mut ordinary = settings.clone();
    let agent: AgentConfig = serde_json::from_value(serde_json::json!({
        "id":"custom-ordinary","label":"Ordinary","color":"#123456","command":"ordinary-command"}))
    .unwrap();
    apply_coding_agent_op(&mut ordinary, &CodingAgentOp::Add { agent }).unwrap();
    let saved = crate::config::settings::save_settings_to_path_preserving_project_paths(
        &ordinary,
        &config.join("settings.json"),
    )
    .unwrap();
    assert!(saved
        .agents
        .iter()
        .any(|agent| agent.id == "custom-ordinary"));
    let registered: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME))
            .unwrap(),
    )
    .unwrap();
    assert!(registered["agents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|agent| agent["id"] == "custom-ordinary"));
    assert_eq!(source_transition_effect_counts(), before);
    assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
    assert!(crate::config::settings::read_own_source_journal(&context)
        .unwrap()
        .1
        .intents
        .is_empty());
}

#[test]
fn single_editor_status_unknown_inactive_zero_write_without_sidecars() {
    use crate::config::settings::SourceReadContext;
    let root = crate::config::settings::source_transition_test_root();
    let settings = AppSettings::default();
    let context =
        SourceReadContext::from_settings(&settings, &root.path().join("settings.json")).unwrap();
    let source = crate::config::settings::SourceRef {
        family: crate::config::settings::SourceFamily::Agents,
        role: crate::config::settings::SourceRole::Registered,
        scope: crate::config::settings::SourceScope::Instance,
        kind: crate::config::settings::SourceKind::RegisteredInstance,
        context_root: std::fs::canonicalize(root.path())
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    };
    for _ in 0..3 {
        let read = get_identity_transition(&context, &source, "unknown-operation").unwrap();
        assert_eq!(read.state, IdentityTransitionReadState::NotFound);
        assert!(read.operation_id.is_none());
        assert!(!read.active);
        assert!(read.status.summary.is_none());
        assert_eq!(
            read.status.journal_revision,
            crate::config::settings::SourceRevision::Absent {}
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn single_editor_status_invalid_present_journal_preserved_not_empty() {
    use crate::config::settings::{SourceCoordinationError, SourceReadContext};
    let root = crate::config::settings::source_transition_test_root();
    let settings = AppSettings::default();
    let context =
        SourceReadContext::from_settings(&settings, &root.path().join("settings.json")).unwrap();
    let source = crate::config::settings::SourceRef {
        family: crate::config::settings::SourceFamily::Agents,
        role: crate::config::settings::SourceRole::Registered,
        scope: crate::config::settings::SourceScope::Instance,
        kind: crate::config::settings::SourceKind::RegisteredInstance,
        context_root: std::fs::canonicalize(root.path())
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    };
    let journal = root
        .path()
        .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
    std::fs::write(&journal, b"{}").unwrap();
    assert!(matches!(
        get_identity_transition(&context, &source, "unknown-operation"),
        Err(SourceCoordinationError::Invalid)
    ));
    assert_eq!(std::fs::read(&journal).unwrap(), b"{}");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: label.to_string(),
        command: command.to_string(),
        color: "#6366f1".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

fn add(agent: AgentConfig) -> CodingAgentOp {
    CodingAgentOp::Add { agent }
}

#[test]
fn issue_2859_preflight_add_and_patch_preserve_supplied_and_dormant_fields() {
    let mut settings = AppSettings::default();
    let mut row = agent("first", "First", "codex");
    row.preflight = Some(" raw  command ".into());
    row.preflight_timeout_seconds = Some(123);
    apply_coding_agent_op(&mut settings, &add(row.clone())).unwrap();
    row.id = "second".into();
    row.label = "Second".into();
    row.preflight = None;
    row.preflight_timeout_seconds = Some(60);
    apply_coding_agent_op(&mut settings, &add(row)).unwrap();
    apply_coding_agent_op(
        &mut settings,
        &CodingAgentOp::Update {
            id: "first".into(),
            patch: AgentPatch {
                label: Some("Renamed".into()),
                ..Default::default()
            },
        },
    )
    .unwrap();
    assert_eq!(
        settings.agents[0].preflight.as_deref(),
        Some(" raw  command ")
    );
    assert_eq!(settings.agents[0].preflight_timeout_seconds, Some(123));
    assert!(settings.agents[1].preflight.is_none());
    assert_eq!(settings.agents[1].preflight_timeout_seconds, Some(60));
}

#[test]
fn issue_2859_preflight_request_save_path_rejects_zero_and_persists_valid_values() {
    for (timeout, accepted) in [(0, false), (60, true), (u32::MAX, true)] {
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let persisted = dir.path().join("saved.json");
        let mut row = agent("row", "Row", "codex");
        row.preflight = Some(" raw  command ".into());
        row.preflight_timeout_seconds = Some(timeout);
        let path = write_request_file(&requests, "preflight", 100, add(row));
        let mut settings = AppSettings::default();
        let mut save = |settings: &AppSettings| {
            std::fs::write(&persisted, serde_json::to_vec(settings).unwrap())
                .map_err(|error| error.to_string())?;
            Ok(settings.clone())
        };
        let disposition =
            process_coding_agent_request(&path, &results, 200, &mut settings, &mut save);
        assert_eq!(
            matches!(disposition, RequestDisposition::Applied { .. }),
            accepted
        );
        assert_eq!(persisted.exists(), accepted);
        let result = read_coding_agent_result(&results, "preflight").unwrap();
        assert_eq!(result.ok, accepted);
        if accepted {
            let saved: AppSettings =
                serde_json::from_slice(&std::fs::read(&persisted).unwrap()).unwrap();
            assert_eq!(saved.agents[0].preflight.as_deref(), Some(" raw  command "));
            assert_eq!(saved.agents[0].preflight_timeout_seconds, Some(timeout));
        } else {
            let error = result.error.unwrap();
            assert!(error.contains("row") && error.contains("preflightTimeoutSeconds"));
            assert!(!error.contains("raw  command"));
        }
    }
}

#[test]
fn issue_2859_preflight_save_path_validates_dormant_profile_cells_before_save() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let mut settings = AppSettings::default();
    apply_coding_agent_op(&mut settings, &add(agent("row", "Row", "codex"))).unwrap();
    let cell = settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("row")
        .unwrap()
        .get_mut("A")
        .unwrap();
    cell.enabled = false;
    cell.preflight_timeout_seconds = Some(0);
    let path = write_request_file(
        &requests,
        "profile-zero",
        100,
        CodingAgentOp::Update {
            id: "row".into(),
            patch: AgentPatch::default(),
        },
    );
    let mut saves = 0;
    let disposition =
        process_coding_agent_request(&path, &results, 200, &mut settings, &mut |st| {
            saves += 1;
            Ok(st.clone())
        });
    assert_eq!(disposition, RequestDisposition::Rejected);
    assert_eq!(saves, 0);
    let error = read_coding_agent_result(&results, "profile-zero")
        .unwrap()
        .error
        .unwrap();
    assert!(
        error.contains("row")
            && error.contains("profile 'A'")
            && error.contains("preflightTimeoutSeconds")
    );
}

#[test]
fn id_hash_activation_apply_uses_final_command_env_and_metadata_exclusions() {
    let mut settings = AppSettings::default();
    let mut row = agent("stable", "Stable", "codex");
    row.id_hash = Some("forged".into());
    row.backend.image = Some("discard-local-image".into());
    let added = apply_coding_agent_op(&mut settings, &add(row))
        .unwrap()
        .agent
        .unwrap();
    assert_eq!(
        serde_json::to_value(&added).unwrap(),
        serde_json::to_value(&settings.agents[0]).unwrap()
    );
    assert!(added.backend.image.is_none());
    assert_eq!(added.id_hash_debug, Some(vec!["codex".into()]));
    let original_hash = added.id_hash.clone();
    let metadata = AgentPatch {
        label: Some("Renamed".into()),
        color: Some("#abc123".into()),
        isolated_home: Some(true),
        ..Default::default()
    };
    let changed = apply_coding_agent_op(
        &mut settings,
        &CodingAgentOp::Update {
            id: "stable".into(),
            patch: metadata,
        },
    )
    .unwrap()
    .agent
    .unwrap();
    assert_eq!(changed.id_hash, original_hash);
    assert_eq!(changed.id_hash_debug, Some(vec!["codex".into()]));
    let disabled = CodingAgentEnv {
        key: "MODE".into(),
        value: "one".into(),
        source: CodingAgentEnvSource::User,
        enabled: false,
    };
    let updated = apply_coding_agent_op(
        &mut settings,
        &CodingAgentOp::Update {
            id: "stable".into(),
            patch: AgentPatch {
                command: Some("codex x".into()),
                envs: Some(vec![disabled]),
                ..Default::default()
            },
        },
    )
    .unwrap()
    .agent
    .unwrap();
    assert_eq!(updated.id, "stable");
    assert_eq!(
        updated.id_hash,
        Some(crate::config::settings::configuration_identity(
            &updated.command,
            &updated.envs
        ))
    );
    assert_ne!(updated.id_hash, original_hash);
    assert_eq!(
        updated.id_hash_debug,
        Some(vec!["codex x".into(), "MODE".into(), "one".into()])
    );
    let disabled_hash = updated.id_hash;
    settings.agents[0].envs[0].value = "two".into();
    let updated = apply_coding_agent_op(
        &mut settings,
        &CodingAgentOp::Update {
            id: "stable".into(),
            patch: AgentPatch::default(),
        },
    )
    .unwrap()
    .agent
    .unwrap();
    assert_ne!(updated.id_hash, disabled_hash);
    assert_eq!(
        updated.id_hash_debug,
        Some(vec!["codex x".into(), "MODE".into(), "two".into()])
    );
    let cleared = apply_coding_agent_op(
        &mut settings,
        &CodingAgentOp::Update {
            id: "stable".into(),
            patch: AgentPatch {
                envs: Some(vec![]),
                ..Default::default()
            },
        },
    )
    .unwrap()
    .agent
    .unwrap();
    assert!(cleared.envs.is_empty());
    assert_eq!(cleared.id_hash_debug, Some(vec!["codex x".into()]));
    assert_eq!(
        cleared.id_hash,
        Some(crate::config::settings::configuration_identity(
            "codex x",
            &[]
        ))
    );
}

#[test]
fn id_hash_activation_queued_real_saver_result_tracks_written_and_reload() {
    let dir = crate::config::settings::source_transition_test_root();
    let settings_path = dir.path().join("settings.json");
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let mut settings = AppSettings::default();
    let mut input = agent("stable", "Stable", "codex");
    input.id_hash = Some("forged-input".into());
    for (request_id, op) in [
        ("add", add(input)),
        (
            "clear",
            CodingAgentOp::Update {
                id: "stable".into(),
                patch: AgentPatch {
                    envs: Some(vec![]),
                    ..Default::default()
                },
            },
        ),
        (
            "remove",
            CodingAgentOp::Remove {
                id: "stable".into(),
            },
        ),
    ] {
        let request = write_request_file(&requests, request_id, 100, op);
        let mut captured = None;
        let disposition = process_coding_agent_request(
            &request,
            &results,
            200,
            &mut settings,
            &mut |candidate| {
                let mut physical = candidate.clone();
                if let Some(row) = physical.agents.first_mut() {
                    // A real saver must be the authority even if its final inputs differ.
                    row.command = "final-saver-command".into();
                    row.id_hash = Some("forged-before-save".into());
                }
                let written =
                    crate::config::settings::save_settings_to_path_preserving_project_paths(
                        &physical,
                        &settings_path,
                    )?;
                captured = Some(written.clone());
                Ok(written)
            },
        );
        assert!(matches!(disposition, RequestDisposition::Applied { .. }));
        let result = read_coding_agent_result(&results, request_id).unwrap();
        assert!(result.ok);
        let written = captured.unwrap();
        let reload = crate::config::settings::load_settings_from_path(&settings_path);
        assert_eq!(
            serde_json::to_value(&settings.agents).unwrap(),
            serde_json::to_value(&written.agents).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&reload.agents).unwrap(),
            serde_json::to_value(&written.agents).unwrap()
        );
        if request_id == "remove" {
            assert!(result.agent.is_none());
            assert_eq!(result.removed_id.as_deref(), Some("stable"));
        } else {
            assert_eq!(
                serde_json::to_value(result.agent.as_ref()).unwrap(),
                serde_json::to_value(written.agents.first()).unwrap()
            );
            assert_eq!(result.agent.unwrap().command, "final-saver-command");
            assert_eq!(
                written.agents[0].id_hash_debug,
                Some(vec!["final-saver-command".into()])
            );
            let disk: serde_json::Value = serde_json::from_slice(
                &std::fs::read(
                    dir.path()
                        .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                disk["agents"][0]["id_hash"],
                written.agents[0].id_hash.clone().unwrap()
            );
            assert_eq!(
                disk["agents"][0]["id_hash_debug"],
                serde_json::json!(["final-saver-command"])
            );
        }
        assert!(!request.exists() && !processing_path_for(&request).exists());
    }
}

#[test]
fn id_hash_activation_queued_errors_never_publish_success() {
    for failure in [
        "invalid-command",
        "invalid-env",
        "bad-disk",
        "missing-written-row",
    ] {
        let dir = crate::config::settings::source_transition_test_root();
        let path = dir.path().join("settings.json");
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let mut row = agent("stable", "Stable", "codex");
        if failure == "invalid-command" {
            row.command = "codex resume --last".into();
        }
        if failure == "invalid-env" {
            row.envs = vec![CodingAgentEnv {
                key: "AGENTSCOMMANDER_TOKEN".into(),
                value: "bad".into(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            }];
        }
        let sidecar = dir
            .path()
            .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
        if failure == "bad-disk" {
            std::fs::write(&sidecar, b"{ malformed").unwrap();
        }
        let request = write_request_file(&requests, "rejected", 100, add(row));
        let mut candidate = AppSettings::default();
        let disposition = process_coding_agent_request(
            &request,
            &results,
            200,
            &mut candidate,
            &mut |settings| {
                let mut physical = settings.clone();
                if failure == "missing-written-row" {
                    physical.agents.clear();
                }
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    &physical, &path,
                )
            },
        );
        assert_eq!(disposition, RequestDisposition::Rejected, "{failure}");
        let result = read_coding_agent_result(&results, "rejected").unwrap();
        assert!(!result.ok && result.agent.is_none(), "{failure}");
        assert!(result.error.is_some());
        if failure == "bad-disk" {
            assert_eq!(std::fs::read(&sidecar).unwrap(), b"{ malformed");
        }
        if failure.starts_with("invalid-") {
            assert!(!path.exists() && !sidecar.exists());
        }
        assert!(!processing_path_for(&request).exists());
    }
}

fn noop_save() -> impl FnMut(&AppSettings) -> Result<AppSettings, String> {
    |s: &AppSettings| Ok(s.clone())
}

// ---- id / color validators ---------------------------------------------

#[test]
fn minted_id_conforms_and_is_lowercase() {
    let id = mint_agent_id();
    assert!(validate_custom_agent_id(&id).is_ok(), "minted id: {id}");
    assert_eq!(id, id.to_lowercase());
}

#[test]
fn custom_id_rules() {
    assert!(validate_custom_agent_id("claude-1").is_ok());
    assert!(validate_custom_agent_id("agent_123_ab12cd").is_ok());
    assert!(validate_custom_agent_id("").is_err());
    assert!(
        validate_custom_agent_id("Claude").is_err(),
        "uppercase rejected"
    );
    assert!(
        validate_custom_agent_id("-foo").is_err(),
        "leading hyphen rejected"
    );
    assert!(validate_custom_agent_id("has space").is_err());
    assert!(
        validate_custom_agent_id(&"a".repeat(65)).is_err(),
        "over 64 rejected"
    );
    assert!(
        validate_custom_agent_id(&"a".repeat(64)).is_ok(),
        "64 allowed"
    );
}

#[test]
fn color_rules() {
    assert!(validate_agent_color("#6366f1").is_ok());
    assert!(validate_agent_color("#FFFFFF").is_ok());
    assert!(validate_agent_color("#fff").is_err());
    assert!(validate_agent_color("#6366f1ff").is_err());
    assert!(validate_agent_color("6366f1").is_err());
    assert!(validate_agent_color("red").is_err());
}

// ---- add ---------------------------------------------------------------

#[test]
fn add_appends_and_repairs_a_cell() {
    let mut s = AppSettings::default();
    let out = apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
    assert_eq!(out.op, "add");
    assert_eq!(s.agents.len(), 1);
    // repair ensures an 'A' profile cell exists for the new agent.
    assert!(s
        .coding_agent_profiles
        .profiles_by_agent
        .get("claude")
        .map(|cells| cells.contains_key("A"))
        .unwrap_or(false));
}

#[test]
fn add_rejects_duplicate_id() {
    let mut s = AppSettings::default();
    apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
    let err = apply_coding_agent_op(&mut s, &add(agent("claude", "Other", "codex"))).unwrap_err();
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn add_rejects_case_insensitive_dup_against_legacy_mixedcase_id() {
    let mut s = AppSettings::default();
    // A legacy agent with a mixed-case id predates the new lowercase-only
    // rule; insert it directly to bypass validate_custom_agent_id. Adding a
    // valid lowercase id that collides case-insensitively must be rejected
    // (find_launch_agent resolves ids case-insensitively).
    s.agents.push(agent("Claude", "Legacy", "claude"));
    let err = apply_coding_agent_op(&mut s, &add(agent("claude", "New", "codex"))).unwrap_err();
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn add_rejects_bad_id_and_empty_label() {
    let mut s = AppSettings::default();
    assert!(apply_coding_agent_op(&mut s, &add(agent("Bad Id", "L", "claude"))).is_err());
    assert!(apply_coding_agent_op(&mut s, &add(agent("ok", "  ", "claude"))).is_err());
}

#[test]
fn add_rejects_banned_continue_and_dup_env() {
    let mut s = AppSettings::default();
    let err =
        apply_coding_agent_op(&mut s, &add(agent("c", "C", "claude --continue"))).unwrap_err();
    assert!(err.contains("continue"), "{err}");

    let mut dup = agent("d", "D", "claude");
    dup.envs = vec![
        CodingAgentEnv {
            key: "FOO".into(),
            value: "1".into(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
        CodingAgentEnv {
            key: "FOO".into(),
            value: "2".into(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        },
    ];
    let mut s2 = AppSettings::default();
    assert!(apply_coding_agent_op(&mut s2, &add(dup)).is_err());
}

#[test]
fn add_rejects_enabled_seed_without_dest() {
    let mut s = AppSettings::default();
    let mut a = agent("s", "S", "claude");
    a.config_seed = Some(ConfigSeedConfig {
        enabled: true,
        dest: String::new(),
    });
    assert!(apply_coding_agent_op(&mut s, &add(a)).is_err());
}

#[test]
fn add_normalizes_backend_image() {
    let mut s = AppSettings::default();
    let mut a = agent("container", "Container", "claude");
    a.backend.kind = SessionBackendKind::ContainerTransport;
    a.backend.image = Some(" image:tag ".to_string());

    apply_coding_agent_op(&mut s, &add(a)).unwrap();

    assert_eq!(
        s.agents[0].backend.kind,
        SessionBackendKind::ContainerTransport
    );
    assert_eq!(s.agents[0].backend.image.as_deref(), Some("image:tag"));
}

#[test]
fn add_clears_local_backend_image_and_rejects_leading_dash() {
    let mut local_settings = AppSettings::default();
    let mut local = agent("local", "Local", "claude");
    local.backend.image = Some(" image:tag ".to_string());
    apply_coding_agent_op(&mut local_settings, &add(local)).unwrap();
    assert!(local_settings.agents[0].backend.image.is_none());

    let mut bad_settings = AppSettings::default();
    let mut bad = agent("bad", "Bad", "claude");
    bad.backend.kind = SessionBackendKind::ContainerTransport;
    bad.backend.image = Some("--privileged".to_string());
    let err = apply_coding_agent_op(&mut bad_settings, &add(bad)).unwrap_err();
    assert!(err.contains("must not start with"), "{err}");
}

#[test]
fn add_accepts_catalog_seeded_non_hex_color() {
    // §14.4 R6 carve-out: the op layer NEVER validates color (only the
    // explicit `--color` CLI flag does, in cmd_add). A catalog-seeded agent
    // whose color is a shorthand like "#fff" must be accepted as-is so that
    // `add --from-catalog <key>` never rejects a trusted catalog color.
    // Guards against a future regression that adds a color check here.
    let mut s = AppSettings::default();
    let mut a = agent("cat", "Catalog Agent", "claude");
    a.color = "#fff".to_string();
    let out = apply_coding_agent_op(&mut s, &add(a)).unwrap();
    assert_eq!(out.op, "add");
    assert_eq!(s.agents[0].color, "#fff", "catalog color persisted as-is");
}

// ---- update ------------------------------------------------------------

#[test]
fn update_partial_keeps_other_fields() {
    let mut s = AppSettings::default();
    apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
    let patch = AgentPatch {
        label: Some("Renamed".into()),
        ..Default::default()
    };
    apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "claude".into(),
            patch,
        },
    )
    .unwrap();
    assert_eq!(s.agents[0].label, "Renamed");
    assert_eq!(s.agents[0].command, "claude");
}

#[test]
fn update_env_replace_all_and_clear() {
    let mut s = AppSettings::default();
    let mut a = agent("claude", "Claude", "claude");
    a.envs = vec![CodingAgentEnv {
        key: "OLD".into(),
        value: "1".into(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    apply_coding_agent_op(&mut s, &add(a)).unwrap();

    let patch = AgentPatch {
        envs: Some(vec![CodingAgentEnv {
            key: "NEW".into(),
            value: "2".into(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }]),
        ..Default::default()
    };
    apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "claude".into(),
            patch,
        },
    )
    .unwrap();
    assert_eq!(s.agents[0].envs.len(), 1);
    assert_eq!(s.agents[0].envs[0].key, "NEW");

    let clear = AgentPatch {
        envs: Some(Vec::new()),
        ..Default::default()
    };
    apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "claude".into(),
            patch: clear,
        },
    )
    .unwrap();
    assert!(s.agents[0].envs.is_empty());
}

#[test]
fn update_unknown_id_lists_available() {
    let mut s = AppSettings::default();
    apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
    let err = apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "nope".into(),
            patch: AgentPatch::default(),
        },
    )
    .unwrap_err();
    assert!(err.contains("not found"), "{err}");
    assert!(err.contains("claude"), "{err}");
}

#[test]
fn update_clear_instructions_and_seed() {
    let mut s = AppSettings::default();
    let mut a = agent("claude", "Claude", "claude");
    a.instructions_filename = Some("CLAUDE.md".into());
    a.config_seed = Some(ConfigSeedConfig {
        enabled: true,
        dest: ".claude".into(),
    });
    apply_coding_agent_op(&mut s, &add(a)).unwrap();

    let patch = AgentPatch {
        clear_instructions_filename: true,
        clear_config_seed: true,
        ..Default::default()
    };
    apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "claude".into(),
            patch,
        },
    )
    .unwrap();
    assert!(s.agents[0].instructions_filename.is_none());
    assert!(s.agents[0].config_seed.is_none());
}

#[test]
fn update_sets_and_clears_container_image() {
    let mut s = AppSettings::default();
    apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();

    let patch = AgentPatch {
        backend_kind: Some(SessionBackendKind::ContainerTransport),
        container_image: Some(" image:tag ".into()),
        ..Default::default()
    };
    apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "claude".into(),
            patch,
        },
    )
    .unwrap();
    assert_eq!(
        s.agents[0].backend.kind,
        SessionBackendKind::ContainerTransport
    );
    assert_eq!(s.agents[0].backend.image.as_deref(), Some("image:tag"));

    let clear = AgentPatch {
        clear_container_image: true,
        ..Default::default()
    };
    apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "claude".into(),
            patch: clear,
        },
    )
    .unwrap();
    assert!(s.agents[0].backend.image.is_none());

    let local = AgentPatch {
        backend_kind: Some(SessionBackendKind::LocalProcess),
        container_image: Some("remembered:tag".into()),
        ..Default::default()
    };
    apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "claude".into(),
            patch: local,
        },
    )
    .unwrap();
    assert_eq!(s.agents[0].backend.kind, SessionBackendKind::LocalProcess);
    assert!(s.agents[0].backend.image.is_none());
}

#[test]
fn update_rejects_leading_dash_container_image() {
    let mut s = AppSettings::default();
    apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();

    let patch = AgentPatch {
        backend_kind: Some(SessionBackendKind::ContainerTransport),
        container_image: Some("--volume=/:/host".into()),
        ..Default::default()
    };
    let err = apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Update {
            id: "claude".into(),
            patch,
        },
    )
    .unwrap_err();
    assert!(err.contains("must not start with"), "{err}");
}

// ---- remove (orphan-preserve) ------------------------------------------

#[test]
fn remove_preserves_profile_orphans() {
    let mut s = AppSettings::default();
    apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
    // repair created profiles_by_agent["claude"]; removing the agent must
    // leave that orphan in place (matches repair/GUI semantics).
    assert!(s
        .coding_agent_profiles
        .profiles_by_agent
        .contains_key("claude"));
    let out = apply_coding_agent_op(
        &mut s,
        &CodingAgentOp::Remove {
            id: "claude".into(),
        },
    )
    .unwrap();
    assert_eq!(out.removed_id.as_deref(), Some("claude"));
    assert!(s.agents.is_empty());
    assert!(
        s.coding_agent_profiles
            .profiles_by_agent
            .contains_key("claude"),
        "profile orphan must survive remove"
    );
}

#[test]
fn remove_unknown_id_errors() {
    let mut s = AppSettings::default();
    assert!(apply_coding_agent_op(&mut s, &CodingAgentOp::Remove { id: "ghost".into() }).is_err());
}

// ---- purity ------------------------------------------------------------

#[test]
fn apply_is_pure_no_disk() {
    // A bare AppSettings with no config dir override: apply must not touch
    // disk (it takes &mut and validates only). If it saved, tests would need
    // a config dir; they do not.
    let mut s = AppSettings::default();
    apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
    assert_eq!(s.agents.len(), 1);
}

// ---- queue helpers -----------------------------------------------------

#[test]
fn is_expired_backward_clock_and_old() {
    // future createdAt (backward clock) -> not expired
    assert!(!is_expired(1_000_000, 900_000));
    // fresh -> not expired
    assert!(!is_expired(1_000_000, 1_000_100));
    // genuinely old -> expired
    assert!(is_expired(0, REQUEST_EXPIRY_MS + 1));
}

#[test]
fn request_result_roundtrip_atomic() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let req = CodingAgentRequest {
        request_id: "req1".into(),
        created_at_ms: 123,
        op: add(agent("claude", "Claude", "claude")),
    };
    let path = write_coding_agent_request(&requests, &req).unwrap();
    assert!(path.exists());
    // no leftover temp file
    assert!(std::fs::read_dir(&requests).unwrap().all(|e| {
        let p = e.unwrap().path();
        p.extension().and_then(|s| s.to_str()) == Some("json")
    }));

    let results = requests.join(RESULTS_SUBDIR);
    write_coding_agent_result(
        &results,
        &CodingAgentResult {
            request_id: "req1".into(),
            ok: true,
            op: Some("add".into()),
            error: None,
            agent: None,
            removed_id: None,
        },
    )
    .unwrap();
    let read = read_coding_agent_result(&results, "req1").unwrap();
    assert!(read.ok);
    assert_eq!(read.op.as_deref(), Some("add"), "op round-trips");
}

#[test]
fn first_request_autocreates_results_dir() {
    // R4: brand-new tempdir, results/ does not exist; write_coding_agent_result
    // must create it.
    let dir = crate::config::settings::source_transition_test_root();
    let results = dir
        .path()
        .join(CODING_AGENT_REQUESTS_DIR)
        .join(RESULTS_SUBDIR);
    assert!(!results.exists());
    write_coding_agent_result(
        &results,
        &CodingAgentResult {
            request_id: "x".into(),
            ok: true,
            op: None,
            error: None,
            agent: None,
            removed_id: None,
        },
    )
    .unwrap();
    assert!(results.join("x.json").exists());
}

// ---- per-request handler ----------------------------------------------

fn write_request_file(requests: &Path, id: &str, created_at_ms: u64, op: CodingAgentOp) -> PathBuf {
    std::fs::create_dir_all(requests).unwrap();
    let req = CodingAgentRequest {
        request_id: id.to_string(),
        created_at_ms,
        op,
    };
    let path = requests.join(format!("{id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&req).unwrap()).unwrap();
    path
}

#[test]
fn handler_applies_and_writes_result_and_deletes_processing() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let path = write_request_file(
        &requests,
        "r1",
        100,
        add(agent("claude", "Claude", "claude")),
    );

    let mut settings = AppSettings::default();
    let mut saved = false;
    let disp = {
        let mut save = |_s: &AppSettings| {
            saved = true;
            Ok(_s.clone())
        };
        process_coding_agent_request(&path, &results, 200, &mut settings, &mut save)
    };
    assert!(matches!(
        disp,
        RequestDisposition::Applied { op: "add", .. }
    ));
    assert!(saved, "save callback invoked");
    assert_eq!(settings.agents.len(), 1);
    // result written, request + processing gone
    assert!(read_coding_agent_result(&results, "r1").unwrap().ok);
    assert!(!path.exists());
    assert!(!processing_path_for(&path).exists());
}

#[test]
fn handler_applies_backend_image_patch_from_request_json() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let mut settings = AppSettings::default();
    apply_coding_agent_op(&mut settings, &add(agent("claude", "Claude", "claude"))).unwrap();
    let patch = AgentPatch {
        backend_kind: Some(SessionBackendKind::ContainerTransport),
        container_image: Some(" image:tag ".into()),
        ..Default::default()
    };
    let path = write_request_file(
        &requests,
        "patch",
        100,
        CodingAgentOp::Update {
            id: "claude".into(),
            patch,
        },
    );

    let disp = process_coding_agent_request(&path, &results, 200, &mut settings, &mut noop_save());

    assert!(matches!(disp, RequestDisposition::Applied { .. }));
    assert_eq!(
        settings.agents[0].backend.kind,
        SessionBackendKind::ContainerTransport
    );
    assert_eq!(
        settings.agents[0].backend.image.as_deref(),
        Some("image:tag")
    );
    assert!(read_coding_agent_result(&results, "patch").unwrap().ok);
}

#[test]
fn handler_skips_when_request_already_removed() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    std::fs::create_dir_all(&requests).unwrap();
    let missing = requests.join("gone.json");
    let mut settings = AppSettings::default();
    let disp =
        process_coding_agent_request(&missing, &results, 200, &mut settings, &mut noop_save());
    assert_eq!(disp, RequestDisposition::Skipped);
    assert!(settings.agents.is_empty());
}

#[test]
fn handler_rejects_expired_without_applying() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let path = write_request_file(
        &requests,
        "old",
        0,
        add(agent("claude", "Claude", "claude")),
    );
    let mut settings = AppSettings::default();
    let disp = process_coding_agent_request(
        &path,
        &results,
        REQUEST_EXPIRY_MS + 10,
        &mut settings,
        &mut noop_save(),
    );
    assert_eq!(disp, RequestDisposition::Rejected);
    assert!(settings.agents.is_empty());
    let res = read_coding_agent_result(&results, "old").unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("expired"));
}

#[test]
fn handler_rejects_validation_error() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let path = write_request_file(&requests, "bad", 100, add(agent("BadId", "L", "claude")));
    let mut settings = AppSettings::default();
    let disp = process_coding_agent_request(&path, &results, 150, &mut settings, &mut noop_save());
    assert_eq!(disp, RequestDisposition::Rejected);
    assert!(settings.agents.is_empty());
    assert!(!read_coding_agent_result(&results, "bad").unwrap().ok);
}

// ---- T10: daemon-path launch consumption (acceptance criterion 2) ------

#[test]
fn daemon_applied_agent_is_launch_resolvable() {
    // Apply an Add through the extracted per-request handler (the daemon-path
    // seam), then confirm the new agent is resolvable to a subsequent
    // `--launch` by id AND by label, and that a spawn command builds for it.
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let path = write_request_file(
        &requests,
        "r",
        100,
        add(agent("claude", "Claude Code", "claude")),
    );

    let mut settings = AppSettings::default();
    let disp = process_coding_agent_request(&path, &results, 200, &mut settings, &mut noop_save());
    assert!(matches!(disp, RequestDisposition::Applied { .. }));

    // find_launch_agent (create_agent.rs) resolves by exact id and by label.
    assert!(crate::cli::create_agent::find_launch_agent(&settings, "claude").is_some());
    assert!(crate::cli::create_agent::find_launch_agent(&settings, "Claude Code").is_some());

    // A spawn command builds against the same state (session.rs). Some(_)
    // means the agent was found and the command composed.
    let cwd = dir.path().to_string_lossy();
    let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
        &settings, "claude", &cwd, None,
    );
    assert!(spawn.is_ok(), "spawn build failed: {:?}", spawn.err());
    assert!(
        spawn.unwrap().is_some(),
        "expected a spawn command for the applied agent"
    );
}

// ---- #2306 P1: explicit registered-agent order -------------------------

/// `a,b,c` with contiguous ordinals, the shape the loaders leave behind.
fn ordered_2306() -> AppSettings {
    let mut settings = AppSettings::default();
    for id in ["a", "b", "c"] {
        apply_coding_agent_op(&mut settings, &add(agent(id, &id.to_uppercase(), "claude")))
            .unwrap();
    }
    settings
}

fn ids_2306(settings: &AppSettings) -> Vec<&str> {
    settings.agents.iter().map(|a| a.id.as_str()).collect()
}

fn positions_2306(settings: &AppSettings) -> Vec<Option<u32>> {
    settings.agents.iter().map(|a| a.order).collect()
}

#[test]
fn add_appends_at_the_next_position_and_ignores_a_caller_supplied_order() {
    let mut settings = ordered_2306();
    let mut incoming = agent("d", "D", "claude");
    incoming.order = Some(0); // forged/stale: must have no authority
    let outcome = apply_coding_agent_op(&mut settings, &add(incoming)).unwrap();
    assert_eq!(ids_2306(&settings), ["a", "b", "c", "d"]);
    assert_eq!(
        positions_2306(&settings),
        [Some(0), Some(1), Some(2), Some(3)]
    );
    assert_eq!(outcome.agent.unwrap().order, Some(3));
}

#[test]
fn remove_compacts_positions_and_update_retains_the_current_position() {
    let mut settings = ordered_2306();
    apply_coding_agent_op(&mut settings, &CodingAgentOp::Remove { id: "b".into() }).unwrap();
    assert_eq!(ids_2306(&settings), ["a", "c"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);

    // A patch carrying an unknown `order` key cannot move the tool.
    let patch: AgentPatch = serde_json::from_str(r#"{"label":"Renamed","order":0}"#).unwrap();
    let outcome = apply_coding_agent_op(
        &mut settings,
        &CodingAgentOp::Update {
            id: "c".into(),
            patch,
        },
    )
    .unwrap();
    assert_eq!(ids_2306(&settings), ["a", "c"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);
    assert_eq!(outcome.agent.unwrap().order, Some(1));
    assert_eq!(settings.agents[1].label, "Renamed");
}

#[test]
fn move_swaps_both_directions_and_returns_the_authoritative_order() {
    let mut settings = ordered_2306();
    let ids = move_registered_agent(&mut settings, "c", "b", AgentMoveDirection::Up).unwrap();
    assert_eq!(ids, ["a", "c", "b"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);

    let ids = move_registered_agent(&mut settings, "c", "b", AgentMoveDirection::Down).unwrap();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);
}

#[test]
fn move_derives_the_effective_order_before_the_adjacency_check() {
    // Vector slots are [b, a] but the stored ordinals put a first; the move must
    // act on the effective order [a, b], and normalize the result.
    let mut settings = AppSettings::default();
    apply_coding_agent_op(&mut settings, &add(agent("a", "A", "claude"))).unwrap();
    apply_coding_agent_op(&mut settings, &add(agent("b", "B", "claude"))).unwrap();
    settings.agents.swap(0, 1);
    settings.agents[0].order = Some(1);
    settings.agents[1].order = Some(0);

    let ids = move_registered_agent(&mut settings, "b", "a", AgentMoveDirection::Up).unwrap();
    assert_eq!(ids, ["b", "a"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);
}

#[test]
fn rejected_moves_leave_the_live_settings_untouched() {
    let cases = [
        ("a", "b", AgentMoveDirection::Up, "top"),
        ("c", "b", AgentMoveDirection::Down, "bottom"),
        ("b", "a", AgentMoveDirection::Down, "not adjacent"),
        ("missing", "a", AgentMoveDirection::Up, "not found"),
        ("a", "missing", AgentMoveDirection::Down, "not found"),
    ];
    for (id, neighbor, direction, expected) in cases {
        let mut settings = ordered_2306();
        let ids_before: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
        let positions_before = positions_2306(&settings);
        let error = move_registered_agent(&mut settings, id, neighbor, direction).unwrap_err();
        assert!(
            error.contains(expected),
            "case ({id},{neighbor}): error {error:?} must mention {expected:?}"
        );
        let ids_after: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids_after, ids_before, "case ({id},{neighbor})");
        assert_eq!(
            positions_2306(&settings),
            positions_before,
            "case ({id},{neighbor})"
        );
    }
}

#[test]
fn move_rejects_duplicate_ids_while_case_distinct_ids_stay_distinct() {
    let mut duplicated = AppSettings {
        agents: vec![agent("dup", "D1", "claude"), agent("dup", "D2", "claude")],
        ..AppSettings::default()
    };
    let error =
        move_registered_agent(&mut duplicated, "dup", "dup", AgentMoveDirection::Up).unwrap_err();
    assert!(error.contains("more than once"), "{error}");

    // Case-distinct ids are distinct records for ordering (Add keeps rejecting
    // case-insensitive duplicates, so this legacy pair is inserted directly).
    let mut upper = agent("A", "Upper", "claude");
    upper.order = Some(0);
    let mut lower = agent("a", "Lower", "claude");
    lower.order = Some(1);
    let mut cased = AppSettings {
        agents: vec![upper, lower],
        ..AppSettings::default()
    };
    let ids = move_registered_agent(&mut cased, "a", "A", AgentMoveDirection::Up).unwrap();
    assert_eq!(ids, ["a", "A"]);
}

fn reorder_fixture_2542() -> AppSettings {
    let mut settings = AppSettings::default();
    for id in ["a", "b", "c", "d", "e"] {
        apply_coding_agent_op(&mut settings, &add(agent(id, &id.to_uppercase(), "claude")))
            .unwrap();
    }
    settings
}

fn owned_ids_2542(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn reorder_moves_multiple_rows_down_and_up_in_one_step() {
    let mut settings = reorder_fixture_2542();
    let expected = owned_ids_2542(&["a", "b", "c", "d", "e"]);
    let ids = reorder_registered_agent(&mut settings, "b", &expected, 3).unwrap();
    assert_eq!(ids, ["a", "c", "d", "b", "e"]);
    assert_eq!(ids_2306(&settings), ["a", "c", "d", "b", "e"]);

    let ids = reorder_registered_agent(&mut settings, "e", &ids, 1).unwrap();
    assert_eq!(ids, ["a", "e", "c", "d", "b"]);
    assert_eq!(
        positions_2306(&settings),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );
}

#[test]
fn reorder_reaches_both_ends_with_contiguous_positions() {
    let mut settings = reorder_fixture_2542();
    let expected = owned_ids_2542(&["a", "b", "c", "d", "e"]);
    let ids = reorder_registered_agent(&mut settings, "d", &expected, 0).unwrap();
    assert_eq!(ids, ["d", "a", "b", "c", "e"]);
    assert_eq!(
        positions_2306(&settings),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );

    let ids = reorder_registered_agent(&mut settings, "a", &ids, 4).unwrap();
    assert_eq!(ids, ["d", "b", "c", "e", "a"]);
    assert_eq!(
        positions_2306(&settings),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );
}

#[test]
fn reorder_rejections_leave_the_live_settings_byte_identical() {
    let fresh = ["a", "b", "c", "d", "e"];
    let cases: [(&str, &[&str], usize, &str); 5] = [
        ("b", &["a", "x", "c", "d", "e"], 3, "changed since"),
        ("b", &["b", "a", "c", "d", "e"], 3, "changed since"),
        ("missing", &fresh, 1, "not found"),
        ("b", &fresh, 5, "out of range"),
        ("b", &fresh, 1, "nothing to reorder"),
    ];
    for (id, expected_ids, target, needle) in cases {
        let mut settings = reorder_fixture_2542();
        let before = serde_json::to_string(&settings.agents).unwrap();
        let error =
            reorder_registered_agent(&mut settings, id, &owned_ids_2542(expected_ids), target)
                .unwrap_err();
        assert!(
            error.contains(needle),
            "case ({id},{target}): error {error:?} must mention {needle:?}"
        );
        assert_eq!(
            serde_json::to_string(&settings.agents).unwrap(),
            before,
            "case ({id},{target})"
        );
    }

    let mut duplicated = AppSettings {
        agents: vec![agent("dup", "D1", "claude"), agent("dup", "D2", "claude")],
        ..AppSettings::default()
    };
    let before = serde_json::to_string(&duplicated.agents).unwrap();
    let error =
        reorder_registered_agent(&mut duplicated, "dup", &owned_ids_2542(&["dup", "dup"]), 1)
            .unwrap_err();
    assert!(error.contains("more than once"), "{error}");
    assert_eq!(serde_json::to_string(&duplicated.agents).unwrap(), before);
}

#[test]
fn reorder_derives_the_effective_order_from_unnormalized_input() {
    // Vector slots are [a, b, c] but the stored ordinals say [c, a, b]; the
    // caller's view and the move act on that effective order.
    let mut settings = ordered_2306();
    settings.agents[0].order = Some(1);
    settings.agents[1].order = Some(2);
    settings.agents[2].order = Some(0);

    let ids =
        reorder_registered_agent(&mut settings, "c", &owned_ids_2542(&["c", "a", "b"]), 2).unwrap();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);
}

mod p02_patch_schema;
mod source_transition_driver;
