use super::*;

#[test]
fn managed_catalog_stale_revision_warning_uses_the_exact_context_reason() {
    let agents = shipped_def_json(&["claude", "grok"]);

    // Direct: the public report wrapper keeps the neutral wording.
    let direct = seed_dir();
    write_managed_base(direct.path(), &agents, "stale-revision", true);
    let report = load_catalog_report(direct.path());
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.code == "refreshFailed")
        .expect("direct refreshFailed");
    assert_eq!(
        warning.reason,
        "The persisted managed catalog revision differs from this build. Current persisted entries \
            remain usable. Catalog refresh runs during initialization."
    );
    assert_eq!(
        warning.path,
        manifest_path(direct.path()).display().to_string()
    );

    // Project: the settings project branch supplies the project wording.
    let project = seed_dir();
    write_managed_base(&ac_dir_for(project.path()), &agents, "stale-revision", true);
    let settings = AppSettings {
        project_paths: vec![project.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.code == "refreshFailed")
        .expect("project refreshFailed");
    assert_eq!(
        warning.reason,
        "The persisted managed catalog revision differs from this build. Current persisted entries \
            remain usable. Restart retries project catalog refresh."
    );
    assert_eq!(
        warning.path,
        manifest_path(&ac_dir_for(project.path()))
            .display()
            .to_string()
    );

    // Instance: the no-project branch supplies the read-only wording
    // (deliberately NOT the public Direct wrapper).
    let instance = seed_dir();
    write_managed_base(instance.path(), &agents, "stale-revision", true);
    let report = load_catalog_report_for_settings_with_config_dir(
        &AppSettings::default(),
        Some(instance.path().to_path_buf()),
    );
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.code == "refreshFailed")
        .expect("instance refreshFailed");
    assert_eq!(
        warning.reason,
        "The persisted instance catalog revision differs from this build. Current persisted \
            entries remain usable. Restart retries instance catalog refresh when no project is \
            registered."
    );
    assert_eq!(
        warning.path,
        manifest_path(instance.path()).display().to_string()
    );
}

#[test]
fn managed_catalog_stale_revision_with_a_valid_pin_survives_repeated_reads() {
    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        true,
    );
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"PINNED"}]}"##,
    );
    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

    for _ in 0..3 {
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].label, "PINNED");
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "refreshFailed"));
    }
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
fn managed_catalog_stale_revision_warning_clears_after_a_successful_restart() {
    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        true,
    );
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "refreshFailed"));

    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "the stale base refreshes on initialization"
    );
    let refreshed = load_catalog_report(dir.path());
    assert!(refreshed.unavailable.is_none());
    assert!(refreshed.warnings.is_empty(), "{:?}", refreshed.warnings);
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
}

#[test]
fn managed_catalog_stale_revision_with_an_invalid_local_reports_both() {
    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        true,
    );
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"broken"}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 1, "the verified base stays served");
    for code in ["refreshFailed", "localInvalid"] {
        assert!(
            report.warnings.iter().any(|warning| warning.code == code),
            "missing {code}: {:?}",
            report.warnings
        );
    }
}

