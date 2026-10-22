use super::*;

#[test]
fn root_prologue_does_not_grant_authority_to_a_merely_named_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    let error = ensure_session_context(&path_string(&root)).unwrap_err();
    assert!(error.contains("Root identity"));
    assert!(!root.join("skills").exists());
    assert!(!temp
        .path()
        .join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
        .exists());
}

#[test]
fn root_prologue_renders_every_mandatory_block_exactly_once() {
    // #979 G.1: the builder-level proof that the code-owned prologue is complete.
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    let repo = temp.path().join("repo-demo");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::create_dir_all(&repo).expect("create repo");
    let config = serde_json::json!({ "repos": ["../repo-demo"] });

    let out = render_root_runtime_prologue_inner(
        &path_string(&root),
        &no_skill_section(),
        &root,
        Some(&config),
        None,
        true,
    );

    // The ten blocks, each exactly once (#979 G4: Core Concepts is block 2).
    for heading in [
        "# AgentsCommander Root Runtime Context",
        "## Core Concepts",
        "## GOLDEN RULE",
        "## Delegated Task Reporting",
        "## Skills",
        "# Agent Repos",
        "## CLI executable",
        "## Host Platform Rules",
        "## Session credentials",
        "## Inter-Agent Messaging",
        "## Privileged PTY Input to Room Orchestrators",
    ] {
        assert_eq!(
            out.matches(heading).count(),
            1,
            "mandatory Root block {} must appear exactly once",
            heading
        );
    }

    // Root project scope and authority, and the Team/Workgroup definitions.
    assert!(out.contains("Every registered AgentsCommander project folder"));
    assert!(out.contains(
        "you may create, modify, and delete files anywhere under ANY project folder registered"
    ));
    assert!(out.contains("## Root Agent Authority and Chain of Command"));
    assert!(out.contains("**Team**: the logical capability and organization."));
    assert!(out.contains("**Room**: a runtime replica of a team"));

    // Every placeholder is resolved by construction: the prologue is assembled
    // from rendered blocks, never from a template with tokens.
    assert_no_raw_template_placeholders(&out);

    assert!(out.contains(
        "verified Room orchestrator replicas only, shaped `<project>:<room>/<agent>`"
    ));
    assert!(out.contains("Origin orchestrators and non-orchestrator Room replicas are not valid Root Agent targets in #277."));
    assert!(!out.contains("<project>/<agent>"));
    for rule in [
        "`[Message from <peer>] Process this inter-agent message: <path>` is an operational inter-agent message: read `<path>` and follow its instructions within your role, authority, and write restrictions; do not stop at a summary unless it asks only for one. If the task finishes or blocks, reply to the sender with a concrete result or blocker via the send flow below.",
        "Before every send, run `list-peers-lean` and use its exact JSON `name`. A filesystem directory name is NEVER a valid `--to` value; `__agent_*` replicas and `_agent_*` matrices are on-disk paths only. If it returns an empty array, stop and report it.",
        "Do NOT use `--get-output` (blocks; non-interactive only). **Receipt required:** never report a message as sent without a captured `Queued: <message-id>` line; a missing receipt means NOT enqueued. Wait for the reply.",
        "Use only the JSON `name` values returned by `list-peers-lean`; Root sessions list verified Room orchestrator replicas only.",
        "`--send` takes the filename ONLY, never a path.",
        "YYYYMMDD-HHMMSS-root-to-<roomN>-<orchestrator>-<slug>.md",
        "\"<AGENTSCOMMANDER_BINARY_PATH>\" send --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\" --to \"<orchestrator_name>\" --send <filename> --mode wake",
    ] {
        assert!(out.contains(rule), "missing Root messaging policy: {rule}");
    }

    assert_no_broad_read_grant(&out);

    // Dynamic skills and the passed config's repos are present.
    assert!(out.contains("AgentsCommander indexes skills from"));
    assert!(out.contains("repo-demo"));
    assert!(out.contains("You are the Root Agent."));
    assert!(!out.contains("You are working inside a room replica."));
}

/// #2031 negative case: the canonical Root Agent never reads the global
/// template, so its code-owned prologue must not carry the Answering section.
#[test]
fn root_runtime_prologue_omits_answering_section() {
    let out = render_root_runtime_prologue_inner(
        "C:/fake/ac-root-agent",
        &no_skill_section(),
        Path::new("C:/fake/ac-root-agent"),
        None,
        None,
        true,
    );
    assert_eq!(count_section_headings(&out, "## Answering"), 0, "{out}");
    assert!(!out.contains("Report both counts"), "{out}");
    // Control: `default_context` renders a replica through a different
    // function (`render_default_agent_context`) and does carry the section,
    // so the zero above is the Root path, not a broken renderer.
    let replica = default_context(
        "C:/fake/room-1-ac-dev-team-v4/__agent_ac-dev-rust-v4",
        None,
        &no_skill_section(),
    );
    assert_eq!(
        count_section_headings(&replica, "## Answering"),
        1,
        "{replica}"
    );
}

