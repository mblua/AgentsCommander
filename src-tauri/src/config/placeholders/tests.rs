use super::*;

#[test]
fn non_path_values_allow_dollar_markers() {
    reject_unexpanded_markers("s3cr$tP4ss ${NAME} $NAME", "env", false).unwrap();
}

#[test]
#[cfg(windows)]
fn strip_extended_prefix_converts_verbatim_unc() {
    assert_eq!(
        strip_extended_prefix(PathBuf::from(r"\\?\UNC\server\share\repo")),
        PathBuf::from(r"\\server\share\repo")
    );
}

#[test]
#[cfg(windows)]
fn same_path_text_matches_ordinary_and_verbatim_unc() {
    assert!(same_path_text(
        Path::new(r"\\server\share\repo"),
        Path::new(r"\\?\UNC\server\share\repo")
    ));
}

#[test]
fn non_path_values_reject_percent_markers() {
    let err = reject_unexpanded_markers("%UNKNOWN%", "env", false).unwrap_err();
    assert!(err.contains("unknown placeholder"), "{err}");
}

#[test]
fn strict_path_values_reject_any_marker() {
    let err = reject_unexpanded_markers("C:/x/$HOME", "CODEX_HOME", true).unwrap_err();
    assert!(err.contains("variable markers"), "{err}");
}

/// Canonicalize + strip the verbatim prefix exactly like
/// `placeholder_context_for_launch_root` does, so expected paths match the
/// context's `replica_root` even when the OS hands back an 8.3 short form.
fn canonical_stripped(path: &Path) -> PathBuf {
    strip_extended_prefix(std::fs::canonicalize(path).unwrap())
}

/// Create a real WG-replica launch root `<temp>/.ac/wg-7-dev-team/__agent_dev-rust`.
fn make_replica(temp: &Path) -> PathBuf {
    let replica = temp
        .join(".ac")
        .join("wg-7-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();
    replica
}

#[test]
fn replica_launch_expands_all_three_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let replica = make_replica(temp.path());
    let ctx = placeholder_context_for_launch_root(&replica).unwrap();

    let expected_replica = canonical_stripped(&replica);
    let expected_ac_root = expected_replica
        .parent()
        .and_then(|parent| parent.parent())
        .expect("replica has a .ac workspace ancestor");
    let expected_matrix = expected_ac_root.join("_agent_dev-rust");

    assert_eq!(ctx.root_kind, PlaceholderRootKind::AcReplicaOrRootAgent);
    assert_eq!(ctx.replica_root, expected_replica);
    assert_eq!(ctx.ac_root.as_deref(), Some(expected_ac_root));
    assert_eq!(ctx.matrix_root.as_deref(), Some(expected_matrix.as_path()));

    assert_eq!(
        expand_placeholders(r"%AC_REPLICA_ROOT%\x", &ctx).unwrap(),
        format!(r"{}\x", expected_replica.to_string_lossy())
    );
    assert_eq!(
        expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &ctx).unwrap(),
        format!(r"{}\x", expected_ac_root.to_string_lossy())
    );
    assert_eq!(
        expand_placeholders(r"%AC_MATRIX_ROOT%\x", &ctx).unwrap(),
        format!(r"{}\x", expected_matrix.to_string_lossy())
    );
}

#[test]
fn non_replica_outside_ac_errors_for_every_token() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo-thing");
    std::fs::create_dir_all(&repo).unwrap();
    let ctx = placeholder_context_for_launch_root(&repo).unwrap();

    assert_eq!(ctx.root_kind, PlaceholderRootKind::NormalLaunchCwd);
    assert_eq!(ctx.ac_root, None);
    assert_eq!(ctx.matrix_root, None);

    assert_eq!(
        expand_placeholders(r"%AC_REPLICA_ROOT%\x", &ctx).unwrap_err(),
        AC_REPLICA_ROOT_ERROR
    );
    assert_eq!(
        expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &ctx).unwrap_err(),
        AC_WORKSPACE_ROOT_ERROR
    );
    assert_eq!(
        expand_placeholders(r"%AC_MATRIX_ROOT%\x", &ctx).unwrap_err(),
        AC_MATRIX_ROOT_ERROR
    );
}

#[test]
fn legacy_ac_root_token_is_now_an_unknown_marker() {
    let temp = tempfile::tempdir().unwrap();
    let replica = make_replica(temp.path());
    let ctx = placeholder_context_for_launch_root(&replica).unwrap();
    // No alias: a leftover %AC_ROOT% is no longer expanded and trips the backstop.
    let err = expand_placeholders(r"%AC_ROOT%\x", &ctx).unwrap_err();
    assert!(err.contains("unknown placeholder"), "{err}");
}

#[test]
fn expand_in_content_substitutes_known_tokens_forward_slashed() {
    let temp = tempfile::tempdir().unwrap();
    let replica = make_replica(temp.path());
    let ctx = placeholder_context_for_launch_root(&replica).unwrap();

    let expected_replica = canonical_stripped(&replica)
        .to_string_lossy()
        .replace('\\', "/");
    let out = expand_placeholders_in_content(
        "root=%AC_REPLICA_ROOT%/x and %AC_WORKSPACE_ROOT% and %AC_MATRIX_ROOT%",
        &ctx,
    );
    assert!(
        out.contains(&format!("root={}/x", expected_replica)),
        "{out}"
    );
    // No backslashes from the substituted paths (forward-slash normalized).
    assert!(!out.contains('\\'), "{out}");
}

