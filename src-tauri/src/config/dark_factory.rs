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
pub struct DarkFactoryLayer {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorLink {
    pub supervisor_team_id: String,
    pub subordinate_team_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub name: String,
    pub members: Vec<TeamMember>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_id: Option<String>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub visible: bool,
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DarkFactoryConfig {
    pub teams: Vec<Team>,
    #[serde(default)]
    pub layers: Vec<DarkFactoryLayer>,
    #[serde(default)]
    pub coordinator_links: Vec<CoordinatorLink>,
}

// ── Agent Identity ──────────────────────────────────────────────────────────
/// What the agent IS: name, role, memory location.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIdentity {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub root_path: String,
    /// Relative path to the role declaration file (e.g. "CLAUDE.md")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role_path: String,
    /// Relative path to the memory store (e.g. ".claude/memory")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub memory_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl AgentIdentity {
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.root_path.is_empty()
            && self.role_path.is_empty()
            && self.memory_path.is_empty()
            && self.description.is_empty()
    }
}

// ── Agent Tooling ──────────────────────────────────────────────────────────
/// Entry tracking a coding app (Claude Code, Codex, OpenCode, etc.) used in this repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentEntry {
    /// Human-readable app name (e.g. "Claude Code", "Codex", "OpenCode")
    #[serde(default)]
    pub app: String,
    /// AgentsCommander session ID (to check if session is still alive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ac_session_id: Option<String>,
    /// ISO 8601 timestamp of last use
    #[serde(default)]
    pub last_used: String,
}

/// Which coding apps have been used to run this agent, plus runtime config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTooling {
    /// Last agent config ID used (maps to AgentConfig.id in settings.json)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_coding_agent: Option<String>,
    /// Per-agent-config-id history of coding apps used in this repo
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub coding_agents: HashMap<String, CodingAgentEntry>,
    /// Telegram bot label to auto-attach when creating sessions for this agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot: Option<String>,
}

impl AgentTooling {
    pub fn is_empty(&self) -> bool {
        self.last_coding_agent.is_none()
            && self.coding_agents.is_empty()
            && self.telegram_bot.is_none()
    }
}

// ── Dark Factory (team coordination) ───────────────────────────────────────
/// Team structure managed by Dark Factory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDarkFactory {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub is_coordinator_of: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supervises: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports_to: Vec<String>,
}

impl AgentDarkFactory {
    pub fn is_empty(&self) -> bool {
        self.teams.is_empty()
            && self.is_coordinator_of.is_empty()
            && self.supervises.is_empty()
            && self.reports_to.is_empty()
    }
}

// ── Per-agent config (the root struct) ─────────────────────────────────────
/// Written to <agent-path>/.agentscommander/config.json
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLocalConfig {
    #[serde(default, skip_serializing_if = "AgentIdentity::is_empty")]
    pub agent: AgentIdentity,
    #[serde(default, skip_serializing_if = "AgentTooling::is_empty")]
    pub tooling: AgentTooling,
    #[serde(default, skip_serializing_if = "AgentDarkFactory::is_empty")]
    pub dark_factory: AgentDarkFactory,
}

/// Update lastCodingAgent and codingAgents in a repo's .agentscommander/config.json.
/// Reads existing config, upserts the coding agent entry, writes back.
pub fn set_last_coding_agent(
    repo_path: &str,
    agent_id: &str,
    app_label: &str,
    ac_session_id: Option<&str>,
) -> Result<(), String> {
    let local_dir_name = crate::config::agent_local_dir_name();
    let config_dir = std::path::Path::new(repo_path).join(local_dir_name.as_str());
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create {} dir: {}", local_dir_name, e))?;

    let config_path = config_dir.join("config.json");

    // Read existing or create default
    let mut config: AgentLocalConfig = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config: {}", e))?;
        serde_json::from_str(&content).unwrap_or_else(|e| {
            log::warn!("Failed to parse config at {:?}, starting fresh: {}", config_path, e);
            AgentLocalConfig::default()
        })
    } else {
        AgentLocalConfig::default()
    };

    config.tooling.last_coding_agent = Some(agent_id.to_string());

    // Upsert codingAgents entry
    let now = chrono::Utc::now().to_rfc3339();
    config.tooling.coding_agents.insert(
        agent_id.to_string(),
        CodingAgentEntry {
            app: app_label.to_string(),
            ac_session_id: ac_session_id.map(|s| s.to_string()),
            last_used: now,
        },
    );

    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    std::fs::write(&config_path, json)
        .map_err(|e| format!("Failed to write config: {}", e))?;

    log::info!("Updated lastCodingAgent to '{}' ({}) in {:?}", agent_id, app_label, config_path);
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
    // Validate coordinator_name membership
    let mut config = config.clone();
    for team in &mut config.teams {
        if let Some(ref coord) = team.coordinator_name {
            if !team.members.iter().any(|m| &m.name == coord) {
                log::warn!(
                    "coordinator_name '{}' is not a member of team '{}', clearing",
                    coord, team.name
                );
                team.coordinator_name = None;
            }
        }
    }

    // Validate no cycles in coordinator_links
    validate_no_cycles(&config)?;

    let dir = dark_factory_dir().ok_or("Could not determine home directory")?;
    let path = dir.join("teams.json");

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create .agentscommander directory: {}", e))?;

    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize dark factory config: {}", e))?;

    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write dark factory config: {}", e))?;

    log::info!("Saved dark factory config to {:?}", path);
    Ok(())
}