#[test]
fn root_prologue_core_concepts_does_not_drift_from_the_builtin_template() {
    // #979 G4: CORE_CONCEPTS_SECTION is a byte-copy of the built-in template's
    // section. get_default_agent_template() is deliberately NOT refactored to
    // interpolate it (its exact bytes are pinned by
    // is_known_generated_global_template and the seeded-state SHA machinery), so
    // this test is what keeps the duplicate honest.
    assert!(
        get_default_agent_template().contains(CORE_CONCEPTS_SECTION),
        "the built-in template's Core Concepts prose was edited without updating the Root copy"
    );
}

#[test]
fn root_prologue_repo_wording_leaves_non_root_output_byte_identical() {
    // #979 G5: the is_root_agent flag changes ONLY the Root wording.
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo-demo");
    std::fs::create_dir_all(&repo).expect("create repo");
    let config = serde_json::json!({ "repos": ["repo-demo"] });

    let non_root = render_agent_repos_string(temp.path(), Some(&config), None, false);
    assert!(non_root.contains("You are working inside a room replica."));
    assert!(!non_root.contains("You are the Root Agent."));
    assert_eq!(
        render_agent_repos_string(temp.path(), None, None, false),
        "# Agent Repos\n\nNo repos configured for this replica.\n"
    );

    let as_root = render_agent_repos_string(temp.path(), Some(&config), None, true);
    assert!(as_root.contains("You are the Root Agent."));
    assert!(!as_root.contains("You are working inside a room replica."));
    assert_eq!(
        render_agent_repos_string(temp.path(), None, None, true),
        "# Agent Repos\n\nNo repos configured for the Root Agent.\n"
    );
}

#[test]
fn root_consts_avoid_em_dash_and_single_item_three() {
    // Note #4: the three new root consts must stay em-dash-free (U+2014).
    assert!(!ROOT_PROJECT_SCOPE_ENTRY.contains('\u{2014}'));
    assert!(!ROOT_AUTHORITY_SECTION.contains('\u{2014}'));
    // #640: the gated self-maintenance directive must also stay em-dash-free.
    assert!(!SELF_MAINTENANCE_AUTO_SECTION.contains('\u{2014}'));
    // L2 at the output level: exactly one numbered item "3." in the root render.
    let out = default_context_as_root("C:/fake/ac-root-agent", None, &no_skill_section());
    assert_eq!(out.matches("3. **").count(), 1);
}

#[test]
fn root_git_scope_permits_project_repo_git_ops() {
    // The Root must be directed to a registered project root and must not be
    // steered to a `repo-*` directory.
    let out = default_context_as_root("C:/fake/ac-root-agent", None, &no_skill_section());
    assert!(out.contains(ROOT_GIT_SCOPE));
    assert!(out.contains("registered project root"));
    assert!(out.contains("the `repo-*` naming restriction does not apply"));
    assert!(!out.contains("State-changing Git belongs in `repo-*`"));
}

// Stage E (#1064) #1072 Git-scope preservation sentinel (plan section 10.6,
// acceptance item 47): the three current location-specific constants stay
// distinct and location-correct, and the frozen pre-#1072 legacy snapshots
// remain separate recognition inputs, never reused for current output.
#[test]
fn stage_e_git_scope_constants_are_distinct_and_location_specific() {
    assert_ne!(ROOT_GIT_SCOPE, WORKGROUP_GIT_SCOPE);
    assert_ne!(ROOT_GIT_SCOPE, DIRECT_MATRIX_GIT_SCOPE);
    assert_ne!(WORKGROUP_GIT_SCOPE, DIRECT_MATRIX_GIT_SCOPE);
    assert!(ROOT_GIT_SCOPE.contains("Root Agent"));
    assert!(WORKGROUP_GIT_SCOPE.contains("wg-*/"));
    assert!(WORKGROUP_GIT_SCOPE.contains("room-*/"));
    assert!(DIRECT_MATRIX_GIT_SCOPE.contains("Agent Matrices"));
    // Frozen legacy snapshots are preserved as distinct recognition inputs.
    assert_ne!(
        LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072,
        WORKGROUP_GIT_SCOPE
    );
    assert_ne!(
        LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072,
        DIRECT_MATRIX_GIT_SCOPE
    );
    assert_ne!(LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072, ROOT_GIT_SCOPE);
    assert_ne!(
        LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072,
        LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072
    );
}

