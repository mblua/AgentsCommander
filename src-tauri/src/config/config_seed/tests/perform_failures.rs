use super::*;

#[test]
fn perform_skips_and_preserves_dest_when_no_template_present() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&replica.join(".claude").join("keep.txt"), b"KEEP");

    let matrix = ac_root.join("_agent_x");
    write_file(&matrix.join(".claude").join("live.txt"), b"LIVE_MATRIX");
    let ctx = ctx_with(&replica, Some(&ac_root), Some(&matrix));
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    assert_skipped(perform_config_seed(&resolved, "s"));
    // Dest fully intact.
    assert_eq!(
        std::fs::read(replica.join(".claude").join("keep.txt")).unwrap(),
        b"KEEP"
    );
    assert!(!replica.join(".claude").join("live.txt").exists());
}

#[test]
fn perform_failed_copy_leaves_dest_intact() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"NEW");
    write_file(&replica.join(".claude").join("keep.txt"), b"OLD");
    // Fault injection: a FILE where the temp DIR would be created makes
    // copy_tree's create_dir_all fail, so the swap never touches dest.
    write_file(&replica.join(".claude.acseed-tmp-sfx"), b"blocker");

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    assert_failed(perform_config_seed(&resolved, "sfx"));
    // Dest fully intact (old content kept).
    assert_eq!(
        std::fs::read(replica.join(".claude").join("keep.txt")).unwrap(),
        b"OLD"
    );
}

#[test]
fn install_failure_restores_the_operation_owned_previous_scope() {
    let (_temp, resolved, replica) = seed_swap_fixture();
    let hooks = ConfigSeedTestHooks {
        fail_install: true,
        ..ConfigSeedTestHooks::default()
    };

    let report =
        perform_config_seed_with_clock_and_hooks(&resolved, "restore-ok", &Utc::now, &hooks);

    assert_failed(report);
    assert_eq!(
        std::fs::read(replica.join(".claude").join("old.txt")).unwrap(),
        b"OLD"
    );
    assert!(!replica.join(".claude.acseed-old-restore-ok").exists());
}

#[test]
fn install_and_restore_failure_reports_logical_removal_and_preserves_trash() {
    let (_temp, resolved, replica) = seed_swap_fixture();
    let hooks = ConfigSeedTestHooks {
        fail_install: true,
        fail_restore: true,
        ..ConfigSeedTestHooks::default()
    };

    let report =
        perform_config_seed_with_clock_and_hooks(&resolved, "restore-fail", &Utc::now, &hooks);

    let ConfigSeedReport::FailedAfterLogicalRemoval(
        ConfigSeedRollbackFailure::PreviousScopeStillStaged {
            trash_path,
            install_error,
            restore_error,
            ..
        },
    ) = report
    else {
        panic!("expected logical-removal failure, got {report:?}");
    };
    assert!(install_error.contains("injected config-seed install failure"));
    assert!(restore_error.contains("injected config-seed restore failure"));
    assert!(restore_error.contains("operation-owned previous config remains staged"));
    assert_eq!(std::fs::read(trash_path.join("old.txt")).unwrap(), b"OLD");
    assert!(!replica.join(".claude").exists());
}

#[test]
fn destination_appearing_before_restore_cannot_collapse_logical_removal() {
    let (_temp, resolved, replica) = seed_swap_fixture();
    let hooks = ConfigSeedTestHooks {
        fail_install: true,
        destination_appears_before_restore: true,
        ..ConfigSeedTestHooks::default()
    };

    let report =
        perform_config_seed_with_clock_and_hooks(&resolved, "dest-appeared", &Utc::now, &hooks);

    let ConfigSeedReport::FailedAfterLogicalRemoval(
        ConfigSeedRollbackFailure::PreviousScopeStillStaged { trash_path, .. },
    ) = report
    else {
        panic!("expected logical-removal failure, got {report:?}");
    };
    assert_eq!(
        std::fs::read(replica.join(".claude").join("foreign.txt")).unwrap(),
        b"FOREIGN"
    );
    assert_eq!(std::fs::read(trash_path.join("old.txt")).unwrap(), b"OLD");
}

#[test]
fn staged_metadata_error_after_restore_failure_remains_logical_removal() {
    let (_temp, resolved, _replica) = seed_swap_fixture();
    let hooks = ConfigSeedTestHooks {
        fail_install: true,
        fail_restore: true,
        fail_staged_metadata: true,
        ..ConfigSeedTestHooks::default()
    };

    let report =
        perform_config_seed_with_clock_and_hooks(&resolved, "metadata-fail", &Utc::now, &hooks);

    let ConfigSeedReport::FailedAfterLogicalRemoval(
        ConfigSeedRollbackFailure::PreviousScopeStillStaged { restore_error, .. },
    ) = report
    else {
        panic!("expected logical-removal failure, got {report:?}");
    };
    assert!(restore_error.contains("injected metadata failure"));
}

