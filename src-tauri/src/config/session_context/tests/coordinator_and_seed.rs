use super::*;

/// #1369 (C4) AC-3: the #664 heuristic's verdict is invariant across the
/// rename. Neither spelling is in the closed `is_known_legacy_default_heading`
/// list, so a rendered context carrying either heading is rejected whether or
/// not the fast-path guard in `looks_like_generated_legacy_default_context`
/// names it.
#[test]
fn legacy_default_heuristic_rejects_both_repos_headings() {
    assert!(!is_known_legacy_default_heading("# Workspace Repos"));
    assert!(!is_known_legacy_default_heading("# Agent Repos"));

    // The reconstruction re-derives the skills section from the matrix root
    // it extracts out of the content, so the fixture must be built with the
    // index of a REAL root, exactly as the #664 healing tests do.
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    let skills_section =
        render_skills_section(&discover_skill_index(Some(&path_string(&matrix_root))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&replica_root),
        Some(&path_string(&matrix_root)),
        &skills_section,
    );
    // Positive control: without either heading this fixture IS recognized,
    // so the two assertions below cannot pass vacuously.
    assert!(looks_like_generated_legacy_default_context(
        &normalize_context_for_compat(&legacy)
    ));

    for heading in ["# Workspace Repos", "# Agent Repos"] {
        let with_heading = format!("{legacy}\n\n{heading}\n\nNo repos configured.\n");
        assert!(
            !looks_like_generated_legacy_default_context(&normalize_context_for_compat(
                &with_heading
            )),
            "a rendered context carrying {heading} must never be treated as a stale generated \
                default"
        );
    }
}

#[test]
fn deprecated_repos_context_token_is_skipped_without_generated_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(&replica_root).expect("create replica root");
    std::fs::write(
        replica_root.join("config.json"),
        concat!(
            r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT","#,
            r#""$REPOS_WORKSPACE_INFO"]}"#
        ),
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
    let repaired: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(replica_root.join("config.json")).expect("read config"),
    )
    .expect("parse repaired config");

    assert!(content.contains("# Agent Repos"));
    assert_eq!(
        repaired["context"],
        serde_json::json!(["$AGENTSCOMMANDER_CONTEXT"])
    );
    assert_eq!(content.matches("# Agent Repos").count(), 1);
}

#[test]
fn edited_agent_template_is_used_for_all_provider_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(&template_path, "CUSTOM_AGENT_BODY").expect("write custom agent template");

    for (target, expected_filename) in [
        (ManagedContextTarget::Codex, "AGENTS.md"),
        (ManagedContextTarget::Pi, "AGENTS.md"),
        (ManagedContextTarget::Claude, "CLAUDE.md"),
        (ManagedContextTarget::Antigravity, "AGENTS.md"),
    ] {
        seed_stale_managed_context_files(&matrix_root);
        let materialized =
            materialize_agent_context_file(&path_string(&matrix_root), target, false)
                .expect("materialize context")
                .expect("context path");
        assert!(materialized.ends_with(expected_filename));
        assert_only_selected_managed_context_file_exists(&matrix_root, expected_filename);
        let content = std::fs::read_to_string(materialized).expect("read materialized context");
        assert!(content.contains("CUSTOM_AGENT_BODY"));
    }

    assert_eq!(
        std::fs::read_to_string(template_path).expect("read agent template"),
        "CUSTOM_AGENT_BODY"
    );
}

#[test]
fn custom_coordinator_template_appends_only_for_coordinator() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_tech-lead");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        "CUSTOM_COORDINATOR_BODY",
    )
    .expect("write custom coordinator template");

    let non_coordinator = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize non-coordinator")
    .expect("context path");
    let non_coordinator_content =
        std::fs::read_to_string(non_coordinator).expect("read non-coordinator context");
    assert!(!non_coordinator_content.contains("CUSTOM_COORDINATOR_BODY"));
    assert!(!non_coordinator_content.contains("# Orchestrator Context"));

    let coordinator = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        true,
    )
    .expect("materialize coordinator")
    .expect("context path");
    let coordinator_content =
        std::fs::read_to_string(coordinator).expect("read coordinator context");
    assert!(coordinator_content.contains("# Orchestrator Context"));
    assert!(coordinator_content.contains("CUSTOM_COORDINATOR_BODY"));
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator template"),
        "CUSTOM_COORDINATOR_BODY"
    );
}

