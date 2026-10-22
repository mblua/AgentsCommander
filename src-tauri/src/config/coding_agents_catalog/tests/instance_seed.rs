use super::*;

/// AC-1: a fresh config dir with no project seeds the instance managed base
/// and the no-project report lists exactly the supported built-ins.
#[test]
fn ensure_seeded_instance_fresh_config_dir_seeds_the_supported_builtins() {
    let instance = seed_dir();
    let published = ensure_seeded_instance(instance.path());
    assert!(published.is_some(), "an absent instance base is created");

    let report = instance_report(instance.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    let expected: Vec<String> = supported_shipped_definitions()
        .into_iter()
        .map(|definition| definition.key)
        .collect();
    let actual: Vec<String> = report
        .catalog
        .iter()
        .map(|definition| definition.key.clone())
        .collect();
    assert_eq!(
        actual, expected,
        "exactly the supported built-in keys in persisted order"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(instance.path())).unwrap(),
        LOCAL_STUB_BYTES,
        "the create-once local stub is part of a fresh instance seed"
    );
}

/// AC-2: a second call leaves the base bytes alone and publishes nothing.
#[test]
fn ensure_seeded_instance_is_idempotent() {
    let instance = seed_dir();
    assert!(ensure_seeded_instance(instance.path()).is_some());
    let base_before = std::fs::read(manifest_path(instance.path())).unwrap();
    let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

    assert!(ensure_seeded_instance(instance.path()).is_none());
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        base_before
    );
    assert_eq!(
        std::fs::read(local_catalog_path(instance.path())).unwrap(),
        local_before
    );
}

/// AC-3: a marker-less legacy instance file is never migrated, rewritten or
/// accompanied by sidecars.
#[test]
fn ensure_seeded_instance_leaves_a_legacy_instance_file_untouched() {
    let instance = seed_dir();
    let legacy = String::from_utf8(legacy_catalog_json()).expect("utf-8 legacy fixture");
    write_legacy_base(instance.path(), &legacy);
    let bytes_before = std::fs::read(manifest_path(instance.path())).unwrap();

    assert!(ensure_seeded_instance(instance.path()).is_none());
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        bytes_before
    );
    assert_eq!(
        non_lock_entries(instance.path()),
        vec![
            RETIRED_CATALOG_LOCK_FILENAME.to_string(),
            CATALOG_MANIFEST_FILENAME.to_string(),
        ],
        "a legacy instance file creates no local, backup or journal sidecar; the retired lock sidecar is the scope lock"
    );
    let report = instance_report(instance.path());
    assert!(report.unavailable.is_none());
    assert_eq!(
        report
            .catalog
            .iter()
            .map(|definition| definition.key.as_str())
            .collect::<Vec<_>>(),
        ["mine"],
        "the legacy instance file stays readable as before"
    );
}

/// AC-4: corrupt instance bytes are preserved and the report keeps the same
/// unavailability code before and after the call.
#[test]
fn ensure_seeded_instance_leaves_a_corrupt_instance_file_untouched() {
    let instance = seed_dir();
    write_legacy_base(instance.path(), "{ not JSON");
    let bytes_before = std::fs::read(manifest_path(instance.path())).unwrap();
    let code_before = instance_report(instance.path())
        .unavailable
        .as_ref()
        .map(|diagnostic| diagnostic.code.clone());
    assert_eq!(code_before.as_deref(), Some("baseInvalid"));

    assert!(ensure_seeded_instance(instance.path()).is_none());
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        bytes_before
    );
    let code_after = instance_report(instance.path())
        .unavailable
        .as_ref()
        .map(|diagnostic| diagnostic.code.clone());
    assert_eq!(code_after, code_before);
}

/// AC-5: a MANAGED instance base must not be imported as legacy territory
/// by a later project; the project gets a fresh managed base instead of
/// MIGRATION_CONFLICT. This drives the same production initialization path
/// that `ensure_seeded_for_project_with_token` uses, with the instance
/// catalog dir injected explicitly because the process-global `config_dir()`
/// has no test seam.
#[test]
fn managed_instance_base_does_not_block_a_later_project_fresh_seed() {
    let instance = seed_dir();
    assert!(ensure_seeded_instance(instance.path()).is_some());
    assert_eq!(
        base_json(instance.path())["managed"]["owner"],
        "agentscommander"
    );

    let project = seed_dir();
    let ac_dir = ac_dir_for(project.path());
    let outcome = run_catalog_initialization(&ac_dir, Some(&catalog_dir(instance.path())), None);
    assert!(
        !outcome
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationConflict"),
        "a managed instance base must not be imported as legacy territory: {:?}",
        outcome.warnings
    );
    assert!(
        outcome.published_at.is_some(),
        "the project fresh-seeds its own managed base"
    );

    let base = base_json(&ac_dir);
    assert_eq!(base["managed"]["owner"], "agentscommander");
    assert_eq!(
        std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
        LOCAL_STUB_BYTES,
        "the project's own local stub is published"
    );
    let entries = non_lock_entries(&ac_dir);
    assert!(
        !entries.contains(&MIGRATION_BACKUP_FILENAME.to_string())
            && !entries.contains(&MIGRATION_JOURNAL_FILENAME.to_string()),
        "no project backup or journal may be written: {entries:?}"
    );

    let report = load_catalog_report_for_settings_with_config_dir(
        &AppSettings {
            project_paths: vec![project.path().to_string_lossy().to_string()],
            ..AppSettings::default()
        },
        None,
    );
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert!(
        !report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationConflict"),
        "the managed instance base must not be imported: {:?}",
        report.warnings
    );
    assert_eq!(report.catalog.len(), 8);
}

