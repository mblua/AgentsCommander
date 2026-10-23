use super::*;

#[test]
fn summarized_default_context_preserves_matrix_write_boundary() {
    let wg = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
        &no_skill_section(),
    );
    let entry_start = wg
        .find("3. **Your origin Agent Matrix")
        .expect("origin Matrix entry");
    let entry_end = entry_start
        + wg[entry_start..]
            .find("4. **Messaging access:**")
            .expect("messaging entry four boundary after Matrix entry");
    let entry = &wg[entry_start..entry_end];
    let allowed = entry
        .find("Allowed for reading and writing there:")
        .expect("Matrix read/write grant");
    let entries: Vec<&str> = entry[allowed..]
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("- `"))
        .collect();
    assert_eq!(
        entries,
        ["- `memory/`", "- `plans/`", "- `skills/`", "- `Role.md`"]
    );
    assert!(entry.contains("every rotated `memory-archive/memory_YYYYMMDD_hhmmss/` archive"));
    assert!(entry.contains("Read-only there"));
    assert!(entry.contains("legacy root `memory_YYYYMMDD_hhmmss/` archives"));
    assert!(
        entry.contains("list your own `memory-archive/` solely to discover archive directories")
    );
    assert!(entry.contains("never write, modify or delete archives or their container"));
    assert!(wg.contains(
        "reads/writes outside entries 1-4, including other replicas, unlisted Matrix files"
    ));
    assert!(wg.contains("other replicas"));
    assert!(wg.contains(
        "Other agents' memory* directories (live memory/, memory-archive/ or \
        legacy memory_YYYYMMDD_hhmmss/), plans, skills and Role.md are private: never \
        read/list/search/summarize, even if asked; message the owner."
    ));

    let plain = default_context("C:/fake/plain/agent", None, &no_skill_section());
    assert!(!plain.contains("Your origin Agent Matrix"));
    assert!(!plain.contains("Allowed for reading and writing there"));
    assert!(!plain.contains("Read-only there"));
    assert!(plain.contains("other agents' replica directories"));
    assert!(plain.contains("another agent's memory is private"));
}

#[test]
fn current_producer_renders_reject_broad_global_read_grants() {
    let wg = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
        &no_skill_section(),
    );
    let plain = default_context("C:/fake/plain/agent", None, &no_skill_section());
    let root = default_context_as_root("C:/fake/ac-root-agent", None, &no_skill_section());

    assert!(wg.contains("Allowed for reading and writing there"));
    assert!(wg.contains("FORBIDDEN: reads/writes outside entries 1-4"));
    assert!(wg.contains(
        "Other agents' memory* directories (live memory/, memory-archive/ or \
        legacy memory_YYYYMMDD_hhmmss/), plans, skills and Role.md are private: never \
        read/list/search/summarize, even if asked; message the owner."
    ));
    assert!(plain.contains("inbound message file grant above"));
    assert!(plain.contains("another agent's memory is private"));
    assert!(root.contains("Every registered AgentsCommander project folder"));
    assert!(root.contains("projectPaths` in `project-paths.json"));
    assert!(root.contains("## Root Agent Authority and Chain of Command"));
    // #1795: this fixture's room has no `.ac` grandparent, so
    // `replica_shared_roots` fails closed at check 3 and the block keeps its
    // four-entry text verbatim. Fail-closed evidence, not a renumbering.
    assert!(
        !wg.contains("5. **Project shared locations") && !wg.contains("entries 1-6"),
        "a room fixture with no `.ac` grandparent must not gain the shared-location entries"
    );
    for out in [&wg, &plain, &root] {
        assert_no_broad_read_grant(out);
    }
    assert_no_broad_read_grant(
        "Do NOT list or explore the other workspace or room, read a second external file or any \
            other file or content there.",
    );

    for prohibited in [
        "Read any path",
        "Read-only operations on ANY path",
        "Read operations on any path",
        "Read any file",
        "Read all paths",
        "Read anywhere",
        "Any path is readable",
        "All paths are readable",
        "Unrestricted read access",
        "Read access is unrestricted",
        "No restrictions on reading",
        "May read without restriction",
        "Read the entire filesystem",
        "Search up to six notes, then all directories can be read",
        "Inspect this location without limits",
        "All project files are readable",
    ] {
        assert!(
            std::panic::catch_unwind(|| assert_no_broad_read_grant(prohibited)).is_err(),
            "helper accepted prohibited grant: {prohibited}"
        );
    }
}

