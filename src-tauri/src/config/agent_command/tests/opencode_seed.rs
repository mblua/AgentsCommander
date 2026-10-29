use super::*;

#[test]
fn command_runs_opencode_matches_basename_and_wrapper() {
    assert!(command_runs_opencode("opencode", &[]));
    assert!(command_runs_opencode("opencode.exe", &[]));
    // cmd /c opencode wrapper: opencode appears as an arg token.
    assert!(command_runs_opencode(
        "cmd.exe",
        &["/c".to_string(), "opencode".to_string()]
    ));
    // Allowlisted executable forms still match as args (npm-style shim).
    assert!(command_runs_opencode(
        "cmd.exe",
        &["/c opencode.bat".to_string()]
    ));
    // Non-opencode commands do not match.
    assert!(!command_runs_opencode("codex", &[]));
    assert!(!command_runs_opencode("claude", &["--resume".to_string()]));
}

// #2561 - `\` is a path separator only on Windows; on POSIX `file_stem` and
// `file_name` keep the whole `C:\tools\...` string, so these run there only.
#[cfg(windows)]
#[test]
fn command_runs_opencode_matches_windows_paths() {
    assert!(command_runs_opencode(
        r"C:\tools\opencode.exe",
        &["--flag".to_string()]
    ));
    // Allowlisted executable forms still match as args (path form).
    assert!(command_runs_opencode(
        "cmd.exe",
        &["/c".to_string(), r"C:\tools\opencode.cmd".to_string()]
    ));
}

#[test]
fn command_runs_opencode_ignores_opencode_doc_and_config_args() {
    // Regression (grinch Finding 1): a non-opencode agent whose command merely
    // references an `opencode.md`/`.json` file must NOT be read as an opencode
    // launch. `file_stem("opencode.md") == "opencode"`, so the prior arg-scan
    // false-matched; the file_name allowlist rejects these forms.
    assert!(!command_runs_opencode(
        "claude",
        &["--instructions".to_string(), "opencode.md".to_string()]
    ));
    assert!(!command_runs_opencode(
        "claude",
        &[r"C:\Users\me\opencode.md".to_string()]
    ));
    assert!(!command_runs_opencode(
        "claude",
        &["opencode.json".to_string()]
    ));
    assert!(!command_runs_opencode(
        "claude",
        &["opencode.txt".to_string()]
    ));
}

#[test]
fn find_opencode_config_dir_prefers_profile_over_agent() {
    let agent_env = opencode_env("agent-value");
    let profile_env = opencode_env("profile-value");
    assert_eq!(
        find_opencode_config_dir(&agent_env, &profile_env).map(String::as_str),
        Some("profile-value")
    );
    // Falls back to the agent layer when the profile lacks the key.
    let empty = BTreeMap::new();
    assert_eq!(
        find_opencode_config_dir(&agent_env, &empty).map(String::as_str),
        Some("agent-value")
    );
    // None when neither layer sets it.
    assert_eq!(find_opencode_config_dir(&empty, &empty), None);
}

#[test]
fn ensure_opencode_config_dir_creates_missing_absolute_dir() {
    let temp = tempfile::tempdir().unwrap();
    // Nested + missing, to prove create_dir_all builds the whole chain.
    let target = temp.path().join("nested").join(".opencode");
    assert!(!target.exists());
    let profile_env = opencode_env(&target.to_string_lossy());

    let outcome = ensure_opencode_config_dir("opencode", &[], &BTreeMap::new(), &profile_env);

    assert_eq!(outcome, OpencodeConfigDirOutcome::Ensured);
    assert!(target.is_dir(), "config dir should have been created");
}

#[test]
fn ensure_opencode_config_dir_existing_dir_is_noop_success() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join(".opencode");
    std::fs::create_dir_all(&target).unwrap();
    let agent_env = opencode_env(&target.to_string_lossy());

    let outcome = ensure_opencode_config_dir("opencode", &[], &agent_env, &BTreeMap::new());

    assert_eq!(outcome, OpencodeConfigDirOutcome::Ensured);
    assert!(target.is_dir());
}