#[test]
fn privileged_pty_context_is_added_only_to_verified_workgroup_coordinator() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project-a");
    let ac_root = project.join(".ac");
    let team = ac_root.join("_team_dev-team");
    let workgroup = ac_root.join("wg-1-dev-team");
    let coordinator_matrix = ac_root.join("_agent_tech-lead");
    let member_matrix = ac_root.join("_agent_dev-rust");
    let coordinator = workgroup.join("__agent_tech-lead");
    let member = workgroup.join("__agent_dev-rust");
    for directory in [
        &team,
        &coordinator_matrix,
        &member_matrix,
        &coordinator,
        &member,
    ] {
        std::fs::create_dir_all(directory).expect("create fixture directory");
    }
    std::fs::write(
        team.join("config.json"),
        concat!(
            r#"{"coordinator":"../_agent_tech-lead","agents":["../_agent_dev-rust","#,
            r#""../_agent_tech-lead"]}"#
        ),
    )
    .expect("write team config");
    std::fs::write(
        coordinator.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write coordinator config");
    std::fs::write(
        member.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write member config");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        "CUSTOM_COORDINATOR_BYTES",
    )
    .expect("write custom coordinator template");

    let coordinator_content =
        resolve_session_context_content(&path_string(&coordinator), true, false, None)
            .expect("resolve coordinator context")
            .expect("coordinator context");
    assert_eq!(
        coordinator_content
            .matches("## Privileged PTY Input")
            .count(),
        1
    );
    assert!(coordinator_content.contains("CUSTOM_COORDINATOR_BYTES"));

    let member_content = resolve_session_context_content(&path_string(&member), true, false, None)
        .expect("resolve member context")
        .expect("member context");
    assert!(!member_content.contains("## Privileged PTY Input"));
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read custom template"),
        "CUSTOM_COORDINATOR_BYTES"
    );
}

#[test]
fn coordinator_template_no_longer_carries_inline_self_maintenance() {
    // #640: the coordinator's self-maintenance directive moved OUT of the raw
    // template into the gated SELF_MAINTENANCE_AUTO_SECTION (single source).
    // The raw template must no longer carry it, must keep the screenshot
    // guidance that preceded the removed block, and stay em-dash-free.
    let tpl = get_default_coordinator_template();
    assert!(!tpl.contains("## Self-Maintenance"));
    assert!(!tpl.contains("self-handoff-and-clear"));
    // Screenshot guidance (the content immediately before the removed block)
    // must survive the removal: this is the §8.C3 DRIFT guard for :1905.
    assert!(tpl.contains("## Sending Screenshots"));
    assert!(tpl.contains("names can be misleading."));
    assert!(
        !tpl.contains('\u{2014}'),
        "coordinator template must stay em-dash-free"
    );
}

#[test]
fn coordinator_template_carries_raise_hand_guidance_and_shared_template_does_not() {
    // #684: the coordinator template gains a short raise-hand usage guide
    // beside the screenshot guidance. It must be coordinator-only, so the
    // shared agent template (every non-coordinator agent) must NOT carry it.
    let coordinator = get_default_coordinator_template();
    assert!(coordinator.contains("## Raising Your Hand"));
    assert!(coordinator.contains("raise-hand --token <AGENTSCOMMANDER_TOKEN>"));
    assert!(coordinator.contains("Sidebar raised-hand indicator for your orchestrator row"));

    let shared = get_default_agent_template();
    assert!(!shared.contains("## Raising Your Hand"));
    assert!(!shared.contains("raise-hand --token"));
}

#[test]
fn coordinator_template_carries_cross_workgroup_rule() {
    // #1030: the user-approved cross-workgroup bullet, verbatim. Asserted as
    // ONE exact string rather than fragments: a body keeping the routing and
    // reply fragments but dropping the "only when your role, the user, or the
    // Root Agent authorizes it" clause between them would satisfy a fragment
    // test while violating the approved rule, and that qualifier is the whole
    // point. The looser anchor count rejects a second, differently worded
    // variant, which the exact count alone accepts.
    const RULE: &str = "- To reach another room, message its orchestrator, never its members, and \
        only when your role, the user, or the Root Agent authorizes it; replying to an \
        orchestrator who messaged you first is always authorized.\n";
    let tpl = get_default_coordinator_template();
    assert_eq!(
        tpl.matches(RULE).count(),
        1,
        "the exact approved bullet must appear exactly once"
    );
    assert_eq!(
        tpl.matches("- To reach another room,").count(),
        1,
        "exactly one cross-workgroup bullet may exist; a second variant contradicts the approved \
            rule"
    );
    assert!(
        !tpl.contains('\u{2014}'),
        "coordinator template must stay em-dash-free"
    );
    assert!(
        !get_default_agent_template().contains(RULE),
        "the rule is coordinator-only"
    );
}