#[test]
fn managed_catalog_non_stale_and_unverified_bases_never_promise_a_restart() {
    // A current verified base is silent and usable.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(!report_json(&report).contains("refreshFailed"));

    // Edited content keeps managedBaseEdited and adds no restart promise.
    let dir = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["label"] = serde_json::json!("HAND EDITED");
    let bytes = write_managed_base(dir.path(), &[claude], "stale-revision", false);
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "managedBaseEdited"));
    assert!(report
        .warnings
        .iter()
        .all(|warning| warning.code != "refreshFailed"));
    assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), bytes);

    // A foreign marker keeps migrationConflict and adds no restart promise.
    let dir = seed_dir();
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": shipped_def_json(&["claude"]),
        "managed": {
            "owner": "somebody-else",
            "version": 1,
            "revision": "stale-revision",
            "contentSha256": "y",
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationConflict"));
    assert!(report
        .warnings
        .iter()
        .all(|warning| warning.code != "refreshFailed"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    // Unknown root fields block refresh and are preserved untouched.
    let dir = seed_dir();
    let definitions: Vec<CodingAgentDefinition> = shipped_def_json(&["claude"])
        .iter()
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect();
    let content = managed_content_sha256(&definitions);
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": definitions,
        "futureRootField": { "keep": "me" },
        "managed": {
            "owner": "agentscommander",
            "version": 1,
            "revision": "stale-revision",
            "contentSha256": content,
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
    assert!(report
        .warnings
        .iter()
        .all(|warning| warning.code != "refreshFailed"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    // Corrupt bytes stay unavailable with no restart promise.
    let dir = seed_dir();
    std::fs::create_dir_all(catalog_dir(dir.path())).unwrap();
    std::fs::write(manifest_path(dir.path()), b"not a catalog at all").unwrap();
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_some());
    assert!(!report_json(&report).contains("refreshFailed"));
}

#[test]
fn managed_catalog_local_symlink_and_dangling_link_are_preserved_as_invalid() {
    // A real file symlink at the local path: the layer is disabled, the
    // link identity/target and the target bytes survive every read.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let target = dir.path().join("user-local-target.json");
    let target_bytes = br##"{"schemaVersion":1,"agents":[{"key":"claude","label":"LINKED"}]}"##;
    std::fs::write(&target, target_bytes).unwrap();
    let local = local_catalog_path(dir.path());
    std::fs::remove_file(&local).unwrap();
    create_manifest_symlink(&target, &local)
        .expect("real local symlink fixture (not a silent skip)");
    assert!(std::fs::symlink_metadata(&local)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&local).unwrap(), target);

    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
    for _ in 0..2 {
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 8, "the managed base stays usable");
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "localInvalid"));
        assert!(!report.catalog.iter().any(|def| def.label == "LINKED"));
    }
    assert!(
        std::fs::symlink_metadata(&local)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is never replaced by a stub"
    );
    assert_eq!(std::fs::read_link(&local).unwrap(), target);
    assert_eq!(std::fs::read(&target).unwrap(), target_bytes);
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );

    // A dangling link: same degradation, link and absence preserved even
    // across a later initialization.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let local = local_catalog_path(dir.path());
    std::fs::remove_file(&local).unwrap();
    let missing = dir.path().join("no-such-local-target.json");
    create_manifest_symlink(&missing, &local).expect("real dangling local link fixture");
    let report = load_catalog_report(dir.path());
    assert_eq!(report.catalog.len(), 8);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "localInvalid"));
    assert!(std::fs::symlink_metadata(&local)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&local).unwrap(), missing);
    assert!(!missing.exists());
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "the verified base needs no rewrite"
    );
    assert!(std::fs::symlink_metadata(&local)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!missing.exists());
}

#[test]
fn managed_catalog_hard_link_unsupported_leaves_prior_artifacts_and_resumes() {
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_bytes = legacy_catalog_json();
    std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();
    let base = manifest_path(project.path());
    let local = local_catalog_path(project.path());
    let backup = catalog_dir(project.path()).join(MIGRATION_BACKUP_FILENAME);
    let journal = migration_journal_path(project.path());

    // SIMULATED error result at the link boundary (arm-level proof); real
    // unsupported-filesystem execution is unmeasured. The fault is armed
    // for the BASE destination only, so the earlier backup, journal and
    // local publications run for real.
    arm_catalog_path_fault("hard_link", &base, std::io::ErrorKind::Unsupported);
    let outcome = run_catalog_initialization(project.path(), Some(legacy.path()), None);
    clear_catalog_path_faults();

    assert!(outcome.published_at.is_none(), "no base was published");
    let conflict = outcome
        .warnings
        .iter()
        .find(|warning| warning.code == "migrationConflict")
        .expect("the link failure surfaces");
    assert!(conflict.reason.contains("hard link"), "{}", conflict.reason);
    assert!(!base.exists(), "the destination stays absent");
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        legacy_bytes,
        "the backup keeps the source bytes exactly"
    );
    assert!(journal.is_file(), "the journal survives");
    let local_bytes = std::fs::read(&local).expect("the extracted local survives");
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        legacy_bytes,
        "the source is never modified"
    );
    assert!(
        temp_residue(project.path()).is_empty(),
        "only this run's temp is removed: {:?}",
        temp_residue(project.path())
    );

    // Fault cleared: the interrupted transaction resumes and completes,
    // then a second initialization is idempotent.
    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
    let completed_base = std::fs::read(&base).unwrap();
    assert_eq!(
        std::fs::read(&local).unwrap(),
        local_bytes,
        "recovery never re-extracts or rewrites the local layer"
    );
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        legacy_bytes
    );
    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
    assert_eq!(std::fs::read(&base).unwrap(), completed_base);
    let base_value: serde_json::Value = serde_json::from_slice(&completed_base).unwrap();
    assert_eq!(base_value["managed"]["owner"], "agentscommander");
}