#[test]
fn expand_in_content_leaves_unknown_markers_literal() {
    let temp = tempfile::tempdir().unwrap();
    let replica = make_replica(temp.path());
    let ctx = placeholder_context_for_launch_root(&replica).unwrap();
    // Unlike expand_placeholders, content expansion is lenient: unknown
    // %...% markers survive verbatim (the template may use them at runtime).
    let out = expand_placeholders_in_content("%UNKNOWN% and %AC_ROOT% stay", &ctx);
    assert_eq!(out, "%UNKNOWN% and %AC_ROOT% stay");
}

#[test]
fn expand_in_content_skips_tokens_with_no_matching_root() {
    // Normal cwd: replica token is NOT substituted (no replica root kind),
    // and workspace/matrix are None, so all three are left literal.
    let normal = PlaceholderContext {
        replica_root: PathBuf::from(if cfg!(windows) { r"C:\cwd" } else { "/cwd" }),
        root_kind: PlaceholderRootKind::NormalLaunchCwd,
        ac_root: None,
        matrix_root: None,
    };
    let out = expand_placeholders_in_content(
        "%AC_REPLICA_ROOT% %AC_WORKSPACE_ROOT% %AC_MATRIX_ROOT%",
        &normal,
    );
    assert_eq!(
        out,
        "%AC_REPLICA_ROOT% %AC_WORKSPACE_ROOT% %AC_MATRIX_ROOT%"
    );
}

// ---- %USER_HOME% (#924) ------------------------------------------------

/// `dirs::home_dir()` forward-slashed, or `None` on a machine without a home.
/// Tests derive the expectation instead of hardcoding a literal, so they stay
/// deterministic across OSes and CI users.
fn expected_home_fwd() -> Option<String> {
    dirs::home_dir().map(|home| home.to_string_lossy().replace('\\', "/"))
}

#[test]
fn content_expands_user_home_forward_slashed() {
    let temp = tempfile::tempdir().unwrap();
    let replica = make_replica(temp.path());
    let ctx = placeholder_context_for_launch_root(&replica).unwrap();

    let out = expand_placeholders_in_content("excl=%USER_HOME%/.claude/CLAUDE.md", &ctx);
    match expected_home_fwd() {
        Some(home) => {
            assert_eq!(out, format!("excl={}/.claude/CLAUDE.md", home));
            assert!(!out.contains('\\'), "{out}");
            assert!(!out.contains(USER_HOME_PLACEHOLDER), "{out}");
        }
        // No home on this machine: the token stays literal, the spawn never aborts.
        None => assert_eq!(out, "excl=%USER_HOME%/.claude/CLAUDE.md"),
    }
}

#[test]
fn content_leaves_unknown_percent_markers_literal() {
    let temp = tempfile::tempdir().unwrap();
    let replica = make_replica(temp.path());
    let ctx = placeholder_context_for_launch_root(&replica).unwrap();
    // %USER_HOME% is the ONLY new token: sibling %VAR% markers that a tool reads
    // at runtime (hook commands, shell vars) must survive verbatim.
    let out = expand_placeholders_in_content("%TEMP% %CD% %USERPROFILE%", &ctx);
    assert_eq!(out, "%TEMP% %CD% %USERPROFILE%");
}

#[test]
fn content_expands_user_home_alongside_ac_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let replica = make_replica(temp.path());
    let ctx = placeholder_context_for_launch_root(&replica).unwrap();

    let out = expand_placeholders_in_content(
        "%AC_REPLICA_ROOT% %AC_WORKSPACE_ROOT% %AC_MATRIX_ROOT% %USER_HOME%",
        &ctx,
    );
    let expected_replica = canonical_stripped(&replica)
        .to_string_lossy()
        .replace('\\', "/");
    assert!(out.starts_with(&expected_replica), "{out}");
    assert!(!out.contains("%AC_"), "{out}");
    match expected_home_fwd() {
        Some(home) => {
            assert!(out.ends_with(&home), "{out}");
            assert!(!out.contains('\\'), "{out}");
        }
        // No home: the AC tokens still expand, %USER_HOME% stays literal.
        None => {
            assert!(out.ends_with(USER_HOME_PLACEHOLDER), "{out}");
            assert!(out.contains("%USER_HOME%"), "{out}");
        }
    }
}

#[test]
fn user_home_is_not_an_ac_placeholder_token() {
    // GATE, not catalog: `value_contains_ac_placeholder` drives the "this spawn
    // requires an AC launch root" decision (agent_command.rs) and the CODEX_HOME
    // validators. %USER_HOME% needs no AC context, so it must stay out.
    assert!(!AC_PLACEHOLDER_TOKENS.contains(&USER_HOME_PLACEHOLDER));
    assert!(!value_contains_ac_placeholder("%USER_HOME%/.claude"));
}

