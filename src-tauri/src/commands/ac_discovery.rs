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
pub struct AcDiscoveryResult {
    pub agents: Vec<AcAgentMatrix>,
    pub teams: Vec<AcTeam>,
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

/// Discover AC-new agent matrices from .ac-new/ directories within configured repo paths.
#[tauri::command]
pub async fn discover_ac_agents(
    settings: State<'_, SettingsState>,
) -> Result<AcDiscoveryResult, String> {
    let cfg = settings.read().await;
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();

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

                    agents.push(AcAgentMatrix {
                        name: display_name,
                        path: path.to_string_lossy().to_string(),
                        role_exists,
                        preferred_agent_id,
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

    // Sort agents alphabetically
    agents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    teams.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(AcDiscoveryResult { agents, teams })
}
