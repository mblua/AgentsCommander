//! #786 - shared op layer for scriptable Coding Agent configuration.
//!
//! Single entry point (`apply_coding_agent_op`) used by BOTH the direct CLI
//! write path (GUI closed) and the daemon-routed poller (GUI running), so the
//! two paths behave identically by construction. The op layer is PURE: it
//! mutates an in-memory `AppSettings` and runs the shared validator suite, but
//! it NEVER touches disk. Callers own persistence (save + in-memory swap).
//!
//! The NEW rules this feature adds (agent id format/uniqueness, color hex,
//! non-empty label) live ONLY here, never in `validate_and_repair_settings`, so
//! legacy settings.json files with arbitrary ids/colors keep loading and saving.
//! Command/env/instructions/config-seed validation is delegated to the existing
//! `validate_and_repair_settings` -> `validate_agent_commands` suite that every
//! other writer already runs.
//!
//! Queue transport (daemon path): the CLI drops a `CodingAgentRequest` JSON into
//! `<config_dir>/coding-agent-requests/`; the `MailboxPoller` claims it with an
//! atomic rename to `.processing` (#786 R2, closes the cancel false-negative
//! race), applies it, writes a `CodingAgentResult` into `results/`, and deletes
//! the `.processing` file LAST. The per-request handler is a pure,
//! path-parameterized free function so it unit-tests against a tempdir with no
//! Tauri handle (modeled on `mailbox::collect_project_refresh_requests`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::settings::{
    validate_and_repair_settings, AgentConfig, AppSettings, CodingAgentEnv, ConfigSeedConfig,
};

/// Subdirectory of the config dir holding pending coding-agent mutation requests.
pub const CODING_AGENT_REQUESTS_DIR: &str = "coding-agent-requests";
/// Results subdirectory (under the requests dir) the CLI polls for outcomes.
pub const RESULTS_SUBDIR: &str = "results";
/// A request older than this (poller `now` minus the CLI `createdAtMs`) is
/// rejected as expired, so a stale request cannot apply on a much later GUI boot.
pub const REQUEST_EXPIRY_MS: u64 = 300_000;
/// Default custom-agent color when none is supplied (matches OnboardingModal).
pub const DEFAULT_CUSTOM_COLOR: &str = "#6366f1";

/// A settings.agents mutation. Serialized (camelCase, internally tagged on
/// `kind`) into the request file: `{ "kind": "add", "agent": { ... } }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CodingAgentOp {
    /// Add a new agent. `id` is already minted and syntactically valid CLI-side.
    Add { agent: AgentConfig },
    /// Patch selected fields of the agent with this exact `id`.
    Update { id: String, patch: AgentPatch },
    /// Remove the agent with this exact `id`.
    Remove { id: String },
}

/// Partial update for `coding-agent update`. Every field is optional; only the
/// provided ones change. `envs: Some(vec)` replaces the WHOLE env list
/// (`Some(empty)` clears it, matching `update_coding_agent_env_settings`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envs: Option<Vec<CodingAgentEnv>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolated_home: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    #[serde(default)]
    pub clear_instructions_filename: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_dest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_enabled: Option<bool>,
    #[serde(default)]
    pub clear_config_seed: bool,
}

/// Result of a successful `apply_coding_agent_op`. `op` is `"add"|"update"|"remove"`.
#[derive(Debug, Clone)]
pub struct CodingAgentOpOutcome {
    pub op: &'static str,
    pub agent: Option<AgentConfig>,
    pub removed_id: Option<String>,
}

/// The request file the CLI drops for the poller (daemon path).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentRequest {
    pub request_id: String,
    pub created_at_ms: u64,
    pub op: CodingAgentOp,
}

/// The result file the poller writes and the CLI polls for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentResult {
    pub request_id: String,
    pub ok: bool,
    /// #786: `"add"|"update"|"remove"` on success. Mirrored into the daemon-path
    /// stdout so the JSON shape matches the direct (GUI-closed) path exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_id: Option<String>,
}

