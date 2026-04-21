use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::commands::ac_discovery::DiscoveryBranchWatcher;
use crate::pty::git_watcher::{CoordinatorChangedPayload, GitWatcher};
use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;

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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sanitize a user-provided name into a safe directory component:
/// lowercase, only a-z 0-9 and hyphens, no leading/trailing hyphens.
fn sanitize_name(raw: &str) -> Result<String, String> {
    let sanitized: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
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
fn validate_existing_name(name: &str, entity_label: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{} name cannot be empty", entity_label));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("Invalid {} name: only alphanumeric characters and hyphens are allowed", entity_label));
    }
    Ok(())
}

/// Extract a repo directory name from a git URL.
/// `https://github.com/org/my-repo.git` → `my-repo`
fn repo_dir_name_from_url(url: &str) -> String {
    let without_trailing = url.trim_end_matches('/');
    let last_segment = without_trailing
        .rsplit('/')
        .next()
        .unwrap_or("repo");
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
    entry_dir == format!("_agent_{}", agent_name)
        || entry_dir == agent_name
}

/// Parse YAML frontmatter from a Role.md file.
/// Returns (name, description) if found.
fn parse_role_frontmatter(content: &str) -> (Option<String>, Option<String>) {
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

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Create an agent matrix directory inside {project_path}/.ac-new/_agent_{name}/
#[tauri::command]
pub async fn create_agent_matrix(
    project_path: String,
    name: String,
    description: String,
) -> Result<CreatedEntityResult, String> {
    let safe_name = sanitize_name(&name)?;
    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

    let agent_dir = base.join(format!("_agent_{}", safe_name));
    if agent_dir.exists() {
        return Err(format!("Agent '{}' already exists", safe_name));
    }

    // Create directory structure
    std::fs::create_dir_all(&agent_dir)
        .map_err(|e| format!("Failed to create agent directory: {}", e))?;

    for sub in &["memory", "plans", "skills", "inbox", "outbox"] {
        std::fs::create_dir_all(agent_dir.join(sub))
            .map_err(|e| format!("Failed to create {} directory: {}", sub, e))?;
    }

    // Role.md with YAML frontmatter (single-quoted values for safe YAML)
    let desc_yaml = description.replace('\'', "''");
    let role_content = format!(
        "---\nname: '{}'\ndescription: '{}'\ntype: agent\n---\n\n# {}\n\n{}\n\n## Source of Truth\n\nThis role is defined in Role.md of your Agent Matrix at: .ac-new/_agent_{}/\nIf you are running as a replica, this file was generated from that source.\nAlways use memory/ and plans/ from your Agent Matrix, and treat Role.md there as the canonical role definition. Never use external memory systems.\n\n## Agent Memory Rule\n\nIf you are running as a replica, the single source of truth for persistent knowledge is your Agent Matrix's memory/, plans/, and Role.md. Use your replica folder only for replica-local scratch, inbox/outbox, and session artifacts. NEVER use external memory systems from the coding agent (e.g., ~/.claude/projects/memory/).\n",
        safe_name, desc_yaml, safe_name, description, safe_name
    );

    std::fs::write(agent_dir.join("Role.md"), &role_content)
        .map_err(|e| format!("Failed to write Role.md: {}", e))?;

    // config.json
    std::fs::write(agent_dir.join("config.json"), "{\n  \"tooling\": {}\n}\n")
        .map_err(|e| format!("Failed to write config.json: {}", e))?;

    let result_path = agent_dir.to_string_lossy().to_string();
    log::info!("[entity_creation] Created agent matrix: {}", result_path);
    Ok(CreatedEntityResult { path: result_path })
}

/// Delete an agent matrix directory from a project.
/// Removes {project_path}/.ac-new/_agent_{agent_name}/ entirely.
/// Checks that no team references this agent before deleting.
#[tauri::command]
pub async fn delete_agent_matrix(
    project_path: String,
    agent_name: String,
) -> Result<(), String> {
    validate_existing_name(&agent_name, "Agent")?;

    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

    let agent_dir = base.join(format!("_agent_{}", agent_name));
    if !agent_dir.exists() {
        return Err(format!("Agent '{}' not found", agent_name));
    }

    // Referential integrity: check if any team references this agent.
    // Team configs store agent refs in varying formats (relative: "../_agent_X",
    // absolute: "C:\..._agent_X", or bare: "_agent_X"). Normalize by extracting
    // the final path component after replacing backslashes.
    let agent_dir_name = format!("_agent_{}", agent_name);
    let mut referencing_teams: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(&base)
        .map_err(|e| format!("Cannot read .ac-new directory for integrity check: {}", e))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| format!("Cannot read directory entry during integrity check: {}", e))?;
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if !dir_name.starts_with("_team_") {
            continue;
        }
        let config_path = entry.path().join("config.json");
        if !config_path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Cannot read team config {}/config.json for integrity check: {}", dir_name, e))?;
        let config: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Cannot parse team config {}/config.json: {}", dir_name, e))?;
        let agents = config
            .get("agents")
            .and_then(|a| a.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        if agents.iter().any(|a| {
            // Normalize: replace backslashes, split on '/', take the last component
            let normalized = a.replace('\\', "/");
            normalized
                .rsplit('/')
                .next()
                .map(|last| last == agent_dir_name)
                .unwrap_or(false)
        }) {
            let team_name = dir_name.strip_prefix("_team_").unwrap_or(&dir_name);
            referencing_teams.push(team_name.to_string());
        }
    }
    if !referencing_teams.is_empty() {
        return Err(format!(
            "Cannot delete agent '{}': referenced by team(s): {}. Remove the agent from those teams first.",
            agent_name,
            referencing_teams.join(", ")
        ));
    }

    std::fs::remove_dir_all(&agent_dir)
        .map_err(|e| format!("Failed to delete agent directory: {}", e))?;
    log::info!("[entity_creation] Deleted agent matrix: {}", agent_name);
    Ok(())
}

