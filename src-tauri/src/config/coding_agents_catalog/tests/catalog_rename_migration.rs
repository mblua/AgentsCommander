use super::*;

#[test]
fn the_migration_runs_before_the_base_is_seeded_in_one_locked_pass() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let user = preserved_base_bytes("mine");
    std::fs::write(catalog.join(OLD_BASE), &user).unwrap();

    fx.seed();

    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        user,
        "the user's catalog survives at the new name; no fresh base was written"
    );
    assert!(!catalog.join(OLD_BASE).exists());
    assert!(
        deprecated_entries(&catalog).is_empty(),
        "nothing was set aside"
    );
    assert_eq!(
        fx.record(&fx.project_key()).status,
        naming_migration::ScopeStatus::Complete
    );
}

#[test]
fn every_seeding_entry_point_migrates_before_it_creates() {
    // ensure_seeded_for_project_in, gated.
    let fx = B2Fixture::new();
    let user = preserved_base_bytes("mine");
    std::fs::write(fx.project_catalog().join(OLD_BASE), &user).unwrap();
    let token = ManifestActivationToken::for_test();
    ensure_seeded_for_project_in(
        fx.root(),
        Some(&token),
        Some(&fx.legacy()),
        Some(fx.journal_dir()),
    );
    // ensure_seeded_for_project_in, activation None.
    let plain = B2Fixture::new();
    std::fs::write(plain.project_catalog().join(OLD_BASE), &user).unwrap();
    plain.seed();
    for (label, catalog) in [
        ("gated", fx.project_catalog()),
        ("activation None", plain.project_catalog()),
    ] {
        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            user,
            "{label}"
        );
        assert!(!catalog.join(OLD_BASE).exists(), "{label}");
        assert!(deprecated_entries(&catalog).is_empty(), "{label}");
    }
    // ensure_seeded_instance_in.
    let instance = tempfile::tempdir().unwrap();
    let catalog = catalog_dir(instance.path());
    std::fs::create_dir_all(&catalog).unwrap();
    std::fs::write(catalog.join(OLD_BASE), &user).unwrap();
    ensure_seeded_instance_in(instance.path(), Some(instance.path()));
    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        user,
        "instance"
    );
    assert!(!catalog.join(OLD_BASE).exists(), "instance");
    assert!(deprecated_entries(&catalog).is_empty(), "instance");
}

#[test]
fn a_recoverable_sidecar_recovers_then_renames_in_one_pass() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    interrupted_1968_migration(&catalog, "after_local");
    let backup = std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
    let journal = std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap();

    fx.seed();

    assert!(!catalog.join(OLD_BASE).exists());
    assert!(!catalog.join(OLD_LOCAL).exists());
    let base: serde_json::Value =
        serde_json::from_slice(&std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap())
            .unwrap();
    assert_eq!(
        base["managed"]["owner"], "agentscommander",
        "the recovered base"
    );
    assert!(
        local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
        "the recovered agent is at the new local name"
    );
    let report = load_catalog_report(&fx.ac_dir());
    assert!(
        report.catalog.iter().any(|agent| agent.key == "mine"),
        "the effective catalog loaded from disk serves the recovered agent: {:?}",
        report.warnings
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        backup
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap(),
        journal
    );
    let record = fx.record(&fx.project_key());
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    for sidecar in [MIGRATION_BACKUP_FILENAME, MIGRATION_JOURNAL_FILENAME] {
        assert!(
            record.notes.iter().any(|note| note.contains(sidecar)),
            "{sidecar} noted: {:?}",
            record.notes
        );
    }
    assert!(deprecated_entries(&catalog).is_empty());
}

