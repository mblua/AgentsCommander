use super::*;

#[test]
fn build_replica_context_refuses_a_root_named_directory() {
    // #979 G.4, guard 2 of 4 (the generic builder). Must hold for BOTH a missing
    // and a present config.json: the guard is the first executable line, ahead of
    // the missing-config `Ok(None)` early return.
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).expect("create root");
    let sentinel_path = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let sentinel = "PARENT_GLOBAL_SENTINEL\n";
    std::fs::write(&sentinel_path, sentinel).expect("write parent sentinel");
    let root_str = path_string(&root);

    let err = build_replica_context(&root_str, None)
        .expect_err("no config.json: the generic builder must still refuse Root");
    assert!(err.contains("build_root_agent_context"), "{}", err);

    std::fs::write(
        root.join("config.json"),
        r#"{"context":["$AGENTSCOMMANDER_CONTEXT","Role.md"]}"#,
    )
    .expect("write config");
    let err = build_replica_context(&root_str, None)
        .expect_err("with config.json: the generic builder must refuse Root");
    assert!(err.contains("build_root_agent_context"), "{}", err);

    assert_eq!(
        std::fs::read(&sentinel_path).expect("read sentinel"),
        sentinel.as_bytes()
    );
}

#[test]
fn ensure_session_context_returns_the_root_prologue_and_never_touches_the_global() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    let sentinel = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&sentinel, "PARENT_GLOBAL_SENTINEL\n").unwrap();
    assert!(ensure_session_context(&path_string(&root))
        .unwrap_err()
        .contains("Root identity"));
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "PARENT_GLOBAL_SENTINEL\n"
    );
}

#[test]
fn resolve_agent_context_refuses_a_root_named_path_without_touching_the_global() {
    // #979 G.4, guard 4 of 4 (the global resolver). The guard is the FIRST
    // executable statement, so nothing is read, created, synced, or rewritten.
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).expect("create root");
    let sentinel_path = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let sentinel = "PARENT_GLOBAL_SENTINEL\n";
    std::fs::write(&sentinel_path, sentinel).expect("write parent sentinel");

    let err = resolve_agent_context(
        &path_string(&root),
        None,
        &no_skill_section(),
        &root,
        None,
        None,
    )
    .expect_err("the resolver must refuse a Root-named path");
    assert!(
        err.contains("must not resolve the global context template"),
        "{}",
        err
    );
    assert_eq!(
        std::fs::read(&sentinel_path).expect("read sentinel"),
        sentinel.as_bytes()
    );

    // ...and with no global on disk, the refusal creates none.
    std::fs::remove_file(&sentinel_path).expect("remove sentinel");
    resolve_agent_context(
        &path_string(&root),
        None,
        &no_skill_section(),
        &root,
        None,
        None,
    )
    .expect_err("the resolver must still refuse a Root-named path");
    assert!(!sentinel_path.exists());
}

#[test]
fn root_context_builder_preserves_order_and_skips_tokens() {
    // #979 G.8: prologue first, then the raw entries in their stored order. The
    // stale global token, the deprecated repos token, and non-string values are
    // all skipped.
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("a.md"), "RAW_ENTRY_A\n").expect("write a.md");
    std::fs::write(root.join("b.md"), "RAW_ENTRY_B\n").expect("write b.md");
    std::fs::write(
        root.join("config.json"),
        r#"{"context":["a.md","$AGENTSCOMMANDER_CONTEXT",42,"$REPOS_WORKSPACE_INFO","b.md"]}"#,
    )
    .expect("write config");

    let built = build_root_agent_context_at(&path_string(&root), temp.path(), None, false)
        .expect("build root context");
    let content = std::fs::read_to_string(&built).expect("read built context");

    assert!(!content.contains("# AgentsCommander Root Runtime Context"));
    let a = content.find("RAW_ENTRY_A").expect("a.md present");
    let b = content.find("RAW_ENTRY_B").expect("b.md present");
    assert!(a < b, "a.md, then b.md");
    assert!(!content.contains("$AGENTSCOMMANDER_CONTEXT"));
    assert!(!content.contains("$REPOS_WORKSPACE_INFO"));
}

