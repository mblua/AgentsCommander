use super::*;

#[test]
fn build_plan_dedupes_by_command_first_entry_wins() {
    let catalog = vec![
        entry("claude", "Claude (primary)", vec!["claude --update"]),
        entry(
            "claude",
            "Claude (secondary)",
            vec!["claude --update --beta"],
        ),
        entry("codex", "Codex", vec!["codex update"]),
    ];
    let answers = BTreeMap::new();
    let registered = HashSet::from(["claude".to_string(), "codex".to_string()]);
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert_eq!(plan.prompts.len(), 2);
    assert!(plan.updates.is_empty());
    let claude = plan
        .prompts
        .iter()
        .find(|t| t.command == "claude")
        .expect("claude prompt");
    assert_eq!(claude.label, "Claude (primary)");
    assert_eq!(claude.commands, vec!["claude --update"]);
}

#[test]
fn build_plan_empty_winner_blocks_later_nonempty_duplicates() {
    // #1967: the FIRST entry of a command is effective even when its
    // sequence is empty - the command is suppressed for the whole pass and
    // a later non-empty duplicate can never re-enable it.
    let catalog = vec![
        entry("claude", "Claude A", vec![]),
        entry("claude", "Claude B", vec!["claude --update"]),
    ];
    let answers = BTreeMap::from([("claude".to_string(), true)]);
    let registered = HashSet::from(["claude".to_string()]);
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert!(plan.prompts.is_empty());
    assert!(plan.updates.is_empty());
}

#[test]
fn build_plan_conflicting_duplicates_never_mix_entries() {
    // Empty winner -> nothing at all; a repeated call is not affected by the
    // consent answer and never borrows the later custom sequence.
    let catalog = vec![
        entry("pi", "Pi", vec![]),
        entry("pi-max", "Pi Max", vec!["pi --custom"]),
    ];
    let registered = HashSet::from(["pi".to_string()]);
    for answers in [
        BTreeMap::new(),
        BTreeMap::from([("pi".to_string(), true)]),
        BTreeMap::from([("pi".to_string(), false)]),
    ] {
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert!(plan.prompts.is_empty());
        assert!(plan.updates.is_empty());
    }
}

#[test]
fn build_plan_preserves_the_effective_entry_sequence_order() {
    let catalog = vec![CodingAgentDefinition {
        key: "ordered".to_string(),
        ..entry(
            "ordered",
            "Ordered",
            vec!["ordered fetch", "ordered build", "ordered install"],
        )
    }];
    let registered = HashSet::from(["ordered".to_string()]);
    let answers = BTreeMap::from([("ordered".to_string(), true)]);
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert_eq!(plan.updates.len(), 1);
    assert_eq!(
        plan.updates[0].commands,
        vec!["ordered fetch", "ordered build", "ordered install"]
    );
}

#[test]
fn build_plan_consent_is_keyed_by_exact_command_and_auto_update_is_inert() {
    // `settings.agent_auto_update_by_command` is the ONLY consent authority,
    // keyed by the EXACT command: catalog `autoUpdate: true` never
    // self-consents, and a changed command cannot inherit another command's
    // opt-in.
    let catalog = vec![
        CodingAgentDefinition {
            key: "renamed-claude".to_string(),
            auto_update: true,
            ..entry("claude --update", "Renamed Claude", vec!["claude --update"])
        },
        entry("claude", "Plain Claude", vec!["claude update"]),
    ];
    let registered = HashSet::from(["claude".to_string(), "claude --update".to_string()]);

    // Consent recorded for a DIFFERENT command: neither inherits it.
    let answers = BTreeMap::from([("codex".to_string(), true)]);
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    let prompt_commands: Vec<&str> = plan.prompts.iter().map(|t| t.command.as_str()).collect();
    assert_eq!(prompt_commands, vec!["claude --update", "claude"]);
    assert!(
        plan.updates.is_empty(),
        "catalog autoUpdate: true must not self-consent"
    );

    // Consent for the exact command updates it; the other stays a prompt.
    let answers = BTreeMap::from([("claude --update".to_string(), true)]);
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert_eq!(plan.updates.len(), 1);
    assert_eq!(plan.updates[0].command, "claude --update");
    assert_eq!(plan.prompts.len(), 1);
    assert_eq!(plan.prompts[0].command, "claude");
}

#[test]
fn build_plan_skips_commands_without_sequence() {
    let catalog = vec![
        entry("claude", "Claude", vec!["claude --update"]),
        entry("hermes", "Hermes", vec![]),
    ];
    let answers = BTreeMap::new();
    let registered = HashSet::from(["claude".to_string(), "hermes".to_string()]);
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert_eq!(plan.prompts.len(), 1);
    assert_eq!(plan.prompts[0].command, "claude");
    assert!(!plan.prompts.iter().any(|t| t.command == "hermes"));
    assert!(plan.updates.is_empty());
}

