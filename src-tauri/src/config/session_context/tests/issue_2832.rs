use super::*;

fn root_payload(content: &str) -> &str {
    assert_eq!(content.matches("## Manual context clear").count(), 1);
    content
        .strip_suffix(super::super::default_context::SELF_CLEAR_MANUAL_SECTION)
        .expect("Root context must end with the complete manual section")
}

#[test]
fn issue_2832_default_blocks_render_once_in_configured_order() {
    let (temp, root) = issue_2832_fixture();
    let source =
        std::fs::read_to_string(temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)).unwrap();
    assert_eq!(
        source,
        crate::config::root_agent::default_root_context_template()
    );
    assert!(source.contains("{{#AUTO_SELF_CLEAR}}"));
    let out = issue_2832_combined(&root, temp.path(), true);
    for heading in [
        "# AgentsCommander Root Runtime Context",
        "## Core Concepts",
        "## GOLDEN RULE",
        "## Root Agent Authority",
        "## Delegated Task Reporting",
        "## Skills",
        "# Agent Repos",
        "## CLI executable",
        "## Host Platform Rules",
        "## Session credentials",
        "## Inter-Agent Messaging",
        "## Privileged PTY Input",
        "## Responsibility",
        "## Self-Maintenance",
    ] {
        assert_eq!(out.matches(heading).count(), 1, "{heading}");
    }
    assert!(!out.contains("{{"));
    assert!(out.contains("\n\n---\n\n# Context: Role.md\n\n# Role\nROOT ROLE\n"));
    assert!(!out.contains("name: root"));
    assert!(out.find("## Editing this context").unwrap() < out.find("ROOT ROLE").unwrap());
    assert!(!out.contains("# Orchestrator Context"));
    assert!(!temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME).exists());
    for filename in [
        HOST_PLATFORM_RULES_FILENAME_WINDOWS,
        HOST_PLATFORM_RULES_FILENAME_LINUX,
        HOST_PLATFORM_RULES_FILENAME_MACOS,
    ] {
        assert!(!temp.path().join(filename).exists());
    }
}

#[test]
fn issue_2832_edits_deletions_and_empty_content_remain_authoritative() {
    let (temp, root) = issue_2832_fixture();
    let path = temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let default = crate::config::root_agent::default_root_context_template();
    let mut edited = default.replace(
        "**You answer to the user, and to no one else.**",
        "AUTHORED POLICY SENTENCE",
    );
    for (start, end) in [
        ("## Inter-Agent Messaging", "## Privileged PTY Input"),
        ("## Privileged PTY Input", "# Agents Commander"),
    ] {
        let a = edited.find(start).unwrap();
        let b = edited[a..].find(end).unwrap() + a;
        edited.replace_range(a..b, "");
    }
    let a = edited.find("## Root Agent Authority").unwrap();
    let b = edited[a..].find("## Delegated Task Reporting").unwrap() + a;
    edited.replace_range(a..b, "AUTHORED POLICY SENTENCE\n\n");
    for authored in [edited.as_str(), "OPERATOR TEXT WITHOUT TOKENS\n", ""] {
        std::fs::write(&path, authored).unwrap();
        for _ in 0..2 {
            crate::config::seeded_context_templates::ensure_root_context_template(temp.path())
                .unwrap();
            let out = issue_2832_combined(&root, temp.path(), false);
            assert!(!out.contains("## Root Agent Authority"));
            assert!(!out.contains("## Privileged PTY Input"));
            assert!(!out.contains("## Inter-Agent Messaging"));
            assert!(out.contains("ROOT ROLE"));
            if authored.is_empty() {
                assert!(out.starts_with("\n\n---\n\n# Context: Role.md"));
            } else if authored == edited {
                assert!(out.contains("AUTHORED POLICY SENTENCE"));
            } else {
                assert!(out.starts_with(authored));
                assert!(!out.contains("## GOLDEN RULE"));
            }
            assert_eq!(std::fs::read_to_string(&path).unwrap(), authored);
        }
    }
}

