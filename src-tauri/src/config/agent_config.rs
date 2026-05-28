use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

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

// ── Legacy Dark Factory fields (kept for backwards-compatible deserialization) ──
/// Preserved so existing config.json files with a "darkFactory" key can still be read.
/// No longer written or used for routing - teams come from discovery.
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
    /// Legacy field - kept for backwards-compatible reads of old config.json files.
    /// No longer used for routing. Teams are discovered from _team_*/config.json.
    #[serde(default, skip_serializing_if = "AgentDarkFactory::is_empty")]
    pub dark_factory: AgentDarkFactory,
}

/// Update lastCodingAgent and codingAgents in a repo's config.
/// Writes to BOTH:
///  - `<repo_path>/config.json` (root, shared across all instances - read by discovery)
///  - `<repo_path>/<agent_local_dir>/config.json` (per-instance)
///
/// Reads existing config, upserts the coding agent entry, writes back.
pub fn set_last_coding_agent(
    repo_path: &str,
    agent_id: &str,
    app_label: &str,
    ac_session_id: Option<&str>,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    let entry = CodingAgentEntry {
        app: app_label.to_string(),
        ac_session_id: ac_session_id.map(|s| s.to_string()),
        last_used: now,
    };

    // Write to per-instance config dir
    let local_dir_name = crate::config::agent_local_dir_name();
    let instance_dir = Path::new(repo_path).join(local_dir_name.as_str());
    std::fs::create_dir_all(&instance_dir)
        .map_err(|e| format!("Failed to create {} dir: {}", local_dir_name, e))?;
    upsert_config(&instance_dir.join("config.json"), agent_id, &entry)?;

    // Also write to root config.json so discovery can find it regardless of instance
    upsert_config(&Path::new(repo_path).join("config.json"), agent_id, &entry)?;

    log::info!(
        "Updated lastCodingAgent to '{}' ({}) in {} + root config.json",
        agent_id,
        app_label,
        local_dir_name
    );
    Ok(())
}

/// Ensure a key in a JSON map is an object, inserting `{}` if missing or resetting if corrupted.
fn ensure_object<'a>(
    map: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    context: &Path,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    let val = map.entry(key).or_insert_with(|| serde_json::json!({}));
    if !val.is_object() {
        log::warn!(
            "upsert_config: '{}' was not an object at {:?}, resetting",
            key,
            context
        );
        *val = serde_json::json!({});
    }
    val.as_object_mut().expect("just set to object")
}

/// Read-modify-write a single config.json: upsert tooling fields while preserving all others.
/// Uses serde_json::Value to avoid dropping unknown top-level fields (e.g. `identity`, `repos`)
/// that aren't part of the AgentLocalConfig struct.
fn upsert_config(
    config_path: &Path,
    agent_id: &str,
    entry: &CodingAgentEntry,
) -> Result<(), String> {
    let mut root: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read config: {}", e))?;
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|e| {
            log::warn!(
                "Failed to parse config at {:?}, starting fresh: {}",
                config_path,
                e
            );
            serde_json::json!({})
        });
        if !parsed.is_object() {
            log::warn!(
                "upsert_config: root at {:?} is not an object, starting fresh",
                config_path
            );
            serde_json::json!({})
        } else {
            parsed
        }
    } else {
        serde_json::json!({})
    };

    let obj = root.as_object_mut().expect("guaranteed object above");

    let tooling = ensure_object(obj, "tooling", config_path);
    tooling.insert("lastCodingAgent".to_string(), serde_json::json!(agent_id));

    let coding_agents = ensure_object(tooling, "codingAgents", config_path);
    let entry_val =
        serde_json::to_value(entry).map_err(|e| format!("Failed to serialize entry: {}", e))?;
    coding_agents.insert(agent_id.to_string(), entry_val);

    let json = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    std::fs::write(config_path, json).map_err(|e| format!("Failed to write config: {}", e))?;

    Ok(())
}