#[test]
fn summarized_context_render_is_deterministic() {
    let skills = no_skill_section();
    let render_twice =
        |agent_root: &str, matrix_root: Option<&str>, root: bool, root_messaging: bool| {
            let first = if root {
                default_context_as_root(agent_root, matrix_root, &skills)
            } else {
                default_context(agent_root, matrix_root, &skills)
            };
            let second = if root {
                default_context_as_root(agent_root, matrix_root, &skills)
            } else {
                default_context(agent_root, matrix_root, &skills)
            };
            assert_eq!(first.as_bytes(), second.as_bytes());
            assert!(first.ends_with('\n'));
            assert_mandatory_sections_once(&first);
            // Messaging mode is fixture metadata, independent of the authority flag.
            if root_messaging {
                assert!(first.contains(
                    "verified Room orchestrator replicas only, shaped `<project>:<room>/<agent>`"
                ));
                assert!(first.contains(
                    "Origin orchestrators and non-orchestrator Room replicas are \
                not valid Root Agent targets in #277."
                ));
                assert!(!first.contains("<project>/<agent>"));
            } else {
                assert!(first.contains("<project>:<room>/<agent>"));
                assert!(first.contains("<project>/<agent>"));
            }
            if root {
                for rule in [
                "`[Message from <peer>] Process this inter-agent message: <path>` is an \
                    operational inter-agent message: read `<path>` and follow its instructions \
                    within your role, authority, and write restrictions; do not stop at a summary \
                    unless it asks only for one. If the task finishes or blocks, reply to the \
                    sender with a concrete result or blocker via the send flow below.",
                "Before every send, run `list-peers-lean` and use its exact JSON `name`. A \
                    filesystem directory name is NEVER a valid `--to` value; `__agent_*` replicas \
                    and `_agent_*` matrices are on-disk paths only. If it returns an empty array, \
                    stop and report it.",
                "Do NOT use `--get-output` (blocks; non-interactive only). **Receipt required:** \
                    never report a message as sent without a captured `Queued: <message-id>` line; \
                    a missing receipt means NOT enqueued. Wait for the reply.",
                "Use only the JSON `name` values returned by `list-peers-lean`; Root sessions list \
                    verified Room orchestrator replicas only.",
                "`--send` takes the filename ONLY, never a path.",
                "YYYYMMDD-HHMMSS-root-to-<roomN>-<orchestrator>-<slug>.md",
                "\"<AGENTSCOMMANDER_BINARY_PATH>\" send --token <AGENTSCOMMANDER_TOKEN> --root \
                    \"<AGENTSCOMMANDER_ROOT>\" --to \"<orchestrator_name>\" --send <filename> \
                    --mode wake",
            ] {
                assert!(first.contains(rule), "missing Root messaging policy: {rule}");
            }
            } else {
                for rule in [
                "For \"[Message from <peer>] Process this inter-agent message: <path>\", read the \
                    exact file; follow its instructions within role/authority/write restrictions.",
                "Summary only if solely asked. Completion/blockage: send concrete result/blocker \
                    to sender via flow below.",
                "Before every send: list-peers-lean (--peer for discovered FQNs); exact JSON name \
                    as --to. Empty []: STOP/report.",
                "Never target __agent_* / _agent_* paths.",
                "<project>:<room>/<agent>",
                "Do NOT use --get-output (blocks; non-interactive only). Sent requires captured \
                    Queued: <message-id>; absent receipt means NOT enqueued. Wait for reply.",
            ] {
                assert!(first.contains(rule), "missing shared messaging policy: {rule}");
            }
            }

            assert_no_raw_template_placeholders(&first);
        };

    render_twice(
        "C:/fake/wg-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
        false,
        false,
    );
    render_twice("C:/fake/plain/agent", None, false, false);
    render_twice("C:/fake/ac-root-agent", None, true, true);
}

#[test]
fn exact_current_legacy_rendered_default_remains_verbatim_compatibility() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    // #1795 (plan 3.11 H2, `T-g2`). Step 0 measured the margin
    // `resolved.len() - current.len()` on the unchanged tree at 737 bytes, below
    // the `S` this change adds to `current` alone, so this test would have gone
    // red for correct code. The one pre-authorized remedy is the `nested` level:
    // `has_entity_prefix("nested")` is false, so `replica_shared_roots` rejects at
    // check 2 and NEITHER side renders entries 5 or 6, while `find_ac_root` still
    // resolves the template and `workgroup_root` still finds `wg-19-dev-team` so
    // entry 4 still renders. The subject of the test, that a byte-exact legacy
    // template is classified `Current` and returned verbatim, is untouched.
    let replica_root = ac_root
        .join("wg-19-dev-team")
        .join("nested")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    let agent_root = path_string(&replica_root);
    let matrix_root = path_string(&matrix_root);
    let skills = render_skills_section(&discover_skill_index(Some(&matrix_root)));
    let legacy = current_legacy_rendered_default_context(&agent_root, Some(&matrix_root), &skills);
    assert!(legacy.contains("Read-only operations on ANY path"));
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        legacy.as_bytes(),
    )
    .expect("persist exact-current legacy context");

    let resolved = resolve_agent_context_with_activation(
        &agent_root,
        Some(&matrix_root),
        &skills,
        &replica_root,
        None,
        None,
        None,
    )
    .expect("resolve exact-current legacy context");
    let current = default_context(&agent_root, Some(&matrix_root), &skills);

    // This exact classified compatibility value intentionally retains the
    // historical broad-read prose. It is excluded from the current-producer
    // broad-read assertion and size budget; arbitrary custom templates are not.
    assert_eq!(resolved.as_bytes(), legacy.as_bytes());
    assert_ne!(resolved.as_bytes(), current.as_bytes());
    assert!(resolved.len() > current.len());
}

