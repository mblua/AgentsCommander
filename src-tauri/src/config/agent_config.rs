pub(super) use crate::config::local_config_io::ConfigPublisher;
#[cfg(test)]
pub(super) use crate::config::local_config_io::{publish_temp_config, temp_config_path};
use crate::filesystem::{config_locations, documents};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

mod discovery;
pub(crate) use discovery::{
    discovery_replica_config_exists, read_discovery_replica_context, read_discovery_replica_json,
};

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
    config_locations::ensure_shared_directories(&instance_dir)
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
    chrono::DateTime::parse_from_rfc3339(at_rfc3339)
        .map_err(|_| "invalidActivityTimestamp: expected RFC3339".to_string())?;
    let local_dir_name = crate::config::agent_local_dir_name();
    let instance_dir = Path::new(repo_path).join(local_dir_name.as_str());
    config_locations::ensure_shared_directories(&instance_dir)
        .map_err(|e| format!("Failed to create {} dir: {}", local_dir_name, e))?;
    let path = instance_dir.join("config.json");

    let inserted = activity_only_update(&path, at_rfc3339)?;

    log::debug!(
        "lastAgentMessageAt for {}: {} (inserted: {})",
        repo_path,
        at_rfc3339,
        inserted
    );
    Ok(inserted)
}

/// Activity has a distinct pending path: PAIR locks only, no cleanup, split
/// marker or journal. Outside a reservation the ordinary legacy policy stays.
fn activity_only_update(path: &Path, at_rfc3339: &str) -> Result<bool, String> {
    let inserted = std::cell::Cell::new(false);
    update_agent_config_with_activity(
        path,
        production_journal_dir().as_deref(),
        &|_| {},
        |_decisions, state| {
            let tooling = ensure_object(state, "tooling", path);
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
        },
        Some((at_rfc3339, &inserted)),
    )?;
    Ok(inserted.get())
}

