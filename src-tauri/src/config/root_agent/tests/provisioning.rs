use super::*;

#[test]
fn root_self_handoff_skill_is_seeded_and_customized_bytes_survive() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    ensure_root_agent_dir_at(&root).unwrap();
    let path = root.join("skills/self-handoff/SKILL.md");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        SELF_HANDOFF_SKILL_CONTENT.as_bytes()
    );
    assert_eq!(
        default_root_skill("self-handoff").content,
        SELF_HANDOFF_SKILL_CONTENT
    );
    std::fs::write(&path, "USER CUSTOMIZATION\n").unwrap();
    ensure_default_root_agent_skills_at(&root).unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "USER CUSTOMIZATION\n"
    );
}

#[test]
fn project_and_root_skill_publisher_repairs_only_known_prior_versions() {
    const PRIOR_VERSION: &str = "# Fixture of an earlier handoff skill\n";
    let skill = DefaultRootSkill {
        legacy_snapshots: &[PRIOR_VERSION],
        ..SELF_HANDOFF_DEFAULT_SKILL
    };
    let temp = tempfile::tempdir().unwrap();
    for leaf in ["project-skills", "skills"] {
        let root = temp.path().join(leaf);
        let skills_root = root.join(leaf);
        let path = skills_root.join("self-handoff/SKILL.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, PRIOR_VERSION).unwrap();
        crate::config::root_agent::provisioning::ensure_default_root_skill_file(
            &root,
            &skills_root,
            &skill,
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), skill.content.as_bytes());
        let edited = format!("{PRIOR_VERSION}User addition\n");
        std::fs::write(&path, &edited).unwrap();
        crate::config::root_agent::provisioning::ensure_default_root_skill_file(
            &root,
            &skills_root,
            &skill,
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), edited.as_bytes());
    }
}

#[test]
fn self_handoff_skill_pins_structure_exclusions_and_shortening_contract() {
    let content = SELF_HANDOFF_SKILL_CONTENT;
    let mut previous = 0;
    for heading in [
        "1. **First action**",
        "2. **Active task**",
        "3. **Where the work is**",
        "4. **In flight**",
        "5. **Coordination**",
        "6. **Pending decisions and temporary holds**",
        "7. **Next**",
    ] {
        let position = content.find(heading).expect(heading);
        assert!(position > previous);
        previous = position;
    }
    for required in [
        "The clear erases only the conversation.",
        "context file, `Role.md` and skill index reload",
        "## Never include",
        "Identity, role, language or answer rules.",
        "Skill procedures: name the skill instead.",
        "anything already in `SELF-FORGET.md`",
        "Round history, hash lists or status revision numbers",
        "Credentials.",
        "Durable lessons:",
        "Pending to save:",
        "Count the words.",
        "Rewrite to half or fewer.",
        "Never cut a fact",
        "no glued words, no invented abbreviations",
        "Words: draft <N>, final <M>.",
        "self-handoff-and-clear",
        "self-handoff-and-switch",
        "self-handoff-and-restart",
    ] {
        assert!(
            content.contains(required),
            "missing skill contract: {required}"
        );
    }
}

#[test]
fn root_template_before_self_handoff_skill_snapshot_is_byte_exact() {
    use sha2::{Digest, Sha256};
    let content =
        crate::config::root_agent::legacy_contexts::ROOT_CONTEXT_BEFORE_SELF_HANDOFF_SKILL_MD;
    assert_eq!(content.len(), 19433);
    assert_eq!(
        format!("{:x}", Sha256::digest(content.as_bytes())),
        "188a11f923edc180b5d90c99d4c0a88b3b5d48464b84233367a6f9e5cbf19803"
    );
    assert!(is_known_generated_root_context_template(content));
}

#[test]
fn missing_role_seed_uses_minimal_role_without_copying_custom_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let custom_template =
        format!("# Custom Root Template\n\n{FAIL_ROOT_ROLE_WRITE_MARKER}\n\nComplete seed body.\n");
    std::fs::write(&template_path, &custom_template).expect("write template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD
    );
    assert_eq!(
        std::fs::read_to_string(template_path).expect("read template"),
        custom_template
    );
}

#[test]
fn ensure_root_agent_dir_at_migrates_pre_boundary_audit_generated_root_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD)
        .expect("write old generated template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    let migrated = std::fs::read_to_string(template_path).expect("read template");
    assert_eq!(migrated, default_root_context_template());
    assert!(migrated.contains("role-skill-boundary-audit"));
}

