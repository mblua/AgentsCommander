use super::*;

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