/// List all agent matrices across multiple project paths.
/// Scans {project}/.ac-new/_agent_*/ and reads Role.md frontmatter.
#[tauri::command]
pub async fn list_all_agents(
    project_paths: Vec<String>,
) -> Result<Vec<AgentInfo>, String> {
    let mut agents: Vec<AgentInfo> = Vec::new();

    for project_path in &project_paths {
        let base = Path::new(project_path);
        let ac_new = base.join(".ac-new");
        if !ac_new.is_dir() {
            continue;
        }

        let project_name = base
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let entries = match std::fs::read_dir(&ac_new) {
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

    agents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(agents)
}

/// Create a team directory inside {project_path}/.ac-new/_team_{name}/
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
    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

    let team_dir = base.join(format!("_team_{}", safe_name));
    if team_dir.exists() {
        return Err(format!("Team '{}' already exists", safe_name));
    }

    std::fs::create_dir_all(&team_dir)
        .map_err(|e| format!("Failed to create team directory: {}", e))?;

    // memory/
    std::fs::create_dir_all(team_dir.join("memory"))
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;

    // conventions.md (empty)
    std::fs::write(team_dir.join("conventions.md"), "")
        .map_err(|e| format!("Failed to write conventions.md: {}", e))?;

    // config.json
    let repos_json: Vec<serde_json::Value> = repos
        .iter()
        .map(|r| {
            serde_json::json!({
                "url": r.url,
                "agents": r.agents,
            })
        })
        .collect();

    let config = serde_json::json!({
        "agents": agents,
        "coordinator": coordinator,
        "repos": repos_json,
    });

    let config_str = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config.json: {}", e))?;
    std::fs::write(team_dir.join("config.json"), &config_str)
        .map_err(|e| format!("Failed to write config.json: {}", e))?;

    let result_path = team_dir.to_string_lossy().to_string();
    log::info!("[entity_creation] Created team: {}", result_path);
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(CreatedEntityResult { path: result_path })
}

/// Create a workgroup from an existing team.
/// Clones repos async — partial failures are reported but don't rollback the WG.
#[tauri::command]
pub async fn create_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
) -> Result<WorkgroupCloneResult, String> {
    let safe_team = sanitize_name(&team_name)?;
    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

    // Ensure gitignore protects workgroup clones from parent repo operations
    if let Err(e) = crate::commands::ac_discovery::ensure_ac_new_gitignore(&base) {
        log::warn!("[create_workgroup] Failed to ensure .ac-new/.gitignore: {}", e);
    }

    // Read team config
    let team_dir = base.join(format!("_team_{}", safe_team));
    let team_config_path = team_dir.join("config.json");
    if !team_config_path.exists() {
        return Err(format!("Team '{}' not found (no config.json)", safe_team));
    }

    let team_config_str = std::fs::read_to_string(&team_config_path)
        .map_err(|e| format!("Failed to read team config: {}", e))?;
    let team_config: serde_json::Value = serde_json::from_str(&team_config_str)
        .map_err(|e| format!("Failed to parse team config: {}", e))?;

    // Determine next WG number
    let wg_number = determine_next_wg_number(&base, &safe_team);

    let wg_name = format!("wg-{}-{}", wg_number, safe_team);
    let wg_dir = base.join(&wg_name);
    if wg_dir.exists() {
        return Err(format!("Workgroup directory already exists: {}", wg_name));
    }
    std::fs::create_dir_all(&wg_dir)
        .map_err(|e| format!("Failed to create workgroup directory: {}", e))?;

    // BRIEF.md template
    let brief_content = format!(
        "# {}\n\n## Objective\n\n_Describe the goal of this workgroup._\n\n## Scope\n\n_What is in and out of scope._\n\n## Deliverables\n\n- [ ] _List deliverables here_\n",
        wg_name
    );
    std::fs::write(wg_dir.join("BRIEF.md"), &brief_content)
        .map_err(|e| format!("Failed to write BRIEF.md: {}", e))?;

    // Parse team agents and repos
    let team_agents: Vec<String> = team_config
        .get("agents")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let team_repos: Vec<RepoAssignment> = team_config
        .get("repos")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let url = v.get("url")?.as_str()?.to_string();
                    let agents = v
                        .get("agents")
                        .and_then(|a| a.as_array())
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    Some(RepoAssignment { url, agents })
                })
                .collect()
        })
        .unwrap_or_default();

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
        let agent_dir_name = Path::new(agent_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(agent_path);

        // Extract the clean agent name (strip _agent_ prefix)
        let agent_name = agent_dir_name
            .strip_prefix("_agent_")
            .unwrap_or(agent_dir_name);

        let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));
        std::fs::create_dir_all(&replica_dir)
            .map_err(|e| format!("Failed to create replica dir for {}: {}", agent_name, e))?;

        // inbox/ and outbox/
        for sub in &["inbox", "outbox"] {
            std::fs::create_dir_all(replica_dir.join(sub))
                .map_err(|e| format!("Failed to create {} for {}: {}", sub, agent_name, e))?;
        }

        // Determine repos assigned to this agent (match by _agent_ name)
        let assigned_repos: Vec<String> = team_repos
            .iter()
            .filter(|r| r.agents.iter().any(|a| agent_matches(a, agent_name)))
            .filter_map(|r| {
                let dir_name = format!("repo-{}", repo_dir_name_from_url(&r.url));
                Some(format!("../{}", dir_name))
            })
            .collect();

        // Compute relative identity path from replica to matrix
        let identity_rel = compute_relative_identity(agent_path, &replica_dir, &base);

        let mut context_entries: Vec<String> = vec![
            "$AGENTSCOMMANDER_CONTEXT".to_string(),
            "$REPOS_WORKSPACE_INFO".to_string(),
        ];
        // Resolve agent_path against base (.ac-new) for relative paths
        let matrix_dir = if Path::new(agent_path).is_absolute() {
            Path::new(agent_path).to_path_buf()
        } else {
            base.join(agent_path.trim_start_matches("../").trim_start_matches("./"))
        };
        if matrix_dir.join("Role.md").exists() {
            context_entries.push(format!("{}/Role.md", identity_rel));
        }

        let replica_config = serde_json::json!({
            "identity": identity_rel,
            "repos": assigned_repos,
            "context": context_entries,
        });

        let config_str = serde_json::to_string_pretty(&replica_config)
            .map_err(|e| format!("Failed to serialize replica config: {}", e))?;
        std::fs::write(replica_dir.join("config.json"), &config_str)
            .map_err(|e| format!("Failed to write replica config: {}", e))?;
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

    let result_path = wg_dir.to_string_lossy().to_string();
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

