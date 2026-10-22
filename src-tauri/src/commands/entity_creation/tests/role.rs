use super::*;

/// Test #21 — byte-for-byte golden for the no-template scaffold. Baseline
/// since #1005 S5: the token-minimized Source of Truth / Agent Memory Rule
/// bodies (before that, the pre-#271 format). The format string is
/// reproduced verbatim here (NOT imported from the helper) so the two
/// transcriptions can cross-check each other — any drift in
/// `build_role_content` instantly fails this test.
#[test]
fn build_role_content_no_template_matches_legacy() {
    let safe_name = "alpha";
    let description = "Test agent description.";
    let desc_yaml = description.replace('\'', "''");
    let golden = format!(
        "---\nname: '{}'\ndescription: '{}'\ntype: agent\n---\n\n# {}\n\n{}\n\n## Source of Truth\n\nThis role is defined in Role.md of your Agent Matrix at: .ac/_agent_{}/\nIf you are running as a replica, this file was generated from that source; the Agent Matrix is canonical.\n\n## Agent Memory Rule\n\nYour Agent Matrix's memory/, plans/, skills/, and Role.md are the single source of persistent knowledge. Use your replica folder only for replica-local scratch, inbox/outbox, and session artifacts. NEVER use external memory systems from the coding agent (e.g., ~/.claude/projects/memory/).\n",
        safe_name, desc_yaml, safe_name, description, safe_name
    );
    let actual = build_role_content(safe_name, description, None);
    assert_eq!(
        actual, golden,
        "no-template Role.md must be byte-identical to the #1005 S5 golden"
    );
}

/// Test #22 — supplying a resolved template inserts the `## Role Profile`
/// section between the heading/description and the Source of Truth section,
/// using the template body verbatim (post-trim). Body must be fenced by the
/// plan §7.2 HTML-comment delimiters with the opening tag carrying the
/// resolved template id as `source="…"`.
#[test]
fn build_role_content_with_template_inserts_role_profile() {
    let template = crate::commands::role_templates::ResolvedRoleTemplate {
        id: "agency:my-test".into(),
        body: "Template body line one.\nTemplate body line two.".into(),
        skills_src: None,
    };
    let out = build_role_content("alpha", "Test description.", Some(&template));
    assert!(
        out.contains("## Role Profile"),
        "must add a Role Profile section: {}",
        out
    );
    assert!(
        out.contains("Template body line one."),
        "must include the template body verbatim"
    );
    assert!(
        out.contains("Template body line two."),
        "must include subsequent template body lines"
    );
    // Plan §7.2: opening delimiter carries provenance via source="<id>".
    assert!(
        out.contains("<!-- ac:role-profile source=\"agency:my-test\""),
        "must include opening ac:role-profile delimiter with template id: {}",
        out
    );
    // Plan §7.2: closing delimiter fences the imported body.
    assert!(
        out.contains("<!-- ac:role-profile:end -->"),
        "must include closing ac:role-profile:end delimiter: {}",
        out
    );
    // Role Profile must come BEFORE Source of Truth in the file.
    let profile_idx = out.find("## Role Profile").expect("Role Profile present");
    let sot_idx = out
        .find("## Source of Truth")
        .expect("Source of Truth present");
    assert!(
        profile_idx < sot_idx,
        "Role Profile must precede Source of Truth"
    );
    // Delimiters must bracket the body — opening before, closing after.
    let open_idx = out
        .find("<!-- ac:role-profile source=")
        .expect("opening delimiter present");
    let close_idx = out
        .find("<!-- ac:role-profile:end -->")
        .expect("closing delimiter present");
    let body_idx = out
        .find("Template body line one.")
        .expect("template body present");
    assert!(
        open_idx < body_idx && body_idx < close_idx,
        "delimiters must bracket the imported body"
    );
    // Closing delimiter must come BEFORE the mandatory AC sections so the
    // AC sections remain outside the fenced template region.
    assert!(
        close_idx < sot_idx,
        "closing delimiter must precede Source of Truth (mandatory AC sections stay last)"
    );
}

