use super::*;

#[test]
fn materialize_replica_context_repairs_stale_identity_before_role_injection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("AgentsCommander_ac");
    let ac_root = project.join(".ac");
    let matrix_root = ac_root.join("_agent_tech-lead");
    let replica_root = ac_root.join("wg-2-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "# Tech Lead\n\nLOCAL_MATRIX_ROLE_BODY\n",
    )
    .expect("write Role.md");
    std::fs::write(
        replica_root.join("config.json"),
        r#"{"identity":"../../../../agentscommander-old/.ac/_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT","../../../../agentscommander-old/.ac/_agent_tech-lead/Role.md"]}"#,
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

    assert_global_context_before_one_role(&content, "LOCAL_MATRIX_ROLE_BODY");
    assert!(!content.contains("agentscommander-old"));

    let repaired: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(replica_root.join("config.json")).expect("read config"),
    )
    .expect("parse repaired config");
    assert_eq!(repaired["identity"], "../../_agent_tech-lead");
    assert_eq!(
        repaired["context"],
        serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "../../_agent_tech-lead/Role.md"])
    );
}

#[test]
fn ensure_session_context_rejects_unrepairable_replica_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("AgentsCommander_ac");
    let ac_root = project.join(".ac");
    let matrix_root = ac_root.join("_agent_tech-lead");
    let replica_root = ac_root.join("wg-2-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::write(
        replica_root.join("config.json"),
        r#"{"identity":"../../../../agentscommander-old/.ac/_agent_architect"}"#,
    )
    .expect("write replica config");

    let err = ensure_session_context(&path_string(&replica_root))
        .expect_err("unrepairable identity must fail context generation");

    assert!(err.contains("Invalid Room replica identity"), "{err}");
    assert!(err.contains("_agent_architect"), "{err}");
    assert!(err.contains("_agent_tech-lead"), "{err}");
}

#[test]
fn materialize_agent_context_file_includes_skills_for_direct_matrix_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join(".ac").join("_agent_dev-rust");
    write_skill(
        &matrix_root,
        "runtime",
        "---\nname: runtime\ndescription: Direct runtime skill metadata.\nwhen_to_use: Use directly from the canonical matrix.\n---\nDIRECT_BODY_SHOULD_NOT_RENDER\n",
    );

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let materialized_path = PathBuf::from(&materialized);
    let content =
        std::fs::read_to_string(&materialized_path).expect("read materialized context");

    assert_eq!(
        materialized_path.file_name().and_then(|name| name.to_str()),
        Some("AGENTS.md")
    );
    assert!(content.contains("## Skills"));
    assert!(content.contains("`runtime`"));
    assert!(content.contains("Direct runtime skill metadata."));
    assert!(content.contains("When to use: Use directly from the canonical matrix."));
    assert_contains_canonical_path(
        &content,
        &matrix_root.join("skills").join("runtime").join("SKILL.md"),
    );
    assert!(!content.contains("DIRECT_BODY_SHOULD_NOT_RENDER"));
}

#[test]
fn materialize_agent_context_file_includes_local_role_for_direct_matrix_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join(".ac").join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "# Direct Role\n\nDIRECT_MATRIX_ROLE_BODY\n",
    )
    .expect("write Role.md");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_global_context_before_one_role(&content, "DIRECT_MATRIX_ROLE_BODY");
}

#[test]
fn materialize_direct_matrix_default_config_includes_global_and_one_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join(".ac").join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        matrix_root.join("config.json"),
        r#"{"tooling":{},"context":["$AGENTSCOMMANDER_CONTEXT","Role.md"]}"#,
    )
    .expect("write config");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "# Direct Role\n\nDEFAULT_CONFIG_ROLE_BODY\n",
    )
    .expect("write Role.md");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_global_context_before_one_role(&content, "DEFAULT_CONFIG_ROLE_BODY");
}

#[test]
fn materialize_direct_matrix_role_only_config_prepends_global_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join(".ac").join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        matrix_root.join("config.json"),
        r#"{"tooling":{},"context":["Role.md"]}"#,
    )
    .expect("write config");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "# Direct Role\n\nROLE_ONLY_CONFIG_BODY\n",
    )
    .expect("write Role.md");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_global_context_before_one_role(&content, "ROLE_ONLY_CONFIG_BODY");
}

