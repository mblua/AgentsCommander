use super::*;

#[test]
fn issue_1937_creation_materializes_matrix_default_on_first_creation() {
    let fixture = replica_creation_fixture();
    write_matrix_default(
        &fixture,
        serde_json::json!({
            "tooling": {
                "defaultProfile": "A",
                "replicaSelectionDefault": {
                    "codingAgentId": "codex",
                    "requestedProfile": "b",
                    "selectionLocked": true
                }
            }
        }),
    );

    let replica_dir = create_replica(&fixture).expect("create replica");
    let config = replica_config(&replica_dir);

    assert_eq!(config["identity"], "../../_agent_tech-lead");
    assert_eq!(config["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(config["tooling"]["profile"], "B");
    assert_eq!(config["tooling"]["instanceProfileOverride"], "B");
    assert_eq!(config["tooling"]["instanceProfileOverrideSource"], "manual");
    assert_eq!(config["tooling"]["selectionLocked"], true);
    // The Matrix profile-only default stays independent of the replica.
    assert!(config["tooling"].get("defaultProfile").is_none());
    let context = config["context"].as_array().expect("context array");
    assert!(context
        .iter()
        .any(|entry| entry.as_str() == Some("../../_agent_tech-lead/Role.md")));
}

#[test]
fn issue_1937_creation_absent_default_keeps_old_creation_shape() {
    let fixture = replica_creation_fixture();

    let replica_dir = create_replica(&fixture).expect("create replica");
    let config = replica_config(&replica_dir);

    assert_eq!(config["identity"], "../../_agent_tech-lead");
    assert!(config.get("tooling").is_none(), "{config}");
    assert!(config["repos"].is_array());
    let context = config["context"].as_array().expect("context array");
    assert_eq!(context[0], "$AGENTSCOMMANDER_CONTEXT");
}

#[test]
fn issue_1937_creation_existing_config_skips_reading_the_default() {
    let fixture = replica_creation_fixture();
    let replica_dir = create_replica(&fixture).expect("create replica");

    // The replica already carries a selection; the Matrix default is now
    // invalid and names a different pair. An existing config must not read
    // it, so the retry still succeeds and keeps the saved selection.
    write_matrix_default(
        &fixture,
        serde_json::json!({
            "tooling": {
                "replicaSelectionDefault": {
                    "codingAgentId": "claude",
                    "requestedProfile": "C"
                }
            }
        }),
    );
    let mut config = replica_config(&replica_dir);
    config["tooling"] = serde_json::json!({
        "currentCodingAgent": "codex",
        "profile": "B",
        "selectionLocked": true,
        "lastCodingAgent": "claude"
    });
    config["customTopLevel"] = serde_json::json!({"keep": true});
    std::fs::write(
        replica_dir.join("config.json"),
        serde_json::to_string(&config).expect("serialize"),
    )
    .expect("write replica config");

    create_replica(&fixture).expect("existing config must skip default reading");

    let saved = replica_config(&replica_dir);
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "B");
    assert_eq!(saved["tooling"]["selectionLocked"], true);
    assert_eq!(
        replica_state(&replica_dir)["tooling"]["lastCodingAgent"],
        "claude"
    );
    assert!(saved["tooling"].get("lastCodingAgent").is_none());
    assert_eq!(saved["customTopLevel"]["keep"], true);
    assert_eq!(saved["identity"], "../../_agent_tech-lead");
}

/// #2816 (C3) E3b, site 7.
#[test]
fn an_absent_config_still_refuses_and_creates_nothing() {
    let fixture = replica_creation_fixture();
    let replica_dir = fixture.wg_dir.join("__agent_tech-lead");
    std::fs::create_dir_all(&replica_dir).expect("create replica dir");
    let config = replica_dir.join("config.json");

    let error = apply_wg_replica_repos(&replica_dir, &[]).expect_err("absent config");

    assert_eq!(
        error,
        format!("Local config {} does not exist", config.display())
    );
    assert!(!config.exists(), "site 7 created config.json");
    assert!(
        !replica_dir
            .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
            .exists(),
        "site 7 created a state file"
    );
}

/// #2816 (C3) E4, site 5, through its nearest reachable caller.
#[test]
fn matrix_creation_writes_the_decisions_file_and_no_state_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
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

    assert_eq!(
        replica_config(&created.agent_dir),
        default_agent_matrix_config()
    );
    assert!(
        !created
            .agent_dir
            .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
            .exists(),
        "site 5 created a state file"
    );
}