#[test]
fn issue_2832_custom_context_arrays_order_and_raw_files() {
    let (temp, root) = issue_2832_fixture();
    std::fs::write(
        root.join("Context.root-agent.md"),
        "RAW {{AGENT_ROOT}}\n{{#HOST_WINDOWS}}\nRAW CONDITION\n{{/HOST_WINDOWS}}\n",
    )
    .unwrap();
    std::fs::write(root.join("extra.md"), "EXTRA").unwrap();
    for (entries, expected) in [
        (serde_json::json!([]), ""),
        (serde_json::json!(["Role.md"]), "# Role\nROOT ROLE\n"),
    ] {
        std::fs::write(
            root.join("config.json"),
            serde_json::json!({"context":entries}).to_string(),
        )
        .unwrap();
        let out = issue_2832_combined(&root, temp.path(), true);
        assert_eq!(root_payload(&out), expected);
    }
    std::fs::write(
        root.join("config.json"),
        serde_json::json!({"context":["extra.md", "../\
        Context.root-agent.md", "Context.root-agent.md", "Role.md"]})
        .to_string(),
    )
    .unwrap();
    let out = issue_2832_combined(&root, temp.path(), false);
    let out = root_payload(&out);
    assert!(out.starts_with("EXTRA\n\n---\n\n# Context: Context.root-agent.md"));
    assert!(out.contains("RAW {{AGENT_ROOT}}\n{{#HOST_WINDOWS}}\nRAW CONDITION"));
    assert!(out.find("EXTRA").unwrap() < out.find("## GOLDEN RULE").unwrap());
    assert!(out.find("## GOLDEN RULE").unwrap() < out.find("RAW {{AGENT_ROOT}}").unwrap());
}

#[test]
fn issue_2832_missing_invalid_and_directory_bases_fail_before_overrides() {
    let (temp, root) = issue_2832_fixture();
    let base = temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(
        temp.path().join("Context.root-agent.local.md"),
        "VALID OVERRIDE",
    )
    .unwrap();
    std::fs::write(&base, [0xff, 0xfe]).unwrap();
    assert!(
        build_root_agent_context_at(&path_string(&root), temp.path(), None, false)
            .unwrap_err()
            .contains("Failed to read context file")
    );
    std::fs::remove_file(&base).unwrap();
    assert!(
        build_root_agent_context_at(&path_string(&root), temp.path(), None, false)
            .unwrap_err()
            .contains("missing context files")
    );
    std::fs::create_dir(&base).unwrap();
    assert!(
        build_root_agent_context_at(&path_string(&root), temp.path(), None, false)
            .unwrap_err()
            .contains("Failed to read context file")
    );
    std::fs::write(root.join("config.json"), "{").unwrap();
    assert!(
        build_root_agent_context_at(&path_string(&root), temp.path(), None, false)
            .unwrap_err()
            .contains("Failed to parse")
    );
}

#[test]
fn issue_2832_overrides_render_once_empty_suppresses_and_rejection_falls_back() {
    let (temp, root) = issue_2832_fixture();
    let overlay = temp.path().join("Context.root-agent.local.md");
    std::fs::write(
        &overlay,
        "LOCAL {{AGENT_ROOT}} {{UNKNOWN}}\n{{#AUTO_SELF_CLEAR}}\nAUTHORED \
        CLEAR\n{{/AUTO_SELF_CLEAR}}\n",
    )
    .unwrap();
    let on = issue_2832_combined(&root, temp.path(), true);
    assert!(on.contains("AUTHORED CLEAR"));
    assert!(on.contains("{{UNKNOWN}}"));
    assert!(!on.contains("{{AGENT_ROOT}}"));
    assert!(!on.contains("## GOLDEN RULE"));
    assert!(!issue_2832_combined(&root, temp.path(), false).contains("AUTHORED CLEAR"));
    std::fs::write(&overlay, "").unwrap();
    assert!(!issue_2832_combined(&root, temp.path(), true).contains("## Self-Maintenance"));
    std::fs::write(&overlay, [0xff]).unwrap();
    assert!(issue_2832_combined(&root, temp.path(), false).contains("## GOLDEN RULE"));
}

