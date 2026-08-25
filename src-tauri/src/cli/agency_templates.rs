use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::commands::role_templates::{
    agency_template_metas_from_cache, agency_templates_dir, agency_templates_status,
    collect_agency_templates_from_dir, parse_agency_template_frontmatter, slug_segment,
    strip_yaml_frontmatter_for_role_template, title_case_slug, validate_role_template_body,
    AgencyTemplatesManifest, AGENCY_MANIFEST_FILE, AGENCY_TEMPLATES_DIR,
};

pub const DEFAULT_REPO: &str = "https://github.com/msitarzewski/agency-agents";
pub const DEFAULT_REFERENCE: &str = "main";
const GIT_LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_FETCH_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const GIT_CHECKOUT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Args)]
#[command(after_help = "\
NOTES:\n  \
  Downloads never run during app startup, list, resolve, or create.\n  \
  update writes <config-dir>/agency-agents_templates atomically.\n  \
  list and status read only the validated local cache and print JSON.")]
pub struct AgencyTemplatesArgs {
    #[command(subcommand)]
    pub command: AgencyTemplatesCommand,
}

#[derive(Subcommand)]
pub enum AgencyTemplatesCommand {
    /// Download or refresh the Agency Agents template cache
    Update(AgencyTemplatesUpdateArgs),
    /// Print cached Agency template metadata as JSON
    List(AgencyTemplatesListArgs),
    /// Print cache manifest/status as JSON
    Status(AgencyTemplatesStatusArgs),
}

#[derive(Args)]
pub struct AgencyTemplatesUpdateArgs {
    #[arg(long, default_value = DEFAULT_REPO)]
    pub repo: String,
    #[arg(long = "ref", default_value = DEFAULT_REFERENCE)]
    pub reference: String,
    /// Compatibility no-op. agency-templates commands always print JSON.
    #[arg(long)]
    pub json: bool,
}

impl AgencyTemplatesUpdateArgs {
    pub fn default_ui_update() -> Self {
        Self {
            repo: DEFAULT_REPO.into(),
            reference: DEFAULT_REFERENCE.into(),
            json: true,
        }
    }
}

#[derive(Args)]
pub struct AgencyTemplatesListArgs {
    /// Compatibility no-op. agency-templates list always prints JSON.
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct AgencyTemplatesStatusArgs {
    /// Compatibility no-op. agency-templates status always prints JSON.
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub commit: String,
    pub template_count: usize,
    pub path: String,
    pub updated: bool,
}

struct CacheLock {
    path: PathBuf,
    _file: File,
}

struct TempCacheDir {
    path: PathBuf,
}

impl TempCacheDir {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempCacheDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheLockInfo {
    pid: u32,
    created_unix_secs: u64,
}

impl CacheLock {
    fn acquire(config_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(config_dir).map_err(|e| {
            format!(
                "Failed to create config dir {}: {}",
                config_dir.display(),
                e
            )
        })?;
        let path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let created_unix_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let info = CacheLockInfo {
                        pid: process::id(),
                        created_unix_secs,
                    };
                    writeln!(
                        file,
                        "{}",
                        serde_json::to_string(&info)
                            .map_err(|e| format!("Failed to encode Agency cache lock: {}", e))?
                    )
                    .map_err(|e| {
                        format!(
                            "Failed to write Agency template cache lock {}: {}",
                            path.display(),
                            e
                        )
                    })?;
                    return Ok(Self { path, _file: file });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if recover_stale_lock_if_possible(config_dir, &path)? {
                        continue;
                    }
                    return Err(format!(
                        "Agency template cache lock exists at {}; another agency-templates update is already running or recovery is required",
                        path.display()
                    ));
                }
                Err(e) => {
                    return Err(format!(
                        "Failed to create Agency template cache lock {}: {}",
                        path.display(),
                        e
                    ));
                }
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.path) {
            eprintln!(
                "Warning: failed to remove Agency template cache lock {}: {}",
                self.path.display(),
                e
            );
        }
    }
}

fn recover_stale_lock_if_possible(config_dir: &Path, path: &Path) -> Result<bool, String> {
    let raw = fs::read_to_string(path).unwrap_or_default();
    let pid = serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| value.get("pid").and_then(|pid| pid.as_u64()))
        .and_then(|pid| u32::try_from(pid).ok());
    let stale = match pid {
        Some(pid) => !process_is_running(pid),
        None => live_missing_with_one_valid_previous_cache(config_dir),
    };
    if !stale {
        return Ok(false);
    }
    fs::remove_file(path).map_err(|e| {
        format!(
            "Failed to remove stale Agency template cache lock {}: {}",
            path.display(),
            e
        )
    })?;
    Ok(true)
}