#[test]
fn root_context_builder_reports_every_missing_file_together() {
    // #979 G.8: one aggregated `Root Agent` error, not the generic `Replica` one.
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(
        root.join("config.json"),
        r#"{"context":["missing-one.md","missing-two.md"]}"#,
    )
    .expect("write config");

    let err = build_root_agent_context_at(&path_string(&root), temp.path(), None, false)
        .expect_err("missing raw context files must fail the build");
    assert!(
        err.contains("Root Agent has missing context files"),
        "{}",
        err
    );
    assert!(err.contains("missing-one.md"), "{}", err);
    assert!(err.contains("missing-two.md"), "{}", err);
    assert!(!err.contains("Replica"), "{}", err);
}

#[test]
fn root_context_builder_emits_the_prologue_without_any_config() {
    // The prologue is unconditional: no config.json, no context[], no way to
    // suppress Root's mandatory governance.
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).expect("create root");

    std::fs::write(
        temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
        super::super::super::root_agent::default_root_context_template(),
    )
    .unwrap();
    std::fs::write(root.join("Role.md"), "ROLE").unwrap();
    let built = build_root_agent_context_at(&path_string(&root), temp.path(), None, false).unwrap();
    let content = std::fs::read_to_string(&built).unwrap();
    assert!(content.contains("# AgentsCommander Root Runtime Context"));
    assert!(content.contains("## GOLDEN RULE"));
    assert!(content.contains("## Inter-Agent Messaging"));

    std::fs::write(root.join("config.json"), r#"{"context":[]}"#).expect("write empty context");
    let built = build_root_agent_context_at(&path_string(&root), temp.path(), None, false)
        .expect("build root context");
    let content = std::fs::read_to_string(&built).expect("read built context");
    assert_eq!(
        content,
        super::super::default_context::SELF_CLEAR_MANUAL_SECTION
    );
}

#[test]
fn stale_generated_legacy_default_heals_after_hard_link_migration() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let old_matrix = ac_root.join("_agent_dev-rust");
    let new_matrix = ac_root.join("_agent_tech-lead");
    let old_replica = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let new_replica = ac_root.join("wg-19-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&old_matrix).expect("create old matrix");
    std::fs::create_dir_all(&new_matrix).expect("create new matrix");
    std::fs::create_dir_all(&old_replica).expect("create old replica");
    std::fs::create_dir_all(&new_replica).expect("create new replica");

    // Seed ONLY the legacy-named template (G5). The resolve path migrates it
    // via hard-link to the current name, then the heal replaces that
    // (single-link) file. The healed bytes and the absence of any stray
    // legacy-named file are both asserted.
    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    let legacy_path = ac_root.join(LEGACY_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&legacy_path, &legacy).expect("write legacy-named template");

    let rendered = resolve_agent_context(
        &path_string(&new_replica),
        Some(&path_string(&new_matrix)),
        &no_skill_section(),
        &new_replica,
        None,
        None,
    )
    .expect("resolve context");
    assert_mandatory_sections_once(&rendered);
    assert_no_raw_template_placeholders(&rendered);

    let new_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let healed = std::fs::read_to_string(&new_path).expect("read healed template");
    assert_eq!(healed, get_default_agent_template());
    assert!(
        !legacy_path.exists(),
        "stray legacy-named template left in play after heal"
    );
    assert_no_context_template_temp_leftover(&ac_root);
}

#[test]
fn atomic_context_publication_captures_clock_after_replace() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&path, "old contents").expect("write old target");
    let expected = fixed_publication_time();
    let later = expected + chrono::Duration::seconds(30);
    let observed_time = std::cell::Cell::new(expected);
    let clock_sampled = std::cell::Cell::new(false);
    let mut clock = || {
        assert_eq!(
            std::fs::read_to_string(&path).expect("read target at commit clock"),
            "new contents",
            "the target replacement must commit before the clock sample"
        );
        clock_sampled.set(true);
        observed_time.get()
    };

    let published_at =
        atomically_replace_context_template_with(&path, "new contents", &mut clock, |parent| {
            assert_eq!(parent, temp.path());
            assert!(
                clock_sampled.get(),
                "the commit-point clock must be sampled before parent sync"
            );
            observed_time.set(later);
        })
        .expect("replace target");

    assert_eq!(published_at, expected);
    assert_eq!(observed_time.get(), later);
    assert_no_context_template_temp_leftover(temp.path());
}

