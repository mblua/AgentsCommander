//! Integration tests for project registration CLI refresh requests.
//!
//! Each test copies the binary under test into a temp directory so
//! `config_dir()` resolves to an isolated sibling config directory.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Tmp {
    fn new(prefix: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("ac-{}-{}", prefix, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("create tmp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    std::fs::copy(src, &dst).expect("copy binary");
    dst
}

fn config_dir_for_bin(bin: &Path) -> PathBuf {
    let stem = bin
        .file_stem()
        .expect("bin stem")
        .to_string_lossy()
        .to_string();
    bin.parent().expect("bin parent").join(format!(".{}", stem))
}

fn write_settings(config_dir: &Path, project_paths: &[&Path]) {
    std::fs::create_dir_all(config_dir).expect("create config dir");
    let settings = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "projectPaths": project_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
    });
    std::fs::write(
        config_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings).expect("settings json"),
    )
    .expect("write settings");
}

fn project_refresh_request_paths(config_dir: &Path) -> Vec<PathBuf> {
    let requests_dir = config_dir.join("project-refresh-requests");
    if !requests_dir.exists() {
        return Vec::new();
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&requests_dir)
        .expect("read project-refresh-requests")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    paths.sort();
    paths
}

fn clear_project_refresh_requests(config_dir: &Path) {
    let requests_dir = config_dir.join("project-refresh-requests");
    if requests_dir.exists() {
        std::fs::remove_dir_all(requests_dir).expect("clear requests");
    }
}

fn read_single_project_refresh_request(config_dir: &Path) -> serde_json::Value {
    let requests = project_refresh_request_paths(config_dir);
    assert_eq!(requests.len(), 1, "expected one project refresh request");
    serde_json::from_str(&std::fs::read_to_string(&requests[0]).expect("read request"))
        .expect("request json")
}

fn assert_registration_request(request: &serde_json::Value, project: &Path) {
    uuid::Uuid::parse_str(request["id"].as_str().expect("id")).expect("request id");
    assert!(!request["timestamp"].as_str().expect("timestamp").is_empty());
    assert_eq!(
        request["projectPath"].as_str().expect("projectPath"),
        std::fs::canonicalize(project)
            .expect("canonical project")
            .to_string_lossy()
            .as_ref()
    );
    assert!(request
        .get("changedPath")
        .is_none_or(|value| value.is_null()));
    assert!(request
        .get("changedName")
        .is_none_or(|value| value.is_null()));
    assert_eq!(request["reason"], "projectRegistered");
}

fn run_success(bin: &Path, args: &[&str]) {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_json(bin: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout json")
}

fn run_failure(bin: &Path, args: &[&str]) {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        !out.status.success(),
        "expected failure\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_no_ac_side_effect_dirs(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                assert!(
                    !(name.starts_with("wg-")
                        || name.starts_with("_team_")
                        || name.starts_with("_agent_")),
                    "unexpected side-effect directory {}",
                    path.display()
                );
                stack.push(path);
            }
        }
    }
}

#[test]
fn new_project_writes_project_registered_refresh() {
    let tmp = Tmp::new("cli-new-project-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["new-project", &project_arg]);

    assert!(project.join(".ac").is_dir());
    let request = read_single_project_refresh_request(&config_dir);
    assert_registration_request(&request, &project);
}

#[test]
fn open_project_writes_project_registered_refresh_for_new_registration() {
    let tmp = Tmp::new("cli-open-project-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac")).expect("create project");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["open-project", &project_arg]);

    let request = read_single_project_refresh_request(&config_dir);
    assert_registration_request(&request, &project);
}

