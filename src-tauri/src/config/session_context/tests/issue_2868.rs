use super::*;

/// #1795 (plan 8.0). A replica that `resolve_replica_matrix_root` accepts.
/// Returns the tempdir-derived `.ac` root, the room root, the replica root and
/// the matrix root, each canonicalized and rendered with `display_path`, exactly
/// as production does at `:992`.
///
/// A bare directory is not enough: `:163` runs `resolve_replica_matrix_root`,
/// which for any `__agent_*` basename reads `config.json` with
/// `allow_create = false` and returns `Err` when it is absent, and the `?` there
/// turns that into a failed session.
fn issue_2868_team(ac: &Path, name: &str, config: serde_json::Value) -> PathBuf {
    let root = ac.join(format!("_team_{name}"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.json"), config.to_string()).unwrap();
    root
}

fn issue_2868_team_skill(root: &Path, name: &str) -> PathBuf {
    let entry = root.join("team-skills").join(name).join("SKILL.md");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(
        &entry,
        format!("---\nname: {name}\ndescription: Team metadata\n---\nTEAM_BODY_SECRET\n"),
    )
    .unwrap();
    entry
}

#[test]
fn issue_2868_team_final_cache_option_a_custom_override_refresh_and_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    let origin = Path::new(&matrix);
    write_skill(
        origin,
        "shared",
        "---\nname: shared\ndescription: Agent\n---\n",
    );
    issue_2868_write_project_skill(
        &ac,
        "shared",
        "---\nname: shared\ndescription: Project\n---\n",
    );
    let current = issue_2868_team(
        &ac,
        "dev-team",
        serde_json::json!({"agents":["../_agent_dev-rust"]}),
    );
    let other = issue_2868_team(
        &ac,
        "other-team",
        serde_json::json!({"coordinator": matrix}),
    );
    let current_entry = issue_2868_team_skill(&current, "shared");
    issue_2868_team_skill(&other, "shared");
    let nonmember = issue_2868_team(
        &ac,
        "outsiders",
        serde_json::json!({"agents":["_agent_missing"]}),
    );
    issue_2868_team_skill(&nonmember, "excluded");
    let custom = ac.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let override_path = ac.join("Context.AgentsCommander.local.md");
    let wg = ac.join("wg-20-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(
        wg.join("config.json"),
        "{\"identity\":\"../../_agent_dev-rust\"}",
    )
    .unwrap();
    for override_on in [false, true] {
        std::fs::write(&custom, "CUSTOM_TEAM_CONTEXT\n").unwrap();
        if override_on {
            std::fs::write(&override_path, "OVERRIDE_TEAM_CONTEXT\n").unwrap();
        }
        for root in [origin, replica.as_path(), wg.as_path()] {
            let content = issue_2868_cache(root);
            assert!(content.contains(if override_on {
                "OVERRIDE_TEAM_CONTEXT"
            } else {
                "CUSTOM_TEAM_CONTEXT"
            }));
            assert_eq!(content.matches("## Team Skills").count(), 1);
            assert!(content.contains("`shared`"));
            assert!(content.contains("`project:shared`"));
            assert!(content.contains("`team:dev-team:shared`"));
            assert_eq!(content.contains("`team:other-team:shared`"), root == origin);
            assert!(!content.contains("team:outsiders:"));
            assert!(!content.contains("TEAM_BODY_SECRET"));
            assert!(content.contains(&display_path(&canonical_or_original(&current_entry))));
            let grant = format!(
                "Filesystem authorization amendment: You MAY READ {} and its descendants",
                display_path(&canonical_or_original(&current.join("team-skills")))
            );
            assert_eq!(content.matches(&grant).count(), 1);
            assert!(content.contains("This amendment grants no write permission and no access to external link/reference targets"));
            assert_eq!(
                issue_2868_cache(root),
                content,
                "refresh replaces rather than duplicates catalogs"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&custom).unwrap(),
            "CUSTOM_TEAM_CONTEXT\n"
        );
        if override_on {
            assert_eq!(
                std::fs::read_to_string(&override_path).unwrap(),
                "OVERRIDE_TEAM_CONTEXT\n"
            );
        }
    }
    assert_eq!(
        replica_skill_team("C:/project/.ac/room-14-ac-dev-team-v4/__agent_x"),
        Some("ac-dev-team-v4")
    );
    assert_eq!(
        replica_skill_team("/project/.ac/wg-2-a-b/__agent_x"),
        Some("a-b")
    );
    assert_eq!(replica_skill_team("/project/.ac/room-x-a/__agent_x"), None);
}

#[test]
fn issue_2868_team_empty_invalid_configs_membership_and_occupancy() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    let team = issue_2868_team(
        &ac,
        "dev-team",
        serde_json::json!({"agents":["_agent_dev-rust"]}),
    );
    let content = issue_2868_cache(&replica);
    assert!(team.join("team-skills").is_dir());
    assert!(content.contains("Canonical team skills root:"));
    let foreign = ac.join("_agent_foreign");
    std::fs::create_dir(&foreign).unwrap();
    assert!(!issue_2868_cache(&foreign).contains("## Team Skills"));
    for config in ["malformed", "{}"] {
        std::fs::write(team.join("config.json"), config).unwrap();
        assert!(!issue_2868_cache(&replica).contains("## Team Skills"));
    }
    std::fs::remove_file(team.join("config.json")).unwrap();
    assert!(!accept_skill_team(&team));
    std::fs::create_dir(team.join("config.json")).unwrap();
    assert!(!accept_skill_team(&team));
    std::fs::remove_dir(team.join("config.json")).unwrap();
    std::fs::write(
        team.join("config.json"),
        serde_json::json!({"agents":[matrix]}).to_string(),
    )
    .unwrap();
    for name in ["bad_name", "bad.name"] {
        let invalid = issue_2868_team(&ac, name, serde_json::json!({"agents":[matrix]}));
        assert!(!accept_skill_team(&invalid));
        assert!(!invalid.join("team-skills").exists());
    }
    let valid = issue_2868_team_skill(&team, "valid");
    let bad = team.join("team-skills/bad/SKILL.md");
    std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
    std::fs::write(bad, "---\nname: [bad\n---\n").unwrap();
    let content = issue_2868_cache(&replica);
    assert!(content.contains("team:dev-team:valid"));
    assert!(content.contains("Warning [team-skills:dev-team]"));
    std::fs::remove_file(valid).unwrap();
    let occupied = issue_2868_team(&ac, "occupied", serde_json::json!({"agents":[matrix]}));
    std::fs::write(occupied.join("team-skills"), "KEEP_SENTINEL").unwrap();
    assert!(!issue_2868_cache(Path::new(&matrix)).contains("### Team occupied"));
    assert_eq!(
        std::fs::read_to_string(occupied.join("team-skills")).unwrap(),
        "KEEP_SENTINEL"
    );
    let other_ac = temp.path().join("other-project/.ac");
    let same_name = other_ac.join("_agent_dev-rust");
    std::fs::create_dir_all(&same_name).unwrap();
    let other_team = issue_2868_team(
        &other_ac,
        "dev-team",
        serde_json::json!({"agents":[matrix]}),
    );
    issue_2868_team_skill(&other_team, "foreign");
    assert!(!issue_2868_cache(&same_name).contains("team:dev-team:foreign"));
    let normalized = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(&matrix).unwrap(),
    );
    assert!(canonical_skill_member(Path::new(&matrix), &normalized));
    assert!(!canonical_skill_member(&ac.join("missing"), &normalized));
}

