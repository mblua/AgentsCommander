use super::*;

// ---- resolve_config_seed (pure path math, fail-soft) -------------------

#[test]
fn resolve_orders_matrix_then_workspace_each_profile_then_base_lowercased() {
    let ac_root = abs(r"C:\ws", "/ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    let matrix = ac_root.join("_agent_x");
    let ctx = ctx_with(&replica, Some(&ac_root), Some(&matrix));

    let resolved = resolve_config_seed(&seed_cfg(".claude"), "C", Some(&ctx), None).expect("some");
    // All matrix tiers outrank all workspace tiers; profile beats base in each.
    assert_eq!(resolved.candidates.len(), 4);
    assert_eq!(resolved.candidates[0].tier, ConfigSeedTier::MatrixProfile);
    assert_eq!(
        resolved.candidates[0].path,
        matrix.join("default_profile_c.claude")
    );
    assert_eq!(resolved.candidates[1].tier, ConfigSeedTier::MatrixBase);
    assert_eq!(resolved.candidates[1].path, matrix.join("default.claude"));
    assert_eq!(
        resolved.candidates[2].tier,
        ConfigSeedTier::WorkspaceProfile
    );
    assert_eq!(
        resolved.candidates[2].path,
        ac_root.join("default_profile_c.claude")
    );
    assert_eq!(resolved.candidates[3].tier, ConfigSeedTier::WorkspaceBase);
    assert_eq!(resolved.candidates[3].path, ac_root.join("default.claude"));
    assert_eq!(resolved.dest, replica.join(".claude"));
    // Regression guard: the matrix's bare `.claude` (the agent's own live
    // config) must NEVER be a source candidate.
    assert!(
        !resolved
            .candidates
            .iter()
            .any(|c| c.path == matrix.join(".claude")),
        "bare matrix .claude must not be a seed source"
    );
}

#[test]
fn resolve_omits_matrix_when_absent() {
    let ac_root = abs(r"C:\ws", "/ws");
    let replica = ac_root.join("ac-root-agent");
    let ctx = ctx_with(&replica, Some(&ac_root), None);

    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).expect("some");
    // Root-agent: matrix is None, so only the two workspace tiers remain.
    let tiers: Vec<_> = resolved.candidates.iter().map(|c| c.tier).collect();
    assert_eq!(
        tiers,
        vec![
            ConfigSeedTier::WorkspaceProfile,
            ConfigSeedTier::WorkspaceBase
        ]
    );
}

#[test]
fn resolve_is_none_for_non_replica_root() {
    let ctx = PlaceholderContext {
        replica_root: abs(r"C:\cwd", "/cwd"),
        root_kind: PlaceholderRootKind::NormalLaunchCwd,
        ac_root: Some(abs(r"C:\ws", "/ws")),
        matrix_root: None,
    };
    assert!(resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).is_none());
}

#[test]
fn resolve_is_none_for_bad_dest_failsoft() {
    let ac_root = abs(r"C:\ws", "/ws");
    let replica = ac_root.join("wg-1").join("__agent_x");
    let ctx = ctx_with(&replica, Some(&ac_root), None);
    // A dest that bypassed save validation must fail soft (None), not abort.
    assert!(resolve_config_seed(&seed_cfg("../escape"), "A", Some(&ctx), None).is_none());
    assert!(resolve_config_seed(&seed_cfg("a/b"), "A", Some(&ctx), None).is_none());
}

#[test]
fn resolve_is_none_without_context() {
    assert!(resolve_config_seed(&seed_cfg(".claude"), "A", None, None).is_none());
}

// ---- perform_config_seed (trash-first atomic swap) ---------------------

