use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

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

impl CacheLock {
    fn acquire(config_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(config_dir)
            .map_err(|e| format!("Failed to create config dir {}: {}", config_dir.display(), e))?;
        let path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => Ok(Self { path, _file: file }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(format!(
                "Agency template cache lock exists at {}; another agency-templates update is already running or recovery is required",
                path.display()
            )),
            Err(e) => Err(format!(
                "Failed to create Agency template cache lock {}: {}",
                path.display(),
                e
            )),
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
        return Err(format!("Staging path already exists: {}", staging.display()));
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
    let mut manifest: AgencyTemplatesManifest =
        serde_json::from_str(&fs::read_to_string(staging.join(AGENCY_MANIFEST_FILE)).map_err(
            |e| format!("Failed to read staged manifest: {}", e),
        )?)
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
            path: agency_templates_dir(&config_dir).to_string_lossy().to_string(),
            updated: true,
        },
        false,
    )
}

fn parse_github_repo(input: &str) -> Result<(), String> {
    let Some(rest) = input.strip_prefix("https://github.com/") else {
        return Err("Only https://github.com/<owner>/<repo> Agency repositories are supported".into());
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

fn fetch_repo_with_git(
    repo: &str,
    commit: &str,
    config_dir: &Path,
) -> Result<PathBuf, String> {
    let temp = config_dir.join(format!(
        "{}.download-{}",
        AGENCY_TEMPLATES_DIR,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&temp)
        .map_err(|e| format!("Failed to create Agency download dir {}: {}", temp.display(), e))?;
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
        let meta = division
            .metadata()
            .map_err(|e| format!("Failed to stat Agency division: {}", e))?;
        if !meta.is_dir() {
            continue;
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
            let meta = file
                .metadata()
                .map_err(|e| format!("Failed to stat Agency role file: {}", e))?;
            if !meta.is_file()
                || path.extension().and_then(|e| e.to_str()) != Some("md")
            {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("Agency role file has invalid name: {}", path.display()))?;
            let stem_slug = slug_segment(stem)?;
            let id = format!("agency:{}-{}", division_slug, stem_slug);
            if !ids.insert(id.clone()) {
                return Err(format!("Duplicate Agency template id after slugging: {}", id));
            }
            let path_key = format!("{}\\{}", division_slug, stem_slug).to_ascii_lowercase();
            if !paths_ci.insert(path_key) {
                return Err(format!(
                    "Duplicate Agency template output path after slugging: {}/{}",
                    division_slug, stem_slug
                ));
            }
            let raw = fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read Agency role file {}: {}", path.display(), e))?;
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
            let role_path = staging.join(&division_slug).join(&stem_slug).join("Role.md");
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
    for entry in fs::read_dir(config_dir).map_err(|e| format!("Failed to read config dir: {}", e))? {
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
