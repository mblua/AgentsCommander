use super::*;

#[test]
fn builtin_agent_support_table_matches_embedded_default_keys_in_order() {
    // R1: the table and the embedded JSON must list the same keys in the
    // same order (exact order implies set equality both ways and no dupes).
    let table_keys: Vec<&str> = BUILTIN_AGENT_SUPPORT.iter().map(|(k, _)| *k).collect();
    let catalog = embedded_default_catalog();
    let json_keys: Vec<&str> = catalog.agents.iter().map(|def| def.key.as_str()).collect();
    assert_eq!(table_keys, json_keys);
}

#[test]
pub(crate) fn builtin_agent_support_ships_only_muse_disabled() {
    // R2: #1999 flips muse off; every other row still ships enabled.
    let disabled: Vec<&str> = BUILTIN_AGENT_SUPPORT
        .iter()
        .filter(|(_, on)| !*on)
        .map(|(key, _)| *key)
        .collect();
    assert_eq!(disabled, ["muse"], "only muse ships disabled");
}

#[test]
fn support_override_scopes_to_closure_and_restores_shipped_table() {
    // R3: the override reaches ensure_seeded + load_catalog and restores the
    // shipped table.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let before = load_catalog(dir.path()).expect("seeded catalog");
    assert_eq!(before.len(), 8);
    assert_no_key(&before, "muse");

    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "the table is part of the managed revision"
        );
        let inside = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(inside.len(), 9);
        assert!(inside.iter().any(|a| a.key == "muse"));
    });

    let after = load_catalog(dir.path()).expect("seeded catalog");
    assert_eq!(after.len(), 8);
    assert_no_key(&after, "muse");
}

#[test]
fn desupported_row_dropped_from_seeded_manifest_and_corrupt_is_unavailable() {
    // R4 + #1967 P4: the read gate drops a de-supported row from a seeded
    // manifest; a corrupt persisted file is `baseInvalid` (no self-heal) and
    // its bytes are preserved.
    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let agents = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(agents.len(), 8);
        assert_no_key(&agents, "muse");
        assert_eq!(agents[0].key, "claude");

        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let garbage = b"{ this is not valid json";
        std::fs::write(&path, garbage).unwrap();
        let unavailable = load_catalog(dir.path()).expect_err("corrupt catalog is unavailable");
        assert_eq!(unavailable.code, "baseInvalid");
        assert_eq!(std::fs::read(&path).unwrap(), garbage);
    });
}

#[test]
fn desupported_row_dropped_from_parsed_manifest_and_dedup_cannot_resurrect_it() {
    // R5: a duplicate-bearing user manifest cannot resurrect a de-supported
    // key; a user entry with ANOTHER key and the same command is kept.
    let agents = concat!(
        r##"[
"##,
        r##"        {"key":"muse","label":"First","description":"d","color":"#0668E1","##,
        r##""command":"muse","envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"        {"key":"muse","label":"Second","description":"d","color":"#0668E1","##,
        r##""command":"muse","envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"        {"key":"mine","label":"Mine","description":"d","color":"#111","##,
        r##""command":"muse","envs":[],"isolatedHome":false,"removable":true}
"##,
        r##"    ]"##,
    );

    // Control: the all-enabled table keeps muse -> duplicate key dedups
    // first-wins, custom key kept.
    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(keys_of(&loaded), ["muse", "mine"]);
        assert_eq!(loaded[0].label, "First");
    });

    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(keys_of(&loaded), ["mine"]);
        assert_eq!(loaded[0].command, "muse");
    });
}

#[test]
fn desupported_row_dropped_from_load_catalog_for_settings_primary() {
    // R6: the settings read root (primary project) honors the read gate on
    // the persisted file path, and an absent primary is unavailable.
    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let primary = seed_dir();
        let ac_dir = primary.path().join(".ac");
        let settings = AppSettings {
            project_paths: vec![primary.path().to_string_lossy().to_string()],
            ..AppSettings::default()
        };

        let unavailable =
            load_catalog_for_settings(&settings).expect_err("absent primary is unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");

        let user_file = manifest_json(concat!(
            r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","##,
            r##""command":"muse","envs":[],"isolatedHome":false,"removable":true},
"##,
            r##"            {"key":"custom","label":"Custom","description":"d","##,
            r##""color":"#333","command":"custom","envs":[],"isolatedHome":false,"##,
            r##""removable":true}]"##,
        ));
        std::fs::create_dir_all(manifest_path(&ac_dir).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(&ac_dir), &user_file).unwrap();
        let loaded = load_catalog_for_settings(&settings).expect("persisted primary");
        assert_eq!(keys_of(&loaded), ["custom"]);
    });
}

