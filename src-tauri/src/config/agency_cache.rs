#[cfg(test)]
use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Args;
use serde::Serialize;

use crate::filesystem::agency_templates::{
    agency_entries, agency_metadata, agency_path_exists, create_agency_lock,
    ensure_agency_directory, read_agency_text, remove_agency_file, remove_agency_tree,
    rename_agency_path, write_agency_lock, write_agency_text,
};

use crate::config::role_templates::{
    agency_template_metas_from_cache, agency_templates_dir, agency_templates_status,
    collect_agency_templates_from_dir, parse_agency_template_frontmatter,
    read_cached_agency_manifest, read_staged_agency_manifest, slug_segment,
    strip_yaml_frontmatter_for_role_template, title_case_slug, validate_role_template_body,
    write_normalized_agency_manifest, write_staged_agency_manifest, AgencyTemplatesManifest,
    AGENCY_TEMPLATES_DIR,
};

use crate::process::is_running as process_is_running;

mod git;
#[cfg(test)]
mod tests;
#[cfg(test)]
use git::run_process_with_timeout;
use git::{fetch_repo_with_git, resolve_commit_with_git};

pub const DEFAULT_REPO: &str = "https://github.com/msitarzewski/agency-agents";
pub const DEFAULT_REFERENCE: &str = "main";

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
        let _ = remove_agency_tree(&self.path);
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
        ensure_agency_directory(config_dir).map_err(|e| {
            format!(
                "Failed to create config dir {}: {}",
                config_dir.display(),
                e
            )
        })?;
        let path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
        loop {
            match create_agency_lock(&path) {
                Ok(mut file) => {
                    let created_unix_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let info = CacheLockInfo {
                        pid: process::id(),
                        created_unix_secs,
                    };
                    write_agency_lock(
                        &mut file,
                        &serde_json::to_string(&info)
                            .map_err(|e| format!("Failed to encode Agency cache lock: {}", e))?,
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
                        "Agency template cache lock exists at {}; \
                         another agency-templates update is already running \
                         or recovery is required",
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
        if let Err(e) = remove_agency_file(&self.path) {
            eprintln!(
                "Warning: failed to remove Agency template cache lock {}: {}",
                self.path.display(),
                e
            );
        }
    }
}

fn recover_stale_lock_if_possible(config_dir: &Path, path: &Path) -> Result<bool, String> {
    let raw = read_agency_text(path).unwrap_or_default();
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
    remove_agency_file(path).map_err(|e| {
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
    if agency_path_exists(&live) {
        return false;
    }
    let Ok(entries) = agency_entries(config_dir) else {
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

pub(crate) fn config_dir_or_err() -> Result<PathBuf, String> {
    crate::config::config_dir()
        .ok_or_else(|| "Could not resolve AgentsCommander config dir".to_string())
}

pub fn status_cache_at(
    config_dir: &Path,
) -> Result<crate::config::role_templates::AgencyTemplatesStatus, String> {
    match CacheLock::acquire(config_dir) {
        Ok(_lock) => {
            if let Err(e) = recover_interrupted_publish(config_dir) {
                if e.starts_with("cacheInvalid") {
                    return Ok(crate::config::role_templates::AgencyTemplatesStatus {
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
            return Ok(crate::config::role_templates::AgencyTemplatesStatus {
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

pub fn status_cache() -> Result<crate::config::role_templates::AgencyTemplatesStatus, String> {
    let config_dir = config_dir_or_err()?;
    status_cache_at(&config_dir)
}

pub(crate) fn list_cache_at(
    config_dir: &Path,
) -> Result<Vec<crate::config::role_templates::RoleTemplateMeta>, String> {
    match CacheLock::acquire(config_dir) {
        Ok(_lock) => {
            let _ = recover_interrupted_publish(config_dir)?;
        }
        Err(e) if e.contains("already running") => {
            return Err(format!(
                "Agency template cache is locked; run agency-templates status \
                 and retry after the updater finishes ({})",
                e
            ));
        }
        Err(e) => return Err(e),
    }
    Ok(agency_template_metas_from_cache(config_dir))
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
    if agency_path_exists(&staging) {
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
    let mut manifest = read_staged_agency_manifest(staging.path())?;
    manifest.template_count = templates.len();
    write_staged_agency_manifest(staging.path(), &manifest)?;
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

fn current_manifest_if_valid(config_dir: &Path) -> Result<AgencyTemplatesManifest, String> {
    let root = agency_templates_dir(config_dir);
    let templates = collect_agency_templates_from_dir(&root)?;
    let manifest = read_cached_agency_manifest(&root)?;
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

pub(crate) fn normalize_extracted_repo_to_cache(
    extracted_root: &Path,
    staging: &Path,
    mut manifest: AgencyTemplatesManifest,
) -> Result<(), String> {
    if agency_path_exists(staging) {
        remove_agency_tree(staging)
            .map_err(|e| format!("Failed to remove existing staging dir: {}", e))?;
    }
    ensure_agency_directory(staging)
        .map_err(|e| format!("Failed to create staging dir {}: {}", staging.display(), e))?;
    let mut ids = std::collections::HashSet::new();
    let mut paths_ci = std::collections::HashSet::new();
    for division in agency_entries(extracted_root)
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
    write_normalized_agency_manifest(staging, &manifest)?;
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
    for entry in agency_entries(current_dir).map_err(|e| {
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
    let raw = read_agency_text(path)
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
        ensure_agency_directory(parent)
            .map_err(|e| format!("Failed to create normalized role dir: {}", e))?;
    }
    write_agency_text(&role_path, &normalized)
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
    let meta = agency_metadata(path)
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
    let had_live = agency_path_exists(&live);
    if had_live {
        rename_agency_path(&live, &previous).map_err(|e| {
            format!(
                "Failed to move live Agency cache {} to {}: {}",
                live.display(),
                previous.display(),
                e
            )
        })?;
    }
    if let Err(publish_err) = rename_agency_path(staging, &live) {
        if had_live {
            if let Err(rollback_err) = rename_agency_path(&previous, &live) {
                return Err(format!(
                    "Failed to publish Agency cache: {}; rollback also failed: {}",
                    publish_err, rollback_err
                ));
            }
        }
        return Err(format!("Failed to publish Agency cache: {}", publish_err));
    }
    if had_live {
        let _ = remove_agency_tree(&previous);
    }
    Ok(())
}

fn recover_interrupted_publish(config_dir: &Path) -> Result<String, String> {
    let live = agency_templates_dir(config_dir);
    if agency_path_exists(&live) {
        cleanup_publish_residue(config_dir);
        return Ok("available".into());
    }
    let mut valid_prev = Vec::new();
    for entry in
        agency_entries(config_dir).map_err(|e| format!("Failed to read config dir: {}", e))?
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
            rename_agency_path(&valid_prev[0], &live).map_err(|e| {
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
    if let Ok(entries) = agency_entries(config_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{}.next-", AGENCY_TEMPLATES_DIR))
                || name.starts_with(&format!("{}.download-", AGENCY_TEMPLATES_DIR))
                || (agency_path_exists(&agency_templates_dir(config_dir))
                    && name.starts_with(&format!("{}.prev-", AGENCY_TEMPLATES_DIR)))
            {
                let _ = remove_agency_tree(&entry.path());
            }
        }
    }
}
