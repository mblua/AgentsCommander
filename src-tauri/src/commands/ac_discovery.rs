use serde::Serialize;
use std::path::Path;
use tauri::State;

use crate::config::settings::SettingsState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcAgentMatrix {
    /// Display name: "{project_folder}/{agent_name}" with _agent_ prefix stripped
    pub name: String,
    /// Absolute path to the agent matrix directory
    pub path: String,
    /// Whether Role.md exists in the agent directory
    pub role_exists: bool,
    /// Preferred coding agent ID from config.json tooling.lastCodingAgent
    pub preferred_agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcTeam {
    /// Team directory name with _team_ prefix stripped
    pub name: String,
    /// Agent display names belonging to this team
    pub agents: Vec<String>,
    /// Coordinator agent display name, if any
    pub coordinator: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcAgentReplica {
    /// Display name: agent dir name with __agent_ prefix stripped
    pub name: String,
    /// Absolute path to the replica agent directory
    pub path: String,
    /// Resolved identity path from config.json "identity" field
    pub identity_path: Option<String>,
    /// Preferred coding agent ID inherited from the identity matrix
    pub preferred_agent_id: Option<String>,
    /// Absolute paths to repos this replica works on (resolved from config.json "repos")
    pub repo_paths: Vec<String>,
    /// Git branch of the first repo (if exactly one repo), for sidebar display
    pub repo_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcWorkgroup {
    /// Workgroup name (wg-* dir name)
    pub name: String,
    /// Absolute path to the workgroup directory
    pub path: String,
    /// First line of BRIEF.md (if exists)
    pub brief: Option<String>,
    /// Replica agents inside this workgroup
    pub agents: Vec<AcAgentReplica>,
    /// Absolute path to the first repo-* directory found (for CWD)
    pub repo_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcDiscoveryResult {
    pub agents: Vec<AcAgentMatrix>,
    pub teams: Vec<AcTeam>,
    pub workgroups: Vec<AcWorkgroup>,
}

/// Derive agent display name from its path.
/// Format: "{project_folder}/{agent_name}" where:
/// - project_folder = directory containing .ac-new/
/// - agent_name = folder name with "_agent_" prefix stripped
fn agent_display_name(project_folder: &str, dir_name: &str) -> String {
    let agent_name = dir_name
        .strip_prefix("_agent_")
        .unwrap_or(dir_name);
    format!("{}/{}", project_folder, agent_name)
}

/// Resolve a relative agent ref (e.g. "../_agent_tech-lead") to a display name.
fn resolve_agent_ref(project_folder: &str, agent_ref: &str) -> String {
    let dir_name = agent_ref
        .trim_start_matches("../")
        .trim_start_matches("./");
    agent_display_name(project_folder, dir_name)
}

/// Detect git branch synchronously for a given directory path.
fn detect_git_branch_sync(dir: &str) -> Option<String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new("git");
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() || branch == "HEAD" {
                None
            } else {
                Some(branch)
            }
        }
        _ => None,
    }
}

/// Discover AC-new agent matrices from .ac-new/ directories within configured repo paths.
#[tauri::command]
pub async fn discover_ac_agents(
    settings: State<'_, SettingsState>,
) -> Result<AcDiscoveryResult, String> {
    let cfg = settings.read().await;
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();

    for base_path in &cfg.repo_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }

        // Also check children of the base path (same pattern as search_repos)
        let dirs_to_check: Vec<std::path::PathBuf> = {
            let mut dirs = vec![base.to_path_buf()];
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !name.starts_with('.') {
                            dirs.push(p);
                        }
                    }
                }
            }
            dirs
        };

        for repo_dir in dirs_to_check {
            let ac_new_dir = repo_dir.join(".ac-new");
            if !ac_new_dir.is_dir() {
                continue;
            }

            let project_folder = repo_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let entries = match std::fs::read_dir(&ac_new_dir) {
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

                // Agent matrices: _agent_* (single underscore prefix)
                if dir_name.starts_with("_agent_") {
                    let display_name = agent_display_name(&project_folder, &dir_name);
                    let role_exists = path.join("Role.md").exists();

                    let preferred_agent_id = path.join("config.json")
                        .exists()
                        .then(|| std::fs::read_to_string(path.join("config.json")).ok())
                        .flatten()
                        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                        .and_then(|v| v.get("tooling")?.get("lastCodingAgent")?.as_str().map(String::from));

                    log::info!("[BUG#1] AC Discovery agent: dir={:?}, preferred_agent_id={:?}", dir_name, preferred_agent_id);

                    agents.push(AcAgentMatrix {
                        name: display_name,
                        path: path.to_string_lossy().to_string(),
                        role_exists,
                        preferred_agent_id,
                    });
                }

                // Workgroups: wg-*
                if dir_name.starts_with("wg-") {
                    let brief = path.join("BRIEF.md")
                        .exists()
                        .then(|| std::fs::read_to_string(path.join("BRIEF.md")).ok())
                        .flatten()
                        .and_then(|content| content.lines().next().map(|l| l.trim_start_matches("# ").to_string()));

                    // Find first repo-* directory for CWD
                    let repo_path = std::fs::read_dir(&path)
                        .ok()
                        .and_then(|entries| {
                            entries.flatten().find(|e| {
                                let n = e.file_name();
                                let name = n.to_string_lossy();
                                name.starts_with("repo-") && e.path().is_dir()
                            })
                        })
                        .map(|e| e.path().to_string_lossy().to_string());

                    // Scan __agent_* replicas inside the WG
                    let mut wg_agents: Vec<AcAgentReplica> = Vec::new();
                    if let Ok(wg_entries) = std::fs::read_dir(&path) {
                        for wg_entry in wg_entries.flatten() {
                            let wg_path = wg_entry.path();
                            if !wg_path.is_dir() {
                                continue;
                            }
                            let wg_dir_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                                Some(n) => n.to_string(),
                                None => continue,
                            };
                            if wg_dir_name.starts_with("__agent_") {
                                let replica_name = wg_dir_name
                                    .strip_prefix("__agent_")
                                    .unwrap_or(&wg_dir_name)
                                    .to_string();

                                let replica_config = wg_path.join("config.json")
                                    .exists()
                                    .then(|| std::fs::read_to_string(wg_path.join("config.json")).ok())
                                    .flatten()
                                    .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());

                                let identity_path = replica_config.as_ref()
                                    .and_then(|v| v.get("identity")?.as_str().map(String::from));

                                // Resolve identity to matrix dir and read its lastCodingAgent
                                let preferred_agent_id = identity_path.as_ref().and_then(|rel| {
                                    let matrix_dir = wg_path.join(rel);
                                    let matrix_config = matrix_dir.join("config.json");
                                    std::fs::read_to_string(&matrix_config).ok()
                                        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                                        .and_then(|v| v.get("tooling")?.get("lastCodingAgent")?.as_str().map(String::from))
                                });

                                // Extract repos from config.json and resolve to absolute paths
                                let repo_paths: Vec<String> = replica_config.as_ref()
                                    .and_then(|v| v.get("repos")?.as_array().cloned())
                                    .unwrap_or_default()
                                    .iter()
                                    .filter_map(|r| r.as_str())
                                    .filter_map(|rel| {
                                        let resolved = wg_path.join(rel);
                                        std::fs::canonicalize(&resolved).ok()
                                            .map(|p| {
                                                let s = p.to_string_lossy();
                                                // Strip \\?\ UNC prefix that canonicalize adds on Windows
                                                s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
                                            })
                                    })
                                    .collect();

                                // Detect git branch for single-repo replicas
                                let repo_branch = if repo_paths.len() == 1 {
                                    detect_git_branch_sync(&repo_paths[0])
                                } else {
                                    None
                                };

                                wg_agents.push(AcAgentReplica {
                                    name: replica_name,
                                    path: wg_path.to_string_lossy().to_string(),
                                    identity_path,
                                    preferred_agent_id,
                                    repo_paths,
                                    repo_branch,
                                });
                            }
                        }
                    }
                    wg_agents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

                    workgroups.push(AcWorkgroup {
                        name: dir_name.clone(),
                        path: path.to_string_lossy().to_string(),
                        brief,
                        agents: wg_agents,
                        repo_path,
                    });
                }

                // Teams: _team_*
                if dir_name.starts_with("_team_") {
                    let team_name = dir_name
                        .strip_prefix("_team_")
                        .unwrap_or(&dir_name)
                        .to_string();

                    let config_path = path.join("config.json");
                    if let Ok(content) = std::fs::read_to_string(&config_path) {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                            let team_agents = parsed
                                .get("agents")
                                .and_then(|a| a.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str())
                                        .map(|r| resolve_agent_ref(&project_folder, r))
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();

                            let coordinator = parsed
                                .get("coordinator")
                                .and_then(|c| c.as_str())
                                .map(|r| resolve_agent_ref(&project_folder, r));

                            teams.push(AcTeam {
                                name: team_name,
                                agents: team_agents,
                                coordinator,
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort alphabetically
    agents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    teams.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    workgroups.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(AcDiscoveryResult { agents, teams, workgroups })
}
