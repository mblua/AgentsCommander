use super::*;

// #529 - filename-based writer (materialize_agent_context_file_with_filename).

#[test]
fn materialize_with_filename_writes_custom_and_cleans_builtins_and_extra() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    // Pre-existing managed files that must be removed before the new write.
    std::fs::write(matrix_root.join("CLAUDE.md"), "stale claude").expect("write CLAUDE.md");
    std::fs::write(matrix_root.join("AGENTS.md"), "stale agents").expect("write AGENTS.md");
    std::fs::write(matrix_root.join("MyTeam.md"), "stale custom").expect("write MyTeam.md");

    let materialized = materialize_agent_context_file_with_filename(
        &path_string(&matrix_root),
        "Squad.md",
        &["MyTeam.md".to_string()],
        false,
        false,
        None,
    )
    .expect("materialize")
    .expect("context path");

    assert!(materialized.ends_with("Squad.md"));
    assert!(matrix_root.join("Squad.md").is_file());
    assert!(!matrix_root.join("CLAUDE.md").exists());
    assert!(!matrix_root.join("AGENTS.md").exists());
    assert!(
        !matrix_root.join("MyTeam.md").exists(),
        "a configured custom name in the cleanup set must be removed"
    );
    let content = std::fs::read_to_string(matrix_root.join("Squad.md")).expect("read Squad.md");
    assert!(!content.is_empty());
    assert_ne!(content, "stale custom");
}

#[test]
fn materialize_with_filename_orphans_unlisted_custom_name() {
    // R1.6 documented limitation: a custom name NOT in the cleanup set is not
    // removed (a *.md sweep would be too aggressive). This locks the behavior.
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(matrix_root.join("MyTeam.md"), "stale custom").expect("write MyTeam.md");

    materialize_agent_context_file_with_filename(
        &path_string(&matrix_root),
        "Squad.md",
        &[], // MyTeam.md intentionally not listed -> it survives.
        false,
        false,
        None,
    )
    .expect("materialize")
    .expect("context path");

    assert!(
        matrix_root.join("MyTeam.md").is_file(),
        "an unlisted custom name is intentionally orphaned, not swept"
    );
    assert_eq!(
        std::fs::read_to_string(matrix_root.join("MyTeam.md")).expect("read MyTeam.md"),
        "stale custom"
    );
    assert!(matrix_root.join("Squad.md").is_file());
}

#[test]
fn materialize_with_filename_rejects_path_separators() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");

    let err = materialize_agent_context_file_with_filename(
        &path_string(&matrix_root),
        "sub/evil.md",
        &[],
        false,
        false,
        None,
    )
    .expect_err("a filename with separators must be rejected by the writer");
    assert!(err.contains("separators"), "{err}");
}

#[test]
fn materialize_with_filename_refuses_to_write_through_file_symlink() {
    // G1: AGENTS.md is a FILE symlink to an out-of-root target. The writer
    // must remove the link entry and write a fresh regular file, never write
    // THROUGH the link to its target.
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    let outside = temp.path().join("outside-secret.txt");
    std::fs::write(&outside, "SENTINEL").expect("write outside target");
    let link = matrix_root.join("AGENTS.md");

    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(&outside, &link).is_err() {
            return;
        }
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(&outside, &link).is_err() {
            return; // symlink creation can need privilege; skip where unsupported.
        }
    }

    let materialized = materialize_agent_context_file_with_filename(
        &path_string(&matrix_root),
        "AGENTS.md",
        &[],
        false,
        false,
        None,
    )
    .expect("materialize should succeed by replacing the link")
    .expect("context path");

    assert_eq!(
        std::fs::read_to_string(&outside).expect("read outside target"),
        "SENTINEL",
        "the writer must NOT write through the symlink to its out-of-root target"
    );
    let meta = std::fs::symlink_metadata(&link).expect("stat AGENTS.md");
    assert!(
        !is_link_or_reparse(&meta),
        "AGENTS.md must be a regular file after materialize, not a link"
    );
    let content = std::fs::read_to_string(&materialized).expect("read materialized");
    assert_ne!(content, "SENTINEL");
    assert!(!content.is_empty());
}