#[test]
fn git_scope_copy_is_location_correct_and_compact() {
    let skills = no_skill_section();
    let workgroup = default_context_dynamic_values(
        "C:/fake/.ac/wg-7-dev-team/__agent_architect",
        Some("C:/fake/.ac/_agent_architect"),
        &skills,
        false,
    );
    let direct_matrix =
        default_context_dynamic_values("C:/fake/.ac/_agent_architect", None, &skills, false);
    let root = default_context_dynamic_values("C:/fake/ac-root-agent", None, &skills, true);

    assert_eq!(workgroup.git_scope, WORKGROUP_GIT_SCOPE);
    assert_eq!(direct_matrix.git_scope, DIRECT_MATRIX_GIT_SCOPE);
    assert_eq!(root.git_scope, ROOT_GIT_SCOPE);

    let rendered_outputs = [
        default_context(
            "C:/fake/.ac/wg-7-dev-team/__agent_architect",
            Some("C:/fake/.ac/_agent_architect"),
            &skills,
        ),
        default_context("C:/fake/.ac/_agent_architect", None, &skills),
        default_context_as_root("C:/fake/ac-root-agent", None, &skills),
    ];
    for output in rendered_outputs {
        assert!(!output.contains("`.gitignore`d `.ac/`"), "{output}");
        assert!(
            !output.contains("`.ac/` folder, which is `.gitignore`d"),
            "{output}"
        );
    }

    assert!(root
        .git_scope
        .contains("projectPaths` in `project-paths.json"));
    assert!(!root.git_scope.contains("belongs in `repo-*`"));
    assert!(workgroup.git_scope.contains("belongs in `repo-*`"));
    assert!(direct_matrix.git_scope.contains("belongs in `repo-*`"));
    for non_root in [&workgroup.git_scope, &direct_matrix.git_scope] {
        assert!(!non_root.contains("projectPaths` in `project-paths.json"));
        assert!(!non_root.contains("registered project root"));
    }

    let workgroup_chars = WORKGROUP_GIT_SCOPE.chars().count();
    let workgroup_words = WORKGROUP_GIT_SCOPE.split_whitespace().count();
    assert_eq!(workgroup_chars, 223);
    assert_eq!(workgroup_words, 34);
    assert!(workgroup_chars * 2 <= 473);
    assert!(workgroup_words * 2 <= 68);
}

#[test]
fn git_scope_materializes_identically_for_all_managed_targets() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::write(
        replica_root.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write replica config");

    let mut expected_content: Option<String> = None;
    for target in [
        ManagedContextTarget::Claude,
        ManagedContextTarget::Antigravity,
        ManagedContextTarget::Codex,
        ManagedContextTarget::Pi,
    ] {
        let materialized =
            materialize_agent_context_file(&path_string(&replica_root), target, false)
                .expect("materialize context")
                .expect("context path");
        assert_eq!(
            Path::new(&materialized)
                .file_name()
                .and_then(|name| name.to_str()),
            Some(target.filename())
        );

        let content = std::fs::read_to_string(materialized).expect("read materialized context");
        assert_eq!(content.matches(WORKGROUP_GIT_SCOPE).count(), 1);
        assert!(!content.contains("`.gitignore`d `.ac/`"));
        assert!(!content.contains("`.ac/` folder, which is `.gitignore`d"));
        assert!(!content.contains("{{GIT_SCOPE}}"));
        assert_no_raw_template_placeholders(&content);

        if let Some(expected) = &expected_content {
            assert_eq!(&content, expected);
        } else {
            expected_content = Some(content);
        }
    }
}

#[test]
fn pre_1072_legacy_git_scope_snapshots_are_byte_exact() {
    use sha2::{Digest, Sha256};

    assert_eq!(LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072.len(), 598);
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072.as_bytes())
        ),
        "db90740fff6b6e44bf2f73fe929e8ec792f3ab5f459350f5e2d612f7151fd050"
    );
    assert_eq!(LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072.len(), 570);
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072.as_bytes())
        ),
        "fe831abf46aa70fdefaecdaa2f0932966b2895a6f709f50b3c5981a07327b2f2"
    );
}