/// #1682 - the stored stamp for `repo_path`, or `None` when the file is absent,
/// unparseable, or carries no `tooling.lastAgentMessageAt`. Reads the greatest
/// landed valid state/legacy timestamp; invalid legacy-only data stays visible.
pub fn read_last_agent_message_at(repo_path: &str) -> Option<String> {
    let dir = Path::new(repo_path).join(crate::config::agent_local_dir_name().as_str());
    // Read only landed state and tracked legacy stamps. A pending activity
    // intent must never be advertised before its state publish.
    let mut maximum: Option<(chrono::DateTime<chrono::FixedOffset>, String)> = None;
    for path in [dir.join("config.json"), state_file_path(&dir)] {
        let value = documents::read_agent_activity_bytes(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| {
                value
                    .get("tooling")?
                    .get("lastAgentMessageAt")?
                    .as_str()
                    .map(str::to_string)
            });
        if let Some(value) = value {
            if let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(&value) {
                if maximum.as_ref().is_none_or(|(old, _)| timestamp > *old) {
                    maximum = Some((timestamp, value));
                }
            }
        }
    }
    maximum
        .map(|(_, value)| value)
        .or_else(|| read_agent_local_config(&dir).and_then(|cfg| cfg.tooling.last_agent_message_at))
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

/// #2786 C1 r28 candidate instrumentation, option C. Test-only. The recorder
/// is ARMED by a process-start environment flag, read once; armed off it is
/// inert (no lock, no record, no panic), so the ordinary suite is untouched.
/// Observation rows run in a dedicated process that sets the flag through
/// `Command::env`, never by mutating this process's environment.
#[cfg(test)]
pub(crate) mod load_probe;

/// #2786 C1 r28 candidate instrumentation, option C: the dedicated-process
/// harness the E15 observation rows run through. Test-only.
///
/// r28 change vs r27: the child is owned by a `ChildGuard` from spawn to reap.
/// `try_wait` errors are handled instead of unwinding out of the row, and a
/// termination or reap that FAILS is reported as the error it returned; the
/// harness never claims a cleanup it did not achieve.
#[cfg(test)]
pub(crate) mod load_probe_harness;

/// #2786 (C1) - the one reader of an agent's local config: `config.json` in
/// `dir`, with the state keys served key-wise from the state file beside it.
/// `None` when neither file yields a config. Each file must also pass the
/// typed parse on its own text, as the direct typed readers did before C1, so
/// a duplicate known field is still a rejection and never collapses to the
/// last value through `Value`. A state file that fails it is ignored.
pub fn read_agent_local_config(dir: &Path) -> Option<AgentLocalConfig> {
    #[cfg(test)]
    load_probe::record_load(dir);
    let decisions = read_config_text(&dir.join("config.json"));
    if decisions.as_ref().is_some_and(|text| !typed_ok(text)) {
        return None;
    }
    let state = read_config_text(&state_file_path(dir)).filter(|text| typed_ok(text));
    let value = merge_config_texts(decisions, state).ok()??;
    serde_json::from_value(value).ok()
}

/// #2786 (C1) - the raw JSON the loader serves: `config.json` in `dir` with
/// the state file's keys overlaid. `Ok(None)` when neither file exists; `Err`
/// when `config.json` exists but is not JSON. A pure read: it writes nothing.
/// The state file must pass the typed parse, exactly as in the typed wrapper,
/// so both wrappers admit the same state files; `config.json` keeps its base
/// `Value` semantics and is not filtered.
pub fn read_agent_local_config_json(dir: &Path) -> Result<Option<serde_json::Value>, String> {
    #[cfg(test)]
    load_probe::record_load(dir);
    merge_config_texts(
        read_config_text(&dir.join("config.json")),
        read_config_text(&state_file_path(dir)).filter(|text| typed_ok(text)),
    )
}

/// #2786 (C1) - overlay an already parsed state file on an already parsed
/// `config.json`, for a caller that reads both through its own guarded path.
/// A state value that is not an object is ignored.
pub fn overlay_agent_local_state(
    decisions: serde_json::Value,
    state: Option<serde_json::Value>,
) -> serde_json::Value {
    let state = match state {
        Some(serde_json::Value::Object(map)) => Some(map),
        _ => None,
    };
    overlay_state(decisions, state)
}

fn state_file_path(dir: &Path) -> std::path::PathBuf {
    dir.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

fn read_config_text(path: &Path) -> Option<String> {
    documents::read_config_document(path).ok()
}

/// Merge the two texts. `config.json` that is not JSON is an error; a state
/// file that is absent, not JSON or not an object is ignored, so a reader
/// falls back to `config.json` rather than failing.
/// #2786 (C1) - the typed parse both wrappers apply to a file's own text.
fn typed_ok(text: &str) -> bool {
    serde_json::from_str::<AgentLocalConfig>(text).is_ok()
}

fn merge_config_texts(
    decisions: Option<String>,
    state: Option<String>,
) -> Result<Option<serde_json::Value>, String> {
    let decisions = decisions
        .map(|text| serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string()))
        .transpose()?;
    let state = state
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        });
    if decisions.is_none() && state.is_none() {
        return Ok(None);
    }
    let decisions = decisions.unwrap_or_else(|| serde_json::json!({}));
    Ok(Some(overlay_state(decisions, state)))
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

// ── Config pair entry point (#2807, C2) ─────────────────────────────────────
type JsonMap = serde_json::Map<String, serde_json::Value>;

/// #2807 (C2) - the one writer of an agent's config pair: `decisions` is the
/// tracked `config.json`, and the state file is the one beside it. `mutate`
/// gets `(decisions, state)`; a decision-only writer ignores the second map.
/// Any state key still in the tracked file is moved to the state file first,
/// inside the same guard.
pub fn update_agent_config<F>(decisions: &Path, mutate: F) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_agent_config_in(decisions, production_journal_dir().as_deref(), mutate)
}

/// The exact refusal of the pre-C3 `update_config_json_object(path, false, ..)`
/// callers: an absent decisions file is an error and nothing is created. As in
/// the old helper the decision is taken UNDER the file locks, so a concurrent
/// creator mid-publish is waited for, not refused.
pub fn update_existing_agent_config<F>(decisions: &Path, mutate: F) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_existing_agent_config_with_publish(decisions, mutate, None)
}

pub(super) fn update_existing_agent_config_with_publish<F>(
    decisions: &Path,
    mutate: F,
    publish: Option<crate::config::local_config_io::ConfigPublisher<'_>>,
) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_agent_config_with_activity_and_publish(
        decisions,
        production_journal_dir().as_deref(),
        &|_: &str| {},
        |d, s| {
            if !documents::config_document_exists(decisions) {
                return Err(format!(
                    "Local config {} does not exist",
                    decisions.display()
                ));
            }
            mutate(d, s)
        },
        None,
        publish,
    )
}

