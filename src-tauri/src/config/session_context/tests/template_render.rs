use super::*;

const SUMMARIZED_FINE_PLACEHOLDER_FIXTURE: &str = r#"# AgentsCommander Context

## GOLDEN RULE — Repository Access Restrictions

2. Own root: {{AGENT_ROOT}}
{{MATRIX_SECTION}}{{MESSAGING_EXCEPTION}}Replica root: {{AGENT_ROOT}}
{{MATRIX_ALLOWED}}{{MESSAGING_ALLOWED}}Forbidden: {{FORBIDDEN_SCOPE}}
Git: {{GIT_SCOPE}}

## Delegated Task Reporting

Reply with a concrete result.

{{SKILLS_SECTION}}

## CLI executable

Use the configured binary.

## Session credentials

Use the session environment.

## Inter-Agent Messaging

{{PEER_NAME_FORMAT}}
{{SEND_MESSAGE_INSTRUCTIONS}}
"#;

#[test]
fn legacy_inline_template_does_not_double() {
    // Case i: a non-root (replica) render of the stale-hybrid template must
    // NOT emit a second copy of any inline governance section.
    let out =
        render_global_template_for_test(STALE_HYBRID_TEMPLATE, "C:/fake/__agent_dev-rust", false);
    assert_eq!(count_section_headings(&out, "## GOLDEN RULE"), 1, "{out}");
    assert_eq!(count_section_headings(&out, "## CLI executable"), 1);
    assert_eq!(count_section_headings(&out, "## Session credentials"), 1);
    assert_eq!(count_section_headings(&out, "## Inter-Agent Messaging"), 1);
    assert_eq!(
        count_section_headings(&out, "## Delegated Task Reporting"),
        1
    );
    // The fine-grained tokens inside the inline copy are still filled.
    assert_no_raw_template_placeholders(&out);
    // {{AGENT_REPOS}} had neither token nor inline section, so the safety
    // net must still append it exactly once.
    assert_eq!(count_section_headings(&out, "# Agent Repos"), 1);
}

#[test]
fn incomplete_template_still_gets_section_appended() {
    // Case ii (SAFETY NET): a genuinely-missing section is still appended
    // even though another section is present inline.
    let missing_session = r#"# AgentsCommander Context

## CLI executable

Credentials are in environment variables.

{{SKILLS_SECTION}}
"#;
    let out = render_global_template_for_test(missing_session, "C:/fake/__agent_dev-rust", false);
    // `## CLI executable` was inline -> deduped to one copy.
    assert_eq!(count_section_headings(&out, "## CLI executable"), 1);
    // `## Session credentials` was absent -> the safety net appended it once.
    assert_eq!(
        count_section_headings(&out, "## Session credentials"),
        1,
        "{out}"
    );
    assert_no_raw_template_placeholders(&out);

    // Sub-case: the ONLY occurrence of "## Inter-Agent Messaging" is a
    // backtick reference mid-line (no real heading line, no token). A raw
    // `contains` would false-positive and skip the append; line-anchoring
    // must still append the real messaging block.
    let backtick_trap = concat!(
        r#"# AgentsCommander Context

## CLI executable

Credentials are in environment variables.

## Self-discovery via --help

For peer discovery, the sections below (`## Inter-Agent Messaging` and `### List available "#,
        r#"peers`) are the authoritative reference.

{{SKILLS_SECTION}}
"#
    );
    let out = render_global_template_for_test(backtick_trap, "C:/fake/__agent_dev-rust", false);
    // The real messaging block was appended (its h3 sub-heading proves it is
    // the rendered block, not the mid-line backtick reference). The backtick
    // reference is still present as prose but is not a heading line.
    assert_eq!(
        count_section_headings(&out, "## Inter-Agent Messaging"),
        1,
        "{out}"
    );
    assert!(out.contains("### Send a message to another agent"), "{out}");
    assert_no_raw_template_placeholders(&out);
}

#[test]
fn current_tokenized_template_renders_once() {
    // Case iii (regression guard): the current coarse-token default template
    // renders every mandatory section exactly once.
    let out = default_context("C:/fake/__agent_dev-rust", None, &no_skill_section());
    assert_mandatory_sections_once(&out);
    assert_no_raw_template_placeholders(&out);
}

#[test]
fn default_agent_template_keeps_coarse_placeholder_order_after_summarization() {
    let template = get_default_agent_template();
    let placeholders: Vec<&str> = template
        .match_indices("{{")
        .map(|(start, _)| {
            let suffix = &template[start..];
            let end = suffix.find("}}").expect("placeholder closes") + 2;
            &suffix[..end]
        })
        .collect();

    assert_eq!(
        placeholders,
        [
            "{{WRITE_RESTRICTIONS}}",
            "{{DELEGATED_TASK_REPORTING}}",
            "{{SKILLS_SECTION}}",
            "{{AGENT_REPOS}}",
            "{{CLI_CONTEXT}}",
            "{{HOST_PLATFORM_RULES}}",
            "{{SESSION_CREDENTIALS}}",
            "{{INTER_AGENT_MESSAGING}}",
        ]
    );
    for placeholder in MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS {
        assert_eq!(template.matches(placeholder).count(), 1, "{placeholder}");
    }
    assert!(template.contains(
        "You are in AgentsCommander, a terminal session manager coordinating multiple AI agents."
    ));
    assert!(template.contains(CORE_CONCEPTS_SECTION));
    for fine_only in [
        "{{AGENT_ROOT}}",
        "{{MATRIX_SECTION}}",
        "{{MESSAGING_EXCEPTION}}",
        "{{MATRIX_ALLOWED}}",
        "{{MESSAGING_ALLOWED}}",
        "{{FORBIDDEN_SCOPE}}",
        "{{GIT_SCOPE}}",
        "{{PEER_NAME_FORMAT}}",
        "{{SEND_MESSAGE_INSTRUCTIONS}}",
    ] {
        assert!(
            !template.contains(fine_only),
            "factory contains {fine_only}"
        );
    }
    assert!(template.ends_with('\n'));
}