#[test]
fn build_plan_answer_kinds_and_disjointness() {
    let catalog = vec![
        entry("claude", "Claude", vec!["claude --update"]),
        entry("codex", "Codex", vec!["codex update"]),
        entry("pi", "Pi", vec!["pi update"]),
    ];
    let answers = BTreeMap::from([("claude".to_string(), true)]);
    let registered = HashSet::from(["claude".to_string(), "codex".to_string(), "pi".to_string()]);
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert_eq!(plan.prompts.len(), 2);
    assert_eq!(plan.updates.len(), 1);
    assert_eq!(plan.updates[0].command, "claude");
    // catalog order preserved for prompts
    let prompt_commands: Vec<&str> = plan.prompts.iter().map(|t| t.command.as_str()).collect();
    assert_eq!(prompt_commands, vec!["codex", "pi"]);
    // disjoint by construction
    for p in &plan.prompts {
        assert!(!plan.updates.iter().any(|u| u.command == p.command));
    }
}

#[test]
fn build_plan_filters_to_registered_commands() {
    let catalog = vec![
        entry("claude", "Claude", vec!["claude --update"]),
        entry("codex", "Codex", vec!["codex update"]),
        entry("pi", "Pi", vec!["pi update"]),
    ];
    let registered = HashSet::from(["claude".to_string(), "pi".to_string()]);
    let answers = BTreeMap::new();
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert_eq!(plan.prompts.len(), 2);
    assert!(plan.updates.is_empty());
    // catalog order preserved for the registered subset
    let prompt_commands: Vec<&str> = plan.prompts.iter().map(|t| t.command.as_str()).collect();
    assert_eq!(prompt_commands, vec!["claude", "pi"]);
    // catalog-only command skipped: never prompted, never updated
    assert!(!plan.prompts.iter().any(|t| t.command == "codex"));
    assert!(!plan.updates.iter().any(|t| t.command == "codex"));
}

#[test]
fn build_plan_zero_registered_commands_returns_empty_plan() {
    let catalog = vec![
        entry("claude", "Claude", vec!["claude --update"]),
        entry("codex", "Codex", vec!["codex update"]),
    ];
    let registered = HashSet::new();
    let answers = BTreeMap::new();
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert!(plan.prompts.is_empty());
    assert!(plan.updates.is_empty());
}

#[test]
fn build_plan_registered_filter_preserves_first_entry_semantics() {
    let catalog = vec![
        entry("claude", "Claude (primary)", vec!["claude --update"]),
        entry(
            "claude",
            "Claude (secondary)",
            vec!["claude --update --beta"],
        ),
        entry("codex", "Codex", vec!["codex update"]),
    ];
    let registered = HashSet::from(["claude".to_string()]);
    let answers = BTreeMap::from([("claude".to_string(), true)]);
    let plan = build_update_plan(&catalog, &registered, &answers, cwd());
    assert!(plan.prompts.is_empty());
    assert_eq!(plan.updates.len(), 1);
    let claude = &plan.updates[0];
    assert_eq!(claude.label, "Claude (primary)"); // first entry's label wins
    assert_eq!(claude.commands, vec!["claude --update"]); // ...and its exact sequence
}

// ---------------------------------------------------------------------
// #1968 R2 - P4 parity on real migrated / local-composed on-disk catalogs.
// The production reader feeds the real (pure) build_update_plan; nothing
// here launches a vendor process.
// ---------------------------------------------------------------------