#[test]
fn materialize_direct_matrix_null_context_keeps_global_context_and_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join(".ac").join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        matrix_root.join("config.json"),
        r#"{"tooling":{},"context":[null]}"#,
    )
    .expect("write config");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "# Direct Role\n\nNULL_CONFIG_ROLE_BODY\n",
    )
    .expect("write Role.md");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_global_context_before_one_role(&content, "NULL_CONFIG_ROLE_BODY");
}

#[test]
fn materialize_agent_context_file_includes_root_context_and_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    crate::config::root_agent::ensure_root_agent_dir_at(&root).expect("ensure root agent dir");
    let root_context_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);

    let content = issue_2832_combined(&root, root.parent().unwrap(), false);

    // The complete default owns policy and supplement; Role follows it.
    let prologue = content
        .find("# AgentsCommander Root Runtime Context")
        .expect("prologue present");
    let supplement = content
        .find("You are the AgentsCommander Root Agent")
        .expect("root supplement present");
    let role = content
        .find("You are the personal Root Agent for AgentsCommander.")
        .expect("role present");
    assert!(
        prologue < supplement && supplement < role,
        "prologue, then the Root supplement, then Role"
    );
    assert!(content.contains("## Core Concepts"));
    assert!(content.contains("## GOLDEN RULE"));
    assert!(content.contains("verified Room orchestrator replicas only"));
    assert_eq!(
        content.matches("Root messaging is **file-based**").count(),
        1,
        "root operational messaging comes only from the selected file"
    );
    assert!(!content.contains("Direct file-based workgroup messaging is not available"));

    // Editing the file removes all default prose from final materialization.
    std::fs::write(
        &root_context_path,
        "# Live Root Context\n\nLIVE_ROOT_CONTEXT_BODY\n",
    )
    .expect("edit root context");

    let content = issue_2832_combined(&root, root.parent().unwrap(), false);

    assert!(content.contains("LIVE_ROOT_CONTEXT_BODY"));
    assert!(!content.contains("You are the AgentsCommander Root Agent"));
    assert!(!content.contains("# AgentsCommander Root Runtime Context"));
    assert!(!content.contains("## Core Concepts"));
    assert!(!content.contains("## GOLDEN RULE"));
    assert_eq!(
        content.matches("Root messaging is **file-based**").count(),
        0,
        "an edited Root file suppresses the removed messaging prose"
    );
    assert!(content.contains("You are the personal Root Agent for AgentsCommander."));
}

#[test]
fn role_frontmatter_stripped_in_replica_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("AgentsCommander_ac");
    let ac_root = project.join(".ac");
    let matrix_root = ac_root.join("_agent_tech-lead");
    let replica_root = ac_root.join("wg-2-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "---\nname: replica-hidden-role-name\n---\nREPLICA_ROLE_BODY_2364\n",
    )
    .expect("write Role.md");
    std::fs::write(
        replica_root.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
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

    let role_heading = content
        .find("# Context: Role.md")
        .expect("Role heading present");
    let role_section = &content[role_heading..];
    assert!(
        role_section.contains("REPLICA_ROLE_BODY_2364"),
        "{role_section}"
    );
    assert!(
        !role_section.contains("replica-hidden-role-name"),
        "frontmatter value must not leak into the Role section: {role_section}"
    );
    assert!(
        !role_section.contains("name:"),
        "frontmatter key must not leak into the Role section: {role_section}"
    );
}

#[test]
fn role_frontmatter_stripped_in_direct_matrix_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join(".ac").join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        matrix_root.join("config.json"),
        r#"{"tooling":{},"context":["$AGENTSCOMMANDER_CONTEXT","Role.md"]}"#,
    )
    .expect("write config");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "---\nname: direct-hidden-role-name\n---\nDIRECT_ROLE_BODY_2364\n",
    )
    .expect("write Role.md");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert_global_context_before_one_role(&content, "DIRECT_ROLE_BODY_2364");
    assert!(
        !content.contains("direct-hidden-role-name"),
        "frontmatter value must not leak into the direct-matrix context: {content}"
    );
    let role_heading = content
        .find("# Context: Role.md")
        .expect("Role heading present");
    assert!(
        !content[role_heading..].contains("name:"),
        "frontmatter key must not leak into the Role section: {content}"
    );
}