/// Check that coordinator_links form a DAG (no cycles)
fn validate_no_cycles(config: &DarkFactoryConfig) -> Result<(), String> {
    use std::collections::{HashSet, VecDeque};

    // Build adjacency: supervisor -> [subordinates]
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for link in &config.coordinator_links {
        adj.entry(link.supervisor_team_id.clone())
            .or_default()
            .push(link.subordinate_team_id.clone());
    }

    // BFS from each node to detect cycles
    for start in adj.keys() {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        for next in adj.get(start).unwrap_or(&vec![]) {
            queue.push_back(next.clone());
        }
        while let Some(node) = queue.pop_front() {
            if &node == start {
                let team_name = config.teams.iter()
                    .find(|t| t.id == *start)
                    .map(|t| t.name.as_str())
                    .unwrap_or(&start);
                return Err(format!(
                    "Cycle detected in coordinator links involving team '{}'",
                    team_name
                ));
            }
            if visited.insert(node.clone()) {
                for next in adj.get(&node).unwrap_or(&vec![]) {
                    queue.push_back(next.clone());
                }
            }
        }
    }

    Ok(())
}

/// Intermediate struct for building per-agent config
#[derive(Default)]
struct AgentSyncData {
    teams: Vec<String>,
    coordinator_of: Vec<String>,
    supervises: Vec<String>,
    reports_to: Vec<String>,
}

/// Sync per-agent .agentscommander/config.json for all members across all teams
pub fn sync_agent_configs(config: &DarkFactoryConfig) -> Result<(), String> {
    let mut agent_map: HashMap<String, AgentSyncData> = HashMap::new();

    // First pass: teams and coordinator_of
    for team in &config.teams {
        for member in &team.members {
            let entry = agent_map.entry(member.path.clone()).or_default();
            entry.teams.push(team.name.clone());

            if team.coordinator_name.as_deref() == Some(&member.name) {
                entry.coordinator_of.push(team.name.clone());
            }
        }
    }

    // Second pass: coordinator links → supervises / reports_to
    for link in &config.coordinator_links {
        let sup_team = config.teams.iter().find(|t| t.id == link.supervisor_team_id);
        let sub_team = config.teams.iter().find(|t| t.id == link.subordinate_team_id);

        let (sup, sub) = match (sup_team, sub_team) {
            (Some(s), Some(t)) => (s, t),
            _ => continue,
        };
        let sup_coord = match &sup.coordinator_name {
            Some(name) if sup.members.iter().any(|m| &m.name == name) => name,
            _ => continue,
        };
        let sub_coord = match &sub.coordinator_name {
            Some(name) if sub.members.iter().any(|m| &m.name == name) => name,
            _ => {
                log::warn!("CoordinatorLink skip: team '{}' has no valid coordinator", sub.name);
                continue;
            }
        };

        let sup_path = sup.members.iter().find(|m| &m.name == sup_coord).map(|m| &m.path);
        let sub_path = sub.members.iter().find(|m| &m.name == sub_coord).map(|m| &m.path);

        if let Some(path) = sup_path {
            agent_map.entry(path.clone()).or_default()
                .supervises.push(sub.name.clone());
        }
        if let Some(path) = sub_path {
            agent_map.entry(path.clone()).or_default()
                .reports_to.push(sup.name.clone());
        }
    }

    // Write per-agent configs
    for (agent_path, data) in &agent_map {
        let config_dir = Path::new(agent_path).join(crate::config::agent_local_dir_name());

        // Preserve existing agent identity and tooling sections
        let existing = config_dir
            .join("config.json")
            .to_str()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|c| serde_json::from_str::<AgentLocalConfig>(&c).ok());

        let agent_config = AgentLocalConfig {
            agent: existing.as_ref().map(|c| c.agent.clone()).unwrap_or_default(),
            tooling: existing.as_ref().map(|c| c.tooling.clone()).unwrap_or_default(),
            dark_factory: AgentDarkFactory {
                teams: data.teams.clone(),
                is_coordinator_of: data.coordinator_of.clone(),
                supervises: data.supervises.clone(),
                reports_to: data.reports_to.clone(),
            },
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