#[test]
fn issue_2868_team_aggregate_budget_does_not_grant_omitted_roots() {
    let sources: Vec<_> = (0..120)
        .map(|i| {
            (
                format!("team-{i:03}"),
                SkillIndex {
                    matrix_root: Some(format!("/project/.ac/_team_{i:03}")),
                    skills_root: Some(format!("/project/.ac/_team_{i:03}/team-skills")),
                    skills: Vec::new(),
                    warnings: Vec::new(),
                },
            )
        })
        .collect();
    let content = render_team_skills_section(&sources);
    assert!(content.len() <= 65536);
    assert!(content.contains("Omitted roots gain no read permission"));
    assert!(content.contains("### Team team-000"));
    assert!(!content.contains("_team_119/team-skills"));
    assert!(!content.contains("### Team team-119"));
    assert!(!content.contains("You MAY READ /project/.ac/_team_*"));
}

#[cfg(windows)]
#[test]
fn issue_2868_team_junction_parent_leaf_and_dangling_are_rejected() {
    use super::super::super::teams::create_test_junction as junction;
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), "KEEP_OUTSIDE").unwrap();
    std::fs::write(
        outside.join("config.json"),
        serde_json::json!({"agents":[matrix]}).to_string(),
    )
    .unwrap();
    let parent = ac.join("_team_dev-team");
    junction(&parent, &outside);
    assert!(!accept_skill_team(&parent));
    assert!(!issue_2868_cache(&replica).contains("## Team Skills"));
    assert!(!outside.join("team-skills").exists());
    std::fs::remove_dir(&parent).unwrap();
    let team = issue_2868_team(&ac, "dev-team", serde_json::json!({"agents":[matrix]}));
    let leaf = team.join("team-skills");
    junction(&leaf, &outside);
    assert!(!issue_2868_cache(&replica).contains("## Team Skills"));
    assert_eq!(
        std::fs::read_to_string(outside.join("sentinel")).unwrap(),
        "KEEP_OUTSIDE"
    );
    std::fs::remove_dir(&leaf).unwrap();
    junction(&leaf, &temp.path().join("absent-target"));
    assert!(super::super::super::shared_locations::ensure_team_skills_dir(&team).is_err());
    assert!(!temp.path().join("absent-target").exists());
    std::fs::remove_dir(&leaf).unwrap();
}

