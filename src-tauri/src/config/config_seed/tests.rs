use super::*;
use crate::config::settings::CONFIG_SEED_OS_TOKENS;

mod copy_and_warnings;

mod perform_failures;

mod resolve_and_publish;

fn ctx_with(replica: &Path, ac_root: Option<&Path>, matrix: Option<&Path>) -> PlaceholderContext {
    PlaceholderContext {
        replica_root: replica.to_path_buf(),
        root_kind: PlaceholderRootKind::AcReplicaOrRootAgent,
        ac_root: ac_root.map(|p| p.to_path_buf()),
        matrix_root: matrix.map(|p| p.to_path_buf()),
    }
}

fn seed_cfg(dest: &str) -> ConfigSeedConfig {
    ConfigSeedConfig {
        enabled: true,
        dest: dest.to_string(),
    }
}

fn write_file(path: &Path, content: &[u8]) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn abs(win: &str, unix: &str) -> PathBuf {
    PathBuf::from(if cfg!(windows) { win } else { unix })
}

fn assert_published(report: ConfigSeedReport) -> ConfigSeedPublication {
    let ConfigSeedReport::Published(publication) = report else {
        panic!("expected config publication, got {report:?}");
    };
    publication
}

fn assert_skipped(report: ConfigSeedReport) {
    assert!(matches!(report, ConfigSeedReport::Skipped(_)), "{report:?}");
}

fn assert_failed(report: ConfigSeedReport) {
    assert!(matches!(report, ConfigSeedReport::Failed(_)), "{report:?}");
}

fn seed_swap_fixture() -> (tempfile::TempDir, ResolvedConfigSeed, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("new.txt"), b"NEW");
    write_file(&replica.join(".claude").join("old.txt"), b"OLD");
    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    (temp, resolved, replica)
}

// ---- #2162 OS-aware tiers ---------------------------------------------

/// Expected candidate pair (OS variant first, then base) for one tier.
fn os_pair(tier: ConfigSeedTier, root: &Path, folder: &str, os: &str) -> Vec<ConfigSeedCandidate> {
    vec![
        ConfigSeedCandidate {
            tier,
            os_specific: true,
            path: root.join(format!("{folder}.{os}")),
        },
        ConfigSeedCandidate {
            tier,
            os_specific: false,
            path: root.join(folder),
        },
    ]
}

/// Test 1: every token produces the exact 8-entry Matrix-first list.
/// Injected, so one host covers all three.
#[test]
fn os_candidates_for_each_token() {
    let ac_root = abs(r"C:\ws", "/ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    let matrix = ac_root.join("_agent_x");
    let ctx = ctx_with(&replica, Some(&ac_root), Some(&matrix));

    for os in CONFIG_SEED_OS_TOKENS {
        let resolved =
            resolve_config_seed(&seed_cfg(".claude"), "C", Some(&ctx), Some(os)).expect("some");
        let mut expected = Vec::new();
        expected.extend(os_pair(
            ConfigSeedTier::MatrixProfile,
            &matrix,
            "default_profile_c.claude",
            os,
        ));
        expected.extend(os_pair(
            ConfigSeedTier::MatrixBase,
            &matrix,
            "default.claude",
            os,
        ));
        expected.extend(os_pair(
            ConfigSeedTier::WorkspaceProfile,
            &ac_root,
            "default_profile_c.claude",
            os,
        ));
        expected.extend(os_pair(
            ConfigSeedTier::WorkspaceBase,
            &ac_root,
            "default.claude",
            os,
        ));
        assert_eq!(resolved.candidates, expected, "token {os}");
    }
}

/// Test 2: no token => the four plain Matrix-first candidates.
/// This is also the only coverage of `host_os_token`'s `None` branch shape.
#[test]
fn no_os_token_matches_today() {
    let ac_root = abs(r"C:\ws", "/ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    let matrix = ac_root.join("_agent_x");
    let ctx = ctx_with(&replica, Some(&ac_root), Some(&matrix));

    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).expect("some");
    assert_eq!(
        resolved.candidates,
        vec![
            ConfigSeedCandidate {
                tier: ConfigSeedTier::MatrixProfile,
                os_specific: false,
                path: matrix.join("default_profile_a.claude"),
            },
            ConfigSeedCandidate {
                tier: ConfigSeedTier::MatrixBase,
                os_specific: false,
                path: matrix.join("default.claude"),
            },
            ConfigSeedCandidate {
                tier: ConfigSeedTier::WorkspaceProfile,
                os_specific: false,
                path: ac_root.join("default_profile_a.claude"),
            },
            ConfigSeedCandidate {
                tier: ConfigSeedTier::WorkspaceBase,
                os_specific: false,
                path: ac_root.join("default.claude"),
            },
        ]
    );
}

/// Test 3: within one tier, the OS variant beats its base.
#[test]
fn os_variant_wins_within_its_tier() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"BASE");
    write_file(&ac_root.join("default.claude.linux").join("f.txt"), b"OS");
    let ctx = ctx_with(&replica, Some(&ac_root), None);

    let resolved =
        resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), Some("linux")).unwrap();
    let published = assert_published(perform_config_seed(&resolved, "t3"));
    assert_eq!(published.tier, ConfigSeedTier::WorkspaceBase);
    assert_eq!(
        std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
        b"OS"
    );
}

