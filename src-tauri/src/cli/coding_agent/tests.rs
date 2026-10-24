use super::*;
use serde_json::json;

#[test]
fn id_hash_activation_cli_custom_and_catalog_flags_print_written_metadata() {
    for catalog_seed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_cli_catalog(
            dir.path(),
            concat!(
                r##"[{"key":"sentinel","label":"Seed","description":"d","color":"#111","##,
                r##""command":"seed-command","envs":[{"key":"SEED","value":"old"}],"##,
                r##""isolatedHome":false,"removable":true}]"##,
            ),
        );
        let catalog_settings = settings_for(dir.path());
        let mut args = add_args();
        args.from_catalog = catalog_seed.then(|| "sentinel".into());
        args.id = catalog_seed.then(|| "supplied-stable".into());
        args.label = Some("Final label".into());
        args.command = Some("final-command".into());
        args.env = vec!["MODE=final-value".into()];
        let mut printed = None;
        cmd_add_with_io(
            args,
            false,
            || resolve_catalog_for_cli(&catalog_settings),
            |op, _, _| {
                let input_id = match &op {
                    CodingAgentOp::Add { agent } => agent.id.clone(),
                    _ => unreachable!(),
                };
                let outcome = apply_and_save_direct(AppSettings::default(), &op, |candidate| {
                    assert_eq!(candidate.agents[0].command, "final-command");
                    assert_eq!(
                        candidate.agents[0].envs,
                        parse_env_flags(&["MODE=final-value".into()]).unwrap()
                    );
                    let mut physical = candidate.clone();
                    physical.agents[0].command = "published-command".into();
                    physical.agents[0].id_hash = Some("forged-before-publish".into());
                    save_at(&physical, &path)
                })?;
                assert_eq!(outcome.agent.as_ref().unwrap().id, input_id);
                printed = Some(outcome_json(&outcome));
                Ok(())
            },
        )
        .unwrap();
        let payload = printed.unwrap();
        let reload = crate::config::settings::load_settings_from_path(&path);
        let row = &reload.agents[0];
        assert_eq!(payload["agent"], serde_json::to_value(row).unwrap());
        assert_eq!(row.command, "published-command");
        assert_eq!(
            row.envs,
            parse_env_flags(&["MODE=final-value".into()]).unwrap()
        );
        assert_eq!(
            row.id_hash,
            Some(crate::config::settings::configuration_identity(
                &row.command,
                &row.envs
            ))
        );
        if catalog_seed {
            assert_eq!(row.id, "supplied-stable");
        } else {
            assert!(ops::validate_custom_agent_id(&row.id).is_ok());
        }
        assert!(payload["agent"].get("idHash").is_none());
        let disk: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                dir.path()
                    .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(disk["agents"][0]["id_hash"], payload["agent"]["id_hash"]);
        let expected_debug = serde_json::json!(["published-command", "MODE", "final-value"]);
        assert_eq!(payload["agent"]["id_hash_debug"], expected_debug);
        assert_eq!(disk["agents"][0]["id_hash_debug"], expected_debug);
        let removed_id = row.id.clone();
        let removed = apply_and_save_direct(
            reload,
            &CodingAgentOp::Remove { id: removed_id },
            |candidate| save_at(candidate, &path),
        )
        .unwrap();
        assert_eq!(outcome_json(&removed)["id"], payload["agent"]["id"]);
    }
}

fn save_at(settings: &AppSettings, path: &Path) -> Result<AppSettings, String> {
    crate::config::settings::save_settings_to_path_preserving_project_paths(settings, path)
}

fn add_args() -> AddArgs {
    AddArgs {
        from_catalog: None,
        id: None,
        label: None,
        command: None,
        color: None,
        env: Vec::new(),
        isolated_home: None,
        instructions_filename: None,
        config_seed_dest: None,
        config_seed_enabled: None,
        backend: None,
        container_image: None,
        confirm_timeout: 0,
    }
}

fn update_args(id: &str) -> UpdateArgs {
    UpdateArgs {
        id: id.to_string(),
        label: None,
        command: None,
        color: None,
        env: Vec::new(),
        clear_envs: false,
        isolated_home: None,
        instructions_filename: None,
        clear_instructions_filename: false,
        config_seed_dest: None,
        config_seed_enabled: None,
        clear_config_seed: false,
        backend: None,
        container_image: None,
        clear_container_image: false,
        confirm_timeout: 0,
    }
}

// ---- flag-conflict / early errors (return before touching disk) --------