#[test]
fn issue_2832_condition_errors_repetition_unknowns_and_single_pass_values() {
    let path = Path::new("fixture/Context.root-agent.md");
    let values = HashMap::from([
        (
            "AGENT_ROOT",
            "path/{{APP_CONFIG_DIR}}/{{#HOST_WINDOWS}}".to_string(),
        ),
        ("APP_CONFIG_DIR", "config".to_string()),
    ]);
    let source = "{{UNKNOWN}} {{AGENT_ROOT}} \
        {{AGENT_ROOT}}\n{{#HOST_WINDOWS}}\nW\n{{/HOST_WINDOWS}}\n{{#HOST_WINDOWS}}\nW2\n{{/\
        HOST_WINDOWS}}\n{{#HOST_LINUX}}\nL\n{{/HOST_LINUX}}\n{{#HOST_MACOS}}\nM\n{{/HOST_MACOS}}\n";
    for (predicates, wanted) in [
        ([true, false, false, false], "W\nW2\n"),
        ([false, true, false, false], "L\n"),
        ([false, false, true, false], "M\n"),
        ([false, false, false, false], ""),
    ] {
        let out = render_root_template_text(source, path, &values, predicates).unwrap();
        assert_eq!(
            out,
            format!(
                "{{{{UNKNOWN}}}} path/{{{{APP_CONFIG_DIR}}}}/{{{{#HOST_WINDOWS}}}} \
            path/{{{{APP_CONFIG_DIR}}}}/{{{{#HOST_WINDOWS}}}}\n{wanted}"
            )
        );
    }
    for malformed in [
        "{{/HOST_WINDOWS}}\n",
        "{{#HOST_WINDOWS}}\n",
        "{{#HOST_WINDOWS}}\n{{#HOST_LINUX}}\n",
        "{{#HOST_WINDOWS}}\n{{/HOST_LINUX}}\n",
    ] {
        let error = render_root_template_text(malformed, path, &values, [false; 4]).unwrap_err();
        assert!(error.contains("Context.root-agent.md:"));
    }
    assert_eq!(
        render_root_template_text("", path, &values, [true; 4]).unwrap(),
        ""
    );
    assert_eq!(
        root_conditions(
            Some(&crate::pty::container_repos::RepoMountResolution::default()),
            true
        ),
        [false, false, false, true]
    );
}

#[test]
fn issue_2832_self_maintenance_is_authored_and_never_heading_rewritten() {
    let (temp, root) = issue_2832_fixture();
    let base = temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let custom = "## Self-Maintenance custom heading\nKEEP THIS\n{{#AUTO_SELF_CLEAR}}\nAUTHORED \
        AUTO\n{{/AUTO_SELF_CLEAR}}\n";
    std::fs::write(&base, custom).unwrap();
    let on = issue_2832_combined(&root, temp.path(), true);
    let off = issue_2832_combined(&root, temp.path(), false);
    for out in [&on, &off] {
        assert!(out.contains("## Self-Maintenance custom heading\nKEEP THIS"));
        assert!(!out.contains("reaches 3 such lines"));
    }
    assert!(on.contains("AUTHORED AUTO"));
    assert!(!off.contains("AUTHORED AUTO"));
    std::fs::write(&base, "## Self-Maintenance custom heading\nKEEP THIS\n").unwrap();
    assert!(!issue_2832_combined(&root, temp.path(), true).contains("AUTHORED AUTO"));
}

