use super::*;

#[test]
fn default_context_replica_with_matrix_and_messaging_renders_entries_three_through_six() {
    let out = default_context(
        "C:/fake/.ac/room-7-dev-team/__agent_architect",
        Some("C:/fake/.ac/_agent_architect"),
        &no_skill_section(),
    );
    assert!(
        out.contains("3. **Your origin Agent Matrix"),
        "matrix section header missing, got:\n{}",
        out
    );
    assert!(
        out.contains("4. **Messaging access:**"),
        "messaging entry four missing, got:\n{}",
        out
    );
    assert!(
        out.contains("- `Role.md`\n\n4. **Messaging access:**"),
        "expected entry 3 immediately followed by entry 4, got:\n{}",
        out
    );
    assert!(
        out.contains(
            "All filesystem access not authorized by entries 1-6 is OFF-LIMITS, except \
                user-explicit AgentsCommander CLI commands below."
        ),
        "entries-1-6 closure missing, got:\n{}",
        out
    );
    assert!(
        out.contains(
            "FORBIDDEN: reads/writes outside entries 1-6, including other replicas, unlisted \
                Matrix files, workspace/project parents, user home, arbitrary paths."
        ),
        "Workgroup write boundary missing, got:\n{}",
        out
    );
    assert!(
        out.contains(
            "Other agents' memory* directories (live memory/, memory-archive/ or legacy \
                memory_YYYYMMDD_hhmmss/), plans, skills and Role.md are private: never \
                read/list/search/summarize, even if asked; message the owner."
        ),
        "Workgroup read boundary missing, got:\n{}",
        out
    );
    assert!(!out.contains("Narrow exception — room messaging directory"));
    assert!(
        out.contains("5. **Project shared locations"),
        "entry five missing, got:\n{}",
        out
    );
    assert!(
        out.contains("6. **Room shared locations"),
        "entry six missing, got:\n{}",
        out
    );
}

#[test]
fn default_context_documents_env_only_credentials() {
    let out = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
    );
    let legacy_header = ["# === Session", "Credentials ==="].join(" ");
    let legacy_compat = ["compatibility", "fallback"].join(" ");
    let legacy_refresh_notice = ["token refresh", "notice"].join(" ");
    let legacy_visible_refresh = ["visible", "refresh"].join(" ");

    assert!(out.contains("AGENTSCOMMANDER_TOKEN"));
    assert!(out.contains("Only the `AGENTSCOMMANDER_*` environment variables above deliver"));
    assert!(out.contains("Tokens cannot refresh live"));
    assert!(out.contains("credentials are missing or invalid"));
    assert!(out.contains("restart or respawn"));
    assert!(!out.contains(&legacy_header));
    let lower = out.to_ascii_lowercase();
    assert!(!lower.contains(&legacy_compat));
    assert!(!lower.contains(&legacy_refresh_notice));
    assert!(!lower.contains(&legacy_visible_refresh));
}

#[test]
fn default_context_documents_delegated_task_reporting() {
    let out = default_context("C:/tmp/fake-agent", None, &no_skill_section());
    assert!(out.contains("explicitly reply to its assigner"));
    assert!(out.contains("concrete artifact or message"));
    assert!(out.contains("before going idle, waiting, or setting working-false"));
}