/// #2031: the shipped default carries the Answering rule exactly once, after
/// the messaging token, byte-for-byte, with no U+2014.
#[test]
fn default_agent_template_carries_answering_section_once_after_messaging() {
    const SECTION: &str = "## Answering\n\nDraft, count words, then rewrite to half or fewer; \
        report both counts. Paths, commands, digests, figures, and quoted evidence are exempt: not \
        counted, not cut. Never cut a fact to reach half; stop at the smallest size that keeps \
        every fact and say so.\n\nUse plain, easy language.";
    let template = get_default_agent_template();
    assert_eq!(template.matches(SECTION).count(), 1, "{template}");
    let messaging = template
        .find("{{INTER_AGENT_MESSAGING}}")
        .expect("messaging token");
    let answering = template.find(SECTION).expect("answering section");
    assert!(
        messaging < answering,
        "## Answering must follow {{INTER_AGENT_MESSAGING}}"
    );
    assert!(
        !template.contains('\u{2014}'),
        "the default template must stay em-dash-free"
    );
}

#[test]
fn summarized_fine_placeholder_fixture_preserves_order_and_boundaries() {
    let fixture = SUMMARIZED_FINE_PLACEHOLDER_FIXTURE;
    let placeholders: Vec<&str> = fixture
        .match_indices("{{")
        .map(|(start, _)| {
            let suffix = &fixture[start..];
            let end = suffix.find("}}").expect("placeholder closes") + 2;
            &suffix[..end]
        })
        .collect();
    assert_eq!(
        placeholders,
        [
            "{{AGENT_ROOT}}",
            "{{MATRIX_SECTION}}",
            "{{MESSAGING_EXCEPTION}}",
            "{{AGENT_ROOT}}",
            "{{MATRIX_ALLOWED}}",
            "{{MESSAGING_ALLOWED}}",
            "{{FORBIDDEN_SCOPE}}",
            "{{GIT_SCOPE}}",
            "{{SKILLS_SECTION}}",
            "{{PEER_NAME_FORMAT}}",
            "{{SEND_MESSAGE_INSTRUCTIONS}}",
        ]
    );
    assert!(fixture.contains("{{MATRIX_SECTION}}{{MESSAGING_EXCEPTION}}"));
    assert!(fixture.contains("{{MATRIX_ALLOWED}}{{MESSAGING_ALLOWED}}"));
}

#[test]
fn summarized_fine_placeholder_fixture_renders_supported_shapes() {
    const SKILLS: &str = "## Skills\n\nUNIQUE_SKILLS_SENTINEL\n";
    let wg = render_agent_context_template_inner(
        SUMMARIZED_FINE_PLACEHOLDER_FIXTURE,
        "C:/fake/room-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
        SKILLS,
        Path::new("C:/fake/room-7-dev-team/__agent_architect"),
        None,
        None,
        false,
    );
    let plain = render_agent_context_template_inner(
        SUMMARIZED_FINE_PLACEHOLDER_FIXTURE,
        "C:/fake/plain/agent",
        None,
        SKILLS,
        Path::new("C:/fake/plain/agent"),
        None,
        None,
        false,
    );

    for out in [&wg, &plain] {
        assert_eq!(out.matches("UNIQUE_SKILLS_SENTINEL").count(), 1, "{out}");
        assert_eq!(count_section_headings(out, "## GOLDEN RULE"), 1, "{out}");
        assert_eq!(count_section_headings(out, "# Agent Repos"), 1, "{out}");
        assert_mandatory_sections_once(out);
        assert!(out.contains("<project>:<room>/<agent>"));
        assert!(out.contains("<project>/<agent>"));

        assert_no_raw_template_placeholders(out);
    }
    assert!(wg.contains("C:/fake/_agent_architect"));
    assert!(!wg.contains("Narrow exception — room messaging directory"));
    assert!(!plain.contains("Your origin Agent Matrix"));
    assert!(!plain.contains("Narrow exception — room messaging directory"));
    assert!(plain.contains("This session has no messaging directory"));

    assert!(wg.contains("Messages are **file-based** to avoid PTY truncation"));
    assert!(wg.contains("--send <filename> --mode wake"));
    assert!(wg.contains("filename ONLY, never a path"));
    assert!(wg.contains("YYYYMMDD-HHMMSS-<roomN>-<you>-to-<roomN>-<peer>-<slug>.md"));
    assert!(plain.contains("Do NOT search the filesystem for one."));
    assert!(plain.contains("read that file, act, and report here instead of using `send --send`"));

    let values = default_context_dynamic_values(
        "C:/fake/room-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
        SKILLS,
        false,
    );
    assert!(values.messaging_exception.is_empty());

    let normal = default_context(
        "C:/fake/room-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
        SKILLS,
    );
    assert_eq!(
        normal.matches("4. **Messaging access:**").count(),
        1,
        "{normal}"
    );

    let write_restrictions_only = render_agent_context_template_inner(
        "{{WRITE_RESTRICTIONS}}",
        "C:/fake/room-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
        SKILLS,
        Path::new("C:/fake/room-7-dev-team/__agent_architect"),
        None,
        None,
        false,
    );
    assert!(write_restrictions_only.contains("4. **Messaging access:**"));
    assert!(!write_restrictions_only.contains("Narrow exception — room messaging directory"));
}
