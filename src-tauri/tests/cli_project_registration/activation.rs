use super::*;
use agentscommander_lib::config::seeded_context_templates;

// ---------------------------------------------------------------------------
// #1065 Stage F - real-boundary activation coverage for the project-context
// boundaries (Grinch Finding 1).
//
// The per-module unit tests drive each boundary's INNER `*_recorded` / `*_impl`
// helper with a `#[cfg(test)] for_test()` token. That proves the recorder logic
// but NOT that the production entry point constructs a real
// `ManifestActivationToken::production()` and threads it, because the outer
// wiring is `#[cfg(not(test))]`-gated and is compiled OUT of every unit-test
// build. This integration binary links the library in NON-test mode, so those
// gates are live and `production()` tokens are real. Two mechanisms, both of
// which red when a real adapter call is removed:
//
// (a) BEHAVIORAL - drive the real production entry point and assert the
//     resulting `.ac/seed-manifest.toml` mutation. Used wherever the boundary is
//     reachable from a plain `pub` entry point that takes no `AppHandle`/`State`.
// (b) SOURCE-SCRAPE WIRING ASSERTION - the sanctioned fallback (same form as
//     `commands/session.rs` `create_session_inner_keeps_both_archive_activation_gates`)
//     for boundaries no test can practically drive. Each pins the boundary's
//     production `ManifestActivationToken::production()` threading, so removing
//     the call (or flipping the gated activation to `None`) reds a test.
//
// The lifecycle and config-seed boundaries are covered in
// `tests/cli_workgroup_team.rs`.
// ---------------------------------------------------------------------------

// (a) V1CoverageBoundary::DirectCreateAcProjectFreshRoot, prod wiring
// `commands/ac_discovery.rs` `create_ac_project` -> Some(production()).
// A bare fresh-root create publishes its two project context templates, so
// `.ac/seed-manifest.toml` is emitted with one `project_context_template` row
// for `context:agentscommander` and one for `context:coordinator`. Removing the
// `Some(...production())` at the boundary (activation -> None) stops emission
// and reds this test.
#[tokio::test]
async fn create_ac_project_fresh_root_emits_seed_manifest_in_production_build() {
    let tmp = Tmp::new("activation-create-ac-project");
    let root = tmp.path().join("FreshProject");
    let root_arg = root.to_string_lossy().to_string();

    agentscommander_lib::commands::ac_discovery::create_ac_project(root_arg)
        .await
        .expect("create_ac_project on a fresh root succeeds");

    assert!(root.join(".ac").is_dir(), "the fresh root gains an .ac");
    let manifest = std::fs::read_to_string(root.join(".ac").join("seed-manifest.toml")).expect(
        "a fresh-root create_ac_project must emit .ac/seed-manifest.toml in a production build; \
         if this fails, the DirectCreateAcProjectFreshRoot production token was removed",
    );

    assert!(
        manifest.contains("schema_version = 1"),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("coverage_version = 2"),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("kind = \"project_context_template\""),
        "the create must record project context templates: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:agentscommander\""),
        "manifest must record the agentscommander project context template: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:coordinator\""),
        "manifest must record the coordinator project context template: {manifest}"
    );
}