#[test]
fn no_embedded_donor_backfills_a_custom_key_bound_to_a_builtin_command() {
    // #1967 P4: the embedded default is no longer a runtime command donor.
    // A custom key bound to the builtin `claude` command, in a manifest with
    // no updateCommands, reads as empty and the persisted bytes are kept;
    // the de-supported table changes nothing about that.
    let manifest = manifest_json(concat!(
        r##"[{"key":"my-claude","label":"My Claude","description":"d","color":"#d97706","##,
        r##""command":"claude","envs":[],"isolatedHome":false,"removable":true}]"##,
    ));

    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &manifest).unwrap();
    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert!(loaded[0].update_commands.is_empty());
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        manifest.as_bytes()
    );

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &manifest).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert!(loaded[0].update_commands.is_empty());
    });
}

#[test]
fn managed_catalog_fresh_base_bytes_carry_only_enabled_rows() {
    // The fresh managed base carries the enabled shipped rows plus the
    // ownership marker: the shipped table seeds the enabled 8 (grok in,
    // muse out) and its mirror override seeds the same 8 while the read
    // gate hides the key.
    let dir = seed_dir();
    assert!(ensure_seeded(dir.path(), None).is_some());
    let bytes = std::fs::read(manifest_path(dir.path())).unwrap();
    let base: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(base["agents"].as_array().unwrap().len(), 8);
    assert_eq!(base["managed"]["owner"], "agentscommander");
    assert!(
        base["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|agent| agent["key"] == "grok"),
        "the fresh base publishes grok"
    );

    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "a first seed publishes"
        );
        let bytes = std::fs::read(manifest_path(dir.path())).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("\"muse\""),
            "seeded bytes must not carry the de-supported key"
        );
        let catalog: CodingAgentCatalog =
            serde_json::from_slice(&bytes).expect("seeded manifest parses");
        assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
        assert_eq!(catalog.agents.len(), 8);
        let loaded = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(loaded.len(), 8);
        assert_no_key(&loaded, "muse");
    });
}

#[test]
fn managed_catalog_migration_backup_keeps_a_desupported_key_verbatim() {
    // Under a false support row a migrated legacy source is preserved
    // byte-for-byte in the backup; the read gate hides the de-supported key
    // from the effective catalog while the custom key survives.
    let legacy_bytes = manifest_json(concat!(
        r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","##,
        r##""command":"muse","envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"        {"key":"custom","label":"Custom","description":"d","color":"#333","##,
        r##""command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
    ))
    .into_bytes();

    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let project = seed_dir();
        let legacy = legacy_dir();
        std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();
        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
        assert_eq!(
            std::fs::read(catalog_dir(project.path()).join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            legacy_bytes,
            "the backup is user data: byte-for-byte even with a de-supported key"
        );
        let loaded = load_catalog(project.path()).expect("migrated catalog");
        assert_eq!(keys_of(&loaded), ["custom"]);
    });
}

#[test]
fn managed_catalog_false_row_is_part_of_the_revision_and_refreshes_only_the_base() {
    // #1968/#1999: the shipped false muse row is part of the managed
    // revision, so a base seeded all-enabled refreshes to the enabled
    // shipped set; the user-owned local layer is never touched and the key
    // stays hidden either way.
    let dir = seed_dir();
    let seeded = with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        ensure_seeded(dir.path(), None);
        assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 9);
        std::fs::read(manifest_path(dir.path())).unwrap()
    });
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "a support-gate change is part of the managed revision"
    );
    assert_ne!(std::fs::read(manifest_path(dir.path())).unwrap(), seeded);
    assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 8);
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before,
        "the user-owned local layer is never touched by a refresh"
    );
    let loaded = load_catalog(dir.path()).expect("seeded catalog");
    assert_eq!(loaded.len(), 8);
    assert_no_key(&loaded, "muse");

    // Back under the all-enabled table the base returns byte-for-byte.
    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), seeded);
    });
}