fn live_missing_with_one_valid_previous_cache(config_dir: &Path) -> bool {
    let live = agency_templates_dir(config_dir);
    if live.exists() {
        return false;
    }
    let Ok(entries) = fs::read_dir(config_dir) else {
        return false;
    };
    let mut valid_prev = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&format!("{}.prev-", AGENCY_TEMPLATES_DIR))
            && collect_agency_templates_from_dir(&entry.path()).is_ok()
        {
            valid_prev += 1;
        }
    }
    valid_prev == 1
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }
    let filter = format!("PID eq {}", pid);
    // #992 - reachable from the GUI process (Tauri get_agency_templates_status /
    // update_agency_templates -> CacheLock::acquire -> recover_stale_lock_if_possible),
    // which owns no console: without the flag Windows pops a Windows Terminal tab for
    // tasklist.exe. Both pipes are captured, so nothing would ever be shown in it.
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = Command::new("tasklist");
    cmd.args(["/FI", &filter, "/NH", "/FO", "CSV"])
        .creation_flags(CREATE_NO_WINDOW);
    let Ok(output) = cmd.output() else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().any(|line| {
        line.split(',')
            .any(|field| field.trim_matches('"').trim() == pid.to_string())
    })
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(true)
}

#[cfg(not(any(unix, windows)))]
fn process_is_running(pid: u32) -> bool {
    pid == process::id()
}