#[test]
fn managed_catalog_exclusive_publication_collision_preserves_the_destination() {
    let scratch = seed_dir();
    let destination = scratch.path().join("agents.local.json");
    let sentinel = b"USER SENTINEL BYTES\n";
    std::fs::write(&destination, sentinel).unwrap();
    let source = scratch.path().join("source-bytes.json");
    std::fs::write(&source, b"REPLACEMENT CONTENT\n").unwrap();

    // The REAL (uninjected) hard link reports AlreadyExists on this pair;
    // the injected Unsupported arm covers the absent destination instead.
    let real = std::fs::hard_link(&source, &destination).unwrap_err();
    assert_eq!(real.kind(), std::io::ErrorKind::AlreadyExists);

    let error = publish_exclusive(&destination, b"new content").unwrap_err();
    assert!(error.contains("exclusive publication"), "{error}");
    assert_eq!(std::fs::read(&destination).unwrap(), sentinel);
    assert!(
        dir_entries(scratch.path())
            .iter()
            .all(|name| !name.to_string_lossy().contains(".tmp")),
        "no publication temp survives"
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"REPLACEMENT CONTENT\n");
}

#[cfg(windows)]
#[test]
fn managed_catalog_replace_share_denial_keeps_bytes_and_recovers() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        true,
    );
    let base = manifest_path(dir.path());
    let base_before = std::fs::read(&base).unwrap();
    // A real destination handle that denies replacement: ReplaceFileW
    // fails through the production error path.
    let blocker = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&base)
        .unwrap();

    let outcome = run_catalog_initialization(dir.path(), None, None);
    assert!(outcome.published_at.is_none(), "ReplaceFileW must fail");
    assert!(
        outcome.warnings.iter().any(|warning| {
            warning.code == "refreshFailed" && warning.reason.contains("Failed to replace")
        }),
        "{:?}",
        outcome.warnings
    );
    assert_eq!(
        std::fs::read(&base).unwrap(),
        base_before,
        "the original bytes are unchanged"
    );
    assert!(
        temp_residue(dir.path()).is_empty(),
        "the failed publication removes its own temp: {:?}",
        temp_residue(dir.path())
    );

    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(
        report.catalog.len(),
        1,
        "the stale verified base stays usable"
    );
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "refreshFailed"));

    // Release the denial: two restarts refresh once and then stay idempotent.
    drop(blocker);
    let refreshed = run_catalog_initialization(dir.path(), None, None);
    assert!(refreshed.published_at.is_some(), "release then refresh");
    let base_after = std::fs::read(&base).unwrap();
    let second = run_catalog_initialization(dir.path(), None, None);
    assert!(second.published_at.is_none(), "the refresh is idempotent");
    assert_eq!(std::fs::read(&base).unwrap(), base_after);
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn managed_catalog_degraded_manifest_record_failure_repairs_without_republishing() {
    let temp = seed_dir();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join(".ac")).unwrap();
    let ac_dir = ac_dir_for(&root);
    let seed_manifest = root.join(".ac").join(SEED_MANIFEST_FILENAME);

    // A malformed canonical manifest is preserved read-only: the base still
    // publishes, but the row cannot be recorded (PublishedUnrecorded).
    std::fs::write(&seed_manifest, b"not = [valid toml").unwrap();
    ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
    assert!(
        manifest_path(&ac_dir).is_file(),
        "the base published anyway"
    );
    assert!(
        crate::config::seed_manifest::has_catalog_publication(&root).is_err(),
        "a malformed manifest is unreadable"
    );
    {
        let mut guard = ProjectSeedManifestGuard::acquire(&root)
            .expect("a malformed manifest is held read-only, not rejected");
        let row = PublishedManifestRow::coding_agent_catalog(
            ManifestPathIdentity::from_relative_path(Path::new(".ac/coding-agents/agents.json"))
                .unwrap(),
            Utc::now(),
        )
        .unwrap();
        let outcome = guard
            .publication_permit()
            .record_file(&ManifestActivationToken::for_test(), row);
        assert!(
            matches!(
                outcome,
                crate::config::seed_manifest::ManifestRecordOutcome::PublishedUnrecorded(_)
            ),
            "expected PublishedUnrecorded, got {outcome:?}"
        );
        guard.release();
    }

    let settings = AppSettings {
        project_paths: vec![root.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    assert!(report.unavailable.is_none());
    assert!(!report.catalog.is_empty(), "the catalog stays usable");
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "publicationUntracked"));
    let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
    let local_before = std::fs::read(local_catalog_path(&ac_dir)).unwrap();

    // Repair only the manifest: two initializations restore the row without
    // republishing the base or rewriting the local layer.
    std::fs::write(&seed_manifest, REPAIRED_EMPTY_MANIFEST).unwrap();
    ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
    assert!(crate::config::seed_manifest::has_catalog_publication(&root).unwrap());
    let manifest_after_repair = std::fs::read(&seed_manifest).unwrap();
    ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
    assert_eq!(
        std::fs::read(&seed_manifest).unwrap(),
        manifest_after_repair,
        "the second initialization neither re-records nor republishes"
    );
    assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_before);
    assert_eq!(
        std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
        local_before
    );
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    assert!(!report_json(&report).contains("publicationUntracked"));
}