#[test]
fn role_frontmatter_stripped_in_root_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    crate::config::root_agent::ensure_root_agent_dir_at(&root).expect("ensure root agent dir");
    std::fs::write(
        root.join("config.json"),
        r#"{"tooling":{},"context":["../Context.root-agent.md","Role.md"]}"#,
    )
    .expect("write root config");
    std::fs::write(
        root.join(ROLE_MD_FILENAME),
        "---\nname: root-hidden-role-name\n---\nROOT_ROLE_BODY_2364\n",
    )
    .expect("write root Role.md");

    let content = issue_2832_combined(&root, root.parent().unwrap(), false);

    let prologue = content
        .find("# AgentsCommander Root Runtime Context")
        .expect("prologue present");
    let supplement = content
        .find("You are the AgentsCommander Root Agent")
        .expect("root supplement present");
    let role = content
        .find("ROOT_ROLE_BODY_2364")
        .expect("role body present");
    assert!(
        prologue < supplement && supplement < role,
        "prologue, then the Root supplement, then Role"
    );
    assert!(
        !content.contains("root-hidden-role-name"),
        "frontmatter value must not leak into the Root context: {content}"
    );
    let role_heading = content
        .find("# Context: Role.md")
        .expect("Role heading present");
    assert!(
        !content[role_heading..].contains("name:"),
        "frontmatter key must not leak into the Role section: {content}"
    );
}

#[test]
fn role_frontmatter_strip_handles_lf_crlf_and_bom() {
    assert_eq!(
        strip_yaml_frontmatter("---\nname: X\n---\nBODY\n"),
        "BODY\n"
    );
    assert_eq!(
        strip_yaml_frontmatter("---\r\nname: X\r\n---\r\nBODY\r\n"),
        "BODY\r\n"
    );
    assert_eq!(
        strip_yaml_frontmatter("\u{FEFF}---\nname: X\n---\nBODY\n"),
        "BODY\n"
    );
}

#[test]
fn role_md_unterminated_frontmatter_is_preserved() {
    assert_eq!(
        strip_yaml_frontmatter("---\nname: X\nBODY\n"),
        "---\nname: X\nBODY\n"
    );
    assert_eq!(strip_yaml_frontmatter("---x\nBODY\n"), "---x\nBODY\n");
}

#[test]
fn non_role_context_source_keeps_frontmatter() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("AgentsCommander_ac");
    let ac_root = project.join(".ac");
    let matrix_root = ac_root.join("_agent_tech-lead");
    let replica_root = ac_root.join("wg-2-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "---\nname: hidden-role-name-2364\n---\nROLE_BODY_2364\n",
    )
    .expect("write Role.md");
    std::fs::write(
        replica_root.join("notes.md"),
        "---\ntitle: keep-notes-title\n---\nNOTES_BODY_2364\n",
    )
    .expect("write notes.md");
    std::fs::write(
        replica_root.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT","notes.md"]}"#,
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

    assert!(
        content.contains("---\ntitle: keep-notes-title\n---\nNOTES_BODY_2364"),
        "a non-Role source keeps its frontmatter: {content}"
    );
    assert!(content.contains("ROLE_BODY_2364"), "{content}");
    assert!(
        !content.contains("hidden-role-name-2364"),
        "Role.md frontmatter must be stripped: {content}"
    );
}

#[test]
fn role_frontmatter_strip_keeps_heading_and_separator_bytes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let matrix_root = temp.path().join(".ac").join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        matrix_root.join("config.json"),
        r#"{"tooling":{},"context":["$AGENTSCOMMANDER_CONTEXT","Role.md"]}"#,
    )
    .expect("write config");
    std::fs::write(
        matrix_root.join(ROLE_MD_FILENAME),
        "---\nname: separator-hidden-role-name\n---\nROLE_BODY_2364\n",
    )
    .expect("write Role.md");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(
        content.contains("\n\n---\n\n# Context: Role.md\n\nROLE_BODY_2364"),
        "separator and heading bytes must be unchanged: {content}"
    );
}