/// The journal directory of a production call. Under `cfg(test)` the live
/// instance journal is unreachable: the default is `None`, and a test that
/// wants a note opts in to its own temporary directory. One body, one call
/// site, no second production code path.
fn production_journal_dir() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        journal_redirect::current()
    }
    #[cfg(not(test))]
    {
        crate::config::config_dir()
    }
}

/// #2816 (C3) - the only route from a lib test to a journal: a thread-local
/// directory, so parallel tests never see each other's.
#[cfg(test)]
pub(crate) mod journal_redirect;

fn update_agent_config_in<F>(
    decisions: &Path,
    journal_dir: Option<&Path>,
    mutate: F,
) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_agent_config_with_stage(decisions, journal_dir, &|_: &str| {}, mutate)
}

/// The only body. `on_stage` is a no-op in production and a pause point in
/// tests, so no second code path exists.
fn update_agent_config_with_stage<F>(
    decisions: &Path,
    journal_dir: Option<&Path>,
    on_stage: &dyn Fn(&str),
    mutate: F,
) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_agent_config_with_activity(decisions, journal_dir, on_stage, mutate, None)
}

fn update_agent_config_with_activity<F>(
    decisions: &Path,
    journal_dir: Option<&Path>,
    on_stage: &dyn Fn(&str),
    mutate: F,
    activity: Option<(&str, &std::cell::Cell<bool>)>,
) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_agent_config_with_activity_and_publish(
        decisions,
        journal_dir,
        on_stage,
        mutate,
        activity,
        None,
    )
}

fn update_agent_config_with_activity_and_publish<F>(
    decisions: &Path,
    journal_dir: Option<&Path>,
    on_stage: &dyn Fn(&str),
    mutate: F,
    activity: Option<(&str, &std::cell::Cell<bool>)>,
    publish: Option<crate::config::local_config_io::ConfigPublisher<'_>>,
) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    let parent = decisions.parent().ok_or_else(|| {
        format!(
            "Local config {} has no parent directory",
            decisions.display()
        )
    })?;
    let state = state_file_path(parent);
    let moved = std::cell::RefCell::new(Vec::new());
    let overridden = std::cell::RefCell::new(Vec::new());
    let nonpending_preflight = || {
        if let Some(ac_root) = nearest_ac_root(decisions) {
            ensure_config_state_ignore_rows(ac_root)?;
        }
        Ok(())
    };
    let cleanup = |d: &mut JsonMap, s: &mut JsonMap| absorb_state_keys(d, s, &moved, &overridden);
    // The marker is stamped here and nowhere else: the cleanup has published
    // the clean tracked file before `mutate` runs.
    let finish = |s: &mut JsonMap| {
        stamp_split_marker(s);
        Ok(())
    };
    let reservation =
        parent.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME);
    let args = (
        decisions,
        state.as_path(),
        reservation.as_path(),
        &STATE_KEYS,
        &nonpending_preflight as &dyn Fn() -> Result<(), String>,
        &cleanup as crate::config::local_config_io::ConfigPairCleanup<'_>,
        on_stage,
        &finish as crate::config::local_config_io::ConfigPairFinish<'_>,
        activity,
    );
    match publish {
        None => crate::config::local_config_io::update_config_pair_guarded(
            args.0,
            args.1,
            args.2,
            args.3,
            Some(args.4),
            Some(args.5),
            args.6,
            mutate,
            args.7,
            args.8,
        ),
        Some(publish) => crate::config::local_config_io::update_config_pair_inner(
            args.0,
            args.1,
            Some(args.2),
            args.3,
            Some(args.5),
            args.6,
            mutate,
            args.7,
            args.8,
            Some(args.4),
            publish,
        ),
    }?;

    for (key, state_value, tracked_value) in overridden.into_inner() {
        log::warn!(
            "[config-state] '{}' in {} was {} and is now {}, the value found in {}",
            key,
            state.display(),
            state_value,
            tracked_value,
            decisions.display()
        );
    }
    let moved = moved.into_inner();
    if !moved.is_empty() {
        note_moved_state_keys(decisions, journal_dir, &moved);
    }
    Ok(())
}

/// Pure source-reference PAIR preparation using the same cleanup and split
/// policy as the ordinary writer. No ignores, journal, locks or files are
/// changed here; the coordinator owns authorization and durable plan storage.
#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
pub(crate) fn prepare_identity_reference_pair<F>(
    before: [crate::config::local_config_io::PhysicalState; 2],
    mutate: F,
) -> Result<
    crate::config::local_config_io::PreparedConfigPairPlan,
    crate::config::local_config_io::PreparedPairError,
