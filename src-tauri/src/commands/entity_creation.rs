use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::cli::task_ops;
use crate::commands::ac_discovery::DiscoveryBranchWatcher;
use crate::config::replica_identity::{
    expected_wg_replica_identity, normalize_wg_replica_context_entries,
    repair_wg_replica_config_value, ROLE_MD_FILENAME, WG_REPLICA_REQUIRED_CONTEXT,
};
use crate::config::settings::{AppSettings, SettingsState};
use crate::config::workspace::existing_workspace_dir;
use crate::pty::git_watcher::{CoordinatorChangedPayload, GitWatcher};
use crate::session::manager::SessionManager;
use crate::session::session::{SessionRepo, SessionStatus};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedEntityResult {
    /// Absolute path to the created directory
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub path: String,
    pub project_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoAssignment {
    pub url: String,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamConfigResult {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub coordinator: String,
    #[serde(default)]
    pub repos: Vec<RepoAssignment>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkgroupCloneResult {
    /// Absolute path to the created workgroup directory
    pub path: String,
    /// Repos that failed to clone (url + error message). Empty = all succeeded.
    pub clone_errors: Vec<CloneError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneError {
    pub url: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncError {
    pub replica: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub workgroups_updated: u32,
    pub replicas_updated: u32,
    pub errors: Vec<SyncError>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkgroupDiskCreateArgs {
    pub project_path: PathBuf,
    pub team_name: String,
    pub task_title: String,
    // Legacy provisioning path. Ordinary callers should pass None and empty
    // vectors so workgroup creation activates an existing team config.
    pub coordinator: Option<String>,
    pub agents: Vec<String>,
    pub repos: Vec<RepoAssignment>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReplicaDiskCreateArgs {
    pub workspace_dir: PathBuf,
    pub wg_dir: PathBuf,
    pub agent_path: String,
    pub team_repos: Vec<RepoAssignment>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Agent Matrix / replica directory contract (issue #209)
// ---------------------------------------------------------------------------
//
// Agent Matrices own canonical state (`memory/`, `plans/`, `skills/`) plus
// mailboxes. Workgroup replicas own only mailboxes; their config points back
// to the origin matrix for canonical state.
pub(crate) const AGENT_MATRIX_DIRS: &[&str] = &["memory", "plans", "skills", "inbox", "outbox"];
pub(crate) const AGENT_REPLICA_DIRS: &[&str] = &["inbox", "outbox"];

fn create_agent_matrix_subdirs(agent_dir: &Path) -> Result<(), (&'static str, std::io::Error)> {
    for &sub in AGENT_MATRIX_DIRS {
        std::fs::create_dir_all(agent_dir.join(sub)).map_err(|e| (sub, e))?;
    }
    Ok(())
}

/// Create the full Agent Matrix layout, including the root directory.
///
/// The returned tag is `"agent_dir"` for root failures or the failing subdir
/// name for child directory failures, so callers can preserve diagnostics.
pub(crate) fn create_agent_matrix_layout(
    agent_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    std::fs::create_dir_all(agent_dir).map_err(|e| ("agent_dir", e))?;
    create_agent_matrix_subdirs(agent_dir)
}

/// Create a new Agent Matrix layout after atomically claiming the root.
///
/// Unlike `create_agent_matrix_layout`, this fails if the root already exists.
/// Use it for user-facing creation paths so concurrent creates cannot overwrite
/// each other's Role.md or config.json.
pub(crate) fn create_new_agent_matrix_layout(
    agent_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    std::fs::create_dir(agent_dir).map_err(|e| ("agent_dir", e))?;
    create_agent_matrix_subdirs(agent_dir)
}

/// Create the full workgroup replica layout, including the root directory.
///
/// Canonical state is intentionally absent here; replicas reference the origin
/// Agent Matrix through `config.json`.
pub(crate) fn create_agent_replica_layout(
    replica_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    std::fs::create_dir_all(replica_dir).map_err(|e| ("replica_dir", e))?;
    for &sub in AGENT_REPLICA_DIRS {
        std::fs::create_dir_all(replica_dir.join(sub)).map_err(|e| (sub, e))?;
    }
    Ok(())
}

/// Sanitize a user-provided name into a safe directory component:
/// lowercase, only a-z 0-9 and hyphens, no leading/trailing hyphens.
pub(crate) fn sanitize_name(raw: &str) -> Result<String, String> {
    let sanitized: String = raw
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if sanitized.is_empty() {
        return Err("Name must contain at least one alphanumeric character".into());
    }
    Ok(sanitized)
}

/// Validate that an existing entity name is safe for path operations.
/// Unlike `sanitize_name`, this does NOT transform the name — it just rejects
/// names that contain path traversal or separator characters.
///
/// `pub(crate)` so the sentinel-collision invariant test in
/// `wg_delete_diagnostic::tests` can prove that no valid WG name can collide
/// with the `BLOCKERS:` / `DIRTY_REPOS:` sentinel prefixes.
pub(crate) fn validate_existing_name(name: &str, entity_label: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{} name cannot be empty", entity_label));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "Invalid {} name: only alphanumeric characters and hyphens are allowed",
            entity_label
        ));
    }
    Ok(())
}

/// Extract a repo directory name from a git URL.
/// `https://github.com/org/my-repo.git` → `my-repo`
fn repo_dir_name_from_url(url: &str) -> String {
    let without_trailing = url.trim_end_matches('/');
    let last_segment = without_trailing.rsplit('/').next().unwrap_or("repo");
    last_segment
        .strip_suffix(".git")
        .unwrap_or(last_segment)
        .to_string()
}

/// Check if a team-config agent entry (absolute path or dir name) matches a given agent name.
/// `agent_name` is the bare name (e.g., "dev-rust"), not prefixed.
fn agent_matches(team_agent_entry: &str, agent_name: &str) -> bool {
    let entry_dir = Path::new(team_agent_entry)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(team_agent_entry);
    entry_dir == format!("_agent_{}", agent_name) || entry_dir == agent_name
}

/// Parse YAML frontmatter from a Role.md file.
/// Returns (name, description) if found. Strips a leading UTF-8 BOM first so a
/// `Role.md` saved as UTF-8-with-BOM still yields its display name/description
/// — mirrors `role_templates::parse_template_frontmatter`.
fn parse_role_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !content.starts_with("---") {
        return (None, None);
    }

    let rest = &content[3..];
    let end = match rest.find("---") {
        Some(i) => i,
        None => return (None, None),
    };

    let frontmatter = &rest[..end];
    let mut name = None;
    let mut description = None;

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("name:") {
            name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(val) = trimmed.strip_prefix("description:") {
            description = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }

    (name, description)
}

/// Extract a `title:` field from the YAML frontmatter at the start of `content`.
///
/// Best-effort frontmatter detection — NOT a YAML implementation. Suitable
/// only for the narrow case of one optional scalar field at the top of
/// TASK.md.
///
/// Returns `Some(title)` when:
///   - `content` starts with `---`,
///   - a closing `---` exists,
///   - a line of the form `<key>: <value>` exists between the delimiters
///     where `<key>` matches `title` case-insensitively (`title:`, `Title:`,
///     `TITLE:`, mixed casing all accepted).
///
/// The value half is preserved verbatim (case-sensitive), then stripped of
/// surrounding `"` or `'` quote pairs.
///
/// Returns `None` otherwise (no frontmatter, no title key, or empty value).
///
/// Mirrors `parse_role_frontmatter`'s shape — both speak the same on-disk
/// format. See plan `_plans/107-auto-brief-title.md` §6 for why we do not
/// pull in `serde_yaml`.
pub(crate) fn parse_task_title(content: &str) -> Option<String> {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("---")?;
    let frontmatter = &rest[..end];

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        // Case-insensitive key match on `title:`. Round 2 fold (F3 / G3):
        // agents stochastically capitalize keys (`Title:`, `TITLE:`); a
        // case-sensitive match would let duplicate `title:` lines accumulate
        // across restarts. Split on the first `:` so we compare just the key.
        let Some((key, value_raw)) = trimmed.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("title") {
            continue;
        }
        let value = value_raw
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        if value.is_empty() {
            return None;
        }
        return Some(value);
    }
    None
}

/// TASK.md content for a brand-new workgroup.
///
/// Workgroups now start with an explicit task title, so there is no follow-up
/// auto-title prompt. Store the title in the same frontmatter shape used by
/// the title editor.
fn build_task_content(task_title: &str) -> String {
    // #738: the workgroup-creation title is a human decision, so store it as
    // user-owned (`USER:`). This locks it against coordinator auto-retitle until
    // a Clean resets the task. `user_owned_title` trims and avoids double-prefix.
    let title = task_ops::user_owned_title(task_title);
    let escaped = title.replace('\'', "''");
    format!("---\ntitle: '{}'\n---\n", escaped)
}

/// Build the Role.md body written by `create_agent_matrix`. Kept as a separate
/// pure helper so the legacy (no-template) format can be unit-tested without
/// touching the Tauri command surface. `template` is the resolved template; when
/// `None`, output is byte-identical to the pre-#271 inline `format!`.
///
/// When a template is supplied, its body is inserted as a `## Role Profile`
/// section between the title/description and the mandatory `## Source of Truth`
/// / `## Agent Memory Rule` sections (plan §7) — the mandatory sections always
/// land last so a template body cannot push them off, and Role-Profile content
/// inherits the picker's display order (the section header makes the source of
/// the imported text obvious to a human reader of Role.md).
fn build_role_content(
    safe_name: &str,
    description: &str,
    template: Option<&crate::commands::role_templates::ResolvedRoleTemplate>,
) -> String {
    let desc_yaml = description.replace('\'', "''");
    // Plan §7.2: imported template body is fenced by HTML-comment delimiters so
    // the section boundary stays machine-detectable (and the opening tag
    // records provenance via the template id). Pre-release maintainer review
    // (§10.2) screens template bodies for the literal closing delimiter to
    // prevent a body from "escaping" the fenced section.
    let profile = match template {
        Some(t) => format!(
            "\n## Role Profile\n\n\
             <!-- ac:role-profile source=\"{}\" — imported template body; \
             the AC sections below are mandatory and must stay last -->\n\n\
             {}\n\n\
             <!-- ac:role-profile:end -->\n",
            t.id,
            t.body.trim(),
        ),
        None => String::new(),
    };
    format!(
        "---\nname: '{name}'\ndescription: '{desc_yaml}'\ntype: agent\n---\n\n\
         # {name}\n\n\
         {description}\n\
         {profile}\n\
         ## Source of Truth\n\n\
         This role is defined in Role.md of your Agent Matrix at: .ac/_agent_{name}/\n\
         If you are running as a replica, this file was generated from that source.\n\
         Always use memory/, plans/, and skills/ from your Agent Matrix, and treat Role.md \
         there as the canonical role definition. Never use external memory systems.\n\n\
         ## Agent Memory Rule\n\n\
         If you are running as a replica, the single source of truth for persistent knowledge \
         is your Agent Matrix's memory/, plans/, skills/, and Role.md. Use your replica folder \
         only for replica-local scratch, inbox/outbox, and session artifacts. NEVER use \
         external memory systems from the coding agent (e.g., ~/.claude/projects/memory/).\n",
        name = safe_name,
        desc_yaml = desc_yaml,
        description = description,
        profile = profile,
    )
}

pub(crate) struct CreateAgentMatrixDiskArgs<'a> {
    pub project_path: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub role_template_id: Option<&'a str>,
    pub settings: &'a AppSettings,
    pub config_dir: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub(crate) struct CreatedAgentMatrixOnDisk {
    pub agent_dir: PathBuf,
    pub display_name: String,
    pub safe_name: String,
    pub role_path: PathBuf,
}

fn agent_matrix_display_name(project_path: &Path, safe_name: &str) -> String {
    let project_folder = project_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| project_path.to_string_lossy().to_string());
    format!("{}/{}", project_folder, safe_name)
}

pub(crate) fn create_agent_matrix_on_disk(
    args: CreateAgentMatrixDiskArgs<'_>,
) -> Result<CreatedAgentMatrixOnDisk, String> {
    let safe_name = sanitize_name(args.name)?;
    let project = Path::new(args.project_path);
    let base = selected_workspace_dir(project)?;

    let agent_dir = base.join(format!("_agent_{}", safe_name));
    if agent_dir.exists() {
        return Err(format!("Agent '{}' already exists", safe_name));
    }

    // Resolve the picked template before any target disk mutation so unknown or
    // unreadable template ids cannot leave a half-built matrix behind.
    let resolved_template: Option<crate::commands::role_templates::ResolvedRoleTemplate> =
        match args
            .role_template_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => {
                let config_dir = args
                    .config_dir
                    .ok_or_else(|| "Could not determine config directory".to_string())?;
                Some(crate::commands::role_templates::resolve_role_template(
                    id,
                    args.settings,
                    config_dir,
                )?)
            }
            None => None,
        };

    create_new_agent_matrix_layout(&agent_dir).map_err(|(sub, e)| match (sub, e.kind()) {
        ("agent_dir", std::io::ErrorKind::AlreadyExists) => {
            format!("Agent '{}' already exists", safe_name)
        }
        ("agent_dir", _) => format!("Failed to create agent directory: {}", e),
        _ => format!("Failed to create {} directory: {}", sub, e),
    })?;

    let role_content = build_role_content(&safe_name, args.description, resolved_template.as_ref());
    let role_path = agent_dir.join("Role.md");
    std::fs::write(&role_path, &role_content)
        .map_err(|e| format!("Failed to write Role.md: {}", e))?;

    if let Some(ref t) = resolved_template {
        if let Some(ref src) = t.skills_src {
            let dst = agent_dir.join("skills");
            let failures = crate::commands::role_templates::copy_dir_recursive(src, &dst);
            if !failures.is_empty() {
                log::error!(
                    "[entity_creation] some files from template '{}' skills/ could not be \
                     copied into {} ({} failure(s)): {}",
                    t.id,
                    dst.display(),
                    failures.len(),
                    failures.join("; ")
                );
            }
        }
    }

    write_local_config_value(
        &agent_dir.join("config.json"),
        default_agent_matrix_config(),
    )?;

    let display_name = agent_matrix_display_name(project, &safe_name);
    Ok(CreatedAgentMatrixOnDisk {
        agent_dir,
        display_name,
        safe_name,
        role_path,
    })
}

pub(crate) struct CreateAgentMatrixFromRoleArgs<'a> {
    pub workspace_dir: &'a Path,
    pub safe_name: &'a str,
    pub role_bytes: &'a [u8],
}

pub(crate) fn create_agent_matrix_from_role(
    args: CreateAgentMatrixFromRoleArgs<'_>,
) -> Result<CreatedAgentMatrixOnDisk, String> {
    if sanitize_name(args.safe_name)? != args.safe_name {
        return Err(format!(
            "Agent '{}' must already be a lowercase slug",
            args.safe_name
        ));
    }
    validate_existing_name(args.safe_name, "Agent")?;
    let agent_dir = args
        .workspace_dir
        .join(format!("_agent_{}", args.safe_name));
    if agent_dir.exists() {
        return Err(format!("Agent '{}' already exists", args.safe_name));
    }
    create_new_agent_matrix_layout(&agent_dir).map_err(|(sub, e)| match (sub, e.kind()) {
        ("agent_dir", std::io::ErrorKind::AlreadyExists) => {
            format!("Agent '{}' already exists", args.safe_name)
        }
        ("agent_dir", _) => format!("Failed to create agent directory: {}", e),
        _ => format!("Failed to create {} directory: {}", sub, e),
    })?;

    let role_path = agent_dir.join("Role.md");
    std::fs::write(&role_path, args.role_bytes)
        .map_err(|e| format!("Failed to write Role.md: {}", e))?;

    write_local_config_value(
        &agent_dir.join("config.json"),
        default_agent_matrix_config(),
    )?;

    let project_path = args
        .workspace_dir
        .parent()
        .ok_or_else(|| "Project AC Root has no project parent".to_string())?;
    Ok(CreatedAgentMatrixOnDisk {
        agent_dir,
        display_name: agent_matrix_display_name(project_path, args.safe_name),
        safe_name: args.safe_name.to_string(),
        role_path,
    })
}

fn default_agent_matrix_config() -> serde_json::Value {
    serde_json::json!({
        "tooling": {},
        "context": ["$AGENTSCOMMANDER_CONTEXT", "Role.md"],
    })
}

fn write_local_config_value(config_path: &Path, value: serde_json::Value) -> Result<(), String> {
    crate::config::local_config_io::update_config_json_object(config_path, true, |obj| {
        let map = value
            .as_object()
            .ok_or_else(|| "Local config value must be a JSON object".to_string())?;
        *obj = map.clone();
        Ok(())
    })?;
    Ok(())
}

fn selected_workspace_dir(project: &Path) -> Result<PathBuf, String> {
    existing_workspace_dir(project)
        .ok_or_else(|| format!(".ac directory not found in {}", project.display()))
}

pub(crate) fn read_team_config(
    workspace_dir: &Path,
    team_name: &str,
) -> Result<TeamConfigResult, String> {
    validate_existing_name(team_name, "Team")?;
    let team_dir = workspace_dir.join(format!("_team_{}", team_name));
    let config_path = team_dir.join("config.json");
    if !config_path.exists() {
        return Err(format!("Team '{}' config not found", team_name));
    }
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.json: {}", e))?;
    let config: TeamConfigResult = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse config.json: {}", e))?;
    normalize_team_config_for_project(workspace_dir, &config)
}

pub(crate) fn write_team_config(
    workspace_dir: &Path,
    team_name: &str,
    config: &TeamConfigResult,
) -> Result<PathBuf, String> {
    validate_existing_name(team_name, "Team")?;
    let team_dir = workspace_dir.join(format!("_team_{}", team_name));
    std::fs::create_dir_all(&team_dir)
        .map_err(|e| format!("Failed to create team directory: {}", e))?;
    std::fs::create_dir_all(team_dir.join("memory"))
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;
    let conventions = team_dir.join("conventions.md");
    if !conventions.exists() {
        std::fs::write(&conventions, "")
            .map_err(|e| format!("Failed to write conventions.md: {}", e))?;
    }
    let config = normalize_team_config_for_project(workspace_dir, config)?;
    let config_str = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config.json: {}", e))?;
    std::fs::write(team_dir.join("config.json"), config_str)
        .map_err(|e| format!("Failed to write config.json: {}", e))?;
    Ok(team_dir)
}

pub(crate) fn create_new_team_config_on_disk(
    workspace_dir: &Path,
    team_name: &str,
    config: &TeamConfigResult,
) -> Result<PathBuf, String> {
    validate_existing_name(team_name, "Team")?;
    let team_dir = workspace_dir.join(format!("_team_{}", team_name));
    std::fs::create_dir(&team_dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("Team '{}' already exists", team_name),
        _ => format!("Failed to create team directory: {}", e),
    })?;
    std::fs::create_dir(team_dir.join("memory"))
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;
    std::fs::write(team_dir.join("conventions.md"), "")
        .map_err(|e| format!("Failed to write conventions.md: {}", e))?;
    let config = normalize_team_config_for_project(workspace_dir, config)?;
    let config_str = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config.json: {}", e))?;
    std::fs::write(team_dir.join("config.json"), config_str)
        .map_err(|e| format!("Failed to write config.json: {}", e))?;
    Ok(team_dir)
}

pub(crate) fn normalize_team_config_for_project(
    workspace_dir: &Path,
    config: &TeamConfigResult,
) -> Result<TeamConfigResult, String> {
    let agents = normalize_team_agent_refs(workspace_dir, &config.agents)?;
    let coordinator = if config.coordinator.trim().is_empty() {
        String::new()
    } else {
        resolve_agent_ref(workspace_dir, &config.coordinator)?
    };
    if !coordinator.is_empty() && !agents.contains(&coordinator) {
        return Err("Coordinator must be one of the selected agents".to_string());
    }
    let mut repos = Vec::with_capacity(config.repos.len());
    for repo in &config.repos {
        repos.push(RepoAssignment {
            url: repo.url.clone(),
            agents: normalize_team_agent_refs(workspace_dir, &repo.agents)?,
        });
    }
    Ok(TeamConfigResult {
        agents,
        coordinator,
        repos,
    })
}

fn normalize_team_agent_refs(workspace_dir: &Path, refs: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for agent in refs {
        let resolved = resolve_agent_ref(workspace_dir, agent)?;
        if !normalized.contains(&resolved) {
            normalized.push(resolved);
        }
    }
    Ok(normalized)
}

pub(crate) fn list_workgroup_dirs(workspace_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(workspace_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if parse_team_from_workgroup_name(name).is_ok() {
                dirs.push(path);
            }
        }
    }
    dirs.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });
    dirs
}