#[test]
fn materialize_with_filename_replaces_dir_link_without_touching_target() {
    // G1: AGENTS.md is a DIRECTORY symlink/junction (a reparse point). The
    // writer must remove the link entry (not the target tree) and write a
    // regular file. Pre-#529 this bricked the launch (remove_file on a dir
    // failed -> Err -> "context files missing" + rollback).
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    let outside_dir = temp.path().join("outside-dir");
    std::fs::create_dir_all(&outside_dir).expect("create outside dir");
    std::fs::write(outside_dir.join("keep.txt"), "KEEP").expect("write inside outside dir");
    let link = matrix_root.join("AGENTS.md");

    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(&outside_dir, &link).is_err() {
            return;
        }
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(&outside_dir, &link).is_err() {
            return; // dir symlink/junction creation can need privilege.
        }
    }

    materialize_agent_context_file_with_filename(
        &path_string(&matrix_root),
        "AGENTS.md",
        &[],
        false,
        false,
        None,
    )
    .expect("materialize should replace the dir link, not brick the launch")
    .expect("context path");

    assert!(
        outside_dir.join("keep.txt").is_file(),
        "the out-of-root directory and contents must survive (link entry removed, not target)"
    );
    assert_eq!(
        std::fs::read_to_string(outside_dir.join("keep.txt")).expect("read keep"),
        "KEEP"
    );
    let meta = std::fs::symlink_metadata(&link).expect("stat AGENTS.md");
    assert!(!is_link_or_reparse(&meta));
    assert!(meta.is_file());
}

#[test]
fn discover_skill_index_empty_skills_dir_lists_none() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    std::fs::create_dir_all(matrix_root.join(SKILLS_DIR_NAME)).expect("create skills dir");

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert!(index.skills.is_empty());
    assert!(index.warnings.is_empty());
    let rendered = render_skills_section(&index);
    assert!(rendered.contains("No valid skills"));
}

#[test]
fn resolve_skill_owner_root_supports_origin_matrix_sessions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join(".ac").join("_agent_dev-rust");
    write_skill(
        &matrix_root,
        "example",
        "---\nname: example\ndescription: Example skill metadata.\n---\nBody not indexed.\n",
    );

    let owner = resolve_skill_owner_root(&path_string(&matrix_root), None)
        .expect("origin matrix should resolve as skill owner");
    let index = discover_skill_index(Some(&owner));
    assert_eq!(index.skills.len(), 1);
    assert_eq!(index.skills[0].name, "example");
}

#[test]
fn discover_skill_index_valid_skills_are_sorted_and_metadata_rendered() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    write_skill(
        &matrix_root,
        "zeta",
        "---\nname: zeta\ndescription: Zeta description.\nwhen_to_use: Use for zeta tasks.\n---\nZETA_BODY_ONLY\n",
    );
    write_skill(
        &matrix_root,
        "alpha",
        "---\nname: alpha\ndescription: Alpha description.\nwhen_to_use: Use for alpha tasks.\n---\nALPHA_BODY_ONLY\n",
    );

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert_eq!(index.skills.len(), 2);
    assert_eq!(index.skills[0].name, "alpha");
    assert_eq!(index.skills[1].name, "zeta");

    let rendered = render_skills_section(&index);
    let alpha_pos = rendered.find("`alpha`").expect("alpha renders");
    let zeta_pos = rendered.find("`zeta`").expect("zeta renders");
    assert!(alpha_pos < zeta_pos);
    assert!(rendered.contains("Alpha description."));
    assert!(rendered.contains("When to use: Use for alpha tasks."));
    assert!(rendered.contains("Scope: canonical Agent Matrix"));
    assert!(!rendered.contains("ALPHA_BODY_ONLY"));
    assert!(!rendered.contains("ZETA_BODY_ONLY"));
}

#[test]
fn discover_skill_index_missing_skill_md_warns_and_skips() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    std::fs::create_dir_all(matrix_root.join(SKILLS_DIR_NAME).join("no-entry"))
        .expect("create skill dir");

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert!(index.skills.is_empty());
    let rendered = render_skills_section(&index);
    assert!(rendered.contains("missing exact SKILL.md"));
}

#[test]
fn discover_skill_index_wrong_case_skill_md_warns_on_windows_too() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    let skill_dir = matrix_root.join(SKILLS_DIR_NAME).join("wrong-case");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("skill.md"),
        "---\nname: wrong-case\ndescription: Wrong case.\n---\n",
    )
    .expect("write wrong-case skill.md");

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert!(index.skills.is_empty());
    let rendered = render_skills_section(&index);
    assert!(rendered.contains("missing exact SKILL.md"));
}