/// Test #23 — mandatory sections are always last, even when a template body
/// itself contains a heading that LOOKS like a mandatory section. The
/// template is inserted between description and the mandatory block, never
/// after it; the actual `## Source of Truth` and `## Agent Memory Rule`
/// added by the helper must therefore appear AFTER the template body.
#[test]
fn build_role_content_keeps_mandatory_sections_last() {
    let template = crate::commands::role_templates::ResolvedRoleTemplate {
        id: "agency:tricky".into(),
        body: "Body before.\n\n## Source of Truth\n\nLook-alike heading inside template.\n\n## Agent Memory Rule\n\nLook-alike heading.\n\nBody after.".into(),
        skills_src: None,
    };
    let out = build_role_content("alpha", "desc", Some(&template));
    // The mandatory block's exact opening line appears in both the template
    // body AND the helper's tail — so `rfind` must point at the helper's
    // copy, which has to sit AFTER the entire template body.
    let last_sot = out
        .rfind("## Source of Truth")
        .expect("Source of Truth heading present");
    let last_memory_rule = out
        .rfind("## Agent Memory Rule")
        .expect("Agent Memory Rule heading present");
    let body_after = out.find("Body after.").expect("template body retained");
    assert!(
        last_sot > body_after,
        "the helper's `## Source of Truth` must come AFTER the template body"
    );
    assert!(
        last_memory_rule > last_sot,
        "`## Agent Memory Rule` must come last"
    );
    // And the helper's tail content must still be present verbatim.
    assert!(
        out.contains("This role is defined in Role.md of your Agent Matrix"),
        "mandatory Source of Truth body must still be appended"
    );
    assert!(
        out.contains("NEVER use external memory systems from the coding agent"),
        "mandatory Agent Memory Rule body must still be appended"
    );
}

/// Test #24 — single quotes in the description are doubled inside the YAML
/// frontmatter `description:` value so the file stays valid YAML, while the
/// human-readable body keeps the original character.
#[test]
fn build_role_content_escapes_single_quote_in_description() {
    let desc = "Don't break the YAML 'header'.";
    let out = build_role_content("alpha", desc, None);
    // YAML line uses single-quoted value with doubled apostrophes.
    assert!(
        out.contains("description: 'Don''t break the YAML ''header''.'"),
        "YAML description must double-escape single quotes; got:\n{}",
        out
    );
    // Body keeps the original (un-doubled) description.
    assert!(
        out.contains("Don't break the YAML 'header'."),
        "human-readable description body must keep the original characters; got:\n{}",
        out
    );
}

#[test]
fn create_agent_matrix_from_role_writes_role_bytes_exactly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("workspace");
    let role_bytes = b"\xEF\xBB\xBF# Variant\r\n\r\nKeep bytes.\r\n";

    let created = create_agent_matrix_from_role(CreateAgentMatrixFromRoleArgs {
        ac_root: &ac_root,
        safe_name: "tech-lead-control",
        role_bytes,
    })
    .expect("create matrix from role");

    assert_eq!(
        std::fs::read(&created.role_path).expect("read role"),
        role_bytes
    );
    assert!(created.agent_dir.join("config.json").is_file());
    for dir in AGENT_MATRIX_DIRS {
        assert!(
            created.agent_dir.join(dir).is_dir(),
            "missing expected matrix dir {}",
            dir
        );
    }
}

#[test]
fn create_agent_matrix_from_role_rejects_existing_target() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(ac_root.join("_agent_tech-lead-control")).expect("existing target");

    let err = create_agent_matrix_from_role(CreateAgentMatrixFromRoleArgs {
        ac_root: &ac_root,
        safe_name: "tech-lead-control",
        role_bytes: b"# Role\n",
    })
    .unwrap_err();

    assert!(err.contains("already exists"), "unexpected error: {}", err);
}

#[test]
fn create_agent_matrix_from_role_requires_slug_identity() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("ProjectAlpha");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("workspace");

    let err = create_agent_matrix_from_role(CreateAgentMatrixFromRoleArgs {
        ac_root: &ac_root,
        safe_name: "Tech Lead",
        role_bytes: b"# Role\n",
    })
    .unwrap_err();

    assert!(err.contains("lowercase slug"), "unexpected error: {}", err);
    assert!(!ac_root.join("_agent_Tech Lead").exists());
}

// ── #621 workgroup-delete clock + cache cleanup (integration) ─────────────
