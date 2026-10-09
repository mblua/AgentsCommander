//! Integration tests for project registration CLI refresh requests.
//!
//! Each test copies the binary under test into a temp directory so
//! `config_dir()` resolves to an isolated sibling config directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

/// Excludes one test's open write descriptor on a freshly copied binary from
/// overlapping another test's fork/exec.
///
/// These tests run in parallel and each copies the binary into its own temp dir
/// before exec'ing it. `Command::spawn` forks, and the child inherits the write
/// descriptor another thread still holds on *its* copy; exec'ing a binary that
/// any process holds open for writing fails with `ETXTBSY`. Covering both the
/// copy and the spawn closes that window. The lock is released before output is
/// collected, so the binary runs themselves still overlap.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn spawn_lock() -> MutexGuard<'static, ()> {
    // A test that panics elsewhere must not disable the guard for the rest.
    SPAWN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn command_for_binary(bin: &Path) -> Command {
    let mut command = Command::new(bin);
    let stem = bin.file_stem().expect("bin stem").to_string_lossy();
    if !stem.contains('_') {
        command.env("AGENTSCOMMANDER_CONFIG_DIR", config_dir_for_bin(bin));
    }
    command
}

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
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    {
        let _guard = spawn_lock();
        std::fs::copy(src, &dst).expect("copy binary");
    }
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
        config_dir.join("settings.30.instance.no-git.json"),
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

/// Drain both pipes while waiting so output cannot defeat the child deadline.
fn bounded_output(mut child: std::process::Child) -> std::process::Output {
    use std::io::Read;
    use std::time::{Duration, Instant};

    let stdout = child.stdout.take().expect("stdout pipe");
    let stderr = child.stderr.take().expect("stderr pipe");
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    };
    let stdout = drain(Box::new(stdout));
    let stderr = drain(Box::new(stderr));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => {
                let reason = format!("child deadline/wait failure: {result:?}");
                let _ = child.kill();
                let reaped = child.wait();
                break Err(format!("{reason}; reap: {reaped:?}"));
            }
        }
    };
    let stdout = stdout.join().expect("stdout reader").expect("read stdout");
    let stderr = stderr.join().expect("stderr reader").expect("read stderr");
    std::process::Output {
        status: status.unwrap_or_else(|error| {
            panic!(
                "{error}
stdout: {}
stderr: {}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            )
        }),
        stdout,
        stderr,
    }
}