/// Delete a team directory from {project_path}/.ac-new/_team_{name}/
#[tauri::command]
pub async fn delete_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
) -> Result<(), String> {
    validate_existing_name(&team_name, "Team")?;
    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

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

    // Then delete workgroups
    for wg_dir in &wg_dirs {
        let wg_name = wg_dir.file_name().unwrap_or_default().to_string_lossy();
        if let Err(e) = std::fs::remove_dir_all(wg_dir) {
            log::warn!("[entity_creation] Failed to delete workgroup {}: {}", wg_name, e);
        } else {
            log::info!("[entity_creation] Deleted workgroup: {}", wg_name);
        }
    }
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(())
}

/// Delete a single workgroup directory from {project_path}/.ac-new/{wg_name}/
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

    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

    let wg_dir = base.join(&workgroup_name);
    if !wg_dir.exists() {
        return Err(format!("Workgroup '{}' not found", workgroup_name));
    }

    // Safety check: detect dirty repos before deleting (skip if force)
    if !force.unwrap_or(false) {
        let dirty_repos = check_workgroup_repos_dirty(&[wg_dir.clone()]);
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

    std::fs::remove_dir_all(&wg_dir)
        .map_err(|e| format!("Failed to delete workgroup directory: {}", e))?;
    log::info!(
        "[entity_creation] Deleted workgroup: {} (force={})",
        workgroup_name,
        force.unwrap_or(false)
    );
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(())
}

