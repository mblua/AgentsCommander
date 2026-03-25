use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct ListPeersArgs {
    /// Session token for authentication
    #[arg(long)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentLocalConfig {
    #[serde(default)]
    teams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    is_coordinator_of: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_coding_agent: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerInfo {
    name: String,
    path: String,
    status: String,
    role: String,
    teams: Vec<String>,
    last_coding_agent: Option<String>,
}

/// Resolve the current repo's .agentscommander directory (walks up from cwd).
fn find_ac_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let ac = dir.join(".agentscommander");
        if ac.is_dir() {
            return Some(ac);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Get the agent name (parent/repo) from a path.
fn agent_name_from_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if components.len() >= 2 {
        format!(
            "{}/{}",
            components[components.len() - 2],
            components[components.len() - 1]
        )
    } else {
        normalized
    }
}

/// Read role from CLAUDE.md: extract ## Role Prompt section, or first 5 lines.
fn read_role(repo_path: &str) -> String {
    let claude_md = Path::new(repo_path).join("CLAUDE.md");
    let content = match std::fs::read_to_string(&claude_md) {
        Ok(c) => c,
        Err(_) => return "No role description available.".to_string(),
    };

    // Try to extract ## Role Prompt section
    let lines: Vec<&str> = content.lines().collect();
    let mut in_role = false;
    let mut role_lines = Vec::new();

    for line in &lines {
        if line.starts_with("## Role Prompt") || line.starts_with("## Role") {
            in_role = true;
            continue;
        }
        if in_role {
            if line.starts_with("## ") || line.starts_with("---") {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                role_lines.push(trimmed);
            }
        }
    }

    if !role_lines.is_empty() {
        // Return up to first 3 non-empty lines for conciseness
        return role_lines.into_iter().take(3).collect::<Vec<_>>().join(" ");
    }

    // Fallback: first 5 non-empty lines
    let first_lines: Vec<&str> = lines
        .iter()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .take(5)
        .copied()
        .collect();

    if first_lines.is_empty() {
        "No role description available.".to_string()
    } else {
        first_lines.join(" ")
    }
}

/// Load the teams.json from the global config directory.
fn load_teams_config() -> Option<serde_json::Value> {
    let home = dirs::home_dir()?;
    let dir_name = if cfg!(debug_assertions) {
        ".agentscommander-dev"
    } else {
        ".agentscommander"
    };
    let teams_path = home.join(dir_name).join("teams.json");
    let content = std::fs::read_to_string(teams_path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn execute(args: ListPeersArgs) -> i32 {
    let ac_dir = match find_ac_dir() {
        Some(d) => d,
        None => {
            eprintln!("Error: no .agentscommander directory found");
            return 1;
        }
    };

    // Read our own config
    let config_path = ac_dir.join("config.json");
    let my_config: AgentLocalConfig = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or(AgentLocalConfig {
            teams: vec![],
            is_coordinator_of: vec![],
            last_coding_agent: None,
        });

    let my_repo_path = ac_dir.parent().unwrap_or(Path::new("."));
    let my_name = agent_name_from_path(&my_repo_path.to_string_lossy());

    let mut peers: Vec<PeerInfo> = Vec::new();

    if my_config.teams.is_empty() {
        // No teams → no peers. Only team members can communicate.
        println!("[]");
        return 0;
    }

    // Show all members of our teams
    if let Some(teams_json) = load_teams_config() {
        if let Some(teams) = teams_json.get("teams").and_then(|t| t.as_array()) {
            for team in teams {
                let team_name = team.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if !my_config.teams.contains(&team_name.to_string()) {
                    continue;
                }

                if let Some(members) = team.get("members").and_then(|m| m.as_array()) {
                    for member in members {
                        let member_path = member.get("path").and_then(|p| p.as_str()).unwrap_or("");

                        // Skip ourselves
                        let peer_name = agent_name_from_path(member_path);
                        if peer_name == my_name {
                            continue;
                        }

                        // Skip duplicates — add team to existing peer
                        if peers.iter().any(|p| p.name == peer_name) {
                            if let Some(existing) = peers.iter_mut().find(|p| p.name == peer_name) {
                                if !existing.teams.contains(&team_name.to_string()) {
                                    existing.teams.push(team_name.to_string());
                                }
                            }
                            continue;
                        }

                        let peer_ac = Path::new(member_path).join(".agentscommander");
                        let status = if peer_ac.join("active").exists() {
                            "active"
                        } else {
                            "unknown"
                        };

                        let peer_config: AgentLocalConfig = peer_ac
                            .join("config.json")
                            .to_str()
                            .and_then(|p| std::fs::read_to_string(p).ok())
                            .and_then(|c| serde_json::from_str(&c).ok())
                            .unwrap_or(AgentLocalConfig {
                                teams: vec![],
                                is_coordinator_of: vec![],
                                last_coding_agent: None,
                            });

                        peers.push(PeerInfo {
                            name: peer_name,
                            path: member_path.to_string(),
                            status: status.to_string(),
                            role: read_role(member_path),
                            teams: vec![team_name.to_string()],
                            last_coding_agent: peer_config.last_coding_agent,
                        });
                    }
                }
            }
        }
    }

    // Output as JSON
    match serde_json::to_string_pretty(&peers) {
        Ok(json) => {
            println!("{}", json);
            let _ = args; // token validated if needed
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize peers: {}", e);
            1
        }
    }
}