/// Test 4: an OS variant never outranks a base of a HIGHER tier.
#[test]
fn os_never_jumps_a_tier() {
    for (higher, lower_os, expected_tier, expected_bytes) in [
        (
            "default.claude",
            "default_profile_a.claude.linux",
            ConfigSeedTier::MatrixBase,
            b"MX_BASE".as_slice(),
        ),
        (
            "default_profile_a.claude",
            "default.claude.linux",
            ConfigSeedTier::MatrixProfile,
            b"MX_PROFILE".as_slice(),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let ac_root = temp.path().join("ws");
        let replica = ac_root.join("wg-1-team").join("__agent_x");
        let matrix = ac_root.join("_agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&matrix.join(higher).join("f.txt"), expected_bytes);
        let lower_root = if expected_tier == ConfigSeedTier::MatrixBase {
            &ac_root
        } else {
            &matrix
        };
        write_file(&lower_root.join(lower_os).join("f.txt"), b"LOWER_OS");
        let ctx = ctx_with(&replica, Some(&ac_root), Some(&matrix));
        let resolved =
            resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), Some("linux")).unwrap();
        let published = assert_published(perform_config_seed(&resolved, "t4"));
        assert_eq!(published.tier, expected_tier);
        assert_eq!(
            std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
            expected_bytes
        );
    }
}

/// Test 5: with no `.<os>` folder on disk, an OS token changes nothing --
/// same files, same tier, same manifest `source`.
#[test]
fn fallback_is_byte_identical() {
    fn run(os_token: Option<&str>, sfx: &str) -> (ConfigSeedPublication, ManifestSource) {
        let temp = tempfile::tempdir().unwrap();
        let ac_root = temp.path().join("ws");
        let replica = ac_root.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&ac_root.join("default.claude").join("f.txt"), b"BASE");
        let ctx = ctx_with(&replica, Some(&ac_root), None);
        let resolved =
            resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), os_token).unwrap();
        let published = assert_published(perform_config_seed(&resolved, sfx));
        assert_eq!(
            std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
            b"BASE"
        );
        let source = manifest_source_for_tier(published.tier);
        (published, source)
    }
    let (with_os, source_with_os) = run(Some("linux"), "t5a");
    let (without, source_without) = run(None, "t5b");
    assert_eq!(with_os.tier, without.tier);
    assert_eq!(source_with_os, source_without);
    let names = |p: &ConfigSeedPublication| match &p.files {
        CollectedSeedFiles::Exact(v) => v.clone(),
        other => panic!("expected exact files, got {other:?}"),
    };
    assert_eq!(names(&with_os), names(&without));
}