#[test]
fn materialized_context_gates_self_maintenance_directive_by_flag() {
    // #640: the gated SELF_MAINTENANCE_AUTO_SECTION is appended to a coding
    // agent's materialized context only when auto_self_clear is true. Driven
    // through the production materialize path, not the raw template.
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    let cwd = path_string(&matrix_root);

    let on =
        materialize_agent_context_file_with_filename(&cwd, "CLAUDE.md", &[], false, true, None)
            .expect("materialize ON")
            .expect("context path");
    let on_content = std::fs::read_to_string(&on).expect("read ON context");
    assert!(on_content.contains("## Self-Maintenance (auto self-handoff-and-clear)"));
    assert!(on_content.contains("reaches 3 such lines"));
    assert!(on_content.contains("max 240 char forgotten summary"));
    assert!(on_content.contains("closed background"));

    let off =
        materialize_agent_context_file_with_filename(&cwd, "CLAUDE.md", &[], false, false, None)
            .expect("materialize OFF")
            .expect("context path");
    let off_content = std::fs::read_to_string(&off).expect("read OFF context");
    assert!(!off_content.contains("## Self-Maintenance"));
    assert!(!off_content.contains("max 240 char forgotten summary"));
}

/// #2031: a rendered replica context file carries the Answering section once.
#[test]
fn materialized_replica_context_carries_answering_section() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    let cwd = path_string(&matrix_root);
    for filename in ["CLAUDE.md", "AGENTS.md"] {
        let path =
            materialize_agent_context_file_with_filename(&cwd, filename, &[], false, false, None)
                .expect("materialize")
                .expect("context path");
        let content = std::fs::read_to_string(&path).expect("read context");
        assert_eq!(
            count_section_headings(&content, "## Answering"),
            1,
            "{filename}: {content}"
        );
        assert!(content.contains("Use plain, easy language."), "{filename}");
    }
}

#[test]
fn coordinator_on_disk_legacy_self_maintenance_is_stripped_and_replaced() {
    // #640 M1: an existing workgroup froze the OLD `## Self-Maintenance` block
    // into its persisted Context.coordinator.md on disk. The render-time strip
    // must remove it so the materialized context has EXACTLY ONE such section
    // (the new gated one) when ON, and ZERO when OFF (proving OFF truly
    // disables a coordinator that already carried the always-on block).
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_tech-lead");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        "COORD BODY\n\n## Self-Maintenance\n\nLEGACY_SENTINEL old qualitative trigger.\n",
    )
    .expect("write legacy coordinator template");
    let cwd = path_string(&matrix_root);

    let on = materialize_agent_context_file_with_filename(&cwd, "CLAUDE.md", &[], true, true, None)
        .expect("materialize ON")
        .expect("context path");
    let on_content = std::fs::read_to_string(&on).expect("read ON context");
    assert_eq!(
        on_content.matches("## Self-Maintenance").count(),
        1,
        "exactly one self-maintenance section after strip-and-replace"
    );
    assert!(
        on_content.contains("reaches 3 such lines"),
        "the surviving section is the NEW gated directive"
    );
    assert!(
        !on_content.contains("LEGACY_SENTINEL"),
        "the old on-disk block must be stripped"
    );
    assert!(
        on_content.contains("COORD BODY"),
        "coordinator body preserved"
    );

    let off =
        materialize_agent_context_file_with_filename(&cwd, "CLAUDE.md", &[], true, false, None)
            .expect("materialize OFF")
            .expect("context path");
    let off_content = std::fs::read_to_string(&off).expect("read OFF context");
    assert_eq!(
        off_content.matches("## Self-Maintenance").count(),
        0,
        "OFF strips the legacy block and appends nothing"
    );
    assert!(!off_content.contains("LEGACY_SENTINEL"));
    assert!(off_content.contains("COORD BODY"));
}

#[test]
fn root_materialized_context_gates_self_maintenance_by_flag() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        temp.path().join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
        crate::config::root_agent::default_root_context_template(),
    )
    .unwrap();
    std::fs::write(root.join("Role.md"), "ROOT BASE CONTEXT").unwrap();
    let on = issue_2832_combined(&root, temp.path(), true);
    assert!(on.contains("max 240 char forgotten summary"));
    assert!(on.contains("closed background"));
    assert!(on.contains("ROOT BASE CONTEXT"));
    let off = issue_2832_combined(&root, temp.path(), false);
    assert!(!off.contains("## Self-Maintenance"));
    assert!(off.contains("ROOT BASE CONTEXT"));
}