#[test]
fn ensure_opencode_config_dir_skips_relative_value() {
    // Not an absolute path -> never created (cannot trust where it would land).
    let profile_env = opencode_env(r"relative\.opencode");
    let outcome = ensure_opencode_config_dir("opencode", &[], &BTreeMap::new(), &profile_env);
    assert_eq!(outcome, OpencodeConfigDirOutcome::Skipped);
}

#[test]
fn ensure_opencode_config_dir_skips_non_opencode_command() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join(".opencode");
    let profile_env = opencode_env(&target.to_string_lossy());

    let outcome = ensure_opencode_config_dir("codex", &[], &BTreeMap::new(), &profile_env);

    assert_eq!(outcome, OpencodeConfigDirOutcome::Skipped);
    assert!(
        !target.exists(),
        "must not create the dir for a non-opencode command"
    );
}

#[test]
fn ensure_opencode_config_dir_skips_opencode_md_arg_with_absolute_value() {
    // Regression (grinch Finding 1): a non-opencode launch (here `claude`)
    // whose args reference an `opencode.md` file, even with an absolute
    // OPENCODE_CONFIG_DIR set, must NOT be read as an opencode launch, so the
    // dir is never created.
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("nested").join(".opencode");
    assert!(!target.exists());
    let profile_env = opencode_env(&target.to_string_lossy());

    let outcome = ensure_opencode_config_dir(
        "claude",
        &["--instructions".to_string(), "opencode.md".to_string()],
        &BTreeMap::new(),
        &profile_env,
    );

    assert_eq!(outcome, OpencodeConfigDirOutcome::Skipped);
    assert!(
        !target.exists(),
        "an opencode.md arg must not trigger OPENCODE_CONFIG_DIR creation"
    );
}

#[test]
fn ensure_opencode_config_dir_skips_when_unset() {
    let outcome = ensure_opencode_config_dir("opencode", &[], &BTreeMap::new(), &BTreeMap::new());
    assert_eq!(outcome, OpencodeConfigDirOutcome::Skipped);
}

#[test]
fn ensure_opencode_config_dir_warns_and_continues_on_create_failure() {
    // Make an ancestor a regular file so create_dir_all cannot succeed.
    let temp = tempfile::tempdir().unwrap();
    let blocker = temp.path().join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();
    let target = blocker.join(".opencode");
    let profile_env = opencode_env(&target.to_string_lossy());

    let outcome = ensure_opencode_config_dir("opencode", &[], &BTreeMap::new(), &profile_env);

    assert_eq!(outcome, OpencodeConfigDirOutcome::Failed);
    assert!(!target.exists());
}

#[test]
fn resolve_is_read_only_and_prepare_is_idempotent_for_opencode() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("resolved").join(".opencode");
    let mut settings = AppSettings {
        agents: vec![agent("opencode", "opencode")],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("opencode".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: opencode_env(&target.to_string_lossy()),
                notes: String::new(),
            },
        );

    let spawn = resolve_agent_spawn_command(&settings, "opencode", None, Some("A"), false).unwrap();
    assert!(
        !target.exists(),
        "read-only resolution must not create the directory"
    );

    prepare_agent_spawn_command(&spawn).unwrap();
    assert!(target.is_dir());
    prepare_agent_spawn_command(&spawn).unwrap();
    assert!(target.is_dir(), "preparation must be idempotent");
}