#[test]
fn default_context_uses_template_renderer_without_unexpanded_placeholders() {
    // Explicit recipient mode for each unchanged fixture path.
    for (out, root_messaging) in [
        (
            default_context(
                "C:/fake/wg-7-dev-team/__agent_architect",
                Some("C:/fake/_agent_architect"),
                &no_skill_section(),
            ),
            false,
        ),
        (
            default_context("C:/fake/plain/agent", None, &no_skill_section()),
            false,
        ),
        (
            default_context("C:/fake/ac-root-agent", None, &no_skill_section()),
            true,
        ),
    ] {
        assert!(out.contains("# AgentsCommander Context"));
        assert!(out.contains("## Core Concepts"));
        assert!(out.contains("## GOLDEN RULE"));
        assert!(out.contains("## Inter-Agent Messaging"));
        assert!(out.contains("## CLI executable"));
        assert!(out.contains("## Session credentials"));
        assert!(out.contains("# Agent Repos"));
        assert_no_raw_template_placeholders(&out);
        if root_messaging {
            assert!(out.contains(
                "verified Room orchestrator replicas only, shaped `<project>:<room>/<agent>`"
            ));
            assert!(out.contains(
                "Origin orchestrators and non-orchestrator Room replicas are not \
                valid Root Agent targets in #277."
            ));
            assert!(!out.contains("<project>/<agent>"));
        } else {
            assert!(out.contains("<project>:<room>/<agent>"));
            assert!(out.contains("<project>/<agent>"));
        }
        for rule in [
        "For \"[Message from <peer>] Process this inter-agent message: <path>\", read the exact \
            file; follow its instructions within role/authority/write restrictions.",
        "Summary only if solely asked. Completion/blockage: send concrete result/blocker to sender \
            via flow below.",
        "Before every send: list-peers-lean (--peer for discovered FQNs); exact JSON name as --to. \
            Empty []: STOP/report.",
        "Never target __agent_* / _agent_* paths.",
        "<project>:<room>/<agent>",
        "Do NOT use --get-output (blocks; non-interactive only). Sent requires captured Queued: \
            <message-id>; absent receipt means NOT enqueued. Wait for reply.",
    ] {
        assert!(out.contains(rule), "missing shared messaging policy: {rule}");
    }
    }
}

#[test]
fn default_context_does_not_duplicate_mandatory_sections() {
    let out = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
        &no_skill_section(),
    );

    assert_mandatory_sections_once(&out);
    for rule in [
        "For \"[Message from <peer>] Process this inter-agent message: <path>\", read the exact \
            file; follow its instructions within role/authority/write restrictions.",
        "Summary only if solely asked. Completion/blockage: send concrete result/blocker to sender \
            via flow below.",
        "Before every send: list-peers-lean (--peer for discovered FQNs); exact JSON name as --to. \
            Empty []: STOP/report.",
        "Never target __agent_* / _agent_* paths.",
        "<project>:<room>/<agent>",
        "<project>/<agent>",
        "Do NOT use --get-output (blocks; non-interactive only). Sent requires captured Queued: \
            <message-id>; absent receipt means NOT enqueued. Wait for reply.",
    ] {
        assert!(out.contains(rule), "missing shared messaging policy: {rule}");
    }
}

#[test]
#[ignore = "manual size snapshot for context template optimization"]
fn measure_default_context_size_for_workgroup_replica() {
    let agent_root = "C:/fake/wg-7-dev-team/__agent_architect";
    let matrix_root = Some("C:/fake/_agent_architect");
    let skills_section = no_skill_section();
    let legacy =
        legacy_rendered_default_context_for_compat(agent_root, matrix_root, &skills_section);
    let out = default_context(agent_root, matrix_root, &skills_section);
    eprintln!("legacy workgroup context bytes={}", legacy.len());
    eprintln!("default workgroup context bytes={}", out.len());
    eprintln!(
        "default workgroup context savings_bytes={}",
        legacy.len() as isize - out.len() as isize
    );
}