fn issue_2868_write_project_skill(ac_root: &Path, folder: &str, metadata: &str) -> PathBuf {
    let path = ac_root.join("project-skills").join(folder).join("SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, metadata).unwrap();
    path
}

#[test]
fn issue_2868_origins_team_room_wg_and_other_project_cache_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, room, matrix) = make_valid_replica(temp.path());
    let project_skill = issue_2868_write_project_skill(
        &ac,
        "shared",
        "---\nname: shared\ndescription: Own project metadata\n---\nBODY_2868_SECRET\n",
    );
    let origin_a = PathBuf::from(matrix);
    let origin_b = ac.join("_agent_team-member");
    write_skill(
        &origin_a,
        "rust-own",
        "---\nname: rust-own\ndescription: Rust\n---\n",
    );
    write_skill(
        &origin_b,
        "team-own",
        "---\nname: team-own\ndescription: Team\n---\n",
    );
    let wg = ac.join("wg-20-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(
        wg.join("config.json"),
        "{\"identity\":\"../../_agent_dev-rust\"}",
    )
    .unwrap();
    for (root, own) in [
        (&origin_a, "rust-own"),
        (&origin_b, "team-own"),
        (&room, "rust-own"),
        (&wg, "rust-own"),
    ] {
        let content = issue_2868_cache(root);
        issue_2868_assert_permission(&content, &ac.join("project-skills"));
        assert!(content.contains(&display_path(&canonical_or_original(&project_skill))));
        assert!(content.contains(&format!("`{own}`")));
        assert!(content.contains("`project:shared`"));
        assert!(!content.contains("BODY_2868_SECRET"));
    }
    let other = temp.path().join("other").join(".ac");
    let other_agent = other.join("_agent_other");
    std::fs::create_dir_all(&other_agent).unwrap();
    issue_2868_write_project_skill(
        &other,
        "other",
        "---\nname: other\ndescription: Other project\n---\n",
    );
    let content = issue_2868_cache(&other_agent);
    issue_2868_assert_permission(&content, &other.join("project-skills"));
    assert!(!content.contains("Own project metadata"));
    assert!(!content.contains(&display_path(&ac.join("project-skills"))));
}

#[test]
fn issue_2868_generated_custom_and_override_append_only_once_without_authored_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, _) = make_valid_replica(temp.path());
    issue_2868_write_project_skill(
        &ac,
        "example",
        "---\nname: example\ndescription: Example\n---\n",
    );
    let generated = issue_2868_cache(&replica);
    issue_2868_assert_permission(&generated, &ac.join("project-skills"));
    let base = ac.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    for authored in [
        "CUSTOM_NO_RULE_OR_PLACEHOLDER\n",
        "CUSTOM_WITH_SKILLS\n{{SKILLS_SECTION}}\n",
        "## GOLDEN RULE — Repository Access Restrictions\nABSOLUTE AND NON-NEGOTIABLE: Only entries 1-6 are allowed. All other reads forbidden; refuse them. Nothing else under .ac is readable.\nCUSTOM_EXCLUSIVE\n",
    ] {
        std::fs::write(&base, authored).unwrap();
        for _ in 0..2 {
            let content = issue_2868_cache(&replica);
            issue_2868_assert_permission(&content, &ac.join("project-skills"));
            assert!(content.contains(authored.lines().next().unwrap()));
            assert!(content.contains("`project:example`"));
            assert_eq!(std::fs::read_to_string(&base).unwrap(), authored);
        }
    }
    let override_path = ac.join("Context.AgentsCommander.local.md");
    let authored = "LOCAL_OVERRIDE_WITHOUT_PLACEHOLDER\n";
    std::fs::write(&override_path, authored).unwrap();
    let base_before = std::fs::read(&base).unwrap();
    let content = issue_2868_cache(&replica);
    issue_2868_assert_permission(&content, &ac.join("project-skills"));
    assert!(content.contains(authored));
    assert_eq!(std::fs::read_to_string(&override_path).unwrap(), authored);
    assert_eq!(std::fs::read(&base).unwrap(), base_before);
}