#[test]
fn open_project_noop_does_not_write_refresh() {
    let tmp = Tmp::new("cli-open-project-noop-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac")).expect("create project");
    write_settings(&config_dir, &[&project]);
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["open-project", &project_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn new_project_bad_parent_path_does_not_write_settings_or_refresh() {
    let tmp = Tmp::new("cli-new-project-bad-parent");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let config_sentinel = config_dir.join("sentinel.txt");
    std::fs::write(&config_sentinel, "keep").expect("write config sentinel");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let parent_file = tmp.path().join("parent-file");
    std::fs::write(&parent_file, "not a dir").expect("write parent file");
    let bad_project = parent_file.join("ChildProject");
    let bad_project_arg = bad_project.to_string_lossy().to_string();

    run_failure(&bin, &["new-project", &bad_project_arg]);

    assert!(!bad_project.exists());
    assert!(
        !config_dir.join("settings.json").exists(),
        "settings.json should not be written on failed new-project"
    );
    assert!(project_refresh_request_paths(&config_dir).is_empty());
    assert_eq!(
        std::fs::read_to_string(&config_sentinel).expect("read config sentinel"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_no_ac_side_effect_dirs(tmp.path());
}

#[test]
fn new_project_noop_does_not_write_refresh() {
    let tmp = Tmp::new("cli-new-project-noop-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();
    run_success(&bin, &["new-project", &project_arg]);
    clear_project_refresh_requests(&config_dir);

    run_success(&bin, &["new-project", &project_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn open_project_invalid_path_does_not_write_refresh() {
    let tmp = Tmp::new("cli-open-project-invalid-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let missing = tmp.path().join("MissingProject");
    let missing_arg = missing.to_string_lossy().to_string();

    run_failure(&bin, &["open-project", &missing_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

// #1065 Stage F ACTIVATED: registering a fresh project publishes its two project
// context templates, so `new-project` emits `.ac/seed-manifest.toml` with one
// `project_context_template` row each for `context:agentscommander` and
// `context:coordinator`. A subsequent open of the already-registered project is a
// no-op registration that must not backfill or rewrite the manifest (acceptance
// items 22/38). Public-outcome only: no activation token or private hook.
#[test]
fn new_project_emits_seed_manifest_and_open_does_not_backfill() {
    let tmp = Tmp::new("cli-project-registration-activated");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["new-project", &project_arg]);
    assert!(project.join(".ac").is_dir());
    let manifest_path = project.join(".ac").join("seed-manifest.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .expect("fresh new-project must emit a seed manifest once Stage F is activated");
    assert!(
        manifest.contains("schema_version = 1"),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("coverage_version = 2"),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:agentscommander\""),
        "manifest must record the project context template: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:coordinator\""),
        "manifest must record the coordinator context template: {manifest}"
    );
    assert!(
        manifest.contains("kind = \"project_context_template\""),
        "manifest: {manifest}"
    );
    assert!(
        !manifest.contains("replica_config_file"),
        "registration publishes no config-folder rows: {manifest}"
    );

    // A subsequent open of the now-registered project is a no-op registration: the
    // templates already exist, so nothing is published and the manifest is unchanged.
    run_success(&bin, &["open-project", &project_arg]);
    let after_open = std::fs::read_to_string(&manifest_path).expect("manifest persists");
    assert_eq!(
        manifest, after_open,
        "open of an already-registered project must not rewrite or backfill the manifest"
    );
}

fn copy_dir_recursive(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create clone dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("entry");
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir_recursive(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy file");
        }
    }
}

// #1065 Stage F: a cloned project (whose `.ac` was copied from an origin that a
// fresh `new-project` seeded) carries the origin's committed seed manifest
// byte-for-byte, and opening the clone (a no-op registration of an existing `.ac`)
// preserves it without backfilling, re-timestamping, or pruning (plan section 5.4
// clone row, acceptance items 19/38). Public-outcome only: no activation token,
// private hook, or helper barrier is used.
#[test]
fn cloned_project_open_preserves_the_manifest_byte_for_byte() {
    let tmp = Tmp::new("cli-project-clone-activated");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let origin = tmp.path().join("Origin");
    let origin_arg = origin.to_string_lossy().to_string();
    run_success(&bin, &["new-project", &origin_arg]);
    let origin_manifest = origin.join(".ac").join("seed-manifest.toml");
    let origin_bytes = std::fs::read(&origin_manifest)
        .expect("origin new-project emits a seed manifest once Stage F is activated");

    // Clone: copy the whole project (including `.ac` and its manifest) to a new
    // location.
    let clone = tmp.path().join("Clone");
    copy_dir_recursive(&origin, &clone);
    let clone_manifest = clone.join(".ac").join("seed-manifest.toml");
    assert_eq!(
        std::fs::read(&clone_manifest).expect("the clone carries its manifest"),
        origin_bytes,
        "the clone starts with the origin's manifest bytes"
    );

    let clone_arg = clone.to_string_lossy().to_string();
    run_success(&bin, &["open-project", &clone_arg]);

    assert_eq!(
        std::fs::read(&clone_manifest).expect("clone manifest persists"),
        origin_bytes,
        "opening a clone preserves the manifest byte-for-byte (no backfill, reprune, or re-timestamp)"
    );
    assert_eq!(
        std::fs::read(&origin_manifest).expect("origin manifest persists"),
        origin_bytes,
        "the origin manifest is untouched by opening the clone"
    );
}

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

/// Read a crate-relative source file, drop its `#[cfg(test)] mod tests` block,
/// and collapse ALL whitespace. Dropping the test block matches the cited
/// precedent and makes the "production only" property ENFORCED rather than
/// assumed: a future `#[cfg(test)]` use of `production()` can no longer offset a
/// removed production wiring. Collapsing whitespace makes assertions match token
/// sequences irrespective of rustfmt line-wrapping.
fn normalized_production_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let normalized = body.split_whitespace().collect::<String>();
    let end = normalized.find("#[cfg(test)]modtests{").unwrap_or_else(|| {
        panic!("{relative}: no `#[cfg(test)] mod tests` block found to split off")
    });
    normalized[..end].to_string()
}

/// Count production activation-token constructions in a scraped production
/// slice. The token string carries no interior whitespace, so it survives
/// normalization.
fn production_token_count(production: &str) -> usize {
    production
        .matches("ManifestActivationToken::production()")
        .count()
}

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
        agentscommander_lib::config::seeded_context_templates::scan_project_context_template_updates(
            &project, &workspace,
        )
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
        production.contains(
            "scan_project_context_templates_recorded(&base,&ac_root,activation.as_ref()"
        ),
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

// #1318: a fresh `new-project` registration seeds the coding-agent catalog +
// masters into `<project>/.ac/coding-agents/` immediately (no restart needed)
// and records one `coding_agent_catalog` row in the seed manifest (production
// binary: the token gate emits the row under `CatalogSeed`). Re-running
// `new-project` must not change a byte of the catalog (whole-file seed-once).
#[test]
fn new_project_seeds_catalog_into_ac() {
    let tmp = Tmp::new("new-project-catalog");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["new-project", &project_arg]);
    let catalog_path = project
        .join(".ac")
        .join("coding-agents")
        .join("agents.json");
    let catalog = std::fs::read_to_string(&catalog_path)
        .expect("fresh new-project must seed .ac/coding-agents/agents.json");
    let parsed: serde_json::Value = serde_json::from_str(&catalog).expect("catalog parses");
    assert_eq!(
        parsed["agents"].as_array().map(Vec::len),
        Some(7),
        "the seeded catalog carries the 7 built-ins: {catalog}"
    );
    let claude = parsed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["key"] == "claude")
        .expect("claude entry");
    assert_eq!(
        claude["updateCommands"],
        serde_json::json!(["claude --update"]),
        "claude carries its update command in the seeded file"
    );
    // Masters seeded per built-in dest.
    for (dest, file) in [
        (".claude", "settings.json"),
        (".codex", "config.toml"),
        (".opencode", "opencode.json"),
    ] {
        assert!(
            project
                .join(".ac")
                .join("coding-agents")
                .join("_seed")
                .join(dest)
                .join(file)
                .is_file(),
            "{dest} master must be seeded"
        );
    }
    // Seed manifest declares the catalog publication (coverage v2 after the
    // one-shot upgrade).
    let manifest = std::fs::read_to_string(project.join(".ac").join("seed-manifest.toml"))
        .expect("seed manifest");
    assert!(
        manifest.contains("kind = \"coding_agent_catalog\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"catalog:coding-agents\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("path = \".ac/coding-agents/agents.json\""),
        "manifest: {manifest}"
    );

    // Re-run new-project: seed-once keeps the catalog bytes identical.
    run_success(&bin, &["new-project", &project_arg]);
    assert_eq!(
        std::fs::read_to_string(&catalog_path).expect("catalog still exists"),
        catalog,
        "a second registration must not touch the seeded catalog"
    );
}

// #1318 CLI read contract: `coding-agent catalog` with no registered project
// serves the legacy `<config_dir>/coding-agents/agents.json` when one exists
// (read-only fallback; pre-migration installs keep today's read behavior), else
// the 6 embedded built-ins. Never an Err for file reasons.
#[test]
fn cli_catalog_serves_legacy_fallback_then_embedded_without_projects() {
    let tmp = Tmp::new("cli-catalog-legacy");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);

    // Legacy catalog present -> served verbatim (custom entry observable).
    let legacy_dir = config_dir.join("coding-agents");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    let legacy = r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]}"##;
    std::fs::write(legacy_dir.join("agents.json"), legacy).unwrap();
    let out = run_json(&bin, &["coding-agent", "catalog"]);
    assert_eq!(
        out.as_array().map(Vec::len),
        Some(1),
        "the legacy catalog is served when no project is registered: {out}"
    );
    assert_eq!(out[0]["key"], "mine");

    // No legacy -> 7 embedded built-ins.
    std::fs::remove_dir_all(&legacy_dir).unwrap();
    let out = run_json(&bin, &["coding-agent", "catalog"]);
    assert_eq!(
        out.as_array().map(Vec::len),
        Some(7),
        "without a legacy catalog the embedded default is served: {out}"
    );
}