#[test]
fn custom_agent_template_is_used_for_wg_replica() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::create_dir_all(&ac_root).expect("create workspace dir");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        "root={{AGENT_ROOT}}\n{{MATRIX_SECTION}}\n{{MESSAGING_EXCEPTION}}\n{{SKILLS_SECTION}}",
    )
    .expect("write custom agent template");
    write_skill(
        &matrix_root,
        "templated",
        "---\nname: templated\ndescription: Template skill.\n---\n",
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

    assert!(content.contains(&format!("root={}", canonical_display_path(&replica_root))));
    assert!(content.contains("3. **Your origin Agent Matrix"));
    assert!(content.contains("4. **Messaging access:**"));
    assert!(!content.contains("Narrow exception — room messaging directory"));
    assert!(content.contains("Template skill."));
    for rule in [
        "For \"[Message from <peer>] Process this inter-agent message: <path>\", read the exact \
            file; follow its instructions within role/authority/write restrictions.",
        "Summary only if solely asked. Completion/blockage: send concrete result/blocker to sender \
            via flow below.",
        "Before every send: list-peers-lean (--peer for discovered FQNs); exact JSON name as --to. \
            Empty []: STOP/report.",
        "Never target __agent_* / _agent_* paths.",
        "<project>:<room>/<agent>",
        "<project>/<agent>",
        "Do NOT use --get-output (blocks; non-interactive only). Sent requires captured Queued: \
            <message-id>; absent receipt means NOT enqueued. Wait for reply.",
    ] {
        assert!(content.contains(rule), "missing shared messaging policy: {rule}");
    }
}

#[test]
fn legacy_agent_template_is_migrated_to_global_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    let legacy_path = ac_root.join(LEGACY_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let new_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&legacy_path, "LEGACY_BODY {{AGENT_ROOT}}").expect("write legacy template");
    let mut publications = Vec::new();

    let migrated = read_or_create_context_template_with_publications(
        &path_string(&matrix_root),
        GLOBAL_CONTEXT_TEMPLATE_FILENAME,
        get_default_agent_template(),
        &mut |filename, publication| publications.push((filename, publication)),
    )
    .expect("migrate legacy template")
    .expect("migrated content");

    assert_eq!(migrated, "LEGACY_BODY {{AGENT_ROOT}}");
    assert!(
        publications.is_empty(),
        "the legacy hard-link migration must remain unrecorded"
    );

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(content.contains("LEGACY_BODY"));
    assert_contains_canonical_path(&content, &matrix_root);
    assert!(new_path.is_file());
    assert!(!legacy_path.exists());
    assert_eq!(
        std::fs::read_to_string(new_path).expect("read migrated template"),
        "LEGACY_BODY {{AGENT_ROOT}}"
    );
}

#[test]
fn missing_global_template_placeholders_are_force_appended() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        "CUSTOM_ONLY {{WRITE_RESTRICTIONS}}",
    )
    .expect("write partial template");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(content.contains("CUSTOM_ONLY"));
    for rule in [
        "For \"[Message from <peer>] Process this inter-agent message: <path>\", read the exact \
            file; follow its instructions within role/authority/write restrictions.",
        "Summary only if solely asked. Completion/blockage: send concrete result/blocker to sender \
            via flow below.",
        "Before every send: list-peers-lean (--peer for discovered FQNs); exact JSON name as --to. \
            Empty []: STOP/report.",
        "Never target __agent_* / _agent_* paths.",
        "<project>:<room>/<agent>",
        "<project>/<agent>",
        "Do NOT use --get-output (blocks; non-interactive only). Sent requires captured Queued: \
            <message-id>; absent receipt means NOT enqueued. Wait for reply.",
    ] {
        assert!(content.contains(rule), "missing shared messaging policy: {rule}");
    }

    assert!(content.contains("## GOLDEN RULE"));
    assert!(content.contains("## Delegated Task Reporting"));
    assert!(content.contains("## Skills"));
    assert!(content.contains("# Agent Repos"));
    assert!(content.contains("## CLI executable"));
    assert!(content.contains("## Session credentials"));
    assert!(content.contains("## Inter-Agent Messaging"));
    for placeholder in MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS {
        assert!(
            !content.contains(placeholder),
            "placeholder {placeholder} should be rendered"
        );
    }
}