fn run_success(bin: &Path, args: &[&str]) {
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = bounded_output(child);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Success capture that also exposes stderr: #1967 asserts the CLI diagnostics
/// (stdout JSON plus stderr warnings) while keeping this file's frozen route
/// set (the issue_1867 isolation contract counts `command_for_binary` routes).
fn run_json(bin: &Path, args: &[&str]) -> (serde_json::Value, String) {
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = bounded_output(child);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (
        serde_json::from_slice(&out.stdout).expect("stdout json"),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Failure capture returning the raw output so the caller can assert the exact
/// exit status, stdout and stderr (the #1967 unavailable diagnostic).
fn run_failure(bin: &Path, args: &[&str]) -> std::process::Output {
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = bounded_output(child);
    assert!(
        !out.status.success(),
        "expected failure\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
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
        !config_dir.join("settings.30.instance.no-git.json").exists(),
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
        .join("agents.10.default.json");
    let catalog = std::fs::read_to_string(&catalog_path)
        .expect("fresh new-project must seed .ac/coding-agents/agents.10.default.json");
    let parsed: serde_json::Value = serde_json::from_str(&catalog).expect("catalog parses");
    assert_eq!(
        parsed["agents"].as_array().map(Vec::len),
        Some(8),
        "the seeded catalog carries the 8 enabled built-ins: {catalog}"
    );
    assert_eq!(parsed["schemaVersion"], 1);
    assert!(
        !catalog.contains("\"muse\""),
        "the disabled muse row must not be seeded: {catalog}"
    );
    assert_eq!(
        parsed["agents"].as_array().unwrap().last().unwrap(),
        &serde_json::json!({
            "key": "grok",
            "label": "Grok Build",
            "description": "Coding Agent by SpaceXAI",
            "color": "#64748b",
            "command": "grok",
            "instructionsFilename": "AGENTS.md",
            "envs": [],
            "isolatedHome": false,
            "removable": true,
            "installCommands": {
                "windows": "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AeAAuAGEAaQAvAGMAbABpAC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAEYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA",
                "macos": "ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash",
                "linux": "ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash",
                "default": "echo No verified installer for this platform 1>&2 && exit 1"
            },
            "updateCommands": [],
            "autoUpdate": false,
            "idleBurst": {
                "maxBytes": 1024,
                "maxSecs": 3.0,
                "priorSilenceSecs": 60.0
            }
        })
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
    let seed_root = project.join(".ac").join("coding-agents").join("_seed");
    let mut seed_names: Vec<_> = std::fs::read_dir(&seed_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    seed_names.sort();
    assert_eq!(seed_names, [".claude", ".codex", ".opencode"]);
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

// #1318/#1967 CLI read contract: `coding-agent catalog` with no registered
// project serves the legacy `<config_dir>/coding-agents/agents.10.default.json` when one
// exists (read-only; pre-migration installs keep today's read behavior). With
// no legacy catalog the persisted-only read is UNAVAILABLE: nonzero exit, the
// baseUnavailable diagnostic (code + selected path + reason) on stderr, no
// catalog on stdout, no embedded fallback and no read-time directory creation.
#[test]
fn cli_catalog_serves_legacy_then_unavailable_without_projects() {
    let tmp = Tmp::new("cli-catalog-legacy");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);

    // Legacy catalog present -> served verbatim (custom entry observable) as a
    // JSON array on stdout, with the migrationPending warning on stderr.
    let legacy_dir = config_dir.join("coding-agents");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    let legacy = r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]}"##;
    std::fs::write(legacy_dir.join("agents.10.default.json"), legacy).unwrap();
    let (served, stderr) = run_json(&bin, &["coding-agent", "catalog"]);
    assert_eq!(
        served.as_array().map(Vec::len),
        Some(1),
        "the legacy catalog is served when no project is registered: {served}"
    );
    assert_eq!(served[0]["key"], "mine");
    assert_eq!(served[0]["updateCommands"], serde_json::json!([]));
    assert!(stderr.contains("migrationPending"), "stderr: {stderr}");
    assert_eq!(
        std::fs::read_to_string(legacy_dir.join("agents.10.default.json")).unwrap(),
        legacy,
        "the read leaves the legacy bytes untouched"
    );

    // No legacy -> unavailable. Direct output capture (run_failure now returns
    // the raw Output): nonzero, the selected agents.10.default.json path plus reason on
    // stderr, no success catalog on stdout, and repeated calls never recreate
    // the legacy directory.
    std::fs::remove_dir_all(&legacy_dir).unwrap();
    let selected = legacy_dir.join("agents.10.default.json");
    for _ in 0..2 {
        let out = run_failure(&bin, &["coding-agent", "catalog"]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.trim().is_empty(),
            "no catalog may be printed for an unavailable base: {stdout}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("baseUnavailable"), "stderr: {stderr}");
        assert!(
            stderr.contains(&selected.display().to_string()),
            "stderr must name the selected agents.10.default.json path: {stderr}"
        );
        assert!(
            stderr.contains("no persisted catalog"),
            "stderr must carry the reason: {stderr}"
        );
    }
    assert!(
        !legacy_dir.exists(),
        "catalog reads must not create the legacy catalog directory"
    );
}

// #1967 P4 CLI contract: `add --from-catalog` resolves the PERSISTED catalog
// only. An unknown key keeps the existing not-found error and must leave
// settings byte-identical; a successful add seeds from the persisted sentinel
// and leaves the unrelated pre-existing registration unchanged.
#[test]
fn cli_add_from_catalog_is_persisted_only_and_preserves_existing_agents() {
    let tmp = Tmp::new("cli-catalog-add");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(&project).expect("create project");
    write_settings(&config_dir, &[&project]);

    // One unrelated, pre-existing registration.
    let settings_path = config_dir.join("settings.30.instance.no-git.json");
    let mut settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read settings"))
            .expect("settings json");
    settings["agents"] = serde_json::json!([{
        "id": "existing-1967",
        "label": "Existing",
        "command": "existing-1967-command",
        "color": "#123456",
        "envs": [],
        "isolatedHome": false
    }]);
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).expect("settings json"),
    )
    .expect("write settings");
    let before = std::fs::read(&settings_path).expect("read settings bytes");

    // Persisted catalog with one sentinel entry.
    let catalog_dir = project.join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("create catalog dir");
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        r##"{"schemaVersion":1,"agents":[{"key":"sentinel-1967","label":"Sentinel","description":"d","color":"#654321","command":"sentinel-1967-command","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["sentinel update"]}]}"##,
    )
    .expect("write catalog");

    // Unknown key: existing not-found error, settings byte-identical.
    let out = run_failure(
        &bin,
        &["coding-agent", "add", "--from-catalog", "missing-key"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("catalog key 'missing-key' not found"),
        "stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read(&settings_path).expect("settings bytes"),
        before,
        "a failed add must not mutate settings"
    );

    // Successful add: seeded from the persisted sentinel; the pre-existing
    // registration keeps every field.
    run_success(
        &bin,
        &[
            "coding-agent",
            "add",
            "--from-catalog",
            "sentinel-1967",
            "--id",
            "added-1967",
        ],
    );
    // #2716 (B3): the saved `agents` live in the agents file beside the settings file.
    let after: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config_dir.join("agents.30.instance.no-git.json"))
            .expect("read agents file"),
    )
    .expect("agents json");
    let agents = after["agents"].as_array().expect("agents array");
    assert_eq!(agents.len(), 2, "exactly one agent was added: {after}");
    let existing = agents
        .iter()
        .find(|agent| agent["id"] == "existing-1967")
        .expect("existing agent");
    assert_eq!(existing["label"], "Existing");
    assert_eq!(existing["command"], "existing-1967-command");
    assert_eq!(existing["color"], "#123456");
    assert_eq!(existing["envs"], serde_json::json!([]));
    assert_eq!(existing["isolatedHome"], serde_json::json!(false));
    let added = agents
        .iter()
        .find(|agent| agent["id"] == "added-1967")
        .expect("added agent");
    assert_eq!(added["label"], "Sentinel");
    assert_eq!(added["command"], "sentinel-1967-command");
    assert_eq!(added["color"], "#654321");
    // #2306 P1 - the pre-existing record keeps position 0, the add lands at 1.
    assert_eq!(existing["order"], serde_json::json!(0));
    assert_eq!(added["order"], serde_json::json!(1));

    // #2306 P1 - `list`/`show` surface the normalized explicit order after load.
    let (listed, _) = run_json(&bin, &["coding-agent", "list"]);
    let listed = listed.as_array().expect("list is a JSON array");
    assert_eq!(listed[0]["id"], "existing-1967");
    assert_eq!(listed[0]["order"], serde_json::json!(0));
    assert_eq!(listed[1]["id"], "added-1967");
    assert_eq!(listed[1]["order"], serde_json::json!(1));
    let (shown, _) = run_json(&bin, &["coding-agent", "show", "--id", "added-1967"]);
    assert_eq!(shown["order"], serde_json::json!(1));
    assert_eq!(
        std::fs::read_to_string(catalog_dir.join("agents.10.default.json"))
            .expect("catalog persists"),
        r##"{"schemaVersion":1,"agents":[{"key":"sentinel-1967","label":"Sentinel","description":"d","color":"#654321","command":"sentinel-1967-command","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["sentinel update"]}]}"##,
        "add --from-catalog never writes the persisted catalog"
    );
}

// P05: production consumers must never finalize GUI migrations while reading.
const P05_TOKEN: &str = "a873f0dd-732b-46b7-abd4-f690dc37080a";
const P05_MESSAGE: &str = "20261009-150000-room01-peer-to-room01-coord-p05.md";

struct ReadonlyFixture {
    _tmp: Tmp,
    bin: PathBuf,
    config: PathBuf,
    project: PathBuf,
    coordinator: String,
    peer: String,
    origin: String,
}

impl ReadonlyFixture {
    fn new() -> Self {
        let tmp = Tmp::new("p05-readonly");
        let bin = copy_binary_into(tmp.path());
        let config = config_dir_for_bin(&bin);
        let project = tmp.path().join("RegisteredRemote");
        let ac = project.join(".ac");
        let room = ac.join("room-01-devs");
        for name in ["coord", "peer"] {
            let origin = ac.join(format!("_agent_{name}"));
            let replica = room.join(format!("__agent_{name}"));
            std::fs::create_dir_all(&origin).unwrap();
            std::fs::create_dir_all(&replica).unwrap();
            std::fs::write(
                origin.join("config.json"),
                r#"{"tooling":{"lastCodingAgent":"codex"}}"#,
            )
            .unwrap();
            std::fs::write(
                replica.join("config.json"),
                format!(r#"{{"identity":"../../_agent_{name}"}}"#),
            )
            .unwrap();
        }
        let team = ac.join("_team_devs");
        std::fs::create_dir_all(&team).unwrap();
        std::fs::write(
            team.join("config.json"),
            r#"{"agents":["_agent_coord","_agent_peer"],"coordinator":"_agent_coord","repos":[]}"#,
        )
        .unwrap();
        let messaging = room.join("messaging");
        std::fs::create_dir_all(&messaging).unwrap();
        std::fs::write(messaging.join(P05_MESSAGE), "P05 message").unwrap();
        write_settings(&config, &[&project]);
        // Use the registered folder name required by CLI project resolution.
        // Settle naming journal/lock before any managed-byte snapshot or probe.
        let _ = run_json(
            &bin,
            &["workgroup", "list", "--project", "RegisteredRemote"],
        );
        let value = serde_json::json!({
            "defaultShell":"powershell.exe", "defaultShellArgs":[],
            "agents":[], "projectPaths":[project],
            "startOnlyCoordinators":true, "sidebarZoom":1.25
        });
        std::fs::write(
            config.join("settings.30.instance.no-git.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        Self {
            bin,
            config,
            coordinator: room.join("__agent_coord").to_string_lossy().into_owned(),
            peer: room.join("__agent_peer").to_string_lossy().into_owned(),
            origin: ac.join("_agent_coord").to_string_lossy().into_owned(),
            project,
            _tmp: tmp,
        }
    }

    fn managed_bytes(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(&self.config)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                // Lock/journal/log files have separate, existing authority.
                path.is_file()
                    && (name.starts_with("settings")
                        || name.starts_with("agents")
                        || name.starts_with("project-paths"))
                    && !name.ends_with(".lock")
            })
            .map(|path| {
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read(path).unwrap(),
                )
            })
            .collect()
    }

    fn outbox_for_root(&self, root: &str) -> PathBuf {
        Path::new(root)
            .join(self.config.file_name().unwrap())
            .join("outbox")
    }

    fn assert_no_outbox(&self) {
        for root in [&self.coordinator, &self.peer] {
            let path = self.outbox_for_root(root);
            assert!(!path.exists() || std::fs::read_dir(path).unwrap().next().is_none());
        }
    }

    fn assert_reader(&self, args: &[&str]) -> serde_json::Value {
        let before = self.managed_bytes();
        let (value, _) = run_json(&self.bin, args);
        assert_eq!(self.managed_bytes(), before, "managed writes from {args:?}");
        value
    }

    fn assert_denied(&self, args: &[&str], diagnostic: &str) {
        let before = self.managed_bytes();
        let out = run_failure(&self.bin, args);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(diagnostic),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains("Queued:"));
        assert!(
            serde_json::from_slice::<serde_json::Value>(&out.stdout).is_err(),
            "source/authorization refusal emitted success JSON"
        );
        assert_eq!(self.managed_bytes(), before, "managed writes from {args:?}");
        self.assert_no_outbox();
    }
}