#[test]
fn perform_seeds_from_highest_present_tier_and_clean_replaces_dest() {
    // Independent fixtures: Matrix base/profile, missing Matrix templates,
    // absent Matrix root, project base, and empty Matrix base.
    for (
        matrix_present,
        matrix_profile,
        matrix_base,
        project_profile,
        expected_tier,
        expected_bytes,
    ) in [
        (
            true,
            false,
            Some(b"MX_BASE".as_slice()),
            true,
            ConfigSeedTier::MatrixBase,
            b"MX_BASE".as_slice(),
        ),
        (
            true,
            true,
            Some(b"MX_BASE".as_slice()),
            true,
            ConfigSeedTier::MatrixProfile,
            b"MX_PROFILE".as_slice(),
        ),
        (
            true,
            false,
            None,
            true,
            ConfigSeedTier::WorkspaceProfile,
            b"PROFILE".as_slice(),
        ),
        (
            false,
            false,
            None,
            true,
            ConfigSeedTier::WorkspaceProfile,
            b"PROFILE".as_slice(),
        ),
        (
            true,
            false,
            None,
            false,
            ConfigSeedTier::WorkspaceBase,
            b"BASE".as_slice(),
        ),
        (
            true,
            false,
            Some(b"".as_slice()),
            true,
            ConfigSeedTier::MatrixBase,
            b"".as_slice(),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let ac_root = temp.path().join("ws");
        let replica = ac_root.join("wg-1-team").join("__agent_x");
        let matrix = ac_root.join("_agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&ac_root.join("default.claude").join("f.txt"), b"BASE");
        if project_profile {
            write_file(
                &ac_root.join("default_profile_c.claude").join("f.txt"),
                b"PROFILE",
            );
        }
        if matrix_present {
            // Bare live Matrix config must never become a template.
            write_file(&matrix.join(".claude").join("live.txt"), b"LIVE_MATRIX");
        }
        if let Some(bytes) = matrix_base {
            let base = matrix.join("default.claude");
            std::fs::create_dir_all(&base).unwrap();
            if !bytes.is_empty() {
                write_file(&base.join("f.txt"), bytes);
            }
        }
        if matrix_profile {
            write_file(
                &matrix.join("default_profile_c.claude").join("f.txt"),
                b"MX_PROFILE",
            );
        }
        write_file(&replica.join(".claude").join("stale.txt"), b"OLD");
        let ctx = ctx_with(
            &replica,
            Some(&ac_root),
            matrix_present.then_some(matrix.as_path()),
        );
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "C", Some(&ctx), None).unwrap();
        let publication = assert_published(perform_config_seed(&resolved, "sfx1"));
        assert_eq!(publication.tier, expected_tier);
        assert_eq!(publication.dest, replica.join(".claude"));
        if expected_bytes.is_empty() {
            assert_eq!(publication.files, CollectedSeedFiles::Exact(vec![]));
            assert_eq!(std::fs::read_dir(&publication.dest).unwrap().count(), 0);
        } else {
            assert_eq!(
                publication.files,
                CollectedSeedFiles::Exact(vec![PathBuf::from("f.txt")])
            );
            assert_eq!(
                std::fs::read(publication.dest.join("f.txt")).unwrap(),
                expected_bytes
            );
        }
        assert!(!publication.dest.join("live.txt").exists());
        assert!(!publication.dest.join("stale.txt").exists());
        assert!(!replica.join(".claude.acseed-tmp-sfx1").exists());
        assert!(!replica.join(".claude.acseed-old-sfx1").exists());
    }
}

/// Replica config seeding still runs under the project gate, but its report and
/// installed target no longer create seed-manifest rows.
#[test]
fn config_seed_exact_publish_preserves_report_without_creating_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    let ac_root = project.join(".ac");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"SEED");

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "C", Some(&ctx), None).unwrap();
    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let publication =
        assert_published(perform_config_seed_recorded(&resolved, "sfx", Some(&token)));
    assert_eq!(publication.tier, ConfigSeedTier::WorkspaceBase);
    assert_eq!(publication.dest, resolved.dest);
    assert_eq!(
        publication.files,
        CollectedSeedFiles::Exact(vec![PathBuf::from("f.txt")])
    );
    assert_eq!(std::fs::read(resolved.dest.join("f.txt")).unwrap(), b"SEED");
    assert!(!ac_root.join("seed-manifest.toml").exists());
}