#[test]
#[should_panic]
fn issue_2868_vague_supplement_does_not_satisfy_exclusive_rule_contract() {
    issue_2868_assert_permission(
        "Only entries 1-6 allowed. ## Project Skills\nYou may read project skills.",
        Path::new("/project/.ac/project-skills"),
    );
}

#[test]
fn issue_2868_invalid_identity_unrecognized_nested_and_root_have_no_project_source() {
    let temp = tempfile::tempdir().unwrap();
    let ac = temp.path().join(".ac");
    for name in [
        "standalone",
        "nested/_agent_dev",
        "ac-root-agent",
        "_agent_dev/nested",
    ] {
        let root = ac.join(name);
        std::fs::create_dir_all(&root).unwrap();
        assert!(resolve_project_skill_root(&display_path(&root), None).is_none());
        let empty = discover_skill_index(None);
        assert!(project_skills_for_session(&display_path(&root), None, &empty).is_empty());
    }
    assert!(!ac.join("project-skills").exists());
    let replica = ac.join("room-4-dev").join("__agent_dev");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(
        replica.join("config.json"),
        "{\"identity\":\"../../_agent_missing\"}",
    )
    .unwrap();
    assert!(ensure_session_context(&display_path(&replica))
        .unwrap_err()
        .contains("Invalid Room replica identity"));
    assert!(!ac.join("project-skills").exists());
}

#[test]
fn issue_2868_collision_invalid_agent_and_source_local_deterministic_duplicates() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    let agent = PathBuf::from(matrix);
    let agent_entry = write_skill(
        &agent,
        "same",
        "---\nname: same\ndescription: Agent valid\n---\n",
    );
    let project_entry = issue_2868_write_project_skill(
        &ac,
        "A-first",
        "---\nname: same\ndescription: Project first\n---\n",
    );
    issue_2868_write_project_skill(
        &ac,
        "a-second",
        "---\nname: same\ndescription: Project second\n---\n",
    );
    issue_2868_write_project_skill(
        &ac,
        "fallback",
        "---\nname: fallback\ndescription: Project fallback\n---\n",
    );
    write_skill(
        &agent,
        "fallback",
        "---\nname: INVALID\ndescription: Invalid agent\n---\n",
    );
    let content = issue_2868_cache(&replica);
    assert!(content.contains("`same`"));
    assert!(content.contains("`project:same`"));
    assert!(content.contains("`project:fallback`"));
    assert!(content.contains(
        "unqualified name selects a valid agent skill first, otherwise the valid project skill"
    ));
    assert!(content.contains("Warning [project-skills]: Name collision `same`"));
    for path in [agent_entry, project_entry] {
        assert!(content.contains(&display_path(&canonical_or_original(&path))));
    }
    assert!(content.contains("Project first"));
    assert!(!content.contains("Project second"));
    assert!(content.contains("duplicate skill name"));
}

#[test]
fn issue_2868_project_scanner_preserves_valid_siblings_and_exact_entrypoint() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    write_skill(
        Path::new(&matrix),
        "agent",
        "---\nname: agent\ndescription: Agent stays\n---\n",
    );
    let missing = issue_2868_cache(&replica);
    assert!(missing.contains("`agent`"));
    assert!(ac.join("project-skills").is_dir());
    issue_2868_write_project_skill(
        &ac,
        "good",
        "---\nname: good\ndescription: Good sibling\n---\nBODY_NOT_READ_2868",
    );
    let wrong_case = issue_2868_write_project_skill(&ac, "wrong", "---\nname: wrong\n---\n");
    std::fs::rename(&wrong_case, wrong_case.with_file_name("skill.md")).unwrap();
    issue_2868_write_project_skill(&ac, "broken", "---\nname: [broken\n---\n");
    issue_2868_write_project_skill(
        &ac,
        "oversize",
        &format!(
            "---\nname: oversize\ndescription: {}\n---\n",
            "x".repeat(17000)
        ),
    );
    let directory = ac.join("project-skills/directory/SKILL.md");
    std::fs::create_dir_all(&directory).unwrap();
    let content = issue_2868_cache(&replica);
    assert!(content.contains("`agent`"));
    assert!(content.contains("`project:good`"));
    assert!(content.contains("missing exact SKILL.md"));
    assert!(content.contains("frontmatter exceeds"));
    assert!(content.contains("not a regular file"));
    assert!(content.contains("YAML parse error"));
    assert!(!content.contains("BODY_NOT_READ_2868"));
}