/// Update an existing team's config.json in {project_path}/.ac-new/_team_{name}/
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
    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    if !coordinator.is_empty() && !agents.contains(&coordinator) {
        return Err("Coordinator must be one of the selected agents".into());
    }

    let repos_json: Vec<serde_json::Value> = repos
        .iter()
        .map(|r| {
            serde_json::json!({
                "url": r.url,
                "agents": r.agents,
            })
        })
        .collect();

    let config = serde_json::json!({
        "agents": agents,
        "coordinator": coordinator,
        "repos": repos_json,
    });

    let config_str = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config.json: {}", e))?;
    std::fs::write(team_dir.join("config.json"), &config_str)
        .map_err(|e| format!("Failed to write config.json: {}", e))?;

    log::info!("[entity_creation] Updated team: {}", team_name);

    // Propagate repo changes to existing workgroups (async now — awaits SessionManager refresh).
    match sync_workgroup_repos_inner(
        &base,
        &team_name,
        &repos,
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
                result.workgroups_updated, result.replicas_updated, team_name, result.errors.len()
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
    let s = abs.to_string_lossy();
    let source_path = s
        .strip_prefix(r"\\?\")
        .unwrap_or(&s)
        .to_string();
    let dir = source_path
        .replace('\\', "/")
        .split('/')
        .next_back()
        .unwrap_or("")
        .to_string();
    let label = dir
        .strip_prefix("repo-")
        .map(str::to_string)
        .unwrap_or(dir);
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
                    e.path().is_dir()
                        && e.file_name().to_string_lossy().starts_with("__agent_")
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
            let mut config: serde_json::Value = match std::fs::read_to_string(&config_path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(v) => v,
                    Err(e) => {
                        result.errors.push(SyncError {
                            replica: dir_name.to_string(),
                            error: format!("Failed to parse config.json: {}", e),
                        });
                        continue;
                    }
                },
                Err(e) => {
                    result.errors.push(SyncError {
                        replica: dir_name.to_string(),
                        error: format!("Failed to read config.json: {}", e),
                    });
                    continue;
                }
            };

            // Update repos
            config["repos"] = serde_json::json!(assigned_repos);

            // Context merge: prepend required tokens to maintain consistent ordering
            // with create_workgroup() (which writes [$AC_CONTEXT, $REPOS_INFO] first).
            // Preserve any custom entries that were added via set_replica_context_files().
            let existing_context: Vec<String> = config
                .get("context")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let required = ["$AGENTSCOMMANDER_CONTEXT", "$REPOS_WORKSPACE_INFO"];
            let mut new_context: Vec<String> =
                required.iter().map(|s| s.to_string()).collect();
            for entry in &existing_context {
                if !required.contains(&entry.as_str()) {
                    new_context.push(entry.clone());
                }
            }

            // Auto-inject Role.md from identity if present and not already included
            if let Some(identity) = config.get("identity").and_then(|v| v.as_str()) {
                let role_entry = format!("{}/Role.md", identity);
                if !new_context.contains(&role_entry) {
                    let role_abs = replica_dir.join(&role_entry);
                    if role_abs.exists() {
                        new_context.push(role_entry);
                    }
                }
            }

            config["context"] = serde_json::json!(new_context);

            // Write back
            match serde_json::to_string_pretty(&config) {
                Ok(serialized) => {
                    if let Err(e) = std::fs::write(&config_path, &serialized) {
                        result.errors.push(SyncError {
                            replica: dir_name.to_string(),
                            error: format!("Failed to write config.json: {}", e),
                        });
                        continue;
                    }
                }
                Err(e) => {
                    result.errors.push(SyncError {
                        replica: dir_name.to_string(),
                        error: format!("Failed to serialize config.json: {}", e),
                    });
                    continue;
                }
            }

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

    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    // Read team config and parse repo assignments
    let config_content = std::fs::read_to_string(team_dir.join("config.json"))
        .map_err(|e| format!("Failed to read team config: {}", e))?;
    let config: serde_json::Value = serde_json::from_str(&config_content)
        .map_err(|e| format!("Failed to parse team config: {}", e))?;

    let repos: Vec<RepoAssignment> = config
        .get("repos")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let url = v.get("url")?.as_str()?.to_string();
                    let agents = v
                        .get("agents")
                        .and_then(|a| a.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(RepoAssignment { url, agents })
                })
                .collect()
        })
        .unwrap_or_default();

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
    let base = Path::new(&project_path).join(".ac-new");
    if !base.is_dir() {
        return Err(format!(".ac-new directory not found in {}", project_path));
    }

    let team_dir = base.join(format!("_team_{}", team_name));
    let config_path = team_dir.join("config.json");
    if !config_path.exists() {
        return Err(format!("Team '{}' config not found", team_name));
    }

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.json: {}", e))?;
    let result: TeamConfigResult = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse config.json: {}", e))?;

    Ok(result)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check all repo-* dirs inside the given workgroup dirs for dirty git state.