/// Disposition of a single processed request, returned by the pure handler so
/// the poller knows whether to swap in-memory state and emit an event.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestDisposition {
    /// The request file was gone before we could claim it (a timed-out CLI
    /// cancelled it). Nothing applied, no result written; caller must NOT swap.
    Skipped,
    /// A result file was written but settings were NOT mutated/persisted
    /// (expired, malformed, validation error, or save failure). Do NOT swap.
    Rejected,
    /// Settings were mutated AND saved; a success result was written. The caller
    /// SHOULD swap in-memory state and emit `coding_agent_settings_updated`.
    Applied {
        op: &'static str,
        agent_id: Option<String>,
    },
}

/// Validate a user-supplied custom agent id: `^[a-z0-9][a-z0-9_-]{0,63}$`.
/// Deliberately distinct from (not a superset of) the catalog key rule
/// (`^[a-z0-9-]+$`): first-char alnum + a 64-char cap, with `_` allowed so
/// FE-minted `agent_<ms>_<n>` ids conform. Lowercase-only because
/// `find_launch_agent` resolves ids case-insensitively.
pub fn validate_custom_agent_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    if bytes.is_empty() {
        return Err("agent id must not be empty".to_string());
    }
    if bytes.len() > 64 {
        return Err(format!("agent id '{id}' must be at most 64 characters"));
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "agent id '{id}' must start with a lowercase letter or digit"
        ));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
    {
        return Err(format!(
            "agent id '{id}' must match ^[a-z0-9][a-z0-9_-]{{0,63}}$ (lowercase letters, digits, hyphen, underscore)"
        ));
    }
    Ok(())
}

/// Validate an explicit `--color` flag value: strict `^#[0-9a-fA-F]{6}$`.
pub fn validate_agent_color(color: &str) -> Result<(), String> {
    let bytes = color.as_bytes();
    let ok = bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit);
    if ok {
        Ok(())
    } else {
        Err(format!(
            "color '{color}' must be a 6-digit hex value like #RRGGBB"
        ))
    }
}

/// Mint an agent id in the FE-compatible shape `agent_<unix_ms>_<6 hex>`.
/// Collision-safe for scripted loops (the uuid suffix disambiguates same-ms
/// mints); conforms to `validate_custom_agent_id`.
pub fn mint_agent_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("agent_{}_{}", ms, &uuid[..6])
}

/// Error message for an unknown selector id that lists the available ids.
pub fn unknown_agent_id_error(id: &str, agents: &[AgentConfig]) -> String {
    let available: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    let list = if available.is_empty() {
        "(none)".to_string()
    } else {
        available.join(", ")
    };
    format!("agent id '{id}' not found. Available ids: {list}")
}

/// #786 R8: PURE op application. Mutates `settings` in memory and runs the shared
/// validator/repair suite; NEVER saves to disk. Both callers own persistence.
pub fn apply_coding_agent_op(
    settings: &mut AppSettings,
    op: &CodingAgentOp,
) -> Result<CodingAgentOpOutcome, String> {
    let outcome = match op {
        CodingAgentOp::Add { agent } => {
            validate_custom_agent_id(&agent.id)?;
            if settings
                .agents
                .iter()
                .any(|a| a.id.eq_ignore_ascii_case(&agent.id))
            {
                return Err(format!("agent id '{}' already exists", agent.id));
            }
            if agent.label.trim().is_empty() {
                return Err("agent label must not be empty".to_string());
            }
            ensure_seed_consistent(&agent.config_seed)?;
            warn_on_duplicate_label(settings, agent.label.trim(), None);
            let mut new_agent = agent.clone();
            new_agent.label = new_agent.label.trim().to_string();
            settings.agents.push(new_agent.clone());
            CodingAgentOpOutcome {
                op: "add",
                agent: Some(new_agent),
                removed_id: None,
            }
        }
        CodingAgentOp::Update { id, patch } => {
            let idx = settings
                .agents
                .iter()
                .position(|a| a.id == *id)
                .ok_or_else(|| unknown_agent_id_error(id, &settings.agents))?;
            let mut updated = settings.agents[idx].clone();
            apply_patch(&mut updated, patch)?;
            warn_on_duplicate_label(settings, updated.label.trim(), Some(idx));
            settings.agents[idx] = updated.clone();
            CodingAgentOpOutcome {
                op: "update",
                agent: Some(updated),
                removed_id: None,
            }
        }
        CodingAgentOp::Remove { id } => {
            let idx = settings
                .agents
                .iter()
                .position(|a| a.id == *id)
                .ok_or_else(|| unknown_agent_id_error(id, &settings.agents))?;
            settings.agents.remove(idx);
            // Orphan `profilesByAgent[id]` / label entries are intentionally left
            // in place: `repair_coding_agent_profiles_config` never prunes by
            // agent id, and the GUI leaves them too. A same-id re-add resurrects
            // the old cells (documented, accepted).
            CodingAgentOpOutcome {
                op: "remove",
                agent: None,
                removed_id: Some(id.clone()),
            }
        }
    };

    // Authoritative shared suite: command bans, env rows, instructions filename,
    // config-seed dest, plus profile repair (A-cell bookkeeping for a new agent).
    // This is exactly what every existing writer enforces.
    validate_and_repair_settings(settings)?;
    Ok(outcome)
}