#[test]
fn materialize_root_context_seeds_boundary_audit_skill_without_prior_bootstrap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(
        root.join("config.json"),
        r#"{"tooling":{},"context":["$AGENTSCOMMANDER_CONTEXT","../Context.root-agent.md","Role.md"]}"#,
    )
    .expect("write config");
    std::fs::write(root.join("Role.md"), "# Root Role\n\nROOT_ROLE_BODY\n")
        .expect("write role");
    std::fs::write(
        temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
        "# Root Context\n\nROOT_TEMPLATE_BODY\n{{SKILLS_LIST}}\n",
    )
    .expect("write root context");
    let skill = root
        .join("skills")
        .join("role-skill-boundary-audit")
        .join("SKILL.md");

    let content = issue_2832_combined(&root, root.parent().unwrap(), false);

    assert!(skill.is_file());
    assert!(content.contains("role-skill-boundary-audit"));
    assert!(content.contains("Scope: Root Agent durable skills"));
    assert!(content.contains("Audit where governance instructions belong (Role.md"));
}

#[test]
fn materialize_root_context_recreates_missing_boundary_audit_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    crate::config::root_agent::ensure_root_agent_dir_at(&root).expect("ensure root");
    let skill = root
        .join("skills")
        .join("role-skill-boundary-audit")
        .join("SKILL.md");
    std::fs::remove_file(&skill).expect("remove skill");

    let content = issue_2832_combined(&root, root.parent().unwrap(), false);

    assert!(skill.is_file());
    assert!(content.contains("role-skill-boundary-audit"));
    assert!(content.contains("Scope: Root Agent durable skills"));
}

#[test]
fn materialize_root_context_never_consumes_a_ac_ancestor_global_template() {
    // #979 G.6, the inverse of the deleted
    // `materialize_root_context_uses_standalone_sibling_global_template`. An old
    // Root config still carrying the token, plus a custom global under a `.ac`
    // ancestor (the dev/WG layout, where `config_dir()` lives inside `.ac`): the
    // sentinel must not reach the output, and the file must not be read into it,
    // rewritten, or created.
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_dir = temp.path().join(".ac");
    let root = ac_dir
        .join("wg-1-demo")
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root under a .ac ancestor");
    let global_template_path = ac_dir.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let sentinel = "CUSTOM_STANDALONE_GLOBAL {{AGENT_ROOT}}";
    std::fs::write(&global_template_path, sentinel).expect("write global template");
    std::fs::write(
        root.join("config.json"),
        r#"{"tooling":{},"context":["$AGENTSCOMMANDER_CONTEXT","../Context.root-agent.md","Role.md"]}"#,
    )
    .expect("write config");
    std::fs::write(root.join("Role.md"), "# Root Role\n\nROOT_ROLE_BODY\n")
        .expect("write role");
    std::fs::write(
        root.parent()
            .expect("root parent")
            .join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
        "# Root Context\n\nROOT_TEMPLATE_BODY\n{{AGENT_ROOT}}\n",
    )
    .expect("write root context");

    let content = issue_2832_combined(&root, root.parent().unwrap(), false);

    // The sentinel never reaches the Root's prompt...
    assert!(!content.contains("CUSTOM_STANDALONE_GLOBAL"));
    // ...and the file is byte-identical: not synced, not healed, not rewritten.
    assert_eq!(
        std::fs::read_to_string(&global_template_path).expect("read global template"),
        sentinel
    );

    // No removed default section reappears; authored body and Role keep order.
    for heading in [
        "# AgentsCommander Root Runtime Context",
        "## Core Concepts",
        "## GOLDEN RULE",
        "## Delegated Task Reporting",
        "## CLI executable",
        "## Session credentials",
        "## Inter-Agent Messaging",
    ] {
        assert_eq!(
            content.matches(heading).count(),
            0,
            "removed Root block {} must stay absent",
            heading
        );
    }
    assert_contains_canonical_path(&content, &root);
    let supplement = content
        .find("ROOT_TEMPLATE_BODY")
        .expect("supplement present");
    let role = content.find("ROOT_ROLE_BODY").expect("role present");
    assert!(supplement < role, "the Root supplement precedes Role");

    // With the global ABSENT, Root creates none.
    std::fs::remove_file(&global_template_path).expect("remove global template");
    issue_2832_combined(&root, root.parent().unwrap(), false);
    assert!(
        !global_template_path.exists(),
        "Root must never create a project global context template"
    );
}

