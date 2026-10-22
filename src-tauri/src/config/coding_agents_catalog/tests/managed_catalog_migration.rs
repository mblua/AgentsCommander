use super::*;

#[test]
fn managed_catalog_migrates_project_legacy_with_pins_changes_and_tombstones() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"claude","label":"My Claude","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
         {"key":"pi","label":"Pi Custom","description":"d","color":"#ec4899","command":"pi-custom","envs":[],"isolatedHome":false,"removable":true},
         {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    assert!(ensure_seeded(dir.path(), None).is_some());

    let journal = migration_journal_json(dir.path());
    assert_eq!(journal["version"], 1);
    assert_eq!(journal["sourceKind"], "project");
    assert_eq!(
        journal["sourcePath"],
        std::fs::canonicalize(manifest_path(dir.path()))
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(
        std::fs::read(catalog_dir(dir.path()).join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        legacy.as_bytes(),
        "the backup is the byte-exact source"
    );

    let local = local_json(dir.path());
    assert_eq!(local["order"], serde_json::json!(["claude", "pi", "mine"]));
    let rows = local["agents"].as_array().unwrap();
    let claude = rows.iter().find(|row| row["key"] == "claude").unwrap();
    assert_eq!(claude["label"], "My Claude", "explicit values are pinned");
    assert_eq!(claude["updateCommands"], serde_json::json!([]));
    assert!(
        claude.get("instructionsFilename").is_none() && claude.get("configSeed").is_none(),
        "absent fields inherit the shipped default instead of being pinned"
    );
    let pi = rows.iter().find(|row| row["key"] == "pi").unwrap();
    assert_eq!(pi["command"], "pi-custom");
    assert_eq!(pi["updateCommands"], serde_json::json!([]));
    assert!(rows.iter().any(|row| row["key"] == "mine"));
    assert_eq!(
        rows.iter().filter(|row| row["remove"] == true).count(),
        6,
        "the six shipped keys absent from the legacy catalog are tombstoned"
    );

    let loaded = load_catalog(dir.path()).unwrap();
    assert_eq!(keys_of(&loaded), ["claude", "pi", "mine"]);
    assert_eq!(loaded[0].label, "My Claude");
    assert!(loaded[0].update_commands.is_empty(), "explicit [] clears");
    assert_eq!(loaded[1].command, "pi-custom");
    assert!(
        loaded[1].update_commands.is_empty(),
        "a changed command never inherits the shipped sequence"
    );

    // Completed migration: restart is a no-op and preserves every byte.
    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before
    );
}

#[test]
fn managed_catalog_migration_refuses_an_existing_local() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let user_local = r##"{"schemaVersion":1,"agents":[]}"##;
    write_local(dir.path(), user_local);

    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);
    assert_eq!(read_text(&local_catalog_path(dir.path())), user_local);
    assert!(!catalog_dir(dir.path())
        .join(MIGRATION_BACKUP_FILENAME)
        .exists());
    assert!(!catalog_dir(dir.path())
        .join(MIGRATION_JOURNAL_FILENAME)
        .exists());
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
}

#[test]
fn managed_catalog_migration_refuses_ambiguous_legacy_sources() {
    let cases = [
        (
            "unknown root field",
            r##"{"schemaVersion":1,"agents":[],"future":1}"##,
        ),
        (
            "unknown row field",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"x","description":"d","color":"#111","command":"claude","mystery":1}]}"##,
        ),
        (
            "invalid definition",
            r##"{"schemaVersion":1,"agents":[{"key":"BAD KEY","label":"x","description":"d","color":"#111","command":"x"}]}"##,
        ),
        (
            "duplicate keys",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"a","description":"d","color":"#111","command":"claude"},{"key":"claude","label":"b","description":"d","color":"#111","command":"claude"}]}"##,
        ),
        ("unsupported schema", r##"{"schemaVersion":2,"agents":[]}"##),
    ];
    for (label, legacy) in cases {
        let dir = seed_dir();
        write_legacy_base(dir.path(), legacy);
        assert!(ensure_seeded(dir.path(), None).is_none(), "{label}");
        assert_eq!(
            read_text(&manifest_path(dir.path())),
            legacy,
            "{label}: bytes preserved"
        );
        assert!(
            !catalog_dir(dir.path())
                .join(MIGRATION_BACKUP_FILENAME)
                .exists(),
            "{label}: no backup is written"
        );
        // The legacy base stays readable through the legacy resolver.
        let report = load_catalog_report(dir.path());
        assert!(
            report.unavailable.is_none()
                || report.unavailable.as_ref().unwrap().code == "baseInvalid",
            "{label}"
        );
    }
}