/// Test 6: the OS variant of tier 5 keeps the absent-only + non-empty gate.
#[test]
fn catalog_default_os_variant_keeps_absent_only_and_nonempty() {
    // Build a tier-5-only seed the way the spawn caller does, with both the
    // OS master and the base master, OS first.
    fn fixture(
        temp: &Path,
        os_master_files: &[(&str, &[u8])],
        base_master_files: &[(&str, &[u8])],
        dest_exists: bool,
    ) -> (ResolvedConfigSeed, PathBuf) {
        let ac_root = temp.join("ws");
        let replica = ac_root.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        let os_master = temp.join("masters").join(".claude.linux");
        let base_master = temp.join("masters").join(".claude");
        std::fs::create_dir_all(&os_master).unwrap();
        std::fs::create_dir_all(&base_master).unwrap();
        for (n, c) in os_master_files {
            write_file(&os_master.join(n), c);
        }
        for (n, c) in base_master_files {
            write_file(&base_master.join(n), c);
        }
        if dest_exists {
            write_file(&replica.join(".claude").join("live.txt"), b"LIVE");
        }
        let ctx = ctx_with(&replica, Some(&ac_root), None);
        let mut resolved =
            resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), Some("linux")).unwrap();
        resolved.candidates.push(ConfigSeedCandidate {
            tier: ConfigSeedTier::CatalogDefault,
            os_specific: true,
            path: os_master,
        });
        resolved.candidates.push(ConfigSeedCandidate {
            tier: ConfigSeedTier::CatalogDefault,
            os_specific: false,
            path: base_master,
        });
        (resolved, replica)
    }

    // (a) OS master non-empty + dest absent => the OS master fills.
    let t = tempfile::tempdir().unwrap();
    let (resolved, replica) = fixture(t.path(), &[("f.txt", b"OS")], &[("f.txt", b"BASE")], false);
    let published = assert_published(perform_config_seed(&resolved, "t6a"));
    assert_eq!(published.tier, ConfigSeedTier::CatalogDefault);
    assert_eq!(
        std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
        b"OS"
    );

    // (b) dest present => skipped, the live config is untouched.
    let t = tempfile::tempdir().unwrap();
    let (resolved, replica) = fixture(t.path(), &[("f.txt", b"OS")], &[("f.txt", b"BASE")], true);
    assert_skipped(perform_config_seed(&resolved, "t6b"));
    assert_eq!(
        std::fs::read(replica.join(".claude").join("live.txt")).unwrap(),
        b"LIVE"
    );

    // (c) OS master EMPTY => reads as "not present"; the base master fills.
    let t = tempfile::tempdir().unwrap();
    let (resolved, replica) = fixture(t.path(), &[], &[("f.txt", b"BASE")], false);
    let published = assert_published(perform_config_seed(&resolved, "t6c"));
    assert_eq!(published.tier, ConfigSeedTier::CatalogDefault);
    assert_eq!(
        std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
        b"BASE"
    );
}

/// Test 7: the manifest source depends on the tier ALONE; an OS variant
/// records exactly what its base records, for all five tiers.
#[test]
fn manifest_source_ignores_os_specific() {
    for tier in [
        ConfigSeedTier::WorkspaceProfile,
        ConfigSeedTier::WorkspaceBase,
        ConfigSeedTier::MatrixProfile,
        ConfigSeedTier::MatrixBase,
        ConfigSeedTier::CatalogDefault,
    ] {
        let os_variant = ConfigSeedCandidate {
            tier,
            os_specific: true,
            path: PathBuf::from("os"),
        };
        let base = ConfigSeedCandidate {
            tier,
            os_specific: false,
            path: PathBuf::from("base"),
        };
        assert_eq!(
            manifest_source_for_tier(os_variant.tier),
            manifest_source_for_tier(base.tier),
            "{tier:?}"
        );
    }
}