pub(crate) fn parse_team_from_workgroup_name(workgroup_name: &str) -> Result<String, String> {
    let rest = workgroup_name
        .strip_prefix("wg-")
        .ok_or_else(|| format!("Invalid workgroup name '{}'", workgroup_name))?;
    let Some((number, team)) = rest.split_once('-') else {
        return Err(format!("Invalid workgroup name '{}'", workgroup_name));
    };
    let parsed = number
        .parse::<u32>()
        .map_err(|_| format!("Invalid workgroup number in '{}'", workgroup_name))?;
    if parsed == 0 || team.is_empty() {
        return Err(format!("Invalid workgroup name '{}'", workgroup_name));
    }
    validate_existing_name(team, "Team")?;
    Ok(team.to_string())
}

pub(crate) fn resolve_agent_ref(workspace_dir: &Path, raw_agent: &str) -> Result<String, String> {
    let trimmed = raw_agent.trim();
    if trimmed.is_empty() {
        return Err("Agent reference cannot be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("Agent reference must not contain NUL".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains(':') {
        return Err(format!(
            "Invalid agent reference '{}': team configs must use portable refs like '_agent_name' or 'name', not filesystem paths",
            raw_agent
        ));
    }
    if Path::new(trimmed).is_absolute() {
        return Err(format!(
            "Invalid agent reference '{}': absolute paths are not allowed in team configs",
            raw_agent
        ));
    }
    let bare = trimmed.strip_prefix("_agent_").unwrap_or(trimmed);
    validate_existing_name(bare, "Agent")?;
    let canonical_ref = format!("_agent_{}", bare);
    let matrix_dir = workspace_dir.join(&canonical_ref);
    if !matrix_dir.is_dir() {
        return Err(format!("Agent '{}' not found", bare));
    }
    Ok(canonical_ref)
}

pub(crate) fn agent_ref_bare_name(agent_ref: &str) -> String {
    let dir_name = Path::new(agent_ref)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(agent_ref);
    dir_name
        .strip_prefix("_agent_")
        .unwrap_or(dir_name)
        .to_string()
}

pub(crate) async fn create_workgroup_on_disk(
    args: WorkgroupDiskCreateArgs,
) -> Result<WorkgroupCloneResult, String> {
    let safe_team = sanitize_name(&args.team_name)?;
    let task_title = args.task_title.trim().to_string();
    validate_task_title(&task_title)?;
    let base = selected_workspace_dir(&args.project_path)?;

    if let Err(e) = crate::commands::ac_discovery::ensure_workspace_gitignore(&base) {
        log::warn!(
            "[create_workgroup] Failed to ensure Project AC Root .gitignore: {}",
            e
        );
    }

    let team_config =
        if !args.agents.is_empty() || args.coordinator.is_some() || !args.repos.is_empty() {
            let coordinator = args.coordinator.clone().ok_or_else(|| {
                "Coordinator is required when provisioning team config".to_string()
            })?;
            if !args.agents.contains(&coordinator) {
                return Err("Coordinator must be one of the selected agents".to_string());
            }
            let config = TeamConfigResult {
                agents: args.agents.clone(),
                coordinator,
                repos: args.repos.clone(),
            };
            let config = normalize_team_config_for_project(&base, &config)?;
            write_team_config(&base, &safe_team, &config)?;
            config
        } else {
            read_team_config(&base, &safe_team)?
        };

    let wg_number = determine_next_wg_number(&base);
    let wg_name = format!("wg-{}-{}", wg_number, safe_team);
    let wg_dir = base.join(&wg_name);
    if wg_dir.exists() {
        return Err(format!("Workgroup directory already exists: {}", wg_name));
    }
    std::fs::create_dir_all(&wg_dir)
        .map_err(|e| format!("Failed to create workgroup directory: {}", e))?;
    std::fs::create_dir_all(wg_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME))
        .map_err(|e| format!("Failed to create messaging directory: {}", e))?;
    std::fs::write(wg_dir.join("TASK.md"), build_task_content(&task_title))
        .map_err(|e| format!("Failed to write TASK.md: {}", e))?;

    for agent_path in &team_config.agents {
        create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
            workspace_dir: base.clone(),
            wg_dir: wg_dir.clone(),
            agent_path: agent_path.clone(),
            team_repos: team_config.repos.clone(),
        })?;
    }

    let clone_errors = clone_missing_repos_for_workgroup(&wg_dir, &team_config.repos).await;
    let result_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&wg_dir);
    log::info!(
        "[entity_creation] Created workgroup: {} ({} clone errors)",
        result_path,
        clone_errors.len()
    );
    Ok(WorkgroupCloneResult {
        path: result_path,
        clone_errors,
    })
}

pub(crate) fn create_or_update_replica_on_disk(
    args: ReplicaDiskCreateArgs,
) -> Result<PathBuf, String> {
    let agent_ref = resolve_agent_ref(&args.workspace_dir, &args.agent_path)?;
    let agent_name = agent_ref_bare_name(&agent_ref);
    validate_existing_name(&agent_name, "Agent")?;
    let replica_dir = args.wg_dir.join(format!("__agent_{}", agent_name));

    create_agent_replica_layout(&replica_dir).map_err(|(sub, e)| match sub {
        "replica_dir" => format!("Failed to create replica dir for {}: {}", agent_name, e),
        _ => format!("Failed to create {} for {}: {}", sub, agent_name, e),
    })?;

    let assigned_repos: Vec<String> = args
        .team_repos
        .iter()
        .filter(|r| r.agents.iter().any(|a| agent_matches(a, &agent_name)))
        .map(|r| {
            let dir_name = format!("repo-{}", repo_dir_name_from_url(&r.url));
            format!("../{}", dir_name)
        })
        .collect();

    let identity = expected_wg_replica_identity(&replica_dir)?;
    let context_entries = normalize_wg_replica_context_entries(
        &[],
        WG_REPLICA_REQUIRED_CONTEXT,
        &identity.identity,
        identity.matrix_dir.join(ROLE_MD_FILENAME).exists(),
    );

    let replica_config = serde_json::json!({
        "identity": identity.identity,
        "repos": assigned_repos,
        "context": context_entries,
    });
    write_local_config_value(&replica_dir.join("config.json"), replica_config)?;
    Ok(replica_dir)
}

pub(crate) fn remove_replica_dir(replica_dir: &Path) -> Result<(), String> {
    if !replica_dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(replica_dir)
        .map_err(|e| format!("Failed to delete replica directory: {}", e))
}

pub(crate) async fn clone_missing_repos_for_workgroup(
    wg_dir: &Path,
    repos: &[RepoAssignment],
) -> Vec<CloneError> {
    let mut clone_errors = Vec::new();
    let mut seen_urls = HashSet::new();
    for repo in repos {
        if !seen_urls.insert(repo.url.clone()) {
            continue;
        }
        let dir_name = format!("repo-{}", repo_dir_name_from_url(&repo.url));
        let target = wg_dir.join(&dir_name);
        if target.exists() {
            continue;
        }
        match git_clone_async(&repo.url, &target).await {
            Ok(_) => log::info!(
                "[entity_creation] Cloned {} -> {}",
                repo.url,
                target.display()
            ),
            Err(e) => {
                log::error!("[entity_creation] Failed to clone {}: {}", repo.url, e);
                clone_errors.push(CloneError {
                    url: repo.url.clone(),
                    error: e,
                });
            }
        }
    }
    clone_errors
}

fn validate_task_title(task_title: &str) -> Result<(), String> {
    if task_title.is_empty() {
        return Err("Task title cannot be empty".to_string());
    }
    if task_title.chars().any(|c| c.is_control() && c != '\t') {
        return Err("Task title must be a single line of printable characters \
             (control characters other than tab are not allowed)"
            .to_string());
    }
    if task_title.chars().count() > 256 {
        return Err("Task title is too long (max 256 characters)".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Create an agent matrix directory inside {project_path}/.ac/_agent_{name}/
///
/// `role_template_id` is the picker selection from `list_role_templates`; when
/// `Some`, the resolved template's body is inserted as a `## Role Profile`
/// section and (if the template ships one) its `skills/` are copied into the
/// new matrix. `None` (or a missing arg from older callers) preserves the
/// pre-#271 behavior exactly.
#[tauri::command]
pub async fn create_agent_matrix(
    settings: State<'_, SettingsState>,
    project_path: String,
    name: String,
    description: String,
    role_template_id: Option<String>,
) -> Result<CreatedEntityResult, String> {
    let settings_snapshot = settings.read().await.clone();
    let config_dir = crate::config::config_dir();

    let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_path,
        name: &name,
        description: &description,
        role_template_id: role_template_id.as_deref(),
        settings: &settings_snapshot,
        config_dir: config_dir.as_deref(),
    })?;

    let result_path =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&created.agent_dir);
    log::debug!(
        "[entity_creation] Created agent matrix safe name: {}",
        created.safe_name
    );
    log::info!("[entity_creation] Created agent matrix: {}", result_path);
    Ok(CreatedEntityResult { path: result_path })
}

/// Delete an agent matrix identity from a project.
#[tauri::command]
pub async fn delete_agent_matrix(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    project_path: String,
    agent_path: String,
) -> Result<(), String> {
    let base = selected_workspace_dir(Path::new(&project_path))?;
    let plan = collect_agent_delete_plan(&base, Path::new(&agent_path))?;
    preflight_agent_delete(&plan, session_mgr.inner()).await?;

    let metadata = prepare_agent_delete_metadata(&plan, settings.inner()).await?;
    let staged = stage_agent_delete_targets(&plan, session_mgr.inner()).await?;

    if let Err(live_err) = ensure_no_live_sessions_under_target_keys(
        &plan.target_keys,
        &plan.agent_name,
        session_mgr.inner(),
    )
    .await
    {
        let rollback = rollback_staged_agent_delete_targets(&staged);
        return Err(format_agent_delete_post_stage_live_failure(
            live_err, rollback,
        ));
    }

    let persist = persist_agent_delete_metadata(&metadata, settings.inner()).await;
    if let Err(e) = persist {
        let restore = restore_agent_delete_metadata_snapshots(&metadata, settings.inner()).await;
        let rollback = rollback_staged_agent_delete_targets(&staged);
        return Err(format_agent_delete_metadata_failure(e, restore, rollback));
    }

    remove_staged_agent_delete_targets(&staged)?;

    log::info!(
        "[entity_creation] Deleted agent matrix identity '{}' at '{}' (targets={}, team_configs={})",
        plan.agent_ref,
        plan.origin_dir.display(),
        plan.targets.len(),
        metadata.team_mutations.len()
    );
    if metadata.settings_changed.load(Ordering::SeqCst) {
        let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
    }
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(())
}

#[derive(Debug, Clone)]
struct AgentDeleteTarget {
    original_path: PathBuf,
    original_key: String,
    label: String,
}

#[derive(Debug, Clone)]
struct StagedAgentDeleteTarget {
    original_path: PathBuf,
    original_key: String,
    staged_path: PathBuf,
    label: String,
}