#[test]
fn issue_2868_occupied_file_keeps_bytes_and_cache_for_origin_and_replica() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    let source = ac.join("project-skills");
    std::fs::write(&source, "OCCUPIED_SENTINEL").unwrap();
    write_skill(
        Path::new(&matrix),
        "agent",
        "---\nname: agent\ndescription: Agent stays\n---\n",
    );
    assert_eq!(
        crate::config::shared_locations::ensure_project_skills_dir(&ac)
            .unwrap_err()
            .to_string(),
        "not an ordinary directory"
    );
    for root in [Path::new(&matrix), replica.as_path()] {
        let content = issue_2868_cache(root);
        issue_2868_assert_permission(&content, &source);
        assert!(content.contains("`agent`"));
        assert!(!content.contains("OCCUPIED_SENTINEL"));
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "OCCUPIED_SENTINEL"
        );
    }
    for name in ["plans", "tools", "errors", "project-shared"] {
        assert!(ac.join(name).is_dir());
    }
}

#[test]
fn issue_2868_overflow_cache_keeps_complete_permission_and_metadata_only() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, _) = make_valid_replica(temp.path());
    for i in 0..450 {
        issue_2868_write_project_skill(
            &ac,
            &format!("skill-{i:04}"),
            &format!(
                "---\nname: skill-{i:04}\ndescription: '{}'\n---\nBODY_OVERFLOW_MARKER_2868\n",
                "é` special ".repeat(300)
            ),
        );
    }
    let content = issue_2868_cache(&replica);
    issue_2868_assert_permission(&content, &ac.join("project-skills"));
    let section = &content[content.find("\n\n## Project Skills").unwrap()..];
    assert!(section.len() <= 65536, "{}", section.len());
    assert!(section.contains("omitted"));
    assert!(section.contains("manual discovery"));
    assert!(section.contains("bodies load on demand"));
    assert!(section.contains("unqualified name selects a valid agent skill first"));
    assert!(!section.contains("BODY_OVERFLOW_MARKER_2868"));
    assert!(!section.contains("é` special"));
}

#[test]
fn issue_2868_legacy_classifier_and_healing_keep_agent_only_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    let root = display_path(&replica);
    write_skill(
        Path::new(&matrix),
        "agent",
        "---\nname: agent\ndescription: Agent\n---\n",
    );
    let index = discover_skill_index(Some(&matrix));
    let current_skills = render_skills_section(&index);
    let current =
        current_legacy_rendered_default_context(&root, Some(&matrix), &current_skills);
    let old_intro = current_skills.replacen(
        GENERATED_SKILLS_SECTION_INTRO,
        LEGACY_GENERATED_SKILLS_SECTION_INTRO,
        1,
    );
    let pre_room = current_skills.replace(
        GENERATED_SKILLS_SECTION_REPLICA_LINE,
        GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME,
    );
    let cases = [
        current.clone(),
        legacy_rendered_default_context_for_compat(&root, Some(&matrix), &old_intro),
        legacy_rendered_default_context_for_compat(&root, Some(&matrix), &pre_room),
        pre_1072_legacy_rendered_default_context_for_compat(
            &root,
            Some(&matrix),
            &current_skills,
        ),
        "CUSTOM_PRESERVED\n".to_string(),
    ];
    let before: Vec<_> = cases
        .iter()
        .map(|text| {
            classify_legacy_rendered_default_context(
                text,
                &root,
                Some(&matrix),
                &current_skills,
            )
        })
        .collect();
    issue_2868_write_project_skill(
        &ac,
        "project",
        "---\nname: project\ndescription: Project\n---\n",
    );
    assert_eq!(
        render_skills_section(&discover_skill_index(Some(&matrix))),
        current_skills
    );
    let template = ac.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    for (text, expected) in cases.iter().zip(before) {
        let classified = classify_legacy_rendered_default_context(
            text,
            &root,
            Some(&matrix),
            &current_skills,
        );
        assert_eq!(
            std::mem::discriminant(&classified),
            std::mem::discriminant(&expected)
        );
        std::fs::write(&template, text).unwrap();
        let content = issue_2868_cache(&replica);
        issue_2868_assert_permission(&content, &ac.join("project-skills"));
        assert!(content.contains("`project:project`"));
        let disk = std::fs::read_to_string(&template).unwrap();
        if !matches!(expected, LegacyRenderedDefaultContext::StaleGenerated) {
            assert_eq!(&disk, text);
        } else {
            assert_eq!(disk, get_default_agent_template());
        }
    }
}