#[test]
fn managed_catalog_migration_of_an_empty_catalog_tombstones_every_shipped_key() {
    let dir = seed_dir();
    write_legacy_base(dir.path(), r##"{"schemaVersion":1,"agents":[]}"##);
    assert!(ensure_seeded(dir.path(), None).is_some());
    let local = local_json(dir.path());
    let rows = local["agents"].as_array().unwrap();
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().all(|row| row["remove"] == true));
    assert_eq!(local["order"], serde_json::json!([]));
    assert!(load_catalog(dir.path()).unwrap().is_empty());
}

#[test]
fn managed_catalog_migration_pins_explicit_values_even_equal_to_defaults() {
    let dir = seed_dir();
    let claude = shipped_def_json(&["claude"]).remove(0);
    let legacy = serde_json::json!({"schemaVersion": 1, "agents": [claude]}).to_string();
    write_legacy_base(dir.path(), &legacy);
    assert!(ensure_seeded(dir.path(), None).is_some());

    let local = local_json(dir.path());
    let row = local["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "claude")
        .unwrap();
    assert_eq!(row["label"], "Claude Code");
    assert_eq!(
        row["updateCommands"],
        serde_json::json!(["claude --update"])
    );
    assert_eq!(row["configSeed"]["dest"], ".claude");
    assert_eq!(row["instructionsFilename"], "CLAUDE.md");
    let loaded = load_catalog(dir.path()).unwrap();
    assert_eq!(
        loaded
            .iter()
            .find(|d| d.key == "claude")
            .unwrap()
            .update_commands,
        vec!["claude --update".to_string()]
    );
}

#[test]
fn managed_catalog_migration_never_imports_the_instance_local_layer() {
    let project = seed_dir();
    let legacy = legacy_dir();
    std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();
    let instance_local = r##"{"schemaVersion":1,"agents":[{"key":"secret","label":"Secret","description":"d","color":"#111","command":"secret","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]}"##;
    std::fs::write(legacy.path().join("agents.local.json"), instance_local).unwrap();

    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
    let local = read_text(&local_catalog_path(project.path()));
    assert!(
        !local.contains("secret"),
        "the instance local layer is never imported: {local}"
    );
    assert!(load_catalog(project.path())
        .unwrap()
        .iter()
        .all(|definition| definition.key != "secret"));
    assert_eq!(
        read_text(&legacy.path().join("agents.50.personal.no-git.json")),
        instance_local,
        "the instance local file is read-only"
    );
}

#[test]
fn managed_catalog_interrupt_after_backup_resumes_backup_only() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_backup", || ensure_seeded(dir.path(), None));

    let catalog = catalog_dir(dir.path());
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        legacy.as_bytes()
    );
    assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
    assert!(!local_catalog_path(dir.path()).exists());
    assert_eq!(
        read_text(&manifest_path(dir.path())),
        legacy,
        "base untouched"
    );
    assert!(
        load_catalog(dir.path()).is_ok(),
        "the readable legacy source stays usable while recovery is pending"
    );

    // Restart: the backup equals the current project source, so the
    // backup-only continuation completes deterministically.
    assert!(ensure_seeded(dir.path(), None).is_some());
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
    assert!(
        !migration_journal_path(dir.path()).exists(),
        "a backup-only resume never invents a journal"
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        legacy.as_bytes(),
        "the backup is retained for audit"
    );
}