>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    let moved = std::cell::RefCell::new(Vec::new());
    let overridden = std::cell::RefCell::new(Vec::new());
    let cleanup = |d: &mut JsonMap, s: &mut JsonMap| absorb_state_keys(d, s, &moved, &overridden);
    crate::config::local_config_io::prepare_config_pair_plan(before, Some(&cleanup), |d, s| {
        mutate(d, s)?;
        stamp_split_marker(s);
        Ok(())
    })
}

/// Internal only until P11. Executes original images, never re-runs policy or
/// a mutation callback during recovery. The caller supplies authorized targets.
#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
pub(crate) fn execute_identity_reference_pair(
    decisions: &Path,
    plan: &crate::config::local_config_io::PreparedConfigPairPlan,
) -> Result<(), crate::config::local_config_io::PreparedPairError> {
    let parent = decisions.parent().ok_or(
        crate::config::local_config_io::PreparedPairError::InvalidPlan("PAIR target has no parent"),
    )?;
    crate::config::local_config_io::execute_unreserved_config_pair(
        decisions,
        &state_file_path(parent),
        &parent.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME),
        plan,
    )
}

/// Inventory/C/source authorization precedes this boundary. Pure cleanup and
/// caller preparation run inside the same PAIR segment as packet publication.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare_and_reserve_identity_reference_pair<F>(
    decisions: &Path,
    request: &crate::config::local_config_io::PairReservationRequest,
    mutate: F,
) -> Result<
    crate::config::local_config_io::PairReservation,
    crate::config::local_config_io::PreparedPairError,
>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    prepare_and_reserve_identity_reference_pair_guarded(decisions, request, mutate, &|| Ok(()))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare_and_reserve_identity_reference_pair_guarded<F>(
    decisions: &Path,
    request: &crate::config::local_config_io::PairReservationRequest,
    mutate: F,
    validate: &dyn Fn() -> Result<(), crate::config::local_config_io::PreparedPairError>,
) -> Result<
    crate::config::local_config_io::PairReservation,
    crate::config::local_config_io::PreparedPairError,
>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    validate()?;
    use crate::config::local_config_io::PreparedPairError;
    let parent = decisions
        .parent()
        .ok_or(PreparedPairError::InvalidPlan("PAIR target has no parent"))?;
    if let Some(ac_root) = nearest_ac_root(decisions) {
        ensure_config_state_ignore_rows(ac_root).map_err(PreparedPairError::Preparation)?;
    }
    let moved = std::cell::RefCell::new(Vec::new());
    let overridden = std::cell::RefCell::new(Vec::new());
    let cleanup = |d: &mut JsonMap, s: &mut JsonMap| absorb_state_keys(d, s, &moved, &overridden);
    crate::config::local_config_io::prepare_and_reserve_config_pair_guarded(
        decisions,
        &state_file_path(parent),
        &parent.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME),
        request,
        Some(&cleanup),
        |d, s| {
            mutate(d, s)?;
            stamp_split_marker(s);
            Ok(())
        },
        validate,
    )
}

fn nearest_ac_root(decisions: &Path) -> Option<&Path> {
    decisions
        .ancestors()
        .skip(1)
        .find(|dir| dir.file_name().is_some_and(|name| name == ".ac"))
}

/// #2807 (C2) 4.1 - appends to `<ac_root>/.gitignore` the agent state rows it
/// lacks. An appending write, never a rewrite, so no line another writer added
/// can be lost.
fn ensure_config_state_ignore_rows(ac_root: &Path) -> Result<(), String> {
    let path = ac_root.join(".gitignore");
    let io = |what: &str, e: std::io::Error| format!("failed to {what} {}: {e}", path.display());
    let content = match documents::read_config_document(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io("read", e)),
    };
    let mut blocks = crate::config::naming_migration::missing_ignore_rows(
        &content,
        &crate::config::naming_migration::config_state_ignore_rows(),
    );
    use crate::config::instance_artifacts::{
        CONFIG_IDENTITY_RESERVATION_LOCK, CONFIG_IDENTITY_RESERVATION_NAME,
        CONFIG_IDENTITY_RESERVATION_TMP,
    };
    for name in [
        CONFIG_IDENTITY_RESERVATION_NAME,
        CONFIG_IDENTITY_RESERVATION_LOCK,
        CONFIG_IDENTITY_RESERVATION_TMP,
    ] {
        let row = format!("**/{name}");
        if !content.lines().any(|line| line == row) {
            blocks.push_str(&format!(
                "# AgentsCommander: private PAIR reservation state\n{row}\n"
            ));
        }
    }
    if blocks.is_empty() {
        return Ok(());
    }
    let separator = if content.is_empty() || content.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    documents::append_config_ignore_rows(&path, separator, &blocks).map_err(|e| io("append to", e))
}

