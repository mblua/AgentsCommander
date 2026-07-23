//! #1065 Stage F - real-boundary activation coverage (Grinch Finding 1).
//!
//! The per-module unit tests exercise each boundary's INNER `*_recorded` /
//! `*_impl` / `*_activated` helper with a `#[cfg(test)] for_test()` token. That
//! proves the recorder logic but NOT that the production entry point constructs a
//! real `ManifestActivationToken::production()` and threads it: the outer wiring
//! is `#[cfg(not(test))]`-gated, so it is compiled OUT of every unit-test build
//! (the `#[cfg(test)]` arm hard-codes `activation = None`). This file closes that
//! gap for the boundaries that a unit build cannot reach, satisfying mandatory
//! acceptance item 22 ("a declaration that remains green after its actual adapter
//! call is removed is insufficient") and in-plan Grinch #15.
//!
//! Two mechanisms, both of which red when a real adapter call is removed:
//!
//! (a) BEHAVIORAL - drive the real production boundary through the library
//!     compiled in NON-test mode (an integration-test binary links the crate
//!     without `cfg(test)`, so `production()` tokens are real) and assert the
//!     resulting `.ac/seed-manifest.toml` mutation. Used where the boundary is
//!     reachable from a plain `pub` entry point.
//!
//! (b) SOURCE-SCRAPE WIRING ASSERTION - the sanctioned fallback (same form as
//!     `commands/session.rs` `create_session_inner_keeps_both_archive_activation_gates`)
//!     for boundaries a test cannot practically drive (config-seed / session
//!     context / coordinator sync need a real spawned agent session; the delete
//!     commands are Tauri commands with no CLI path; `prepare_new_project` is a
//!     `pub(crate)` GUI/web entry). Each asserts the production source still
//!     threads `ManifestActivationToken::production()` into the boundary's adapter,
//!     so removing that call (or flipping the gated activation to `None`) reds a
//!     test. `production()` is verified to appear ONLY in production code in each
//!     scraped file (never in its `#[cfg(test)]` blocks), so a whole-file token
//!     count is an exact production-wiring count.
//!
//! End-to-end (a) coverage for the remaining boundaries lives in the CLI suites:
//! ContextCreate (`tests/cli_project_registration.rs` `new-project`),
//! LifecycleReplicaRemoval and LifecycleWorkgroupRemoval
//! (`tests/cli_workgroup_team.rs` `team remove-member` / `workgroup remove`).

use std::path::Path;

/// Read a crate-relative source file and collapse ALL whitespace, so assertions
/// match token sequences irrespective of rustfmt line-wrapping/indentation.
fn normalized_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    body.split_whitespace().collect::<String>()
}

/// Count the production activation-token constructions in a scraped file. The
/// token string carries no interior whitespace, so it survives normalization.
fn production_token_count(normalized: &str) -> usize {
    normalized
        .matches("ManifestActivationToken::production()")
        .count()
}

// ---------------------------------------------------------------------------
// (a) BEHAVIORAL - V1CoverageBoundary::DirectCreateAcProjectFreshRoot
// prod wiring: commands/ac_discovery.rs `create_ac_project` -> Some(production())
// ---------------------------------------------------------------------------