#[test]
fn managed_catalog_reads_never_touch_an_interrupted_instance_source() {
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_bytes = legacy_catalog_json();
    std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();

    // Stop the migration after the local layer: base absent, backup +
    // journal + local present, and the instance source still the only
    // donor the read may consult.
    let _ = with_failure_at("after_local", || {
        ensure_seeded(project.path(), Some(legacy.path()))
    });
    let catalog = catalog_dir(project.path());
    assert!(catalog.join(MIGRATION_BACKUP_FILENAME).is_file());
    assert!(migration_journal_path(project.path()).is_file());
    assert!(local_catalog_path(project.path()).is_file());
    assert!(!manifest_path(project.path()).exists());

    assert_reads_leave_state_unchanged(project.path());
    let report = load_catalog_report(project.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        legacy_bytes,
        "reads never write the instance source; its bytes survive the rename at the new name"
    );
    assert!(!legacy.path().join("agents.json").exists());
}

#[test]
fn managed_catalog_stub_failure_is_nonfatal_and_never_retried() {
    let temp = seed_dir();
    let root = temp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let ac_dir = ac_dir_for(&root);
    let local = local_catalog_path(&ac_dir);

    // Destination-aware fault at the STUB publication only: the base link
    // runs for real because it targets a different path.
    arm_catalog_path_fault("hard_link", &local, std::io::ErrorKind::Unsupported);
    let outcome = run_catalog_initialization(&ac_dir, None, None);
    clear_catalog_path_faults();

    assert!(
        outcome.published_at.is_some(),
        "the base publication succeeds"
    );
    let canonical_local = std::fs::canonicalize(catalog_dir(&ac_dir))
        .unwrap()
        .join(LOCAL_CATALOG_FILENAME);
    let warning = outcome
        .warnings
        .iter()
        .find(|warning| warning.code == "refreshFailed")
        .expect("the transient stub warning");
    assert_eq!(warning.path, canonical_local.display().to_string());
    assert!(
        warning.reason.starts_with(
            "The managed catalog base is usable, but its local overrides stub could not be \
                created: "
        ),
        "{}",
        warning.reason
    );
    assert!(
        warning
            .reason
            .ends_with("An absent local file is valid; no automatic stub retry is scheduled."),
        "{}",
        warning.reason
    );
    assert!(!local.exists(), "the stub stays absent");
    assert_eq!(
        load_catalog(&ac_dir).unwrap().len(),
        8,
        "the base stays usable"
    );
    assert!(
        temp_residue(&ac_dir).is_empty(),
        "{:?}",
        temp_residue(&ac_dir)
    );

    // No persistent report warning: the base is already at this revision.
    let report = load_catalog_report(&ac_dir);
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    // Bookkeeping still records the verified current base without
    // republishing it, and a later initialization leaves the absent local
    // absent (no creation-on-every-start).
    let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
    ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
    assert!(crate::config::seed_manifest::has_catalog_publication(&root).unwrap());
    assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_before);
    assert!(!local.exists());
    let _ = run_catalog_initialization(&ac_dir, None, None);
    assert!(!local.exists());
}