#[test]
fn materialize_agent_context_file_ignores_standalone_agent_named_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let standalone_root = temp.path().join("_agent_notes");
    std::fs::create_dir_all(&standalone_root).expect("create standalone root");
    let existing_context = "user-authored prompt content\n";
    std::fs::write(standalone_root.join("AGENTS.md"), existing_context)
        .expect("write existing prompt");

    let materialized = materialize_agent_context_file(
        &path_string(&standalone_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context should not error");

    assert!(
        materialized.is_none(),
        "standalone _agent_* directories are not canonical Agent Matrix roots"
    );
    assert_eq!(
        std::fs::read_to_string(standalone_root.join("AGENTS.md"))
            .expect("read existing prompt"),
        existing_context
    );
}

// ── #621 context-cache GC sweep tests ────────────────────────────────────

#[test]
fn sweep_removes_stale_keeps_fresh_and_non_cache() {
    use std::time::{Duration, SystemTime};
    let tmp = tempfile::tempdir().expect("temp dir");
    let dir = tmp.path();

    let old = SystemTime::now() - Duration::from_secs(40 * 24 * 60 * 60); // 40d
    let mk = |name: &str, backdate: bool| {
        let p = dir.join(name);
        std::fs::write(&p, "x").unwrap();
        if backdate {
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(old).unwrap();
        }
        p
    };

    let stale_replica = mk("replica-context-111.md", true);
    let stale_matrix = mk("matrix-context-222.md", true);
    let stale_global = mk("ac-context-333.md", true);
    let fresh_replica = mk("replica-context-444.md", false); // live agent, just launched
    let other_md = mk("notes.md", true); // not a context prefix
    let other_txt = mk("replica-context-555.txt", true); // wrong extension

    let removed = sweep_context_cache_dir(
        dir,
        SystemTime::now(),
        Duration::from_secs(30 * 24 * 60 * 60),
    );

    assert_eq!(removed, 3, "the three stale generated files are removed");
    assert!(!stale_replica.exists());
    assert!(!stale_matrix.exists());
    assert!(!stale_global.exists());
    assert!(fresh_replica.exists(), "a freshly-written cache survives");
    assert!(other_md.exists(), "non-context .md is never touched");
    assert!(other_txt.exists(), "non-.md is never touched");
}

#[test]
fn sweep_missing_dir_is_noop() {
    use std::time::{Duration, SystemTime};
    let tmp = tempfile::tempdir().expect("temp dir");
    let missing = tmp.path().join("does-not-exist");
    assert_eq!(
        sweep_context_cache_dir(&missing, SystemTime::now(), Duration::ZERO),
        0
    );
}

#[test]
fn sweep_keeps_file_with_future_mtime() {
    // (#621 LOW-3d) clock skew: a file whose mtime is AHEAD of `now` makes
    // `now.duration_since(mtime)` return Err -> KEEP (never delete on skew).
    use std::time::{Duration, SystemTime};
    let tmp = tempfile::tempdir().expect("temp dir");
    let p = tmp.path().join("replica-context-future.md");
    std::fs::write(&p, "x").unwrap();
    let future = SystemTime::now() + Duration::from_secs(10 * 24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(future)
        .unwrap();
    // Even with a zero retention, a future mtime must not be deleted.
    assert_eq!(
        sweep_context_cache_dir(tmp.path(), SystemTime::now(), Duration::ZERO),
        0
    );
    assert!(p.exists(), "future-mtime file kept");
}

#[test]
fn is_generated_context_filename_matches_three_prefixes_only() {
    assert!(is_generated_context_filename("ac-context-1.md"));
    assert!(is_generated_context_filename("replica-context-1.md"));
    assert!(is_generated_context_filename("matrix-context-1.md"));
    assert!(!is_generated_context_filename("ac-context-1.txt"));
    assert!(!is_generated_context_filename("random.md"));
    assert!(!is_generated_context_filename("context-1.md"));
}