pub fn execute(args: AgencyTemplatesArgs) -> i32 {
    let result = match args.command {
        AgencyTemplatesCommand::Update(args) => update(args),
        AgencyTemplatesCommand::List(args) => list(args),
        AgencyTemplatesCommand::Status(args) => status(args),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn config_dir_or_err() -> Result<PathBuf, String> {
    crate::config::config_dir()
        .ok_or_else(|| "Could not resolve AgentsCommander config dir".to_string())
}

fn print_json<T: Serialize>(value: &T, pretty: bool) -> Result<(), String> {
    let json = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
    crate::cli_println!("{}", json);
    Ok(())
}

pub fn status_cache_at(
    config_dir: &Path,
) -> Result<crate::commands::role_templates::AgencyTemplatesStatus, String> {
    match CacheLock::acquire(config_dir) {
        Ok(_lock) => {
            if let Err(e) = recover_interrupted_publish(config_dir) {
                if e.starts_with("cacheInvalid") {
                    return Ok(crate::commands::role_templates::AgencyTemplatesStatus {
                        available: false,
                        path: agency_templates_dir(config_dir)
                            .to_string_lossy()
                            .to_string(),
                        repo: None,
                        reference: None,
                        commit: None,
                        template_count: None,
                        reason: Some("cacheInvalid".into()),
                    });
                }
                return Err(e);
            }
        }
        Err(e) if e.contains("already running") => {
            return Ok(crate::commands::role_templates::AgencyTemplatesStatus {
                available: false,
                path: agency_templates_dir(config_dir)
                    .to_string_lossy()
                    .to_string(),
                repo: None,
                reference: None,
                commit: None,
                template_count: None,
                reason: Some("locked".into()),
            });
        }
        Err(e) => return Err(e),
    }
    Ok(agency_templates_status(config_dir))
}

pub fn status_cache() -> Result<crate::commands::role_templates::AgencyTemplatesStatus, String> {
    let config_dir = config_dir_or_err()?;
    status_cache_at(&config_dir)
}

fn status(args: AgencyTemplatesStatusArgs) -> Result<(), String> {
    let _ = args.json;
    print_json(&status_cache()?, args.pretty)
}

fn list(args: AgencyTemplatesListArgs) -> Result<(), String> {
    let _ = args.json;
    let config_dir = config_dir_or_err()?;
    match CacheLock::acquire(&config_dir) {
        Ok(_lock) => {
            let _ = recover_interrupted_publish(&config_dir)?;
        }
        Err(e) if e.contains("already running") => {
            return Err(format!(
                "Agency template cache is locked; run agency-templates status and retry after the updater finishes ({})",
                e
            ));
        }
        Err(e) => return Err(e),
    }
    print_json(&agency_template_metas_from_cache(&config_dir), args.pretty)
}

pub fn update_cache_at(
    config_dir: &Path,
    args: AgencyTemplatesUpdateArgs,
) -> Result<UpdateResult, String> {
    let _lock = CacheLock::acquire(config_dir)?;
    let _ = recover_interrupted_publish(config_dir)?;
    cleanup_publish_residue(config_dir);
    let _ = args.json;

    parse_github_repo(&args.repo)?;
    let commit = resolve_commit_with_git(&args.repo, &args.reference)?;
    let destination = agency_templates_dir(config_dir);
    if let Ok(current) = current_manifest_if_valid(config_dir) {
        if current.repo == args.repo
            && current.reference == args.reference
            && current.commit == commit
        {
            return Ok(UpdateResult {
                repo: current.repo,
                reference: current.reference,
                commit: current.commit,
                template_count: current.template_count,
                path: destination.to_string_lossy().to_string(),
                updated: false,
            });
        }
    }
    let extracted = fetch_repo_with_git(&args.repo, &commit, config_dir)?;
    let extracted = TempCacheDir::new(extracted);
    let staging = config_dir.join(format!(
        "{}.next-{}",
        AGENCY_TEMPLATES_DIR,
        uuid::Uuid::new_v4()
    ));
    if staging.exists() {
        return Err(format!(
            "Staging path already exists: {}",
            staging.display()
        ));
    }
    let staging = TempCacheDir::new(staging);
    normalize_extracted_repo_to_cache(
        extracted.path(),
        staging.path(),
        AgencyTemplatesManifest {
            repo: args.repo,
            reference: args.reference,
            commit,
            template_count: 0,
        },
    )?;
    let templates = collect_agency_templates_from_dir(staging.path())
        .map_err(|e| format!("Staged Agency cache failed validation: {}", e))?;
    let mut manifest: AgencyTemplatesManifest = serde_json::from_str(
        &fs::read_to_string(staging.path().join(AGENCY_MANIFEST_FILE))
            .map_err(|e| format!("Failed to read staged manifest: {}", e))?,
    )
    .map_err(|e| format!("Failed to parse staged manifest: {}", e))?;
    manifest.template_count = templates.len();
    fs::write(
        staging.path().join(AGENCY_MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("Failed to encode manifest: {}", e))?,
    )
    .map_err(|e| format!("Failed to write staged manifest: {}", e))?;
    collect_agency_templates_from_dir(staging.path())
        .map_err(|e| format!("Staged Agency cache failed validation: {}", e))?;

    publish_staging(config_dir, staging.path())?;
    cleanup_publish_residue(config_dir);
    Ok(UpdateResult {
        repo: manifest.repo,
        reference: manifest.reference,
        commit: manifest.commit,
        template_count: manifest.template_count,
        path: destination.to_string_lossy().to_string(),
        updated: true,
    })
}

pub fn update_cache(args: AgencyTemplatesUpdateArgs) -> Result<UpdateResult, String> {
    let config_dir = config_dir_or_err()?;
    update_cache_at(&config_dir, args)
}

fn update(args: AgencyTemplatesUpdateArgs) -> Result<(), String> {
    let result = update_cache(args)?;
    print_json(&result, false)
}

fn current_manifest_if_valid(config_dir: &Path) -> Result<AgencyTemplatesManifest, String> {
    let root = agency_templates_dir(config_dir);
    let templates = collect_agency_templates_from_dir(&root)?;
    let manifest: AgencyTemplatesManifest = serde_json::from_str(
        &fs::read_to_string(root.join(AGENCY_MANIFEST_FILE))
            .map_err(|e| format!("Failed to read cached manifest: {}", e))?,
    )
    .map_err(|e| format!("Failed to parse cached manifest: {}", e))?;
    if templates.len() != manifest.template_count {
        return Err("Cached manifest template count mismatch".into());
    }
    Ok(manifest)
}

fn parse_github_repo(input: &str) -> Result<(), String> {
    let Some(rest) = input.strip_prefix("https://github.com/") else {
        return Err(
            "Only https://github.com/<owner>/<repo> Agency repositories are supported".into(),
        );
    };
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if parts.len() != 2 || parts.iter().any(|p| p.trim().is_empty()) {
        return Err("Agency repository must be https://github.com/<owner>/<repo>".into());
    }
    Ok(())
}

fn run_git_with_timeout(
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    run_process_with_timeout("git", current_dir, args, timeout)
        .map_err(|e| format!("Failed to run git {:?}: {}", args, e))
}

fn run_process_with_timeout(
    program: &str,
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    let mut cmd = Command::new(program);
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn failed for {} {:?}: {}", program, args, e))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;

    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.read_to_end(&mut bytes);
        (result, bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr.read_to_end(&mut bytes);
        (result, bytes)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("wait failed for {} {:?}: {}", program, args, e))?
        {
            break status;
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_reader(stdout_reader, "stdout")?;
            let stderr = join_reader(stderr_reader, "stderr")?;
            let stderr_text = String::from_utf8_lossy(&stderr);
            return Err(format!(
                "{} {:?} timed out after {} seconds; killed and reaped. stderr: {} stdout_bytes={}",
                program,
                args,
                timeout.as_secs(),
                stderr_text.trim(),
                stdout.len()
            ));
        }

        std::thread::sleep(Duration::from_millis(50));
    };

    let stdout = join_reader(stdout_reader, "stdout")?;
    let stderr = join_reader(stderr_reader, "stderr")?;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn join_reader(
    reader: std::thread::JoinHandle<(std::io::Result<usize>, Vec<u8>)>,
    stream_name: &str,
) -> Result<Vec<u8>, String> {
    let (result, bytes) = reader
        .join()
        .map_err(|_| format!("{stream_name} reader thread panicked"))?;
    result.map_err(|e| format!("{stream_name} read failed: {e}"))?;
    Ok(bytes)
}

fn resolve_commit_with_git(repo: &str, reference: &str) -> Result<String, String> {
    #[cfg(test)]
    if let Ok(commit) = std::env::var("AGENTSCOMMANDER_AGENCY_TEST_COMMIT") {
        let _ = (repo, reference);
        return Ok(commit);
    }

    let output = run_git_with_timeout(None, &["ls-remote", repo, reference], GIT_LS_REMOTE_TIMEOUT)
        .map_err(|e| {
            format!(
                "Failed to run git. Install git or use an environment where git is on PATH: {}",
                e
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "Failed to resolve Agency repository ref with git ls-remote: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sha = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .find(|candidate| candidate.len() == 40 && candidate.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| "git ls-remote did not return a 40-character commit sha".to_string())?;
    if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("git ls-remote returned an invalid sha".into());
    }
    Ok(sha.to_string())
}

fn fetch_repo_with_git(repo: &str, commit: &str, config_dir: &Path) -> Result<PathBuf, String> {
    let temp = config_dir.join(format!(
        "{}.download-{}",
        AGENCY_TEMPLATES_DIR,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&temp).map_err(|e| {
        format!(
            "Failed to create Agency download dir {}: {}",
            temp.display(),
            e
        )
    })?;
    let run = |args: &[&str], timeout: Duration| -> Result<(), String> {
        let output = run_git_with_timeout(Some(&temp), args, timeout)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    };
    run(&["init", "--quiet"], GIT_CHECKOUT_TIMEOUT)?;
    run(&["remote", "add", "origin", repo], GIT_CHECKOUT_TIMEOUT)?;
    run(
        &["fetch", "--depth", "1", "origin", commit],
        GIT_FETCH_TIMEOUT,
    )?;
    run(
        &["checkout", "--quiet", "--detach", commit],
        GIT_CHECKOUT_TIMEOUT,
    )?;
    Ok(temp)
}

pub(crate) fn normalize_extracted_repo_to_cache(
    extracted_root: &Path,
    staging: &Path,
    mut manifest: AgencyTemplatesManifest,
) -> Result<(), String> {
    if staging.exists() {
        fs::remove_dir_all(staging)
            .map_err(|e| format!("Failed to remove existing staging dir: {}", e))?;
    }
    fs::create_dir_all(staging)
        .map_err(|e| format!("Failed to create staging dir {}: {}", staging.display(), e))?;
    let mut ids = std::collections::HashSet::new();
    let mut paths_ci = std::collections::HashSet::new();
    for division in fs::read_dir(extracted_root)
        .map_err(|e| format!("Failed to read extracted Agency repo: {}", e))?
    {
        let division = division.map_err(|e| format!("Failed to read Agency division: {}", e))?;
        let division_name = division.file_name().to_string_lossy().to_string();
        if should_skip_upstream_division(&division_name) {
            continue;
        }
        let meta = lstat_no_links(&division.path(), "Agency division")?;
        if !meta.is_dir() {
            if meta.is_file() {
                continue;
            }
            return Err(format!(
                "Agency division {} is not a regular directory",
                division_name
            ));
        }
        let division_slug = slug_segment(&division_name)?;
        normalize_agency_role_tree(
            &division.path(),
            &division.path(),
            &division_slug,
            staging,
            &mut ids,
            &mut paths_ci,
        )?;
    }
    manifest.template_count = ids.len();
    if manifest.template_count == 0 {
        return Err("Agency repository did not contain any role templates".into());
    }
    fs::write(
        staging.join(AGENCY_MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("Failed to encode Agency manifest: {}", e))?,
    )
    .map_err(|e| format!("Failed to write Agency manifest: {}", e))?;
    collect_agency_templates_from_dir(staging)
        .map_err(|e| format!("Normalized Agency cache failed validation: {}", e))?;
    Ok(())
}

fn normalize_agency_role_tree(
    division_root: &Path,
    current_dir: &Path,
    division_slug: &str,
    staging: &Path,
    ids: &mut std::collections::HashSet<String>,
    paths_ci: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    for entry in fs::read_dir(current_dir).map_err(|e| {
        format!(
            "Failed to read Agency role directory {}: {}",
            current_dir.display(),
            e
        )
    })? {
        let entry = entry.map_err(|e| format!("Failed to read Agency role entry: {}", e))?;
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();
        if matches!(file_name.as_str(), "CLAUDE.md" | "AGENTS.md") {
            return Err(format!(
                "Agency role tree {} contains managed prompt file {}",
                division_root.display(),
                file_name
            ));
        }
        let meta = lstat_no_links(&path, "Agency role entry")?;
        if meta.is_dir() {
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                return Err(format!(
                    "Agency role entry {} is not a regular file",
                    path.display()
                ));
            }
            normalize_agency_role_tree(
                division_root,
                &path,
                division_slug,
                staging,
                ids,
                paths_ci,
            )?;
            continue;
        }
        if !meta.is_file() {
            return Err(format!(
                "Agency role entry {} is not a regular file",
                path.display()
            ));
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        normalize_agency_role_file(&path, division_root, division_slug, staging, ids, paths_ci)?;
    }
    Ok(())
}

fn normalize_agency_role_file(
    path: &Path,
    division_root: &Path,
    division_slug: &str,
    staging: &Path,
    ids: &mut std::collections::HashSet<String>,
    paths_ci: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    let relative = path
        .strip_prefix(division_root)
        .map_err(|e| format!("Failed to relativize Agency role file: {}", e))?;
    let mut slug_parts = Vec::new();
    for component in relative.components() {
        let value = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| format!("Agency role file has invalid name: {}", path.display()))?;
        let segment = if relative.file_name() == Some(component.as_os_str()) {
            Path::new(value)
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("Agency role file has invalid name: {}", path.display()))?
        } else {
            value
        };
        slug_parts.push(slug_segment(segment)?);
    }
    if slug_parts.is_empty() {
        return Err(format!(
            "Agency role file has invalid name: {}",
            path.display()
        ));
    }
    let role_slug = slug_parts.join("-");
    let id = format!("agency:{}-{}", division_slug, role_slug);
    if !ids.insert(id.clone()) {
        return Err(format!(
            "Duplicate Agency template id after slugging: {}",
            id
        ));
    }
    let path_key = format!("{}\\{}", division_slug, role_slug).to_ascii_lowercase();
    if !paths_ci.insert(path_key) {
        return Err(format!(
            "Duplicate Agency template output path after slugging: {}/{}",
            division_slug, role_slug
        ));
    }
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read Agency role file {}: {}", path.display(), e))?;
    let fm = parse_agency_template_frontmatter(&raw);
    let stripped = strip_yaml_frontmatter_for_role_template(&raw);
    let body = sanitize_agency_role_body(stripped.trim())
        .trim()
        .to_string();
    validate_role_template_body(&id, &body)?;
    let name = fm.name.unwrap_or_else(|| title_case_slug(&role_slug));
    let mut normalized = String::new();
    normalized.push_str("---\n");
    normalized.push_str(&format!("name: {}\n", yaml_scalar(&name)));
    if let Some(description) = fm.description {
        normalized.push_str(&format!("description: {}\n", yaml_scalar(&description)));
    }
    if let Some(color) = fm.color {
        normalized.push_str(&format!("color: {}\n", yaml_scalar(&color)));
    }
    normalized.push_str("---\n\n");
    normalized.push_str(&body);
    normalized.push('\n');
    let role_path = staging.join(division_slug).join(&role_slug).join("Role.md");
    if let Some(parent) = role_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create normalized role dir: {}", e))?;
    }
    fs::write(&role_path, normalized)
        .map_err(|e| format!("Failed to write normalized Role.md: {}", e))?;
    Ok(())
}

fn sanitize_agency_role_body(body: &str) -> String {
    body.chars()
        .filter(|ch| !(*ch == '\0' || (ch.is_control() && !matches!(*ch, '\t' | '\n' | '\r'))))
        .collect()
}

fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn lstat_no_links(path: &Path, label: &str) -> Result<std::fs::Metadata, String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to stat {} {}: {}", label, path.display(), e))?;
    if is_link_or_reparse(&meta) {
        return Err(format!(
            "{} {} is a symlink or reparse point",
            label,
            path.display()
        ));
    }
    Ok(meta)
}

fn should_skip_upstream_division(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".github" | "scripts" | "integrations" | "examples" | "docs" | "node_modules"
    )
}