#[test]
fn discover_skill_index_malformed_frontmatter_warns_and_skips() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    write_skill(
        &matrix_root,
        "bad",
        "name: bad\ndescription: Missing frontmatter delimiter.\n",
    );

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert!(index.skills.is_empty());
    let rendered = render_skills_section(&index);
    assert!(rendered.contains("frontmatter"));
}

#[test]
fn discover_skill_index_missing_description_keeps_skill_with_warning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    write_skill(
        &matrix_root,
        "no-desc",
        "---\nname: no-desc\n---\nBody fallback ignored.\n",
    );

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert_eq!(index.skills.len(), 1);
    assert!(index.skills[0]
        .metadata_warnings
        .iter()
        .any(|warning| warning.contains("description metadata is missing")));
    let rendered = render_skills_section(&index);
    assert!(rendered.contains("No description metadata; inspect SKILL.md before use."));
    assert!(rendered.contains("description metadata is missing"));
    assert!(!rendered.contains("Body fallback ignored"));
}

#[test]
fn discover_skill_index_invalid_name_rejects_without_sanitizing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    write_skill(
        &matrix_root,
        "good-folder",
        "---\nname: Bad Name\ndescription: Valid description.\n---\n",
    );

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert!(index.skills.is_empty());
    assert!(index
        .warnings
        .iter()
        .any(|warning| warning.contains("invalid skill name")));
}

#[test]
fn discover_skill_index_duplicate_names_rejects_later_duplicate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    write_skill(
        &matrix_root,
        "alpha",
        "---\nname: shared\ndescription: Alpha shared.\n---\n",
    );
    write_skill(
        &matrix_root,
        "beta",
        "---\nname: shared\ndescription: Beta shared.\n---\n",
    );

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert_eq!(index.skills.len(), 1);
    assert_eq!(index.skills[0].folder_name, "alpha");
    assert!(index
        .warnings
        .iter()
        .any(|warning| warning.contains("duplicate skill name")));
}

#[test]
fn discover_skill_index_unknown_fields_are_ignored() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    write_skill(
        &matrix_root,
        "portable",
        "---\nname: portable\ndescription: Portable metadata.\nallowed-tools:\n  - Bash\nmodel: opus\nhooks:\n  pre: test\nunknown-key: value\n---\n",
    );

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert_eq!(index.skills.len(), 1);
    assert!(index.warnings.is_empty());
    assert!(index.skills[0].metadata_warnings.is_empty());
    let rendered = render_skills_section(&index);
    assert!(!rendered.contains("allowed-tools"));
    assert!(!rendered.contains("unknown-key"));
    assert!(!rendered.contains("opus"));
}

#[test]
fn discover_skill_index_invalid_description_type_keeps_skill_with_warning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    write_skill(
        &matrix_root,
        "typed",
        "---\nname: typed\ndescription: [bad]\n---\nBody fallback ignored.\n",
    );

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert_eq!(index.skills.len(), 1);
    assert!(index.skills[0]
        .metadata_warnings
        .iter()
        .any(|warning| warning.contains("description must be a string")));
    let rendered = render_skills_section(&index);
    assert!(rendered.contains("No description metadata; inspect SKILL.md before use."));
    assert!(rendered.contains("description must be a string"));
    assert!(!rendered.contains("Body fallback ignored"));
}

#[test]
fn discover_skill_index_frontmatter_size_limit_warns() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    let oversized = format!(
        "---\ndescription: {}\n---\n",
        "a".repeat(SKILL_FRONTMATTER_MAX_BYTES + 20)
    );
    write_skill(&matrix_root, "big", &oversized);

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert!(index.skills.is_empty());
    assert!(index
        .warnings
        .iter()
        .any(|warning| warning.contains("byte limit")));
}

#[test]
fn discover_skill_index_directory_entrypoint_warns_cross_platform() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    let entrypoint_dir = matrix_root
        .join(SKILLS_DIR_NAME)
        .join("broken")
        .join(SKILL_MD_FILENAME);
    std::fs::create_dir_all(&entrypoint_dir).expect("create directory entrypoint");

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert!(index.skills.is_empty());
    assert!(index
        .warnings
        .iter()
        .any(|warning| warning.contains("not a regular file")));
}