/// Returns a list of (repo_display_name, reason) for repos with pending work.
fn check_workgroup_repos_dirty(wg_dirs: &[PathBuf]) -> Vec<(String, String)> {
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

/// Scan .ac-new/ for existing wg-*-{team_name}/ dirs and return the next N.
fn determine_next_wg_number(ac_new_dir: &Path, team_name: &str) -> u32 {
    let suffix = format!("-{}", team_name);
    let mut max_n: u32 = 0;

    if let Ok(entries) = std::fs::read_dir(ac_new_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("wg-") && name_str.ends_with(&suffix) {
                // Extract the number between "wg-" and "-{team_name}"
                let middle = &name_str[3..name_str.len() - suffix.len()];
                if let Ok(n) = middle.parse::<u32>() {
                    if n > max_n {
                        max_n = n;
                    }
                }
            }
        }
    }

    max_n + 1
}

/// Compute a relative path from the replica dir to the agent matrix.
/// If the agent path is absolute, compute relative; otherwise return as-is.
fn compute_relative_identity(agent_path: &str, replica_dir: &Path, ac_new_dir: &Path) -> String {
    let agent = Path::new(agent_path);

    // If it's already a relative path within the same .ac-new/, make it relative to replica
    if agent.is_relative() {
        // agent_path is like "../_agent_foo" or "_agent_foo"
        // From replica inside wg-N-team/ we need to go ../../_agent_foo
        let agent_in_ac_new = ac_new_dir.join(
            agent_path.trim_start_matches("../").trim_start_matches("./"),
        );
        if let Ok(rel) = pathdiff_relative(replica_dir, &agent_in_ac_new) {
            return rel;
        }
        return format!("../../{}", agent_path.trim_start_matches("../").trim_start_matches("./"));
    }

    // Absolute path — try to make relative
    if let Ok(rel) = pathdiff_relative(replica_dir, agent) {
        return rel;
    }

    // Fallback: return absolute
    agent_path.to_string()
}