#[derive(Debug, Clone)]
struct AgentTeamMutation {
    team_name: String,
    config_path: PathBuf,
    before_json: Vec<u8>,
    after_json: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PreparedAgentDeleteMetadata {
    team_mutations: Vec<AgentTeamMutation>,
    agent_name: String,
    settings_changed: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct AgentDeletePlan {
    agent_name: String,
    agent_ref: String,
    origin_dir: PathBuf,
    target_keys: Vec<String>,
    targets: Vec<AgentDeleteTarget>,
    team_mutations: Vec<AgentTeamMutation>,
}

fn resolve_agent_delete_identity(
    base: &Path,
    agent_path: &Path,
) -> Result<(String, String, PathBuf), String> {
    if !agent_path.is_absolute() {
        return Err("Agent path must be absolute".to_string());
    }
    let final_segment = agent_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Agent path must end with an agent matrix directory".to_string())?;
    let agent_name = final_segment
        .strip_prefix("_agent_")
        .ok_or_else(|| "Agent path must end with _agent_<name>".to_string())?;
    validate_existing_name(agent_name, "Agent")?;
    validate_delete_root_not_link_or_reparse(agent_path)
        .map_err(|e| format!("Selected agent path is not deletable: {}", e))?;

    let agent_ref = format!("_agent_{}", agent_name);
    let origin_dir = base.join(&agent_ref);
    if path_key_for_delete(agent_path) != path_key_for_delete(&origin_dir) {
        return Err("Agent path does not match the selected project".to_string());
    }
    validate_delete_root_not_link_or_reparse(&origin_dir)
        .map_err(|e| format!("Agent '{}' not found or not deletable: {}", agent_name, e))?;

    Ok((agent_name.to_string(), agent_ref, origin_dir))
}

fn collect_agent_delete_plan(base: &Path, agent_path: &Path) -> Result<AgentDeletePlan, String> {
    let (agent_name, agent_ref, origin_dir) = resolve_agent_delete_identity(base, agent_path)?;
    let team_mutations = collect_agent_team_mutations_raw(base, &agent_name, &agent_ref)?;

    let mut targets = vec![AgentDeleteTarget {
        original_key: path_key_for_delete(&origin_dir),
        original_path: origin_dir.clone(),
        label: agent_ref.clone(),
    }];

    for wg_dir in list_workgroup_dirs(base) {
        let replica = wg_dir.join(format!("__agent_{}", agent_name));
        if replica.exists() {
            let wg_name = wg_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workgroup");
            targets.push(AgentDeleteTarget {
                original_key: path_key_for_delete(&replica),
                original_path: replica,
                label: format!("{}/__agent_{}", wg_name, agent_name),
            });
        }
    }

    targets.sort_by(|a, b| a.original_key.cmp(&b.original_key));
    let mut target_keys = Vec::new();
    for target in &targets {
        for key in target_keys_for_delete(&target.original_path) {
            if !target_keys.contains(&key) {
                target_keys.push(key);
            }
        }
    }

    Ok(AgentDeletePlan {
        agent_name,
        agent_ref,
        origin_dir,
        target_keys,
        targets,
        team_mutations,
    })
}

fn collect_agent_team_mutations_raw(
    base: &Path,
    agent_name: &str,
    agent_ref: &str,
) -> Result<Vec<AgentTeamMutation>, String> {
    let mut team_dirs = Vec::new();
    let entries = std::fs::read_dir(base).map_err(|e| {
        format!(
            "Cannot read Project AC Root directory for agent delete: {}",
            e
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            format!(
                "Cannot read directory entry during agent delete scan: {}",
                e
            )
        })?;
        let path = entry.path();
        let Some(dir_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_dir() && dir_name.starts_with("_team_") {
            team_dirs.push(path);
        }
    }
    team_dirs.sort();

    let mut coordinator_blockers = Vec::new();
    let mut mutations = Vec::new();
    for team_dir in team_dirs {
        let dir_name = team_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("_team_unknown");
        let team_name = dir_name.strip_prefix("_team_").unwrap_or(dir_name);
        let config_path = team_dir.join("config.json");
        if !config_path.exists() {
            continue;
        }

        let before_json = std::fs::read(&config_path)
            .map_err(|e| format!("Cannot read team '{}' config.json: {}", team_name, e))?;
        let mut value: serde_json::Value = match serde_json::from_slice(&before_json) {
            Ok(value) => value,
            Err(e) => {
                let raw = String::from_utf8_lossy(&before_json);
                if raw_text_mentions_agent_token(&raw, agent_name, agent_ref) {
                    return Err(format!(
                        "Cannot delete agent '{}': cannot verify team '{}' references because config.json is invalid: {}",
                        agent_name, team_name, e
                    ));
                }
                log::warn!(
                    "[entity_creation] Skipping unrelated malformed team config for '{}': {}",
                    team_name,
                    e
                );
                continue;
            }
        };

        if value
            .get("coordinator")
            .and_then(|coordinator| coordinator.as_str())
            .is_some_and(|coordinator| agent_ref_matches_target(coordinator, agent_name))
        {
            coordinator_blockers.push(team_name.to_string());
        }

        let mut changed = false;
        if let Some(agents) = value
            .get_mut("agents")
            .and_then(|agents| agents.as_array_mut())
        {
            let before_len = agents.len();
            agents.retain(|agent| match agent.as_str() {
                Some(agent) => !agent_ref_matches_target(agent, agent_name),
                None => true,
            });
            changed |= agents.len() != before_len;
        }

        if let Some(repos) = value
            .get_mut("repos")
            .and_then(|repos| repos.as_array_mut())
        {
            for repo in repos {
                let Some(repo_obj) = repo.as_object_mut() else {
                    continue;
                };
                let Some(agents) = repo_obj
                    .get_mut("agents")
                    .and_then(|agents| agents.as_array_mut())
                else {
                    continue;
                };
                let before_len = agents.len();
                agents.retain(|agent| match agent.as_str() {
                    Some(agent) => !agent_ref_matches_target(agent, agent_name),
                    None => true,
                });
                changed |= agents.len() != before_len;
            }
        }

        if changed {
            let mut after_json = serde_json::to_vec_pretty(&value)
                .map_err(|e| format!("Cannot serialize team '{}' config.json: {}", team_name, e))?;
            after_json.push(b'\n');
            mutations.push(AgentTeamMutation {
                team_name: team_name.to_string(),
                config_path,
                before_json,
                after_json,
            });
        }
    }

    if !coordinator_blockers.is_empty() {
        coordinator_blockers.sort();
        return Err(format!(
            "Cannot delete agent '{}': coordinator of team(s): {}. Reassign the coordinator first.",
            agent_name,
            coordinator_blockers.join(", ")
        ));
    }

    Ok(mutations)
}

fn agent_ref_matches_target(raw_ref: &str, agent_name: &str) -> bool {
    let bare = raw_agent_ref_bare_name(raw_ref);
    agent_name_matches(&bare, agent_name)
}

fn raw_agent_ref_bare_name(raw_ref: &str) -> String {
    let normalized = raw_ref.replace('\\', "/");
    let final_segment = normalized.rsplit('/').next().unwrap_or(&normalized);
    final_segment
        .strip_prefix("_agent_")
        .unwrap_or(final_segment)
        .to_string()
}

fn raw_text_mentions_agent_token(raw: &str, agent_name: &str, agent_ref: &str) -> bool {
    contains_whole_agent_token(raw, agent_ref) || contains_whole_agent_token(raw, agent_name)
}

fn contains_whole_agent_token(raw: &str, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    #[cfg(windows)]
    let (haystack, needle) = (raw.to_ascii_lowercase(), token.to_ascii_lowercase());
    #[cfg(not(windows))]
    let (haystack, needle) = (raw.to_string(), token.to_string());

    let mut start = 0;
    while let Some(offset) = haystack[start..].find(&needle) {
        let match_start = start + offset;
        let match_end = match_start + needle.len();
        let before_ok = haystack[..match_start]
            .chars()
            .next_back()
            .map(|c| !is_agent_token_char(c))
            .unwrap_or(true);
        let after_ok = haystack[match_end..]
            .chars()
            .next()
            .map(|c| !is_agent_token_char(c))
            .unwrap_or(true);
        if before_ok && after_ok {
            return true;
        }
        start = match_end;
    }
    false
}

fn is_agent_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn agent_name_matches(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

async fn preflight_agent_delete(
    plan: &AgentDeletePlan,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    for target in &plan.targets {
        validate_delete_root_not_link_or_reparse(&target.original_path).map_err(|e| {
            format!(
                "Cannot delete agent '{}': target '{}' is not deletable: {}",
                plan.agent_name, target.label, e
            )
        })?;
    }
    ensure_no_live_sessions_under_target_keys(&plan.target_keys, &plan.agent_name, session_mgr)
        .await
}

async fn ensure_no_live_sessions_under_target_keys(
    target_keys: &[String],
    agent_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    let sessions = { session_mgr.read().await.list_sessions().await };
    let blockers: Vec<_> = sessions
        .into_iter()
        .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
        .filter(|session| {
            let working_dir = Path::new(&session.working_directory);
            let working_key = path_key_for_delete(working_dir);
            target_keys.iter().any(|root_key| {
                working_key == *root_key || working_key.starts_with(&(root_key.clone() + "/"))
            })
        })
        .collect();

    if blockers.is_empty() {
        return Ok(());
    }

    let details = blockers
        .iter()
        .map(|session| {
            format!(
                "  - {} at {} ({:?})",
                session.name, session.working_directory, session.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "Cannot delete agent '{}' while live sessions exist:\n{}",
        agent_name, details
    ))
}

async fn prepare_agent_delete_metadata(
    plan: &AgentDeletePlan,
    settings: &SettingsState,
) -> Result<PreparedAgentDeleteMetadata, String> {
    let settings_snapshot = settings.read().await;
    let settings_changed = settings_snapshot
        .auto_self_clear_by_agent
        .contains_key(&plan.agent_name)
        || settings_snapshot
            .coding_agent_profiles
            .default_profile_by_agent
            .contains_key(&plan.agent_name);

    Ok(PreparedAgentDeleteMetadata {
        team_mutations: plan.team_mutations.clone(),
        agent_name: plan.agent_name.clone(),
        settings_changed: Arc::new(AtomicBool::new(settings_changed)),
    })
}

async fn persist_agent_delete_metadata(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
) -> Result<(), String> {
    persist_agent_delete_metadata_with_saver(
        metadata,
        settings,
        crate::config::settings::save_settings,
    )
    .await
}

async fn persist_agent_delete_metadata_with_saver<S>(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    save_settings_fn: S,
) -> Result<(), String>
where
    S: Fn(&AppSettings) -> Result<AppSettings, String>,
{
    persist_agent_delete_metadata_with_writers(
        metadata,
        settings,
        write_team_config_json_atomic,
        save_settings_fn,
    )
    .await
}

async fn persist_agent_delete_metadata_with_writers<T, S>(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    mut write_team_config_fn: T,
    save_settings_fn: S,
) -> Result<(), String>
where
    T: FnMut(&Path, &[u8]) -> Result<(), String>,
    S: Fn(&AppSettings) -> Result<AppSettings, String>,
{
    metadata.settings_changed.store(false, Ordering::SeqCst);

    for mutation in &metadata.team_mutations {
        if let Err(e) = write_team_config_fn(&mutation.config_path, &mutation.after_json) {
            let restore = restore_team_config_snapshots(metadata);
            return Err(format!(
                "Failed to write team '{}' config during agent delete: {}{}",
                mutation.team_name,
                e,
                format_restore_suffix(restore)
            ));
        }
    }

    let settings_result = {
        let mut guard = settings.write().await;
        let removed_auto = guard.auto_self_clear_by_agent.remove(&metadata.agent_name);
        let removed_default = guard
            .coding_agent_profiles
            .default_profile_by_agent
            .remove(&metadata.agent_name);
        let changed = removed_auto.is_some() || removed_default.is_some();
        metadata.settings_changed.store(changed, Ordering::SeqCst);

        if changed {
            let mut candidate = guard.clone();
            if let Err(e) = crate::config::settings::validate_and_repair_settings(&mut candidate) {
                restore_removed_settings(
                    &mut guard,
                    &metadata.agent_name,
                    removed_auto,
                    removed_default,
                );
                Err(format!(
                    "Failed to validate settings after agent delete: {}",
                    e
                ))
            } else {
                match save_settings_fn(&candidate) {
                    Ok(written) => {
                        *guard = written;
                        Ok(())
                    }
                    Err(e) => {
                        restore_removed_settings(
                            &mut guard,
                            &metadata.agent_name,
                            removed_auto,
                            removed_default,
                        );
                        Err(format!("Failed to save settings after agent delete: {}", e))
                    }
                }
            }
        } else {
            Ok(())
        }
    };

    if let Err(e) = settings_result {
        let restore = restore_team_config_snapshots(metadata);
        return Err(format!("{}{}", e, format_restore_suffix(restore)));
    }

    Ok(())
}

fn restore_removed_settings(
    settings: &mut AppSettings,
    agent_name: &str,
    removed_auto: Option<bool>,
    removed_default: Option<String>,
) {
    if let Some(value) = removed_auto {
        settings
            .auto_self_clear_by_agent
            .insert(agent_name.to_string(), value);
    }
    if let Some(value) = removed_default {
        settings
            .coding_agent_profiles
            .default_profile_by_agent
            .insert(agent_name.to_string(), value);
    }
}

async fn restore_agent_delete_metadata_snapshots(
    metadata: &PreparedAgentDeleteMetadata,
    _settings: &SettingsState,
) -> Result<(), String> {
    restore_team_config_snapshots(metadata)
}

fn restore_team_config_snapshots(metadata: &PreparedAgentDeleteMetadata) -> Result<(), String> {
    let mut errors = Vec::new();
    for mutation in &metadata.team_mutations {
        if let Err(e) = write_team_config_json_atomic(&mutation.config_path, &mutation.before_json)
        {
            errors.push(format!("{}: {}", mutation.config_path.display(), e));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Failed to restore team config snapshot(s): {}",
            errors.join("; ")
        ))
    }
}

fn format_restore_suffix(restore: Result<(), String>) -> String {
    match restore {
        Ok(()) => String::new(),
        Err(e) => format!("; snapshot restore failed: {}", e),
    }
}

fn write_team_config_json_atomic(config_path: &Path, json: &[u8]) -> Result<(), String> {
    crate::config::local_config_io::write_file_atomic(config_path, json)
}

fn format_agent_delete_metadata_failure(
    persist_error: String,
    restore_result: Result<(), String>,
    dir_rollback_result: Result<(), String>,
) -> String {
    match (restore_result, dir_rollback_result) {
        (Ok(()), Ok(())) => persist_error,
        (Err(restore), Ok(())) => format!(
            "{}. Metadata snapshot restore also failed: {}",
            persist_error, restore
        ),
        (Ok(()), Err(rollback)) => format!(
            "{}. Directory rollback also failed: {}",
            persist_error, rollback
        ),
        (Err(restore), Err(rollback)) => format!(
            "{}. Metadata snapshot restore also failed: {}. Directory rollback also failed: {}",
            persist_error, restore, rollback
        ),
    }
}

fn format_agent_delete_post_stage_live_failure(
    live_error: String,
    dir_rollback_result: Result<(), String>,
) -> String {
    match dir_rollback_result {
        Ok(()) => live_error,
        Err(rollback) => format!(
            "Agent delete post-stage live-session check failed and rollback also failed. Live-session error: {}. Rollback error: {}",
            live_error, rollback
        ),
    }
}

/// List all agent matrices across multiple project paths.
/// Scans {project}/.ac/_agent_*/ and reads Role.md frontmatter.
#[tauri::command]
pub async fn list_all_agents(project_paths: Vec<String>) -> Result<Vec<AgentInfo>, String> {
    let mut agents: Vec<AgentInfo> = Vec::new();

    for project_path in &project_paths {
        let base = Path::new(project_path);
        let Some(workspace_dir) = existing_workspace_dir(base) else {
            continue;
        };

        let project_name = base
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let entries = match std::fs::read_dir(&workspace_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            if !dir_name.starts_with("_agent_") {
                continue;
            }

            let agent_name_from_dir = dir_name
                .strip_prefix("_agent_")
                .unwrap_or(&dir_name)
                .to_string();

            // Try to read Role.md frontmatter for richer metadata
            let role_path = path.join("Role.md");
            let (fm_name, fm_description) = if role_path.exists() {
                match std::fs::read_to_string(&role_path) {
                    Ok(content) => parse_role_frontmatter(&content),
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };

            agents.push(AgentInfo {
                name: fm_name.unwrap_or(agent_name_from_dir),
                description: fm_description.unwrap_or_default(),
                path: path.to_string_lossy().to_string(),
                project_name: project_name.clone(),
            });
        }
    }

    agents.sort_by_key(|a| a.name.to_lowercase());
    Ok(agents)
}

/// Create a team directory inside {project_path}/.ac/_team_{name}/
#[tauri::command]
pub async fn create_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    name: String,
    agents: Vec<String>,
    coordinator: String,
    repos: Vec<RepoAssignment>,
) -> Result<CreatedEntityResult, String> {
    let safe_name = sanitize_name(&name)?;
    let base = selected_workspace_dir(Path::new(&project_path))?;

    let config = normalize_team_config_for_project(
        &base,
        &TeamConfigResult {
            agents,
            coordinator,
            repos,
        },
    )?;
    let team_dir = create_new_team_config_on_disk(&base, &safe_name, &config)?;

    let result_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&team_dir);
    log::info!("[entity_creation] Created team: {}", result_path);
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(CreatedEntityResult { path: result_path })
}

/// Create a workgroup from an existing team.
/// Clones repos async — partial failures are reported but don't rollback the WG.
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
    task_title: String,
) -> Result<WorkgroupCloneResult, String> {
    let safe_team = sanitize_name(&team_name)?;
    let task_title = task_title.trim().to_string();
    if task_title.is_empty() {
        return Err("Task title cannot be empty".to_string());
    }
    if task_title.chars().any(|c| c.is_control() && c != '\t') {
        return Err("Task title must be a single line of printable characters \
             (control characters other than tab are not allowed)"
            .to_string());
    }
    if task_title.chars().count() > 256 {
        return Err("Task title is too long (max 256 characters)".to_string());
    }
    let base = selected_workspace_dir(Path::new(&project_path))?;

    // Ensure gitignore protects workgroup clones from parent repo operations
    if let Err(e) = crate::commands::ac_discovery::ensure_workspace_gitignore(&base) {
        log::warn!(
            "[create_workgroup] Failed to ensure Project AC Root .gitignore: {}",
            e
        );
    }

    let team_config = read_team_config(&base, &safe_team)?;

    // Determine next WG number
    let wg_number = determine_next_wg_number(&base);

    let wg_name = format!("wg-{}-{}", wg_number, safe_team);
    let wg_dir = base.join(&wg_name);
    if wg_dir.exists() {
        return Err(format!("Workgroup directory already exists: {}", wg_name));
    }
    std::fs::create_dir_all(&wg_dir)
        .map_err(|e| format!("Failed to create workgroup directory: {}", e))?;
    std::fs::create_dir_all(wg_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME))
        .map_err(|e| format!("Failed to create messaging directory: {}", e))?;

    // TASK.md: every new workgroup starts with an explicit task title.
    let task_content = build_task_content(&task_title);
    std::fs::write(wg_dir.join("TASK.md"), &task_content)
        .map_err(|e| format!("Failed to write TASK.md: {}", e))?;

    let team_agents = team_config.agents;
    let team_repos = team_config.repos;

    // Collect unique repo URLs and their directory names
    let mut unique_repos: Vec<(String, String)> = Vec::new(); // (url, dir_name)
    let mut seen_urls: HashSet<String> = HashSet::new();
    for repo in &team_repos {
        if seen_urls.insert(repo.url.clone()) {
            let dir_name = format!("repo-{}", repo_dir_name_from_url(&repo.url));
            unique_repos.push((repo.url.clone(), dir_name));
        }
    }

    // Create __agent_*/ replica dirs
    for agent_path in &team_agents {
        let agent_ref = resolve_agent_ref(&base, agent_path)?;
        let agent_name = agent_ref_bare_name(&agent_ref);

        let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));

        // Per-replica layout. Canonical state stays in the origin matrix.
        create_agent_replica_layout(&replica_dir).map_err(|(sub, e)| match sub {
            "replica_dir" => format!("Failed to create replica dir for {}: {}", agent_name, e),
            _ => format!("Failed to create {} for {}: {}", sub, agent_name, e),
        })?;

        // Determine repos assigned to this agent (match by _agent_ name)
        let assigned_repos: Vec<String> = team_repos
            .iter()
            .filter(|r| r.agents.iter().any(|a| agent_matches(a, &agent_name)))
            .map(|r| {
                let dir_name = format!("repo-{}", repo_dir_name_from_url(&r.url));
                format!("../{}", dir_name)
            })
            .collect();

        let identity = expected_wg_replica_identity(&replica_dir)?;
        let context_entries = normalize_wg_replica_context_entries(
            &[],
            WG_REPLICA_REQUIRED_CONTEXT,
            &identity.identity,
            identity.matrix_dir.join(ROLE_MD_FILENAME).exists(),
        );

        let replica_config = serde_json::json!({
            "identity": identity.identity,
            "repos": assigned_repos,
            "context": context_entries,
        });

        write_local_config_value(&replica_dir.join("config.json"), replica_config)?;
    }

    // Clone repos (async, partial failures logged but don't rollback)
    let mut clone_errors: Vec<CloneError> = Vec::new();
    for (url, dir_name) in &unique_repos {
        let target = wg_dir.join(dir_name);
        match git_clone_async(url, &target).await {
            Ok(_) => {
                log::info!("[entity_creation] Cloned {} → {}", url, target.display());
            }
            Err(e) => {
                log::error!("[entity_creation] Failed to clone {}: {}", url, e);
                clone_errors.push(CloneError {
                    url: url.clone(),
                    error: e,
                });
            }
        }
    }

    let result_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&wg_dir);
    log::info!(
        "[entity_creation] Created workgroup: {} ({} clone errors)",
        result_path,
        clone_errors.len()
    );
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(WorkgroupCloneResult {
        path: result_path,
        clone_errors,
    })
}

/// Delete a team directory from {project_path}/.ac/_team_{name}/
#[tauri::command]
pub async fn delete_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
) -> Result<(), String> {
    validate_existing_name(&team_name, "Team")?;
    let base = selected_workspace_dir(Path::new(&project_path))?;

    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    // Collect associated workgroup dirs (wg-N-{team_name}/)
    let wg_suffix = format!("-{}", team_name);
    let mut wg_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("wg-") && name_str.ends_with(&wg_suffix) {
                let middle = &name_str[3..name_str.len() - wg_suffix.len()];
                if middle.parse::<u32>().is_ok() {
                    wg_dirs.push(entry.path());
                }
            }
        }
    }

    // Check workgroup repos for dirty git state before deleting
    let dirty_repos = check_workgroup_repos_dirty(&wg_dirs);
    if !dirty_repos.is_empty() {
        let list = dirty_repos
            .iter()
            .map(|(repo, reason)| format!("  - {} ({})", repo, reason))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "Cannot delete team: the following repos have pending work:\n{}\n\nCommit or push changes before deleting.",
            list
        ));
    }

    // Delete team dir first — bail before touching workgroups if this fails
    std::fs::remove_dir_all(&team_dir)
        .map_err(|e| format!("Failed to delete team directory: {}", e))?;
    log::info!("[entity_creation] Deleted team: {}", team_name);

    // (#621) Count clock keys removed across the cascade; persist once after the loop.
    let mut team_clock_removed = 0usize;
    // Then delete workgroups
    for wg_dir in &wg_dirs {
        let wg_name = wg_dir.file_name().unwrap_or_default().to_string_lossy();
        if let Err(e) = std::fs::remove_dir_all(wg_dir) {
            log::warn!(
                "[entity_creation] Failed to delete workgroup {}: {}",
                wg_name,
                e
            );
        } else {
            log::info!("[entity_creation] Deleted workgroup: {}", wg_name);
            // (#621) Drop this workgroup's coordinator_clocks keys (in-memory only;
            // one persist after the loop, see below).
            if let Some(project_name) = Path::new(&project_path)
                .file_name()
                .and_then(|n| n.to_str())
            {
                if let Some(clocks) =
                    app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
                {
                    team_clock_removed += clocks
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove_workgroup(project_name, wg_name.as_ref());
                }
            }
        }
    }
    // (#621 MED-2) Single persist for the whole cascade (avoids N concurrent saves).
    if team_clock_removed > 0 {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
            if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
                log::warn!("[delete-team] clocks save failed: {}", e);
            }
        }
    }
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(())
}

/// Delete a single workgroup directory from {project_path}/.ac/{wg_name}/
/// Returns dirty repo list as an Err if any repos have uncommitted/unpushed work.
/// Pass `force = true` to skip the dirty-repo safety check (user already confirmed).
#[tauri::command]
pub async fn delete_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    workgroup_name: String,
    force: Option<bool>,
) -> Result<(), String> {
    validate_existing_name(&workgroup_name, "Workgroup")?;

    let base = selected_workspace_dir(Path::new(&project_path))?;

    let wg_dir = base.join(&workgroup_name);
    validate_delete_root_not_link_or_reparse(&wg_dir)?;

    ensure_no_live_sessions_under_manager(&wg_dir, session_mgr.inner()).await?;

    // Safety check: detect dirty repos before deleting (skip if force)
    if !force.unwrap_or(false) {
        let dirty_repos = check_workgroup_repos_dirty(std::slice::from_ref(&wg_dir));
        if !dirty_repos.is_empty() {
            let list = dirty_repos
                .iter()
                .map(|(repo, reason)| format!("  - {} ({})", repo, reason))
                .collect::<Vec<_>>()
                .join("\n");
            // DIRTY_REPOS: prefix is a sentinel the frontend uses to detect this error type
            return Err(format!(
                "DIRTY_REPOS:Cannot delete workgroup: the following repos have pending work:\n{}\n\nCommit or push changes before deleting.",
                list
            ));
        }
    }

    // Preflight rename probe (#113 follow-up): try an atomic same-parent rename
    // BEFORE remove_dir_all. NTFS rename requires DELETE access on every open
    // handle to the dir or any descendant; if any blocker holds a handle without
    // FILE_SHARE_DELETE (terminal cwd, VSCode workspace open, file watcher,
    // memory-mapped TASK.md), the rename fails atomically — no files touched —
    // and we run the diagnostic on the still-intact tree. On success the dir is
    // re-parented to a sentinel name and removed; the user-visible WG is gone.
    delete_workgroup_dir_backend(&wg_dir, &workgroup_name, session_mgr.inner()).await?;
    // (#621) Drop the workgroup's coordinator_clocks keys from the in-memory store
    // and persist immediately (this command is not on the auto-close flush tick).
    if let Some(project_name) = Path::new(&project_path)
        .file_name()
        .and_then(|n| n.to_str())
    {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            crate::config::coordinator_clocks::remove_workgroup_in_state(
                clocks.inner(),
                project_name,
                &workgroup_name,
            );
        }
    }
    log::info!(
        "[entity_creation] Deleted workgroup: {} (force={})",
        workgroup_name,
        force.unwrap_or(false)
    );
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(())
}

pub(crate) async fn delete_workgroup_dir_backend(
    wg_dir: &Path,
    workgroup_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    delete_workgroup_dir_backend_with_outcome(
        wg_dir,
        workgroup_name,
        session_mgr,
        try_atomic_delete_wg(wg_dir),
    )
    .await
}

pub(crate) async fn delete_workgroup_dir_backend_with_outcome(
    wg_dir: &Path,
    workgroup_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    outcome: WgDeleteOutcome,
) -> Result<(), String> {
    match outcome {
        WgDeleteOutcome::Deleted => {
            // fall through to success path
        }
        WgDeleteOutcome::Blocked(e) => {
            let raw = e.to_string();
            log::info!(
                "[entity_creation] delete_workgroup: file-in-use detected for '{}' on rename probe, running blocker diagnostic on intact tree",
                workgroup_name
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_blockers(
                wg_dir,
                workgroup_name,
                &raw, // raw OS error verbatim — see plan §C.1
                session_mgr,
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!(
                    "Failed to serialize blocker report: {}; original error: {}",
                    se, raw
                )
            })?;
            return Err(format!("BLOCKERS:{}", json));
        }
        WgDeleteOutcome::Partial { orphan_path, error } => {
            return Err(format!(
                "Partial workgroup delete: renamed '{}' to orphan '{}', but failed to remove orphan: {}",
                wg_dir.display(),
                orphan_path.display(),
                error
            ));
        }
        WgDeleteOutcome::Other(e) => {
            return Err(format!("Failed to delete workgroup directory: {}", e));
        }
    }
    Ok(())
}

