use super::*;

#[test]
fn legacy_absent_or_not_a_file_seeds_embedded_default() {
    // Absent legacy dir -> embedded default.
    let project = seed_dir();
    let legacy = legacy_dir();
    ensure_seeded(project.path(), Some(legacy.path()));
    assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);

    // Legacy agents.json is a DIRECTORY -> not a regular file -> embedded.
    let project = seed_dir();
    std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
    ensure_seeded(project.path(), Some(legacy.path()));
    assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);

    // No legacy at all -> embedded default.
    let project = seed_dir();
    ensure_seeded(project.path(), None);
    assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);
}

#[test]
fn project_file_present_never_touched_even_when_legacy_differs() {
    let project = seed_dir();
    let legacy = legacy_dir();
    std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();

    // First seed from legacy, then hand-edit the PROJECT file.
    ensure_seeded(project.path(), Some(legacy.path()));
    let project_file = manifest_path(project.path());
    let custom = manifest_json(concat!(
        r##"[{"key":"hand","label":"Hand","description":"d","color":"#222","command":"hand","##,
        r##""envs":[],"isolatedHome":false,"removable":true}]"##,
    ));
    std::fs::write(&project_file, &custom).unwrap();

    // A present project file is user-owned: a differing legacy must not win.
    ensure_seeded(project.path(), Some(legacy.path()));
    assert_eq!(std::fs::read_to_string(&project_file).unwrap(), custom);
}

#[test]
fn legacy_masters_tree_copied_per_builtin_dest_and_embedded_fallback() {
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_seed = legacy.path().join("_seed");
    // .claude and .codex masters exist in the legacy tree (one file each,
    // distinct bytes); .opencode does NOT (embedded default must win).
    std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap();
    std::fs::create_dir_all(legacy_seed.join(".codex")).unwrap();
    std::fs::write(legacy_seed.join(".claude/settings.json"), b"LEGACY CLAUDE").unwrap();
    std::fs::write(legacy_seed.join(".codex/config.toml"), b"LEGACY CODEX").unwrap();

    ensure_seeded_masters(project.path(), Some(legacy.path()));

    let claude_dir = master_dir_for_dest(project.path(), ".claude");
    assert_eq!(
        std::fs::read(claude_dir.join("settings.json")).unwrap(),
        b"LEGACY CLAUDE"
    );
    let codex_dir = master_dir_for_dest(project.path(), ".codex");
    assert_eq!(
        std::fs::read(codex_dir.join("config.toml")).unwrap(),
        b"LEGACY CODEX"
    );
    // .opencode: no legacy master -> embedded default staged.
    let opencode_dir = master_dir_for_dest(project.path(), ".opencode");
    let opencode_master = master("opencode");
    assert_eq!(
        std::fs::read(opencode_dir.join(opencode_master.files[0].rel_path)).unwrap(),
        opencode_master.files[0].bytes
    );
    // The legacy tree is untouched and re-running is a no-op (present).
    assert_eq!(
        std::fs::read(legacy_seed.join(".claude/settings.json")).unwrap(),
        b"LEGACY CLAUDE"
    );
    std::fs::write(legacy_seed.join(".claude/settings.json"), b"CHANGED").unwrap();
    ensure_seeded_masters(project.path(), Some(legacy.path()));
    assert_eq!(
        std::fs::read(claude_dir.join("settings.json")).unwrap(),
        b"LEGACY CLAUDE"
    );
}

#[test]
fn primary_project_root_first_entry_wins_legacy_fallback_none() {
    let mut settings = AppSettings::default();
    assert_eq!(primary_project_root(&settings), None);

    // project_paths first non-empty trimmed entry wins, whitespace padded.
    settings.project_paths = vec![
        "  ".to_string(),
        "  C:\\first\\project  ".to_string(),
        "C:\\second\\project".to_string(),
    ];
    assert_eq!(
        primary_project_root(&settings),
        Some(PathBuf::from("C:\\first\\project"))
    );

    // Empty project_paths -> legacy project_path fallback.
    settings.project_paths.clear();
    assert_eq!(primary_project_root(&settings), None);
    settings.project_path = Some("  C:\\legacy\\project ".to_string());
    assert_eq!(
        primary_project_root(&settings),
        Some(PathBuf::from("C:\\legacy\\project"))
    );

    // Whitespace-only legacy path -> None.
    settings.project_path = Some("   ".to_string());
    assert_eq!(primary_project_root(&settings), None);
}