#[test]
fn perform_refuses_to_recreate_a_stale_missing_replica() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"NEW");
    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    std::fs::remove_dir_all(&replica).unwrap();

    let report = perform_config_seed(&resolved, "stale");

    assert!(matches!(
        report,
        ConfigSeedReport::Skipped(ConfigSeedSkipReason::StaleReplica { .. })
    ));
    assert!(!replica.exists());
}

#[test]
fn publication_carries_the_injected_commit_point_time() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"NEW");
    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    let expected = DateTime::parse_from_rfc3339("2026-07-20T15:47:23.456Z")
        .unwrap()
        .with_timezone(&Utc);

    let publication = assert_published(perform_config_seed_with_clock(&resolved, "clock", &|| {
        expected
    }));

    assert_eq!(publication.published_at, expected);
}

#[test]
fn collector_switches_irreversibly_to_constant_memory_at_the_row_bound() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    let mut collector = SeedFileCollector::new(&resolved, ConfigSeedTier::WorkspaceBase);
    for index in 0..=MAX_MANIFEST_ROWS {
        collector.record(PathBuf::from(format!("f-{index}")));
    }

    assert_eq!(
        collector.finish(),
        CollectedSeedFiles::OverBound {
            reason: ResourceBoundKind::Rows,
            observed_at_least: (MAX_MANIFEST_ROWS + 1) as u64,
        }
    );
}

/// H1 "dest in use" branch: when the existing dest is locked by a live
/// process, the dest->trash rename fails and the seed is SKIPPED with the
/// existing config kept fully intact. Windows-specific (Unix renames a
/// directory even with open handles inside it), which is the whole reason H1
/// exists: the prior process still holding handles in `.claude`.
#[cfg(windows)]
#[test]
fn perform_skips_and_preserves_dest_when_dest_is_locked() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"NEW");
    write_file(&replica.join(".claude").join("keep.txt"), b"OLD");

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();

    // Hold an open handle to a file inside dest: Windows then refuses to
    // rename the dest directory (File::open grants no FILE_SHARE_DELETE),
    // forcing the dest->trash rename to fail -> skip, dest untouched.
    let locked = std::fs::File::open(replica.join(".claude").join("keep.txt")).unwrap();
    let report = perform_config_seed(&resolved, "sfx");
    drop(locked);

    assert_skipped(report);
    // Existing config fully intact.
    assert_eq!(
        std::fs::read(replica.join(".claude").join("keep.txt")).unwrap(),
        b"OLD"
    );
    // Staged temp was cleaned up.
    assert!(!replica.join(".claude.acseed-tmp-sfx").exists());
}

#[test]
fn perform_clears_stale_temp_and_trash_from_prior_run() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"NEW");
    // Stale dirs from a crashed prior run.
    write_file(
        &replica.join(".claude.acseed-tmp-sfx").join("junk.txt"),
        b"stale",
    );
    write_file(
        &replica.join(".claude.acseed-old-sfx").join("junk.txt"),
        b"stale",
    );

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    assert_published(perform_config_seed(&resolved, "sfx"));
    assert_eq!(
        std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
        b"NEW"
    );
    assert!(!replica.join(".claude.acseed-tmp-sfx").exists());
    assert!(!replica.join(".claude.acseed-old-sfx").exists());
}

#[test]
fn perform_reclaims_leaked_scratch_from_other_session_ids() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"NEW");
    // Scratch leaked by a prior run whose removal was locked — note the
    // DIFFERENT session id. A per-suffix cleanup would never reclaim these.
    write_file(
        &replica.join(".claude.acseed-old-OLDID").join("junk.txt"),
        b"stale",
    );
    write_file(
        &replica.join(".claude.acseed-tmp-OLDID").join("junk.txt"),
        b"stale",
    );

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    assert_published(perform_config_seed(&resolved, "NEWID"));
    assert!(!replica.join(".claude.acseed-old-OLDID").exists());
    assert!(!replica.join(".claude.acseed-tmp-OLDID").exists());
}