#[test]
fn atomically_replace_context_template_writes_absent_and_existing_dest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let mut clock = chrono::Utc::now;

    // Absent destination: plain create + rename publish.
    atomically_replace_context_template(&path, "first contents", &mut clock)
        .expect("replace over absent dest");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read first"),
        "first contents"
    );

    // Existing destination: exercises the Windows ReplaceFileW branch.
    atomically_replace_context_template(&path, "second contents", &mut clock)
        .expect("replace over existing dest");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read second"),
        "second contents"
    );

    assert_no_context_template_temp_leftover(temp.path());
}

#[test]
fn atomically_replace_context_template_errors_without_leaving_temp() {
    let temp = tempfile::tempdir().expect("tempdir");
    // The parent directory does not exist, so the temp create fails: the
    // helper returns Err and never creates the directory or a temp file.
    let missing_dir = temp.path().join("does-not-exist");
    let path = missing_dir.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let mut clock = chrono::Utc::now;
    let result =
        atomically_replace_context_template(&path, get_default_agent_template(), &mut clock);
    assert!(
        result.is_err(),
        "expected Err when the parent dir is missing"
    );
    assert!(
        !missing_dir.exists(),
        "the helper must not create the missing directory"
    );
}

#[cfg(unix)]
#[test]
fn heal_failure_is_non_fatal_and_preserves_render() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let old_matrix = ac_root.join("_agent_dev-rust");
    let new_matrix = ac_root.join("_agent_tech-lead");
    let old_replica = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let new_replica = ac_root.join("wg-19-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&old_matrix).expect("create old matrix");
    std::fs::create_dir_all(&new_matrix).expect("create new matrix");
    std::fs::create_dir_all(&old_replica).expect("create old replica");
    std::fs::create_dir_all(&new_replica).expect("create new replica");

    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write stale generated default");

    // Make the context dir read-only so the atomic temp-create fails and the
    // heal cannot publish.
    let mut perms = std::fs::metadata(&ac_root).expect("metadata").permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&ac_root, perms).expect("set read-only");

    let rendered = resolve_agent_context(
        &path_string(&new_replica),
        Some(&path_string(&new_matrix)),
        &no_skill_section(),
        &new_replica,
        None,
        None,
    );

    // Restore write perms before any assertion so tempdir cleanup works.
    let mut perms = std::fs::metadata(&ac_root).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&ac_root, perms).expect("restore perms");

    // The heal failed, but resolve still returns the correct in-memory
    // render (best-effort, never propagated).
    let rendered = rendered.expect("resolve must succeed even when heal fails");
    assert_mandatory_sections_once(&rendered);
    assert_no_raw_template_placeholders(&rendered);
    // The on-disk template is left as the stale legacy, unchanged.
    let on_disk = std::fs::read_to_string(&template_path).expect("read template");
    assert_eq!(on_disk, legacy);
}

#[test]
fn agent_repos_placeholder_via_legacy_token_uses_repaired_replica_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let repo_dir = ac_root.join("wg-19-dev-team").join("repo-Example");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::create_dir_all(&repo_dir).expect("create repo dir");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        "{{WORKSPACE_REPOS}}",
    )
    .expect("write repos-only template");
    std::fs::write(
        replica_root.join("config.json"),
        concat!(
            r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"],"#,
            r#""repos":["../repo-Example"]}"#
        ),
    )
    .expect("write replica config");

    let materialized = materialize_agent_context_file(
        &path_string(&replica_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(content.contains("## Repos"));
    assert!(content.contains("repo-Example"));
    assert_contains_canonical_path(&content, &repo_dir);
    assert!(!content.contains("No repos configured"));
    assert!(
        content.starts_with("# Agent Repos"),
        "legacy token was appended instead of rendered in place"
    );
}

#[test]
fn agent_repos_placeholder_uses_missing_context_repaired_to_global() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let repo_dir = ac_root.join("wg-19-dev-team").join("repo-Example");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::create_dir_all(&repo_dir).expect("create repo dir");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        "{{AGENT_REPOS}}",
    )
    .expect("write repos-only template");
    std::fs::write(
        replica_root.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","repos":["../repo-Example"]}"#,
    )
    .expect("write replica config");

    let materialized = materialize_agent_context_file(
        &path_string(&replica_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");
    let repaired: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(replica_root.join("config.json")).expect("read config"),
    )
    .expect("parse repaired config");

    assert!(content.contains("repo-Example"));
    assert_contains_canonical_path(&content, &repo_dir);
    assert!(!content.contains("No repos configured"));
    assert_eq!(
        repaired["context"],
        serde_json::json!(["$AGENTSCOMMANDER_CONTEXT"])
    );
}