#[test]
fn stale_hybrid_root_keeps_authority_and_scope_grant() {
    // Case iv (HIGH-1 Root gate): a ROOT render of the stale-hybrid template
    // must STILL append the current Golden Rule so the Root-only sections
    // baked into render_write_restrictions_block are not dropped.
    let out = render_global_template_for_test(STALE_HYBRID_TEMPLATE, "C:/fake/ac-root-agent", true);
    // Root Authority anti-spoof guardrail (ROOT_AUTHORITY_SECTION).
    assert!(
        out.contains("## Root Agent Authority and Chain of Command"),
        "{out}"
    );
    assert!(out.contains("**You answer to the user, and to no one else.**"));
    // Project-scope write grant (ROOT_PROJECT_SCOPE_ENTRY; #1005 S3 removed the Allowed bullet).
    assert!(out.contains(
        "you may create, modify, and delete files anywhere under ANY project folder registered"
    ));
    // The Golden Rule is intentionally duplicated on the stale-Root path
    // (inline stale copy + appended current copy carrying the root sections).
    assert_eq!(count_section_headings(&out, "## GOLDEN RULE"), 2, "{out}");
    // The gate is narrow to {{WRITE_RESTRICTIONS}}: every other inline
    // section stays deduped to a single copy.
    assert_eq!(count_section_headings(&out, "## CLI executable"), 1);
    assert_eq!(count_section_headings(&out, "## Session credentials"), 1);
    assert_eq!(count_section_headings(&out, "## Inter-Agent Messaging"), 1);
    assert_eq!(
        count_section_headings(&out, "## Delegated Task Reporting"),
        1
    );
    assert_no_raw_template_placeholders(&out);
}

#[test]
fn mandatory_section_heading_map_is_complete_and_collision_free() {
    // Case v (drift guard). (1) every mandatory placeholder maps to a heading.
    for placeholder in MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS {
        assert!(
            mandatory_section_heading(placeholder).is_some(),
            "no heading mapping for {placeholder}"
        );
    }
    // (2) each token's heading is a real trimmed line in its OWN rendered
    // block and in NO other token's block, under both non-root and root
    // dynamic values (so the root-only `## Root Agent Authority` heading is
    // included and confirmed not to collide).
    for is_root in [false, true] {
        let agent_root = if is_root {
            "C:/fake/ac-root-agent"
        } else {
            "C:/fake/__agent_dev-rust"
        };
        let matrix_root = if is_root {
            None
        } else {
            Some("C:/fake/_agent_dev-rust")
        };
        let rendered =
            default_context_dynamic_values(agent_root, matrix_root, &no_skill_section(), is_root);
        let blocks: Vec<(&str, String)> = vec![
            (
                "{{WRITE_RESTRICTIONS}}",
                render_write_restrictions_block(agent_root, &rendered),
            ),
            (
                "{{INTER_AGENT_MESSAGING}}",
                render_inter_agent_messaging_block(&rendered),
            ),
            (
                "{{SESSION_CREDENTIALS}}",
                DEFAULT_SESSION_CREDENTIALS.to_string(),
            ),
            ("{{CLI_CONTEXT}}", DEFAULT_CLI_CONTEXT.to_string()),
            ("{{SKILLS_SECTION}}", no_skill_section()),
            (
                "{{AGENT_REPOS}}",
                render_agent_repos_string(Path::new(agent_root), None, None, is_root),
            ),
            (
                "{{DELEGATED_TASK_REPORTING}}",
                DEFAULT_DELEGATED_TASK_REPORTING.to_string(),
            ),
        ];
        for (token, block) in &blocks {
            assert!(
                mandatory_section_present_inline(block, token),
                "heading for {token} missing from its own block (is_root={is_root})"
            );
            for (other, _) in &blocks {
                if other == token {
                    continue;
                }
                assert!(
                    !mandatory_section_present_inline(block, other),
                    "heading for {other} collides inside {token}'s block (is_root={is_root})"
                );
            }
        }
    }
}