/// AC-8: an existing instance local layer still wins over the seeded base
/// with no project registered.
#[test]
fn ensure_seeded_instance_local_overrides_win_with_no_project() {
    let instance = seed_dir();
    write_local(
        instance.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MY CLAUDE","updateCommands":["my-claude-update"]}]}"##,
    );
    let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

    assert!(ensure_seeded_instance(instance.path()).is_some());
    assert_eq!(
        std::fs::read(local_catalog_path(instance.path())).unwrap(),
        local_before
    );

    let report = instance_report(instance.path());
    assert!(report.unavailable.is_none());
    let claude = report
        .catalog
        .iter()
        .find(|definition| definition.key == "claude")
        .expect("claude present");
    assert_eq!(claude.label, "MY CLAUDE");
    assert_eq!(
        claude.update_commands,
        vec!["my-claude-update".to_string()],
        "the local override wins over the shipped command list"
    );
    let codex = report
        .catalog
        .iter()
        .find(|definition| definition.key == "codex")
        .expect("codex present");
    assert_eq!(codex.label, "Codex", "unpinned agents keep shipped values");
}

/// AC-9: a stale verified managed instance base is refreshed to the current
/// fresh-seed bytes; an edited one is never replaced.
#[test]
fn ensure_seeded_instance_refreshes_only_a_stale_verified_base() {
    let stale = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["label"] = serde_json::json!("OLD LABEL");
    write_managed_base(stale.path(), &[claude], "stale-revision", true);
    assert!(
        ensure_seeded_instance(stale.path()).is_some(),
        "a stale managed instance base refreshes"
    );
    let refreshed = std::fs::read(manifest_path(stale.path())).unwrap();
    assert_eq!(
        base_json(stale.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    let fresh = seed_dir();
    assert!(ensure_seeded_instance(fresh.path()).is_some());
    assert_eq!(
        refreshed,
        std::fs::read(manifest_path(fresh.path())).unwrap(),
        "the refreshed bytes equal a fresh instance seed"
    );
    let report = instance_report(stale.path());
    assert!(
        !report
            .warnings
            .iter()
            .any(|warning| warning.code == "refreshFailed"),
        "a successful refresh leaves no stale-revision warning: {:?}",
        report.warnings
    );
    assert_eq!(report.catalog.len(), 8);

    // Edited managed base: readable but never replaced.
    let edited = seed_dir();
    write_managed_base(
        edited.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        false,
    );
    let edited_before = std::fs::read(manifest_path(edited.path())).unwrap();
    assert!(ensure_seeded_instance(edited.path()).is_none());
    assert_eq!(
        std::fs::read(manifest_path(edited.path())).unwrap(),
        edited_before
    );
}

/// AC-10: an existing local layer with an absent base is kept verbatim and
/// its overrides show in the report.
#[test]
fn ensure_seeded_instance_keeps_an_existing_local_with_an_absent_base() {
    let instance = seed_dir();
    write_local(
        instance.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"pi","label":"PINNED PI"}]}"##,
    );
    let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

    assert!(ensure_seeded_instance(instance.path()).is_some());
    assert_eq!(
        std::fs::read(local_catalog_path(instance.path())).unwrap(),
        local_before,
        "the create-once stub never overwrites an existing local layer"
    );
    let report = instance_report(instance.path());
    assert!(report.unavailable.is_none());
    let pi = report
        .catalog
        .iter()
        .find(|definition| definition.key == "pi")
        .expect("pi present");
    assert_eq!(pi.label, "PINNED PI");
}

/// 2.2 instance policy: a foreign journal beside the instance catalog is
/// never resumed, rewritten or removed; the base still fresh-seeds.
#[test]
fn ensure_seeded_instance_never_resumes_a_foreign_sidecar() {
    let instance = seed_dir();
    let journal_path = catalog_dir(instance.path()).join(MIGRATION_JOURNAL_FILENAME);
    std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
    std::fs::write(&journal_path, b"{ foreign journal }").unwrap();
    let journal_before = std::fs::read(&journal_path).unwrap();

    assert!(
        ensure_seeded_instance(instance.path()).is_some(),
        "the instance base still fresh-seeds"
    );
    assert_eq!(
        std::fs::read(&journal_path).unwrap(),
        journal_before,
        "the instance never resumes or rewrites a foreign journal"
    );
}

#[test]
fn command_account_key_drops_arguments_and_keeps_the_program_token() {
    assert_eq!(command_account_key("claude"), Some("claude".to_string()));
    assert_eq!(
        command_account_key("claude --dangerously-skip-permissions"),
        Some("claude".to_string())
    );
}