#[test]
fn legacy_rendered_default_template_is_not_fallback_appended() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");

    let agent_root = path_string(&replica_root);
    let matrix_root = path_string(&matrix_root);
    let legacy = legacy_rendered_default_context_for_compat(
        &agent_root,
        Some(&matrix_root),
        &no_skill_section(),
    );
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), &legacy)
        .expect("write legacy rendered template");

    let rendered = resolve_agent_context(
        &agent_root,
        Some(&matrix_root),
        &no_skill_section(),
        &replica_root,
        None,
        None,
    )
    .expect("resolve context");

    assert_eq!(rendered, legacy);
    assert_eq!(count_context_occurrences(&rendered, "## GOLDEN RULE"), 1);
}

#[test]
fn stale_generated_legacy_default_for_other_wg_replica_regenerates_current_context() {
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
    std::fs::write(
        new_replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write replica config");

    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), &legacy)
        .expect("write stale generated default");

    let materialized = materialize_agent_context_file(
        &path_string(&new_replica),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_contains_canonical_path(&content, &new_replica);
    assert_contains_canonical_path(&content, &new_matrix);
    assert!(!content.contains(&canonical_display_path(&old_replica)));
    assert!(!content.contains(&canonical_display_path(&old_matrix)));
    assert_mandatory_sections_once(&content);
    assert_no_raw_template_placeholders(&content);
}

#[test]
fn stale_generated_legacy_default_with_generated_skills_regenerates_current_context() {
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
    std::fs::write(
        new_replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write replica config");
    write_skill(
        &old_matrix,
        "legacy-skill",
        "---\nname: legacy-skill\ndescription: Legacy skill description.\nwhen_to_use: Use legacy \
            contexts.\n---\nLegacy skill body.\n",
    );

    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), &legacy)
        .expect("write stale generated default");

    let materialized = materialize_agent_context_file(
        &path_string(&new_replica),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_contains_canonical_path(&content, &new_replica);
    assert_contains_canonical_path(&content, &new_matrix);
    assert!(!content.contains(&canonical_display_path(&old_replica)));
    assert!(!content.contains(&canonical_display_path(&old_matrix)));
    assert!(!content.contains("Legacy skill description."));
    assert!(!content.contains("legacy-skill"));
    assert_mandatory_sections_once(&content);
    assert_no_raw_template_placeholders(&content);
}

#[test]
fn edited_legacy_skills_section_preserves_custom_template_content() {
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
    std::fs::write(
        new_replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write replica config");

    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    let edited = legacy.replace(
        "No valid skills with parseable SKILL.md frontmatter were discovered.",
        "No valid skills with parseable SKILL.md frontmatter were \
            discovered.\nKEEP_CUSTOM_SKILLS_RULE_IN_CONTEXT",
    );
    assert_ne!(edited, legacy);
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), edited)
        .expect("write edited rendered legacy template");

    let materialized = materialize_agent_context_file(
        &path_string(&new_replica),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_eq!(
        count_context_occurrences(&content, "KEEP_CUSTOM_SKILLS_RULE_IN_CONTEXT"),
        1
    );
    assert_contains_canonical_path(&content, &new_replica);
    assert_contains_canonical_path(&content, &new_matrix);
    assert!(content.contains("## GOLDEN RULE"));
    assert!(content.contains("## Skills"));
    assert!(content.contains("## Inter-Agent Messaging"));
}