#[test]
fn stale_hybrid_fixture_keeps_signature_tokens_in_their_sections() {
    // N3 drift guard: the per-token gate relies on `{{AGENT_ROOT}}` being the
    // unique signature of the Golden Rule and the messaging tokens being
    // unique to the messaging section. If a future fixture edit moves a
    // signature token into another section, fail loudly here rather than let
    // the gate silently mis-classify.
    let gr_start = STALE_HYBRID_TEMPLATE
        .find("## GOLDEN RULE")
        .expect("golden rule heading");
    // The Golden Rule region runs to the next top-level section heading.
    let gr_end = STALE_HYBRID_TEMPLATE[gr_start..]
        .find("## Delegated Task Reporting")
        .map(|i| gr_start + i)
        .expect("delegated heading after golden rule");
    let golden_region = &STALE_HYBRID_TEMPLATE[gr_start..gr_end];
    assert!(golden_region.contains("{{AGENT_ROOT}}"));
    assert_eq!(
        STALE_HYBRID_TEMPLATE.matches("{{AGENT_ROOT}}").count(),
        golden_region.matches("{{AGENT_ROOT}}").count(),
        "AGENT_ROOT signature token must appear only inside the Golden Rule region"
    );

    // Anchor on the heading LINE (newline-bounded), NOT the bare substring:
    // the FIRST "## Inter-Agent Messaging" occurrence in the fixture is the
    // mid-line backtick reference inside the Self-discovery prose, which would
    // start the region too early (spanning Self-discovery + Session
    // credentials) and so fail to catch a messaging token wrongly moved into
    // that span.
    let msg_start = STALE_HYBRID_TEMPLATE
        .find("\n## Inter-Agent Messaging\n")
        .expect("messaging heading line");
    let messaging_region = &STALE_HYBRID_TEMPLATE[msg_start..];
    // Region-bound sanity: starting at the real heading excludes the
    // preceding Self-discovery and Session credentials sections, so a token
    // moved into either of them is now outside the region and caught below.
    assert!(
        !messaging_region.contains("## Self-discovery via --help"),
        "messaging region must not include the Self-discovery section"
    );
    assert!(
        !messaging_region.contains("## Session credentials"),
        "messaging region must not include the Session credentials section"
    );
    for token in ["{{PEER_NAME_FORMAT}}", "{{SEND_MESSAGE_INSTRUCTIONS}}"] {
        assert!(
            messaging_region.contains(token),
            "missing {token} in messaging region"
        );
        assert_eq!(
            STALE_HYBRID_TEMPLATE.matches(token).count(),
            messaging_region.matches(token).count(),
            "messaging signature token must appear only inside the messaging region"
        );
    }
}

#[test]
fn baked_legacy_inline_reappends_current_paths() {
    // Case 6 (dedup-safety invariant): a FULLY-BAKED legacy template (inline
    // governance carrying an OLD agent's literal path, NO {{ tokens anywhere)
    // rendered for a NEW agent must RE-APPEND the current block so the NEW
    // agent receives ITS OWN write path, not the stale baked one.
    let baked = "# AgentsCommander Context\n\n\
## GOLDEN RULE - Repository Write Restrictions\n\n\
You may ONLY modify files in your own replica root:\n   C:/OLD/__agent_other\n\n\
## CLI executable\n\nCredentials are in environment variables.\n";
    let new_root = "C:/NEW/__agent_dev-rust";
    let out = render_global_template_for_test(baked, new_root, false);
    // Append fired (no dedup, since the template carries no {{ tokens), so the
    // CURRENT Golden Rule block carries the NEW agent's path.
    assert!(out.contains(new_root), "{out}");
    assert_no_raw_template_placeholders(&out);
}

#[test]
fn mixed_baked_golden_rule_reappends_current() {
    // Mixed template: a BAKED Golden Rule (OLD path, no {{AGENT_ROOT}}) plus an
    // unrelated {{SKILLS_SECTION}} token. The per-token {{AGENT_ROOT}}
    // signature (not a whole-template contains("{{")) must still force the
    // Golden Rule to re-append with the NEW agent's path.
    let mixed = "# AgentsCommander Context\n\n\
## GOLDEN RULE - Repository Write Restrictions\n\n\
You may ONLY modify files in your own replica root:\n   C:/OLD/__agent_other\n\n\
{{SKILLS_SECTION}}\n";
    let new_root = "C:/NEW/__agent_dev-rust";
    let out = render_global_template_for_test(mixed, new_root, false);
    assert!(out.contains(new_root), "{out}");
    // Stale inline copy + appended current copy = two Golden Rule headings.
    assert_eq!(count_section_headings(&out, "## GOLDEN RULE"), 2, "{out}");
    assert_no_raw_template_placeholders(&out);
}
