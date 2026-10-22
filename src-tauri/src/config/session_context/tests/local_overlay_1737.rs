// ─────────────────────────────────────────────────────────────────────────
// #1737 - the Markdown half of the `.local` override layer. Plan section 11,
// C1 to C11.
// ─────────────────────────────────────────────────────────────────────────

use super::*;
use crate::config::local_overlay::MarkdownOverride;

/// A `.ac` tree with a matrix and a replica agent root, the layout every
/// consumer-side substitution point resolves through.
struct Fixture {
    _temp: tempfile::TempDir,
    ac_root: PathBuf,
    replica_root: PathBuf,
    matrix_root: PathBuf,
}

fn new_fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix_root = ac_root.join("_agent_dev-rust");
    let replica_root = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix_root).expect("matrix root");
    std::fs::create_dir_all(&replica_root).expect("replica root");
    Fixture {
        _temp: temp,
        ac_root,
        replica_root,
        matrix_root,
    }
}

fn resolve_global(fixture: &Fixture) -> Result<String, String> {
    let agent_root = path_string(&fixture.replica_root);
    let matrix_root = path_string(&fixture.matrix_root);
    let skills = render_skills_section(&discover_skill_index(Some(&matrix_root)));
    resolve_agent_context_with_activation(
        &agent_root,
        Some(&matrix_root),
        &skills,
        &fixture.replica_root,
        None,
        None,
        None,
    )
}

// C3
#[test]
fn a_global_override_is_rendered_and_the_base_file_is_untouched() {
    let fixture = new_fixture();
    let base_path = fixture.ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&base_path, "BASE GLOBAL TEMPLATE\n").expect("base");
    let base_bytes = std::fs::read(&base_path).expect("base bytes");
    std::fs::write(
        fixture.ac_root.join("Context.AgentsCommander.local.md"),
        "LOCAL GLOBAL OVERRIDE for {{AGENT_ROOT}}\n",
    )
    .expect("override");

    let resolved = resolve_global(&fixture).expect("resolve");

    assert!(resolved.contains("LOCAL GLOBAL OVERRIDE"), "{resolved}");
    assert!(!resolved.contains("BASE GLOBAL TEMPLATE"), "{resolved}");
    assert_no_raw_template_placeholders(&resolved);
    assert_eq!(
        std::fs::read(&base_path).expect("base bytes after"),
        base_bytes,
        "the base file must not be rewritten"
    );
}

// C4
#[test]
fn a_stale_base_is_still_self_healed_while_the_session_gets_the_override() {
    // A base that classifies `StaleGenerated`, baked the same way the
    // existing self-heal tests bake theirs: a legacy generated default for
    // an OLD agent, resolved for a NEW one.
    let fixture = new_fixture();
    let old_matrix = fixture.ac_root.join("_agent_old-agent");
    let old_replica = fixture
        .ac_root
        .join("wg-19-dev-team")
        .join("__agent_old-agent");
    std::fs::create_dir_all(&old_matrix).expect("old matrix");
    std::fs::create_dir_all(&old_replica).expect("old replica");
    let old_skills =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills,
    );
    let base_path = fixture.ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&base_path, &legacy).expect("stale base");
    assert!(
        matches!(
            classify_legacy_rendered_default_context(
                &legacy,
                &path_string(&fixture.replica_root),
                Some(&path_string(&fixture.matrix_root)),
                &no_skill_section(),
            ),
            LegacyRenderedDefaultContext::StaleGenerated
        ),
        "the fixture must classify stale for this test to mean anything"
    );
    std::fs::write(
        fixture.ac_root.join("Context.AgentsCommander.local.md"),
        "LOCAL GLOBAL OVERRIDE\n",
    )
    .expect("override");

    let resolved = resolve_global(&fixture).expect("resolve");

    assert!(resolved.contains("LOCAL GLOBAL OVERRIDE"), "{resolved}");
    let healed = std::fs::read_to_string(&base_path).expect("healed base");
    assert_ne!(
        healed, legacy,
        "the base file is still self-healed exactly as today"
    );
    assert!(
        !healed.contains("LOCAL GLOBAL OVERRIDE"),
        "the heal must never write override bytes into the base file"
    );
}