#[test]
fn add_custom_requires_label_and_command() {
    // No from-catalog, no label/command -> error before minting/dispatch.
    let code = run(
        CodingAgentArgs {
            cmd: CodingAgentCmd::Add(add_args()),
        },
        false,
    );
    assert_eq!(code, 1);
}

#[test]
fn add_rejects_bad_color_flag() {
    let mut a = add_args();
    a.label = Some("L".into());
    a.command = Some("claude".into());
    a.color = Some("#fff".into());
    assert!(cmd_add(a, false).is_err());
}

#[test]
fn update_clear_envs_conflicts_with_env() {
    let mut a = update_args("x");
    a.clear_envs = true;
    a.env = vec!["FOO=bar".into()];
    assert!(cmd_update(a, false).is_err());
}

#[test]
fn update_clear_instructions_conflicts() {
    let mut a = update_args("x");
    a.clear_instructions_filename = true;
    a.instructions_filename = Some("AGENTS.md".into());
    assert!(cmd_update(a, false).is_err());
}

#[test]
fn update_clear_seed_conflicts() {
    let mut a = update_args("x");
    a.clear_config_seed = true;
    a.config_seed_dest = Some(".claude".into());
    assert!(cmd_update(a, false).is_err());
}

#[test]
fn update_clear_container_image_conflicts() {
    let mut a = update_args("x");
    a.clear_container_image = true;
    a.container_image = Some("image:tag".into());
    assert!(cmd_update(a, false).is_err());
}

#[test]
fn backend_flags_apply_container_semantics() {
    let mut agent = blank_agent();
    apply_backend_flags(
        &mut agent,
        None,
        Some(" agentscommander/ac-claude:latest ".into()),
    )
    .unwrap();
    assert_eq!(agent.backend.kind, SessionBackendKind::ContainerTransport);
    assert_eq!(
        agent.backend.image.as_deref(),
        Some("agentscommander/ac-claude:latest")
    );

    let mut local = blank_agent();
    assert!(apply_backend_flags(
        &mut local,
        Some(CliBackendKind::Local),
        Some("image:tag".into())
    )
    .is_err());

    let mut bad = blank_agent();
    assert!(apply_backend_flags(&mut bad, None, Some("--privileged".into())).is_err());
    assert!(apply_backend_flags(&mut bad, None, Some("   ".into())).is_err());
}

#[test]
fn backend_patch_fields_follow_update_semantics() {
    let (kind, image, clear) =
        backend_patch_fields(None, Some(" image:tag ".into()), false).unwrap();
    assert_eq!(kind, Some(SessionBackendKind::ContainerTransport));
    assert_eq!(image.as_deref(), Some("image:tag"));
    assert!(!clear);

    let (kind, image, clear) =
        backend_patch_fields(Some(CliBackendKind::Local), None, false).unwrap();
    assert_eq!(kind, Some(SessionBackendKind::LocalProcess));
    assert!(image.is_none());
    assert!(clear);

    assert!(
        backend_patch_fields(Some(CliBackendKind::Local), Some("image:tag".into()), false).is_err()
    );
    assert!(backend_patch_fields(None, Some("image:tag".into()), true).is_err());
    assert!(backend_patch_fields(None, Some("--volume=/:/host".into()), false).is_err());
}

#[test]
fn env_parse_first_equals_and_missing() {
    let envs = parse_env_flags(&["FOO=a=b".to_string()]).unwrap();
    assert_eq!(envs[0].key, "FOO");
    assert_eq!(envs[0].value, "a=b");
    assert!(parse_env_flags(&["NOEQUALS".to_string()]).is_err());
}

// ---- branch seam: daemon path writes a request and NEVER saves ---------

#[test]
fn daemon_path_queues_request_and_never_saves() {
    let dir = tempfile::tempdir().unwrap();
    let op = CodingAgentOp::Remove {
        id: "whatever".into(),
    };
    // confirm_timeout 0 -> one non-blocking result check, then cancel.
    let res = dispatch_via_daemon(dir.path(), op, 0);
    assert!(res.is_err(), "no poller -> timeout/cancel");
    // A request was written under coding-agent-requests/ (then cancelled),
    // and settings.json was NEVER written by the daemon path.
    assert!(dir.path().join(ops::CODING_AGENT_REQUESTS_DIR).is_dir());
    assert!(!dir.path().join("settings.json").exists());
}

// ---- #1967 P4: persisted-only catalog resolution -----------------------

fn write_cli_catalog(project: &Path, agents_json: &str) {
    let dir = project.join(".ac").join("coding-agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agents.10.default.json"),
        format!("{{\"schemaVersion\":1,\"agents\":{agents_json}}}"),
    )
    .unwrap();
}