#[test]
fn registered_project_roots_multi_and_legacy_only() {
    let mut settings = AppSettings::default();
    assert!(registered_project_roots(&settings).is_empty());

    settings.project_paths = vec![
        "  ".to_string(),
        "C:\\one".to_string(),
        "C:\\two".to_string(),
    ];
    assert_eq!(
        registered_project_roots(&settings),
        vec![PathBuf::from("C:\\one"), PathBuf::from("C:\\two")]
    );

    // Empty project_paths -> legacy project_path alone.
    settings.project_paths.clear();
    settings.project_path = Some("C:\\legacy".to_string());
    assert_eq!(
        registered_project_roots(&settings),
        vec![PathBuf::from("C:\\legacy")]
    );
    // Whitespace-only legacy path -> no roots.
    settings.project_path = Some(" ".to_string());
    assert!(registered_project_roots(&settings).is_empty());
}

#[test]
fn load_catalog_for_settings_primary_wins_and_never_falls_back() {
    let primary = seed_dir();
    let ac_dir = primary.path().join(".ac");
    let settings = AppSettings {
        project_paths: vec![primary.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };

    // Primary file absent -> unavailable: never an embedded default, never
    // the legacy instance copy.
    let unavailable = load_catalog_for_settings(&settings)
        .expect_err("absent primary is unavailable, never a self-heal");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(
        unavailable.path,
        manifest_path(&ac_dir).display().to_string()
    );
    assert!(!catalog_dir(&ac_dir).exists());

    // Hand-edited primary file is observable (primary wins over everything).
    let custom = manifest_json(concat!(
        r##"[{"key":"custom","label":"Custom","description":"d","color":"#333","##,
        r##""command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
    ));
    std::fs::create_dir_all(manifest_path(&ac_dir).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(&ac_dir), &custom).unwrap();
    let loaded = load_catalog_for_settings(&settings).expect("persisted primary catalog");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].key, "custom");

    // Primary file DELETED -> unavailable again, never a legacy copy.
    std::fs::remove_file(manifest_path(&ac_dir)).unwrap();
    let unavailable =
        load_catalog_for_settings(&settings).expect_err("deleted primary is unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
}

#[test]
fn load_catalog_unavailable_display_carries_code_path_and_reason() {
    let dir = seed_dir();
    let unavailable = load_catalog(dir.path()).expect_err("absent catalog");
    let rendered = unavailable.to_string();
    assert!(rendered.contains("baseUnavailable"), "{rendered}");
    assert!(
        rendered.contains(&manifest_path(dir.path()).display().to_string()),
        "{rendered}"
    );
    assert!(rendered.contains("no persisted catalog"), "{rendered}");
    assert_eq!(
        rendered,
        format!(
            "catalog unavailable (baseUnavailable) at {}: {}",
            unavailable.path, unavailable.reason
        )
    );
}

#[test]
fn load_catalog_for_settings_two_projects_switch_order_and_isolation() {
    let alpha = seed_dir();
    let beta = seed_dir();
    write_report_manifest(
        &alpha.path().join(".ac"),
        &manifest_json(concat!(
            r##"[{"key":"alpha-sentinel","label":"Alpha","description":"d","color":"#111","##,
            r##""command":"alpha-sentinel","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["alpha update"]}]"##,
        )),
    );
    write_report_manifest(
        &beta.path().join(".ac"),
        &manifest_json(concat!(
            r##"[{"key":"beta-sentinel","label":"Beta","description":"d","color":"#222","##,
            r##""command":"beta-sentinel","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["beta update"]}]"##,
        )),
    );
    let alpha_path = alpha.path().to_string_lossy().to_string();
    let beta_path = beta.path().to_string_lossy().to_string();

    let settings = AppSettings {
        project_paths: vec![alpha_path.clone(), beta_path.clone()],
        ..AppSettings::default()
    };
    let loaded = load_catalog_for_settings_with_config_dir(&settings, None).expect("alpha catalog");
    assert_eq!(keys_of(&loaded), ["alpha-sentinel"]);

    // Switch order: the first entry is primary and nothing is merged in.
    let switched = AppSettings {
        project_paths: vec![beta_path.clone(), alpha_path.clone()],
        ..AppSettings::default()
    };
    let loaded = load_catalog_for_settings_with_config_dir(&switched, None).expect("beta catalog");
    assert_eq!(keys_of(&loaded), ["beta-sentinel"]);

    // A failing primary never falls back to the second project.
    let missing = seed_dir();
    let failed = AppSettings {
        project_paths: vec![
            missing.path().to_string_lossy().to_string(),
            alpha_path.clone(),
        ],
        ..AppSettings::default()
    };
    let unavailable = load_catalog_for_settings_with_config_dir(&failed, None)
        .expect_err("failing primary is unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(
        unavailable.path,
        manifest_path(&missing.path().join(".ac"))
            .display()
            .to_string()
    );
}