#[test]
fn managed_catalog_lock_timeout_is_bounded_and_named() {
    assert_eq!(CATALOG_LOCK_TIMEOUT, Duration::from_secs(5));
    let dir = seed_dir();
    let catalog = catalog_dir(dir.path());
    std::fs::create_dir_all(&catalog).unwrap();
    let canonical = std::fs::canonicalize(&catalog).unwrap();
    let paths = CatalogPaths::new(&canonical);
    let held = acquire_catalog_lock(&paths).unwrap();

    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_millis(150))));
    let started = Instant::now();
    let error = acquire_catalog_lock(&paths).unwrap_err();
    let elapsed = started.elapsed();
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));

    assert!(error.contains("catalogLockTimeout"), "{error}");
    assert!(error.contains(CATALOG_LOCK_FILENAME), "{error}");
    assert!(error.contains("150 ms"), "{error}");
    assert!(
        elapsed >= Duration::from_millis(150) && elapsed < Duration::from_secs(2),
        "bounded wait, elapsed {elapsed:?}"
    );
    drop(held);
    let reacquired = acquire_catalog_lock(&paths).unwrap();
    drop(reacquired);
    assert!(paths.lock.is_file(), "the lock file is never removed");
}

#[test]
fn managed_catalog_lock_child() {
    let Some(action) = std::env::var_os(LOCK_CHILD_ACTION_ENV) else {
        return;
    };
    let action = action.to_string_lossy().into_owned();
    let dir = PathBuf::from(std::env::var_os(LOCK_CHILD_DIR_ENV).expect("child dir env"));
    let paths = CatalogPaths::new(&dir);
    match action.as_str() {
        "hold" => {
            let _lock = acquire_catalog_lock(&paths).expect("child must acquire the lock");
            std::fs::write(dir.join("child-ready"), b"ready").expect("child announces ready");
            let deadline = Instant::now() + Duration::from_secs(60);
            while !dir.join("child-release").exists() {
                assert!(
                    Instant::now() < deadline,
                    "child hold exceeded its 60s bound"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        other => panic!("unknown child action {other}"),
    }
    println!("AC_1968_CATALOG_LOCK_CHILD_DONE action={action}");
}

#[test]
fn managed_catalog_process_lock_contention_times_out_and_recovers() {
    let dir = seed_dir();
    let catalog = catalog_dir(dir.path());
    std::fs::create_dir_all(&catalog).unwrap();
    let canonical = std::fs::canonicalize(&catalog).unwrap();
    let (held_dir, mut child) = spawn_catalog_lock_child(&canonical);
    let paths = CatalogPaths::new(&held_dir);

    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_millis(250))));
    let error = acquire_catalog_lock(&paths).unwrap_err();
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));
    assert!(error.contains("catalogLockTimeout"), "{error}");

    std::fs::write(held_dir.join("child-release"), b"release").unwrap();
    let started = Instant::now();
    loop {
        match acquire_catalog_lock(&paths) {
            Ok(_) => break,
            Err(error) => {
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "the released child lock must become acquirable: {error}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let status = child.wait().expect("reap the lock child");
    assert!(status.success(), "the lock child must exit cleanly");
}

#[test]
fn managed_catalog_process_lock_holder_death_releases_the_lock() {
    let dir = seed_dir();
    let catalog = catalog_dir(dir.path());
    std::fs::create_dir_all(&catalog).unwrap();
    let canonical = std::fs::canonicalize(&catalog).unwrap();
    let exe = std::env::current_exe().expect("current test exe");
    let mut child = std::process::Command::new(exe)
        .args([
            "--exact",
            LOCK_CHILD_TEST_FQN,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(LOCK_CHILD_ACTION_ENV, "hold")
        .env(LOCK_CHILD_DIR_ENV, &canonical)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn catalog lock child");
    wait_for_path(
        &canonical.join("child-ready"),
        Duration::from_secs(30),
        "catalog lock child",
    );
    child.kill().expect("kill the lock holder");
    let _ = child.wait();

    let paths = CatalogPaths::new(&canonical);
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_secs(3))));
    let acquired = acquire_catalog_lock(&paths);
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));
    assert!(
        acquired.is_ok(),
        "a crashed holder releases the OS lock: {acquired:?}"
    );
}