// C5
#[test]
fn the_root_template_override_reaches_the_combined_file_under_its_base_label() {
    let fixture = new_fixture();
    let cache_seed = fixture.replica_root.join("ac-context-seed.md");
    std::fs::write(&cache_seed, "CACHE SEED\n").expect("seed");
    let base = fixture.ac_root.join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&base, "BASE ROOT TEMPLATE\n").expect("base");
    std::fs::write(
        fixture.ac_root.join("Context.root-agent.local.md"),
        "LOCAL ROOT OVERRIDE\n",
    )
    .expect("override");

    let resolved_paths = vec![
        ("ac-context-seed.md".to_string(), cache_seed),
        (ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME.to_string(), base),
    ];
    let combined_path = write_combined_context_file(
        &path_string(&fixture.replica_root),
        &resolved_paths,
        "replica-context",
    )
    .expect("combine");
    let combined = std::fs::read_to_string(&combined_path).expect("combined");

    assert!(combined.contains("LOCAL ROOT OVERRIDE"), "{combined}");
    assert!(!combined.contains("BASE ROOT TEMPLATE"), "{combined}");
    assert!(
        combined.contains("# Context: Context.root-agent.md"),
        "the heading is still derived from the BASE path: {combined}"
    );
    assert!(combined.starts_with("CACHE SEED"), "{combined}");
}

// C10
#[test]
fn a_local_md_for_an_unmanaged_filename_is_ignored() {
    let fixture = new_fixture();
    let seed = fixture.replica_root.join("ac-context-seed.md");
    std::fs::write(&seed, "CACHE SEED\n").expect("seed");
    let role = fixture.replica_root.join("Role.md");
    std::fs::write(&role, "BASE ROLE\n").expect("role");
    std::fs::write(fixture.replica_root.join("Role.local.md"), "LOCAL ROLE\n")
        .expect("override");

    let resolved_paths = vec![
        ("ac-context-seed.md".to_string(), seed),
        ("Role.md".to_string(), role),
    ];
    let combined_path = write_combined_context_file(
        &path_string(&fixture.replica_root),
        &resolved_paths,
        "replica-context",
    )
    .expect("combine");
    let combined = std::fs::read_to_string(&combined_path).expect("combined");

    assert!(combined.contains("BASE ROLE"), "{combined}");
    assert!(!combined.contains("LOCAL ROLE"), "{combined}");
}

// C11
#[test]
fn the_base_read_still_fails_when_an_override_exists() {
    let fixture = new_fixture();
    let seed = fixture.replica_root.join("ac-context-seed.md");
    std::fs::write(&seed, "CACHE SEED\n").expect("seed");
    // The BASE is a directory; the override beside it is perfectly valid.
    let base = fixture.ac_root.join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::create_dir(&base).expect("directory-shaped base");
    std::fs::write(
        fixture.ac_root.join("Context.root-agent.local.md"),
        "LOCAL ROOT OVERRIDE\n",
    )
    .expect("override");

    let resolved_paths = vec![
        ("ac-context-seed.md".to_string(), seed),
        (ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME.to_string(), base),
    ];
    let error = write_combined_context_file(
        &path_string(&fixture.replica_root),
        &resolved_paths,
        "replica-context",
    )
    .expect_err("the base read must still propagate");
    assert!(error.contains("Failed to read context file"), "{error}");
}

// C8a
#[test]
fn an_override_does_not_substitute_for_a_missing_root_base() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let root_agent = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&ac_root).expect("ac root");
    std::fs::create_dir_all(&root_agent).expect("root agent");
    // The override exists; the BASE deliberately does not.
    std::fs::write(
        ac_root.join("Context.root-agent.local.md"),
        "LOCAL ROOT OVERRIDE\n",
    )
    .expect("override");
    std::fs::write(root_agent.join("Role.md"), "ROLE\n").expect("role");
    std::fs::write(
        root_agent.join("config.json"),
        serde_json::json!({
            "context": ["../.ac/Context.root-agent.md", "Role.md"]
        })
        .to_string(),
    )
    .expect("config");

    let error =
        build_root_agent_context_at(&path_string(&root_agent), &ac_root, None, false)
            .expect_err("a missing base is still a hard error");
    assert!(
        error.contains("Root Agent has missing context files"),
        "{error}"
    );
    assert!(
        !error.contains("local"),
        "the override is not consulted: {error}"
    );
}

// C9
#[test]
fn a_rejected_markdown_override_is_reported_and_the_base_content_is_used() {
    let fixture = new_fixture();
    let base_path = fixture.ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&base_path, "BASE GLOBAL TEMPLATE\n").expect("base");
    let local = fixture.ac_root.join("Context.AgentsCommander.local.md");
    std::fs::create_dir(&local).expect("directory-shaped override");

    // The typed diagnostic, which is what makes the report falsifiable.
    let value = crate::config::local_overlay::read_markdown_override(&base_path);
    assert!(matches!(value, MarkdownOverride::Rejected { .. }));
    let diagnostic = value.diagnostic().expect("a rejection has a diagnostic");
    assert_eq!(
        diagnostic.level(),
        crate::config::local_overlay::OverlayDiagnosticLevel::Error
    );
    let rendered = diagnostic.render();
    assert!(
        rendered.contains(&local.display().to_string()),
        "{rendered}"
    );
    assert!(rendered.contains("regular file"), "{rendered}");

    // And the session still resolves, on the base content.
    let resolved = resolve_global(&fixture).expect("resolve");
    assert!(resolved.contains("BASE GLOBAL TEMPLATE"), "{resolved}");

    // A non-UTF-8 override is rejected the same way.
    std::fs::remove_dir(&local).expect("remove dir");
    std::fs::write(&local, [0x66u8, 0xff, 0x66]).expect("invalid utf-8");
    assert!(matches!(
        crate::config::local_overlay::read_markdown_override(&base_path),
        MarkdownOverride::Rejected { .. }
    ));
    let resolved = resolve_global(&fixture).expect("resolve");
    assert!(resolved.contains("BASE GLOBAL TEMPLATE"), "{resolved}");
}

