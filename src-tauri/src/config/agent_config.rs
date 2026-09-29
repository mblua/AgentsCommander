use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
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
    /// #2433 - `canonical_command_text` of the agent's command, computed by the
    /// caller. Empty when an older build wrote the entry.
    #[serde(default)]
    pub command: String,
    /// #2433 - enabled profile letter -> `cell_identity` digest, computed by the
    /// caller. An agent with no enabled cells writes `{}`; an absent key means an
    /// older build wrote the entry.
    #[serde(default)]
    pub identity: BTreeMap<String, String>,
}

/// Keys of a `codingAgents` entry this build owns. `upsert_config` removes
/// them before merging so an optional one this write omits does not linger;
/// any other key (written by a newer build) survives.
const CODING_AGENT_ENTRY_KEYS: [&str; 3] = ["app", "acSessionId", "lastUsed"];

/// #2433 - the descriptor keys. Written (and replaced) only when the caller
/// supplies a descriptor; otherwise an existing descriptor is left untouched.
const CODING_AGENT_DESCRIPTOR_KEYS: [&str; 2] = ["command", "identity"];

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
    descriptor: Option<(&str, &BTreeMap<String, String>)>,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    let entry = CodingAgentEntry {
        app: app_label.to_string(),
        ac_session_id: ac_session_id.map(|s| s.to_string()),
        last_used: now,
        command: descriptor
            .map(|(command, _)| command.to_string())
            .unwrap_or_default(),
        identity: descriptor
            .map(|(_, identity)| identity.clone())
            .unwrap_or_default(),
    };
    let write_descriptor = descriptor.is_some();

    // Write to per-instance config dir
    let local_dir_name = crate::config::agent_local_dir_name();
    let instance_dir = Path::new(repo_path).join(local_dir_name.as_str());
    std::fs::create_dir_all(&instance_dir)
        .map_err(|e| format!("Failed to create {} dir: {}", local_dir_name, e))?;
    upsert_config(
        &instance_dir.join("config.json"),
        agent_id,
        &entry,
        write_descriptor,
    )?;

    // Also write to root config.json so discovery can find it regardless of instance
    upsert_config(
        &Path::new(repo_path).join("config.json"),
        agent_id,
        &entry,
        write_descriptor,
    )?;

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
    let dir = Path::new(repo_path).join(crate::config::agent_local_dir_name().as_str());
    read_agent_local_config(&dir).and_then(|cfg| cfg.tooling.last_agent_message_at)
}

// ── State file loader (#2786, C1) ───────────────────────────────────────────
/// #2786 (C1) - the D7 split marker's top-level key in the state file.
pub(crate) const SPLIT_MARKER_KEY: &str = "split";
/// #2786 (C1) - a marker whose integer `v` is at least this reads as present.
pub(crate) const SPLIT_MARKER_VERSION: u64 = 1;

/// #2786 (C1) - the `tooling` keys whose home is the state file.
const STATE_KEYS: [&str; 4] = [
    "lastCodingAgent",
    "codingAgents",
    "lastAgentMessageAt",
    "profileContentHash",
];

/// #2786 (C1) - the one reader of an agent's local config: `config.json` in
/// `dir`, with the state keys served key-wise from the state file beside it.
/// `None` when neither file yields a config.
pub fn read_agent_local_config(dir: &Path) -> Option<AgentLocalConfig> {
    let value = read_agent_local_config_json(dir).ok()??;
    serde_json::from_value(value).ok()
}

/// #2786 (C1) - the raw JSON the loader serves: `config.json` in `dir` with
/// the state file's keys overlaid. `Ok(None)` when neither file exists; `Err`
/// when `config.json` exists but is not JSON. A pure read: it writes nothing.
pub fn read_agent_local_config_json(dir: &Path) -> Result<Option<serde_json::Value>, String> {
    let path = dir.join("config.json");
    let decisions = match std::fs::read_to_string(&path) {
        Ok(content) => {
            Some(serde_json::from_str::<serde_json::Value>(&content).map_err(|e| e.to_string())?)
        }
        Err(_) => None,
    };
    let state = read_state_file(dir);
    if decisions.is_none() && state.is_none() {
        return Ok(None);
    }
    let decisions = decisions.unwrap_or_else(|| serde_json::json!({}));
    Ok(Some(overlay_state(decisions, state)))
}