#[test]
fn config_seed_clocks_do_not_change_existing_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    let ac_root = project.join(".ac");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"SEED");
    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "C", Some(&ctx), None).unwrap();
    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let manifest_path = ac_root.join("seed-manifest.toml");
    let manifest_before = concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 1\n",
        "coverage_version = 2\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/Context.AgentsCommander.md\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"project_context_template\"\n",
        "scope = \"context:agentscommander\"\n",
        "source = \"builtin\"\n",
        "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/wg-1-team/__agent_x/.claude/settings.json\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"replica_config_file\"\n",
        "scope = \"config:.ac/wg-1-team/__agent_x/.claude\"\n",
        "source = \"workspace_base\"\n",
        "last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n"
    )
    .as_bytes()
    .to_vec();
    std::fs::write(&manifest_path, &manifest_before).unwrap();

    for (unique_sfx, published_at) in [
        (
            "sfx-clock-one",
            DateTime::parse_from_rfc3339("2026-07-20T15:47:23.456Z")
                .unwrap()
                .with_timezone(&Utc),
        ),
        (
            "sfx-clock-two",
            DateTime::parse_from_rfc3339("2026-07-21T15:47:23.456Z")
                .unwrap()
                .with_timezone(&Utc),
        ),
    ] {
        let publication = assert_published(perform_config_seed_recorded_with(
            &resolved,
            Some(&token),
            || {
                let nested =
                    crate::config::seed_manifest::ProjectSeedManifestGuard::acquire_with_timeout(
                        project,
                        std::time::Duration::ZERO,
                    )
                    .unwrap_err();
                assert!(matches!(nested, SeedManifestError::BusyTimeout { .. }));
                perform_config_seed_with_clock(&resolved, unique_sfx, &|| published_at)
            },
        ));
        assert_eq!(publication.tier, ConfigSeedTier::WorkspaceBase);
        assert_eq!(publication.dest, resolved.dest);
        assert_eq!(
            publication.files,
            CollectedSeedFiles::Exact(vec![PathBuf::from("f.txt")])
        );
        assert_eq!(publication.published_at, published_at);
        assert_eq!(std::fs::read(resolved.dest.join("f.txt")).unwrap(), b"SEED");
        assert_eq!(std::fs::read(&manifest_path).unwrap(), manifest_before);

        let reacquired =
            crate::config::seed_manifest::ProjectSeedManifestGuard::acquire_with_timeout(
                project,
                std::time::Duration::ZERO,
            )
            .unwrap();
        reacquired.release();
    }
}

// Stage E (#1064) end-to-end config-staging scale benchmark (plan section
// 7.4, 10.5 item 7): config staging is the only per-file target I/O, and the
// collector returns exactly the installed regular-file list. `#[ignore]`
// ---- #1596 pi shellPath seed (Windows-only) ---------------------------

#[cfg(target_os = "windows")]
fn seed_pi_fixture() -> (tempfile::TempDir, ResolvedConfigSeed, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.pi").join("extensions.txt"), b"EXT");
    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".pi"), "A", Some(&ctx), None).unwrap();
    (temp, resolved, replica)
}

#[cfg(target_os = "windows")]
#[test]
fn pi_seed_writes_shell_path_when_settings_absent() {
    // #1596: a `.pi` template without settings.json gets the Git-Bash
    // shellPath written; the seed report stays Published.
    let (_temp, resolved, replica) = seed_pi_fixture();
    let publication = assert_published(perform_config_seed(&resolved, "sfx"));
    assert_eq!(publication.dest, replica.join(".pi"));
    assert_eq!(
        std::fs::read(replica.join(".pi").join("settings.json")).unwrap(),
        b"{\"shellPath\": \"C:/Program Files/Git/bin/bash.exe\"}"
    );
    assert_eq!(
        std::fs::read(replica.join(".pi").join("extensions.txt")).unwrap(),
        b"EXT"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn pi_seed_preserves_existing_settings() {
    // #1596: a settings.json shipped by the template (operator-authored
    // content) is never overwritten: absent-only.
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(
        &ac_root.join("default.pi").join("settings.json"),
        b"{\"shellPath\": \"C:/custom/pwsh.exe\"}",
    );
    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".pi"), "A", Some(&ctx), None).unwrap();
    let publication = assert_published(perform_config_seed(&resolved, "sfx"));
    assert_eq!(publication.dest, replica.join(".pi"));
    assert_eq!(
        std::fs::read(replica.join(".pi").join("settings.json")).unwrap(),
        b"{\"shellPath\": \"C:/custom/pwsh.exe\"}"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn non_pi_seed_writes_no_settings_json() {
    // #1596: only an exact `.pi` dest triggers the shellPath step; a
    // non-`.pi` seed must not write settings.json anywhere.
    let (_temp, resolved, replica) = seed_swap_fixture(); // `.claude` dest
    let publication = assert_published(perform_config_seed(&resolved, "sfx"));
    assert_eq!(publication.dest, replica.join(".claude"));
    assert!(!replica.join(".claude").join("settings.json").exists());
    assert!(!replica.join(".pi").join("settings.json").exists());
    assert!(!replica.join("settings.json").exists());
}