#[test]
fn p05_real_peer_discovery_uses_registered_remote_without_managed_writes() {
    let fixture = ReadonlyFixture::new();
    // A root outside the registered project cannot supply its path by augmentation.
    let root = fixture.config.join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    for verb in ["list-peers", "list-peers-lean"] {
        for caller in [
            root.to_str().unwrap(),
            fixture.origin.as_str(),
            fixture.peer.as_str(),
        ] {
            let peers = fixture.assert_reader(&[verb, "--token", P05_TOKEN, "--root", caller]);
            let coord = peers
                .as_array()
                .unwrap()
                .iter()
                .find(|peer| peer["name"] == "RegisteredRemote:room-01-devs/coord")
                .expect("registered coordinator peer");
            assert_eq!(coord["path"], fixture.coordinator);
        }
    }
}

#[test]
fn p05_real_send_enqueues_and_close_resolves_before_coordinator_denial() {
    let fixture = ReadonlyFixture::new();
    let before = fixture.managed_bytes();
    let out = run_failure(
        &fixture.bin,
        &[
            "send",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--to",
            "RegisteredRemote:room-01-devs/coord",
            "--send",
            P05_MESSAGE,
            "--mode",
            "wake",
            "--confirm-timeout",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let receipt = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Queued: "))
        .expect("durable enqueue receipt");
    let id = receipt.split_whitespace().next().unwrap();
    uuid::Uuid::parse_str(id).unwrap();
    assert_eq!(receipt, id, "exact default enqueue receipt");
    assert!(String::from_utf8_lossy(&out.stderr).contains("delivery confirmation timeout after 0s"));
    let outbox = fixture.outbox_for_root(&fixture.peer);
    let queued_path = outbox.join(format!("{id}.json"));
    let queued: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&queued_path).unwrap()).unwrap();
    assert_eq!(queued["to"], "RegisteredRemote:room-01-devs/coord");
    assert_eq!(queued["from"], "RegisteredRemote:room-01-devs/peer");
    assert_eq!(queued["mode"], "wake");
    assert_eq!(queued["body"], "P05 message");
    assert_eq!(fixture.managed_bytes(), before);
    std::fs::remove_file(queued_path).unwrap();
    fixture.assert_denied(
        &[
            "close-session",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--target",
            "RegisteredRemote:room-01-devs/coord",
        ],
        "authorization denied",
    );
}