#[test]
fn load_catalog_for_settings_project_path_fallback_and_no_project_instance() {
    let project = seed_dir();
    write_report_manifest(
        &project.path().join(".ac"),
        &manifest_json(concat!(
            r##"[{"key":"legacy-root","label":"Legacy root","description":"d","##,
            r##""color":"#111","command":"legacy-root","envs":[],"isolatedHome":false,"##,
            r##""removable":true,"updateCommands":["legacy-root update"]}]"##,
        )),
    );
    let settings = AppSettings {
        project_paths: vec!["   ".to_string()],
        project_path: Some(format!("  {}  ", project.path().display())),
        ..AppSettings::default()
    };
    let loaded = load_catalog_for_settings_with_config_dir(&settings, None)
        .expect("trimmed legacy project_path catalog");
    assert_eq!(keys_of(&loaded), ["legacy-root"]);

    // No-project mode reads the existing instance catalog read-only.
    let instance = seed_dir();
    write_report_manifest(
        instance.path(),
        &manifest_json(concat!(
            r##"[{"key":"instance","label":"Instance","description":"d","color":"#111","##,
            r##""command":"instance","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["instance update"]}]"##,
        )),
    );
    let before = std::fs::read(manifest_path(instance.path())).unwrap();
    let loaded = load_catalog_for_settings_with_config_dir(
        &AppSettings::default(),
        Some(instance.path().to_path_buf()),
    )
    .expect("instance catalog");
    assert_eq!(keys_of(&loaded), ["instance"]);
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        before
    );

    // Absent instance catalog: unavailable, never another project, and
    // repeated reads create nothing.
    let absent = seed_dir();
    for _ in 0..2 {
        let unavailable = load_catalog_for_settings_with_config_dir(
            &AppSettings::default(),
            Some(absent.path().to_path_buf()),
        )
        .expect_err("absent instance catalog");
        assert_eq!(unavailable.code, "baseUnavailable");
    }
    assert!(!catalog_dir(absent.path()).exists());

    // Unresolvable config dir: unavailable with an empty path.
    let unavailable = load_catalog_for_settings_with_config_dir(&AppSettings::default(), None)
        .expect_err("unresolvable config dir");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert!(unavailable.path.is_empty());
}

#[test]
fn load_catalog_keeps_valid_entry_while_warning_about_a_legacy_one() {
    // A migrationPending warning never blocks usable persisted commands.
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        &manifest_json(concat!(
            r##"[{"key":"legacy","label":"Legacy","description":"d","color":"#111","##,
            r##""command":"legacy","envs":[],"isolatedHome":false,"removable":true},
"##,
            r##"             {"key":"fresh","label":"Fresh","description":"d","##,
            r##""color":"#222","command":"fresh","envs":[],"isolatedHome":false,"##,
            r##""removable":true,"updateCommands":["fresh update"]}]"##,
        )),
    );

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(keys_of(&loaded), ["legacy", "fresh"]);
    assert!(loaded[0].update_commands.is_empty());
    assert_eq!(loaded[1].update_commands, vec!["fresh update".to_string()]);

    let report = report_for(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].code, "migrationPending");
}

#[test]
fn ensure_seeded_for_project_skips_missing_or_relative_root_without_writes() {
    let base = seed_dir();
    // Missing root: nothing must be created.
    let missing = base.path().join("does-not-exist");
    ensure_seeded_for_project(&missing);
    assert!(!missing.exists());

    // Relative root: nothing must be created relative to the process CWD.
    let relative = Path::new("some-relative-project");
    ensure_seeded_for_project(relative);
    assert!(!relative.exists());
}

#[test]
fn ensure_seeded_for_project_steady_state_precheck_skips_gate_and_writes() {
    let project = seed_dir();
    let root = project.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    // First run seeds everything under the (test) ungated path.
    ensure_seeded_for_project(&root);
    assert!(manifest_path(&root.join(".ac")).is_file());
    for m in EMBEDDED_SEED_MASTERS {
        assert!(master_dir_for_dest(&root.join(".ac"), m.dest).is_dir());
    }
    let manifest_bytes = std::fs::read(manifest_path(&root.join(".ac"))).unwrap();

    // Steady state: re-running must not touch a byte (and must not even
    // need the manifest to parse; the pre-check is existence-only).
    ensure_seeded_for_project(&root);
    assert_eq!(
        std::fs::read(manifest_path(&root.join(".ac"))).unwrap(),
        manifest_bytes
    );

    // A missing master dir is re-seeded by the next run (masters self-heal).
    let claude_dir = master_dir_for_dest(&root.join(".ac"), ".claude");
    std::fs::remove_dir_all(&claude_dir).unwrap();
    ensure_seeded_for_project(&root);
    assert!(claude_dir.is_dir());
}