#[test]
fn issue_2868_discover_skill_index_skips_linked_skill_dirs_where_supported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join("_agent_dev");
    let skills_root = matrix_root.join(SKILLS_DIR_NAME);
    let target_dir = temp.path().join("outside-skill");
    std::fs::create_dir_all(&skills_root).expect("create skills root");
    std::fs::create_dir_all(&target_dir).expect("create target dir");
    let linked_dir = skills_root.join("linked");

    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(&target_dir, &linked_dir).is_err() {
            eprintln!("UNAVAILABLE issue_2868 linked/dangling source fixture: requires supported exact-head CI; this host is not proof");
            return;
        }
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(&target_dir, &linked_dir).is_err() {
            eprintln!("UNAVAILABLE issue_2868 linked/dangling source fixture: requires supported exact-head CI; this host is not proof");
            return;
        }
    }

    let index = discover_skill_index(Some(&path_string(&matrix_root)));
    assert!(index.skills.is_empty());
    assert!(index
        .warnings
        .iter()
        .any(|warning| warning.contains("linked skill directory")));
    // Reuse the supported pre-existing link fixture; no additional link
    // creation or privileges. Unsupported hosts need exact-head CI evidence.
    let (ac, _, replica, matrix) = make_valid_replica(&temp.path().join("project"));
    let source = ac.join("project-skills");
    std::fs::rename(&linked_dir, &source).unwrap();
    let target_skill = target_dir.join("linked-secret");
    std::fs::create_dir(&target_skill).unwrap();
    std::fs::write(target_skill.join("SKILL.md"), "---\nname: linked-secret\ndescription: LINK_TARGET_METADATA_2868\n---\nLINK_TARGET_BODY_2868\n").unwrap();
    std::fs::write(target_dir.join("sentinel"), "TARGET_SENTINEL").unwrap();
    write_skill(
        Path::new(&matrix),
        "retained",
        "---\nname: retained\ndescription: Agent retained\n---\n",
    );
    for dangling in [false, true] {
        if dangling {
            std::fs::remove_file(target_skill.join("SKILL.md")).unwrap();
            std::fs::remove_dir(&target_skill).unwrap();
            std::fs::remove_file(target_dir.join("sentinel")).unwrap();
            std::fs::remove_dir(&target_dir).unwrap();
        }
        assert_eq!(
            crate::config::shared_locations::ensure_project_skills_dir(&ac)
                .unwrap_err()
                .to_string(),
            "linked/reparse directory is not allowed"
        );
        create_default_context_templates(&ac).unwrap();
        for name in ["plans", "tools", "errors", "project-shared"] {
            assert!(ac.join(name).is_dir());
        }
        assert!(ac.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME).is_file());
        for root in [Path::new(&matrix), replica.as_path()] {
            let content = issue_2868_cache(root);
            issue_2868_assert_permission(&content, &source);
            assert!(content.contains("`retained`"));
            assert!(!content.contains("LINK_TARGET_METADATA_2868"));
            assert!(!content.contains("LINK_TARGET_BODY_2868"));
            assert!(!content.contains(&format!("You MAY READ {}", display_path(&target_dir))));
        }
        assert!(is_link_or_reparse(
            &std::fs::symlink_metadata(&source).unwrap()
        ));
        if !dangling {
            assert_eq!(
                std::fs::read_to_string(target_dir.join("sentinel")).unwrap(),
                "TARGET_SENTINEL"
            );
        }
    }
}

#[test]
fn render_skills_section_truncates_trigger_text() {
    let long_text = "a".repeat(SKILL_TRIGGER_TEXT_MAX_CHARS + 100);
    let index = SkillIndex {
        matrix_root: Some("C:/matrix".to_string()),
        skills_root: Some("C:/matrix/skills".to_string()),
        skills: vec![SkillMetadata {
            folder_name: "long".to_string(),
            name: "long".to_string(),
            entrypoint_path: "C:/matrix/skills/long/SKILL.md".to_string(),
            description: Some(long_text.clone()),
            when_to_use: Some("more text".to_string()),
            metadata_warnings: Vec::new(),
        }],
        warnings: Vec::new(),
    };

    let rendered = render_skills_section(&index);
    assert!(rendered.contains("..."));
    assert!(!rendered.contains(&long_text));
}