/// Update an existing team's config.json in {project_path}/.ac/_team_{name}/
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn update_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    git_watcher: State<'_, Arc<GitWatcher>>,
    discovery_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
    project_path: String,
    team_name: String,
    agents: Vec<String>,
    coordinator: String,
    repos: Vec<RepoAssignment>,
) -> Result<(), String> {
    validate_existing_name(&team_name, "Team")?;
    let base = selected_workspace_dir(Path::new(&project_path))?;

    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    let config = normalize_team_config_for_project(
        &base,
        &TeamConfigResult {
            agents,
            coordinator,
            repos,
        },
    )?;
    write_team_config(&base, &team_name, &config)?;

    log::info!("[entity_creation] Updated team: {}", team_name);

    // Propagate repo changes to existing workgroups (async now — awaits SessionManager refresh).
    match sync_workgroup_repos_inner(
        &base,
        &team_name,
        &config.repos,
        session_mgr.inner(),
        git_watcher.inner(),
        discovery_watcher.inner(),
        &app,
    )
    .await
    {
        Ok(result) => {
            log::info!(
                "[entity_creation] Synced {} workgroups, {} replicas for team '{}' ({} errors)",
                result.workgroups_updated,
                result.replicas_updated,
                team_name,
                result.errors.len()
            );
        }
        Err(e) => {
            log::warn!("[entity_creation] Failed to sync workgroup repos: {}", e);
            // Non-fatal: team config was saved successfully
        }
    }

    // Refresh coordinator flags — a team edit can add/remove the coordinator or change its target.
    emit_coordinator_refresh(&app, session_mgr.inner()).await;

    Ok(())
}

/// Canonicalize an absolute or relative repo path and derive (label, absolute_path).
/// Mirrors ac_discovery.rs's source_path production so `Vec<SessionRepo>` equality
/// between the two writers holds (order and path shape both matter).
fn build_session_repo(replica_dir: &Path, rel: &str) -> Option<SessionRepo> {
    let resolved = replica_dir.join(rel);
    let abs = std::fs::canonicalize(&resolved).ok()?;
    let source_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&abs);
    let dir = source_path
        .replace('\\', "/")
        .split('/')
        .next_back()
        .unwrap_or("")
        .to_string();
    let label = dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
    Some(SessionRepo {
        label,
        source_path,
        branch: None,
    })
}

/// Core sync logic — updates repos and context in all replica configs for a team's workgroups.
/// After successful per-replica writes, pushes the new `git_repos` to any matching live session
/// via `refresh_git_repos_for_sessions` + watcher cache invalidation + `session_git_repos` emit.
/// Async so it can await the RwLock on `SessionManager`.
async fn sync_workgroup_repos_inner(
    base: &Path,
    team_name: &str,
    repos: &[RepoAssignment],
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    git_watcher: &Arc<GitWatcher>,
    discovery_watcher: &Arc<DiscoveryBranchWatcher>,
    app: &AppHandle,
) -> Result<SyncResult, String> {
    let mut result = SyncResult {
        workgroups_updated: 0,
        replicas_updated: 0,
        errors: Vec::new(),
    };

    // `updates` is built ONLY from replicas whose config.json write succeeded
    // (Grinch #15 partial-failure filter). In-memory state must match on-disk.
    let mut updates: Vec<(String, Vec<SessionRepo>)> = Vec::new();
    // Replica paths touched successfully — used for `invalidate_replicas` so the next
    // discovery poll re-registers them with fresh data (§3.2.5 / Grinch #17).
    let mut touched_replica_paths: Vec<String> = Vec::new();

    // Find all workgroups for this team (same discovery as delete_team())
    let wg_suffix = format!("-{}", team_name);
    let mut wg_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("wg-") && name_str.ends_with(&wg_suffix) {
                let middle = &name_str[3..name_str.len() - wg_suffix.len()];
                if middle.parse::<u32>().is_ok() {
                    wg_dirs.push(entry.path());
                }
            }
        }
    }

    for wg_dir in &wg_dirs {
        let mut wg_touched = false;
        let wg_name = wg_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // List __agent_* directories in this workgroup
        let replica_dirs: Vec<PathBuf> = match std::fs::read_dir(wg_dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| {
                    e.path().is_dir() && e.file_name().to_string_lossy().starts_with("__agent_")
                })
                .map(|e| e.path())
                .collect(),
            Err(e) => {
                log::warn!("Failed to read workgroup dir {}: {}", wg_dir.display(), e);
                continue;
            }
        };

        for replica_dir in &replica_dirs {
            let dir_name = replica_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // __agent_dev-rust → dev-rust
            let replica_name = dir_name.strip_prefix("__agent_").unwrap_or(dir_name);

            // Compute assigned repos (relative strings, written to config.json).
            let assigned_repos: Vec<String> = repos
                .iter()
                .filter(|r| r.agents.iter().any(|a| agent_matches(a, replica_name)))
                .map(|r| {
                    let d = format!("repo-{}", repo_dir_name_from_url(&r.url));
                    format!("../{}", d)
                })
                .collect();

            // Read existing config, preserving identity/tooling/other runtime fields
            let config_path = replica_dir.join("config.json");
            let mut repaired_identity = None;
            let write_result = crate::config::local_config_io::update_config_json_object(
                &config_path,
                false,
                |obj| {
                    let mut config = serde_json::Value::Object(std::mem::take(obj));
                    let identity = repair_wg_replica_config_value(
                        replica_dir,
                        &mut config,
                        WG_REPLICA_REQUIRED_CONTEXT,
                    )?;

                    // Update repos
                    config["repos"] = serde_json::json!(assigned_repos);

                    // Context merge: prepend required tokens to maintain consistent ordering
                    // with create_workgroup() (which writes [$AC_CONTEXT, $REPOS_INFO] first).
                    // Preserve custom non-Role entries while replacing identity-derived Role.md
                    // entries with the repaired same-workspace identity.
                    let existing_context: Vec<String> = config
                        .get("context")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();

                    config["context"] = serde_json::json!(normalize_wg_replica_context_entries(
                        &existing_context,
                        &["$AGENTSCOMMANDER_CONTEXT"],
                        &identity.identity,
                        identity.matrix_dir.join(ROLE_MD_FILENAME).exists(),
                    ));

                    let final_obj = config.as_object_mut().ok_or_else(|| {
                        format!(
                            "Replica config {} must be a JSON object",
                            config_path.display()
                        )
                    })?;
                    *obj = std::mem::take(final_obj);
                    repaired_identity = Some(identity);
                    Ok(())
                },
            );

            let _identity = match write_result {
                Ok(_) => repaired_identity.expect("identity repaired before successful write"),
                Err(e) => {
                    result.errors.push(SyncError {
                        replica: dir_name.to_string(),
                        error: e,
                    });
                    continue;
                }
            };

            // Write succeeded — record for in-memory refresh. Canonicalize each repo
            // path so source_path matches DiscoveryBranchWatcher's shape. Order of
            // `assigned_repos` = team config `repos` order, preserved via the filter
            // above — do NOT sort or dedupe.
            let session_repos: Vec<SessionRepo> = assigned_repos
                .iter()
                .filter_map(|rel| build_session_repo(replica_dir, rel))
                .collect();
            let session_name = format!("{}/{}", wg_name, replica_name);
            updates.push((session_name, session_repos));
            touched_replica_paths.push(replica_dir.to_string_lossy().to_string());

            result.replicas_updated += 1;
            wg_touched = true;
        }

        if wg_touched {
            result.workgroups_updated += 1;
        }
    }

    if !result.errors.is_empty() {
        log::warn!(
            "[entity_creation] sync_workgroup_repos for '{}': {} replicas updated, {} errors",
            team_name,
            result.replicas_updated,
            result.errors.len()
        );
    }

    // Refresh live sessions' git_repos in-memory so the sidebar updates before the next
    // discovery poll. CAS-guarded via git_repos_gen bump (Grinch #14 race fix).
    if !updates.is_empty() {
        let changed = {
            let mgr = session_mgr.read().await;
            mgr.refresh_git_repos_for_sessions(&updates).await
        };

        // Force DiscoveryBranchWatcher to re-register these replicas with fresh data
        // on the next `discover_project` call (§3.2.5 / Grinch #17).
        discovery_watcher.invalidate_replicas(&touched_replica_paths);

        for (session_id, repos) in changed {
            // Clear GitWatcher's cache slot so the next tick re-emits with detected branches.
            git_watcher.invalidate_session_cache(session_id);
            let _ = app.emit(
                "session_git_repos",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "repos": repos,
                }),
            );
        }
    }

    Ok(result)
}

/// Sync repo assignments and context tokens from team config to all existing workgroup replicas.
#[tauri::command]
pub async fn sync_workgroup_repos(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    git_watcher: State<'_, Arc<GitWatcher>>,
    discovery_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
    project_path: String,
    team_name: String,
) -> Result<SyncResult, String> {
    validate_existing_name(&team_name, "Team")?;

    let base = selected_workspace_dir(Path::new(&project_path))?;

    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    let repos = read_team_config(&base, &team_name)?.repos;

    sync_workgroup_repos_inner(
        &base,
        &team_name,
        &repos,
        session_mgr.inner(),
        git_watcher.inner(),
        discovery_watcher.inner(),
        &app,
    )
    .await
}

/// Refresh `is_coordinator` on every live session and emit `session_coordinator_changed`
/// for those whose flag flipped. Called by team-CRUD commands (§2).
pub(crate) async fn emit_coordinator_refresh(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) {
    let teams = crate::config::teams::discover_teams();
    let changes = {
        let mgr = session_mgr.read().await;
        mgr.refresh_coordinator_flags(&teams).await
    };
    for (id, is_coord) in changes {
        let _ = app.emit(
            "session_coordinator_changed",
            CoordinatorChangedPayload {
                session_id: id.to_string(),
                is_coordinator: is_coord,
            },
        );
    }
}

/// Read a team's config.json and return its contents.
#[tauri::command]
pub async fn get_team_config(
    project_path: String,
    team_name: String,
) -> Result<TeamConfigResult, String> {
    validate_existing_name(&team_name, "Team")?;
    let base = selected_workspace_dir(Path::new(&project_path))?;
    read_team_config(&base, &team_name)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check all repo-* dirs inside the given workgroup dirs for dirty git state.
/// Returns a list of (repo_display_name, reason) for repos with pending work.
pub(crate) fn check_workgroup_repos_dirty(wg_dirs: &[PathBuf]) -> Vec<(String, String)> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut dirty: Vec<(String, String)> = Vec::new();

    for wg_dir in wg_dirs {
        let wg_name = wg_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let entries = match std::fs::read_dir(wg_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name();
            let dir_name_str = dir_name.to_string_lossy();
            if !dir_name_str.starts_with("repo-") {
                continue;
            }
            if !path.join(".git").exists() {
                continue;
            }

            let display = format!("{}/{}", wg_name, dir_name_str);
            let mut reasons: Vec<&str> = Vec::new();

            // Check for uncommitted changes (staged + unstaged + untracked)
            let mut cmd = std::process::Command::new("git");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
            cmd.args(["status", "--porcelain"])
                .current_dir(&path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            if let Ok(output) = cmd.output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.trim().is_empty() {
                    reasons.push("uncommitted changes");
                }
            }

            // Check for unpushed commits
            let mut cmd2 = std::process::Command::new("git");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd2);
            cmd2.args(["log", "@{upstream}..HEAD", "--oneline"])
                .current_dir(&path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                cmd2.creation_flags(CREATE_NO_WINDOW);
            }
            match cmd2.output() {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if !stdout.trim().is_empty() {
                        reasons.push("unpushed commits");
                    }
                }
                _ => {
                    // No upstream configured — local-only branch = unpushed work
                    reasons.push("no remote upstream");
                }
            }

            if !reasons.is_empty() {
                dirty.push((display, reasons.join(", ")));
            }
        }
    }

    dirty
}

/// Result of a preflight-rename `delete_workgroup` attempt.
///
/// `pub(crate)` so the unit tests can pattern-match on the variants.
pub(crate) enum WgDeleteOutcome {
    /// Rename succeeded and the renamed dir was removed.
    Deleted,
    /// Rename succeeded, but deleting the renamed orphan failed. The
    /// user-visible workgroup name is gone, but callers must report this as a
    /// partial failure and skip refresh.
    Partial {
        orphan_path: PathBuf,
        error: std::io::Error,
    },
    /// Rename failed with a Windows file-in-use error. Tree is intact; caller
    /// should run the blocker diagnostic and return `BLOCKERS:` to the frontend.
    Blocked(std::io::Error),
    /// Rename failed with any other error (NotFound, permission, invalid path,
    /// …). Caller passes the raw error through unchanged.
    Other(std::io::Error),
}

/// Atomically detect blockers before deleting a workgroup directory.
///
/// Strategy: rename the WG dir to a unique sentinel name in the same parent
/// (NTFS metadata-only operation, fails atomically if any handle blocks it),
/// then `remove_dir_all` the renamed dir. If rename fails with a file-in-use
/// error the WG is still intact, so the caller can run the diagnostic over the
/// original tree and surface a `BLOCKERS:` report.
///
/// Suffix scheme: `.deleting-<wg_name>-<uuid>` — leading `.` keeps any orphan
/// (rare race: rename succeeds but remove_dir_all fails) invisible to the
/// `starts_with("wg-")` filters in `ac_discovery` and `cli::list_peers`, so an
/// orphan won't surface as a ghost workgroup. UUID is
/// used (already in `Cargo.toml`) to guarantee uniqueness across rapid retries.
///
/// `pub(crate)` so unit tests can drive it directly.
pub(crate) fn try_atomic_delete_wg(wg_dir: &Path) -> WgDeleteOutcome {
    try_atomic_delete_wg_with_remove(wg_dir, |path| std::fs::remove_dir_all(path))
}

pub(crate) fn try_atomic_delete_wg_with_remove(
    wg_dir: &Path,
    remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()>,
) -> WgDeleteOutcome {
    let parent = match wg_dir.parent() {
        Some(p) => p,
        None => {
            return WgDeleteOutcome::Other(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "workgroup directory has no parent",
            ));
        }
    };
    let original_name = match wg_dir.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => {
            return WgDeleteOutcome::Other(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "workgroup directory has no filename",
            ));
        }
    };
    let temp_name = format!(".deleting-{}-{}", original_name, uuid::Uuid::new_v4());
    let temp_path = parent.join(&temp_name);

    match std::fs::rename(wg_dir, &temp_path) {
        Ok(()) => {
            if let Err(e) = remove_dir_all(&temp_path) {
                log::warn!(
                    "[entity_creation] Renamed workgroup '{}' to '{}' but remove_dir_all failed: {}. \
                     User-visible WG is gone; orphan remains on disk.",
                    wg_dir.display(),
                    temp_path.display(),
                    e
                );
                return WgDeleteOutcome::Partial {
                    orphan_path: temp_path,
                    error: e,
                };
            }
            WgDeleteOutcome::Deleted
        }
        Err(e) => {
            if is_rename_blocked_by_handle(&e) {
                WgDeleteOutcome::Blocked(e)
            } else {
                WgDeleteOutcome::Other(e)
            }
        }
    }
}

#[derive(Debug)]
enum AgentDeleteStageError {
    Blocked {
        target: AgentDeleteTarget,
        raw_error: String,
    },
    Other(String),
    RollbackFailed(String),
}

async fn stage_agent_delete_targets(
    plan: &AgentDeletePlan,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<Vec<StagedAgentDeleteTarget>, String> {
    match stage_agent_delete_targets_with_rename(
        plan,
        |from: &Path, to: &Path| std::fs::rename(from, to),
        session_mgr,
    ) {
        Ok(staged) => Ok(staged),
        Err(AgentDeleteStageError::Blocked { target, raw_error }) => {
            log::info!(
                "[entity_creation] delete_agent_matrix: file-in-use detected for '{}' on rename probe",
                target.label
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_delete_root_blockers(
                &target.original_path,
                &target.label,
                &raw_error,
                session_mgr,
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!(
                    "Failed to serialize blocker report: {}; original error: {}",
                    se, raw_error
                )
            })?;
            Err(format!("BLOCKERS:{}", json))
        }
        Err(AgentDeleteStageError::Other(e)) | Err(AgentDeleteStageError::RollbackFailed(e)) => {
            Err(e)
        }
    }
}

fn stage_agent_delete_targets_with_rename<R>(
    plan: &AgentDeletePlan,
    mut rename: R,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<Vec<StagedAgentDeleteTarget>, AgentDeleteStageError>
where
    R: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    let mut staged = Vec::new();
    for target in &plan.targets {
        let parent = target.original_path.parent().ok_or_else(|| {
            AgentDeleteStageError::Other(format!(
                "Agent delete target '{}' has no parent",
                target.original_path.display()
            ))
        })?;
        let original_name = target
            .original_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                AgentDeleteStageError::Other(format!(
                    "Agent delete target '{}' has no filename",
                    target.original_path.display()
                ))
            })?;
        let staged_path = parent.join(format!(
            ".deleting-{}-{}",
            original_name,
            uuid::Uuid::new_v4()
        ));

        match rename(&target.original_path, &staged_path) {
            Ok(()) => staged.push(StagedAgentDeleteTarget {
                original_path: target.original_path.clone(),
                original_key: target.original_key.clone(),
                staged_path,
                label: target.label.clone(),
            }),
            Err(e) => {
                let raw_error = e.to_string();
                let blocked = is_rename_blocked_by_handle(&e);
                let rollback = rollback_staged_agent_delete_targets_with_rename(&staged, rename);
                if let Err(rollback_err) = rollback {
                    return Err(AgentDeleteStageError::RollbackFailed(
                        format_stage_rollback_failed(&staged, &raw_error, rollback_err),
                    ));
                }
                if blocked {
                    return Err(AgentDeleteStageError::Blocked {
                        target: target.clone(),
                        raw_error,
                    });
                }
                return Err(AgentDeleteStageError::Other(format!(
                    "Failed to stage agent delete target '{}': {}",
                    target.original_path.display(),
                    raw_error
                )));
            }
        }
    }
    Ok(staged)
}

fn rollback_staged_agent_delete_targets(staged: &[StagedAgentDeleteTarget]) -> Result<(), String> {
    rollback_staged_agent_delete_targets_with_rename(staged, |from: &Path, to: &Path| {
        std::fs::rename(from, to)
    })
}

fn rollback_staged_agent_delete_targets_with_rename<R>(
    staged: &[StagedAgentDeleteTarget],
    mut rename: R,
) -> Result<(), String>
where
    R: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    let mut errors = Vec::new();
    for target in staged.iter().rev() {
        if let Err(e) = rename(&target.staged_path, &target.original_path) {
            errors.push(format!(
                "{} [{}] {} -> {}: {}",
                target.label,
                target.original_key,
                target.staged_path.display(),
                target.original_path.display(),
                e
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Hidden cleanup dirs remain after rollback failure: {}",
            errors.join("; ")
        ))
    }
}

fn remove_staged_agent_delete_targets(staged: &[StagedAgentDeleteTarget]) -> Result<(), String> {
    let mut errors = Vec::new();
    for target in staged {
        if let Err(e) = std::fs::remove_dir_all(&target.staged_path) {
            errors.push(format!(
                "{} [{}] {}: {}",
                target.label,
                target.original_key,
                target.staged_path.display(),
                e
            ));
        } else {
            log::debug!(
                "[entity_creation] Removed staged agent delete target '{}' ({})",
                target.label,
                target.original_key
            );
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Agent was removed, but hidden cleanup dir(s) remain: {}",
            errors.join("; ")
        ))
    }
}

fn format_stage_rollback_failed(
    staged: &[StagedAgentDeleteTarget],
    original_error: &str,
    rollback_error: String,
) -> String {
    let hidden_paths = staged
        .iter()
        .map(|target| target.staged_path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Agent delete staging failed and rollback also failed. Hidden cleanup dirs remain: {}. Original error: {}. Rollback error: {}",
        hidden_paths, original_error, rollback_error
    )
}