fn yaml_scalar(value: &str) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|_| format!("{:?}", value))
        .trim()
        .trim_start_matches("---")
        .trim()
        .trim_end_matches("...")
        .trim()
        .to_string()
}

fn publish_staging(config_dir: &Path, staging: &Path) -> Result<(), String> {
    let live = agency_templates_dir(config_dir);
    let previous = config_dir.join(format!(
        "{}.prev-{}",
        AGENCY_TEMPLATES_DIR,
        uuid::Uuid::new_v4()
    ));
    let had_live = live.exists();
    if had_live {
        fs::rename(&live, &previous).map_err(|e| {
            format!(
                "Failed to move live Agency cache {} to {}: {}",
                live.display(),
                previous.display(),
                e
            )
        })?;
    }
    if let Err(publish_err) = fs::rename(staging, &live) {
        if had_live {
            if let Err(rollback_err) = fs::rename(&previous, &live) {
                return Err(format!(
                    "Failed to publish Agency cache: {}; rollback also failed: {}",
                    publish_err, rollback_err
                ));
            }
        }
        return Err(format!("Failed to publish Agency cache: {}", publish_err));
    }
    if had_live {
        let _ = fs::remove_dir_all(previous);
    }
    Ok(())
}

fn recover_interrupted_publish(config_dir: &Path) -> Result<String, String> {
    let live = agency_templates_dir(config_dir);
    if live.exists() {
        cleanup_publish_residue(config_dir);
        return Ok("available".into());
    }
    let mut valid_prev = Vec::new();
    for entry in
        fs::read_dir(config_dir).map_err(|e| format!("Failed to read config dir: {}", e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read config dir entry: {}", e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&format!("{}.prev-", AGENCY_TEMPLATES_DIR))
            && collect_agency_templates_from_dir(&entry.path()).is_ok()
        {
            valid_prev.push(entry.path());
        }
    }
    match valid_prev.len() {
        0 => Ok("missing".into()),
        1 => {
            fs::rename(&valid_prev[0], &live).map_err(|e| {
                format!(
                    "cacheInvalid: failed to restore Agency cache {} to {}: {}",
                    valid_prev[0].display(),
                    live.display(),
                    e
                )
            })?;
            Ok("recovered".into())
        }
        _ => Err("cacheInvalid: multiple valid previous Agency caches found".into()),
    }
}

fn cleanup_publish_residue(config_dir: &Path) {
    if let Ok(entries) = fs::read_dir(config_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{}.next-", AGENCY_TEMPLATES_DIR))
                || name.starts_with(&format!("{}.download-", AGENCY_TEMPLATES_DIR))
                || (agency_templates_dir(config_dir).exists()
                    && name.starts_with(&format!("{}.prev-", AGENCY_TEMPLATES_DIR)))
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(template_count: usize) -> AgencyTemplatesManifest {
        AgencyTemplatesManifest {
            repo: "https://github.com/example/agency-agents".into(),
            reference: "main".into(),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            template_count,
        }
    }

    fn write_valid_cache(root: &Path) {
        let role_dir = root.join("engineering").join("planner");
        fs::create_dir_all(&role_dir).expect("create role dir");
        fs::write(
            role_dir.join("Role.md"),
            "---\nname: Planner\n---\n\nPlan backend work.\n",
        )
        .expect("write role");
        fs::write(
            root.join(AGENCY_MANIFEST_FILE),
            serde_json::to_string_pretty(&manifest(1)).expect("manifest json"),
        )
        .expect("write manifest");
    }

    #[test]
    fn bounded_process_timeout_kills_and_reaps_child() {
        #[cfg(windows)]
        let (program, args) = (
            "powershell.exe",
            vec!["-NoProfile", "-Command", "Start-Sleep -Seconds 5"],
        );
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c", "sleep 5"]);

        let err =
            run_process_with_timeout(program, None, &args, Duration::from_millis(100)).unwrap_err();

        assert!(err.contains("timed out"), "unexpected error: {err}");
        assert!(err.contains("killed and reaped"), "unexpected error: {err}");
    }

    #[test]
    fn normalize_rejects_directory_named_md_role_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let extracted = tmp.path().join("extracted");
        let division = extracted.join("Engineering");
        let staging = tmp.path().join("staging");
        fs::create_dir_all(division.join("Planner.md")).expect("create directory named md");

        let err = normalize_extracted_repo_to_cache(&extracted, &staging, manifest(0))
            .expect_err("directory named md must be rejected");

        assert!(err.contains("not a regular file"));
    }

    #[test]
    fn normalize_skips_root_metadata_files_and_loads_valid_division() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let extracted = tmp.path().join("extracted");
        let division = extracted.join("Engineering");
        let staging = tmp.path().join("staging");
        fs::create_dir_all(&division).expect("create division");
        for name in [
            ".gitattributes",
            ".gitignore",
            "CONTRIBUTING.md",
            "LICENSE",
            "README.md",
            "SECURITY.md",
        ] {
            fs::write(extracted.join(name), "repo metadata\n").expect("write metadata file");
        }
        fs::write(
            division.join("Planner.md"),
            "---\nname: Planner\n---\n\nPlan backend work.\n",
        )
        .expect("write role");

        normalize_extracted_repo_to_cache(&extracted, &staging, manifest(0))
            .expect("normalize repo");

        let templates = collect_agency_templates_from_dir(&staging).expect("collect templates");
        assert_eq!(templates.len(), 1);
        assert!(staging
            .join("engineering")
            .join("planner")
            .join("Role.md")
            .is_file());
    }

    #[test]
    fn normalize_and_publish_handles_upstream_worktree_with_nested_roles() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path().join("config");
        let extracted = tmp.path().join("extracted");
        let direct = extracted.join("Game Development");
        let nested = direct.join("Unity");
        let staging = tmp.path().join("staging");
        fs::create_dir_all(&config_dir).expect("create config dir");
        fs::create_dir_all(&nested).expect("create nested division");
        for name in [
            ".gitattributes",
            ".gitignore",
            "CONTRIBUTING.md",
            "LICENSE",
            "README.md",
            "SECURITY.md",
        ] {
            fs::write(extracted.join(name), "repo metadata\n").expect("write metadata file");
        }
        fs::write(
            direct.join("game-designer.md"),
            "---\nname: Game Designer\n---\n\nDesign game systems.\n",
        )
        .expect("write direct role");
        fs::write(
            nested.join("unity-architect.md"),
            "---\nname: Unity Architect\n---\n\nDesign\u{4} Unity architecture.\n",
        )
        .expect("write nested role");

        normalize_extracted_repo_to_cache(&extracted, &staging, manifest(0))
            .expect("normalize repo");
        publish_staging(&config_dir, &staging).expect("publish cache");

        let live = agency_templates_dir(&config_dir);
        let manifest: AgencyTemplatesManifest = serde_json::from_str(
            &fs::read_to_string(live.join(AGENCY_MANIFEST_FILE)).expect("read manifest"),
        )
        .expect("parse manifest");
        assert_eq!(manifest.template_count, 2);
        assert!(live
            .join("game-development")
            .join("game-designer")
            .join("Role.md")
            .is_file());
        assert!(live
            .join("game-development")
            .join("unity-unity-architect")
            .join("Role.md")
            .is_file());
        let nested_body = fs::read_to_string(
            live.join("game-development")
                .join("unity-unity-architect")
                .join("Role.md"),
        )
        .expect("read nested role");
        assert!(!nested_body.contains('\u{4}'));
    }

    #[test]
    fn normalize_rejects_symlinked_upstream_md_role_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let extracted = tmp.path().join("extracted");
        let division = extracted.join("Engineering");
        let staging = tmp.path().join("staging");
        fs::create_dir_all(&division).expect("create division");
        let outside = tmp.path().join("outside.md");
        fs::write(&outside, "External role body.\n").expect("write outside target");
        let link = division.join("Planner.md");
        if !try_symlink_file(&outside, &link) {
            return;
        }

        let err = normalize_extracted_repo_to_cache(&extracted, &staging, manifest(0))
            .expect_err("symlinked md must be rejected");

        assert!(err.contains("symlink") || err.contains("reparse"));
    }

    #[test]
    fn legacy_stale_lock_with_missing_live_cache_recovers_one_valid_previous_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path();
        let prev = config_dir.join(format!("{}.prev-one", AGENCY_TEMPLATES_DIR));
        write_valid_cache(&prev);
        let lock_path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
        fs::write(&lock_path, "").expect("write legacy stale lock");

        let lock = CacheLock::acquire(config_dir).expect("recover stale lock");
        assert!(lock_path.exists(), "new lock should be held after recovery");
        assert_eq!(
            recover_interrupted_publish(config_dir).expect("recover previous cache"),
            "recovered"
        );
        drop(lock);

        assert!(agency_templates_dir(config_dir).is_dir());
        assert!(!lock_path.exists(), "lock should be removed on drop");
    }

    #[test]
    fn dead_owner_lock_is_replaced() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path();
        let lock_path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
        let dead_pid = 999_999;
        if process_is_running(dead_pid) {
            return;
        }
        fs::write(
            &lock_path,
            format!(r#"{{"pid":{},"createdUnixSecs":1}}"#, dead_pid),
        )
        .expect("write dead-owner lock");

        let lock = CacheLock::acquire(config_dir).expect("replace dead-owner lock");
        assert!(lock_path.exists(), "new lock should be held");
        drop(lock);

        assert!(!lock_path.exists(), "lock should be removed on drop");
    }

    #[test]
    fn status_cache_at_recovers_valid_previous_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path();
        let prev = config_dir.join(format!("{}.prev-one", AGENCY_TEMPLATES_DIR));
        write_valid_cache(&prev);

        let status = status_cache_at(config_dir).expect("status recovers previous cache");

        assert!(status.available);
        assert!(agency_templates_dir(config_dir).is_dir());
        assert!(!prev.exists(), "previous cache should be promoted");
        assert_eq!(status.template_count, Some(1));
    }

    #[test]
    fn status_cache_at_reports_live_lock_as_locked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path();
        let lock_path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
        fs::write(
            &lock_path,
            format!(r#"{{"pid":{},"createdUnixSecs":1}}"#, process::id()),
        )
        .expect("write live lock");

        let status = status_cache_at(config_dir).expect("locked status should be non-fatal");

        assert!(!status.available);
        assert_eq!(status.reason.as_deref(), Some("locked"));
        fs::remove_file(lock_path).expect("cleanup live lock");
    }

    #[test]
    fn update_cache_at_reports_already_current_without_printed_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path();
        let live = agency_templates_dir(config_dir);
        write_valid_cache(&live);
        let current = current_manifest_if_valid(config_dir).expect("valid manifest");
        std::env::set_var("AGENTSCOMMANDER_AGENCY_TEST_COMMIT", &current.commit);

        let result = update_cache_at(
            config_dir,
            AgencyTemplatesUpdateArgs {
                repo: current.repo.clone(),
                reference: current.reference.clone(),
                json: true,
            },
        )
        .expect("already-current update should succeed");

        std::env::remove_var("AGENTSCOMMANDER_AGENCY_TEST_COMMIT");
        assert!(!result.updated);
        assert_eq!(result.repo, current.repo);
        assert_eq!(result.reference, current.reference);
        assert_eq!(result.commit, current.commit);
        assert_eq!(result.template_count, current.template_count);
        assert_eq!(result.path, live.to_string_lossy().to_string());
    }

    #[cfg(unix)]
    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    #[cfg(not(any(unix, windows)))]
    fn try_symlink_file(_target: &Path, _link: &Path) -> bool {
        false
    }
}
