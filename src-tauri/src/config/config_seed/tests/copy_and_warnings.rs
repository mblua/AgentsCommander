use super::*;

// ---- copy_file_substituted (size-first, binary/UTF-8) ------------------

#[test]
fn copy_file_substitutes_utf8_tokens_forward_slashed() {
    let temp = tempfile::tempdir().unwrap();
    let replica = abs(r"C:\replica\root", "/replica/root");
    let ctx = ctx_with(&replica, Some(&abs(r"C:\ws", "/ws")), None);

    let src = temp.path().join("a.txt");
    std::fs::write(&src, b"path=%AC_REPLICA_ROOT%/x").unwrap();
    let dst = temp.path().join("a.out");
    copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();

    let expected = replica.to_string_lossy().replace('\\', "/");
    assert_eq!(
        std::fs::read_to_string(&dst).unwrap(),
        format!("path={}/x", expected)
    );
}

#[test]
fn copy_file_preserves_binary_verbatim() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

    let src = temp.path().join("b.bin");
    let raw: &[u8] = b"%AC_REPLICA_ROOT%\x00binary";
    std::fs::write(&src, raw).unwrap();
    let dst = temp.path().join("b.out");
    copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();
    // NUL present -> verbatim, no token substitution.
    assert_eq!(std::fs::read(&dst).unwrap(), raw);
}

#[test]
fn copy_file_preserves_non_utf8_without_nul_verbatim() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

    let src = temp.path().join("c.dat");
    let raw: &[u8] = &[0xFF, 0xFE, b'h', b'i'];
    std::fs::write(&src, raw).unwrap();
    let dst = temp.path().join("c.out");
    copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), raw);
}

#[test]
fn copy_file_over_cap_is_streamed_without_substitution() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

    let src = temp.path().join("big.txt");
    let mut content = vec![b'a'; (CONTENT_SUBSTITUTION_CAP as usize) + 16];
    content[..18].copy_from_slice(b"%AC_REPLICA_ROOT%\n");
    std::fs::write(&src, &content).unwrap();
    let dst = temp.path().join("big.out");
    copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();

    let out = std::fs::read(&dst).unwrap();
    assert_eq!(out.len(), content.len());
    // Streamed verbatim: token NOT substituted.
    assert!(out.starts_with(b"%AC_REPLICA_ROOT%"));
}

// ---- %USER_HOME% in seeded content (#924) ------------------------------

/// `dirs::home_dir()` forward-slashed. `None` on a machine without a home,
/// where the token is expected to stay literal.
fn home_fwd() -> Option<String> {
    dirs::home_dir().map(|home| home.to_string_lossy().replace('\\', "/"))
}

#[test]
fn copy_file_substitutes_user_home() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

    let src = temp.path().join("s.json");
    std::fs::write(&src, b"excl=%USER_HOME%/.claude/CLAUDE.md").unwrap();
    let dst = temp.path().join("s.out");
    copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();

    let out = std::fs::read_to_string(&dst).unwrap();
    match home_fwd() {
        Some(home) => assert_eq!(out, format!("excl={}/.claude/CLAUDE.md", home)),
        None => assert_eq!(out, "excl=%USER_HOME%/.claude/CLAUDE.md"),
    }
}

#[test]
fn copy_file_over_cap_does_not_substitute_user_home() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

    let src = temp.path().join("big.json");
    let mut content = vec![b'a'; (CONTENT_SUBSTITUTION_CAP as usize) + 16];
    content[..12].copy_from_slice(b"%USER_HOME%\n");
    std::fs::write(&src, &content).unwrap();
    let dst = temp.path().join("big.out");
    copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();

    let out = std::fs::read(&dst).unwrap();
    assert_eq!(out.len(), content.len());
    // Over the cap: streamed verbatim, the token is NOT substituted.
    assert!(out.starts_with(b"%USER_HOME%"));
}

#[test]
fn copy_file_binary_with_user_home_bytes_is_verbatim() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

    let src = temp.path().join("b.bin");
    let raw: &[u8] = b"%USER_HOME%\x00binary";
    std::fs::write(&src, raw).unwrap();
    let dst = temp.path().join("b.out");
    copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();
    // NUL in the leading window -> verbatim, no substitution.
    assert_eq!(std::fs::read(&dst).unwrap(), raw);
}

#[test]
fn copy_tree_verbatim_preserves_user_home_when_substitute_false() {
    // embedded -> master copies raw templates: the token must survive so the
    // later replica seed can expand it.
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    write_file(&src.join("a.json"), b"excl=%USER_HOME%/.claude/CLAUDE.md");
    let dst = temp.path().join("dst");
    copy_tree(&src, &dst, 0, None, false).unwrap();
    assert_eq!(
        std::fs::read(dst.join("a.json")).unwrap(),
        b"excl=%USER_HOME%/.claude/CLAUDE.md"
    );
}