#[test]
fn managed_catalog_corrupt_instance_legacy_blocks_transfer_without_writes() {
    // An unreadable legacy source must not be "migrated" or trashed: the
    // transfer is refused, no base is published, and every byte stays put.
    let project = seed_dir();
    let legacy = legacy_dir();
    let garbage = b"{ this is not valid json".to_vec();
    std::fs::write(legacy.path().join("agents.json"), &garbage).unwrap();

    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
    let catalog = catalog_dir(project.path());
    assert!(
        !manifest_path(project.path()).exists(),
        "no base is published"
    );
    assert!(!catalog.join(MIGRATION_BACKUP_FILENAME).exists());
    assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
    assert!(!local_catalog_path(project.path()).exists());
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        garbage
    );
    let unavailable = load_catalog(project.path()).expect_err("no project base");
    assert_eq!(unavailable.code, "baseUnavailable");

    // Recovery: remove the corrupt source; the next initialization seeds.
    std::fs::remove_file(legacy.path().join("agents.10.default.json")).unwrap();
    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
    assert_eq!(
        load_catalog(project.path())
            .expect("fresh managed base")
            .len(),
        8
    );
}

#[test]
fn legacy_masters_empty_dir_copied_verbatim_and_inert() {
    let owner = CatalogReseedState::new();
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_seed = legacy.path().join("_seed");
    std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap(); // EMPTY

    ensure_seeded_masters(project.path(), Some(legacy.path()));
    let claude_dir = master_dir_for_dest(project.path(), ".claude");
    assert!(
        claude_dir.is_dir(),
        "empty legacy master is copied verbatim"
    );
    assert!(
        !crate::config::config_seed::is_nonempty_seed_dir(&claude_dir),
        "present-but-empty master stays inert for the spawn tier"
    );
    assert_eq!(std::fs::read_dir(&claude_dir).unwrap().count(), 0);
    // The embedded default is NOT seeded (present = user-owned rule); the
    // Settings re-seed button restores it.
    reseed_master_for_command(&owner, project.path(), "claude").unwrap();
    assert!(crate::config::config_seed::is_nonempty_seed_dir(
        &claude_dir
    ));
}

#[test]
fn reseed_with_no_primary_targets_legacy_config_dir() {
    let owner = CatalogReseedState::new();
    // The Settings re-seed button must keep working on pre-migration
    // installs with zero registered projects: no primary root -> the legacy
    // `<config_dir>/coding-agents/_seed/<dest>` masters are the target.
    //
    // #2001: this fixture dir stands in for the per-binary `config_dir()`.
    // This test is the ONLY writer of the real `<config_dir>/coding-agents`
    // tree, and separate test processes share that path; an in-process lock
    // cannot serialize processes, so the real path is never resolved here.
    let config_dir = seed_dir();
    let legacy_master = config_dir
        .path()
        .join("coding-agents")
        .join("_seed")
        .join(".claude");
    std::fs::create_dir_all(&legacy_master).unwrap();
    std::fs::write(legacy_master.join("marker"), b"x").unwrap();

    let result = reseed_master_for_command(&owner, config_dir.path(), "claude");
    assert!(result.is_ok(), "legacy reseed works: {result:?}");
    let m = master("claude");
    assert_eq!(
        std::fs::read(legacy_master.join(m.files[0].rel_path)).unwrap(),
        m.files[0].bytes
    );
}

#[test]
fn definition_defaults_update_commands_empty_auto_update_false_when_absent() {
    // An old agents.json (no new fields) parses with the documented
    // defaults: empty update commands, auto-update off.
    let json = r##"
    {
        "schemaVersion": 1,
        "agents": [
            {
                "key": "old",
                "label": "Old",
                "description": "d",
                "color": "#000",
                "command": "old",
                "envs": [],
                "isolatedHome": false,
                "removable": true
            }
        ]
    }
    "##;
    let parsed: CodingAgentCatalog = serde_json::from_str(json).expect("old manifest parses");
    let def = &parsed.agents[0];
    assert!(def.update_commands.is_empty());
    assert!(!def.auto_update);

    // camelCase round-trip: always serialize both fields.
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), json).unwrap();
    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded[0].update_commands.len(), 0);
    assert!(!loaded[0].auto_update);

    let mut def = loaded[0].clone();
    def.update_commands = vec!["claude --update".to_string()];
    def.auto_update = true;
    let round = serde_json::to_value(&def).expect("serialize def");
    assert_eq!(
        round["updateCommands"],
        serde_json::json!(["claude --update"])
    );
    assert_eq!(round["autoUpdate"], serde_json::json!(true));
}