#[test]
fn root_never_appends_creates_or_rewrites_a_coordinator_template() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp
        .path()
        .join(".ac")
        .join("room-1-demo")
        .join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    let coordinator = root
        .parent()
        .unwrap()
        .join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&coordinator, "COORDINATOR_SENTINEL").unwrap();
    assert!(
        resolve_session_context_content(&path_string(&root), true, false, None)
            .unwrap_err()
            .contains("Root identity")
    );
    assert_eq!(
        std::fs::read_to_string(&coordinator).unwrap(),
        "COORDINATOR_SENTINEL"
    );
    std::fs::remove_file(&coordinator).unwrap();
    assert!(resolve_session_context_content(&path_string(&root), true, false, None).is_err());
    assert!(!coordinator.exists());
}

#[test]
fn create_only_publication_captures_clock_before_temp_cleanup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let expected = fixed_publication_time();
    let mut clock_called = false;
    let mut clock = || {
        clock_called = true;
        assert!(
            path.is_file(),
            "the final target must exist before publication time is captured"
        );
        let temp_still_present = std::fs::read_dir(temp.path())
            .expect("read template directory")
            .filter_map(Result::ok)
            .any(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(".Context.AgentsCommander.md") && name.ends_with(".tmp")
            });
        assert!(
            temp_still_present,
            "the winning temp must remain linked until after the clock sample"
        );
        expected
    };

    let outcome = write_template_if_missing_with_clock(&path, "complete", &mut clock)
        .expect("publish missing template");

    assert_eq!(
        outcome,
        CreateOnlyPublication::Published {
            published_at: expected
        }
    );
    assert!(clock_called);
    assert_eq!(
        std::fs::read_to_string(&path).expect("read target"),
        "complete"
    );
    assert_no_context_template_temp_leftover(temp.path());
}

#[test]
fn create_only_lost_race_is_already_present_without_a_clock_sample() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let mut clock_calls = 0_u32;
    let mut clock = || {
        clock_calls += 1;
        fixed_publication_time()
    };

    let outcome = write_template_if_missing_with::<std::fs::File, _>(
        &path,
        "losing contents",
        |temp_path| {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(temp_path)?;
            std::fs::write(&path, "winning contents")?;
            Ok(file)
        },
        &mut clock,
    )
    .expect("lost create race is not an error");

    assert_eq!(outcome, CreateOnlyPublication::AlreadyPresent);
    assert_eq!(
        clock_calls, 0,
        "a lost race must not sample publication time"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read winning target"),
        "winning contents"
    );
    assert_no_context_template_temp_leftover(temp.path());
}

#[test]
fn managed_read_never_falls_back_to_an_ungated_direct_create() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    let target = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);

    for synchronization in [Err("injected synchronization failure".to_string()), Ok(())] {
        let mut on_publication =
            |_: &'static str, _: crate::config::seeded_context_templates::ContextPublication| {};
        let result = read_or_create_context_template_with_sync(
            &path_string(&matrix_root),
            GLOBAL_CONTEXT_TEMPLATE_FILENAME,
            get_default_agent_template(),
            &mut on_publication,
            |_, _, _| synchronization,
        )
        .expect("managed fallback remains fail-soft");

        assert_eq!(result, None);
        assert!(
            !target.exists(),
            "managed sync failure or skip must use the in-memory default"
        );
    }
}

#[test]
fn missing_templates_are_created_and_used_during_regeneration() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        true,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(content.contains("# AgentsCommander Context"));
    assert!(content.contains("## Core Concepts"));
    assert!(content.contains("**Team**: the logical capability and organization"));
    assert!(content.contains("**Room**: a runtime replica of a team for a specific task"));
    assert!(content.contains("explicitly reply to its assigner"));
    assert!(content.contains("# Orchestrator Context"));
    assert!(content.contains("You are the orchestrator for your team"));
    assert_eq!(
        std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read created agent template"),
        get_default_agent_template()
    );
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read created coordinator template"),
        get_default_coordinator_template()
    );
}

#[test]
fn failed_template_seed_removes_partial_file_and_retry_uses_complete_defaults() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");

    for (filename, default_content) in [
        (
            GLOBAL_CONTEXT_TEMPLATE_FILENAME,
            get_default_agent_template(),
        ),
        (
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            get_default_coordinator_template(),
        ),
    ] {
        let path = ac_root.join(filename);
        let mut clock = chrono::Utc::now;
        let err = write_template_if_missing_with::<PartialFailWriter, _>(
            &path,
            default_content,
            |path| {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                Ok(PartialFailWriter {
                    file,
                    bytes_written: 0,
                    fail_after_bytes: 8,
                })
            },
            &mut clock,
        )
        .expect_err("injected partial write must fail");

        assert!(err.contains("failed to write context template"), "{err}");
        assert!(
            !path.exists(),
            "partial {filename} must be removed after write failure"
        );
        let temp_leftovers = std::fs::read_dir(&ac_root)
            .expect("read workspace dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(&format!(".{filename}."))
            })
            .count();
        assert_eq!(temp_leftovers, 0, "failed seed must remove temp files");
    }

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        true,
    )
    .expect("retry materializes context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(content.contains("# AgentsCommander Context"));
    assert!(content.contains("explicitly reply to its assigner"));
    assert!(content.contains("# Orchestrator Context"));
    assert!(content.contains("You are the orchestrator for your team"));
    assert_eq!(
        std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read retried agent template"),
        get_default_agent_template()
    );
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read retried coordinator template"),
        get_default_coordinator_template()
    );
}