#[test]
fn desupported_master_not_seeded_not_reseedable_and_reseed_refused() {
    let owner = CatalogReseedState::new();
    // R11: a de-supported key's master is not seeded, is not listed as
    // reseedable, and its re-seed is refused server-side; the other masters
    // are unaffected.
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    for dest in [".claude", ".codex", ".opencode"] {
        assert!(
            crate::config::config_seed::is_nonempty_seed_dir(&master_dir_for_dest(
                dir.path(),
                dest
            )),
            "{dest} master should be non-empty"
        );
    }
    let mut got = reseedable_command_basenames();
    got.sort();
    assert_eq!(got, vec!["claude", "codex", "opencode"]);

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        let dir = seed_dir();
        ensure_seeded_masters(dir.path(), None);
        assert!(
            !master_dir_for_dest(dir.path(), ".claude").exists(),
            "de-supported master must not be seeded"
        );
        for dest in [".codex", ".opencode"] {
            assert!(
                crate::config::config_seed::is_nonempty_seed_dir(&master_dir_for_dest(
                    dir.path(),
                    dest
                )),
                "{dest} master must still be seeded"
            );
        }
        let mut got = reseedable_command_basenames();
        got.sort();
        assert_eq!(got, vec!["codex", "opencode"]);
        let err = reseed_master_for_command(&owner, dir.path(), "claude").unwrap_err();
        assert!(err.contains("not a recognized built-in"), "{err}");
        assert!(reseed_master_for_command(&owner, dir.path(), "codex").is_ok());
    });
}

#[test]
fn desupported_master_skips_legacy_tree_copy_too() {
    // R12: the legacy `_seed/<dest>` tree copy is skipped for a de-supported
    // key; supported masters are still copied verbatim; the legacy tree is
    // untouched.
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_seed = legacy.path().join("_seed");
    std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap();
    std::fs::create_dir_all(legacy_seed.join(".codex")).unwrap();
    std::fs::write(legacy_seed.join(".claude/settings.json"), b"LEGACY CLAUDE").unwrap();
    std::fs::write(legacy_seed.join(".codex/config.toml"), b"LEGACY CODEX").unwrap();

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        ensure_seeded_masters(project.path(), Some(legacy.path()));
        assert!(
            !master_dir_for_dest(project.path(), ".claude").exists(),
            "de-supported master must not be copied from the legacy tree"
        );
        assert_eq!(
            std::fs::read(master_dir_for_dest(project.path(), ".codex").join("config.toml"))
                .unwrap(),
            b"LEGACY CODEX"
        );
        assert_eq!(
            std::fs::read(legacy_seed.join(".claude/settings.json")).unwrap(),
            b"LEGACY CLAUDE",
            "the legacy tree itself is never touched"
        );
    });
}

#[test]
fn managed_catalog_all_masters_present_ignores_desupported_master() {
    // R13 (#1968): only the MASTER existence optimization remains, and it
    // requires only SUPPORTED masters, so a claude-off install is steady
    // without `.claude`. The catalog itself always validates under the
    // gate/lock; no existence-only shortcut may skip that (proven by the
    // steady-state test below).
    let project = seed_dir();
    let root = project.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let ac_dir = root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        ensure_seeded(&ac_dir, None);
        ensure_seeded_masters(&ac_dir, None);
        assert!(manifest_path(&ac_dir).is_file());
        assert!(!master_dir_for_dest(&ac_dir, ".claude").exists());
        assert!(crate::config::config_seed::is_nonempty_seed_dir(
            &master_dir_for_dest(&ac_dir, ".codex")
        ));
        assert!(crate::config::config_seed::is_nonempty_seed_dir(
            &master_dir_for_dest(&ac_dir, ".opencode")
        ));
        assert!(
            all_masters_present(&ac_dir),
            "steady inside the override: `.claude` must not be required"
        );

        // A missing SUPPORTED master makes the predicate false and is
        // re-seeded by the plain project entry point; `.claude` stays absent.
        let codex_dir = master_dir_for_dest(&ac_dir, ".codex");
        std::fs::remove_dir_all(&codex_dir).unwrap();
        assert!(!all_masters_present(&ac_dir));
        ensure_seeded_for_project(&root);
        assert!(crate::config::config_seed::is_nonempty_seed_dir(&codex_dir));
        assert!(!master_dir_for_dest(&ac_dir, ".claude").exists());
        assert!(all_masters_present(&ac_dir));
    });

    // Outside the override the same tree has no `.claude`: the shipped
    // table requires it and the predicate is false.
    assert!(!all_masters_present(&ac_dir));
}