#[test]
fn ensure_root_agent_dir_at_preserves_custom_root_template_with_boundary_audit_text() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let custom = "# Custom Root Template\n\nrole-skill-boundary-audit stays custom.\n";
    std::fs::write(&template_path, custom).expect("write custom template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(template_path).expect("read template"),
        custom
    );
}

#[test]
fn ensure_root_agent_dir_at_is_idempotent_and_preserves_custom_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, "# Custom Template\n\nTemplate body.\n")
        .expect("write template");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Role.md"), "custom role").expect("write role");

    ensure_root_agent_dir_at(&root).expect("first ensure");
    ensure_root_agent_dir_at(&root).expect("second ensure");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        "custom role"
    );
}

#[cfg(windows)]
#[test]
fn rtk_install_windows_fresh_provisioning_is_byte_exact_and_idempotent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skill = root.join("skills/rtk-install-windows/SKILL.md");
    let expected = default_root_skill("rtk-install-windows").content.as_bytes();

    ensure_root_agent_dir_at(&root).expect("ensure root");
    assert_eq!(std::fs::read(&skill).expect("read skill"), expected);
    assert!(root
        .join("skills/role-skill-boundary-audit/SKILL.md")
        .is_file());
    assert!(root.join("skills/agency-agents-roles/SKILL.md").is_file());
    ensure_root_agent_dir_at(&root).expect("ensure root again");
    assert_eq!(std::fs::read(&skill).expect("read skill again"), expected);
}

#[cfg(windows)]
#[test]
fn rtk_install_windows_direct_seeder_recreates_missing_entrypoint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    ensure_root_agent_dir_at(&root).expect("ensure root");
    let skill = root.join("skills/rtk-install-windows/SKILL.md");
    std::fs::remove_file(&skill).expect("remove skill");

    ensure_default_root_agent_skills_at(&root).expect("seed missing skill");
    assert_eq!(
        std::fs::read(&skill).expect("read recreated skill"),
        default_root_skill("rtk-install-windows").content.as_bytes()
    );
}

#[test]
fn rtk_install_windows_preserves_custom_copied_profile_bytes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skill = root.join("skills/rtk-install-windows/SKILL.md");
    let custom = b"custom RTK skill\r\nkeep these bytes\r\n";
    std::fs::create_dir_all(skill.parent().expect("skill parent")).expect("create skill dir");
    std::fs::write(&skill, custom).expect("write custom skill");

    ensure_root_agent_dir_at(&root).expect("ensure root");
    assert_eq!(std::fs::read(&skill).expect("read custom skill"), custom);
    ensure_default_root_agent_skills_at(&root).expect("seed skills");
    assert_eq!(
        std::fs::read(&skill).expect("read custom skill again"),
        custom
    );
}

#[cfg(not(windows))]
#[test]
fn rtk_install_windows_is_not_seeded_on_non_windows() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    ensure_root_agent_dir_at(&root).expect("ensure root");
    assert!(!root.join("skills/rtk-install-windows").exists());
    ensure_default_root_agent_skills_at(&root).expect("seed skills");
    assert!(!root.join("skills/rtk-install-windows").exists());
    assert!(root
        .join("skills/role-skill-boundary-audit/SKILL.md")
        .is_file());
    assert!(root.join("skills/agency-agents-roles/SKILL.md").is_file());
}

#[cfg(windows)]
#[test]
fn rtk_install_windows_embedded_bytes_match_supplied_sha256() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(default_root_skill("rtk-install-windows").content.as_bytes())
        ),
        "1f52562c2f8741ef02f7a7968b68d7675846439f2b5e84b174fa4d7b7c3e9927"
    );
}

#[test]
fn ensure_root_agent_dir_at_preserves_existing_boundary_audit_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skill = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("role-skill-boundary-audit")
        .join(SKILL_MD_FILENAME);
    std::fs::create_dir_all(skill.parent().expect("skill parent")).expect("create skill dir");
    std::fs::write(&skill, "custom boundary skill").expect("write custom skill");

    ensure_root_agent_dir_at(&root).expect("ensure root");
    ensure_root_agent_dir_at(&root).expect("ensure root again");

    assert_eq!(
        std::fs::read_to_string(&skill).expect("read skill"),
        "custom boundary skill"
    );
}