#[test]
fn concurrent_template_seed_never_exposes_partial_default_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_tech-lead");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");

    let path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let (partial_written_tx, partial_written_rx) = mpsc::channel();
    let release_barrier = Arc::new(Barrier::new(2));
    let writer_release_barrier = Arc::clone(&release_barrier);
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        let mut clock = chrono::Utc::now;
        write_template_if_missing_with::<BlockingPartialWriter, _>(
            &writer_path,
            get_default_agent_template(),
            |path| {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                Ok(BlockingPartialWriter {
                    file,
                    bytes_written: 0,
                    first_chunk_len: 8,
                    partial_written_tx: Some(partial_written_tx),
                    release_barrier: writer_release_barrier,
                })
            },
            &mut clock,
        )
    });

    partial_written_rx
        .recv()
        .expect("blocked writer reaches partial temp write");
    assert!(
        !path.exists(),
        "final template must not exist while temp content is partial"
    );
    assert_eq!(
        read_context_template(&path_string(&matrix_root), GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .expect("read context template"),
        None
    );

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let materialized_content =
        std::fs::read_to_string(materialized).expect("read materialized context");
    assert!(materialized_content.contains("# AgentsCommander Context"));
    assert!(materialized_content.contains("explicitly reply to its assigner"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("read created agent template"),
        get_default_agent_template()
    );

    release_barrier.wait();
    writer
        .join()
        .expect("join blocked writer")
        .expect("blocked writer returns success after existing final wins");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read final agent template"),
        get_default_agent_template()
    );
}

#[test]
fn legacy_template_migration_does_not_overwrite_concurrent_new_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create workspace dir");
    let legacy_path = ac_root.join(LEGACY_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let new_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&legacy_path, "LEGACY_TEMPLATE").expect("write legacy template");

    migrate_legacy_agent_context_template_with(&ac_root, |legacy_path, new_path| {
        std::fs::write(new_path, "USER_NEW_TEMPLATE")?;
        std::fs::hard_link(legacy_path, new_path)
    })
    .expect("race-lost migration should succeed without overwrite");

    assert_eq!(
        std::fs::read_to_string(&new_path).expect("read new template"),
        "USER_NEW_TEMPLATE"
    );
    assert_eq!(
        std::fs::read_to_string(&legacy_path).expect("read preserved legacy template"),
        "LEGACY_TEMPLATE"
    );
}

#[test]
fn empty_agent_template_falls_back_to_mandatory_blocks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_tech-lead");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), "")
        .expect("write empty agent template");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        "COORDINATOR_ONLY",
    )
    .expect("write coordinator template");

    let materialized = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect("materialize context")
    .expect("context path");
    let content = std::fs::read_to_string(materialized).expect("read materialized context");

    assert!(content.contains("## GOLDEN RULE"));
    assert!(content.contains("## Delegated Task Reporting"));
    assert!(content.contains("## Skills"));
    assert!(content.contains("# Agent Repos"));
    assert!(content.contains("## CLI executable"));
    assert!(content.contains("## Session credentials"));
    assert!(content.contains("## Inter-Agent Messaging"));
    assert!(!content.contains("COORDINATOR_ONLY"));
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
fn existing_non_file_agent_template_returns_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::create_dir_all(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
        .expect("create template directory");

    let err = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect_err("directory template must error");

    assert!(err.contains("Context template"));
    assert!(err.contains("not a regular file"));
}

#[test]
fn invalid_utf8_agent_template_returns_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("create matrix root");
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), [0xff, 0xfe])
        .expect("write invalid utf8 template");

    let err = materialize_agent_context_file(
        &path_string(&matrix_root),
        ManagedContextTarget::Codex,
        false,
    )
    .expect_err("invalid utf8 template must error");

    assert!(err.contains("not valid UTF-8"));
    assert!(err.contains(GLOBAL_CONTEXT_TEMPLATE_FILENAME));
}