/// grinch HIGH-1 characterization: proves WHY `perform_config_seed` must be
/// serialized for all dests. We reconstruct the exact mid-swap state of a
/// concurrent spawn A (dest already renamed to trash_A, staged temp_A
/// present) and run what spawn B's step-2 cleanup does for the SAME dest with
/// a DIFFERENT session id. The prefix-sweep deletes BOTH of A's in-flight
/// dirs, after which A can neither install (temp gone) nor restore (trash
/// gone) -> config lost. This is the data-loss the session chokepoint's
/// ConfigSeedLockState serialization (held across the seed for ALL dests)
/// prevents; a regression that drops or narrows that lock re-opens it.
#[test]
fn unserialized_prefix_sweep_would_destroy_a_concurrent_inflight_swap() {
    let temp = tempfile::tempdir().unwrap();
    let replica = temp.path().join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();

    // Spawn A is mid-swap on dest ".claude-amp" (the previously-unlocked path):
    // dest has been renamed to trash_A (its ONLY surviving old copy) and
    // temp_A holds the freshly staged new config.
    let trash_a = replica.join(".claude-amp.acseed-old-idA");
    let temp_a = replica.join(".claude-amp.acseed-tmp-idA");
    write_file(&trash_a.join("keep.txt"), b"OLD");
    write_file(&temp_a.join("f.txt"), b"NEW");
    assert!(!replica.join(".claude-amp").exists(), "A moved dest aside");

    // Spawn B (different session id) runs its step-2 prefix-sweep concurrently.
    clear_stale_seed_scratch(&replica, ".claude-amp");

    // Hazard: B deleted BOTH of A's in-flight dirs. A's later
    // rename(temp_A->dest) and restore(trash_A->dest) would both fail,
    // leaving the replica with NO .claude-amp -> the exact data loss the
    // chokepoint lock serializes against.
    assert!(
        !temp_a.exists(),
        "prefix-sweep deletes a concurrent spawn's staged temp"
    );
    assert!(
        !trash_a.exists(),
        "prefix-sweep deletes a concurrent spawn's only old copy"
    );
}

// ---- CatalogDefault tier (#769 P2: absent-only + non-empty) ------------

/// Append the absent-only CatalogDefault candidate LAST, exactly as the spawn
/// caller (`build_agent_spawn_command`) does.
fn with_catalog_default(mut resolved: ResolvedConfigSeed, master: &Path) -> ResolvedConfigSeed {
    resolved.candidates.push(ConfigSeedCandidate {
        tier: ConfigSeedTier::CatalogDefault,
        os_specific: false,
        path: master.to_path_buf(),
    });
    resolved
}

#[test]
fn catalog_default_fills_absent_dest_from_nonempty_master() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    // No workspace/matrix templates on disk: the 4 legacy tiers are absent.
    let master = temp.path().join("master");
    write_file(&master.join("settings.json"), b"{\"seeded\":true}");

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    let resolved = with_catalog_default(resolved, &master);

    // dest absent + master non-empty -> CatalogDefault wins and fills.
    assert_published(perform_config_seed(&resolved, "sfx"));
    assert_eq!(
        std::fs::read(replica.join(".claude").join("settings.json")).unwrap(),
        b"{\"seeded\":true}"
    );
}

#[test]
fn catalog_default_absent_only_never_overwrites_existing_dest() {
    // The E1/G1 data-loss killer: a present dest is preserved, tier skips.
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    // Live replica config the seed must never touch.
    write_file(&replica.join(".claude").join("creds.json"), b"SECRET");
    let master = temp.path().join("master");
    write_file(&master.join("settings.json"), b"{\"seeded\":true}");

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    let resolved = with_catalog_default(resolved, &master);

    // dest present -> CatalogDefault gate fails -> Skipped, dest fully intact.
    assert_skipped(perform_config_seed(&resolved, "sfx"));
    assert_eq!(
        std::fs::read(replica.join(".claude").join("creds.json")).unwrap(),
        b"SECRET"
    );
    // The master content did NOT leak in.
    assert!(!replica.join(".claude").join("settings.json").exists());
}

#[test]
fn catalog_default_empty_master_is_inert() {
    // Non-empty gate: an empty master reads as "not present" -> Skipped, and
    // (dest absent) nothing is created, so spawn behavior is byte-identical.
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    let master = temp.path().join("master");
    std::fs::create_dir_all(&master).unwrap(); // exists but EMPTY

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    let resolved = with_catalog_default(resolved, &master);

    assert_skipped(perform_config_seed(&resolved, "sfx"));
    assert!(!replica.join(".claude").exists());
}

#[test]
fn catalog_default_yields_to_a_present_legacy_tier() {
    // A present workspace tier still wins (unchanged behavior); CatalogDefault
    // is only reached when all 4 legacy tiers are absent.
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"WORKSPACE");
    let master = temp.path().join("master");
    write_file(&master.join("f.txt"), b"CATALOG");

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    let resolved = with_catalog_default(resolved, &master);

    assert_published(perform_config_seed(&resolved, "sfx"));
    assert_eq!(
        std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
        b"WORKSPACE"
    );
}

#[test]
fn copy_tree_verbatim_preserves_tokens_when_substitute_false() {
    // The reseed backup copies the master verbatim (raw %AC_% tokens intact).
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    write_file(&src.join("a.txt"), b"path=%AC_REPLICA_ROOT%/x");
    let dst = temp.path().join("dst");
    copy_tree(&src, &dst, 0, None, false).unwrap();
    assert_eq!(
        std::fs::read(dst.join("a.txt")).unwrap(),
        b"path=%AC_REPLICA_ROOT%/x"
    );
}