#[test]
fn managed_catalog_interrupt_after_local_resumes_from_the_journal() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));

    assert!(migration_journal_path(dir.path()).exists());
    assert!(local_catalog_path(dir.path()).exists());
    assert_eq!(
        read_text(&manifest_path(dir.path())),
        legacy,
        "the base was not published yet"
    );
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "the restart completes from the journal"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before,
        "no re-extraction: the verified local bytes are kept"
    );
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);

    // Third run: completed, idempotent.
    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );
}

#[test]
fn managed_catalog_interrupt_after_journal_recomputes_the_extraction() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_journal", || ensure_seeded(dir.path(), None));

    let catalog = catalog_dir(dir.path());
    assert!(catalog.join(MIGRATION_BACKUP_FILENAME).exists());
    assert!(migration_journal_path(dir.path()).exists());
    assert!(
        !local_catalog_path(dir.path()).exists(),
        "the local layer was not published"
    );
    assert_eq!(
        read_text(&manifest_path(dir.path())),
        legacy,
        "base untouched"
    );
    let backup = std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
    assert_eq!(backup, legacy.as_bytes());

    // Restart: no local exists, so recovery RECOMPUTES the extraction from
    // the backup against the journal's saved managed base and verifies it
    // against the recorded local hash before publishing anything.
    assert!(ensure_seeded(dir.path(), None).is_some());
    assert!(local_catalog_path(dir.path()).exists());
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        backup,
        "the backup is retained for audit"
    );
}

#[test]
fn managed_catalog_interrupt_before_base_resumes_from_the_journal() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("before_base", || ensure_seeded(dir.path(), None));
    assert!(migration_journal_path(dir.path()).exists());
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);

    assert!(ensure_seeded(dir.path(), None).is_some());
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
}

#[test]
fn managed_catalog_interrupt_after_base_completes_without_re_extraction() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_base", || ensure_seeded(dir.path(), None));
    assert!(migration_journal_path(dir.path()).exists());
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();
    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();

    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "a base matching the journal is already completed"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before
    );
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
}

#[test]
fn managed_catalog_interrupt_across_a_revision_change_completes_then_refreshes() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("before_base", || ensure_seeded(dir.path(), None));
    assert_eq!(
        migration_journal_json(dir.path())["managedBase"]["agents"]
            .as_array()
            .unwrap()
            .len(),
        8,
        "the journal saved the enabled shipped base"
    );

    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "the journal saved base is completed first"
        );
        let base = base_json(dir.path());
        assert_eq!(
            base["agents"].as_array().unwrap().len(),
            9,
            "then the normal verified refresh runs under the same lock"
        );
        assert!(
            base["agents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|agent| agent["key"] == "muse"),
            "the all-enabled refresh publishes muse"
        );
        assert_eq!(
            base["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
    });
}

#[test]
fn managed_catalog_interrupt_with_a_local_edit_blocks_recovery() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));
    let edited = r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"EDITED BETWEEN RUNS"}]}"##;
    write_local(dir.path(), edited);
    let entries_before = dir_entries(&catalog_dir(dir.path()));

    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(read_text(&local_catalog_path(dir.path())), edited);
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);
    assert_eq!(
        dir_entries(&catalog_dir(dir.path())),
        entries_before,
        "a conflicting recovery deletes nothing"
    );
    // The read stays readable on the legacy source and reports the pending
    // migration; the conflict itself blocks only continuation.
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
}

#[test]
fn managed_catalog_sidecar_collisions_block_without_cleanup() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let catalog = catalog_dir(dir.path());
    std::fs::create_dir_all(&catalog).unwrap();
    std::fs::write(catalog.join(MIGRATION_BACKUP_FILENAME), b"foreign backup").unwrap();
    assert!(ensure_seeded(dir.path(), None).is_none());
    let entries_before = dir_entries(&catalog);
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);
    assert_eq!(dir_entries(&catalog), entries_before);
    assert!(load_catalog(dir.path()).is_ok());

    // An invalid journal is also a conflict, and nothing is deleted.
    std::fs::remove_file(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
    std::fs::write(catalog.join(MIGRATION_JOURNAL_FILENAME), b"not json").unwrap();
    let entries_before = dir_entries(&catalog);
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);
    assert_eq!(dir_entries(&catalog), entries_before);
}