fn settings_for(project: &Path) -> AppSettings {
    AppSettings {
        project_paths: vec![project.to_string_lossy().to_string()],
        ..AppSettings::default()
    }
}

#[test]
fn resolve_catalog_for_cli_serves_persisted_entries() {
    let dir = tempfile::tempdir().unwrap();
    write_cli_catalog(
        dir.path(),
        concat!(
            r##"[{"key":"sentinel","label":"Sentinel","description":"d","color":"#111","##,
            r##""command":"sentinel","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["sentinel update"]}]"##,
        ),
    );
    let catalog = resolve_catalog_for_cli(&settings_for(dir.path())).expect("persisted catalog");
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].key, "sentinel");
    assert_eq!(
        catalog[0].update_commands,
        vec!["sentinel update".to_string()]
    );
}

#[test]
fn resolve_catalog_for_cli_unavailable_is_an_error_with_code_path_and_reason() {
    let dir = tempfile::tempdir().unwrap();
    let error = resolve_catalog_for_cli(&settings_for(dir.path())).expect_err("absent catalog");
    assert!(error.contains("baseUnavailable"), "{error}");
    assert!(error.contains("agents.10.default.json"), "{error}");
    assert!(error.contains("no persisted catalog"), "{error}");
}

#[test]
fn resolve_catalog_for_cli_never_invents_update_commands() {
    let dir = tempfile::tempdir().unwrap();
    write_cli_catalog(
        dir.path(),
        concat!(
            r##"[{"key":"legacy","label":"Legacy","description":"d","color":"#111","##,
            r##""command":"legacy","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
    );
    let catalog = resolve_catalog_for_cli(&settings_for(dir.path())).expect("persisted catalog");
    assert!(catalog[0].update_commands.is_empty());
}

// ---- help text ---------------------------------------------------------

#[test]
fn help_text_documents_coding_agent() {
    use clap::CommandFactory;
    let help = crate::cli::Cli::command().render_help().to_string();
    assert!(help.contains("coding-agent"), "help missing verb: {help}");
}

/// #2716 (B3, option C) E37: `list` and `show` report an unreadable agents
/// layer, naming the path, instead of an empty list or an unknown id.
#[test]
fn an_unreadable_layer_is_not_an_empty_list() {
    let path = std::path::Path::new("C:/cfg/agents.30.instance.no-git.json");
    let mut settings = AppSettings {
        agents_layer: crate::config::settings::AgentsLayerState::Unreadable {
            path: path.to_path_buf(),
            reason: "test".to_string(),
        },
        ..Default::default()
    };
    settings.agents.clear();
    let refused = refuse_unreadable_agents_layer(&settings).unwrap_err();
    assert_eq!(
        refused,
        crate::config::settings::agents_unreadable_cli_error(path)
    );
    assert!(refused.contains(&path.display().to_string()));

    settings.agents_layer = crate::config::settings::AgentsLayerState::Readable;
    assert!(refuse_unreadable_agents_layer(&settings).is_ok());
    assert_eq!(
        ops::unknown_agent_id_error("alpha", &settings.agents),
        "agent id 'alpha' not found. Available ids: (none)"
    );
}

#[test]
fn p02_direct_and_queued_updates_share_canonical_serializer_and_effect() {
    let settings = AppSettings {
        agents: vec![serde_json::from_value(
            json!({"id":"own","label":"Own","color":"#123456","command":"tool --before"}),
        )
        .unwrap()],
        ..AppSettings::default()
    };
    let op: CodingAgentOp = serde_json::from_value(
        json!({"kind":"update","id":"own","patch":{"command":"tool --after"}}),
    )
    .unwrap();
    let request = CodingAgentRequest {
        request_id: "p02".into(),
        created_at_ms: 1,
        op: op.clone(),
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["op"]["patch"]["baseCommand"], "tool --after");
    assert!(wire["op"]["patch"].get("command").is_none());
    let queued: CodingAgentRequest = serde_json::from_value(wire).unwrap();
    let direct =
        apply_and_save_direct(settings.clone(), &op, |settings| Ok(settings.clone())).unwrap();
    let queued =
        apply_and_save_direct(settings, &queued.op, |settings| Ok(settings.clone())).unwrap();
    assert_eq!(direct.agent.as_ref().unwrap().command, "tool --after");
    assert_eq!(
        serde_json::to_value(direct.agent).unwrap(),
        serde_json::to_value(queued.agent).unwrap()
    );
}