#[test]
fn build_plan_from_a_migrated_on_disk_catalog_keeps_first_entry_precedence() {
    use crate::config::coding_agents_catalog::ensure_seeded;

    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let ac_dir = project.join(".ac");
    let catalog_dir = ac_dir.join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).unwrap();
    // A legacy user-owned catalog with: a custom command appearing twice
    // (its FIRST row leaves updateCommands absent -> materialized as []),
    // an ordered custom sequence, and an authored claude sequence.
    let legacy = serde_json::json!({
        "schemaVersion": 1,
        "agents": [
            {"key": "mytool-first", "label": "My Tool First", "description": "d", "color": "#111", "command": "mytool", "envs": [], "isolatedHome": false, "removable": true},
            {"key": "mytool-second", "label": "My Tool Second", "description": "d", "color": "#111", "command": "mytool", "envs": [], "isolatedHome": false, "removable": true, "updateCommands": ["mytool up"]},
            {"key": "ordered", "label": "Ordered", "description": "d", "color": "#222", "command": "ordered", "envs": [], "isolatedHome": false, "removable": true, "updateCommands": ["ordered fetch", "ordered build"]},
            {"key": "claude", "label": "My Claude", "description": "d", "color": "#d97706", "command": "claude", "envs": [], "isolatedHome": false, "removable": true, "updateCommands": ["claude --legacy"]}
        ]
    });
    let mut bytes = serde_json::to_vec_pretty(&legacy).unwrap();
    bytes.push(b'\n');
    std::fs::write(catalog_dir.join("agents.10.default.json"), &bytes).unwrap();
    assert!(
        ensure_seeded(&ac_dir, None).is_some(),
        "the legacy catalog migrates"
    );

    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let catalog = load_catalog_for_settings(&settings)
        .expect("the production reader serves the migrated catalog");

    let registered = HashSet::from(["claude".to_string(), "mytool".to_string()]);
    let pending = build_update_plan(&catalog, &registered, &BTreeMap::new(), cwd());
    let claude = pending
        .prompts
        .iter()
        .find(|target| target.command == "claude")
        .expect("claude awaits its own SI/NO");
    assert_eq!(claude.label, "My Claude", "first-entry values survive");
    assert_eq!(claude.commands, vec!["claude --legacy"]);
    assert!(
        pending
            .prompts
            .iter()
            .all(|target| target.command != "ordered"),
        "catalog-only commands are never prompted"
    );
    assert!(
        pending
            .prompts
            .iter()
            .all(|target| target.command != "mytool"),
        "the first row's empty sequence suppresses the later duplicate"
    );

    let accepted = build_update_plan(
        &catalog,
        &registered,
        &BTreeMap::from([("claude".to_string(), true)]),
        cwd(),
    );
    assert!(accepted.prompts.is_empty());
    assert_eq!(accepted.updates.len(), 1);
    assert_eq!(accepted.updates[0].commands, vec!["claude --legacy"]);

    let declined = build_update_plan(
        &catalog,
        &registered,
        &BTreeMap::from([("claude".to_string(), false)]),
        cwd(),
    );
    assert!(declined.prompts.is_empty());
    assert!(declined.updates.is_empty());
}

#[test]
fn build_plan_from_a_local_composed_on_disk_catalog_honors_exact_command_consent() {
    use crate::config::coding_agents_catalog::ensure_seeded;

    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let ac_dir = project.join(".ac");
    let catalog_dir = ac_dir.join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).unwrap();
    assert!(ensure_seeded(&ac_dir, None).is_some(), "fresh managed base");
    let local = serde_json::json!({
        "schemaVersion": 1,
        "agents": [
            {"key": "claude", "updateCommands": ["claude --custom", "claude --second"]},
            {"key": "mytool-first", "label": "My Tool First", "description": "d", "color": "#111", "command": "mytool", "envs": [], "isolatedHome": false, "removable": true, "updateCommands": [], "autoUpdate": false},
            {"key": "mytool-second", "label": "My Tool Second", "description": "d", "color": "#111", "command": "mytool", "envs": [], "isolatedHome": false, "removable": true, "updateCommands": ["mytool up"], "autoUpdate": false}
        ]
    });
    let mut bytes = serde_json::to_vec_pretty(&local).unwrap();
    bytes.push(b'\n');
    std::fs::write(catalog_dir.join("agents.50.personal.no-git.json"), &bytes).unwrap();

    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let catalog = load_catalog_for_settings(&settings)
        .expect("the production reader composes the local layer");
    let registered = HashSet::from(["claude".to_string(), "mytool".to_string()]);

    // No consent yet: claude waits, with the exact ordered local sequence.
    let pending = build_update_plan(&catalog, &registered, &BTreeMap::new(), cwd());
    let claude = pending
        .prompts
        .iter()
        .find(|target| target.command == "claude")
        .expect("claude prompt");
    assert_eq!(claude.commands, vec!["claude --custom", "claude --second"]);
    assert!(
        pending
            .prompts
            .iter()
            .all(|target| target.command != "mytool"),
        "an empty first row suppresses the duplicate"
    );

    // Consent keyed by the EXACT command opts only that command in.
    let accepted = build_update_plan(
        &catalog,
        &registered,
        &BTreeMap::from([("claude".to_string(), true)]),
        cwd(),
    );
    assert!(accepted.prompts.is_empty());
    assert_eq!(accepted.updates.len(), 1);
    assert_eq!(accepted.updates[0].command, "claude");
    assert_eq!(
        accepted.updates[0].commands,
        vec!["claude --custom", "claude --second"]
    );

    // Explicit refusal runs nothing.
    let declined = build_update_plan(
        &catalog,
        &registered,
        &BTreeMap::from([("claude".to_string(), false)]),
        cwd(),
    );
    assert!(declined.prompts.is_empty());
    assert!(declined.updates.is_empty());

    // A consent for a different (suppressed) command neither runs it nor
    // answers for claude: claude still owes its own answer.
    let other = build_update_plan(
        &catalog,
        &registered,
        &BTreeMap::from([("mytool".to_string(), true)]),
        cwd(),
    );
    assert!(other.updates.is_empty());
    assert_eq!(other.prompts.len(), 1);
    assert_eq!(other.prompts[0].command, "claude");
}