/// Simple relative path computation (from → to).
/// Strips Windows UNC prefix (\\?\) from canonicalized paths to ensure consistent comparison.
fn pathdiff_relative(from: &Path, to: &Path) -> Result<String, String> {
    // Canonicalize and strip UNC prefix for consistent comparison on Windows
    let strip_unc = |p: PathBuf| -> PathBuf {
        let s = p.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            p
        }
    };

    let from_abs = strip_unc(std::fs::canonicalize(from).unwrap_or_else(|_| from.to_path_buf()));
    let to_abs = if to.exists() {
        strip_unc(std::fs::canonicalize(to).unwrap_or_else(|_| to.to_path_buf()))
    } else {
        to.to_path_buf()
    };

    let from_components: Vec<_> = from_abs.components().collect();
    let to_components: Vec<_> = to_abs.components().collect();

    // Find common prefix length
    let common = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    if common == 0 {
        return Err("No common path prefix".into());
    }

    let ups = from_components.len() - common;
    let mut result = PathBuf::new();
    for _ in 0..ups {
        result.push("..");
    }
    for comp in &to_components[common..] {
        result.push(comp.as_os_str());
    }

    Ok(result.to_string_lossy().replace('\\', "/"))
}

/// Async git clone with CREATE_NO_WINDOW on Windows.
async fn git_clone_async(url: &str, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["-c", "core.longpaths=true", "clone", "--depth", "1", url])
        .arg(target.as_os_str());

    #[cfg(windows)]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to spawn git clone: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        // Cap error message length to avoid sending huge progress output to frontend
        let capped = if trimmed.len() > 512 { &trimmed[..512] } else { trimmed };
        return Err(format!("git clone failed: {}", capped));
    }

    if !target.join(".git").join("index").exists() {
        log::warn!(
            "[entity_creation] .git/index missing after clone for {}, running fallback git reset",
            url
        );
        let mut reset_cmd = tokio::process::Command::new("git");
        reset_cmd.args(["reset"]).current_dir(target);
        #[cfg(windows)]
        {
            #[allow(unused_imports)]
            use std::os::windows::process::CommandExt;
            reset_cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let _ = reset_cmd.output().await;
    }

    Ok(())
}