#[test]
fn pre_1072_legacy_with_matrix_classifies_stale_and_heals_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");

    let agent_root = path_string(&replica_root);
    let matrix_root = path_string(&matrix_root);
    let skills_section = render_skills_section(&discover_skill_index(Some(&matrix_root)));
    let legacy = pre_1072_legacy_rendered_default_context_for_compat(
        &agent_root,
        Some(&matrix_root),
        &skills_section,
    );
    assert_eq!(
        legacy
            .matches(LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072)
            .count(),
        1
    );
    assert!(matches!(
        classify_legacy_rendered_default_context(
            &legacy,
            &agent_root,
            Some(&matrix_root),
            &skills_section,
        ),
        LegacyRenderedDefaultContext::StaleGenerated
    ));

    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write pre-1072 template");
    let first = resolve_agent_context(
        &agent_root,
        Some(&matrix_root),
        &skills_section,
        &replica_root,
        None,
        None,
    )
    .expect("resolve pre-1072 context");
    assert_eq!(first.matches(WORKGROUP_GIT_SCOPE).count(), 1);
    assert!(!first.contains(LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072));
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read healed template"),
        get_default_agent_template()
    );
    assert_no_context_template_temp_leftover(&ac_root);

    let second = resolve_agent_context(
        &agent_root,
        Some(&matrix_root),
        &skills_section,
        &replica_root,
        None,
        None,
    )
    .expect("resolve healed context");
    assert_eq!(second, first);
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read converged template"),
        get_default_agent_template()
    );
}

#[test]
fn pre_1072_legacy_without_matrix_classifies_stale_and_heals_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");

    let agent_root = path_string(&matrix_root);
    let skills_section = render_skills_section(&discover_skill_index(Some(&agent_root)));
    let legacy =
        pre_1072_legacy_rendered_default_context_for_compat(&agent_root, None, &skills_section);
    assert_eq!(
        legacy
            .matches(LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072)
            .count(),
        1
    );
    assert!(matches!(
        classify_legacy_rendered_default_context(&legacy, &agent_root, None, &skills_section,),
        LegacyRenderedDefaultContext::StaleGenerated
    ));

    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write pre-1072 template");
    let first =
        resolve_agent_context(&agent_root, None, &skills_section, &matrix_root, None, None)
            .expect("resolve pre-1072 context");
    assert_eq!(first.matches(DIRECT_MATRIX_GIT_SCOPE).count(), 1);
    assert!(!first.contains(LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072));
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read healed template"),
        get_default_agent_template()
    );
    assert_no_context_template_temp_leftover(&ac_root);

    let second =
        resolve_agent_context(&agent_root, None, &skills_section, &matrix_root, None, None)
            .expect("resolve healed context");
    assert_eq!(second, first);
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read converged template"),
        get_default_agent_template()
    );
}

#[test]
fn one_byte_edited_pre_1072_git_scope_remains_custom() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");

    let agent_root = path_string(&replica_root);
    let matrix_root = path_string(&matrix_root);
    let skills_section = render_skills_section(&discover_skill_index(Some(&matrix_root)));
    let legacy = pre_1072_legacy_rendered_default_context_for_compat(
        &agent_root,
        Some(&matrix_root),
        &skills_section,
    );
    let warning_start = legacy
        .find(LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072)
        .expect("old Matrix warning");
    let byte_offset = warning_start
        + LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072
            .find("typically")
            .expect("typically in old warning")
        + 1;
    assert_eq!(legacy.as_bytes()[byte_offset], b'y');
    let mut edited_bytes = legacy.as_bytes().to_vec();
    edited_bytes[byte_offset] = b'z';
    let edited = String::from_utf8(edited_bytes).expect("ASCII edit remains UTF-8");
    assert_eq!(edited.len(), legacy.len());
    assert_eq!(
        edited
            .bytes()
            .zip(legacy.bytes())
            .filter(|(edited, original)| edited != original)
            .count(),
        1
    );
    assert!(matches!(
        classify_legacy_rendered_default_context(
            &edited,
            &agent_root,
            Some(&matrix_root),
            &skills_section,
        ),
        LegacyRenderedDefaultContext::NotLegacy
    ));

    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, edited.as_bytes()).expect("write edited template");
    let rendered = resolve_agent_context(
        &agent_root,
        Some(&matrix_root),
        &skills_section,
        &replica_root,
        None,
        None,
    )
    .expect("resolve edited context");
    assert!(!rendered.is_empty());
    assert_eq!(
        std::fs::read(&template_path).expect("read preserved template"),
        edited.as_bytes()
    );
    assert_no_context_template_temp_leftover(&ac_root);
}