#[test]
fn strict_expand_still_rejects_user_home() {
    // v1 scope: env values and command args keep their fail-closed contract,
    // because `home_dir()` can return None and strict mode cannot degrade.
    let temp = tempfile::tempdir().unwrap();
    let replica = make_replica(temp.path());
    let ctx = placeholder_context_for_launch_root(&replica).unwrap();
    let err = expand_placeholders("%USER_HOME%/x", &ctx).unwrap_err();
    assert!(err.contains("unknown placeholder"), "{err}");
}

#[test]
fn value_contains_ac_placeholder_matches_only_known_tokens() {
    assert!(value_contains_ac_placeholder(r"%AC_REPLICA_ROOT%\x"));
    assert!(value_contains_ac_placeholder(r"%AC_WORKSPACE_ROOT%\x"));
    assert!(value_contains_ac_placeholder(r"%AC_MATRIX_ROOT%\x"));
    assert!(!value_contains_ac_placeholder(r"%AC_ROOT%\x"));
    assert!(!value_contains_ac_placeholder("%UNKNOWN%"));
    assert!(!value_contains_ac_placeholder("plain text"));
}

#[test]
fn root_agent_and_normal_contexts_resolve_replica_token_only() {
    // Hermetic: build the context directly because `is_root_agent_dir` reads the
    // real configured root_agent_dir() from disk and cannot be faked in a unit test.
    let root_agent = PlaceholderContext {
        replica_root: PathBuf::from(if cfg!(windows) {
            r"C:\some\replica"
        } else {
            "/some/replica"
        }),
        root_kind: PlaceholderRootKind::AcReplicaOrRootAgent,
        ac_root: None,
        matrix_root: None,
    };
    assert!(expand_placeholders(r"%AC_REPLICA_ROOT%\x", &root_agent).is_ok());
    assert_eq!(
        expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &root_agent).unwrap_err(),
        AC_WORKSPACE_ROOT_ERROR
    );
    assert_eq!(
        expand_placeholders(r"%AC_MATRIX_ROOT%\x", &root_agent).unwrap_err(),
        AC_MATRIX_ROOT_ERROR
    );

    let normal = PlaceholderContext {
        replica_root: PathBuf::from(if cfg!(windows) {
            r"C:\some\cwd"
        } else {
            "/some/cwd"
        }),
        root_kind: PlaceholderRootKind::NormalLaunchCwd,
        ac_root: None,
        matrix_root: None,
    };
    assert_eq!(
        expand_placeholders(r"%AC_REPLICA_ROOT%\x", &normal).unwrap_err(),
        AC_REPLICA_ROOT_ERROR
    );
}

#[test]
fn derive_matrix_root_matches_canonical_formula() {
    let ac_root = PathBuf::from(if cfg!(windows) {
        r"C:\proj\.ac"
    } else {
        "/proj/.ac"
    });
    let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
    assert_eq!(
        derive_matrix_root(&replica, Some(&ac_root)),
        Some(ac_root.join("_agent_dev-rust"))
    );

    // Non-replica leaf -> None.
    let repo = ac_root.join("wg-7-dev-team").join("repo-thing");
    assert_eq!(derive_matrix_root(&repo, Some(&ac_root)), None);

    // Replica leaf but no .ac ancestor -> None (guards the ac_root.map arm).
    assert_eq!(derive_matrix_root(&replica, None), None);

    // F1: degenerate / invalid agent name -> None (parity with validate_agent_name).
    let empty = ac_root.join("wg-7-dev-team").join("__agent_");
    assert_eq!(derive_matrix_root(&empty, Some(&ac_root)), None);
    let dotted = ac_root.join("wg-7-dev-team").join("__agent_bad.name");
    assert_eq!(derive_matrix_root(&dotted, Some(&ac_root)), None);
}

#[test]
fn non_replica_inside_ac_resolves_ac_root_only() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join(".ac").join("wg-7-dev-team").join("repo-x");
    std::fs::create_dir_all(&repo).unwrap();
    let ctx = placeholder_context_for_launch_root(&repo).unwrap();

    assert_eq!(ctx.root_kind, PlaceholderRootKind::NormalLaunchCwd);
    assert_eq!(ctx.matrix_root, None);

    let expected_replica = canonical_stripped(&repo);
    let expected_ac_root = expected_replica
        .parent()
        .and_then(|parent| parent.parent())
        .expect("repo-x has a .ac workspace ancestor");
    assert_eq!(ctx.ac_root.as_deref(), Some(expected_ac_root));

    assert_eq!(
        expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &ctx).unwrap(),
        format!(r"{}\x", expected_ac_root.to_string_lossy())
    );
    assert_eq!(
        expand_placeholders(r"%AC_REPLICA_ROOT%\x", &ctx).unwrap_err(),
        AC_REPLICA_ROOT_ERROR
    );
    assert_eq!(
        expand_placeholders(r"%AC_MATRIX_ROOT%\x", &ctx).unwrap_err(),
        AC_MATRIX_ROOT_ERROR
    );
}