#[test]
fn prepare_codex_home_failure_short_circuits_opencode_directory_creation() {
    let temp = tempfile::tempdir().unwrap();
    let blocker = temp.path().join("not-a-directory");
    std::fs::write(&blocker, b"x").unwrap();
    let home = blocker.join("home");
    let parent = temp.path().join("oc");
    std::fs::create_dir_all(&parent).unwrap();
    let target = parent.join(".opencode");
    assert!(parent.is_dir());
    assert!(!target.exists());
    let settings = AppSettings {
        agents: vec![agent("opencode", "opencode")],
        ..AppSettings::default()
    };
    let mut spawn =
        resolve_agent_spawn_command(&settings, "opencode", None, Some("A"), false).unwrap();
    spawn.isolated_codex_home_to_prepare = Some(home.clone());
    spawn.profile_env = opencode_env(&target.to_string_lossy());
    let expected = std::fs::create_dir_all(&home).unwrap_err();

    let error = prepare_agent_spawn_command(&spawn).unwrap_err();

    assert_eq!(
        error,
        format!(
            "Failed to create isolated CODEX_HOME '{}': {}",
            home.display(),
            expected
        )
    );
    assert!(!target.exists());
}

#[test]
fn resolve_is_read_only_and_public_build_prepares_isolated_codex_home() {
    let id = format!("codex-resolve-{}", uuid::Uuid::new_v4());
    let mut codex = agent(&id, "codex");
    codex.isolated_home = true;
    let settings = AppSettings {
        agents: vec![codex],
        ..AppSettings::default()
    };
    let expected = crate::config::config_dir()
        .unwrap()
        .join(crate::config::instance_artifacts::CODEX_HOME_DIR_NAME)
        .join(super::sanitize_codex_home_id(&id));
    let _ = std::fs::remove_dir_all(&expected);

    let resolved = resolve_agent_spawn_command(&settings, &id, None, Some("A"), false).unwrap();
    assert_eq!(
        resolved.effective_codex_home.as_deref(),
        Some(expected.as_path())
    );
    assert!(
        !expected.exists(),
        "read-only resolution must not create CODEX_HOME"
    );

    prepare_agent_spawn_command(&resolved).unwrap();
    assert!(expected.is_dir());
    prepare_agent_spawn_command(&resolved).unwrap();
    assert!(expected.is_dir(), "preparation must be idempotent");
    std::fs::remove_dir_all(&expected).unwrap();

    let built = build_agent_spawn_command(&settings, &id, None, Some("A")).unwrap();
    assert_eq!(
        built.effective_codex_home.as_deref(),
        Some(expected.as_path())
    );
    assert!(
        expected.is_dir(),
        "public build must retain preparation behavior"
    );
    std::fs::remove_dir_all(expected).unwrap();
}

#[test]
fn build_spawn_creates_opencode_config_dir_for_literal_absolute_value() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("nested").join(".opencode");
    assert!(!target.exists());

    let mut settings = AppSettings {
        agents: vec![agent("opencode", "opencode")],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("opencode".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: opencode_env(&target.to_string_lossy()),
                notes: String::new(),
            },
        );

    // launch_path None is fine: a literal absolute value needs no placeholder context.
    let spawn = build_agent_spawn_command(&settings, "opencode", None, Some("A")).unwrap();

    assert!(
        target.is_dir(),
        "build should have created OPENCODE_CONFIG_DIR"
    );
    let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();
    assert_eq!(
        env.get("OPENCODE_CONFIG_DIR").map(String::as_str),
        Some(target.to_string_lossy().as_ref())
    );
}