#[test]
fn issue_2832_modified_snapshots_survive_provisioning_and_final_materialization() {
    use sha2::{Digest, Sha256};
    let mut sources = crate::config::root_agent::frozen_root_context_snapshots().to_vec();
    sources.push(crate::config::root_agent::default_root_context_template());
    let mut cases = 0;
    for source in sources {
        for custom in [
            format!("{source} "),
            format!(" {source}"),
            format!("{source}\n"),
            source.strip_suffix('\n').unwrap().to_string(),
            source.replacen("AgentsCommander", "AgentsCommandex", 1),
            source.replace('\n', "\r\n"),
            String::new(),
            "\n".to_string(),
        ] {
            for forged in [false, true] {
                let (temp, root) = issue_2832_fixture();
                let path = temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
                std::fs::write(&path, &custom).unwrap();
                let state_path = temp.path().join(
                    crate::config::seeded_context_templates::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME,
                );
                if forged {
                    let state = serde_json::json!({
                        "schemaVersion": 1,
                        "templates": {
                            "rootAgent": {
                                "templateId": "rootAgent",
                                "currentVersion": 8,
                                "lastSeededSha256":
                                    format!("{:x}", Sha256::digest(custom.as_bytes()))
                            }
                        }
                    });
                    std::fs::write(&state_path, state.to_string()).unwrap();
                } else {
                    std::fs::remove_file(&state_path).unwrap();
                }
                std::fs::write(
                    root.join("config.json"),
                    serde_json::json!({"context":["../Context.root-agent.md"]}).to_string(),
                )
                .unwrap();
                for _ in 0..2 {
                    crate::config::seeded_context_templates::ensure_root_context_template(
                        temp.path(),
                    )
                    .unwrap();
                    let out = issue_2832_combined(&root, temp.path(), false);
                    let expected = render_root_template_text(
                        &custom,
                        &path,
                        &root_runtime_values(&path_string(&root), temp.path(), None, None),
                        root_conditions(None, false),
                    )
                    .unwrap();
                    assert_eq!(root_payload(&out), expected);
                    assert_eq!(std::fs::read(&path).unwrap(), custom.as_bytes());
                    if custom.is_empty() || custom == "\n" {
                        assert_eq!(root_payload(&out), custom);
                    }
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 208);
    println!("issue_2832 custom-byte/state materialization cases: {cases}");
}

#[test]
fn issue_2832_successful_root_resolver_and_direct_ensure_preserve_exclusions() {
    let temp = tempfile::tempdir().unwrap();
    let project_context = temp.path().join(".ac");
    let config_dir = project_context.join("room-1-demo");
    let root = config_dir.join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    let base = config_dir.join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let overlay = config_dir.join("Context.root-agent.local.md");
    let role = "## Self-Maintenance\nAUTHORED ROLE\n";
    std::fs::write(root.join("Role.md"), role).unwrap();
    let protected: Vec<PathBuf> = [&project_context, &config_dir, &root]
        .into_iter()
        .flat_map(|dir| {
            [
                GLOBAL_CONTEXT_TEMPLATE_FILENAME,
                COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
                HOST_PLATFORM_RULES_FILENAME_WINDOWS,
                HOST_PLATFORM_RULES_FILENAME_LINUX,
                HOST_PLATFORM_RULES_FILENAME_MACOS,
            ]
            .into_iter()
            .map(move |name| dir.join(name))
        })
        .collect();
    let authored = "## Self-Maintenance\nAUTHORED POLICY\n{{#AUTO_SELF_CLEAR}}\nAUTHORED \
        AUTO\n{{/AUTO_SELF_CLEAR}}\n";
    let authored_off = "## Self-Maintenance\nAUTHORED POLICY\n";
    let local = "## Self-Maintenance\nAUTHORED LOCAL\n";
    let mut cases = 0;
    for sentinels_present in [false, true] {
        for path in &protected {
            if sentinels_present {
                std::fs::write(path, "PROTECTED CONTEXT SENTINEL\n").unwrap();
            }
        }
        for (source, selected_override, entries, selected_off, selected_on) in [
            (
                authored,
                None,
                serde_json::json!(["../Context.root-agent.md"]),
                authored_off,
                "## Self-Maintenance\nAUTHORED POLICY\nAUTHORED AUTO\n",
            ),
            (
                "DELETED ALL DEFAULT SECTIONS\n",
                None,
                serde_json::json!(["../Context.root-agent.md"]),
                "DELETED ALL DEFAULT SECTIONS\n",
                "DELETED ALL DEFAULT SECTIONS\n",
            ),
            (
                "",
                None,
                serde_json::json!(["../Context.root-agent.md"]),
                "",
                "",
            ),
            (
                authored,
                Some(local),
                serde_json::json!(["../Context.root-agent.md"]),
                local,
                local,
            ),
            (
                authored,
                Some(""),
                serde_json::json!(["../Context.root-agent.md"]),
                "",
                "",
            ),
            (
                authored,
                None,
                serde_json::json!([]),
                authored_off,
                "## Self-Maintenance\nAUTHORED POLICY\nAUTHORED AUTO\n",
            ),
            (
                authored,
                None,
                serde_json::json!(["Role.md"]),
                authored_off,
                "## Self-Maintenance\nAUTHORED POLICY\nAUTHORED AUTO\n",
            ),
        ] {
            std::fs::write(&base, source).unwrap();
            match selected_override {
                Some(text) => std::fs::write(&overlay, text).unwrap(),
                None => {
                    if overlay.exists() {
                        std::fs::remove_file(&overlay).unwrap();
                    }
                }
            }
            std::fs::write(
                root.join("config.json"),
                serde_json::json!({"context":entries}).to_string(),
            )
            .unwrap();
            for auto in [false, true] {
                let expected = if entries == serde_json::json!([]) {
                    ""
                } else if entries == serde_json::json!(["Role.md"]) {
                    role
                } else if auto {
                    selected_on
                } else {
                    selected_off
                };
                for _ in 0..2 {
                    // Actual public materializer -> production resolver -> Root
                    // identity/builder -> final writer, with coordinator=true.
                    let out = issue_2832_combined(&root, &config_dir, auto);
                    assert_eq!(root_payload(&out), expected);
                    let direct = with_issue_2832_root_config(&config_dir, || {
                        let path = ensure_session_context(&path_string(&root)).unwrap();
                        std::fs::read_to_string(path).unwrap()
                    });
                    assert_eq!(
                        direct, selected_off,
                        "direct ensure renders selected file with \
                        auto=false, independently of context[]"
                    );
                    for path in &protected {
                        if sentinels_present {
                            assert_eq!(
                                std::fs::read_to_string(path).unwrap(),
                                "PROTECTED CONTEXT SENTINEL\n",
                                "{}",
                                path.display()
                            );
                        } else {
                            assert!(!path.exists(), "created excluded file {}", path.display());
                        }
                    }
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 28);
    println!("issue_2832 production resolver/direct-ensure cases: {cases}, each repeated twice");
    // The seam changes only configuration, never accepts another Root path.
    let spoof = temp.path().join("ac-root-agent");
    std::fs::create_dir(&spoof).unwrap();
    with_issue_2832_root_config(&config_dir, || {
        assert!(ensure_session_context(&path_string(&spoof))
            .unwrap_err()
            .contains("Root identity"));
    });
    assert!(!spoof.join("skills").exists());
}

// Leave a deliberate 1024-byte gap before the large overflow entries.
// Two short descriptions provide bounded padding independent of host path
// lengths. Measurements use the real renderer only to size fixture inputs;
// the assertions below still require both actual fallback paths to execute.
fn issue_2832_pad_budget_fixture(
    mut index: SkillIndex,
    render: fn(&SkillIndex) -> String,
) -> SkillIndex {
    const GAP: usize = 1024;
    let names = ["overflow-000", "overflow-001"];
    for name in names {
        let skill = index
            .skills
            .iter_mut()
            .find(|skill| skill.name == name)
            .unwrap();
        skill.description = Some("x".to_string());
    }
    assert!(index.warnings.is_empty());
    assert!(index
        .skills
        .iter()
        .all(|skill| skill.metadata_warnings.is_empty()));
    for count in 1..index.skills.len() {
        let mut prefix = index.clone();
        prefix.skills.truncate(count);
        let size = render(&prefix).len();
        if size < SKILL_INDEX_TOTAL_MAX_BYTES - 3072 {
            continue;
        }
        assert!(size < SKILL_INDEX_TOTAL_MAX_BYTES - GAP);
        let mut padding = SKILL_INDEX_TOTAL_MAX_BYTES - GAP - size;
        for name in names {
            assert!(prefix.skills.iter().any(|skill| skill.name == name));
            let extra = padding.min(SKILL_TRIGGER_TEXT_MAX_CHARS - 1);
            index
                .skills
                .iter_mut()
                .find(|skill| skill.name == name)
                .unwrap()
                .description = Some("x".repeat(1 + extra));
            padding -= extra;
        }
        assert_eq!(padding, 0);
        prefix.skills = index.skills[..count].to_vec();
        assert_eq!(render(&prefix).len(), SKILL_INDEX_TOTAL_MAX_BYTES - GAP);
        return index;
    }
    panic!("fixture did not reach the startup-context budget");
}

#[test]
fn issue_2832_skill_budget_overflow_is_data_only_and_non_root_keeps_instructions() {
    for path_width in [0, 13, 43, 59] {
        let temp = tempfile::Builder::new()
            .prefix(&format!("budget-{}", "p".repeat(path_width)))
            .tempdir()
            .unwrap();
        let root = temp.path().join("ac-root-agent");
        std::fs::create_dir(&root).unwrap();
        crate::config::seeded_context_templates::ensure_root_context_template(temp.path()).unwrap();
        crate::config::root_agent::ensure_default_root_agent_skills_at(&root).unwrap();
        std::fs::write(
            root.join("config.json"),
            serde_json::json!({"context":["../Context.root-agent.md"]}).to_string(),
        )
        .unwrap();
        std::fs::write(
            temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
            "{{SKILLS_LIST}}",
        )
        .unwrap();
        for index in 0..200 {
            write_skill(
                &root,
                &format!("overflow-{index:03}"),
                &format!(
                    "---\nname: overflow-{index:03}\ndescription: {}\n---\nBODY\n",
                    "x".repeat(2048)
                ),
            );
        }
        let initial = discover_skill_index(Some(&path_string(&root)));
        let tuned = issue_2832_pad_budget_fixture(initial, render_root_skills_list);
        for name in ["overflow-000", "overflow-001"] {
            let skill = tuned
                .skills
                .iter()
                .find(|skill| skill.name == name)
                .unwrap();
            write_skill(
                &root,
                name,
                &format!(
                    "---\nname: {name}\ndescription: {}\n---\nBODY\n",
                    skill.description.as_ref().unwrap()
                ),
            );
        }
        let rendered = issue_2832_combined(&root, temp.path(), true);
        assert!(
            rendered.len()
                <= SKILL_INDEX_TOTAL_MAX_BYTES
                    + super::super::default_context::SELF_CLEAR_MANUAL_SECTION.len()
        );
        let rendered = root_payload(&rendered);
        assert!(rendered.len() <= SKILL_INDEX_TOTAL_MAX_BYTES);
        assert!(
            rendered.contains("Metadata omitted because"),
            "minimal-entry fallback exercised"
        );
        assert!(
            rendered.contains("budget reached; omitted"),
            "summary fallback exercised"
        );
        assert!(rendered.contains("Entrypoint:"));
        assert!(!rendered.contains("inspect SKILL.md"));
        assert!(!rendered.contains("Inspect SKILL.md"));
        assert!(!rendered.contains("## Skills"));
        assert!(!rendered.contains("Self-Maintenance"));
        let index = discover_skill_index(Some(&path_string(&root)));
        let non_root_index = issue_2832_pad_budget_fixture(index, render_skills_section);
        let non_root = render_skills_section(&non_root_index);
        assert!(non_root.len() <= SKILL_INDEX_TOTAL_MAX_BYTES);
        assert!(non_root.contains("Inspect SKILL.md files if needed."));
        assert!(non_root.contains("inspect SKILL.md if needed."));
        let missing_description = SkillIndex {
            matrix_root: None,
            skills_root: None,
            skills: vec![SkillMetadata {
                folder_name: "no-description".to_string(),
                name: "no-description".to_string(),
                entrypoint_path: "skills/no-description/SKILL.md".to_string(),
                description: None,
                when_to_use: None,
                metadata_warnings: vec![],
            }],
            warnings: vec![],
        };
        assert!(render_root_skills_list(&missing_description).contains("No description metadata."));
        assert!(!render_root_skills_list(&missing_description).contains("inspect"));
        assert!(
            render_skills_section(&missing_description).contains("inspect SKILL.md before use.")
        );
        assert!(crate::config::root_agent::default_root_context_template()
            .contains("inspect the canonical SKILL.md files if needed."));
        println!("issue_2832 overflow host-path padding: {path_width}");
    }
}

#[test]
fn issue_2832_runtime_skills_and_repos_refresh_without_instructional_helpers() {
    let (temp, root) = issue_2832_fixture();
    std::fs::write(
        temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
        "{{SKILLS_LIST}}\n{{AGENT_REPOS_LIST}}\n",
    )
    .unwrap();
    let first = issue_2832_combined(&root, temp.path(), false);
    assert!(first.contains("No repos configured"));
    write_skill(
        &root,
        "new-skill",
        "---\nname: new-skill\ndescription: RUNTIME METADATA\n---\nBODY\n",
    );
    std::fs::write(
        root.join("config.json"),
        serde_json::json!({"repos":["../\
        repo-{{AGENT_ROOT}}"],"context":["../Context.root-agent.md"]})
        .to_string(),
    )
    .unwrap();
    let second = issue_2832_combined(&root, temp.path(), false);
    assert!(second.contains("RUNTIME METADATA"));
    assert!(second.contains("repo-{{AGENT_ROOT}}"));
    assert!(second.contains("NOT FOUND"));
    for heading in [
        "## Skills",
        "### Available Skills",
        "# Agent Repos",
        "You are the Root Agent",
        "AgentsCommander indexes skills",
    ] {
        assert!(!second.contains(heading));
    }
}

#[test]
fn issue_2832_spoofed_root_rejected_before_config_context_and_provisioning() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.json"), "{").unwrap();
    std::fs::write(
        temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
        [0xff],
    )
    .unwrap();
    let cwd = path_string(&root);
    for result in [
        ensure_session_context(&cwd),
        build_root_agent_context_with_auto(&cwd, None, true),
        resolve_session_context_content(&cwd, true, true, None).map(|_| String::new()),
    ] {
        assert!(result.unwrap_err().contains("Root identity"));
    }
    assert!(!root.join("skills").exists());
    assert!(!root.join("AGENTS.md").exists());
    assert!(!temp.path().join(CONTEXT_CACHE_DIR_NAME).exists());
    assert!(!temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME).exists());
}

#[test]
fn issue_2832_raw_and_canonical_aliases_cannot_enter_other_context_routes() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target").join("ac-root-agent");
    let alias = temp.path().join("__agent_alias");
    std::fs::create_dir_all(&target).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(&target, &alias).is_err() {
        println!("issue_2832 alias fixture unavailable: Windows symlink privilege");
        return;
    }
    std::fs::write(target.join("config.json"), "{").unwrap();
    for root in [&alias, &target] {
        let cwd = path_string(root);
        assert!(ensure_session_context(&cwd)
            .unwrap_err()
            .contains("Root identity"));
        assert!(resolve_session_context_content(&cwd, true, true, None)
            .unwrap_err()
            .contains("Root identity"));
        assert!(build_replica_context(&cwd, None)
            .unwrap_err()
            .contains("Root identity"));
    }
    assert!(!target.join("skills").exists());
    assert!(!target
        .parent()
        .unwrap()
        .join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
        .exists());
}