#[test]
fn p05_real_task_consumers_preserve_coordinator_and_denial_contracts() {
    let fixture = ReadonlyFixture::new();
    let before = fixture.managed_bytes();
    for (verb, flag, text) in [
        ("task-set-title", "--title", "P05"),
        ("task-append-body", "--text", "First"),
        ("task-set-body", "--text", "Replacement"),
    ] {
        run_success(
            &fixture.bin,
            &[
                verb,
                "--token",
                P05_TOKEN,
                "--root",
                &fixture.coordinator,
                flag,
                text,
            ],
        );
        assert_eq!(fixture.managed_bytes(), before, "{verb}");
        let task = fixture.project.join(".ac/room-01-devs/TASK.md");
        let task_before = std::fs::read(&task).unwrap();
        fixture.assert_denied(
            &[
                verb,
                "--token",
                P05_TOKEN,
                "--root",
                &fixture.peer,
                flag,
                "Denied",
            ],
            "authorization denied",
        );
        assert_eq!(std::fs::read(task).unwrap(), task_before);
    }
    let snapshot = fixture.assert_reader(&[
        "task-get",
        "--token",
        P05_TOKEN,
        "--root",
        &fixture.coordinator,
    ]);
    let revision = snapshot["revision"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let status = fixture.assert_reader(&[
        "task-status-set",
        "--token",
        P05_TOKEN,
        "--root",
        &fixture.coordinator,
        "--expected-revision",
        revision,
        "--request-id",
        &request,
        "--text",
        "P05 pending",
    ]);
    assert_eq!(status["status"], "P05 pending");
    for verb in ["task-get", "task-status-set"] {
        let mut args = vec![verb, "--token", P05_TOKEN, "--root", &fixture.peer];
        if verb == "task-status-set" {
            args.extend([
                "--expected-revision",
                revision,
                "--request-id",
                &request,
                "--text",
                "Denied",
            ]);
        }
        fixture.assert_denied(&args, "authorization_denied");
    }
    let after = fixture.assert_reader(&[
        "task-get",
        "--token",
        P05_TOKEN,
        "--root",
        &fixture.coordinator,
    ]);
    assert_eq!(after["status"], "P05 pending");
}

#[test]
fn p05_malformed_source_refuses_direct_and_task_consumers_before_outputs() {
    let fixture = ReadonlyFixture::new();
    std::fs::write(
        fixture.config.join("settings.30.instance.no-git.json"),
        b"{malformed",
    )
    .unwrap();
    for verb in ["list-peers", "list-peers-lean"] {
        fixture.assert_denied(
            &[verb, "--token", P05_TOKEN, "--root", &fixture.peer],
            "could not be parsed",
        );
    }
    fixture.assert_denied(
        &[
            "send",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--to",
            "RegisteredRemote:room-01-devs/coord",
            "--send",
            P05_MESSAGE,
            "--confirm-timeout",
            "0",
        ],
        "could not be parsed",
    );
    fixture.assert_denied(
        &[
            "close-session",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--target",
            "RegisteredRemote:room-01-devs/coord",
        ],
        "could not be parsed",
    );
    for (verb, flag) in [
        ("task-set-title", "--title"),
        ("task-append-body", "--text"),
        ("task-set-body", "--text"),
    ] {
        fixture.assert_denied(
            &[
                verb,
                "--token",
                P05_TOKEN,
                "--root",
                &fixture.coordinator,
                flag,
                "Denied",
            ],
            "authorization denied",
        );
    }
    fixture.assert_denied(
        &[
            "task-get",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.coordinator,
        ],
        "authorization_denied",
    );
    fixture.assert_denied(
        &[
            "task-status-set",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.coordinator,
            "--expected-revision",
            "legacy:0",
            "--request-id",
            P05_TOKEN,
            "--text",
            "Denied",
        ],
        "authorization_denied",
    );
    assert!(!fixture.project.join(".ac/room-01-devs/TASK.md").exists());
    assert!(!fixture
        .project
        .join(".ac/room-01-devs/TASK-status.jsonl")
        .exists());
}

#[test]
fn p05_token_only_reads_preserve_root_master_and_uuid_behavior() {
    let fixture = ReadonlyFixture::new();
    for token in ["", "malformed-token"] {
        fixture.assert_denied(
            &["list-peers-lean", "--token", token, "--root", &fixture.peer],
            if token.is_empty() {
                "--token is required"
            } else {
                "invalid token supplied"
            },
        );
    }
    fixture.assert_denied(
        &["list-peers-lean", "--root", &fixture.peer],
        "--token is required",
    );
    // UUID is shape-valid but not privileged: the team gate still denies this peer.
    fixture.assert_denied(
        &[
            "task-set-title",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--title",
            "Denied",
        ],
        "authorization denied",
    );
    let path = fixture.config.join("settings.30.instance.no-git.json");
    let mut settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    settings["rootToken"] = "persisted-root".into();
    std::fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
    let before = fixture.managed_bytes();
    run_success(
        &fixture.bin,
        &[
            "task-set-title",
            "--token",
            "persisted-root",
            "--root",
            &fixture.peer,
            "--title",
            "Root bypass",
        ],
    );
    assert_eq!(fixture.managed_bytes(), before);
    settings.as_object_mut().unwrap().remove("rootToken");
    std::fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
    std::fs::write(
        fixture.config.join("master-token.txt"),
        "persisted-master\n",
    )
    .unwrap();
    let before = fixture.managed_bytes();
    run_success(
        &fixture.bin,
        &[
            "task-set-title",
            "--token",
            "persisted-master",
            "--root",
            &fixture.peer,
            "--title",
            "Master bypass",
        ],
    );
    assert_eq!(fixture.managed_bytes(), before);
}

#[test]
fn p05_missing_and_valid_empty_settings_remain_readonly() {
    let fixture = ReadonlyFixture::new();
    let root = fixture.config.join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    let path = fixture.config.join("settings.30.instance.no-git.json");
    for absent in [true, false] {
        if absent {
            std::fs::remove_file(&path).unwrap();
        } else {
            write_settings(&fixture.config, &[]);
        }
        let peers = fixture.assert_reader(&[
            "list-peers-lean",
            "--token",
            P05_TOKEN,
            "--root",
            root.to_str().unwrap(),
        ]);
        assert_eq!(peers, serde_json::json!([]));
        // WG still reports its own peer from the caller's room after a valid read.
        let peers = fixture.assert_reader(&[
            "list-peers-lean",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
        ]);
        assert!(peers
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "RegisteredRemote:room-01-devs/coord"));
    }
}