pub(crate) fn validate_delete_root_not_link_or_reparse(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("delete_root_not_directory".to_string());
        }
        Err(e) => return Err(format!("delete_root_metadata_failed: {e}")),
    };

    if delete_root_has_windows_reparse_point(&metadata) {
        return Err("delete_root_is_reparse_point".to_string());
    }
    if metadata.file_type().is_symlink() {
        return Err("delete_root_is_symlink".to_string());
    }
    if !metadata.is_dir() {
        return Err("delete_root_not_directory".to_string());
    }
    Ok(())
}

async fn ensure_no_live_sessions_under_manager(
    root: &Path,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    let root_key = path_key_for_delete(root);
    let sessions = { session_mgr.read().await.list_sessions().await };
    let blockers: Vec<_> = sessions
        .into_iter()
        .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
        .filter(|session| {
            let working_dir = Path::new(&session.working_directory);
            let working_key = path_key_for_delete(working_dir);
            working_key == root_key || working_key.starts_with(&(root_key.clone() + "/"))
        })
        .collect();

    if blockers.is_empty() {
        return Ok(());
    }

    let details = blockers
        .iter()
        .map(|session| {
            format!(
                "  - {} at {} ({:?})",
                session.name, session.working_directory, session.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "Cannot delete while live sessions exist under {}:\n{}",
        root.display(),
        details
    ))
}

fn path_key_for_delete(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path_key_for_delete_without_canonicalize(&resolved)
}

fn path_key_for_delete_without_canonicalize(path: &Path) -> String {
    let text = crate::path_utils::path_to_string_without_windows_verbatim_prefix(path);
    text.replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn target_keys_for_delete(path: &Path) -> Vec<String> {
    let canonical_key = path_key_for_delete(path);
    let raw_key = path_key_for_delete_without_canonicalize(path);
    if canonical_key == raw_key {
        vec![canonical_key]
    } else {
        vec![canonical_key, raw_key]
    }
}

#[cfg(target_os = "windows")]
fn delete_root_has_windows_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn delete_root_has_windows_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// True iff the rename-probe error indicates a blocker holds an open handle.
///
/// Superset of `is_file_in_use_error`: matches the same {32, 33, 1224} codes
/// PLUS `ERROR_ACCESS_DENIED` (5). Empirically `MoveFileEx` (and therefore
/// `std::fs::rename`) returns 5, not 32, when an existing open handle on the
/// source's descendant lacks `FILE_SHARE_DELETE` — the most common real-world
/// blocker shape (default-share opens by IDEs and terminals). This is the
/// rename-path counterpart to `is_file_in_use_error`, which was tuned for
/// `remove_dir_all` semantics where ACCESS_DENIED typically means a real
/// permission failure (read-only file) rather than a share-mode mismatch.
///
/// `pub(crate)` so unit tests can exercise it without going through `try_atomic_delete_wg`.
pub(crate) fn is_rename_blocked_by_handle(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        const ERROR_ACCESS_DENIED: i32 = 5;
        if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
            return true;
        }
        is_file_in_use_error(e)
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

/// True iff `e` represents a Windows "file in use" error.
///
/// Matches the Win32 codes that surface when another process holds an open or
/// memory-mapped handle to a file we tried to delete:
/// - `ERROR_SHARING_VIOLATION` (32) — standard open with a deny-share mode.
/// - `ERROR_LOCK_VIOLATION` (33) — byte-range lock collision.
/// - `ERROR_USER_MAPPED_FILE` (1224) — file is mapped into another process's address
///   space. This is the VSCode / IDE memory-mapped-I/O case and was the motivating
///   real-world scenario for the blocker diagnostic. See plan §6.1.
///
/// On non-Windows always returns false: Linux / macOS produce different error codes
/// for "directory not empty due to open file" and we don't run the Restart-Manager
/// diagnostic there.
///
/// `pub(crate)` so the unit test in `wg_delete_diagnostic::tests` can exercise it
/// without moving the test into this module.
pub(crate) fn is_file_in_use_error(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        const ERROR_SHARING_VIOLATION: i32 = 32;
        const ERROR_LOCK_VIOLATION: i32 = 33;
        const ERROR_USER_MAPPED_FILE: i32 = 1224;
        matches!(
            e.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION | ERROR_USER_MAPPED_FILE)
        )
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

/// Scan the selected Project AC Root for existing `wg-<N>-{team_name}/` dirs and return the
/// **lowest free positive integer** starting at 1.
///
/// Issue #177: previously this returned `max(existing) + 1`, which left
/// permanent gaps after a workgroup was destroyed. The new policy reuses
/// any freed numbers so the user-facing sequence stays compact.
///
/// Filtering rules:
/// - Only directories are considered (regular files are ignored).
/// - The directory name must match `wg-<positive digits>-<team>`.
/// - Team suffix is ignored for allocation, so workgroup numbers are unique
///   across the whole project.
///
/// Slot 1 is always reachable because the lowest-free search starts at
/// 1 (see the `find` call below); a stray `wg-0-{team}` directory ends
/// up in `taken` but is never tested by `find` and so cannot displace
/// slot 1.
///
/// Read-error degradation: if `std::fs::read_dir(workspace_dir)` fails
/// (permission denied, transient I/O, broken junction, path-too-long
/// on Windows), the function returns `1` as a graceful fallback. The
/// post-allocate `wg_dir.exists()` guard in `create_workgroup` will
/// surface the real condition as an "already exists" error if a
/// `wg-1-{team}` is in fact present; otherwise the slot-1 creation
/// succeeds with stale state. Surfacing the read error is tracked
/// separately and is out of scope for #177.
pub(crate) fn determine_next_wg_number(workspace_dir: &Path) -> u32 {
    let mut taken: HashSet<u32> = HashSet::new();

    if let Ok(entries) = std::fs::read_dir(workspace_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(rest) = name_str.strip_prefix("wg-") {
                if let Some((middle, team)) = rest.split_once('-') {
                    if team.is_empty() {
                        continue;
                    }
                    if let Ok(n) = middle.parse::<u32>() {
                        taken.insert(n);
                    }
                }
            }
        }
    }

    // Lowest free positive integer ≥ 1. The bounded `..=u32::MAX` form avoids
    // any iterator-overflow footgun in debug builds; `find` short-circuits at
    // the first miss so the actual cost is O(taken.len() + 1) in practice.
    // A `0` may end up in `taken` (from a stray `wg-0-{team}`) but is never
    // tested here — the search starts at 1, so slot 1 is always reachable.
    (1u32..=u32::MAX).find(|n| !taken.contains(n)).unwrap_or(1)
}

/// Async git clone with CREATE_NO_WINDOW on Windows.
async fn git_clone_async(url: &str, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const GIT_CLONE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
    const GIT_RESET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    let git_target = git_cli_path(target);
    let mut cmd = tokio::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    cmd.args(["-c", "core.longpaths=true", "clone", "--depth", "1", url])
        .arg(git_target.as_os_str());
    cmd.kill_on_drop(true);

    #[cfg(windows)]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = tokio::time::timeout(GIT_CLONE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            format!(
                "git clone timed out after {} seconds",
                GIT_CLONE_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("Failed to spawn git clone: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        // Cap error message length to avoid sending huge progress output to frontend
        let capped = if trimmed.len() > 512 {
            &trimmed[..512]
        } else {
            trimmed
        };
        return Err(format!("git clone failed: {}", capped));
    }

    if !target.join(".git").join("index").exists() {
        log::warn!(
            "[entity_creation] .git/index missing after clone for {}, running fallback git reset",
            url
        );
        let mut reset_cmd = tokio::process::Command::new("git");
        crate::pty::credentials::scrub_credentials_from_tokio_command(&mut reset_cmd);
        reset_cmd.args(["reset"]).current_dir(&git_target);
        reset_cmd.kill_on_drop(true);
        #[cfg(windows)]
        {
            #[allow(unused_imports)]
            use std::os::windows::process::CommandExt;
            reset_cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match tokio::time::timeout(GIT_RESET_TIMEOUT, reset_cmd.output()).await {
            Ok(Ok(output)) if output.status.success() => {}
            Ok(Ok(output)) => {
                log::warn!(
                    "[entity_creation] fallback git reset failed for {}: {}",
                    url,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Ok(Err(e)) => {
                log::warn!(
                    "[entity_creation] failed to spawn fallback git reset for {}: {}",
                    url,
                    e
                );
            }
            Err(_) => {
                log::warn!(
                    "[entity_creation] fallback git reset timed out after {} seconds for {}",
                    GIT_RESET_TIMEOUT.as_secs(),
                    url
                );
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
fn git_cli_path(path: &Path) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(path)
}

#[cfg(not(windows))]
fn git_cli_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    //! Tests for Agent Matrix/replica layout invariants, the preflight-rename
    //! `delete_workgroup` helper added in the #113 follow-up dispatch, plus
    //! the #107 helper `parse_task_title`.

    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering as TestOrdering};

    #[test]
    #[cfg(windows)]
    fn git_cli_path_strips_windows_verbatim_prefix() {
        assert_eq!(
            git_cli_path(Path::new(r"\\?\C:\tmp\repo-Hello-World")),
            PathBuf::from(r"C:\tmp\repo-Hello-World")
        );
        assert_eq!(
            git_cli_path(Path::new(r"\\?\UNC\server\share\repo-Hello-World")),
            PathBuf::from(r"\\server\share\repo-Hello-World")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn git_cli_path_preserves_non_windows_path() {
        let path = Path::new("/tmp/repo-Hello-World");
        assert_eq!(git_cli_path(path), path);
    }

    #[test]
    fn create_agent_matrix_layout_creates_root_and_canonical_subdirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let agent_dir = tmp.path().join("_agent_test");

        assert!(!agent_dir.exists(), "agent_dir must not exist before call");

        create_agent_matrix_layout(&agent_dir).expect("create_agent_matrix_layout");

        assert!(
            agent_dir.is_dir(),
            "helper must create the matrix root itself"
        );
        for canonical in &["memory", "plans", "skills"] {
            assert!(
                agent_dir.join(canonical).is_dir(),
                "expected canonical state dir {}/",
                canonical
            );
        }
        assert!(agent_dir.join("inbox").is_dir(), "expected inbox/");
        assert!(agent_dir.join("outbox").is_dir(), "expected outbox/");
    }

    #[test]
    fn create_agent_matrix_layout_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let agent_dir = tmp.path().join("_agent_test");

        create_agent_matrix_layout(&agent_dir).expect("first call");
        let sentinel = agent_dir.join("memory").join("sentinel.md");
        std::fs::write(&sentinel, b"x").expect("write sentinel");

        create_agent_matrix_layout(&agent_dir).expect("second call");
        assert!(
            sentinel.is_file(),
            "second call must not wipe pre-existing files in memory/"
        );
    }

    #[test]
    fn create_agent_matrix_on_disk_creates_full_layout_without_template() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let workspace = project.join(".ac");
        std::fs::create_dir_all(&workspace).expect("create .ac");
        let settings = AppSettings::default();
        let project_s = project.to_string_lossy().to_string();

        let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
            project_path: &project_s,
            name: "Architect",
            description: "Build plans",
            role_template_id: None,
            settings: &settings,
            config_dir: None,
        })
        .expect("create matrix");

        let agent_dir = workspace.join("_agent_architect");
        assert_eq!(created.agent_dir, agent_dir);
        assert_eq!(created.safe_name, "architect");
        assert_eq!(created.display_name, "ProjectAlpha/architect");
        assert!(agent_dir.join("Role.md").is_file());
        assert!(agent_dir.join("config.json").is_file());
        for canonical in AGENT_MATRIX_DIRS {
            assert!(
                agent_dir.join(canonical).is_dir(),
                "missing canonical dir {}",
                canonical
            );
        }
    }

    #[test]
    fn create_agent_matrix_on_disk_rejects_non_ac_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let invalid_workspace = project.join(".workspace");
        std::fs::create_dir_all(&invalid_workspace).expect("create invalid workspace");
        let settings = AppSettings::default();
        let project_s = project.to_string_lossy().to_string();

        let err = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
            project_path: &project_s,
            name: "Architect",
            description: "Build plans",
            role_template_id: None,
            settings: &settings,
            config_dir: None,
        })
        .unwrap_err();

        assert!(err.contains(".ac directory not found"));
        assert!(!invalid_workspace.join("_agent_architect").exists());
    }

    #[test]
    fn create_agent_matrix_on_disk_uses_ac_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let workspace = project.join(".ac");
        std::fs::create_dir_all(&workspace).expect("create .ac");
        let settings = AppSettings::default();
        let project_s = project.to_string_lossy().to_string();

        let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
            project_path: &project_s,
            name: "Architect",
            description: "Build plans",
            role_template_id: None,
            settings: &settings,
            config_dir: None,
        })
        .expect("create matrix");

        assert_eq!(created.agent_dir, workspace.join("_agent_architect"));
    }

    #[test]
    fn create_agent_matrix_on_disk_resolves_template_before_mutation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let workspace_dir = project.join(".ac");
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&workspace_dir).expect("create .ac");
        std::fs::create_dir_all(&config_dir).expect("create config");
        let settings = AppSettings::default();
        let project_s = project.to_string_lossy().to_string();

        let err = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
            project_path: &project_s,
            name: "Architect",
            description: "Build plans",
            role_template_id: Some("agency:not-real"),
            settings: &settings,
            config_dir: Some(&config_dir),
        })
        .expect_err("invalid template");

        assert!(err.contains("missing"));
        assert!(
            !workspace_dir.join("_agent_architect").exists(),
            "invalid template must not create the target matrix directory"
        );
    }

    #[test]
    fn create_agent_matrix_on_disk_existing_root_fails_without_overwrite() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let workspace_dir = project.join(".ac");
        let agent_dir = workspace_dir.join("_agent_architect");
        std::fs::create_dir_all(&agent_dir).expect("create existing matrix root");
        std::fs::write(agent_dir.join("Role.md"), "keep role").expect("write existing role");
        std::fs::write(agent_dir.join("config.json"), "keep config")
            .expect("write existing config");
        let settings = AppSettings::default();
        let project_s = project.to_string_lossy().to_string();

        let err = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
            project_path: &project_s,
            name: "Architect",
            description: "Build plans",
            role_template_id: None,
            settings: &settings,
            config_dir: None,
        })
        .expect_err("existing root");

        assert_eq!(err, "Agent 'architect' already exists");
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("Role.md")).expect("read existing role"),
            "keep role"
        );
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("config.json")).expect("read existing config"),
            "keep config"
        );
    }

    #[test]
    fn create_agent_matrix_on_disk_applies_local_template_and_skills() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let workspace_dir = project.join(".ac");
        let config_dir = tmp.path().join("config");
        let template_dir = config_dir.join("agent-templates").join("my-template");
        std::fs::create_dir_all(&workspace_dir).expect("create .ac");
        std::fs::create_dir_all(template_dir.join("skills").join("example"))
            .expect("create template skill dir");
        std::fs::write(
            template_dir.join("Role.md"),
            "---\nname: My Template\n---\n\n# Template Body\n\nUse this profile.\n",
        )
        .expect("write template role");
        std::fs::write(
            template_dir.join("skills").join("example").join("SKILL.md"),
            "# Skill\n",
        )
        .expect("write template skill");
        let settings = AppSettings::default();
        let project_s = project.to_string_lossy().to_string();

        let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
            project_path: &project_s,
            name: "Architect",
            description: "Build plans",
            role_template_id: Some("local:my-template"),
            settings: &settings,
            config_dir: Some(&config_dir),
        })
        .expect("create matrix from template");

        let role = std::fs::read_to_string(created.role_path).expect("read Role.md");
        assert!(role.contains("## Role Profile"));
        assert!(role.contains("# Template Body"));
        assert!(role.contains("Use this profile."));
        assert!(created
            .agent_dir
            .join("skills")
            .join("example")
            .join("SKILL.md")
            .is_file());
    }

    #[test]
    fn agent_matrix_dirs_contains_memory_plans_skills() {
        let names: std::collections::HashSet<&str> = AGENT_MATRIX_DIRS.iter().copied().collect();
        for required in &["memory", "plans", "skills"] {
            assert!(
                names.contains(required),
                "AGENT_MATRIX_DIRS must contain {} (issue #209)",
                required
            );
        }
    }

    #[test]
    fn default_agent_matrix_config_includes_context_and_role() {
        let config = default_agent_matrix_config();

        assert_eq!(config["tooling"], serde_json::json!({}));
        assert_eq!(
            config["context"],
            serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "Role.md"])
        );
    }

    #[test]
    fn team_config_normalization_stores_portable_agent_refs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace_dir = tmp.path().join(".ac");
        let architect = workspace_dir.join("_agent_architect");
        let dev_rust = workspace_dir.join("_agent_dev-rust");
        std::fs::create_dir_all(&architect).expect("architect matrix");
        std::fs::create_dir_all(&dev_rust).expect("dev-rust matrix");

        let config = TeamConfigResult {
            agents: vec![
                "architect".to_string(),
                "_agent_dev-rust".to_string(),
                "architect".to_string(),
            ],
            coordinator: "_agent_architect".to_string(),
            repos: vec![RepoAssignment {
                url: "https://example.test/repo.git".to_string(),
                agents: vec!["architect".to_string(), "_agent_dev-rust".to_string()],
            }],
        };

        let normalized =
            normalize_team_config_for_project(&workspace_dir, &config).expect("normalize config");
        assert_eq!(
            normalized.agents,
            vec![
                "_agent_architect".to_string(),
                "_agent_dev-rust".to_string()
            ]
        );
        assert_eq!(normalized.coordinator, "_agent_architect");
        assert_eq!(
            normalized.repos[0].agents,
            vec![
                "_agent_architect".to_string(),
                "_agent_dev-rust".to_string()
            ]
        );

        write_team_config(&workspace_dir, "dev-team", &config).expect("write config");
        let written =
            std::fs::read_to_string(workspace_dir.join("_team_dev-team").join("config.json"))
                .expect("read config");
        assert!(
            !written.contains(&tmp.path().to_string_lossy().to_string()),
            "team config must not persist absolute project paths: {}",
            written
        );
    }

    #[test]
    fn team_config_normalization_rejects_filesystem_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace_dir = tmp.path().join(".ac");
        std::fs::create_dir_all(workspace_dir.join("_agent_architect")).expect("architect matrix");

        let absolute_config = TeamConfigResult {
            agents: vec![workspace_dir
                .join("_agent_architect")
                .to_string_lossy()
                .to_string()],
            coordinator: "_agent_architect".to_string(),
            repos: Vec::new(),
        };
        let err = normalize_team_config_for_project(&workspace_dir, &absolute_config)
            .expect_err("absolute path refs must be rejected");
        assert!(
            err.contains("filesystem paths"),
            "unexpected error: {}",
            err
        );

        let windows_config = TeamConfigResult {
            agents: vec![r"C:\Users\maria\project\.ac\_agent_architect".to_string()],
            coordinator: "_agent_architect".to_string(),
            repos: Vec::new(),
        };
        let err = normalize_team_config_for_project(&workspace_dir, &windows_config)
            .expect_err("windows path refs must be rejected");
        assert!(
            err.contains("filesystem paths"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn replica_creation_rejects_filesystem_agent_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace_dir = tmp.path().join(".ac");
        let wg_dir = workspace_dir.join("wg-1-dev-team");
        let matrix_dir = workspace_dir.join("_agent_architect");
        std::fs::create_dir_all(&matrix_dir).expect("architect matrix");
        std::fs::create_dir_all(&wg_dir).expect("workgroup");

        let err = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
            workspace_dir: workspace_dir.clone(),
            wg_dir: wg_dir.clone(),
            agent_path: matrix_dir.to_string_lossy().to_string(),
            team_repos: Vec::new(),
        })
        .expect_err("absolute path refs must be rejected");

        assert!(
            err.contains("filesystem paths"),
            "unexpected error: {}",
            err
        );
        assert!(
            !wg_dir
                .join(format!("__agent_{}", matrix_dir.to_string_lossy()))
                .exists(),
            "replica creation must not create path-derived agent directories"
        );
    }

    #[test]
    fn create_agent_replica_layout_creates_only_inbox_outbox() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let replica_dir = tmp.path().join("__agent_test");

        assert!(
            !replica_dir.exists(),
            "replica_dir must not exist before call"
        );

        create_agent_replica_layout(&replica_dir).expect("create_agent_replica_layout");

        assert!(
            replica_dir.is_dir(),
            "helper must create the replica root itself"
        );
        assert!(replica_dir.join("inbox").is_dir(), "expected inbox/");
        assert!(replica_dir.join("outbox").is_dir(), "expected outbox/");
        for canonical in &["memory", "plans", "skills"] {
            assert!(
                !replica_dir.join(canonical).exists(),
                "replica MUST NOT have {}/; origin matrix is canonical",
                canonical
            );
        }
    }

    #[test]
    fn agent_matrix_and_replica_dir_sets_are_disjoint_on_canonical_state() {
        let canonical: std::collections::HashSet<&str> =
            ["memory", "plans", "skills"].into_iter().collect();
        let replica: std::collections::HashSet<&str> = AGENT_REPLICA_DIRS.iter().copied().collect();
        assert!(
            canonical.is_disjoint(&replica),
            "AGENT_REPLICA_DIRS must not include canonical state names; overlap: {:?}",
            canonical.intersection(&replica).collect::<Vec<_>>()
        );
    }

    #[test]
    fn replica_identity_resolves_to_origin_matrix_role_md() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace_dir = tmp.path().join(".ac");
        let matrix_dir = workspace_dir.join("_agent_alpha");
        let replica_dir = workspace_dir.join("wg-1-team").join("__agent_alpha");
        std::fs::create_dir_all(&matrix_dir).expect("create matrix_dir");
        std::fs::create_dir_all(&replica_dir).expect("create replica_dir");

        let matrix_role = matrix_dir.join("Role.md");
        std::fs::write(&matrix_role, b"# Alpha\n").expect("write matrix Role.md");

        let identity = crate::config::replica_identity::expected_wg_replica_identity(&replica_dir)
            .expect("expected replica identity")
            .identity;

        assert_eq!(
            identity, "../../_agent_alpha",
            "expected identity to traverse wg-1-team -> .ac -> _agent_alpha"
        );

        let resolved = replica_dir.join(&identity).join("Role.md");
        assert_eq!(
            resolved
                .canonicalize()
                .expect("canonicalize resolved Role.md"),
            matrix_role
                .canonicalize()
                .expect("canonicalize matrix Role.md"),
            "replica Role.md context entry must resolve to origin matrix Role.md"
        );
    }

    #[test]
    fn create_replica_rejects_filesystem_agent_ref() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("AgentsCommander_ac");
        let workspace = project.join(".ac");
        let matrix_dir = workspace.join("_agent_tech-lead");
        let wg_dir = workspace.join("wg-2-dev-team");
        std::fs::create_dir_all(&matrix_dir).expect("create matrix");
        std::fs::create_dir_all(&wg_dir).expect("create wg");
        std::fs::write(matrix_dir.join("Role.md"), "# Tech Lead\n").expect("write role");
        let stale_agent_ref = tmp
            .path()
            .join("agentscommander-old")
            .join(".ac")
            .join("_agent_tech-lead")
            .to_string_lossy()
            .replace('\\', "/");

        let err = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
            workspace_dir: workspace,
            wg_dir: wg_dir.clone(),
            agent_path: stale_agent_ref,
            team_repos: Vec::new(),
        })
        .expect_err("filesystem path refs must be rejected");

        assert!(
            err.contains("filesystem paths"),
            "unexpected error: {}",
            err
        );
        assert!(
            !wg_dir.join("__agent_tech-lead").exists(),
            "path refs must not create replicas"
        );
    }

    #[test]
    fn create_replica_writes_expected_local_identity_for_portable_ref() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("AgentsCommander_ac");
        let workspace = project.join(".ac");
        let matrix_dir = workspace.join("_agent_tech-lead");
        let wg_dir = workspace.join("wg-2-dev-team");
        std::fs::create_dir_all(&matrix_dir).expect("create matrix");
        std::fs::create_dir_all(&wg_dir).expect("create wg");
        std::fs::write(matrix_dir.join("Role.md"), "# Tech Lead\n").expect("write role");

        let replica_dir = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
            workspace_dir: workspace,
            wg_dir,
            agent_path: "_agent_tech-lead".to_string(),
            team_repos: Vec::new(),
        })
        .expect("create replica");

        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(replica_dir.join("config.json")).expect("read config"),
        )
        .expect("parse config");
        assert_eq!(config["identity"], "../../_agent_tech-lead");
    }

    /// Success path: a clean WG dir with no blockers gets renamed and removed.
    /// The original path must not exist after the call, and there must be no
    /// `.deleting-*` orphan left in the parent.
    #[test]
    fn try_atomic_delete_wg_removes_clean_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        std::fs::create_dir(&wg_dir).expect("create wg_dir");
        std::fs::write(wg_dir.join("TASK.md"), "# test\n").expect("write TASK.md");
        std::fs::create_dir(wg_dir.join("repo-foo")).expect("create repo-foo");
        std::fs::write(wg_dir.join("repo-foo").join("README.md"), "x").expect("write inside");

        let outcome = try_atomic_delete_wg(&wg_dir);
        assert!(
            matches!(outcome, WgDeleteOutcome::Deleted),
            "clean dir must report Deleted"
        );
        assert!(!wg_dir.exists(), "wg_dir must be gone after delete");

        // Parent must contain no `.deleting-*` orphan.
        let parent = tmp.path();
        let orphans: Vec<_> = std::fs::read_dir(parent)
            .expect("read tempdir")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".deleting-"))
            .collect();
        assert!(
            orphans.is_empty(),
            "no .deleting-* orphan should remain after a clean delete; found {:?}",
            orphans.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
    }

    /// Other-error path: deleting a nonexistent WG dir surfaces as
    /// `WgDeleteOutcome::Other` (NotFound), NOT as `Blocked`.
    #[test]
    fn try_atomic_delete_wg_classifies_missing_dir_as_other() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        let outcome = try_atomic_delete_wg(&missing);
        match outcome {
            WgDeleteOutcome::Other(e) => {
                assert_eq!(
                    e.kind(),
                    std::io::ErrorKind::NotFound,
                    "missing-dir error must classify as NotFound"
                );
            }
            WgDeleteOutcome::Blocked(_) => {
                panic!("missing dir must NOT classify as Blocked")
            }
            WgDeleteOutcome::Deleted => panic!("missing dir cannot be Deleted"),
            WgDeleteOutcome::Partial { .. } => panic!("missing dir cannot be Partial"),
        }
    }

    #[test]
    fn try_atomic_delete_wg_reports_partial_orphan_when_remove_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        std::fs::create_dir(&wg_dir).expect("create wg_dir");
        std::fs::write(wg_dir.join("TASK.md"), "# test\n").expect("write TASK.md");

        let outcome = try_atomic_delete_wg_with_remove(&wg_dir, |_path| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "forced remove failure",
            ))
        });
        match outcome {
            WgDeleteOutcome::Partial { orphan_path, error } => {
                assert!(!wg_dir.exists(), "original workgroup path should be gone");
                assert!(orphan_path.is_dir(), "orphan should remain on disk");
                assert!(orphan_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("orphan filename")
                    .starts_with(".deleting-wg-1-test-"));
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                assert!(error.to_string().contains("forced remove failure"));
            }
            WgDeleteOutcome::Deleted => panic!("remove failure cannot be Deleted"),
            WgDeleteOutcome::Blocked(e) => panic!("remove failure cannot be Blocked: {}", e),
            WgDeleteOutcome::Other(e) => panic!("remove failure cannot be Other: {}", e),
        }
    }

    #[tokio::test]
    async fn gui_partial_delete_outcome_returns_error_before_refresh() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        let orphan = tmp.path().join(".deleting-wg-1-test-forced");
        std::fs::create_dir(&wg_dir).expect("create wg_dir");
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

        let err = delete_workgroup_dir_backend_with_outcome(
            &wg_dir,
            "wg-1-test",
            &manager,
            WgDeleteOutcome::Partial {
                orphan_path: orphan.clone(),
                error: std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "forced remove failure",
                ),
            },
        )
        .await
        .expect_err("partial delete must error before caller can refresh");

        assert!(err.contains("Partial workgroup delete"));
        assert!(err.contains(&orphan.to_string_lossy().to_string()));
    }

    #[test]
    fn validate_delete_root_rejects_non_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("wg-1-test");
        std::fs::write(&file, "not a dir").expect("write file");

        assert_eq!(
            validate_delete_root_not_link_or_reparse(&file).unwrap_err(),
            "delete_root_not_directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn validate_delete_root_rejects_symlink_root() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let real = tmp.path().join("real");
        let link = tmp.path().join("wg-1-test");
        std::fs::create_dir(&real).expect("create real");
        symlink(&real, &link).expect("create symlink");

        assert_eq!(
            validate_delete_root_not_link_or_reparse(&link).unwrap_err(),
            "delete_root_is_symlink"
        );
    }

    #[cfg(windows)]
    #[test]
    fn validate_delete_root_rejects_reparse_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let real = tmp.path().join("real");
        let junction = tmp.path().join("wg-1-test");
        std::fs::create_dir(&real).expect("create real");
        let output = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                junction.to_str().expect("junction path"),
                real.to_str().expect("real path"),
            ])
            .output()
            .expect("run mklink");
        if !output.status.success() {
            println!(
                "skipping validate_delete_root reparse check; see docs/testing/destructive-filesystem-regression.md#helper-reparse-root-check: stdout: {} stderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        assert_eq!(
            validate_delete_root_not_link_or_reparse(&junction).unwrap_err(),
            "delete_root_is_reparse_point"
        );
    }

    #[tokio::test]
    async fn gui_live_session_refusal_precedes_delete() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        std::fs::create_dir(&wg_dir).expect("create wg_dir");
        let work_dir = wg_dir.join("__agent_dev-rust");
        std::fs::create_dir_all(&work_dir).expect("create work_dir");
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        {
            let guard = manager.read().await;
            guard
                .create_session(
                    "shell".to_string(),
                    Vec::new(),
                    work_dir.to_string_lossy().to_string(),
                    None,
                    None,
                    Vec::new(),
                    false,
                    crate::pty::backend::SessionBackendKind::LocalProcess,
                )
                .await
                .expect("create live session");
        }

        let err = ensure_no_live_sessions_under_manager(&wg_dir, &manager)
            .await
            .expect_err("live session should block delete");
        assert!(err.contains("Cannot delete while live sessions exist"));
        assert!(err.contains("Session 1"));
        assert!(err.contains("Active"));
    }

    fn create_test_workspace() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("Project").join(".ac")).expect("create .ac");
        tmp
    }

    fn test_base(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().join("Project").join(".ac")
    }

    fn create_test_agent(base: &Path, name: &str, role_display: &str) -> PathBuf {
        let agent_dir = base.join(format!("_agent_{}", name));
        std::fs::create_dir_all(&agent_dir).expect("create agent");
        std::fs::write(
            agent_dir.join("Role.md"),
            format!("---\nname: '{}'\n---\n", role_display),
        )
        .expect("write role");
        agent_dir
    }

    fn write_team_value(
        base: &Path,
        team_name: &str,
        value: serde_json::Value,
    ) -> (PathBuf, Vec<u8>) {
        let team_dir = base.join(format!("_team_{}", team_name));
        std::fs::create_dir_all(&team_dir).expect("create team");
        let config_path = team_dir.join("config.json");
        let mut bytes = serde_json::to_vec_pretty(&value).expect("serialize team");
        bytes.push(b'\n');
        std::fs::write(&config_path, &bytes).expect("write team config");
        (config_path, bytes)
    }

    fn settings_state(settings: AppSettings) -> SettingsState {
        Arc::new(tokio::sync::RwLock::new(settings))
    }

    fn profile_cell(command: &str) -> crate::config::settings::ProfileCellConfig {
        crate::config::settings::ProfileCellConfig {
            enabled: true,
            command: command.to_string(),
            env: BTreeMap::new(),
            notes: "keep".to_string(),
        }
    }

    fn test_manager() -> Arc<tokio::sync::RwLock<SessionManager>> {
        Arc::new(tokio::sync::RwLock::new(SessionManager::new()))
    }

    async fn add_live_session(
        manager: &Arc<tokio::sync::RwLock<SessionManager>>,
        working_dir: &Path,
    ) {
        let guard = manager.read().await;
        guard
            .create_session(
                "shell".to_string(),
                Vec::new(),
                working_dir.to_string_lossy().to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create live session");
    }

    async fn run_agent_delete_for_test(
        base: &Path,
        agent_path: &Path,
        settings: &SettingsState,
        manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    ) -> Result<PreparedAgentDeleteMetadata, String> {
        let plan = collect_agent_delete_plan(base, agent_path)?;
        preflight_agent_delete(&plan, manager).await?;
        let metadata = prepare_agent_delete_metadata(&plan, settings).await?;
        let staged = stage_agent_delete_targets(&plan, manager).await?;

        if let Err(live_err) =
            ensure_no_live_sessions_under_target_keys(&plan.target_keys, &plan.agent_name, manager)
                .await
        {
            let rollback = rollback_staged_agent_delete_targets(&staged);
            return Err(format_agent_delete_post_stage_live_failure(
                live_err, rollback,
            ));
        }

        if let Err(e) = persist_agent_delete_metadata_with_saver(&metadata, settings, |candidate| {
            Ok(candidate.clone())
        })
        .await
        {
            let restore = restore_agent_delete_metadata_snapshots(&metadata, settings).await;
            let rollback = rollback_staged_agent_delete_targets(&staged);
            return Err(format_agent_delete_metadata_failure(e, restore, rollback));
        }

        remove_staged_agent_delete_targets(&staged)?;
        Ok(metadata)
    }

    #[test]
    fn delete_agent_plan_uses_path_slug_not_role_display_name() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Displayed Rust Agent");

        let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");

        assert_eq!(plan.agent_name, "dev-rust");
        assert_eq!(plan.agent_ref, "_agent_dev-rust");
        assert_eq!(plan.origin_dir, agent_dir);
    }

    #[tokio::test]
    async fn delete_agent_unreferenced_origin_only_prunes_settings() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        let mut settings = AppSettings::default();
        settings
            .auto_self_clear_by_agent
            .insert("dev-rust".to_string(), true);
        settings
            .coding_agent_profiles
            .default_profile_by_agent
            .insert("dev-rust".to_string(), "A".to_string());
        let mut cells = BTreeMap::new();
        cells.insert("A".to_string(), profile_cell("codex"));
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .insert("dev-rust".to_string(), cells.clone());
        let mut labels = BTreeMap::new();
        labels.insert("A".to_string(), "Rust".to_string());
        settings
            .coding_agent_profiles
            .profile_labels_by_agent
            .insert("dev-rust".to_string(), labels.clone());
        let settings = settings_state(settings);
        let manager = test_manager();

        let metadata = run_agent_delete_for_test(&base, &agent_dir, &settings, &manager)
            .await
            .expect("delete agent");

        assert!(!agent_dir.exists());
        assert!(metadata.settings_changed.load(Ordering::SeqCst));
        let saved = settings.read().await;
        assert!(!saved.auto_self_clear_by_agent.contains_key("dev-rust"));
        assert!(!saved
            .coding_agent_profiles
            .default_profile_by_agent
            .contains_key("dev-rust"));
        assert_eq!(
            saved.coding_agent_profiles.profiles_by_agent["dev-rust"],
            cells
        );
        assert_eq!(
            saved.coding_agent_profiles.profile_labels_by_agent["dev-rust"],
            labels
        );
    }

    #[tokio::test]
    async fn delete_agent_cascade_removes_team_repo_refs_and_replicas() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        create_test_agent(&base, "architect", "Architect");
        let wg_dir = base.join("wg-1-dev-team");
        let replica = wg_dir.join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).expect("create replica");
        std::fs::create_dir_all(wg_dir.join("repo-service")).expect("create repo");
        std::fs::create_dir_all(wg_dir.join("messaging")).expect("create messaging");
        let config_path = write_team_value(
            &base,
            "dev-team",
            serde_json::json!({
                "agents": ["_agent_architect", "_agent_dev-rust", "../_agent_dev-rust"],
                "coordinator": "_agent_architect",
                "repos": [
                    { "url": "https://example.test/service.git", "agents": ["_agent_dev-rust", "_agent_architect"] },
                    { "url": "https://example.test/docs.git", "agents": ["../_agent_dev-rust"] }
                ]
            }),
        )
        .0;
        let settings = settings_state(AppSettings::default());
        let manager = test_manager();

        run_agent_delete_for_test(&base, &agent_dir, &settings, &manager)
            .await
            .expect("delete agent");

        assert!(!agent_dir.exists());
        assert!(!replica.exists());
        assert!(wg_dir.join("repo-service").is_dir());
        assert!(wg_dir.join("messaging").is_dir());
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("read config"))
                .expect("parse config");
        assert_eq!(config["agents"], serde_json::json!(["_agent_architect"]));
        assert_eq!(
            config["repos"][0]["agents"],
            serde_json::json!(["_agent_architect"])
        );
        assert_eq!(config["repos"][1]["agents"], serde_json::json!([]));
    }

    #[test]
    fn delete_agent_coordinator_blocks_and_touches_nothing() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        let (config_path, before) = write_team_value(
            &base,
            "dev-team",
            serde_json::json!({
                "agents": ["_agent_dev-rust"],
                "coordinator": "../_agent_dev-rust",
                "repos": []
            }),
        );

        let err = collect_agent_delete_plan(&base, &agent_dir).expect_err("coordinator blocks");

        assert!(err.contains("coordinator of team(s): dev-team"), "{err}");
        assert!(agent_dir.is_dir());
        assert_eq!(std::fs::read(&config_path).expect("read config"), before);
    }

    #[test]
    #[cfg(windows)]
    fn delete_agent_case_mismatched_coordinator_blocks_on_windows() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "DevRust", "Dev Rust");
        write_team_value(
            &base,
            "dev-team",
            serde_json::json!({
                "agents": ["_agent_devrust"],
                "coordinator": "_agent_devrust",
                "repos": []
            }),
        );

        let err = collect_agent_delete_plan(&base, &agent_dir).expect_err("coordinator blocks");

        assert!(err.contains("coordinator of team(s): dev-team"), "{err}");
    }

    #[test]
    #[cfg(windows)]
    fn delete_agent_case_mismatched_refs_are_pruned_on_windows() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "DevRust", "Dev Rust");
        let config_path = write_team_value(
            &base,
            "dev-team",
            serde_json::json!({
                "agents": ["_agent_devrust", "_agent_architect"],
                "coordinator": "_agent_architect",
                "repos": [{ "url": "u", "agents": ["_agent_devrust"] }]
            }),
        )
        .0;

        let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");
        write_team_config_json_atomic(&config_path, &plan.team_mutations[0].after_json)
            .expect("write mutation");

        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("read config"))
                .expect("parse config");
        assert_eq!(config["agents"], serde_json::json!(["_agent_architect"]));
        assert_eq!(config["repos"][0]["agents"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn delete_agent_unrelated_stale_team_ref_does_not_block() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-docs", "Dev Docs");
        write_team_value(
            &base,
            "dev-team",
            serde_json::json!({
                "agents": ["_agent_ghost", "_agent_dev-docs"],
                "coordinator": "_agent_ghost",
                "repos": [{ "url": "u", "agents": ["_agent_dev-docs", "_agent_ghost"] }]
            }),
        );
        let settings = settings_state(AppSettings::default());
        let manager = test_manager();

        run_agent_delete_for_test(&base, &agent_dir, &settings, &manager)
            .await
            .expect("delete with stale unrelated ref");

        assert!(!agent_dir.exists());
    }

    #[test]
    fn delete_agent_relevant_invalid_team_config_blocks_and_touches_nothing() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-docs", "Dev Docs");
        let team_dir = base.join("_team_dev-team");
        std::fs::create_dir_all(&team_dir).expect("create team");
        let config_path = team_dir.join("config.json");
        let before = b"{ invalid _agent_dev-docs".to_vec();
        std::fs::write(&config_path, &before).expect("write config");

        let err =
            collect_agent_delete_plan(&base, &agent_dir).expect_err("invalid relevant config");

        assert!(err.contains("cannot verify team 'dev-team'"), "{err}");
        assert!(agent_dir.is_dir());
        assert_eq!(std::fs::read(&config_path).expect("read config"), before);
    }

    #[test]
    fn delete_agent_invalid_team_config_uses_token_match_not_substring() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev", "Dev");
        let team_dir = base.join("_team_dev-team");
        std::fs::create_dir_all(&team_dir).expect("create team");
        std::fs::write(team_dir.join("config.json"), b"{ invalid developer").expect("write config");

        let plan =
            collect_agent_delete_plan(&base, &agent_dir).expect("substring should not block");

        assert!(plan.team_mutations.is_empty());
    }

    #[tokio::test]
    async fn delete_agent_live_origin_session_blocks() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        let manager = test_manager();
        add_live_session(&manager, &agent_dir).await;
        let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");

        let err = preflight_agent_delete(&plan, &manager)
            .await
            .expect_err("live origin blocks");

        assert!(err.contains("Cannot delete agent 'dev-rust'"), "{err}");
        assert!(err.contains("Session 1"), "{err}");
        assert!(agent_dir.is_dir());
    }

    #[tokio::test]
    async fn delete_agent_live_replica_session_blocks() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        let replica = base.join("wg-1-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).expect("create replica");
        let manager = test_manager();
        add_live_session(&manager, &replica).await;
        let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");

        let err = preflight_agent_delete(&plan, &manager)
            .await
            .expect_err("live replica blocks");

        assert!(err.contains("Cannot delete agent 'dev-rust'"), "{err}");
        assert!(err.contains("Session 1"), "{err}");
        assert!(agent_dir.is_dir());
        assert!(replica.is_dir());
    }

    #[tokio::test]
    async fn delete_agent_post_stage_live_session_recheck_rolls_back() {
        let tmp = create_test_workspace();
        let real_base = test_base(&tmp);
        let agent_dir = create_test_agent(&real_base, "dev-rust", "Dev Rust");
        let marker = real_base.join("marker");
        std::fs::create_dir_all(&marker).expect("create marker");
        let base = marker.join("..");
        let selected_agent_path = base.join("_agent_dev-rust");
        let (config_path, before_config) = write_team_value(
            &real_base,
            "dev-team",
            serde_json::json!({
                "agents": ["_agent_dev-rust"],
                "coordinator": "_agent_architect",
                "repos": []
            }),
        );
        let settings = settings_state(AppSettings::default());
        let manager = test_manager();
        let plan = collect_agent_delete_plan(&base, &selected_agent_path).expect("collect plan");
        preflight_agent_delete(&plan, &manager)
            .await
            .expect("preflight");
        let _metadata = prepare_agent_delete_metadata(&plan, &settings)
            .await
            .expect("metadata");
        let staged = stage_agent_delete_targets(&plan, &manager)
            .await
            .expect("stage");
        assert!(!agent_dir.exists());
        let post_stage_fallback_key = path_key_for_delete(&selected_agent_path);
        assert!(
            plan.target_keys.contains(&post_stage_fallback_key),
            "post-stage recheck must use the pre-stage lexical target key"
        );
        add_live_session(&manager, &selected_agent_path).await;

        let err = ensure_no_live_sessions_under_target_keys(
            &plan.target_keys,
            &plan.agent_name,
            &manager,
        )
        .await
        .expect_err("post-stage live session blocks");
        let rollback = rollback_staged_agent_delete_targets(&staged);
        let msg = format_agent_delete_post_stage_live_failure(err, rollback);

        assert!(msg.contains("Cannot delete agent 'dev-rust'"), "{msg}");
        assert!(agent_dir.is_dir());
        for target in staged {
            assert!(!target.staged_path.exists(), "staged path left behind");
        }
        assert_eq!(
            std::fs::read(&config_path).expect("read config"),
            before_config
        );
    }

    #[tokio::test]
    async fn delete_agent_orphan_replica_removed() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        let replica = base.join("wg-1-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).expect("create orphan replica");
        let settings = settings_state(AppSettings::default());
        let manager = test_manager();

        run_agent_delete_for_test(&base, &agent_dir, &settings, &manager)
            .await
            .expect("delete orphan replica");

        assert!(!agent_dir.exists());
        assert!(!replica.exists());
    }

    #[test]
    fn delete_agent_stage_targets_rolls_back_on_second_target_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = tmp.path().join("_agent_dev-rust");
        let second = tmp.path().join("wg-1-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&first).expect("first");
        std::fs::create_dir_all(&second).expect("second");
        let plan = AgentDeletePlan {
            agent_name: "dev-rust".to_string(),
            agent_ref: "_agent_dev-rust".to_string(),
            origin_dir: first.clone(),
            target_keys: Vec::new(),
            targets: vec![
                AgentDeleteTarget {
                    original_path: first.clone(),
                    original_key: path_key_for_delete(&first),
                    label: "origin".to_string(),
                },
                AgentDeleteTarget {
                    original_path: second.clone(),
                    original_key: path_key_for_delete(&second),
                    label: "replica".to_string(),
                },
            ],
            team_mutations: Vec::new(),
        };
        let manager = test_manager();

        let err = stage_agent_delete_targets_with_rename(
            &plan,
            |from: &Path, to: &Path| {
                if from == second {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "forced",
                    ))
                } else {
                    std::fs::rename(from, to)
                }
            },
            &manager,
        )
        .expect_err("second target fails");

        assert!(matches!(err, AgentDeleteStageError::Other(_)));
        assert!(first.is_dir());
        assert!(second.is_dir());
    }

    #[test]
    fn delete_agent_stage_targets_rollback_failure_reports_hidden_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = tmp.path().join("_agent_dev-rust");
        let second = tmp.path().join("wg-1-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&first).expect("first");
        std::fs::create_dir_all(&second).expect("second");
        let plan = AgentDeletePlan {
            agent_name: "dev-rust".to_string(),
            agent_ref: "_agent_dev-rust".to_string(),
            origin_dir: first.clone(),
            target_keys: Vec::new(),
            targets: vec![
                AgentDeleteTarget {
                    original_path: first.clone(),
                    original_key: path_key_for_delete(&first),
                    label: "origin".to_string(),
                },
                AgentDeleteTarget {
                    original_path: second.clone(),
                    original_key: path_key_for_delete(&second),
                    label: "replica".to_string(),
                },
            ],
            team_mutations: Vec::new(),
        };
        let manager = test_manager();

        let err = stage_agent_delete_targets_with_rename(
            &plan,
            |from: &Path, to: &Path| {
                if from == second || to == first {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "forced",
                    ))
                } else {
                    std::fs::rename(from, to)
                }
            },
            &manager,
        )
        .expect_err("rollback fails");

        let AgentDeleteStageError::RollbackFailed(msg) = err else {
            panic!("expected rollback failure");
        };
        assert!(msg.contains("Hidden cleanup dirs remain"), "{msg}");
        assert!(!msg.starts_with("BLOCKERS:"), "{msg}");
        assert!(!first.exists());
        assert!(second.is_dir());
    }

    #[tokio::test]
    async fn delete_agent_metadata_team_write_failure_restores_configs_and_rolls_back_dirs() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        let (config_a, before_a) = write_team_value(
            &base,
            "alpha",
            serde_json::json!({
                "agents": ["_agent_dev-rust"],
                "coordinator": "_agent_architect",
                "repos": []
            }),
        );
        let (config_b, before_b) = write_team_value(
            &base,
            "beta",
            serde_json::json!({
                "agents": ["_agent_dev-rust"],
                "coordinator": "_agent_architect",
                "repos": []
            }),
        );
        let settings = settings_state(AppSettings::default());
        let manager = test_manager();
        let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");
        let metadata = prepare_agent_delete_metadata(&plan, &settings)
            .await
            .expect("metadata");
        let staged = stage_agent_delete_targets(&plan, &manager)
            .await
            .expect("stage");
        let writes = AtomicUsize::new(0);

        let persist_err = persist_agent_delete_metadata_with_writers(
            &metadata,
            &settings,
            |path, bytes| {
                let count = writes.fetch_add(1, TestOrdering::SeqCst);
                if count == 1 {
                    Err("forced team write failure".to_string())
                } else {
                    write_team_config_json_atomic(path, bytes)
                }
            },
            |candidate| Ok(candidate.clone()),
        )
        .await
        .expect_err("team write failure");
        let restore = restore_agent_delete_metadata_snapshots(&metadata, &settings).await;
        let rollback = rollback_staged_agent_delete_targets(&staged);
        let msg = format_agent_delete_metadata_failure(persist_err, restore, rollback);

        assert!(msg.contains("forced team write failure"), "{msg}");
        assert!(agent_dir.is_dir());
        assert_eq!(std::fs::read(&config_a).expect("read a"), before_a);
        assert_eq!(std::fs::read(&config_b).expect("read b"), before_b);
    }

    #[tokio::test]
    async fn delete_agent_metadata_settings_save_failure_restores_configs_and_rolls_back_dirs() {
        let tmp = create_test_workspace();
        let base = test_base(&tmp);
        let agent_dir = create_test_agent(&base, "dev-rust", "Dev Rust");
        let (config_path, before_config) = write_team_value(
            &base,
            "dev-team",
            serde_json::json!({
                "agents": ["_agent_dev-rust"],
                "coordinator": "_agent_architect",
                "repos": []
            }),
        );
        let mut settings = AppSettings::default();
        settings
            .auto_self_clear_by_agent
            .insert("dev-rust".to_string(), false);
        settings
            .coding_agent_profiles
            .default_profile_by_agent
            .insert("dev-rust".to_string(), "A".to_string());
        let settings = settings_state(settings);
        let manager = test_manager();
        let plan = collect_agent_delete_plan(&base, &agent_dir).expect("collect plan");
        let metadata = prepare_agent_delete_metadata(&plan, &settings)
            .await
            .expect("metadata");
        let staged = stage_agent_delete_targets(&plan, &manager)
            .await
            .expect("stage");

        let persist_err = persist_agent_delete_metadata_with_saver(&metadata, &settings, |_| {
            Err("forced settings save failure".to_string())
        })
        .await
        .expect_err("settings save failure");
        let restore = restore_agent_delete_metadata_snapshots(&metadata, &settings).await;
        let rollback = rollback_staged_agent_delete_targets(&staged);
        let msg = format_agent_delete_metadata_failure(persist_err, restore, rollback);

        assert!(msg.contains("forced settings save failure"), "{msg}");
        assert!(agent_dir.is_dir());
        assert_eq!(
            std::fs::read(&config_path).expect("read config"),
            before_config
        );
        let saved = settings.read().await;
        assert_eq!(saved.auto_self_clear_by_agent.get("dev-rust"), Some(&false));
        assert_eq!(
            saved
                .coding_agent_profiles
                .default_profile_by_agent
                .get("dev-rust")
                .map(String::as_str),
            Some("A")
        );
    }

    #[tokio::test]
    async fn delete_workgroup_dir_backend_returns_blockers_json() {
        #[cfg(not(windows))]
        {
            return;
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 0x00000001;

            let tmp = tempfile::tempdir().expect("tempdir");
            let wg_dir = tmp.path().join("wg-1-test");
            std::fs::create_dir(&wg_dir).expect("create wg_dir");
            let inside = wg_dir.join("locked.bin");
            std::fs::write(&inside, b"hold me").expect("write inside file");
            let _handle = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&inside)
                .expect("open with restricted share mode");
            let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

            let err = delete_workgroup_dir_backend(&wg_dir, "wg-1-test", &manager)
                .await
                .expect_err("blocked delete should return blockers json");
            let json = err
                .strip_prefix("BLOCKERS:")
                .unwrap_or_else(|| panic!("expected BLOCKERS prefix, got {}", err));
            let report: serde_json::Value = serde_json::from_str(json).expect("blockers json");
            assert_eq!(report["workgroup"], "wg-1-test");
            assert!(!report["rawDeleteError"]
                .as_str()
                .expect("rawDeleteError")
                .is_empty());
            assert!(wg_dir.is_dir(), "blocked tree should remain intact");
        }
    }

    /// Blocked path (Windows-only): a child file opened without
    /// `FILE_SHARE_DELETE` blocks the parent-dir rename with
    /// `ERROR_SHARING_VIOLATION` (32). The rename must fail before any file is
    /// touched, so the WG dir + child must both be intact afterward.
    #[cfg(windows)]
    #[test]
    fn try_atomic_delete_wg_blocked_with_restrictive_share_mode() {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ only — explicitly NO FILE_SHARE_DELETE.
        const FILE_SHARE_READ: u32 = 0x00000001;

        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        std::fs::create_dir(&wg_dir).expect("create wg_dir");
        let inside = wg_dir.join("locked.bin");
        std::fs::write(&inside, b"hold me").expect("write inside file");

        // Hold a handle that denies DELETE share. Drop scope at end of test.
        let _handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&inside)
            .expect("open with restricted share mode");

        let outcome = try_atomic_delete_wg(&wg_dir);
        match &outcome {
            WgDeleteOutcome::Blocked(_) => {
                assert!(wg_dir.is_dir(), "wg_dir must remain after blocked rename");
                assert!(
                    inside.is_file(),
                    "inner file must remain after blocked rename"
                );
            }
            WgDeleteOutcome::Deleted => {
                panic!(
                    "expected Blocked when child file is held without FILE_SHARE_DELETE; \
                     got Deleted (rename succeeded — Windows behavior may have changed)"
                );
            }
            WgDeleteOutcome::Other(e) => {
                panic!("expected Blocked, got Other({:?}={})", e.kind(), e);
            }
            WgDeleteOutcome::Partial { .. } => {
                panic!("blocked rename cannot report Partial");
            }
        }
    }

    /// Suffix scheme invariant (#113 follow-up): the orphan name produced on a
    /// rename-success-then-remove-fails race must NOT match the
    /// `starts_with("wg-")` filters used by `ac_discovery` and `cli::list_peers`.
    /// We test this by asserting the format directly:
    /// the temp name starts with `.deleting-`, which automatically dodges the
    /// `wg-` prefix filter.
    #[test]
    fn temp_name_format_dodges_workgroup_filter() {
        // Inline the same name construction `try_atomic_delete_wg` uses, so the
        // assertion locks the contract independent of fs interaction.
        let original_name = "wg-7-dev-team";
        let temp_name = format!(".deleting-{}-{}", original_name, uuid::Uuid::new_v4());
        assert!(
            temp_name.starts_with(".deleting-"),
            "temp name must start with .deleting- so future cleanup tooling can identify orphans"
        );
        assert!(
            !temp_name.starts_with("wg-"),
            "temp name must NOT match the wg- discovery filter (would surface as ghost workgroup)"
        );
    }

    /// `is_rename_blocked_by_handle` matches `ERROR_ACCESS_DENIED` (5).
    /// MoveFileEx returns 5 — not 32 — when an open handle on a descendant
    /// lacks `FILE_SHARE_DELETE`. Empirical, verified by the
    /// `try_atomic_delete_wg_blocked_with_restrictive_share_mode` test below.
    #[cfg(windows)]
    #[test]
    fn is_rename_blocked_by_handle_matches_access_denied() {
        let e = std::io::Error::from_raw_os_error(5);
        assert!(
            is_rename_blocked_by_handle(&e),
            "os error 5 (ERROR_ACCESS_DENIED) must classify as a rename blocker"
        );
    }

    /// `is_rename_blocked_by_handle` is a superset of `is_file_in_use_error` —
    /// the existing 32/33/1224 codes still match.
    #[cfg(windows)]
    #[test]
    fn is_rename_blocked_by_handle_matches_file_in_use_codes() {
        for code in [32, 33, 1224] {
            let e = std::io::Error::from_raw_os_error(code);
            assert!(
                is_rename_blocked_by_handle(&e),
                "os error {} must classify as a rename blocker",
                code
            );
        }
    }

    /// `is_rename_blocked_by_handle` does NOT match unrelated errors. NotFound
    /// (2) is the canonical legitimate non-blocker error path.
    #[cfg(windows)]
    #[test]
    fn is_rename_blocked_by_handle_rejects_not_found() {
        let e = std::io::Error::from_raw_os_error(2);
        assert!(
            !is_rename_blocked_by_handle(&e),
            "os error 2 (ERROR_FILE_NOT_FOUND) must NOT classify as blocker"
        );
    }

    /// Off Windows the helper always returns false — diagnostic isn't run on
    /// non-Windows platforms.
    #[cfg(not(windows))]
    #[test]
    fn is_rename_blocked_by_handle_no_op_on_non_windows() {
        let e = std::io::Error::from_raw_os_error(5);
        assert!(
            !is_rename_blocked_by_handle(&e),
            "non-Windows must always return false"
        );
    }

    // ── #738: build_task_content marks creation titles as user-owned ──

    #[test]
    fn build_task_content_prefixes_user_title() {
        assert_eq!(
            build_task_content("Build login"),
            "---\ntitle: 'USER: Build login'\n---\n"
        );
    }

    #[test]
    fn build_task_content_does_not_double_prefix() {
        assert_eq!(
            build_task_content("USER: Existing"),
            "---\ntitle: 'USER: Existing'\n---\n"
        );
    }

    #[test]
    fn build_task_content_escapes_quotes_after_prefix() {
        // The single quote is YAML-escaped (doubled) after the USER: prefix.
        assert_eq!(
            build_task_content("Ana's task"),
            "---\ntitle: 'USER: Ana''s task'\n---\n"
        );
    }

    // ── parse_task_title — dev-rust R7 cases ──

    #[test]
    fn parse_task_title_returns_some_for_canonical_frontmatter() {
        assert_eq!(
            parse_task_title("---\ntitle: Hello world\n---\n\nbody\n"),
            Some("Hello world".to_string())
        );
    }

    #[test]
    fn parse_task_title_strips_double_quotes() {
        assert_eq!(
            parse_task_title("---\ntitle: \"Quoted\"\n---\n"),
            Some("Quoted".to_string())
        );
    }

    #[test]
    fn parse_task_title_strips_single_quotes() {
        assert_eq!(
            parse_task_title("---\ntitle: 'Quoted'\n---\n"),
            Some("Quoted".to_string())
        );
    }

    #[test]
    fn parse_task_title_returns_none_when_no_frontmatter() {
        assert_eq!(parse_task_title("# Heading\n\nbody\n"), None);
    }

    #[test]
    fn parse_task_title_returns_none_for_empty_value() {
        assert_eq!(parse_task_title("---\ntitle:\n---\n"), None);
    }

    #[test]
    fn parse_task_title_returns_none_when_closing_delimiter_missing() {
        assert_eq!(parse_task_title("---\ntitle: foo\nbody only\n"), None);
    }

    #[test]
    fn parse_task_title_returns_none_when_title_field_absent() {
        assert_eq!(parse_task_title("---\nname: foo\n---\n"), None);
    }

    #[test]
    fn parse_task_title_preserves_inner_colon() {
        assert_eq!(
            parse_task_title("---\ntitle: a: b\n---\n"),
            Some("a: b".to_string())
        );
    }

    #[test]
    fn parse_task_title_handles_indented_key() {
        assert_eq!(
            parse_task_title("---\n  title: foo\n---\n"),
            Some("foo".to_string())
        );
    }

    // ── parse_task_title — dev-rust-grinch G3 / G13 case-insensitivity ──

    #[test]
    fn parse_task_title_handles_capital_t() {
        assert_eq!(
            parse_task_title("---\nTitle: Foo\n---\n"),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn parse_task_title_handles_all_caps_key() {
        assert_eq!(
            parse_task_title("---\nTITLE: Foo\n---\n"),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn parse_task_title_handles_mixed_case_key() {
        assert_eq!(
            parse_task_title("---\ntItLe: Foo\n---\n"),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn parse_task_title_value_remains_case_sensitive() {
        // The key match is case-insensitive; the value MUST round-trip
        // verbatim (it is user-visible content, not a structural marker).
        assert_eq!(
            parse_task_title("---\nTitle: MixedCASE Value\n---\n"),
            Some("MixedCASE Value".to_string())
        );
    }

    // ── parse_task_title — UTF-8 BOM (grinch MEDIUM) ──
    // Mirrors `cli/task_ops.rs::parse_task` which already strips the BOM.
    // Without this, TASK.md saved as "UTF-8 with BOM" breaks gate-4
    // idempotency and risks silent overwrite of a user-edited title.

    #[test]
    fn parse_task_title_strips_utf8_bom() {
        assert_eq!(
            parse_task_title("\u{FEFF}---\ntitle: Foo\n---\n"),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn parse_task_title_returns_none_for_bom_without_frontmatter() {
        assert_eq!(parse_task_title("\u{FEFF}# Heading\n\nbody\n"), None);
    }

    // ── #177 — determine_next_wg_number lowest-free reuse ──

    /// Helper: create an empty directory at `<root>/<name>` for the test.
    fn touch_dir(root: &Path, name: &str) {
        std::fs::create_dir(root.join(name))
            .unwrap_or_else(|e| panic!("create_dir {}: {}", name, e));
    }

    /// Empty workspace returns slot 1, the lowest positive integer.
    #[test]
    fn determine_next_wg_number_returns_one_when_no_wg_dirs_exist() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(determine_next_wg_number(tmp.path()), 1);
    }

    /// Contiguous allocation: `wg-1`, `wg-2`, `wg-3` already exist for the team
    /// → next slot is 4 (no internal gap to reuse).
    #[test]
    fn determine_next_wg_number_returns_next_after_contiguous_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-1-dev-team");
        touch_dir(tmp.path(), "wg-2-dev-team");
        touch_dir(tmp.path(), "wg-3-dev-team");
        assert_eq!(determine_next_wg_number(tmp.path()), 4);
    }

    /// Gap reuse — the load-bearing case from issue #177.
    /// `wg-1` and `wg-3` exist (someone destroyed `wg-2`) → next slot is 2.
    #[test]
    fn determine_next_wg_number_reuses_lowest_internal_gap() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-1-dev-team");
        touch_dir(tmp.path(), "wg-3-dev-team");
        assert_eq!(determine_next_wg_number(tmp.path()), 2);
    }

    /// Leading gap — `wg-1` is free even though higher slots are taken.
    #[test]
    fn determine_next_wg_number_reuses_slot_one_when_only_higher_slots_exist() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-2-dev-team");
        touch_dir(tmp.path(), "wg-3-dev-team");
        assert_eq!(determine_next_wg_number(tmp.path()), 1);
    }

    /// Project scoping: dirs for any team block slot reuse.
    /// `wg-1-dev-team` and `wg-3-qa-team` make slot 2 the next free slot.
    #[test]
    fn determine_next_wg_number_is_global_across_team_suffixes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-1-dev-team");
        touch_dir(tmp.path(), "wg-1-qa-team");
        touch_dir(tmp.path(), "wg-3-qa-team");
        assert_eq!(determine_next_wg_number(tmp.path()), 2);
    }

    /// Invalid `wg-*` directory names must not occupy any slot.
    /// - `wg-abc-dev-team`: non-numeric middle → parse fails → ignored.
    /// - `wg--dev-team`:    empty middle (`[3..3]` slice) → parse fails → ignored.
    ///
    /// Only `wg-2-dev-team` is real, so slot 1 is still free.
    /// (The `wg-dev-team` no-number case is covered by its own test below
    /// because it specifically exercises the checked-slicing guard.)
    #[test]
    fn determine_next_wg_number_ignores_invalid_directory_names() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-abc-dev-team");
        touch_dir(tmp.path(), "wg--dev-team");
        touch_dir(tmp.path(), "wg-2-dev-team");
        assert_eq!(determine_next_wg_number(tmp.path()), 1);
    }

    /// `wg-0-<team>` does not block slot 1. The allocator's lowest-free
    /// search starts at 1, so any `0` that ends up in `taken` is never
    /// tested by `find` — slot 1 stays reachable. The allocator only ever
    /// produces values ≥ 1.
    #[test]
    fn determine_next_wg_number_ignores_zero_numbered_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-0-dev-team");
        touch_dir(tmp.path(), "wg-2-dev-team");
        assert_eq!(determine_next_wg_number(tmp.path()), 1);
    }

    /// Files (not directories) named like a workgroup must not occupy a slot —
    /// the allocator only considers real workgroup directories.
    #[test]
    fn determine_next_wg_number_ignores_files_named_like_workgroups() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("wg-1-dev-team"), b"not a dir").expect("write file");
        assert_eq!(determine_next_wg_number(tmp.path()), 1);
    }

    /// Regression for the suffix-overlaps-prefix slice case: a directory
    /// named `wg-{team}` (no number, e.g. `wg-dev-team`) passes both the
    /// `starts_with("wg-")` and `ends_with("-{team}")` checks, but the
    /// digits slice would be `&name_str[3..2]` — invalid. With `&str[..]`
    /// indexing this panics; with `name_str.get(..)` it returns `None` and
    /// the entry is silently ignored. This test locks in the no-panic
    /// behavior so a future refactor cannot reintroduce the bug.
    #[test]
    fn determine_next_wg_number_does_not_panic_on_no_number_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-dev-team");
        // Must return slot 1 (the bogus dir is ignored, not counted as taken).
        assert_eq!(determine_next_wg_number(tmp.path()), 1);
    }

    /// In-flight `.deleting-wg-N-team-<uuid>` directories must NOT be
    /// counted as occupying slot N. Locks the contract that #177 relies
    /// on: the leading `.` of the temp name (set in `try_atomic_delete_wg`
    /// at line 1535 — `.deleting-{wg_name}-{uuid}`) dodges the
    /// `starts_with("wg-")` filter, so a freed slot is reusable on the
    /// very next allocation tick. A future temp-name refactor that drops
    /// the leading `.` would silently re-introduce the gap-leak this issue
    /// closes; this test catches that regression.
    #[test]
    fn determine_next_wg_number_ignores_deleting_temp_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-1-dev-team");
        touch_dir(
            tmp.path(),
            ".deleting-wg-2-dev-team-00000000-0000-0000-0000-000000000000",
        );
        // wg-2 is mid-delete: the `.deleting-…` entry must not block slot 2.
        assert_eq!(determine_next_wg_number(tmp.path()), 2);
    }

    /// Team suffix overlap is irrelevant to global allocation.
    #[test]
    fn determine_next_wg_number_handles_subset_team_suffixes_globally() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch_dir(tmp.path(), "wg-1-dev-team");
        touch_dir(tmp.path(), "wg-1-team");
        assert_eq!(determine_next_wg_number(tmp.path()), 2);
    }

    // ── #271 — build_role_content (Role.md template merge) ──

    /// Test #21 — byte-for-byte parity with the pre-#271 inline `format!`
    /// so callers that pass no template see the legacy file. The legacy
    /// format string is reproduced verbatim here (NOT imported from the
    /// helper) so the two transcriptions can cross-check each other —
    /// any drift in `build_role_content` instantly fails this test.
    #[test]
    fn build_role_content_no_template_matches_legacy() {
        let safe_name = "alpha";
        let description = "Test agent description.";
        let desc_yaml = description.replace('\'', "''");
        let legacy = format!(
            "---\nname: '{}'\ndescription: '{}'\ntype: agent\n---\n\n# {}\n\n{}\n\n## Source of Truth\n\nThis role is defined in Role.md of your Agent Matrix at: .ac/_agent_{}/\nIf you are running as a replica, this file was generated from that source.\nAlways use memory/, plans/, and skills/ from your Agent Matrix, and treat Role.md there as the canonical role definition. Never use external memory systems.\n\n## Agent Memory Rule\n\nIf you are running as a replica, the single source of truth for persistent knowledge is your Agent Matrix's memory/, plans/, skills/, and Role.md. Use your replica folder only for replica-local scratch, inbox/outbox, and session artifacts. NEVER use external memory systems from the coding agent (e.g., ~/.claude/projects/memory/).\n",
            safe_name, desc_yaml, safe_name, description, safe_name
        );
        let actual = build_role_content(safe_name, description, None);
        assert_eq!(
            actual, legacy,
            "no-template Role.md must be byte-identical to the pre-#271 format"
        );
    }

    /// Test #22 — supplying a resolved template inserts the `## Role Profile`
    /// section between the heading/description and the Source of Truth section,
    /// using the template body verbatim (post-trim). Body must be fenced by the
    /// plan §7.2 HTML-comment delimiters with the opening tag carrying the
    /// resolved template id as `source="…"`.
    #[test]
    fn build_role_content_with_template_inserts_role_profile() {
        let template = crate::commands::role_templates::ResolvedRoleTemplate {
            id: "agency:my-test".into(),
            body: "Template body line one.\nTemplate body line two.".into(),
            skills_src: None,
        };
        let out = build_role_content("alpha", "Test description.", Some(&template));
        assert!(
            out.contains("## Role Profile"),
            "must add a Role Profile section: {}",
            out
        );
        assert!(
            out.contains("Template body line one."),
            "must include the template body verbatim"
        );
        assert!(
            out.contains("Template body line two."),
            "must include subsequent template body lines"
        );
        // Plan §7.2: opening delimiter carries provenance via source="<id>".
        assert!(
            out.contains("<!-- ac:role-profile source=\"agency:my-test\""),
            "must include opening ac:role-profile delimiter with template id: {}",
            out
        );
        // Plan §7.2: closing delimiter fences the imported body.
        assert!(
            out.contains("<!-- ac:role-profile:end -->"),
            "must include closing ac:role-profile:end delimiter: {}",
            out
        );
        // Role Profile must come BEFORE Source of Truth in the file.
        let profile_idx = out.find("## Role Profile").expect("Role Profile present");
        let sot_idx = out
            .find("## Source of Truth")
            .expect("Source of Truth present");
        assert!(
            profile_idx < sot_idx,
            "Role Profile must precede Source of Truth"
        );
        // Delimiters must bracket the body — opening before, closing after.
        let open_idx = out
            .find("<!-- ac:role-profile source=")
            .expect("opening delimiter present");
        let close_idx = out
            .find("<!-- ac:role-profile:end -->")
            .expect("closing delimiter present");
        let body_idx = out
            .find("Template body line one.")
            .expect("template body present");
        assert!(
            open_idx < body_idx && body_idx < close_idx,
            "delimiters must bracket the imported body"
        );
        // Closing delimiter must come BEFORE the mandatory AC sections so the
        // AC sections remain outside the fenced template region.
        assert!(
            close_idx < sot_idx,
            "closing delimiter must precede Source of Truth (mandatory AC sections stay last)"
        );
    }

    /// Test #23 — mandatory sections are always last, even when a template body
    /// itself contains a heading that LOOKS like a mandatory section. The
    /// template is inserted between description and the mandatory block, never
    /// after it; the actual `## Source of Truth` and `## Agent Memory Rule`
    /// added by the helper must therefore appear AFTER the template body.
    #[test]
    fn build_role_content_keeps_mandatory_sections_last() {
        let template = crate::commands::role_templates::ResolvedRoleTemplate {
            id: "agency:tricky".into(),
            body: "Body before.\n\n## Source of Truth\n\nLook-alike heading inside template.\n\n## Agent Memory Rule\n\nLook-alike heading.\n\nBody after.".into(),
            skills_src: None,
        };
        let out = build_role_content("alpha", "desc", Some(&template));
        // The mandatory block's exact opening line appears in both the template
        // body AND the helper's tail — so `rfind` must point at the helper's
        // copy, which has to sit AFTER the entire template body.
        let last_sot = out
            .rfind("## Source of Truth")
            .expect("Source of Truth heading present");
        let last_memory_rule = out
            .rfind("## Agent Memory Rule")
            .expect("Agent Memory Rule heading present");
        let body_after = out.find("Body after.").expect("template body retained");
        assert!(
            last_sot > body_after,
            "the helper's `## Source of Truth` must come AFTER the template body"
        );
        assert!(
            last_memory_rule > last_sot,
            "`## Agent Memory Rule` must come last"
        );
        // And the helper's tail content must still be present verbatim.
        assert!(
            out.contains("This role is defined in Role.md of your Agent Matrix"),
            "mandatory Source of Truth body must still be appended"
        );
        assert!(
            out.contains("NEVER use external memory systems from the coding agent"),
            "mandatory Agent Memory Rule body must still be appended"
        );
    }

    /// Test #24 — single quotes in the description are doubled inside the YAML
    /// frontmatter `description:` value so the file stays valid YAML, while the
    /// human-readable body keeps the original character.
    #[test]
    fn build_role_content_escapes_single_quote_in_description() {
        let desc = "Don't break the YAML 'header'.";
        let out = build_role_content("alpha", desc, None);
        // YAML line uses single-quoted value with doubled apostrophes.
        assert!(
            out.contains("description: 'Don''t break the YAML ''header''.'"),
            "YAML description must double-escape single quotes; got:\n{}",
            out
        );
        // Body keeps the original (un-doubled) description.
        assert!(
            out.contains("Don't break the YAML 'header'."),
            "human-readable description body must keep the original characters; got:\n{}",
            out
        );
    }

    #[test]
    fn create_agent_matrix_from_role_writes_role_bytes_exactly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let workspace = project.join(".ac");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let role_bytes = b"\xEF\xBB\xBF# Variant\r\n\r\nKeep bytes.\r\n";

        let created = create_agent_matrix_from_role(CreateAgentMatrixFromRoleArgs {
            workspace_dir: &workspace,
            safe_name: "tech-lead-control",
            role_bytes,
        })
        .expect("create matrix from role");

        assert_eq!(
            std::fs::read(&created.role_path).expect("read role"),
            role_bytes
        );
        assert!(created.agent_dir.join("config.json").is_file());
        for dir in AGENT_MATRIX_DIRS {
            assert!(
                created.agent_dir.join(dir).is_dir(),
                "missing expected matrix dir {}",
                dir
            );
        }
    }

    #[test]
    fn create_agent_matrix_from_role_rejects_existing_target() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let workspace = project.join(".ac");
        std::fs::create_dir_all(workspace.join("_agent_tech-lead-control"))
            .expect("existing target");

        let err = create_agent_matrix_from_role(CreateAgentMatrixFromRoleArgs {
            workspace_dir: &workspace,
            safe_name: "tech-lead-control",
            role_bytes: b"# Role\n",
        })
        .unwrap_err();

        assert!(err.contains("already exists"), "unexpected error: {}", err);
    }

    #[test]
    fn create_agent_matrix_from_role_requires_slug_identity() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("ProjectAlpha");
        let workspace = project.join(".ac");
        std::fs::create_dir_all(&workspace).expect("workspace");

        let err = create_agent_matrix_from_role(CreateAgentMatrixFromRoleArgs {
            workspace_dir: &workspace,
            safe_name: "Tech Lead",
            role_bytes: b"# Role\n",
        })
        .unwrap_err();

        assert!(err.contains("lowercase slug"), "unexpected error: {}", err);
        assert!(!workspace.join("_agent_Tech Lead").exists());
    }

    // ── #621 workgroup-delete clock + cache cleanup (integration) ─────────────

    #[test]
    fn workgroup_removal_clears_clock_key_and_cache_sibling_intact() {
        use crate::config::coordinator_clocks::CoordinatorClocks;
        use crate::config::session_context::sweep_context_cache_dir;
        use std::time::{Duration, SystemTime};

        // (#621 E4) inline timestamp; no helper exists in this module.
        let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();

        let tmp = tempfile::tempdir().expect("temp dir");
        let proj = tmp.path().join("Proj");
        let ac = proj.join(".ac");
        let wg1 = ac.join("wg-1-team");
        let wg2 = ac.join("wg-2-team");
        std::fs::create_dir_all(&wg1).unwrap();
        std::fs::create_dir_all(&wg2).unwrap();

        // Seed clocks: target wg, sibling wg, origin agent.
        let mut clocks = CoordinatorClocks::default();
        clocks.note_user_message("Proj:wg-1-team/coord", ts);
        clocks.note_user_message("Proj:wg-2-team/coord", ts);
        clocks.seed_if_absent("Proj/architect", ts);

        // Seed cache: a stale file for the removed wg's replica + a fresh sibling.
        let cache = tmp.path().join("context-cache");
        std::fs::create_dir_all(&cache).unwrap();
        let old = SystemTime::now() - Duration::from_secs(40 * 24 * 60 * 60);
        let stale = cache.join("replica-context-aaa.md");
        std::fs::write(&stale, "x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let fresh = cache.join("replica-context-bbb.md");
        std::fs::write(&fresh, "x").unwrap();

        // Remove wg-1: dir + clock key (cache via the age sweep).
        assert!(matches!(
            try_atomic_delete_wg(&wg1),
            WgDeleteOutcome::Deleted
        ));
        assert_eq!(clocks.remove_workgroup("Proj", "wg-1-team"), 1);
        let swept = sweep_context_cache_dir(
            &cache,
            SystemTime::now(),
            Duration::from_secs(30 * 24 * 60 * 60),
        );

        assert!(!wg1.exists() && wg2.exists(), "only wg-1 dir removed");
        assert_eq!(clocks.last_user_message_at("Proj:wg-1-team/coord"), None);
        assert!(
            clocks
                .last_user_message_at("Proj:wg-2-team/coord")
                .is_some(),
            "sibling clock intact"
        );
        assert!(
            clocks.last_user_message_at("Proj/architect").is_some(),
            "origin clock intact"
        );
        assert_eq!(swept, 1);
        assert!(
            !stale.exists() && fresh.exists(),
            "stale cache gone, fresh kept"
        );
    }

    #[test]
    fn team_cascade_clears_each_wg_clock_keeps_others() {
        // (#621 LOW-3b) delete_team loops its wg-N-<team> dirs; each removal drops that
        // wg's keys. A non-team wg and an origin agent survive. Mirrors the §3.3 loop's
        // per-wg `remove_workgroup` over multiple wgs (accumulator semantics).
        use crate::config::coordinator_clocks::CoordinatorClocks;
        let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();

        let mut clocks = CoordinatorClocks::default();
        for k in [
            "Proj:wg-1-squad/coord",
            "Proj:wg-2-squad/coord",
            "Proj:wg-2-squad/dev",   // second agent in a team wg
            "Proj:wg-9-other/coord", // a DIFFERENT team's wg, must survive
        ] {
            clocks.note_user_message(k, ts);
        }
        clocks.seed_if_absent("Proj/architect", ts); // origin, survives

        // Simulate the cascade over the "squad" team's wgs.
        let mut total = 0usize;
        for wg in ["wg-1-squad", "wg-2-squad"] {
            total += clocks.remove_workgroup("Proj", wg);
        }
        assert_eq!(total, 3, "wg-1 (1) + wg-2 (2 agents) keys removed");
        assert_eq!(clocks.last_user_message_at("Proj:wg-1-squad/coord"), None);
        assert_eq!(clocks.last_user_message_at("Proj:wg-2-squad/coord"), None);
        assert_eq!(clocks.last_user_message_at("Proj:wg-2-squad/dev"), None);
        assert!(
            clocks
                .last_user_message_at("Proj:wg-9-other/coord")
                .is_some(),
            "other team wg intact"
        );
        assert!(
            clocks.last_user_message_at("Proj/architect").is_some(),
            "origin intact"
        );
    }

    #[test]
    fn failed_wg_delete_keeps_clock_key() {
        // (#621 LOW-3c) the clock cleanup is gated on a SUCCESSFUL dir delete (delete_workgroup
        // via `?` early-return; delete_team via the `else` success branch). Force a failed
        // remove and assert, applying that exact gate, that the key survives.
        use crate::config::coordinator_clocks::CoordinatorClocks;
        let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();

        let tmp = tempfile::tempdir().expect("temp dir");
        let wg = tmp.path().join("wg-1-team");
        std::fs::create_dir_all(&wg).unwrap();
        let mut clocks = CoordinatorClocks::default();
        clocks.note_user_message("Proj:wg-1-team/coord", ts);

        // Force the remove step to fail (rename succeeds, remove_dir_all errors -> Partial).
        let outcome = try_atomic_delete_wg_with_remove(&wg, |_p| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "blocked",
            ))
        });
        // The production gate: clean the clock ONLY on Deleted.
        if matches!(outcome, WgDeleteOutcome::Deleted) {
            clocks.remove_workgroup("Proj", "wg-1-team");
        }
        assert!(
            !matches!(outcome, WgDeleteOutcome::Deleted),
            "forced failure is not a Deleted outcome"
        );
        assert!(
            clocks
                .last_user_message_at("Proj:wg-1-team/coord")
                .is_some(),
            "a failed delete must keep the clock key"
        );
    }
}