#[test]
fn render_skills_section_uses_root_agent_scope_for_root_agent_skills() {
    let index = SkillIndex {
        matrix_root: Some("C:/project/.ac/ac-root-agent".to_string()),
        skills_root: Some("C:/project/.ac/ac-root-agent/skills".to_string()),
        skills: vec![SkillMetadata {
            folder_name: "role-skill-boundary-audit".to_string(),
            name: "role-skill-boundary-audit".to_string(),
            entrypoint_path:
                "C:/project/.ac/ac-root-agent/skills/role-skill-boundary-audit/SKILL.md"
                    .to_string(),
            description: Some("Root skill metadata.".to_string()),
            when_to_use: None,
            metadata_warnings: Vec::new(),
        }],
        warnings: Vec::new(),
    };

    let rendered = render_skills_section(&index);

    assert!(rendered.contains("Scope: Root Agent durable skills"));
    assert!(!rendered.contains("Scope: canonical Agent Matrix"));
}

#[test]
fn render_skills_section_sanitizes_prompt_metadata() {
    let index = SkillIndex {
        matrix_root: Some("C:/matrix".to_string()),
        skills_root: Some("C:/matrix/skills".to_string()),
        skills: vec![SkillMetadata {
            folder_name: "prompt".to_string(),
            name: "prompt".to_string(),
            entrypoint_path: "C:/matrix/skills/prompt/SKILL.md".to_string(),
            description: Some(
                "First line\n# injected heading\n```code fence```\nUse `danger`".to_string(),
            ),
            when_to_use: None,
            metadata_warnings: Vec::new(),
        }],
        warnings: Vec::new(),
    };

    let rendered = render_skills_section(&index);
    assert!(!rendered.contains("\n# injected heading"));
    assert!(!rendered.contains("```code fence```"));
    assert!(!rendered.contains("`danger`"));
    assert!(rendered.contains("First line # injected heading '''code fence''' Use 'danger'"));
}

#[test]
fn render_skills_section_caps_total_budget() {
    let mut skills = Vec::new();
    for idx in 0..2000 {
        skills.push(SkillMetadata {
            folder_name: format!("skill-{}", idx),
            name: format!("skill-{}", idx),
            entrypoint_path: format!("C:/matrix/skills/skill-{}/SKILL.md", idx),
            description: Some("x".repeat(2048)),
            when_to_use: None,
            metadata_warnings: Vec::new(),
        });
    }
    let warnings = (0..2000)
        .map(|idx| format!("warning {} {}", idx, "y".repeat(80)))
        .collect();
    let index = SkillIndex {
        matrix_root: Some("C:/matrix".to_string()),
        skills_root: Some("C:/matrix/skills".to_string()),
        skills,
        warnings,
    };

    let rendered = render_skills_section(&index);
    assert!(rendered.len() <= SKILL_INDEX_TOTAL_MAX_BYTES);
    assert!(rendered.contains("budget reached"));
    assert!(rendered.contains("omitted"));
}

#[test]
fn materialize_agent_context_file_includes_skills_for_replica_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    write_skill(
        &matrix_root,
        "runtime",
        "---\nname: runtime\ndescription: Runtime skill metadata.\n---\nBODY_SHOULD_NOT_RENDER\n",
    );
    std::fs::write(
        replica_root.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
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
    assert_eq!(content.matches("## GOLDEN RULE").count(), 1);
    assert_eq!(content.matches("## Skills").count(), 1);
    assert_eq!(content.matches("# Agent Repos").count(), 1);
    assert!(content.contains("runtime"));
    assert!(content.contains("Runtime skill metadata."));
    assert_contains_canonical_path(&content, &matrix_root.join("skills"));
    assert_contains_canonical_path(
        &content,
        &matrix_root.join("skills").join("runtime").join("SKILL.md"),
    );
    assert!(content.contains("Role.md"));
    assert!(content.contains("filename ONLY"));
    assert_no_raw_template_placeholders(&content);
    assert_no_broad_read_grant(&content);
    assert!(!content.contains("BODY_SHOULD_NOT_RENDER"));
}