// C6 and C7: tripwires, not AC-2 evidence. The structural argument is that
// the only `read_dir` in `seeded_context_templates.rs` is inside its own
// `mod tests`, and the one production enumeration filters on a prefix no
// `.local.md` name satisfies.
#[test]
fn tripwire_a_full_resolve_never_touches_or_records_a_local_md() {
    // The base global template is deliberately ABSENT and the resolve is
    // driven through the ACTIVATED path, so the sync creates it, records a
    // publication, and marks the loaded state dirty. That is what makes
    // `persist_state_best_effort` (`seeded_context_templates.rs:1048-1050`)
    // actually write the state file, so C7's assertion below is never
    // vacuous. The test asserts the file's existence before reading it.
    let fixture = new_fixture();
    let base_path = fixture.ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    assert!(!base_path.exists());
    let overrides = [
        ("Context.AgentsCommander.local.md", "GLOBAL OVERRIDE\n"),
        ("Context.coordinator.local.md", "COORDINATOR OVERRIDE\n"),
        ("Context.root-agent.local.md", "ROOT OVERRIDE\n"),
    ];
    for (name, body) in overrides {
        std::fs::write(fixture.ac_root.join(name), body).expect("override");
    }
    let before: Vec<Vec<u8>> = overrides
        .iter()
        .map(|(name, _)| std::fs::read(fixture.ac_root.join(name)).expect("bytes"))
        .collect();

    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let _ = resolve_global_activated(&fixture, Some(&token));
    assert!(
        base_path.exists(),
        "the activated resolve must create the base template"
    );

    // C6: every override is byte-identical and none was retired.
    for (index, (name, _)) in overrides.iter().enumerate() {
        assert_eq!(
            std::fs::read(fixture.ac_root.join(name)).expect("bytes after"),
            before[index],
            "{name} must not be rewritten"
        );
    }
    let mut entries: Vec<String> = std::fs::read_dir(&fixture.ac_root)
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    entries.sort();
    assert!(
        !entries
            .iter()
            .any(|name| name.contains(".local.md.retired-")),
        "no .local.md may be retired: {entries:?}"
    );

    // C7: the seed manifest records no `.local.md` entry. Its existence is
    // asserted first, so this half cannot silently become a no-op if the
    // activated path ever stops persisting state.
    let manifest_path = fixture
        .ac_root
        .join(crate::config::instance_artifacts::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
    let manifest = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "the seed manifest state file must exist for C7 to assert anything ({}): {e}",
            manifest_path.display()
        )
    });
    assert!(
        !manifest.contains(".local.md"),
        "the seed manifest recorded a .local.md: {manifest}"
    );
}