// A bare fresh-root `create_ac_project` publishes its two project context
// templates. Because this integration binary links the library in non-test mode,
// the command constructs a real `ManifestActivationToken::production()` and
// threads it, so `.ac/seed-manifest.toml` is emitted with one
// `project_context_template` row for `context:agentscommander` and one for
// `context:coordinator`. Removing the `Some(...production())` at the boundary
// (activation -> None) stops emission and reds this test.
#[tokio::test]
async fn create_ac_project_fresh_root_emits_seed_manifest_in_production_build() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("FreshProject");
    let root_arg = root.to_string_lossy().to_string();

    agentscommander_lib::commands::ac_discovery::create_ac_project(root_arg)
        .await
        .expect("create_ac_project on a fresh root succeeds");

    assert!(root.join(".ac").is_dir(), "the fresh root gains an .ac");
    let manifest_path = root.join(".ac").join("seed-manifest.toml");
    let manifest = std::fs::read_to_string(&manifest_path).expect(
        "a fresh-root create_ac_project must emit .ac/seed-manifest.toml in a production build; \
         if this fails, the DirectCreateAcProjectFreshRoot production token was removed",
    );

    assert!(
        manifest.contains("schema_version = 1"),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("coverage_version = 1"),
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

// ---------------------------------------------------------------------------
// (b) SOURCE-SCRAPE - commands/session.rs
// Config-seed publish (#9/#10/#11) and session-spawn context read/self-heal +
// coordinator v2/v3->v4 sync (#4/#6/#7/#8). Both live behind a real spawned
// agent session, which the CLI suites deliberately never start.
// ---------------------------------------------------------------------------

#[test]
fn session_rs_threads_production_tokens_for_config_seed_and_context() {
    let normalized = normalized_source("src/commands/session.rs");

    // Two `#[cfg(not(test))]` activation sites: the config-seed publish chokepoint
    // and the session-spawn context/self-heal/coordinator sync. Flipping either to
    // `None` (or deleting the block) drops the count and reds this test.
    assert_eq!(
        production_token_count(&normalized),
        2,
        "session.rs must construct exactly two production activation tokens \
         (config-seed publish + session-spawn context sync)"
    );

    // Grinch failure scenario #1: revert the recorded publish to the still-present
    // non-recording `perform_config_seed`. Both identifiers occur only here in
    // session.rs, so their presence is an exact production-wiring assertion.
    assert!(
        normalized.contains("perform_config_seed_recorded("),
        "the config-seed publish (ConfigExactPublish/OverBoundPublish/FailedRestore) must call \
         the recording adapter perform_config_seed_recorded, not the non-recording variant"
    );
    assert!(
        normalized.contains("materialize_agent_context_file_with_filename_activated("),
        "the session-spawn context sync (ContextSelfHeal + Coordinator v2/v3->v4) must call the \
         activation-threading adapter materialize_agent_context_file_with_filename_activated"
    );
}

// ---------------------------------------------------------------------------
// (b) SOURCE-SCRAPE - commands/ac_discovery.rs
// DirectCreateAcProjectFreshRoot (#1, also covered by (a) above), discovery
// context-update (#3, two scan sites), context overwrite (#5).
// ---------------------------------------------------------------------------

#[test]
fn ac_discovery_rs_threads_production_tokens_for_context_boundaries() {
    let normalized = normalized_source("src/commands/ac_discovery.rs");

    // Four `#[cfg(not(test))]` activation sites: fresh-root create, two
    // context-update scans, and the overwrite command.
    assert_eq!(
        production_token_count(&normalized),
        4,
        "ac_discovery.rs must construct exactly four production activation tokens \
         (create_ac_project + two context-update scans + overwrite)"
    );

    // Precise production-call proofs (the `activation.as_ref()` argument ties each
    // to its cfg-gated production site, never to a `for_test()` unit call).
    assert!(
        normalized.contains(
            "scan_project_context_templates_recorded(&repo_dir,&workspace_dir,activation.as_ref()"
        ),
        "the per-repo discovery context-update (ContextUpdate) must thread the activation token"
    );
    assert!(
        normalized.contains(
            "scan_project_context_templates_recorded(&base,&workspace_dir,activation.as_ref()"
        ),
        "the workgroup discovery context-update (ContextUpdate) must thread the activation token"
    );
    assert!(
        normalized.contains("overwrite_context_template_recorded(Path::new(&path),&workspace_dir,&filename,&current_file_sha256,&current_default_sha256,activation.as_ref()"),
        "the overwrite command (ContextOverwrite) must thread the activation token"
    );
}

// ---------------------------------------------------------------------------
// (b) SOURCE-SCRAPE - config/projects.rs
// New-project registration ContextCreate: the CLI path (#2, register_new_project,
// also covered end-to-end by tests/cli_project_registration.rs) and the GUI/web
// path (#2 GUI, prepare_new_project - a pub(crate) entry no integration test can
// call).
// ---------------------------------------------------------------------------

#[test]
fn projects_rs_threads_production_tokens_for_new_project_registration() {
    let normalized = normalized_source("src/config/projects.rs");

    assert_eq!(
        production_token_count(&normalized),
        2,
        "projects.rs must construct exactly two production activation tokens \
         (CLI register_new_project + GUI prepare_new_project)"
    );

    assert!(
        normalized.contains(
            "register_new_project_with_store(settings,raw_path,store,activation.as_ref())"
        ),
        "the CLI new-project registration (ContextCreate) must thread the activation token"
    );
    assert!(
        normalized.contains("prepare_new_project_impl(raw_path,activation.as_ref(),"),
        "the GUI/web new-project registration (ContextCreate GUI) must thread the activation token"
    );
}

// ---------------------------------------------------------------------------
// (b) SOURCE-SCRAPE - commands/entity_creation.rs
// The GUI Tauri delete commands, invoked by no test: delete_team (#14),
// delete_agent_matrix (#15) + its pending-inclusive staged-rollback prune (#16),
// and delete_workgroup (LifecycleWorkgroupRemoval GUI site; the enum boundary is
// covered end-to-end by the CLI `workgroup remove`). Each OUTER command must
// construct and thread the token into its inner transaction - exactly what no
// prior test asserted.
// ---------------------------------------------------------------------------

#[test]
fn entity_creation_rs_threads_production_tokens_through_the_delete_commands() {
    let normalized = normalized_source("src/commands/entity_creation.rs");

    assert_eq!(
        production_token_count(&normalized),
        3,
        "entity_creation.rs must construct exactly three production activation tokens \
         (delete_team + delete_agent_matrix + delete_workgroup)"
    );

    // Precise proofs that each outer Tauri command threads the token into its inner
    // transaction. The inner-fn names also appear in unit calls and definitions, so
    // the token argument is what pins each assertion to the production command.
    assert!(
        normalized.contains("delete_team_inner(&app,session_mgr.inner(),&project_path,&team_name,Some(ManifestActivationToken::production()),"),
        "delete_team (LifecycleTeamDeletion) must thread Some(production()) into delete_team_inner"
    );
    assert!(
        normalized.contains("delete_agent_matrix_inner(&app,session_mgr.inner(),settings.inner(),&project_path,&agent_path,Some(ManifestActivationToken::production()),"),
        "delete_agent_matrix (LifecycleAgentMatrixDeletion + PendingInclusiveDeleteProtection) \
         must thread Some(production()) into delete_agent_matrix_inner"
    );
    assert!(
        normalized.contains("letactivation=ManifestActivationToken::production();gated_workgroup_delete_with(&project_root,&workgroup,Some(&activation),"),
        "delete_workgroup (LifecycleWorkgroupRemoval GUI site) must construct and thread the token \
         into gated_workgroup_delete_with"
    );
}