fn write_p05_custom_settings(fixture: &ReadonlyFixture) {
    let value = serde_json::json!({
        "defaultShell":"powershell.exe", "defaultShellArgs":[],
        "projectPaths":[fixture.project], "npmUpdateNotificationsEnabled":false,
        "sidebarZoom":1.0, "mainZoom":1.0,
        "agents":[{"id":"p05","label":"P05 custom","command":"codex","color":"#123456","instructionsFilename":"P05-INSTRUCTIONS.md"}]
    });
    std::fs::write(
        fixture.config.join("settings.30.instance.no-git.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

#[test]
fn p05_production_workgroup_gitignore_reads_custom_names_without_managed_writes() {
    let fixture = ReadonlyFixture::new();
    write_p05_custom_settings(&fixture);
    let before = fixture.managed_bytes();
    run_success(
        &fixture.bin,
        &[
            "workgroup",
            "add",
            "--project",
            "RegisteredRemote",
            "--team",
            "devs",
            "--title",
            "P05 created room",
        ],
    );
    assert_eq!(
        fixture.managed_bytes(),
        before,
        "production gitignore loader wrote managed config"
    );
    let ignore = std::fs::read_to_string(fixture.project.join(".ac/.gitignore")).unwrap();
    assert!(
        ignore.contains("**/__agent_*/P05-INSTRUCTIONS.md"),
        "{ignore}"
    );
    let rooms = run_json(
        &fixture.bin,
        &["workgroup", "list", "--project", "RegisteredRemote"],
    )
    .0;
    assert_eq!(rooms.as_array().unwrap().len(), 2);
}

#[test]
fn p05_production_new_project_preserves_authorized_legacy_writer_contract() {
    let fixture = ReadonlyFixture::new();
    write_p05_custom_settings(&fixture);
    for preexisting in [false, true] {
        let project = fixture
            ._tmp
            .path()
            .join(if preexisting { "Preexisting" } else { "Fresh" });
        if preexisting {
            std::fs::create_dir_all(project.join(".ac")).unwrap();
        }
        run_success(&fixture.bin, &["new-project", project.to_str().unwrap()]);
        let settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.config.join("settings.30.instance.no-git.json")).unwrap(),
        )
        .unwrap();
        assert!(
            settings.get("rootToken").is_none_or(|v| v.is_null()),
            "incidental loader generated root privilege: {settings}"
        );
        assert_eq!(settings["defaultShell"], "powershell.exe");
        assert_eq!(settings["npmUpdateNotificationsEnabled"], false);
        assert!(settings["projectPaths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str()
                == Some(
                    std::fs::canonicalize(&project)
                        .unwrap()
                        .to_string_lossy()
                        .as_ref()
                )));
        let ignore = std::fs::read_to_string(project.join(".ac/.gitignore")).unwrap();
        assert!(
            ignore.contains("**/__agent_*/P05-INSTRUCTIONS.md"),
            "{ignore}"
        );
        assert!(
            fixture
                .managed_bytes()
                .keys()
                .all(|name| !name.starts_with("project-paths")),
            "P05 must keep migration dormant"
        );
    }
}

#[test]
fn p05_root_send_resolves_remote_registration_without_own_project_fallback() {
    let fixture = ReadonlyFixture::new();
    let root = fixture.config.join("ac-root-agent");
    std::fs::create_dir_all(root.join("messaging")).unwrap();
    std::fs::write(root.join("messaging").join(P05_MESSAGE), "Remote P05").unwrap();
    let before = fixture.managed_bytes();
    let out = run_failure(
        &fixture.bin,
        &[
            "send",
            "--token",
            P05_TOKEN,
            "--root",
            root.to_str().unwrap(),
            "--to",
            "RegisteredRemote:room-01-devs/coord",
            "--send",
            P05_MESSAGE,
            "--mode",
            "wake",
            "--confirm-timeout",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Queued: "))
        .expect("exact enqueue receipt");
    uuid::Uuid::parse_str(id).unwrap();
    let queued: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .outbox_for_root(root.to_str().unwrap())
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(queued["to"], "RegisteredRemote:room-01-devs/coord");
    assert_eq!(queued["from"], "agentscommander://root-agent");
    assert!(String::from_utf8_lossy(&out.stderr).contains("delivery confirmation timeout after 0s"));
    assert_eq!(fixture.managed_bytes(), before);
    // Close resolves this same remote target before denying non-coordinator origin.
    let outsider = fixture._tmp.path().join("Unregistered/.ac/_agent_outsider");
    std::fs::create_dir_all(&outsider).unwrap();
    fixture.assert_denied(
        &[
            "close-session",
            "--token",
            P05_TOKEN,
            "--root",
            outsider.to_str().unwrap(),
            "--target",
            "RegisteredRemote:room-01-devs/coord",
        ],
        "authorization denied",
    );
}