/// #2807 (C2) 3.3 - the cleanup of the pair, and the write-side twin of
/// `overlay_state`: every state key leaves the tracked map. Marker absent, the
/// state value wins where both hold the key; marker present, the tracked value
/// wins. It takes no lock, opens no file and names no path; `moved` and
/// `overridden` carry what it did back to the entry point.
fn absorb_state_keys(
    decisions: &mut JsonMap,
    state: &mut JsonMap,
    moved: &std::cell::RefCell<Vec<&'static str>>,
    overridden: &std::cell::RefCell<Vec<(&'static str, serde_json::Value, serde_json::Value)>>,
) -> Result<(), String> {
    // A malformed `tooling` is never silently replaced (#1939).
    if state.get("tooling").is_some_and(|t| !t.is_object()) {
        return Err("'tooling' must be a JSON object in the state file".to_string());
    }
    let Some(tracked) = decisions.get_mut("tooling") else {
        return Ok(());
    };
    let tracked = tracked
        .as_object_mut()
        .ok_or_else(|| "'tooling' must be a JSON object in the tracked config file".to_string())?;
    let tracked_wins = split_marker_present(state.get(SPLIT_MARKER_KEY));
    for key in STATE_KEYS {
        let Some(tracked_value) = tracked.remove(key) else {
            continue;
        };
        let state_tooling = state
            .entry("tooling")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .expect("checked above");
        match state_tooling.get(key) {
            // Never split: the tracked copy is our own interrupted copy.
            Some(_) if !tracked_wins => {}
            Some(state_value) => {
                overridden
                    .borrow_mut()
                    .push((key, state_value.clone(), tracked_value.clone()));
                state_tooling.insert(key.to_string(), tracked_value);
            }
            None => {
                state_tooling.insert(key.to_string(), tracked_value);
            }
        }
        moved.borrow_mut().push(key);
    }
    Ok(())
}

/// #2807 (C2) 3.4 - a no-op unless the state map holds a state key; otherwise
/// the D7 split marker, exactly `{ "v": 1, "keys": [the four names] }`.
fn stamp_split_marker(state: &mut JsonMap) {
    let holds_state_key = state
        .get("tooling")
        .and_then(|tooling| tooling.as_object())
        .is_some_and(|tooling| STATE_KEYS.iter().any(|key| tooling.contains_key(*key)));
    if holds_state_key {
        state.insert(
            SPLIT_MARKER_KEY.to_string(),
            serde_json::json!({ "v": SPLIT_MARKER_VERSION, "keys": STATE_KEYS }),
        );
    }
}

/// #2807 (C2) 3.5 - an audit note in the instance journal, nothing more: no
/// step record, and no decision ever reads it. A failure is logged and the
/// write still succeeded, because both files are already consistent.
fn note_moved_state_keys(decisions: &Path, journal_dir: Option<&Path>, moved: &[&str]) {
    use crate::config::naming_migration;
    let canonical =
        documents::canonical_config_document(decisions).unwrap_or_else(|_| decisions.to_path_buf());
    let scope = format!("config-state:{}", canonical.display());
    let note = format!("moved to the state file: {}", moved.join(", "));
    let noted = naming_migration::update_journal(journal_dir, |journal| {
        journal.set_status(&scope, naming_migration::ScopeStatus::Complete);
        journal.note(&scope, &note);
    });
    if let Err(refusal) = noted {
        log::warn!(
            "[config-state] could not record the move for {}: {:?}",
            decisions.display(),
            refusal
        );
    }
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
    update_agent_config(config_path, |_decisions, state| {
        // #1939 - the discriminator is the JSON shape, not the path: for both
        // `root/config.json` and `root/<agent_local_dir>/config.json`, an absent
        // top-level tooling is created, an object is preserved and updated, and
        // any present non-object (including null) is an error before
        // mutation/publication. The historical repair-by-reset is deliberately
        // gone: a malformed tooling is never silently replaced. Nested
        // `codingAgents` repair stays.
        let tooling_value = state
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
mod tests;