/// Test 9: the log marker, and the two formatted strings of plan section 3.8.
#[test]
fn os_marker_is_emitted_only_for_os_variants() {
    assert_eq!(os_marker(true), "+os");
    assert_eq!(os_marker(false), "");
    let os_variant = ConfigSeedCandidate {
        tier: ConfigSeedTier::WorkspaceBase,
        os_specific: true,
        path: PathBuf::from("os"),
    };
    let base = ConfigSeedCandidate {
        tier: ConfigSeedTier::WorkspaceBase,
        os_specific: false,
        path: PathBuf::from("base"),
    };
    assert_eq!(
        format!("{:?}{}", os_variant.tier, os_marker(os_variant.os_specific)),
        "WorkspaceBase+os"
    );
    assert_eq!(
        format!("{:?}{}", base.tier, os_marker(base.os_specific)),
        "WorkspaceBase"
    );
}

/// Test 11: the ONLY test that reads the host token. Compiled per target;
/// on the ubuntu runner it can pass only if the `linux` arm of
/// `host_os_token` exists and is taken (plan section 3.10, guard 2b).
#[test]
fn host_token_matches_target_os() {
    #[cfg(target_os = "linux")]
    assert_eq!(host_os_token(), Some("linux"));
    #[cfg(target_os = "windows")]
    assert_eq!(host_os_token(), Some("windows"));
    #[cfg(target_os = "macos")]
    assert_eq!(host_os_token(), Some("macos"));
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    assert_eq!(host_os_token(), None);
}

/// Positive control: selection has NO intrinsic preference for
/// `os_specific`. With the pair inverted inside one tier and both folders on
/// disk, the BASE wins -- so precedence is a property of the vector built in
/// plan section 3.4 and nothing else.
#[test]
fn selection_is_order_sensitive() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    write_file(&ac_root.join("default.claude").join("f.txt"), b"BASE");
    write_file(&ac_root.join("default.claude.linux").join("f.txt"), b"OS");
    let ctx = ctx_with(&replica, Some(&ac_root), None);

    let mut resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    // Hand-built, INVERTED pair: base first, OS variant second.
    resolved.candidates = vec![
        ConfigSeedCandidate {
            tier: ConfigSeedTier::WorkspaceBase,
            os_specific: false,
            path: ac_root.join("default.claude"),
        },
        ConfigSeedCandidate {
            tier: ConfigSeedTier::WorkspaceBase,
            os_specific: true,
            path: ac_root.join("default.claude.linux"),
        },
    ];
    assert_published(perform_config_seed(&resolved, "pc"));
    assert_eq!(
        std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
        b"BASE"
    );
}

// (release-mode measurement; registered in test-debt.allowlist.json).
#[test]
#[ignore = "release-mode end-to-end config staging benchmark (plan 7.4/10.5 item 7)"]
fn bench_end_to_end_config_staging_1k_10k_100k() {
    for &n in &[1_000usize, 10_000, 100_000] {
        let temp = tempfile::tempdir().unwrap();
        let ac_root = temp.path().join("ws");
        let replica = ac_root.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        let source = ac_root.join("default.claude");
        std::fs::create_dir_all(&source).unwrap();
        for i in 0..n {
            std::fs::write(source.join(format!("file-{i:07}.txt")), b"seed").unwrap();
        }
        let ctx = ctx_with(&replica, Some(&ac_root), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();

        let started = std::time::Instant::now();
        let publication = assert_published(perform_config_seed(&resolved, "sfx"));
        let elapsed = started.elapsed();

        let count = match &publication.files {
            CollectedSeedFiles::Exact(files) => files.len(),
            other => panic!("expected exact collected files, got {other:?}"),
        };
        assert_eq!(
            count, n,
            "config staging must collect exactly the installed regular files"
        );
        eprintln!("[stage-e bench config-staging] files={n} elapsed={elapsed:?}");
    }
}