#[test]
fn generated_shaped_manual_legacy_skills_content_is_preserved() {
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
    std::fs::write(
        new_replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write replica config");

    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    let manual_skills = format!(
        "{}Canonical skills root: `{}`\n\nWhen running from a workgroup replica, resolve \
            skills/... against the origin Agent Matrix path above, not against the replica \
            CWD.\n\n### Available Skills\n\n- `manual-ops-rule` - \
            KEEP_MANUAL_SKILLS_RULE_IN_CONTEXT\n  Scope: canonical Agent Matrix\n  Entrypoint: \
            `C:/notes/manual-rule/SKILL.md`\n\n### Skill Discovery Warnings\n\n- Skipped skill \
            `manual-warning`: KEEP_MANUAL_WARNING_IN_CONTEXT",
        GENERATED_SKILLS_SECTION_INTRO,
        canonical_display_path(&old_matrix.join(SKILLS_DIR_NAME))
    );
    let edited = legacy.replace(&old_skills_section, &manual_skills);
    assert_ne!(edited, legacy);
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), edited)
        .expect("write edited rendered legacy template");

    let materialized = materialize_agent_context_file(
        &path_string(&new_replica),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_eq!(
        count_context_occurrences(&content, "KEEP_MANUAL_SKILLS_RULE_IN_CONTEXT"),
        1
    );
    assert_eq!(
        count_context_occurrences(&content, "KEEP_MANUAL_WARNING_IN_CONTEXT"),
        1
    );
    assert_contains_canonical_path(&content, &new_replica);
    assert_contains_canonical_path(&content, &new_matrix);
    assert!(content.contains("## GOLDEN RULE"));
    assert!(content.contains("## Skills"));
    assert!(content.contains("## Inter-Agent Messaging"));
}

#[test]
fn edited_legacy_rendered_default_preserves_custom_template_content() {
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
    std::fs::write(
        new_replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write replica config");

    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &no_skill_section(),
    );
    let edited = format!("{legacy}\n\n## Project Rules\n\nKEEP_CUSTOM_PROJECT_RULES_IN_CONTEXT\n");
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &edited).expect("write edited rendered legacy template");

    let materialized = materialize_agent_context_file(
        &path_string(&new_replica),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(content.contains("KEEP_CUSTOM_PROJECT_RULES_IN_CONTEXT"));
    assert_contains_canonical_path(&content, &new_replica);
    assert_contains_canonical_path(&content, &new_matrix);
    assert!(content.contains("## GOLDEN RULE"));
    assert!(content.contains("## Inter-Agent Messaging"));
    // #664: this template is NotLegacy (unknown `## Project Rules` heading),
    // so the heal must NOT touch it. The on-disk bytes are unchanged.
    let on_disk = std::fs::read_to_string(&template_path).expect("read template");
    assert_eq!(on_disk, edited);
}

#[test]
fn inline_edited_legacy_rendered_default_preserves_custom_template_content() {
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
    std::fs::write(
        new_replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write replica config");

    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &no_skill_section(),
    );
    let edited = legacy.replace(
        "Your agent root is your current working directory.",
        "Your agent root is your current working directory.\n\nKEEP_INLINE_CUSTOM_RULE_IN_CONTEXT",
    );
    assert_ne!(edited, legacy);
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &edited).expect("write inline edited rendered legacy template");

    let materialized = materialize_agent_context_file(
        &path_string(&new_replica),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(content.contains("KEEP_INLINE_CUSTOM_RULE_IN_CONTEXT"));
    assert_contains_canonical_path(&content, &new_replica);
    assert_contains_canonical_path(&content, &new_matrix);
    assert!(content.contains("## GOLDEN RULE"));
    assert!(content.contains("## Inter-Agent Messaging"));
    // #664: this template is NotLegacy via reconstruction inequality (no
    // unknown heading), the stronger preserve guard. The heal must NOT touch
    // it: the on-disk bytes are unchanged.
    let on_disk = std::fs::read_to_string(&template_path).expect("read template");
    assert_eq!(on_disk, edited);
}

#[test]
fn stale_generated_legacy_default_for_direct_matrix_regenerates_current_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let old_matrix = ac_root.join("_agent_dev-rust");
    let target_matrix = ac_root.join("_agent_architect");
    std::fs::create_dir_all(&old_matrix).expect("create old matrix");
    std::fs::create_dir_all(&target_matrix).expect("create target matrix");

    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_matrix),
        None,
        &old_skills_section,
    );
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), &legacy)
        .expect("write stale generated default");

    let materialized = materialize_agent_context_file(
        &path_string(&target_matrix),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_contains_canonical_path(&content, &target_matrix);
    assert!(!content.contains(&canonical_display_path(&old_matrix)));
    assert_mandatory_sections_once(&content);
    assert_no_raw_template_placeholders(&content);
}