#[test]
fn ensure_root_agent_dir_at_recreates_missing_boundary_audit_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    ensure_root_agent_dir_at(&root).expect("ensure root");

    let skill = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("role-skill-boundary-audit")
        .join(SKILL_MD_FILENAME);
    std::fs::remove_file(&skill).expect("remove skill");

    ensure_root_agent_dir_at(&root).expect("ensure root again");

    assert_eq!(
        std::fs::read_to_string(&skill).expect("read skill"),
        DEFAULT_ROOT_SKILLS[0].content
    );
}

#[test]
fn ensure_root_agent_dir_at_recreates_missing_agency_agents_roles_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    ensure_root_agent_dir_at(&root).expect("ensure root");

    let skill = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("agency-agents-roles")
        .join(SKILL_MD_FILENAME);
    std::fs::remove_file(&skill).expect("remove skill");

    ensure_root_agent_dir_at(&root).expect("ensure root again");

    assert_eq!(
        std::fs::read_to_string(&skill).expect("read skill"),
        DEFAULT_ROOT_SKILLS[1].content
    );
}

#[test]
fn ensure_root_agent_dir_at_migrates_pre_agency_skill_generated_root_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD)
        .expect("write old generated template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    let migrated = std::fs::read_to_string(template_path).expect("read template");
    assert_eq!(migrated, default_root_context_template());
    assert!(migrated.contains("skills/agency-agents-roles/SKILL.md"));
    assert!(migrated.contains("agency-templates update"));
}

#[test]
fn ensure_root_agent_dir_at_reduces_pre_boundary_audit_generated_role_to_minimal_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Role.md"), ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD)
        .expect("write generated role");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD
    );
}

#[test]
fn ensure_root_agent_dir_at_rejects_default_skill_dir_as_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skill_path = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("role-skill-boundary-audit");
    std::fs::create_dir_all(skill_path.parent().expect("skill parent"))
        .expect("create skills root");
    std::fs::write(&skill_path, "not a directory").expect("write skill dir file");

    let err = ensure_root_agent_dir_at(&root).expect_err("skill dir file must fail");

    assert!(err.contains("not a regular directory"), "{err}");
    assert!(!skill_path.join(SKILL_MD_FILENAME).exists());
}

#[test]
fn ensure_root_agent_dir_at_rejects_default_skill_entrypoint_as_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skill_file = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("role-skill-boundary-audit")
        .join(SKILL_MD_FILENAME);
    std::fs::create_dir_all(&skill_file).expect("create directory entrypoint");

    let err = ensure_root_agent_dir_at(&root).expect_err("skill file dir must fail");

    assert!(err.contains("not a regular file"), "{err}");
    assert!(skill_file.is_dir());
}

#[test]
fn ensure_root_agent_dir_at_rejects_root_symlink_where_supported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join("target-root");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&target).expect("create target root");
    if try_symlink_dir(&target, &root).is_err() {
        return;
    }

    let err = ensure_root_agent_dir_at(&root).expect_err("root symlink must fail");

    assert!(err.contains("not a regular directory"), "{err}");
    assert!(!target.join("Role.md").exists());
}

#[test]
fn ensure_root_agent_dir_at_rejects_default_skill_file_symlink_where_supported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skill_file = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("role-skill-boundary-audit")
        .join(SKILL_MD_FILENAME);
    let target = temp.path().join("target-skill.md");
    std::fs::create_dir_all(skill_file.parent().expect("skill parent")).expect("create skill dir");
    std::fs::write(&target, "target skill").expect("write target");
    if try_symlink_file(&target, &skill_file).is_err() {
        return;
    }

    let err = ensure_root_agent_dir_at(&root).expect_err("skill symlink must fail");

    assert!(err.contains("not a regular file"), "{err}");
}

#[test]
fn publish_missing_default_skill_file_revalidates_raced_existing_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(SKILL_MD_FILENAME);
    let temp_path = temp.path().join(".tmp-skill");
    std::fs::write(&temp_path, "default skill").expect("write temp");
    std::fs::create_dir(&path).expect("create invalid raced target");

    let err = publish_missing_default_skill_file_with(&temp_path, &path, |_, _| {
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "raced target",
        ))
    })
    .expect_err("invalid raced target must fail");

    assert!(err.contains("not a regular file"), "{err}");
}