fn apply_patch(agent: &mut AgentConfig, patch: &AgentPatch) -> Result<(), String> {
    if let Some(label) = &patch.label {
        if label.trim().is_empty() {
            return Err("agent label must not be empty".to_string());
        }
        agent.label = label.trim().to_string();
    }
    if let Some(command) = &patch.command {
        agent.command = command.clone();
    }
    if let Some(color) = &patch.color {
        agent.color = color.clone();
    }
    if let Some(envs) = &patch.envs {
        agent.envs = envs.clone();
    }
    if let Some(isolated) = patch.isolated_home {
        agent.isolated_home = isolated;
    }
    if patch.clear_instructions_filename {
        agent.instructions_filename = None;
    } else if let Some(name) = &patch.instructions_filename {
        agent.instructions_filename = Some(name.clone());
    }
    if patch.clear_config_seed {
        agent.config_seed = None;
    } else if patch.config_seed_dest.is_some() || patch.config_seed_enabled.is_some() {
        let mut seed = agent.config_seed.clone().unwrap_or(ConfigSeedConfig {
            enabled: true,
            dest: String::new(),
        });
        if let Some(dest) = &patch.config_seed_dest {
            seed.dest = dest.clone();
        }
        if let Some(enabled) = patch.config_seed_enabled {
            seed.enabled = enabled;
        }
        ensure_seed_consistent(&Some(seed.clone()))?;
        agent.config_seed = Some(seed);
    }
    Ok(())
}

/// Reject an enabled seed with no destination (the GUI picker never produces
/// this; enabling requires a dest). An inactive seed (disabled, any dest) is fine.
fn ensure_seed_consistent(seed: &Option<ConfigSeedConfig>) -> Result<(), String> {
    if let Some(s) = seed {
        if s.enabled && s.dest.trim().is_empty() {
            return Err(
                "config seed is enabled but has no destination; pass --config-seed-dest"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn warn_on_duplicate_label(settings: &AppSettings, label: &str, skip_idx: Option<usize>) {
    let dup = settings
        .agents
        .iter()
        .enumerate()
        .any(|(i, a)| Some(i) != skip_idx && a.label.trim().eq_ignore_ascii_case(label));
    if dup {
        log::warn!(
            "[coding-agent] duplicate label '{label}'; launch-by-label resolution may be ambiguous"
        );
    }
}

// ---------------------------------------------------------------------------
// Queue transport helpers (daemon path). All writes are atomic temp+rename.
// ---------------------------------------------------------------------------

/// Atomically write a request into `dir` as `<requestId>.json`. Returns the
/// final path (used by the CLI to cancel on timeout).
pub fn write_coding_agent_request(dir: &Path, req: &CodingAgentRequest) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create coding-agent-requests dir: {e}"))?;
    let final_path = dir.join(format!("{}.json", req.request_id));
    let tmp_path = dir.join(format!("{}.json.tmp", req.request_id));
    let json = serde_json::to_string_pretty(req)
        .map_err(|e| format!("Failed to serialize request: {e}"))?;
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write request temp file: {e}"))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize request file: {e}")
    })?;
    Ok(final_path)
}