// (a) V1CoverageBoundary::ContextOverwrite, prod wiring
// `commands/ac_discovery.rs` `overwrite_context_template_with_default` ->
// Some(production()). The command takes four plain `String` params (no
// `AppHandle`, no `State`), so the real outer command is callable here.
//
// The workspace deliberately starts BARE (an `.ac` created directly, with only a
// customised coordinator template and NO manifest) rather than via
// `create_ac_project`: a fresh-root create would itself publish a
// `context:coordinator` row, which would keep a "manifest contains
// context:coordinator" assertion green even with the overwrite boundary
// un-wired. Starting with no manifest makes the emission attributable ONLY to
// the overwrite, so flipping that boundary's activation to `None` reds this test.
#[tokio::test]
async fn overwrite_context_template_with_default_records_the_overwrite_in_production_build() {
    let tmp = Tmp::new("activation-context-overwrite");
    let project = tmp.path().join("OverwriteProject");
    let workspace = project.join(".ac");
    std::fs::create_dir_all(&workspace).expect("create the bare workspace");

    let filename =
        agentscommander_lib::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME;
    std::fs::write(workspace.join(filename), "custom coordinator guidance")
        .expect("write a customised coordinator template");

    let manifest_path = workspace.join("seed-manifest.toml");
    assert!(
        !manifest_path.exists(),
        "the bare workspace must start with no manifest, so only the overwrite can create one"
    );

    let update =
        seeded_context_templates::scan_project_context_template_updates(&project, &workspace)
            .expect("scan project context template updates")
            .into_iter()
            .find(|update| update.filename == filename)
            .expect("a pending coordinator overwrite update");

    agentscommander_lib::commands::ac_discovery::overwrite_context_template_with_default(
        project.to_string_lossy().to_string(),
        update.filename.clone(),
        update.current_file_sha256.clone(),
        update.current_default_sha256.clone(),
    )
    .await
    .expect("overwrite_context_template_with_default succeeds");

    let manifest = std::fs::read_to_string(&manifest_path).expect(
        "an explicit overwrite must create .ac/seed-manifest.toml in a production build; \
         if this fails, the ContextOverwrite production token was removed",
    );
    assert!(
        manifest.contains("scope = \"context:coordinator\""),
        "the overwrite must record the coordinator context template: {manifest}"
    );
    assert!(
        manifest.contains("kind = \"project_context_template\""),
        "manifest: {manifest}"
    );
}

// (b) `commands/ac_discovery.rs`: the fresh-root create (also covered by (a)
// above), the two discovery context-update scans (ContextUpdate), and the
// overwrite command (ContextOverwrite, also covered by (a) above). The two scans
// sit behind `discover_ac_agents` / `discover_project`, which each require an
// `AppHandle` plus four `State<'_, ...>` params, so they are not drivable here.
#[test]
fn ac_discovery_rs_threads_production_tokens_for_context_boundaries() {
    let production = normalized_production_source("src/commands/ac_discovery.rs");

    assert_eq!(
        production_token_count(&production),
        4,
        "ac_discovery.rs must construct exactly four production activation tokens \
         (create_ac_project + two context-update scans + overwrite)"
    );

    // The `activation.as_ref()` argument pins each assertion to its cfg-gated
    // production site; a unit call threading a `for_test()` token cannot satisfy it.
    assert!(
        production.contains(
            "scan_project_context_templates_recorded(&repo_dir,&ac_root,activation.as_ref()"
        ),
        "the per-repo discovery context-update (ContextUpdate) must thread the activation token"
    );
    assert!(
        production
            .contains("scan_project_context_templates_recorded(&base,&ac_root,activation.as_ref()"),
        "the workgroup discovery context-update (ContextUpdate) must thread the activation token"
    );
    assert!(
        production.contains(
            "overwrite_context_template_recorded(Path::new(&path),&ac_root,&filename,\
             &current_file_sha256,&current_default_sha256,activation.as_ref()"
        ),
        "the overwrite command (ContextOverwrite) must thread the activation token"
    );
}

// (b) `config/projects.rs`: new-project registration ContextCreate. The CLI path
// (`register_new_project`) is additionally covered end-to-end by
// `new_project_emits_seed_manifest_and_open_does_not_backfill` above; the GUI/web
// path (`prepare_new_project`) is `pub(crate)` and unreachable from an
// integration binary.
#[test]
fn projects_rs_threads_production_tokens_for_new_project_registration() {
    let production = normalized_production_source("src/config/projects.rs");

    assert_eq!(
        production_token_count(&production),
        2,
        "projects.rs must construct exactly two production activation tokens \
         (CLI register_new_project + GUI prepare_new_project)"
    );

    assert!(
        production.contains(
            "register_new_project_with_store(settings,raw_path,store,activation.as_ref())"
        ),
        "the CLI new-project registration (ContextCreate) must thread the activation token"
    );
    assert!(
        production.contains("prepare_new_project_impl(raw_path,activation.as_ref(),"),
        "the GUI/web new-project registration (ContextCreate GUI) must thread the activation token"
    );
}
