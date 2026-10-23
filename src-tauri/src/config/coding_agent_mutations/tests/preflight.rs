use super::*;

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