#[test]
fn build_spawn_expands_and_creates_opencode_config_dir_for_replica_placeholder() {
    // Finding 2 (grinch): the literal-value test does not cover the real user
    // scenario, where OPENCODE_CONFIG_DIR holds %AC_REPLICA_ROOT%\.opencode and
    // is expanded against the replica launch root before the dir is created.
    // This drives that path end-to-end: expand -> absolute -> create.
    let temp = tempfile::tempdir().unwrap();
    let replica = temp
        .path()
        .join("root with spaces")
        .join(".ac")
        .join("wg-7-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();

    let mut settings = AppSettings {
        agents: vec![agent("opencode", "opencode")],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("opencode".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: opencode_env(r"%AC_REPLICA_ROOT%\.opencode"),
                notes: String::new(),
            },
        );

    let spawn = build_agent_spawn_command(&settings, "opencode", Some(&replica), Some("A"))
        .expect("replica launch should expand %AC_REPLICA_ROOT% and build the spawn");

    // Mirror the builder's canonicalize + verbatim-prefix strip, then append the
    // literal "\.opencode" the way placeholder expansion concatenates it (see the
    // matrix CODEX_HOME test for why the whole-path comparison stays cross-platform).
    let canonical_root = std::fs::canonicalize(&replica).unwrap();
    let canonical_text = canonical_root.to_string_lossy();
    let expected_replica = canonical_text
        .strip_prefix(r"\\?\")
        .map(std::path::PathBuf::from)
        .unwrap_or(canonical_root);
    let expected_config = format!("{}\\.opencode", expected_replica.to_string_lossy());

    // The child env carries the fully-expanded absolute path...
    let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();
    assert_eq!(
        env.get("OPENCODE_CONFIG_DIR").map(String::as_str),
        Some(expected_config.as_str())
    );
    // ...and the build created that exact directory end-to-end.
    assert!(
        std::path::Path::new(&expected_config).is_dir(),
        "build should have created the expanded OPENCODE_CONFIG_DIR at {expected_config}"
    );
}

// #598 - config-folder seed resolution wired into build_agent_spawn_command.

#[test]
fn build_spawn_resolves_active_seed_for_replica_without_touching_fs() {
    let temp = tempfile::tempdir().unwrap();
    let replica = seed_replica(temp.path());

    let mut claude = agent("claude", "claude");
    claude.config_seed = Some(ConfigSeedConfig {
        enabled: true,
        dest: ".claude".to_string(),
    });
    let settings = AppSettings {
        agents: vec![claude],
        ..AppSettings::default()
    };

    let spawn = build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
    let seed = spawn.seed.expect("active seed resolves to Some");

    let expected_replica = canonical_replica(&replica);
    let ac_root = expected_replica.parent().unwrap().parent().unwrap();
    let matrix = ac_root.join("_agent_dev-rust");
    let letter = spawn
        .profile_resolution
        .effective_profile
        .to_ascii_lowercase();

    use crate::config::config_seed::ConfigSeedTier;
    // Matrix tiers outrank workspace tiers; profile beats base in each. The
    // matrix's bare `.claude` (the agent's own live config) is never a source.
    // #769 P2 + #1318 appends the absent-only CatalogDefault master LAST
    // (still pure path math; perform_config_seed gates it on absent-dest +
    // non-empty), resolved from the SESSION WORKSPACE's
    // `.ac/coding-agents/_seed/<dest>`.
    // #2162: each tier is preceded by its `.<os>` variant when the host has
    // a token, so the list is 10 entries on linux/windows/macos and 5 on any
    // other target. Built here from the same host token the code uses, so
    // the assertion is exact on every supported target.
    use crate::config::config_seed::{host_os_token, ConfigSeedCandidate};
    let os = host_os_token();
    let seed_master = ac_root.join("coding-agents").join("_seed");
    let mut expected: Vec<ConfigSeedCandidate> = Vec::new();
    let mut tier_pair = |tier: ConfigSeedTier, root: &std::path::Path, folder: String| {
        if let Some(os) = os {
            expected.push(ConfigSeedCandidate {
                tier,
                os_specific: true,
                path: root.join(format!("{}.{}", folder, os)),
            });
        }
        expected.push(ConfigSeedCandidate {
            tier,
            os_specific: false,
            path: root.join(folder),
        });
    };
    tier_pair(
        ConfigSeedTier::MatrixProfile,
        &matrix,
        format!("default_profile_{}.claude", letter),
    );
    tier_pair(
        ConfigSeedTier::MatrixBase,
        &matrix,
        "default.claude".to_string(),
    );
    tier_pair(
        ConfigSeedTier::WorkspaceProfile,
        ac_root,
        format!("default_profile_{}.claude", letter),
    );
    tier_pair(
        ConfigSeedTier::WorkspaceBase,
        ac_root,
        "default.claude".to_string(),
    );
    tier_pair(
        ConfigSeedTier::CatalogDefault,
        &seed_master,
        ".claude".to_string(),
    );
    assert_eq!(seed.candidates.len(), if os.is_some() { 10 } else { 5 });
    assert_eq!(seed.candidates, expected);
    assert_eq!(seed.dest, expected_replica.join(".claude"));
    // Pure resolution: no template dirs were created.
    assert!(!ac_root.join("default.claude").exists());
}

#[test]
fn build_spawn_catalog_default_absent_without_ac_root() {
    // A replica-shaped launch root with NO `.ac` workspace ancestor (the
    // only shape that could reach the fill's config-dir legacy fallback).
    // `resolve_config_seed` derives its candidates exclusively from the
    // workspace and matrix roots, both of which require a workspace, so the
    // seed does not resolve at all and no CatalogDefault tier is produced:
    // the `.or_else(config_dir)` legacy fallback in the fill is structurally
    // unreachable for a resolved seed and stays defense-in-depth for
    // pre-migration shapes.
    let temp = tempfile::tempdir().unwrap();
    let replica = temp.path().join("wg-1-dev-team").join("__agent_legacy");
    std::fs::create_dir_all(&replica).unwrap();

    let mut claude = agent("claude", "claude");
    claude.config_seed = Some(ConfigSeedConfig {
        enabled: true,
        dest: ".claude".to_string(),
    });
    let settings = AppSettings {
        agents: vec![claude],
        ..AppSettings::default()
    };

    let spawn = build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
    assert!(
        spawn.seed.is_none(),
        "a workspace-less replica resolves no seed and no CatalogDefault tier"
    );
}

#[test]
fn build_spawn_computes_seed_config_dir_warning() {
    // A Claude agent with no CLAUDE_CONFIG_DIR gets the "not configured"
    // warning, computed at build time from the real launch command.
    let temp = tempfile::tempdir().unwrap();
    let replica = seed_replica(temp.path());

    let mut claude = agent("claude", "claude");
    claude.config_seed = Some(ConfigSeedConfig {
        enabled: true,
        dest: ".claude".to_string(),
    });
    let settings = AppSettings {
        agents: vec![claude],
        ..AppSettings::default()
    };

    let spawn = build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
    let warning = spawn
        .seed
        .expect("seed")
        .config_dir_warning
        .expect("warning computed for the launch command");
    assert!(warning.contains("CLAUDE_CONFIG_DIR"), "{warning}");
}

#[test]
fn build_spawn_seed_none_for_normal_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo-thing");
    std::fs::create_dir_all(&repo).unwrap();

    let mut claude = agent("claude", "claude");
    claude.config_seed = Some(ConfigSeedConfig {
        enabled: true,
        dest: ".claude".to_string(),
    });
    let settings = AppSettings {
        agents: vec![claude],
        ..AppSettings::default()
    };

    let spawn = build_agent_spawn_command(&settings, "claude", Some(&repo), Some("A")).unwrap();
    assert!(
        spawn.seed.is_none(),
        "a normal (non-replica) cwd must not seed"
    );
}

#[test]
fn build_spawn_seed_reapply_none_for_non_claude_dest() {
    let temp = tempfile::tempdir().unwrap();
    let replica = seed_replica(temp.path());

    let mut claude = agent("claude", "claude");
    claude.config_seed = Some(ConfigSeedConfig {
        enabled: true,
        dest: ".claude-amp".to_string(),
    });
    let settings = AppSettings {
        agents: vec![claude],
        ..AppSettings::default()
    };

    let spawn = build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
    let seed = spawn.seed.expect("seed");
    assert_eq!(seed.dest.file_name().unwrap(), ".claude-amp");
}

#[test]
fn build_spawn_seed_none_when_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let replica = seed_replica(temp.path());

    let mut claude = agent("claude", "claude");
    claude.config_seed = Some(ConfigSeedConfig {
        enabled: false,
        dest: ".claude".to_string(),
    });
    let settings = AppSettings {
        agents: vec![claude],
        ..AppSettings::default()
    };

    let spawn = build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
    assert!(spawn.seed.is_none(), "disabled seed must resolve to None");
}

// #592 - profile content-hash (drift detector) tests.