#[test]
fn create_default_skill_temp_file_retries_stale_temp_collision() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(SKILL_MD_FILENAME);
    let first = temp.path().join(".first.tmp");
    let second = temp.path().join(".second.tmp");
    std::fs::write(&first, "stale").expect("write stale temp");
    let mut calls = 0;

    let (created_path, file) = create_default_skill_temp_file_with(&path, |_| {
        calls += 1;
        if calls == 1 {
            first.clone()
        } else {
            second.clone()
        }
    })
    .expect("create temp after collision");
    drop(file);

    assert_eq!(created_path, second);
    assert_eq!(calls, 2);
    assert!(first.exists());
    assert!(second.exists());
}

#[test]
fn ensure_root_agent_dir_at_reduces_current_builtin_role_to_minimal_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let custom_template = "# Custom Root Template\n\nReplace built-in text.\n";
    std::fs::write(&template_path, custom_template).expect("write template");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Role.md"), ROOT_ROLE_MD).expect("write current role");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD
    );
    assert_eq!(
        std::fs::read_to_string(template_path).expect("read template"),
        custom_template
    );
}

#[test]
fn ensure_root_agent_dir_at_migrates_old_builtin_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Role.md"), OLD_ROOT_ROLE_MD).expect("write old role");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    let migrated = std::fs::read_to_string(root.join("Role.md")).expect("read role");
    assert_eq!(
        normalize_role_text(&migrated),
        normalize_role_text(MINIMAL_ROOT_ROLE_MD)
    );
    assert!(!migrated.contains("verified workgroup coordinator replicas only"));
    assert!(!migrated.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH));
}

#[test]
fn ensure_root_agent_dir_at_reduces_old_builtin_role_to_minimal_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let custom_template = "# Custom Root Template\n\nMigrate old default here.\n";
    std::fs::write(&template_path, custom_template).expect("write template");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Role.md"), OLD_ROOT_ROLE_MD).expect("write old role");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD
    );
    assert_eq!(
        std::fs::read_to_string(template_path).expect("read template"),
        custom_template
    );
}

#[test]
fn ensure_root_agent_dir_at_replaces_old_deferred_paragraph_in_custom_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    let custom = format!(
        "# Custom Root\n\n{}\n\nKeep this custom tail.",
        OLD_DEFERRED_MESSAGING_PARAGRAPH
    );
    std::fs::write(root.join("Role.md"), custom).expect("write custom role");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    let migrated = std::fs::read_to_string(root.join("Role.md")).expect("read role");
    assert!(migrated.starts_with("# Custom Root"));
    assert!(migrated.contains("Keep this custom tail."));
    assert!(migrated.contains(ROOT_COORDINATION_MESSAGING_PARAGRAPH));
    assert!(!migrated.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH));
}

#[test]
fn ensure_root_agent_dir_at_errors_when_root_template_is_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::create_dir_all(&template_path).expect("create template directory");

    let err = ensure_root_agent_dir_at(&root).expect_err("directory template must fail");

    assert!(err.contains("not a regular file"), "{err}");
    assert!(!root.join("Role.md").exists());
}

#[test]
fn ensure_root_agent_dir_at_errors_when_root_template_is_symlink() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let target = temp.path().join("target.md");
    std::fs::write(&target, "linked template").expect("write target");
    let Ok(()) = try_symlink_file(&target, &template_path) else {
        return;
    };

    let err = ensure_root_agent_dir_at(&root).expect_err("symlink template must fail");

    assert!(err.contains("not a regular file"), "{err}");
    assert!(!root.join("Role.md").exists());
}