#[test]
fn managed_catalog_steady_state_takes_the_gate_and_keeps_bytes() {
    // #1968: the existence-only catalog shortcut is gone. A second
    // tokenized initialization takes the project gate (lock file appears)
    // and the catalog lock, yet an unedited base at the shipped revision is
    // NOT rewritten: byte identity is preserved.
    let project = seed_dir();
    let root = project.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let ac_dir = root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
    ensure_seeded(&ac_dir, None);
    ensure_seeded_masters(&ac_dir, None);

    let base_bytes = std::fs::read(manifest_path(&ac_dir)).unwrap();
    let local_bytes = std::fs::read(local_catalog_path(&ac_dir)).unwrap();
    let gate_lock = ac_dir.join(crate::config::seed_manifest::SEED_MANIFEST_LOCK_FILENAME);
    assert!(!gate_lock.exists(), "no gate lock from the ungated seed");

    let token = ManifestActivationToken::for_test();
    ensure_seeded_for_project_with_token(&root, Some(&token));
    assert!(
        gate_lock.is_file(),
        "the catalog has no existence shortcut: the gate must have been taken"
    );
    assert_eq!(
        std::fs::read(manifest_path(&ac_dir)).unwrap(),
        base_bytes,
        "an unedited base at the shipped revision is never rewritten"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
        local_bytes,
        "the user-owned local layer is never rewritten"
    );

    // Second tokenized run: still byte-identical.
    ensure_seeded_for_project_with_token(&root, Some(&token));
    assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_bytes);
    assert_eq!(
        std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
        local_bytes
    );
}

#[test]
fn already_seeded_master_never_removed_by_a_false_row() {
    // R14: an already-present master is user-owned and never removed by a
    // false row; only the reseedable list reflects the switch.
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    let claude_file = master_dir_for_dest(dir.path(), ".claude").join("settings.json");
    let seeded = std::fs::read(&claude_file).unwrap();

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        ensure_seeded_masters(dir.path(), None);
        assert_eq!(
            std::fs::read(&claude_file).unwrap(),
            seeded,
            "an already-present master is never removed or rewritten"
        );
        let mut got = reseedable_command_basenames();
        got.sort();
        assert_eq!(got, vec!["codex", "opencode"]);
    });
}

#[test]
fn managed_catalog_fresh_base_bytes_carry_only_enabled_rows_with_nonregular_legacy() {
    // A non-regular instance source is NOT a source: fresh initialization
    // runs, and under a false support row only the enabled rows land in the
    // managed base.
    for shape in ["absent", "directory"] {
        let project = seed_dir();
        let legacy = legacy_dir();
        if shape == "directory" {
            std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
        }
        assert!(
            ensure_seeded(project.path(), Some(legacy.path())).is_some(),
            "{shape}: a first seed publishes"
        );
        let bytes = std::fs::read(manifest_path(project.path())).unwrap();
        let catalog: CodingAgentCatalog =
            serde_json::from_slice(&bytes).expect("seeded manifest parses");
        assert_eq!(catalog.agents.len(), 8, "{shape}: 8 agents seeded");

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let project = seed_dir();
            let legacy = legacy_dir();
            if shape == "directory" {
                std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
            }
            assert!(
                ensure_seeded(project.path(), Some(legacy.path())).is_some(),
                "{shape}: a first seed publishes"
            );
            let bytes = std::fs::read(manifest_path(project.path())).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains("\"muse\""),
                "{shape}: seeded bytes must not carry the de-supported key"
            );
            let catalog: CodingAgentCatalog =
                serde_json::from_slice(&bytes).expect("seeded manifest parses");
            assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
            assert_eq!(catalog.agents.len(), 8, "{shape}: 8 agents seeded");
            let loaded = load_catalog(project.path()).expect("persisted catalog");
            assert_eq!(loaded.len(), 8);
            assert_no_key(&loaded, "muse");
            assert_eq!(loaded[0].key, "claude");
        });
    }
}