#[test]
fn agent_repos_placeholder_uses_empty_context_repaired_to_global() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let repo_dir = ac_root.join("wg-19-dev-team").join("repo-Example");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::create_dir_all(&repo_dir).expect("create repo dir");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        "{{AGENT_REPOS}}",
    )
    .expect("write repos-only template");
    std::fs::write(
        replica_root.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","context":[],"repos":["../repo-Example"]}"#,
    )
    .expect("write replica config");

    let materialized = materialize_agent_context_file(
        &path_string(&replica_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");
    let repaired: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(replica_root.join("config.json")).expect("read config"),
    )
    .expect("parse repaired config");

    assert!(content.contains("repo-Example"));
    assert_contains_canonical_path(&content, &repo_dir);
    assert!(!content.contains("No repos configured"));
    assert_eq!(
        repaired["context"],
        serde_json::json!(["$AGENTSCOMMANDER_CONTEXT"])
    );
}

/// #1369 (C4) AC-2: render `template` as the whole global template of a
/// replica that has one repo, end to end through
/// `materialize_agent_context_file`, and return the materialized bytes. Same
/// shape as the three legacy-token tests above; kept local to the alias
/// cases so those tests stay untouched.
fn render_repos_only_template(template: &str) -> String {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let repo_dir = ac_root.join("wg-19-dev-team").join("repo-Example");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::create_dir_all(&repo_dir).expect("create repo dir");
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), template)
        .expect("write repos-only template");
    std::fs::write(
        replica_root.join("config.json"),
        concat!(
            r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"],"#,
            r#""repos":["../repo-Example"]}"#
        ),
    )
    .expect("write replica config");

    let materialized = materialize_agent_context_file(
        &path_string(&replica_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    std::fs::read_to_string(materialized).expect("read materialized context")
}

/// AC-2 case 2.1: the current token renders exactly one block.
#[test]
fn agent_repos_current_token_renders_exactly_one_block() {
    let content = render_repos_only_template("{{AGENT_REPOS}}");

    assert_eq!(count_section_headings(&content, "# Agent Repos"), 1);
    assert_no_raw_template_placeholders(&content);
    assert!(content.contains("## Repos"));
    assert!(content.contains("repo-Example"));
}

/// AC-2 case 2.2 (the issue's AC#2 verbatim): a template carrying ONLY the
/// pre-#1369 token renders exactly one block, and the legacy token itself
/// does not survive into the prompt.
#[test]
fn agent_repos_legacy_token_renders_exactly_one_block() {
    let content = render_repos_only_template("{{WORKSPACE_REPOS}}");

    assert_eq!(count_section_headings(&content, "# Agent Repos"), 1);
    assert_no_raw_template_placeholders(&content);
    assert!(content.contains("## Repos"));
    assert!(content.contains("repo-Example"));
    assert!(!content.contains("{{WORKSPACE_REPOS}}"));
}

/// AC-2 case 2.3: both tokens in one template still emit exactly one block.
/// This is the ONLY detector of a deleted `legacy_repos_replacement` guard:
/// without it two `# Agent Repos` blocks are emitted with no raw
/// placeholder left, so `assert_no_raw_template_placeholders` stays green.
#[test]
fn agent_repos_both_tokens_render_exactly_one_block() {
    let content = render_repos_only_template("{{AGENT_REPOS}}\n\n{{WORKSPACE_REPOS}}");

    assert_eq!(count_section_headings(&content, "# Agent Repos"), 1);
    assert_no_raw_template_placeholders(&content);
}

#[test]
fn root_manual_clear_survives_custom_template_and_auto_setting_once() {
    for auto in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ac-root-agent");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
            "CUSTOM_ROOT_TEMPLATE",
        )
        .unwrap();
        std::fs::write(root.join("Role.md"), "ROOT_ROLE").unwrap();
        let path =
            build_root_agent_context_at(&path_string(&root), temp.path(), None, auto).unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(content.matches("## Manual context clear").count(), 1);
        assert!(content.contains("CUSTOM_ROOT_TEMPLATE"));
        assert!(content.contains("ROOT_ROLE"));
        assert!(content.contains("including Root"));
        assert!(content.contains("self-clear ON or OFF"));
    }
}