#[test]
fn create_default_context_templates_does_not_create_root_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");

    crate::config::session_context::create_default_context_templates(&ac_root)
        .expect("create default templates");

    assert!(ac_root
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(ac_root
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(!ac_root
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
        .exists());
}

#[test]
fn root_sender_uses_reserved_non_path_namespace() {
    assert_eq!(ROOT_AGENT_SENDER, "agentscommander://root-agent");
    assert_ne!(
        ROOT_AGENT_SENDER,
        crate::config::teams::agent_fqn_from_path("C:/tmp/agentscommander/_agent_root-agent")
    );
}

#[test]
fn h2_strict_identity_and_protection_truth_table() {
    use crate::config::root_agent::provisioning::root_agent_path_requires_protection_at;
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let root = config.join(ROOT_AGENT_DIR_NAME);
    let ordinary = temp.path().join("ordinary");
    let missing = temp.path().join("missing");
    let file = temp.path().join("file");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir(&ordinary).unwrap();
    std::fs::write(&file, "not a directory").unwrap();
    assert!(paths_equivalent(&root, &root));
    assert!(!paths_equivalent(&missing, &missing));
    assert!(!paths_equivalent(&file, &file));
    assert!(!paths_equivalent(&root, &ordinary));
    for (cwd, expected) in [
        (&root, true),
        (&ordinary, false),
        (&missing, true),
        (&file, true),
    ] {
        assert_eq!(
            root_agent_path_requires_protection_at(cwd.to_str().unwrap(), &config),
            expected,
            "{}",
            cwd.display()
        );
    }
    let absent_config = temp.path().join("absent-config");
    for cwd in [&root, &ordinary, &missing, &file] {
        assert!(root_agent_path_requires_protection_at(
            cwd.to_str().unwrap(),
            &absent_config
        ));
    }
    assert!(!is_root_agent_path_at(
        missing.to_str().unwrap(),
        &absent_config
    ));
    assert!(!missing.join("skills").exists());
    assert!(!missing.join("Role.md").exists());
    ensure_root_agent_dir_at(&root).unwrap();
    assert!(root.join("Role.md").is_file());
    assert!(root
        .join("skills/role-skill-boundary-audit/SKILL.md")
        .is_file());
    assert!(!missing.exists());
}

#[cfg(unix)]
#[test]
fn h2_literal_backslash_identity() {
    let temp = tempfile::tempdir().unwrap();
    let literal = temp.path().join(r"a\b");
    let separated = temp.path().join("a/b");
    std::fs::create_dir(&literal).unwrap();
    std::fs::create_dir_all(&separated).unwrap();
    assert!(!paths_equivalent(&literal, &separated));
    let left = crate::path_identity::verify_directory(&literal).unwrap();
    let right = crate::path_identity::verify_directory(&separated).unwrap();
    assert_ne!(left.object_id, right.object_id);
}

#[cfg(unix)]
#[test]
fn h2_protection_resolved_alias() {
    use crate::config::root_agent::provisioning::root_agent_path_requires_protection_at;
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let root = config.join(ROOT_AGENT_DIR_NAME);
    let ordinary = temp.path().join("ordinary");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir(&ordinary).unwrap();
    let ordinary_link = temp.path().join("ordinary-link");
    let root_link = temp.path().join("root-link");
    let broken = temp.path().join("broken");
    std::os::unix::fs::symlink(&ordinary, &ordinary_link).unwrap();
    std::os::unix::fs::symlink(&root, &root_link).unwrap();
    std::os::unix::fs::symlink(temp.path().join("absent"), &broken).unwrap();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let parent = nested.join("..");
    for (alias, target, protects) in [
        (&ordinary_link, Some(&ordinary), false),
        (&root_link, Some(&root), true),
        (&parent, Some(&root), true),
        (&broken, None, true),
    ] {
        assert_eq!(
            root_agent_path_requires_protection_at(alias.to_str().unwrap(), &config),
            protects,
            "{}",
            alias.display()
        );
        if let Some(target) = target {
            let resolved = std::fs::canonicalize(alias).unwrap();
            assert_eq!(
                crate::path_identity::verify_directory(&resolved)
                    .unwrap()
                    .object_id,
                crate::path_identity::verify_directory(target)
                    .unwrap()
                    .object_id
            );
        }
    }
    for alias in [&ordinary_link, &root_link, &broken] {
        assert!(!paths_equivalent(alias, &root));
        assert!(!is_root_agent_path_at(alias.to_str().unwrap(), &config));
    }
    assert!(ensure_root_agent_dir_at(&root_link).is_err());
    assert!(!root.join("Role.md").exists());
    assert!(!root.join("skills").exists());
    ensure_root_agent_dir_at(&root).unwrap();
    assert!(root.join("Role.md").is_file());
}

#[test]
fn h2_invalid_supplied_root_publishes_no_context_or_skills() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    assert!(
        crate::config::session_context::ensure_session_context(root.to_str().unwrap()).is_err()
    );
    assert!(!root.exists());
    std::fs::create_dir(&root).unwrap();
    assert!(
        crate::config::session_context::ensure_session_context(root.to_str().unwrap()).is_err()
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert!(!temp.path().join("Context.root-agent.md").exists());
    // Provisioning the intended explicit Root remains independent of a supplied impostor.
    ensure_root_agent_dir_at(&root).unwrap();
    assert!(root.join("Role.md").is_file());
    assert!(root
        .join("skills/role-skill-boundary-audit/SKILL.md")
        .is_file());
}