#[test]
fn stale_generated_legacy_default_heals_on_disk_and_converges() {
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

    // Bake the legacy with the matrix-owner skills section so the
    // reconstruction recognizer (which re-derives the owner from the file's
    // extracted paths) classifies it as StaleGenerated, mirroring the
    // existing stale-generated tests.
    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    assert!(!legacy.contains("### Incoming Message Notifications"));
    assert!(!legacy.contains("Process this inter-agent message"));
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write stale generated default");

    let new_replica_root = path_string(&new_replica);
    let new_matrix_root = path_string(&new_matrix);

    // First resolve: classifies StaleGenerated, returns the current render
    // for the NEW agent AND heals the on-disk template as a side effect.
    let rendered = resolve_agent_context(
        &new_replica_root,
        Some(&new_matrix_root),
        &no_skill_section(),
        &new_replica,
        None,
        None,
    )
    .expect("resolve context");
    assert!(rendered.contains("### Incoming Message Notifications"));
    assert!(rendered.contains("Process this inter-agent message"));

    // (a) the returned render is the healthy current default for the NEW
    // agent: sections once, no placeholders, the new path baked in, the old
    // path gone.
    assert_mandatory_sections_once(&rendered);
    assert_no_raw_template_placeholders(&rendered);
    assert!(rendered.contains(&new_replica_root));
    assert!(!rendered.contains(&path_string(&old_replica)));

    // (b) the on-disk template is healed to the current tokenized default,
    // byte-for-byte. This assertion runs on Windows (G6): it is the guard
    // for the success-path drop-before-replace (G1) and the ReplaceFileW
    // publish.
    let healed = std::fs::read_to_string(&template_path).expect("read healed template");
    assert_eq!(healed, get_default_agent_template());

    // (d) no scratch temp file is left behind.
    assert_no_context_template_temp_leftover(&ac_root);

    // (c) a SECOND resolve reads the healed file, classifies NotLegacy, and
    // returns the same correct render (convergence in exactly one heal).
    let rendered_again = resolve_agent_context(
        &new_replica_root,
        Some(&new_matrix_root),
        &no_skill_section(),
        &new_replica,
        None,
        None,
    )
    .expect("resolve context again");
    assert_eq!(rendered_again, rendered);
    let after_second = std::fs::read_to_string(&template_path).expect("read template again");
    assert_eq!(after_second, get_default_agent_template());
}

#[test]
fn stale_global_self_heal_returns_commit_point_publication() {
    use crate::config::seeded_context_templates::{ContextPublication, TemplatePublication};

    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let old_matrix_root = ac_root.join("_agent_dev-rust");
    let new_matrix_root = ac_root.join("_agent_tech-lead");
    let old_replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let new_replica_root = ac_root.join("wg-19-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&old_matrix_root).expect("create old matrix root");
    std::fs::create_dir_all(&new_matrix_root).expect("create new matrix root");
    std::fs::create_dir_all(&old_replica_root).expect("create old replica root");
    std::fs::create_dir_all(&new_replica_root).expect("create new replica root");
    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix_root))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica_root),
        Some(&path_string(&old_matrix_root)),
        &old_skills_section,
    );
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, legacy).expect("write stale generated default");
    let expected = fixed_publication_time();
    let mut clock = || expected;

    let execution = heal_stale_global_context_template_with_clock(
        &path_string(&new_replica_root),
        Some(&path_string(&new_matrix_root)),
        &no_skill_section(),
        &mut clock,
    );

    let publication = ContextPublication {
        published_at: expected,
    };
    assert_eq!(execution.published, Some(publication));
    assert_eq!(
        execution.completion.expect("self-heal completion"),
        TemplatePublication::Published(publication)
    );
    assert_eq!(
        std::fs::read_to_string(template_path).expect("read healed target"),
        get_default_agent_template()
    );
}