/// #786 R4: the result writer OWNS the results dir (`create_dir_all` first) so
/// the very first result on a fresh config dir cannot fail for a missing dir.
pub fn write_coding_agent_result(
    results_dir: &Path,
    res: &CodingAgentResult,
) -> Result<(), String> {
    std::fs::create_dir_all(results_dir)
        .map_err(|e| format!("Failed to create results dir: {e}"))?;
    let final_path = results_dir.join(format!("{}.json", res.request_id));
    let tmp_path = results_dir.join(format!("{}.json.tmp", res.request_id));
    let json = serde_json::to_string_pretty(res)
        .map_err(|e| format!("Failed to serialize result: {e}"))?;
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write result temp file: {e}"))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize result file: {e}")
    })?;
    Ok(())
}

/// Read a result file if present (used by the CLI while polling).
pub fn read_coding_agent_result(results_dir: &Path, request_id: &str) -> Option<CodingAgentResult> {
    let path = results_dir.join(format!("{request_id}.json"));
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// #786 R5: expiry with a saturating subtraction. A `created_at_ms` in the
/// future (wall clock stepped back between the CLI write and the poller read)
/// yields age 0, so a fresh request is never wrongly rejected as expired.
pub fn is_expired(created_at_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(created_at_ms) > REQUEST_EXPIRY_MS
}

fn processing_path_for(request_path: &Path) -> PathBuf {
    let mut s = request_path.as_os_str().to_os_string();
    s.push(".processing");
    PathBuf::from(s)
}

/// #786 R8: process ONE request file end to end. PURE of Tauri:
/// `settings` is the candidate to mutate and `save` persists it. Order (R2):
/// claim (atomic rename to `.processing`) -> parse -> expiry -> apply -> write
/// result -> delete `.processing` LAST. Path-parameterized for tempdir tests.
pub fn process_coding_agent_request(
    request_path: &Path,
    results_dir: &Path,
    now_ms: u64,
    settings: &mut AppSettings,
    save: &mut dyn FnMut(&AppSettings) -> Result<(), String>,
) -> RequestDisposition {
    // Claim: an atomic rename on the same volume. ENOENT means a timed-out CLI
    // already removed the request; never apply it.
    let processing_path = processing_path_for(request_path);
    if std::fs::rename(request_path, &processing_path).is_err() {
        return RequestDisposition::Skipped;
    }
    // The CLI polls `results/<stem>.json`, where `<stem>` is the request-file
    // stem (also the requestId it generated).
    let request_id = request_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let disposition = process_claimed(
        &processing_path,
        results_dir,
        &request_id,
        now_ms,
        settings,
        save,
    );
    // Delete the claim file LAST, after the result is durably written.
    let _ = std::fs::remove_file(&processing_path);
    disposition
}

fn process_claimed(
    processing_path: &Path,
    results_dir: &Path,
    request_id: &str,
    now_ms: u64,
    settings: &mut AppSettings,
    save: &mut dyn FnMut(&AppSettings) -> Result<(), String>,
) -> RequestDisposition {
    let content = match std::fs::read_to_string(processing_path) {
        Ok(c) => c,
        Err(e) => {
            write_reject(
                results_dir,
                request_id,
                format!("failed to read request: {e}"),
            );
            return RequestDisposition::Rejected;
        }
    };
    let request: CodingAgentRequest = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            write_reject(results_dir, request_id, format!("malformed request: {e}"));
            return RequestDisposition::Rejected;
        }
    };
    if is_expired(request.created_at_ms, now_ms) {
        write_reject(results_dir, request_id, "request expired".to_string());
        return RequestDisposition::Rejected;
    }
    match apply_coding_agent_op(settings, &request.op) {
        Ok(outcome) => {
            if let Err(e) = save(settings) {
                write_reject(
                    results_dir,
                    request_id,
                    format!("failed to persist settings: {e}"),
                );
                return RequestDisposition::Rejected;
            }
            let agent_id = outcome
                .agent
                .as_ref()
                .map(|a| a.id.clone())
                .or_else(|| outcome.removed_id.clone());
            let _ = write_coding_agent_result(
                results_dir,
                &CodingAgentResult {
                    request_id: request_id.to_string(),
                    ok: true,
                    op: Some(outcome.op.to_string()),
                    error: None,
                    agent: outcome.agent,
                    removed_id: outcome.removed_id,
                },
            );
            RequestDisposition::Applied {
                op: outcome.op,
                agent_id,
            }
        }
        Err(e) => {
            write_reject(results_dir, request_id, e);
            RequestDisposition::Rejected
        }
    }
}