#[test]
fn issue_1937_creation_invalid_default_fails_before_publishing() {
    let fixture = replica_creation_fixture();
    write_matrix_default(
        &fixture,
        serde_json::json!({
            "tooling": {
                "replicaSelectionDefault": {
                    "codingAgentId": "codex",
                    "requestedProfile": "B",
                    "selectionLocked": "yes"
                }
            }
        }),
    );

    let error = create_replica(&fixture).expect_err("invalid default must fail");
    assert!(error.contains("selectionLocked"), "{error}");
    let replica_dir = fixture.wg_dir.join("__agent_tech-lead");
    assert!(
        !replica_dir.join("config.json").exists(),
        "no misleading replica config may be published"
    );
}

#[test]
fn issue_1937_creation_merges_context_and_preserves_tooling_unknown_keys() {
    let fixture = replica_creation_fixture();
    let replica_dir = create_replica(&fixture).expect("create replica");

    let mut config = replica_config(&replica_dir);
    config["tooling"] = serde_json::json!({
        "currentCodingAgent": "codex",
        "profile": "B",
        "profileContentHash": "deadbeef"
    });
    config["context"] = serde_json::json!([
        "$AGENTSCOMMANDER_CONTEXT",
        "custom-notes.md",
        "../../agentscommander-old/.ac/_agent_tech-lead/Role.md"
    ]);
    config["unknownKey"] = serde_json::json!({"nested": [1, 2, 3]});
    std::fs::write(
        replica_dir.join("config.json"),
        serde_json::to_string(&config).expect("serialize"),
    )
    .expect("write replica config");

    create_replica(&fixture).expect("re-creation merges");

    let saved = replica_config(&replica_dir);
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "B");
    assert_eq!(
        replica_state(&replica_dir)["tooling"]["profileContentHash"],
        "deadbeef"
    );
    assert!(saved["tooling"].get("profileContentHash").is_none());
    assert_eq!(saved["unknownKey"]["nested"][2], 3);
    let context: Vec<&str> = saved["context"]
        .as_array()
        .expect("context array")
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    assert_eq!(context[0], "$AGENTSCOMMANDER_CONTEXT");
    assert!(context.contains(&"custom-notes.md"), "{context:?}");
    assert!(
        context.contains(&"../../_agent_tech-lead/Role.md"),
        "{context:?}"
    );
    assert!(
        !context
            .iter()
            .any(|entry| entry.contains("agentscommander-old")),
        "stale Role.md entry must be replaced: {context:?}"
    );
}

#[test]
fn issue_1937_creation_concurrent_and_retry_initialization_is_lossless() {
    let fixture = replica_creation_fixture();
    write_matrix_default(
        &fixture,
        serde_json::json!({
            "tooling": {
                "replicaSelectionDefault": {
                    "codingAgentId": "codex",
                    "requestedProfile": "B",
                    "selectionLocked": false
                }
            }
        }),
    );

    let mut handles = Vec::new();
    for _ in 0..4 {
        let ac_root = fixture.ac_root.clone();
        let wg_dir = fixture.wg_dir.clone();
        handles.push(std::thread::spawn(move || {
            create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
                ac_root,
                wg_dir,
                agent_path: "_agent_tech-lead".to_string(),
                team_repos: Vec::new(),
            })
        }));
    }
    for handle in handles {
        handle.join().expect("join").expect("concurrent create");
    }

    let replica_dir = fixture.wg_dir.join("__agent_tech-lead");
    let config = replica_config(&replica_dir);
    assert_eq!(config["identity"], "../../_agent_tech-lead");
    assert_eq!(config["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(config["tooling"]["profile"], "B");
    assert_eq!(config["tooling"]["selectionLocked"], false);
    let context = config["context"].as_array().expect("context array");
    assert!(context
        .iter()
        .any(|entry| entry.as_str() == Some("../../_agent_tech-lead/Role.md")));
}

#[test]
fn issue_1937_creation_gui_loop_and_on_disk_initialization_are_identical() {
    let fixture = replica_creation_fixture();
    write_matrix_default(
        &fixture,
        serde_json::json!({
            "tooling": {
                "replicaSelectionDefault": {
                    "codingAgentId": "codex",
                    "requestedProfile": "B",
                    "selectionLocked": true
                }
            }
        }),
    );

    let on_disk_replica = create_replica(&fixture).expect("on-disk create");

    // The GUI workgroup loop is this shared helper; both entry points must
    // publish byte-identical initialization.
    let gui_wg_dir = fixture.ac_root.join("wg-3-dev-team");
    std::fs::create_dir_all(&gui_wg_dir).expect("gui wg dir");
    initialize_workgroup_replicas_on_disk(
        &fixture.ac_root,
        &gui_wg_dir,
        &["_agent_tech-lead".to_string()],
        &[],
    )
    .expect("gui loop initialization");
    let gui_replica = gui_wg_dir.join("__agent_tech-lead");

    assert_eq!(
        std::fs::read(on_disk_replica.join("config.json")).expect("read on-disk"),
        std::fs::read(gui_replica.join("config.json")).expect("read gui"),
        "the GUI loop must initialize through the same guarded helper"
    );
}
