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
    /// Selection UI coding agent assignment. Does not replace lastCodingAgent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_coding_agent: Option<String>,
    /// Selection UI profile assignment. Legacy instanceProfileOverride is
    /// read separately from raw JSON during the migration window.
    #[serde(
        default,
        alias = "instanceProfileOverride",
        skip_serializing_if = "Option::is_none"
    )]
    pub profile: Option<String>,
    /// Per-agent-config-id history of coding apps used in this repo
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub coding_agents: HashMap<String, CodingAgentEntry>,
    /// #1682 - RFC3339/UTC instant of the most recent busy->idle edge on a session
    /// that a message write armed and that the stamp gates judged an agent turn. That
    /// is this plan's proxy for the coding agent having finished responding, NOT a
    /// proof of it: R7 and R8 arm with nothing submitted. Distinct from
    /// `codingAgents[<id>].lastUsed`, which is when a coding agent was last LAUNCHED.
    /// Written only by `set_last_agent_message_at`; read only by the terminal status strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_agent_message_at: Option<String>,
    /// Telegram bot label to auto-attach when creating sessions for this agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot: Option<String>,
}

impl AgentTooling {
    pub fn is_empty(&self) -> bool {
        self.last_coding_agent.is_none()
            && self.current_coding_agent.is_none()
            && self.profile.is_none()
            && self.coding_agents.is_empty()
            && self.last_agent_message_at.is_none()
            && self.telegram_bot.is_none()
    }
}

// ── Legacy Dark Factory fields (kept for backwards-compatible deserialization) ──
/// Preserved so existing config.json files with a "darkFactory" key can still be read.
/// No longer written or used for routing — teams come from discovery.
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
    /// Legacy field — kept for backwards-compatible reads of old config.json files.
    /// No longer used for routing. Teams are discovered from _team_*/config.json.
    #[serde(default, skip_serializing_if = "AgentDarkFactory::is_empty")]
    pub dark_factory: AgentDarkFactory,
}

/// Update lastCodingAgent and codingAgents in a repo's config.
/// Writes to BOTH:
///  - `<repo_path>/config.json` (root, shared across all instances — read by discovery)
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

/// #1682 - record `at_rfc3339`, the instant a busy->idle edge closed an armed
/// turn for the coding agent in `repo_path`. The caller judges that, and R7 and
/// R8 mean an armed turn is not proof the agent responded. Writes ONLY the
/// per-instance config; the root `config.json` is deliberately not touched (see
/// D2). Monotonic: a stored value that is already at or after `at_rfc3339` is
/// kept. Returns whether the file now carries `at_rfc3339`.
pub fn set_last_agent_message_at(repo_path: &str, at_rfc3339: &str) -> Result<bool, String> {
    let local_dir_name = crate::config::agent_local_dir_name();
    let instance_dir = Path::new(repo_path).join(local_dir_name.as_str());
    std::fs::create_dir_all(&instance_dir)
        .map_err(|e| format!("Failed to create {} dir: {}", local_dir_name, e))?;
    let path = instance_dir.join("config.json");

    let inserted = std::cell::Cell::new(false);
    crate::config::local_config_io::update_config_json_object(&path, true, |obj| {
        let tooling = ensure_object(obj, "tooling", &path);
        let stored_is_not_older = tooling
            .get("lastAgentMessageAt")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .zip(chrono::DateTime::parse_from_rfc3339(at_rfc3339).ok())
            .is_some_and(|(stored, new)| stored >= new);
        if stored_is_not_older {
            return Ok(());
        }
        tooling.insert(
            "lastAgentMessageAt".to_string(),
            serde_json::json!(at_rfc3339),
        );
        inserted.set(true);
        Ok(())
    })?;

    log::debug!(
        "lastAgentMessageAt for {}: {} (inserted: {})",
        repo_path,
        at_rfc3339,
        inserted.get()
    );
    Ok(inserted.get())
}