#[test]
fn issue_2868_root_raw_and_canonical_guard_preserve_durable_output() {
    let (temp, root) = issue_2832_fixture();
    let base = temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let before = std::fs::read(&base).unwrap();
    let control = with_issue_2832_root_config(temp.path(), || issue_2868_cache(&root));
    let nested = root.join("ordinary-child");
    std::fs::create_dir(&nested).unwrap();
    let alias = nested.join("..");
    for identity in [&root, &alias] {
        let content = with_issue_2832_root_config(temp.path(), || issue_2868_cache(identity));
        assert!(!content.contains("## Project Skills"));
        assert!(!content.contains("Filesystem authorization amendment:"));
        assert!(content.contains("Root Agent durable skills"));
        assert!(!temp.path().join("project-skills").exists());
        assert!(!root.join("project-skills").exists());
    }
    assert!(!control.contains("## Project Skills"));
    assert_eq!(std::fs::read(&base).unwrap(), before);
}

#[cfg(windows)]
#[test]
fn issue_2868_unreadable_locked_entry_preserves_valid_sibling_and_agent() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    write_skill(
        Path::new(&matrix),
        "agent",
        "---\nname: agent\ndescription: Agent stays\n---\n",
    );
    let locked = issue_2868_write_project_skill(
        &ac,
        "locked",
        "---\nname: locked\ndescription: Must not appear\n---\n",
    );
    issue_2868_write_project_skill(
        &ac,
        "good",
        "---\nname: good\ndescription: Good sibling\n---\n",
    );
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(locked)
        .unwrap();
    let content = issue_2868_cache(&replica);
    assert!(content.contains("`agent`"));
    assert!(content.contains("`project:good`"));
    assert!(!content.contains("`project:locked`"));
    assert!(content.contains("failed to open SKILL.md frontmatter"));
    assert!(content.contains("Warning [project-skills]"));
}

#[cfg(windows)]
#[test]
fn issue_2868_junction_source_bootstrap_and_sessions_reject_target() {
    let temp = tempfile::tempdir().unwrap();
    let (ac, _, replica, matrix) = make_valid_replica(temp.path());
    let source = ac.join("project-skills");
    let target = temp.path().join("junction-target");
    std::fs::create_dir(&target).unwrap();
    super::super::super::teams::create_test_junction(&source, &target);
    assert!(is_link_or_reparse(
        &std::fs::symlink_metadata(&source).unwrap()
    ));
    let skill = target.join("secret");
    std::fs::create_dir(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: secret\ndescription: JUNCTION_METADATA_SENTINEL\n---\n",
    )
    .unwrap();
    std::fs::write(target.join("sentinel"), "JUNCTION_TARGET_UNCHANGED").unwrap();
    write_skill(
        Path::new(&matrix),
        "agent",
        "---\nname: agent\ndescription: Agent\n---\n",
    );
    for dangling in [false, true] {
        if dangling {
            std::fs::remove_file(skill.join("SKILL.md")).unwrap();
            std::fs::remove_dir(&skill).unwrap();
            std::fs::remove_file(target.join("sentinel")).unwrap();
            std::fs::remove_dir(&target).unwrap();
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
        for root in [Path::new(&matrix), replica.as_path()] {
            let content = issue_2868_cache(root);
            issue_2868_assert_permission(&content, &source);
            assert!(content.contains("`agent`"));
            assert!(!content.contains("JUNCTION_METADATA_SENTINEL"));
            assert!(!content.contains(&format!("You MAY READ {}", display_path(&target))));
        }
        assert!(is_link_or_reparse(
            &std::fs::symlink_metadata(&source).unwrap()
        ));
        if !dangling {
            assert_eq!(
                std::fs::read_to_string(target.join("sentinel")).unwrap(),
                "JUNCTION_TARGET_UNCHANGED"
            );
        }
    }
}