/// The gate-state variants (C8b to C8d) run through the ACTIVATED path,
/// which is the only one that consults `acquire_project_gate_soft`. The
/// project root is the parent of the `.ac` directory.
fn resolve_global_activated(
    fixture: &Fixture,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<String, String> {
    let agent_root = path_string(&fixture.replica_root);
    let matrix_root = path_string(&fixture.matrix_root);
    let skills = render_skills_section(&discover_skill_index(Some(&matrix_root)));
    resolve_agent_context_with_activation(
        &agent_root,
        Some(&matrix_root),
        &skills,
        &fixture.replica_root,
        None,
        None,
        activation,
    )
}

fn project_root_of(fixture: &Fixture) -> PathBuf {
    fixture
        .ac_root
        .parent()
        .expect("the project root is the parent of .ac")
        .to_path_buf()
}

// C8b
#[test]
fn a_held_gate_creates_the_base_from_the_seeded_template_and_serves_the_override() {
    let fixture = new_fixture();
    let base_path = fixture.ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    assert!(!base_path.exists(), "the base is deliberately absent");
    std::fs::write(
        fixture.ac_root.join("Context.AgentsCommander.local.md"),
        "LOCAL GLOBAL OVERRIDE\n",
    )
    .expect("override");

    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let resolved = resolve_global_activated(&fixture, Some(&token)).expect("resolve");

    assert!(resolved.contains("LOCAL GLOBAL OVERRIDE"), "{resolved}");
    let created = std::fs::read_to_string(&base_path).expect("the base was created");
    assert_eq!(
        created,
        get_default_agent_template(),
        "the created bytes are the seeded template's current content"
    );
    assert!(
        !created.contains("LOCAL GLOBAL OVERRIDE"),
        "no override byte may reach the base file"
    );
}

// C8c
#[test]
fn an_unavailable_gate_creates_no_base_and_still_serves_the_override() {
    let fixture = new_fixture();
    let base_path = fixture.ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(
        fixture.ac_root.join("Context.AgentsCommander.local.md"),
        "LOCAL GLOBAL OVERRIDE\n",
    )
    .expect("override");

    // Contend for the same project root in this process so the soft gate's
    // acquire times out and reports `Unavailable`. This costs the default
    // gate timeout in wall clock, which is why this test is slow.
    let project_root = project_root_of(&fixture);
    let held =
        crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(&project_root)
            .expect("hold the project gate");

    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let resolved = resolve_global_activated(&fixture, Some(&token)).expect("resolve");
    held.release();

    assert!(resolved.contains("LOCAL GLOBAL OVERRIDE"), "{resolved}");
    assert!(
        !base_path.exists(),
        "an Unavailable gate must create no base file"
    );
}

// C8d
#[test]
fn the_coordinator_base_absent_case_behaves_the_same_under_both_gate_states() {
    // Held: the base is created from the seeded template and the session
    // still receives the override bytes.
    let fixture = new_fixture();
    let cwd = path_string(&fixture.replica_root);
    std::fs::write(
        fixture.replica_root.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("replica config");
    let base_path = fixture.ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    assert!(!base_path.exists());
    std::fs::write(
        fixture.ac_root.join("Context.coordinator.local.md"),
        "LOCAL COORDINATOR BODY\n",
    )
    .expect("override");

    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let resolved = resolve_session_context_content_with_activation(
        &cwd,
        true,
        false,
        None,
        Some(&token),
    )
    .expect("resolve")
    .expect("some content");
    assert!(resolved.contains("LOCAL COORDINATOR BODY"), "{resolved}");
    let created = std::fs::read_to_string(&base_path).expect("the base was created");
    assert_eq!(created, get_default_coordinator_template());
    assert!(!created.contains("LOCAL COORDINATOR BODY"));

    // Unavailable: no base file is created and the override still wins.
    let unavailable = new_fixture();
    let fixture = unavailable;
    let cwd = path_string(&fixture.replica_root);
    std::fs::write(
        fixture.replica_root.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("replica config");
    let base_path = fixture.ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(
        fixture.ac_root.join("Context.coordinator.local.md"),
        "LOCAL COORDINATOR BODY\n",
    )
    .expect("override");
    let project_root = project_root_of(&fixture);
    let held =
        crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(&project_root)
            .expect("hold the project gate");
    let resolved = resolve_session_context_content_with_activation(
        &cwd,
        true,
        false,
        None,
        Some(&token),
    )
    .expect("resolve")
    .expect("some content");
    held.release();
    assert!(resolved.contains("LOCAL COORDINATOR BODY"), "{resolved}");
    assert!(
        !base_path.exists(),
        "an Unavailable gate must create no base file"
    );
}
// C1 and C2
#[test]
fn the_coordinator_body_is_substituted_and_the_no_override_shape_is_recomputed() {
    let fixture = new_fixture();
    let cwd = path_string(&fixture.replica_root);
    std::fs::write(
        fixture.replica_root.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("replica config");
    let base_path = fixture.ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&base_path, "BASE COORDINATOR BODY\n").expect("base");

    // C2: with no override the resolved context carries the base body under
    // the fixed framing, recomputed here from the bytes this test wrote.
    let control = resolve_session_context_content(&cwd, true, false, None)
        .expect("resolve")
        .expect("some content");
    let base_body = std::fs::read_to_string(&base_path).expect("base body");
    assert!(
        control.contains(&format!("\n\n---\n\n# Orchestrator Context\n\n{base_body}")),
        "{control}"
    );

    // C1: with an override the session receives its bytes and not the base's.
    std::fs::write(
        fixture.ac_root.join("Context.coordinator.local.md"),
        "LOCAL COORDINATOR BODY\n",
    )
    .expect("override");
    let resolved = resolve_session_context_content(&cwd, true, false, None)
        .expect("resolve")
        .expect("some content");
    assert!(resolved.contains("LOCAL COORDINATOR BODY"), "{resolved}");
    assert!(!resolved.contains("BASE COORDINATOR BODY"), "{resolved}");
    assert!(
        resolved.contains("# Orchestrator Context"),
        "the override receives exactly the treatment the base body receives: {resolved}"
    );
    assert_eq!(
        std::fs::read_to_string(&base_path).expect("base after"),
        base_body,
        "the base file is untouched"
    );
}