fn write_reject(results_dir: &Path, request_id: &str, error: String) {
    let _ = write_coding_agent_result(
        results_dir,
        &CodingAgentResult {
            request_id: request_id.to_string(),
            ok: false,
            op: None,
            error: Some(error),
            agent: None,
            removed_id: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::CodingAgentEnvSource;

    fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            label: label.to_string(),
            command: command.to_string(),
            color: "#6366f1".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            backend: Default::default(),
        }
    }

    fn add(agent: AgentConfig) -> CodingAgentOp {
        CodingAgentOp::Add { agent }
    }

    fn noop_save() -> impl FnMut(&AppSettings) -> Result<(), String> {
        |_s: &AppSettings| Ok(())
    }

    // ---- id / color validators ---------------------------------------------

    #[test]
    fn minted_id_conforms_and_is_lowercase() {
        let id = mint_agent_id();
        assert!(validate_custom_agent_id(&id).is_ok(), "minted id: {id}");
        assert_eq!(id, id.to_lowercase());
    }

    #[test]
    fn custom_id_rules() {
        assert!(validate_custom_agent_id("claude-1").is_ok());
        assert!(validate_custom_agent_id("agent_123_ab12cd").is_ok());
        assert!(validate_custom_agent_id("").is_err());
        assert!(
            validate_custom_agent_id("Claude").is_err(),
            "uppercase rejected"
        );
        assert!(
            validate_custom_agent_id("-foo").is_err(),
            "leading hyphen rejected"
        );
        assert!(validate_custom_agent_id("has space").is_err());
        assert!(
            validate_custom_agent_id(&"a".repeat(65)).is_err(),
            "over 64 rejected"
        );
        assert!(
            validate_custom_agent_id(&"a".repeat(64)).is_ok(),
            "64 allowed"
        );
    }

    #[test]
    fn color_rules() {
        assert!(validate_agent_color("#6366f1").is_ok());
        assert!(validate_agent_color("#FFFFFF").is_ok());
        assert!(validate_agent_color("#fff").is_err());
        assert!(validate_agent_color("#6366f1ff").is_err());
        assert!(validate_agent_color("6366f1").is_err());
        assert!(validate_agent_color("red").is_err());
    }

    // ---- add ---------------------------------------------------------------

    #[test]
    fn add_appends_and_repairs_a_cell() {
        let mut s = AppSettings::default();
        let out = apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        assert_eq!(out.op, "add");
        assert_eq!(s.agents.len(), 1);
        // repair ensures an 'A' profile cell exists for the new agent.
        assert!(s
            .coding_agent_profiles
            .profiles_by_agent
            .get("claude")
            .map(|cells| cells.contains_key("A"))
            .unwrap_or(false));
    }

    #[test]
    fn add_rejects_duplicate_id() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        let err =
            apply_coding_agent_op(&mut s, &add(agent("claude", "Other", "codex"))).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn add_rejects_case_insensitive_dup_against_legacy_mixedcase_id() {
        let mut s = AppSettings::default();
        // A legacy agent with a mixed-case id predates the new lowercase-only
        // rule; insert it directly to bypass validate_custom_agent_id. Adding a
        // valid lowercase id that collides case-insensitively must be rejected
        // (find_launch_agent resolves ids case-insensitively).
        s.agents.push(agent("Claude", "Legacy", "claude"));
        let err = apply_coding_agent_op(&mut s, &add(agent("claude", "New", "codex"))).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn add_rejects_bad_id_and_empty_label() {
        let mut s = AppSettings::default();
        assert!(apply_coding_agent_op(&mut s, &add(agent("Bad Id", "L", "claude"))).is_err());
        assert!(apply_coding_agent_op(&mut s, &add(agent("ok", "  ", "claude"))).is_err());
    }

    #[test]
    fn add_rejects_banned_continue_and_dup_env() {
        let mut s = AppSettings::default();
        let err =
            apply_coding_agent_op(&mut s, &add(agent("c", "C", "claude --continue"))).unwrap_err();
        assert!(err.contains("continue"), "{err}");

        let mut dup = agent("d", "D", "claude");
        dup.envs = vec![
            CodingAgentEnv {
                key: "FOO".into(),
                value: "1".into(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "FOO".into(),
                value: "2".into(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
        ];
        let mut s2 = AppSettings::default();
        assert!(apply_coding_agent_op(&mut s2, &add(dup)).is_err());
    }

    #[test]
    fn add_rejects_enabled_seed_without_dest() {
        let mut s = AppSettings::default();
        let mut a = agent("s", "S", "claude");
        a.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: String::new(),
        });
        assert!(apply_coding_agent_op(&mut s, &add(a)).is_err());
    }

    #[test]
    fn add_accepts_catalog_seeded_non_hex_color() {
        // §14.4 R6 carve-out: the op layer NEVER validates color (only the
        // explicit `--color` CLI flag does, in cmd_add). A catalog-seeded agent
        // whose color is a shorthand like "#fff" must be accepted as-is so that
        // `add --from-catalog <key>` never rejects a trusted catalog color.
        // Guards against a future regression that adds a color check here.
        let mut s = AppSettings::default();
        let mut a = agent("cat", "Catalog Agent", "claude");
        a.color = "#fff".to_string();
        let out = apply_coding_agent_op(&mut s, &add(a)).unwrap();
        assert_eq!(out.op, "add");
        assert_eq!(s.agents[0].color, "#fff", "catalog color persisted as-is");
    }

    // ---- update ------------------------------------------------------------

    #[test]
    fn update_partial_keeps_other_fields() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        let patch = AgentPatch {
            label: Some("Renamed".into()),
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        )
        .unwrap();
        assert_eq!(s.agents[0].label, "Renamed");
        assert_eq!(s.agents[0].command, "claude");
    }

    #[test]
    fn update_env_replace_all_and_clear() {
        let mut s = AppSettings::default();
        let mut a = agent("claude", "Claude", "claude");
        a.envs = vec![CodingAgentEnv {
            key: "OLD".into(),
            value: "1".into(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        apply_coding_agent_op(&mut s, &add(a)).unwrap();

        let patch = AgentPatch {
            envs: Some(vec![CodingAgentEnv {
                key: "NEW".into(),
                value: "2".into(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            }]),
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        )
        .unwrap();
        assert_eq!(s.agents[0].envs.len(), 1);
        assert_eq!(s.agents[0].envs[0].key, "NEW");

        let clear = AgentPatch {
            envs: Some(Vec::new()),
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch: clear,
            },
        )
        .unwrap();
        assert!(s.agents[0].envs.is_empty());
    }

    #[test]
    fn update_unknown_id_lists_available() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        let err = apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "nope".into(),
                patch: AgentPatch::default(),
            },
        )
        .unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("claude"), "{err}");
    }

    #[test]
    fn update_clear_instructions_and_seed() {
        let mut s = AppSettings::default();
        let mut a = agent("claude", "Claude", "claude");
        a.instructions_filename = Some("CLAUDE.md".into());
        a.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: ".claude".into(),
        });
        apply_coding_agent_op(&mut s, &add(a)).unwrap();

        let patch = AgentPatch {
            clear_instructions_filename: true,
            clear_config_seed: true,
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        )
        .unwrap();
        assert!(s.agents[0].instructions_filename.is_none());
        assert!(s.agents[0].config_seed.is_none());
    }

    // ---- remove (orphan-preserve) ------------------------------------------

    #[test]
    fn remove_preserves_profile_orphans() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        // repair created profiles_by_agent["claude"]; removing the agent must
        // leave that orphan in place (matches repair/GUI semantics).
        assert!(s
            .coding_agent_profiles
            .profiles_by_agent
            .contains_key("claude"));
        let out = apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Remove {
                id: "claude".into(),
            },
        )
        .unwrap();
        assert_eq!(out.removed_id.as_deref(), Some("claude"));
        assert!(s.agents.is_empty());
        assert!(
            s.coding_agent_profiles
                .profiles_by_agent
                .contains_key("claude"),
            "profile orphan must survive remove"
        );
    }

    #[test]
    fn remove_unknown_id_errors() {
        let mut s = AppSettings::default();
        assert!(
            apply_coding_agent_op(&mut s, &CodingAgentOp::Remove { id: "ghost".into() }).is_err()
        );
    }

    // ---- purity ------------------------------------------------------------

    #[test]
    fn apply_is_pure_no_disk() {
        // A bare AppSettings with no config dir override: apply must not touch
        // disk (it takes &mut and validates only). If it saved, tests would need
        // a config dir; they do not.
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        assert_eq!(s.agents.len(), 1);
    }

    // ---- queue helpers -----------------------------------------------------

    #[test]
    fn is_expired_backward_clock_and_old() {
        // future createdAt (backward clock) -> not expired
        assert!(!is_expired(1_000_000, 900_000));
        // fresh -> not expired
        assert!(!is_expired(1_000_000, 1_000_100));
        // genuinely old -> expired
        assert!(is_expired(0, REQUEST_EXPIRY_MS + 1));
    }

    #[test]
    fn request_result_roundtrip_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let req = CodingAgentRequest {
            request_id: "req1".into(),
            created_at_ms: 123,
            op: add(agent("claude", "Claude", "claude")),
        };
        let path = write_coding_agent_request(&requests, &req).unwrap();
        assert!(path.exists());
        // no leftover temp file
        assert!(std::fs::read_dir(&requests).unwrap().all(|e| {
            let p = e.unwrap().path();
            p.extension().and_then(|s| s.to_str()) == Some("json")
        }));

        let results = requests.join(RESULTS_SUBDIR);
        write_coding_agent_result(
            &results,
            &CodingAgentResult {
                request_id: "req1".into(),
                ok: true,
                op: Some("add".into()),
                error: None,
                agent: None,
                removed_id: None,
            },
        )
        .unwrap();
        let read = read_coding_agent_result(&results, "req1").unwrap();
        assert!(read.ok);
        assert_eq!(read.op.as_deref(), Some("add"), "op round-trips");
    }

    #[test]
    fn first_request_autocreates_results_dir() {
        // R4: brand-new tempdir, results/ does not exist; write_coding_agent_result
        // must create it.
        let dir = tempfile::tempdir().unwrap();
        let results = dir
            .path()
            .join(CODING_AGENT_REQUESTS_DIR)
            .join(RESULTS_SUBDIR);
        assert!(!results.exists());
        write_coding_agent_result(
            &results,
            &CodingAgentResult {
                request_id: "x".into(),
                ok: true,
                op: None,
                error: None,
                agent: None,
                removed_id: None,
            },
        )
        .unwrap();
        assert!(results.join("x.json").exists());
    }

    // ---- per-request handler ----------------------------------------------

    fn write_request_file(
        requests: &Path,
        id: &str,
        created_at_ms: u64,
        op: CodingAgentOp,
    ) -> PathBuf {
        std::fs::create_dir_all(requests).unwrap();
        let req = CodingAgentRequest {
            request_id: id.to_string(),
            created_at_ms,
            op,
        };
        let path = requests.join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&req).unwrap()).unwrap();
        path
    }

    #[test]
    fn handler_applies_and_writes_result_and_deletes_processing() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let path = write_request_file(
            &requests,
            "r1",
            100,
            add(agent("claude", "Claude", "claude")),
        );

        let mut settings = AppSettings::default();
        let mut saved = false;
        let disp = {
            let mut save = |_s: &AppSettings| {
                saved = true;
                Ok(())
            };
            process_coding_agent_request(&path, &results, 200, &mut settings, &mut save)
        };
        assert!(matches!(
            disp,
            RequestDisposition::Applied { op: "add", .. }
        ));
        assert!(saved, "save callback invoked");
        assert_eq!(settings.agents.len(), 1);
        // result written, request + processing gone
        assert!(read_coding_agent_result(&results, "r1").unwrap().ok);
        assert!(!path.exists());
        assert!(!processing_path_for(&path).exists());
    }

    #[test]
    fn handler_skips_when_request_already_removed() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        std::fs::create_dir_all(&requests).unwrap();
        let missing = requests.join("gone.json");
        let mut settings = AppSettings::default();
        let disp =
            process_coding_agent_request(&missing, &results, 200, &mut settings, &mut noop_save());
        assert_eq!(disp, RequestDisposition::Skipped);
        assert!(settings.agents.is_empty());
    }

    #[test]
    fn handler_rejects_expired_without_applying() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let path = write_request_file(
            &requests,
            "old",
            0,
            add(agent("claude", "Claude", "claude")),
        );
        let mut settings = AppSettings::default();
        let disp = process_coding_agent_request(
            &path,
            &results,
            REQUEST_EXPIRY_MS + 10,
            &mut settings,
            &mut noop_save(),
        );
        assert_eq!(disp, RequestDisposition::Rejected);
        assert!(settings.agents.is_empty());
        let res = read_coding_agent_result(&results, "old").unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("expired"));
    }

    #[test]
    fn handler_rejects_validation_error() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let path = write_request_file(&requests, "bad", 100, add(agent("BadId", "L", "claude")));
        let mut settings = AppSettings::default();
        let disp =
            process_coding_agent_request(&path, &results, 150, &mut settings, &mut noop_save());
        assert_eq!(disp, RequestDisposition::Rejected);
        assert!(settings.agents.is_empty());
        assert!(!read_coding_agent_result(&results, "bad").unwrap().ok);
    }

    // ---- T10: daemon-path launch consumption (acceptance criterion 2) ------

    #[test]
    fn daemon_applied_agent_is_launch_resolvable() {
        // Apply an Add through the extracted per-request handler (the daemon-path
        // seam), then confirm the new agent is resolvable to a subsequent
        // `--launch` by id AND by label, and that a spawn command builds for it.
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let path = write_request_file(
            &requests,
            "r",
            100,
            add(agent("claude", "Claude Code", "claude")),
        );

        let mut settings = AppSettings::default();
        let disp =
            process_coding_agent_request(&path, &results, 200, &mut settings, &mut noop_save());
        assert!(matches!(disp, RequestDisposition::Applied { .. }));

        // find_launch_agent (create_agent.rs) resolves by exact id and by label.
        assert!(crate::cli::create_agent::find_launch_agent(&settings, "claude").is_some());
        assert!(crate::cli::create_agent::find_launch_agent(&settings, "Claude Code").is_some());

        // A spawn command builds against the same state (session.rs). Some(_)
        // means the agent was found and the command composed.
        let cwd = dir.path().to_string_lossy();
        let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings, "claude", &cwd, None,
        );
        assert!(spawn.is_ok(), "spawn build failed: {:?}", spawn.err());
        assert!(
            spawn.unwrap().is_some(),
            "expected a spawn command for the applied agent"
        );
    }
}