/// #2786 (C1) - overlay the state file in `dir` on a `config.json` the caller
/// has already read and parsed through its own guarded path.
pub fn overlay_agent_local_state(dir: &Path, decisions: serde_json::Value) -> serde_json::Value {
    overlay_state(decisions, read_state_file(dir))
}

/// The state file as an object, or `None` when it is absent or unusable: a
/// reader falls back to `config.json` rather than failing.
fn read_state_file(dir: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let path = dir.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME);
    let content = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<serde_json::Value>(&content).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

/// D7, "present" is exact: an object whose `v` is an integer at or above
/// [`SPLIT_MARKER_VERSION`]. A newer version reads as present on purpose.
fn split_marker_present(marker: Option<&serde_json::Value>) -> bool {
    marker
        .and_then(|marker| marker.get("v"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v >= SPLIT_MARKER_VERSION)
}

/// Key-wise overlay. Marker absent: the state file wins for every key it
/// holds. Marker present: a key the tracked file holds keeps the tracked value.
/// `split.keys` is not consulted, and the marker is never returned.
fn overlay_state(
    mut decisions: serde_json::Value,
    state: Option<serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Value {
    let Some(state) = state else {
        return decisions;
    };
    let Some(state_tooling) = state.get("tooling").and_then(|t| t.as_object()) else {
        return decisions;
    };
    let tracked_wins = split_marker_present(state.get(SPLIT_MARKER_KEY));
    let Some(root) = decisions.as_object_mut() else {
        return decisions;
    };
    let Some(tooling) = root
        .entry("tooling")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    else {
        return decisions;
    };
    for key in STATE_KEYS {
        let Some(value) = state_tooling.get(key) else {
            continue;
        };
        if tracked_wins && tooling.contains_key(key) {
            continue;
        }
        tooling.insert(key.to_string(), value.clone());
    }
    decisions
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
    write_descriptor: bool,
) -> Result<(), String> {
    crate::config::local_config_io::update_config_json_object(config_path, true, |obj| {
        // #1939 - the discriminator is the JSON shape, not the path: for both
        // `root/config.json` and `root/<agent_local_dir>/config.json`, an absent
        // top-level tooling is created, an object is preserved and updated, and
        // any present non-object (including null) is an error before
        // mutation/publication. The historical repair-by-reset is deliberately
        // gone: a malformed tooling is never silently replaced. Nested
        // `codingAgents` repair stays.
        let tooling_value = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let tooling = tooling_value.as_object_mut().ok_or_else(|| {
            format!(
                "'tooling' must be a JSON object at {}",
                config_path.display()
            )
        })?;
        tooling.insert("lastCodingAgent".to_string(), serde_json::json!(agent_id));

        let coding_agents = ensure_object(tooling, "codingAgents", config_path);
        let serde_json::Value::Object(mut new_fields) =
            serde_json::to_value(entry).map_err(|e| format!("Failed to serialize entry: {}", e))?
        else {
            return Err("CodingAgentEntry did not serialize to an object".to_string());
        };
        // #2433 - merge into the existing entry so an unknown key written by a
        // newer build survives; a non-object entry is replaced as before.
        let slot = coding_agents
            .entry(agent_id.to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !slot.is_object() {
            *slot = serde_json::json!({});
        }
        let existing = slot.as_object_mut().expect("just set to object");
        for key in CODING_AGENT_ENTRY_KEYS {
            existing.remove(key);
        }
        for key in CODING_AGENT_DESCRIPTOR_KEYS {
            if write_descriptor {
                existing.remove(key);
            } else {
                new_fields.remove(key);
            }
        }
        existing.extend(new_fields);
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

    /// Raw JSON currently stored in `dir`'s root config.
    fn stored_root(dir: &Path) -> serde_json::Value {
        let raw = std::fs::read_to_string(dir.join("config.json")).expect("read root config");
        serde_json::from_str(&raw).expect("root config is JSON")
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
        set_last_coding_agent(repo, "claude", "Claude Code", Some("sid"), None)
            .expect("restart rewrite");

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

    // ------------------------------------------------------------------
    // #1939 tooling-shape discriminator for config metadata writes.
    // ------------------------------------------------------------------

    fn codex_entry() -> CodingAgentEntry {
        CodingAgentEntry {
            app: "Codex".to_string(),
            ac_session_id: Some("sid".to_string()),
            last_used: T1.to_string(),
            command: String::new(),
            identity: BTreeMap::new(),
        }
    }

    fn root_config(dir: &Path) -> PathBuf {
        dir.join("config.json")
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_missing_and_object_succeed_on_both_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");

            // Missing file: the write creates the tooling object.
            upsert_config(&path, "codex", &codex_entry(), true)
                .expect("missing config must succeed");
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(saved["tooling"]["lastCodingAgent"], "codex");

            // Present object: preserved and updated.
            upsert_config(&path, "claude", &codex_entry(), true)
                .expect("object tooling must succeed");
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(saved["tooling"]["lastCodingAgent"], "claude");
            assert_eq!(saved["tooling"]["codingAgents"]["claude"]["app"], "Codex");
        }
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_non_object_fails_without_rewrite_on_both_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            for literal in ["null", "5", "\"tooling\"", "[]", "false"] {
                let original = format!(r#"{{"tooling":{literal},"repos":["repo-a"]}}"#);
                std::fs::write(&path, &original).expect("seed");
                let error = upsert_config(&path, "codex", &codex_entry(), true)
                    .expect_err("non-object tooling must fail");
                assert!(
                    error.contains("'tooling' must be a JSON object"),
                    "{literal}: {error}"
                );
                assert_eq!(
                    std::fs::read_to_string(&path).expect("read"),
                    original,
                    "{literal}: bytes must be preserved"
                );
            }
        }
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_nested_coding_agents_repairs_on_both_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            std::fs::write(
                &path,
                r#"{"tooling":{"lastCodingAgent":"claude","codingAgents":7},"repos":["repo-a"]}"#,
            )
            .expect("seed");
            upsert_config(&path, "codex", &codex_entry(), true)
                .expect("nested repair must succeed");
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(saved["tooling"]["codingAgents"]["codex"]["app"], "Codex");
            assert_eq!(saved["tooling"]["lastCodingAgent"], "codex");
            assert_eq!(saved["repos"][0], "repo-a");
        }
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_malformed_selection_locked_is_preserved() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            std::fs::write(
                &path,
                r#"{"tooling":{"selectionLocked":"yes","lastCodingAgent":"claude"},"repos":["repo-a"]}"#,
            )
            .expect("seed");
            upsert_config(&path, "codex", &codex_entry(), true)
                .expect("metadata write must succeed");
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(saved["tooling"]["selectionLocked"], "yes");
            assert_eq!(saved["tooling"]["lastCodingAgent"], "codex");
            assert_eq!(saved["repos"][0], "repo-a");
        }
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_plain_repo_metadata_stays_compatible() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            std::fs::write(
                &path,
                r#"{"identity":"../../_agent_dev-rust","repos":["repo-a"],"tooling":{"lastCodingAgent":"claude"}}"#,
            )
            .expect("seed");
            upsert_config(&path, "codex", &codex_entry(), true)
                .expect("valid plain-repo metadata must stay compatible");
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(saved["identity"], "../../_agent_dev-rust");
            assert_eq!(saved["repos"][0], "repo-a");
            assert_eq!(saved["tooling"]["lastCodingAgent"], "codex");
        }
    }

    // ── #2433 descriptor persistence ──

    fn fixed_entry(command: &str, identity: &[(&str, &str)]) -> CodingAgentEntry {
        CodingAgentEntry {
            app: "Claude Code".to_string(),
            ac_session_id: Some("sid".to_string()),
            last_used: T1.to_string(),
            command: command.to_string(),
            identity: identity
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn coding_agent_entry_round_trips_command_and_identity() {
        let entry = fixed_entry("claude --foo", &[("A", "9f2c1ab4de77c001"), ("B", "3e0a")]);
        let value = serde_json::to_value(&entry).expect("serialize");
        assert_eq!(value["command"], serde_json::json!("claude --foo"));
        assert_eq!(
            value["identity"],
            serde_json::json!({"A": "9f2c1ab4de77c001", "B": "3e0a"})
        );
        let back: CodingAgentEntry = serde_json::from_value(value.clone()).expect("deserialize");
        assert_eq!(serde_json::to_value(&back).expect("reserialize"), value);
    }

    #[test]
    fn entry_without_identity_deserializes() {
        let old = serde_json::json!({"app": "Codex", "acSessionId": "s", "lastUsed": T1});
        let entry: CodingAgentEntry = serde_json::from_value(old).expect("old shape loads");
        assert!(entry.identity.is_empty());
        assert!(entry.command.is_empty());
    }

    #[test]
    fn upsert_merges_into_an_existing_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_instance_config(
            dir,
            &serde_json::json!({"tooling": {"codingAgents": {"claude": {
                "app": "Old", "acSessionId": "old-sid", "futureKey": {"x": 1}
            }}}}),
        );
        let mut entry = fixed_entry("claude", &[("A", "aa")]);
        entry.ac_session_id = None;
        upsert_config(&instance_config(dir), "claude", &entry, true).expect("upsert");

        let written = &stored(dir)["tooling"]["codingAgents"]["claude"];
        assert_eq!(written["futureKey"], serde_json::json!({"x": 1}));
        assert_eq!(written["command"], serde_json::json!("claude"));
        assert_eq!(written["identity"], serde_json::json!({"A": "aa"}));
        assert_eq!(written["app"], serde_json::json!("Claude Code"));
        // A known optional key this write omits does not linger.
        assert!(written.get("acSessionId").is_none(), "{written}");
    }

    #[test]
    fn agent_with_no_enabled_cells_writes_an_empty_identity_object() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let repo = dir.to_str().expect("utf-8 temp path");
        set_last_coding_agent(
            repo,
            "claude",
            "Claude Code",
            None,
            Some(("claude", &BTreeMap::new())),
        )
        .expect("write");
        for value in [stored(dir), stored_root(dir)] {
            let entry = &value["tooling"]["codingAgents"]["claude"];
            assert_eq!(
                entry.get("identity"),
                Some(&serde_json::json!({})),
                "{entry}"
            );
            assert_eq!(entry["command"], serde_json::json!("claude"));
        }
    }

    #[test]
    fn writing_the_same_descriptor_twice_is_byte_identical() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let path = instance_config(dir);
        std::fs::create_dir_all(path.parent().expect("instance dir")).expect("mkdir");
        let entry = fixed_entry("claude --x", &[("A", "aa"), ("C", "cc")]);
        upsert_config(&path, "claude", &entry, true).expect("first");
        let first = std::fs::read(&path).expect("read first");
        upsert_config(&path, "claude", &entry, true).expect("second");
        assert_eq!(std::fs::read(&path).expect("read second"), first);
    }

    #[test]
    fn write_without_descriptor_leaves_the_existing_descriptor_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let repo = dir.to_str().expect("utf-8 temp path");
        let identity = BTreeMap::from([("A".to_string(), "aa".to_string())]);
        set_last_coding_agent(repo, "gone", "Gone", None, Some(("gone --x", &identity)))
            .expect("first write");
        set_last_coding_agent(repo, "gone", "Gone", Some("sid2"), None).expect("second write");
        for value in [stored(dir), stored_root(dir)] {
            let entry = &value["tooling"]["codingAgents"]["gone"];
            assert_eq!(entry["command"], serde_json::json!("gone --x"), "{entry}");
            assert_eq!(entry["identity"], serde_json::json!({"A": "aa"}), "{entry}");
            assert_eq!(entry["acSessionId"], serde_json::json!("sid2"));
        }
    }

    #[test]
    fn write_without_descriptor_on_a_fresh_entry_writes_no_descriptor_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let repo = dir.to_str().expect("utf-8 temp path");
        set_last_coding_agent(repo, "gone", "Gone", None, None).expect("write");
        let value = stored(dir);
        let entry = &value["tooling"]["codingAgents"]["gone"];
        assert!(entry.get("command").is_none(), "{entry}");
        assert!(entry.get("identity").is_none(), "{entry}");
    }

    /// #2786 (C1) E7 - seed `config.json` and the state file in a fresh dir.
    fn loader_fixture(
        decisions: Option<serde_json::Value>,
        state: Option<serde_json::Value>,
    ) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        if let Some(value) = decisions {
            std::fs::write(temp.path().join("config.json"), value.to_string())
                .expect("seed decisions");
        }
        if let Some(value) = state {
            std::fs::write(
                temp.path()
                    .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
                value.to_string(),
            )
            .expect("seed state");
        }
        temp
    }

    /// Every file in `dir` with its bytes, to prove the loader wrote nothing.
    fn dir_bytes(dir: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                let entry = entry.expect("entry");
                (
                    entry.file_name(),
                    std::fs::read(entry.path()).expect("read"),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// The four state keys, read through the loader, as one JSON object.
    fn loaded_state_keys(dir: &Path) -> serde_json::Value {
        let before = dir_bytes(dir);
        let config = read_agent_local_config(dir).expect("the loader returns a config");
        let raw = read_agent_local_config_json(dir)
            .expect("parseable")
            .expect("present");
        assert_eq!(dir_bytes(dir), before, "the loader must write nothing");
        serde_json::json!({
            "lastCodingAgent": config.tooling.last_coding_agent,
            "codingAgents": config.tooling.coding_agents.keys().collect::<Vec<_>>(),
            "lastAgentMessageAt": config.tooling.last_agent_message_at,
            "profileContentHash": raw["tooling"]["profileContentHash"],
        })
    }

    fn all_four(tag: &str) -> serde_json::Value {
        serde_json::json!({
            "lastCodingAgent": format!("{tag}-agent"),
            "codingAgents": {format!("{tag}-agent"): {"app": tag}},
            "lastAgentMessageAt": format!("{tag}-at"),
            "profileContentHash": format!("{tag}-hash"),
        })
    }

    fn expect_all_four(tag: &str) -> serde_json::Value {
        serde_json::json!({
            "lastCodingAgent": format!("{tag}-agent"),
            "codingAgents": [format!("{tag}-agent")],
            "lastAgentMessageAt": format!("{tag}-at"),
            "profileContentHash": format!("{tag}-hash"),
        })
    }

    #[test]
    fn the_loader_is_key_wise_and_marker_aware() {
        let marker = serde_json::json!({"v": 1, "keys": STATE_KEYS});

        // 1. State only.
        let temp = loader_fixture(
            None,
            Some(serde_json::json!({"tooling": all_four("state")})),
        );
        assert_eq!(loaded_state_keys(temp.path()), expect_all_four("state"));

        // 2. Decisions only, with a non-state key the loader must keep.
        let mut decisions = all_four("tracked");
        decisions["telegramBot"] = serde_json::json!("bot");
        let temp = loader_fixture(Some(serde_json::json!({"tooling": decisions})), None);
        assert_eq!(loaded_state_keys(temp.path()), expect_all_four("tracked"));
        assert_eq!(
            read_agent_local_config(temp.path())
                .expect("config")
                .tooling
                .telegram_bot
                .as_deref(),
            Some("bot")
        );

        // 3. A state file with two of the four keys hides none of the others.
        let temp = loader_fixture(
            Some(serde_json::json!({"tooling": all_four("tracked")})),
            Some(serde_json::json!({"tooling": {
                "lastCodingAgent": "state-agent",
                "profileContentHash": "state-hash",
            }})),
        );
        assert_eq!(
            loaded_state_keys(temp.path()),
            serde_json::json!({
                "lastCodingAgent": "state-agent",
                "codingAgents": ["tracked-agent"],
                "lastAgentMessageAt": "tracked-at",
                "profileContentHash": "state-hash",
            })
        );

        // 4. Both files, marker present: the tracked value wins.
        let temp = loader_fixture(
            Some(serde_json::json!({"tooling": all_four("tracked")})),
            Some(serde_json::json!({"tooling": all_four("state"), "split": marker})),
        );
        assert_eq!(loaded_state_keys(temp.path()), expect_all_four("tracked"));

        // 5. Both files, no marker: the state file wins.
        let temp = loader_fixture(
            Some(serde_json::json!({"tooling": all_four("tracked")})),
            Some(serde_json::json!({"tooling": all_four("state")})),
        );
        assert_eq!(loaded_state_keys(temp.path()), expect_all_four("state"));

        // 6. Marker shapes: only an integer `v` >= 1 is present.
        let shapes = [
            (serde_json::json!(null), "state"),
            (serde_json::json!(3), "state"),
            (serde_json::json!({"v": "1"}), "state"),
            (serde_json::json!({}), "state"),
            (serde_json::json!({"v": 2, "keys": []}), "tracked"),
        ];
        for (shape, winner) in shapes {
            let temp = loader_fixture(
                Some(serde_json::json!({"tooling": all_four("tracked")})),
                Some(serde_json::json!({"tooling": all_four("state"), "split": shape.clone()})),
            );
            assert_eq!(
                loaded_state_keys(temp.path()),
                expect_all_four(winner),
                "split = {shape}"
            );
        }
    }
}
