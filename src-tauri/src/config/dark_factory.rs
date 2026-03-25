use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub name: String,
    pub members: Vec<TeamMember>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DarkFactoryConfig {
    pub teams: Vec<Team>,
}

/// Per-agent config written to <agent-path>/.agentscommander/config.json
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLocalConfig {
    pub teams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub is_coordinator_of: Vec<String>,
    /// Last coding agent CLI used in this repo (e.g., "claude", "codex").
    /// Used as fallback for --agent auto in wake-and-sleep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_coding_agent: Option<String>,
}

/// Update lastCodingAgent in a repo's .agentscommander/config.json.
/// Reads existing config, merges the field, writes back.
pub fn set_last_coding_agent(repo_path: &str, agent_id: &str) -> Result<(), String> {
    let config_dir = std::path::Path::new(repo_path).join(".agentscommander");
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create .agentscommander dir: {}", e))?;

    let config_path = config_dir.join("config.json");

    // Read existing or create default
    let mut config: AgentLocalConfig = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config: {}", e))?;
        serde_json::from_str(&content).unwrap_or(AgentLocalConfig {
            teams: vec![],
            is_coordinator_of: vec![],
            last_coding_agent: None,
        })
    } else {
        AgentLocalConfig {
            teams: vec![],
            is_coordinator_of: vec![],
            last_coding_agent: None,
        }
    };

    config.last_coding_agent = Some(agent_id.to_string());

    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    std::fs::write(&config_path, json)
        .map_err(|e| format!("Failed to write config: {}", e))?;

    log::info!("Updated lastCodingAgent to '{}' in {:?}", agent_id, config_path);
    Ok(())
}

/// Returns the app config dir (delegates to config::config_dir)
fn dark_factory_dir() -> Option<PathBuf> {
    super::config_dir()
}

/// Returns ~/.agentscommander/teams.json
fn teams_path() -> Option<PathBuf> {
    dark_factory_dir().map(|d| d.join("teams.json"))
}

/// Load teams config from ~/.agentscommander/teams.json
pub fn load_dark_factory() -> DarkFactoryConfig {
    let path = match teams_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory for dark factory config");
            return DarkFactoryConfig::default();
        }
    };

    if !path.exists() {
        return DarkFactoryConfig::default();
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<DarkFactoryConfig>(&contents) {
            Ok(config) => {
                log::info!("Loaded dark factory config from {:?}", path);
                config
            }
            Err(e) => {
                log::error!("Failed to parse dark factory config: {}", e);
                DarkFactoryConfig::default()
            }
        },
        Err(e) => {
            log::error!("Failed to read dark factory config: {}", e);
            DarkFactoryConfig::default()
        }
    }
}

/// Save teams config to ~/.agentscommander/teams.json
pub fn save_dark_factory(config: &DarkFactoryConfig) -> Result<(), String> {
    let dir = dark_factory_dir().ok_or("Could not determine home directory")?;
    let path = dir.join("teams.json");

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create .agentscommander directory: {}", e))?;

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize dark factory config: {}", e))?;

    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write dark factory config: {}", e))?;

    log::info!("Saved dark factory config to {:?}", path);
    Ok(())
}

/// Sync per-agent .agentscommander/config.json for all members across all teams
pub fn sync_agent_configs(config: &DarkFactoryConfig) -> Result<(), String> {
    // Build a map: agent_path -> (teams, coordinator_of)
    let mut agent_map: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();

    for team in &config.teams {
        for member in &team.members {
            let entry = agent_map
                .entry(member.path.clone())
                .or_insert_with(|| (Vec::new(), Vec::new()));
            entry.0.push(team.name.clone());

            if team.coordinator_name.as_deref() == Some(&member.name) {
                entry.1.push(team.name.clone());
            }
        }
    }

    for (agent_path, (teams, coordinator_of)) in &agent_map {
        let config_dir = Path::new(agent_path).join(".agentscommander");

        // Preserve existing lastCodingAgent if present
        let existing_last_agent = config_dir
            .join("config.json")
            .to_str()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|c| serde_json::from_str::<AgentLocalConfig>(&c).ok())
            .and_then(|c| c.last_coding_agent);

        let agent_config = AgentLocalConfig {
            teams: teams.clone(),
            is_coordinator_of: coordinator_of.clone(),
            last_coding_agent: existing_last_agent,
        };
        if let Err(e) = std::fs::create_dir_all(&config_dir) {
            log::warn!(
                "Failed to create .agentscommander dir at {:?}: {}",
                config_dir, e
            );
            continue;
        }

        let config_path = config_dir.join("config.json");
        match serde_json::to_string_pretty(&agent_config) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&config_path, json) {
                    log::warn!("Failed to write agent config at {:?}: {}", config_path, e);
                }
            }
            Err(e) => {
                log::warn!("Failed to serialize agent config: {}", e);
            }
        }
    }

    Ok(())
}