/// #1682 - the stored stamp for `repo_path`, or `None` when the file is absent,
/// unparseable, or carries no `tooling.lastAgentMessageAt`. Never validates the
/// string: rendering owns that.
pub fn read_last_agent_message_at(repo_path: &str) -> Option<String> {
    let path = Path::new(repo_path)
        .join(crate::config::agent_local_dir_name().as_str())
        .join("config.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<AgentLocalConfig>(&c).ok())
        .and_then(|cfg| cfg.tooling.last_agent_message_at)
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
    crate::config::local_config_io::update_config_json_object(config_path, true, |obj| {
        let tooling = ensure_object(obj, "tooling", config_path);
        tooling.insert("lastCodingAgent".to_string(), serde_json::json!(agent_id));

        let coding_agents = ensure_object(tooling, "codingAgents", config_path);
        let entry_val =
            serde_json::to_value(entry).map_err(|e| format!("Failed to serialize entry: {}", e))?;
        coding_agents.insert(agent_id.to_string(), entry_val);
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const T1: &str = "2026-09-02T01:00:00+00:00";
    const T2: &str = "2026-09-02T02:00:00+00:00";
    const T3: &str = "2026-09-02T03:00:00+00:00";

    /// Path of the per-instance config the writer under test targets.
    fn instance_config(dir: &Path) -> PathBuf {
        dir.join(crate::config::agent_local_dir_name())
            .join("config.json")
    }

    /// Seed `dir`'s per-instance config with `value`, creating the instance dir.
    fn seed_instance_config(dir: &Path, value: &serde_json::Value) {
        let path = instance_config(dir);
        std::fs::create_dir_all(path.parent().expect("instance dir")).expect("create instance dir");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(value).expect("serialize"),
        )
        .expect("seed config");
    }

    /// Raw JSON currently stored in `dir`'s per-instance config.
    fn stored(dir: &Path) -> serde_json::Value {
        let raw = std::fs::read_to_string(instance_config(dir)).expect("read stored config");
        serde_json::from_str(&raw).expect("stored config is JSON")
    }

    fn set(dir: &Path, at: &str) -> Result<bool, String> {
        set_last_agent_message_at(dir.to_str().expect("utf-8 temp path"), at)
    }

    fn read(dir: &Path) -> Option<String> {
        read_last_agent_message_at(dir.to_str().expect("utf-8 temp path"))
    }

    #[test]
    fn set_last_agent_message_at_writes_only_the_instance_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        assert_eq!(set(dir, T2), Ok(true));

        assert_eq!(
            stored(dir)["tooling"]["lastAgentMessageAt"],
            serde_json::json!(T2)
        );
        // D2: the root copy is deliberately not a write target for this stamp.
        assert!(
            !dir.join("config.json").exists(),
            "the root config must not be written"
        );
    }

    #[test]
    fn set_last_agent_message_at_preserves_existing_tooling_and_unknown_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_instance_config(
            dir,
            &serde_json::json!({
                "tooling": {
                    "lastCodingAgent": "claude",
                    "codingAgents": {
                        "claude": { "app": "Claude Code", "lastUsed": T1 }
                    }
                },
                "repos": ["repo-AgentsCommander"]
            }),
        );

        assert_eq!(set(dir, T2), Ok(true));

        let after = stored(dir);
        assert_eq!(
            after["tooling"]["lastCodingAgent"],
            serde_json::json!("claude")
        );
        assert_eq!(
            after["tooling"]["codingAgents"]["claude"],
            serde_json::json!({ "app": "Claude Code", "lastUsed": T1 })
        );
        assert_eq!(after["repos"], serde_json::json!(["repo-AgentsCommander"]));
        assert_eq!(
            after["tooling"]["lastAgentMessageAt"],
            serde_json::json!(T2)
        );
    }

    #[test]
    fn set_last_agent_message_at_is_monotonic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        assert_eq!(set(dir, T2), Ok(true));

        // Older instant: skipped, stored value untouched.
        assert_eq!(set(dir, T1), Ok(false));
        assert_eq!(read(dir), Some(T2.to_string()));

        // Newer instant: written.
        assert_eq!(set(dir, T3), Ok(true));
        assert_eq!(read(dir), Some(T3.to_string()));

        // An unparseable stored value is not a reason to skip.
        seed_instance_config(
            dir,
            &serde_json::json!({ "tooling": { "lastAgentMessageAt": "not-a-timestamp" } }),
        );
        assert_eq!(set(dir, T1), Ok(true));
        assert_eq!(read(dir), Some(T1.to_string()));
    }

    #[test]
    fn a_session_restart_rewrite_preserves_the_stamp() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let repo = dir.to_str().expect("utf-8 temp path");

        assert_eq!(set(dir, T2), Ok(true));
        set_last_coding_agent(repo, "claude", "Claude Code", Some("sid")).expect("restart rewrite");

        assert_eq!(read(dir), Some(T2.to_string()));
        assert_eq!(
            stored(dir)["tooling"]["lastCodingAgent"],
            serde_json::json!("claude")
        );
    }

    #[test]
    fn read_last_agent_message_at_binds_present_and_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        // Absent file.
        assert_eq!(read(dir), None);

        // Invalid JSON.
        let path = instance_config(dir);
        std::fs::create_dir_all(path.parent().expect("instance dir")).expect("create instance dir");
        std::fs::write(&path, "{ not json").expect("write invalid json");
        assert_eq!(read(dir), None);

        // Tooling absent.
        seed_instance_config(dir, &serde_json::json!({}));
        assert_eq!(read(dir), None);

        // Tooling present without the key.
        seed_instance_config(
            dir,
            &serde_json::json!({ "tooling": { "lastCodingAgent": "claude" } }),
        );
        assert_eq!(read(dir), None);

        // Tooling set to a non-object.
        seed_instance_config(dir, &serde_json::json!({ "tooling": 5 }));
        assert_eq!(read(dir), None);

        // Control: a well-formed value is read back.
        assert_eq!(set(dir, T2), Ok(true));
        assert_eq!(read(dir), Some(T2.to_string()));
    }

    #[test]
    fn is_empty_tracks_the_stamp() {
        assert!(AgentTooling::default().is_empty());
        assert!(!AgentTooling {
            last_agent_message_at: Some(T2.to_string()),
            ..Default::default()
        }
        .is_empty());
    }
}