/// E8e: data already at the new names beside a #1968 sidecar, with no
/// journal dir. F1 must not resurrect the old names; without its
/// `has_old_data` guard the set-aside ordinal grows by one every pass.
#[test]
fn a_sidecar_without_old_data_runs_no_recovery() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    interrupted_1968_migration(&catalog, "after_local");
    // Finish the data move by hand: the old names are gone.
    std::fs::rename(
        catalog.join(OLD_BASE),
        catalog.join(CATALOG_MANIFEST_FILENAME),
    )
    .unwrap();
    std::fs::rename(
        catalog.join(OLD_LOCAL),
        catalog.join(LOCAL_CATALOG_FILENAME),
    )
    .unwrap();
    assert!(catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
    assert!(catalog.join(MIGRATION_BACKUP_FILENAME).exists());
    for pass in 1..=2 {
        ensure_seeded_for_project_in(fx.root(), None, None, None);
        assert!(
            !catalog.join(OLD_BASE).exists(),
            "pass {pass}: agents.json recreated"
        );
        assert!(
            !catalog.join(OLD_LOCAL).exists(),
            "pass {pass}: agents.local.json recreated"
        );
        assert_eq!(
            deprecated_entries(&catalog),
            Vec::<String>::new(),
            "pass {pass}: nothing set aside"
        );
    }
    fx.seed();
    assert_eq!(
        fx.record(&fx.project_key()).status,
        naming_migration::ScopeStatus::Complete
    );
    assert!(deprecated_entries(&catalog).is_empty());
}

#[test]
fn a_blocked_recovery_defers_and_renames_nothing() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let source = interrupted_1968_migration(&catalog, "after_local");
    let backup_path = catalog.join(MIGRATION_BACKUP_FILENAME);
    let backup = std::fs::read(&backup_path).unwrap();
    let local = std::fs::read(catalog.join(OLD_LOCAL)).unwrap();
    std::fs::remove_file(&backup_path).unwrap();

    let outcome = run_catalog_initialization_with_scope(
        &fx.ac_dir(),
        Some(&fx.legacy()),
        Some(fx.journal_dir()),
        InitScope::Project,
    );
    assert!(outcome
        .warnings
        .iter()
        .any(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT));
    assert_eq!(std::fs::read(catalog.join(OLD_BASE)).unwrap(), source);
    assert_eq!(std::fs::read(catalog.join(OLD_LOCAL)).unwrap(), local);
    assert!(
        !catalog.join(CATALOG_MANIFEST_FILENAME).exists(),
        "nothing seeded"
    );
    assert!(
        !catalog.join(LOCAL_CATALOG_FILENAME).exists(),
        "nothing renamed"
    );
    let record = fx.record(&fx.project_key());
    assert_eq!(record.status, naming_migration::ScopeStatus::Deferred);
    assert!(
        record.notes.iter().any(|note| note.contains("backup")),
        "{:?}",
        record.notes
    );

    // Restoring the backup heals it with no journal surgery.
    std::fs::write(&backup_path, &backup).unwrap();
    fx.seed();
    assert!(!catalog.join(OLD_BASE).exists());
    assert!(local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"));
    assert_eq!(
        fx.record(&fx.project_key()).status,
        naming_migration::ScopeStatus::Complete
    );

    // Backup only (follow-up #2709): both renames done, every byte kept,
    // and the pass ends with the one pre-existing MIGRATION_CONFLICT.
    let only = B2Fixture::new();
    let catalog = only.project_catalog();
    let source = interrupted_1968_migration(&catalog, "after_backup");
    assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
    for pass in 1..=2 {
        let outcome = run_catalog_initialization_with_scope(
            &only.ac_dir(),
            Some(&only.legacy()),
            Some(only.journal_dir()),
            InitScope::Project,
        );
        let conflicts: Vec<_> = outcome
            .warnings
            .iter()
            .filter(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
            .collect();
        assert_eq!(conflicts.len(), 1, "pass {pass}: {:?}", outcome.warnings);
        assert!(!catalog.join(OLD_BASE).exists(), "pass {pass}");
        assert!(!catalog.join(OLD_LOCAL).exists(), "pass {pass}");
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            source,
            "pass {pass}"
        );
        let base: serde_json::Value = serde_json::from_slice(
            &std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(base["managed"]["owner"], "agentscommander", "pass {pass}");
        assert!(
            local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
            "pass {pass}"
        );
        assert!(deprecated_entries(&catalog).is_empty(), "pass {pass}");
        assert_eq!(
            only.record(&only.project_key()).status,
            naming_migration::ScopeStatus::Complete,
            "pass {pass}"
        );
    }
}

#[test]
fn the_instance_directory_never_resumes_a_sidecar() {
    let config = tempfile::tempdir().unwrap();
    let catalog = catalog_dir(config.path());
    std::fs::create_dir_all(&catalog).unwrap();
    let base = legacy_catalog_json();
    let local = local_bytes("instance-local");
    let journal = b"{\"not\":\"ours\"}".to_vec();
    std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
    std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();
    std::fs::write(catalog.join(MIGRATION_JOURNAL_FILENAME), &journal).unwrap();

    ensure_seeded_instance_in(config.path(), Some(config.path()));

    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert_eq!(
        std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
        local
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap(),
        journal
    );
    assert!(
        !catalog.join(MIGRATION_BACKUP_FILENAME).exists(),
        "no recovery ran"
    );
    let key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
    let record = naming_migration::read_journal(Some(config.path()))
        .unwrap()
        .unwrap()
        .scope(&key)
        .cloned()
        .unwrap();
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    assert!(record
        .notes
        .iter()
        .any(|note| note.contains(MIGRATION_JOURNAL_FILENAME)));
}

#[test]
fn happy_path_is_byte_preserving_and_idempotent() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let base = preserved_base_bytes("mine");
    let local = local_bytes("happy");
    std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
    std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();
    std::fs::write(catalog.join(RETIRED_CATALOG_LOCK_FILENAME), b"").unwrap();

    fx.seed();
    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert_eq!(
        std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
        local
    );
    assert!(
        catalog.join(RETIRED_CATALOG_LOCK_FILENAME).is_file(),
        "lock not renamed"
    );
    let record = fx.record(&fx.project_key());
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    assert!(record
        .notes
        .iter()
        .any(|note| note.contains(RETIRED_CATALOG_LOCK_FILENAME)));

    let before = fx.record(&fx.project_key());
    fx.seed();
    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert_eq!(
        std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
        local
    );
    assert_eq!(
        fx.record(&fx.project_key()),
        before,
        "a second run is a no-op"
    );
    assert!(deprecated_entries(&catalog).is_empty());
}