#[test]
fn seed_settings_local_expands_user_home_but_not_hook_markers() {
    // Realistic settings.local.json: `claudeMdExcludes` uses %USER_HOME% (must
    // expand), while a PreToolUse hook command uses %TEMP% (must NOT).
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("ws");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    let template = concat!(
        "{\n",
        "  \"claudeMdExcludes\": [\"%USER_HOME%/.claude/CLAUDE.md\"],\n",
        "  \"hooks\": { \"PreToolUse\": [{ \"command\": \"echo %TEMP%/log.txt\" }] }\n",
        "}\n"
    );
    write_file(
        &ac_root.join("default.claude").join("settings.local.json"),
        template.as_bytes(),
    );

    let ctx = ctx_with(&replica, Some(&ac_root), None);
    let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx), None).unwrap();
    assert_published(perform_config_seed(&resolved, "sfx924"));

    let out = std::fs::read_to_string(replica.join(".claude").join("settings.local.json")).unwrap();
    match home_fwd() {
        Some(home) => {
            assert!(
                out.contains(&format!("\"{}/.claude/CLAUDE.md\"", home)),
                "{out}"
            );
            assert!(!out.contains("%USER_HOME%"), "{out}");
        }
        // No home: the seed still succeeds, the token is written literally.
        None => assert!(out.contains("\"%USER_HOME%/.claude/CLAUDE.md\""), "{out}"),
    }
    // The unknown marker inside the hook command survives verbatim.
    assert!(out.contains("echo %TEMP%/log.txt"), "{out}");
}

// ---- copy_tree (reparse skip + depth bound) ----------------------------

#[test]
fn copy_tree_truncates_beyond_max_depth() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    write_file(&src.join("f.txt"), b"x");
    let dst = temp.path().join("dst");
    let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);
    // Start already over the bound -> immediate Ok, nothing created.
    copy_tree(&src, &dst, MAX_SEED_DEPTH + 1, Some(&ctx), true).unwrap();
    assert!(
        !dst.exists(),
        "over-depth call must not create the dst tree"
    );
}

#[test]
fn copy_tree_skips_symlink_and_junction_entries() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_file(&src.join("real.txt"), b"real");
    let target = temp.path().join("target");
    write_file(&target.join("inside.txt"), b"x");

    let link = src.join("linked");
    let created;
    #[cfg(unix)]
    {
        created = std::os::unix::fs::symlink(&target, &link).is_ok();
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("cmd");
        crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
        let status = cmd
            .args([
                "/C",
                "mklink",
                "/J",
                link.to_str().unwrap(),
                target.to_str().unwrap(),
            ])
            .status();
        created = matches!(status, Ok(s) if s.success());
    }
    if !created {
        return; // environment cannot create links — skip gracefully.
    }

    let dst = temp.path().join("dst");
    let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);
    copy_tree(&src, &dst, 0, Some(&ctx), true).unwrap();
    assert!(dst.join("real.txt").exists(), "real file must be copied");
    assert!(
        !dst.join("linked").exists(),
        "symlink/junction entry must be skipped"
    );
}

// ---- compute_config_dir_warning (two cases) ----------------------------

#[test]
fn warning_mismatch_when_config_dir_env_differs() {
    let dest = abs(r"C:\replica\.claude", "/replica/.claude");
    let mut profile_env = BTreeMap::new();
    profile_env.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        abs(r"C:\replica\.claude-amp", "/replica/.claude-amp")
            .to_string_lossy()
            .to_string(),
    );
    let w = compute_config_dir_warning(&dest, "claude", &[], &BTreeMap::new(), &profile_env, None)
        .expect("warning");
    assert!(w.contains("does not match"), "{w}");
}

#[test]
fn warning_overwrite_when_dest_equals_active_config_dir() {
    let dest = abs(r"C:\replica\.claude", "/replica/.claude");
    let mut env = BTreeMap::new();
    // Different parent, same final segment -> "equal active dir" case.
    env.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        abs(r"C:\other\.claude", "/other/.claude")
            .to_string_lossy()
            .to_string(),
    );
    let w = compute_config_dir_warning(&dest, "claude", &[], &BTreeMap::new(), &env, None)
        .expect("warning");
    assert!(w.contains("active") && w.contains("overwritten"), "{w}");
}

#[test]
fn warning_unset_when_no_config_dir_env() {
    let dest = abs(r"C:\replica\.claude", "/replica/.claude");
    let w = compute_config_dir_warning(
        &dest,
        "claude",
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
    )
    .expect("warning");
    assert!(w.contains("not configured"), "{w}");
}