#[test]
fn managed_catalog_reads_never_write_in_any_state() {
    // Managed base with local overrides.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"Mine"}]}"##,
    );
    assert_reads_leave_state_unchanged(dir.path());

    // Legacy base.
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
    );
    assert_reads_leave_state_unchanged(dir.path());

    // Absent base and no directory at all.
    let dir = seed_dir();
    assert_reads_leave_state_unchanged(dir.path());

    // Interrupted migration boundary (journal + local, base still legacy).
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
    );
    let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));
    assert_reads_leave_state_unchanged(dir.path());
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
}

#[test]
fn managed_catalog_no_embedded_commands_escape_a_failed_publication() {
    let project = seed_dir();
    let legacy = legacy_dir();
    std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();
    let _ = with_failure_at("publication_temp_synced", || {
        ensure_seeded(project.path(), Some(legacy.path()))
    });
    assert!(
        !manifest_path(project.path()).exists(),
        "no base was published"
    );
    assert!(!catalog_dir(project.path())
        .join(MIGRATION_JOURNAL_FILENAME)
        .exists());
    let report = load_catalog_report(project.path());
    assert!(report.unavailable.is_some());
    let text = report_json(&report);
    for sentinel in [
        "claude --update",
        "pi update",
        "codex update",
        "hermes update --yes",
        "opencode upgrade",
        "agy update",
    ] {
        assert!(
            !text.contains(sentinel),
            "embedded command leaked: {sentinel}"
        );
    }
    assert!(load_catalog(project.path()).is_err());
}

#[test]
fn managed_catalog_duplicate_command_identities_and_base_order_stay_intact() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["pi --custom"],"autoUpdate":false}]}"##,
    );
    let loaded = load_catalog(dir.path()).unwrap();
    let keys = keys_of(&loaded);
    assert_eq!(keys[0], "claude");
    assert_eq!(keys[4], "pi");
    assert_eq!(keys[5], "opencode");
    assert_eq!(keys[8], "pi-max", "local additions append after the base");
    let pi = loaded.iter().find(|d| d.key == "pi").unwrap();
    let pi_max = loaded.iter().find(|d| d.key == "pi-max").unwrap();
    assert_eq!(pi.update_commands, vec!["pi update".to_string()]);
    assert_eq!(pi_max.update_commands, vec!["pi --custom".to_string()]);
}

#[test]
fn managed_catalog_untracked_publication_is_retried_without_republishing() {
    let project = seed_dir();
    let root = project.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let ac_dir = ac_dir_for(&root);
    ensure_seeded(&ac_dir, None);
    let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
    assert!(
        !crate::config::seed_manifest::has_catalog_publication(&root).unwrap(),
        "the ungated seed recorded nothing"
    );

    let token = ManifestActivationToken::for_test();
    ensure_seeded_for_project_with_token(&root, Some(&token));
    assert!(
        crate::config::seed_manifest::has_catalog_publication(&root).unwrap(),
        "the verified current base is recorded on the next initialization"
    );
    assert_eq!(
        std::fs::read(manifest_path(&ac_dir)).unwrap(),
        base_before,
        "bookkeeping never republishes or rewrites the base"
    );
}

#[test]
fn managed_catalog_report_flags_an_untracked_verified_base() {
    let tracked = seed_dir();
    let root = tracked.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    ensure_seeded(&ac_dir_for(&root), None);
    let settings = AppSettings {
        project_paths: vec![root.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let report = report_json(&load_catalog_report_for_settings(&settings));
    assert!(report.contains("publicationUntracked"), "{report}");

    // A legacy base skips the bookkeeping query entirely.
    let legacy = seed_dir();
    write_legacy_base(
        &ac_dir_for(legacy.path()),
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
    );
    let settings = AppSettings {
        project_paths: vec![legacy.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let report = report_json(&load_catalog_report_for_settings(&settings));
    assert!(!report.contains("publicationUntracked"), "{report}");
}