#[test]
fn generated_project_policy_tracks_project_catalog_and_ignores_personal() {
    let temp = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(temp.path())
            .output()
            .expect("git runs")
    };
    let init = git(&["init", "--quiet"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let ac = temp.path().join(".ac");
    std::fs::create_dir_all(ac.join("coding-agents")).unwrap();
    crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[]).unwrap();
    for (name, expected) in [
        (
            crate::config::instance_artifacts::CODING_AGENTS_PROJECT_TARGET_NAME,
            1,
        ),
        (LOCAL_CATALOG_FILENAME, 0),
    ] {
        let relative = format!(".ac/coding-agents/{name}");
        std::fs::write(temp.path().join(&relative), b"user owned").unwrap();
        let result = git(&["check-ignore", "-q", &relative]);
        assert_eq!(
            result.status.code(),
            Some(expected),
            "{relative}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let result = git(&["check-ignore", "-q", ".ac/.gitignore"]);
    assert_eq!(
        result.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn a_pre_existing_target_wins_and_the_old_catalog_is_set_aside() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let old = preserved_base_bytes("old");
    let winner = preserved_base_bytes("winner");
    std::fs::write(catalog.join(OLD_BASE), &old).unwrap();
    std::fs::write(catalog.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
    crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(
        &fx.ac_dir(),
        &["CLAUDE.md".to_string(), "AGENTS.md".to_string()],
    )
    .unwrap();

    fx.seed();

    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        winner
    );
    assert!(!catalog.join(OLD_BASE).exists());
    let set_aside = catalog.join("agents.json.deprecated-1.no-git");
    assert_eq!(std::fs::read(&set_aside).unwrap(), old);
    assert_eq!(
        deprecated_entries(&catalog),
        ["agents.json.deprecated-1.no-git"]
    );
    let record = fx.record(&fx.project_key());
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    assert!(record
        .steps
        .iter()
        .any(|step| step.from == OLD_BASE && step.state == naming_migration::StepState::SetAside));

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(fx.root())
            .output()
            .expect("git runs")
    };
    assert!(git(&["init", "-q"]).status.success());
    let ignored = git(&[
        "check-ignore",
        "-q",
        ".ac/coding-agents/agents.json.deprecated-1.no-git",
    ]);
    assert!(ignored.status.success(), "the set-aside catalog is ignored");
    let tracked = git(&[
        "check-ignore",
        "-q",
        ".ac/coding-agents/agents.10.default.json",
    ]);
    assert_eq!(
        tracked.status.code(),
        Some(1),
        "the new base stays trackable"
    );
}

#[test]
fn an_unreachable_catalog_dir_is_recorded_and_retried() {
    let journal = tempfile::tempdir().unwrap();
    let base = preserved_base_bytes("mine");
    // A root that does not exist, and one that is a file: skipped before
    // the chokepoint, no scope row, nothing Complete.
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("missing");
    let file_root = parent.path().join("file-root");
    std::fs::write(&file_root, b"x").unwrap();
    for root in [&missing, &file_root] {
        ensure_seeded_for_project_in(root, None, None, Some(journal.path()));
    }
    assert!(naming_migration::read_journal(Some(journal.path()))
        .unwrap()
        .is_none());
    // A root whose `.ac` cannot be created: recorded Unreachable.
    let blocked = tempfile::tempdir().unwrap();
    let ac = blocked
        .path()
        .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
    std::fs::write(&ac, b"not a directory").unwrap();
    ensure_seeded_for_project_in(blocked.path(), None, None, Some(journal.path()));
    let keys = scope_keys(journal.path());
    assert_eq!(keys.len(), 1, "{keys:?}");
    let journal_now = naming_migration::read_journal(Some(journal.path()))
        .unwrap()
        .unwrap();
    assert_eq!(
        naming_migration::status(&journal_now, &keys[0]),
        Some(naming_migration::ScopeStatus::Unreachable)
    );

    // Each made reachable migrates on the next pass.
    std::fs::create_dir_all(missing.join(".ac").join(CATALOG_DIR_NAME)).unwrap();
    std::fs::remove_file(&file_root).unwrap();
    std::fs::create_dir_all(file_root.join(".ac").join(CATALOG_DIR_NAME)).unwrap();
    std::fs::remove_file(&ac).unwrap();
    std::fs::create_dir_all(ac.join(CATALOG_DIR_NAME)).unwrap();
    for root in [missing.as_path(), file_root.as_path(), blocked.path()] {
        let catalog = catalog_dir(&root.join(".ac"));
        std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
        ensure_seeded_for_project_in(root, None, None, Some(journal.path()));
        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            base,
            "{}",
            root.display()
        );
        let key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
        let journal_now = naming_migration::read_journal(Some(journal.path()))
            .unwrap()
            .unwrap();
        assert_eq!(
            naming_migration::status(&journal_now, &key),
            Some(naming_migration::ScopeStatus::Complete)
        );
    }
}

#[test]
fn a_new_project_still_imports_the_legacy_instance_catalog() {
    let registered = B2Fixture::new();
    let legacy = registered.legacy();
    std::fs::create_dir_all(&legacy).unwrap();
    let instance = legacy_catalog_json();
    std::fs::write(legacy.join(OLD_BASE), &instance).unwrap();
    // A project already registered, with its own catalog.
    std::fs::write(
        registered.project_catalog().join(OLD_BASE),
        preserved_base_bytes("registered"),
    )
    .unwrap();
    registered.seed();

    // A second project created later, same instance.
    let second = tempfile::tempdir().unwrap();
    ensure_seeded_for_project_in(
        second.path(),
        None,
        Some(&legacy),
        Some(registered.journal_dir()),
    );
    let catalog = catalog_dir(&second.path().join(".ac"));
    assert!(
        local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
        "the new project imported the real instance catalog"
    );
    assert_eq!(
        std::fs::read(legacy.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        instance
    );
    assert!(!legacy.join(OLD_BASE).exists());
    for key in [registered.legacy_key(), registered.project_key()] {
        assert_eq!(
            registered.record(&key).status,
            naming_migration::ScopeStatus::Complete
        );
    }
    let second_key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
    assert_eq!(
        registered.record(&second_key).status,
        naming_migration::ScopeStatus::Complete
    );
}

#[test]
fn a_failed_legacy_scope_seeds_nothing() {
    let fx = B2Fixture::new();
    let legacy = fx.legacy();
    std::fs::create_dir_all(&legacy).unwrap();
    let instance = legacy_catalog_json();
    std::fs::write(legacy.join(OLD_BASE), &instance).unwrap();
    // The new lock sidecar is a directory: the scope's lock open fails
    // with an I/O refusal.
    std::fs::create_dir(legacy.join(CATALOG_LOCK_FILENAME)).unwrap();

    let outcome = run_catalog_initialization_with_scope(
        &fx.ac_dir(),
        Some(&legacy),
        Some(fx.journal_dir()),
        InitScope::Project,
    );
    let conflict = outcome
        .warnings
        .iter()
        .find(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
        .expect("a MIGRATION_CONFLICT outcome");
    assert!(conflict.reason.contains(OLD_BASE), "{}", conflict.reason);
    assert!(
        conflict.reason.contains(CATALOG_MANIFEST_FILENAME),
        "{}",
        conflict.reason
    );
    let catalog = fx.project_catalog();
    assert!(
        !catalog.join(CATALOG_MANIFEST_FILENAME).exists(),
        "no base written"
    );
    assert!(!catalog.join(LOCAL_CATALOG_FILENAME).exists());
    assert_eq!(std::fs::read(legacy.join(OLD_BASE)).unwrap(), instance);
    assert!(!legacy.join(CATALOG_MANIFEST_FILENAME).exists());

    // Both names present in the legacy dir: the winner is imported.
    let both = B2Fixture::new();
    let legacy = both.legacy();
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join(OLD_BASE), legacy_catalog_json()).unwrap();
    let winner = manifest_json(concat!(
        r##"[{"key":"winner","label":"Winner","description":"d","color":"#111","##,
        r##""command":"win","envs":[],"isolatedHome":false,"removable":true}]"##,
    ));
    std::fs::write(legacy.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
    both.seed();
    assert_eq!(
        std::fs::read_to_string(legacy.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        winner
    );
    assert_eq!(
        deprecated_entries(&legacy),
        ["agents.json.deprecated-1.no-git"]
    );
    assert!(local_carries(
        &both.project_catalog().join(LOCAL_CATALOG_FILENAME),
        "winner"
    ));
}

#[test]
fn the_two_catalog_locks_never_nest() {
    let fx = B2Fixture::new();
    let legacy = fx.legacy();
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join(OLD_BASE), legacy_catalog_json()).unwrap();
    let canonical_legacy = std::fs::canonicalize(&legacy).unwrap();
    let project_catalog = std::fs::canonicalize(fx.project_catalog()).unwrap();
    let project_paths = CatalogPaths::new(&project_catalog);
    let armed = naming_migration::pause::arm("before_rename", &canonical_legacy);

    let root = fx.root().to_path_buf();
    let journal = fx.journal_dir().to_path_buf();
    let worker = std::thread::spawn(move || {
        ensure_seeded_for_project_in(&root, None, Some(&legacy), Some(&journal));
    });
    armed
        .reached
        .recv_timeout(Duration::from_secs(30))
        .expect("the legacy scope reached its rename");
    // Leg 1: the project's section has not started.
    assert!(
        !project_paths.lock.exists(),
        "acquire_catalog_lock for the project has not run"
    );
    // Leg 2: the project lock is free while the legacy scope is held.
    let probe = acquire_catalog_lock(&project_paths);
    assert!(probe.is_ok(), "the project lock is free: {:?}", probe.err());
    drop(probe);
    armed.release.send(()).unwrap();
    worker.join().expect("worker completes");
    assert!(project_catalog.join(CATALOG_MANIFEST_FILENAME).exists());
}

#[test]
fn a_recovered_publication_is_still_recorded() {
    for point in ["after_local", "after_backup"] {
        // The pass's outcome carries the recovery's instant.
        let fx = B2Fixture::new();
        interrupted_1968_migration(&fx.project_catalog(), point);
        let outcome = run_catalog_initialization_with_scope(
            &fx.ac_dir(),
            Some(&fx.legacy()),
            Some(fx.journal_dir()),
            InitScope::Project,
        );
        assert!(
            outcome.published_at.is_some(),
            "{point}: {:?}",
            outcome.warnings
        );
        let conflicts = outcome
            .warnings
            .iter()
            .filter(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
            .count();
        assert_eq!(conflicts, usize::from(point == "after_backup"), "{point}");

        // Under the held project gate it is recorded exactly once.
        let gated = B2Fixture::new();
        interrupted_1968_migration(&gated.project_catalog(), point);
        let token = ManifestActivationToken::for_test();
        ensure_seeded_for_project_in(
            gated.root(),
            Some(&token),
            Some(&gated.legacy()),
            Some(gated.journal_dir()),
        );
        assert!(
            has_catalog_publication(gated.root()).unwrap(),
            "{point}: the publication row was recorded"
        );
        let manifest =
            std::fs::read_to_string(gated.ac_dir().join(SEED_MANIFEST_FILENAME)).unwrap();
        assert_eq!(
            manifest.matches("kind = \"coding_agent_catalog\"").count(),
            1,
            "{point}: {manifest}"
        );
    }
}

#[test]
fn the_migration_does_not_re_acquire_the_catalog_lock() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    std::fs::write(catalog.join(OLD_BASE), preserved_base_bytes("mine")).unwrap();
    std::fs::write(catalog.join(OLD_LOCAL), local_bytes("lock")).unwrap();
    let started = Instant::now();
    run_catalog_initialization_with_scope(
        &fx.ac_dir(),
        None,
        Some(fx.journal_dir()),
        InitScope::Project,
    );
    assert!(started.elapsed() < naming_migration::MIGRATION_LOCK_BUDGET);
    assert!(catalog.join(CATALOG_MANIFEST_FILENAME).is_file());
    assert!(catalog.join(LOCAL_CATALOG_FILENAME).is_file());
    assert!(!catalog.join(OLD_BASE).exists());
    assert!(!catalog.join(OLD_LOCAL).exists());
}

#[test]
fn b2_catalog_migration_child() {
    let (Some(root), Some(journal)) = (
        std::env::var_os(B2_CHILD_ROOT_ENV),
        std::env::var_os(B2_CHILD_JOURNAL_ENV),
    ) else {
        return;
    };
    ensure_seeded_for_project_in(Path::new(&root), None, None, Some(Path::new(&journal)));
    println!("AC_2715_CATALOG_CHILD_DONE");
}

#[test]
fn two_catalog_scopes_keep_both_record_sets() {
    let journal = tempfile::tempdir().unwrap();
    let rendezvous = tempfile::tempdir().unwrap();
    let projects = [B2Fixture::new(), B2Fixture::new()];
    let bytes = [
        preserved_base_bytes("first"),
        preserved_base_bytes("second"),
    ];
    for (fx, base) in projects.iter().zip(&bytes) {
        std::fs::write(fx.project_catalog().join(OLD_BASE), base).unwrap();
        std::fs::write(fx.project_catalog().join(OLD_LOCAL), local_bytes("two")).unwrap();
    }
    // The child migrates the first project and pauses mid-scope, holding
    // that project's locks while its first journal records exist.
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            B2_CHILD_TEST_FQN,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(B2_CHILD_ROOT_ENV, projects[0].root())
        .env(B2_CHILD_JOURNAL_ENV, journal.path())
        .env(naming_migration::pause::PAUSE_DIR_ENV, rendezvous.path())
        .env(naming_migration::pause::PAUSE_STAGE_ENV, "after_rename")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the migration child");
    wait_for_path(
        &rendezvous.path().join(naming_migration::pause::READY_FILE),
        Duration::from_secs(60),
        "migration child",
    );
    // This process migrates the second project in full meanwhile.
    ensure_seeded_for_project_in(projects[1].root(), None, None, Some(journal.path()));
    std::fs::write(
        rendezvous
            .path()
            .join(naming_migration::pause::RELEASE_FILE),
        b"go",
    )
    .unwrap();
    let output = child.wait_with_output().expect("reap the child");
    assert!(
        output.status.success(),
        "child: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("AC_2715_CATALOG_CHILD_DONE"));

    let journal_now = naming_migration::read_journal(Some(journal.path()))
        .unwrap()
        .unwrap();
    for (fx, base) in projects.iter().zip(&bytes) {
        let key = fx.project_key();
        let record = journal_now
            .scope(&key)
            .cloned()
            .expect("both scopes recorded");
        assert_eq!(
            record.status,
            naming_migration::ScopeStatus::Complete,
            "{key}"
        );
        for rename in &CATALOG_FAMILY_RENAMES {
            assert!(
                record.steps.iter().any(|step| step.from == rename.from
                    && step.state == naming_migration::StepState::Renamed),
                "{key}: {} step: {:?}",
                rename.from,
                record.steps
            );
        }
        assert_eq!(
            std::fs::read(fx.project_catalog().join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            *base
        );
    }
}

#[test]
fn the_instance_directory_has_one_scope_key() {
    let config = tempfile::tempdir().unwrap();
    let catalog = config.path().join(CATALOG_DIR_NAME);
    std::fs::create_dir_all(&catalog).unwrap();
    std::fs::write(catalog.join(OLD_BASE), legacy_catalog_json()).unwrap();
    let mut spellings = vec![
        catalog.clone(),
        config.path().join(".").join(CATALOG_DIR_NAME),
    ];
    if cfg!(windows) {
        spellings.push(PathBuf::from(catalog.to_string_lossy().to_uppercase()));
    }
    for legacy in &spellings {
        let project = tempfile::tempdir().unwrap();
        ensure_seeded_for_project_in(project.path(), None, Some(legacy), Some(config.path()));
    }
    ensure_seeded_instance_in(config.path(), Some(config.path()));
    let instance_key = catalog_scope_key(&ensure_catalog_dir(config.path()).unwrap());
    let keys = scope_keys(config.path());
    let for_instance: Vec<_> = keys
        .iter()
        .filter(|key| key.to_lowercase() == instance_key.to_lowercase())
        .collect();
    assert_eq!(for_instance, [&instance_key], "{keys:?}");
}

#[test]
fn a_complete_scope_whose_old_name_is_back_is_re_run() {
    for both_present in [false, true] {
        let fx = B2Fixture::new();
        let catalog = std::fs::canonicalize(fx.project_catalog()).unwrap();
        let key = fx.project_key();
        naming_migration::update_journal(Some(fx.journal_dir()), |journal| {
            for rename in &CATALOG_FAMILY_RENAMES {
                journal.record_step(
                    &key,
                    naming_migration::StepRecord::new(
                        &catalog,
                        rename.from,
                        rename.to,
                        naming_migration::StepState::Renamed,
                    ),
                );
            }
            journal.set_status(&key, naming_migration::ScopeStatus::Complete);
        })
        .unwrap();
        let old = preserved_base_bytes("back");
        std::fs::write(catalog.join(OLD_BASE), &old).unwrap();
        let winner = preserved_base_bytes("winner");
        if both_present {
            std::fs::write(catalog.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
        }

        fx.seed();

        assert!(!catalog.join(OLD_BASE).exists(), "both={both_present}");
        let expected = if both_present { &winner } else { &old };
        assert_eq!(
            &std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            expected,
            "both={both_present}: no fresh base"
        );
        if both_present {
            assert_eq!(
                std::fs::read(catalog.join("agents.json.deprecated-1.no-git")).unwrap(),
                old
            );
        } else {
            assert!(deprecated_entries(&catalog).is_empty());
        }
        assert_eq!(
            fx.record(&key).status,
            naming_migration::ScopeStatus::Complete
        );
    }
}

#[test]
fn no_config_dir_still_migrates_and_never_seeds_a_second_base() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let base = preserved_base_bytes("mine");
    let local = local_bytes("none");
    std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
    std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();

    ensure_seeded_for_project_in(fx.root(), None, None, None);

    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert_eq!(
        std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
        local
    );
    assert!(
        !catalog.join(OLD_BASE).exists(),
        "no base seeded beside agents.json"
    );
    let journal_names = |dir: &Path| -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.contains("naming-migration"))
            .collect()
    };
    assert!(journal_names(&catalog).is_empty());
    assert!(journal_names(&fx.ac_dir()).is_empty());
    assert!(journal_names(fx.journal_dir()).is_empty());

    // A second pass with a real journal finds nothing to do.
    fx.seed();
    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert!(deprecated_entries(&catalog).is_empty(), "nothing demoted");
    assert_eq!(
        fx.record(&fx.project_key()).status,
        naming_migration::ScopeStatus::Complete
    );
}