#[test]
fn warning_uses_effective_codex_home_for_codex() {
    let dest = abs(r"C:\replica\.codex", "/replica/.codex");
    let eff = abs(r"C:\replica\.codex", "/replica/.codex");
    let w = compute_config_dir_warning(
        &dest,
        "codex",
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&eff),
    )
    .expect("warning");
    assert!(w.contains("active"), "{w}");
}

/// #1873 - Muse has no AC-managed config-dir warning and ships no built-in
/// seed, factory master, or credential flow; a user-configured `configSeed`
/// still resolves provider-neutrally through the same path every agent uses.
#[test]
fn muse_has_no_config_dir_warning_or_seed_contract() {
    let dest = abs(r"C:\replica\.muse", "/replica/.muse");
    let mut env = BTreeMap::new();
    env.insert("MUSE_HOME".to_string(), "/elsewhere/.muse".to_string());
    env.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        "/elsewhere/.claude".to_string(),
    );
    env.insert("CODEX_HOME".to_string(), "/elsewhere/.codex".to_string());
    for (shell, args) in [
        ("muse", Vec::<String>::new()),
        (
            "/opt/muse/bin/muse",
            vec!["resume".to_string(), "--last".to_string()],
        ),
        (
            "muse",
            vec!["--workspace".to_string(), "/tmp/codex".to_string()],
        ),
    ] {
        assert_eq!(
            CodingAgentKind::detect(shell, &args),
            Some(CodingAgentKind::Muse)
        );
        assert!(
            compute_config_dir_warning(&dest, shell, &args, &env, &env, None).is_none(),
            "shell={shell:?} args={args:?}"
        );
    }

    // No built-in seed/factory master and no automatic credential flow.
    let catalog = crate::config::coding_agents_catalog::embedded_default_catalog();
    let muse = catalog
        .agents
        .iter()
        .find(|a| a.key == "muse")
        .expect("#1860 catalog ships the Muse built-in");
    assert!(
        muse.config_seed.is_none(),
        "Muse must ship configSeed UNSET"
    );
    assert!(
        !crate::config::coding_agents_catalog::reseedable_command_basenames()
            .iter()
            .any(|b| b == "muse"),
        "no embedded seed master for Muse"
    );
    assert!(CodingAgentKind::Muse
        .profile()
        .container_credential
        .is_none());

    // A user-configured active seed resolves provider-neutrally: same
    // candidates/dest as any other agent, and no Muse-specific warning.
    let replica = abs(r"C:\proj\.ac\wg-1\__agent_x", "/proj/.ac/wg-1/__agent_x");
    let ac_root = abs(r"C:\proj\.ac", "/proj/.ac");
    let matrix = abs(r"C:\proj\.ac\_agent_x", "/proj/.ac/_agent_x");
    let ctx = ctx_with(&replica, Some(&ac_root), Some(&matrix));
    let resolved = resolve_config_seed(&seed_cfg(".muse"), "A", Some(&ctx), None)
        .expect("active user seed resolves");
    assert_eq!(resolved.dest, replica.join(".muse"));
    assert_eq!(
        resolved
            .candidates
            .iter()
            .map(|c| c.tier)
            .collect::<Vec<_>>(),
        vec![
            ConfigSeedTier::MatrixProfile,
            ConfigSeedTier::MatrixBase,
            ConfigSeedTier::WorkspaceProfile,
            ConfigSeedTier::WorkspaceBase,
        ]
    );
    assert_eq!(
        resolved.candidates[0].path,
        matrix.join("default_profile_a.muse")
    );
    assert!(resolved.config_dir_warning.is_none());
    // Identical resolution for a non-Muse agent proves provider neutrality.
    let other = resolve_config_seed(&seed_cfg(".muse"), "A", Some(&ctx), None).unwrap();
    assert_eq!(other.dest, resolved.dest);
    assert_eq!(other.candidates, resolved.candidates);
}

#[test]
fn warning_none_for_non_config_dir_agent() {
    let dest = abs(r"C:\replica\.antigravity", "/replica/.antigravity");
    // Antigravity and Pi have no managed config-dir env; a custom command also yields None.
    for (shell, args) in [
        ("agy", Vec::new()),
        (
            "pi",
            vec!["--model".to_string(), "claude-sonnet".to_string()],
        ),
    ] {
        assert!(
            compute_config_dir_warning(
                &dest,
                shell,
                &args,
                &BTreeMap::new(),
                &BTreeMap::new(),
                None,
            )
            .is_none(),
            "shell={shell:?}"
        );
    }
    assert!(compute_config_dir_warning(
        &dest,
        "my-custom-agent",
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None
    )
    .is_none());
}
