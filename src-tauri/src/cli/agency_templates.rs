use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::commands::role_templates::{
    agency_manifest_path, agency_template_metas_from_cache, agency_templates_dir,
    agency_templates_status, collect_agency_templates_from_dir, parse_agency_template_frontmatter,
    slug_segment, strip_yaml_frontmatter_for_role_template, title_case_slug,
    validate_role_template_body, AgencyTemplatesManifest, AGENCY_MANIFEST_FILE,
    AGENCY_TEMPLATES_DIR,
};

const DEFAULT_REPO: &str = "https://github.com/msitarzewski/agency-agents";

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
    #[arg(long = "ref", default_value = "main")]
    pub reference: String,
}

#[derive(Args)]
pub struct AgencyTemplatesListArgs {
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct AgencyTemplatesStatusArgs {
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateResult {
    repo: String,
    #[serde(rename = "ref")]
    reference: String,
    commit: String,
    template_count: usize,
    path: String,
    updated: bool,
}

struct CacheLock {
    path: PathBuf,
    _file: File,
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
    let Ok(output) = Command::new("tasklist")
        .args(["/FI", &filter, "/NH", "/FO", "CSV"])
        .output()
    else {
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

fn status(args: AgencyTemplatesStatusArgs) -> Result<(), String> {
    let config_dir = config_dir_or_err()?;
    match CacheLock::acquire(&config_dir) {
        Ok(_lock) => {
            if let Err(e) = recover_interrupted_publish(&config_dir) {
                if e.starts_with("cacheInvalid") {
                    let value = serde_json::json!({
                        "available": false,
                        "path": agency_templates_dir(&config_dir).to_string_lossy(),
                        "reason": "cacheInvalid",
                    });
                    return print_json(&value, args.pretty);
                }
                return Err(e);
            }
        }
        Err(e) if e.contains("already running") => {
            let value = serde_json::json!({
                "available": false,
                "path": agency_templates_dir(&config_dir).to_string_lossy(),
                "reason": "locked",
            });
            return print_json(&value, args.pretty);
        }
        Err(e) => return Err(e),
    }
    print_json(&agency_templates_status(&config_dir), args.pretty)
}

fn list(args: AgencyTemplatesListArgs) -> Result<(), String> {
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

fn update(args: AgencyTemplatesUpdateArgs) -> Result<(), String> {
    let config_dir = config_dir_or_err()?;
    let _lock = CacheLock::acquire(&config_dir)?;
    let _ = recover_interrupted_publish(&config_dir)?;
    cleanup_publish_residue(&config_dir);

    parse_github_repo(&args.repo)?;
    let commit = resolve_commit_with_git(&args.repo, &args.reference)?;
    let extracted = fetch_repo_with_git(&args.repo, &commit, &config_dir)?;
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
    normalize_extracted_repo_to_cache(
        &extracted,
        &staging,
        AgencyTemplatesManifest {
            repo: args.repo,
            reference: args.reference,
            commit,
            template_count: 0,
        },
    )?;
    let templates = collect_agency_templates_from_dir(&staging)
        .map_err(|e| format!("Staged Agency cache failed validation: {}", e))?;
    let mut manifest: AgencyTemplatesManifest = serde_json::from_str(
        &fs::read_to_string(staging.join(AGENCY_MANIFEST_FILE))
            .map_err(|e| format!("Failed to read staged manifest: {}", e))?,
    )
    .map_err(|e| format!("Failed to parse staged manifest: {}", e))?;
    manifest.template_count = templates.len();
    fs::write(
        staging.join(AGENCY_MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("Failed to encode manifest: {}", e))?,
    )
    .map_err(|e| format!("Failed to write staged manifest: {}", e))?;
    collect_agency_templates_from_dir(&staging)
        .map_err(|e| format!("Staged Agency cache failed validation: {}", e))?;

    publish_staging(&config_dir, &staging)?;
    cleanup_publish_residue(&config_dir);
    print_json(
        &UpdateResult {
            repo: manifest.repo,
            reference: manifest.reference,
            commit: manifest.commit,
            template_count: manifest.template_count,
            path: agency_templates_dir(&config_dir)
                .to_string_lossy()
                .to_string(),
            updated: true,
        },
        false,
    )
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

fn resolve_commit_with_git(repo: &str, reference: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(["ls-remote", repo, reference])
        .output()
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
    let run = |args: &[&str]| -> Result<(), String> {
        let output = Command::new("git")
            .current_dir(&temp)
            .args(args)
            .output()
            .map_err(|e| format!("Failed to run git {:?}: {}", args, e))?;
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
    run(&["init", "--quiet"])?;
    run(&["remote", "add", "origin", repo])?;
    run(&["fetch", "--depth", "1", "origin", commit])?;
    run(&["checkout", "--quiet", "--detach", commit])?;
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
        for file in fs::read_dir(division.path())
            .map_err(|e| format!("Failed to read Agency division {}: {}", division_name, e))?
        {
            let file = file.map_err(|e| format!("Failed to read Agency role file: {}", e))?;
            let path = file.path();
            let file_name = file.file_name().to_string_lossy().to_string();
            if matches!(file_name.as_str(), "CLAUDE.md" | "AGENTS.md" | "GEMINI.md") {
                return Err(format!(
                    "Agency division {} contains managed prompt file {}",
                    division_name, file_name
                ));
            }
            let meta = lstat_no_links(&path, "Agency role file")?;
            if !meta.is_file() {
                return Err(format!(
                    "Agency role entry {} is not a regular file",
                    path.display()
                ));
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("Agency role file has invalid name: {}", path.display()))?;
            let stem_slug = slug_segment(stem)?;
            let id = format!("agency:{}-{}", division_slug, stem_slug);
            if !ids.insert(id.clone()) {
                return Err(format!(
                    "Duplicate Agency template id after slugging: {}",
                    id
                ));
            }
            let path_key = format!("{}\\{}", division_slug, stem_slug).to_ascii_lowercase();
            if !paths_ci.insert(path_key) {
                return Err(format!(
                    "Duplicate Agency template output path after slugging: {}/{}",
                    division_slug, stem_slug
                ));
            }
            let raw = fs::read_to_string(&path).map_err(|e| {
                format!("Failed to read Agency role file {}: {}", path.display(), e)
            })?;
            let fm = parse_agency_template_frontmatter(&raw);
            let body = strip_yaml_frontmatter_for_role_template(&raw)
                .trim()
                .to_string();
            validate_role_template_body(&id, &body)?;
            let name = fm.name.unwrap_or_else(|| title_case_slug(&stem_slug));
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
            let role_path = staging
                .join(&division_slug)
                .join(&stem_slug)
                .join("Role.md");
            if let Some(parent) = role_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create normalized role dir: {}", e))?;
            }
            fs::write(&role_path, normalized)
                .map_err(|e| format!("Failed to write normalized Role.md: {}", e))?;
        }
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

#[allow(dead_code)]
fn _manifest_path_for_docs(config_dir: &Path) -> PathBuf {
    agency_manifest_path(config_dir)
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