// #1065 Stage F activation coverage: a stale-generated global self-heal records
// its commit-point publication into the seed manifest under the gate. Removing the
// `record_project_context_publication` call from `heal_stale_global_recorded` (the
// adapter) would leave no manifest and fail this test (plan acceptance item 22).
#[test]
fn self_heal_records_to_the_seed_manifest_under_the_gate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let old_matrix_root = ac_root.join("_agent_dev-rust");
    let old_replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let new_replica_root = ac_root.join("wg-19-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&old_matrix_root).expect("create old matrix root");
    std::fs::create_dir_all(&old_replica_root).expect("create old replica root");
    std::fs::create_dir_all(&new_replica_root).expect("create new replica root");
    let old_skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix_root))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica_root),
        Some(&path_string(&old_matrix_root)),
        &old_skills_section,
    );
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), legacy)
        .expect("write stale generated default");

    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let execution = heal_stale_global_recorded(
        &path_string(&new_replica_root),
        None,
        &no_skill_section(),
        Some(&token),
    );
    assert!(
        execution.published.is_some(),
        "the self-heal target publishes at its commit point"
    );

    let manifest = std::fs::read_to_string(ac_root.join("seed-manifest.toml"))
        .expect("the self-heal publication is recorded under the gate");
    assert!(
        manifest.contains("scope = \"context:agentscommander\""),
        "manifest: {manifest}"
    );
}

#[test]
fn current_tokenized_default_on_disk_is_not_rewritten() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");

    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, get_default_agent_template()).expect("write tokenized default");

    let rendered = resolve_agent_context(
        &path_string(&replica_root),
        Some(&path_string(&matrix_root)),
        &no_skill_section(),
        &replica_root,
        None,
        None,
    )
    .expect("resolve context");
    assert_mandatory_sections_once(&rendered);
    assert_no_raw_template_placeholders(&rendered);

    // The tokenized default is NotLegacy (it carries `{{` tokens), so the
    // NotLegacy arm renders without ever rewriting the on-disk template.
    let on_disk = std::fs::read_to_string(&template_path).expect("read template");
    assert_eq!(on_disk, get_default_agent_template());
    assert_no_context_template_temp_leftover(&ac_root);
}

#[test]
fn root_never_reads_creates_syncs_or_heals_a_ac_ancestor_global() {
    let temp = tempfile::tempdir().unwrap();
    let ac = temp.path().join(".ac");
    let root = ac.join("room-1-demo").join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    let sentinel = ac.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&sentinel, "PROJECT_GLOBAL_SENTINEL {{AGENT_ROOT}}\n").unwrap();
    std::fs::write(root.join("config.json"), r#"{"context":[]}"#).unwrap();
    let cache =
        build_root_agent_context_at(&path_string(&root), root.parent().unwrap(), None, false)
            .unwrap();
    assert_eq!(std::fs::read_to_string(cache).unwrap(), "");
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "PROJECT_GLOBAL_SENTINEL {{AGENT_ROOT}}\n"
    );
    std::fs::remove_file(&sentinel).unwrap();
    build_root_agent_context_at(&path_string(&root), root.parent().unwrap(), None, false).unwrap();
    for filename in [
        GLOBAL_CONTEXT_TEMPLATE_FILENAME,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        HOST_PLATFORM_RULES_FILENAME_WINDOWS,
        HOST_PLATFORM_RULES_FILENAME_LINUX,
        HOST_PLATFORM_RULES_FILENAME_MACOS,
    ] {
        assert!(!ac.join(filename).exists());
    }
}
